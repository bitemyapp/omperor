//! The program: Omperor's state, the messages that change it, and the
//! work it starts away from the interface thread.
//!
//! Nothing slow happens on the interface thread (AGENTS.md): reading
//! OMP's journals and Omperor's registry, git, the file index, Markdown and
//! syntax colouring, attachments, archives all run on the kit's workers
//! (`Cx::spawn`) or on threads of their own, and answer with messages.
//! Each live session's OMP child runs on a thread of the runtime, whose
//! events wake the program through [`crate::wake`]; while any session is
//! working, a frame timer drains the text it streams.

mod accounts;
mod boot;
mod changes;
mod compose;
mod insights;
mod live;
mod projects;
mod sessions;
mod sheets;
mod subagents;
mod tools;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aizome_kit::prelude::*;
use aizome_kit::EditorContent;

pub(crate) use boot::Boot;
pub(crate) use insights::{MetricsPage, TimelineChange, TimelineWindow, UsageWindow};

use crate::agents;
use crate::archive;
use crate::attachments;
use crate::chat::TranscriptMessage;
use crate::housekeeping;
use crate::live_sessions::{LiveSession, LiveSessions, SessionKey};
use crate::markdown::Doc;
use crate::omp_runtime::{AccountUsage, AuthMethod, AuthPrompt, AuthProvider};
use crate::preferences::{self, Preferences, Project, SessionRecord, StoredAccount};
use crate::rules::{AccountEntry, SessionSourceFilter};
use crate::session::SessionSummary;
use crate::workspace;

/// How often streamed text is drawn while a session works.
const FRAME: Duration = Duration::from_millis(33);
/// How long runtime events may be applied in one pass before the rest wait
/// for the next.
const DRAIN_BUDGET: Duration = Duration::from_millis(6);
/// How long after the last edit a draft is saved.
const SAVE_VIEWS_AFTER: Duration = Duration::from_secs(2);

/// The panel beside the conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Panel {
    #[default]
    Changes,
    Agents,
}

impl Panel {
    pub(crate) fn index(self) -> isize {
        match self {
            Panel::Changes => 0,
            Panel::Agents => 1,
        }
    }

    pub(crate) fn from_index(index: isize) -> Panel {
        if index == 1 { Panel::Agents } else { Panel::Changes }
    }
}

/// The pages of the Settings window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Pane {
    #[default]
    General,
    Accounts,
    Projects,
    Archives,
    Data,
    Metrics,
    Developer,
}

impl Pane {
    pub(crate) const ALL: [Pane; 7] =
        [Pane::General, Pane::Accounts, Pane::Projects, Pane::Archives, Pane::Data, Pane::Metrics, Pane::Developer];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Accounts => "Accounts",
            Pane::Projects => "Projects",
            Pane::Archives => "Archives",
            Pane::Data => "Sessions & Data",
            Pane::Metrics => "Metrics",
            Pane::Developer => "Developer",
        }
    }

    pub(crate) fn icon(self) -> Icon {
        match self {
            Pane::General => Icon::Settings,
            Pane::Accounts => Icon::UserRound,
            Pane::Projects => Icon::FolderGit,
            Pane::Archives => Icon::Archive,
            Pane::Data => Icon::HardDrive,
            Pane::Metrics => Icon::Gauge,
            Pane::Developer => Icon::Wrench,
        }
    }
}

/// One transcript row as drawn: the message, and its body parsed.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub(crate) message: Rc<TranscriptMessage>,
    /// A hash of the body, to tell when it changed.
    pub(crate) hash: u64,
    /// The body parsed, for answers and prompts; the hash it was parsed at.
    pub(crate) doc: Option<(u64, Arc<Doc>)>,
    pub(crate) parsing: bool,
}

/// What the `@` and `/` pickers above the composer offer.
#[derive(Debug, Clone, Default)]
pub(crate) struct Picker {
    pub(crate) kind: PickerKind,
    /// Where the `@` or `/` is, in characters.
    pub(crate) start: usize,
    pub(crate) items: Vec<PickerItem>,
    pub(crate) selected: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PickerKind {
    #[default]
    File,
    Command,
}

#[derive(Debug, Clone)]
pub(crate) struct PickerItem {
    pub(crate) label: String,
    pub(crate) detail: String,
    /// What replaces the `@query` or `/query`.
    pub(crate) insert: String,
}

/// A file's diff in the Changes panel.
#[derive(Debug, Clone)]
pub(crate) enum Diff {
    Loading,
    Ready(Arc<Vec<Vec<crate::markdown::Run>>>),
    Failed(String),
}

/// What a session's window shows besides the session itself.
pub(crate) struct SessionView {
    pub(crate) editor: EditorContent,
    pub(crate) rows: Vec<Row>,
    /// Activity rows open to show their detail, by message index.
    pub(crate) expanded: HashSet<usize>,
    /// Bumped to bring the transcript to its end (a prompt sent).
    pub(crate) scroll_token: u64,
    /// The panel, the agents' filter, the draft: what's saved of it.
    pub(crate) view: agents::ViewState,
    pub(crate) picker: Option<Picker>,
    pub(crate) diff: Option<(PathBuf, Diff)>,
    pub(crate) agent_rows: HashMap<String, Vec<Row>>,
    /// Output tokens a second, a reading a second while answers stream.
    pub(crate) rates: VecDeque<f64>,
}

impl SessionView {
    fn new(view: agents::ViewState) -> SessionView {
        SessionView {
            editor: EditorContent::new(&view.desk.draft),
            rows: Vec::new(),
            expanded: HashSet::new(),
            scroll_token: 0,
            view,
            picker: None,
            diff: None,
            agent_rows: HashMap::new(),
            rates: VecDeque::new(),
        }
    }

