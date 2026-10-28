use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    error::Error,
    fmt,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::SystemTime,
};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::perf;

const MAX_TITLE_CHARS: usize = 88;
const INITIAL_HISTORY_PAGE_MESSAGES: usize = 8;
const HISTORY_PAGE_MESSAGES: usize = 16;
const REVERSE_READ_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speaker {
    User,
    Assistant,
    /// The model's reasoning, replayed from a durable session.
    Thinking,
    /// A tool call and its outcome, replayed from a durable session.
    Tool,
    /// A folded activity or notice line with no richer structure.
    Event,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionMessage {
    pub(crate) speaker: Speaker,
    /// Inline text: prose for spoken turns, a compact summary for tool rows.
    pub(crate) text: String,
    /// The tool name behind a `Tool` row, used to title its detail modal.
    pub(crate) tool: Option<String>,
    /// Full payload shown when a tool or event row is expanded.
    pub(crate) detail: Option<String>,
    /// Whether a `Tool` row represents a failed call.
    pub(crate) failed: bool,
}

impl SessionMessage {
    /// A spoken turn (`User`/`Assistant`) or a bare `Event`/`Thinking` line.
    pub(crate) fn spoken(speaker: Speaker, text: impl Into<String>) -> Self {
        Self {
            speaker,
            text: text.into(),
            tool: None,
            detail: None,
            failed: false,
        }
    }

    /// A tool row carrying its descriptive summary, full detail, and status.
    pub(crate) fn tool(
        name: impl Into<String>,
        summary: impl Into<String>,
        detail: Option<String>,
        failed: bool,
    ) -> Self {
        Self {
            speaker: Speaker::Tool,
            text: summary.into(),
            tool: Some(name.into()),
            detail,
            failed,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SessionSummary {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) project: String,
    pub(crate) path: PathBuf,
    pub(crate) cwd: Option<PathBuf>,
    modified: SystemTime,
    history_mode: HistoryMode,
    ownership: SessionOwnership,
    /// The model the session last ran on, when its journal recorded one.
    pub(crate) model: Option<String>,
    /// How the session's last turn ended, when its journal recorded one.
    pub(crate) outcome: Option<SessionOutcome>,
    /// Ids of the sub-agents the session spawned. Their journals sit beside
    /// the session's and are not sessions of their own.
    pub(crate) spawned: Vec<String>,
}

/// How a session's most recent turn ended, read from its journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionOutcome {
    /// The turn ran to completion.
    Completed,
    /// The turn ended with an error notice.
    Errored,
    /// The child exited while the turn was still running.
    Interrupted,
    /// The turn had started and nothing has ended it: a live child is still
    /// working, or one died without recording its exit.
    InProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionOwnership {
    Omperor,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryMode {
    Chronological,
    LegacyChain,
    /// A journal-first `.oms` session read through `crate::journal`.
    Journal,
}

#[derive(Debug)]
pub(crate) enum HistoryUpdate {
    Page(Vec<SessionMessage>),
    Complete,
    Failed(SessionError),
}

#[derive(Debug)]
pub(crate) enum DiscoveryUpdate {
    Omperor(Result<Vec<SessionSummary>, SessionError>),
    Omp(Result<Vec<SessionSummary>, SessionError>),
}

impl SessionSummary {
    /// Builds a summary for tests in other modules, which cannot name the
    /// private ownership and history fields.
    #[cfg(test)]
    pub(crate) fn for_test(id: &str, title: &str, cwd: Option<&str>, omperor_owned: bool) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            project: cwd
                .and_then(|cwd| Path::new(cwd).file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Unknown project".into()),
            path: PathBuf::from(if omperor_owned {
                format!("/tmp/{id}.oms")
            } else {
                format!("/tmp/{id}.jsonl")
            }),
            cwd: cwd.map(PathBuf::from),
            modified: SystemTime::UNIX_EPOCH,
            history_mode: if omperor_owned {
                HistoryMode::Journal
            } else {
                HistoryMode::Chronological
            },
            ownership: if omperor_owned {
                SessionOwnership::Omperor
            } else {
                SessionOwnership::External
            },
            model: None,
            outcome: None,
            spawned: Vec::new(),
        }
    }

    /// When the session's journal last changed, in unix milliseconds.
    pub(crate) fn modified_ms(&self) -> i64 {
        self.modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
    }

    /// Whether this is the mock backend's transient conversation.
    pub(crate) fn is_mock(&self) -> bool {
        self.id.starts_with("mock-")
    }

    pub(crate) fn menu_label(&self) -> String {
        let source = if self.is_omperor_owned() {
            "Omperor"
        } else {
            "OMP"
        };
        format!("{} — {} — {source}", self.title, self.project)
    }

    /// Whether Omperor created this session and the bundled OMP can still
    /// resume it. Sessions written before OMP moved to `.oms` journals stay
    /// readable but are listed with the read-only OMP sessions.
    pub(crate) fn is_omperor_owned(&self) -> bool {
        self.ownership == SessionOwnership::Omperor && self.history_mode == HistoryMode::Journal
    }
}

#[derive(Debug)]
pub(crate) enum SessionError {
    HomeUnavailable,
    Read { path: PathBuf, source: io::Error },
    NoMessages(PathBuf),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeUnavailable => write!(
                formatter,
                "OMP's data directory could not be resolved because HOME is unset"
            ),
            Self::Read { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::NoMessages(path) => {
                write!(
                    formatter,
                    "{} has no renderable chat messages",
                    path.display()
                )
            }
        }
    }
}

impl Error for SessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::HomeUnavailable | Self::NoMessages(_) => None,
        }
    }
}

pub(crate) fn stream_discovery(
    omperor_sessions: &Path,
    omperor_data: &Path,
    sender: flume::Sender<DiscoveryUpdate>,
) {
    stream_discovery_with_roots(omperor_sessions, omperor_data, sender, data_roots);
}

