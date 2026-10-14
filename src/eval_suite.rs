//! Eval suites. A task is a directory: `task.toml` (prompt, workspace, checks),
//! an optional `fixture/` copied into a fresh workspace for every attempt, its
//! graders, and an optional `reference/` solution that `validate` uses to prove
//! the graders can tell a solved task from an unsolved one. `suite` runs tasks
//! × models × repeats; every attempt is a child `omperor eval run`, so attempts
//! are isolated from one another, can run in parallel, and stop cleanly.
//! See docs/EVALS.md.

use crate::{
    eval::{INTERRUPTED, InterruptHandlers},
    eval_report::{self, AttemptRecord, Outcome},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    net::{Ipv4Addr, TcpListener, TcpStream},
    os::unix::{fs::DirBuilderExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex, atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SUITE_HELP: &str = "\
Usage: omperor eval suite [OPTIONS] [TASK_OR_DIRECTORY...] --output DIR

Run every task (a directory with task.toml, or a directory of them; default
evals/tasks) against each model, REPEAT times, in fresh workspaces.

  --output DIR        New directory for all artifacts (required)
  --model MODEL       Model to evaluate; repeat for a matrix (default: each task's model)
  --account ID        Account to pin for its provider; repeat for several providers
                      (default: the provider's default account in Omperor's settings)
  --thinking LEVEL    Reasoning level for every attempt (default: the task's, else the backend's)
  --repeat N          Attempts per task and model (default 1)
  --jobs N            Attempts run at once (default 1)
  --task PATTERN      Only tasks whose id contains PATTERN; repeatable
  --tag TAG           Only tasks with this tag; repeatable
  --exclude-tag TAG   Skip tasks with this tag, e.g. external; repeatable
  --resume            Continue an interrupted DIR, rerunning unfinished attempts
  --prune-workspaces  Delete each workspace after recording its diff
  --dry-run           Print the planned attempts and exit
  --json              Print the final summary as JSON instead of Markdown

Progress goes to stderr; the summary to stdout and DIR/summary.{md,json}.
Exit: 0 all graded attempts passed, 1 a failure or infrastructure problem,
2 invalid invocation, 3 nothing graded.
";

const MAX_PATCH_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn dispatch(args: &[OsString]) -> Option<i32> {
    let command = args.get(1)?.to_str()?;
    if !matches!(command, "suite" | "report" | "tasks" | "validate" | "doctor" | "login") {
        return None;
    }
    let Some(rest) = args[2..]
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    else {
        eprintln!("omperor eval {command}: arguments must be UTF-8");
        return Some(2);
    };
    let result = match command {
        "suite" => suite(&rest),
        "report" => report(&rest),
        "tasks" => list_tasks(&rest),
        "validate" => validate(&rest),
        "doctor" => doctor(&rest),
        _ => login(&rest),
    };
    Some(result.unwrap_or_else(|error| {
        eprintln!("omperor eval {command}: {error}");
        2
    }))
}

// ---------------------------------------------------------------------------
// Arguments

#[derive(Default)]
struct Arguments {
    positional: Vec<String>,
    values: Vec<(String, String)>,
    flags: HashSet<String>,
}

impl Arguments {
    fn parse(args: &[String], flags: &[&str], valued: &[&str]) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg == "--" {
                parsed.positional.extend(iter.by_ref().cloned());
                break;
            }
            let Some(name) = arg.strip_prefix("--") else {
                parsed.positional.push(arg.clone());
                continue;
            };
            let (name, inline) = match name.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (name, None),
            };
            if flags.contains(&name) && inline.is_none() {
                parsed.flags.insert(name.into());
            } else if valued.contains(&name) {
                let value = match inline {
                    Some(value) => value,
                    None => iter
                        .next()
                        .cloned()
                        .ok_or_else(|| format!("--{name} needs a value"))?,
                };
                parsed.values.push((name.into(), value));
            } else {
                return Err(format!("unknown option --{name}"));
            }
        }
        Ok(parsed)
    }
    fn all(&self, name: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }
    fn one(&self, name: &str) -> Result<Option<String>, String> {
        let mut values = self.all(name);
        if values.len() > 1 {
            return Err(format!("--{name} given more than once"));
        }
        Ok(values.pop())
    }
    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
    fn number(&self, name: &str, default: u64, range: std::ops::RangeInclusive<u64>) -> Result<u64, String> {
        match self.one(name)? {
            None => Ok(default),
            Some(text) => text
                .parse::<u64>()
                .ok()
                .filter(|value| range.contains(value))
                .ok_or_else(|| format!("--{name} must be a number from {} to {}", range.start(), range.end())),
        }
    }
}