    pub(crate) fn panel(&self) -> Option<Panel> {
        self.view.desk.workspace_inspector.then(|| Panel::from_index(self.view.desk.panel))
    }
}

/// The state of a sign-in, from choosing a method to the account
/// connected.
#[derive(Debug, Clone)]
pub(crate) struct SignIn {
    pub(crate) provider: AuthProvider,
    pub(crate) method: AuthMethod,
    /// The live session whose OMP child runs it.
    pub(crate) owner: SessionKey,
    pub(crate) generation: u64,
    /// OMP's id for it, once acknowledged.
    pub(crate) session: Option<String>,
    pub(crate) busy: bool,
    pub(crate) cancelling: bool,
    pub(crate) url: Option<(String, Option<String>)>,
    pub(crate) device: Option<(String, String)>,
    pub(crate) waiting: Option<String>,
    pub(crate) prompt: Option<AuthPrompt>,
    /// What the user typed: a key, a token, a prompt's answer.
    pub(crate) secret: String,
    pub(crate) submitting: bool,
    pub(crate) failure: Option<String>,
    /// The window the sheet is over, and the sheet.
    pub(crate) sheet: WindowKey,
}

/// A question asked in a sheet, with a text field.
#[derive(Debug, Clone)]
pub(crate) struct Prompt {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) value: String,
    pub(crate) confirm: String,
    pub(crate) action: PromptAction,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum PromptAction {
    RenameProject(String),
    LabelAccount(String),
    ForkBranch { summary: Box<SessionSummary>, directory: PathBuf, project_id: String, point: workspace::BranchPoint },
}

/// A choice asked in a sheet.
#[derive(Debug, Clone)]
pub(crate) struct Choice {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) options: Vec<String>,
    pub(crate) selected: usize,
    pub(crate) confirm: String,
    pub(crate) action: ChoiceAction,
}

#[derive(Debug, Clone)]
pub(crate) enum ChoiceAction {
    /// Which folder of a project a new session works in.
    Folder { project_id: String, folders: Vec<PathBuf> },
    /// Which account serves a provider (ids, in the options' order).
    Account { provider: AuthProvider, model: Option<String>, accounts: Vec<String> },
}

/// Where a fork into a worktree would branch from: the session, its folder,
/// the project, and the branch point git found.
pub(crate) type ForkProbe = (SessionSummary, PathBuf, String, Result<Option<workspace::BranchPoint>, String>);

/// A worktree made for a fork: the session, the worktree, the branch, how
/// `git worktree add` went, and the branch point.
pub(crate) type WorktreeAdded = (String, PathBuf, String, Result<(), String>, workspace::BranchPoint);

/// What a sheet is for.
#[derive(Debug, Clone)]
pub(crate) enum Sheet {
    SignIn,
    Prompt(Prompt),
    Choice(Choice),
}

/// Work under way that the window shows an indicator for.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub(crate) label: String,
}

pub(crate) struct App {
    // Windows.
    pub(crate) main: WindowKey,
    pub(crate) settings: Option<WindowKey>,
    pub(crate) pane: Pane,
    pub(crate) timeline: Option<TimelineWindow>,
    pub(crate) usage_window: Option<UsageWindow>,
    pub(crate) metrics_page: MetricsPage,
    /// Numbers each read whose answer may arrive after a newer one.
    pub(crate) next_generation: u64,
    /// The user said to quit although an agent was working.
    pub(crate) quit_confirmed: bool,
    pub(crate) sheets: HashMap<WindowKey, Sheet>,
    // Startup.
    pub(crate) booted: bool,
    pub(crate) boot_error: Option<String>,
    pub(crate) data_dir: Option<PathBuf>,
    pub(crate) sessions_dir: Option<PathBuf>,
    pub(crate) preferences: Preferences,
    // The registry.
    pub(crate) projects: Vec<Project>,
    pub(crate) missing_directories: HashSet<PathBuf>,
    pub(crate) records: HashMap<String, SessionRecord>,
    pub(crate) positions: HashMap<String, i64>,
    pub(crate) bootstrap_pending: bool,
    // Sessions on disk.
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) discovering: bool,
    pub(crate) discovery: Option<flume::Receiver<crate::session::DiscoveryUpdate>>,
    pub(crate) discovery_error: Option<String>,
    pub(crate) removed_session_ids: HashSet<String>,
    // The sidebar.
    pub(crate) sidebar_query: String,
    pub(crate) sidebar_source: SessionSourceFilter,
    pub(crate) expanded_projects: HashSet<String>,
    // Live sessions.
    pub(crate) live: LiveSessions,
    pub(crate) views: HashMap<SessionKey, SessionView>,
    pub(crate) saved_views: HashMap<Option<PathBuf>, agents::ViewState>,
    pub(crate) save_views_timer: Option<TimerHandle>,
    pub(crate) pending_forks: HashMap<SessionKey, (SessionKey, String)>,
    // Accounts.
    pub(crate) accounts: Vec<AccountEntry>,
    pub(crate) account_metadata: Vec<StoredAccount>,
    pub(crate) account_choices: HashMap<(String, String), String>,
    pub(crate) account_usage: HashMap<String, AccountUsage>,
    pub(crate) usage_loading: bool,
    pub(crate) usage_fetched_ms: Option<u64>,
    pub(crate) expanded_accounts: HashSet<String>,
    pub(crate) sign_in: Option<SignIn>,
    pub(crate) sign_in_generation: u64,
    // Archives.
    pub(crate) archives: Option<Result<Vec<archive::Listing>, String>>,
    // Images loaded for answers, by path.
    pub(crate) images: Rc<HashMap<String, Image>>,
    pub(crate) images_loading: HashSet<String>,
    // Work and time.
    pub(crate) jobs: HashMap<u64, Job>,
    pub(crate) next_job: u64,
    pub(crate) frame: Option<TimerHandle>,
    pub(crate) clock: Option<TimerHandle>,
    pub(crate) note: Option<(String, Instant)>,
    pub(crate) quitting: bool,
    pub(crate) quit_deadline: Option<Instant>,
    /// Folders from the command line, opened once startup is done.
    pub(crate) pending_folders: Vec<PathBuf>,
    /// The editors, terminals and browsers found at boot.
    pub(crate) launchers: crate::launchers::Installed,
    /// The OMP helper new sessions run, as found at boot.
    pub(crate) helper: Option<PathBuf>,
    pub(crate) metrics: Option<crate::metrics::Log>,
    pub(crate) inspecting: HashSet<SessionKey>,
    pub(crate) inspection_generation: HashMap<SessionKey, u64>,
    pub(crate) watch_sender: flume::Sender<PathBuf>,
    pub(crate) watch_notices: flume::Receiver<PathBuf>,
    pub(crate) archive_origins: HashMap<u64, projects::Origin>,
    pub(crate) pending_tools: HashMap<(SessionKey, u64), crate::omperor_tools::Request>,
    /// A second launch's arguments come through here.
    pub(crate) listener: Option<std::os::unix::net::UnixListener>,
}