fn stream_discovery_with_roots(
    omperor_sessions: &Path,
    omperor_data: &Path,
    sender: flume::Sender<DiscoveryUpdate>,
    external_roots: impl FnOnce() -> Result<Vec<PathBuf>, SessionError>,
) {
    let _span = perf::span("gui.session_discovery");
    let omperor = {
        let _span = perf::span("gui.session_discovery.omperor");
        discover_omperor_in(omperor_sessions, omperor_data)
    };
    let excluded = omperor
        .as_ref()
        .map(|sessions| session_identities(sessions))
        .unwrap_or_default();
    if sender.send(DiscoveryUpdate::Omperor(omperor)).is_err() {
        return;
    }
    let omp = {
        let _span = perf::span("gui.session_discovery.omp");
        external_roots().and_then(|roots| discover_external_in(&roots, &excluded))
    };
    let _ = sender.send(DiscoveryUpdate::Omp(omp));
}

pub(crate) fn stream_history(summary: SessionSummary, sender: flume::Sender<HistoryUpdate>) {
    let pager = HistoryPager::open(&summary);
    stream_pages(pager, &summary.path, sender);
}

fn stream_pages(pager: Result<HistoryPager, SessionError>, path: &Path, sender: flume::Sender<HistoryUpdate>) {
    let result = (|| {
        let mut pager = pager?;
        let mut message_count = 0usize;
        let mut page_limit = INITIAL_HISTORY_PAGE_MESSAGES;
        while let Some(page) = pager.next_page(page_limit)? {
            message_count += page.len();
            if sender.send(HistoryUpdate::Page(page)).is_err() {
                return Ok(());
            }
            page_limit = HISTORY_PAGE_MESSAGES;
        }
        if message_count == 0 {
            return Err(SessionError::NoMessages(path.to_owned()));
        }
        let _ = sender.send(HistoryUpdate::Complete);
        Ok(())
    })();
    if let Err(error) = result {
        let _ = sender.send(HistoryUpdate::Failed(error));
    }
}

struct HistoryPager {
    lines: Option<ReverseLines>,
    /// Remaining journal messages, oldest first; pages are taken from the end.
    journal: Option<Vec<SessionMessage>>,
    path: PathBuf,
    mode: HistoryMode,
    pending: VecDeque<Value>,
    legacy_started: bool,
    legacy_parent: Option<String>,
    legacy_complete: bool,
}

impl HistoryPager {
    fn open(summary: &SessionSummary) -> Result<Self, SessionError> {
        if summary.history_mode == HistoryMode::Journal {
            let transcript =
                crate::journal::read(&summary.path).map_err(|message| SessionError::Read {
                    path: summary.path.clone(),
                    source: io::Error::other(message),
                })?;
            return Ok(Self {
                lines: None,
                journal: Some(transcript.messages),
                path: summary.path.clone(),
                mode: summary.history_mode,
                pending: VecDeque::new(),
                legacy_started: false,
                legacy_parent: None,
                legacy_complete: false,
            });
        }
        let lines = ReverseLines::open(&summary.path).map_err(|source| SessionError::Read {
            path: summary.path.clone(),
            source,
        })?;
        Ok(Self {
            lines: Some(lines),
            journal: None,
            path: summary.path.clone(),
            mode: summary.history_mode,
            pending: VecDeque::new(),
            legacy_started: false,
            legacy_parent: None,
            legacy_complete: false,
        })
    }

    fn next_page(&mut self, limit: usize) -> Result<Option<Vec<SessionMessage>>, SessionError> {
        let _span = perf::span_with(
            "gui.session_history_page",
            || serde_json::json!({ "page_limit": limit }),
        );
        if let Some(remaining) = &mut self.journal {
            let start = remaining.len().saturating_sub(limit.max(1));
            let page = remaining.split_off(start);
            return Ok((!page.is_empty()).then_some(page));
        }
        let mut messages = Vec::with_capacity(limit);
        while messages.len() < limit && !self.legacy_complete {
            let Some(entry) = self.next_entry()? else {
                break;
            };
            if self.mode == HistoryMode::LegacyChain && !self.accept_legacy_entry(&entry) {
                continue;
            }
            if let Some(message) = renderable_message(&entry) {
                messages.push(message);
            }
        }
        messages.reverse();
        Ok((!messages.is_empty()).then_some(messages))
    }

    fn next_entry(&mut self) -> Result<Option<Value>, SessionError> {
        loop {
            if let Some(value) = self.pending.pop_front() {
                return Ok(Some(value));
            }
            let Some(lines) = self.lines.as_mut() else {
                return Ok(None);
            };
            let Some(value) = lines.previous_json().map_err(|source| SessionError::Read {
                path: self.path.clone(),
                source,
            })?
            else {
                return Ok(None);
            };
            self.pending
                .extend(record_events(&value).into_iter().rev().cloned());
        }
    }

    fn accept_legacy_entry(&mut self, entry: &Value) -> bool {
        let id = entry.get("id").and_then(Value::as_str);
        if !self.legacy_started {
            if id.is_none() || entry.get("type").and_then(Value::as_str) == Some("session") {
                return false;
            }
            self.legacy_started = true;
        } else if id != self.legacy_parent.as_deref() {
            return false;
        }

        self.legacy_parent = entry
            .get("parentId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.legacy_complete = self.legacy_parent.is_none();
        true
    }
}

struct ReverseLines {
    file: File,
    next_end: u64,
    buffer: Vec<u8>,
    cursor: usize,
}

impl ReverseLines {
    fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let next_end = file.metadata()?.len();
        Ok(Self {
            file,
            next_end,
            buffer: Vec::new(),
            cursor: 0,
        })
    }