// ---------------------------------------------------------------------------
// Tasks

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskFile {
    id: Option<String>,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    prompt: String,
    timeout_secs: Option<u64>,
    model: Option<String>,
    thinking_level: Option<String>,
    #[serde(default)]
    computer_use: bool,
    #[serde(default)]
    workspace: WorkspaceFile,
    #[serde(default)]
    services: Vec<ServiceSpec>,
    #[serde(default)]
    checks: Vec<CheckSpec>,
    /// Variables for setup, services, the agent's tools, and the graders.
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceFile {
    fixture: Option<PathBuf>,
    git: Option<GitSource>,
    git_init: Option<bool>,
    #[serde(default)]
    setup: Vec<Vec<String>>,
    setup_timeout_secs: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GitSource {
    /// A URL, or a path relative to the task directory.
    repo: String,
    rev: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceSpec {
    name: String,
    argv: Vec<String>,
    ready_timeout_secs: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum CheckSpec {
    AnswerContains {
        text: String,
    },
    Command {
        argv: Vec<String>,
        timeout_secs: Option<u64>,
    },
}

#[derive(Clone, Debug)]
enum Source {
    Empty,
    Fixture(PathBuf),
    Git { repo: String, rev: String },
}

#[derive(Debug)]
pub(crate) struct Task {
    id: String,
    dir: PathBuf,
    description: Option<String>,
    tags: Vec<String>,
    prompt: String,
    timeout_secs: u64,
    model: Option<String>,
    thinking_level: Option<String>,
    computer_use: bool,
    source: Source,
    git_init: bool,
    setup: Vec<Vec<String>>,
    setup_timeout_secs: u64,
    services: Vec<ServiceSpec>,
    checks: Vec<CheckSpec>,
    env: BTreeMap<String, String>,
    reference_dir: Option<PathBuf>,
    reference_patch: Option<PathBuf>,
    /// Run in the workspace to solve it, for solutions a file overlay cannot
    /// express (finishing a merge, making commits).
    reference_script: Option<PathBuf>,
    reference_answer: Option<PathBuf>,
}

impl Task {
    fn has_reference(&self) -> bool {
        self.reference_dir.is_some()
            || self.reference_patch.is_some()
            || self.reference_script.is_some()
            || self.reference_answer.is_some()
    }
}

fn identifier(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('.')
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn timeout(seconds: Option<u64>, default: u64, what: &str) -> Result<u64, String> {
    let seconds = seconds.unwrap_or(default);
    if (1..=3600).contains(&seconds) {
        Ok(seconds)
    } else {
        Err(format!("{what} must be between 1 and 3600 seconds"))
    }
}

fn load_task(dir: &Path) -> Result<Task, String> {
    let dir = fs::canonicalize(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let path = dir.join("task.toml");
    let text = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let file: TaskFile = toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let invalid = |detail: String| format!("{}: {detail}", path.display());
    let id = match file.id {
        Some(id) => id,
        None => dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    if !identifier(&id) {
        return Err(invalid(format!("task id {id:?} must use letters, digits, '.', '_', '-'")));
    }
    if file.prompt.trim().is_empty() {
        return Err(invalid("prompt must be nonempty".into()));
    }
    for (name, value) in [("model", &file.model), ("thinking_level", &file.thinking_level)] {
        if value.as_ref().is_some_and(|value| value.trim().is_empty()) {
            return Err(invalid(format!("{name} must be nonempty when given")));
        }
    }
    let workspace = file.workspace;
    let source = match (workspace.git, workspace.fixture) {
        (Some(_), Some(_)) => return Err(invalid("workspace takes a fixture or git, not both".into())),
        (Some(git), None) => {
            if git.repo.trim().is_empty() || git.rev.trim().is_empty() {
                return Err(invalid("workspace.git needs repo and rev".into()));
            }
            let repo = if git.repo.contains("://") || git.repo.contains('@') {
                git.repo
            } else {
                let local = dir.join(&git.repo);
                fs::canonicalize(&local)
                    .map_err(|error| invalid(format!("workspace.git.repo {}: {error}", local.display())))?
                    .to_string_lossy()
                    .into_owned()
            };
            Source::Git { repo, rev: git.rev }
        }
        (None, Some(fixture)) => {
            let fixture = dir.join(fixture);
            if !fixture.is_dir() {
                return Err(invalid(format!("fixture {} is not a directory", fixture.display())));
            }
            Source::Fixture(fixture)
        }
        (None, None) if dir.join("fixture").is_dir() => Source::Fixture(dir.join("fixture")),
        (None, None) => Source::Empty,
    };
    if workspace.setup.iter().any(|argv| argv.first().is_none_or(String::is_empty)) {
        return Err(invalid("each setup command needs an executable".into()));
    }
    let mut names = HashSet::new();
    for service in &file.services {
        if !identifier(&service.name) || !names.insert(service.name.clone()) {
            return Err(invalid(format!("service name {:?} is invalid or repeated", service.name)));
        }
        if service.argv.first().is_none_or(String::is_empty) {
            return Err(invalid(format!("service {} needs an executable", service.name)));
        }
        timeout(service.ready_timeout_secs, 10, "ready_timeout_secs").map_err(invalid)?;
    }
    for check in &file.checks {
        match check {
            CheckSpec::AnswerContains { text } if text.is_empty() => {
                return Err(invalid("answer_contains text must be nonempty".into()));
            }
            CheckSpec::Command { argv, timeout_secs } => {
                if argv.first().is_none_or(String::is_empty) {
                    return Err(invalid("command checks need an executable".into()));
                }
                timeout(*timeout_secs, 300, "check timeout_secs").map_err(invalid)?;
            }
            CheckSpec::AnswerContains { .. } => {}
        }
    }
    if file
        .env
        .keys()
        .any(|key| key.is_empty() || key.contains('=') || key.contains('\0'))
    {
        return Err(invalid("env names must be nonempty and contain no '=' or NUL".into()));
    }
    let existing = |name: &str, directory: bool| {
        let path = dir.join(name);
        (if directory { path.is_dir() } else { path.is_file() }).then_some(path)
    };
    Ok(Task {
        timeout_secs: timeout(file.timeout_secs, 600, "timeout_secs").map_err(invalid)?,
        setup_timeout_secs: timeout(workspace.setup_timeout_secs, 600, "setup_timeout_secs").map_err(invalid)?,
        git_init: workspace.git_init.unwrap_or(!matches!(source, Source::Git { .. })),
        reference_dir: existing("reference", true),
        reference_patch: existing("reference.patch", false),
        reference_script: existing("reference.sh", false),
        reference_answer: existing("reference-answer.txt", false),
        id,
        description: file.description,
        tags: file.tags,
        prompt: file.prompt,
        model: file.model,
        thinking_level: file.thinking_level,
        computer_use: file.computer_use,
        source,
        setup: workspace.setup,
        services: file.services,
        checks: file.checks,
        env: file.env,
        dir,
    })
}

fn default_sources() -> Result<Vec<PathBuf>, String> {
    let relative = PathBuf::from("evals/tasks");
    if relative.is_dir() {
        return Ok(vec![relative]);
    }
    let source = Path::new(option_env!("OMPEROR_SOURCE_DIR").unwrap_or(env!("CARGO_MANIFEST_DIR")));
    let tasks = source.join("evals/tasks");
    if tasks.is_dir() {
        return Ok(vec![tasks]);
    }
    Err("no task directories given and no evals/tasks here".into())
}

fn discover(sources: &[String]) -> Result<Vec<Task>, String> {
    let sources = if sources.is_empty() {
        default_sources()?
    } else {
        sources.iter().map(PathBuf::from).collect()
    };
    let mut tasks = Vec::new();
    for source in sources {
        if source.join("task.toml").is_file() {
            tasks.push(load_task(&source)?);
            continue;
        }
        let entries = fs::read_dir(&source).map_err(|error| format!("{}: {error}", source.display()))?;
        let mut directories: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join("task.toml").is_file())
            .collect();
        if directories.is_empty() {
            return Err(format!("{} has no task.toml and no task directories", source.display()));
        }
        directories.sort();
        for directory in directories {
            tasks.push(load_task(&directory)?);
        }
    }
    let mut seen = HashSet::new();
    for task in &tasks {
        if !seen.insert(task.id.clone()) {
            return Err(format!("task id {} appears twice", task.id));
        }
    }
    Ok(tasks)
}

fn select(tasks: Vec<Task>, args: &Arguments) -> Vec<Task> {
    let (patterns, tags, excluded) = (args.all("task"), args.all("tag"), args.all("exclude-tag"));
    tasks
        .into_iter()
        .filter(|task| patterns.is_empty() || patterns.iter().any(|pattern| task.id.contains(pattern.as_str())))
        .filter(|task| tags.is_empty() || task.tags.iter().any(|tag| tags.contains(tag)))
        .filter(|task| !task.tags.iter().any(|tag| excluded.contains(tag)))
        .collect()
}

// ---------------------------------------------------------------------------
// Accounts and the data store

fn state_root() -> Result<PathBuf, String> {
    crate::app_state::eval_root().map_err(|error| error.to_string())
}

/// Desktop account metadata, read without creating a database that is absent.
fn desktop_accounts(root: &Path) -> Vec<crate::preferences::StoredAccount> {
    let path = root.join("omperor.sqlite3");
    if !path.is_file() {
        return vec![];
    }
    crate::preferences::load_accounts_from(&path).unwrap_or_default()
}

fn provider_of(model: &str) -> &str {
    model.split_once('/').map_or(model, |(provider, _)| provider)
}

/// The account each model's attempts pin: an explicit `--account` for that
/// provider, else the provider's default, else its only account, else none
/// (OMP's own selection, reported in each result).
fn resolve_accounts(
    models: &[String],
    explicit: &[String],
    known: &[crate::preferences::StoredAccount],
) -> Result<HashMap<String, Option<String>>, String> {
    for account in explicit {
        let provider = account.split_once(':').map_or("", |(provider, _)| provider);
        if !models.iter().any(|model| provider_of(model) == provider) {
            return Err(format!("--account {account} matches none of the models' providers"));
        }
    }
    let mut chosen = HashMap::new();
    for model in models {
        let provider = provider_of(model);
        let explicit: Vec<_> = explicit
            .iter()
            .filter(|account| account.split_once(':').is_some_and(|(p, _)| p == provider))
            .collect();
        if explicit.len() > 1 {
            return Err(format!("more than one --account for provider {provider}"));
        }
        let candidates: Vec<_> = known.iter().filter(|account| account.provider == provider).collect();
        let account = explicit.first().map(|account| (*account).clone()).or_else(|| {
            candidates
                .iter()
                .find(|account| account.is_default)
                .or_else(|| (candidates.len() == 1).then(|| &candidates[0]))
                .map(|account| account.account_id.clone())
        });
        chosen.insert(model.clone(), account);
    }
    Ok(chosen)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LiveAccount {
    id: String,
    provider: String,
    state: String,
    principal: Option<String>,
}

fn parse_live_accounts(text: &str) -> Vec<LiveAccount> {
    text.lines()
        .filter_map(|line| {
            let fields: HashMap<_, _> = line
                .split_whitespace()
                .filter_map(|field| field.split_once('='))
                .collect();
            Some(LiveAccount {
                id: (*fields.get("selector")?).into(),
                provider: (*fields.get("provider")?).into(),
                state: fields.get("state").copied().unwrap_or("unknown").into(),
                principal: fields.get("principal").map(|p| (*p).to_owned()),
            })
        })
        .collect()
}

fn helper_command(helper: &Path, data_dir: &Path) -> Command {
    let mut command = Command::new(helper);
    command.envs(crate::omp_runtime::data_store_environment(data_dir));
    command
}

/// OMP's own non-secret account list for a store: the source of truth for
/// which credentials exist and whether they need a refresh.
fn live_accounts(helper: &Path, data_dir: &Path) -> Result<Vec<LiveAccount>, String> {
    let mut command = helper_command(helper, data_dir);
    command
        .arg("auth")
        .arg("--data-dir")
        .arg(data_dir)
        .arg("list")
        .stdin(Stdio::null());
    let (status, stdout, stderr) = output_within(command, Duration::from_secs(30))?;
    if !status.success() {
        return Err(format!("omp auth list failed ({status}): {}", stderr.trim()));
    }
    Ok(parse_live_accounts(&stdout))
}

fn output_within(mut command: Command, limit: Duration) -> Result<(ExitStatus, String, String), String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| error.to_string())?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let readers = (
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            text
        }),
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        }),
    );
    let status = wait_within(&mut child, limit).map_err(|error| error.to_string())?;
    let stdout = readers.0.join().unwrap_or_default();
    let stderr = readers.1.join().unwrap_or_default();
    status
        .map(|status| (status, stdout, stderr))
        .ok_or_else(|| format!("timed out after {} s", limit.as_secs()))
}

/// Waits for a child that owns its process group, killing the group at the
/// deadline or on interruption. `None` means it did not finish in time.
fn wait_within(child: &mut Child, limit: Duration) -> io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            kill_group(child);
            return Ok(Some(status));
        }
        if started.elapsed() >= limit || INTERRUPTED.load(Ordering::Relaxed) != 0 {
            kill_group(child);
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn kill_group(child: &Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: kill takes only integers; the group is the child's own,
        // created with process_group(0), never this process's group.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

// ---------------------------------------------------------------------------
// Workspaces

/// `git`, found on PATH once. A bare program name with a changed environment
/// makes std fork and exec instead of `posix_spawn`, and a forked child
/// briefly holds every descriptor this process has open, including another
/// thread's journal lease; an absolute path keeps the spawn atomic.
fn git_executable() -> &'static Path {
    static GIT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    GIT.get_or_init(|| crate::eval::resolve_program("git", &BTreeMap::new()))
}

fn git_command(directory: Option<&Path>) -> Command {
    let mut command = Command::new(git_executable());
    if let Some(directory) = directory {
        command.arg("-C").arg(directory);
    }
    // Fixtures must not run the operator's hooks, need a signing key, or prompt.
    command
        .args(["-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "omperor-eval")
        .env("GIT_AUTHOR_EMAIL", "eval@omperor.invalid")
        .env("GIT_COMMITTER_NAME", "omperor-eval")
        .env("GIT_COMMITTER_EMAIL", "eval@omperor.invalid")
        .stdin(Stdio::null());
    command
}

fn git(directory: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut command = git_command(directory);
    command.args(args);
    let (status, stdout, stderr) = output_within(command, Duration::from_secs(600))
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if status.success() {
        Ok(stdout)
    } else {
        Err(format!("git {} failed: {}", args.join(" "), stderr.trim()))
    }
}

fn fnv(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn cache_root() -> PathBuf {
    std::env::var_os("OMPEROR_EVAL_CACHE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| state_root().ok().map(|root| root.join("eval-cache")))
        .unwrap_or_else(|| std::env::temp_dir().join("omperor-eval-cache"))
}

/// A bare clone per repository, fetched once and shared by every attempt's
/// `--shared` clone, so a large repository is downloaded once per machine.
fn ensure_git_cache(repo: &str, rev: &str) -> Result<PathBuf, String> {
    let directory = cache_root().join("git");
    fs::create_dir_all(&directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let cache = directory.join(fnv(repo));
    if !cache.join("HEAD").is_file() {
        let staging = directory.join(format!("{}.partial-{}", fnv(repo), std::process::id()));
        let _ = fs::remove_dir_all(&staging);
        eprintln!("Cloning {repo} into the eval cache (once)...");
        git(None, &["clone", "--bare", "--quiet", repo, &staging.to_string_lossy()])?;
        if fs::rename(&staging, &cache).is_err() {
            // Another suite finished the same clone first.
            let _ = fs::remove_dir_all(&staging);
        }
    }
    let present = |cache: &Path| git(Some(cache), &["cat-file", "-e", &format!("{rev}^{{commit}}")]).is_ok();
    if !present(&cache) {
        let _ = git(Some(&cache), &["fetch", "--quiet", "origin", rev]);
        if !present(&cache) {
            git(Some(&cache), &["fetch", "--quiet", "--tags", "origin", "+refs/heads/*:refs/heads/*"])?;
        }
        if !present(&cache) {
            return Err(format!("{repo} has no commit {rev}"));
        }
    }
    Ok(cache)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let (source, target) = (entry.path(), to.join(&name));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
        } else if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn commit_baseline(workspace: &Path) -> Result<(), String> {
    git(Some(workspace), &["init", "--quiet"])?;
    git(Some(workspace), &["add", "-A", "."])?;
    git(Some(workspace), &["commit", "--quiet", "--allow-empty", "--no-verify", "-m", "Eval fixture baseline"])?;
    Ok(())
}

/// Builds the attempt's workspace from the task's source and runs its setup.
/// Returns the workspace and, when it is a git repository, the commit the
/// agent starts from.
fn provision(
    task: &Task,
    attempt: &Path,
    caches: &HashMap<String, PathBuf>,
) -> Result<(PathBuf, Option<String>), String> {
    let workspace = attempt.join("workspace");
    match &task.source {
        Source::Git { repo, rev } => {
            let cache = caches.get(repo).ok_or("repository cache was not prepared")?;
            git(None, &["clone", "--quiet", "--shared", "--no-checkout", &cache.to_string_lossy(), &workspace.to_string_lossy()])?;
            git(Some(&workspace), &["-c", "advice.detachedHead=false", "checkout", "--quiet", "--detach", rev])?;
        }
        Source::Fixture(fixture) => {
            copy_tree(fixture, &workspace).map_err(|error| format!("copy fixture: {error}"))?;
        }
        Source::Empty => fs::create_dir(&workspace).map_err(|error| error.to_string())?,
    }
    if task.git_init {
        commit_baseline(&workspace)?;
    }
    let workspace = fs::canonicalize(&workspace).map_err(|error| error.to_string())?;
    if !task.setup.is_empty() {
        let log_path = attempt.join("setup.log");
        let variables = BTreeMap::from([
            ("task".to_owned(), task.dir.to_string_lossy().into_owned()),
            ("workspace".to_owned(), workspace.to_string_lossy().into_owned()),
            ("attempt".to_owned(), attempt.to_string_lossy().into_owned()),
        ]);
        for argv in &task.setup {
            let argv: Vec<_> = argv.iter().map(|arg| expand(arg, &variables)).collect();
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map_err(|error| error.to_string())?;
            let mut command = Command::new(crate::eval::resolve_program(&argv[0], &task.env));
            command
                .args(&argv[1..])
                .current_dir(&workspace)
                .envs(&task.env)
                .stdin(Stdio::null())
                .stdout(log.try_clone().map_err(|error| error.to_string())?)
                .stderr(log)
                .process_group(0);
            let mut child = command.spawn().map_err(|error| format!("setup {}: {error}", argv[0]))?;
            match wait_within(&mut child, Duration::from_secs(task.setup_timeout_secs)) {
                Ok(Some(status)) if status.success() => {}
                Ok(Some(status)) => return Err(format!("setup {:?} failed ({status}); see setup.log", argv)),
                Ok(None) => return Err(format!("setup {:?} did not finish in time", argv)),
                Err(error) => return Err(error.to_string()),
            }
        }
        if task.git_init {
            // Setup output becomes part of the baseline, not the agent's diff.
            git(Some(&workspace), &["add", "-A", "."])?;
            git(Some(&workspace), &["commit", "--quiet", "--allow-empty", "--no-verify", "-m", "Eval setup"])?;
        }
    }
    let baseline = git(Some(&workspace), &["rev-parse", "--verify", "--quiet", "HEAD"])
        .ok()
        .map(|sha| sha.trim().to_owned())
        .filter(|sha| !sha.is_empty());
    Ok((workspace, baseline))
}

/// Everything the agent changed since `baseline`, committed or not, without
/// touching the workspace's own index: (files, insertions, deletions), with
/// the patch written to `changes.patch` in `output`.
pub(crate) fn capture_changes(workspace: &Path, output: &Path, baseline: &str) -> Option<(u64, u64, u64)> {
    if !workspace.join(".git").exists() {
        return None;
    }
    let index = output.join(".changes-index");
    let run = |args: &[&str]| -> Option<String> {
        let mut command = git_command(Some(workspace));
        command.env("GIT_INDEX_FILE", &index).args(args);
        let (status, stdout, _) = output_within(command, Duration::from_secs(120)).ok()?;
        status.success().then_some(stdout)
    };
    let result = (|| {
        run(&["read-tree", "HEAD"])?;
        run(&["add", "-A", "."])?;
        let mut patch = run(&["diff", "--cached", "--binary", baseline])?;
        if patch.len() > MAX_PATCH_BYTES {
            let mut cut = MAX_PATCH_BYTES;
            while !patch.is_char_boundary(cut) {
                cut -= 1;
            }
            patch.truncate(cut);
            patch.push_str("\n[omperor eval: patch truncated at 8 MiB]\n");
        }
        fs::write(output.join("changes.patch"), patch).ok()?;
        let numstat = run(&["diff", "--cached", "--numstat", baseline])?;
        let mut totals = (0, 0, 0);
        for line in numstat.lines() {
            let mut fields = line.split('\t');
            let (added, removed) = (fields.next()?, fields.next()?);
            totals.0 += 1;
            totals.1 += added.parse::<u64>().unwrap_or(0);
            totals.2 += removed.parse::<u64>().unwrap_or(0);
        }
        Some(totals)
    })();
    let _ = fs::remove_file(&index);
    result
}

// ---------------------------------------------------------------------------
// Services

struct Services(Vec<(String, Child)>);

impl Drop for Services {
    fn drop(&mut self) {
        for (_, child) in &mut self.0 {
            kill_group(child);
            let _ = child.wait();
        }
    }
}

fn free_port() -> Result<u16, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("allocate a loopback port: {error}"))
}

fn start_services(
    task: &Task,
    attempt: &Path,
    variables: &BTreeMap<String, String>,
) -> Result<Services, String> {
    let mut services = Services(Vec::new());
    for spec in &task.services {
        let port: u16 = variables[&format!("port:{}", spec.name)].parse().expect("allocated port");
        let log = File::create(attempt.join(format!("service-{}.log", spec.name))).map_err(|error| error.to_string())?;
        let argv: Vec<_> = spec.argv.iter().map(|arg| expand(arg, variables)).collect();
        let child = Command::new(crate::eval::resolve_program(&argv[0], &task.env))
            .args(&argv[1..])
            .current_dir(attempt)
            .envs(&task.env)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|error| error.to_string())?)
            .stderr(log)
            .process_group(0)
            .spawn()
            .map_err(|error| format!("service {}: {error}", spec.name))?;
        services.0.push((spec.name.clone(), child));
        let deadline = Instant::now() + Duration::from_secs(spec.ready_timeout_secs.unwrap_or(10));
        loop {
            if TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok() {
                break;
            }
            let (_, child) = services.0.last_mut().expect("just pushed");
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("service {} exited ({status}); see service-{}.log", spec.name, spec.name));
            }
            if Instant::now() >= deadline || INTERRUPTED.load(Ordering::Relaxed) != 0 {
                return Err(format!("service {} did not accept connections on port {port}", spec.name));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    Ok(services)
}

/// Replaces only the known placeholders, so prompts may contain other braces.
fn expand(text: &str, variables: &BTreeMap<String, String>) -> String {
    let mut text = text.to_owned();
    for (name, value) in variables {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

fn variables(task: &Task, workspace: &Path, attempt: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut variables = BTreeMap::from([
        ("task".to_owned(), task.dir.to_string_lossy().into_owned()),
        ("workspace".to_owned(), workspace.to_string_lossy().into_owned()),
        ("attempt".to_owned(), attempt.to_string_lossy().into_owned()),
    ]);
    for service in &task.services {
        variables.insert(format!("port:{}", service.name), free_port()?.to_string());
    }
    Ok(variables)
}

fn checks_json(task: &Task, variables: &BTreeMap<String, String>) -> Vec<Value> {
    task.checks
        .iter()
        .map(|check| match check {
            CheckSpec::AnswerContains { text } => json!({"type": "answer_contains", "text": text}),
            CheckSpec::Command { argv, timeout_secs } => json!({
                "type": "command",
                "argv": argv.iter().map(|arg| expand(arg, variables)).collect::<Vec<_>>(),
                "timeout_secs": timeout_secs.unwrap_or(300),
            }),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Attempts

#[derive(Clone, Debug)]
struct Plan {
    task: usize,
    model: String,
    account: Option<String>,
    thinking_level: Option<String>,
    trial: u32,
    directory: PathBuf,
}

impl Plan {
    /// Attempts that share a credential stop after it fails to authenticate.
    fn credential(&self) -> String {
        self.account
            .clone()
            .unwrap_or_else(|| format!("{}:<backend default>", provider_of(&self.model)))
    }
}

struct Context {
    executable: PathBuf,
    tasks: Vec<Task>,
    caches: HashMap<String, PathBuf>,
    prune_workspaces: bool,
    rejected_credentials: Mutex<HashSet<String>>,
}

fn slug(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect()
}

fn run_attempt(context: &Context, plan: &Plan) -> AttemptRecord {
    let task = &context.tasks[plan.task];
    let mut record = AttemptRecord::new(
        &task.id,
        &plan.model,
        plan.account.clone(),
        plan.thinking_level.clone(),
        plan.trial,
        plan.directory.clone(),
    );
    let credential = plan.credential();
    if context
        .rejected_credentials
        .lock()
        .expect("credential set")
        .contains(&credential)
    {
        record.outcome = Outcome::Skipped;
        record.status = "skipped".into();
        record.error = Some(format!("{credential} failed to authenticate in an earlier attempt; sign in again (omperor eval doctor)"));
    } else {
        let started = Instant::now();
        if let Err(error) = attempt(context, task, plan, &mut record) {
            record.outcome = Outcome::Infrastructure;
            if record.status == "not_started" {
                record.status = "setup_failed".into();
            }
            record.error = Some(error);
            record.elapsed_ms = started.elapsed().as_millis() as u64;
        }
        if record.status == "authentication_required" {
            context
                .rejected_credentials
                .lock()
                .expect("credential set")
                .insert(credential);
        }
    }
    let _ = fs::create_dir_all(&plan.directory);
    if let Err(error) = write_json(&plan.directory.join("attempt.json"), &record) {
        eprintln!("omperor eval suite: {error}");
    }
    record
}

fn attempt(context: &Context, task: &Task, plan: &Plan, record: &mut AttemptRecord) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&plan.directory)
        .map_err(|error| format!("{}: {error}", plan.directory.display()))?;
    let (workspace, baseline) = provision(task, &plan.directory, &context.caches)?;
    let variables = variables(task, &workspace, &plan.directory)?;
    let services = start_services(task, &plan.directory, &variables)?;
    let mut spec = json!({
        "schema_version": 1,
        "id": task.id,
        "workspace": workspace,
        "model": plan.model,
        "prompt": expand(&task.prompt, &variables),
        "computer_use": task.computer_use,
        "timeout_secs": task.timeout_secs,
        "checks": checks_json(task, &variables),
        "env": task.env,
    });
    if let Some(baseline) = &baseline {
        spec["diff_baseline"] = json!(baseline);
    }
    if let Some(account) = &plan.account {
        spec["account"] = json!(account);
    }
    if let Some(level) = &plan.thinking_level {
        spec["thinking_level"] = json!(level);
    }
    let spec_path = plan.directory.join("spec.json");
    write_json(&spec_path, &spec)?;
    let run = plan.directory.join("run");
    let output = |name: &str| File::create(plan.directory.join(name)).map_err(|error| error.to_string());
    // Its own process group, so whatever the attempt leaves behind (a browser
    // the agent spawned, a sub-agent's daemon) is reaped with it below.
    let mut child = Command::new(&context.executable)
        .arg("eval")
        .arg("run")
        .arg(&spec_path)
        .arg(&run)
        .stdin(Stdio::null())
        .stdout(output("run.stdout")?)
        .stderr(output("run.stderr")?)
        .process_group(0)
        .spawn()
        .map_err(|error| format!("start eval run: {error}"))?;
    // The run bounds itself (turn deadline, 20 s shutdown, grader deadlines);
    // this outer limit only catches a wedged child.
    let checks: u64 = task
        .checks
        .iter()
        .map(|check| match check {
            CheckSpec::Command { timeout_secs, .. } => timeout_secs.unwrap_or(300),
            CheckSpec::AnswerContains { .. } => 0,
        })
        .sum();
    let limit = Duration::from_secs(task.timeout_secs + checks + 120);
    let started = Instant::now();
    let mut forwarded = false;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if !forwarded && INTERRUPTED.load(Ordering::Relaxed) != 0 {
            // The child's group does not get the terminal's Ctrl-C, so an
            // interrupt is forwarded and the child still saves its result.
            if let Ok(pid) = i32::try_from(child.id()) {
                // SAFETY: kill takes only integers and targets our own child.
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
            forwarded = true;
        }
        if started.elapsed() > limit {
            kill_group(&child);
            let _ = child.wait();
            return Err("eval run exceeded its outer deadline and was killed".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    kill_group(&child);
    drop(services);
    let result = fs::read(run.join("result.json"))
        .map_err(|error| format!("run/result.json: {error}; see run.stderr"))
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| format!("run/result.json: {error}")));
    record.absorb_result(status.code(), result);
    for (index, check) in record.checks.iter_mut().enumerate() {
        if !check.passed {
            check.detail = check_detail(&run, index, &check.detail);
        }
    }
    if context.prune_workspaces {
        let _ = fs::remove_dir_all(&workspace);
    }
    Ok(())
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    // Write-then-rename keeps a reader from seeing half a summary.
    let partial = path.with_extension("json.partial");
    fs::write(&partial, bytes)
        .and_then(|()| fs::rename(&partial, path))
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn progress(done: usize, total: usize, record: &AttemptRecord) {
    let outcome = match record.outcome {
        Outcome::Passed => "passed",
        Outcome::Failed => "FAILED",
        Outcome::Ungraded => "ungraded",
        Outcome::Infrastructure => "INFRASTRUCTURE",
        Outcome::Skipped => "skipped",
    };
    let detail = match record.outcome {
        Outcome::Infrastructure | Outcome::Skipped => {
            record.error.clone().unwrap_or_default().chars().take(160).collect::<String>()
        }
        _ => format!(
            "{:.1} s, {} tokens over {} requests, {} tool calls ({} errors){}",
            record.elapsed_ms as f64 / 1000.0,
            record.tokens,
            record.requests,
            record.tool_calls,
            record.tool_errors,
            if record.status == "completed" { String::new() } else { format!(", {}", record.status) }
        ),
    };
    eprintln!(
        "[{done}/{total}] {} {} #{}: {outcome} — {detail}",
        record.task, record.model, record.trial
    );
}

fn suite(args: &[String]) -> Result<i32, String> {
    let args = Arguments::parse(
        args,
        &["resume", "prune-workspaces", "dry-run", "json", "help"],
        &["output", "model", "account", "thinking", "repeat", "jobs", "task", "tag", "exclude-tag"],
    )?;
    if args.flag("help") {
        print!("{SUITE_HELP}");
        return Ok(0);
    }
    let output = PathBuf::from(args.one("output")?.ok_or("--output DIR is required (see --help)")?);
    let repeat = args.number("repeat", 1, 1..=1000)? as u32;
    let jobs = args.number("jobs", 1, 1..=64)? as usize;
    let tasks = select(discover(&args.positional)?, &args);
    if tasks.is_empty() {
        return Err("no tasks match".into());
    }
    let requested = args.all("model");
    if requested.iter().any(|model| model.trim().is_empty() || model.starts_with("mock/")) {
        return Err("--model needs a real inference model".into());
    }
    let thinking = args.one("thinking")?;
    // Every model any task will run, for account resolution.
    let mut models: Vec<String> = Vec::new();
    for task in &tasks {
        let task_models = if requested.is_empty() {
            vec![task.model.clone().ok_or_else(|| {
                format!("task {} names no model; pass --model", task.id)
            })?]
        } else {
            requested.clone()
        };
        for model in task_models {
            if !models.contains(&model) {
                models.push(model);
            }
        }
    }
    let root = state_root()?;
    let accounts = resolve_accounts(&models, &args.all("account"), &desktop_accounts(&root))?;
    let output_exists = output.exists();
    if output_exists && !args.flag("resume") {
        return Err(format!("{} exists; choose a new directory or pass --resume", output.display()));
    }
    let output_absolute = if output.is_absolute() {
        output.clone()
    } else {
        std::env::current_dir().map_err(|error| error.to_string())?.join(&output)
    };
    let mut plans = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        let task_models = if requested.is_empty() { vec![task.model.clone().expect("checked")] } else { requested.clone() };
        for model in task_models {
            for trial in 1..=repeat {
                plans.push(Plan {
                    task: index,
                    account: accounts[&model].clone(),
                    thinking_level: thinking.clone().or_else(|| task.thinking_level.clone()),
                    directory: output_absolute
                        .join("attempts")
                        .join(&task.id)
                        .join(slug(&model))
                        .join(format!("{trial:03}")),
                    model: model.clone(),
                    trial,
                });
            }
        }
    }
    if args.flag("dry-run") {
        println!("{} attempts, {jobs} at a time; state root {}", plans.len(), root.display());
        for plan in &plans {
            println!(
                "{} {} #{} account={} thinking={}",
                tasks[plan.task].id,
                plan.model,
                plan.trial,
                plan.account.as_deref().unwrap_or("<backend default>"),
                plan.thinking_level.as_deref().unwrap_or("<default>"),
            );
        }
        return Ok(0);
    }
    for model in &models {
        if accounts[model].is_none() {
            eprintln!(
                "warning: no saved account for {}; OMP will choose one. Pin one with --account (see omperor eval doctor).",
                provider_of(model)
            );
        }
    }
    let mut caches = HashMap::new();
    for task in &tasks {
        if let Source::Git { repo, rev } = &task.source {
            caches.insert(repo.clone(), ensure_git_cache(repo, rev)?);
        }
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&output_absolute)
        .map_err(|error| format!("{}: {error}", output_absolute.display()))?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    write_json(
        &output_absolute.join("suite.json"),
        &json!({
            "schema_version": 1,
            "app_version": env!("CARGO_PKG_VERSION"),
            "created_at_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            "executable": executable,
            "helper": crate::omp_runtime::eval_executable(),
            "state_root": root,
            "repeat": repeat,
            "jobs": jobs,
            "tasks": tasks.iter().map(|task| json!({
                "id": task.id, "directory": task.dir, "description": task.description, "tags": task.tags,
            })).collect::<Vec<_>>(),
            "attempts": plans.iter().map(|plan| json!({
                "task": tasks[plan.task].id, "model": plan.model, "account": plan.account,
                "thinking_level": plan.thinking_level, "trial": plan.trial, "directory": plan.directory,
            })).collect::<Vec<_>>(),
        }),
    )?;
    // Finished attempts are kept on resume; partial ones start over.
    let mut records = Vec::new();
    let mut queue = VecDeque::new();
    for plan in plans {
        let finished = fs::read(plan.directory.join("attempt.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AttemptRecord>(&bytes).ok());
        match finished {
            Some(record) if record.outcome != Outcome::Skipped => records.push(record),
            _ => {
                if plan.directory.exists() {
                    fs::remove_dir_all(&plan.directory).map_err(|error| format!("{}: {error}", plan.directory.display()))?;
                }
                queue.push_back(plan);
            }
        }
    }
    let total = records.len() + queue.len();
    if !records.is_empty() {
        eprintln!("Resuming: {} of {total} attempts already finished.", records.len());
    }
    let _interrupts = InterruptHandlers::install().map_err(|error| error.to_string())?;
    let context = Arc::new(Context {
        executable,
        tasks,
        caches,
        prune_workspaces: args.flag("prune-workspaces"),
        rejected_credentials: Mutex::new(HashSet::new()),
    });
    let queue = Arc::new(Mutex::new(queue));
    let (sender, receiver) = mpsc::channel();
    let workers: Vec<_> = (0..jobs)
        .map(|_| {
            let (context, queue, sender) = (context.clone(), queue.clone(), sender.clone());
            thread::spawn(move || {
                loop {
                    if INTERRUPTED.load(Ordering::Relaxed) != 0 {
                        break;
                    }
                    let Some(plan) = queue.lock().expect("attempt queue").pop_front() else {
                        break;
                    };
                    if sender.send(run_attempt(&context, &plan)).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    drop(sender);
    for record in receiver {
        records.push(record);
        progress(records.len(), total, records.last().expect("pushed"));
        let summary = eval_report::summarize(&records);
        write_json(&output_absolute.join("summary.json"), &summary)?;
    }
    for worker in workers {
        let _ = worker.join();
    }
    records.sort_by(|a, b| (&a.task, &a.model, a.trial).cmp(&(&b.task, &b.model, b.trial)));
    let summary = eval_report::summarize(&records);
    write_json(&output_absolute.join("summary.json"), &summary)?;
    let markdown = eval_report::markdown(&summary, &records);
    fs::write(output_absolute.join("summary.md"), &markdown).map_err(|error| error.to_string())?;
    if args.flag("json") {
        println!("{}", serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?);
    } else {
        print!("{markdown}");
    }
    if INTERRUPTED.load(Ordering::Relaxed) != 0 {
        eprintln!(
            "Interrupted with {} of {total} attempts finished; rerun with --resume to continue.",
            records.len()
        );
        return Ok(1);
    }
    Ok(eval_report::exit_status(&summary))
}

fn report(args: &[String]) -> Result<i32, String> {
    let args = Arguments::parse(args, &["json"], &["compare"])?;
    let [directory] = args.positional.as_slice() else {
        return Err("usage: omperor eval report DIR [--compare BASELINE_DIR] [--json]".into());
    };
    let records = eval_report::load_records(Path::new(directory))?;
    let summary = eval_report::summarize(&records);
    if let Some(baseline) = args.one("compare")? {
        let baseline = eval_report::summarize(&eval_report::load_records(Path::new(&baseline))?);
        print!("{}", eval_report::compare(&baseline, &summary));
    } else if args.flag("json") {
        println!("{}", serde_json::to_string_pretty(&summary).map_err(|error| error.to_string())?);
    } else {
        print!("{}", eval_report::markdown(&summary, &records));
    }
    Ok(0)
}

fn list_tasks(args: &[String]) -> Result<i32, String> {
    let args = Arguments::parse(args, &[], &["task", "tag", "exclude-tag"])?;
    let tasks = select(discover(&args.positional)?, &args);
    let id_width = tasks.iter().map(|task| task.id.len()).max().unwrap_or(0);
    let tag_width = tasks.iter().map(|task| task.tags.join(",").len()).max().unwrap_or(0);
    for task in tasks {
        println!(
            "{:<id_width$}  {:<tag_width$}  {:<9}  {}",
            task.id,
            task.tags.join(","),
            if task.has_reference() { "reference" } else { "" },
            task.description.as_deref().unwrap_or_default(),
        );
    }
    Ok(0)
}

// ---------------------------------------------------------------------------
// Validation: graders must reject the untouched fixture and accept the reference.

fn run_checks(task: &Task, variables: &BTreeMap<String, String>, workspace: &Path, run: &Path, answer: &str) -> Result<Vec<(bool, String)>, String> {
    fs::create_dir_all(run).map_err(|error| error.to_string())?;
    fs::write(run.join("answer.txt"), answer).map_err(|error| error.to_string())?;
    fs::write(run.join("events.jsonl"), "").map_err(|error| error.to_string())?;
    Ok(task
        .checks
        .iter()
        .enumerate()
        .map(|(index, check)| match check {
            CheckSpec::AnswerContains { text } => (answer.contains(text.as_str()), "answer substring check".into()),
            CheckSpec::Command { argv, timeout_secs } => {
                let argv: Vec<_> = argv.iter().map(|arg| expand(arg, variables)).collect();
                crate::eval::run_grader(&argv, timeout_secs.unwrap_or(300), index, workspace, run, &task.env)
                    .unwrap_or_else(|error| (false, error.to_string()))
            }
        })
        .collect())
}

fn apply_reference(task: &Task, workspace: &Path) -> Result<(), String> {
    if let Some(reference) = &task.reference_dir {
        copy_tree(reference, workspace).map_err(|error| format!("copy reference: {error}"))?;
    }
    if let Some(patch) = &task.reference_patch {
        git(Some(workspace), &["apply", "--whitespace=nowarn", &patch.to_string_lossy()])?;
    }
    if let Some(script) = &task.reference_script {
        // NixOS and others have no /bin/bash.
        let bash = if cfg!(target_os = "macos") {
            PathBuf::from("/bin/bash")
        } else {
            crate::eval::resolve_program("bash", &task.env)
        };
        let mut command = Command::new(bash);
        command
            .arg(script)
            .current_dir(workspace)
            .envs(&task.env)
            .env("OMPEROR_EVAL_TASK_DIR", &task.dir)
            .stdin(Stdio::null());
        let (status, stdout, stderr) = output_within(command, Duration::from_secs(task.setup_timeout_secs))
            .map_err(|error| format!("reference.sh: {error}"))?;
        if !status.success() {
            return Err(format!("reference.sh failed ({status}): {}", tail(&format!("{stdout}{stderr}"))));
        }
    }
    Ok(())
}

/// The last lines of a command's output, for a one-line report.
fn tail(text: &str) -> String {
    let lines: Vec<_> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(3)..].join(" ⏎ ")
}

/// A failed check's detail with the end of what its grader printed.
fn check_detail(run: &Path, index: usize, detail: &str) -> String {
    let output = ["stdout", "stderr"]
        .iter()
        .filter_map(|stream| fs::read_to_string(run.join(format!("check-{index}.{stream}"))).ok())
        .collect::<String>();
    let output = tail(&output);
    if output.is_empty() { detail.to_owned() } else { format!("{detail}: {output}") }
}

fn validate_pass(task: &Task, caches: &HashMap<String, PathBuf>, directory: &Path, solved: bool) -> Result<Vec<(bool, String)>, String> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let (workspace, _) = provision(task, directory, caches)?;
    if solved {
        apply_reference(task, &workspace)?;
    }
    let variables = variables(task, &workspace, directory)?;
    let _services = start_services(task, directory, &variables)?;
    let answer = match (&task.reference_answer, solved) {
        (Some(path), true) => fs::read_to_string(path).map_err(|error| error.to_string())?,
        _ => String::new(),
    };
    let run = directory.join("run");
    Ok(run_checks(task, &variables, &workspace, &run, &answer)?
        .into_iter()
        .enumerate()
        .map(|(index, (passed, detail))| {
            let detail = if passed { detail } else { check_detail(&run, index, &detail) };
            (passed, detail)
        })
        .collect())
}

fn validate(args: &[String]) -> Result<i32, String> {
    let args = Arguments::parse(args, &["keep"], &["task", "tag", "exclude-tag"])?;
    let tasks = select(discover(&args.positional)?, &args);
    let scratch = std::env::temp_dir().join(format!("omperor-eval-validate-{}", std::process::id()));
    let _interrupts = InterruptHandlers::install().map_err(|error| error.to_string())?;
    let mut problems = 0;
    for task in &tasks {
        let mut caches = HashMap::new();
        if let Source::Git { repo, rev } = &task.source {
            caches.insert(repo.clone(), ensure_git_cache(repo, rev)?);
        }
        let directory = scratch.join(&task.id);
        let unsolved = if task.checks.is_empty() {
            Err("no checks: every attempt would be ungraded".into())
        } else {
            validate_pass(task, &caches, &directory.join("unsolved"), false)
        };
        let unsolved = match unsolved {
            Ok(results) if results.iter().all(|(passed, _)| *passed) => {
                Err("the untouched fixture already passes every check".to_owned())
            }
            Ok(_) => Ok("rejects the unsolved fixture".to_owned()),
            Err(error) => Err(error),
        };
        let solved = if task.has_reference() {
            match validate_pass(task, &caches, &directory.join("solved"), true) {
                Ok(results) => match results.iter().find(|(passed, _)| !passed) {
                    None => Ok("accepts the reference".to_owned()),
                    Some((_, detail)) => Err(format!("rejects the reference: {detail}")),
                },
                Err(error) => Err(error),
            }
        } else {
            Ok("no reference solution".to_owned())
        };
        for (label, outcome) in [("unsolved", &unsolved), ("solved", &solved)] {
            match outcome {
                Ok(text) => println!("ok   {} [{label}]: {text}", task.id),
                Err(text) => {
                    problems += 1;
                    println!("BAD  {} [{label}]: {text}", task.id);
                }
            }
        }
    }
    // Keep the evidence whenever something is wrong.
    if args.flag("keep") || problems > 0 {
        println!("Validation workspaces kept in {}", scratch.display());
    } else {
        let _ = fs::remove_dir_all(&scratch);
    }
    Ok(i32::from(problems > 0))
}

// ---------------------------------------------------------------------------
// Doctor and login: everything a headless machine needs before a suite.

fn doctor(args: &[String]) -> Result<i32, String> {
    let _ = Arguments::parse(args, &[], &[])?;
    let root = state_root()?;
    let data = root.join("omp/data");
    let helper = crate::omp_runtime::eval_executable();
    let desktop = desktop_accounts(&root);
    let mut problems = Vec::new();
    let live = if !helper.is_file() {
        problems.push(format!(
            "OMP helper {} is missing: build with scripts/build-app.sh or set OMPEROR_OMP_EXECUTABLE",
            helper.display()
        ));
        None
    } else if !data.join("credentials.db").is_file() {
        problems.push(format!("no credential store at {}: run omperor eval login PROVIDER", data.display()));
        None
    } else {
        match live_accounts(&helper, &data) {
            Ok(accounts) => Some(accounts),
            Err(error) => {
                problems.push(format!("could not list accounts: {error}"));
                None
            }
        }
    };
    let mut accounts = Vec::new();
    let mut warnings = Vec::new();
    for account in live.iter().flatten() {
        let metadata = desktop.iter().find(|stored| stored.account_id == account.id);
        if account.state != "Active" {
            // Only a default account blocks a suite that relies on defaults.
            let message = format!(
                "{} is {}: run omperor eval login {} (or sign in from Settings → Accounts)",
                account.id, account.state, account.provider
            );
            if metadata.is_some_and(|stored| stored.is_default) {
                problems.push(message);
            } else {
                warnings.push(message);
            }
        }
        accounts.push(json!({
            "id": account.id, "provider": account.provider, "state": account.state,
            "label": metadata.and_then(|stored| stored.label.clone()),
            "is_default": metadata.is_some_and(|stored| stored.is_default),
        }));
    }
    if live.as_ref().is_some_and(|live| !live.iter().any(|account| account.state == "Active")) {
        problems.push("no account is signed in and active: run omperor eval login PROVIDER".into());
    }
    let ready = problems.is_empty();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version": 1,
            "ready": ready,
            "state_root": root,
            "data_dir": data,
            "helper": helper,
            "accounts": accounts,
            "problems": problems,
            "warnings": warnings,
            "note": "Active means OMP holds a credential; it is verified only when a request is made.",
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(if ready { 0 } else { 1 })
}

fn login(args: &[String]) -> Result<i32, String> {
    let args = Arguments::parse(args, &["default"], &["label"])?;
    let [provider] = args.positional.as_slice() else {
        return Err("usage: omperor eval login PROVIDER [--label LABEL] [--default]  (PROVIDER: openai-codex, anthropic, ...)".into());
    };
    let root = state_root()?;
    let data = root.join("omp/data");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&data)
        .map_err(|error| format!("{}: {error}", data.display()))?;
    let helper = crate::omp_runtime::eval_executable();
    let before: HashSet<_> = live_accounts(&helper, &data)
        .unwrap_or_default()
        .into_iter()
        .map(|account| account.id)
        .collect();
    // OMP's interactive login: it prints an authorization URL and accepts the
    // pasted callback URL, so it works over SSH without a local browser.
    let status = helper_command(&helper, &data)
        .arg("auth")
        .arg("--data-dir")
        .arg(&data)
        .arg("login")
        .arg(provider)
        .status()
        .map_err(|error| format!("start {}: {error}", helper.display()))?;
    if !status.success() {
        return Ok(1);
    }
    let added: Vec<_> = live_accounts(&helper, &data)?
        .into_iter()
        .filter(|account| account.provider == *provider && !before.contains(&account.id))
        .collect();
    let database = root.join("omperor.sqlite3");
    let has_default = desktop_accounts(&root)
        .iter()
        .any(|account| account.provider == *provider && account.is_default);
    let label = args.one("label")?;
    for account in &added {
        crate::preferences::record_account_in(&database, &account.id, provider, account.principal.as_deref(), None)
            .map_err(|error| error.to_string())?;
        if let Some(label) = label.as_deref().filter(|_| added.len() == 1) {
            crate::preferences::set_account_label_in(&database, &account.id, label).map_err(|error| error.to_string())?;
        }
        if (args.flag("default") || !has_default) && added.len() == 1 {
            crate::preferences::set_default_account_in(&database, provider, Some(&account.id))
                .map_err(|error| error.to_string())?;
        }
    }
    println!(
        "{}",
        json!({
            "schema_version": 1,
            "added": added.iter().map(|account| &account.id).collect::<Vec<_>>(),
            "note": if added.is_empty() { "No new account appeared; an existing one may have been refreshed." } else { "Pin it in specs or with --account; run omperor eval doctor to check." },
        })
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::StoredAccount;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("omperor-eval-suite-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn stored(id: &str, provider: &str, is_default: bool) -> StoredAccount {
        StoredAccount {
            account_id: id.into(),
            provider: provider.into(),
            principal: None,
            label: None,
            authenticated_at_ms: 0,
            is_default,
        }
    }

    #[test]
    fn arguments_take_inline_and_separate_values() {
        let args: Vec<String> = ["a", "--model=x", "--model", "y", "--resume", "b"].map(String::from).into();
        let parsed = Arguments::parse(&args, &["resume"], &["model"]).unwrap();
        assert_eq!(parsed.positional, ["a", "b"]);
        assert_eq!(parsed.all("model"), ["x", "y"]);
        assert!(parsed.flag("resume"));
        assert!(Arguments::parse(&["--bogus".into()], &[], &[]).is_err());
        assert!(Arguments::parse(&["--model".into()], &[], &["model"]).is_err());
        assert!(parsed.one("model").is_err());
    }

    #[test]
    fn accounts_resolve_explicit_then_default_then_only() {
        let known = vec![
            stored("openai-codex:a", "openai-codex", false),
            stored("openai-codex:b", "openai-codex", true),
            stored("anthropic:c", "anthropic", false),
        ];
        let models = vec!["openai-codex/m".to_owned(), "anthropic/n".to_owned(), "ollama/o".to_owned()];
        let chosen = resolve_accounts(&models, &[], &known).unwrap();
        assert_eq!(chosen["openai-codex/m"].as_deref(), Some("openai-codex:b"));
        assert_eq!(chosen["anthropic/n"].as_deref(), Some("anthropic:c"));
        assert_eq!(chosen["ollama/o"], None);
        let chosen = resolve_accounts(&models, &["openai-codex:a".into()], &known).unwrap();
        assert_eq!(chosen["openai-codex/m"].as_deref(), Some("openai-codex:a"));
        assert!(resolve_accounts(&models, &["google:z".into()], &known).is_err());
    }

    #[test]
    fn live_account_lines_parse() {
        let parsed = parse_live_accounts(
            "selector=anthropic:x@y provider=anthropic state=Active principal=x@y\n\
             selector=openai-codex:1 provider=openai-codex state=RefreshRequired principal=1\n\
             garbage line\n",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].state, "RefreshRequired");
        assert_eq!(parsed[0].principal.as_deref(), Some("x@y"));
    }

    #[test]
    fn placeholders_expand_and_other_braces_survive() {
        let variables = BTreeMap::from([("workspace".to_owned(), "/w".to_owned()), ("port:web".to_owned(), "8080".to_owned())]);
        assert_eq!(
            expand("cd {workspace}; open http://127.0.0.1:{port:web}; return {\"a\": 1} {unknown}", &variables),
            "cd /w; open http://127.0.0.1:8080; return {\"a\": 1} {unknown}"
        );
    }

    #[test]
    fn task_files_load_with_defaults_and_reject_mistakes() {
        let root = scratch("load");
        let task = root.join("sample-task");
        fs::create_dir_all(task.join("fixture")).unwrap();
        fs::create_dir_all(task.join("reference")).unwrap();
        fs::write(task.join("task.toml"), "prompt = \"Do it\"\ntags = [\"x\"]\n[[checks]]\ntype = \"command\"\nargv = [\"true\"]\n").unwrap();
        let loaded = load_task(&task).unwrap();
        assert_eq!(loaded.id, "sample-task");
        assert!(matches!(loaded.source, Source::Fixture(_)));
        assert!(loaded.git_init);
        assert!(loaded.has_reference());
        assert_eq!(loaded.timeout_secs, 600);
        fs::write(task.join("task.toml"), "prompt = \"x\"\ntypo = 1\n").unwrap();
        assert!(load_task(&task).is_err());
        fs::write(task.join("task.toml"), "prompt = \"x\"\n[[checks]]\ntype = \"command\"\nargv = []\n").unwrap();
        assert!(load_task(&task).is_err());
        fs::write(task.join("task.toml"), "id = \"../escape\"\nprompt = \"x\"\n").unwrap();
        assert!(load_task(&task).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provisioning_commits_a_baseline_and_diffs_only_the_agents_changes() {
        let root = scratch("provision");
        let task_dir = root.join("t");
        fs::create_dir_all(task_dir.join("fixture/src")).unwrap();
        fs::write(task_dir.join("fixture/src/a.txt"), "one\n").unwrap();
        fs::write(
            task_dir.join("task.toml"),
            "prompt = \"x\"\n[workspace]\nsetup = [[\"sh\", \"-c\", \"echo generated > setup.txt; test -d {task}\"]]\n",
        )
        .unwrap();
        let task = load_task(&task_dir).unwrap();
        let attempt = root.join("attempt");
        fs::create_dir_all(&attempt).unwrap();
        let (workspace, baseline) = provision(&task, &attempt, &HashMap::new()).unwrap();
        let baseline = baseline.unwrap();
        assert_eq!(fs::read_to_string(workspace.join("setup.txt")).unwrap(), "generated\n");
        fs::write(workspace.join("src/a.txt"), "one\ntwo\n").unwrap();
        // A change the agent commits still counts as its own.
        git(Some(&workspace), &["commit", "--quiet", "-am", "agent commit"]).unwrap();
        fs::write(workspace.join("new.txt"), "new\n").unwrap();
        let (files, insertions, deletions) = capture_changes(&workspace, &attempt, &baseline).unwrap();
        assert_eq!((files, insertions, deletions), (2, 2, 0));
        let patch = fs::read_to_string(attempt.join("changes.patch")).unwrap();
        assert!(patch.contains("+two") && patch.contains("new.txt") && !patch.contains("setup.txt"));
        // The workspace's own index is untouched: new.txt is still untracked.
        let status = git(Some(&workspace), &["status", "--porcelain"]).unwrap();
        assert!(status.contains("?? new.txt"), "{status}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validation_requires_graders_to_tell_solved_from_unsolved() {
        let root = scratch("validate");
        let task_dir = root.join("v");
        fs::create_dir_all(task_dir.join("fixture")).unwrap();
        fs::create_dir_all(task_dir.join("reference")).unwrap();
        fs::write(task_dir.join("fixture/value.txt"), "wrong\n").unwrap();
        fs::write(task_dir.join("reference/value.txt"), "right\n").unwrap();
        fs::write(
            task_dir.join("task.toml"),
            "prompt = \"x\"\n[[checks]]\ntype = \"command\"\nargv = [\"grep\", \"-q\", \"right\", \"value.txt\"]\n",
        )
        .unwrap();
        let task = load_task(&task_dir).unwrap();
        let unsolved = validate_pass(&task, &HashMap::new(), &root.join("u"), false).unwrap();
        assert!(!unsolved[0].0);
        let solved = validate_pass(&task, &HashMap::new(), &root.join("s"), true).unwrap();
        assert!(solved[0].0, "{solved:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn services_serve_on_their_allocated_port_and_stop() {
        let root = scratch("service");
        let task_dir = root.join("s");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(
            task_dir.join("task.toml"),
            "prompt = \"x\"\n[[services]]\nname = \"web\"\nargv = [\"python3\", \"-m\", \"http.server\", \"--bind\", \"127.0.0.1\", \"{port:web}\"]\n",
        )
        .unwrap();
        let task = load_task(&task_dir).unwrap();
        let variables = variables(&task, &root, &root).unwrap();
        let port: u16 = variables["port:web"].parse().unwrap();
        let services = start_services(&task, &root, &variables).unwrap();
        assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok());
        drop(services);
        thread::sleep(Duration::from_millis(200));
        assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn processes_an_attempt_leaves_behind_are_reaped() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("reap");
        let task_dir = root.join("t");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(task_dir.join("task.toml"), "prompt = \"x\"\n").unwrap();
        // Stands in for `omperor eval run` (`eval run SPEC RUN`): it leaves a
        // process running, as a browser the agent spawned would, and exits.
        let executable = root.join("fake-omperor");
        fs::write(&executable, "#!/bin/sh\nsleep 300 &\necho $! > \"$4.leftover\"\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let context = Context {
            executable,
            tasks: vec![load_task(&task_dir).unwrap()],
            caches: HashMap::new(),
            prune_workspaces: true,
            rejected_credentials: Mutex::default(),
        };
        let plan = Plan {
            task: 0,
            model: "test/model".into(),
            account: None,
            thinking_level: None,
            trial: 1,
            directory: root.join("attempt"),
        };
        run_attempt(&context, &plan);
        let pid: i32 = fs::read_to_string(root.join("attempt/run.leftover")).unwrap().trim().parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        // SAFETY: signal 0 only asks whether the process still exists.
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: as above.
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "process {pid} outlived its attempt");
        fs::remove_dir_all(root).unwrap();
    }
}