#[derive(Clone, Debug)]
pub(crate) enum Msg {
    // Life.
    Booted(Arc<Result<Boot, String>>),
    Wake,
    Frame,
    Tick,
    QuitPoll,
    WatchSettled,
    UsageRead(u64, SessionKey, usize, Arc<crate::usage_card::Body>),
    Launch(Vec<String>),
    ClearNote,
    SaveViews,
    Saved(Result<(), String>),
    // The sidebar.
    Search(String),
    Source(usize),
    ToggleProject(String),
    ShowMore(String),
    Open(String),
    SelectLive(SessionKey),
    NewSession(Option<String>),
    NewProject,
    ProjectFolders(Option<Vec<PathBuf>>),
    ProjectPrepared(u64, Arc<Result<(Project, PathBuf), String>>),
    Fork(String),
    ForkIntoWorktree(String),
    ForkProbed(u64, Arc<ForkProbe>),
    WorktreeAdded(u64, Arc<WorktreeAdded>),
    DeleteSession(String),
    ConfirmDeleteSession(String),
    Deleted(u64, Arc<housekeeping::Report>),
    StopSession(SessionKey),
    MoveProject(String, isize),
    MoveSession(String, isize),
    FocusSearch,
    // Projects.
    RenameProject(String),
    AddFolder(String),
    FolderChosen(String, Option<Vec<PathBuf>>),
    RemoveFolder(String, PathBuf),
    RemoveProject(String),
    ConfirmRemoveProject(String),
    ArchiveProject(String),
    ConfirmArchiveProject(String),
    Archived(u64, Arc<Result<archive::Archived, String>>),
    ArchivesListed(Arc<Result<Vec<archive::Listing>, String>>),
    RestoreArchive(String),
    Restored(u64, Arc<Result<archive::Restored, String>>),
    DeleteArchive(String),
    ConfirmDeleteArchive(String),
    ArchiveDeleted(u64, Arc<Result<u64, String>>),
    RegistryReloaded(Option<u64>, Arc<Result<preferences::Registry, String>>),
    // The composer.
    Edit(EditorAction),
    Send,
    Stop,
    HistoryBack,
    HistoryForward,
    PickerMove(isize),
    PickerChoose(Option<usize>),
    PickerDismiss,
    Attach,
    AttachChosen(Option<Vec<PathBuf>>),
    /// Files pasted, as the clipboard named them.
    AttachPaths(Vec<PathBuf>),
    PasteData(Vec<String>),
    Dropped(Vec<String>, Option<String>),
    AttachmentReady(SessionKey, u64, Arc<Result<attachments::PreparedAttachment, String>>),
    RemoveAttachment(u64),
    PromptReady(SessionKey, u64, Arc<attachments::PreparedPrompt>),
    PickModel(String),
    PickThinking(String),
    PickAccount(String),
    ToggleFast(bool),
    ProjectFiles(SessionKey, PathBuf, Arc<crate::file_picker::ProjectIndex>),
    // The transcript.
    Parsed(SessionKey, Option<String>, usize, u64, Arc<Doc>),
    ToggleRow(usize),
    Copy(String),
    OpenLink(String),
    /// Hand `target` to the desktop as it is.
    OpenUrl(String),
    ImageLoaded(String, Option<Image>),
    // The panel beside the conversation.
    ShowPanel(Panel),
    TogglePanel,
    Inspected(SessionKey, u64, Arc<Result<workspace::WorkspaceSnapshot, String>>),
    RefreshChanges,
    SelectChange(PathBuf),
    ToggleChangeDir(PathBuf),
    DiffLoaded(SessionKey, PathBuf, Arc<Result<Vec<Vec<crate::markdown::Run>>, String>>),
    OpenInEditor(PathBuf),
    OpenTerminal(PathBuf),
    /// Open a folder in the file manager.
    ShowInFiles(PathBuf),
    Launched(Result<(), String>),
    SelectAgent(Option<String>),
    AgentScope(usize),
    AgentQuery(String),
    TranscriptsSynced(SessionKey, Arc<crate::transcripts::SyncResult>),
    DiscussAgent(String),
    // Looking back: the Timeline and Usage windows, Settings' Metrics.
    ShowTimeline(Option<String>),
    TimelineUpdate(u64, Arc<crate::timeline::Update>),
    TimelineFilter(TimelineChange),
    TimelinePick(Option<usize>),
    TimelineSlowest,
    ShowUsage,
    UsageReport(u64, Arc<crate::usage::Report>),
    UsagePage(usize),
    RefreshUsageReport,
    LoadMetrics,
    MetricsRead(u64, Arc<insights::MetricsRead>),
    MetricsScope(usize),
    // Settings and accounts.
    OpenSettings(Pane),
    ShowPane(Pane),
    SetDeveloper(bool),
    SetBackend(usize),
    SetComputerUse(bool),
    SetTextSize(usize),
    SetInlineImages(bool),
    SetRemoteImages(bool),
    SetLauncher(crate::launchers::LauncherKind, usize),
    SetDefaultModel(String),
    SetDefaultThinking(String),
    RefreshModels,
    InsertExample,
    SweepEmpty,
    Swept(u64, Arc<Result<housekeeping::Sweep, String>>),
    SignIn(AuthProvider),
    SignInMethod(AuthMethod),
    SignInSecret(String),
    SignInSubmit,
    SignInOpenPage,
    SignInCopyCode,
    SignInCancel,
    RenameAccount(String),
    MakeDefault(String),
    RemoveAccount(String),
    ConfirmRemoveAccount(String),
    AccountsStored(Arc<Result<Vec<StoredAccount>, String>>),
    RefreshUsage,
    ToggleAccount(String),
    /// Hide the account alert: for half an hour, or until the next launch.
    SnoozeAlert(bool),
    // Sheets.
    SheetValue(String),
    SheetChoose(usize),
    SheetConfirm,
    SheetCancel,
    // The host tools' answers.
    ToolAnswered(SessionKey, u64, Arc<Result<serde_json::Value, String>>),
    ConfirmTool(SessionKey, u64, bool),
    ConfirmQuit,
}