    fn previous_json(&mut self) -> io::Result<Option<Value>> {
        while let Some(line) = self.previous_line()? {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if let Ok(value) = serde_json::from_slice(&line) {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    fn previous_line(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut fragments = Vec::new();
        loop {
            if self.cursor > 0 {
                if let Some(newline) = self.buffer[..self.cursor]
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                {
                    fragments.push(self.buffer[newline + 1..self.cursor].to_vec());
                    self.cursor = newline;
                    return Ok(Some(join_reverse_fragments(fragments)));
                }
                fragments.push(self.buffer[..self.cursor].to_vec());
                self.cursor = 0;
            }
            if self.next_end == 0 {
                return Ok((!fragments.is_empty()).then(|| join_reverse_fragments(fragments)));
            }

            let start = self.next_end.saturating_sub(REVERSE_READ_BYTES);
            let length = usize::try_from(self.next_end - start).expect("read chunk fits usize");
            self.file.seek(SeekFrom::Start(start))?;
            self.buffer.resize(length, 0);
            self.file.read_exact(&mut self.buffer)?;
            self.next_end = start;
            self.cursor = length;
        }
    }
}

fn join_reverse_fragments(fragments: Vec<Vec<u8>>) -> Vec<u8> {
    let length = fragments.iter().map(Vec::len).sum();
    let mut line = Vec::with_capacity(length);
    for fragment in fragments.into_iter().rev() {
        line.extend(fragment);
    }
    line
}

#[cfg(test)]
pub(crate) fn render(messages: &[SessionMessage]) -> String {
    messages
        .iter()
        .map(|message| {
            let author = match message.speaker {
                Speaker::User => "You",
                Speaker::Assistant => "omp",
                Speaker::Thinking => "omp · thinking",
                Speaker::Tool => "omp · tool",
                Speaker::Event => "OMP event",
            };
            format!("{author}\n{}", message.text)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn data_roots() -> Result<Vec<PathBuf>, SessionError> {
    let home = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(SessionError::HomeUnavailable)?;
    let mut candidates = Vec::new();
    if let Some(explicit) = env::var_os("OMP_DATA_DIR").filter(|value| !value.is_empty()) {
        let explicit = PathBuf::from(explicit);
        candidates.push(explicit.join("agent"));
        candidates.push(explicit);
    }
    // The TypeScript application stores durable state here. The omp2 branch
    // uses the XDG data location by default, so both are searched during the
    // migration period.
    candidates.push(home.join(".omp/agent"));
    candidates.push(home.join(".local/share/omp"));

    let mut seen = HashSet::new();
    candidates.retain(|path| seen.insert(path.clone()));
    Ok(candidates)
}

#[cfg(test)]
fn discover_in(
    roots: &[PathBuf],
    omperor_sessions: Option<&Path>,
    omperor_data: Option<&Path>,
) -> Result<Vec<SessionSummary>, SessionError> {
    let mut omperor = match (omperor_sessions, omperor_data) {
        (Some(sessions), Some(data)) => discover_omperor_in(sessions, data)?,
        _ => Vec::new(),
    };
    let excluded = session_identities(&omperor);
    let mut external = discover_external_in(roots, &excluded)?;
    omperor.append(&mut external);
    sort_sessions(&mut omperor);
    Ok(omperor)
}

fn discover_omperor_in(
    sessions_directory: &Path,
    data_directory: &Path,
) -> Result<Vec<SessionSummary>, SessionError> {
    let titles = indexed_titles(std::slice::from_ref(&data_directory.to_owned()));
    let mut paths = Vec::new();
    collect_direct_sessions(sessions_directory, &mut paths);
    let sessions = probe_sessions(paths, &titles, SessionOwnership::Omperor, &HashSet::new())?;
    Ok(without_spawned_children(sessions))
}

/// Drops the journals that belong to sub-agents: OMP writes a child's
/// journal beside its parent's, named by the child's id, so a sidebar that
/// listed every journal would show each sub-agent as a session. The Agents
/// panel shows them under their parent instead.
fn without_spawned_children(sessions: Vec<SessionSummary>) -> Vec<SessionSummary> {
    let children: HashSet<String> = sessions
        .iter()
        .flat_map(|summary| summary.spawned.iter().cloned())
        .collect();
    sessions
        .into_iter()
        .filter(|summary| !children.contains(&summary.id))
        .collect()
}

fn discover_external_in(
    roots: &[PathBuf],
    excluded: &HashSet<PathBuf>,
) -> Result<Vec<SessionSummary>, SessionError> {
    let titles = indexed_titles(roots);
    let mut paths = Vec::new();
    for root in roots {
        collect_legacy_or_direct_sessions(&root.join("sessions"), &mut paths);
        collect_project_sessions(&root.join("projects"), &mut paths);
    }
    probe_sessions(paths, &titles, SessionOwnership::External, excluded)
}

fn session_identities(sessions: &[SessionSummary]) -> HashSet<PathBuf> {
    sessions
        .iter()
        .map(|session| fs::canonicalize(&session.path).unwrap_or_else(|_| session.path.clone()))
        .collect()
}

fn probe_sessions(
    paths: Vec<PathBuf>,
    titles: &HashMap<String, String>,
    ownership: SessionOwnership,
    excluded: &HashSet<PathBuf>,
) -> Result<Vec<SessionSummary>, SessionError> {
    let mut seen = HashSet::new();
    let mut sessions = Vec::new();
    for path in paths {
        let identity = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if excluded.contains(&identity) || !seen.insert(identity) {
            continue;
        }
        match probe_session(&path, titles, ownership) {
            Ok(Some(session)) => sessions.push(session),
            Ok(None) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(SessionError::Read { path, source }),
        }
    }
    sort_sessions(&mut sessions);
    Ok(sessions)
}

fn sort_sessions(sessions: &mut [SessionSummary]) {
    sessions.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn collect_direct_sessions(directory: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    output.extend(
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_jsonl_file(path) || crate::journal::is_journal(path)),
    );
}

fn collect_legacy_or_direct_sessions(directory: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    let mut direct = entries
        .iter()
        .map(fs::DirEntry::path)
        .filter(|path| is_jsonl_file(path))
        .collect::<Vec<_>>();
    if !direct.is_empty() {
        output.append(&mut direct);
        return;
    }

    // Legacy OMP groups top-level sessions by an encoded project directory.
    // Only descend one level: transcript artifact directories can themselves
    // contain subagent JSONL files and must not appear as resumable sessions.
    for project in entries
        .iter()
        .map(fs::DirEntry::path)
        .filter(|path| path.is_dir())
    {
        let Ok(entries) = fs::read_dir(project) else {
            continue;
        };
        output.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| is_jsonl_file(path)),
        );
    }
}

fn collect_project_sessions(projects: &Path, output: &mut Vec<PathBuf>) {
    let Ok(projects) = fs::read_dir(projects) else {
        return;
    };
    for project in projects.filter_map(Result::ok) {
        let directory = project.path().join("sessions");
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        output.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| is_jsonl_file(path)),
        );
    }
}

fn is_jsonl_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
}

fn indexed_titles(roots: &[PathBuf]) -> HashMap<String, String> {
    let mut titles = HashMap::new();
    for root in roots {
        query_titles(
            &root.join("history.db"),
            "SELECT session_id, title FROM session_titles",
            &mut titles,
        );
        query_titles(
            &root.join("sessions.sqlite3"),
            "SELECT id, title FROM sessions WHERE title IS NOT NULL",
            &mut titles,
        );
        let Ok(projects) = fs::read_dir(root.join("projects")) else {
            continue;
        };
        for project in projects.filter_map(Result::ok) {
            query_titles(
                &project.path().join("sessions.sqlite3"),
                "SELECT id, title FROM sessions WHERE title IS NOT NULL",
                &mut titles,
            );
        }
    }
    titles
}

fn query_titles(path: &Path, query: &str, titles: &mut HashMap<String, String>) {
    if !path.is_file() {
        return;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(connection) = Connection::open_with_flags(path, flags) else {
        return;
    };
    let Ok(mut statement) = connection.prepare(query) else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return;
    };
    for (id, title) in rows.filter_map(Result::ok) {
        if let Some(title) = sanitize_title(&title) {
            titles.insert(id, title);
        }
    }
}

fn probe_session(
    path: &Path,
    indexed_titles: &HashMap<String, String>,
    ownership: SessionOwnership,
) -> io::Result<Option<SessionSummary>> {
    if crate::journal::is_journal(path) {
        return probe_journal(path, indexed_titles, ownership);
    }
    let file = File::open(path)?;
    let mut session_id = None;
    let mut cwd = None;
    let mut indexed_title = None;
    let mut slot_title = None;
    let mut latest_title = None;
    let mut header_title = None;
    let mut first_user = None;
    let mut has_history = false;
    let mut legacy_title_slot = false;
    let mut transcript_v4 = false;
    let mut linked_history = false;

    'lines: for line in BufReader::new(file).split(b'\n') {
        let line = line?;
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        for value in record_events(&value) {
            transcript_v4 |= value.get("v").and_then(Value::as_u64) == Some(4);
            let kind = value.get("type").and_then(Value::as_str);
            if kind == Some("title") && value.get("pad").is_some() {
                legacy_title_slot = true;
                slot_title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .and_then(sanitize_title);
            }
            if kind == Some("session") || value.get("v").and_then(Value::as_u64) == Some(4) {
                session_id = value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or(session_id);
                cwd = value
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .or(cwd);
                header_title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .and_then(sanitize_title)
                    .or(header_title);
                if let Some(id) = session_id.as_deref() {
                    indexed_title = indexed_titles.get(id).cloned();
                }
            }
            if matches!(kind, Some("title" | "title_change"))
                || value.get("k").and_then(Value::as_str) == Some("title")
            {
                latest_title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .and_then(sanitize_title)
                    .or(latest_title);
            }
            if let Some(message) = renderable_message(value) {
                has_history = true;
                linked_history |= value.get("id").and_then(Value::as_str).is_some();
                if message.speaker == Speaker::User && first_user.is_none() {
                    first_user = sanitize_title(&message.text);
                }
            }

            // The legacy first-line title slot is rewritten whenever its title
            // changes. Once the session header and first visible message have
            // also been observed, no full transcript scan is necessary.
            if legacy_title_slot && session_id.is_some() && has_history {
                break 'lines;
            }
        }
    }

