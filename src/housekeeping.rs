//! Deleting sessions: every file Omperor and OMP keep beside a session's
//! journal, the sub-agent journals only it refers to, its registry rows and
//! metrics lines, and a clean worktree it was forked into. Nothing else is
//! touched: a sub-agent journal another session also names stays, a
//! worktree with changes stays, and no branch is ever deleted.
//!
//! Everything here reads the disk or runs git, so it runs on a worker.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
};

use omp_journal::{Journal, kind};

use crate::{journal, metrics, perf, preferences, workspace};

/// One session to delete, as the main thread knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) id: String,
    pub(crate) title: String,
    /// Whether the session has a journal in the sessions directory. Drafts
    /// and the mock backend's sessions have none.
    pub(crate) journal: bool,
    /// A worktree Omperor created for this session that no other session
    /// uses: removed if git finds it clean.
    pub(crate) worktree: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub(crate) sessions_dir: PathBuf,
    pub(crate) database: PathBuf,
    pub(crate) targets: Vec<Target>,
    pub(crate) metrics: Option<metrics::Editor>,
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) removed: Vec<Removed>,
    pub(crate) failed: Vec<Failed>,
    /// Sub-agent journals left because another session still names them.
    pub(crate) shared_children: usize,
    /// Things that went wrong after the sessions themselves were removed.
    pub(crate) warnings: Vec<String>,
}

impl Report {
    pub(crate) fn bytes(&self) -> u64 {
        self.removed.iter().map(|removed| removed.bytes).sum()
    }

    /// One line for the status bar or an agent: what went, and what stayed.
    pub(crate) fn summary(&self) -> String {
        let mut parts = Vec::new();
        match self.removed.len() {
            0 => parts.push("No sessions were deleted".to_owned()),
            1 => parts.push(format!("Deleted “{}”", self.removed[0].title)),
            count => parts.push(format!("Deleted {count} sessions")),
        }
        let bytes = self.bytes();
        if bytes > 0 {
            parts.push(format!("freed {}", byte_text(bytes)));
        }
        let worktrees = self
            .removed
            .iter()
            .filter(|removed| {
                matches!(
                    removed.worktree,
                    Some((_, workspace::WorktreeRemoval::Removed { .. }))
                )
            })
            .count();
        if worktrees > 0 {
            parts.push(format!(
                "removed {worktrees} clean worktree{}",
                plural(worktrees)
            ));
        }
        let kept = self
            .removed
            .iter()
            .filter(|removed| {
                matches!(
                    removed.worktree,
                    Some((_, workspace::WorktreeRemoval::Kept(_)))
                )
            })
            .count();
        if kept > 0 {
            parts.push(format!("kept {kept} worktree{}", plural(kept)));
        }
        if !self.failed.is_empty() {
            parts.push(format!("{} could not be deleted", self.failed.len()));
        }
        parts.join(" • ")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Removed {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) bytes: u64,
    /// Sub-agent journals deleted with it.
    pub(crate) children: usize,
    pub(crate) worktree: Option<(PathBuf, workspace::WorktreeRemoval)>,
}

#[derive(Debug, Clone)]
pub(crate) struct Failed {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) reason: String,
}

pub(crate) fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// A byte count for people: "812 KB", "3.4 MB".
pub(crate) fn byte_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The journal a session or sub-agent with this id keeps in `dir`. OMP
/// names a sub-agent's journal after its id with anything but ASCII
/// letters, digits, `-`, and `_` replaced by `_`.
pub(crate) fn journal_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.{}", stem(id), journal::JOURNAL_EXTENSION))
}

