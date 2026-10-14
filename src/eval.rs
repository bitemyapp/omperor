//! Headless evaluations use the desktop's RPC runtime and receipt ledger.
//! Each invocation owns a fresh session; graders run only after it has stopped.
use crate::{
    mock_inference::Backend,
    omp_runtime::{OmpRuntime, RuntimeStartup, SessionStartup, TurnOutcome, UiEvent},
    usage,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::os::unix::{fs::DirBuilderExt, process::CommandExt};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicI32, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HELP: &str = "\
Usage: omperor eval suite [OPTIONS] [TASK_OR_DIRECTORY...] --output DIR
       omperor eval report DIR [--compare BASELINE_DIR] [--json]
       omperor eval tasks [TASK_OR_DIRECTORY...]
       omperor eval validate [TASK_OR_DIRECTORY...]
       omperor eval doctor
       omperor eval login PROVIDER [--label LABEL] [--default]
       omperor eval accounts
       omperor eval run SPEC.json OUTPUT_DIRECTORY

Run agents headlessly against real inference and measure success and tokens.
suite runs task directories (task.toml, fixture, graders) as fresh attempts
across models; run executes one schema-1 request. `omperor eval suite --help`
lists suite options. Evals default to Omperor Dev's state; OMPEROR_STATE_DIR
selects another absolute root. See docs/EVALS.md.

Exit: 0 passed, 1 failed, 2 invalid request/infrastructure, 3 completed but ungraded.
";
const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) static INTERRUPTED: AtomicI32 = AtomicI32::new(0);

extern "C" fn interrupt(signal: libc::c_int) {
    // An atomic store is the only work done in the signal handler. The normal
    // event loop owns aborting, reaping, and writing the final result.
    INTERRUPTED.store(signal, Ordering::Relaxed);
}