impl App {
    pub(crate) fn new(listener: Option<std::os::unix::net::UnixListener>) -> App {
        let (watch_sender, watch_notices) = flume::unbounded();
        App {
            main: WindowKey::NONE,
            settings: None,
            pane: Pane::General,
            timeline: None,
            usage_window: None,
            metrics_page: MetricsPage::default(),
            next_generation: 0,
            quit_confirmed: false,
            sheets: HashMap::new(),
            booted: false,
            boot_error: None,
            data_dir: None,
            sessions_dir: None,
            preferences: Preferences::default(),
            projects: Vec::new(),
            missing_directories: HashSet::new(),
            records: HashMap::new(),
            positions: HashMap::new(),
            bootstrap_pending: false,
            sessions: Vec::new(),
            discovering: false,
            discovery: None,
            discovery_error: None,
            removed_session_ids: HashSet::new(),
            sidebar_query: String::new(),
            sidebar_source: SessionSourceFilter::Omperor,
            expanded_projects: HashSet::new(),
            live: LiveSessions::default(),
            views: HashMap::new(),
            saved_views: HashMap::new(),
            save_views_timer: None,
            pending_forks: HashMap::new(),
            accounts: Vec::new(),
            account_metadata: Vec::new(),
            account_choices: HashMap::new(),
            account_usage: HashMap::new(),
            usage_loading: false,
            usage_fetched_ms: None,
            expanded_accounts: HashSet::new(),
            sign_in: None,
            sign_in_generation: 0,
            archives: None,
            images: Rc::new(HashMap::new()),
            images_loading: HashSet::new(),
            jobs: HashMap::new(),
            next_job: 0,
            frame: None,
            clock: None,
            note: None,
            quitting: false,
            quit_deadline: None,
            pending_folders: Vec::new(),
            launchers: crate::launchers::Installed::default(),
            helper: None,
            metrics: None,
            inspecting: HashSet::new(),
            inspection_generation: HashMap::new(),
            watch_sender,
            watch_notices,
            archive_origins: HashMap::new(),
            pending_tools: HashMap::new(),
            listener,
        }
    }

    /// The session in view.
    pub(crate) fn session(&self) -> Rc<LiveSession> {
        self.live.selected()
    }

    /// The session in view's window state.
    pub(crate) fn view_of(&self, key: SessionKey) -> Option<&SessionView> {
        self.views.get(&key)
    }

    /// A session's window state, made when first asked for.
    pub(crate) fn view_mut(&mut self, key: SessionKey) -> &mut SessionView {
        let saved = &self.saved_views;
        let live = &self.live;
        self.views.entry(key).or_insert_with(|| {
            let path = live.get(key).and_then(|s| s.selected_session_path.borrow().clone());
            SessionView::new(saved.get(&path).cloned().unwrap_or_default())
        })
    }

    /// Start a job the window shows as under way.
    pub(crate) fn begin_job(&mut self, label: impl Into<String>) -> u64 {
        self.next_job += 1;
        self.jobs.insert(self.next_job, Job { label: label.into() });
        self.next_job
    }

    pub(crate) fn finish_job(&mut self, job: u64) {
        self.jobs.remove(&job);
    }

    /// The work under way, newest first, for the status line.
    pub(crate) fn activity(&self) -> Option<String> {
        let session = self.session();
        session
            .activity_label()
            .map(str::to_owned)
            .or_else(|| self.jobs.iter().max_by_key(|(id, _)| **id).map(|(_, job)| job.label.clone()))
            .or_else(|| self.discovering.then(|| "Reading sessions…".to_owned()))
    }

    /// Say something in the status line for a while.
    pub(crate) fn note(&mut self, text: impl Into<String>, cx: &mut Cx<Msg>) {
        self.note = Some((text.into(), Instant::now()));
        cx.after(Duration::from_secs(6), Msg::ClearNote);
    }

