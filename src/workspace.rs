use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::perf;

const MAX_DIFF_BYTES: usize = 512 * 1024;

/// Git, by absolute path, which keeps the spawn atomic (see
/// `eval::resolve_program`): the `git` on PATH, as NixOS and others have no
/// `/usr/bin/git`. Found once, on a worker.
pub(crate) fn git() -> &'static Path {
    static GIT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    GIT.get_or_init(|| crate::eval::resolve_program("git", &Default::default()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceChange {
    pub(crate) status: String,
    pub(crate) path: PathBuf,
    /// Lines added and removed against HEAD, when they can be counted — a
    /// binary or a new directory has no line count.
    pub(crate) insertions: Option<u64>,
    pub(crate) deletions: Option<u64>,
}

impl WorkspaceChange {
    /// A one-word reading of the porcelain status.
    pub(crate) fn kind(&self) -> &'static str {
        status_label(&self.status)
    }
}

/// How the checked-out branch stands against its upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Upstream {
    pub(crate) ahead: u64,
    pub(crate) behind: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceSnapshot {
    pub(crate) project: PathBuf,
    pub(crate) repository_root: Option<PathBuf>,
    pub(crate) branch: Option<String>,
    pub(crate) changes: Vec<WorkspaceChange>,
    /// `None` when the branch has no upstream (or HEAD is detached).
    pub(crate) upstream: Option<Upstream>,
    /// The subject line of the commit at HEAD.
    pub(crate) head_subject: Option<String>,
}

impl WorkspaceSnapshot {
    /// Lines added and removed across every counted change.
    pub(crate) fn totals(&self) -> (u64, u64) {
        self.changes
            .iter()
            .fold((0, 0), |(added, removed), change| {
                (
                    added + change.insertions.unwrap_or(0),
                    removed + change.deletions.unwrap_or(0),
                )
            })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkspaceError {
    #[error("could not run git in {project}")]
    Git {
        project: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not read changed file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The change list was inspected before this path was removed.
    #[error("{path} no longer exists; it was removed after the last inspection")]
    Missing { path: PathBuf },
    #[error("git {command} failed in {project}: {message}")]
    Command {
        project: PathBuf,
        command: String,
        message: String,
    },
}

/// Where a directory sits in git: the repository that contains it, the branch
/// checked out there when it is not detached, and the exact commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchPoint {
    pub(crate) repository_root: PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) head: String,
}

/// Returns `None` when the directory is not inside a git repository.
pub(crate) fn branch_point(directory: &Path) -> Result<Option<BranchPoint>, WorkspaceError> {
    let root = git_output(directory, &["rev-parse", "--show-toplevel"])?;
    if !root.status.success() {
        return Ok(None);
    }
    let head = git_output(directory, &["rev-parse", "HEAD"])?;
    if !head.status.success() {
        return Err(command_error(directory, "rev-parse HEAD", &head.stderr));
    }
    let branch = git_output(directory, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    Ok(Some(BranchPoint {
        repository_root: PathBuf::from(String::from_utf8_lossy(&root.stdout).trim()),
        head: String::from_utf8_lossy(&head.stdout).trim().to_owned(),
        branch: branch
            .status
            .success()
            .then(|| String::from_utf8_lossy(&branch.stdout).trim().to_owned())
            .filter(|branch| !branch.is_empty()),
    }))
}

/// Applies git's branch-name rules (`git check-ref-format --branch`) without
/// spawning git, so a prompt can validate on the main thread.
pub(crate) fn branch_name_is_valid(name: &str) -> bool {
    if name.is_empty()
        || name == "@"
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name.contains("\\")
    {
        return false;
    }
    if name.chars().any(|character| {
        character.is_ascii_control()
            || matches!(character, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\x7f')
    }) {
        return false;
    }
    name.split('/').all(|component| {
        !component.is_empty() && !component.starts_with('.') && !component.ends_with(".lock")
    })
}

/// Creates `branch` at the commit checked out in `directory` and checks it out
/// in a new worktree at `path`, which must not exist yet.
pub(crate) fn add_worktree(
    directory: &Path,
    branch: &str,
    path: &Path,
) -> Result<(), WorkspaceError> {
    let _span = perf::span_with(
        "gui.workspace.add_worktree",
        || serde_json::json!({ "directory": directory, "branch": branch, "path": path }),
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| WorkspaceError::Read {
            path: parent.to_owned(),
            source,
        })?;
    }
    let output = Command::new(git())
        .arg("-C")
        .arg(directory)
        .args(["worktree", "add", "-b", branch])
        .arg(path)
        .arg("HEAD")
        .output()
        .map_err(|source| WorkspaceError::Git {
            project: directory.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(command_error(directory, "worktree add", &output.stderr));
    }
    Ok(())
}

/// What became of the worktree a deleted session was forked into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeRemoval {
    /// Removed. Its branch, named when it had one, is kept with every commit.
    Removed { branch: Option<String> },
    /// It was already gone from disk.
    Missing,
    /// Left in place, and why.
    Kept(String),
}

/// Removes the worktree at `path` if git reports nothing uncommitted or
/// untracked in it; ignored files (build output) go with it, as they do for
/// `git worktree remove`. The branch is never deleted, so no commit is lost.
/// Runs git: worker only.
pub(crate) fn remove_clean_worktree(path: &Path) -> WorktreeRemoval {
    let _span = perf::span_with("gui.workspace.remove_worktree", || serde_json::json!({ "path": path }));
    if !path.is_dir() {
        return WorktreeRemoval::Missing;
    }
    let kept = |error: WorkspaceError| WorktreeRemoval::Kept(error.to_string());
    let status = match git_output(path, &["status", "--porcelain", "--untracked-files=all"]) {
        Ok(output) if output.status.success() => output.stdout,
        Ok(output) => return kept(command_error(path, "status", &output.stderr)),
        Err(error) => return kept(error),
    };
    if !status.iter().all(u8::is_ascii_whitespace) {
        return WorktreeRemoval::Kept("it has uncommitted or untracked changes".into());
    }
    let branch = git_output(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|branch| !branch.is_empty());
    let common = match git_output(path, &["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        Ok(output) if output.status.success() => {
            PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        }
        Ok(output) => return kept(command_error(path, "rev-parse", &output.stderr)),
        Err(error) => return kept(error),
    };
    // Git is asked from the repository the worktree belongs to, not from
    // inside the directory it is about to delete.
    let repository = match common.file_name() {
        Some(name) if name == ".git" => common.parent().map_or(common.clone(), Path::to_path_buf),
        _ => common,
    };
    match git_output_path(&repository, &["worktree", "remove"], path) {
        Ok(output) if output.status.success() => WorktreeRemoval::Removed { branch },
        Ok(output) => kept(command_error(&repository, "worktree remove", &output.stderr)),
        Err(error) => kept(error),
    }
}

fn command_error(project: &Path, command: &str, stderr: &[u8]) -> WorkspaceError {
    let message = String::from_utf8_lossy(stderr).trim().to_owned();
    WorkspaceError::Command {
        project: project.to_owned(),
        command: command.to_owned(),
        message: if message.is_empty() {
            "no diagnostic output".to_owned()
        } else {
            message
        },
    }
}

impl WorkspaceError {
    /// Whether the error means the snapshot no longer matches the working
    /// tree, so the caller should inspect the workspace again.
    pub(crate) fn is_stale(&self) -> bool {
        matches!(self, Self::Missing { .. })
    }
}

pub(crate) fn inspect(project: &Path) -> Result<WorkspaceSnapshot, WorkspaceError> {
    let _span = perf::span_with(
        "gui.workspace.inspect",
        || serde_json::json!({ "project": project }),
    );
    let root = git_output(project, &["rev-parse", "--show-toplevel"])?;
    if !root.status.success() {
        return Ok(WorkspaceSnapshot {
            project: project.to_owned(),
            repository_root: None,
            branch: None,
            changes: Vec::new(),
            upstream: None,
            head_subject: None,
        });
    }
    let repository_root = PathBuf::from(String::from_utf8_lossy(&root.stdout).trim());
    let branch = git_output(&repository_root, &["branch", "--show-current"])?;
    let branch = branch
        .status
        .success()
        .then(|| String::from_utf8_lossy(&branch.stdout).trim().to_owned());
    let branch = branch.filter(|branch| !branch.is_empty());
    let status = git_output(
        &repository_root,
        // `all` lists every untracked file; `normal` collapses a new
        // directory into one `assets/` entry that has no diff to show.
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut changes = if status.status.success() {
        parse_porcelain(&status.stdout)
    } else {
        Vec::new()
    };
    // Line counts: git's for tracked changes against HEAD, a line count of
    // the file itself for an untracked one.
    let numstat = git_output(&repository_root, &["diff", "--numstat", "-z", "HEAD", "--"])?;
    let counts = if numstat.status.success() {
        parse_numstat(&numstat.stdout)
    } else {
        HashMap::new()
    };
    for change in &mut changes {
        if change.status == "??" {
            let lines = untracked_line_count(&repository_root.join(&change.path));
            change.insertions = lines;
            change.deletions = lines.map(|_| 0);
        } else if let Some((insertions, deletions)) = counts.get(&change.path) {
            change.insertions = *insertions;
            change.deletions = *deletions;
        }
    }
    let upstream = upstream_status(&repository_root)?;
    let head = git_output(&repository_root, &["log", "-1", "--format=%s"])?;
    let head_subject = head
        .status
        .success()
        .then(|| String::from_utf8_lossy(&head.stdout).trim().to_owned())
        .filter(|subject| !subject.is_empty());
    Ok(WorkspaceSnapshot {
        project: project.to_owned(),
        repository_root: Some(repository_root),
        branch,
        changes,
        upstream,
        head_subject,
    })
}

/// Commits ahead of and behind the upstream branch, or `None` when there is
/// no upstream to compare with.
fn upstream_status(root: &Path) -> Result<Option<Upstream>, WorkspaceError> {
    let output = git_output(
        root,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.split_whitespace();
    let (Some(behind), Some(ahead)) = (fields.next(), fields.next()) else {
        return Ok(None);
    };
    Ok(Some(Upstream {
        ahead: ahead.parse().unwrap_or(0),
        behind: behind.parse().unwrap_or(0),
    }))
}

/// Lines added and removed per path from `git diff --numstat -z`: each
/// record is `added\tremoved\tpath`, a binary's counts are `-`, and a
/// rename's path is empty and followed by the old and new paths.
fn parse_numstat(output: &[u8]) -> HashMap<PathBuf, (Option<u64>, Option<u64>)> {
    let mut counts = HashMap::new();
    let mut records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        let text = String::from_utf8_lossy(record);
        let mut fields = text.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            let _old = records.next();
            let Some(new) = records.next() else {
                break;
            };
            PathBuf::from(String::from_utf8_lossy(new).into_owned())
        } else {
            PathBuf::from(path)
        };
        counts.insert(path, (added.parse().ok(), removed.parse().ok()));
    }
    counts
}

/// The lines an untracked text file would add; `None` for a directory or a
/// binary.
fn untracked_line_count(path: &Path) -> Option<u64> {
    if path.is_dir() {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes[..bytes.len().min(MAX_DIFF_BYTES)].contains(&0) {
        return None;
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    let unterminated = u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    Some(newlines + unterminated)
}

pub(crate) fn diff(
    snapshot: &WorkspaceSnapshot,
    change: &WorkspaceChange,
) -> Result<String, WorkspaceError> {
    let _span = perf::span_with(
        "gui.workspace.diff",
        || serde_json::json!({ "path": change.path }),
    );
    let Some(root) = snapshot.repository_root.as_deref() else {
        return Ok("This workspace is not a Git repository.".into());
    };
    if change.status == "??" {
        return untracked_preview(&root.join(&change.path));
    }
    let output = git_output_path(root, &["diff", "HEAD", "--"], &change.path)?;
    if output.status.success() && !output.stdout.is_empty() {
        return Ok(truncate_bytes(&output.stdout));
    }
    let staged = git_output_path(root, &["diff", "--cached", "--"], &change.path)?;
    if staged.status.success() && !staged.stdout.is_empty() {
        return Ok(truncate_bytes(&staged.stdout));
    }
    Ok(format!(
        "{} is reported as changed, but Git did not produce a textual diff.",
        change.path.display()
    ))
}

fn git_output(project: &Path, arguments: &[&str]) -> Result<std::process::Output, WorkspaceError> {
    Command::new(git())
        .arg("-C")
        .arg(project)
        .args(arguments)
        .output()
        .map_err(|source| WorkspaceError::Git {
            project: project.to_owned(),
            source,
        })
}

fn git_output_path(
    project: &Path,
    arguments: &[&str],
    path: &Path,
) -> Result<std::process::Output, WorkspaceError> {
    Command::new(git())
        .arg("-C")
        .arg(project)
        .args(arguments)
        .arg(path)
        .output()
        .map_err(|source| WorkspaceError::Git {
            project: project.to_owned(),
            source,
        })
}

fn parse_porcelain(output: &[u8]) -> Vec<WorkspaceChange> {
    let mut records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut changes = Vec::new();
    while let Some(record) = records.next() {
        if record.len() < 4 || record[2] != b' ' {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]).into_owned();
        let path = PathBuf::from(String::from_utf8_lossy(&record[3..]).into_owned());
        if status.bytes().any(|byte| matches!(byte, b'R' | b'C')) && records.next().is_none() {
            break;
        }
        changes.push(WorkspaceChange {
            status,
            path,
            insertions: None,
            deletions: None,
        });
    }
    changes
}

fn status_label(status: &str) -> &'static str {
    match status {
        "??" => "New",
        "!!" => "Ignored",
        value if value.contains('D') => "Deleted",
        value if value.contains('R') => "Renamed",
        value if value.contains('A') => "Added",
        value if value.contains('U') => "Conflict",
        _ => "Modified",
    }
}

fn untracked_preview(path: &Path) -> Result<String, WorkspaceError> {
    if path.is_dir() {
        return untracked_directory_preview(path);
    }
    let bytes = fs::read(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            WorkspaceError::Missing {
                path: path.to_owned(),
            }
        } else {
            WorkspaceError::Read {
                path: path.to_owned(),
                source,
            }
        }
    })?;
    if bytes.iter().take(MAX_DIFF_BYTES).any(|byte| *byte == 0) {
        return Ok(format!("New binary file: {}", path.display()));
    }
    Ok(format!(
        "New file: {}\n\n{}",
        path.display(),
        truncate_bytes(&bytes)
    ))
}

/// Lists a new directory's entries instead of failing to read it as a
/// file; Git reports one only when every entry is untracked.
fn untracked_directory_preview(path: &Path) -> Result<String, WorkspaceError> {
    let entries = fs::read_dir(path).map_err(|source| WorkspaceError::Read {
        path: path.to_owned(),
        source,
    })?;
    let mut names = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let mut name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                name.push('/');
            }
            name
        })
        .collect::<Vec<_>>();
    names.sort();
    let listing = if names.is_empty() {
        "(empty)".to_owned()
    } else {
        names.join("\n")
    };
    Ok(format!("New directory: {}\n\n{listing}", path.display()))
}