pub(crate) fn stem(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// The journal's sidecar lock: OMP holds it for as long as a child has the
/// session open.
fn lock_path(journal: &Path) -> PathBuf {
    let mut name = journal.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    journal.with_file_name(name)
}

/// Names in the sessions directory that every session shares: the metrics
/// and frame logs, the blob (attachment) store, and OMP's scratch space. A
/// sub-agent's journal is named by the model, so one called `metrics` or
/// `blobs` must never take these with it.
const SHARED_NAMES: [&str; 4] = ["metrics", "frames", "blobs", "tmp"];

/// Every file kept beside one journal: the journal and its lock, the
/// transcript heights, the session's `local://` folder, and the stubs and
/// transcripts an older OMP wrote under the same id. A stem that collides with
/// a shared name keeps only the files that are unambiguously its own.
pub(crate) fn files_for(dir: &Path, stem: &str) -> Vec<PathBuf> {
    let journal = dir.join(format!("{stem}.{}", journal::JOURNAL_EXTENSION));
    let mut files = vec![
        crate::saved_heights::path_for(&journal),
        dir.join(format!("{stem}.heights.tmp")),
        dir.join(format!("{stem}.rpc.json")),
    ];
    if !stem.is_empty() && !stem.starts_with('.') && !SHARED_NAMES.contains(&stem) {
        files.push(dir.join(format!("{stem}.jsonl")));
        files.push(dir.join(stem));
    }
    files.push(journal.clone());
    files.push(lock_path(&journal));
    files
}

/// A journal's writer lock, held so no OMP child can open the session while
/// its files are copied or removed.
#[derive(Debug)]
pub(crate) struct Lease {
    _file: File,
}

/// Takes a journal's writer lock, waiting at most `LEASE_RETRIES` short
/// intervals. Fails if an OMP child, in this Omperor or another, has the
/// session open. On a worker: it can sleep.
pub(crate) fn lease(journal: &Path) -> Result<Lease, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path(journal))
        .map_err(|error| format!("could not lock {}: {error}", journal.display()))?;
    let mut retries = 0;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Lease { _file: file }),
            // A process that another thread is starting holds a copy of every
            // descriptor until it runs its program, so a lock that was just
            // released can look held for a moment. An OMP child holds its
            // lock for as long as the session is open; a short wait tells
            // the two apart.
            Err(TryLockError::WouldBlock) if retries < LEASE_RETRIES => {
                retries += 1;
                std::thread::sleep(LEASE_RETRY_INTERVAL);
            }
            Err(TryLockError::WouldBlock) => return Err("an OMP process has it open".into()),
            Err(TryLockError::Error(error)) => {
                return Err(format!("could not lock {}: {error}", journal.display()));
            }
        }
    }
}

const LEASE_RETRIES: u32 = 5;
const LEASE_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// The sub-agent ids a journal names, on every branch of its history, so a
/// rewind cannot bring back a reference to a journal that was deleted.
pub(crate) fn spawned(journal: &Path) -> Result<Vec<String>, String> {
    let entries = Journal::scan(journal).map_err(|error| error.to_string())?;
    Ok(entries
        .iter()
        .filter(|entry| {
            entry.kind.name.as_str() == kind::PATCH && entry.label.as_deref() == Some("jobs.insert")
        })
        .flat_map(|entry| journal::inserted_subagent_ids(entry.data.as_str()))
        .collect())
}