    if !has_history {
        return Ok(None);
    }
    let id = session_id.unwrap_or_else(|| session_id_from_path(path));
    if id.is_empty() {
        return Ok(None);
    }
    let title = indexed_title
        .or(slot_title)
        .or(latest_title)
        .or(header_title)
        .or(first_user)
        .unwrap_or_else(|| "Untitled session".to_owned());
    let project = cwd
        .as_deref()
        .and_then(Path::file_name)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unknown project".to_owned());
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    Ok(Some(SessionSummary {
        id,
        title,
        project,
        path: path.to_owned(),
        cwd,
        modified,
        history_mode: if transcript_v4 || !linked_history {
            HistoryMode::Chronological
        } else {
            HistoryMode::LegacyChain
        },
        ownership,
        model: None,
        outcome: None,
        spawned: Vec::new(),
    }))
}

/// Summarizes a journal-first session from its committed live chain. A
/// journal that has not recorded a conversation yet is not listed, matching
/// how OMP itself refuses to resume it.
fn probe_journal(
    path: &Path,
    indexed_titles: &HashMap<String, String>,
    ownership: SessionOwnership,
) -> io::Result<Option<SessionSummary>> {
    let Ok(transcript) = crate::journal::read(path) else {
        return Ok(None);
    };
    if !transcript.has_conversation() {
        return Ok(None);
    }
    let id = session_id_from_path(path);
    if id.is_empty() {
        return Ok(None);
    }
    let title = indexed_titles
        .get(&id)
        .cloned()
        .or_else(|| transcript.first_prompt.as_deref().and_then(sanitize_title))
        .unwrap_or_else(|| "Untitled session".to_owned());
    let project = transcript
        .cwd
        .as_deref()
        .and_then(Path::file_name)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unknown project".to_owned());
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(Some(SessionSummary {
        id,
        title,
        project,
        path: path.to_owned(),
        cwd: transcript.cwd,
        modified,
        history_mode: HistoryMode::Journal,
        ownership,
        model: transcript.model,
        outcome: transcript.outcome,
        spawned: transcript.spawned,
    }))
}