    pub(crate) fn project_by_id(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// The registry key a session's account choices are kept under.
    pub(crate) fn project_key(&self, session: &LiveSession) -> String {
        session
            .active_project_id
            .borrow()
            .clone()
            .unwrap_or_else(|| session.current_project.borrow().to_string_lossy().into_owned())
    }

    /// Write to the registry or preferences on a worker; failures are said.
    pub(crate) fn write(
        &mut self,
        cx: &mut Cx<Msg>,
        work: impl FnOnce() -> Result<(), preferences::PreferencesError> + Send + 'static,
    ) {
        cx.spawn(move || Msg::Saved(work().map_err(|e| e.to_string())));
    }

    pub(crate) fn store_preferences(&mut self, cx: &mut Cx<Msg>) {
        let preferences = self.preferences.clone();
        self.write(cx, move || preferences::store(&preferences));
    }

    /// Keep the frame timer running while anything streams, and the clock
    /// while anything shows time passing.
    pub(crate) fn pace(&mut self, cx: &mut Cx<Msg>) {
        let busy = self.live.entries().iter().any(|s| {
            s.chat_busy.get()
                || s.runtime_starting.get()
                || s.attachment_submission.get()
                || s.subagents.borrow().iter().any(|a| agents::AgentState::from_status(&a.status).is_busy())
                || matches!(
                    s.lifecycle.get(),
                    crate::live_sessions::Lifecycle::Stopping | crate::live_sessions::Lifecycle::Retiring
                )
        }) || self.sign_in.as_ref().is_some_and(|s| s.busy);
        match (busy, self.frame) {
            (true, None) => self.frame = Some(cx.every(FRAME, Msg::Frame)),
            (false, Some(timer)) => {
                cx.cancel(timer);
                self.frame = None;
            }
            _ => {}
        }
        let ticking = busy || self.session().cache_clock.borrow().warning(crate::rules::wall_clock_ms()).is_some();
        match (ticking, self.clock) {
            (true, None) => self.clock = Some(cx.every(Duration::from_secs(1), Msg::Tick)),
            (false, Some(timer)) => {
                cx.cancel(timer);
                self.clock = None;
            }
            _ => {}
        }
    }

    /// Save every session's arrangement soon (debounced).
    pub(crate) fn schedule_save_views(&mut self, cx: &mut Cx<Msg>) {
        if let Some(timer) = self.save_views_timer.take() {
            cx.cancel(timer);
        }
        self.save_views_timer = Some(cx.after(SAVE_VIEWS_AFTER, Msg::SaveViews));
    }

    /// Capture each live session's arrangement into the saved views.
    pub(crate) fn capture_views(&mut self) {
        for session in self.live.entries() {
            let Some(view) = self.views.get_mut(&session.key) else { continue };
            view.view.desk.draft = view.editor.text();
            view.view.desk.attachments = session.pending_attachments.borrow().saved();
            let path = session.selected_session_path.borrow().clone();
            // Sent, the draft is the session's journal now: what was saved
            // of it as a draft goes.
            if path.as_deref().and_then(crate::session_state::draft_key).is_none() {
                self.saved_views.remove(&Some(crate::session_state::draft_path(session.key.0)));
            }
            // A draft not yet sent keeps where and how it was to be sent.
            view.view.desk.draft_context = path.as_deref().and_then(crate::session_state::draft_key).map(|_| {
                crate::session_state::DraftContext {
                    project: session.current_project.borrow().clone(),
                    project_id: session.active_project_id.borrow().clone(),
                    model: session.current_model.borrow().clone(),
                    account: session.current_account.borrow().clone(),
                    thinking_level: session.current_thinking_level.borrow().clone(),
                    fast_mode: session.fast_mode.get(),
                }
            });
            if path.is_some() {
                self.saved_views.insert(path, view.view.clone());
            }
        }
    }

    pub(crate) fn save_views(&mut self, cx: &mut Cx<Msg>) {
        self.save_views_timer = None;
        self.capture_views();
        let saved = crate::session_state::SavedViews::capture(&self.saved_views);
        self.write(cx, move || preferences::store_session_views(&saved));
    }
}

impl Program for App {
    type Msg = Msg;

    fn info(&self) -> AppInfo {
        let channel = crate::app_state::Channel::current();
        AppInfo::new(crate::APP_ID, channel.app_name(), "Coding Agents")
            .icon(Icon::Crown)
            .version(env!("CARGO_PKG_VERSION"))
    }