/// Which journals each journal in `dir` names as its sub-agents, by stem.
/// Fails if any journal cannot be read: what it names is then unknown, and
/// a shared journal must not be mistaken for an unshared one.
pub(crate) fn graph(dir: &Path) -> Result<HashMap<String, Vec<String>>, String> {
    let _span = perf::span("gui.housekeeping.graph");
    let mut graph = HashMap::new();
    let entries =
        fs::read_dir(dir).map_err(|error| format!("could not list {}: {error}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path
            .extension()
            .is_none_or(|extension| extension != journal::JOURNAL_EXTENSION)
        {
            continue;
        }
        let Some(parent) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let children = spawned(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        if !children.is_empty() {
            graph.insert(
                parent.to_owned(),
                children.iter().map(|id| stem(id)).collect(),
            );
        }
    }
    Ok(graph)
}

/// The descendants of `roots` that no journal outside them names, so they
/// can go with them, and the count of descendants that must stay because
/// another journal still names them (and so do their own children).
pub(crate) fn exclusive_descendants(
    graph: &HashMap<String, Vec<String>>,
    roots: &HashSet<String>,
) -> (HashSet<String>, usize) {
    let mut descendants = HashSet::new();
    let mut queue: VecDeque<&String> = roots.iter().collect();
    while let Some(parent) = queue.pop_front() {
        for child in graph.get(parent).into_iter().flatten() {
            if !roots.contains(child) && descendants.insert(child.clone()) {
                queue.push_back(child);
            }
        }
    }
    let mut owned = descendants.clone();
    retain_exclusive(graph, roots, &mut owned);
    let shared = descendants.len() - owned.len();
    (owned, shared)
}

/// Prune candidates again whenever a lock or removal fails. A journal
/// that stays protects all its descendants, including through other roots.
pub(crate) fn retain_exclusive(
    graph: &HashMap<String, Vec<String>>,
    roots: &HashSet<String>,
    owned: &mut HashSet<String>,
) {
    let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
    for (parent, children) in graph {
        for child in children {
            parents
                .entry(child.as_str())
                .or_default()
                .push(parent.as_str());
        }
    }
    // Keeping one journal makes everything it names referenced from
    // outside, so prune until nothing changes.
    loop {
        let outside: Vec<String> = owned
            .iter()
            .filter(|child| {
                parents
                    .get(child.as_str())
                    .into_iter()
                    .flatten()
                    .any(|parent| !roots.contains(*parent) && !owned.contains(*parent))
            })
            .cloned()
            .collect();
        if outside.is_empty() {
            break;
        }
        for child in outside {
            owned.remove(&child);
        }
    }
}

/// Bytes a path holds, walking a folder.
pub(crate) fn size_of(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|entry| size_of(&entry.path())).sum())
        .unwrap_or(0)
}