/// A journal (`.oms`) is named by its session id, which may itself contain
/// `_` (sub-agents are named by the model). Only legacy `.jsonl` transcripts
/// were named `<timestamp>_<id>`.
fn session_id_from_path(path: &Path) -> String {
    let journal = path
        .extension()
        .is_some_and(|extension| extension == crate::journal::JOURNAL_EXTENSION);
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| match stem.rsplit_once('_') {
            Some((_, id)) if !journal => id,
            _ => stem,
        })
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
fn read_json_lines(reader: impl Read) -> io::Result<Vec<Value>> {
    let mut entries = Vec::new();
    for line in BufReader::new(reader).split(b'\n') {
        let line = line?;
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        entries.extend(record_events(&value).into_iter().cloned());
    }
    Ok(entries)
}

fn record_events(value: &Value) -> Vec<&Value> {
    if value.get("$omp_group").and_then(Value::as_u64) == Some(1) {
        value
            .get("events")
            .and_then(Value::as_array)
            .map_or_else(Vec::new, |events| events.iter().collect())
    } else {
        vec![value]
    }
}

#[cfg(test)]
fn active_entry_indices(entries: &[Value]) -> Vec<usize> {
    let by_id = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id, index))
        })
        .collect::<HashMap<_, _>>();
    let Some(mut current) = entries.iter().rposition(|entry| {
        entry.get("id").and_then(Value::as_str).is_some()
            && entry.get("type").and_then(Value::as_str) != Some("session")
    }) else {
        return (0..entries.len()).collect();
    };

    let mut indices = Vec::new();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            break;
        }
        indices.push(current);
        let Some(parent) = entries
            .get(current)
            .and_then(|entry| entry.get("parentId"))
            .and_then(Value::as_str)
        else {
            break;
        };
        let Some(parent_index) = by_id.get(parent).copied() else {
            break;
        };
        current = parent_index;
    }
    indices.reverse();
    indices
}

fn renderable_message(entry: &Value) -> Option<SessionMessage> {
    let kind = entry
        .get("type")
        .or_else(|| entry.get("k"))
        .and_then(Value::as_str);
    if matches!(kind, Some("custom_message" | "custom")) {
        if entry.get("display").and_then(Value::as_bool) == Some(false) {
            return None;
        }
        let text = visible_content(entry.get("content").or_else(|| entry.get("context")))?;
        return Some(SessionMessage::spoken(Speaker::Event, text));
    }

    if let Some(message) = canonical_item_message(entry) {
        return Some(message);
    }

    let message = if entry.get("role").and_then(Value::as_str).is_some() {
        entry
    } else {
        [
            "/message",
            "/data/message",
            "/item/message",
            "/data/item/message",
        ]
        .into_iter()
        .find_map(|pointer| entry.pointer(pointer).filter(|value| value.is_object()))?
    };
    let speaker = match message.get("role").and_then(Value::as_str)? {
        "user" => Speaker::User,
        "assistant" => Speaker::Assistant,
        _ => return None,
    };
    let text = visible_content(message.get("content").or_else(|| message.get("parts")))?;
    Some(SessionMessage::spoken(speaker, text))
}

fn canonical_item_message(entry: &Value) -> Option<SessionMessage> {
    let item = entry.get("item")?;
    let kind = item.get("kind")?;
    let message = kind.get("Message").or_else(|| kind.get("message"))?;
    let speaker = match message.get("role")? {
        Value::Number(role) if role.as_i64() == Some(2) => Speaker::User,
        Value::Number(role) if role.as_i64() == Some(3) => Speaker::Assistant,
        Value::String(role) if role.eq_ignore_ascii_case("role_user") || role == "user" => {
            Speaker::User
        }
        Value::String(role)
            if role.eq_ignore_ascii_case("role_assistant") || role == "assistant" =>
        {
            Speaker::Assistant
        }
        _ => return None,
    };
    let text = canonical_parts(message.get("parts")?)?;
    Some(SessionMessage::spoken(speaker, text))
}

fn canonical_parts(parts: &Value) -> Option<String> {
    let mut output = Vec::new();
    for part in parts.as_array()? {
        let kind = part.get("kind")?;
        if let Some(text) = kind
            .get("Text")
            .or_else(|| kind.get("text"))
            .and_then(Value::as_str)
        {
            let text = text.trim();
            if !text.is_empty() {
                output.push(text.to_owned());
            }
        } else if kind.get("Blob").or_else(|| kind.get("blob")).is_some() {
            output.push("[Image]".to_owned());
        }
    }
    (!output.is_empty()).then(|| output.join("\n\n"))
}