    fn launched(&mut self, cx: &mut Cx<Msg>) {
        let sender = cx.stream();
        crate::wake::install(move || {
            sender.send(Msg::Wake);
        });
        if let Some(listener) = self.listener.take() {
            let sender = cx.stream();
            crate::instance::listen(listener, move |args| sender.send(Msg::Launch(args)));
        }
        let name = crate::app_state::Channel::current().app_name();
        // OMPEROR_WINDOW=WxH opens it at another size (screenshots, tests).
        let (width, height) = std::env::var("OMPEROR_WINDOW")
            .ok()
            .and_then(|s| {
                let (w, h) = s.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or((1320.0, 840.0));
        self.main = cx.open_window(WindowSpec::new(name).size(width, height).min_size(860.0, 560.0));
        // The grammars load slowly: a thread loads them now, before the
        // first answer with code needs them.
        let _ = std::thread::Builder::new().name("omperor-syntax".into()).spawn(crate::markdown::highlight::warm_up);
        let job = self.begin_job("Opening Omperor…");
        cx.spawn(move || {
            let booted = Boot::load().map_err(|e| e.to_string());
            let _ = job;
            Msg::Booted(Arc::new(booted))
        });
    }

    fn update(&mut self, msg: Msg, cx: &mut Cx<Msg>) {
        match msg {
            Msg::Booted(result) => self.booted(result, cx),
            Msg::Wake => {
                crate::wake::taken();
                self.drain(cx);
            }
            Msg::Frame => self.drain(cx),
            Msg::Tick => self.tick(),
            Msg::QuitPoll => self.quit_poll(cx),
            Msg::WatchSettled => self.watch_settled(cx),
            Msg::UsageRead(job, key, index, body) => self.usage_read(job, key, index, &body),
            Msg::Launch(args) => {
                cx.activate(self.main);
                for path in args.iter().filter(|a| !a.starts_with('-')) {
                    self.open_directory(PathBuf::from(path), cx);
                }
            }
            Msg::ClearNote => {
                if self.note.as_ref().is_some_and(|(_, at)| at.elapsed() >= Duration::from_secs(5)) {
                    self.note = None;
                }
            }
            Msg::SaveViews => self.save_views(cx),
            Msg::Saved(result) => {
                if let Err(error) = result {
                    self.note(format!("Could not save: {error}"), cx);
                }
            }
            // The sidebar and sessions.
            Msg::Search(query) => self.sidebar_query = query,
            Msg::Source(index) => self.sidebar_source = SessionSourceFilter::from_segment(index),
            Msg::ToggleProject(id) => self.toggle_project(id, cx),
            Msg::ShowMore(id) => {
                if !self.expanded_projects.remove(&id) {
                    self.expanded_projects.insert(id);
                }
            }
            Msg::Open(id) => self.open_session(&id, cx),
            Msg::SelectLive(key) => {
                if self.live.select(key) {
                    self.view_mut(key);
                }
            }
            Msg::NewSession(project) => self.new_session(project, cx),
            Msg::NewProject => cx.open_panel(
                self.main,
                FileDialog::new().title("Choose a project folder").directories(),
                Msg::ProjectFolders,
            ),
            Msg::ProjectFolders(paths) => {
                if let Some(path) = paths.and_then(|p| p.into_iter().next()) {
                    self.open_directory(path, cx);
                }
            }
            Msg::ProjectPrepared(job, result) => self.project_prepared(job, &result, cx),
            Msg::Fork(id) => self.fork_in_place(&id, cx),
            Msg::ForkIntoWorktree(id) => self.fork_into_worktree(&id, cx),
            Msg::ForkProbed(job, probe) => self.fork_probed(job, &probe, cx),
            Msg::WorktreeAdded(job, added) => self.worktree_added(job, &added, cx),
            Msg::DeleteSession(id) => self.ask_delete_session(&id, cx),
            Msg::ConfirmDeleteSession(id) => self.delete_session(&id, cx),
            Msg::Deleted(job, report) => self.deleted(job, &report, cx),
            Msg::StopSession(key) => self.stop(key, cx),
            Msg::MoveProject(id, by) => self.move_project(&id, by, cx),
            Msg::MoveSession(id, by) => self.move_session(&id, by, cx),
            Msg::FocusSearch => cx.focus("session-search"),
            // Projects and archives.
            Msg::RenameProject(id) => self.ask_rename_project(&id, cx),
            Msg::AddFolder(id) => {
                cx.open_panel(self.main_or_settings(), FileDialog::new().title("Add a folder").directories(), move |p| {
                    Msg::FolderChosen(id.clone(), p)
                })
            }
            Msg::FolderChosen(id, paths) => self.folder_chosen(&id, paths, cx),
            Msg::RemoveFolder(id, folder) => self.remove_folder(&id, folder, cx),
            Msg::RemoveProject(id) => self.ask_remove_project(&id, cx),
            Msg::ConfirmRemoveProject(id) => self.remove_project(&id, cx),
            Msg::ArchiveProject(id) => self.ask_archive_project(&id, cx),
            Msg::ConfirmArchiveProject(id) => self.archive_project(&id, cx),
            Msg::Archived(job, result) => self.archived(job, &result, cx),
            Msg::ArchivesListed(result) => self.archives = Some((*result).clone()),
            Msg::RestoreArchive(id) => self.restore_archive(&id, cx),
            Msg::Restored(job, result) => self.restored(job, &result, cx),
            Msg::DeleteArchive(id) => self.ask_delete_archive(&id, cx),
            Msg::ConfirmDeleteArchive(id) => self.delete_archive(&id, cx),
            Msg::ArchiveDeleted(job, result) => self.archive_deleted(job, &result, cx),
            Msg::RegistryReloaded(job, result) => {
                if let Some(job) = job {
                    self.finish_job(job);
                }
                self.registry_reloaded(&result);
            }
            // The composer.
            Msg::Edit(action) => self.edit(action, cx),
            Msg::Send => self.send(cx),
            Msg::Stop => {
                let key = self.session().key;
                self.stop(key, cx);
            }
            Msg::HistoryBack => self.recall(-1),
            Msg::HistoryForward => self.recall(1),
            Msg::PickerMove(delta) => self.picker_move(delta),
            Msg::PickerChoose(index) => self.picker_choose(index),
            Msg::PickerDismiss => {
                let key = self.session().key;
                self.view_mut(key).picker = None;
            }
            Msg::Attach => cx.open_panel(self.main, FileDialog::new().title("Attach files").multiple(), Msg::AttachChosen),
            Msg::AttachChosen(paths) => {
                if let Some(paths) = paths {
                    self.attach_paths(paths, cx);
                }
            }
            Msg::AttachPaths(paths) => {
                if !paths.is_empty() {
                    self.attach_paths(paths, cx);
                }
            }
            Msg::PasteData(types) => self.paste_data(&types, cx),
            Msg::Dropped(urls, text) => self.dropped(urls, text, cx),
            Msg::AttachmentReady(key, id, result) => self.attachment_ready(key, id, &result, cx),
            Msg::RemoveAttachment(id) => {
                let session = self.session();
                session.pending_attachments.borrow_mut().remove(id);
                self.schedule_save_views(cx);
            }
            Msg::PromptReady(key, generation, prompt) => self.prompt_ready(key, generation, (*prompt).clone(), cx),
            Msg::PickModel(model) => self.request_model(model, cx),
            Msg::PickThinking(level) => self.request_thinking(level, cx),
            Msg::PickAccount(account) => self.request_account(account, cx),
            Msg::ToggleFast(on) => self.request_fast(on, cx),
            Msg::ProjectFiles(key, dir, index) => self.project_files(key, dir, index, cx),
            // The transcript.
            Msg::Parsed(key, agent, index, hash, doc) => self.parsed(key, agent, index, hash, doc, cx),
            Msg::ToggleRow(index) => {
                let key = self.session().key;
                let view = self.view_mut(key);
                if !view.expanded.remove(&index) {
                    view.expanded.insert(index);
                }
            }
            Msg::Copy(text) => {
                cx.set_clipboard_text(&text);
                self.note("Copied", cx);
            }
            Msg::OpenLink(target) => self.open_link(&target, cx),
            Msg::OpenUrl(target) => cx.open_url(&target),
            Msg::ImageLoaded(url, picture) => {
                self.images_loading.remove(&url);
                if let Some(picture) = picture {
                    Rc::make_mut(&mut self.images).insert(url, picture);
                }
            }
            // The panel.
            Msg::ShowPanel(panel) => self.show_panel(panel, cx),
            Msg::TogglePanel => {
                let key = self.session().key;
                let view = self.view_mut(key);
                view.view.desk.workspace_inspector = !view.view.desk.workspace_inspector;
                self.schedule_save_views(cx);
            }
            Msg::Inspected(key, generation, result) => self.inspected(key, generation, &result, cx),
            Msg::RefreshChanges => {
                let session = self.session();
                self.inspect_workspace(&session, cx);
            }
            Msg::SelectChange(path) => self.select_change(path, cx),
            Msg::ToggleChangeDir(path) => {
                let session = self.session();
                let mut collapsed = session.change_tree_collapsed.borrow_mut();
                if !collapsed.remove(&path) {
                    collapsed.insert(path);
                }
            }
            Msg::DiffLoaded(key, path, result) => self.diff_loaded(key, path, &result),
            Msg::OpenInEditor(path) => self.launch(crate::launchers::LauncherKind::Editor, path, cx),
            Msg::OpenTerminal(path) => self.launch(crate::launchers::LauncherKind::Terminal, path, cx),
            Msg::ShowInFiles(dir) => cx.open_url(&format!("file://{}", dir.display())),
            Msg::Launched(result) => {
                if let Err(error) = result {
                    self.note(error, cx);
                }
            }
            Msg::SelectAgent(id) => self.select_agent(id, cx),
            Msg::AgentScope(index) => {
                let key = self.session().key;
                self.view_mut(key).view.scope = match index {
                    1 => agents::WorkScope::Active,
                    2 => agents::WorkScope::Attention,
                    _ => agents::WorkScope::All,
                };
            }
            Msg::AgentQuery(query) => {
                let key = self.session().key;
                self.view_mut(key).view.query = query;
            }
            Msg::TranscriptsSynced(key, result) => self.transcripts_synced(key, &result, cx),
            Msg::DiscussAgent(id) => self.discuss_agent(&id),
            // Looking back.
            Msg::ShowTimeline(id) => self.show_timeline(id, cx),
            Msg::TimelineUpdate(generation, update) => self.timeline_update(generation, update),
            Msg::TimelineFilter(change) => self.timeline_filter(change),
            Msg::TimelinePick(row) => self.timeline_pick(row),
            Msg::TimelineSlowest => self.timeline_slowest(),
            Msg::ShowUsage => self.show_usage(cx),
            Msg::UsageReport(generation, report) => self.usage_report(generation, report),
            Msg::UsagePage(page) => {
                if let Some(window) = &mut self.usage_window {
                    window.page = page;
                }
            }
            Msg::RefreshUsageReport => self.read_usage(cx),
            Msg::LoadMetrics => self.load_metrics(cx),
            Msg::MetricsRead(generation, read) => self.metrics_read(generation, read),
            Msg::MetricsScope(index) => self.metrics_scope(index, cx),
            // Settings and accounts.
            Msg::OpenSettings(pane) => self.open_settings(pane, cx),
            Msg::ShowPane(pane) => self.show_pane(pane, cx),
            Msg::SetDeveloper(on) => {
                self.preferences.developer_mode = on;
                self.store_preferences(cx);
            }
            Msg::SetBackend(index) => {
                self.preferences.inference_backend =
                    if index == 1 { crate::mock_inference::Backend::Mock } else { crate::mock_inference::Backend::Omp };
                self.store_preferences(cx);
                self.note("New sessions use this backend", cx);
            }
            Msg::SetComputerUse(on) => {
                self.preferences.computer_use = on;
                self.store_preferences(cx);
            }
            Msg::SetTextSize(index) => {
                self.preferences.chat_text_size = match index {
                    0 => preferences::ChatTextSize::Compact,
                    2 => preferences::ChatTextSize::Large,
                    _ => preferences::ChatTextSize::Default,
                };
                self.store_preferences(cx);
            }
            Msg::SetInlineImages(on) => {
                self.preferences.show_inline_images = on;
                self.store_preferences(cx);
            }
            Msg::SetRemoteImages(on) => {
                self.preferences.load_remote_images = on;
                self.store_preferences(cx);
            }
            Msg::SetLauncher(kind, index) => self.set_launcher(kind, index, cx),
            Msg::SetDefaultModel(model) => {
                self.preferences.model = Some(model);
                self.store_preferences(cx);
                self.note("New sessions start with this model", cx);
            }
            Msg::SetDefaultThinking(level) => {
                self.preferences.thinking_level = Some(level);
                self.store_preferences(cx);
            }
            Msg::RefreshModels => self.refresh_models(cx),
            Msg::InsertExample => {
                let key = self.session().key;
                self.set_composer_text(key, crate::mock_inference::EXAMPLE);
                cx.activate(self.main);
                cx.focus("composer");
            }
            Msg::SweepEmpty => self.sweep_empty(cx),
            Msg::Swept(job, result) => self.swept(job, &result, cx),
            Msg::SignIn(provider) => self.begin_sign_in(provider, cx),
            Msg::SignInMethod(method) => self.sign_in_method(method, cx),
            Msg::SignInSecret(secret) => {
                if let Some(sign_in) = &mut self.sign_in {
                    sign_in.secret = secret;
                }
            }
            Msg::SignInSubmit => self.sign_in_submit(cx),
            Msg::SignInOpenPage => self.open_sign_in_page(cx),
            Msg::SignInCopyCode => {
                if let Some((code, _)) = self.sign_in.as_ref().and_then(|s| s.device.clone()) {
                    cx.set_clipboard_text(&code);
                    self.note("Code copied", cx);
                }
            }
            Msg::SignInCancel => self.cancel_sign_in(cx),
            Msg::RenameAccount(id) => self.ask_rename_account(&id, cx),
            Msg::MakeDefault(id) => self.make_default(&id, cx),
            Msg::RemoveAccount(id) => self.ask_remove_account(&id, cx),
            Msg::ConfirmRemoveAccount(id) => self.remove_account(&id, cx),
            Msg::AccountsStored(result) => match &*result {
                Ok(stored) => self.apply_account_metadata(stored.clone()),
                Err(error) => self.note(format!("Could not read accounts: {error}"), cx),
            },
            Msg::RefreshUsage => self.request_account_usage(true, cx),
            Msg::ToggleAccount(id) => {
                if !self.expanded_accounts.remove(&id) {
                    self.expanded_accounts.insert(id);
                }
            }
            Msg::SnoozeAlert(later) => {
                let session = self.session();
                let mut alerts = session.account_alerts.borrow_mut();
                let now = Instant::now();
                if let Some(alert) = alerts.visible(now).cloned() {
                    alerts.suppress(&alert, later, now);
                }
            }
            // Sheets.
            Msg::SheetValue(value) => self.sheet_value(value),
            Msg::SheetChoose(index) => self.sheet_choose(index),
            Msg::SheetConfirm => self.sheet_confirm(cx),
            Msg::SheetCancel => self.sheet_cancel(cx),
            Msg::ToolAnswered(key, call, result) => self.tool_answered(key, call, (*result).clone(), cx),
            Msg::ConfirmTool(key, call, approved) => self.confirm_tool(key, call, approved, cx),
            Msg::ConfirmQuit => {
                self.quit_confirmed = true;
                self.quit(cx);
            }
        }
        self.after_update(cx);
    }

    fn view(&self, window: WindowKey) -> Element<Msg> {
        crate::ui::view(self, window)
    }

    fn menus(&self) -> MenuBar<Msg> {
        crate::ui::menus(self)
    }

    fn close_requested(&mut self, window: WindowKey, cx: &mut Cx<Msg>) -> bool {
        if let Some(sheet) = self.sheets.remove(&window) {
            if matches!(sheet, Sheet::SignIn) {
                self.cancel_sign_in(cx);
            }
            return true;
        }
        if Some(window) == self.settings {
            self.settings = None;
            return true;
        }
        if self.timeline.as_ref().is_some_and(|t| t.window == window) {
            self.close_timeline();
            return true;
        }
        if self.usage_window.as_ref().is_some_and(|u| u.window == window) {
            self.usage_window = None;
            return true;
        }
        if window == self.main {
            // An agent mid-turn is asked about first.
            if !self.quit_confirmed && self.live.entries().iter().any(|s| s.chat_busy.get()) {
                cx.alert(
                    self.main,
                    Alert::new(
                        "An agent is still working",
                        "Quitting stops it mid-turn. What it has done so far stays in its session.",
                    )
                    .destructive_button("Quit Anyway", Msg::ConfirmQuit)
                    .cancel_with("Keep Working", Msg::ClearNote),
                );
                return false;
            }
            // The window goes at once; the children stop and the views are
            // saved before the app ends.
            self.quit(cx);
            return false;
        }
        true
    }

    /// Folders opened from the desktop. A file is not a project: the
    /// worker that registers the project turns it away.
    fn open(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<Msg>) {
        for path in paths {
            self.open_directory(path, cx);
        }
    }
}

impl App {
    /// What every message is followed by: the transcript caught up with
    /// the conversation, the timers paced, the panel's work started.
    fn after_update(&mut self, cx: &mut Cx<Msg>) {
        self.sync_transcripts(cx);
        self.pace(cx);
    }

    pub(crate) fn main_or_settings(&self) -> WindowKey {
        self.settings.unwrap_or(self.main)
    }

    /// Stop every session's child and save, then quit once they have
    /// stopped (or a few seconds have passed).
    pub(crate) fn quit(&mut self, cx: &mut Cx<Msg>) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        cx.hide_window(self.main);
        if let Some(settings) = self.settings.take() {
            cx.close_window(settings);
        }
        if let Some(timeline) = self.timeline.as_ref().map(|t| t.window) {
            self.close_timeline();
            cx.close_window(timeline);
        }
        if let Some(usage) = self.usage_window.take() {
            cx.close_window(usage.window);
        }
        self.save_views(cx);
        for session in self.live.entries() {
            if let Some(runtime) = session.omp_runtime.get() {
                runtime.begin_shutdown();
            }
        }
        self.quit_deadline = Some(Instant::now() + Duration::from_secs(5));
        cx.every(Duration::from_millis(50), Msg::QuitPoll);
    }

    fn quit_poll(&mut self, cx: &mut Cx<Msg>) {
        let stopped = self.live.entries().iter().all(|s| s.omp_runtime.get().is_none_or(crate::omp_runtime::OmpRuntime::try_reap));
        let late = self.quit_deadline.is_some_and(|at| Instant::now() >= at);
        if stopped || late {
            crate::instance::release();
            cx.quit();
        }
    }

    /// A second passed: the session in view's output speed is read.
    fn tick(&mut self) {
        let session = self.session();
        let reading = session.live_rate.borrow_mut().reading(Instant::now());
        let rate = match reading {
            crate::metrics::Reading::Streaming { estimate_tps } => Some(estimate_tps),
            _ => None,
        };
        if let Some(rate) = rate {
            let rates = &mut self.view_mut(session.key).rates;
            rates.push_back(rate);
            while rates.len() > 120 {
                rates.pop_front();
            }
        }
    }
}