pub(crate) struct InterruptHandlers(Vec<(libc::c_int, libc::sigaction)>);
impl InterruptHandlers {
    pub(crate) fn install() -> io::Result<Self> {
        INTERRUPTED.store(0, Ordering::Relaxed);
        let mut handlers = Self(Vec::new());
        for signal in [libc::SIGINT, libc::SIGTERM] {
            // SAFETY: these are C signal-action structs initialized before use;
            // interrupt has the required C ABI and does not allocate or lock.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed(); // ubs:ignore — POSIX C struct with zero-valid integer/pointer fields; handler and mask assigned below.
                action.sa_sigaction = interrupt as *const () as usize;
                libc::sigemptyset(&mut action.sa_mask);
                let mut previous = std::mem::zeroed(); // ubs:ignore — zero-valid C output buffer; only retained after successful sigaction.
                if libc::sigaction(signal, &action, &mut previous) != 0 {
                    return Err(io::Error::last_os_error());
                }
                handlers.0.push((signal, previous));
            }
        }
        Ok(handlers)
    }
}
impl Drop for InterruptHandlers {
    fn drop(&mut self) {
        for (signal, previous) in &self.0 {
            // SAFETY: restore the actions returned by the successful installs.
            unsafe {
                libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    schema_version: u32,
    id: String,
    workspace: PathBuf,
    model: String,
    prompt: String,
    account: Option<String>,
    thinking_level: Option<String>,
    /// Existing OMP credential/model store; defaults to the Dev store.
    data_dir: Option<PathBuf>,
    #[serde(default)]
    computer_use: bool,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default)]
    checks: Vec<Check>,
    /// Variables for the agent's tools and the graders, e.g. offline flags.
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    /// A commit in the workspace to diff against once the agent stops and
    /// before any grader runs, so the record holds the agent's own changes
    /// even when it commits them.
    diff_baseline: Option<String>,
}
fn default_timeout() -> u64 {
    300
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Check {
    AnswerContains {
        text: String,
    },
    Command {
        argv: Vec<String>,
        #[serde(default = "default_timeout")]
        timeout_secs: u64,
    },
}

impl Spec {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("unsupported schema_version (expected 1)".into());
        }
        if self.id.trim().is_empty()
            || self.prompt.trim().is_empty()
            || self.model.trim().is_empty()
        {
            return Err("id, prompt, and model must be nonempty".into());
        }
        if self.model.starts_with("mock/") {
            return Err("eval run requires a real inference model".into());
        }
        if self
            .account
            .as_ref()
            .is_some_and(|account| account.trim().is_empty())
            || self
                .thinking_level
                .as_ref()
                .is_some_and(|level| level.trim().is_empty())
        {
            return Err("account and thinking_level must be nonempty when supplied".into());
        }
        if !self.workspace.is_absolute() || !self.workspace.is_dir() {
            return Err("workspace must be an existing absolute directory".into());
        }
        valid_timeout(self.timeout_secs)?;
        if self
            .env
            .keys()
            .any(|key| key.is_empty() || key.contains('=') || key.contains('\0'))
        {
            return Err("env names must be nonempty and contain no '=' or NUL".into());
        }
        if self
            .data_dir
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err("data_dir must be absolute".into());
        }
        for check in &self.checks {
            match check {
                Check::AnswerContains { text } if text.is_empty() => {
                    return Err("answer_contains text must be nonempty".into());
                }
                Check::Command { argv, timeout_secs } => {
                    if argv.first().is_none_or(|arg| arg.is_empty()) {
                        return Err("command argv must contain an executable".into());
                    }
                    valid_timeout(*timeout_secs)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
fn valid_timeout(seconds: u64) -> Result<(), String> {
    if (1..=3600).contains(&seconds) {
        Ok(())
    } else {
        Err("timeout_secs must be between 1 and 3600".into())
    }
}

#[derive(Debug, Serialize)]
struct CheckResult {
    index: usize,
    passed: bool,
    detail: String,
}
#[derive(Debug, Serialize)]
struct ResultRecord {
    schema_version: u32,
    app_version: &'static str,
    id: String,
    backend: &'static str,
    requested_model: String,
    actual_model: Option<String>,
    requested_account: Option<String>,
    account: Option<String>,
    requested_thinking_level: Option<String>,
    thinking_level: Option<String>,
    workspace: PathBuf,
    started_at_ms: u64,
    elapsed_ms: u64,
    status: String,
    error: Option<String>,
    success: Option<bool>,
    shutdown_complete: bool,
    session_id: Option<String>,
    session_file: Option<PathBuf>,
    tool_calls: u64,
    tool_errors: u64,
    usage: usage::Card,
    /// The workspace's changes against `diff_baseline`, captured before the
    /// graders ran; `changes.patch` holds the diff.
    changes: Option<Changes>,
    checks: Vec<CheckResult>,
}

#[derive(Debug, Serialize)]
struct Changes {
    files: u64,
    insertions: u64,
    deletions: u64,
}

pub(crate) fn dispatch(args: Vec<OsString>) -> Option<i32> {
    if args.first().is_none_or(|a| a != "eval") {
        return None;
    }
    if args.len() == 1 || args.get(1).is_some_and(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Some(0);
    }
    if args.len() == 2 && args[1] == "accounts" {
        return Some(match crate::app_state::eval_root()
            .map_err(crate::preferences::PreferencesError::from)
            .and_then(|root| crate::preferences::load_accounts_from(&root.join("omperor.sqlite3"))) {
            Ok(accounts) => {
                let accounts: Vec<_> = accounts
                    .into_iter()
                    .map(|account| {
                        json!({
                            "id": account.account_id, "provider": account.provider,
                            "label": account.label, "is_default": account.is_default,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    json!({"schema_version":1, "source":"desktop_metadata", "credentials_verified":false, "accounts":accounts})
                );
                0
            }
            Err(error) => {
                eprintln!(
                    "{}",
                    json!({"schema_version":1, "status":"infrastructure_error", "error":error.to_string()})
                );
                2
            }
        });
    }
    if let Some(code) = crate::eval_suite::dispatch(&args) {
        return Some(code);
    }
    if args.len() != 4 || args[1] != "run" {
        eprint!("{HELP}");
        return Some(2);
    }
    Some(match run(Path::new(&args[2]), Path::new(&args[3])) {
        Ok(record) => {
            println!(
                "{}",
                serde_json::to_string(&record).expect("result serialization")
            );
            exit_code(&record.status, record.success)
        }
        Err(error) => {
            eprintln!(
                "{}",
                json!({"schema_version":1, "status":"infrastructure_error", "error":error})
            );
            2
        }
    })
}
pub(crate) fn exit_code(status: &str, success: Option<bool>) -> i32 {
    match (status, success) {
        ("completed", Some(true)) => 0,
        ("completed", None) => 3,
        _ => 1,
    }
}
fn millis(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

struct EventLog {
    file: File,
    bytes: u64,
    started: Instant,
}
impl EventLog {
    fn append(&mut self, kind: &str, data: Value) -> Result<(), String> {
        let mut line = serde_json::to_vec(
            &json!({"schema_version":1,"elapsed_ms":millis(self.started),"event":kind,"data":data}),
        )
        .map_err(|e| e.to_string())?;
        line.push(b'\n');
        self.bytes += line.len() as u64;
        if self.bytes > MAX_LOG_BYTES {
            return Err("event log exceeded 64 MiB".into());
        }
        self.file.write_all(&line).map_err(|e| e.to_string())
    }
}

fn run(spec_path: &Path, output: &Path) -> Result<ResultRecord, String> {
    let spec: Spec = serde_json::from_slice(&fs::read(spec_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    spec.validate()?;
    let _interrupt_handlers = InterruptHandlers::install().map_err(|e| e.to_string())?;
    let data_dir =
        spec.data_dir.clone().map(Ok).unwrap_or_else(|| {
            crate::app_state::eval_root().map(|root| root.join("omp/data")).map_err(|e| e.to_string())
        })?;
    // A new, private directory prevents accidental overwrite. Credentials stay
    // in their existing store; no credential material is copied into artifacts.
    fs::DirBuilder::new()
        .mode(0o700)
        .create(output)
        .map_err(|e| format!("create output directory: {e}"))?;
    let output = fs::canonicalize(output).map_err(|e| e.to_string())?;
    fs::copy(spec_path, output.join("spec.json")).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut log = EventLog {
        file: File::create(output.join("events.jsonl")).map_err(|e| e.to_string())?,
        bytes: 0,
        started,
    };
    let mut record = ResultRecord {
        schema_version: 1,
        app_version: env!("CARGO_PKG_VERSION"),
        id: spec.id.clone(),
        backend: "omp",
        requested_model: spec.model.clone(),
        actual_model: None,
        requested_account: spec.account.clone(),
        account: None,
        requested_thinking_level: spec.thinking_level.clone(),
        thinking_level: None,
        workspace: spec.workspace.clone(),
        started_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        elapsed_ms: 0,
        status: "starting".into(),
        error: None,
        success: None,
        shutdown_complete: false,
        session_id: None,
        session_file: None,
        tool_calls: 0,
        tool_errors: 0,
        usage: usage::Card::default(),
        changes: None,
        checks: vec![],
    };
    let runtime = OmpRuntime::start_configured(RuntimeStartup {
        helper: Some(crate::omp_runtime::eval_executable()),
        backend: Backend::Omp,
        project: spec.workspace.clone(),
        model: Some(spec.model.clone()),
        thinking_level: spec.thinking_level.clone(),
        fast_mode: false,
        omperor_tools: false,
        computer_use: spec.computer_use,
        account: spec.account.clone(),
        data_dir,
        session_dir: output.join("sessions"),
        environment: spec.env.clone().into_iter().collect(),
        session: SessionStartup::Fresh,
    });
    let mut answer = String::new();
    let execution = execute(&runtime, &spec, &mut record, &mut log, &mut answer);
    if let Err(error) = execution {
        record.status = "error".into();
        record.error = Some(error);
    }
    // Always stop/reap, including disk errors, provider errors, and timeouts.
    runtime.abort();
    runtime.begin_shutdown();
    let shutdown = Instant::now();
    while !runtime.try_reap() && shutdown.elapsed() < Duration::from_secs(20) {
        thread::sleep(Duration::from_millis(10));
    }
    record.shutdown_complete = runtime.try_reap();
    record.elapsed_ms = millis(started);
    if !record.shutdown_complete {
        record.status = "error".into();
        record.error =
            Some("RPC worker did not stop within 20 seconds; graders were skipped".into());
    }
    // Same deduplicated ledger as /usage, including discovered child journals.
    record.usage = usage::read(
        record.session_file.clone(),
        vec![],
        record
            .actual_model
            .clone()
            .unwrap_or_else(|| spec.model.clone()),
    )
    .card;
    fs::write(output.join("answer.txt"), &answer).map_err(|e| e.to_string())?;
    if record.shutdown_complete
        && let Some(baseline) = &spec.diff_baseline
    {
        record.changes = crate::eval_suite::capture_changes(&spec.workspace, &output, baseline)
            .map(|(files, insertions, deletions)| Changes { files, insertions, deletions });
    }
    if record.status == "completed" {
        for (index, check) in spec.checks.iter().enumerate() {
            if INTERRUPTED.load(Ordering::Relaxed) != 0 {
                break;
            }
            record
                .checks
                .push(grade(check, index, &answer, &spec.workspace, &output, &spec.env));
        }
    }
    if INTERRUPTED.load(Ordering::Relaxed) != 0 {
        record.status = "interrupted".into();
        record.error = Some("evaluation interrupted by signal".into());
    }
    record.success = verdict(&record.status, &record.checks);
    write_json(&output.join("result.json"), &record)?;
    Ok(record)
}
fn verdict(status: &str, checks: &[CheckResult]) -> Option<bool> {
    if status != "completed" {
        Some(false)
    } else if checks.is_empty() {
        None
    } else {
        Some(checks.iter().all(|c| c.passed))
    }
}

fn execute(
    runtime: &OmpRuntime,
    spec: &Spec,
    record: &mut ResultRecord,
    log: &mut EventLog,
    answer: &mut String,
) -> Result<(), String> {
    let mut submitted = false;
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) != 0 {
            record.status = "interrupted".into();
            return Ok(());
        }
        if log.started.elapsed() >= Duration::from_secs(spec.timeout_secs) {
            record.status = "timed_out".into();
            record.error = Some("evaluation deadline exceeded".into());
            return Ok(());
        }
        let Some((_, event)) = runtime.try_event() else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        let kind = event.kind();
        match event {
            UiEvent::Ready {
                model,
                session_id,
                session_file,
                account,
                thinking_level,
                ..
            } => {
                record.actual_model = Some(model.clone());
                record.account = account;
                record.thinking_level = Some(thinking_level);
                record.session_id = session_id;
                record.session_file = session_file;
                log.append(kind, json!({"model":model,"session_id":record.session_id}))?;
                if !submitted {
                    if spec.account.is_some() && spec.account != record.account {
                        return Err(
                            "runtime did not confirm the requested account; prompt was not sent"
                                .into(),
                        );
                    }
                    if spec.thinking_level.is_some() && spec.thinking_level != record.thinking_level
                    {
                        return Err("runtime did not confirm the requested thinking level; prompt was not sent".into());
                    }
                    if model != spec.model {
                        return Err(format!(
                            "requested {}, runtime selected {model}",
                            spec.model
                        ));
                    }
                    if !runtime.prompt(spec.prompt.clone().into()) {
                        return Err("runtime rejected prompt".into());
                    }
                    submitted = true;
                    record.status = "running".into();
                }
            }
            UiEvent::AssistantStarted => {
                answer.clear();
                log.append(kind, Value::Null)?;
            }
            UiEvent::AssistantDelta(text) => {
                answer.push_str(&text);
                log.append(kind, json!({"text":text}))?;
            }
            UiEvent::AssistantFinished(text) => {
                if let Some(text) = text {
                    *answer = text;
                }
                log.append(kind, json!({"text":answer}))?;
            }
            UiEvent::InferenceReceipt(receipt) => log.append(kind, json!(receipt))?,
            UiEvent::ToolStarted {
                call_id,
                name,
                args,
                ..
            } => {
                record.tool_calls += 1;
                log.append(
                    kind,
                    json!({"call_id":call_id,"name":name,"arguments":args}),
                )?;
            }
            UiEvent::ToolArguments { call_id, delta } => {
                log.append(kind, json!({"call_id":call_id,"delta":delta}))?;
            }
            UiEvent::ToolUpdated {
                call_id,
                content,
                args,
            } => {
                log.append(
                    kind,
                    json!({"call_id":call_id,"content":content,"arguments":args}),
                )?;
            }
            UiEvent::ToolFinished(result) => {
                record.tool_errors += u64::from(result.is_error == Some(true));
                log.append(kind, json!({"call_id":result.call_id,"name":result.name,"is_error":result.is_error,"result":result.result}))?;
            }
            UiEvent::TurnFinished(outcome) => {
                match outcome {
                    TurnOutcome::Completed => record.status = "completed".into(),
                    TurnOutcome::Interrupted => record.status = "interrupted".into(),
                    TurnOutcome::Errored(error) => {
                        record.status = "error".into();
                        record.error = Some(error);
                    }
                }
                log.append(kind, json!({"status":record.status,"error":record.error}))?;
                return Ok(());
            }
            UiEvent::AuthenticationRequired { error, .. } => {
                record.status = "authentication_required".into();
                record.error = Some(error);
                log.append(kind, json!({"error":record.error}))?;
                return Ok(());
            }
            UiEvent::Failed(error) => {
                log.append(kind, json!({"error":error}))?;
                return Err(error);
            }
            UiEvent::Stopped => return Err("runtime stopped before turn completion".into()),
            UiEvent::HostToolCall { call, .. } => {
                runtime.answer_host_tool(
                    call,
                    Err("interactive host tools are unavailable during evals".into()),
                );
            }
            UiEvent::Notice(text) => log.append(kind, json!({"text":text}))?,
            _ => log.append(kind, Value::Null)?,
        }
    }
}

fn grade(
    check: &Check,
    index: usize,
    answer: &str,
    workspace: &Path,
    output: &Path,
    env: &std::collections::BTreeMap<String, String>,
) -> CheckResult {
    let result = match check {
        Check::AnswerContains { text } => {
            Ok((answer.contains(text), "answer substring check".into()))
        }
        Check::Command { argv, timeout_secs } => {
            run_grader(argv, *timeout_secs, index, workspace, output, env)
        }
    };
    match result {
        Ok((passed, detail)) => CheckResult {
            index,
            passed,
            detail,
        },
        Err(error) => CheckResult {
            index,
            passed: false,
            detail: error.to_string(),
        },
    }
}
/// A program found on PATH (the child's own, when the caller sets one) as an
/// absolute path. With a bare name and a changed environment, std forks and
/// execs instead of using `posix_spawn`, and the forked child briefly holds
/// every descriptor this process has open, including other threads' file
/// locks; an absolute path keeps the spawn atomic.
pub(crate) fn resolve_program(program: &str, env: &std::collections::BTreeMap<String, String>) -> PathBuf {
    if program.contains('/') {
        return PathBuf::from(program);
    }
    let path = env
        .get("PATH")
        .map(OsString::from)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(program))
}

pub(crate) fn run_grader(
    argv: &[String],
    timeout_secs: u64,
    index: usize,
    workspace: &Path,
    output: &Path,
    env: &std::collections::BTreeMap<String, String>,
) -> io::Result<(bool, String)> {
    let stdout = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output.join(format!("check-{index}.stdout")))?;
    let stderr = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output.join(format!("check-{index}.stderr")))?;
    let mut child = Command::new(resolve_program(&argv[0], env))
        .args(&argv[1..])
        .current_dir(workspace)
        .envs(env)
        .env("OMPEROR_EVAL_ANSWER_PATH", output.join("answer.txt"))
        .env("OMPEROR_EVAL_RUN_DIR", output)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0)
        .spawn()?;
    let started = Instant::now();
    let result = loop {
        if INTERRUPTED.load(Ordering::Relaxed) != 0 {
            break Ok((false, "grader interrupted".into()));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok((status.success(), status.to_string())),
            Ok(None) if started.elapsed() < Duration::from_secs(timeout_secs) => {
                thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => break Ok((false, "grader deadline exceeded".into())),
            Err(error) => break Err(error),
        }
    };
    // Each grader owns its process group; terminate descendants too, including
    // ones left running by a successful parent. Never target the app's group.
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: kill takes only an integer process-group id, not a pointer.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> Spec {
        serde_json::from_value(json!({"schema_version":1,"id":"test","workspace":std::env::current_dir().unwrap(),"model":"openai-codex/example","prompt":"work"})).unwrap()
    }
    #[test]
    fn rejects_ambiguous_or_nonreal_specs() {
        let mut s = spec();
        assert!(s.validate().is_ok());
        s.schema_version = 2;
        assert!(s.validate().is_err());
        s.schema_version = 1;
        s.model = "mock/deterministic".into();
        assert!(s.validate().is_err());
        s.model = "openai/test".into();
        s.timeout_secs = 0;
        assert!(s.validate().is_err());
        s.timeout_secs = 10;
        s.checks.push(Check::Command {
            argv: vec![],
            timeout_secs: 1,
        });
        assert!(s.validate().is_err());
        assert!(serde_json::from_value::<Spec>(json!({"schema_version":1,"id":"x","workspace":"/tmp","model":"openai/x","prompt":"x","typo":true})).is_err());
    }
    #[test]
    fn completion_is_not_success_without_a_grader() {
        assert_eq!(verdict("completed", &[]), None);
        assert_eq!(exit_code("completed", None), 3);
        let checks = vec![CheckResult {
            index: 0,
            passed: true,
            detail: String::new(),
        }];
        assert_eq!(verdict("error", &checks), Some(false));
        assert_eq!(verdict("completed", &checks), Some(true));
        assert_eq!(exit_code("completed", Some(false)), 1);
    }
    #[test]
    fn command_grader_checks_exit_and_deadline() {
        let dir = std::env::temp_dir().join(format!("omperor-eval-grader-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        assert!(
            !run_grader(
                &["/bin/sh".into(), "-c".into(), "exit 7".into()],
                1,
                0,
                &dir,
                &dir,
                &Default::default()
            )
            .unwrap()
            .0
        );
        let start = Instant::now();
        assert!(
            !run_grader(&["/bin/sleep".into(), "10".into()], 1, 1, &dir, &dir, &Default::default())
                .unwrap()
                .0
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        fs::remove_dir_all(dir).unwrap();
    }
}