/// Removes every file kept for one stem, the lock last. Returns the bytes
/// freed. A missing file is not an error; a journal that cannot be removed
/// is, and leaves the lock in place.
pub(crate) fn remove_stem(dir: &Path, stem: &str) -> io::Result<u64> {
    let mut freed = 0;
    for path in files_for(dir, stem) {
        let size = size_of(&path);
        let removed = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path),
            Ok(_) => fs::remove_file(&path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => Err(error),
        };
        match removed {
            Ok(()) => freed += size,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(freed)
}

/// Remove descendants only after their parents have gone. On failure,
/// protect everything still referenced by the journal that stayed.
pub(crate) fn remove_descendants(
    dir: &Path,
    graph: &HashMap<String, Vec<String>>,
    mut deleted: HashSet<String>,
    mut candidates: HashSet<String>,
) -> (HashMap<String, u64>, Vec<String>) {
    let mut removed = HashMap::new();
    let mut warnings = Vec::new();
    while !candidates.is_empty() {
        retain_exclusive(graph, &deleted, &mut candidates);
        let pending_children: HashSet<&String> = candidates
            .iter()
            .flat_map(|parent| graph.get(parent).into_iter().flatten())
            .collect();
        let ready: Vec<String> = candidates
            .iter()
            .filter(|child| !pending_children.contains(child))
            .cloned()
            .collect();
        if ready.is_empty() {
            if !candidates.is_empty() {
                warnings.push("Kept sub-agent transcripts with cyclic references".into());
            }
            break;
        }
        for child in ready {
            candidates.remove(&child);
            match remove_stem(dir, &child) {
                Ok(bytes) => {
                    deleted.insert(child.clone());
                    removed.insert(child, bytes);
                }
                Err(error) => warnings.push(format!(
                    "Could not remove sub-agent transcript {child}: {error}"
                )),
            }
        }
    }
    (removed, warnings)
}

/// Deletes requested sessions while keeping files still used by sessions
/// that could not be removed.
pub(crate) fn delete(request: Request) -> Report {
    let _span = perf::span_with(
        "gui.housekeeping.delete",
        || serde_json::json!({ "sessions": request.targets.len() }),
    );
    let dir = &request.sessions_dir;
    let mut report = Report::default();
    let mut leases = Vec::new();
    let mut accepted = Vec::new();
    for target in &request.targets {
        if target.journal {
            match lease(&journal_path(dir, &target.id)) {
                Ok(lease) => leases.push(lease),
                Err(reason) => {
                    report.failed.push(Failed {
                        id: target.id.clone(),
                        title: target.title.clone(),
                        reason,
                    });
                    continue;
                }
            }
        }
        accepted.push(target);
    }
    let roots: HashSet<String> = accepted
        .iter()
        .filter(|target| target.journal)
        .map(|target| stem(&target.id))
        .collect();
    // Only a session that spawned sub-agents needs the whole directory read.
    let has_children = roots.iter().any(|root| {
        spawned(&dir.join(format!("{root}.{}", journal::JOURNAL_EXTENSION)))
            .is_ok_and(|children| !children.is_empty())
    });
    let (mut children, graph) = if has_children {
        match graph(dir) {
            Ok(graph) => {
                let (owned, shared) = exclusive_descendants(&graph, &roots);
                report.shared_children = shared;
                (owned, graph)
            }
            Err(error) => {
                report.warnings.push(format!(
                    "Sub-agent transcripts were kept because a journal could not be read: {error}"
                ));
                (HashSet::new(), HashMap::new())
            }
        }
    } else {
        (HashSet::new(), HashMap::new())
    };
    // A sub-agent still running elsewhere keeps its journal, and so does
    // everything it names.
    let candidates = children.len();
    children.retain(|child| {
        match lease(&dir.join(format!("{child}.{}", journal::JOURNAL_EXTENSION))) {
            Ok(lease) => {
                leases.push(lease);
                true
            }
            Err(_) => false,
        }
    });
    retain_exclusive(&graph, &roots, &mut children);
    report.shared_children += candidates - children.len();
    let mut deleted_roots = HashSet::new();
    let mut deleted_ids = Vec::new();
    for target in accepted {
        let mut bytes = 0;
        if target.journal {
            let root = stem(&target.id);
            match remove_stem(dir, &root) {
                Ok(freed) => bytes += freed,
                Err(error) => {
                    report.failed.push(Failed {
                        id: target.id.clone(),
                        title: target.title.clone(),
                        reason: format!("could not remove its files: {error}"),
                    });
                    continue;
                }
            }
            deleted_roots.insert(root);
        }
        deleted_ids.push(target.id.clone());
        report.removed.push(Removed {
            id: target.id.clone(),
            title: target.title.clone(),
            bytes,
            children: 0,
            worktree: None,
        });
    }
    let (mut freed, warnings) = remove_descendants(dir, &graph, deleted_roots, children);
    report.warnings.extend(warnings);
    let deleted_children: HashSet<String> = freed.keys().cloned().collect();
    // Credit each deleted descendant to the first deleted session that
    // names it, once all parent removals have succeeded or failed.
    for removed in &mut report.removed {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::from([stem(&removed.id)]);
        while let Some(parent) = queue.pop_front() {
            for child in graph.get(&parent).into_iter().flatten() {
                if visited.insert(child) {
                    if let Some(bytes) = freed.remove(child) {
                        removed.bytes += bytes;
                        removed.children += 1;
                    }
                    queue.push_back(child.clone());
                }
            }
        }
    }
    // Worktree eligibility was checked for the accepted batch. If any of
    // those sessions stayed, its directory may still be in use.
    let mut handled = HashSet::new();
    for removed in &mut report.removed {
        let Some(path) = request
            .targets
            .iter()
            .find(|target| target.id == removed.id)
            .and_then(|target| target.worktree.as_ref())
        else {
            continue;
        };
        if !handled.insert(path) {
            continue;
        }
        let outcome = if !report.failed.is_empty() {
            workspace::WorktreeRemoval::Kept("a session in this batch could not be deleted".into())
        } else if report.shared_children > 0 || !report.warnings.is_empty() {
            workspace::WorktreeRemoval::Kept("sub-agent transcripts were kept".into())
        } else {
            workspace::remove_clean_worktree(path)
        };
        if matches!(outcome, workspace::WorktreeRemoval::Removed { .. }) {
            remove_empty_parents(path);
        }
        removed.worktree = Some((path.clone(), outcome));
    }
    drop(leases);
    if let Err(error) = preferences::forget_sessions_in(&request.database, &deleted_ids) {
        report
            .warnings
            .push(format!("Could not update the session registry: {error}"));
    }
    if let Some(editor) = &request.metrics {
        let mut sessions: HashSet<String> = deleted_ids.iter().cloned().collect();
        sessions.extend(deleted_children);
        if let Err(error) = editor.take(sessions) {
            report
                .warnings
                .push(format!("Could not remove their metrics: {error}"));
        }
    }
    report
}

/// What a sweep of empty sessions removed.
#[derive(Debug, Default)]
pub(crate) struct Sweep {
    pub(crate) ids: Vec<String>,
    pub(crate) bytes: u64,
    /// Empty sessions left because something still has them open.
    pub(crate) open: usize,
}

impl Sweep {
    pub(crate) fn summary(&self) -> String {
        let mut summary = match self.ids.len() {
            0 => "No empty sessions to remove".to_owned(),
            count => format!(
                "Removed {count} empty session{}, freeing {}",
                plural(count),
                byte_text(self.bytes)
            ),
        };
        if self.open > 0 {
            summary.push_str(&format!(
                "; {} still open {} left",
                self.open,
                if self.open == 1 { "was" } else { "were" }
            ));
        }
        summary
    }
}

/// Removes the sessions OMP started that never held a conversation: no
/// list shows them and OMP will not resume them, so nothing is lost. One
/// that is open (its lock is held, or a live session in `keep` names it),
/// one with sub-agents, and one another journal names as a sub-agent all
/// stay. Reads every journal first and removes nothing if one cannot be
/// read.
pub(crate) fn sweep_empty(
    sessions_dir: &Path,
    database: &Path,
    keep: &HashSet<String>,
    metrics: Option<&metrics::Editor>,
) -> Result<Sweep, String> {
    let _span = perf::span("gui.housekeeping.sweep_empty");
    let graph = graph(sessions_dir)?;
    let named: HashSet<&str> = graph.values().flatten().map(String::as_str).collect();
    let mut sweep = Sweep::default();
    let entries = fs::read_dir(sessions_dir)
        .map_err(|error| format!("could not list {}: {error}", sessions_dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|extension| extension != journal::JOURNAL_EXTENSION)
        {
            continue;
        }
        let Some(stem) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if keep.contains(&stem) || named.contains(stem.as_str()) || graph.contains_key(&stem) {
            continue;
        }
        let Ok(transcript) = journal::read(&path) else {
            continue;
        };
        if transcript.has_conversation() {
            continue;
        }
        let Ok(lease) = lease(&path) else {
            sweep.open += 1;
            continue;
        };
        if let Ok(freed) = remove_stem(sessions_dir, &stem) {
            sweep.bytes += freed;
            sweep.ids.push(stem);
        }
        drop(lease);
    }
    if let Err(error) = preferences::forget_sessions_in(database, &sweep.ids) {
        return Err(format!(
            "removed {} empty sessions but could not update the registry: {error}",
            sweep.ids.len()
        ));
    }
    if let Some(editor) = metrics {
        let _ = editor.take(sweep.ids.iter().cloned().collect());
    }
    Ok(sweep)
}

/// Removes the empty project folder a removed worktree leaves behind in
/// Omperor's worktrees directory.
fn remove_empty_parents(worktree: &Path) {
    if let Some(parent) = worktree.parent() {
        // Fails, harmlessly, unless the folder is empty.
        let _ = fs::remove_dir(parent);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_subagent_named_like_shared_storage_never_takes_it_along() {
        let dir = Path::new("/sessions");
        for shared in SHARED_NAMES {
            let files = files_for(dir, shared);
            assert!(files.contains(&dir.join(format!("{shared}.oms"))));
            assert!(!files.contains(&dir.join(shared)), "{shared}");
            assert!(!files.contains(&dir.join(format!("{shared}.jsonl"))), "{shared}");
        }
        let files = files_for(dir, "01M4A5FZ587GGERJ3YT2AA20RV");
        assert!(files.contains(&dir.join("01M4A5FZ587GGERJ3YT2AA20RV")));
        assert!(files.contains(&dir.join("01M4A5FZ587GGERJ3YT2AA20RV.jsonl")));
    }
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn scratch(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "omperor-housekeeping-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// A real journal, as OMP writes one, naming `children` as sub-agents.
    pub(crate) fn write_journal(path: &Path, children: &[&str]) {
        use omp_journal::{EntryDraft, Kind, KindName};
        let draft = |kind: KindName, label: Option<&str>, data: serde_json::Value, by| EntryDraft {
            kind: Kind::known(kind),
            by,
            prior: None,
            label: label.map(|label| label.to_owned().into()),
            data: data.to_string().into(),
        };
        let mut journal = Journal::create(path).expect("journal fixture");
        let genesis = journal
            .append(draft(
                KindName::Journal,
                None,
                serde_json::json!({ "version": 1, "cwd": "/tmp/project", "created": "2026-09-04T00:00:00Z" }),
                None,
            ))
            .expect("genesis");
        for child in children {
            journal
                .append(draft(
                    KindName::Patch,
                    Some("jobs.insert"),
                    serde_json::json!({ "ops": [["ins", 5, null, {
                        "tag": "subagent", "props": [["id", child], ["status", "done"]]
                    }]] }),
                    Some(genesis.id),
                ))
                .expect("sub-agent");
        }
    }

    #[test]
    fn deleting_a_session_takes_its_own_sub_agents_and_leaves_a_shared_one() {
        let dir = scratch("children");
        write_journal(&dir.join("parent.oms"), &["scout", "shared"]);
        write_journal(&dir.join("fork.oms"), &["shared"]);
        write_journal(&dir.join("scout.oms"), &[]);
        write_journal(&dir.join("shared.oms"), &[]);
        let report = delete(Request {
            sessions_dir: dir.clone(),
            database: dir.join("omperor.sqlite3"),
            targets: vec![Target {
                id: "parent".into(),
                title: "Parent".into(),
                journal: true,
                worktree: None,
            }],
            metrics: None,
        });
        assert!(
            report.failed.is_empty() && report.warnings.is_empty(),
            "{report:?}"
        );
        assert_eq!(report.removed[0].children, 1);
        assert_eq!(report.shared_children, 1);
        assert!(!dir.join("parent.oms").exists());
        assert!(!dir.join("scout.oms").exists());
        assert!(dir.join("shared.oms").is_file());
        assert!(dir.join("fork.oms").is_file());
        fs::remove_dir_all(dir).unwrap();
    }

    fn graph_of(edges: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        edges
            .iter()
            .map(|(parent, children)| {
                (
                    (*parent).to_owned(),
                    children.iter().map(|c| (*c).to_owned()).collect(),
                )
            })
            .collect()
    }

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn a_sub_agent_another_session_names_stays_with_everything_it_names() {
        // `a` is being deleted. `scout` is only its; `shared` is also named
        // by the fork `b`, so it stays, and so does `deep`, which only
        // `shared` names.
        let graph = graph_of(&[
            ("a", &["scout", "shared"]),
            ("b", &["shared"]),
            ("shared", &["deep"]),
            ("scout", &["helper"]),
        ]);
        let (owned, shared) = exclusive_descendants(&graph, &set(&["a"]));
        assert_eq!(owned, set(&["scout", "helper"]));
        assert_eq!(shared, 2);
        // Deleting the fork too frees the whole tree.
        let (owned, shared) = exclusive_descendants(&graph, &set(&["a", "b"]));
        assert_eq!(owned, set(&["scout", "helper", "shared", "deep"]));
        assert_eq!(shared, 0);
    }

    #[test]
    fn deleting_keeps_locked_descendants_even_when_another_path_reaches_them() {
        for direct_leaf in [false, true] {
            let dir = scratch("delete-locked-child");
            let children: &[&str] = if direct_leaf {
                &["busy", "leaf", "free"]
            } else {
                &["busy", "free"]
            };
            write_journal(&dir.join("parent.oms"), children);
            write_journal(&dir.join("busy.oms"), &["leaf"]);
            write_journal(&dir.join("leaf.oms"), &[]);
            write_journal(&dir.join("free.oms"), &[]);
            let busy = lease(&dir.join("busy.oms")).unwrap();
            let report = delete(Request {
                sessions_dir: dir.clone(),
                database: dir.join("omperor.sqlite3"),
                metrics: None,
                targets: vec![Target {
                    id: "parent".into(),
                    title: "Parent".into(),
                    journal: true,
                    worktree: None,
                }],
            });
            assert!(
                report.failed.is_empty() && report.warnings.is_empty(),
                "{report:?}"
            );
            assert_eq!(report.shared_children, 2);
            assert_eq!(report.removed[0].children, 1);
            assert!(dir.join("busy.oms").is_file());
            assert!(dir.join("leaf.oms").is_file());
            assert!(!dir.join("free.oms").exists());
            drop(busy);
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn pruning_after_a_failed_parent_keeps_its_shared_descendants_and_cycles() {
        let graph = graph_of(&[
            ("root", &["busy", "leaf", "free"]),
            ("busy", &["leaf"]),
            ("leaf", &["cycle"]),
            ("cycle", &["leaf"]),
        ]);
        let mut candidates = set(&["leaf", "cycle", "free"]);
        retain_exclusive(&graph, &set(&["root"]), &mut candidates);
        assert_eq!(candidates, set(&["free"]));
        // The root failed too: even its independent child must stay.
        retain_exclusive(&graph, &HashSet::new(), &mut candidates);
        assert!(candidates.is_empty());
    }

    #[test]
    fn batch_keeps_worktrees_used_by_open_sessions_or_children_then_removes_on_retry() {
        use std::process::Command;
        for child in [false, true] {
            let dir = scratch("delete-shared-worktree");
            let repo = dir.join("repo");
            fs::create_dir(&repo).unwrap();
            let git = |args: &[&str]| {
                let output = Command::new(crate::workspace::git())
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .env("GIT_AUTHOR_NAME", "Omperor Test")
                    .env("GIT_AUTHOR_EMAIL", "test@example.com")
                    .env("GIT_COMMITTER_NAME", "Omperor Test")
                    .env("GIT_COMMITTER_EMAIL", "test@example.com")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            git(&["init", "--quiet", "-b", "main"]);
            git(&["commit", "--allow-empty", "--quiet", "-m", "base"]);
            let worktree = dir.join("worktrees/feature");
            workspace::add_worktree(&repo, "omperor/feature", &worktree).unwrap();
            write_journal(&dir.join("ready.oms"), if child { &["busy"] } else { &[] });
            write_journal(&dir.join("busy.oms"), &[]);
            let busy = lease(&dir.join("busy.oms")).unwrap();
            let target = |id: &str| Target {
                id: id.into(),
                title: id.into(),
                journal: true,
                worktree: Some(worktree.clone()),
            };
            let request = |targets| Request {
                sessions_dir: dir.clone(),
                database: dir.join("omperor.sqlite3"),
                metrics: None,
                targets,
            };
            let targets = if child {
                vec![target("ready")]
            } else {
                vec![target("ready"), target("busy")]
            };
            let report = delete(request(targets));
            assert_eq!(report.failed.len(), usize::from(!child));
            assert_eq!(report.shared_children, usize::from(child));
            assert_eq!(report.removed.len(), 1);
            assert!(matches!(
                report.removed[0].worktree,
                Some((_, workspace::WorktreeRemoval::Kept(_)))
            ));
            assert!(worktree.is_dir());
            assert!(dir.join("busy.oms").is_file());
            drop(busy);
            let report = delete(request(vec![target("busy")]));
            assert!(report.failed.is_empty(), "{report:?}");
            assert!(matches!(
                report.removed[0].worktree,
                Some((_, workspace::WorktreeRemoval::Removed { .. }))
            ));
            assert!(!worktree.exists());
            git(&["show-ref", "--verify", "refs/heads/omperor/feature"]);
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn sub_agent_ids_name_journals_as_omp_does() {
        assert_eq!(stem("01M2TM3K"), "01M2TM3K");
        assert_eq!(stem("icon research/2"), "icon_research_2");
        assert_eq!(
            journal_path(Path::new("/s"), "a.b").display().to_string(),
            "/s/a_b.oms"
        );
    }

    #[test]
    fn a_session_without_a_journal_is_forgotten_and_a_held_journal_is_not_touched() {
        let dir = scratch("delete");
        let database = dir.join("omperor.sqlite3");
        for stem in ["held", "loose"] {
            fs::write(dir.join(format!("{stem}.oms")), b"not a real journal").unwrap();
            fs::write(dir.join(format!("{stem}.heights")), b"heights").unwrap();
            fs::create_dir_all(dir.join(stem).join("local")).unwrap();
            fs::write(dir.join(stem).join("local").join("artifact"), b"artifact").unwrap();
        }
        // Another process holds `held`.
        let held = lease(&dir.join("held.oms")).unwrap();
        let target = |id: &str, journal: bool| Target {
            id: id.into(),
            title: format!("Session {id}"),
            journal,
            worktree: None,
        };
        let report = delete(Request {
            sessions_dir: dir.clone(),
            database,
            targets: vec![
                target("held", true),
                target("loose", true),
                target("draft-7", false),
            ],
            metrics: None,
        });
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].id, "held");
        assert_eq!(
            report
                .removed
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["loose", "draft-7"]
        );
        assert!(report.removed[0].bytes > 0);
        for leftover in ["loose.oms", "loose.oms.lock", "loose.heights", "loose"] {
            assert!(!dir.join(leftover).exists(), "{leftover} remains");
        }
        for kept in ["held.oms", "held.heights", "held/local/artifact"] {
            assert!(dir.join(kept).exists(), "{kept} was removed");
        }
        drop(held);
        assert!(report.summary().starts_with("Deleted 2 sessions"));
        fs::remove_dir_all(dir).unwrap();
    }

    /// A journal holding one exchange: a conversation, so it is listed.
    fn write_conversation(path: &Path) {
        use omp_journal::{EntryDraft, Kind, KindName};
        let draft = |kind: KindName, data: serde_json::Value, by| EntryDraft {
            kind: Kind::known(kind),
            by,
            prior: None,
            label: None,
            data: data.to_string().into(),
        };
        let mut journal = Journal::create(path).expect("journal fixture");
        let genesis = journal
            .append(draft(
                KindName::Journal,
                serde_json::json!({ "version": 1, "cwd": "/tmp/project", "created": "2026-09-04T00:00:00Z" }),
                None,
            ))
            .expect("genesis");
        let turn = journal
            .append(draft(
                KindName::TurnStart,
                serde_json::json!({}),
                Some(genesis.id),
            ))
            .expect("turn");
        journal
            .append(draft(
                KindName::MsgUser,
                serde_json::json!({ "text": "hello" }),
                Some(turn.id),
            ))
            .expect("prompt");
    }

    #[test]
    fn a_sweep_takes_only_empty_sessions_nothing_has_open_or_names() {
        let dir = scratch("sweep");
        write_conversation(&dir.join("talked.oms"));
        write_journal(&dir.join("empty.oms"), &[]);
        write_journal(&dir.join("open.oms"), &[]);
        write_journal(&dir.join("kept.oms"), &[]);
        // An empty parent names an empty sub-agent: both stay.
        write_journal(&dir.join("parent.oms"), &["child"]);
        write_journal(&dir.join("child.oms"), &[]);
        let held = lease(&dir.join("open.oms")).unwrap();
        let keep = HashSet::from(["kept".to_owned()]);
        let sweep = sweep_empty(&dir, &dir.join("omperor.sqlite3"), &keep, None).unwrap();
        assert_eq!(sweep.ids, ["empty"]);
        assert_eq!(sweep.open, 1);
        assert!(sweep.bytes > 0);
        assert!(!dir.join("empty.oms").exists() && !dir.join("empty.oms.lock").exists());
        for stays in [
            "talked.oms",
            "open.oms",
            "kept.oms",
            "parent.oms",
            "child.oms",
        ] {
            assert!(dir.join(stays).is_file(), "{stays} was removed");
        }
        drop(held);
        assert!(sweep.summary().starts_with("Removed 1 empty session,"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn byte_counts_read_like_finder() {
        assert_eq!(byte_text(512), "512 bytes");
        assert_eq!(byte_text(1_500), "1.5 KB");
        assert_eq!(byte_text(3_400_000), "3.4 MB");
        assert_eq!(byte_text(250_000_000), "250 MB");
    }
}