fn truncate_bytes(bytes: &[u8]) -> String {
    let truncated = bytes.len() > MAX_DIFF_BYTES;
    let bytes = &bytes[..bytes.len().min(MAX_DIFF_BYTES)];
    let mut value = String::from_utf8_lossy(bytes).into_owned();
    if truncated {
        value.push_str("\n\n… diff truncated by Omperor …");
    }
    value
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        WorktreeRemoval, add_worktree, branch_name_is_valid, branch_point, diff, inspect,
        parse_porcelain, remove_clean_worktree, status_label,
    };

    #[test]
    fn branch_names_follow_git_check_ref_format_rules() {
        for valid in [
            "omperor/feature",
            "fix-1",
            "a.b",
            "release/v1.2",
            "@x",
            "x@y",
        ] {
            assert!(branch_name_is_valid(valid), "{valid} should be valid");
        }
        for invalid in [
            "",
            "@",
            "-flag",
            "/lead",
            "trail/",
            "dot.",
            "a..b",
            "a//b",
            "a b",
            "a~b",
            "a^b",
            "a:b",
            "a?b",
            "a*b",
            "a[b",
            "a\\b",
            "a@{b",
            ".hidden",
            "a/.b",
            "x.lock",
            "a/b.lock",
            "tab\tchar",
        ] {
            assert!(
                !branch_name_is_valid(invalid),
                "{invalid:?} should be invalid"
            );
        }
    }

    static NEXT_REPOSITORY: AtomicU64 = AtomicU64::new(0);

    fn git(directory: &Path, arguments: &[&str]) {
        let status = Command::new(super::git())
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .env("GIT_AUTHOR_NAME", "Omperor Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Omperor Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git process");
        assert!(status.success(), "git {arguments:?} failed");
    }

    #[test]
    fn forks_a_worktree_on_a_new_branch_from_the_current_commit() {
        let sequence = NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "omperor-worktree-{}-{sequence}",
            std::process::id()
        ));
        let repository = scratch.join("repo");
        fs::create_dir_all(&repository).expect("repository fixture directory");
        assert_eq!(branch_point(&repository).expect("probe"), None);
        git(&repository, &["init", "--quiet", "-b", "main"]);
        fs::write(repository.join("notes.txt"), "base\n").expect("fixture file");
        git(&repository, &["add", "notes.txt"]);
        git(&repository, &["commit", "--quiet", "-m", "base"]);

        let base = branch_point(&repository)
            .expect("probe")
            .expect("repository is git");
        assert_eq!(base.branch.as_deref(), Some("main"));
        assert_eq!(base.head.len(), 40);

        let worktree = scratch.join("worktrees").join("feature");
        add_worktree(&repository, "omperor/feature", &worktree).expect("worktree");
        let forked = branch_point(&worktree)
            .expect("probe")
            .expect("worktree is git");
        assert_eq!(forked.branch.as_deref(), Some("omperor/feature"));
        assert_eq!(forked.head, base.head);
        assert!(worktree.join("notes.txt").is_file());
        // The same branch cannot be checked out twice.
        assert!(add_worktree(&repository, "omperor/feature", &scratch.join("again")).is_err());
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn removes_only_a_clean_worktree_and_always_keeps_its_branch() {
        let sequence = NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "omperor-worktree-removal-{}-{sequence}",
            std::process::id()
        ));
        let repository = scratch.join("repo");
        fs::create_dir_all(&repository).expect("repository fixture directory");
        git(&repository, &["init", "--quiet", "-b", "main"]);
        fs::write(repository.join(".gitignore"), "target/\n").expect("fixture file");
        fs::write(repository.join("notes.txt"), "base\n").expect("fixture file");
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "--quiet", "-m", "base"]);

        let dirty = scratch.join("worktrees").join("dirty");
        add_worktree(&repository, "omperor/dirty", &dirty).expect("worktree");
        fs::write(dirty.join("scratch.txt"), "untracked work\n").expect("fixture file");
        assert!(matches!(remove_clean_worktree(&dirty), WorktreeRemoval::Kept(_)));
        assert!(dirty.join("scratch.txt").is_file());

        let clean = scratch.join("worktrees").join("clean");
        add_worktree(&repository, "omperor/clean", &clean).expect("worktree");
        fs::write(clean.join("notes.txt"), "committed on the fork\n").expect("fixture file");
        git(&clean, &["commit", "--quiet", "-am", "fork work"]);
        // Ignored build output does not keep a worktree.
        fs::create_dir_all(clean.join("target")).expect("fixture dir");
        fs::write(clean.join("target").join("out.o"), "object").expect("fixture file");
        assert_eq!(
            remove_clean_worktree(&clean),
            WorktreeRemoval::Removed { branch: Some("omperor/clean".into()) }
        );
        assert!(!clean.exists());
        // The branch, and the commit only it had, survive.
        let log = Command::new(super::git())
            .arg("-C")
            .arg(&repository)
            .args(["log", "--format=%s", "omperor/clean", "-1"])
            .output()
            .expect("git log");
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "fork work");
        assert_eq!(remove_clean_worktree(&clean), WorktreeRemoval::Missing);
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn parses_nul_delimited_status_with_spaces_and_renames() {
        // In -z mode Git emits a rename's destination first, then the source.
        let changes = parse_porcelain(b" M src/main file.rs\0R  new.rs\0old.rs\0?? notes.txt\0");

        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].status, " M");
        assert_eq!(changes[0].path, PathBuf::from("src/main file.rs"));
        assert_eq!(changes[1].status, "R ");
        assert_eq!(changes[1].path, PathBuf::from("new.rs"));
        assert_eq!(changes[2].status, "??");
        assert_eq!(changes[2].path, PathBuf::from("notes.txt"));
    }

    #[test]
    fn parses_numstat_including_binaries_and_renames() {
        let counts = super::parse_numstat(
            b"12\t3\tsrc/main.rs\x00-\t-\tassets/icon.png\x005\t0\t\x00old.rs\x00new.rs\x00",
        );
        assert_eq!(counts[Path::new("src/main.rs")], (Some(12), Some(3)));
        assert_eq!(counts[Path::new("assets/icon.png")], (None, None));
        assert_eq!(counts[Path::new("new.rs")], (Some(5), Some(0)));
        assert!(!counts.contains_key(Path::new("old.rs")));
    }

    #[test]
    fn maps_porcelain_states_to_readable_labels() {
        assert_eq!(status_label("??"), "New");
        assert_eq!(status_label(" M"), "Modified");
        assert_eq!(status_label("D "), "Deleted");
        assert_eq!(status_label("UU"), "Conflict");
    }

    #[test]
    fn inspects_a_repository_and_previews_an_untracked_file() {
        let sequence = NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "omperor-workspace-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&scratch).expect("workspace fixture directory");
        let initialized = Command::new(super::git())
            .arg("init")
            .arg("--quiet")
            .arg(&scratch)
            .status()
            .expect("git init process");
        assert!(initialized.success());
        fs::write(scratch.join("notes.txt"), "native Rust workspace\n")
            .expect("workspace fixture file");
        fs::create_dir_all(scratch.join("assets")).expect("workspace fixture directory");
        fs::write(scratch.join("assets/gate.svg"), "<svg/>\n").expect("workspace fixture asset");
        fs::write(scratch.join("assets/gate-mono.svg"), "<svg/>\n")
            .expect("workspace fixture asset");

        let snapshot = inspect(&scratch).expect("workspace inspection");
        let change = snapshot
            .changes
            .iter()
            .find(|change| change.path == Path::new("notes.txt"))
            .expect("untracked file is reported");
        assert_eq!(change.status, "??");
        assert!(
            diff(&snapshot, change)
                .expect("untracked preview")
                .contains("native Rust workspace")
        );
        // A new directory is expanded into its files rather than reported as
        // one unreadable `assets/` entry.
        let assets = snapshot
            .changes
            .iter()
            .filter(|change| change.path.starts_with("assets"))
            .map(|change| change.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            assets,
            [
                PathBuf::from("assets/gate-mono.svg"),
                PathBuf::from("assets/gate.svg")
            ]
        );
        for change in snapshot
            .changes
            .iter()
            .filter(|change| change.path.starts_with("assets"))
        {
            assert!(
                diff(&snapshot, change)
                    .expect("asset preview")
                    .contains("<svg/>")
            );
        }
        // Counts: the untracked file's own lines; a directory has none.
        assert_eq!(change.insertions, Some(1));
        assert_eq!(change.deletions, Some(0));
        assert_eq!(snapshot.totals(), (3, 0));
        assert_eq!(snapshot.upstream, None);
        let directory = super::WorkspaceChange {
            status: "??".into(),
            path: PathBuf::from("assets"),
            insertions: None,
            deletions: None,
        };
        let listing = diff(&snapshot, &directory).expect("directory preview");
        assert!(listing.starts_with("New directory:"));
        assert!(listing.contains("gate-mono.svg") && listing.contains("gate.svg"));
        // A file removed after inspection reports a stale snapshot instead of
        // a raw read failure, so the pane can inspect the workspace again.
        fs::remove_dir_all(scratch.join("assets")).expect("remove fixture assets");
        let gone = snapshot
            .changes
            .iter()
            .find(|change| change.path == Path::new("assets/gate.svg"))
            .expect("removed file is still in the stale snapshot");
        let error = diff(&snapshot, gone).expect_err("removed file cannot be previewed");
        assert!(error.is_stale(), "{error}");
        assert!(error.to_string().contains("no longer exists"));

        fs::remove_dir_all(scratch).expect("workspace fixture cleanup");
    }
}