fn visible_content(content: Option<&Value>) -> Option<String> {
    fn collect(value: &Value, output: &mut Vec<String>) {
        match value {
            Value::String(text) => output.push(text.clone()),
            Value::Array(values) => {
                for value in values {
                    collect(value, output);
                }
            }
            Value::Object(object) => match object.get("type").and_then(Value::as_str) {
                Some("thinking" | "toolCall" | "tool_call") => {}
                Some("image" | "image_url") => output.push("[Image]".to_owned()),
                _ => {
                    let durable_kind = object.get("t").and_then(Value::as_str);
                    if matches!(durable_kind, Some("think" | "tool")) {
                        return;
                    }
                    if durable_kind == Some("image") {
                        output.push("[Image]".to_owned());
                    } else if let Some(kind) = object.get("kind") {
                        collect(kind, output);
                    } else if let Some(text) = object.get("text").and_then(Value::as_str) {
                        output.push(text.to_owned());
                    } else if let Some(content) = object.get("content") {
                        collect(content, output);
                    }
                }
            },
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    let mut parts = Vec::new();
    collect(content?, &mut parts);
    let text = parts
        .into_iter()
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// The title a conversation takes from its first prompt: the prompt itself,
/// cleaned and cut, the way a discovered session is named.
pub(crate) fn title_from_prompt(prompt: &str) -> String {
    sanitize_title(prompt).unwrap_or_else(|| "Mock conversation".to_owned())
}

fn sanitize_title(value: &str) -> Option<String> {
    let value = strip_ansi(value);
    let first_line = value.lines().next().unwrap_or_default();
    let clean = first_line
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let clean = clean.trim();
    if clean.is_empty() {
        return None;
    }
    let mut title = clean.chars().take(MAX_TITLE_CHARS).collect::<String>();
    if clean.chars().count() > MAX_TITLE_CHARS {
        title.push('…');
    }
    Some(title)
}

fn strip_ansi(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' || characters.peek() != Some(&'[') {
            output.push(character);
            continue;
        }
        characters.next();
        for character in characters.by_ref() {
            if ('@'..='~').contains(&character) {
                break;
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Cursor,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        DiscoveryUpdate, HistoryMode, HistoryPager, ReverseLines, SessionOwnership, SessionSummary,
        Speaker, active_entry_indices, discover_in, read_json_lines, render, renderable_message,
        sanitize_title, stream_discovery_with_roots, without_spawned_children,
    };

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    /// Writes a journal-first session holding one user prompt, the shape the
    /// bundled OMP produces under Omperor's session directory.
    fn write_journal_fixture(path: &std::path::Path, cwd: &str, prompt: &str) {
        use omp_journal::{EntryDraft, Journal, Kind, KindName};
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
                serde_json::json!({ "version": 1, "cwd": cwd, "created": "2026-09-04T00:00:00Z" }),
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
                serde_json::json!({ "text": prompt }),
                Some(turn.id),
            ))
            .expect("prompt");
    }

    fn summary(path: PathBuf, history_mode: HistoryMode) -> SessionSummary {
        SessionSummary {
            id: "test-session".into(),
            title: "Test session".into(),
            project: "project".into(),
            path,
            cwd: Some(PathBuf::from("/work/project")),
            modified: std::time::SystemTime::UNIX_EPOCH,
            history_mode,
            ownership: SessionOwnership::External,
            model: None,
            outcome: None,
            spawned: Vec::new(),
        }
    }

    #[test]
    fn sub_agent_journals_are_not_listed_as_sessions() {
        let mut parent = SessionSummary::for_test("parent", "Parent", Some("/work"), true);
        parent.spawned = vec!["scout".into(), "01CHILDULID".into()];
        let scout = SessionSummary::for_test("scout", "Scout", Some("/work"), true);
        let child = SessionSummary::for_test("01CHILDULID", "Child", Some("/work"), true);
        let other = SessionSummary::for_test("other", "Other", Some("/work"), true);
        let listed = without_spawned_children(vec![parent, scout, child, other]);
        let ids: Vec<&str> = listed.iter().map(|summary| summary.id.as_str()).collect();
        assert_eq!(ids, vec!["parent", "other"]);
    }

    #[test]
    fn reads_recent_chronological_messages_before_older_pages() {
        let scratch = scratch_directory();
        let path = scratch.join("chronological.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"role\":\"user\",\"content\":\"first\"}\n",
                "{\"role\":\"assistant\",\"content\":\"second\"}\n",
                "{\"role\":\"user\",\"content\":\"third\"}\n",
            ),
        )
        .expect("history fixture");
        let mut pager =
            HistoryPager::open(&summary(path, HistoryMode::Chronological)).expect("history pager");

        let recent = pager.next_page(2).expect("recent page").expect("page");
        assert_eq!(
            recent
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            ["second", "third"]
        );
        let older = pager.next_page(2).expect("older page").expect("page");
        assert_eq!(older[0].text, "first");
        assert!(pager.next_page(2).expect("completion").is_none());
        let _ = fs::remove_dir_all(scratch);
    }

    #[test]
    fn pages_back_through_only_the_active_legacy_branch() {
        let scratch = scratch_directory();
        let path = scratch.join("legacy.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"id\":\"session\"}\n",
                "{\"type\":\"message\",\"id\":\"user\",\"parentId\":\"session\",\"message\":{\"role\":\"user\",\"content\":\"question\"}}\n",
                "{\"type\":\"message\",\"id\":\"abandoned\",\"parentId\":\"user\",\"message\":{\"role\":\"assistant\",\"content\":\"wrong\"}}\n",
                "{\"type\":\"message\",\"id\":\"active\",\"parentId\":\"user\",\"message\":{\"role\":\"assistant\",\"content\":\"answer\"}}\n",
            ),
        )
        .expect("history fixture");
        let mut pager =
            HistoryPager::open(&summary(path, HistoryMode::LegacyChain)).expect("history pager");

        assert_eq!(
            pager.next_page(1).expect("recent page").expect("page")[0].text,
            "answer"
        );
        assert_eq!(
            pager.next_page(1).expect("older page").expect("page")[0].text,
            "question"
        );
        assert!(pager.next_page(1).expect("completion").is_none());
        let _ = fs::remove_dir_all(scratch);
    }

    #[test]
    fn reverse_reader_handles_records_larger_than_its_io_chunk() {
        let scratch = scratch_directory();
        let path = scratch.join("long-lines.jsonl");
        let long = "x".repeat(70 * 1024);
        fs::write(&path, format!("first\n{long}\nlast\n")).expect("line fixture");
        let mut lines = ReverseLines::open(&path).expect("reverse reader");

        assert_eq!(lines.previous_line().expect("last line"), Some(Vec::new()));
        assert_eq!(
            lines.previous_line().expect("last record"),
            Some(b"last".to_vec())
        );
        assert_eq!(
            lines.previous_line().expect("long record"),
            Some(long.into_bytes())
        );
        assert_eq!(
            lines.previous_line().expect("first record"),
            Some(b"first".to_vec())
        );
        assert_eq!(lines.previous_line().expect("end"), None);
        let _ = fs::remove_dir_all(scratch);
    }

    #[test]
    fn follows_the_active_legacy_branch() {
        let input = concat!(
            "{\"type\":\"session\",\"id\":\"session\",\"cwd\":\"/work/project\"}\n",
            "{\"type\":\"message\",\"id\":\"user\",\"parentId\":\"session\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Hello\"}]}}\n",
            "{\"type\":\"message\",\"id\":\"abandoned\",\"parentId\":\"user\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Wrong branch\"}]}}\n",
            "{\"type\":\"message\",\"id\":\"active\",\"parentId\":\"user\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Active answer\"}]}}\n",
        );
        let entries = read_json_lines(Cursor::new(input)).expect("fixture parses");
        let messages = active_entry_indices(&entries)
            .into_iter()
            .filter_map(|index| renderable_message(&entries[index]))
            .collect::<Vec<_>>();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].speaker, Speaker::User);
        assert_eq!(messages[0].text, "Hello");
        assert_eq!(messages[1].speaker, Speaker::Assistant);
        assert_eq!(messages[1].text, "Active answer");
    }

    #[test]
    fn hides_thinking_tools_and_non_display_messages() {
        let assistant: serde_json::Value = serde_json::json!({
            "type": "message",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "private"},
                    {"type": "toolCall", "name": "read"},
                    {"type": "text", "text": "Visible answer"}
                ]
            }
        });
        let hidden: serde_json::Value = serde_json::json!({
            "type": "custom_message",
            "display": false,
            "content": "background event"
        });

        assert_eq!(
            renderable_message(&assistant)
                .expect("assistant text is visible")
                .text,
            "Visible answer"
        );
        assert!(renderable_message(&hidden).is_none());
    }

    #[test]
    fn renders_session_messages_separately_from_the_chat_model() {
        let messages = vec![
            super::SessionMessage::spoken(Speaker::User, "Question"),
            super::SessionMessage::spoken(Speaker::Assistant, "Answer"),
        ];

        assert_eq!(render(&messages), "You\nQuestion\n\nomp\nAnswer");
    }

    #[test]
    fn decodes_omp2_canonical_items_and_atomic_groups() {
        let input = concat!(
            "{\"v\":4,\"id\":\"session\",\"created\":1,\"cwd\":\"/work/project\"}\n",
            "{\"$omp_group\":1,\"events\":[",
            "{\"ts\":2,\"k\":\"turn_input\",\"item\":{\"kind\":{\"Message\":{\"role\":2,\"parts\":[{\"kind\":{\"Text\":\"Question\"}}]}}}},",
            "{\"ts\":3,\"k\":\"item\",\"item\":{\"kind\":{\"Message\":{\"role\":3,\"parts\":[{\"kind\":{\"Thinking\":{\"text\":\"private\"}}},{\"kind\":{\"Text\":\"Answer\"}}]}}}}",
            "]}\n",
        );
        let entries = read_json_lines(Cursor::new(input)).expect("v4 fixture parses");
        let messages = entries
            .iter()
            .filter_map(renderable_message)
            .collect::<Vec<_>>();

        assert_eq!(entries.len(), 3);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].speaker, Speaker::User);
        assert_eq!(messages[0].text, "Question");
        assert_eq!(messages[1].speaker, Speaker::Assistant);
        assert_eq!(messages[1].text, "Answer");
    }

    #[test]
    fn sanitizes_terminal_codes_and_long_titles() {
        assert_eq!(
            sanitize_title("\u{1b}[31mUseful title\u{1b}[0m\nignored").as_deref(),
            Some("Useful title")
        );
        let title = sanitize_title(&"x".repeat(100)).expect("long title is retained");
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), 89);
    }

    #[test]
    fn discovers_legacy_sessions_with_the_current_title_slot() {
        let scratch = scratch_directory();
        let project = scratch.join("sessions/-work-project");
        fs::create_dir_all(&project).expect("session directory");
        fs::write(
            project.join("2026-01-01T00-00-00Z_session-1.jsonl"),
            concat!(
                "{\"type\":\"title\",\"title\":\"Most recent title\",\"pad\":\" \"}\n",
                "{\"type\":\"session\",\"id\":\"session-1\",\"cwd\":\"/work/project\"}\n",
                "{\"type\":\"message\",\"id\":\"m1\",\"parentId\":\"session-1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Fallback prompt\"}]}}\n",
            ),
        )
        .expect("session fixture");

        let sessions =
            discover_in(std::slice::from_ref(&scratch), None, None).expect("session discovery");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Most recent title");
        assert_eq!(sessions[0].project, "project");

        fs::remove_dir_all(&scratch).expect("scratch cleanup");
    }

    #[test]
    fn indexed_title_takes_precedence_over_the_transcript_slot() {
        let scratch = scratch_directory();
        let project = scratch.join("sessions/-work-project");
        fs::create_dir_all(&project).expect("session directory");
        fs::write(
            project.join("2026-01-01T00-00-00Z_session-2.jsonl"),
            concat!(
                "{\"type\":\"title\",\"title\":\"Older slot\",\"pad\":\" \"}\n",
                "{\"type\":\"session\",\"id\":\"session-2\",\"cwd\":\"/work/project\"}\n",
                "{\"type\":\"message\",\"id\":\"m1\",\"parentId\":\"session-2\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Prompt\"}]}}\n",
            ),
        )
        .expect("session fixture");
        let connection =
            rusqlite::Connection::open(scratch.join("history.db")).expect("title index opens");
        connection
            .execute_batch(
                "CREATE TABLE session_titles (session_id TEXT PRIMARY KEY, title TEXT NOT NULL);\
                 INSERT INTO session_titles VALUES ('session-2', 'Indexed title');",
            )
            .expect("title index fixture");
        drop(connection);

        let sessions =
            discover_in(std::slice::from_ref(&scratch), None, None).expect("session discovery");
        assert_eq!(sessions[0].title, "Indexed title");

        fs::remove_dir_all(&scratch).expect("scratch cleanup");
    }

    #[test]
    fn discovers_private_sessions_as_omperor_owned() {
        let scratch = scratch_directory();
        let private = scratch.join("private");
        let private_data = scratch.join("data");
        fs::create_dir_all(&private).expect("private session directory");
        write_journal_fixture(
            &private.join("private-id.oms"),
            "/work/project",
            "Private prompt",
        );
        // A transcript written by the pre-journal OMP stays readable but the
        // bundled OMP can no longer resume it, so it is not Omperor-owned.
        fs::write(
            private.join("legacy-private.jsonl"),
            concat!(
                "{\"v\":4,\"id\":\"legacy-id\",\"created\":1,\"cwd\":\"/work/project\"}\n",
                "{\"ts\":2,\"k\":\"turn_input\",\"item\":{\"kind\":{\"Message\":{\"role\":2,\"parts\":[{\"kind\":{\"Text\":\"Legacy prompt\"}}]}}}}\n",
            ),
        )
        .expect("legacy session fixture");
        let private_index = private_data.join("projects/project/sessions.sqlite3");
        fs::create_dir_all(private_index.parent().expect("index has a parent"))
            .expect("private data directory");
        let connection =
            rusqlite::Connection::open(private_index).expect("private title index opens");
        connection
            .execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, title TEXT);\
                 INSERT INTO sessions VALUES ('private-id', 'Private indexed title');",
            )
            .expect("private title fixture");
        drop(connection);

        let sessions = discover_in(&[], Some(&private), Some(&private_data))
            .expect("private session discovery");
        assert_eq!(sessions.len(), 2);
        let journal = sessions
            .iter()
            .find(|session| session.id == "private-id")
            .expect("journal session listed");
        assert!(journal.is_omperor_owned());
        assert_eq!(journal.title, "Private indexed title");
        assert_eq!(journal.project, "project");
        let legacy = sessions
            .iter()
            .find(|session| session.id == "legacy-id")
            .expect("legacy session listed");
        assert!(!legacy.is_omperor_owned());
        assert_eq!(legacy.title, "Legacy prompt");

        fs::remove_dir_all(&scratch).expect("scratch cleanup");
    }

    #[test]
    fn publishes_omperor_sessions_before_scanning_external_omp_roots() {
        let scratch = scratch_directory();
        let private = scratch.join("private");
        let private_data = scratch.join("data");
        let external = scratch.join("external");
        fs::create_dir_all(&private).expect("private session directory");
        fs::create_dir_all(external.join("sessions")).expect("external session directory");
        let journal = |id: &str, prompt: &str| {
            format!(
                "{{\"v\":4,\"id\":\"{id}\",\"created\":1,\"cwd\":\"/work/project\"}}\n\
                 {{\"ts\":2,\"k\":\"turn_input\",\"item\":{{\"kind\":{{\"Message\":{{\"role\":2,\"parts\":[{{\"kind\":{{\"Text\":\"{prompt}\"}}}}]}}}}}}}}\n"
            )
        };
        write_journal_fixture(
            &private.join("native.oms"),
            "/work/project",
            "Native prompt",
        );
        fs::write(
            external.join("sessions/foreign.jsonl"),
            journal("foreign", "Foreign prompt"),
        )
        .expect("external session fixture");

        let (sender, receiver) = flume::bounded(2);
        stream_discovery_with_roots(&private, &private_data, sender, || {
            assert_eq!(
                receiver.len(),
                1,
                "native results must be published before external discovery starts"
            );
            Ok(vec![external])
        });

        let DiscoveryUpdate::Omperor(native) = receiver.recv().expect("native discovery update")
        else {
            panic!("native discovery must be the first update");
        };
        let native = native.expect("native discovery succeeds");
        assert_eq!(native.len(), 1);
        assert!(native[0].is_omperor_owned());

        let DiscoveryUpdate::Omp(foreign) = receiver.recv().expect("OMP discovery update") else {
            panic!("OMP discovery must be the second update");
        };
        let foreign = foreign.expect("OMP discovery succeeds");
        assert_eq!(foreign.len(), 1);
        assert!(!foreign[0].is_omperor_owned());

        fs::remove_dir_all(&scratch).expect("scratch cleanup");
    }

    fn scratch_directory() -> PathBuf {
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "omperor-session-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("scratch directory");
        path
    }

    #[test]
    fn journal_ids_keep_underscores_and_legacy_transcripts_drop_their_timestamp() {
        assert_eq!(super::session_id_from_path(std::path::Path::new("/s/auth_scan.oms")), "auth_scan");
        assert_eq!(super::session_id_from_path(std::path::Path::new("/s/01M4A5FZ587GGERJ3YT2AA20RV.oms")), "01M4A5FZ587GGERJ3YT2AA20RV");
        assert_eq!(super::session_id_from_path(std::path::Path::new("/s/2026-09-01T10-00-00_abc.jsonl")), "abc");
    }
}
