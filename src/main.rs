#![recursion_limit = "512"]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("Omperor supports macOS and Linux");

#[cfg(target_os = "linux")]
use sidestep as _;

mod account_alert;
mod app_state;
mod account_ui;
mod agent_inspection;
mod agent_workspace;
mod attachments;
mod attachment_input;
mod composer_attachments;
mod agents;
mod change_tree;
mod chat;
mod chat_typography;
#[cfg(feature = "design-preview")]
mod design_preview;
mod file_picker;
mod file_watcher;
mod gui_wake;
mod housekeeping;
mod archive;
mod jobs;
mod journal;
mod launchers;
mod layout;
mod sidebar;
mod markdown;
mod markdown_bench;
mod metrics;
mod cache_policy;
mod usage;
mod usage_ui;
mod motion;
mod header_ui;
mod slash_commands;
mod slash_ui;
mod usage_card;
mod mock_inference;
mod developer_settings;
mod display_spans;
mod native_markdown;
mod omp_runtime;
mod omperor_tools;
mod event_spans;
mod eval;
mod eval_report;
mod eval_suite;
mod frame_budget;
mod frame_overlay;
mod perf;
mod preferences;
mod quick_actions;
mod session;
mod session_workspace;
mod session_state;
mod live_sessions;
mod session_persistence;
mod housekeeping_ui;
mod housekeeping_smoke;
mod readme_shots;
mod startup;
mod theme;
mod timeline;
mod timeline_ui;
mod transcripts;
mod transcript_check;
mod transcript_invariant;
mod transcript_parse;
mod transcript_semantics;
mod saved_heights;
mod scroll_reserve;
#[cfg(target_os = "linux")]
mod scroll_smoke;
mod transcript_stack;
mod transcript_style;
mod typography_examples;
mod typography_settings;
mod workspace;
mod workspace_model;

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use chat::TranscriptMessage;
use chat_typography::{TextRole, TextSizes};
use file_picker::{ProjectFile, active_at_query, filter_project_files, index_project_files};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSSearchFieldDelegate, NSWindowDelegate};
use objc2_app_kit::{
    NSAccessibility, NSAlert, NSAlertFirstButtonReturn, NSAlertStyle,
    NSAnimatablePropertyContainer, NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSAutoresizingMaskOptions, NSBackingStoreType, NSBezelStyle,
    NSBezierPath, NSBorderType, NSBox, NSBoxType, NSButton, NSCellImagePosition, NSColor,
    NSControl, NSControlSize, NSControlStateValueOff, NSControlStateValueOn,
    NSControlTextEditingDelegate, NSEvent, NSEventModifierFlags,
    NSFont, NSFontAttributeName, NSFontWeightBold, NSFontWeightMedium, NSFontWeightRegular,
    NSFontWeightSemibold, NSForegroundColorAttributeName, NSImage, NSImageScaling, NSImageView,
    NSKernAttributeName, NSLineCapStyle, NSMenu, NSMenuItem, NSModalResponseOK,
    NSMutableParagraphStyle, NSOpenPanel, NSParagraphStyleAttributeName, NSPasteboard,
    NSPasteboardTypeString, NSPopUpButton, NSProgressIndicator, NSProgressIndicatorStyle,
    NSResponder, NSScrollView, NSSearchField, NSSecureTextField, NSSegmentDistribution,
    NSSegmentStyle, NSSegmentSwitchTracking, NSSegmentedControl, NSSplitView, NSSplitViewDelegate,
    NSSplitViewDividerStyle, NSStepper, NSTextAlignment, NSTextDelegate, NSTextField,
    NSTextFieldDelegate, NSTextView, NSTextViewDelegate, NSTitlePosition, NSTrackingArea,
    NSTrackingAreaOptions, NSView,
    NSViewBoundsDidChangeNotification, NSViewFrameDidChangeNotification,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowOrderingMode, NSWindowStyleMask, NSWindowTitleVisibility,
    NSWindowWillCloseNotification, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSDateFormatter, NSDateFormatterStyle, NSDictionary,
    NSMutableAttributedString, NSNotification, NSNotificationCenter, NSNumber, NSObject,
    NSObjectProtocol, NSPoint, NSRange, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString,
    NSTimer, NSURL, ns_string,
};
use omp_runtime::{
    AccountSummary, AuthMethod, AuthPrompt, AuthProvider, OmpRuntime, TurnOutcome, UiEvent,
};
use preferences::{
    AppearancePreference, ChatTextSize, Preferences, Project, SessionRecord, StoredAccount,
};
use theme::ChatStyle;
use typography_settings::TextSizeSettingsUi;

const WINDOW_WIDTH: f64 = 1440.0;
const WINDOW_HEIGHT: f64 = 900.0;
/// Most file rows the `@` reference picker shows at once.
const FILE_PICKER_MAX_ROWS: usize = 8;
const FILE_PICKER_ROW_HEIGHT: f64 = 30.0;
const FILE_PICKER_PADDING: f64 = 6.0;
/// How long the project tree must stay quiet after an out-of-band change
/// before the workspace is inspected again.
const WATCH_QUIET_PERIOD: std::time::Duration = std::time::Duration::from_secs(1);
const SIDEBAR_WIDTH: f64 = 248.0;
const INSPECTOR_WIDTH: f64 = 352.0;
/// The editor and terminal popups' first item: whichever app is installed.
const SETTINGS_AUTOMATIC_APP: &str = "Automatic";
/// The browser picker's first item: no browser of Omperor's, the system's.
const SETTINGS_SYSTEM_BROWSER: &str = "System default";
/// One row in the Environment pane's summary and change list.
const INSPECTOR_ROW_HEIGHT: f64 = 30.0;
const MAX_MODEL_PICKER_ITEMS: usize = 128;
const SETTINGS_WIDTH: f64 = 900.0;
const SETTINGS_HEIGHT: f64 = 620.0;
const SETTINGS_SIDEBAR_WIDTH: f64 = 220.0;
/// Vertical space the title and subtitle of a Settings page occupy.
const SETTINGS_PAGE_HEADER_HEIGHT: f64 = 100.0;
/// Vertical space reserved under the account cards for the storage note.
const SETTINGS_ACCOUNT_NOTE_HEIGHT: f64 = 62.0;
/// The band under the Accounts header that holds the provider tabs.
const SETTINGS_ACCOUNT_TABS_HEIGHT: f64 = 44.0;
/// Height of one sidebar row: a project heading or a session.
const SIDEBAR_ROW_HEIGHT: f64 = 38.0;
/// Index of the Projects page in the Settings sidebar.
const SETTINGS_PROJECTS_PAGE: usize = 1;
const SETTINGS_ACCOUNTS_PAGE: usize = 5;
const SETTINGS_METRICS_PAGE: usize = 6;
const SETTINGS_TEXT_PAGE: usize = 7;
const SETTINGS_DEVELOPER_PAGE: usize = 8;
const SETTINGS_ARCHIVES_PAGE: usize = 9;
const TEXT_SIZE_FIELD_TAG: isize = 2000;
/// The Metrics page's session list: where it starts and how wide it is.
const SETTINGS_METRICS_LIST_X: f64 = 40.0;
const SETTINGS_METRICS_LIST_WIDTH: f64 = 236.0;
const SETTINGS_METRICS_ROW_HEIGHT: f64 = 44.0;
/// The dial and the figures beside it, at the chat header's right.
const SPEEDOMETER_WIDTH: f64 = 168.0;
/// Tag encoding for folder rows on the Projects page: project index times the
/// stride plus the folder index.
const SETTINGS_FOLDER_TAG_STRIDE: usize = 100;
#[derive(Debug)]
struct SettingsUi {
    developer: developer_settings::DeveloperSettingsUi,
    window: Retained<NSWindow>,
    pages: Vec<Retained<NSView>>,
    category_buttons: Vec<Retained<NSButton>>,
    workspace_label: Retained<NSTextField>,
    model_picker: Retained<NSPopUpButton>,
    thinking_picker: Retained<NSPopUpButton>,
    appearance_picker: Retained<NSPopUpButton>,
    text_size_picker: Retained<NSPopUpButton>,
    chat_style_picker: Retained<NSPopUpButton>,
    inline_images: Retained<NSButton>,
    remote_images: Retained<NSButton>,
    computer_use: Retained<NSButton>,
    text_sizes: TextSizeSettingsUi,
    save_status: Retained<NSTextField>,
    editor_picker: Retained<NSPopUpButton>,
    terminal_picker: Retained<NSPopUpButton>,
    browser_picker: Retained<NSPopUpButton>,
    account_scroll: Retained<NSScrollView>,
    account_content: Retained<NSView>,
    /// One segment per provider; the page shows the selected one's accounts.
    account_tabs: Retained<NSSegmentedControl>,
    projects_scroll: Retained<NSScrollView>,
    projects_content: Retained<NSView>,
    archives_scroll: Retained<NSScrollView>,
    archives_content: Retained<NSView>,
    /// The Metrics page: the session list on the left, the cards on the
    /// right for the session chosen there.
    metrics_list_scroll: Retained<NSScrollView>,
    metrics_list_content: Retained<NSView>,
    metrics_scroll: Retained<NSScrollView>,
    metrics_content: Retained<NSView>,
}

/// The sign-in sheet's views, kept so OMP's events can update it. Every
/// field is a handle, so the whole thing is cloned out of its cell before
/// anything that might present or dismiss a sheet is called.
#[derive(Clone, Debug)]
struct SignInSheet {
    window: Retained<NSWindow>,
    host: Retained<NSWindow>,
    provider: AuthProvider,
    method: Rc<Cell<AuthMethod>>,
    /// Where the method's views begin, measured from the top.
    content_top: f64,
    method_picker: Option<Retained<NSSegmentedControl>>,
    browser_views: Vec<Retained<NSView>>,
    link_field: Retained<NSTextField>,
    copy_link: Retained<NSButton>,
    open_link: Retained<NSButton>,
    callback_field: Retained<NSTextField>,
    use_callback: Retained<NSButton>,
    secret_views: Vec<Retained<NSView>>,
    secret_intro: Retained<NSTextField>,
    secret_field: Retained<NSSecureTextField>,
    submit: Retained<NSButton>,
    retry: Retained<NSButton>,
    cancel: Retained<NSButton>,
    spinner: Retained<NSProgressIndicator>,
    status: Retained<NSTextField>,
}

impl SignInSheet {
    fn views(&self) -> Self {
        self.clone()
    }
}

/// How tall the sheet is for a method: the browser needs room for the page
/// and the callback; a pasted credential needs one field.
fn sign_in_sheet_height(content_top: f64, method: AuthMethod) -> f64 {
    let content = match method {
        AuthMethod::Browser => 178.0,
        AuthMethod::ApiKey | AuthMethod::Token => 98.0,
    };
    content_top + content + 24.0 + 62.0
}

/// What the sheet says above a pasted credential's field.
fn sign_in_secret_intro(provider: AuthProvider, method: AuthMethod) -> String {
    match (provider, method) {
        (AuthProvider::OpenAi, AuthMethod::Token) => {
            "Paste a ChatGPT access token from a machine that is already signed in to Codex. OMP stores it in Omperor's private data directory.".to_owned()
        }
        (AuthProvider::OpenAiApi, _) => {
            "Paste an OpenAI API key. Usage is billed to that key. OMP stores it in Omperor's private data directory and sends it only to OpenAI.".to_owned()
        }
        (AuthProvider::Anthropic, _) => {
            "Paste an Anthropic API key. Usage is billed to that key. OMP stores it in Omperor's private data directory and sends it only to Anthropic.".to_owned()
        }
        (provider, method) => format!(
            "Paste your {} {}. OMP stores it in Omperor's private data directory.",
            provider.product_name(),
            method.label().to_lowercase()
        ),
    }
}

/// How a fork should be launched: seeded from `parent_session_id`, in the
/// parent's folder by default or in a fresh worktree when requested.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ForkPlan {
    parent_session_id: String,
    worktree: Option<WorktreeFork>,
}

/// The git identity of a fork that gets its own branch and worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeFork {
    branch: String,
    base_branch: String,
}

/// Result of one background job, delivered to the main thread by the poll
/// timer. Every variant carries the id `begin_job` handed out.
#[derive(Debug)]
enum StorageUpdate {
    None,
    Accounts(Vec<StoredAccount>),
    Registry(preferences::Registry),
}

#[derive(Debug)]
enum BackgroundEvent {
    /// A session's transcript heights were written (`saved_heights`).
    HeightsSaved(Result<(), String>),
    RevealPrepared { job: u64, result: Result<(std::path::PathBuf, bool), String> },
    AttachmentPromptPrepared { job: u64, key: live_sessions::SessionKey, generation: u64, prompt: attachments::PreparedPrompt },
    AttachmentPrepared {
        key: live_sessions::SessionKey,
        job: u64,
        id: u64,
        result: Result<attachments::PreparedAttachment, String>,
    },
    ProjectChosen {
        key: live_sessions::SessionKey,
        job: u64,
        session: Option<std::path::PathBuf>,
        generation: u64,
        result: Result<(Project, std::path::PathBuf), preferences::PreferencesError>,
    },
    StartupLoaded(Result<startup::Startup, preferences::PreferencesError>),
    SessionViewsLoaded(Result<session_state::SavedViews, preferences::PreferencesError>),
    #[cfg(feature = "design-preview")]
    PreviewPreferencesLoaded(Preferences),
    ForkProbed {
        key: live_sessions::SessionKey,        job: u64,
        summary: session::SessionSummary,
        directory: std::path::PathBuf,
        project_id: String,
        result: Result<Option<workspace::BranchPoint>, workspace::WorkspaceError>,
    },
    WorktreeAdded {
        key: live_sessions::SessionKey,        job: u64,
        project_id: String,
        worktree: std::path::PathBuf,
        plan: ForkPlan,
        result: Result<(), workspace::WorkspaceError>,
    },
    /// The `@`-picker file index for a project directory finished off-thread,
    /// along with the change filter built from the ignore files it honored.
    ProjectFilesIndexed {
        key: live_sessions::SessionKey,        directory: std::path::PathBuf,
        files: Vec<ProjectFile>,
        filter: file_watcher::ChangeFilter,
    },
    /// The `@` picker's matches for one query, tagged with the generation of
    /// the request that posed it so a later keystroke can discard it.
    FilePickerFiltered {
        key: live_sessions::SessionKey,        generation: u64,
        query_start: usize,
        matches: Vec<String>,
    },
    /// `open` finished handing a path to the editor or terminal.
    Launched {
        key: live_sessions::SessionKey,        app: &'static str,
        target: std::path::PathBuf,
        result: Result<(), String>,
    },
    /// A pinned task's journal was read on a worker; session identity guards
    /// against applying a late reply to another workspace.
    WorkspaceEvidenceRead {
        key: live_sessions::SessionKey,        generation: u64,
        session: Option<std::path::PathBuf>,
        id: String,
        result: Result<transcripts::Entry, String>,
    },
    /// A quick peek read, guarded by both selection generation and session.
    AgentInspectionRead {
        key: live_sessions::SessionKey,        generation: u64,
        session: Option<std::path::PathBuf>,
        id: String,
        result: Result<transcripts::Entry, String>,
    },
    /// The warming pass carries only journals that changed.
    SubagentTranscriptsSynced {
        key: live_sessions::SessionKey,        generation: u64,
        session: Option<std::path::PathBuf>,
        result: transcripts::SyncResult,
    },
    /// The metrics log was read for the Metrics page.
    MetricsRead { records: Vec<metrics::Record> },
    /// A deletion or an archive finished on a worker.
    RemovalFinished { id: u64, outcome: housekeeping_ui::Outcome },
    /// The Archives page's list was read.
    ArchivesListed(Result<Vec<archive::Listing>, String>),
    ArchiveDeleted { job: u64, id: String, origin: housekeeping_ui::Origin, result: Result<u64, String> },
    ArchiveRestored {
        job: u64,
        id: String,
        origin: housekeeping_ui::Origin,
        result: Result<archive::Restored, String>,
        registry: Option<preferences::Registry>,
    },
    /// Empty sessions were removed.
    EmptySessionsSwept { job: u64, origin: housekeeping_ui::Origin, result: Result<housekeeping::Sweep, String> },
    /// The answer to an agent's `omperor_` tool call, worked out off the
    /// main thread.
    ToolAnswered { key: live_sessions::SessionKey, call: u64, result: Result<serde_json::Value, String> },
    UsageRead { key: live_sessions::SessionKey, generation: u64, report: usage::Report },
}

/// What the Metrics page shows: nothing chosen yet (the open session, if
/// it has records, otherwise everything), everything, or one session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum MetricsSelection {
    #[default]
    Unchosen,
    All,
    Session(String),
}

/// Releases the layout guard on every exit path, including early returns.
struct ResetOnDrop<'a>(&'a Cell<bool>);

impl Drop for ResetOnDrop<'_> {
    fn drop(&mut self) { self.0.set(false); }
}

/// One account OMP stores, joined with the metadata Omperor keeps for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AccountEntry {
    summary: AccountSummary,
    label: Option<String>,
    authenticated_at_ms: i64,
    is_default: bool,
}

impl AccountEntry {
    fn joined(summary: AccountSummary, stored: &[StoredAccount]) -> Self {
        let metadata = stored
            .iter()
            .find(|account| account.account_id == summary.id);
        Self {
            label: metadata.and_then(|account| account.label.clone()),
            authenticated_at_ms: metadata.map_or(0, |account| account.authenticated_at_ms),
            is_default: metadata.is_some_and(|account| account.is_default),
            summary,
        }
    }

    fn id(&self) -> &str {
        &self.summary.id
    }

    fn provider(&self) -> AuthProvider {
        self.summary.provider
    }

    fn is_usable(&self) -> bool {
        self.summary.state.is_usable()
    }

    /// The name shown wherever the account is chosen: the user's label when
    /// one exists, otherwise the provider-side identity.
    fn display_name(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.identity())
    }

    /// Provider-side identity: the subscriber email when the provider reports
    /// one, otherwise when the account was authenticated through Omperor.
    fn identity(&self) -> String {
        self.summary.email().map_or_else(
            || {
                format!(
                    "Authenticated {}",
                    format_timestamp(self.authenticated_at_ms)
                )
            },
            str::to_owned,
        )
    }
}

/// What the next fresh session the child reports should be recorded as.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingSession {
    project_id: String,
    directory: std::path::PathBuf,
    parent_session_id: Option<String>,
    branch: Option<String>,
    base_branch: Option<String>,
    worktree_path: Option<std::path::PathBuf>,
}

/// What an Agents-panel clock counts from: the live turn's start, or a
/// sub-agent's wall-clock start time in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentClock {
    Turn,
    Started(u64),
}

/// One row of the sidebar's session list.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SidebarRow {
    /// A project heading; toggles collapse when clicked.
    Project { project_id: String },
    /// Heading for sessions whose directory belongs to no project.
    Unfiled,
    ShowMore { project_id: String, hidden: usize, expanded: bool },
    /// A session, indexed into the delegate's session list. `project_id` is set
    /// when the session sits under a project (so it can be reordered there) and
    /// `None` for the flat read-only list and unfiled sessions.
    Session {
        index: usize,
        project_id: Option<String>,
    },
}

/// Per-row hover state for the sidebar session list.
#[derive(Debug, Default)]
struct SessionListIvars {
    /// Row rectangles paired with the button revealed while the pointer is
    /// inside them.
    hover_targets: RefCell<Vec<(NSRect, Retained<NSButton>)>>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond calling the designated
    //   initializer, which `new` does.
    // - MainThreadOnly matches AppKit's threading model.
    // - SessionListView does not implement Drop.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = SessionListIvars]
    #[derive(Debug)]
    struct SessionListView;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for SessionListView {}

    // SAFETY: Each override keeps AppKit's declared signature and calls
    // through to the superclass where the superclass does work.
    impl SessionListView {
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(previous) = self.ivars().tracking_area.borrow_mut().take() {
                self.removeTrackingArea(&previous);
            }
            // SAFETY: The view owns the tracking area and receives its mouse
            // events; it is removed before being replaced.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    NSTrackingAreaOptions::MouseMoved
                        | NSTrackingAreaOptions::MouseEnteredAndExited
                        | NSTrackingAreaOptions::ActiveInKeyWindow
                        | NSTrackingAreaOptions::InVisibleRect,
                    Some::<&AnyObject>(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            self.ivars().tracking_area.replace(Some(area));
            // SAFETY: The superclass implementation exists and takes no arguments.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            self.reveal_hover_target(Some(point));
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            self.reveal_hover_target(Some(point));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.reveal_hover_target(None);
        }
    }
);

impl SessionListView {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SessionListIvars::default());
        // SAFETY: NSView's designated initializer takes a frame rectangle.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Replaces the rows whose action button appears on hover. Every button
    /// starts hidden until the pointer enters its row.
    fn set_hover_targets(&self, targets: Vec<(NSRect, Retained<NSButton>)>) {
        for (_, button) in &targets {
            button.setHidden(true);
        }
        self.ivars().hover_targets.replace(targets);
    }

    fn reveal_hover_target(&self, point: Option<NSPoint>) {
        for (row, button) in self.ivars().hover_targets.borrow().iter() {
            let inside = point.is_some_and(|point| {
                point.x >= row.origin.x
                    && point.x < row.origin.x + row.size.width
                    && point.y >= row.origin.y
                    && point.y < row.origin.y + row.size.height
            });
            button.setHidden(!inside);
        }
    }
}

/// What the speedometer draws: how far round the dial the needle sits, and
/// the figures beside it.
#[derive(Debug, Default)]
struct SpeedometerIvars {
    /// Needle position, 0 at rest to 1 at the end of the dial.
    fraction: Cell<f64>,
    /// The rate in tokens a second the dial's end stands for.
    scale_max: Cell<f64>,
    /// Whether the needle follows an estimate (drawn quieter) or a receipt.
    estimated: Cell<bool>,
    /// Whether anything has ever streamed; a blank dial is drawn dimmer.
    lit: Cell<bool>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond calling the designated
    //   initializer, which `new` does.
    // - MainThreadOnly matches AppKit's threading model.
    // - SpeedometerView does not implement Drop.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = SpeedometerIvars]
    #[derive(Debug)]
    struct SpeedometerView;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for SpeedometerView {}

    // SAFETY: drawRect: keeps AppKit's declared signature; the view draws
    // only inside its own bounds and calls nothing on the superclass, which
    // draws nothing for a plain NSView.
    impl SpeedometerView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl SpeedometerView {
    /// The dial: a half circle across the top of the view, its centre on the
    /// baseline, so the needle swings from the left (nothing) to the right
    /// (the end of the scale).
    const RADIUS: f64 = 15.0;
    const STROKE: f64 = 3.0;

    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SpeedometerIvars {
            scale_max: Cell::new(200.0),
            ..SpeedometerIvars::default()
        });
        // SAFETY: NSView's designated initializer takes a frame rectangle.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Points the needle at `tokens_per_second`, widening the scale when a
    /// rate runs off its end so a fast model is never pinned at full.
    fn show(&self, tokens_per_second: Option<f64>, estimated: bool) {
        let ivars = self.ivars();
        let mut scale = ivars.scale_max.get();
        if let Some(rate) = tokens_per_second {
            while rate > scale * 0.85 && scale < 6400.0 {
                scale *= 2.0;
            }
            ivars.scale_max.set(scale);
            ivars.fraction.set((rate / scale).clamp(0.0, 1.0));
            ivars.lit.set(true);
        } else {
            ivars.fraction.set(0.0);
        }
        ivars.estimated.set(estimated);
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let ivars = self.ivars();
        let bounds = self.bounds();
        let centre = NSPoint::new(bounds.size.width / 2.0, 3.0);
        let radius = Self::RADIUS;
        // The track: the whole half circle, quiet.
        let track = NSBezierPath::bezierPath();
        track.setLineWidth(Self::STROKE);
        track.setLineCapStyle(NSLineCapStyle::Round);
        track.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
            centre, radius, 180.0, 0.0, true,
        );
        theme::hairline().setStroke();
        track.stroke();
        let fraction = ivars.fraction.get();
        let angle = 180.0 - 180.0 * fraction;
        let ink = if !ivars.lit.get() {
            theme::secondary_text()
        } else if ivars.estimated.get() {
            theme::accent_fill_strong()
        } else {
            theme::accent()
        };
        if fraction > 0.0 {
            // The part of the dial the rate has reached.
            let reached = NSBezierPath::bezierPath();
            reached.setLineWidth(Self::STROKE);
            reached.setLineCapStyle(NSLineCapStyle::Round);
            reached.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
                centre, radius, 180.0, angle, true,
            );
            ink.setStroke();
            reached.stroke();
        }
        // The needle, from the hub to just inside the track.
        let radians = angle.to_radians();
        let tip = NSPoint::new(
            centre.x + (radius - 4.5) * radians.cos(),
            centre.y + (radius - 4.5) * radians.sin(),
        );
        let needle = NSBezierPath::bezierPath();
        needle.setLineWidth(1.5);
        needle.setLineCapStyle(NSLineCapStyle::Round);
        needle.moveToPoint(centre);
        needle.lineToPoint(tip);
        NSColor::labelColor().setStroke();
        needle.stroke();
        let hub =
            NSBezierPath::bezierPathWithOvalInRect(rect(centre.x - 2.0, centre.y - 2.0, 4.0, 4.0));
        NSColor::labelColor().setFill();
        hub.fill();
    }
}

/// The timeline's minimap: every event as a mark down a strip beside the
/// list, its length for how long the event took, with the rows in view
/// shown as a window over it.
#[derive(Debug, Default)]
struct TimelineMinimapIvars {
    marks: RefCell<Vec<timeline::Mark>>,
    /// The rows in view, as fractions of the list from the top.
    viewport: Cell<(f64, f64)>,
    /// Where the last click landed, as a fraction of the strip from the top.
    last_click: Cell<f64>,
    /// Who hears about clicks: the delegate, kept as a plain object.
    target: RefCell<Option<Retained<AnyObject>>>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond calling the designated
    //   initializer, which `new` does.
    // - MainThreadOnly matches AppKit's threading model.
    // - TimelineMinimapView does not implement Drop.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TimelineMinimapIvars]
    #[derive(Debug)]
    struct TimelineMinimapView;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for TimelineMinimapView {}

    // SAFETY: both overrides keep AppKit's declared signatures; drawRect:
    // draws only inside the view's bounds, and mouseDown: consumes the click.
    impl TimelineMinimapView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        /// A press on the strip is a jump, never the start of a window drag.
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let height = self.bounds().size.height.max(1.0);
            let position = (1.0 - point.y / height).clamp(0.0, 1.0);
            self.ivars().last_click.set(position);
            let target = self.ivars().target.borrow().clone();
            if let Some(target) = target {
                // SAFETY: the target is the delegate, which implements
                // timelineMinimapClicked: and outlives the sheet.
                let _: () = unsafe { msg_send![&*target, timelineMinimapClicked: self] };
            }
        }
    }
);

impl TimelineMinimapView {
    fn new(frame: NSRect, target: &AnyObject, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TimelineMinimapIvars {
            viewport: Cell::new((0.0, 1.0)),
            target: RefCell::new(Some(target.retain())),
            ..TimelineMinimapIvars::default()
        });
        // SAFETY: NSView's designated initializer takes a frame rectangle.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn set_marks(&self, marks: Vec<timeline::Mark>) {
        self.ivars().marks.replace(marks);
        self.setNeedsDisplay(true);
    }

    fn set_viewport(&self, from: f64, to: f64) {
        self.ivars()
            .viewport
            .set((from.clamp(0.0, 1.0), to.clamp(0.0, 1.0)));
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let _span = perf::span("gui.timeline.draw_minimap");
        let bounds = self.bounds();
        let (width, height) = (bounds.size.width, bounds.size.height);
        // The strip.
        let track = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 3.0, 3.0);
        theme::surface_inset().setFill();
        track.fill();
        let ivars = self.ivars();
        // Each event as a bar from the left edge, as long as it was slow;
        // the slow ones in gold, the rest quiet. Marks nearer each other
        // than a bar is tall share one bar, the longest of them, so a long
        // session draws no more bars than the strip has room for.
        const BAR: f64 = 2.0;
        let slots = (height / BAR).ceil().max(1.0) as usize;
        let mut strongest: Vec<Option<(f64, bool)>> = vec![None; slots];
        for mark in ivars.marks.borrow().iter() {
            let slot = ((mark.position * height / BAR).floor().max(0.0) as usize).min(slots - 1);
            let entry = strongest[slot].get_or_insert((mark.weight, mark.slow));
            entry.0 = entry.0.max(mark.weight);
            entry.1 |= mark.slow;
        }
        let (gold, quiet) = (theme::accent(), theme::secondary_text());
        for (slot, entry) in strongest.into_iter().enumerate() {
            let Some((weight, slow)) = entry else {
                continue;
            };
            let length = (2.0 + (width - 4.0) * weight.sqrt()).min(width - 2.0);
            let y = height - (slot as f64 + 1.0) * BAR;
            if slow { &gold } else { &quiet }.setFill();
            NSBezierPath::bezierPathWithRect(rect(1.0, y.round(), length, BAR)).fill();
        }
        // The rows in view.
        let (from, to) = ivars.viewport.get();
        let top = height - from * height;
        let bottom = height - to * height;
        let window = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect(0.0, bottom, width, (top - bottom).max(4.0)),
            3.0,
            3.0,
        );
        theme::accent_fill().setFill();
        window.fill();
        theme::hairline().setStroke();
        window.setLineWidth(1.0);
        window.stroke();
    }
}

/// Drag state for a sidebar project header, which the user can drag vertically
/// to reorder the projects.
#[derive(Debug)]
struct ProjectButtonIvars {
    project_id: RefCell<String>,
    /// The button's frame origin when the drag began.
    drag_origin: Cell<NSPoint>,
    /// The pointer's window location when the mouse went down.
    press_location: Cell<NSPoint>,
    /// Whether the pointer has moved far enough to count as a drag.
    dragging: Cell<bool>,
}

impl Default for ProjectButtonIvars {
    fn default() -> Self {
        Self {
            project_id: RefCell::new(String::new()),
            drag_origin: Cell::new(NSPoint::new(0.0, 0.0)),
            press_location: Cell::new(NSPoint::new(0.0, 0.0)),
            dragging: Cell::new(false),
        }
    }
}

define_class!(
    // SAFETY:
    // - NSButton is designed for subclassing through its NSView initializer,
    //   which `new` calls.
    // - MainThreadOnly matches AppKit's threading model.
    // - SidebarProjectButton does not implement Drop.
    #[unsafe(super = NSButton)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ProjectButtonIvars]
    #[derive(Debug)]
    struct SidebarProjectButton;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for SidebarProjectButton {}

    // SAFETY: each override keeps AppKit's declared signature. `mouseDown:`
    // deliberately does not call super so the button forgoes NSButton's own
    // tracking and instead receives `mouseDragged:`/`mouseUp:`, distinguishing a
    // click (fire the action) from a vertical drag (reorder the projects).
    impl SidebarProjectButton {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().press_location.set(event.locationInWindow());
            self.ivars().drag_origin.set(self.frame().origin);
            self.ivars().dragging.set(false);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let start = self.ivars().press_location.get();
            let now = event.locationInWindow();
            let delta_y = now.y - start.y;
            let delta_x = now.x - start.x;
            if !self.ivars().dragging.get() && delta_x.hypot(delta_y) > 4.0 {
                self.ivars().dragging.set(true);
                if let Some(app) = self.app_delegate() {
                    app.begin_project_drag();
                }
            }
            if self.ivars().dragging.get() {
                let origin = self.ivars().drag_origin.get();
                self.setFrameOrigin(NSPoint::new(origin.x, origin.y + delta_y));
                if let Some(app) = self.app_delegate() {
                    app.update_project_drag(self);
                }
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            // Keep the button alive across the rebuild the drop triggers, which
            // drops the delegate's references to it.
            let _guard = self.retain();
            if self.ivars().dragging.get() {
                self.ivars().dragging.set(false);
                if let Some(app) = self.app_delegate() {
                    app.finish_project_drag();
                }
            } else if let Some(action) = self.action() {
                // A plain click toggles the project's collapse state.
                // SAFETY: the action is the button's own selector, delivered to
                // its configured target (the delegate), which outlives it.
                unsafe {
                    let _: bool = self.sendAction_to(Some(action), self.target().as_deref());
                }
            }
        }
    }
);

impl SidebarProjectButton {
    fn new(
        title: &str,
        project_id: &str,
        frame: NSRect,
        target: &AppDelegate,
        action: objc2::runtime::Sel,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let ivars = ProjectButtonIvars::default();
        ivars.project_id.replace(project_id.to_owned());
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSButton inherits NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setFrame(frame);
        this.setTitle(&NSString::from_str(title));
        this.setBordered(false);
        this.setAlignment(NSTextAlignment::Left);
        this.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        this.setContentTintColor(Some(&NSColor::labelColor()));
        // SAFETY: the delegate implements the action selector and outlives the
        // entire view hierarchy.
        unsafe {
            this.setTarget(Some(target));
            this.setAction(Some(action));
        }
        this
    }

    fn project_id(&self) -> String {
        self.ivars().project_id.borrow().clone()
    }

    fn app_delegate(&self) -> Option<Retained<AppDelegate>> {
        self.target()?.downcast::<AppDelegate>().ok()
    }
}

/// Drag state for a sidebar session row, which the user can drag vertically to
/// reorder the sessions within its project.
#[derive(Debug)]
struct SessionButtonIvars {
    session_id: RefCell<String>,
    project_id: RefCell<String>,
    drag_origin: Cell<NSPoint>,
    press_location: Cell<NSPoint>,
    dragging: Cell<bool>,
    /// Where this row's wash belongs while it is held down, in the session
    /// list's own coordinates.
    pill_frame: Cell<NSRect>,
}

impl Default for SessionButtonIvars {
    fn default() -> Self {
        Self {
            session_id: RefCell::new(String::new()),
            project_id: RefCell::new(String::new()),
            drag_origin: Cell::new(NSPoint::new(0.0, 0.0)),
            press_location: Cell::new(NSPoint::new(0.0, 0.0)),
            dragging: Cell::new(false),
            pill_frame: Cell::new(rect(0.0, 0.0, 0.0, 0.0)),
        }
    }
}

/// State for one Agents-panel row: which sub-agent it stands for, the pill
/// that marks it chosen, and the seed of gold a press plants at its edge.
#[derive(Debug, Default)]
struct AgentRowIvars {
    subagent_id: RefCell<String>,
    pill: RefCell<Option<Retained<NSBox>>>,
    seed: RefCell<Option<Retained<NSBox>>>,
    pill_frame: Cell<NSRect>,
    seed_frame: Cell<NSRect>,
    pressed: Cell<bool>,
    selected: Cell<bool>,
    click_count: Cell<isize>,
}

define_class!(
    // SAFETY:
    // - NSButton is designed for subclassing through its NSView initializer.
    // - MainThreadOnly matches AppKit's threading model.
    // - AgentRowButton does not implement Drop.
    #[unsafe(super = NSButton)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AgentRowIvars]
    #[derive(Debug)]
    struct AgentRowButton;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for AgentRowButton {}

    // SAFETY: each override keeps AppKit's declared signature.
    impl AgentRowButton {
        /// A row answers the click that brings the window back to the front,
        /// so the first click after a sheet closes opens the next sub-agent
        /// instead of being spent on activation.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        /// Answers the press the moment the pointer goes down, before the
        /// transcript has been assembled. `NSButton`'s own tracking runs until
        /// mouse-up, sends the action when the pointer is still inside (which
        /// fulfils the press), and abandons it when the user drags away.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            // NSButton tracks in a nested run loop, so the panel can rebuild
            // and drop this row while its own handler is still on the stack.
            let _guard = self.retain();
            self.ivars().click_count.set(event.clickCount());
            self.set_pressed(true);
            // SAFETY: forwarding NSButton's own handler with its own signature.
            unsafe {
                let _: () = msg_send![super(self), mouseDown: event];
            }
            self.set_pressed(false);
            self.ivars().click_count.set(0);
        }
    }
);

impl AgentRowButton {
    /// A transparent row-sized target laid over the row's dot and labels, so
    /// a click anywhere in the row counts, not only one on the name. `pill`
    /// and `seed` sit under the row already, at alpha zero.
    #[allow(clippy::too_many_arguments)]
    fn new(
        subagent_id: &str,
        frame: NSRect,
        pill: &NSBox,
        seed: &NSBox,
        target: &AppDelegate,
        action: objc2::runtime::Sel,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let ivars = AgentRowIvars::default();
        ivars.subagent_id.replace(subagent_id.to_owned());
        ivars.pill_frame.set(pill.frame());
        ivars.seed_frame.set(seed.frame());
        ivars.pill.replace(Some(pill.retain()));
        ivars.seed.replace(Some(seed.retain()));
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSButton inherits NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setFrame(frame);
        this.setTitle(ns_string!(""));
        this.setBordered(false);
        // A transparent button draws nothing but still tracks the mouse, so
        // the dot and labels beneath it stay visible and stay unclickable.
        this.setTransparent(true);
        // SAFETY: the delegate implements the action selector and outlives the
        // entire view hierarchy.
        unsafe {
            this.setTarget(Some(target));
            this.setAction(Some(action));
        }
        this
    }

    fn subagent_id(&self) -> String {
        self.ivars().subagent_id.borrow().clone()
    }

    fn set_pressed(&self, pressed: bool) {
        if self.ivars().pressed.replace(pressed) == pressed {
            return;
        }
        if pressed {
            self.show_press();
        } else if !self.ivars().selected.get() {
            // Released outside, or the action did not select: the press
            // was not fulfilled, so it leaves.
            self.fade_out();
        }
    }

    fn set_selected(&self, selected: bool) {
        if self.ivars().selected.replace(selected) == selected {
            return;
        }
        if !selected {
            self.fade_out();
        } else if self.ivars().pressed.get() {
            self.bloom();
        } else {
            // Selected without a press: a rebuild while the sheet is open.
            // The pill is simply there; nothing to fulfil.
            self.settle();
        }
    }

    fn boxes(&self) -> Option<(Retained<NSBox>, Retained<NSBox>)> {
        Some((
            self.ivars().pill.borrow().clone()?,
            self.ivars().seed.borrow().clone()?,
        ))
    }

    /// The press: the row tints with a share of its gold, and a seed of full
    /// gold appears at its left edge.
    fn show_press(&self) {
        let Some((pill, seed)) = self.boxes() else {
            return;
        };
        seed.setFrame(self.ivars().seed_frame.get());
        motion::animate(motion::PRESS_IN, || {
            pill.animator().setAlphaValue(motion::PRESS_TINT);
            seed.animator().setAlphaValue(1.0);
        });
    }

    /// Fulfilment: the seed sweeps across the row and dissolves as the tint
    /// rises to the settled selection.
    fn bloom(&self) {
        let Some((pill, seed)) = self.boxes() else {
            return;
        };
        let pill_frame = self.ivars().pill_frame.get();
        motion::animate(motion::BLOOM, || {
            pill.animator().setAlphaValue(1.0);
            seed.animator().setFrame(pill_frame);
            seed.animator().setAlphaValue(0.0);
        });
    }

    fn settle(&self) {
        let Some((pill, seed)) = self.boxes() else {
            return;
        };
        pill.setAlphaValue(1.0);
        seed.setAlphaValue(0.0);
    }

    fn fade_out(&self) {
        let Some((pill, seed)) = self.boxes() else {
            return;
        };
        motion::animate(motion::FADE_OUT, || {
            pill.animator().setAlphaValue(0.0);
            seed.animator().setAlphaValue(0.0);
        });
    }
}

define_class!(
    // SAFETY:
    // - NSButton is designed for subclassing through its NSView initializer.
    // - MainThreadOnly matches AppKit's threading model.
    // - SidebarSessionButton does not implement Drop.
    #[unsafe(super = NSButton)]
    #[thread_kind = MainThreadOnly]
    #[ivars = SessionButtonIvars]
    #[derive(Debug)]
    struct SidebarSessionButton;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for SidebarSessionButton {}

    // SAFETY: each override keeps AppKit's declared signature. `mouseDown:` skips
    // super so the button receives `mouseDragged:`/`mouseUp:`, distinguishing a
    // click (open the session) from a vertical drag (reorder within the
    // project). Right-click still opens the row's context menu, and the hover
    // "…" button is a separate view, so both are unaffected.
    impl SidebarSessionButton {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().press_location.set(event.locationInWindow());
            self.ivars().drag_origin.set(self.frame().origin);
            self.ivars().dragging.set(false);
            // Say the press landed now, not when the session finishes opening.
            if let Some(app) = self.app_delegate() {
                app.wash_sidebar_row(Some(self.ivars().pill_frame.get()));
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let start = self.ivars().press_location.get();
            let now = event.locationInWindow();
            let delta_y = now.y - start.y;
            let delta_x = now.x - start.x;
            if !self.ivars().dragging.get() && delta_x.hypot(delta_y) > 4.0 {
                self.ivars().dragging.set(true);
                if let Some(app) = self.app_delegate() {
                    // The row itself moves from here on, so a wash under where
                    // it started would be left behind.
                    app.wash_sidebar_row(None);
                    app.begin_session_drag();
                }
            }
            if self.ivars().dragging.get() {
                let origin = self.ivars().drag_origin.get();
                self.setFrameOrigin(NSPoint::new(origin.x, origin.y + delta_y));
                if let Some(app) = self.app_delegate() {
                    app.update_session_drag(self);
                }
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            // Keep the button alive across the rebuild the drop triggers.
            let _guard = self.retain();
            if self.ivars().dragging.get() {
                self.ivars().dragging.set(false);
                if let Some(app) = self.app_delegate() {
                    app.finish_session_drag(self);
                }
            } else if let Some(action) = self.action() {
                // A plain click opens the owner.
                // SAFETY: the action is the button's own selector, delivered to
                // its configured target (the delegate), which outlives it.
                unsafe {
                    let _: bool = self.sendAction_to(Some(action), self.target().as_deref());
                }
            }
        }
    }
);

impl SidebarSessionButton {
    #[allow(clippy::too_many_arguments)]
    fn new(
        title: &str,
        symbol: &str,
        session_id: &str,
        project_id: &str,
        frame: NSRect,
        target: &AppDelegate,
        action: objc2::runtime::Sel,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let ivars = SessionButtonIvars::default();
        ivars.session_id.replace(session_id.to_owned());
        ivars.project_id.replace(project_id.to_owned());
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSButton inherits NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setFrame(frame);
        this.setTitle(&NSString::from_str(title));
        this.setBordered(false);
        this.setAlignment(NSTextAlignment::Left);
        this.setFont(Some(&theme::ui_font(theme::CONTROL_SIZE, false)));
        this.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        this.setContentTintColor(Some(&NSColor::labelColor()));
        // SAFETY: the delegate implements the action selector and outlives the
        // entire view hierarchy.
        unsafe {
            this.setTarget(Some(target));
            this.setAction(Some(action));
        }
        if let Some(image) = theme::row_symbol(symbol, 11.5, 7.0) {
            this.setImage(Some(&image));
            this.setImagePosition(NSCellImagePosition::ImageLeading);
        }
        this
    }

    fn session_id(&self) -> String {
        self.ivars().session_id.borrow().clone()
    }

    fn project_id(&self) -> String {
        self.ivars().project_id.borrow().clone()
    }

    fn app_delegate(&self) -> Option<Retained<AppDelegate>> {
        self.target()?.downcast::<AppDelegate>().ok()
    }
}

/// Clears the delegate's open-prompt flag when an account modal ends.
struct AccountPromptGuard<'a>(&'a Cell<bool>);

impl Drop for AccountPromptGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Outcome of choosing which stored account should serve a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AccountResolution {
    /// The provider has no usable account.
    Unavailable,
    Chosen(String),
    /// Several usable accounts and nothing that singles one out.
    NeedsChoice,
}

const REFRESH_SIDEBAR: u8 = 1;
const REFRESH_AGENTS: u8 = 2;
const REFRESH_WORKSPACE: u8 = 4;
/// Only the sessions' states: served in place when the rows are unchanged.
const REFRESH_SIDEBAR_STATES: u8 = 8;

/// A drawn session row's state indicator, kept so a change of state can swap
/// the spinner or dot without rebuilding the list.
#[derive(Debug)]
struct SidebarRowStatus {
    state: agents::AgentState,
    /// The row's button, which carries the state in its tooltip.
    button: Retained<NSView>,
    indicator: Option<SidebarIndicator>,
    y: f64,
}

#[derive(Debug)]
enum SidebarIndicator {
    Spinner(Retained<NSProgressIndicator>),
    Dot(Retained<NSBox>),
}

type SessionDiscovery = flume::Receiver<session::DiscoveryUpdate>;
type SessionHistory = flume::Receiver<session::HistoryUpdate>;
type WorkspaceDiffDiscovery = flume::Receiver<(
    std::path::PathBuf,
    Result<String, workspace::WorkspaceError>,
)>;

#[derive(Debug)]
struct SessionHistoryLoad {
    receiver: SessionHistory,
    message_count: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SessionSourceFilter {
    #[default]
    Omperor,
    Omp,
}

impl SessionSourceFilter {
    fn from_segment(segment: isize) -> Self {
        match segment {
            1 => Self::Omp,
            _ => Self::Omperor,
        }
    }

    fn includes(self, omperor_owned: bool) -> bool {
        match self {
            Self::Omperor => omperor_owned,
            Self::Omp => !omperor_owned,
        }
    }
}

fn replace_session_source(
    sessions: &mut Vec<session::SessionSummary>,
    mut replacement: Vec<session::SessionSummary>,
    omperor_owned: bool,
) {
    let mut other_source = std::mem::take(sessions)
        .into_iter()
        .filter(|session| session.is_omperor_owned() != omperor_owned)
        .collect::<Vec<_>>();
    if omperor_owned {
        replacement.append(&mut other_source);
        *sessions = replacement;
    } else {
        other_source.append(&mut replacement);
        *sessions = other_source;
    }
}

#[derive(Debug, Default)]
struct AppDelegateIvars {
    live_sessions: RefCell<live_sessions::LiveSessions>,
    /// Source writer remains gated until its independently owned fork is ready.
    pending_forks: RefCell<HashMap<live_sessions::SessionKey, (live_sessions::SessionKey, String, u64)>>,
    window: OnceCell<Retained<NSWindow>>,
    settings_ui: OnceCell<SettingsUi>,
    usage_ui: OnceCell<usage_ui::UsageUi>,
    usage_state: usage_ui::UsageState,
    usage_header: OnceCell<usage_ui::UsageHeader>,
    usage_button: OnceCell<Retained<NSButton>>,
    split_view: OnceCell<Retained<NSSplitView>>,
    inspector_pane: OnceCell<Retained<NSVisualEffectView>>,
    inspector_visible: Cell<bool>,
    quick_actions: RefCell<Option<quick_actions::QuickActions>>,
    workbench_toggle: OnceCell<Retained<NSButton>>,
    composer_surface: OnceCell<Retained<NSBox>>,
    composer_hint: OnceCell<Retained<NSTextField>>,
    attachment_editor: OnceCell<Retained<NSTextView>>,
    chat_transcript: OnceCell<Rc<transcript_stack::Transcript>>,
    chat_status: OnceCell<Retained<NSTextField>>,
    center_title: OnceCell<Retained<NSTextField>>,
    workspace_path_label: OnceCell<Retained<NSTextField>>,
    /// The session's id on the header's path line, as a chip that copies it.
    session_chip: OnceCell<Retained<NSButton>>,
    /// The tokens-a-second dial at the header's right, the rate beside it,
    /// and the line under that (time to first token, tokens); the needle
    /// follows `live_rate`, ticked from the runtime poll while streaming.
    speedometer: OnceCell<Retained<SpeedometerView>>,
    speedometer_rate: OnceCell<Retained<NSTextField>>,
    speedometer_detail: OnceCell<Retained<NSTextField>>,
    speedometer_last_tick: Cell<Option<std::time::Instant>>,
    /// The metrics log beside the sessions, opened at the first record.
    metrics_log: OnceCell<metrics::Log>,
    /// The turn in flight, by when it started on the wall clock: what its
    /// responses and tool calls are recorded under.
    /// Tool calls in flight, by call id, with when each started.
    /// What the Metrics page last read from the log, which scope it shows
    /// (this session or every session), and whether a read is in flight.
    metrics_records: RefCell<Vec<metrics::Record>>,
    /// Whether the log has been read at least once, after which records
    /// made in this process are added as they are written.
    metrics_loaded: Cell<bool>,
    metrics_loading: Cell<bool>,
    /// Which session the Metrics page shows, and the session behind each
    /// row of its list (`None` for every session), by row tag.
    metrics_selection: RefCell<MetricsSelection>,
    metrics_rows: RefCell<Vec<Option<String>>>,
    /// A record was written while the page might be showing; it re-lays
    /// out on the next poll.
    metrics_ui_dirty: Cell<bool>,
    /// The session the header names, with its journal, for the chip's menu.
    /// What the status line is saying, as opposed to what it is momentarily
    /// flashing; a flash goes back to this.
    /// Whether a flash is on the status line waiting to go back.
    chat_status_flashing: Cell<bool>,
    input: OnceCell<Retained<NSTextField>>,
    send_button: OnceCell<Retained<NSButton>>,
    stop_button: OnceCell<Retained<NSButton>>,
    attach_button: OnceCell<Retained<NSButton>>,
    cache_warning_badge: OnceCell<Retained<NSButton>>,
    connect_button: OnceCell<Retained<NSButton>>,
    project_button: OnceCell<Retained<NSButton>>,
    new_chat_button: OnceCell<Retained<NSButton>>,
    new_chat_menu_item: OnceCell<Retained<NSMenuItem>>,
    model_picker: OnceCell<Retained<NSPopUpButton>>,
    model_label: OnceCell<Retained<NSTextField>>,
    thinking_picker: OnceCell<Retained<NSPopUpButton>>,
    thinking_label: OnceCell<Retained<NSTextField>>,
    session_search: OnceCell<Retained<NSSearchField>>,
    session_source_filter: OnceCell<Retained<NSSegmentedControl>>,
    sidebar_sessions_scroll: OnceCell<Retained<NSScrollView>>,
    sidebar_sessions_view: OnceCell<Retained<SessionListView>>,
    sidebar_status: OnceCell<Retained<NSTextField>>,
    /// The height the session rows themselves need, so the list's document
    /// can be re-fit to the clip view when the window changes size.
    sidebar_rows_height: Cell<f64>,
    /// The Environment pane: the pane itself, a status line under its title,
    /// the summary rows' values, the change list (scroll view, container,
    /// row views, and the gold pill behind the selected row), the section
    /// label with its totals, and the diff with its title.
    inspector_view: OnceCell<Retained<NSVisualEffectView>>,
    change_transcript: OnceCell<Retained<NSTextView>>,
    change_status: OnceCell<Retained<NSTextField>>,
    inspector_branch: OnceCell<Retained<NSTextField>>,
    inspector_upstream: OnceCell<Retained<NSTextField>>,
    inspector_commit: OnceCell<Retained<NSTextField>>,
    change_list_scroll: OnceCell<Retained<NSScrollView>>,
    change_list_view: OnceCell<Retained<NSView>>,
    change_rows: RefCell<Vec<Retained<NSView>>>,
    change_selection_pill: RefCell<Option<Retained<NSBox>>>,
    change_section_label: OnceCell<Retained<NSTextField>>,
    change_section_totals: OnceCell<Retained<NSTextField>>,
    diff_title: OnceCell<Retained<NSTextField>>,
    diff_scroll: OnceCell<Retained<NSScrollView>>,
    sessions: RefCell<Vec<session::SessionSummary>>,
    /// Project registry in the user's chosen sidebar order; stable until a
    /// project is dragged to a new position.
    projects: RefCell<Vec<Project>>,
    /// Omperor's own records of sessions it created, keyed by session id.
    session_records: RefCell<HashMap<String, SessionRecord>>,
    /// Project whose folder the running OMP child works in.
    /// Set until the first Omperor session discovery has seeded projects from
    /// the sessions that predate the registry.
    projects_bootstrap_pending: Cell<bool>,
    /// Record to attach to the next fresh session the child reports.
    /// Session to open once discovery lists it, used after a fork.
    sidebar_rows: RefCell<Vec<SidebarRow>>,
    sidebar_expanded_projects: RefCell<HashSet<String>>,
    sidebar_reset_scroll: Cell<bool>,
    sidebar_row_buttons: RefCell<Vec<Retained<NSButton>>>,
    /// Project header buttons, kept apart from the rest so a drag can read their
    /// live positions to work out the dropped order.
    sidebar_project_buttons: RefCell<Vec<Retained<SidebarProjectButton>>>,
    /// Session row buttons, kept apart for the same reason so a session drag can
    /// reorder within its project.
    sidebar_session_buttons: RefCell<Vec<Retained<SidebarSessionButton>>>,
    /// User-chosen sidebar order for sessions, by session id; loaded once and
    /// updated on a drag. Sessions absent here sort by recency.
    session_positions: RefCell<HashMap<String, i64>>,
    /// Set while a project row is being dragged, so periodic sidebar rebuilds do
    /// not tear the dragged button out from under the pointer.
    sidebar_drag_active: Cell<bool>,
    /// A thin accent bar shown during a project drag at the position the dragged
    /// project would land on release.
    drop_indicator: RefCell<Option<Retained<NSBox>>>,
    /// Worker pool for anything that must not run on the main thread.
    jobs: OnceCell<jobs::JobPool<BackgroundEvent>>,
    preference_writer: OnceCell<jobs::JobPool<Result<StorageUpdate, preferences::PreferencesError>>>,
    registry_reload_needed: Cell<bool>,
    missing_project_directories: RefCell<HashSet<std::path::PathBuf>>,
    preference_saves_pending: Cell<usize>,
    preference_save_error: RefCell<Option<String>>,
    /// Background jobs in flight, with the text shown while they run.
    active_jobs: RefCell<Vec<(u64, String)>>,
    next_job_id: Cell<u64>,
    /// The gold pill behind the selected session row, rebuilt with the rows.
    sidebar_selection_pill: RefCell<Option<Retained<NSBox>>>,
    /// One reusable box that tints whichever session row is held down, and
    /// one seed of gold at its edge; both live at the bottom of the list at
    /// alpha zero between presses.
    sidebar_press_wash: RefCell<Option<Retained<NSBox>>>,
    sidebar_press_seed: RefCell<Option<Retained<NSBox>>>,
    /// Set by a click so the rebuild it triggers grows the new selection pill
    /// out of the press rather than dropping it in.
    sidebar_bloom_pending: Cell<bool>,
    /// Selection pills on their way out; the next rebuild removes them.
    sidebar_fading_pills: RefCell<Vec<Retained<NSBox>>>,
    /// The sheet showing an activity row's full payload while it is open.
    activity_sheet: RefCell<Option<Retained<NSWindow>>>,
    /// The timeline window, while one is up: the window itself, its list
    /// (which holds the events that have arrived and the rows as laid out,
    /// what the minimap and a jump refer to), the strip beside it, the
    /// subtitle, the pill that flashes a row a minimap click lands on, and
    /// the journal it shows.
    timeline_window: RefCell<Option<Retained<NSWindow>>>,
    timeline_scroll: RefCell<Option<Retained<NSScrollView>>>,
    timeline_list: RefCell<Option<Rc<timeline_ui::TimelineList>>>,
    timeline_minimap: RefCell<Option<Retained<TimelineMinimapView>>>,
    timeline_subtitle: RefCell<Option<Retained<NSTextField>>>,
    timeline_flash: RefCell<Option<Retained<NSBox>>>,
    timeline_path: RefCell<Option<std::path::PathBuf>>,
    /// The worker's pages, newest first, while it is still sending them;
    /// the spinner and line in the footer say so.
    timeline_stream: RefCell<Option<flume::Receiver<timeline::Update>>>,
    timeline_progress: RefCell<Option<(Retained<NSProgressIndicator>, Retained<NSTextField>)>>,
    /// A `--timeline-smoke` run, while one is going.
    timeline_smoke: RefCell<Option<timeline_ui::TimelineSmoke>>,
    /// The sheet showing one event's facts and texts, while it is up.
    timeline_detail_sheet: RefCell<Option<Retained<NSWindow>>>,
    /// Its filter controls: the search, the minimum duration and the hint
    /// beside it, the kind menu, the note saying how much is shown, and a
    /// gold dot beside each control that is narrowing the list.
    timeline_search: RefCell<Option<Retained<NSSearchField>>>,
    timeline_min_field: RefCell<Option<Retained<NSTextField>>>,
    timeline_min_hint: RefCell<Option<Retained<NSTextField>>>,
    timeline_kind_popup: RefCell<Option<Retained<NSPopUpButton>>>,
    timeline_model_popup: RefCell<Option<Retained<NSPopUpButton>>>,
    timeline_filter_note: RefCell<Option<Retained<NSTextField>>>,
    timeline_filter_dots: RefCell<Vec<Retained<NSBox>>>,
    /// What the list is narrowed to, and which hidden runs the user opened
    /// (by `timeline::Row::run_key`).
    timeline_filter: RefCell<timeline::Filter>,
    timeline_open_runs: RefCell<HashSet<usize>>,
    /// Laying the rows out can make the scroller appear, which changes the
    /// clip's frame and asks for another layout before the first is done;
    /// the second waits its turn.
    timeline_laying_out: Cell<bool>,
    settings_laying_out: Cell<bool>,
    timeline_relayout_needed: Cell<bool>,
    /// The rounded chips behind the composer's model, reasoning, and account
    /// labels; refit whenever a label's text changes.
    model_chip: OnceCell<Retained<NSBox>>,
    thinking_chip: OnceCell<Retained<NSBox>>,
    account_chip: OnceCell<Retained<NSBox>>,
    /// The Fast switch: OpenAI's `priority` tier, shown only for a model
    /// whose backend advertises it for this account. The chip, its bolt and
    /// label, and the transparent button that toggles it.
    fast_chip: OnceCell<Retained<NSBox>>,
    fast_icon: OnceCell<Retained<NSImageView>>,
    fast_label: OnceCell<Retained<NSTextField>>,
    fast_button: OnceCell<Retained<NSButton>>,
    /// Models the child said are offered the tier, and whether the session
    /// asks for it.
    session_discovery: RefCell<Option<SessionDiscovery>>,
    omperor_sessions_loading: Cell<bool>,
    omp_sessions_loading: Cell<bool>,
    omperor_sessions_error: RefCell<Option<String>>,
    omp_sessions_error: RefCell<Option<String>>,
    /// An inspection was requested while one was already running.
    /// Path the user is inspecting, kept selected across automatic refreshes.
    preferences: RefCell<Preferences>,
    omperor_data_directory: OnceCell<std::path::PathBuf>,
    omperor_sessions_directory: OnceCell<std::path::PathBuf>,
    runtime_timer: OnceCell<Retained<NSTimer>>,
    chat_dirty: Cell<bool>,
    /// Refreshes asked for during this pass of the run loop, served once
    /// before it waits again.
    pending_refresh: Cell<u8>,
    /// A sidebar rebuild due in the pass after this one.
    sidebar_next_pass: Cell<bool>,
    authentication_busy: Cell<bool>,
    /// The sign-in in progress, beyond whether there is one: how, OMP's
    /// login session once acknowledged, the page it prepared, a device code,
    /// an answer on its way, a cancel sent before the acknowledgement, and
    /// how the last attempt failed.
    authentication_method: Cell<Option<AuthMethod>>,
    authentication_generation: Cell<u64>,
    authentication_owner: Cell<Option<live_sessions::SessionKey>>,
    concurrency_smoke: RefCell<Option<(std::time::Instant, Vec<live_sessions::SessionKey>, u8)>>,
    authentication_session: RefCell<Option<String>>,
    authentication_url: RefCell<Option<(String, Option<String>)>>,
    authentication_device: RefCell<Option<(String, String)>>,
    authentication_submitting: Cell<bool>,
    authentication_cancelling: Cell<bool>,
    authentication_failure: RefCell<Option<String>>,
    /// The sign-in sheet while it is up.
    sign_in_sheet: RefCell<Option<SignInSheet>>,
    /// The accounts page needs rebuilding on the next poll.
    accounts_ui_dirty: Cell<bool>,
    authentication_provider: Cell<Option<AuthProvider>>,
    accounts: RefCell<Vec<AccountEntry>>,
    account_metadata: RefCell<Vec<StoredAccount>>,
    project_account_choices: RefCell<HashMap<(String, String), String>>,
    /// What each account's provider last said about it (plan, quota
    /// windows), keyed by account id, and whether a fetch is in flight.
    account_usage: RefCell<HashMap<String, omp_runtime::AccountUsage>>,
    /// Which provider's accounts the Accounts page shows.
    accounts_tab: Cell<usize>,
    /// Accounts whose card shows every quota window rather than the one
    /// that runs out first. Folded again on the next launch.
    accounts_usage_expanded: RefCell<HashSet<String>>,
    account_usage_loading: Cell<bool>,
    account_usage_fetched_at: Cell<Option<std::time::Instant>>,
    /// When OMP last answered with account usage: how old the values the
    /// `/usage` card shows are ("Updated 2 min ago"). The ask's own time
    /// (`account_usage_fetched_at`) moves as soon as a refresh is sent.
    account_usage_answered_at: Cell<Option<std::time::Instant>>,
    /// Whether OMP reported its account list for the running child. Without
    /// it no pin can be chosen and OMP's own selection is left alone.
    /// Account OMP confirmed as pinned for the active model's provider.
    /// Set while an account modal runs. The runtime poll timer keeps firing
    /// inside modal loops, so a nested event must not open a second prompt.
    account_prompt_open: Cell<bool>,
    account_picker: OnceCell<Retained<NSPopUpButton>>,
    account_label: OnceCell<Retained<NSTextField>>,
    account_banner: OnceCell<account_ui::Banner>,
    /// The frame budget's live readout above the composer.
    frame_overlay: OnceCell<frame_overlay::Overlay>,
    account_highlight: RefCell<Option<String>>,
    account_highlight_scroll: Cell<bool>,
    termination_pending: Cell<bool>,
    /// Set once the saved preferences have loaded. Until then the in-memory
    /// copy is defaults, and storing it would overwrite every saved choice.
    preferences_loaded: Cell<bool>,
    pending_authentication: RefCell<Option<AuthPrompt>>,
    /// The active project's files for the composer's `@` reference picker,
    /// shared with the filter jobs on the worker pool, with the directory they
    /// were indexed from.
    /// Whether an index walk is in flight, and whether another was asked for
    /// meanwhile — a burst of changes costs one extra walk, not one each.
    /// The FSEvents watcher on the active project, the channel its thread
    /// reports through, and when the tree last changed under it — the
    /// workspace is inspected again once it has been quiet for a moment.
    watch_notices: OnceCell<(
        flume::Sender<std::path::PathBuf>,
        flume::Receiver<std::path::PathBuf>,
    )>,
    /// The `@` file picker: its overlay box and the selection highlight inside
    /// it, the paths it currently shows, the highlighted row, its row buttons,
    /// the field location of the `@` the selection will replace, the
    /// generation of the filter request whose answer is still wanted, and
    /// whether an `@token` is waiting for the index to finish building.
    file_picker: OnceCell<Retained<NSBox>>,
    file_picker_selection: OnceCell<Retained<NSBox>>,
    file_picker_matches: RefCell<Vec<String>>,
    file_picker_selected: Cell<usize>,
    file_picker_row_buttons: RefCell<Vec<Retained<NSButton>>>,
    file_picker_query_start: Cell<usize>,
    file_picker_generation: Cell<u64>,
    file_picker_pending_index: Cell<bool>,
    /// The composer's `/` command list and the `/usage` cards it waits on.
    slash_picker: slash_ui::SlashPicker,
    /// The workbench in the right pane: the Changes | Agents switcher, the two
    /// panel containers it shows one of, and the header buttons that open the
    /// project in the editor or terminal (retitled when the choice changes).
    workbench_switcher: OnceCell<Retained<NSSegmentedControl>>,
    changes_panel: OnceCell<Retained<NSView>>,
    agents_panel: OnceCell<Retained<NSView>>,
    open_editor_button: OnceCell<Retained<NSButton>>,
    open_terminal_button: OnceCell<Retained<NSButton>>,
    editor_menu_item: OnceCell<Retained<NSMenuItem>>,
    terminal_menu_item: OnceCell<Retained<NSMenuItem>>,
    /// The change tree: which directories the user collapsed, and what each
    /// laid-out row refers to, so a click on row N finds its change or folder.
    change_tree_rows: RefCell<Vec<change_tree::TreeRow>>,
    /// The Agents panel: its list, the summary of sub-agents beside the
    /// section label, the rows it shows, their views, the subtitle labels
    /// whose clocks tick with the text they are appended to, and the child's
    /// sub-agent board as last reported.
    agents_scroll: OnceCell<Retained<NSScrollView>>,
    agents_view: OnceCell<Retained<NSView>>,
    agents_summary: OnceCell<Retained<NSTextField>>,
    session_workspace: OnceCell<session_workspace::SessionWorkspace>,
    #[cfg(feature = "design-preview")]
    preview_workload_prepared: Cell<bool>,
    #[cfg(feature = "design-preview")]
    preview_clock_ms: Cell<u64>,
    desk_restore_pending: Cell<bool>,
    project_choice_inflight: Cell<bool>,
    session_views_loaded: Cell<bool>,
    session_views_resolved: Cell<bool>,
    session_views_edited: Cell<bool>,
    session_save_scheduled: Cell<bool>,
    agents_scope: OnceCell<Retained<NSSegmentedControl>>,
    agents_search: OnceCell<Retained<NSSearchField>>,
    agents_density: OnceCell<Retained<NSButton>>,
    agents_search_button: OnceCell<Retained<NSButton>>,
    agent_view_states: RefCell<HashMap<Option<std::path::PathBuf>, agents::ViewState>>,
    agents_reset_scroll: Cell<bool>,
    agents_layout_restore: Cell<bool>,
    agent_inspection: OnceCell<agent_inspection::AgentInspection>,
    agents_rendered_session: RefCell<Option<Option<std::path::PathBuf>>>,
    workbench_location: OnceCell<Retained<NSTextField>>,
    workbench_expand: OnceCell<Retained<NSButton>>,
    workbench_expanded: Cell<bool>,
    agents_rows: RefCell<Vec<agents::AgentRow>>,
    agents_row_views: RefCell<Vec<Retained<NSView>>>,
    agents_row_buttons: RefCell<Vec<Retained<AgentRowButton>>>,
    agents_ticking: RefCell<Vec<(Retained<NSTextField>, String, AgentClock)>>,
    agents_last_tick: Cell<Option<std::time::Instant>>,
    /// The current session's sub-agent journals, folded ahead of any click so
    /// inline evidence opens without a read.
    /// Whether a warming pass is already on a worker.
    /// When the last warming pass was submitted, so settled sub-agents are
    /// not re-examined every frame.
    /// The session the running child serves, what it is doing, and when the
    /// current turn started — the source of the Agents panel's live row and
    /// the sidebar's status dots.
    /// The status dots beside sidebar session rows, rebuilt with the rows.
    sidebar_status_dots: RefCell<Vec<Retained<NSBox>>>,
    /// The spinners beside the rows of sessions that are executing, rebuilt
    /// with the rows, and the selected session's indicator below the composer.
    sidebar_spinners: RefCell<Vec<Retained<NSProgressIndicator>>>,
    /// Per drawn row, in `sidebar_rows` order: a session row's state and
    /// indicator, `None` for the other rows.
    sidebar_row_statuses: RefCell<Vec<Option<SidebarRowStatus>>>,
    busy_spinner: OnceCell<Retained<NSProgressIndicator>>,
    busy_label: OnceCell<Retained<NSTextField>>,
    /// Deletions and archives in flight, by id: waiting for live sessions to
    /// release their journals, then running on the job pool.
    removals: RefCell<HashMap<u64, housekeeping_ui::Removal>>,
    next_removal_id: Cell<u64>,
    /// Sessions deleted or archived this run, so a discovery pass that read
    /// the disk before they went does not list them again.
    removed_session_ids: RefCell<HashSet<String>>,
    /// The Archives page's list, once read.
    archives: RefCell<Option<Result<Vec<archive::Listing>, String>>>,
    archives_loading: Cell<bool>,
    /// Archives being restored or deleted, by id.
    archive_operations: RefCell<HashSet<String>>,
    /// Empty sessions are being removed.
    sweeping: Cell<bool>,
    housekeeping_smoke: RefCell<Option<housekeeping_smoke::Smoke>>,
    readme_shots: RefCell<Option<readme_shots::Shots>>,
    /// Confirmation sheets an agent's tool call is waiting on, by session
    /// and call, so a stopped turn can withdraw its question.
    tool_confirmations: RefCell<HashMap<(live_sessions::SessionKey, u64), Retained<NSAlert>>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements relevant to this delegate.
    // - MainThreadOnly matches AppKit's threading model.
    // - AppDelegate does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: the only overridden editing behavior is the composer's newline
    // handling; every other command falls through to the default. AppDelegate
    // outlives the field it delegates for.
    unsafe impl NSControlTextEditingDelegate for AppDelegate {
        // Return sends the message; Shift+Return inserts a newline so a prompt
        // can span multiple lines. Plain Return usually fires the Send button's
        // key equivalent before the field editor sees it, but the field is the
        // authority when that button is disabled.
        // Live text edits refresh the `@` file-reference picker.
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            if let Some(field) = notification
                .object()
                .and_then(|object| object.downcast::<NSTextField>().ok())
            {
                if field.tag() == quick_actions::FIELD_TAG {
                    self.filter_quick_actions();
                } else if field.tag() == session_workspace::SEARCH_TAG {
                    self.filter_workspace_work(false);
                } else if field.tag() == agent_workspace::SEARCH_TAG {
                    self.filter_agent_work();
                } else if field.tag() < TEXT_SIZE_FIELD_TAG {
                    self.update_slash_picker();
                    self.update_file_picker();
                    self.schedule_session_view_save();
                } else {
                    self.mark_text_size_edited(&field);
                }
            }
        }

        #[unsafe(method(controlTextDidEndEditing:))]
        fn control_text_did_end_editing(&self, notification: &NSNotification) {
            if let Some(field) = notification
                .object()
                .and_then(|object| object.downcast::<NSTextField>().ok())
            {
                if field.tag() >= TEXT_SIZE_FIELD_TAG {
                    self.commit_text_size(&field);
                }
            }
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn control_text_view_do_command(
            &self,
            control: &NSControl,
            text_view: &NSTextView,
            command: objc2::runtime::Sel,
        ) -> objc2::runtime::Bool {
            if control.tag() == quick_actions::FIELD_TAG {
                return self.quick_action_command(command).into();
            }
            if control.tag() == session_workspace::SEARCH_TAG {
                if command == sel!(cancelOperation:) {
                    self.filter_workspace_work(true);
                    return objc2::runtime::Bool::YES;
                }
                return objc2::runtime::Bool::NO;
            }
            if control.tag() == agent_workspace::SEARCH_TAG {
                if command == sel!(cancelOperation:) {
                    self.clear_agent_search();
                    return objc2::runtime::Bool::YES;
                }
                return objc2::runtime::Bool::NO;
            }
            if control.tag() >= TEXT_SIZE_FIELD_TAG {
                return objc2::runtime::Bool::NO;
            }
            // While the command list is open, arrows move the selection, Tab
            // completes it, Return completes (and runs one that takes no
            // arguments), and Escape dismisses it.
            if self.slash_picker_visible() {
                if command == sel!(moveUp:) {
                    self.move_slash_selection(-1);
                    return objc2::runtime::Bool::YES;
                }
                if command == sel!(moveDown:) {
                    self.move_slash_selection(1);
                    return objc2::runtime::Bool::YES;
                }
                if command == sel!(insertTab:) {
                    self.commit_slash_picker(false);
                    return objc2::runtime::Bool::YES;
                }
                if command == sel!(insertNewline:) {
                    self.commit_slash_picker(true);
                    return objc2::runtime::Bool::YES;
                }
                if command == sel!(cancelOperation:) {
                    self.hide_slash_picker();
                    return objc2::runtime::Bool::YES;
                }
            }
            // While the file picker is open, arrows move the selection, Return
            // or Tab inserts it, and Escape dismisses it.
            if self.file_picker_visible() {
                if command == sel!(moveUp:) {
                    self.move_file_picker_selection(-1);
                    return objc2::runtime::Bool::YES;
                }
                if command == sel!(moveDown:) {
                    self.move_file_picker_selection(1);
                    return objc2::runtime::Bool::YES;
                }
                if command == sel!(insertNewline:) || command == sel!(insertTab:) {
                    self.commit_file_picker();
                    return objc2::runtime::Bool::YES;
                }
                if command == sel!(cancelOperation:) {
                    self.hide_file_picker();
                    return objc2::runtime::Bool::YES;
                }
            }
            // Escape stops the running turn, as in other chat apps; with no
            // turn running it keeps AppKit's behavior.
            if command == sel!(cancelOperation:) && self.session().chat_busy.get() {
                // SAFETY: `stopOmp:` is this delegate's own action method.
                let _: () = unsafe { msg_send![self, stopOmp: text_view] };
                return objc2::runtime::Bool::YES;
            }
            if command == sel!(moveUp:) && self.navigate_prompt_history(-1, text_view) {
                return objc2::runtime::Bool::YES;
            }
            if command == sel!(moveDown:) && self.navigate_prompt_history(1, text_view) {
                return objc2::runtime::Bool::YES;
            }
            if command != sel!(insertNewline:) {
                return objc2::runtime::Bool::NO;
            }
            let shift = NSApplication::sharedApplication(self.mtm())
                .currentEvent()
                .is_some_and(|event| {
                    event.modifierFlags().contains(NSEventModifierFlags::Shift)
                });
            // SAFETY: both messages are sent on the main thread to a live
            // field editor / to self, whose `sendMessage:` action exists.
            unsafe {
                if shift {
                    // Insert a newline and keep editing instead of ending it.
                    let _: () =
                        msg_send![text_view, insertNewlineIgnoringFieldEditor: None::<&AnyObject>];
                } else {
                    let _: () = msg_send![self, sendMessage: control];
                }
            }
            objc2::runtime::Bool::YES
        }
    }

    // SAFETY: NSTextFieldDelegate adds no requirements beyond the editing
    // delegate above.
    unsafe impl NSTextFieldDelegate for AppDelegate {}

    // SAFETY: the dedicated field editor is used only by the composer.
    unsafe impl NSWindowDelegate for AppDelegate {
        #[unsafe(method_id(windowWillReturnFieldEditor:toObject:))]
        fn attachment_field_editor(&self, _window: &NSWindow, object: Option<&AnyObject>) -> Option<Retained<objc2_app_kit::NSText>> {
            self.ivars().input.get().filter(|input| object.is_some_and(|object| std::ptr::eq(object, &***input as &AnyObject)))
                .map(|_| self.ivars().attachment_editor.get_or_init(|| attachment_input::field_editor(self.mtm())).clone().into_super())
        }
    }

    // SAFETY: the search field uses the same native editing delegate.
    unsafe impl NSSearchFieldDelegate for AppDelegate {}

    // SAFETY: NSTextDelegate adds no requirements; it is a supertrait of the
    // text-view delegate below.
    unsafe impl NSTextDelegate for AppDelegate {}

    // SAFETY: the only overridden behavior is intercepting the transcript's
    // internal detail links; everything else uses the default. AppDelegate
    // outlives the transcript view.
    unsafe impl NSTextViewDelegate for AppDelegate {
        // Clicking a "Details ▸" affordance opens the row's full payload in a
        // modal; every other link (real URLs in assistant markdown) falls
        // through to the system's default open behavior.
        #[unsafe(method(textView:clickedOnLink:atIndex:))]
        fn text_view_clicked_on_link(
            &self,
            text_view: &NSTextView,
            link: &AnyObject,
            _char_index: usize,
        ) -> objc2::runtime::Bool {
            let Some(url) = link.downcast_ref::<NSURL>() else {
                return objc2::runtime::Bool::NO;
            };
            let is_detail = url
                .scheme()
                .is_some_and(|scheme| scheme.to_string() == native_markdown::DETAIL_URL_SCHEME);
            if !is_detail {
                return objc2::runtime::Bool::NO;
            }
            if let Some(id) = url
                .resourceSpecifier()
                .and_then(|spec| spec.to_string().trim().parse::<usize>().ok())
            {
                if !self.toggle_workspace_tool(text_view, id) { self.present_activity_detail(id); }
            }
            objc2::runtime::Bool::YES
        }
    }

    // SAFETY: NSApplicationDelegate has no additional safety requirements.
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, notification: &NSNotification) {
            let _span = perf::span("gui.application_did_finish_launching");
            let mtm = self.mtm();
            let app = notification
                .object()
                .and_then(|object| object.downcast::<NSApplication>().ok())
                .expect("launch notification must belong to NSApplication");

            // SAFETY: the delegate lives for the process lifetime and handles
            // this notification only on the main run loop.
            unsafe {
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    self, sel!(backgroundJobsReady:), Some(gui_wake::notification()), None,
                );
            }
            install_main_menu(&app, self, mtm);
            self.build_window(mtm);

            if markdown_bench::active() {
                // Headless: never Regular, never activated. The harness drives
                // the window through `markdown_bench::Host`.
                markdown_bench::launched(Rc::new(self.retain()));
                self.schedule_background_start();
                self.jobs().submit(|| BackgroundEvent::SessionViewsLoaded(preferences::load_session_views()));
                return;
            }
            housekeeping_smoke::refuse_unless_scratch_home();
            readme_shots::refuse_unless_scratch_home();
            if housekeeping_smoke::requested() || readme_shots::requested() {
                // Headless and never activated; its window stays off every
                // display.
                self.schedule_background_start();
                self.jobs().submit(|| BackgroundEvent::SessionViewsLoaded(preferences::load_session_views()));
                return;
            }
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            if let Some(journal) = timeline_ui::smoke_journal() {
                // Reads only the named journal and its sub-agents' beside it,
                // and never activates: whatever the user is typing elsewhere
                // stays there rather than landing in the timeline's search.
                self.run_timeline_smoke(journal);
                return;
            }
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            if std::env::args().any(|arg| arg == "--cache-warning-smoke") {
                self.run_cache_warning_smoke();
                return;
            }
            #[cfg(feature = "design-preview")]
            self.show_design_preview();
            if std::env::args().any(|argument| argument == "--composer-input-smoke") {
                unsafe {
                    let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                        0.2, self, sel!(runComposerInputSmoke:), None, false,
                    );
                    NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
                }
                return;
            }
            if std::env::args().any(|argument| argument == "--attachment-paste-smoke") {
                // Let AppKit activate the real window before testing its responders.
                unsafe {
                    let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                        0.2, self, sel!(runAttachmentPasteSmoke:), None, false,
                    );
                    NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
                }
                // Diagnostic invocation never starts an inference child or loads real sessions.
                return;
            }
            if std::env::args().any(|argument| argument == "--sidebar-activity-smoke") {
                self.run_sidebar_activity_smoke();
                return;
            }
            #[cfg(not(feature = "design-preview"))]
            self.schedule_background_start();
            self.jobs().submit(|| BackgroundEvent::SessionViewsLoaded(preferences::load_session_views()));
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn application_should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            // These fixtures never load or save user sessions.
            if std::env::args().any(|argument| matches!(argument.as_str(), "--sidebar-activity-smoke" | "--cache-warning-smoke"))
                || timeline_ui::smoke_journal().is_some()
            {
                return NSApplicationTerminateReply::TerminateNow;
            }
            if !self.ivars().termination_pending.get() && !self.confirm_quit_while_working() {
                return NSApplicationTerminateReply::TerminateCancel;
            }
            if !self.ivars().termination_pending.replace(true) {
                self.capture_selected_binding();
                self.save_session_views();
                let pending = std::mem::take(&mut *self.ivars().pending_forks.borrow_mut());
                for (_, (_, _, job)) in pending { self.finish_job(job); }
                for owner in self.ivars().live_sessions.borrow().entries() {
                    if let Some(runtime) = owner.omp_runtime.get() { owner.lifecycle.set(live_sessions::Lifecycle::Retiring); runtime.begin_shutdown(); }
                }
                self.set_chat_status("Stopping all sessions…");
                self.update_chat_controls();
            }
            if self.all_session_workers_reaped() && self.ivars().preference_saves_pending.get() == 0 && !self.waiting_for_session_views() {
                NSApplicationTerminateReply::TerminateNow
            } else {
                // The regular poll may itself have requested Quit. AppKit enters a
                // nested loop here, where that timer cannot re-enter its callback.
                let timer = unsafe { NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                    1.0 / 30.0, self, sel!(pollTermination:), None, true,
                ) };
                unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes); }
                NSApplicationTerminateReply::TerminateLater
            }
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn application_will_terminate(&self, _notification: &NSNotification) {
            {
                let _span = perf::span("gui.application_will_terminate");
                if let Some(timer) = self.ivars().runtime_timer.get() {
                    timer.invalidate();
                }
                let reaped = self.all_session_workers_reaped();
                if std::env::args().any(|arg| arg == "--concurrency-smoke" || arg == "--lifecycle-children") {
                    assert!(reaped);
                    assert!(self.ivars().pending_forks.borrow().is_empty());
                    eprintln!("concurrency-smoke shutdown=all-workers-reaped pending_forks=empty");
                }
            }
            perf::event("gui.main_run_loop.exit");
            perf::event("perf.profile.run_complete");
            let (passes, over, worst_us) = frame_budget::summary();
            let (events, scrolls) = event_spans::dispatched();
            eprintln!(
                "frame budget at exit: {passes} main-thread passes, {over} over the {} µs frame, worst {worst_us} µs; {events} events dispatched, {scrolls} scroll wheels; {}",
                frame_budget::BUDGET_US,
                frame_budget::snapshot().histogram_line()
            );
        }
    }

    // SAFETY: these layout-only methods match AppKit's declared signatures.
    unsafe impl NSSplitViewDelegate for AppDelegate {
        #[unsafe(method(splitView:constrainSplitPosition:ofSubviewAt:))]
        fn constrain_split(&self, split: &NSSplitView, _proposed: f64, divider: isize) -> f64 {
            if divider == 0 {
                SIDEBAR_WIDTH
            } else {
                split.bounds().size.width - self.activity_width(split.bounds().size.width) - 1.0
            }
        }

        #[unsafe(method(splitView:resizeSubviewsWithOldSize:))]
        fn resize_workspace(&self, split: &NSSplitView, _old_size: NSSize) {
            self.prepare_workspace_layout();
            let size = split.bounds().size;
            let children = split.subviews();
            if children.count() < 2 {
                return;
            }
            let divider = split.dividerThickness();
            let inspector = if children.count() > 2 {
                self.activity_width(size.width) + divider
            } else {
                0.0
            };
            children.objectAtIndex(0).setFrame(rect(0.0, 0.0, SIDEBAR_WIDTH, size.height));
            children.objectAtIndex(1).setFrame(rect(
                SIDEBAR_WIDTH + divider, 0.0,
                size.width - SIDEBAR_WIDTH - divider - inspector, size.height,
            ));
            if children.count() > 2 {
                children.objectAtIndex(2).setFrame(rect(
                    size.width - inspector + divider, 0.0, inspector - divider, size.height,
                ));
            }
        }

        #[unsafe(method(splitView:shouldAdjustSizeOfSubview:))]
        fn adjust_split_subview(&self, split: &NSSplitView, view: &NSView) -> bool {
            let children = split.subviews();
            children.count() > 1 && std::ptr::eq(&*children.objectAtIndex(1), view)
        }
    }

    impl AppDelegate {


        #[unsafe(method(startBackgroundServices:))]
        fn start_background_services(&self, _sender: &AnyObject) {
        let owner = self.session();
            let _span = perf::span("gui.start_background_services");
            if self.ivars().termination_pending.get() {
                return;
            }
            self.set_chat_status("Loading projects and preferences…");
            if !self.jobs().submit(|| BackgroundEvent::StartupLoaded(startup::Startup::load())) {
                owner.runtime_starting.set(false);
                self.set_chat_status("Could not start the background worker");
                self.update_chat_controls();
            }
        }

        #[unsafe(method(pasteAttachment:))]
        fn paste_attachment(&self, _sender: Option<&AnyObject>) {
            if self.can_paste_attachment() {
                self.receive_attachments(&objc2_app_kit::NSPasteboard::generalPasteboard());
            }
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            item.action() != Some(sel!(pasteAttachment:)) || self.can_paste_attachment()
        }

        #[unsafe(method(runAttachmentPasteSmoke:))]
        fn run_attachment_paste_smoke(&self, _sender: &AnyObject) {
            let app = NSApplication::sharedApplication(self.mtm());
            attachment_input::run_paste_smoke(&app, self, self.mtm());
            std::process::exit(0);
        }

        #[unsafe(method(runComposerInputSmoke:))]
        fn run_composer_input_smoke(&self, _sender: &AnyObject) {
            let app = NSApplication::sharedApplication(self.mtm());
            attachment_input::run_composer_smoke(&app, self);
            std::process::exit(0);
        }

        #[unsafe(method(removeAttachment:))]
        fn remove_attachment(&self, sender: &NSButton) {
        let owner = self.session();
            owner.pending_attachments.borrow_mut().remove(sender.tag() as u64);
            self.refresh_pending_attachments();
            self.update_chat_controls();
        }

        #[unsafe(method(sendMessage:))]
        fn send_message(&self, _sender: &AnyObject) {
            // The press is answered at once: the arrow hops, before anything
            // the send starts could have happened.
            if let Some(button) = self.ivars().send_button.get() {
                motion::hop(button);
            }
        let owner = self.session();
            let _span = perf::span("gui.action.send_message");
            if self.ivars().termination_pending.get() || self.fork_owns_session(owner.key) { return; }
            // `/usage` is Omperor's own: answered in the transcript, never
            // sent to OMP or the model, busy or not.
            if let Some(input) = self.ivars().input.get() {
                match self.run_local_command(&input.stringValue().to_string()) {
                    slash_ui::LocalCommand::None => {}
                    slash_ui::LocalCommand::Answered => return,
                    slash_ui::LocalCommand::Send(request) => input.setStringValue(&NSString::from_str(&request)),
                }
            }
            if owner.attachment_submission.get() { return; }
            if owner.session_history.borrow().is_some() {
                self.set_chat_status("Still loading this conversation • send again in a moment");
                return;
            }
            if owner.resuming_session_id.borrow().is_some() {
                self.set_chat_status("Wait for this Omperor session to finish resuming");
                return;
            }
            if owner.read_only_session.get() {
                self.set_chat_status("This session is read-only • start a new conversation to send a prompt");
                return;
            }
            if owner.pending_attachments.borrow().blocked() {
                self.set_chat_status("Wait for attachments to finish, or remove failed attachments");
                return;
            }
            if owner.chat_busy.get() && !owner.pending_attachments.borrow().items.is_empty() {
                self.set_chat_status("Attachments are ready for your next message; wait for this turn to finish");
                return;
            }
            self.hide_file_picker();
            self.hide_slash_picker();
            let input = self
                .ivars()
                .input
                .get()
                .expect("input must exist before handling actions");
            let value = input.stringValue().to_string();

            if owner.pending_prompt.borrow().is_some() {
                self.set_chat_status("Your first prompt is queued while the agent starts • this one can go once it is sent");
                return;
            }

            if owner.chat_busy.get() {
                if !owner.conversation.borrow_mut().steer_user(&value) {
                    return;
                }
                owner.prompt_history.borrow_mut().record(&value);
                input.setStringValue(ns_string!(""));
                owner.draft.replace(String::new());
                self.refresh_chat_transcript();
                self.set_chat_status(&format!("Steering the active {} turn…", self.agent_name()));
                let sent = owner.omp_runtime
                    .get()
                    .expect("OMP runtime must exist before steering")
                    .steer(value.into());
                if !sent {
                    self.runtime_command_failed("steering the active turn");
                }
                return;
            }

            let adding_images = owner.pending_attachments.borrow().items.iter().any(|item| {
                item.result.as_ref().and_then(|r| r.as_ref().ok()).is_some_and(|a| a.mime_type.starts_with("image/"))
            });
            if adding_images && self.current_model_auth_provider() == Some(AuthProvider::Anthropic) && !owner.image_cache_warning_seen.get() {
                self.note_cache_action(cache_policy::CacheAction::ImagePresenceChange { from: false, to: true });
                owner.image_cache_warning_seen.set(true);
            }
            let attachments = owner.pending_attachments.borrow();
            let mut display = value.clone();
            for item in &attachments.items {
                display.push_str(&format!("\n\nAttached: {}", item.name));
            }
            drop(attachments);
            if !owner.conversation.borrow_mut().submit_user(&display) {
                return;
            }

            owner.prompt_history.borrow_mut().record(&value);
            input.setStringValue(ns_string!(""));
            owner.draft.replace(String::new());
            self.schedule_session_view_save();
            self.refresh_chat_transcript();
            self.remember_mock_title(&value);
            self.set_chat_busy(true);
            // The sidebar row and the Agents panel show the session at work from
            // the moment the prompt leaves, not from OMP's first event.
            if owner.live_session_id.borrow().is_some() {
                self.set_live_state(agents::AgentState::Working);
            }
            let items = std::mem::take(&mut owner.pending_attachments.borrow_mut().items);
            // Drop native preview caches on main; only prepared payloads cross threads.
            let attachments: Vec<_> = items.into_iter().filter_map(|item| item.result.and_then(Result::ok)).collect();
            self.refresh_pending_attachments();
            let generation = owner.attachment_generation.get();
            let key = owner.key;
            let job = self.begin_job("Preparing message attachments…");
            owner.attachment_submission.set(true);
            self.update_chat_controls();
            if !self.jobs().submit(move || {
                BackgroundEvent::AttachmentPromptPrepared {
                    job, key, generation, prompt: attachments::compose_prompt(value, &attachments),
                }
            }) {
                self.finish_job(job);
                owner.attachment_submission.set(false);
                self.runtime_command_failed("preparing attachments");
            }
        }

        #[unsafe(method(stopSidebarSession:))]
        fn stop_sidebar_session(&self, sender: &AnyObject) {
            let Some(item) = sender.downcast_ref::<NSMenuItem>() else { return; };
            let key = live_sessions::SessionKey(item.tag() as u64);
            let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
            // An abort with no turn running stays queued in OMP and cancels
            // the next prompt, so only a busy session that is not already
            // stopping is sent one.
            if !owner.chat_busy.get() || matches!(owner.lifecycle.get(), live_sessions::Lifecycle::Stopping) {
                return;
            }
            if let Some(runtime) = owner.omp_runtime.get() {
                if runtime.abort() { owner.lifecycle.set(live_sessions::Lifecycle::Stopping); self.set_session_status(&owner, "Stopping this session…"); }
            }
        }

        #[unsafe(method(stopOmp:))]
        fn stop_omp(&self, _sender: &AnyObject) {
        let owner = self.session();
            // A second press while the first stop is in flight would leave an
            // interrupt queued in OMP that cancels the user's next prompt.
            if !owner.chat_busy.get()
                || matches!(owner.lifecycle.get(), live_sessions::Lifecycle::Stopping)
            {
                return;
            }
            if let Some(runtime) = owner.omp_runtime.get() {
                if runtime.abort() {
                    owner.lifecycle.set(live_sessions::Lifecycle::Stopping);
                    self.set_chat_status(if self.mock_backend_active() { "Stopping mock output…" } else { "Stopping…" });
                } else {
                    self.runtime_command_failed("stopping the active turn");
                }
            }
        }

        #[unsafe(method(pollTermination:))]
        fn poll_termination(&self, timer: &NSTimer) {
            if !self.ivars().termination_pending.get() { timer.invalidate(); return; }
            self.poll_background_jobs();
            self.poll_preference_saves(sel!(pollPreferenceSaves:), self);
            if self.all_session_workers_reaped()
                && self.ivars().preference_saves_pending.get() == 0
                && !self.waiting_for_session_views()
            {
                timer.invalidate();
                self.complete_termination_if_ready();
            }
        }

        #[unsafe(method(pollOmpRuntime:))]
        fn poll_omp_runtime(&self, _sender: &AnyObject) {
            let _tick = perf::span("gui.tick");
            self.request_usage_read(false);
            {
                let _span = perf::span("gui.tick.background_jobs");
                self.poll_background_jobs();
                self.poll_removals();
            }
            {
                let _span = perf::span("gui.tick.project_watcher");
                self.poll_project_watcher();
            }
            {
                let _span = perf::span("gui.tick.session_discovery");
                self.poll_session_discovery();
            }
            {
                let _span = perf::span("gui.tick.session_history");
                self.poll_session_history();
            }
            {
                let _span = perf::span("gui.tick.timeline_stream");
                self.poll_timeline_stream();
            }
            {
                let _span = perf::span("gui.tick.workspace_inspection");
                self.poll_workspace_inspection();
            }
            {
                let _span = perf::span("gui.tick.workspace_diff");
                self.poll_workspace_diff();
            }
            {
                let _span = perf::span("gui.tick.agents_panel");
                self.tick_agents_panel();
            }
            {
                let _span = perf::span("gui.tick.observatory_clocks");
                self.tick_observatory_clocks();
            }
            {
                let _span = perf::span("gui.tick.speedometer");
                self.tick_speedometer();
            }
            {
                let _span = perf::span("gui.tick.account_banner");
                self.sync_account_banner();
            }
            {
                let _span = perf::span("gui.tick.subagent_transcripts");
                self.sync_subagent_transcripts(false);
            }
            if self.ivars().accounts_ui_dirty.replace(false) {
                self.sync_settings_account_ui();
            }
            if self.ivars().metrics_ui_dirty.replace(false) && self.metrics_page_showing() {
                self.sync_settings_metrics_ui();
            }
            let landed = native_markdown::poll_image_loads();
            if !landed.is_empty() {
                // The rows showing them lay out again; activity rows set
                // images of their own when they are next set.
                self.transcript().images_landed(&landed);
            }
            let drain_started = std::time::Instant::now();
            let count = self.ivars().live_sessions.borrow().len();
            let mut empty = 0;
            let mut backlogged = false;
            while empty < count {
                let Some(entry) = self.ivars().live_sessions.borrow_mut().next_poll() else { break; };
                let mut received = 0;
                if let Some(runtime) = entry.omp_runtime.get() {
                    for _ in 0..8 {
                        let Some((arrived, event)) = runtime.try_event() else { break; };
                        self.apply_session_event(&entry, event, arrived);
                        received += 1;
                        if drain_started.elapsed() >= std::time::Duration::from_millis(4) { break; }
                    }
                }
                if received == 0 { empty += 1; } else { empty = 0; }
                if drain_started.elapsed() >= std::time::Duration::from_millis(4) { backlogged = true; break; }
            }
            if self.ivars().chat_dirty.replace(false) {
                self.refresh_chat_transcript();
            }
            self.flush_refreshes();
            self.tick_cache_warning();
            // A headless benchmark draws what this tick changed, in this pass.
            markdown_bench::after_tick();
            if let Some(timer) = self.ivars().runtime_timer.get() {
                let active = backlogged || self.ivars().authentication_busy.get() || self.ivars().live_sessions.borrow().entries().iter().any(|entry| entry.chat_busy.get() || entry.runtime_starting.get() || entry.attachment_submission.get() || matches!(entry.lifecycle.get(), live_sessions::Lifecycle::Stopping | live_sessions::Lifecycle::Retiring) || entry.subagents.borrow().iter().any(|agent| agents::AgentState::from_status(&agent.status).is_busy()))
                    || self.ivars().termination_pending.get()
                    || self.ivars().timeline_stream.borrow().is_some();
                timer.setFireDate(&NSDate::dateWithTimeIntervalSinceNow(if active { 1.0 / 30.0 } else { 0.5 }));
            }
            self.poll_concurrency_smoke();
            #[cfg(target_os = "linux")]
            self.poll_scroll_smoke();
            self.poll_housekeeping_smoke();
            self.poll_readme_shots();
            self.complete_termination_if_ready();
        }

        #[unsafe(method(connectCurrentAccount:))]
        fn connect_current_account(&self, _sender: &AnyObject) {
            let provider = self.current_model_auth_provider().unwrap_or(AuthProvider::OpenAi);
            self.present_sign_in(provider);
        }

        #[unsafe(method(filterSessions:))]
        fn filter_sessions(&self, _sender: &AnyObject) {
            self.ivars().sidebar_reset_scroll.set(true);
            self.rebuild_sidebar_sessions();
        }

        #[unsafe(method(filterSessionSource:))]
        fn filter_session_source(&self, _sender: &AnyObject) {
            self.ivars().sidebar_reset_scroll.set(true);
            self.rebuild_sidebar_sessions();
        }

        #[unsafe(method(openSidebarSession:))]
        fn open_sidebar_session(&self, sender: &AnyObject) {
        let owner = self.session();
            let Some(button) = sender.downcast_ref::<NSButton>() else {
                self.wash_sidebar_row(None);
                return;
            };
            let Some(session_index) = self.sidebar_row_session_index(button.tag()) else {
                self.wash_sidebar_row(None);
                return;
            };
            let Some(summary) = self.ivars().sessions.borrow().get(session_index).cloned() else {
                self.wash_sidebar_row(None);
                return;
            };
            if owner.selected_session_path.borrow().as_ref() == Some(&summary.path) {
                return;
            }
            self.capture_selected_binding();
            let retained = self.ivars().live_sessions.borrow().find_path(&summary.path);
            let existing = retained.is_some();
            let next = retained.unwrap_or_else(|| self.ivars().live_sessions.borrow_mut().create());
            if !existing {
                self.seed_session_runtime_context(&next, &owner, summary.model.as_deref());
            }
            next.selected_session_path.replace(Some(summary.path.clone()));
            self.ivars().live_sessions.borrow_mut().select(next.key);
            if !existing && summary.is_omperor_owned() {
                self.resume_summary(summary);
            } else if !existing {
                self.view_summary(summary);
            }
            self.render_selected_binding();
        }

        #[unsafe(method(showCacheWarning:))]
        fn show_cache_warning_action(&self, _sender: &AnyObject) { self.show_cache_warning(); }

        #[unsafe(method(tickCacheWarning:))]
        fn tick_cache_warning_action(&self, _sender: &AnyObject) { self.tick_cache_warning(); }

        #[unsafe(method(showUsage:))]
        fn show_usage_action(&self, _sender: &AnyObject) { self.show_usage(); }

        #[unsafe(method(usageFrameDidChange:))]
        fn usage_frame_did_change(&self, _notification: &NSNotification) { self.layout_usage_detail(); }

        #[unsafe(method(selectUsagePage:))]
        fn select_usage_page_action(&self, _sender: &AnyObject) { self.select_usage_page(); }

        #[unsafe(method(refreshUsage:))]
        fn refresh_usage_action(&self, _sender: &AnyObject) { self.request_usage_read(true); }

        #[unsafe(method(newChat:))]
        fn new_chat(&self, _sender: &AnyObject) {
        let owner = self.session();
            #[cfg(feature = "design-preview")]
            if cfg!(feature = "design-preview") { self.preview_new_conversation(); return; }
            let active = owner.active_project_id.borrow().clone();
            let project_id = active.or_else(|| {
                let directory = owner.current_project.borrow().clone();
                self.activate_project_for_directory(&directory);
                owner.active_project_id.borrow().clone()
            });
            if let Some(project_id) = project_id {
                self.new_chat_in_project(&project_id);
            }
        }

        #[unsafe(method(toggleSidebarProject:))]
        fn toggle_sidebar_project(&self, sender: &AnyObject) {
            let Some(project_id) = self.sidebar_row_project_id(sender) else {
                return;
            };
            let collapsed = {
                let mut projects = self.ivars().projects.borrow_mut();
                let Some(project) = projects.iter_mut().find(|project| project.id == project_id)
                else {
                    return;
                };
                project.collapsed = !project.collapsed;
                project.collapsed
            };
            self.write_preferences(move || preferences::set_project_collapsed(&project_id, collapsed));
            self.rebuild_sidebar_sessions();
        }

        #[unsafe(method(toggleProjectSessions:))]
        fn toggle_project_sessions(&self, sender: &AnyObject) {
            let Some(project_id) = self.sidebar_row_project_id(sender) else { return; };
            let mut expanded = self.ivars().sidebar_expanded_projects.borrow_mut();
            if !expanded.remove(&project_id) { expanded.insert(project_id); }
            drop(expanded);
            self.rebuild_sidebar_sessions();
        }

        #[unsafe(method(newChatInProject:))]
        fn new_chat_in_project_action(&self, sender: &AnyObject) {
            #[cfg(feature = "design-preview")]
            if cfg!(feature = "design-preview") { self.preview_new_conversation(); return; }
            if let Some(project_id) = self.sidebar_row_project_id(sender) {
                self.new_chat_in_project(&project_id);
            }
        }

        #[unsafe(method(forkSidebarSession:))]
        fn fork_sidebar_session(&self, sender: &AnyObject) {
            if let Some(summary) = self.sender_session(sender) {
                self.fork_session_in_place(summary);
            }
        }

        #[unsafe(method(forkSidebarSessionIntoWorktree:))]
        fn fork_sidebar_session_into_worktree(&self, sender: &AnyObject) {
            if let Some(summary) = self.sender_session(sender) {
                self.fork_session_into_worktree(summary);
            }
        }

        /// The item names its session by id, not by row: the sidebar can be
        /// rebuilt while its menu is open.
        #[unsafe(method(deleteSidebarSession:))]
        fn delete_sidebar_session(&self, sender: &AnyObject) {
            if let Some(id) = Self::represented_id(sender) {
                self.confirm_session_deletion(id);
            }
        }

        #[unsafe(method(newChatInSidebarProject:))]
        fn new_chat_in_sidebar_project(&self, sender: &AnyObject) {
            if let Some(project_id) = Self::represented_id(sender) {
                self.new_chat_in_project(&project_id);
            }
        }

        #[unsafe(method(archiveSidebarProject:))]
        fn archive_sidebar_project(&self, sender: &AnyObject) {
            if let Some(project_id) = Self::represented_id(sender) {
                self.confirm_project_archive(&project_id);
            }
        }

        #[unsafe(method(showSessionMenu:))]
        fn show_session_menu(&self, sender: &AnyObject) {
            let Some(button) = sender.downcast_ref::<NSButton>() else {
                return;
            };
            let tag = button.tag();
            let Some(summary) = self.sender_session(sender) else {
                return;
            };
            let menu = self.session_context_menu(tag, &summary);
            // A non-flipped view's origin is its bottom-left corner, so the
            // menu drops down from the button.
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(0.0, 0.0), Some(button));
        }

        #[unsafe(method(chooseSlashCommand:))]
        fn choose_slash_command(&self, sender: &NSButton) {
            self.choose_slash_row(sender.tag());
        }

        /// A usage card's Retry, sent up the responder chain.
        #[unsafe(method(retryAccountUsage:))]
        fn retry_account_usage_action(&self, _sender: Option<&AnyObject>) {
            self.retry_account_usage();
        }

        #[unsafe(method(chooseFilePickerRow:))]
        fn choose_file_picker_row(&self, sender: &AnyObject) {
            if let Some(button) = sender.downcast_ref::<NSButton>()
                && let Ok(index) = usize::try_from(button.tag())
            {
                self.ivars().file_picker_selected.set(index);
                self.commit_file_picker();
            }
        }

        #[unsafe(method(refreshModels:))]
        fn refresh_models(&self, _sender: &AnyObject) {
        let owner = self.session();
            if owner.chat_busy.get() || owner.new_session_pending.get() {
                self.set_chat_status("Stop the active turn before refreshing the model list");
                return;
            }
            self.set_chat_status("Refreshing the model list from your accounts…");
            let sent = owner.omp_runtime
                .get()
                .is_some_and(|runtime| runtime.refresh_models());
            if !sent {
                self.runtime_command_failed("refreshing the model list");
            }
        }

        #[unsafe(method(showSettingsProjects:))]
        fn show_settings_projects(&self, _sender: &AnyObject) {
            self.show_settings_section(SETTINGS_PROJECTS_PAGE);
        }

        #[unsafe(method(settingsRenameProject:))]
        fn settings_rename_project(&self, sender: &AnyObject) {
            if let Some(project) = self.sender_project(sender) {
                self.rename_project_prompt(&project);
            }
        }

        #[unsafe(method(settingsAddProjectFolder:))]
        fn settings_add_project_folder(&self, sender: &AnyObject) {
            if let Some(project) = self.sender_project(sender) {
                self.add_project_folder(&project);
            }
        }

        #[unsafe(method(settingsRemoveProjectFolder:))]
        fn settings_remove_project_folder(&self, sender: &AnyObject) {
            let Some(button) = sender.downcast_ref::<NSButton>() else {
                return;
            };
            let Ok(tag) = usize::try_from(button.tag()) else {
                return;
            };
            let (project_index, directory_index) = (
                tag / SETTINGS_FOLDER_TAG_STRIDE,
                tag % SETTINGS_FOLDER_TAG_STRIDE,
            );
            let target = self
                .ivars()
                .projects
                .borrow()
                .get(project_index)
                .and_then(|project| {
                    project
                        .directories
                        .get(directory_index)
                        .map(|directory| (project.id.clone(), directory.clone()))
                });
            if let Some((project_id, directory)) = target {
                self.write_project_registry(move || preferences::remove_project_directory(&project_id, &directory));
            }
        }

        #[unsafe(method(settingsDeleteProject:))]
        fn settings_delete_project(&self, sender: &AnyObject) {
            if let Some(project) = self.sender_project(sender) {
                self.delete_project_prompt(&project);
            }
        }

        #[unsafe(method(settingsArchiveProject:))]
        fn settings_archive_project(&self, sender: &AnyObject) {
            if let Some(project) = self.sender_project(sender) {
                self.confirm_project_archive(&project.id);
            }
        }

        #[unsafe(method(settingsRestoreArchive:))]
        fn settings_restore_archive(&self, sender: &NSButton) {
            if let Some(listing) = self.archive_at(sender.tag()) {
                self.restore_archive(listing.manifest.id, housekeeping_ui::Origin::User);
            }
        }

        #[unsafe(method(settingsRevealArchive:))]
        fn settings_reveal_archive(&self, sender: &NSButton) {
            self.reveal_archive(sender.tag());
        }

        #[unsafe(method(settingsDeleteArchive:))]
        fn settings_delete_archive(&self, sender: &NSButton) {
            self.confirm_archive_deletion(sender.tag());
        }

        #[unsafe(method(removeEmptySessions:))]
        fn remove_empty_sessions_action(&self, _sender: &AnyObject) {
            self.remove_empty_sessions(housekeeping_ui::Origin::User);
        }

        #[unsafe(method(revealArchives:))]
        fn reveal_archives(&self, _sender: &AnyObject) {
            if let Ok(path) = preferences::archives_directory() {
                self.reveal_directory(&path);
            }
        }

        #[unsafe(method(chooseProject:))]
        fn choose_project(&self, _sender: &AnyObject) {
        let owner = self.session();
            if self.ivars().project_choice_inflight.get() {
                return;
            }
            let panel = NSOpenPanel::openPanel(self.mtm());
            panel.setCanChooseDirectories(true);
            panel.setCanChooseFiles(false);
            panel.setAllowsMultipleSelection(false);
            if panel.runModal() != NSModalResponseOK {
                return;
            }
            let Some(url) = panel.URLs().firstObject() else {
                return;
            };
            let Some(path) = url.path().map(|path| std::path::PathBuf::from(path.to_string())) else {
                return;
            };
            let session = owner.selected_session_path.borrow().clone();
            let generation = owner.evidence_generation.get();
            let key = owner.key;
            let job = self.begin_job("Opening project…");
            self.ivars().project_choice_inflight.set(true);
            if !self.jobs().submit(move || BackgroundEvent::ProjectChosen {
                job, key, session, generation, result: startup::prepare_project(path),
            }) {
                self.ivars().project_choice_inflight.set(false);
                self.finish_job(job);
                self.set_chat_status("Could not start the project worker");
            }
        }

        #[unsafe(method(showQuickActions:))]
        fn show_quick_actions(&self, _sender: &AnyObject) {
            self.present_quick_actions();
        }

        #[unsafe(method(closeQuickActions:))]
        fn close_quick_actions(&self, _sender: &AnyObject) {
            self.dismiss_quick_actions();
        }

        #[unsafe(method(chooseQuickAction:))]
        fn choose_quick_action(&self, sender: &AnyObject) {
            if let Some(button) = sender.downcast_ref::<NSButton>()
                && let Ok(index) = usize::try_from(button.tag())
            {
                self.run_quick_action(index);
            }
        }

        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: &AnyObject) {
            if self.ivars().settings_ui.get().is_none() {
                let settings = self.build_settings_window(self.mtm());
                self.ivars()
                    .settings_ui
                    .set(settings)
                    .expect("settings window should only be built once");
            }
            self.sync_settings_ui();
            let settings = self
                .ivars()
                .settings_ui
                .get()
                .expect("settings window was just initialized");
            settings.window.makeKeyAndOrderFront(None);
        }

        #[unsafe(method(selectSettingsSection:))]
        fn select_settings_section(&self, sender: &AnyObject) {
            let Some(button) = sender.downcast_ref::<NSButton>() else {
                return;
            };
            self.show_settings_section(button.tag().max(0) as usize);
        }

        #[unsafe(method(chooseSettingsProject:))]
        fn choose_settings_project(&self, sender: &AnyObject) {
            // SAFETY: chooseProject: is implemented above with the standard
            // AppKit action signature on this same delegate instance.
            let _: () = unsafe { msg_send![self, chooseProject: sender] };
            self.sync_settings_ui();
        }

        #[unsafe(method(selectSettingsModel:))]
        fn select_settings_model(&self, _sender: &AnyObject) {
            let Some(model) = self
                .ivars()
                .settings_ui
                .get()
                .and_then(|settings| settings.model_picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.request_model_change(model);
        }

        #[unsafe(method(selectSettingsThinkingLevel:))]
        fn select_settings_thinking_level(&self, _sender: &AnyObject) {
            let Some(level) = self
                .ivars()
                .settings_ui
                .get()
                .and_then(|settings| settings.thinking_picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.request_thinking_change(level);
        }

        #[unsafe(method(settingsDeveloperFrameDidChange:))]
        fn settings_developer_frame_did_change(&self, _notification: &NSNotification) { self.sync_developer_settings(); }

        #[unsafe(method(toggleFrameTiming:))]
        fn toggle_frame_timing(&self, _sender: &AnyObject) { self.change_frame_timing_overlay(); }

        #[unsafe(method(frameOverlayTick:))]
        fn frame_overlay_tick(&self, _sender: &AnyObject) {
            if let Some(overlay) = self.ivars().frame_overlay.get() {
                overlay.refresh();
            }
        }

        #[unsafe(method(toggleDeveloperMode:))]
        fn toggle_developer_mode(&self, _sender: &AnyObject) { self.change_developer_mode(); }

        #[unsafe(method(selectInferenceBackend:))]
        fn select_inference_backend(&self, _sender: &AnyObject) { self.change_inference_backend(); }

        #[unsafe(method(insertMockExample:))]
        fn insert_mock_example_action(&self, _sender: &AnyObject) { self.insert_mock_example(); }

        #[unsafe(method(selectAppearance:))]
        fn select_appearance(&self, _sender: &AnyObject) {
            let Some(value) = self
                .ivars()
                .settings_ui
                .get()
                .and_then(|settings| settings.appearance_picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            let appearance = match value.as_str() {
                "Light" => AppearancePreference::Light,
                "Dark" => AppearancePreference::Dark,
                _ => AppearancePreference::System,
            };
            self.ivars().preferences.borrow_mut().appearance = appearance;
            self.store_preferences();
            self.apply_appearance(appearance);
            self.invalidate_text_size_examples();
            // The rows restyle themselves when the appearance changes.
            self.refresh_chat_transcript();
        }

        #[unsafe(method(selectChatTextSize:))]
        fn select_chat_text_size(&self, _sender: &AnyObject) {
            let Some(value) = self
                .ivars()
                .settings_ui
                .get()
                .and_then(|settings| settings.text_size_picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.ivars().preferences.borrow_mut().chat_text_size = match value.as_str() {
                "Compact" => ChatTextSize::Compact,
                "Large" => ChatTextSize::Large,
                _ => ChatTextSize::Default,
            };
            self.store_preferences();
            self.apply_chat_typography();
        }

        #[unsafe(method(editChatTextSize:))]
        fn edit_chat_context_size(&self, sender: &NSTextField) {
            self.commit_text_size(sender);
        }

        #[unsafe(method(stepChatTextSize:))]
        fn step_chat_context_size(&self, sender: &NSStepper) {
            if let Some(role) = usize::try_from(sender.tag())
                .ok()
                .and_then(|index| TextRole::ALL.get(index))
                .copied()
            {
                let size = sender.doubleValue();
                self.cancel_text_size_edit(Some(role));
                self.change_text_size(role, Some(size));
                self.sync_text_size_settings();
            }
        }

        #[unsafe(method(resetChatTextSize:))]
        fn reset_chat_context_size(&self, sender: &NSButton) {
            if let Some(role) = usize::try_from(sender.tag())
                .ok()
                .and_then(|index| TextRole::ALL.get(index))
                .copied()
            {
                self.cancel_text_size_edit(Some(role));
                self.change_text_size(role, None);
                self.sync_text_size_settings();
            }
        }

        #[unsafe(method(resetAllChatTextSizes:))]
        fn reset_all_chat_context_sizes(&self, _sender: &AnyObject) {
            self.cancel_text_size_edit(None);
            self.ivars().preferences.borrow_mut().chat_text_sizes = TextSizes::DEFAULTS;
            self.apply_chat_typography();
            self.store_preferences();
        }

        #[unsafe(method(backgroundJobsReady:))]
        fn background_jobs_ready(&self, sender: &AnyObject) {
            if let Some(timer) = self.ivars().runtime_timer.get() {
                timer.setFireDate(&NSDate::date());
            }
            self.poll_background_jobs();
            self.poll_timeline_stream();
            self.poll_preference_saves(sel!(pollPreferenceSaves:), sender);
        }

        #[unsafe(method(saveSessionViews:))]
        fn save_session_views_action(&self, _sender: &AnyObject) {
            self.ivars().session_save_scheduled.set(false);
            self.save_session_views();
        }

        #[unsafe(method(pollPreferenceSaves:))]
        fn poll_preference_saves(&self, _sender: &AnyObject) {
            let Some(writer) = self.ivars().preference_writer.get() else {
                return;
            };
            let mut changed = false;
            while let Some(result) = writer.try_next() {
                changed = true;
                self.ivars().preference_saves_pending.set(
                    self.ivars()
                        .preference_saves_pending
                        .get()
                        .saturating_sub(1),
                );
                match result {
                    Ok(StorageUpdate::Accounts(stored)) => self.apply_account_metadata(stored),
                    Ok(StorageUpdate::Registry(registry)) => {
                        // A later queued write may change the optimistic UI.
                        // Publish only a snapshot after all ordered writes land.
                        if self.ivars().preference_saves_pending.get() == 0 {
                            self.ivars().registry_reload_needed.set(false);
                            self.apply_project_registry(registry);
                        } else {
                            self.ivars().registry_reload_needed.set(true);
                        }
                    }
                    Ok(StorageUpdate::None) => {}
                    Err(error) => {
                        self.set_chat_status(&format!("Could not save Omperor preferences: {error}"));
                        self.ivars()
                            .preference_save_error
                            .replace(Some(error.to_string()));
                    }
                }
            }
            if self.ivars().preference_saves_pending.get() == 0
                && self.ivars().registry_reload_needed.replace(false) {
                self.reload_project_registry();
            }
            if changed {
                self.sync_preference_save_status();
                self.sync_activity_indicator();
            }
            self.complete_termination_if_ready();
        }

        #[unsafe(method(selectChatStyle:))]
        fn select_chat_style(&self, _sender: &AnyObject) {
            let Some(value) = self
                .ivars()
                .settings_ui
                .get()
                .and_then(|settings| settings.chat_style_picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.ivars().preferences.borrow_mut().chat_style = ChatStyle::from_label(&value);
            self.store_preferences();
            self.apply_chat_typography();
        }

        #[unsafe(method(sidebarFrameDidChange:))]
        fn sidebar_frame_did_change(&self, _notification: &NSNotification) {
            self.fit_sidebar_document();
        }

        #[unsafe(method(textSizeExamplesFrameDidChange:))]
        fn text_size_examples_frame_did_change(&self, _notification: &NSNotification) {
            self.layout_text_size_examples();
        }

        #[unsafe(method(settingsProjectsFrameDidChange:))]
        fn settings_projects_frame_did_change(&self, _notification: &NSNotification) {
            if self.ivars().settings_ui.get().is_some() {
                self.sync_settings_projects_ui();
            }
        }

        #[unsafe(method(settingsAccountsFrameDidChange:))]
        fn settings_accounts_frame_did_change(&self, _notification: &NSNotification) {
            if self.ivars().settings_ui.get().is_some() {
                self.sync_settings_account_ui();
            }
        }

        #[unsafe(method(settingsMetricsFrameDidChange:))]
        fn settings_metrics_frame_did_change(&self, _notification: &NSNotification) {
            if self.ivars().settings_ui.get().is_some() {
                self.sync_settings_metrics_ui();
            }
        }

        #[unsafe(method(selectMetricsSession:))]
        fn select_metrics_session(&self, sender: &AnyObject) {
            let Some(button) = sender.downcast_ref::<NSButton>() else {
                return;
            };
            let Ok(row) = usize::try_from(button.tag()) else {
                return;
            };
            let chosen = match self.ivars().metrics_rows.borrow().get(row) {
                Some(Some(session)) => MetricsSelection::Session(session.clone()),
                Some(None) => MetricsSelection::All,
                None => return,
            };
            self.ivars().metrics_selection.replace(chosen);
            self.sync_settings_metrics_ui();
            self.scroll_metrics_to_top();
        }

        #[unsafe(method(settingsRefreshMetrics:))]
        fn settings_refresh_metrics(&self, _sender: &AnyObject) {
            self.request_metrics_read();
        }

        #[unsafe(method(revealMetricsLog:))]
        fn reveal_metrics_log(&self, _sender: &AnyObject) {
            if let Ok(directory) = preferences::omp_sessions_directory() {
                self.prepare_reveal(directory.join(metrics::FILE_NAME), false);
            }
        }

        #[unsafe(method(transcriptFrameDidChange:))]
        fn transcript_frame_did_change(&self, _notification: &NSNotification) {
            self.sync_transcript_measure();
            self.layout_header();
            self.refit_composer_chips();
        }

        #[unsafe(method(toggleComputerUse:))]
        fn toggle_computer_use(&self, _sender: &AnyObject) {
            if let Some(settings) = self.ivars().settings_ui.get() {
                self.ivars().preferences.borrow_mut().computer_use = settings.computer_use.state() == NSControlStateValueOn;
                self.store_preferences();
            }
        }

        #[unsafe(method(toggleInlineImages:))]
        fn toggle_inline_images(&self, _sender: &AnyObject) {
            let Some(settings) = self.ivars().settings_ui.get() else {
                return;
            };
            let shown = settings.inline_images.state() == NSControlStateValueOn;
            self.ivars().preferences.borrow_mut().show_inline_images = shown;
            settings.remote_images.setEnabled(shown);
            self.store_preferences();
            self.refresh_chat_transcript();
        }

        #[unsafe(method(toggleRemoteImages:))]
        fn toggle_remote_images(&self, _sender: &AnyObject) {
            let Some(settings) = self.ivars().settings_ui.get() else {
                return;
            };
            let allowed = settings.remote_images.state() == NSControlStateValueOn;
            self.ivars().preferences.borrow_mut().load_remote_images = allowed;
            native_markdown::allow_remote_images(allowed);
            self.store_preferences();
            self.refresh_chat_transcript();
        }

        #[unsafe(method(revealOmperorSessions:))]
        fn reveal_omperor_sessions(&self, _sender: &AnyObject) {
            if let Some(path) = self.ivars().omperor_sessions_directory.get() {
                self.reveal_directory(path);
            }
        }

        #[unsafe(method(revealOmpData:))]
        fn reveal_omp_data(&self, _sender: &AnyObject) {
            if let Some(path) = self.ivars().omperor_data_directory.get() {
                self.reveal_directory(path);
            }
        }

        #[unsafe(method(settingsAddAccount:))]
        fn settings_add_account(&self, sender: &AnyObject) {
            if let Some(provider) = sender_provider(sender) {
                self.present_sign_in(provider);
            }
        }

        /// The Cancel beside "Connecting…" on an accounts card.
        #[unsafe(method(settingsCancelSignIn:))]
        fn settings_cancel_sign_in(&self, _sender: &AnyObject) {
            self.cancel_sign_in_session();
            self.dismiss_sign_in_sheet();
        }

        #[unsafe(method(signInMethodChanged:))]
        fn sign_in_method_changed(&self, sender: &AnyObject) {
            let Some(picker) = sender.downcast_ref::<NSSegmentedControl>() else {
                return;
            };
            let Some(sheet) = self.ivars().sign_in_sheet.borrow().as_ref().map(SignInSheet::views)
            else {
                return;
            };
            let Some(method) = usize::try_from(picker.selectedSegment())
                .ok()
                .and_then(|index| sheet.provider.methods().get(index))
                .copied()
            else {
                return;
            };
            if method == sheet.method.get() {
                return;
            }
            // The method is a different conversation with OMP: end the one
            // under way and start over.
            self.cancel_sign_in_session();
            self.ivars().authentication_failure.borrow_mut().take();
            self.show_sign_in_method(method);
            self.begin_authentication(sheet.provider, method);
        }

        #[unsafe(method(signInOpenBrowser:))]
        fn sign_in_open_browser(&self, _sender: &AnyObject) {
            self.open_sign_in_page();
        }

        #[unsafe(method(signInCopyLink:))]
        fn sign_in_copy_link(&self, _sender: &AnyObject) {
            let Some((url, _)) = self.ivars().authentication_url.borrow().clone() else {
                return;
            };
            self.copy_to_pasteboard(&url, "the sign-in link");
            if let Some(sheet) = self.ivars().sign_in_sheet.borrow().as_ref() {
                sheet
                    .status
                    .setStringValue(ns_string!("Copied · open it in any browser on this Mac"));
                sheet.status.setTextColor(Some(&theme::accent()));
            }
            // Then back to what the sheet was saying.
            // SAFETY: AppDelegate implements refreshSignInSheet: with the
            // standard timer action signature and outlives the timer.
            let timer = unsafe {
                NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                    motion::FLASH_HOLD,
                    self,
                    sel!(refreshSignInSheet:),
                    None,
                    false,
                )
            };
            // SAFETY: the timer and mode are valid Foundation objects; the run
            // loop retains the one-shot timer until it fires.
            unsafe {
                NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
            }
        }

        #[unsafe(method(refreshSignInSheet:))]
        fn refresh_sign_in_sheet_action(&self, _timer: &AnyObject) {
            self.refresh_sign_in_sheet();
        }

        #[unsafe(method(signInUseCallback:))]
        fn sign_in_use_callback(&self, _sender: &AnyObject) {
            let Some(sheet) = self.ivars().sign_in_sheet.borrow().as_ref().map(SignInSheet::views)
            else {
                return;
            };
            let value = sheet.callback_field.stringValue().to_string();
            let value = value.trim().to_owned();
            if !value.contains("://") {
                sheet
                    .status
                    .setStringValue(ns_string!("Paste the whole address, starting with http"));
                sheet.status.setTextColor(Some(&NSColor::systemRedColor()));
                return;
            }
            self.answer_sign_in(value);
        }

        #[unsafe(method(signInSubmitSecret:))]
        fn sign_in_submit_secret(&self, _sender: &AnyObject) {
            let Some(sheet) = self.ivars().sign_in_sheet.borrow().as_ref().map(SignInSheet::views)
            else {
                return;
            };
            let value = sheet.secret_field.stringValue().to_string();
            let value = value.trim().to_owned();
            if value.is_empty() {
                sheet.window.makeFirstResponder(Some(&sheet.secret_field));
                return;
            }
            sheet.secret_field.setStringValue(ns_string!(""));
            self.answer_sign_in(value);
        }

        #[unsafe(method(signInCancel:))]
        fn sign_in_cancel(&self, _sender: &AnyObject) {
            self.cancel_sign_in_session();
            self.dismiss_sign_in_sheet();
        }

        #[unsafe(method(signInRetry:))]
        fn sign_in_retry(&self, _sender: &AnyObject) {
            let Some(sheet) = self.ivars().sign_in_sheet.borrow().as_ref().map(SignInSheet::views)
            else {
                return;
            };
            self.ivars().authentication_failure.borrow_mut().take();
            self.begin_authentication(sheet.provider, sheet.method.get());
        }

        #[unsafe(method(settingsMakeDefaultAccount:))]
        fn settings_make_default_account(&self, sender: &AnyObject) {
            let Some(entry) = self.sender_account(sender) else {
                return;
            };
            self.write_account_metadata(move || preferences::set_default_account(entry.provider().id(), Some(entry.id())));
        }

        #[unsafe(method(settingsRenameAccount:))]
        fn settings_rename_account(&self, sender: &AnyObject) {
            if let Some(entry) = self.sender_account(sender) {
                self.rename_account(&entry);
            }
        }

        #[unsafe(method(settingsRefreshUsage:))]
        fn settings_refresh_usage(&self, _sender: &AnyObject) {
            self.request_account_usage(true);
        }

        #[unsafe(method(toggleAccountUsageDetails:))]
        fn toggle_account_usage_details(&self, sender: &AnyObject) {
            let Some(entry) = self.sender_account(sender) else {
                return;
            };
            {
                let mut expanded = self.ivars().accounts_usage_expanded.borrow_mut();
                if !expanded.remove(&entry.summary.id) {
                    expanded.insert(entry.summary.id.clone());
                }
            }
            self.sync_settings_account_ui();
        }

        #[unsafe(method(selectAccountsProvider:))]
        fn select_accounts_provider(&self, sender: &AnyObject) {
            let Some(control) = sender.downcast_ref::<NSSegmentedControl>() else {
                return;
            };
            let selected = usize::try_from(control.selectedSegment()).unwrap_or(0);
            self.ivars()
                .accounts_tab
                .set(selected.min(AuthProvider::ALL.len() - 1));
            self.sync_settings_account_ui();
            self.scroll_accounts_to_top();
        }

        #[unsafe(method(settingsRemoveAccount:))]
        fn settings_remove_account(&self, sender: &AnyObject) {
            if let Some(entry) = self.sender_account(sender) {
                self.confirm_remove_account(&entry);
            }
        }

        #[unsafe(method(settingsReauthAccount:))]
        fn settings_reauth_account(&self, sender: &AnyObject) {
            if let Some(entry) = self.sender_account(sender) { self.present_sign_in(entry.provider()); }
        }

        #[unsafe(method(accountAlertManage:))]
        fn account_alert_manage(&self, _sender: &AnyObject) { self.manage_account_alert(); }

        #[unsafe(method(accountAlertSnooze:))]
        fn account_alert_snooze(&self, _sender: &AnyObject) { self.suppress_account_alert(true); }

        #[unsafe(method(accountAlertDismiss:))]
        fn account_alert_dismiss(&self, _sender: &AnyObject) { self.suppress_account_alert(false); }

        #[unsafe(method(selectAccount:))]
        fn select_account(&self, _sender: &AnyObject) {
            #[cfg(feature = "design-preview")]
            if cfg!(feature = "design-preview") { self.preview_select_account(); return; }
            let Some(picker) = self.ivars().account_picker.get() else {
                return;
            };
            let chosen = usize::try_from(picker.indexOfSelectedItem())
                .ok()
                .and_then(|index| {
                    self.provider_accounts(self.current_model_auth_provider())
                        .into_iter()
                        .nth(index)
                });
            match chosen {
                Some(entry) => self.request_account_change(entry.id().to_owned()),
                None => self.sync_account_controls(),
            }
        }

        #[unsafe(method(attachFiles:))]
        fn attach_files(&self, _sender: &AnyObject) {
            let owner = self.session();
            if !self.ivars().can_receive_attachments(&owner) { return; }
            let panel = NSOpenPanel::openPanel(self.mtm());
            panel.setTitle(Some(ns_string!("Attach files")));
            panel.setCanChooseDirectories(false);
            panel.setCanChooseFiles(true);
            panel.setAllowsMultipleSelection(true);
            if panel.runModal() != NSModalResponseOK || self.session().key != owner.key { return; }
            let sources = panel.URLs().iter().filter_map(|url| {
                url.path().map(|path| attachments::AttachmentSource::File(path.to_string().into()))
            }).collect();
            self.add_attachment_sources(sources);
        }

        #[unsafe(method(selectModel:))]
        fn select_model(&self, _sender: &AnyObject) {
            #[cfg(feature = "design-preview")]
            if cfg!(feature = "design-preview") { self.preview_select_model(); return; }
            let Some(model) = self
                .ivars()
                .model_picker
                .get()
                .and_then(|picker| picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.request_model_change(model);
        }

        #[unsafe(method(selectThinkingLevel:))]
        fn select_thinking_level(&self, _sender: &AnyObject) {
            #[cfg(feature = "design-preview")]
            if cfg!(feature = "design-preview") { self.preview_select_thinking(); return; }
            let Some(level) = self
                .ivars()
                .thinking_picker
                .get()
                .and_then(|picker| picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.request_thinking_change(level);
        }

        #[unsafe(method(toggleFastMode:))]
        fn toggle_fast_mode(&self, _sender: &AnyObject) {
        let owner = self.session();
            #[cfg(feature = "design-preview")]
            if cfg!(feature = "design-preview") { self.preview_toggle_fast(); return; }
            let enabled = !owner.fast_mode.get();
            self.request_fast_mode(enabled);
        }

        #[unsafe(method(closeActivityDetail:))]
        fn close_activity_detail(&self, _sender: &AnyObject) {
            self.mark_agent_row_selection();
            let Some(sheet) = self.ivars().activity_sheet.borrow_mut().take() else {
                return;
            };
            if let Some(window) = self.ivars().window.get() {
                window.endSheet(&sheet);
            }
            sheet.orderOut(None);
        }

        #[unsafe(method(showSessionTimeline:))]
        fn show_session_timeline(&self, _sender: &AnyObject) {
        let owner = self.session();
            let Some((id, journal)) = owner.header_session.borrow().clone() else {
                return;
            };
            let title = self
                .ivars()
                .sessions
                .borrow()
                .iter()
                .find(|summary| summary.id == id)
                .map(|summary| summary.title.clone())
                .unwrap_or_else(|| format!("Session {}", short_session_id(&id)));
            self.open_timeline(title, journal);
        }

        #[unsafe(method(showSidebarSessionTimeline:))]
        fn show_sidebar_session_timeline(&self, sender: &AnyObject) {
            if let Some(summary) = self.sender_session(sender) {
                self.open_timeline(summary.title.clone(), summary.path.clone());
            }
        }

        #[unsafe(method(timelineDidScroll:))]
        fn timeline_did_scroll(&self, _notification: &NSNotification) {
            self.sync_timeline_rows();
            self.sync_timeline_viewport();
        }

        #[unsafe(method(timelineFrameDidChange:))]
        fn timeline_frame_did_change(&self, _notification: &NSNotification) {
            self.layout_timeline_rows();
        }

        #[unsafe(method(timelineWindowWillClose:))]
        fn timeline_window_will_close(&self, _notification: &NSNotification) {
            self.clear_timeline_state();
        }

        #[unsafe(method(closeTimeline:))]
        fn close_timeline(&self, _sender: &AnyObject) {
            let window = self.ivars().timeline_window.borrow().clone();
            if let Some(window) = window {
                window.close();
            }
        }

        #[unsafe(method(timelineSearchChanged:))]
        fn timeline_search_changed(&self, _sender: &AnyObject) {
            self.apply_timeline_filter();
        }

        #[unsafe(method(timelineMinChanged:))]
        fn timeline_min_changed(&self, _sender: &AnyObject) {
            self.apply_timeline_filter();
        }

        #[unsafe(method(timelineKindChanged:))]
        fn timeline_kind_changed(&self, _sender: &AnyObject) {
            self.apply_timeline_filter();
        }

        #[unsafe(method(timelineModelChanged:))]
        fn timeline_model_changed(&self, _sender: &AnyObject) {
            self.apply_timeline_filter();
        }

        /// A click on the list: an event opens its details, a hidden run
        /// opens or folds.
        #[unsafe(method(timelineRowClicked:))]
        fn timeline_row_clicked(&self, sender: &AnyObject) {
            let Some(row) = sender.downcast_ref::<timeline_ui::TimelineRowView>() else {
                return;
            };
            match *row.row() {
                timeline::Row::Event { index, .. } => self.present_timeline_event(index),
                timeline::Row::Hidden { first, count, .. } => {
                    let key = timeline::Row::run_key(first, count);
                    {
                        let mut open = self.ivars().timeline_open_runs.borrow_mut();
                        if !open.remove(&key) {
                            open.insert(key);
                        }
                    }
                    self.drop_timeline_flash();
                    self.refresh_timeline_rows();
                }
            }
        }

        #[unsafe(method(closeTimelineDetail:))]
        fn close_timeline_detail(&self, _sender: &AnyObject) {
            self.dismiss_timeline_detail();
        }

        #[unsafe(method(timelineSmokeTick:))]
        fn timeline_smoke_tick(&self, timer: &NSTimer) {
            self.tick_timeline_smoke(timer);
        }

        #[unsafe(method(timelineMinimapClicked:))]
        fn timeline_minimap_clicked(&self, sender: &AnyObject) {
            let Some(minimap) = sender.downcast_ref::<TimelineMinimapView>() else {
                return;
            };
            let position = minimap.ivars().last_click.get();
            let marks = minimap.ivars().marks.borrow().clone();
            let row = timeline::target_row(&marks, position);
            perf::event_with("gui.timeline.minimap_click", || {
                serde_json::json!({ "position": position, "row": row })
            });
            if let Some(row) = row {
                self.jump_timeline_to_row(row);
            }
        }

        #[unsafe(method(copySessionId:))]
        fn copy_session_id(&self, _sender: &AnyObject) {
        let owner = self.session();
            let Some((id, _)) = owner.header_session.borrow().clone() else {
                return;
            };
            self.copy_to_pasteboard(&id, &format!("session ID {id}"));
        }

        #[unsafe(method(copySessionJournalPath:))]
        fn copy_session_journal_path(&self, _sender: &AnyObject) {
        let owner = self.session();
            let Some((_, journal)) = owner.header_session.borrow().clone() else {
                return;
            };
            self.copy_to_pasteboard(&journal.to_string_lossy(), "the journal path");
        }

        /// A flash has held long enough: fade the line out, put back what it
        /// was saying, and fade it in. A real status change in the meantime
        /// has already cleared the flash, and nothing happens.
        #[unsafe(method(restoreChatStatus:))]
        fn restore_chat_status(&self, _timer: &AnyObject) {
        let owner = self.session();
            if !self.ivars().chat_status_flashing.replace(false) {
                return;
            }
            let Some(status) = self.ivars().chat_status.get() else {
                return;
            };
            let text = owner.chat_status_text.borrow().clone();
            let fading = status.clone();
            let returning = status.clone();
            let size = self.chat_text_size_for(TextRole::Metadata);
            motion::animate_then(
                motion::FADE_OUT,
                move || fading.animator().setAlphaValue(0.0),
                move || {
                    returning.setAttributedStringValue(&status_attributed(&text, size));
                    motion::animate(motion::FADE_OUT, || {
                        returning.animator().setAlphaValue(1.0);
                    });
                },
            );
        }

        #[unsafe(method(revealSessionJournal:))]
        fn reveal_session_journal(&self, _sender: &AnyObject) {
        let owner = self.session();
            let Some((_, journal)) = owner.header_session.borrow().clone() else {
                return;
            };
            self.reveal_in_finder(&journal);
        }

        #[unsafe(method(refreshWorkspace:))]
        fn refresh_workspace(&self, _sender: &AnyObject) {
            self.start_workspace_inspection();
        }

        #[unsafe(method(selectChange:))]
        fn select_change(&self, sender: &AnyObject) {
        let owner = self.session();
            let Some(row) = sender.downcast_ref::<NSButton>() else {
                return;
            };
            let Ok(row_index) = usize::try_from(row.tag()) else {
                return;
            };
            let index = match self.ivars().change_tree_rows.borrow().get(row_index) {
                Some(change_tree::TreeRow::File { change, .. }) => *change,
                _ => return,
            };
            let Some(snapshot) = owner.workspace_snapshot.borrow().clone() else {
                return;
            };
            let Some(change) = snapshot.changes.get(index).cloned() else {
                return;
            };
            owner.selected_change
                .replace(Some(change.path.clone()));
            self.highlight_change_row(Some(row_index));
            self.load_change_diff(snapshot, change);
        }

        #[unsafe(method(toggleChangeDirectory:))]
        fn toggle_change_directory(&self, sender: &AnyObject) {
        let owner = self.session();
            let Some(row) = Self::sender_row(sender) else {
                return;
            };
            let path = match self.ivars().change_tree_rows.borrow().get(row) {
                Some(change_tree::TreeRow::Directory { path, .. }) => path.clone(),
                _ => return,
            };
            {
                let mut collapsed = owner.change_tree_collapsed.borrow_mut();
                if !collapsed.remove(&path) {
                    collapsed.insert(path);
                }
            }
            self.repopulate_change_tree();
        }

        #[unsafe(method(openChangeInEditor:))]
        fn open_change_in_editor(&self, sender: &AnyObject) {
            if let Some((absolute, _)) = Self::sender_row(sender).and_then(|row| self.change_row_target(row)) {
                self.launch_in(launchers::LauncherKind::Editor, absolute);
            }
        }

        #[unsafe(method(openChangeInTerminal:))]
        fn open_change_in_terminal(&self, sender: &AnyObject) {
            if let Some((absolute, _)) = Self::sender_row(sender).and_then(|row| self.change_row_target(row)) {
                self.launch_in(launchers::LauncherKind::Terminal, absolute);
            }
        }

        #[unsafe(method(revealChange:))]
        fn reveal_change(&self, sender: &AnyObject) {
            if let Some((absolute, _)) = Self::sender_row(sender).and_then(|row| self.change_row_target(row)) {
                self.reveal_in_finder(&absolute);
            }
        }

        #[unsafe(method(copyChangePath:))]
        fn copy_change_path(&self, sender: &AnyObject) {
            if let Some((absolute, _)) = Self::sender_row(sender).and_then(|row| self.change_row_target(row)) {
                self.copy_to_pasteboard(&absolute.to_string_lossy(), "the path");
            }
        }

        #[unsafe(method(copyChangeRelativePath:))]
        fn copy_change_relative_path(&self, sender: &AnyObject) {
            if let Some((_, relative)) = Self::sender_row(sender).and_then(|row| self.change_row_target(row)) {
                self.copy_to_pasteboard(&relative.to_string_lossy(), "the relative path");
            }
        }

        #[unsafe(method(openProjectInEditor:))]
        fn open_project_in_editor(&self, _sender: &AnyObject) {
        let owner = self.session();
            let project = owner.current_project.borrow().clone();
            self.launch_in(launchers::LauncherKind::Editor, project);
        }

        #[unsafe(method(openProjectInTerminal:))]
        fn open_project_in_terminal(&self, _sender: &AnyObject) {
        let owner = self.session();
            let project = owner.current_project.borrow().clone();
            self.launch_in(launchers::LauncherKind::Terminal, project);
        }

        #[unsafe(method(revealProject:))]
        fn reveal_project(&self, _sender: &AnyObject) {
        let owner = self.session();
            let project = owner.current_project.borrow().clone();
            self.reveal_in_finder(&project);
        }

        #[unsafe(method(switchWorkbenchPanel:))]
        fn switch_workbench_panel(&self, sender: &AnyObject) {
            let index = sender
                .downcast_ref::<NSSegmentedControl>()
                .map_or(0, |switcher| switcher.selectedSegment());
            self.show_workbench_panel(index);
        }

        #[unsafe(method(toggleWorkbench:))]
        fn toggle_workbench(&self, _sender: &AnyObject) {
            self.set_workbench_visible(!self.ivars().inspector_visible.get());
        }

        #[unsafe(method(expandWorkbench:))]
        fn expand_workbench_action(&self, _sender: &AnyObject) {
            self.expand_agent_workspace();
        }

        #[unsafe(method(selectAgentScope:))]
        fn select_agent_scope(&self, sender: &AnyObject) {
            if let Some(control) = sender.downcast_ref::<NSSegmentedControl>() {
                let scope = match control.selectedSegment() {
                    1 => agents::WorkScope::Active,
                    2 => agents::WorkScope::Attention,
                    _ => agents::WorkScope::All,
                };
                self.edit_agent_view(|view| {
                    view.scope = scope;
                    view.page = 0;
                });
            }
        }

        #[unsafe(method(toggleAgentDensity:))]
        fn toggle_agent_density(&self, _sender: &AnyObject) {
            self.edit_agent_view(|view| view.compact = !view.compact);
        }

        #[unsafe(method(toggleAgentSearch:))]
        fn toggle_agent_search(&self, _sender: &AnyObject) {
            self.reveal_agent_search();
        }

        #[unsafe(method(toggleFinishedAgents:))]
        fn toggle_finished_agents(&self, _sender: &AnyObject) {
            self.edit_agent_view(|view| {
                view.finished_collapsed = !view.finished_collapsed;
                view.page = 0;
            });
        }

        #[unsafe(method(pageAgentWork:))]
        fn page_agent_work(&self, sender: &AnyObject) {
            let next = Self::sender_row(sender) == Some(1);
            self.edit_agent_view(|view| {
                view.page = if next {
                    view.page.saturating_add(1)
                } else {
                    view.page.saturating_sub(1)
                };
            });
        }

        #[unsafe(method(agentWorkspaceFrameDidChange:))]
        fn agent_workspace_frame_changed(&self, _notification: &NSNotification) {
            self.refresh_agents_panel();
        }

        #[unsafe(method(showChangesPanel:))]
        fn show_changes_panel(&self, _sender: &AnyObject) {
            self.show_workbench_panel(0);
        }

        #[unsafe(method(showAgentsPanel:))]
        fn show_agents_panel(&self, _sender: &AnyObject) {
            self.show_workbench_panel(1);
        }

        #[unsafe(method(showConversationSurface:))]
        fn show_conversation_surface(&self, _sender: &AnyObject) { self.set_session_surface(false); }

        #[unsafe(method(showSessionWorkspace:))]
        fn show_session_workspace(&self, _sender: &AnyObject) { self.set_session_surface(true); }

        #[unsafe(method(selectSessionSurface:))]
        fn select_session_surface(&self, sender: &NSSegmentedControl) {
            self.set_session_surface(sender.selectedSegment() == 1);
        }

        #[unsafe(method(pinInspectedTask:))]
        fn pin_inspected_task(&self, _sender: &AnyObject) {
            if let Some(id) = self.agent_view().selected { self.pin_workspace_task(&id); }
        }

        #[unsafe(method(selectWorkspaceTask:))]
        fn select_workspace_task(&self, sender: &AnyObject) {
            if let Some(row) = sender.downcast_ref::<AgentRowButton>() {
                self.open_workspace_agent(&row.subagent_id(), row.ivars().click_count.get() == 2);
            }
        }

        #[unsafe(method(observatoryDidScroll:))]
        fn observatory_did_scroll(&self, _notification: &NSNotification) {
            self.scroll_observatory();
        }

        #[unsafe(method(workspaceEvidenceAction:))]
        fn workspace_evidence_action(&self, sender: &NSButton) {
            self.evidence_action(sender.tag());
        }

        #[unsafe(method(openAgentTab:))]
        fn open_agent_tab(&self, sender: &NSMenuItem) {
            self.evidence_action(2000 + sender.tag());
        }

        #[unsafe(method(openSelectedAgentTab:))]
        fn open_selected_agent_tab(&self, _sender: &AnyObject) {
            self.persist_selected_agent_tab();
        }

        #[unsafe(method(composeFromEvidence:))]
        fn compose_from_evidence_action(&self, _sender: &AnyObject) {
            self.compose_from_evidence();
        }

        #[unsafe(method(observatoryScope:))]
        fn observatory_scope_action(&self, sender: &NSSegmentedControl) {
            self.workspace_scope(sender.selectedSegment());
        }

        #[unsafe(method(observatoryArrangement:))]
        fn observatory_arrangement(&self, sender: &NSSegmentedControl) {
            self.set_observatory_focus(workspace_model::ObservatoryFocus::from_index(sender.selectedSegment()));
        }

        #[unsafe(method(workspaceScope:))]
        fn workspace_scope_action(&self, sender: &NSButton) {
            self.workspace_scope(sender.tag());
        }

        #[unsafe(method(inspectAgent:))]
        fn inspect_agent(&self, sender: &AnyObject) {
            self.inspect_agent_row(sender);
        }

        #[unsafe(method(closeAgentInspection:))]
        fn close_agent_inspection_action(&self, _sender: &AnyObject) {
            self.close_agent_inspection();
        }

        #[unsafe(method(selectEditorApp:))]
        fn select_editor_app(&self, _sender: &AnyObject) {
            let Some(value) = self
                .ivars()
                .settings_ui
                .get()
                .and_then(|settings| settings.editor_picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.ivars().preferences.borrow_mut().editor =
                (value != SETTINGS_AUTOMATIC_APP).then_some(value);
            self.store_preferences();
            self.sync_launcher_titles();
        }

        #[unsafe(method(selectBrowserApp:))]
        fn select_browser_app(&self, _sender: &AnyObject) {
            let Some(value) = self
                .ivars()
                .settings_ui
                .get()
                .and_then(|settings| settings.browser_picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.ivars().preferences.borrow_mut().browser =
                (value != SETTINGS_SYSTEM_BROWSER).then_some(value);
            self.store_preferences();
            self.refresh_sign_in_sheet();
        }

        #[unsafe(method(selectTerminalApp:))]
        fn select_terminal_app(&self, _sender: &AnyObject) {
            let Some(value) = self
                .ivars()
                .settings_ui
                .get()
                .and_then(|settings| settings.terminal_picker.titleOfSelectedItem())
                .map(|value| value.to_string())
            else {
                return;
            };
            self.ivars().preferences.borrow_mut().terminal =
                (value != SETTINGS_AUTOMATIC_APP).then_some(value);
            self.store_preferences();
            self.sync_launcher_titles();
        }
    }
);

impl AppDelegate {
    fn all_session_workers_reaped(&self) -> bool {
        let mut reaped = true;
        for owner in self.ivars().live_sessions.borrow().entries() {
            if let Some(runtime) = owner.omp_runtime.get() { reaped &= runtime.try_reap(); }
        }
        reaped
    }

    fn session(&self) -> Rc<live_sessions::LiveSession> {
        self.ivars().live_sessions.borrow().selected()
    }

    fn capture_selected_binding(&self) {
        let _span = perf::span("gui.switch.capture");
        let owner = self.session();
        if let Some(input) = self.ivars().input.get() {
            owner.draft.replace(input.stringValue().to_string());
        }
        self.capture_session_desk();
        owner.pending_attachments.borrow_mut().detach_presentation();
        self.schedule_session_view_save();
    }

    /// Shows the selected session. Each step is named in the frame ledger:
    /// a switch over budget says which.
    fn render_selected_binding(&self) {
        let _span = perf::span("gui.switch.render");
        let owner = self.session();
        if let Some(input) = self.ivars().input.get() {
            input.setStringValue(&NSString::from_str(&owner.draft.borrow()));
        }
        {
            let _span = perf::span("gui.switch.transcript_reset");
            self.transcript().reset();
        }
        self.ivars().desk_restore_pending.set(true);
        {
            let _span = perf::span("gui.switch.desk");
            self.restore_session_desk();
        }
        self.refresh_chat_transcript();
        {
            let _span = perf::span("gui.switch.chrome");
            self.refresh_pending_attachments();
            self.refresh_project_location();
            self.sync_selected_session_header();
            self.populate_model_picker(&owner.available_models.borrow(), &owner.current_model.borrow());
            self.set_thinking_label(&owner.current_thinking_level.borrow());
            self.refresh_fast_chip();
            self.refresh_speedometer();
            self.update_chat_controls();
        }
        // A new chat left empty goes when another session is opened.
        self.discard_empty_drafts();
        // The sidebar follows in the next pass: this one shows the
        // session's rows in sight, within the frame.
        self.rebuild_sidebar_next_pass();
        {
            let _span = perf::span("gui.switch.agents");
            self.request_agents_refresh();
        }
        let status = owner.chat_status_text.borrow().clone();
        self.set_chat_status(&status);
    }

    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppDelegateIvars::default());
        // SAFETY: NSObject's -init signature is correctly declared by objc2.
        unsafe { msg_send![super(this), init] }
    }

    fn build_window(&self, mtm: MainThreadMarker) {
        let _span = perf::span("gui.build_window");
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        // SAFETY: setReleasedWhenClosed(false) below opts out of AppKit's legacy
        // over-release behavior for windows created without NSWindowController.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, WINDOW_WIDTH, WINDOW_HEIGHT),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.setTitle(&NSString::from_str(app_state::Channel::current().app_name()));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setContentMinSize(NSSize::new(1080.0, 720.0));
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));

        let content = window
            .contentView()
            .expect("window must have a content view");
        let split_view = NSSplitView::initWithFrame(
            NSSplitView::alloc(mtm),
            rect(0.0, 0.0, WINDOW_WIDTH, WINDOW_HEIGHT),
        );
        split_view.setVertical(true);
        split_view.setDelegate(Some(ProtocolObject::from_ref(self)));
        split_view.setDividerStyle(NSSplitViewDividerStyle::Thin);
        split_view.setAutosaveName(Some(ns_string!("OmperorCommandWorkspaceSplit")));
        split_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let center_width = WINDOW_WIDTH - SIDEBAR_WIDTH - 1.0;
        split_view.addSubview(&self.build_sidebar(mtm));
        split_view.addSubview(&self.build_conversation_pane(center_width, mtm));
        self.ivars()
            .inspector_pane
            .set(self.build_inspector(mtm))
            .expect("inspector once");
        split_view.adjustSubviews();
        split_view.setPosition_ofDividerAtIndex(SIDEBAR_WIDTH, 0);

        content.addSubview(&split_view);
        self.refresh_chat_transcript();
        self.refit_composer_chips();
        self.ivars()
            .split_view
            .set(split_view)
            .expect("split view should only be initialized once");
        self.show_workbench_panel(1);

        // A headless benchmark or dump places the window off every display
        // and orders it in without activating; otherwise it is shown.
        if housekeeping_smoke::requested() {
            housekeeping_smoke::hide(&window);
        } else if readme_shots::requested() {
            readme_shots::place(&window);
        } else if !markdown_bench::present(&window) {
            // The size and place the user left the window in come back next
            // launch; a first launch centers it.
            let restored = window.setFrameUsingName(ns_string!("OmperorMainWindow"));
            window.setFrameAutosaveName(ns_string!("OmperorMainWindow"));
            if !restored {
                window.center();
            }
            window.makeKeyAndOrderFront(None);
        }
        perf::event("gui.window_visible");
        window.makeFirstResponder(Some(
            self.ivars()
                .input
                .get()
                .expect("input was just initialized"),
        ));
        self.ivars()
            .window
            .set(window)
            .expect("window should only be initialized once");
    }

    fn build_settings_window(&self, mtm: MainThreadMarker) -> SettingsUi {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        // SAFETY: This window is retained by SettingsUi and explicitly opts
        // out of AppKit's legacy release-on-close ownership behavior.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, SETTINGS_WIDTH, SETTINGS_HEIGHT),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&format!("{} Settings", app_state::Channel::current().app_name())));
        window.setContentMinSize(NSSize::new(820.0, 560.0));
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));

        let root = window
            .contentView()
            .expect("settings window must have a content view");
        let sidebar = NSVisualEffectView::initWithFrame(
            NSVisualEffectView::alloc(mtm),
            rect(0.0, 0.0, SETTINGS_SIDEBAR_WIDTH, SETTINGS_HEIGHT),
        );
        sidebar.setMaterial(NSVisualEffectMaterial::Sidebar);
        sidebar.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        sidebar.setState(NSVisualEffectState::FollowsWindowActiveState);
        sidebar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        let wash = fill_box(
            rect(0.0, 0.0, SETTINGS_SIDEBAR_WIDTH, SETTINGS_HEIGHT),
            &theme::sidebar(),
            0.0,
            mtm,
        );
        wash.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        sidebar.addSubview(&wash);

        let sidebar_title = text_label("Settings", 21.0, true, mtm);
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        sidebar_title.setFont(Some(&NSFont::systemFontOfSize_weight(22.0, unsafe {
            NSFontWeightSemibold
        })));
        sidebar_title.setFrame(rect(22.0, SETTINGS_HEIGHT - 58.0, 170.0, 28.0));
        sidebar_title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        sidebar.addSubview(&sidebar_title);

        let categories = [
            (0, "General", "gearshape"),
            (1, "Projects", "folder"),
            (SETTINGS_ARCHIVES_PAGE, "Archives", "archivebox"),
            (2, "Agent", "sparkles"),
            (3, "Appearance", "paintbrush"),
            (SETTINGS_TEXT_PAGE, "Chat text", "textformat.size"),
            (4, "Sessions & Data", "externaldrive"),
            (5, "Accounts", "person.crop.circle"),
            (6, "Metrics", "gauge.with.needle"),
            (SETTINGS_DEVELOPER_PAGE, "Developer", "hammer"),
        ];
        let mut category_buttons = Vec::with_capacity(categories.len());
        for (index, (page_index, title, symbol)) in categories.into_iter().enumerate() {
            let y = SETTINGS_HEIGHT - 108.0 - index as f64 * 43.0;
            let button = navigation_button(
                title,
                symbol,
                rect(14.0, y, SETTINGS_SIDEBAR_WIDTH - 28.0, 34.0),
                self,
                sel!(selectSettingsSection:),
                mtm,
            );
            button.setTag(page_index as isize);
            let font = if index == 0 {
                NSFont::boldSystemFontOfSize(13.0)
            } else {
                NSFont::systemFontOfSize(13.0)
            };
            let color = if index == 0 {
                theme::accent()
            } else {
                NSColor::labelColor()
            };
            button.setFont(Some(&font));
            button.setContentTintColor(Some(&color));
            button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
            sidebar.addSubview(&button);
            category_buttons.push(button);
        }
        let saved = text_label("Changes save automatically", 10.0, false, mtm);
        saved.setTextColor(Some(&theme::secondary_text()));
        saved.setFrame(rect(22.0, 22.0, 180.0, 18.0));
        saved.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        sidebar.addSubview(&saved);
        root.addSubview(&sidebar);

        let content_frame = rect(
            SETTINGS_SIDEBAR_WIDTH,
            0.0,
            SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH,
            SETTINGS_HEIGHT,
        );
        let general = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
        let agent = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
        let appearance = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
        let data = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
        let account = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
        let projects = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
        let metrics_page = NSView::initWithFrame(NSView::alloc(mtm), content_frame);
        let pages = vec![
            general,
            projects,
            agent,
            appearance,
            data,
            account,
            metrics_page,
            NSView::initWithFrame(NSView::alloc(mtm), content_frame),
            NSView::initWithFrame(NSView::alloc(mtm), content_frame),
            NSView::initWithFrame(NSView::alloc(mtm), content_frame),
        ];
        for (index, page) in pages.iter().enumerate() {
            page.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            page.setHidden(index != 0);
            root.addSubview(page);
        }

        settings_page_header(&pages[0], "General", "Workspace defaults", mtm);
        let general_card = settings_card(rect(40.0, 82.0, 600.0, 428.0), mtm);
        settings_row_text(
            &general_card,
            374.0,
            "Active project",
            "New chats and agent tools run in the active project's folder.",
            mtm,
        );
        let workspace_label = text_label("Loading…", 11.0, false, mtm);
        workspace_label.setTextColor(Some(&theme::secondary_text()));
        workspace_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
        workspace_label.setFrame(rect(20.0, 301.0, 420.0, 20.0));
        workspace_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        general_card.addSubview(&workspace_label);
        let manage_projects = settings_button(
            "Projects…",
            rect(476.0, 297.0, 100.0, 30.0),
            self,
            sel!(showSettingsProjects:),
            mtm,
        );
        manage_projects.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        general_card.addSubview(&manage_projects);
        settings_separator(&general_card, 276.0, mtm);
        settings_row_text(
            &general_card,
            231.0,
            "Open project in",
            "The editor \"Open Project\" and a change's \"Open in\" hand files to.",
            mtm,
        );
        let editor_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(384.0, 217.0, 192.0, 34.0),
            false,
        );
        unsafe {
            editor_picker.setTarget(Some::<&AnyObject>(self));
            editor_picker.setAction(Some(sel!(selectEditorApp:)));
        }
        editor_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        general_card.addSubview(&editor_picker);
        settings_separator(&general_card, 190.0, mtm);
        settings_row_text(
            &general_card,
            145.0,
            "Open terminal in",
            "The terminal \"Open Terminal\" starts in the project or a changed file's folder.",
            mtm,
        );
        let terminal_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(384.0, 131.0, 192.0, 34.0),
            false,
        );
        unsafe {
            terminal_picker.setTarget(Some::<&AnyObject>(self));
            terminal_picker.setAction(Some(sel!(selectTerminalApp:)));
        }
        terminal_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        general_card.addSubview(&terminal_picker);
        settings_separator(&general_card, 104.0, mtm);
        settings_row_text(
            &general_card,
            59.0,
            "Sign in with",
            "The browser account sign-in pages open in.",
            mtm,
        );
        let browser_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(384.0, 45.0, 192.0, 34.0),
            false,
        );
        unsafe {
            browser_picker.setTarget(Some::<&AnyObject>(self));
            browser_picker.setAction(Some(sel!(selectBrowserApp:)));
        }
        browser_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        general_card.addSubview(&browser_picker);
        general_card.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        pages[0].addSubview(&general_card);
        let developer = self.build_developer_settings(&pages[SETTINGS_DEVELOPER_PAGE], mtm);

        settings_page_header(
            &pages[SETTINGS_PROJECTS_PAGE],
            "Projects",
            "Each project groups sessions; every session runs in one of its folders",
            mtm,
        );
        let projects_note = text_label(
            "Forking a session creates a git worktree on a new branch and files the fork under the same project.",
            11.0,
            false,
            mtm,
        );
        projects_note.setTextColor(Some(&theme::secondary_text()));
        projects_note.setMaximumNumberOfLines(2);
        projects_note.setFrame(rect(42.0, 18.0, 596.0, 38.0));
        projects_note.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        pages[SETTINGS_PROJECTS_PAGE].addSubview(&projects_note);
        let projects_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(
                0.0,
                SETTINGS_ACCOUNT_NOTE_HEIGHT,
                SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH,
                SETTINGS_HEIGHT - SETTINGS_PAGE_HEADER_HEIGHT - SETTINGS_ACCOUNT_NOTE_HEIGHT,
            ),
        );
        projects_scroll.setBorderType(NSBorderType::NoBorder);
        projects_scroll.setHasVerticalScroller(true);
        projects_scroll.setAutohidesScrollers(true);
        projects_scroll.setDrawsBackground(false);
        projects_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let projects_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(
                0.0,
                0.0,
                SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH,
                SETTINGS_HEIGHT - SETTINGS_PAGE_HEADER_HEIGHT - SETTINGS_ACCOUNT_NOTE_HEIGHT,
            ),
        );
        projects_scroll.setDocumentView(Some(&projects_content));
        // The project cards are laid out against the scroll view's size, so
        // a resized window re-fits them rather than leaving a document sized
        // for the old window.
        let projects_clip = projects_scroll.contentView();
        projects_clip.setPostsFrameChangedNotifications(true);
        // SAFETY: the delegate implements settingsProjectsFrameDidChange: and,
        // like the clip view, lives for the process; the name is an AppKit static.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(settingsProjectsFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&projects_clip),
            );
        }
        pages[SETTINGS_PROJECTS_PAGE].addSubview(&projects_scroll);
        let (archives_scroll, archives_content) =
            self.build_archives_page(&pages[SETTINGS_ARCHIVES_PAGE], mtm);

        settings_page_header(
            &pages[2],
            "Agent",
            "Defaults for native Omperor sessions",
            mtm,
        );
        let agent_card = settings_card(rect(40.0, 127.0, 600.0, 383.0), mtm);
        settings_row_text(
            &agent_card,
            333.0,
            "Default model",
            "Used for this chat and future chats.",
            mtm,
        );
        let model_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(336.0, 319.0, 240.0, 34.0),
            false,
        );
        model_picker.addItemWithTitle(ns_string!("Loading models…"));
        model_picker.setEnabled(false);
        unsafe {
            model_picker.setTarget(Some::<&AnyObject>(self));
            model_picker.setAction(Some(sel!(selectSettingsModel:)));
        }
        model_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        agent_card.addSubview(&model_picker);
        settings_separator(&agent_card, 292.0, mtm);
        settings_row_text(
            &agent_card,
            247.0,
            "Reasoning effort",
            "Controls how much reasoning the selected model uses.",
            mtm,
        );
        let thinking_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(424.0, 233.0, 152.0, 34.0),
            false,
        );
        for level in ["off", "minimal", "low", "medium", "high", "xhigh"] {
            thinking_picker.addItemWithTitle(&NSString::from_str(level));
        }
        unsafe {
            thinking_picker.setTarget(Some::<&AnyObject>(self));
            thinking_picker.setAction(Some(sel!(selectSettingsThinkingLevel:)));
        }
        thinking_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        agent_card.addSubview(&thinking_picker);
        settings_separator(&agent_card, 206.0, mtm);
        settings_row_text(
            &agent_card,
            160.0,
            "Tool access",
            "Read, edit, shell, agents, and browser tools.",
            mtm,
        );
        let tools = text_label("Standard tool set", 11.0, true, mtm);
        tools.setTextColor(Some(&theme::secondary_text()));
        tools.setAlignment(NSTextAlignment::Right);
        tools.setFrame(rect(430.0, 147.0, 146.0, 20.0));
        tools.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        agent_card.addSubview(&tools);
        settings_separator(&agent_card, 119.0, mtm);
        settings_row_text(
            &agent_card,
            73.0,
            "Model list",
            "Ask OMP to rediscover the models your connected accounts can use.",
            mtm,
        );
        let refresh_models = settings_button(
            "Refresh Models…",
            rect(430.0, 55.0, 146.0, 30.0),
            self,
            sel!(refreshModels:),
            mtm,
        );
        refresh_models.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        agent_card.addSubview(&refresh_models);
        agent_card.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        pages[2].addSubview(&agent_card);
        let computer_use = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Allow computer use in new sessions"), Some(self), Some(sel!(toggleComputerUse:)), mtm)
        };
        computer_use.setFrame(rect(56.0, 78.0, 500.0, 28.0));
        computer_use.setToolTip(Some(ns_string!("Allows agents to capture the screen and control native apps. Requires macOS permissions. Applies when a session's backend next starts.")));
        computer_use.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin);
        pages[2].addSubview(&computer_use);


        settings_page_header(
            &pages[3],
            "Appearance",
            "Native interface and transcript rendering",
            mtm,
        );
        let appearance_card = settings_card(rect(40.0, 128.0, 600.0, 382.0), mtm);
        settings_row_text(
            &appearance_card,
            332.0,
            "Interface appearance",
            "Follow macOS or keep Omperor light or dark.",
            mtm,
        );
        let appearance_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(424.0, 318.0, 152.0, 34.0),
            false,
        );
        for option in ["System", "Light", "Dark"] {
            appearance_picker.addItemWithTitle(&NSString::from_str(option));
        }
        unsafe {
            appearance_picker.setTarget(Some::<&AnyObject>(self));
            appearance_picker.setAction(Some(sel!(selectAppearance:)));
        }
        appearance_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        appearance_card.addSubview(&appearance_picker);
        settings_separator(&appearance_card, 291.0, mtm);
        settings_row_text(
            &appearance_card,
            246.0,
            "Chat style",
            "Set the conversation in San Francisco, on a serif page, or in monospace.",
            mtm,
        );
        let chat_style_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(424.0, 232.0, 152.0, 34.0),
            false,
        );
        for style in ChatStyle::ALL {
            chat_style_picker.addItemWithTitle(&NSString::from_str(style.label()));
        }
        unsafe {
            chat_style_picker.setTarget(Some::<&AnyObject>(self));
            chat_style_picker.setAction(Some(sel!(selectChatStyle:)));
        }
        chat_style_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        appearance_card.addSubview(&chat_style_picker);
        settings_separator(&appearance_card, 205.0, mtm);
        settings_row_text(
            &appearance_card,
            160.0,
            "Chat text scale",
            "Scales default transcript sizes. Fine-tune each context in Chat text.",
            mtm,
        );
        let text_size_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(424.0, 146.0, 152.0, 34.0),
            false,
        );
        for option in ["Compact", "Default", "Large"] {
            text_size_picker.addItemWithTitle(&NSString::from_str(option));
        }
        unsafe {
            text_size_picker.setTarget(Some::<&AnyObject>(self));
            text_size_picker.setAction(Some(sel!(selectChatTextSize:)));
        }
        text_size_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        appearance_card.addSubview(&text_size_picker);
        settings_separator(&appearance_card, 119.0, mtm);
        settings_row_text(
            &appearance_card,
            73.0,
            "Inline images",
            "Show images from this Mac in replies. Web images load only if allowed: fetching one tells its server the reply was read.",
            mtm,
        );
        let inline_images = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Show images"),
                Some::<&AnyObject>(self),
                Some(sel!(toggleInlineImages:)),
                mtm,
            )
        };
        inline_images.setFrame(rect(448.0, 55.0, 130.0, 28.0));
        inline_images.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        appearance_card.addSubview(&inline_images);
        let remote_images = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Load web images"),
                Some::<&AnyObject>(self),
                Some(sel!(toggleRemoteImages:)),
                mtm,
            )
        };
        remote_images.setFrame(rect(448.0, 27.0, 130.0, 28.0));
        remote_images.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        appearance_card.addSubview(&remote_images);
        appearance_card.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        pages[3].addSubview(&appearance_card);

        settings_page_header(
            &pages[4],
            "Sessions & Data",
            "Private storage for Omperor and its OMP backend",
            mtm,
        );
        let data_card = settings_card(rect(40.0, 214.0, 600.0, 296.0), mtm);
        settings_row_text(
            &data_card,
            246.0,
            "Omperor sessions",
            "Resumable sessions created by this app, isolated from the OMP TUI.",
            mtm,
        )
        .setFrame(rect(20.0, 214.0, 556.0, 32.0));
        let sessions_path = preferences::omp_sessions_directory()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|error| error.to_string());
        let sessions_path_label =
            settings_path_label(&sessions_path, rect(20.0, 202.0, 420.0, 20.0), mtm);
        sessions_path_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        data_card.addSubview(&sessions_path_label);
        let reveal_sessions = settings_button(
            "Show in Finder",
            rect(452.0, 196.0, 124.0, 30.0),
            self,
            sel!(revealOmperorSessions:),
            mtm,
        );
        reveal_sessions.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        data_card.addSubview(&reveal_sessions);
        settings_separator(&data_card, 171.0, mtm);
        settings_row_text(
            &data_card,
            126.0,
            "OMP backend data",
            "Credentials and mutable backend state used only by Omperor.",
            mtm,
        )
        .setFrame(rect(20.0, 94.0, 556.0, 32.0));
        let data_path = preferences::omp_data_directory()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|error| error.to_string());
        let data_path_label = settings_path_label(&data_path, rect(20.0, 82.0, 420.0, 20.0), mtm);
        data_path_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        data_card.addSubview(&data_path_label);
        let reveal_data = settings_button(
            "Show in Finder",
            rect(452.0, 76.0, 124.0, 30.0),
            self,
            sel!(revealOmpData:),
            mtm,
        );
        reveal_data.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        data_card.addSubview(&reveal_data);
        data_card.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        pages[4].addSubview(&data_card);
        let cleanup_card = settings_card(rect(40.0, 60.0, 600.0, 136.0), mtm);
        settings_row_text(
            &cleanup_card,
            100.0,
            "Empty sessions",
            "Sessions OMP started that never received a message. Nothing lists them and they can't be resumed.",
            mtm,
        );
        let sweep = settings_button(
            "Remove Empty Sessions",
            rect(380.0, 90.0, 196.0, 30.0),
            self,
            sel!(removeEmptySessions:),
            mtm,
        );
        sweep.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        cleanup_card.addSubview(&sweep);
        settings_separator(&cleanup_card, 52.0, mtm);
        let omperor_hint = text_label(
            "To clear out drafts, mock sessions, or old projects, type /omperor in a chat and ask the agent.",
            11.0,
            false,
            mtm,
        );
        omperor_hint.setTextColor(Some(&theme::secondary_text()));
        omperor_hint.setFrame(rect(20.0, 18.0, 556.0, 18.0));
        omperor_hint.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        cleanup_card.addSubview(&omperor_hint);
        cleanup_card.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        pages[4].addSubview(&cleanup_card);

        settings_page_header(
            &pages[5],
            "Accounts",
            "Subscriptions OMP can sign in to, and which one each provider uses",
            mtm,
        );
        let account_note = text_label(
            "Authentication is handled by OMP and stored in Omperor's private backend data directory.",
            11.0,
            false,
            mtm,
        );
        account_note.setTextColor(Some(&theme::secondary_text()));
        account_note.setMaximumNumberOfLines(2);
        account_note.setFrame(rect(42.0, 18.0, 596.0, 38.0));
        account_note.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        pages[5].addSubview(&account_note);
        // One tab per provider, so each provider's accounts and their usage
        // get the whole page to themselves.
        let account_tabs = NSSegmentedControl::initWithFrame(
            NSSegmentedControl::alloc(mtm),
            rect(
                40.0,
                SETTINGS_HEIGHT - SETTINGS_PAGE_HEADER_HEIGHT - 32.0,
                360.0,
                26.0,
            ),
        );
        account_tabs.setSegmentCount(AuthProvider::ALL.len() as isize);
        for (index, provider) in AuthProvider::ALL.iter().enumerate() {
            account_tabs
                .setLabel_forSegment(&NSString::from_str(provider.product_name()), index as isize);
        }
        account_tabs.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
        account_tabs.setSegmentStyle(NSSegmentStyle::Rounded);
        account_tabs.setSegmentDistribution(NSSegmentDistribution::FillEqually);
        account_tabs.setSelectedSegment(0);
        account_tabs.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        // SAFETY: AppDelegate implements selectAccountsProvider: and outlives the control.
        unsafe {
            account_tabs.setTarget(Some::<&AnyObject>(self));
            account_tabs.setAction(Some(sel!(selectAccountsProvider:)));
        }
        pages[5].addSubview(&account_tabs);
        // Account cards grow with the number of stored accounts, so the page
        // body scrolls instead of assuming a fixed card count.
        let account_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(
                0.0,
                SETTINGS_ACCOUNT_NOTE_HEIGHT,
                SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH,
                SETTINGS_HEIGHT
                    - SETTINGS_PAGE_HEADER_HEIGHT
                    - SETTINGS_ACCOUNT_TABS_HEIGHT
                    - SETTINGS_ACCOUNT_NOTE_HEIGHT,
            ),
        );
        account_scroll.setBorderType(NSBorderType::NoBorder);
        account_scroll.setHasVerticalScroller(true);
        account_scroll.setAutohidesScrollers(true);
        account_scroll.setDrawsBackground(false);
        account_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let account_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(
                0.0,
                0.0,
                SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH,
                SETTINGS_HEIGHT
                    - SETTINGS_PAGE_HEADER_HEIGHT
                    - SETTINGS_ACCOUNT_TABS_HEIGHT
                    - SETTINGS_ACCOUNT_NOTE_HEIGHT,
            ),
        );
        account_scroll.setDocumentView(Some(&account_content));
        let account_clip = account_scroll.contentView();
        account_clip.setPostsFrameChangedNotifications(true);
        // SAFETY: as for the projects page.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(settingsAccountsFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&account_clip),
            );
        }
        pages[5].addSubview(&account_scroll);

        settings_page_header(
            &pages[SETTINGS_METRICS_PAGE],
            "Metrics",
            "Where a turn's time goes, and how fast each model answers",
            mtm,
        );
        let metrics_note = text_label(
            "Recorded beside the sessions as metrics.jsonl: the provider's token counts and OMP's timings for every response, tool call, and turn. Sub-agents run inside the task tool, so their inference counts as tool time here.",
            11.0,
            false,
            mtm,
        );
        metrics_note.setTextColor(Some(&theme::secondary_text()));
        metrics_note.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
        metrics_note.setMaximumNumberOfLines(3);
        if let Some(cell) = metrics_note.cell() {
            cell.setWraps(true);
            cell.setUsesSingleLineMode(false);
        }
        metrics_note.setFrame(rect(42.0, 12.0, 596.0, 44.0));
        metrics_note.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMaxYMargin | NSAutoresizingMaskOptions::ViewWidthSizable,
        );
        pages[SETTINGS_METRICS_PAGE].addSubview(&metrics_note);
        // The sessions the log knows, down the left; the cards for the one
        // chosen, on the right. Both scroll on their own.
        let metrics_body_height =
            SETTINGS_HEIGHT - SETTINGS_PAGE_HEADER_HEIGHT - SETTINGS_ACCOUNT_NOTE_HEIGHT;
        let metrics_list_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(
                SETTINGS_METRICS_LIST_X,
                SETTINGS_ACCOUNT_NOTE_HEIGHT,
                SETTINGS_METRICS_LIST_WIDTH,
                metrics_body_height,
            ),
        );
        metrics_list_scroll.setBorderType(NSBorderType::NoBorder);
        metrics_list_scroll.setHasVerticalScroller(true);
        metrics_list_scroll.setAutohidesScrollers(true);
        metrics_list_scroll.setDrawsBackground(false);
        metrics_list_scroll.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        let metrics_list_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, SETTINGS_METRICS_LIST_WIDTH, metrics_body_height),
        );
        metrics_list_scroll.setDocumentView(Some(&metrics_list_content));
        pages[SETTINGS_METRICS_PAGE].addSubview(&metrics_list_scroll);
        let metrics_detail_x = SETTINGS_METRICS_LIST_X + SETTINGS_METRICS_LIST_WIDTH + 14.0;
        let metrics_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(
                metrics_detail_x,
                SETTINGS_ACCOUNT_NOTE_HEIGHT,
                SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH - metrics_detail_x,
                metrics_body_height,
            ),
        );
        metrics_scroll.setBorderType(NSBorderType::NoBorder);
        metrics_scroll.setHasVerticalScroller(true);
        metrics_scroll.setAutohidesScrollers(true);
        metrics_scroll.setDrawsBackground(false);
        metrics_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let metrics_content = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(
                0.0,
                0.0,
                SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH - metrics_detail_x,
                metrics_body_height,
            ),
        );
        metrics_scroll.setDocumentView(Some(&metrics_content));
        let metrics_clip = metrics_scroll.contentView();
        metrics_clip.setPostsFrameChangedNotifications(true);
        // SAFETY: as for the projects page.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(settingsMetricsFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&metrics_clip),
            );
        }
        pages[SETTINGS_METRICS_PAGE].addSubview(&metrics_scroll);

        let text_sizes = self.build_chat_typography_page(&pages[SETTINGS_TEXT_PAGE], mtm);

        // The size and place the user leaves the window in come back next
        // launch; a first launch centers it. This waits until every page is
        // in place so the restored size flows through their resizing masks.
        let restored = window.setFrameUsingName(ns_string!("OmperorSettings"));
        window.setFrameAutosaveName(ns_string!("OmperorSettings"));
        if !restored {
            window.center();
        }

        SettingsUi {
            developer,
            text_sizes,
            save_status: saved,
            window,
            pages,
            category_buttons,
            workspace_label,
            model_picker,
            thinking_picker,
            appearance_picker,
            text_size_picker,
            chat_style_picker,
            inline_images,
            remote_images,
            computer_use,
            editor_picker,
            terminal_picker,
            browser_picker,
            account_scroll,
            account_content,
            account_tabs,
            projects_scroll,
            projects_content,
            archives_scroll,
            archives_content,
            metrics_list_scroll,
            metrics_list_content,
            metrics_scroll,
            metrics_content,
        }
    }

    fn build_sidebar(&self, mtm: MainThreadMarker) -> Retained<NSVisualEffectView> {
        let view = NSVisualEffectView::initWithFrame(
            NSVisualEffectView::alloc(mtm),
            rect(0.0, 0.0, SIDEBAR_WIDTH, WINDOW_HEIGHT),
        );
        view.setMaterial(NSVisualEffectMaterial::Sidebar);
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        // An opaque sidebar keeps text contrast stable over any desktop.
        let wash = fill_box(
            rect(0.0, 0.0, SIDEBAR_WIDTH, WINDOW_HEIGHT),
            &theme::navigation_surface(),
            0.0,
            mtm,
        );
        wash.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.addSubview(&wash);

        // Center the symbol on the title’s visible cap height, not its text-field
        // bounds (which include baseline/descent padding).
        let mark = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            rect(18.0, WINDOW_HEIGHT - 78.0, 28.0, 28.0),
        );
        mark.setImage(
            theme::symbol("crown.fill", 25.0, unsafe { NSFontWeightSemibold }, None).as_deref(),
        );
        mark.setContentTintColor(Some(&theme::accent()));
        mark.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        mark.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        let brand = text_label(app_state::Channel::current().app_name(), 22.0, true, mtm);
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        brand.setFont(Some(&NSFont::systemFontOfSize_weight(22.0, unsafe {
            NSFontWeightSemibold
        })));
        brand.setFrame(rect(56.0, WINDOW_HEIGHT - 83.0, 180.0, 30.0));
        brand.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        let new_chat = navigation_button(
            "New conversation",
            "plus",
            rect(12.0, WINDOW_HEIGHT - 137.0, SIDEBAR_WIDTH - 24.0, 34.0),
            self,
            sel!(newChat:),
            mtm,
        );
        let new_chat_fill = fill_box(new_chat.frame(), &theme::accent_fill_strong(), 9.0, mtm);
        new_chat_fill.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        view.addSubview(&new_chat_fill);
        new_chat.setAccessibilityHelp(Some(ns_string!(
            "Start a new conversation in the active project. Command N."
        )));
        new_chat.setEnabled(false);
        new_chat.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        let project = navigation_button(
            "New project",
            "folder.badge.plus",
            rect(12.0, WINDOW_HEIGHT - 176.0, SIDEBAR_WIDTH - 24.0, 34.0),
            self,
            sel!(chooseProject:),
            mtm,
        );
        project.setAccessibilityHelp(Some(ns_string!(
            "Choose a folder to add a project."
        )));
        project.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);

        let search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            rect(12.0, WINDOW_HEIGHT - 219.0, SIDEBAR_WIDTH - 24.0, 30.0),
        );
        search.setPlaceholderString(Some(ns_string!("Search sessions")));
        search.setFont(Some(&theme::ui_font(theme::CONTROL_SIZE, false)));
        search.setSendsSearchStringImmediately(true);
        search.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        // SAFETY: AppDelegate implements filterSessions: and outlives the field.
        unsafe {
            search.setTarget(Some::<&AnyObject>(self));
            search.setAction(Some(sel!(filterSessions:)));
        }

        let source_filter = NSSegmentedControl::initWithFrame(
            NSSegmentedControl::alloc(mtm),
            rect(12.0, WINDOW_HEIGHT - 257.0, SIDEBAR_WIDTH - 24.0, 26.0),
        );
        source_filter.setSegmentCount(2);
        source_filter.setLabel_forSegment(ns_string!("Chats"), 0);
        source_filter.setLabel_forSegment(ns_string!("OMP archive"), 1);
        source_filter.setToolTip_forSegment(Some(ns_string!("Omperor sessions • resumable")), 0);
        source_filter.setToolTip_forSegment(Some(ns_string!("OMP sessions • always read-only")), 1);
        source_filter.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
        source_filter.setSegmentStyle(NSSegmentStyle::Rounded);
        source_filter.setSegmentDistribution(NSSegmentDistribution::FillEqually);
        source_filter.setSelectedSegment(0);
        source_filter.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        // SAFETY: AppDelegate implements filterSessionSource: and outlives the control.
        unsafe {
            source_filter.setTarget(Some::<&AnyObject>(self));
            source_filter.setAction(Some(sel!(filterSessionSource:)));
        }

        let recent = section_label("Projects", mtm);
        recent.setFrame(rect(18.0, WINDOW_HEIGHT - 289.0, 140.0, 18.0));
        recent.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        let status = text_label("Loading…", 10.0, false, mtm);
        status.setTextColor(Some(&theme::secondary_text()));
        status.setFrame(rect(166.0, WINDOW_HEIGHT - 289.0, 68.0, 18.0));
        status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);

        let sessions_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(8.0, 66.0, SIDEBAR_WIDTH - 16.0, WINDOW_HEIGHT - 355.0),
        );
        sessions_scroll.setBorderType(NSBorderType::NoBorder);
        sessions_scroll.setHasVerticalScroller(true);
        sessions_scroll.setAutohidesScrollers(true);
        sessions_scroll.setDrawsBackground(false);
        sessions_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let sessions_view = SessionListView::new(
            rect(0.0, 0.0, SIDEBAR_WIDTH - 16.0, WINDOW_HEIGHT - 355.0),
            mtm,
        );
        sessions_scroll.setDocumentView(Some(&sessions_view));
        // The rows are laid out against the clip view's height; when the
        // window is resized the document is re-fit, or a document sized for
        // a taller window would leave a scroller behind.
        let clip = sessions_scroll.contentView();
        clip.setPostsFrameChangedNotifications(true);
        // SAFETY: the delegate implements sidebarFrameDidChange: and, like the
        // clip view, lives for the process; the notification name is an
        // AppKit static.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(sidebarFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&clip),
            );
        }

        let account = navigation_button(
            "Connect OpenAI",
            "person.crop.circle",
            rect(12.0, 18.0, SIDEBAR_WIDTH - 66.0, 34.0),
            self,
            sel!(connectCurrentAccount:),
            mtm,
        );
        account.setEnabled(false);
        account.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);

        view.addSubview(&mark);
        view.addSubview(&brand);
        let commands = icon_button(
            "command",
            "Quick actions (⌘K)",
            rect(SIDEBAR_WIDTH - 45.0, WINDOW_HEIGHT - 83.0, 30.0, 30.0),
            self,
            sel!(showQuickActions:),
            mtm,
        );
        commands.setContentTintColor(Some(&theme::secondary_text()));
        commands.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        view.addSubview(&commands);
        view.addSubview(&new_chat);
        view.addSubview(&project);
        view.addSubview(&search);
        view.addSubview(&source_filter);
        view.addSubview(&recent);
        view.addSubview(&status);
        view.addSubview(&sessions_scroll);
        view.addSubview(&account);
        let settings = icon_button(
            "gearshape",
            "Settings (⌘,)",
            rect(SIDEBAR_WIDTH - 48.0, 18.0, 34.0, 34.0),
            self,
            sel!(showSettings:),
            mtm,
        );
        settings.setContentTintColor(Some(&theme::secondary_text()));
        view.addSubview(&settings);

        self.ivars()
            .new_chat_button
            .set(new_chat)
            .expect("new chat button once");
        self.ivars()
            .project_button
            .set(project)
            .expect("project button once");
        self.ivars()
            .connect_button
            .set(account)
            .expect("connect button once");
        self.ivars()
            .session_search
            .set(search)
            .expect("session search once");
        self.ivars()
            .session_source_filter
            .set(source_filter)
            .expect("session source filter once");
        self.ivars()
            .sidebar_sessions_view
            .set(sessions_view)
            .expect("session list once");
        self.ivars()
            .sidebar_sessions_scroll
            .set(sessions_scroll)
            .expect("session scroll once");
        self.ivars()
            .sidebar_status
            .set(status)
            .expect("sidebar status once");
        view
    }

    fn build_conversation_pane(
        &self,
        width: f64,
        mtm: MainThreadMarker,
    ) -> Retained<NSView> {
        let view = attachment_input::drop_view(mtm, rect(0.0, 0.0, width, WINDOW_HEIGHT));
        let canvas = fill_box(
            rect(0.0, 0.0, width, WINDOW_HEIGHT),
            &theme::canvas(),
            0.0,
            mtm,
        );
        canvas.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.addSubview(&canvas);
        let workbench_toggle = icon_button(
            "sidebar.right",
            "Show or hide workbench (⌘⇧I)",
            rect(width - 102.0, WINDOW_HEIGHT - 68.0, 80.0, 30.0),
            self,
            sel!(toggleWorkbench:),
            mtm,
        );
        workbench_toggle.setTitle(ns_string!("Focus"));
        workbench_toggle.setImagePosition(NSCellImagePosition::ImageLeading);
        workbench_toggle.setFont(Some(&theme::ui_font(12.0, true)));
        workbench_toggle.setImage(theme::row_symbol("sidebar.right", 13.0, 5.0).as_deref());
        workbench_toggle.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.addSubview(&workbench_toggle);
        self.ivars()
            .workbench_toggle
            .set(workbench_toggle)
            .expect("workbench toggle once");
        let usage_button = navigation_button("Usage", "chart.bar", rect(width - 196.0, WINDOW_HEIGHT - 68.0, 84.0, 30.0), self, sel!(showUsage:), mtm);
        usage_button.setToolTip(Some(ns_string!("Token usage, provider cache policy, and cache-change warnings")));
        usage_button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin);
        view.addSubview(&usage_button);
        self.ivars().usage_button.set(usage_button).expect("usage button once");
        self.ivars().usage_header.set(usage_ui::UsageHeader::new(&view, mtm)).expect("usage header once");
        let title = text_label("New agent session", 15.0, true, mtm);
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        title.setFont(Some(&NSFont::systemFontOfSize_weight(20.0, unsafe {
            NSFontWeightSemibold
        })));
        // The title stops short of the speedometer at the header's right.
        title.setFrame(rect(
            28.0,
            WINDOW_HEIGHT - 63.0,
            width - 56.0 - SPEEDOMETER_WIDTH - 12.0,
            24.0,
        ));
        title.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        // Tokens a second, at the header's right: the dial on the title row
        // with the rate beside it, and the response's other figures under.
        // The block ends a gap short of the Focus toggle (at `width - 102`);
        // `layout_header` refits it to its text once the pane is laid out.
        let speedometer_x = width - 112.0 - SPEEDOMETER_WIDTH;
        let speedometer =
            SpeedometerView::new(rect(speedometer_x, WINDOW_HEIGHT - 66.0, 36.0, 24.0), mtm);
        speedometer.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        speedometer.setToolTip(Some(ns_string!(
            "Output tokens a second: the provider's own count of output tokens (reasoning included) over the time from the first streamed token to the last, as the API benchmarks measure it. While a response streams the needle follows an estimate; it settles on the exact figure when OMP's receipt arrives."
        )));
        let speedometer_rate = text_label("— tok/s", 12.0, true, mtm);
        speedometer_rate.setTextColor(Some(&theme::secondary_text()));
        speedometer_rate.setAlignment(NSTextAlignment::Left);
        speedometer_rate.setFrame(rect(
            speedometer_x + 42.0,
            WINDOW_HEIGHT - 64.0,
            SPEEDOMETER_WIDTH - 42.0,
            18.0,
        ));
        speedometer_rate.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let speedometer_detail = text_label("", 10.0, false, mtm);
        speedometer_detail.setTextColor(Some(&theme::secondary_text()));
        speedometer_detail.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        speedometer_detail.setFrame(rect(
            speedometer_x + 42.0,
            WINDOW_HEIGHT - 80.0,
            SPEEDOMETER_WIDTH - 42.0,
            15.0,
        ));
        speedometer_detail.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        // A hairline closes the header off from the transcript.
        let header_rule = fill_box(
            rect(28.0, WINDOW_HEIGHT - 114.0, width - 56.0, 1.0),
            &theme::hairline(),
            0.0,
            mtm,
        );
        header_rule.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        // The path line is one row of crumbs: the project and its path, then
        // the session, all on one centre line; `layout_header` places them.
        let path = text_label("Opening workspace…", 11.0, false, mtm);
        path.setTextColor(Some(&theme::secondary_text()));
        path.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
        path.setFrame(rect(28.0, WINDOW_HEIGHT - 83.0, width - 56.0, 18.0));
        path.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        // The session crumb is a button: one click puts the id on the
        // pasteboard, and the menu behind it reaches the journal. Hidden
        // until there is a owner.
        // SAFETY: the delegate implements copySessionId: and outlives the header.
        let chip = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!(""),
                Some::<&AnyObject>(self),
                Some(sel!(copySessionId:)),
                mtm,
            )
        };
        chip.setBordered(false);
        chip.setAlignment(NSTextAlignment::Left);
        chip.setImagePosition(NSCellImagePosition::ImageTrailing);
        chip.setContentTintColor(Some(&theme::secondary_text()));
        chip.setToolTip(Some(ns_string!(
            "Copy the session ID · right-click for the journal"
        )));
        chip.setFrame(rect(28.0, WINDOW_HEIGHT - 84.0, 10.0, 20.0));
        chip.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        chip.setHidden(true);
        // SAFETY: the menu's items target the delegate, which outlives the header.
        unsafe { chip.setMenu(Some(&self.session_chip_menu())) };
        let status = text_label("Starting the OMP inference harness…", 11.0, false, mtm);
        status.setTextColor(Some(&theme::secondary_text()));
        // A flash on the line fades back to what it was saying.
        status.setWantsLayer(true);
        status.setFrame(rect(28.0, WINDOW_HEIGHT - 105.0, width - 84.0, 18.0));
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        // Activity stays beside the input, in the footer below the composer.
        let busy_spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm), rect(28.0, 15.0, 12.0, 12.0),
        );
        busy_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        busy_spinner.setIndeterminate(true);
        busy_spinner.setControlSize(NSControlSize::Mini);
        busy_spinner.setDisplayedWhenStopped(false);
        busy_spinner.setHidden(true);
        let busy_label = text_label("Working…", 11.0, false, mtm);
        busy_label.setTextColor(Some(&theme::secondary_text()));
        busy_label.setHidden(true);
        busy_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        // The transcript's text edge lines up with the header and the rule
        // above it; the rows carry their own leading marks.
        let transcript = transcript_stack::Transcript::new(rect(23.0, 218.0, width - 46.0, WINDOW_HEIGHT - 340.0), mtm);
        let transcript_scroll = transcript.scroll_view().clone();
        // Let the chat container receive attachment drops instead of the
        // read-only rows attempting to import graphics into the transcript.
        transcript_scroll.unregisterDraggedTypes();
        // The measure follows the pane: the clip view reports its resizes.
        let clip = transcript_scroll.contentView();
        clip.setPostsFrameChangedNotifications(true);
        // SAFETY: the delegate implements transcriptFrameDidChange: and, like
        // the clip view, lives for the process, so the observer is never
        // removed; the notification name is an AppKit static.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(transcriptFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&clip),
            );
        }
        // Activity rows' detail links come here, as the NSTextViewDelegate;
        // the Markdown rows' through the transcript's events. The delegate
        // is unretained by AppKit and AppDelegate outlives the views.
        transcript.set_text_delegate(ProtocolObject::from_retained(self.retain()));
        let this = self.retain();
        transcript.set_event_handler(move |event| match event {
            transcript_stack::TranscriptEvent::OpenDetail(id) => this.present_activity_detail(id),
        });

        // The composer floats over the page: rounder than a settings card,
        // lifted on a soft plum-black shadow.
        let composer = card_box(
            rect(24.0, 40.0, width - 48.0, layout::COMPOSER_HEIGHT),
            22.0,
            mtm,
        );
        float_card(&composer);
        composer.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        // Chips behind the control labels; each is refit to its text.
        let model_chip = fill_box(
            rect(0.0, 0.0, 10.0, 22.0),
            &theme::surface_inset(),
            7.0,
            mtm,
        );
        model_chip.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        let thinking_chip = fill_box(
            rect(0.0, 0.0, 10.0, 22.0),
            &theme::surface_inset(),
            7.0,
            mtm,
        );
        thinking_chip.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        let account_chip = fill_box(
            rect(0.0, 0.0, 10.0, 22.0),
            &theme::surface_inset(),
            7.0,
            mtm,
        );
        account_chip.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        account_chip.setHidden(true);

        let input = composer_input_field(mtm);
        input.setFrame(rect(44.0, 121.0, width - 88.0, 56.0));
        // SAFETY: AppDelegate implements sendMessage: and outlives the field.
        unsafe {
            input.setTarget(Some::<&AnyObject>(self));
            input.setAction(Some(sel!(sendMessage:)));
        }
        // The field editor sends on Return but grows a new line on
        // Shift+Return; see the NSControlTextEditingDelegate impl.
        // SAFETY: the delegate is unretained and AppDelegate outlives the field.
        unsafe { input.setDelegate(Some(ProtocolObject::from_ref(self))) };
        input.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );

        let model_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(38.0, 32.0, 248.0, 28.0),
            false,
        );
        model_picker.addItemWithTitle(ns_string!("Loading models…"));
        model_picker.setAccessibilityLabel(Some(ns_string!("Model")));
        model_picker.setTransparent(true);
        model_picker.setEnabled(false);
        model_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        // SAFETY: AppDelegate implements selectModel: and outlives the picker.
        unsafe {
            model_picker.setTarget(Some::<&AnyObject>(self));
            model_picker.setAction(Some(sel!(selectModel:)));
        }
        let model_label = composer_control_label("Loading models…  ▾", mtm);
        model_label.setTextColor(Some(&theme::secondary_text()));
        model_label.setFrame(rect(46.0, 37.0, 232.0, 18.0));
        model_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);

        let thinking_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(296.0, 32.0, 118.0, 28.0),
            false,
        );
        for level in ["off", "minimal", "low", "medium", "high", "xhigh"] {
            thinking_picker.addItemWithTitle(&NSString::from_str(level));
        }
        thinking_picker.selectItemWithTitle(ns_string!("medium"));
        thinking_picker.setAccessibilityLabel(Some(ns_string!("Reasoning effort")));
        thinking_picker.setTransparent(true);
        thinking_picker.setEnabled(false);
        thinking_picker.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        // SAFETY: AppDelegate implements selectThinkingLevel: and outlives the picker.
        unsafe {
            thinking_picker.setTarget(Some::<&AnyObject>(self));
            thinking_picker.setAction(Some(sel!(selectThinkingLevel:)));
        }
        let thinking_label = composer_control_label("Reasoning: medium  ▾", mtm);
        thinking_label.setTextColor(Some(&theme::secondary_text()));
        thinking_label.setFrame(rect(304.0, 37.0, 110.0, 18.0));
        thinking_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);

        // Fast: a chip with a bolt that reads as a switch. It stays hidden
        // until the model in view is one the backend offers the tier on.
        let fast_chip = fill_box(
            rect(0.0, 0.0, 10.0, 22.0),
            &theme::surface_inset(),
            7.0,
            mtm,
        );
        fast_chip.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        fast_chip.setHidden(true);
        let fast_icon =
            NSImageView::initWithFrame(NSImageView::alloc(mtm), rect(0.0, 0.0, 11.0, 11.0));
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        fast_icon.setImage(
            theme::symbol("bolt.fill", 10.0, unsafe { NSFontWeightSemibold }, None).as_deref(),
        );
        fast_icon.setContentTintColor(Some(&theme::secondary_text()));
        fast_icon.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        fast_icon.setHidden(true);
        let fast_label = text_label("Fast", theme::CONTROL_SIZE, false, mtm);
        fast_label.setTextColor(Some(&theme::secondary_text()));
        fast_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        fast_label.setHidden(true);
        // SAFETY: AppDelegate implements toggleFastMode: and outlives the button.
        let fast_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!(""),
                Some::<&AnyObject>(self),
                Some(sel!(toggleFastMode:)),
                mtm,
            )
        };
        fast_button.setAccessibilityLabel(Some(ns_string!("Fast mode")));
        fast_button.setBordered(false);
        fast_button.setTransparent(true);
        fast_button.setEnabled(false);
        fast_button.setHidden(true);
        fast_button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);

        // The account control occupies the lower row beside Fast. The shared
        // composer layout sets its frame after all controls are installed.
        let account_picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(0.0, 0.0, 1.0, layout::CHIP_HEIGHT),
            false,
        );
        account_picker.setAccessibilityLabel(Some(ns_string!("Account")));
        account_picker.setTransparent(true);
        account_picker.setEnabled(false);
        account_picker.setHidden(true);
        account_picker.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        // SAFETY: AppDelegate implements selectAccount: and outlives the picker.
        unsafe {
            account_picker.setTarget(Some::<&AnyObject>(self));
            account_picker.setAction(Some(sel!(selectAccount:)));
        }
        let account_label = text_label("", theme::CONTROL_SIZE, false, mtm);
        account_label.setTextColor(Some(&theme::secondary_text()));
        account_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        account_label.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        account_label.setHidden(true);

        let send_button = icon_button(
            "arrow.up.circle.fill",
            "Send",
            rect(width - 74.0, 27.0, 38.0, 38.0),
            self,
            sel!(sendMessage:),
            mtm,
        );
        send_button.setContentTintColor(Some(&theme::accent()));
        send_button.setKeyEquivalent(ns_string!("\r"));
        send_button.setKeyEquivalentModifierMask(NSEventModifierFlags::empty());
        send_button.setEnabled(false);
        send_button.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );

        let cache_warning = icon_button("clock", "KV cache timer", rect(0.0, 0.0, layout::CACHE_BADGE_WIDTH, 28.0), self, sel!(showCacheWarning:), mtm);
        cache_warning.setImagePosition(NSCellImagePosition::ImageLeading);
        cache_warning.setTitle(ns_string!("KV —"));
        cache_warning.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(11.0, 0.0)));
        cache_warning.setContentTintColor(Some(&theme::secondary_text()));
        cache_warning.setHidden(true);
        let attach_button = icon_button("paperclip", "Attach files", rect(width - 114.0, 29.0, 34.0, 34.0), self, sel!(attachFiles:), mtm);
        attach_button.setToolTip(Some(ns_string!("Attach images, documents, or other files")));
        let stop_button = icon_button(
            "stop.circle",
            "Stop",
            rect(width - 114.0, 29.0, 34.0, 34.0),
            self,
            sel!(stopOmp:),
            mtm,
        );
        stop_button.setEnabled(false);
        stop_button.setHidden(true);
        stop_button.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );

        // These frames are owned by layout_header/refit_composer_chips.
        // Autoresizing them as well would apply the resize delta twice when
        // the transcript's clip-view notification fires during parent layout.
        for item in [
            &*title as &NSView,
            &*path,
            &*chip,
            &*composer,
            &*input,
            &*model_chip,
            &*thinking_chip,
            &*fast_chip,
            &*account_chip,
            &*model_label,
            &*thinking_label,
            &*fast_label,
            &*account_label,
            &*fast_icon,
            &*model_picker,
            &*thinking_picker,
            &*account_picker,
            &*fast_button,
            &*send_button,
            &*stop_button,
            &*attach_button,
            &*cache_warning,
        ] {
            item.setAutoresizingMask(NSAutoresizingMaskOptions::empty());
        }
        view.addSubview(&title);
        view.addSubview(&path);
        view.addSubview(&chip);
        view.addSubview(&status);
        view.addSubview(&header_rule);
        view.addSubview(&busy_spinner);
        view.addSubview(&busy_label);
        view.addSubview(&speedometer);
        view.addSubview(&speedometer_rate);
        view.addSubview(&speedometer_detail);
        self.ivars()
            .speedometer
            .set(speedometer)
            .expect("speedometer once");
        self.ivars()
            .speedometer_rate
            .set(speedometer_rate)
            .expect("speedometer rate once");
        self.ivars()
            .speedometer_detail
            .set(speedometer_detail)
            .expect("speedometer detail once");
        self.ivars().busy_spinner.set(busy_spinner).expect("busy spinner once");
        self.ivars().busy_label.set(busy_label).expect("busy label once");
        view.addSubview(&transcript_scroll);
        view.addSubview(&composer);
        self.ivars()
            .composer_surface
            .set(composer)
            .expect("composer surface once");
        let hint = text_label(
            "Return to send   ·   Shift Return for a new line   ·   @ to add a file",
            theme::CAPTION_SIZE,
            false,
            mtm,
        );
        hint.setTextColor(Some(&theme::secondary_text()));
        hint.setAlignment(NSTextAlignment::Center);
        hint.setFrame(rect(24.0, 13.0, width - 48.0, 18.0));
        hint.setAutoresizingMask(NSAutoresizingMaskOptions::empty());
        view.addSubview(&hint);
        self.ivars()
            .composer_hint
            .set(hint)
            .expect("composer hint once");
        view.addSubview(&input);
        view.addSubview(&model_chip);
        view.addSubview(&thinking_chip);
        view.addSubview(&fast_chip);
        view.addSubview(&account_chip);
        view.addSubview(&model_label);
        view.addSubview(&thinking_label);
        view.addSubview(&fast_icon);
        view.addSubview(&fast_label);
        view.addSubview(&account_label);
        self.ivars()
            .fast_chip
            .set(fast_chip)
            .expect("fast chip should only be initialized once");
        self.ivars()
            .fast_icon
            .set(fast_icon)
            .expect("fast icon should only be initialized once");
        self.ivars()
            .fast_label
            .set(fast_label)
            .expect("fast label should only be initialized once");
        self.ivars()
            .model_chip
            .set(model_chip)
            .expect("model chip should only be initialized once");
        self.ivars()
            .thinking_chip
            .set(thinking_chip)
            .expect("thinking chip should only be initialized once");
        self.ivars()
            .account_chip
            .set(account_chip)
            .expect("account chip should only be initialized once");
        view.addSubview(&model_picker);
        view.addSubview(&thinking_picker);
        view.addSubview(&fast_button);
        self.ivars()
            .fast_button
            .set(fast_button)
            .expect("fast button should only be initialized once");
        view.addSubview(&account_picker);
        view.addSubview(&stop_button);
        view.addSubview(&attach_button);
        view.addSubview(&cache_warning);
        self.ivars().cache_warning_badge.set(cache_warning).expect("cache warning badge once");
        self.ivars().attach_button.set(attach_button).expect("attachment button initialized once");
        view.addSubview(&send_button);
        self.ivars()
            .account_picker
            .set(account_picker)
            .expect("account picker should only be initialized once");
        self.ivars()
            .account_label
            .set(account_label)
            .expect("account label should only be initialized once");
        self.ivars()
            .chat_transcript
            .set(transcript)
            .expect("chat transcript should only be initialized once");
        self.ivars()
            .chat_status
            .set(status)
            .expect("chat status should only be initialized once");
        self.ivars()
            .center_title
            .set(title)
            .expect("center title once");
        self.ivars()
            .workspace_path_label
            .set(path)
            .expect("workspace path once");
        self.ivars()
            .session_chip
            .set(chip)
            .expect("session chip once");
        self.layout_header();
        self.ivars()
            .input
            .set(input)
            .expect("input should only be initialized once");
        // The `/` list is ready before the first key asks for it.
        self.prepare_slash_picker();
        self.ivars()
            .send_button
            .set(send_button)
            .expect("send button should only be initialized once");
        self.ivars()
            .stop_button
            .set(stop_button)
            .expect("stop button should only be initialized once");
        self.ivars()
            .model_picker
            .set(model_picker)
            .expect("model picker should only be initialized once");
        self.ivars()
            .model_label
            .set(model_label)
            .expect("model label should only be initialized once");
        self.ivars()
            .thinking_picker
            .set(thinking_picker)
            .expect("thinking picker should only be initialized once");
        self.ivars()
            .thinking_label
            .set(thinking_label)
            .expect("thinking label should only be initialized once");
        self.install_session_workspace(&view);
        let banner = account_ui::Banner::new(self);
        view.addSubview(&banner.view);
        let _ = self.ivars().account_banner.set(banner);
        // The frame budget's readout sits above the composer, on top of the
        // transcript, and is shown only while Developer mode asks for it.
        let overlay = frame_overlay::Overlay::new(width, mtm);
        view.addSubview(&overlay.view);
        let _ = self.ivars().frame_overlay.set(overlay);

        view
    }

    fn build_inspector(&self, mtm: MainThreadMarker) -> Retained<NSVisualEffectView> {
        let view = NSVisualEffectView::initWithFrame(
            NSVisualEffectView::alloc(mtm),
            rect(0.0, 0.0, INSPECTOR_WIDTH, WINDOW_HEIGHT),
        );
        view.setMaterial(NSVisualEffectMaterial::Sidebar);
        view.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        // A quiet neutral surface distinguishes the workbench from the canvas.
        let wash = fill_box(
            rect(0.0, 0.0, INSPECTOR_WIDTH, WINDOW_HEIGHT),
            &theme::inspector(),
            0.0,
            mtm,
        );
        wash.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.addSubview(&wash);

        // Each panel fills the pane and the switcher shows one. Both are
        // added before the header controls: a later subview is hit-tested
        // first, and a transparent full-height panel over the header would
        // swallow every click meant for the switcher and its buttons.
        let changes = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, INSPECTOR_WIDTH, WINDOW_HEIGHT),
        );
        changes.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.addSubview(&changes);
        let agents = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, INSPECTOR_WIDTH, WINDOW_HEIGHT),
        );
        agents.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        agents.setHidden(true);
        view.addSubview(&agents);

        let location = text_label("", 12.0, false, mtm);
        location.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            12.0,
            unsafe { NSFontWeightRegular },
        )));
        location.setTextColor(Some(&theme::secondary_text()));
        location.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
        location.setFrame(rect(
            18.0,
            WINDOW_HEIGHT - 105.0,
            INSPECTOR_WIDTH - 36.0,
            20.0,
        ));
        location.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.addSubview(&location);
        self.ivars()
            .workbench_location
            .set(location)
            .expect("workspace location once");
        let expand = icon_button(
            "arrow.left.and.right",
            "Expand agent panel",
            rect(INSPECTOR_WIDTH - 42.0, WINDOW_HEIGHT - 77.0, 28.0, 28.0),
            self,
            sel!(expandWorkbench:),
            mtm,
        );
        expand.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.addSubview(&expand);
        self.ivars()
            .workbench_expand
            .set(expand)
            .expect("workspace expand once");
        // The header: the Changes | Agents switcher on the left, and on the
        // right the actions that hand the project to the tools the user
        // already has — the editor, a terminal, the Finder — plus refresh.
        let switcher = NSSegmentedControl::initWithFrame(
            NSSegmentedControl::alloc(mtm),
            rect(16.0, WINDOW_HEIGHT - 78.0, INSPECTOR_WIDTH - 68.0, 30.0),
        );
        switcher.setSegmentCount(2);
        switcher.setLabel_forSegment(ns_string!("Changes"), 0);
        switcher.setLabel_forSegment(ns_string!("Agents"), 1);
        switcher.setToolTip_forSegment(
            Some(ns_string!("Git status and the working tree's changes")),
            0,
        );
        switcher.setToolTip_forSegment(
            Some(ns_string!(
                "This session’s primary agent and delegated tasks"
            )),
            1,
        );
        switcher.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
        switcher.setSegmentStyle(NSSegmentStyle::Rounded);
        switcher.setSegmentDistribution(NSSegmentDistribution::FillEqually);
        switcher.setSelectedSegment(0);
        switcher.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        // SAFETY: AppDelegate implements switchWorkbenchPanel: and outlives the control.
        unsafe {
            switcher.setTarget(Some::<&AnyObject>(self));
            switcher.setAction(Some(sel!(switchWorkbenchPanel:)));
        }
        view.addSubview(&switcher);
        let status = text_label("Inspecting workspace…", 10.5, false, mtm);
        status.setTextColor(Some(&theme::secondary_text()));
        status.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
        status.setFrame(rect(
            18.0,
            WINDOW_HEIGHT - 137.0,
            INSPECTOR_WIDTH - 36.0,
            16.0,
        ));
        status.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        changes.addSubview(&status);
        let header_buttons: [(&str, &str, objc2::runtime::Sel, f64); 4] = [
            ("arrow.clockwise", "Refresh", sel!(refreshWorkspace:), 44.0),
            (
                "folder",
                "Reveal the project in the Finder",
                sel!(revealProject:),
                72.0,
            ),
            (
                "terminal",
                "Open a terminal in the project",
                sel!(openProjectInTerminal:),
                100.0,
            ),
            (
                "chevron.left.forwardslash.chevron.right",
                "Open the project in the editor",
                sel!(openProjectInEditor:),
                128.0,
            ),
        ];
        for (symbol, label, action, offset) in header_buttons {
            let button = icon_button(
                symbol,
                label,
                rect(INSPECTOR_WIDTH - offset, 18.0, 26.0, 26.0),
                self,
                action,
                mtm,
            );
            button.setContentTintColor(Some(&theme::secondary_text()));
            button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            if cfg!(feature = "design-preview") {
                button.setEnabled(false);
                button.setToolTip(Some(ns_string!("Available in a connected workspace")));
            }
            view.addSubview(&button);
            if action == sel!(openProjectInEditor:) {
                self.ivars()
                    .open_editor_button
                    .set(button)
                    .expect("editor button once");
            } else if action == sel!(openProjectInTerminal:) {
                self.ivars()
                    .open_terminal_button
                    .set(button)
                    .expect("terminal button once");
            }
        }

        // Three summary rows — branch, upstream, last commit — each a symbol,
        // a quiet label, and a value on the right, the way a status panel
        // reads at a glance.
        let rows: [(&str, &str, &OnceCell<Retained<NSTextField>>); 3] = [
            (
                "arrow.triangle.branch",
                "Branch",
                &self.ivars().inspector_branch,
            ),
            (
                "arrow.up.arrow.down",
                "Upstream",
                &self.ivars().inspector_upstream,
            ),
            ("clock", "Last commit", &self.ivars().inspector_commit),
        ];
        for (index, (symbol, label, slot)) in rows.into_iter().enumerate() {
            let y = WINDOW_HEIGHT - 168.0 - INSPECTOR_ROW_HEIGHT * index as f64;
            let value = inspector_row(&changes, y, symbol, label, mtm);
            slot.set(value).expect("inspector row set once");
        }
        let rule = fill_box(
            rect(18.0, WINDOW_HEIGHT - 236.0, INSPECTOR_WIDTH - 36.0, 1.0),
            &theme::hairline(),
            0.0,
            mtm,
        );
        rule.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        changes.addSubview(&rule);

        // The change list: a section label with the totals on its right,
        // then one row per changed file.
        let section = section_label("Changes", mtm);
        section.setFrame(rect(18.0, WINDOW_HEIGHT - 262.0, 160.0, 16.0));
        section.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        let totals = text_label("", 11.0, false, mtm);
        totals.setAlignment(NSTextAlignment::Right);
        totals.setFrame(rect(
            INSPECTOR_WIDTH - 178.0,
            WINDOW_HEIGHT - 263.0,
            160.0,
            18.0,
        ));
        totals.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        changes.addSubview(&section);
        changes.addSubview(&totals);
        let list_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(12.0, WINDOW_HEIGHT - 302.0, INSPECTOR_WIDTH - 24.0, 32.0),
        );
        list_scroll.setBorderType(NSBorderType::NoBorder);
        list_scroll.setHasVerticalScroller(true);
        list_scroll.setAutohidesScrollers(true);
        list_scroll.setDrawsBackground(false);
        list_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let list_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, INSPECTOR_WIDTH - 24.0, 32.0),
        );
        list_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        list_scroll.setDocumentView(Some(&list_view));
        changes.addSubview(&list_scroll);

        // The diff of the selected file, under its own title.
        let diff_title = text_label("", 12.0, true, mtm);
        diff_title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
        diff_title.setFrame(rect(
            18.0,
            WINDOW_HEIGHT - 336.0,
            INSPECTOR_WIDTH - 36.0,
            18.0,
        ));
        diff_title.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        changes.addSubview(&diff_title);
        let (diff_scroll, transcript) = transcript_control(
            rect(12.0, 64.0, INSPECTOR_WIDTH - 24.0, WINDOW_HEIGHT - 416.0),
            mtm,
        );
        // Prose (the placeholder, "no changes") reads in the system face; a
        // diff brings its own monospaced attributes when rendered.
        transcript.setFont(Some(&theme::ui_font(theme::CONTROL_SIZE, false)));
        transcript.setTextColor(Some(&theme::secondary_text()));
        transcript.setString(ns_string!("Select a changed file to read its diff."));
        // A diff never wraps: long lines scroll sideways rather than folding,
        // which would break the alignment a diff is read by.
        diff_scroll.setHasHorizontalScroller(true);
        transcript.setHorizontallyResizable(true);
        transcript.setMaxSize(NSSize::new(1.0e7, 1.0e7));
        // SAFETY: NSTextView owns its text container for its lifetime.
        if let Some(container) = unsafe { transcript.textContainer() } {
            container.setWidthTracksTextView(false);
            container.setContainerSize(NSSize::new(1.0e7, 1.0e7));
        }
        changes.addSubview(&diff_scroll);

        // The session's activity is a connected, progressively filtered tree.
        let agents_summary = text_label("", 12.0, false, mtm);
        agents_summary.setTextColor(Some(&theme::secondary_text()));
        agents_summary.setFrame(rect(
            18.0,
            WINDOW_HEIGHT - 143.0,
            INSPECTOR_WIDTH - 36.0,
            20.0,
        ));
        agents_summary.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        agents.addSubview(&agents_summary);
        let scope = NSSegmentedControl::initWithFrame(
            NSSegmentedControl::alloc(mtm),
            rect(16.0, WINDOW_HEIGHT - 179.0, INSPECTOR_WIDTH - 98.0, 28.0),
        );
        scope.setSegmentCount(3);
        for (i, title) in ["All", "Active", "Attention"].iter().enumerate() {
            scope.setLabel_forSegment(&NSString::from_str(title), i as isize);
        }
        scope.setFont(Some(&theme::ui_font(12.0, false)));
        scope.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
        scope.setSegmentStyle(NSSegmentStyle::Rounded);
        scope.setSegmentDistribution(NSSegmentDistribution::FillEqually);
        scope.setSelectedSegment(0);
        scope.setAccessibilityLabel(Some(ns_string!("Filter this session’s agents")));
        scope.setToolTip_forSegment(
            Some(ns_string!(
                "Failed, interrupted, or completed with a warning"
            )),
            2,
        );
        scope.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        unsafe {
            scope.setTarget(Some::<&AnyObject>(self));
            scope.setAction(Some(sel!(selectAgentScope:)));
        }
        agents.addSubview(&scope);
        self.ivars()
            .agents_scope
            .set(scope)
            .expect("agent scope once");
        let density = icon_button(
            "line.3.horizontal.decrease",
            "Use compact rows",
            rect(INSPECTOR_WIDTH - 74.0, WINDOW_HEIGHT - 179.0, 28.0, 28.0),
            self,
            sel!(toggleAgentDensity:),
            mtm,
        );
        density.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        agents.addSubview(&density);
        self.ivars()
            .agents_density
            .set(density)
            .expect("agent density once");
        let search_button = icon_button(
            "magnifyingglass",
            "Search agents",
            rect(INSPECTOR_WIDTH - 42.0, WINDOW_HEIGHT - 179.0, 28.0, 28.0),
            self,
            sel!(toggleAgentSearch:),
            mtm,
        );
        search_button.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        agents.addSubview(&search_button);
        self.ivars()
            .agents_search_button
            .set(search_button)
            .expect("agent search button once");
        let search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            rect(16.0, WINDOW_HEIGHT - 215.0, INSPECTOR_WIDTH - 32.0, 28.0),
        );
        search.setPlaceholderString(Some(ns_string!("Find a task, role, or error…")));
        search.setAccessibilityLabel(Some(ns_string!("Search this session’s agents")));
        search.setFont(Some(&theme::ui_font(13.0, false)));
        search.setTag(agent_workspace::SEARCH_TAG);
        search.setSendsSearchStringImmediately(true);
        search.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        unsafe {
            search.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        search.setHidden(true);
        agents.addSubview(&search);
        self.ivars()
            .agents_search
            .set(search)
            .expect("agent search once");
        let agents_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(12.0, 64.0, INSPECTOR_WIDTH - 24.0, WINDOW_HEIGHT - 256.0),
        );
        agents_scroll.setBorderType(NSBorderType::NoBorder);
        agents_scroll.setHasVerticalScroller(true);
        agents_scroll.setAutohidesScrollers(true);
        agents_scroll.setDrawsBackground(false);
        agents_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let agents_view = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, INSPECTOR_WIDTH - 24.0, WINDOW_HEIGHT - 256.0),
        );
        agents_scroll.setDocumentView(Some(&agents_view));
        agents.addSubview(&agents_scroll);
        self.ivars()
            .agent_inspection
            .set(agent_inspection::AgentInspection::new(&agents, self, mtm))
            .expect("agent inspection once");
        agents.setPostsFrameChangedNotifications(true);
        // SAFETY: the delegate and observed view both live for the app lifetime.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(agentWorkspaceFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&agents),
            );
        }

        let footer = text_label("This Mac", 12.0, false, mtm);
        footer.setTextColor(Some(&theme::secondary_text()));
        footer.setFrame(rect(20.0, 22.0, 110.0, 18.0));
        view.addSubview(&footer);
        let footer_rule = fill_box(
            rect(16.0, 56.0, INSPECTOR_WIDTH - 32.0, 1.0),
            &theme::hairline(),
            0.0,
            mtm,
        );
        footer_rule.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        view.addSubview(&footer_rule);
        let ivars = self.ivars();
        ivars
            .workbench_switcher
            .set(switcher)
            .expect("workbench switcher once");
        ivars
            .changes_panel
            .set(changes)
            .expect("changes panel once");
        ivars.agents_panel.set(agents).expect("agents panel once");
        ivars
            .agents_scroll
            .set(agents_scroll)
            .expect("agents scroll once");
        ivars
            .agents_view
            .set(agents_view)
            .expect("agents view once");
        ivars
            .agents_summary
            .set(agents_summary)
            .expect("agents summary once");
        ivars
            .change_transcript
            .set(transcript)
            .expect("change transcript should only be initialized once");
        ivars
            .change_status
            .set(status)
            .expect("change status should only be initialized once");
        ivars
            .change_list_scroll
            .set(list_scroll)
            .expect("change list scroll once");
        ivars
            .change_list_view
            .set(list_view)
            .expect("change list view once");
        ivars
            .change_section_label
            .set(section)
            .expect("change section label once");
        ivars
            .change_section_totals
            .set(totals)
            .expect("change totals once");
        ivars.diff_title.set(diff_title).expect("diff title once");
        ivars
            .diff_scroll
            .set(diff_scroll)
            .expect("diff scroll once");
        ivars
            .inspector_view
            .set(view.clone())
            .expect("inspector view once");
        view
    }

    /// Fills the change list as a tree — directories first, collapsible,
    /// each with the files it holds and their line counts — or a single note
    /// when there is nothing to list, then sizes the list and the diff below
    /// it to share the pane: the list takes what its rows need up to two
    /// fifths of the space, the diff takes the rest.
    fn populate_change_list(&self, changes: &[workspace::WorkspaceChange], note: Option<&str>) {
        let owner = self.session();
        let mtm = self.mtm();
        let ivars = self.ivars();
        let (Some(container), Some(list_scroll), Some(diff_title), Some(diff_scroll), Some(pane)) = (
            ivars.change_list_view.get(),
            ivars.change_list_scroll.get(),
            ivars.diff_title.get(),
            ivars.diff_scroll.get(),
            ivars.inspector_view.get(),
        ) else {
            return;
        };
        for row in ivars.change_rows.borrow_mut().drain(..) {
            row.removeFromSuperview();
        }
        if let Some(pill) = ivars.change_selection_pill.borrow_mut().take() {
            pill.removeFromSuperview();
        }
        let collapsed = owner.change_tree_collapsed.borrow().clone();
        let tree = change_tree::tree_rows(changes, &collapsed);
        let pane_height = pane.bounds().size.height;
        let width = list_scroll.frame().size.width;
        let list_top = pane_height - 270.0;
        let rows = tree.len().max(usize::from(note.is_some()));
        let needed = rows as f64 * INSPECTOR_ROW_HEIGHT + 6.0;
        let room = (list_top - 64.0 - 40.0).max(INSPECTOR_ROW_HEIGHT);
        let list_height = needed.min(room * 0.42).max(INSPECTOR_ROW_HEIGHT + 6.0);
        let document_height = needed.max(list_height);
        container.setFrameSize(NSSize::new(width, document_height));
        list_scroll.setFrame(rect(12.0, list_top - list_height, width, list_height));

        let mut views: Vec<Retained<NSView>> = Vec::new();
        if let Some(note) = note {
            let label = text_label(note, 12.0, false, mtm);
            label.setTextColor(Some(&theme::secondary_text()));
            label.setFrame(rect(
                8.0,
                document_height - INSPECTOR_ROW_HEIGHT - 3.0,
                width - 16.0,
                INSPECTOR_ROW_HEIGHT,
            ));
            label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            container.addSubview(&label);
            views.push(label.into_super().into_super());
        }
        let counts_width = 74.0;
        // The context menus name the editor and terminal; resolved once here,
        // not per row, since each lookup stats the Applications folders.
        let editor_name = self.launcher_name(launchers::LauncherKind::Editor);
        let terminal_name = self.launcher_name(launchers::LauncherKind::Terminal);
        for (row_index, row) in tree.iter().enumerate() {
            let y = document_height - INSPECTOR_ROW_HEIGHT * (row_index as f64 + 1.0) - 3.0;
            let (button, insertions, deletions, depth, tooltip) = match row {
                change_tree::TreeRow::Directory {
                    path,
                    name,
                    depth,
                    collapsed,
                    files,
                    insertions,
                    deletions,
                } => (
                    directory_row_button(name, *collapsed, *files, row_index, self, mtm),
                    *insertions,
                    *deletions,
                    *depth,
                    path.display().to_string(),
                ),
                change_tree::TreeRow::File {
                    change,
                    name,
                    depth,
                } => {
                    let change = &changes[*change];
                    (
                        change_row_button(change, name, row_index, self, mtm),
                        change.insertions,
                        change.deletions,
                        *depth,
                        change.label(),
                    )
                }
            };
            let indent = depth as f64 * 14.0;
            button.setFrame(rect(
                6.0 + indent,
                y,
                width - 12.0 - counts_width - indent,
                INSPECTOR_ROW_HEIGHT,
            ));
            button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            button.setToolTip(Some(&NSString::from_str(&tooltip)));
            // SAFETY: the row retains its menu and every item targets the
            // delegate, which outlives the pane.
            unsafe {
                button.setMenu(Some(&self.change_row_menu(
                    row_index,
                    &editor_name,
                    &terminal_name,
                )))
            };
            container.addSubview(&button);
            let counts = text_label("", 11.0, false, mtm);
            counts.setAlignment(NSTextAlignment::Right);
            counts.setAttributedStringValue(&aligned_right(&change_counts_attributed(
                insertions, deletions, 11.0,
            )));
            counts.setFrame(rect(
                width - 8.0 - counts_width,
                y + 5.0,
                counts_width,
                16.0,
            ));
            counts.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            container.addSubview(&counts);
            views.push(button.into_super().into_super());
            views.push(counts.into_super().into_super());
        }
        *ivars.change_rows.borrow_mut() = views;
        *ivars.change_tree_rows.borrow_mut() = tree;
        container.scrollPoint(NSPoint::new(0.0, document_height));

        // The diff title and the diff itself fill whatever the list left.
        let diff_top = list_top - list_height - 12.0;
        diff_title.setFrame(rect(18.0, diff_top - 18.0, width - 12.0, 18.0));
        diff_scroll.setFrame(rect(12.0, 64.0, width, (diff_top - 24.0 - 64.0).max(40.0)));
    }

    /// The tree row showing change `index`, if it is not inside a collapsed
    /// directory.
    fn change_row_for(&self, index: usize) -> Option<usize> {
        self.ivars().change_tree_rows.borrow().iter().position(
            |row| matches!(row, change_tree::TreeRow::File { change, .. } if *change == index),
        )
    }

    /// Lays the tree out again after a directory was collapsed or expanded,
    /// keeping the selected file's highlight when it is still visible.
    fn repopulate_change_tree(&self) {
        let owner = self.session();
        let Some(snapshot) = owner.workspace_snapshot.borrow().clone() else {
            return;
        };
        if snapshot.changes.is_empty() {
            return;
        }
        self.populate_change_list(&snapshot.changes, None);
        let selected = owner.selected_change.borrow().clone();
        let row = selected.and_then(|selected| {
            snapshot
                .changes
                .iter()
                .position(|change| change.path == selected)
                .and_then(|index| self.change_row_for(index))
        });
        self.highlight_change_row(row);
    }

    /// The path a tree row refers to, relative to the repository and
    /// absolute; directories and files alike.
    fn change_row_target(&self, row: usize) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let owner = self.session();
        let snapshot = owner.workspace_snapshot.borrow();
        let snapshot = snapshot.as_ref()?;
        let rows = self.ivars().change_tree_rows.borrow();
        let relative = match rows.get(row)? {
            change_tree::TreeRow::Directory { path, .. } => path.clone(),
            change_tree::TreeRow::File { change, .. } => {
                snapshot.changes.get(*change)?.path.clone()
            }
        };
        let root = snapshot
            .repository_root
            .clone()
            .unwrap_or_else(|| snapshot.project.clone());
        Some((root.join(&relative), relative))
    }

    fn sender_row(sender: &AnyObject) -> Option<usize> {
        let tag = sender
            .downcast_ref::<NSMenuItem>()
            .map(|item| item.tag())
            .or_else(|| sender.downcast_ref::<NSButton>().map(|button| button.tag()))?;
        usize::try_from(tag).ok()
    }

    /// Context menu for a change-tree row: open it in the editor, a terminal
    /// at its directory, the Finder, or copy its path.
    fn change_row_menu(&self, row: usize, editor: &str, terminal: &str) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Change"));
        menu.setAutoenablesItems(false);
        let items: [(String, objc2::runtime::Sel); 5] = [
            (format!("Open in {editor}"), sel!(openChangeInEditor:)),
            (format!("Open {terminal} Here"), sel!(openChangeInTerminal:)),
            ("Reveal in Finder".to_owned(), sel!(revealChange:)),
            ("Copy Path".to_owned(), sel!(copyChangePath:)),
            (
                "Copy Relative Path".to_owned(),
                sel!(copyChangeRelativePath:),
            ),
        ];
        for (index, (title, action)) in items.into_iter().enumerate() {
            if index == 3 {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            // SAFETY: the delegate implements every selector and outlives the menu.
            let item = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(&title),
                    Some(action),
                    ns_string!(""),
                )
            };
            // SAFETY: the delegate outlives every menu it targets.
            unsafe { item.setTarget(Some::<&AnyObject>(self)) };
            item.setTag(isize::try_from(row).unwrap_or(isize::MAX));
        }
        menu
    }

    /// The chosen application's name, or the kind's plain noun when none is
    /// installed, for menu titles.
    fn launcher_name(&self, kind: launchers::LauncherKind) -> String {
        self.launcher_for(kind).map_or_else(
            || {
                let noun = kind.noun();
                let mut name = String::with_capacity(noun.len());
                let mut characters = noun.chars();
                if let Some(first) = characters.next() {
                    name.extend(first.to_uppercase());
                    name.push_str(characters.as_str());
                }
                name
            },
            |launcher| launcher.name.to_owned(),
        )
    }

    /// The editor or terminal the user chose, or the first one installed.
    fn launcher_for(&self, kind: launchers::LauncherKind) -> Option<launchers::Launcher> {
        let preference = {
            let preferences = self.ivars().preferences.borrow();
            match kind {
                launchers::LauncherKind::Editor => preferences.editor.clone(),
                launchers::LauncherKind::Terminal => preferences.terminal.clone(),
                launchers::LauncherKind::Browser => preferences.browser.clone(),
            }
        };
        // With nothing stored, a page goes to the system's default browser
        // rather than to the first one on Omperor's list.
        if kind == launchers::LauncherKind::Browser && preference.is_none() {
            return None;
        }
        launchers::choose(kind, preference.as_deref())
    }

    /// Hands `target` to the editor or terminal on a worker; the poll timer
    /// reports how it went.
    fn launch_in(&self, kind: launchers::LauncherKind, target: std::path::PathBuf) {
        let key = self.session().key;
        let Some(launcher) = self.launcher_for(kind) else {
            self.set_chat_status(&format!(
                "No {} is installed that Omperor knows how to open",
                kind.noun()
            ));
            return;
        };
        self.set_chat_status(&format!(
            "Opening {} in {}…",
            path_leaf(&target),
            launcher.name
        ));
        let submitted = self.jobs().submit(move || {
            let target = if matches!(kind, launchers::LauncherKind::Terminal) && !target.is_dir() {
                target.parent().map_or_else(|| target.clone(), std::path::Path::to_path_buf)
            } else { target };
            let result = launcher.launch(&target);
            BackgroundEvent::Launched {
                key,
                app: launcher.name,
                target,
                result,
            }
        });
        if !submitted {
            self.set_chat_status(&format!(
                "Could not start {} to open the project",
                launcher.name
            ));
        }
    }

    /// The menu behind the session chip: the id, and the journal it names.
    fn session_chip_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Session"));
        menu.setAutoenablesItems(false);
        let items: [(&str, objc2::runtime::Sel); 4] = [
            ("Timeline…", sel!(showSessionTimeline:)),
            ("Copy Session ID", sel!(copySessionId:)),
            ("Copy Journal Path", sel!(copySessionJournalPath:)),
            ("Reveal Journal in Finder", sel!(revealSessionJournal:)),
        ];
        for (index, (title, action)) in items.into_iter().enumerate() {
            if index == 1 || index == 2 {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            // SAFETY: the delegate implements every selector and outlives the menu.
            let item = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(title),
                    Some(action),
                    ns_string!(""),
                )
            };
            // SAFETY: the delegate outlives every menu it targets.
            unsafe { item.setTarget(Some::<&AnyObject>(self)) };
        }
        menu
    }

    /// Names the session the header is showing, or none. The journal path
    /// is the summary's when a stored session is opened, and derived from
    /// the sessions directory when OMP reports a fresh one.
    fn set_header_session(&self, id: Option<&str>, journal: Option<std::path::PathBuf>) {
        let owner = self.session();
        let ivars = self.ivars();
        let session = id.map(|id| {
            let journal = journal.unwrap_or_else(|| {
                ivars
                    .omperor_sessions_directory
                    .get()
                    .cloned()
                    .unwrap_or_default()
                    .join(format!("{id}.oms"))
            });
            (id.to_owned(), journal)
        });
        let Some(chip) = ivars.session_chip.get() else {
            owner.header_session.replace(session);
            return;
        };
        match &session {
            Some((id, _)) => {
                chip.setAttributedTitle(&attributed(
                    "Session",
                    &theme::ui_font(theme::CAPTION_SIZE, false),
                    &theme::secondary_text(),
                    0.0,
                ));
                chip.setToolTip(Some(&NSString::from_str(&format!(
                    "{id} · Click to copy · Right-click for actions"
                ))));
                if let Some(image) = theme::row_symbol("doc.on.doc", 11.0, 4.0) {
                    chip.setImage(Some(&image));
                }
                chip.setHidden(false);
            }
            None => chip.setHidden(true),
        }
        owner.header_session.replace(session);
        self.layout_header();
    }

    fn sync_selected_session_header(&self) {
        let owner = self.session();
        let path = owner.selected_session_path.borrow().clone();
        let summary = self.ivars().sessions.borrow().iter()
            .find(|summary| Some(&summary.path) == path.as_ref()).cloned();
        // Until the session is listed, a mock one goes by the title its
        // sidebar row already shows.
        let mock_title = owner.mock_session_title.borrow().clone();
        let title = summary.as_ref().map(|summary| summary.title.as_str())
            .or(mock_title.as_deref())
            .unwrap_or("New conversation");
        if let Some(label) = self.ivars().center_title.get() {
            label.setStringValue(&NSString::from_str(title));
        }
        let session = owner.header_session.borrow().clone();
        self.set_header_session(session.as_ref().map(|(id, _)| id.as_str()), session.as_ref().map(|(_, path)| path.clone()));
    }

    /// Names the model and account on the header's pill, from the composer's
    /// own labels so the two never disagree.
    fn sync_header_identity(&self) {
        let ivars = self.ivars();
        let (Some(header), Some(model)) = (ivars.usage_header.get(), ivars.model_label.get()) else {
            return;
        };
        let model = model.stringValue().to_string();
        let model = model.trim_end_matches('▾').trim();
        if model.is_empty() || model.starts_with("Loading") {
            if header.set_identity("", "") {
                self.layout_header();
            }
            return;
        }
        let account = ivars
            .account_label
            .get()
            .filter(|label| !label.isHidden())
            .map(|label| label.stringValue().to_string())
            .and_then(|label| {
                label
                    .trim_end_matches('▾')
                    .trim()
                    .strip_prefix("Account: ")
                    .map(str::to_owned)
            });
        let mut text = model.to_owned();
        let mut tip = format!("Model: {model}");
        if let Some(account) = account {
            text.push_str(&format!(" · {account}"));
            tip.push_str(&format!(" · Account: {account}"));
        }
        if self.mock_backend_active() && !mock_inference::tour() {
            text.push_str(" · temporary");
            tip.push_str(" · temporary: nothing is sent to a provider or kept");
        }
        if header.set_identity(&text, &tip) {
            self.layout_header();
        }
    }

    /// Places the path line's crumbs: the project and path first, then the
    /// session, sharing one centre line under the title. The path gives way
    /// first when the pane is narrow, truncating in its middle.
    fn layout_header(&self) {
        let _span = perf::span("gui.layout_header");
        let ivars = self.ivars();
        let (Some(path), Some(chip)) = (ivars.workspace_path_label.get(), ivars.session_chip.get())
        else {
            return;
        };
        // SAFETY: both live in the header for the process lifetime.
        let Some(pane) = (unsafe { path.superview() }) else {
            return;
        };
        let bounds = pane.frame().size;
        if let Some(header) = ivars.usage_header.get() { header.layout(bounds); }
        let prefs = ivars.preferences.borrow();
        let typography =
            native_markdown::Typography::new(prefs.chat_style, prefs.chat_text_size.scale())
                .with_sizes(prefs.chat_text_sizes);
        drop(prefs);
        let metadata_font = typography.interface_font(TextRole::Metadata);
        path.setFont(Some(&metadata_font));
        chip.setFont(Some(&metadata_font));
        if let Some(status) = ivars.chat_status.get() {
            status.setFont(Some(&metadata_font));
            // The model pill leads the status line; the state follows it.
            let pill = ivars.usage_header.get().map_or(0.0, |header| {
                header.place_pill(STATUS_X, bounds.height - 97.5, (bounds.width * 0.4).max(120.0))
            });
            // With the pill, the line drops a few points so its text and the
            // pill share a centre and the pill clears the path above.
            let (x, y) = if pill > 0.0 {
                (STATUS_X + pill + 8.0, bounds.height - 113.0)
            } else {
                (STATUS_X, bounds.height - 110.0)
            };
            status.setFrame(rect(x, y, bounds.width - 56.0 - x, 24.0));
        }
        let left = 28.0;
        let compact = bounds.width < 720.0;
        // The metrics block sizes to its longest line and the title yields to
        // it: a fixed width clipped the settled line ("447 to…"), and the
        // block's old right edge ran under the Focus toggle. The rate line
        // always fits the floor, so only the settled detail can widen the
        // block — once per response, never per streaming tick.
        let metrics_width = if compact {
            0.0
        } else {
            let detail_natural = ivars.speedometer_detail.get().map_or(0.0, |detail| {
                detail.sizeToFit();
                detail.frame().size.width.ceil() + 2.0
            });
            let floor = SPEEDOMETER_WIDTH - 42.0;
            let label_width = detail_natural
                .max(floor)
                .min((bounds.width * 0.35).max(floor));
            let block_width = 42.0 + label_width;
            let toggle_left = ivars
                .workbench_toggle
                .get()
                .map_or(bounds.width - 102.0, |toggle| toggle.frame().origin.x);
            let x = toggle_left - 104.0 - block_width;
            if let Some(dial) = ivars.speedometer.get() {
                dial.setFrame(rect(x, bounds.height - 66.0, 36.0, 24.0));
            }
            if let Some(rate) = ivars.speedometer_rate.get() {
                rate.setFrame(rect(x + 42.0, bounds.height - 64.0, label_width, 18.0));
            }
            if let Some(detail) = ivars.speedometer_detail.get() {
                detail.setFrame(rect(x + 42.0, bounds.height - 80.0, label_width, 15.0));
            }
            block_width + 12.0
        };
        let right = bounds.width - 174.0 - metrics_width;
        if let Some(title) = ivars.center_title.get() {
            title.setFont(Some(&typography.interface_font(TextRole::Title)));
            title.setFrame(rect(
                left,
                bounds.height - 66.0,
                (bounds.width - 234.0 - metrics_width).max(120.0),
                32.0,
            ));
            title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            title.setToolTip(Some(&title.stringValue()));
        }
        for item in [ivars.speedometer_rate.get(), ivars.speedometer_detail.get()]
            .into_iter()
            .flatten()
        {
            item.setHidden(compact);
        }
        if let Some(dial) = ivars.speedometer.get() {
            dial.setHidden(compact);
        }

        let centre = bounds.height - 78.0;
        path.sizeToFit();
        let natural = path.frame().size;
        chip.sizeToFit();
        let chip_size = chip.frame().size;
        let chip_room = if chip.isHidden() {
            0.0
        } else {
            chip_size.width + 6.0
        };
        let path_width = natural.width.min((right - left - chip_room).max(0.0));
        path.setFrame(rect(
            left,
            (centre - natural.height / 2.0).round(),
            path_width,
            natural.height,
        ));
        chip.setFrame(rect(
            left + path_width + 6.0,
            (centre - chip_size.height / 2.0).round(),
            chip_size.width,
            chip_size.height,
        ));
    }

    fn reveal_in_finder(&self, path: &std::path::Path) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    }

    /// Puts `text` on the pasteboard and says so for a moment, naming `what`
    /// was copied rather than echoing a long path back.
    fn copy_to_pasteboard(&self, text: &str, what: &str) {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        // SAFETY: NSPasteboardTypeString is an AppKit static valid for the
        // process lifetime.
        pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        self.flash_chat_status(&format!("Copied {what}"));
    }

    /// Names the chosen editor and terminal in the File menu and the pane's
    /// header tooltips, so "Open Project in Zed" says what it will do.
    fn sync_launcher_titles(&self) {
        let editor = self.launcher_name(launchers::LauncherKind::Editor);
        let terminal = self.launcher_name(launchers::LauncherKind::Terminal);
        if let Some(item) = self.ivars().editor_menu_item.get() {
            item.setTitle(&NSString::from_str(&format!("Open Project in {editor}")));
        }
        if let Some(item) = self.ivars().terminal_menu_item.get() {
            item.setTitle(&NSString::from_str(&format!("Open Terminal in {terminal}")));
        }
        if let Some(button) = self.ivars().open_editor_button.get() {
            button.setToolTip(Some(&NSString::from_str(&format!(
                "Open the project in {editor}"
            ))));
        }
        if let Some(button) = self.ivars().open_terminal_button.get() {
            button.setToolTip(Some(&NSString::from_str(&format!(
                "Open {terminal} in the project"
            ))));
        }
    }

    /// Shows one workbench panel and hides the other.
    /// Showing the workbench only changes view geometry. Discovery and RPC
    /// continue on their existing workers whether this pane is visible or not.
    fn set_workbench_visible(&self, visible: bool) {
        let ivars = self.ivars();
        let (Some(split), Some(inspector)) = (ivars.split_view.get(), ivars.inspector_pane.get())
        else {
            return;
        };
        if ivars.inspector_visible.replace(visible) == visible {
            return;
        }
        if visible {
            split.addSubview(inspector);
        } else {
            inspector.removeFromSuperview();
        }
        split.adjustSubviews();
        split.setPosition_ofDividerAtIndex(SIDEBAR_WIDTH, 0);
        if visible {
            split.setPosition_ofDividerAtIndex(
                split.bounds().size.width - self.activity_width(split.bounds().size.width) - 1.0,
                1,
            );
        }
        if let Some(toggle) = ivars.workbench_toggle.get() {
            let tint = if visible {
                theme::accent()
            } else {
                theme::secondary_text()
            };
            toggle.setContentTintColor(Some(&tint));
            toggle.setTitle(if visible {
                ns_string!("Focus")
            } else {
                ns_string!("Agents")
            });
            toggle.setToolTip(Some(if visible {
                ns_string!("Hide the agent panel and give this view more room (⌘⇧I)")
            } else {
                ns_string!("Show the agent panel beside this view (⌘⇧I)")
            }));
            toggle.setAccessibilityLabel(Some(if visible {
                ns_string!("Focus on this view (⌘⇧I)")
            } else {
                ns_string!("Show agent panel (⌘⇧I)")
            }));
        }
        self.layout_header();
        self.sync_transcript_measure();
        self.refit_composer_chips();
    }

    fn show_workbench_panel(&self, index: isize) {
        self.set_workbench_visible(true);
        let ivars = self.ivars();
        let agents = index == 1;
        self.refresh_workbench_location();
        if let Some(panel) = ivars.changes_panel.get() {
            panel.setHidden(agents);
        }
        if let Some(panel) = ivars.agents_panel.get() {
            panel.setHidden(!agents);
        }
        if let Some(switcher) = ivars.workbench_switcher.get()
            && switcher.selectedSegment() != index
        {
            switcher.setSelectedSegment(index);
        }
        if agents {
            self.request_agents_refresh();
        } else {
            self.request_observatory_refresh();
        }
    }

    /// Asks for the sidebar to be rebuilt once the current pass of the run
    /// loop is done: every request made while a batch of events is applied,
    /// or while an action runs, is served by one rebuild before the loop
    /// waits again — never a rebuild per event.
    pub(crate) fn request_sidebar_rebuild(&self) {
        self.request_refresh(REFRESH_SIDEBAR);
    }

    /// Rebuilds the sidebar in a pass of its own, after this one is drawn
    /// and committed: a timer, since a block or a due timer would run
    /// before the run loop waits and so inside this pass.
    fn rebuild_sidebar_next_pass(&self) {
        if self.ivars().sidebar_next_pass.replace(true) {
            return;
        }
        let delegate = self.retain();
        let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
            if delegate.ivars().sidebar_next_pass.replace(false) {
                delegate.rebuild_sidebar_sessions();
            }
        });
        // SAFETY: the block runs on the main run loop, which owns the
        // delegate; common modes keep it firing during tracking.
        let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(0.001, false, &block) };
        // SAFETY: the main run loop and the mode constant live for the process.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    }

    /// Only the sidebar's state indicators, likewise: a session started,
    /// stopped or began waiting, and nothing else the list shows changed.
    fn request_sidebar_state_refresh(&self) {
        self.request_refresh(REFRESH_SIDEBAR_STATES);
    }

    /// The Agents panel, likewise, once the pass is done.
    pub(crate) fn request_agents_refresh(&self) {
        self.request_refresh(REFRESH_AGENTS);
    }

    /// The Observatory, likewise, once the pass is done.
    pub(crate) fn request_observatory_refresh(&self) {
        self.request_refresh(REFRESH_WORKSPACE);
    }

    fn request_refresh(&self, what: u8) {
        let pending = self.ivars().pending_refresh.get();
        self.ivars().pending_refresh.set(pending | what);
        if pending != 0 {
            return;
        }
        let Some(run_loop) = objc2_core_foundation::CFRunLoop::main() else {
            self.flush_refreshes();
            return;
        };
        let delegate = self.retain();
        let block = block2::RcBlock::new(move || delegate.flush_refreshes());
        // SAFETY: the block runs on the main run loop, which owns the
        // delegate, before the loop waits; the mode constant is a
        // process-lifetime static.
        unsafe {
            run_loop.perform_block(
                objc2_core_foundation::kCFRunLoopCommonModes.map(|mode| mode.as_ref()),
                Some(&block),
            );
        }
    }

    /// Serves the refreshes asked for so far, the widest one covering the
    /// rest: a sidebar rebuild refreshes the Agents panel, which refreshes
    /// the Observatory.
    pub(crate) fn flush_refreshes(&self) {
        let pending = self.ivars().pending_refresh.replace(0);
        if pending == 0 {
            return;
        }
        let _span = perf::span("gui.flush_refreshes");
        if pending & REFRESH_SIDEBAR != 0 {
            self.rebuild_sidebar_sessions();
            return;
        }
        // A state refresh that finds the rows changed rebuilds them, which
        // covers the rest.
        if pending & REFRESH_SIDEBAR_STATES != 0 && self.refresh_sidebar_states() {
            return;
        }
        if pending & REFRESH_AGENTS != 0 {
            self.refresh_agents_panel();
        } else if pending & REFRESH_WORKSPACE != 0 {
            self.refresh_session_workspace();
        }
    }

    /// Records what the live session is doing and refreshes the Agents panel.
    /// A change of dot family (busy, blocked, done, failed) also redraws the
    /// session's sidebar indicator.
    fn set_live_state(&self, state: agents::AgentState) {
        let owner = self.session();
        let previous = owner.live_state.replace(state.clone());
        if previous == state {
            return;
        }
        if previous.dot() != state.dot() {
            self.request_sidebar_state_refresh();
        }
        self.request_agents_refresh();
    }

    /// A session shows up in the sidebar and the Agents panel once its
    /// journal holds a conversation, which a fresh session's first turn is
    /// writing right now; discovery normally runs at turn end, so it is asked
    /// for early — once — so the working agent is listed while it works.
    /// The log beside the sessions, opened the first time something is
    /// recorded; `None` when there is no home to put it in.
    fn metrics_log(&self) -> Option<&metrics::Log> {
        let ivars = self.ivars();
        if let Some(log) = ivars.metrics_log.get() {
            return Some(log);
        }
        let directory = preferences::omp_sessions_directory().ok()?;
        let _ = ivars
            .metrics_log
            .set(metrics::Log::open(directory.join(metrics::FILE_NAME)));
        ivars.metrics_log.get()
    }

    /// The session and turn a record belongs to.


    /// A response settled: the needle snaps to the receipt's exact rate and
    /// the response is written to the log.


    /// Writes one record to the log and, once the Metrics page has read the
    /// log, keeps what it read current so the page follows the turn.
    fn record_metric(&self, record: metrics::Record) {
        let ivars = self.ivars();
        if ivars.metrics_loaded.get() {
            ivars.metrics_records.borrow_mut().push(record.clone());
            ivars.metrics_ui_dirty.set(true);
        }
        if let Some(log) = self.metrics_log() {
            log.record(record);
        }
    }

    /// Whether the Settings window is up with the Metrics page in front.
    fn metrics_page_showing(&self) -> bool {
        self.ivars().settings_ui.get().is_some_and(|settings| {
            settings.window.isVisible()
                && settings
                    .pages
                    .get(SETTINGS_METRICS_PAGE)
                    .is_some_and(|page| !page.isHidden())
        })
    }

    /// Nothing chosen yet on the Metrics page: the open session when the
    /// log has something for it, otherwise everything.
    fn choose_default_metrics_session(&self) {
        let owner = self.session();
        let ivars = self.ivars();
        if *ivars.metrics_selection.borrow() != MetricsSelection::Unchosen {
            return;
        }
        let live = owner.live_session_id.borrow().clone();
        let chosen = match live {
            Some(live)
                if metrics::sessions(&ivars.metrics_records.borrow())
                    .iter()
                    .any(|session| session.id == live) =>
            {
                MetricsSelection::Session(live)
            }
            _ => MetricsSelection::All,
        };
        ivars.metrics_selection.replace(chosen);
    }

    /// A tool call finished: how long the harness spent in it goes to the
    /// log. A call whose start was never seen (a resumed turn) is skipped
    /// rather than guessed at.


    /// The turn handed control back: its wall time goes to the log, and a
    /// response still counted as in flight is let go.


    /// A session must not display the preceding session's inference receipt.
    fn reset_session_metrics(&self) {
        let owner = self.session();
        owner.live_rate.replace(metrics::LiveRate::default());
        owner.turn_started_at.set(None);
        owner.turn_started_ms.set(None);
        owner.tool_starts.borrow_mut().clear();
        if let Some(header) = self.ivars().usage_header.get() {
            header.clear_rates();
        }
        self.refresh_speedometer();
    }

    /// Moves the needle while tokens stream, ten times a second.
    fn tick_speedometer(&self) {
        let owner = self.session();
        let ivars = self.ivars();
        let now = std::time::Instant::now();
        if ivars
            .speedometer_last_tick
            .get()
            .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_millis(100))
        {
            return;
        }
        let streaming = matches!(
            owner.live_rate.borrow_mut().reading(now),
            metrics::Reading::Streaming { .. } | metrics::Reading::Waiting
        );
        if streaming {
            ivars.speedometer_last_tick.set(Some(now));
            self.refresh_speedometer();
        }
    }

    /// Draws what the live rate says: the needle, the figure, and the line
    /// under it.
    fn refresh_speedometer(&self) {
        let owner = self.session();
        let ivars = self.ivars();
        let (Some(dial), Some(rate), Some(detail)) = (
            ivars.speedometer.get(),
            ivars.speedometer_rate.get(),
            ivars.speedometer_detail.get(),
        ) else {
            return;
        };
        let reading = owner.live_rate
            .borrow_mut()
            .reading(std::time::Instant::now());
        let (figure, line, tone, needle, estimated) = match reading {
            metrics::Reading::Idle => ("— tok/s".to_owned(), String::new(), 2, None, false),
            metrics::Reading::Waiting => (
                "waiting…".to_owned(),
                "for the first token".to_owned(),
                1,
                None,
                true,
            ),
            metrics::Reading::Streaming { estimate_tps } => (
                format!("~{} tok/s", estimate_tps.round() as u64),
                "streaming".to_owned(),
                1,
                Some(estimate_tps),
                true,
            ),
            metrics::Reading::Settled(settled) => (
                settled.tokens_per_second.map_or_else(
                    || "— tok/s".to_owned(),
                    |tps| format!("{} tok/s", tps.round() as u64),
                ),
                speedometer_detail_line(&settled),
                0,
                settled.tokens_per_second,
                false,
            ),
        };
        dial.show(needle, estimated);
        // The strip's sparkline keeps the recent history of the same figure;
        // while a response waits for its first token it holds, so the line
        // skips the estimate's first unsteady beat.
        if let Some(header) = ivars.usage_header.get() {
            match needle {
                Some(tps) => header.record_rate(tps, !estimated),
                None if estimated => header.hold_rates(),
                None => {}
            }
        }
        rate.setStringValue(&NSString::from_str(&figure));
        let ink = match tone {
            0 => NSColor::labelColor(),
            1 => theme::secondary_text(),
            _ => theme::secondary_text(),
        };
        rate.setTextColor(Some(&ink));
        // A new detail line can need more room than the last; the header
        // refits the block to it. The rate alone never does.
        if detail.stringValue().to_string() != line {
            detail.setStringValue(&NSString::from_str(&line));
            self.layout_header();
        }
    }

    /// What the status line calls the thing doing the work.
    fn agent_name(&self) -> &'static str {
        if self.mock_backend_active() { "Mock agent" } else { "OMP" }
    }

    /// The mock backend's live conversation as a sidebar row. Discovery
    /// rebuilds the list from disk, where the mock has nothing, so the row
    /// is put back whenever the list is replaced.
    fn ensure_mock_session_listed(&self) {
        self.merge_retained_session_rows();
    }

    fn merge_retained_session_rows(&self) {
        let entries = self.ivars().live_sessions.borrow().entries();
        let mut rows = self.ivars().sessions.borrow_mut();
        for owner in entries {
            let id = owner.live_session_id.borrow().clone().unwrap_or_else(|| format!("draft-{}", owner.key.0));
            if rows.iter().any(|row| row.id == id) { continue; }
            let path = owner.selected_session_path.borrow().clone().unwrap_or_else(|| {
                self.ivars().omperor_sessions_directory.get().cloned().unwrap_or_default().join(format!("{id}.oms"))
            });
            owner.selected_session_path.replace(Some(path.clone()));
            let mut row = session::SessionSummary::live_mock(&id, &owner.current_project.borrow(), path.parent().unwrap_or(std::path::Path::new("")));
            row.path = path;
            row.model = Some(owner.current_model.borrow().clone());
            row.set_title(owner.mock_session_title.borrow().as_deref().unwrap_or(if id.starts_with("draft-") { "Draft conversation" } else if id.starts_with("mock-") { "Mock conversation" } else { "New conversation" }));
            rows.insert(0, row);
        }
    }



    /// The mock row is the selection: its path is the one the header derives
    /// for the live session, so a click on it is a no-op.


    /// A mock conversation takes its first prompt as its title, the way a
    /// real one does.
    fn remember_mock_title(&self, prompt: &str) {
        let owner = self.session();
        if !self.mock_backend_active() {
            return;
        }
        if owner.mock_session_title.borrow().is_some() {
            return;
        }
        let title = session::title_from_prompt(prompt);
        owner.mock_session_title.replace(Some(title.clone()));
        let mut renamed = false;
        for summary in self
            .ivars()
            .sessions
            .borrow_mut()
            .iter_mut()
            .filter(|summary| owner.live_session_id.borrow().as_deref() == Some(summary.id.as_str()))
        {
            summary.set_title(&title);
            renamed = true;
        }
        // The owner is the open conversation, so the header names it now,
        // listed yet or not.
        if let Some(label) = self.ivars().center_title.get() {
            label.setStringValue(&NSString::from_str(&title));
        }
        if renamed {
            self.request_sidebar_rebuild();
        }
    }



    /// The session's own agent as the panel's root: what the child serving
    /// it is doing, or "not attached" when the history is shown read-only.
    fn root_agent(&self) -> agents::RootAgent {
        let owner = self.session();
        let ivars = self.ivars();
        let attached = owner.live_session_id.borrow().is_some() && !owner.read_only_session.get();
        let title = ivars
            .center_title
            .get()
            .map(|label| label.stringValue().to_string())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "This session".to_owned());
        agents::RootAgent {
            title,
            state: if attached {
                owner.live_state.borrow().clone()
            } else {
                agents::AgentState::Detached
            },
            elapsed: owner.turn_started_at.get().map(|started| started.elapsed()),
            model: owner.current_model.borrow().clone(),
        }
    }

    /// A stored session's state: the live one's, or what its journal says.
    fn session_state(&self, summary: &session::SessionSummary) -> agents::AgentState {
        self.ivars().live_sessions.borrow().find_path(&summary.path)
            .map(|owner| owner.sidebar_state())
            .unwrap_or_else(|| agents::AgentState::from_outcome(summary.outcome))
    }

    /// Marks the selected inline inspection row and clears the rest.
    /// Only fill colors change, so a row may call this from its own action
    /// without disturbing the view it was clicked on.
    fn mark_agent_row_selection(&self) {
        let ivars = self.ivars();
        let open = self.agent_view().selected;
        for button in ivars.agents_row_buttons.borrow().iter() {
            button.set_selected(open.as_deref() == Some(button.subagent_id().as_str()));
        }
    }

    /// Advances every running row's clock once a second while the panel is
    /// showing; nothing else in the panel changes by itself.
    fn tick_agents_panel(&self) {
        let owner = self.session();
        let ivars = self.ivars();
        if ivars
            .agents_panel
            .get()
            .is_none_or(|panel| panel.isHidden())
        {
            return;
        }
        if ivars.agents_ticking.borrow().is_empty() {
            return;
        }
        let now = std::time::Instant::now();
        if ivars
            .agents_last_tick
            .get()
            .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_secs(1))
        {
            return;
        }
        ivars.agents_last_tick.set(Some(now));
        let now_ms = self.agent_clock_ms();
        for (label, prefix, clock) in ivars.agents_ticking.borrow().iter() {
            let elapsed = match clock {
                AgentClock::Turn => owner.turn_started_at.get().map(|started| started.elapsed()),
                AgentClock::Started(started) => Some(std::time::Duration::from_millis(
                    now_ms.saturating_sub(*started),
                )),
            };
            if let Some(elapsed) = elapsed {
                let time = agents::elapsed_text(elapsed);
                label.setStringValue(&NSString::from_str(&if prefix.is_empty() {
                    time
                } else {
                    format!("{prefix} · {time}")
                }));
            }
        }
    }

    /// Folds every current sub-agent journal that changed since the last pass,
    /// on a worker. The cadence follows what the user is looking at: often
    /// while a sub-agent is working or evidence is open, rarely once everything
    /// has settled.
    fn sync_subagent_transcripts(&self, force: bool) {
        let key = self.session().key;
        let owner = self.session();
        if owner.subagent_sync_inflight.get() {
            return;
        }
        let subagents = owner.subagents.borrow();
        if subagents.is_empty() {
            if !owner.subagent_transcripts.borrow().is_empty() {
                owner.subagent_transcripts.borrow_mut().clear();
            }
            return;
        }
        let any_busy = subagents
            .iter()
            .any(|info| agents::AgentState::from_status(&info.status).is_busy());
        if !force
            && owner.subagent_sync_at
                .get()
                .is_some_and(|last| last.elapsed() < transcripts::interval(any_busy, false))
        {
            return;
        }
        let view = self.agent_view();
        let requests = {
            let mut cache = owner.subagent_transcripts.borrow_mut();
            transcripts::prune(&mut cache, &subagents);
            let mut requests = transcripts::plan(&subagents, &cache);
            let inspected = view.desk.pins.iter().map(|pin| pin.id.as_str())
                .chain(view.selected.as_deref());
            for id in inspected {
                if let Some(request) = requests.iter_mut().find(|request| request.id == id) {
                    request.inspected = true;
                    continue;
                }
                if let Some(info) = subagents.iter().find(|info| info.id == id)
                    && let Some(path) = &info.session_file
                {
                    requests.push(transcripts::Request {
                        id: info.id.clone(),
                        path: path.clone(),
                        known: cache
                            .get(&info.id)
                            .map(|entry| entry.stamp)
                            .unwrap_or_default(),
                        inspected: true,
                    });
                }
            }
            cache.retain(|id, _| requests.iter().any(|request| &request.id == id));
            owner.workspace_read_failures.borrow_mut()
                .retain(|id, _| requests.iter().any(|request| &request.id == id));
            requests
        };
        if requests.is_empty() {
            return;
        }
        owner.subagent_sync_at.set(Some(std::time::Instant::now()));
        let session = owner.selected_session_path.borrow().clone();
        let generation = owner.evidence_generation.get();
        let submitted = self
            .jobs()
            .submit(move || BackgroundEvent::SubagentTranscriptsSynced {
                key,
                generation,
                session,
                result: transcripts::sync(requests),
            });
        owner.subagent_sync_inflight.set(submitted);
    }

    /// Files warmed journals and refreshes the nonmodal evidence surfaces.


    fn apply_session_transcript_updates(&self, owner: &live_sessions::LiveSession, updates: Vec<transcripts::Update>) -> bool {
        let _span = perf::span_with(
            "gui.transcripts.apply",
            || serde_json::json!({"updates": updates.len()}),
        );
        if updates.is_empty() {
            return false;
        }
        let changed = transcripts::apply(&mut owner.subagent_transcripts.borrow_mut(), updates);
        if changed.is_empty() { return false; }
        for id in changed {
            owner.workspace_read_failures.borrow_mut().remove(&id);
        }
        if self.session().key == owner.key {
            self.refresh_agent_inspection();
            self.refresh_session_workspace();
        }
        true
    }

    /// Opens the timeline window for the journal at `path` and streams it
    /// in from a worker the way the chat's history arrives: the newest
    /// events as soon as the fold lands, then older pages filling in above
    /// them, one per turn of the run loop, while the window stays
    /// responsive. A window already up is replaced.
    fn open_timeline(&self, title: String, path: std::path::PathBuf) {
        if !journal::is_journal(&path) {
            self.set_chat_status("This session has no journal to draw a timeline from");
            return;
        }
        let previous = self.ivars().timeline_window.borrow().clone();
        if let Some(previous) = previous {
            previous.close();
        }
        self.present_timeline_window(&title);
        self.ivars().timeline_path.replace(Some(path.clone()));
        // One page in flight: the worker holds the rest until the last is
        // taken, and stops once the window closes and drops the receiver.
        let (sender, receiver) = flume::bounded(1);
        self.ivars().timeline_stream.replace(Some(receiver));
        self.show_timeline_progress(Some("Reading the journal…"));
        let started = std::thread::Builder::new()
            .name("omperor-timeline".into())
            .spawn(move || timeline::stream(&path, &sender, gui_wake::notify));
        if started.is_err() {
            self.finish_timeline_stream(Some("Could not start reading the journal"));
        }
    }

    /// Takes the timeline worker's next page, if one is waiting: the newest
    /// events first, then older ones in front of them.
    fn poll_timeline_stream(&self) {
        let update = match self
            .ivars()
            .timeline_stream
            .borrow()
            .as_ref()
            .map(flume::Receiver::try_recv)
        {
            None | Some(Err(flume::TryRecvError::Empty)) => return,
            Some(Ok(update)) => update,
            Some(Err(flume::TryRecvError::Disconnected)) => {
                timeline::Update::Failed("the reader stopped before it finished".to_owned())
            }
        };
        let _span = perf::span("gui.timeline.page");
        match update {
            timeline::Update::Newest { summary, events } => self.start_timeline_list(summary, events),
            timeline::Update::Older(events) => {
                let list = self.ivars().timeline_list.borrow().clone();
                let Some(list) = list else {
                    return;
                };
                list.prepend(events);
                let (arrived, total) = {
                    let loaded = list.loaded();
                    (loaded.len(), loaded.summary.events)
                };
                self.refresh_timeline_rows();
                self.show_timeline_progress(Some(&format!(
                    "Loading earlier events · {arrived} of {total}"
                )));
            }
            timeline::Update::Complete => self.finish_timeline_stream(None),
            timeline::Update::Failed(error) => {
                self.finish_timeline_stream(Some(&format!("Could not read the journal: {error}")));
            }
        }
    }

    /// The worker is done: the footer's spinner goes, and a failure takes
    /// the subtitle's place.
    fn finish_timeline_stream(&self, failure: Option<&str>) {
        self.ivars().timeline_stream.borrow_mut().take();
        self.show_timeline_progress(None);
        if let Some(failure) = failure
            && let Some(subtitle) = self.ivars().timeline_subtitle.borrow().as_ref()
        {
            subtitle.setStringValue(&NSString::from_str(failure));
        }
    }

    /// The footer's spinner and what it is doing, or neither.
    fn show_timeline_progress(&self, doing: Option<&str>) {
        let Some((spinner, label)) = self.ivars().timeline_progress.borrow().clone() else {
            return;
        };
        match doing {
            Some(text) => {
                label.setStringValue(&NSString::from_str(text));
                label.setHidden(false);
                spinner.setHidden(false);
                // SAFETY: the spinner is a live main-thread control.
                unsafe { spinner.startAnimation(None) };
            }
            None => {
                label.setHidden(true);
                // SAFETY: the spinner is a live main-thread control.
                unsafe { spinner.stopAnimation(None) };
                spinner.setHidden(true);
            }
        }
    }

    /// Forgets the window's views once it closes.
    fn clear_timeline_state(&self) {
        self.dismiss_timeline_detail();
        let ivars = self.ivars();
        ivars.timeline_window.borrow_mut().take();
        ivars.timeline_scroll.borrow_mut().take();
        ivars.timeline_list.borrow_mut().take();
        ivars.timeline_minimap.borrow_mut().take();
        ivars.timeline_subtitle.borrow_mut().take();
        ivars.timeline_flash.borrow_mut().take();
        ivars.timeline_path.borrow_mut().take();
        // Dropping the receiver stops the worker at its next page.
        ivars.timeline_stream.borrow_mut().take();
        ivars.timeline_progress.borrow_mut().take();
        ivars.timeline_detail_sheet.borrow_mut().take();
        ivars.timeline_search.borrow_mut().take();
        ivars.timeline_min_field.borrow_mut().take();
        ivars.timeline_min_hint.borrow_mut().take();
        ivars.timeline_kind_popup.borrow_mut().take();
        ivars.timeline_model_popup.borrow_mut().take();
        ivars.timeline_filter_note.borrow_mut().take();
        ivars.timeline_filter_dots.borrow_mut().clear();
        ivars.timeline_filter.replace(timeline::Filter::default());
        ivars.timeline_open_runs.borrow_mut().clear();
    }

    /// A window of its own, which the user can move and size (its frame is
    /// remembered): the heading, a line of totals, the filter row, the
    /// event list with the minimap strip beside its scroller, and Close.
    fn present_timeline_window(&self, title: &str) {
        let _span = perf::span("gui.timeline.present_window");
        let mtm = self.mtm();
        let (width, height) = (900.0, 620.0);
        // SAFETY: the window is retained in an ivar until it closes, and opts
        // out of AppKit's legacy release-on-close behavior.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, width, height),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&format!("Timeline · {title}")));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setMovableByWindowBackground(true);
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        window.setMinSize(NSSize::new(780.0, 420.0));
        let content = window.contentView().expect("window content view");

        let heading = text_label(&format!("Timeline · {title}"), 16.0, true, mtm);
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        heading.setFont(Some(&NSFont::systemFontOfSize_weight(17.0, unsafe {
            NSFontWeightSemibold
        })));
        heading.setFrame(rect(26.0, height - 46.0, width - 52.0, 24.0));
        heading.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        heading.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&heading);
        // The session's totals, once the fold lands; the footer says what
        // is happening until then.
        let subtitle = text_label("", 11.0, false, mtm);
        subtitle.setTextColor(Some(&theme::secondary_text()));
        subtitle.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        subtitle.setFrame(rect(26.0, height - 64.0, width - 52.0 - 170.0, 16.0));
        subtitle.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&subtitle);

        // The filter row: every control starts permissive, and a gold dot
        // appears beside one while it narrows the list.
        let filter_y = height - 100.0;
        let mut dots = Vec::with_capacity(4);
        let mut dot = |x: f64| {
            let mark = fill_box(
                rect(x, filter_y + 9.0, 8.0, 8.0),
                &theme::accent(),
                4.0,
                mtm,
            );
            mark.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
            mark.setHidden(true);
            content.addSubview(&mark);
            dots.push(mark);
        };
        dot(26.0);
        let search = NSSearchField::initWithFrame(
            NSSearchField::alloc(mtm),
            rect(38.0, filter_y, 190.0, 26.0),
        );
        search.setPlaceholderString(Some(ns_string!("Search events")));
        search.setSendsSearchStringImmediately(true);
        search.setToolTip(Some(ns_string!(
            "Plain text, any case; every word must appear in the event's kind, name, detail, sub-agent, duration, or offset."
        )));
        search.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        // SAFETY: AppDelegate implements timelineSearchChanged: and outlives the field.
        unsafe {
            search.setTarget(Some::<&AnyObject>(self));
            search.setAction(Some(sel!(timelineSearchChanged:)));
        }
        content.addSubview(&search);
        dot(240.0);
        let min_label = text_label("at least", 11.0, false, mtm);
        min_label.setTextColor(Some(&theme::secondary_text()));
        min_label.setFrame(rect(252.0, filter_y + 5.0, 48.0, 16.0));
        min_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        content.addSubview(&min_label);
        let min_field = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            rect(300.0, filter_y + 1.0, 64.0, 24.0),
        );
        min_field.setPlaceholderString(Some(ns_string!("0 s")));
        min_field.setFont(Some(&theme::ui_font(theme::CONTROL_SIZE, false)));
        min_field.setToolTip(Some(ns_string!(
            "Only events that took at least this long: 2, 2.5 s, 500 ms, 1m30s, or 1:30."
        )));
        min_field.setContinuous(true);
        min_field.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        // SAFETY: AppDelegate implements timelineMinChanged: and outlives the field.
        unsafe {
            min_field.setTarget(Some::<&AnyObject>(self));
            min_field.setAction(Some(sel!(timelineMinChanged:)));
        }
        content.addSubview(&min_field);
        let min_hint = text_label("", 10.5, false, mtm);
        min_hint.setTextColor(Some(&theme::secondary_text()));
        min_hint.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        min_hint.setFrame(rect(370.0, filter_y + 5.0, 140.0, 16.0));
        min_hint.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        content.addSubview(&min_hint);
        dot(518.0);
        let kind_popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(530.0, filter_y - 1.0, 118.0, 28.0),
            false,
        );
        for title in ["All types", "Prompts", "Responses", "Tools", "Errors"] {
            kind_popup.addItemWithTitle(&NSString::from_str(title));
        }
        kind_popup.setFont(Some(&theme::ui_font(theme::CONTROL_SIZE, false)));
        kind_popup.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        // SAFETY: AppDelegate implements timelineKindChanged: and outlives the menu.
        unsafe {
            kind_popup.setTarget(Some::<&AnyObject>(self));
            kind_popup.setAction(Some(sel!(timelineKindChanged:)));
        }
        content.addSubview(&kind_popup);
        // The models the session used fill in once the journal is read.
        dot(662.0);
        let model_popup = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(674.0, filter_y - 1.0, 150.0, 28.0),
            false,
        );
        model_popup.addItemWithTitle(ns_string!("All models"));
        model_popup.setFont(Some(&theme::ui_font(theme::CONTROL_SIZE, false)));
        model_popup.setToolTip(Some(ns_string!(
            "Only what one model did: its responses and the tool calls they made, in the session or a sub-agent."
        )));
        model_popup.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        // SAFETY: AppDelegate implements timelineModelChanged: and outlives the menu.
        unsafe {
            model_popup.setTarget(Some::<&AnyObject>(self));
            model_popup.setAction(Some(sel!(timelineModelChanged:)));
        }
        content.addSubview(&model_popup);
        // How much of the list is showing, at the end of the totals line.
        let note = text_label("", 11.0, false, mtm);
        note.setTextColor(Some(&theme::secondary_text()));
        note.setAlignment(NSTextAlignment::Right);
        note.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        note.setFrame(rect(width - 26.0 - 160.0, height - 64.0, 160.0, 16.0));
        note.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&note);

        // The list in a card, with the minimap in a gutter at its right.
        const MINIMAP_WIDTH: f64 = 12.0;
        const GUTTER: f64 = 22.0;
        let list_top = height - 112.0;
        let card = card_box(rect(24.0, 62.0, width - 48.0, list_top - 62.0), 10.0, mtm);
        card.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&card);
        let frame = rect(25.0, 63.0, width - 50.0 - GUTTER, list_top - 64.0);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        // Not flipped, so older rows arriving above grow the document at its
        // top and leave the rows in view where they are.
        let document = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, frame.size.width, frame.size.height),
        );
        scroll.setDocumentView(Some(&document));
        content.addSubview(&scroll);
        let list = Rc::new(timeline_ui::TimelineList::new(document, self));
        let clip = scroll.contentView();
        clip.setPostsBoundsChangedNotifications(true);
        clip.setPostsFrameChangedNotifications(true);
        // SAFETY: the delegate implements every selector and outlives the
        // views; the notification names are AppKit statics.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(timelineDidScroll:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
            );
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(timelineFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&clip),
            );
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(timelineWindowWillClose:),
                Some(NSWindowWillCloseNotification),
                Some(&window),
            );
        }
        let minimap = TimelineMinimapView::new(
            rect(
                width - 25.0 - GUTTER + (GUTTER - MINIMAP_WIDTH) / 2.0,
                69.0,
                MINIMAP_WIDTH,
                list_top - 76.0,
            ),
            self,
            mtm,
        );
        minimap.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        minimap.setToolTip(Some(ns_string!(
            "Every event down the session, longer bars for slower ones. Click to jump to the slowest event nearby."
        )));
        content.addSubview(&minimap);

        // SAFETY: AppDelegate implements closeTimeline: and outlives the window.
        let close = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Close"),
                Some::<&AnyObject>(self),
                Some(sel!(closeTimeline:)),
                mtm,
            )
        };
        close.setBezelStyle(NSBezelStyle::Push);
        close.setFrame(rect(width - 24.0 - 88.0, 18.0, 88.0, 30.0));
        close.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        content.addSubview(&close);
        // Across the footer from Close: a spinner and what it is doing while
        // the journal is read and older events arrive.
        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            rect(28.0, 27.0, 12.0, 12.0),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setIndeterminate(true);
        spinner.setControlSize(NSControlSize::Mini);
        spinner.setDisplayedWhenStopped(false);
        spinner.setHidden(true);
        spinner.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        content.addSubview(&spinner);
        let progress = text_label("", 11.0, false, mtm);
        progress.setTextColor(Some(&theme::secondary_text()));
        progress.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        progress.setFrame(rect(46.0, 25.0, width - 46.0 - 24.0 - 88.0 - 16.0, 16.0));
        progress.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        progress.setHidden(true);
        content.addSubview(&progress);
        let escape = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Close"),
                Some::<&AnyObject>(self),
                Some(sel!(closeTimeline:)),
                mtm,
            )
        };
        escape.setFrame(rect(0.0, 0.0, 0.0, 0.0));
        escape.setKeyEquivalent(ns_string!("\u{1b}"));
        escape.setTransparent(true);
        content.addSubview(&escape);

        let ivars = self.ivars();
        ivars.timeline_scroll.replace(Some(scroll));
        ivars.timeline_list.replace(Some(list));
        ivars.timeline_progress.replace(Some((spinner, progress)));
        ivars.timeline_minimap.replace(Some(minimap));
        ivars.timeline_subtitle.replace(Some(subtitle));
        ivars.timeline_search.replace(Some(search));
        ivars.timeline_min_field.replace(Some(min_field));
        ivars.timeline_min_hint.replace(Some(min_hint));
        ivars.timeline_kind_popup.replace(Some(kind_popup));
        ivars.timeline_model_popup.replace(Some(model_popup));
        ivars.timeline_filter_note.replace(Some(note));
        ivars.timeline_filter_dots.replace(dots);
        ivars.timeline_filter.replace(timeline::Filter::default());
        ivars.timeline_open_runs.borrow_mut().clear();
        // Where the user last left it; the first time, over the main window.
        if !window.setFrameUsingName(ns_string!("OmperorTimeline")) {
            window.center();
        }
        window.setFrameAutosaveName(ns_string!("OmperorTimeline"));
        window.makeKeyAndOrderFront(None);
        ivars.timeline_window.replace(Some(window));
    }

    /// The fold landed with the newest events: the whole session's totals
    /// line, the minimum hint, and the models, which older pages leave as
    /// they are, then the rows, scrolled to the newest at the bottom as the
    /// chat is.
    fn start_timeline_list(&self, summary: timeline::Summary, newest: Vec<timeline::Event>) {
        if let Some(subtitle) = self.ivars().timeline_subtitle.borrow().as_ref() {
            let (responses, tools) = (summary.responses, summary.tools);
            subtitle.setStringValue(&NSString::from_str(&if summary.events == 0 {
                "Nothing has happened in this session yet.".to_owned()
            } else {
                format!(
                    "{} over {} · {} response{} · {} tool call{}{} · longest {}",
                    summary.events,
                    duration_text(summary.span_ms),
                    responses,
                    if responses == 1 { "" } else { "s" },
                    tools,
                    if tools == 1 { "" } else { "s" },
                    match summary.agents {
                        0 => String::new(),
                        1 => " · 1 sub-agent, shown one level in".to_owned(),
                        n => format!(" · {n} sub-agents, shown one level in"),
                    },
                    duration_text(summary.longest_ms),
                )
            }));
        }
        if let Some(hint) = self.ivars().timeline_min_hint.borrow().as_ref() {
            let text = match summary.p90_ms {
                Some(p90) => format!(
                    "p90 {} · max {}",
                    duration_text(p90),
                    duration_text(summary.longest_ms)
                ),
                None => String::new(),
            };
            hint.setStringValue(&NSString::from_str(&text));
        }
        if let Some(popup) = self.ivars().timeline_model_popup.borrow().as_ref() {
            popup.removeAllItems();
            popup.addItemWithTitle(ns_string!("All models"));
            for model in &summary.models {
                popup.addItemWithTitle(&NSString::from_str(model));
            }
        }
        let (arrived, total) = (newest.len(), summary.events);
        let list = self.ivars().timeline_list.borrow().clone();
        if let Some(list) = list {
            list.set_loaded(timeline::Loaded::new(summary, newest));
        }
        self.refresh_timeline_rows();
        if let Some(scroll) = self.ivars().timeline_scroll.borrow().clone() {
            let clip = scroll.contentView();
            clip.scrollToPoint(NSPoint::new(0.0, 0.0));
            scroll.reflectScrolledClipView(&clip);
        }
        self.sync_timeline_viewport();
        if arrived < total {
            self.show_timeline_progress(Some(&format!(
                "Loading earlier events · {arrived} of {total}"
            )));
        }
    }

    /// Reads the filter controls into the filter and lays the rows out
    /// again, with a gold dot beside each control that is narrowing the
    /// list and the note saying how much of it is shown.
    fn apply_timeline_filter(&self) {
        let ivars = self.ivars();
        let query = ivars
            .timeline_search
            .borrow()
            .as_ref()
            .map(|search| search.stringValue().to_string())
            .unwrap_or_default();
        let min_text = ivars
            .timeline_min_field
            .borrow()
            .as_ref()
            .map(|field| field.stringValue().to_string())
            .unwrap_or_default();
        let min_ms = timeline::parse_duration_ms(&min_text);
        if let Some(field) = ivars.timeline_min_field.borrow().as_ref() {
            // A minimum that cannot be read is said so, and ignored.
            let ink = if min_ms.is_none() {
                NSColor::systemRedColor()
            } else {
                NSColor::textColor()
            };
            field.setTextColor(Some(&ink));
        }
        let kind = ivars
            .timeline_kind_popup
            .borrow()
            .as_ref()
            .and_then(|popup| match popup.indexOfSelectedItem() {
                1 => Some(timeline::Kind::Prompt),
                2 => Some(timeline::Kind::Response),
                3 => Some(timeline::Kind::Tool),
                4 => Some(timeline::Kind::Notice),
                _ => None,
            });
        let model = ivars
            .timeline_model_popup
            .borrow()
            .as_ref()
            .filter(|popup| popup.indexOfSelectedItem() > 0)
            .and_then(|popup| popup.titleOfSelectedItem())
            .map(|title| title.to_string());
        let filter = timeline::Filter {
            query,
            min_ms: min_ms.unwrap_or(0),
            kind,
            model,
        };
        let active = [
            filter.query_active(),
            filter.min_ms > 0,
            filter.kind.is_some(),
            filter.model.is_some(),
        ];
        for (dot, on) in ivars.timeline_filter_dots.borrow().iter().zip(active) {
            dot.setHidden(!on);
        }
        ivars.timeline_filter.replace(filter);
        self.drop_timeline_flash();
        self.refresh_timeline_rows();
    }

    /// Recomputes the rows for what has arrived under the filter and lays
    /// them out; the minimap and the note follow.
    fn refresh_timeline_rows(&self) {
        let ivars = self.ivars();
        let Some(list) = ivars.timeline_list.borrow().clone() else {
            return;
        };
        let (rows, marks, arrived, total) = {
            let loaded = list.loaded();
            let rows = timeline::rows(
                &loaded,
                &ivars.timeline_filter.borrow(),
                &ivars.timeline_open_runs.borrow(),
            );
            let marks = timeline::marks(&loaded, &rows);
            (rows, marks, loaded.len(), loaded.summary.events)
        };
        let shown = rows
            .iter()
            .filter(|row| matches!(row, timeline::Row::Event { dimmed: false, .. }))
            .count();
        if let Some(note) = ivars.timeline_filter_note.borrow().as_ref() {
            let filtering = ivars.timeline_filter.borrow().is_active();
            let reading = ivars.timeline_stream.borrow().is_some() && arrived == 0;
            let (text, ink) = if reading {
                (String::new(), theme::secondary_text())
            } else if filtering {
                // Of what has arrived: older pages may add to both.
                (format!("{shown} of {arrived} events"), theme::accent())
            } else {
                (
                    format!("{total} event{}", if total == 1 { "" } else { "s" }),
                    theme::secondary_text(),
                )
            };
            note.setStringValue(&NSString::from_str(&text));
            note.setTextColor(Some(&ink));
        }
        if let Some(minimap) = ivars.timeline_minimap.borrow().as_ref() {
            minimap.set_marks(marks);
        }
        list.set_rows(rows);
        self.layout_timeline_rows();
        self.sync_timeline_viewport();
    }

    /// Sizes the list to its rows and the window, and puts views on the
    /// rows in view.
    fn layout_timeline_rows(&self) {
        let ivars = self.ivars();
        if ivars.timeline_laying_out.get() {
            ivars.timeline_relayout_needed.set(true);
            return;
        }
        ivars.timeline_laying_out.set(true);
        let (scroll, list) = (
            ivars.timeline_scroll.borrow().clone(),
            ivars.timeline_list.borrow().clone(),
        );
        if let (Some(scroll), Some(list)) = (scroll, list) {
            list.layout(&scroll.contentView());
        }
        ivars.timeline_laying_out.set(false);
        if ivars.timeline_relayout_needed.replace(false) {
            self.layout_timeline_rows();
        }
    }

    /// Puts views on the rows the scroller has brought into view.
    fn sync_timeline_rows(&self) {
        let (scroll, list) = (
            self.ivars().timeline_scroll.borrow().clone(),
            self.ivars().timeline_list.borrow().clone(),
        );
        if let (Some(scroll), Some(list)) = (scroll, list) {
            list.sync(&scroll.contentView());
        }
    }

    /// Opens a sheet on the timeline window with everything the journal
    /// recorded about event `index`: its facts, then the texts behind it,
    /// selectable so they can be copied.
    fn present_timeline_event(&self, index: usize) {
        let ivars = self.ivars();
        let (Some(window), Some(list)) = (
            ivars.timeline_window.borrow().clone(),
            ivars.timeline_list.borrow().clone(),
        ) else {
            return;
        };
        // A copy, so no borrow of the list is held while the sheet is built.
        let (event, origin_ms) = {
            let loaded = list.loaded();
            let Some(event) = loaded.get(index).cloned() else {
                return;
            };
            (event, loaded.summary.origin_ms)
        };
        self.dismiss_timeline_detail();
        let mtm = self.mtm();
        let (width, height) = (680.0, 560.0);
        // SAFETY: the sheet is retained in an ivar until it is ended, and opts
        // out of AppKit's legacy release-on-close behavior.
        let sheet = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, width, height),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { sheet.setReleasedWhenClosed(false) };
        sheet.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        sheet.setTitlebarAppearsTransparent(true);
        sheet.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        sheet.setMinSize(NSSize::new(480.0, 320.0));
        let content = sheet.contentView().expect("sheet content view");

        let title = match event.agent.as_deref() {
            Some(agent) => format!("{agent} · {}", event.label),
            None => event.label.clone(),
        };
        let heading = text_label(&title, 16.0, true, mtm);
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        heading.setFont(Some(&NSFont::systemFontOfSize_weight(17.0, unsafe {
            NSFontWeightSemibold
        })));
        heading.setFrame(rect(26.0, height - 46.0, width - 52.0, 24.0));
        heading.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        heading.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&heading);
        let when = format!(
            "+{} from the session's start · {} · {}",
            offset_text(event.started_ms.saturating_sub(origin_ms)),
            format_timestamp(i64::try_from(event.started_ms).unwrap_or(i64::MAX)),
            match event.kind {
                timeline::Kind::Prompt => "prompt".to_owned(),
                timeline::Kind::Response => "response".to_owned(),
                timeline::Kind::Tool => "tool call".to_owned(),
                timeline::Kind::Notice => "error".to_owned(),
            }
        );
        let subtitle = text_label(&when, 11.0, false, mtm);
        subtitle.setTextColor(Some(&theme::secondary_text()));
        subtitle.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        subtitle.setFrame(rect(26.0, height - 64.0, width - 52.0, 16.0));
        subtitle.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        content.addSubview(&subtitle);

        let card = card_box(rect(24.0, 62.0, width - 48.0, height - 134.0), 10.0, mtm);
        card.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview(&card);
        let frame = rect(25.0, 63.0, width - 50.0, height - 136.0);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(true);
        text.setDrawsBackground(false);
        text.setTextContainerInset(NSSize::new(16.0, 14.0));
        text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        // Wrap to the view's width, however it is resized.
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        // SAFETY: the container and storage belong to this text view, which
        // is alive and on the main thread for the whole call.
        unsafe {
            if let Some(container) = text.textContainer() {
                container.setWidthTracksTextView(true);
            }
            if let Some(storage) = text.textStorage() {
                storage.setAttributedString(&timeline_event_text(&event.info));
            }
        }
        scroll.setDocumentView(Some(&text));
        content.addSubview(&scroll);

        // SAFETY: AppDelegate implements closeTimelineDetail: and outlives the sheet.
        let close = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Close"),
                Some::<&AnyObject>(self),
                Some(sel!(closeTimelineDetail:)),
                mtm,
            )
        };
        close.setBezelStyle(NSBezelStyle::Push);
        close.setFrame(rect(width - 24.0 - 88.0, 18.0, 88.0, 30.0));
        close.setKeyEquivalent(ns_string!("\r"));
        close.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        content.addSubview(&close);
        let escape = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Close"),
                Some::<&AnyObject>(self),
                Some(sel!(closeTimelineDetail:)),
                mtm,
            )
        };
        escape.setFrame(rect(0.0, 0.0, 0.0, 0.0));
        escape.setKeyEquivalent(ns_string!("\u{1b}"));
        escape.setTransparent(true);
        content.addSubview(&escape);

        window.beginSheet_completionHandler(&sheet, None);
        ivars.timeline_detail_sheet.replace(Some(sheet));
    }

    fn dismiss_timeline_detail(&self) {
        let Some(sheet) = self.ivars().timeline_detail_sheet.borrow_mut().take() else {
            return;
        };
        if let Some(window) = self.ivars().timeline_window.borrow().as_ref() {
            window.endSheet(&sheet);
        }
        sheet.orderOut(None);
    }

    /// Tells the minimap which rows are in view.
    fn sync_timeline_viewport(&self) {
        let (Some(scroll), Some(minimap)) = (
            self.ivars().timeline_scroll.borrow().clone(),
            self.ivars().timeline_minimap.borrow().clone(),
        ) else {
            return;
        };
        let Some(document) = scroll.documentView() else {
            return;
        };
        let total = document.frame().size.height.max(1.0);
        let visible = scroll.contentView().bounds();
        let top = total - (visible.origin.y + visible.size.height);
        minimap.set_viewport(top / total, (top + visible.size.height) / total);
    }

    /// Scrolls the list so `row` sits in the middle and flashes it.
    fn jump_timeline_to_row(&self, row: usize) {
        let (Some(scroll), Some(list)) = (
            self.ivars().timeline_scroll.borrow().clone(),
            self.ivars().timeline_list.borrow().clone(),
        ) else {
            return;
        };
        let Some(document) = scroll.documentView() else {
            return;
        };
        let Some((top, row_height)) = list.row_span(row) else {
            return;
        };
        let mtm = self.mtm();
        let total = document.frame().size.height;
        let visible = scroll.contentView().bounds().size.height;
        let row_top = total - top;
        let target_y =
            (row_top - visible / 2.0 - row_height / 2.0).clamp(0.0, (total - visible).max(0.0));
        let clip = scroll.contentView();
        clip.scrollToPoint(NSPoint::new(0.0, target_y));
        scroll.reflectScrolledClipView(&clip);
        self.sync_timeline_rows();
        self.sync_timeline_viewport();
        // The row lights up and fades, so the eye finds where it landed.
        self.drop_timeline_flash();
        let flash = fill_box(
            rect(
                4.0,
                row_top - row_height + 2.0,
                document.frame().size.width - 8.0,
                row_height - 4.0,
            ),
            &theme::accent_fill_strong(),
            8.0,
            mtm,
        );
        document.addSubview_positioned_relativeTo(&flash, NSWindowOrderingMode::Below, None);
        flash.setAlphaValue(1.0);
        motion::animate(motion::FLASH_HOLD, || {
            flash.animator().setAlphaValue(0.0);
        });
        self.ivars().timeline_flash.replace(Some(flash));
    }

    /// Takes away the pill a jump left, once the rows it marked move.
    fn drop_timeline_flash(&self) {
        if let Some(flash) = self.ivars().timeline_flash.borrow_mut().take() {
            flash.removeFromSuperview();
        }
    }

    /// Answers a press on a session row, or lets an unfulfilled one go.
    ///
    /// The row tints with a share of the gold it would get if chosen, and a
    /// seed of full gold appears at its edge. One tint and one seed serve
    /// every row: a press is momentary, so the list does not carry a pair of
    /// views per row for it. If the click goes through, the rebuild grows the
    /// selection pill out of this tint and sweeps the seed into it.
    fn wash_sidebar_row(&self, frame: Option<NSRect>) {
        let ivars = self.ivars();
        let Some(frame) = frame else {
            let (wash, seed) = (
                ivars.sidebar_press_wash.borrow().clone(),
                ivars.sidebar_press_seed.borrow().clone(),
            );
            motion::animate(motion::FADE_OUT, || {
                if let Some(wash) = &wash {
                    wash.animator().setAlphaValue(0.0);
                }
                if let Some(seed) = &seed {
                    seed.animator().setAlphaValue(0.0);
                }
            });
            return;
        };
        let Some(container) = ivars.sidebar_sessions_view.get() else {
            return;
        };
        let mtm = self.mtm();
        let wash = ivars
            .sidebar_press_wash
            .borrow_mut()
            .get_or_insert_with(|| {
                let wash = fill_box(frame, &theme::accent_fill_strong(), 8.0, mtm);
                wash.setWantsLayer(true);
                wash.setAlphaValue(0.0);
                // Under the rows, so it tints them rather than covering them.
                container.addSubview_positioned_relativeTo(
                    &wash,
                    NSWindowOrderingMode::Below,
                    None,
                );
                wash
            })
            .clone();
        let seed = ivars
            .sidebar_press_seed
            .borrow_mut()
            .get_or_insert_with(|| {
                let seed = fill_box(seed_frame(frame), &theme::accent(), 1.5, mtm);
                seed.setWantsLayer(true);
                seed.setAlphaValue(0.0);
                // Over the tint, so the seed reads on it.
                container.addSubview_positioned_relativeTo(
                    &seed,
                    NSWindowOrderingMode::Above,
                    Some(&wash),
                );
                seed
            })
            .clone();
        wash.setFrame(frame);
        seed.setFrame(seed_frame(frame));
        motion::animate(motion::PRESS_IN, || {
            wash.animator().setAlphaValue(motion::PRESS_TINT);
            seed.animator().setAlphaValue(1.0);
        });
    }

    /// Moves the gold pill behind the selected change row, or removes it.
    fn highlight_change_row(&self, index: Option<usize>) {
        let ivars = self.ivars();
        let Some(container) = ivars.change_list_view.get() else {
            return;
        };
        if let Some(pill) = ivars.change_selection_pill.borrow_mut().take() {
            pill.removeFromSuperview();
        }
        let Some(index) = index else {
            return;
        };
        let rows = ivars.change_rows.borrow();
        // Rows are stored as (button, counts) pairs after any note.
        let Some(row) = rows.get(index * 2) else {
            return;
        };
        let frame = row.frame();
        let width = container.frame().size.width;
        let pill = fill_box(
            rect(4.0, frame.origin.y, width - 8.0, INSPECTOR_ROW_HEIGHT),
            &theme::accent_fill(),
            7.0,
            self.mtm(),
        );
        pill.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        // Behind the rows: added below every sibling.
        container.addSubview_positioned_relativeTo(&pill, NSWindowOrderingMode::Below, None);
        drop(rows);
        ivars.change_selection_pill.replace(Some(pill));
    }

    /// Sets the three summary rows' values.
    fn set_inspector_rows(&self, branch: &str, upstream: &str, commit: &str) {
        let ivars = self.ivars();
        for (slot, value) in [
            (&ivars.inspector_branch, branch),
            (&ivars.inspector_upstream, upstream),
            (&ivars.inspector_commit, commit),
        ] {
            if let Some(label) = slot.get() {
                label.setStringValue(&NSString::from_str(value));
                label.setToolTip(Some(&NSString::from_str(value)));
            }
        }
    }

    fn set_change_section(&self, count: usize, totals: Option<(u64, u64)>) {
        let ivars = self.ivars();
        if let Some(label) = ivars.change_section_label.get() {
            let text = if count > 0 {
                format!("Changes · {count}")
            } else {
                "Changes".to_owned()
            };
            // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
            let font = NSFont::systemFontOfSize_weight(12.0, unsafe { NSFontWeightSemibold });
            label.setAttributedStringValue(&attributed(
                &text,
                &font,
                &theme::secondary_text(),
                0.0,
            ));
        }
        if let Some(label) = ivars.change_section_totals.get() {
            match totals {
                Some((added, removed)) => label.setAttributedStringValue(&aligned_right(
                    &change_counts_attributed(Some(added), Some(removed), 11.0),
                )),
                None => label.setStringValue(ns_string!("")),
            }
        }
    }

    fn set_diff_title(&self, change: Option<&workspace::WorkspaceChange>) {
        let Some(label) = self.ivars().diff_title.get() else {
            return;
        };
        let Some(change) = change else {
            label.setStringValue(ns_string!(""));
            return;
        };
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        let font = NSFont::systemFontOfSize_weight(12.0, unsafe { NSFontWeightSemibold });
        let title = attributed(
            &change.path.display().to_string(),
            &font,
            &NSColor::labelColor(),
            0.0,
        );
        title.appendAttributedString(&attributed(
            &format!("   {}   ", change.kind()),
            &theme::ui_font(theme::CAPTION_SIZE, false),
            &theme::secondary_text(),
            0.0,
        ));
        title.appendAttributedString(&change_counts_attributed(
            change.insertions,
            change.deletions,
            11.0,
        ));
        label.setAttributedStringValue(&title);
        label.setToolTip(Some(&NSString::from_str(
            &change.path.display().to_string(),
        )));
    }

    /// Opens the full payload of an activity row (tool call, reasoning, or a
    /// replayed event) in a sheet over the window: the title in the display
    /// face, the payload on a card, and Close (Return or Escape) to dismiss.
    /// A sheet, unlike an alert, leaves the rest of the app responsive and
    /// looks like part of it.
    fn present_activity_detail(&self, id: usize) {
        let owner = self.session();
        let mtm = self.mtm();
        let Some(detail) = owner.conversation.borrow().detail(id) else {
            return;
        };
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        if self.ivars().activity_sheet.borrow().is_some() {
            return;
        }
        let (width, height) = (760.0, 560.0);
        // SAFETY: the sheet is retained in an ivar until it is ended, and opts
        // out of AppKit's legacy release-on-close behavior.
        let sheet = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, width, height),
                NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { sheet.setReleasedWhenClosed(false) };
        sheet.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        sheet.setTitlebarAppearsTransparent(true);
        sheet.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        let content = sheet.contentView().expect("sheet content view");

        let title = text_label(&detail.title, 16.0, true, mtm);
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        title.setFont(Some(&NSFont::systemFontOfSize_weight(17.0, unsafe {
            NSFontWeightSemibold
        })));
        title.setFrame(rect(26.0, height - 48.0, width - 52.0, 24.0));
        title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        content.addSubview(&title);

        let card = card_box(rect(24.0, 62.0, width - 48.0, height - 124.0), 10.0, mtm);
        content.addSubview(&card);
        let frame = rect(25.0, 63.0, width - 50.0, height - 126.0);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(true);
        text.setDrawsBackground(false);
        text.setTextContainerInset(NSSize::new(16.0, 14.0));
        text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let rendered = native_markdown::render_detail(&detail.sections, 1.0);
        // SAFETY: NSTextView owns its text storage for its lifetime.
        if let Some(storage) = unsafe { text.textStorage() } {
            storage.setAttributedString(&rendered);
        }
        scroll.setDocumentView(Some(&text));
        content.addSubview(&scroll);

        // SAFETY: AppDelegate implements closeActivityDetail: and outlives the sheet.
        let close = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Close"),
                Some::<&AnyObject>(self),
                Some(sel!(closeActivityDetail:)),
                mtm,
            )
        };
        close.setBezelStyle(NSBezelStyle::Push);
        close.setFrame(rect(width - 24.0 - 88.0, 18.0, 88.0, 30.0));
        close.setKeyEquivalent(ns_string!("\r"));
        content.addSubview(&close);
        // Escape closes too, through a second, invisible button (named so
        // assistive technology reads it sensibly).
        let escape = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Close"),
                Some::<&AnyObject>(self),
                Some(sel!(closeActivityDetail:)),
                mtm,
            )
        };
        escape.setFrame(rect(0.0, 0.0, 0.0, 0.0));
        escape.setKeyEquivalent(ns_string!("\u{1b}"));
        escape.setTransparent(true);
        content.addSubview(&escape);

        window.beginSheet_completionHandler(&sheet, None);
        self.ivars().activity_sheet.replace(Some(sheet));
    }

    fn refresh_chat_transcript(&self) {
        let owner = self.session();
        let transcript = self.transcript();
        let image_base = owner.chat_image_base
            .borrow()
            .clone()
            .unwrap_or_else(|| owner.current_project.borrow().clone());
        let settings = {
            let preferences = self.ivars().preferences.borrow();
            transcript_stack::Settings {
                typography: native_markdown::Typography::new(
                    preferences.chat_style,
                    preferences.chat_text_size.scale(),
                )
                .with_sizes(preferences.chat_text_sizes),
                image_base,
                show_inline_images: preferences.show_inline_images,
            }
        };
        self.ivars().chat_dirty.set(false);
        // History loaded above the rows keeps the reader's place: the
        // transcript holds the first row in view where it was.
        owner.history_prepend_pending.set(false);
        transcript.sync(&mut owner.conversation.borrow_mut(), settings);
        self.restore_conversation_position_if_ready();
    }

    /// Once due: the transcript's heights, handed to the job pool to write
    /// beside the session's journal.
    fn save_heights_if_due(&self) {
        let due = SAVED_HEIGHTS.with(|saved| {
            let mut saved = saved.borrow_mut();
            let journal = saved.0.clone().filter(|_| saved.1.is_some_and(|at| std::time::Instant::now() >= at))?;
            saved.1 = None;
            Some(journal)
        });
        let Some(journal) = due else { return };
        let Some(heights) = self.ivars().chat_transcript.get().and_then(|transcript| transcript.saved_heights()) else { return };
        self.jobs().submit(move || BackgroundEvent::HeightsSaved(saved_heights::write(&journal, &heights).map_err(|error| error.to_string())));
    }

    fn transcript(&self) -> Rc<transcript_stack::Transcript> {
        self.ivars()
            .chat_transcript
            .get()
            .expect("chat transcript must exist before rendering")
            .clone()
    }

    fn mark_chat_dirty(&self) {
        self.ivars().chat_dirty.set(true);
    }

    fn view_summary(&self, summary: session::SessionSummary) {
        if summary.is_mock() {
            return;
        }
        self.load_summary(summary, true);
    }

    fn resume_summary(&self, summary: session::SessionSummary) {
        let owner = self.session();
        if let Some(project) = summary.cwd.clone() {
            owner.current_project.replace(project);
        }
        self.activate_project_for_session(&summary);
        owner.live_session_id.replace(Some(summary.id.clone()));
        self.resolve_session_account(&owner);
        self.load_summary(summary, false);
        self.set_chat_status("Ready to continue • sending starts this session's runtime");
    }

    fn load_summary(&self, summary: session::SessionSummary, read_only: bool) {
        self.load_summary_from(summary, read_only, None);
    }

    /// Opens a session: its history streams in pages, newest first, from
    /// its file on a worker — or, for the headless benchmark, from
    /// `messages`, paged the same way.
    fn load_summary_from(
        &self,
        summary: session::SessionSummary,
        read_only: bool,
        messages: Option<Vec<session::SessionMessage>>,
    ) {
        let owner = self.session();
        self.reset_session_metrics();
        // Clear before restoring: the saved desk carries the session's unsent
        // attachments, and clearing afterwards discarded them for good.
        self.clear_pending_attachments();
        self.restore_session_desk();
        self.set_header_session(Some(&summary.id), Some(summary.path.clone()));
        let image_base = summary
            .cwd
            .clone()
            .unwrap_or_else(|| owner.current_project.borrow().clone());
        owner.session_history.borrow_mut().take();
        owner.prompt_history.borrow_mut().clear();
        owner.subagents.borrow_mut().clear();
        owner.read_only_session.set(read_only);
        owner.history_prepend_pending.set(false);
        owner.chat_image_base.replace(Some(image_base));
        self.ivars()
            .center_title
            .get()
            .expect("center title exists before viewing a session")
            .setStringValue(&NSString::from_str(&summary.title));
        owner.conversation
            .borrow_mut()
            .reset("Loading the most recent messages…");
        self.transcript().reset();
        self.transcript().set_history_loading(true);
        // A session read from its journal keeps its heights beside it.
        let journal = messages.is_none().then(|| summary.path.clone());
        SAVED_HEIGHTS.with(|saved| *saved.borrow_mut() = (journal, None));
        self.refresh_chat_transcript();
        self.set_chat_status(if read_only {
            "Read-only OMP session • loading recent messages…"
        } else {
            "Omperor session • loading recent messages…"
        });
        self.update_chat_controls();
        // The sidebar refreshed the panel before this session had a title, so
        // its root row would otherwise keep naming the session just left.
        self.request_agents_refresh();
        let (sender, receiver) = flume::bounded(1);
        owner.session_history
            .replace(Some(SessionHistoryLoad {
                receiver,
                message_count: 0,
            }));
        std::thread::Builder::new()
            .name("omperor-session-history".into())
            .spawn(move || match messages {
                Some(messages) => session::stream_messages(messages, sender),
                None => {
                    // The heights first, so the first page is laid out at them.
                    if let Some(saved) = saved_heights::read(&summary.path) {
                        let _ = sender.send(session::HistoryUpdate::Heights(saved));
                    }
                    session::stream_history(summary, sender)
                }
            })
            .expect("session history worker must start");
    }

    /// The sidebar's search query, lowercased, and its source filter.
    fn sidebar_filter(&self) -> (String, SessionSourceFilter) {
        let query = self
            .ivars()
            .session_search
            .get()
            .map(|search| search.stringValue().to_string().to_lowercase())
            .unwrap_or_default();
        let source_filter = self
            .ivars()
            .session_source_filter
            .get()
            .map(|filter| SessionSourceFilter::from_segment(filter.selectedSegment()))
            .unwrap_or_default();
        (query, source_filter)
    }

    /// The rows the sidebar shows now, folded as drawn: plain data, cheap to
    /// compare with the rows last drawn.
    fn sidebar_rows_now(&self, query: &str, source_filter: SessionSourceFilter) -> Vec<SidebarRow> {
        let owner = self.session();
        let sessions = self.ivars().sessions.borrow();
        let rows = sidebar_rows_for(
            &self.ivars().projects.borrow(),
            &sessions,
            &self.ivars().session_records.borrow(),
            &self.ivars().session_positions.borrow(),
            source_filter,
            query,
        );
        if query.is_empty() && source_filter == SessionSourceFilter::Omperor {
            let selected = owner.selected_session_path.borrow();
            let keep_visible = sessions.iter().enumerate().filter_map(|(i, summary)| {
                (selected.as_deref() == Some(summary.path.as_path()) || self.session_state(summary).is_busy()).then_some(i)
            }).collect();
            sidebar::fold_rows(rows, &self.ivars().sidebar_expanded_projects.borrow(), &keep_visible)
        } else {
            rows
        }
    }

    /// At a session row's end: a spinner while the session is executing,
    /// else a dot for one waiting on the user or ended badly.
    fn add_sidebar_indicator(&self, container: &NSView, state: &agents::AgentState, title: &str, y: f64) -> Option<SidebarIndicator> {
        let mtm = self.mtm();
        if state.is_busy() {
            let spinner = row_spinner(rect(SIDEBAR_WIDTH - 29.5, y + 11.0, 12.0, 12.0), mtm);
            spinner.setAccessibilityLabel(Some(&NSString::from_str(&format!("Running: {title}"))));
            spinner.setToolTip(Some(ns_string!("This session is working")));
            container.addSubview(&spinner);
            self.ivars().sidebar_spinners.borrow_mut().push(spinner.clone());
            Some(SidebarIndicator::Spinner(spinner))
        } else if let Some(dot) = sidebar_dot(state) {
            let dot = agent_dot(dot, rect(SIDEBAR_WIDTH - 27.0, y + 13.5, 7.0, 7.0), mtm);
            container.addSubview(&dot);
            self.ivars().sidebar_status_dots.borrow_mut().push(dot.clone());
            Some(SidebarIndicator::Dot(dot))
        } else {
            None
        }
    }

    fn remove_sidebar_indicator(&self, indicator: SidebarIndicator) {
        match indicator {
            SidebarIndicator::Spinner(spinner) => {
                // SAFETY: stopAnimation: takes an optional sender and makes no other demand.
                unsafe { spinner.stopAnimation(None) };
                spinner.removeFromSuperview();
                self.ivars().sidebar_spinners.borrow_mut().retain(|kept| !std::ptr::eq(Retained::as_ptr(kept), Retained::as_ptr(&spinner)));
            }
            SidebarIndicator::Dot(dot) => {
                dot.removeFromSuperview();
                self.ivars().sidebar_status_dots.borrow_mut().retain(|kept| !std::ptr::eq(Retained::as_ptr(kept), Retained::as_ptr(&dot)));
            }
        }
    }

    /// Brings the sidebar's state indicators up to date. While the rows
    /// themselves are unchanged, only the rows whose state changed are
    /// touched: the indicator swapped and the tooltip rewritten. A change of
    /// rows — a busy session kept visible in a folded project, say — rebuilds
    /// the list instead. Returns whether it rebuilt.
    fn refresh_sidebar_states(&self) -> bool {
        let _span = perf::span("gui.sidebar_states");
        self.merge_retained_session_rows();
        // A drag holds live buttons; the rebuild after the drop catches up.
        if self.ivars().sidebar_drag_active.get() {
            return false;
        }
        let Some(container) = self.ivars().sidebar_sessions_view.get() else {
            return false;
        };
        let (query, source_filter) = self.sidebar_filter();
        let rows = self.sidebar_rows_now(&query, source_filter);
        let unchanged = *self.ivars().sidebar_rows.borrow() == rows
            && self.ivars().sidebar_row_statuses.borrow().len() == rows.len();
        if !unchanged {
            self.rebuild_sidebar_sessions();
            return true;
        }
        let sessions = self.ivars().sessions.borrow();
        let records = self.ivars().session_records.borrow();
        let mut statuses = self.ivars().sidebar_row_statuses.borrow_mut();
        for (row, status) in rows.iter().zip(statuses.iter_mut()) {
            let (SidebarRow::Session { index, .. }, Some(status)) = (row, status) else {
                continue;
            };
            let summary = &sessions[*index];
            let state = self.session_state(summary);
            if state == status.state {
                continue;
            }
            if let Some(indicator) = status.indicator.take() {
                self.remove_sidebar_indicator(indicator);
            }
            status.indicator = self.add_sidebar_indicator(container, &state, &summary.title, status.y);
            let tooltip = format!("{}\n{}", session_tooltip(summary, records.get(&summary.id)), state.label());
            status.button.setToolTip(Some(&NSString::from_str(&tooltip)));
            status.state = state;
        }
        false
    }

    /// Rebuilds the sidebar list: Omperor sessions grouped under their
    /// projects with fork lineage indented, or the flat read-only OMP list.
    fn rebuild_sidebar_sessions(&self) {
        let owner = self.session();
        let _span = perf::span("gui.rebuild_sidebar");
        // A rebuild due next pass is served by this one.
        self.ivars().sidebar_next_pass.set(false);
        self.merge_retained_session_rows();
        // A project drag moves a live button and reads it back on drop; a
        // rebuild mid-drag would remove that button, so defer until the drop.
        if self.ivars().sidebar_drag_active.get() {
            return;
        }
        let Some(container) = self.ivars().sidebar_sessions_view.get() else {
            return;
        };
        let previous_top = self.ivars().sidebar_sessions_scroll.get().map_or(0.0, |scroll| {
            (container.frame().size.height - scroll.contentView().bounds().origin.y - scroll.contentSize().height).max(0.0)
        });
        for button in self.ivars().sidebar_row_buttons.borrow_mut().drain(..) {
            button.removeFromSuperview();
        }
        for button in self.ivars().sidebar_project_buttons.borrow_mut().drain(..) {
            button.removeFromSuperview();
        }
        for button in self.ivars().sidebar_session_buttons.borrow_mut().drain(..) {
            button.removeFromSuperview();
        }
        for pill in self.ivars().sidebar_fading_pills.borrow_mut().drain(..) {
            pill.removeFromSuperview();
        }
        let bloom = self.ivars().sidebar_bloom_pending.replace(false);
        if let Some(pill) = self.ivars().sidebar_selection_pill.borrow_mut().take() {
            if bloom {
                // The session just left: its pill goes the way a press does.
                motion::animate(motion::FADE_OUT, || {
                    pill.animator().setAlphaValue(0.0);
                });
                self.ivars().sidebar_fading_pills.borrow_mut().push(pill);
            } else {
                pill.removeFromSuperview();
            }
        }
        for dot in self.ivars().sidebar_status_dots.borrow_mut().drain(..) {
            dot.removeFromSuperview();
        }
        for spinner in self.ivars().sidebar_spinners.borrow_mut().drain(..) {
            // SAFETY: startAnimation:/stopAnimation: take an optional sender and make no other demand.
            unsafe { spinner.stopAnimation(None) };
            spinner.removeFromSuperview();
        }
        let (query, source_filter) = self.sidebar_filter();
        let rows = self.sidebar_rows_now(&query, source_filter);
        let sessions = self.ivars().sessions.borrow();
        let projects = self.ivars().projects.borrow();
        let records = self.ivars().session_records.borrow();
        let mut statuses: Vec<Option<SidebarRowStatus>> = rows.iter().map(|_| None).collect();
        let session_count = rows
            .iter()
            .filter(|row| matches!(row, SidebarRow::Session { .. }))
            .count();

        let scroll = self
            .ivars()
            .sidebar_sessions_scroll
            .get()
            .expect("session scroll exists before populating sessions");
        let visible_height = scroll.contentSize().height;
        let row_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        let rows_height = f64::from(row_count) * SIDEBAR_ROW_HEIGHT + 8.0;
        self.ivars().sidebar_rows_height.set(rows_height);
        let document_height = rows_height.max(visible_height);
        container.setFrameSize(NSSize::new(SIDEBAR_WIDTH - 16.0, document_height));
        let mut buttons = Vec::new();
        let mut hover_targets = Vec::new();
        let selected = owner.selected_session_path.borrow();
        let active_project = owner.active_project_id.borrow().clone();
        let mtm = self.mtm();
        for (position, row) in rows.iter().enumerate() {
            let position_u32 = u32::try_from(position).unwrap_or(u32::MAX);
            let y = document_height - SIDEBAR_ROW_HEIGHT * (f64::from(position_u32) + 1.0);
            let tag = isize::try_from(position).unwrap_or(isize::MAX);
            match row {
                SidebarRow::Project { project_id } => {
                    let Some(project) = projects.iter().find(|project| &project.id == project_id)
                    else {
                        continue;
                    };
                    let expanded = !project.collapsed || !query.is_empty();
                    let button = SidebarProjectButton::new(
                        &project.name,
                        project_id,
                        rect(4.0, y, SIDEBAR_WIDTH - 62.0, 34.0),
                        self,
                        sel!(toggleSidebarProject:),
                        mtm,
                    );
                    button.setTag(tag);
                    button.setToolTip(Some(&NSString::from_str(&project_tooltip(project))));
                    let active = active_project.as_deref() == Some(project_id.as_str());
                    button.setAttributedTitle(&project_row_title(&project.name, active));
                    if let Some(image) = theme::project_row_symbol(expanded) {
                        button.setImage(Some(&image));
                        button.setImagePosition(NSCellImagePosition::ImageLeading);
                    }
                    button.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                        "{} {}", if expanded { "Collapse" } else { "Expand" }, project.name
                    ))));
                    if active {
                        // The folder glyph turns gold with the name.
                        button.setContentTintColor(Some(&theme::accent()));
                    }
                    // SAFETY: the menu's items target the delegate, which
                    // outlives the sidebar's buttons.
                    unsafe { button.setMenu(Some(&self.project_context_menu(project_id))) };
                    container.addSubview(&button);
                    self.ivars()
                        .sidebar_project_buttons
                        .borrow_mut()
                        .push(button);
                    let plus = icon_button(
                        "plus.circle",
                        "New chat in this project",
                        rect(SIDEBAR_WIDTH - 52.0, y + 5.0, 24.0, 24.0),
                        self,
                        sel!(newChatInProject:),
                        mtm,
                    );
                    plus.setTag(tag);
                    container.addSubview(&plus);
                    buttons.push(plus);
                }
                SidebarRow::ShowMore { project_id, hidden, expanded } => {
                    let title = if *expanded { "Show less".into() } else { format!("Show more ({hidden})") };
                    let button = navigation_button(&title, if *expanded { "chevron.up" } else { "chevron.down" },
                        rect(20.0, y, SIDEBAR_WIDTH - 48.0, 34.0), self, sel!(toggleProjectSessions:), mtm);
                    button.setTag(tag);
                    button.setFont(Some(&theme::ui_font(11.0, false)));
                    button.setContentTintColor(Some(&theme::secondary_text()));
                    let project = projects.iter().find(|p| &p.id == project_id).map_or("project", |p| p.name.as_str());
                    button.setAccessibilityLabel(Some(&NSString::from_str(&format!("{title} in {project}"))));
                    button.setToolTip(Some(ns_string!("Running and selected sessions stay visible. Search includes all sessions.")));
                    container.addSubview(&button);
                    buttons.push(button);
                }
                SidebarRow::Unfiled => {
                    let button = navigation_button(
                        "Other folders",
                        "tray",
                        rect(4.0, y, SIDEBAR_WIDTH - 28.0, 34.0),
                        self,
                        sel!(toggleSidebarProject:),
                        mtm,
                    );
                    button.setTag(tag);
                    button.setEnabled(false);
                    button.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
                    container.addSubview(&button);
                    buttons.push(button);
                }
                SidebarRow::Session { index, project_id } => {
                    let summary = &sessions[*index];
                    let record = records.get(&summary.id);
                    let actionable = summary.is_omperor_owned();
                    let indent = if source_filter == SessionSourceFilter::Omperor {
                        16.0
                    } else {
                        0.0
                    };
                    let trailing = if actionable { 30.0 } else { 0.0 };
                    let symbol = if record.is_some_and(|record| record.worktree_path.is_some()) {
                        "arrow.triangle.branch"
                    } else if summary.is_omperor_owned() {
                        "text.bubble"
                    } else {
                        "terminal"
                    };
                    let frame = rect(
                        4.0 + indent,
                        y,
                        SIDEBAR_WIDTH - 28.0 - indent - trailing,
                        34.0,
                    );
                    let tooltip = format!("{}\n{}", session_tooltip(summary, record), self.session_state(summary).label());
                    let selected_here = selected.as_deref() == Some(summary.path.as_path());
                    let pill_frame = rect(
                        frame.origin.x - 2.0,
                        y + 1.0,
                        frame.size.width + trailing + 4.0,
                        32.0,
                    );
                    let menu = actionable.then(|| self.session_context_menu(tag, summary));
                    if selected_here {
                        // A gold pill behind the open session, added before
                        // the row so it sits underneath.
                        let pill = fill_box(pill_frame, &theme::accent_fill_strong(), 8.0, mtm);
                        if bloom {
                            // Fulfil the press: the pill takes over from the
                            // tint at the tint's own strength and rises, and
                            // the seed sweeps across into it.
                            pill.setWantsLayer(true);
                            pill.setAlphaValue(motion::PRESS_TINT);
                            container.addSubview(&pill);
                            if let Some(wash) = self.ivars().sidebar_press_wash.borrow().as_ref() {
                                wash.setAlphaValue(0.0);
                            }
                            let seed = self.ivars().sidebar_press_seed.borrow().clone();
                            if let Some(seed) = &seed {
                                // The seed sweeps over the pill, not under it.
                                container.addSubview_positioned_relativeTo(
                                    seed,
                                    NSWindowOrderingMode::Above,
                                    Some(&pill),
                                );
                            }
                            motion::animate(motion::BLOOM, || {
                                pill.animator().setAlphaValue(1.0);
                                if let Some(seed) = &seed {
                                    seed.animator().setFrame(pill_frame);
                                    seed.animator().setAlphaValue(0.0);
                                }
                            });
                        } else {
                            container.addSubview(&pill);
                        }
                        self.ivars().sidebar_selection_pill.replace(Some(pill));
                    }
                    let state = self.session_state(summary);
                    let indicator = self.add_sidebar_indicator(container, &state, &summary.title, y);
                    let row_title = session_row_title(&summary.title);
                    // A session under a project is draggable to reorder within
                    // it; the flat read-only list and unfiled sessions are not.
                    if let Some(project_id) = project_id {
                        let button = SidebarSessionButton::new(
                            &summary.title,
                            symbol,
                            &summary.id,
                            project_id,
                            frame,
                            self,
                            sel!(openSidebarSession:),
                            mtm,
                        );
                        button.setTag(tag);
                        button.setToolTip(Some(&NSString::from_str(&tooltip)));
                        button.setAttributedTitle(&row_title);
                        button.ivars().pill_frame.set(pill_frame);
                        if selected_here {
                            // The gold rides the pill and the row's symbol; the
                            // title keeps the label color, which reads on both.
                            button.setContentTintColor(Some(&theme::accent()));
                        }
                        // SAFETY: the row retains its menu and every item targets
                        // the delegate, which outlives the sidebar.
                        if let Some(menu) = &menu {
                            unsafe { button.setMenu(Some(menu)) };
                        }
                        container.addSubview(&button);
                        let view: &NSView = &button;
                        statuses[position] = Some(SidebarRowStatus { state, button: view.retain(), indicator, y });
                        self.ivars()
                            .sidebar_session_buttons
                            .borrow_mut()
                            .push(button);
                    } else {
                        let button = navigation_button(
                            &summary.title,
                            symbol,
                            frame,
                            self,
                            sel!(openSidebarSession:),
                            mtm,
                        );
                        button.setTag(tag);
                        button.setToolTip(Some(&NSString::from_str(&tooltip)));
                        button.setAttributedTitle(&row_title);
                        if selected_here {
                            button.setContentTintColor(Some(&theme::accent()));
                        }
                        if let Some(menu) = &menu {
                            unsafe { button.setMenu(Some(menu)) };
                        }
                        container.addSubview(&button);
                        let view: &NSView = &button;
                        statuses[position] = Some(SidebarRowStatus { state, button: view.retain(), indicator, y });
                        buttons.push(button);
                    }
                    if actionable {
                        let more = icon_button(
                            "ellipsis.circle",
                            "Session actions",
                            rect(SIDEBAR_WIDTH - 52.0, y + 5.0, 24.0, 24.0),
                            self,
                            sel!(showSessionMenu:),
                            mtm,
                        );
                        more.setTag(tag);
                        more.setHidden(true);
                        container.addSubview(&more);
                        hover_targets.push((
                            rect(0.0, y, SIDEBAR_WIDTH - 16.0, SIDEBAR_ROW_HEIGHT),
                            more.clone(),
                        ));
                        buttons.push(more);
                    }
                }
            }
        }
        container.set_hover_targets(hover_targets);
        drop(records);
        drop(projects);
        drop(sessions);
        *self.ivars().sidebar_row_buttons.borrow_mut() = buttons;
        *self.ivars().sidebar_rows.borrow_mut() = rows;
        *self.ivars().sidebar_row_statuses.borrow_mut() = statuses;
        self.refresh_agents_panel();
        let clip = scroll.contentView();
        clip.scrollToPoint(NSPoint::new(
            0.0,
            (document_height - visible_height - if self.ivars().sidebar_reset_scroll.replace(false) { 0.0 } else { previous_top }).max(0.0),
        ));
        scroll.reflectScrolledClipView(&clip);
        let loading = match source_filter {
            SessionSourceFilter::Omperor => self.ivars().omperor_sessions_loading.get(),
            SessionSourceFilter::Omp => self.ivars().omp_sessions_loading.get(),
        };
        let error = match source_filter {
            SessionSourceFilter::Omperor => self.ivars().omperor_sessions_error.borrow().clone(),
            SessionSourceFilter::Omp => self.ivars().omp_sessions_error.borrow().clone(),
        };
        let label = if loading {
            "Loading…".to_owned()
        } else if error.is_some() {
            "Failed".to_owned()
        } else if query.is_empty() {
            format!("{session_count} shown")
        } else {
            format!("{session_count} found")
        };
        let status = self
            .ivars()
            .sidebar_status
            .get()
            .expect("sidebar status exists");
        status.setStringValue(&NSString::from_str(&label));
        status.setToolTip(error.as_deref().map(NSString::from_str).as_deref());
    }

    #[cfg(not(feature = "design-preview"))]
    fn schedule_background_start(&self) {
        // Let AppKit service one run-loop cycle and paint the window before
        // creating worker threads or touching OMP's on-disk session store.
        // SAFETY: AppDelegate implements startBackgroundServices: with the
        // standard timer action signature and lives for the app lifetime. A
        // common-mode timer also remains observable if AppKit temporarily
        // enters a nested run loop before it fires.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                0.05,
                self,
                sel!(startBackgroundServices:),
                None,
                false,
            )
        };
        // SAFETY: The timer and mode are valid Foundation objects. The run
        // loop retains the one-shot timer until it fires.
        unsafe {
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
    }

    fn start_session_discovery(&self) {
        let Some(omperor_sessions) = self.ivars().omperor_sessions_directory.get().cloned() else {
            return;
        };
        let Some(omperor_data) = self.ivars().omperor_data_directory.get().cloned() else {
            return;
        };
        self.ivars().omperor_sessions_loading.set(true);
        self.ivars().omp_sessions_loading.set(true);
        self.ivars().omperor_sessions_error.borrow_mut().take();
        self.ivars().omp_sessions_error.borrow_mut().take();
        let (sender, receiver) = flume::bounded(2);
        self.ivars().session_discovery.replace(Some(receiver));
        std::thread::Builder::new()
            .name("omperor-session-discovery".into())
            .spawn(move || {
                session::stream_discovery(&omperor_sessions, &omperor_data, sender);
            })
            .expect("session discovery worker must start");
    }

    fn poll_session_discovery(&self) {
        let update = {
            let discovery = self.ivars().session_discovery.borrow();
            let Some(discovery) = discovery.as_ref() else {
                return;
            };
            discovery.try_recv()
        };
        let update = match update {
            Ok(update) => update,
            Err(flume::TryRecvError::Empty) => return,
            Err(flume::TryRecvError::Disconnected) => {
                self.ivars().session_discovery.borrow_mut().take();
                let message = Some("Session discovery worker stopped unexpectedly".to_owned());
                if self.ivars().omperor_sessions_loading.replace(false) {
                    self.ivars().omperor_sessions_error.replace(message.clone());
                }
                if self.ivars().omp_sessions_loading.replace(false) {
                    self.ivars().omp_sessions_error.replace(message);
                }
                self.request_sidebar_rebuild();
                return;
            }
        };
        let complete = matches!(update, session::DiscoveryUpdate::Omp(_));
        match update {
            session::DiscoveryUpdate::Omperor(result) => {
                self.ivars().omperor_sessions_loading.set(false);
                self.apply_session_discovery_result(true, result);
            }
            session::DiscoveryUpdate::Omp(result) => {
                self.ivars().omp_sessions_loading.set(false);
                self.apply_session_discovery_result(false, result);
            }
        }
        if complete {
            self.ivars().session_discovery.borrow_mut().take();
        }
    }

    fn apply_session_discovery_result(
        &self,
        omperor_owned: bool,
        result: Result<Vec<session::SessionSummary>, session::SessionError>,
    ) {
        match result {
            Ok(replacement) => {
                if omperor_owned {
                    self.ivars().omperor_sessions_error.borrow_mut().take();
                } else {
                    self.ivars().omp_sessions_error.borrow_mut().take();
                }
                // A pass that read the disk before a deletion or an archive
                // finished must not bring those rows back.
                let mut replacement = replacement;
                replacement.retain(|row| !self.ivars().removed_session_ids.borrow().contains(&row.id));
                replace_session_source(
                    &mut self.ivars().sessions.borrow_mut(),
                    replacement,
                    omperor_owned,
                );
                self.ensure_mock_session_listed();
                self.select_live_session_if_unselected();
                if omperor_owned {
                    self.bootstrap_projects_if_needed();
                    self.open_pending_session_if_listed();
                }
            }
            Err(error) => {
                let error = Some(error.to_string());
                if omperor_owned {
                    self.ivars().omperor_sessions_error.replace(error);
                } else {
                    self.ivars().omp_sessions_error.replace(error);
                }
            }
        }
        self.sync_selected_session_header();
        self.request_sidebar_rebuild();
    }

    fn poll_session_history(&self) {
        self.save_heights_if_due();
        for owner in self.ivars().live_sessions.borrow().entries() {
            let next = || owner.session_history.borrow().as_ref().and_then(|history| history.receiver.try_recv().ok());
            let selected = self.session().key == owner.key;
            let mut update = next();
            // The saved heights come first, alone: taken in the same pass as
            // what follows, so the first page is not a tick later.
            while let Some(session::HistoryUpdate::Heights(saved)) = update {
                if selected {
                    self.transcript().use_saved_heights(saved);
                }
                update = next();
            }
            let Some(update) = update else { continue; };
            let history_finished = matches!(
                &update,
                session::HistoryUpdate::Complete | session::HistoryUpdate::Failed(_)
            );
            match update {
                session::HistoryUpdate::Page(messages) => {
                    let first = {
                        let mut history = owner.session_history.borrow_mut();
                        let Some(history) = history.as_mut() else { continue; };
                        let first = history.message_count == 0;
                        history.message_count += messages.len(); first
                    };
                    if first {
                        owner
                            .prompt_history
                            .borrow_mut()
                            .replace_from_messages(&messages);
                        owner.conversation.borrow_mut().load_history(&messages);
                    } else {
                        owner
                            .prompt_history
                            .borrow_mut()
                            .prepend_from_messages(&messages);
                        owner.conversation.borrow_mut().prepend_history(&messages);
                        owner.history_prepend_pending.set(true);
                    }
                }
                session::HistoryUpdate::Heights(_) => continue,
                session::HistoryUpdate::Complete => {
                    owner.session_history.borrow_mut().take();
                    if selected {
                        self.transcript().set_history_loading(false);
                        SAVED_HEIGHTS.with(|saved| saved.borrow_mut().1 = Some(std::time::Instant::now() + SAVE_HEIGHTS_AFTER));
                    }
                }
                session::HistoryUpdate::Failed(error) => {
                    owner.session_history.borrow_mut().take();
                    if selected {
                        self.transcript().set_history_loading(false);
                    }
                    owner.conversation.borrow_mut().notice(format!("History unavailable: {error}"), true);
                }
            }
            if selected {
                self.mark_chat_dirty();
                // Loading history deliberately disables submission. Once the
                // stream terminates, recompute the native controls immediately
                // so a resumable session does not remain permanently inert.
                if history_finished {
                    self.update_chat_controls();
                }
            }
        }
    }





    #[track_caller]
    fn start_workspace_inspection(&self) {
        let owner = self.session();
        // The perf event names the trigger (the caller's line), which is how
        // a cascade of inspections gets diagnosed; it is distinct from the
        // `gui.workspace.inspect` span the inspection thread records.
        let caller = std::panic::Location::caller();
        perf::event_with(
            "gui.workspace.inspect.requested",
            || serde_json::json!({ "caller": format!("{}:{}", caller.file(), caller.line()) }),
        );
        // Whatever prompted this inspection — a project switch, a tool that
        // changed files, the Refresh button, the watcher — may have changed
        // the file list too, so the `@`-picker index refreshes alongside.
        self.refresh_project_files();
        // This inspection observes any change the watcher noticed so far —
        // and, by event id, every change made up to this moment, so the
        // events for a tool's edits arriving later do not prompt a second
        // inspection.
        owner.workspace_change_seen_at.set(None);
        self.ensure_project_watcher();
        if let Some(watcher) = owner.project_watcher.borrow().as_ref() {
            watcher.mark_covered_through(file_watcher::current_event_id());
        }
        let project = owner.current_project.borrow().clone();
        self.ivars()
            .change_status
            .get()
            .expect("change status must exist before inspecting a workspace")
            .setStringValue(&NSString::from_str(&format!(
                "Inspecting {}…",
                project.display()
            )));
        let (sender, receiver) = flume::bounded(1);
        owner.workspace_discovery.replace(Some(receiver));
        std::thread::Builder::new()
            .name("omperor-workspace-inspection".into())
            .spawn(move || {
                let _ = sender.send(workspace::inspect(&project));
            })
            .expect("workspace inspection worker must start");
    }

    fn poll_workspace_inspection(&self) {
        let owner = self.session();
        let result = {
            let discovery = owner.workspace_discovery.borrow();
            let Some(discovery) = discovery.as_ref() else {
                return;
            };
            discovery.try_recv()
        };
        let Ok(result) = result else {
            return;
        };
        owner.workspace_discovery.borrow_mut().take();
        if matches!(
            &result,
            Ok(snapshot)
                if snapshot.project.as_path()
                    != owner.current_project.borrow().as_path()
        ) {
            if owner.workspace_refresh_pending.replace(false) {
                self.start_workspace_inspection();
            }
            return;
        }
        self.apply_workspace_inspection(result);
        self.restore_selected_change();
        if owner.workspace_refresh_pending.replace(false) {
            self.start_workspace_inspection();
        }
    }

    fn apply_workspace_inspection(
        &self,
        result: Result<workspace::WorkspaceSnapshot, workspace::WorkspaceError>,
    ) {
        let owner = self.session();
        let status = self
            .ivars()
            .change_status
            .get()
            .expect("change status must exist before inspection completes");
        let transcript = self
            .ivars()
            .change_transcript
            .get()
            .expect("change transcript must exist before inspection completes");
        match result {
            Ok(snapshot) if snapshot.repository_root.is_none() => {
                self.set_inspector_rows("Not a repository", "—", "—");
                self.set_change_section(0, None);
                status.setStringValue(&NSString::from_str(&format!(
                    "{} is not a Git repository",
                    snapshot.project.display()
                )));
                self.populate_change_list(&[], Some("Not a Git repository"));
                self.set_diff_title(None);
                transcript.setString(ns_string!(""));
                owner.workspace_snapshot.replace(Some(snapshot));
            }
            Ok(snapshot) => {
                let upstream = match snapshot.upstream {
                    None => "No upstream".to_owned(),
                    Some(upstream) if upstream.ahead == 0 && upstream.behind == 0 => {
                        "Up to date".to_owned()
                    }
                    Some(upstream) => {
                        let mut parts = Vec::new();
                        if upstream.ahead > 0 {
                            parts.push(format!("↑{} to push", upstream.ahead));
                        }
                        if upstream.behind > 0 {
                            parts.push(format!("↓{} to pull", upstream.behind));
                        }
                        parts.join("  ")
                    }
                };
                self.set_inspector_rows(
                    snapshot.branch.as_deref().unwrap_or("detached HEAD"),
                    &upstream,
                    snapshot.head_subject.as_deref().unwrap_or("No commits yet"),
                );
                status.setStringValue(&NSString::from_str(&snapshot.project.display().to_string()));
                if snapshot.changes.is_empty() {
                    self.set_change_section(0, None);
                    self.populate_change_list(&[], Some("Working tree clean"));
                    self.set_diff_title(None);
                    transcript.setString(ns_string!("No uncommitted changes."));
                } else {
                    self.set_change_section(snapshot.changes.len(), Some(snapshot.totals()));
                    self.populate_change_list(&snapshot.changes, None);
                }
                owner.workspace_snapshot.replace(Some(snapshot));
            }
            Err(error) => {
                self.set_inspector_rows("Unavailable", "—", "—");
                self.set_change_section(0, None);
                status.setStringValue(&NSString::from_str(&error.to_string()));
                self.populate_change_list(&[], Some("Could not inspect Git changes"));
                self.set_diff_title(None);
            }
        }
    }

    /// Inspects the workspace again, coalescing bursts: while an inspection
    /// is in flight the request is remembered and runs once it completes, so
    /// a turn with many edits costs at most two `git status` runs.
    fn request_workspace_refresh(&self) {
        let owner = self.session();
        if owner.workspace_discovery.borrow().is_some() {
            owner.workspace_refresh_pending.set(true);
            return;
        }
        self.start_workspace_inspection();
    }

    /// Loads one change's diff on a worker; the poll timer renders it.
    fn load_change_diff(
        &self,
        snapshot: workspace::WorkspaceSnapshot,
        change: workspace::WorkspaceChange,
    ) {
        let owner = self.session();
        #[cfg(feature = "design-preview")]
        if cfg!(feature = "design-preview") {
            self.preview_change_diff(&change);
            return;
        }
        self.set_diff_title(Some(&change));
        self.ivars()
            .change_transcript
            .get()
            .expect("change transcript must exist before loading a diff")
            .setString(ns_string!("Loading diff…"));
        let (sender, receiver) = flume::bounded(1);
        owner.diff_discovery.replace(Some(receiver));
        std::thread::Builder::new()
            .name("omperor-workspace-diff".into())
            .spawn(move || {
                let path = change.path.clone();
                let _ = sender.send((path, workspace::diff(&snapshot, &change)));
            })
            .expect("workspace diff worker must start");
    }

    /// After an automatic refresh, keeps the picker on the file the user was
    /// inspecting and reloads its diff, or says that it is no longer listed.
    fn restore_selected_change(&self) {
        let owner = self.session();
        let Some(selected) = owner.selected_change.borrow().clone() else {
            return;
        };
        let Some(snapshot) = owner.workspace_snapshot.borrow().clone() else {
            return;
        };
        let Some(index) = snapshot
            .changes
            .iter()
            .position(|change| change.path == selected)
        else {
            owner.selected_change.borrow_mut().take();
            self.highlight_change_row(None);
            self.set_diff_title(None);
            if !snapshot.changes.is_empty() {
                self.ivars()
                    .change_transcript
                    .get()
                    .expect("change transcript must exist after inspection")
                    .setString(&NSString::from_str(&format!(
                        "{} is no longer listed as changed.",
                        selected.display()
                    )));
            }
            return;
        };
        self.highlight_change_row(self.change_row_for(index));
        let change = snapshot.changes[index].clone();
        self.load_change_diff(snapshot, change);
    }

    fn poll_workspace_diff(&self) {
        let owner = self.session();
        let result = {
            let discovery = owner.diff_discovery.borrow();
            let Some(discovery) = discovery.as_ref() else {
                return;
            };
            discovery.try_recv()
        };
        let Ok((path, result)) = result else {
            return;
        };
        owner.diff_discovery.borrow_mut().take();
        let transcript = self
            .ivars()
            .change_transcript
            .get()
            .expect("change transcript must exist before a diff completes");
        // A path that vanished after inspection means the change list is
        // stale; show why and inspect again so the picker matches the tree.
        let stale = result
            .as_ref()
            .is_err_and(workspace::WorkspaceError::is_stale);
        let value = result.unwrap_or_else(|error| {
            if stale {
                format!("{error}. Refreshing the change list…")
            } else {
                error.to_string()
            }
        });
        let rendered = native_markdown::render_diff(&value, &path);
        // SAFETY: AppKit's scrollable NSTextView owns an NSTextStorage for the
        // lifetime of the view. The fallback preserves the content if a custom
        // text view implementation ever omits one.
        if let Some(storage) = unsafe { transcript.textStorage() } {
            storage.setAttributedString(&rendered);
        } else {
            transcript.setString(&NSString::from_str(&value));
        }
        transcript.scrollRangeToVisible(NSRange::new(0, 0));
        if stale {
            self.start_workspace_inspection();
        }
    }

    /// `--scroll-smoke` (`scroll_smoke`): stream the showcase into a fresh
    /// mock conversation, then hand the transcript to the scroll driver.
    #[cfg(target_os = "linux")]
    fn poll_scroll_smoke(&self) {
        thread_local! {
            static STAGE: std::cell::Cell<Option<(live_sessions::SessionKey, std::time::Instant, Option<std::time::Instant>)>> = const { std::cell::Cell::new(None) };
            static STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        if !scroll_smoke::requested() || self.ivars().termination_pending.get() || STARTED.with(std::cell::Cell::get) { return; }
        let Some((key, began, quiet)) = STAGE.with(std::cell::Cell::get) else {
            self.ivars().preferences.borrow_mut().developer_mode = true;
            self.ivars().preferences.borrow_mut().inference_backend = mock_inference::Backend::Mock;
            self.apply_appearance(if std::env::args().any(|arg| arg == "--light") { AppearancePreference::Light } else { AppearancePreference::Dark });
            let project = self.session().current_project.borrow().clone();
            let entry = self.ivars().live_sessions.borrow_mut().create();
            entry.current_project.replace(project);
            entry.pending_prompt.replace(Some(scroll_smoke::prompt().into()));
            self.launch_session_runtime(&entry, None, omp_runtime::SessionStartup::Fresh);
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(entry.key);
            self.render_selected_binding();
            STAGE.with(|stage| stage.set(Some((entry.key, std::time::Instant::now(), None))));
            return;
        };
        let Some(entry) = self.ivars().live_sessions.borrow().get(key) else { return };
        assert!(began.elapsed() < scroll_smoke::STREAM_TIMEOUT, "scroll-smoke: the showcase never finished streaming");
        let finished = entry.runtime_ready.get() && !entry.chat_busy.get() && entry.pending_prompt.borrow().is_none() && entry.conversation.borrow().len() > 1;
        match (finished, quiet) {
            (false, _) => STAGE.with(|stage| stage.set(Some((key, began, None)))),
            (true, None) => STAGE.with(|stage| stage.set(Some((key, began, Some(std::time::Instant::now()))))),
            (true, Some(since)) if since.elapsed() >= scroll_smoke::SETTLE => {
                STARTED.with(|started| started.set(true));
                eprintln!("scroll-smoke: showcase streamed in {:.1} s", began.elapsed().as_secs_f64());
                let window = self.ivars().window.get().unwrap();
                if scroll_smoke::switching() {
                    let switch = self.ivars().session_workspace.get().expect("the session workspace").surface_switch();
                    scroll_smoke::start_switching(window, switch);
                } else {
                    scroll_smoke::start(window, self.transcript().scroll_view().clone());
                }
            }
            (true, Some(_)) => {}
        }
    }

    fn poll_concurrency_smoke(&self) {
        let real_children = std::env::args().any(|arg| arg == "--lifecycle-children");
        if (!real_children && !std::env::args().any(|arg| arg == "--concurrency-smoke")) || self.ivars().termination_pending.get() { return; }
        if self.ivars().concurrency_smoke.borrow().is_none() {
            self.ivars().preferences.borrow_mut().developer_mode = true;
            self.ivars().preferences.borrow_mut().inference_backend = if real_children { mock_inference::Backend::Omp } else { mock_inference::Backend::Mock };
            if std::env::args().any(|arg| arg == "--light") { self.apply_appearance(AppearancePreference::Light); }
            else { self.apply_appearance(AppearancePreference::Dark); }
            if std::env::args().any(|arg| arg == "--minimum") { self.ivars().window.get().unwrap().setContentSize(NSSize::new(1080.0, 720.0)); }
            let project = self.session().current_project.borrow().clone();
            let mut keys = Vec::new();
            for index in 0..3 {
                let entry = self.ivars().live_sessions.borrow_mut().create();
                entry.current_project.replace(project.clone());
                if !real_children { entry.pending_prompt.replace(Some(format!("stream rate=40 size=8k owner={index}").into())); }
                self.launch_session_runtime(&entry, None, omp_runtime::SessionStartup::Fresh);
                keys.push(entry.key);
            }
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(keys[0]);
            self.render_selected_binding();
            self.ivars().concurrency_smoke.replace(Some((std::time::Instant::now(), keys, 0)));
            return;
        }
        let (started, keys, phase) = self.ivars().concurrency_smoke.borrow().as_ref().unwrap().clone();
        let entries: Vec<_> = keys.iter().map(|key| self.ivars().live_sessions.borrow().get(*key).unwrap()).collect();
        if real_children {
            assert!(!entries.iter().any(|entry| entry.lifecycle.get() == live_sessions::Lifecycle::Failed), "real helper startup failed: {:?}", entries.iter().map(|entry| entry.chat_status_text.borrow().clone()).collect::<Vec<_>>());
            if entries.iter().all(|entry| entry.runtime_ready.get()) {
                eprintln!("lifecycle-children ready=3 independent_owners=3 shutdown=requested");
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            } else if started.elapsed() > std::time::Duration::from_secs(90) {
                panic!("lifecycle-children startup timed out");
            }
            return;
        }
        let running = entries.iter().filter(|entry| entry.chat_busy.get()).count();
        if phase == 0 && running == 3 && entries.iter().all(|entry| entry.turn_started_ms.get().is_some()) {
            self.ivars().concurrency_smoke.borrow_mut().as_mut().unwrap().2 = 5;
            self.run_controller_audit_smoke();
            let appearance = if std::env::args().any(|arg| arg == "--light") { "light" } else { "dark" };
            let size = if std::env::args().any(|arg| arg == "--minimum") { "minimum" } else { "normal" };
            attachment_input::smoke_capture_window(self.ivars().window.get().unwrap(), &format!("/tmp/omperor-concurrent-{appearance}-{size}.png"));
            let selected_text = self.transcript().plain_text();
            self.apply_session_event(&entries[1], UiEvent::AssistantDelta("BACKGROUND_OWNER_MARKER".into()), std::time::Instant::now());
            self.refresh_chat_transcript();
            assert_eq!(self.transcript().plain_text(), selected_text);
            for (index, expected) in [(1, true), (0, false)] {
                let conversation = entries[index].conversation.borrow();
                assert_eq!((0..conversation.len()).filter_map(|row| conversation.message(row)).any(|message| message.body.contains("BACKGROUND_OWNER_MARKER")), expected);
            }
            self.ivars().input.get().unwrap().setStringValue(&NSString::from_str("retained draft A"));
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(keys[2]);
            self.render_selected_binding();
            self.ivars().input.get().unwrap().setStringValue(&NSString::from_str("retained draft C"));
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(keys[0]);
            self.render_selected_binding();
            assert_eq!(self.ivars().input.get().unwrap().stringValue().to_string(), "retained draft A");
            assert_eq!(&*entries[2].draft.borrow(), "retained draft C");
            let before = self.ivars().live_sessions.borrow().entries().len();
            let history = self.ivars().live_sessions.borrow_mut().create();
            history.current_project.replace(entries[0].current_project.borrow().clone());
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(history.key);
            self.render_selected_binding();
            assert!(history.omp_runtime.get().is_none());
            assert_eq!(self.ivars().live_sessions.borrow().entries().len(), before + 1);
            self.merge_retained_session_rows();
            let path = history.selected_session_path.borrow().clone().unwrap();
            assert_eq!(self.ivars().live_sessions.borrow().find_path(&path).unwrap().key, history.key);
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(keys[2]);
            self.render_selected_binding();
            assert_eq!(self.ivars().input.get().unwrap().stringValue().to_string(), "retained draft C");
            let sidebar_index = self.ivars().sidebar_rows.borrow().iter().position(|row| match row {
                SidebarRow::Session { index, .. } => self.ivars().sessions.borrow()[*index].path == path,
                _ => false,
            }).expect("dormant draft must be listed in sidebar");
            let button = NSButton::new(self.mtm());
            button.setTag(sidebar_index as isize);
            self.open_sidebar_session(sel!(openSidebarSession:), &button);
            self.open_sidebar_session(sel!(openSidebarSession:), &button);
            assert_eq!(self.session().key, history.key);
            assert!(history.omp_runtime.get().is_none());
            assert_eq!(self.ivars().live_sessions.borrow().len(), before + 1);
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(keys[2]);
            self.render_selected_binding();
            eprintln!("concurrency-smoke sidebar_draft_reselect=pass duplicate_owner=none");
            let stop = NSMenuItem::new(self.mtm());
            stop.setTag(keys[1].0 as isize);
            self.stop_sidebar_session(sel!(stopSidebarSession:), &stop);
            eprintln!("concurrency-smoke overlap=3 starts={:?} ids={:?} hidden_event_isolation=pass draft_navigation=pass draft_runtime_lazy=pass abort_target={:?} selected={:?}", entries.iter().map(|entry| entry.turn_started_ms.get()).collect::<Vec<_>>(), entries.iter().map(|entry| entry.live_session_id.borrow().clone()).collect::<Vec<_>>(), keys[1], self.session().key);
            self.ivars().concurrency_smoke.borrow_mut().as_mut().unwrap().2 = 1;
        } else if phase == 1 && !entries[1].chat_busy.get() {
            assert!(entries[0].chat_busy.get() && entries[2].chat_busy.get(), "abort must not stop other owners");
            assert_eq!(self.session().key, keys[2]);
            eprintln!("concurrency-smoke targeted_abort=pass other_sessions_running=2");
            assert!(!self.can_release_for_fork(&entries[0]), "running source cannot fork");
            assert!(self.can_release_for_fork(&entries[1]), "stopped source can fork");
            self.ivars().authentication_owner.set(Some(keys[1]));
            assert!(!self.can_release_for_fork(&entries[1]), "auth owner cannot release");
            self.ivars().authentication_owner.set(None);
            entries[1].attachment_submission.set(true);
            assert!(!self.can_release_for_fork(&entries[1]), "preparing attachments cannot release");
            entries[1].attachment_submission.set(false);
            entries[1].subagents.borrow_mut().push(omp_runtime::SubagentInfo {
                id: "quiescence-check".into(), index: 0, agent: "task".into(), label: "Delegated work".into(),
                status: "running".into(), started_ms: None, duration_ms: None, error: None, warning: None, session_file: None,
            });
            assert!(!self.can_release_for_fork(&entries[1]), "delegated work cannot release");
            entries[1].subagents.borrow_mut().pop();
            let parent = entries[1].live_session_id.borrow().clone().unwrap();
            self.create_session_in("smoke", entries[1].current_project.borrow().clone(), Some(ForkPlan { parent_session_id: parent, worktree: None }), false);
            let target = self.ivars().pending_forks.borrow().get(&keys[1]).unwrap().0;
            assert!(self.fork_owns_session(keys[1]));
            assert_eq!(self.session().key, keys[2]);
            if std::env::args().any(|arg| arg == "--quit-during-fork") {
                eprintln!("concurrency-smoke pending_fork_quit=exercised");
                NSApplication::sharedApplication(self.mtm()).terminate(None);
                return;
            }
            let mut smoke = self.ivars().concurrency_smoke.borrow_mut();
            let smoke = smoke.as_mut().unwrap();
            smoke.1.push(target);
            smoke.2 = 2;
        } else if phase == 2 && entries[3].runtime_ready.get() {
            assert_eq!(self.session().key, keys[2], "fork completion must not steal focus");
            assert!(!entries[1].runtime_ready.get());
            assert_eq!(entries[1].lifecycle.get(), live_sessions::Lifecycle::History);
            assert!(!self.fork_owns_session(keys[1]));
            assert!(entries[1].conversation.borrow().len() > 0);
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(keys[1]);
            self.render_selected_binding();
            assert!(!entries[1].runtime_ready.get(), "navigation must not resume a released writer");
            self.ivars().input.get().unwrap().setStringValue(&NSString::from_str("stream rate=40 size=8k resumed-source"));
            self.send_message(sel!(sendMessage:), self.ivars().input.get().unwrap());
            self.capture_selected_binding();
            self.ivars().live_sessions.borrow_mut().select(keys[2]);
            self.render_selected_binding();
            self.ivars().concurrency_smoke.borrow_mut().as_mut().unwrap().2 = 3;
        } else if phase == 3 && entries[1].runtime_ready.get() && entries[1].chat_busy.get() && !entries[1].attachment_submission.get() {
            assert_eq!(self.session().key, keys[2]);
            eprintln!("concurrency-smoke PASS fork_release=pass retained_source=pass explicit_send_resume=pass no_focus_theft=pass all_owned_shutdown=requested");
            self.ivars().concurrency_smoke.borrow_mut().as_mut().unwrap().2 = 4;
        } else if phase == 4 {
            if !std::env::args().any(|arg| arg == "--smoke-hold") || started.elapsed() > std::time::Duration::from_secs(8) {
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
        } else if started.elapsed() > std::time::Duration::from_secs(15) {
            panic!("concurrency-smoke timeout phase={phase} overlapping={running}");
        }
    }

    /// Give a dormant historical session enough of the last live catalog to
    /// render stable controls and choose its account before starting a child.
    /// Ready replaces these hints with the resumed child's authoritative
    /// catalog. Keeping them here avoids a blank picker and, more importantly,
    /// avoids launching once without an account only to restart immediately.
    fn seed_session_runtime_context(
        &self,
        target: &live_sessions::LiveSession,
        source: &live_sessions::LiveSession,
        recorded_model: Option<&str>,
    ) {
        let model = recorded_model
            .filter(|model| !model.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| {
                let model = source.current_model.borrow();
                (!model.trim().is_empty()).then(|| model.clone())
            })
            .or_else(|| self.ivars().preferences.borrow().model.clone())
            .unwrap_or_default();
        target.current_model.replace(model.clone());
        let mut models = source.available_models.borrow().clone();
        if !model.is_empty() && !models.contains(&model) {
            models.insert(0, model);
        }
        target.available_models.replace(models);
        target.fast_models.replace(source.fast_models.borrow().clone());
        target.fast_mode.set(source.fast_mode.get());
        target
            .current_thinking_level
            .replace(source.current_thinking_level.borrow().clone());
        target
            .authenticated_providers
            .replace(source.authenticated_providers.borrow().clone());
        target.accounts_known.set(source.accounts_known.get());
        target
            .current_account
            .replace(source.current_account.borrow().clone());
    }

    fn launch_session_runtime(&self, owner: &live_sessions::LiveSession, account: Option<String>, target: omp_runtime::SessionStartup) {
        if owner.omp_runtime.get().is_some() || self.ivars().termination_pending.get() { return; }
        if let omp_runtime::SessionStartup::Resume(id) = &target { owner.resuming_session_id.replace(Some(id.clone())); }
        owner.runtime_starting.set(true);
        owner.backend.set(self.ivars().preferences.borrow().effective_backend());
        let startup = omp_runtime::RuntimeStartup {
            helper: None,
            backend: self.ivars().preferences.borrow().effective_backend(),
            project: owner.current_project.borrow().clone(),
            model: {
                let model = owner.current_model.borrow();
                if model.trim().is_empty() { omp_runtime::sanitized_model_preference(self.ivars().preferences.borrow().model.as_deref()) }
                else { omp_runtime::sanitized_model_preference(Some(&model)) }
            },
            account: account.or_else(|| owner.current_account.borrow().clone()),
            thinking_level: (!owner.current_thinking_level.borrow().is_empty()).then(|| owner.current_thinking_level.borrow().clone()),
            fast_mode: owner.fast_mode.get(),
            omperor_tools: owner.omperor_tools.get(),
            computer_use: self.ivars().preferences.borrow().computer_use,
            data_dir: self.ivars().omperor_data_directory.get().expect("runtime data directory").clone(),
            session_dir: self.ivars().omperor_sessions_directory.get().expect("runtime session directory").clone(),
            environment: vec![],
            session: target,
        };
        owner.omp_runtime.set(OmpRuntime::start_configured(startup)).expect("one runtime per session");
    }

    fn start_omp_runtime(&self, account: Option<String>) {
        let owner = self.session();
        self.launch_session_runtime(&owner, account, omp_runtime::SessionStartup::Fresh);
        self.update_chat_controls();
        if self.ivars().runtime_timer.get().is_some() { return; }
        // SAFETY: AppDelegate implements pollOmpRuntime: with the standard
        // timer action signature and remains alive for the application lifetime.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                1.0 / 30.0,
                self,
                sel!(pollOmpRuntime:),
                None,
                true,
            )
        };
        // SAFETY: The timer and mode are valid Foundation objects. Registering
        // in the common modes is essential here: after TerminateLater, AppKit
        // waits in a nested termination run loop. A default-mode-only timer
        // would never observe that OMP exited or send AppKit the final reply.
        unsafe {
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
        self.ivars()
            .runtime_timer
            .set(timer)
            .expect("OMP runtime timer should only be initialized once");
    }

    /// Applies one event the runtime thread received at `arrived`; the
    /// timings recorded for turns and tools are taken from that moment.
    fn set_session_status(&self, owner: &live_sessions::LiveSession, value: &str) {
        owner.chat_status_text.replace(value.to_owned());
        if self.session().key == owner.key { self.set_chat_status(value); }
    }

    fn run_controller_audit_smoke(&self) {
        let selected = self.session().key;
        let a = self.ivars().live_sessions.borrow_mut().create();
        let b = self.ivars().live_sessions.borrow_mut().create();
        self.ivars().authentication_owner.set(Some(b.key));
        self.ivars().authentication_busy.set(true);
        self.ivars().authentication_generation.set(42);
        self.ivars().authentication_session.replace(None);
        for event in [
            UiEvent::AuthenticationStarted { provider: AuthProvider::OpenAi, generation: 41 },
            UiEvent::AuthenticationSession { generation: 41, session_id: "old".into() },
            UiEvent::AuthenticationSession { generation: 42, session_id: "wrong-owner".into() },
            UiEvent::AuthenticationFailed { session_id: None, generation: Some(41), error: "stale".into() },
            UiEvent::AuthenticationFailed { session_id: Some("old".into()), generation: None, error: "stale".into() },
            UiEvent::AuthenticationCancelled { session_id: "old".into() },
            UiEvent::AuthenticationCompleted { session_id: "old".into(), provider: AuthProvider::OpenAi, account_id: None, principal: None, account: None },
        ] { self.apply_global_session_event(&a, event); }
        assert!(self.ivars().authentication_busy.get());
        assert_eq!(self.ivars().authentication_owner.get(), Some(b.key));
        assert!(self.ivars().authentication_session.borrow().is_none());
        assert!(a.authenticated_providers.borrow().contains(&AuthProvider::OpenAi));
        self.apply_global_session_event(&b, UiEvent::AuthenticationFailed { session_id: None, generation: Some(41), error: "stale same owner".into() });
        assert!(self.ivars().authentication_busy.get());
        self.apply_global_session_event(&b, UiEvent::AuthenticationSession { generation: 42, session_id: "new".into() });
        self.apply_global_session_event(&b, UiEvent::AuthenticationCompleted { session_id: "old".into(), provider: AuthProvider::OpenAi, account_id: None, principal: None, account: None });
        assert!(self.ivars().authentication_busy.get());
        assert_eq!(self.ivars().authentication_session.borrow().as_deref(), Some("new"));
        self.ivars().authentication_session.replace(None);
        self.apply_global_session_event(&b, UiEvent::AuthenticationFailed {
            session_id: None, generation: Some(42), error: "expected current failure".into(),
        });
        assert!(!self.ivars().authentication_busy.get());
        assert_eq!(self.ivars().authentication_failure.borrow().as_deref(), Some("expected current failure"));
        self.ivars().authentication_failure.replace(None);
        self.ivars().authentication_owner.set(None);
        self.ivars().authentication_session.replace(None);
        let path = std::path::PathBuf::from("/tmp/omperor-controller-audit-journal");
        self.ivars().live_sessions.borrow_mut().bind_journal(a.key, path.clone()).unwrap();
        let before = b.selected_session_path.borrow().clone();
        b.current_model.replace("retained-model".into());
        let event = UiEvent::Ready {
            model: "unexpected-model".into(), models: vec![], fast_models: vec![], fast_mode: false,
            thinking_level: "high".into(), session_id: Some("collision".into()), session_file: Some(path.clone()),
            project: b.current_project.borrow().clone(), authenticated_providers: vec![], accounts: None,
            account: None, session_is_fresh: true, resumed: None,
        };
        self.apply_session_event(&b, event, std::time::Instant::now());
        assert_eq!(self.ivars().live_sessions.borrow().find_path(&path).unwrap().key, a.key);
        assert_eq!(*b.selected_session_path.borrow(), before);
        assert_eq!(&*b.current_model.borrow(), "retained-model");
        assert!(b.header_session.borrow().is_none());
        assert!(!b.runtime_ready.get());
        assert!(b.read_only_session.get());
        assert_eq!(self.session().key, selected);
        eprintln!("controller-audit-smoke PASS late_auth_completion=preserves_new_attempt stale_owner_and_generation=ignored duplicate_ready=preserves_identity");
    }

    fn apply_global_session_event(&self, owner: &live_sessions::LiveSession, event: UiEvent) {
        let owns_attempt = self.ivars().authentication_owner.get() == Some(owner.key)
            && self.ivars().authentication_busy.get();
        let current_session = |session_id: &str| owns_attempt
            && self.ivars().authentication_session.borrow().as_deref() == Some(session_id);
        match event {
            UiEvent::Ready { accounts: Some(accounts), .. } | UiEvent::AccountsChanged(accounts) => self.replace_accounts(accounts),
            UiEvent::AccountChanged(Some(account_id)) => {
                if let Some(account) = self.account_entry(&account_id) {
                    self.remember_session_account(owner, account.provider(), &account_id);
                }
            }
            UiEvent::AccountUsage(usages) => {
                self.ivars().account_usage_loading.set(false);
                if !usages.is_empty() {
                    self.ivars().account_usage_answered_at.set(Some(std::time::Instant::now()));
                }
                for usage in usages { self.ivars().account_usage.borrow_mut().insert(usage.account_id.clone(), usage); }
                self.ivars().accounts_ui_dirty.set(true);
                self.fill_usage_limits();
            }
            UiEvent::AuthenticationStarted { provider: _, generation } => {
                if !owns_attempt || generation != self.ivars().authentication_generation.get() { return; }
                self.refresh_sign_in_sheet();
            }
            UiEvent::AuthenticationSession {
                generation,
                session_id,
            } => {
                let ivars = self.ivars();
                // The acknowledgement of a login given up on before it
                // arrived, or of one from before a method switch: that
                // session ends here. Only the current attempt's is kept.
                let stale = !owns_attempt
                    || generation != ivars.authentication_generation.get()
                    || ivars.authentication_cancelling.get();
                if stale {
                    if let Some(runtime) = owner.omp_runtime.get() {
                        runtime.cancel_authentication(session_id);
                    }
                } else {
                    ivars.authentication_session.replace(Some(session_id));
                }
            }
            UiEvent::OpenAuthenticationUrl {
                session_id,
                url,
                launch,
            } => {
                if !current_session(&session_id) {
                    return;
                }
                self.ivars().authentication_url.replace(Some((url, launch)));
                self.open_sign_in_page();
                let provider = self
                    .ivars()
                    .authentication_provider
                    .get()
                    .map_or("your account", AuthProvider::product_name);
                self.set_session_status(owner, &format!(
                    "Finish signing in with {provider} in your browser…"
                ));
                self.refresh_sign_in_sheet();
            }
            UiEvent::AuthenticationDeviceCode {
                session_id,
                code,
                verification_url,
            } => {
                if !current_session(&session_id) {
                    return;
                }
                self.ivars()
                    .authentication_device
                    .replace(Some((code, verification_url)));
                self.refresh_sign_in_sheet();
            }
            UiEvent::AuthenticationWaiting {
                session_id,
                message,
            } => {
                if !current_session(&session_id) {
                    return;
                }
                self.set_session_status(owner, &message);
                self.refresh_sign_in_sheet();
            }
            UiEvent::AuthenticationPrompt(prompt) => {
                if !current_session(&prompt.session_id) {
                    return;
                }
                self.ivars().authentication_submitting.set(false);
                self.ivars()
                    .pending_authentication
                    .borrow_mut()
                    .replace(prompt);
                self.refresh_sign_in_sheet();
            }
            UiEvent::AuthenticationCompleted {
                session_id,
                provider,
                account_id,
                principal,
                account,
            } => {
                // A completed sign-in is a credential now, whichever attempt
                // produced it; it is never ignored as stale.
                if current_session(&session_id) {
                    self.dismiss_sign_in_sheet();
                    self.ivars().authentication_failure.borrow_mut().take();
                    self.ivars().authentication_submitting.set(false);
                    self.end_sign_in_state();
                }
                owner.authenticated_providers
                    .borrow_mut()
                    .insert(provider);
                owner.runtime_ready.set(false);
                owner.runtime_starting.set(true);
                owner.chat_busy.set(false);
                self.set_session_status(owner, &account.map_or_else(
                    || {
                        format!(
                            "{} connected • starting a fresh OMP session…",
                            provider.product_name()
                        )
                    },
                    |account| {
                        format!(
                            "{} connected as {account} • starting a fresh OMP session…",
                            provider.product_name()
                        )
                    },
                ));
                if let Some(account_id) = account_id {
                    self.recover_account_alert(&account_id, provider, true);
                    self.register_authenticated_account(provider, account_id, principal);
                }
            }
            UiEvent::AuthenticationFailed { session_id, generation, error } => {
                // A sign-in already given up on may still report how it
                // ended; that is not news.
                let current = match session_id.as_deref() {
                    Some(session_id) => current_session(session_id),
                    None => owns_attempt && generation == Some(self.ivars().authentication_generation.get()) && self.ivars().authentication_session.borrow().is_none(),
                };
                if !current {
                    return;
                }
                self.ivars().authentication_submitting.set(false);
                self.sign_in_failed(&error);
            }
            UiEvent::AuthenticationCancelled { session_id } => {
                if !current_session(&session_id) {
                    return;
                }
                self.end_sign_in_state();
                self.set_session_status(owner, "Sign-in cancelled");
                self.dismiss_sign_in_sheet();
            }
            _ => {}
        }
    }

    fn apply_session_event(&self, owner: &live_sessions::LiveSession, event: UiEvent, arrived: std::time::Instant) {
        // The agent's calls to Omperor's own tools are answered by the app,
        // never shown as transcript events.
        let event = match event {
            UiEvent::HostToolCall { call, tool, arguments } => {
                self.handle_omperor_tool_call(owner, call, &tool, &arguments);
                return;
            }
            UiEvent::HostToolCancelled { call } => {
                self.withdraw_tool_confirmation(owner.key, call);
                return;
            }
            event => event,
        };
        let previous_path = owner.selected_session_path.borrow().clone();
        // Validate identity before Ready can mutate the owner or move its saved view.
        let event = if let UiEvent::Ready { model, session_id, session_file: Some(path), resumed, .. } = &event {
            let backend_matches = !owner.backend_switch_pending.get()
                || (model == mock_inference::MODEL) == (owner.backend.get() == mock_inference::Backend::Mock);
            let resume_matches = owner.resuming_session_id.borrow().as_deref().is_none_or(|expected|
                resumed.as_deref() == Some(expected) && session_id.as_deref() == Some(expected));
            if backend_matches && resume_matches {
                let binding = self.ivars().live_sessions.borrow_mut().bind_journal(owner.key, path.clone());
                if let Err(existing) = binding {
                    if let Some(runtime) = owner.omp_runtime.get() { runtime.begin_shutdown(); }
                    owner.read_only_session.set(true);
                    UiEvent::Failed(format!("OMP journal already belongs to session {} • duplicate runtime stopped", existing.0))
                } else { event }
            } else { event }
        } else { event };
        let released = matches!(&event, UiEvent::SessionReleased);
        let stopped = matches!(&event, UiEvent::Stopped);
        let failed = matches!(&event, UiEvent::Failed(_));
        let model_changed = matches!(&event, UiEvent::ModelChanged(_));
        if std::env::args().any(|arg| arg == "--lifecycle-children") && let UiEvent::Failed(message) = &event { eprintln!("lifecycle-children failed={message}"); }
        let event = match event {
            UiEvent::TurnFinished(TurnOutcome::Errored(reason)) => UiEvent::TurnFinished(TurnOutcome::Errored(name_accounts_in(&reason, &self.ivars().accounts.borrow()))),
            event => event,
        };
        // Streamed text and a turn's progress change what the session is
        // doing, never which rows the sidebar lists; only these leave the
        // rows alone, so the sidebar can update just this session's
        // indicator. Streamed text alone doesn't change the Agents panel.
        let rows_unchanged = matches!(&event,
            UiEvent::AssistantDelta(_) | UiEvent::ThinkingDelta(_) | UiEvent::ToolArguments { .. }
            | UiEvent::ToolUpdated { .. } | UiEvent::Running | UiEvent::AssistantStarted
            | UiEvent::AssistantFinished(_) | UiEvent::InferenceReceipt(_) | UiEvent::ToolStarted { .. }
            | UiEvent::ToolFinished(_) | UiEvent::TurnResults(_) | UiEvent::SubagentsSnapshot(_)
            | UiEvent::SubagentChanged(_) | UiEvent::SubagentsChanged);
        let streamed_text = matches!(&event,
            UiEvent::AssistantDelta(_) | UiEvent::ThinkingDelta(_) | UiEvent::ToolArguments { .. });
        let state_before = owner.sidebar_state();
        let root_before = (owner.live_state.borrow().clone(), owner.current_model.borrow().clone());
        let effects = owner.reduce(event, arrived);
        let rows_unchanged = rows_unchanged
            && !effects.ready
            && !effects.dispatch_prompt
            && !effects.discover_sessions
            && effects.global_events.is_empty()
            && effects.account_trouble.is_empty();
        if effects.ready {
            let next_path = owner.selected_session_path.borrow().clone();
            if previous_path != next_path {
                let view = self.ivars().agent_view_states.borrow_mut().remove(&previous_path);
                if let Some(view) = view { self.ivars().agent_view_states.borrow_mut().insert(next_path, view); }
            }
        }
        if effects.rejected { return; }
        if released || stopped {
            self.removal_released(owner.key);
        }
        if released {
            let pending = self.ivars().pending_forks.borrow().get(&owner.key).cloned();
            if let Some((target, parent, _)) = pending
                && !self.ivars().termination_pending.get()
                && let Some(target) = self.ivars().live_sessions.borrow().get(target)
            {
                self.launch_session_runtime(&target, None, omp_runtime::SessionStartup::Fork(parent));
            }
        }
        if effects.ready || failed {
            let source = self.ivars().pending_forks.borrow().iter()
                .find_map(|(source, (target, _, _))| (*target == owner.key || (failed && *source == owner.key)).then_some(*source));
            if let Some(source) = source {
                let (target, _, job) = self.ivars().pending_forks.borrow_mut().remove(&source).unwrap();
                self.finish_job(job);
                if self.session().key == source { self.update_chat_controls(); }
                if failed && target != owner.key
                    && let Some(target) = self.ivars().live_sessions.borrow().get(target)
                {
                    target.new_session_pending.set(false);
                    target.lifecycle.set(live_sessions::Lifecycle::Failed);
                    self.set_session_status(&target, "Source release failed; choose Fork again to retry");
                }
            }
        }
        for record in effects.metrics { self.record_metric(record); }
        for event in effects.global_events { self.apply_global_session_event(owner, event); }
        if effects.refresh_subagents {
            if let Some(runtime) = owner.omp_runtime.get() { runtime.refresh_subagents(); }
        }
        if effects.discover_sessions { self.start_session_discovery(); }
        if effects.refresh_workspace { owner.workspace_refresh_pending.set(true); }
        if effects.ready {
            self.ivars().sessions.borrow_mut().retain(|row| row.id != format!("draft-{}", owner.key.0));
            self.merge_retained_session_rows();
            if let Some((id, _)) = owner.header_session.borrow().clone() {
                if let Some(pending) = owner.pending_session.borrow_mut().take() {
                    let record = SessionRecord {
                        session_id: id.clone(), project_id: pending.project_id, directory: pending.directory, created_at_ms: preferences::now_ms(),
                        parent_session_id: pending.parent_session_id, branch: pending.branch,
                        base_branch: pending.base_branch, worktree_path: pending.worktree_path,
                    };
                    self.store_session_record(record);
                }
            }
        }
        if effects.ready { self.resolve_session_account(owner); }
        if effects.dispatch_prompt || effects.ready { self.dispatch_session_prompt(owner); }
        if !rows_unchanged {
            self.request_sidebar_rebuild();
        } else if owner.sidebar_state() != state_before {
            self.request_sidebar_state_refresh();
        }
        for (message, primary, provider) in effects.account_trouble { self.note_session_account_trouble(owner, &message, primary, provider); }
        if let Some(error) = effects.authentication_failure {
            if self.ivars().authentication_owner.get() == Some(owner.key) && self.ivars().authentication_busy.get() { self.sign_in_failed(&error); }
        }
        if let Some(provider_id) = effects.account_recovery {
            if let Some(account) = owner.current_account.borrow().as_deref().and_then(|id| self.account_entry(id)) {
                if account.provider().id() == provider_id { owner.account_alerts.borrow_mut().recovered(account.id(), account.provider(), false); }
            }
        }
        if self.session().key != owner.key { return; }
        if effects.ready {
            self.refresh_project_location();
            self.populate_model_picker(&owner.available_models.borrow(), &owner.current_model.borrow());
            self.set_thinking_label(&owner.current_thinking_level.borrow());
            self.refresh_fast_chip();
        } else if model_changed {
            self.show_model_selection(&owner.current_model.borrow());
            self.refresh_fast_chip();
        }
        if effects.sync_subagent_transcripts { self.sync_subagent_transcripts(true); }
        if owner.chat_dirty.replace(false) { self.mark_chat_dirty(); }
        self.refresh_speedometer();
        self.update_chat_controls();
        let root_after = (owner.live_state.borrow().clone(), owner.current_model.borrow().clone());
        if !streamed_text || root_after != root_before {
            self.request_agents_refresh();
        }
        let status = owner.chat_status_text.borrow().clone();
        self.set_chat_status(&status);
    }

    fn fit_sidebar_document(&self) {
        let ivars = self.ivars();
        let (Some(scroll), Some(container)) = (
            ivars.sidebar_sessions_scroll.get(),
            ivars.sidebar_sessions_view.get(),
        ) else {
            return;
        };
        let visible_height = scroll.contentSize().height;
        let target = ivars.sidebar_rows_height.get().max(visible_height);
        let current = container.frame();
        let delta = target - current.size.height;
        if delta.abs() < 0.5 {
            return;
        }
        let subviews = container.subviews();
        for index in 0..subviews.count() {
            let subview = subviews.objectAtIndex(index);
            let frame = subview.frame();
            subview.setFrameOrigin(NSPoint::new(frame.origin.x, frame.origin.y + delta));
        }
        container.setFrameSize(NSSize::new(current.size.width, target));
    }

    fn sync_transcript_measure(&self) {
        if let Some(transcript) = self.ivars().chat_transcript.get() {
            transcript.viewport_changed();
        }
    }

    fn chat_text_size_for(&self, role: TextRole) -> f64 {
        let prefs = self.ivars().preferences.borrow();
        prefs
            .chat_text_sizes
            .resolve(role, prefs.chat_style, prefs.chat_text_size.scale())
    }

    fn set_chat_status(&self, value: &str) {
        let owner = self.session();
        let labelled;
        let value = if self.mock_backend_active() && !value.starts_with(MOCK_STATUS_PREFIX) {
            labelled = format!("{MOCK_STATUS_PREFIX}{value}");
            &labelled
        } else { value };
        let status = self
            .ivars()
            .chat_status
            .get()
            .expect("chat status must exist before changing its value");
        owner.chat_status_text.replace(value.to_owned());
        // A real change supersedes a flash: there is nothing to go back to.
        self.ivars().chat_status_flashing.set(false);
        status.setAlphaValue(1.0);
        status.setAttributedStringValue(&status_attributed(value, self.chat_text_size_for(TextRole::Metadata)));
        // The line truncates; a failure's reason is often past the cut.
        status.setToolTip(Some(&NSString::from_str(value)));
    }

    /// Says something on the status line for a moment, in gold, then fades
    /// back to what the line was saying. For confirmations such as "Copied",
    /// which are answers to a click rather than states of the owner.
    fn flash_chat_status(&self, value: &str) {
        let Some(status) = self.ivars().chat_status.get() else {
            return;
        };
        self.ivars().chat_status_flashing.set(true);
        status.setAlphaValue(1.0);
        status.setAttributedStringValue(&status_flash_attributed(
            value,
            self.chat_text_size_for(TextRole::Metadata),
        ));
        // SAFETY: AppDelegate implements restoreChatStatus: with the standard
        // timer action signature and outlives the timer.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                motion::FLASH_HOLD,
                self,
                sel!(restoreChatStatus:),
                None,
                false,
            )
        };
        // SAFETY: the timer and mode are valid Foundation objects; the run
        // loop retains the one-shot timer until it fires.
        unsafe {
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
    }

    // ---- Signing in -------------------------------------------------------
    //
    // A sign-in is a conversation with OMP, and the sheet is where it is
    // had. The sheet is presented before OMP is asked, so the user always
    // has somewhere to look and a way out; OMP's events update it; the
    // user's answers go back through it. The composer stays a composer.

    /// Sends the sheet's answer to the prompt OMP is waiting on: a pasted
    /// credential, or a callback URL carried back by hand.
    fn answer_sign_in(&self, value: String) {
        let Some(owner) = self.ivars().authentication_owner.get().and_then(|key| self.ivars().live_sessions.borrow().get(key)) else { return; };
        let Some(prompt) = self.ivars().pending_authentication.borrow().clone() else {
            return;
        };
        self.ivars().authentication_submitting.set(true);
        self.refresh_sign_in_sheet();
        let sent = owner.omp_runtime
            .get()
            .is_some_and(|runtime| runtime.answer_authentication(prompt, value));
        if !sent {
            self.ivars().authentication_submitting.set(false);
            self.sign_in_failed("Could not send the answer to OMP");
        }
    }

    /// Whether an event belongs to the sign-in in progress. Events from a
    /// login given up on, or from an earlier method, still arrive; they are
    /// not this sign-in's.
    /// The window a sign-in sheet hangs from: always Settings, open to the
    /// Accounts page, so a sign-in never sits in front of the conversation.
    /// Sessions carry on underneath while the browser is out.
    fn sign_in_host(&self) -> Option<Retained<NSWindow>> {
        if self.ivars().settings_ui.get().is_none() {
            // Only a sign-in begun from the main window finds no Settings
            // window; one begun from an accounts card already has it, and a
            // full sync here would rebuild the card under that button.
            let settings = self.build_settings_window(self.mtm());
            self.ivars().settings_ui.set(settings).ok()?;
            self.sync_settings_ui();
        }
        self.show_settings_section(SETTINGS_ACCOUNTS_PAGE);
        let settings = self.ivars().settings_ui.get()?;
        settings.window.makeKeyAndOrderFront(None);
        Some(settings.window.clone())
    }

    /// Opens the sign-in sheet for `provider` and starts its usual method.
    /// A sign-in already in progress for the same provider just brings its
    /// sheet back; one for another provider is cancelled first.
    fn present_sign_in(&self, provider: AuthProvider) {
        if self.mock_backend_active() { self.set_chat_status("Select OMP in Settings → General to use this action"); return; }
        // The same provider's sheet just comes back; another provider's
        // sign-in gives way.
        if let Some(sheet) = self
            .ivars()
            .sign_in_sheet
            .borrow()
            .as_ref()
            .map(SignInSheet::views)
            && sheet.provider == provider
        {
            sheet.host.makeKeyAndOrderFront(None);
            return;
        }
        if self.ivars().sign_in_sheet.borrow().is_some() {
            self.cancel_sign_in_session();
            self.dismiss_sign_in_sheet();
        }
        if self.ivars().authentication_busy.get() {
            if self.ivars().authentication_provider.get() != Some(provider) {
                self.cancel_sign_in_session();
            } else {
                // Re-attach the sheet to the sign-in already running.
                let method = self
                    .ivars()
                    .authentication_method
                    .get()
                    .unwrap_or(provider.methods()[0]);
                self.build_sign_in_sheet(provider, method);
                self.refresh_sign_in_sheet();
                return;
            }
        }
        let method = provider.methods()[0];
        self.build_sign_in_sheet(provider, method);
        self.begin_authentication(provider, method);
    }

    /// Asks OMP to start signing `provider` in by `method`. The sheet, if
    /// one is up, shows the start; the events that follow fill it in.
    fn begin_authentication(&self, provider: AuthProvider, method: AuthMethod) {
        let owner = self.session();
        let ivars = self.ivars();
        if ivars.authentication_busy.replace(true) {
            return;
        }
        ivars.authentication_owner.set(Some(owner.key));
        ivars.authentication_provider.set(Some(provider));
        ivars.authentication_method.set(Some(method));
        ivars.authentication_session.borrow_mut().take();
        ivars.authentication_url.borrow_mut().take();
        ivars.authentication_device.borrow_mut().take();
        ivars.authentication_failure.borrow_mut().take();
        ivars.pending_authentication.borrow_mut().take();
        ivars.authentication_cancelling.set(false);
        let generation = ivars.authentication_generation.get() + 1;
        ivars.authentication_generation.set(generation);
        self.update_chat_controls();
        self.mark_accounts_ui_dirty();
        self.set_chat_status(&format!("Starting {} sign-in…", provider.product_name()));
        self.refresh_sign_in_sheet();
        let sent = owner.omp_runtime
            .get()
            .is_some_and(|runtime| runtime.login(provider, method, generation));
        if !sent {
            self.sign_in_failed(&format!(
                "Could not ask OMP to start the {} sign-in",
                provider.product_name()
            ));
        }
    }

    /// Ends the login session OMP is running for us, if any, and clears the
    /// sign-in state. The sheet, if up, stays for the caller to decide about.
    fn cancel_sign_in_session(&self) {
        let Some(owner) = self.ivars().authentication_owner.get().and_then(|key| self.ivars().live_sessions.borrow().get(key)) else { return; };
        let ivars = self.ivars();
        if !ivars.authentication_busy.get() {
            return;
        }
        let session = ivars.authentication_session.borrow().clone();
        match session {
            Some(session_id) => {
                if let Some(runtime) = owner.omp_runtime.get() {
                    runtime.cancel_authentication(session_id);
                }
            }
            // OMP has not acknowledged the login yet; cancel it when it does.
            None => ivars.authentication_cancelling.set(true),
        }
        self.end_sign_in_state();
        self.set_chat_status("Sign-in cancelled");
    }

    /// The user is done with the sheet, one way or another.
    fn dismiss_sign_in_sheet(&self) {
        let Some(sheet) = self.ivars().sign_in_sheet.borrow_mut().take() else {
            return;
        };
        sheet.host.endSheet(&sheet.window);
        sheet.window.orderOut(None);
    }

    /// Forgets the sign-in in progress without touching OMP: for after a
    /// terminal outcome, or once a cancel has been sent.
    fn end_sign_in_state(&self) {
        let ivars = self.ivars();
        ivars.authentication_busy.set(false);
        ivars.authentication_provider.set(None);
        ivars.authentication_method.set(None);
        ivars.authentication_session.borrow_mut().take();
        ivars.authentication_url.borrow_mut().take();
        ivars.authentication_device.borrow_mut().take();
        ivars.pending_authentication.borrow_mut().take();
        self.update_chat_controls();
        self.mark_accounts_ui_dirty();
    }

    /// Rebuilds the accounts page on the next poll rather than now: a card's
    /// own button may be what asked, and a rebuild in its handler would drop
    /// it while it is still on the stack.
    fn mark_accounts_ui_dirty(&self) {
        self.ivars().accounts_ui_dirty.set(true);
    }

    /// A sign-in ended badly. The sheet keeps the reason in front of the
    /// user with a way to try again; the state is otherwise cleared.
    fn sign_in_failed(&self, error: &str) {
        self.ivars()
            .authentication_failure
            .replace(Some(error.to_owned()));
        self.end_sign_in_state();
        self.set_chat_status(error);
        self.refresh_sign_in_sheet();
    }

    /// Opens the sign-in page in the browser the user chose, or the system's
    /// default. OMP's loopback launcher is preferred for a browser on this
    /// Mac: it redirects to the page and is what the callback expects.
    fn open_sign_in_page(&self) {
        let Some(owner) = self.ivars().authentication_owner.get()
            .and_then(|key| self.ivars().live_sessions.borrow().get(key)) else { return; };
        let key = owner.key;
        let Some((url, launch)) = self.ivars().authentication_url.borrow().clone() else {
            return;
        };
        let target = launch.unwrap_or(url);
        match self.launcher_for(launchers::LauncherKind::Browser) {
            Some(browser) => {
                let name = browser.name;
                let submitted = self.jobs().submit(move || {
                    let result = browser.launch_url(&target);
                    BackgroundEvent::Launched {
                key,
                        app: name,
                        target: std::path::PathBuf::from("the sign-in page"),
                        result,
                    }
                });
                if !submitted {
                    self.set_session_status(&owner, "Could not start the browser");
                }
            }
            None => {
                let Some(url) = NSURL::URLWithString(&NSString::from_str(&target)) else {
                    self.set_session_status(&owner, "OMP returned an invalid sign-in URL");
                    return;
                };
                if !NSWorkspace::sharedWorkspace().openURL(&url) {
                    self.set_session_status(&owner, "Could not open the sign-in page");
                }
            }
        }
    }

    /// The name on the "Open in …" button.
    fn sign_in_browser_title(&self) -> String {
        match self.launcher_for(launchers::LauncherKind::Browser) {
            Some(browser) => format!("Open in {}", browser.name),
            None => "Open in browser".to_owned(),
        }
    }

    /// Builds the sheet for `provider` showing `method`, and presents it.
    fn build_sign_in_sheet(&self, provider: AuthProvider, method: AuthMethod) {
        let mtm = self.mtm();
        let Some(host) = self.sign_in_host() else {
            return;
        };
        let width = 560.0;
        let methods = provider.methods();
        let has_picker = methods.len() > 1;
        let content_top = if has_picker { 152.0 } else { 112.0 };
        let height = sign_in_sheet_height(content_top, method);
        // SAFETY: the sheet is retained in an ivar until it is ended, and opts
        // out of AppKit's legacy release-on-close behavior.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, width, height),
                NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        let content = window.contentView().expect("sheet content view");
        let top = NSAutoresizingMaskOptions::ViewMinYMargin;
        let margin = 26.0;
        let inner = width - 2.0 * margin;

        let title = text_label(
            &format!("Sign in to {}", provider.product_name()),
            17.0,
            true,
            mtm,
        );
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        title.setFont(Some(&NSFont::systemFontOfSize_weight(17.0, unsafe {
            NSFontWeightSemibold
        })));
        title.setFrame(rect(margin, height - 52.0, inner, 24.0));
        title.setAutoresizingMask(top);
        content.addSubview(&title);
        let subtitle = text_label(
            &format!("{} · for native agent sessions", provider.account_noun()),
            11.0,
            false,
            mtm,
        );
        subtitle.setTextColor(Some(&theme::secondary_text()));
        subtitle.setFrame(rect(margin, height - 74.0, inner, 18.0));
        subtitle.setAutoresizingMask(top);
        content.addSubview(&subtitle);

        let method_picker = has_picker.then(|| {
            let picker = NSSegmentedControl::initWithFrame(
                NSSegmentedControl::alloc(mtm),
                rect(margin, height - 112.0, 130.0 * methods.len() as f64, 24.0),
            );
            picker.setSegmentCount(methods.len() as isize);
            for (index, candidate) in methods.iter().enumerate() {
                picker.setLabel_forSegment(&NSString::from_str(candidate.label()), index as isize);
            }
            picker.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
            picker.setSegmentStyle(NSSegmentStyle::Rounded);
            picker.setSegmentDistribution(NSSegmentDistribution::FillEqually);
            picker.setSelectedSegment(
                methods
                    .iter()
                    .position(|candidate| *candidate == method)
                    .unwrap_or(0) as isize,
            );
            picker.setAutoresizingMask(top);
            // SAFETY: AppDelegate implements signInMethodChanged: and outlives the sheet.
            unsafe {
                picker.setTarget(Some::<&AnyObject>(self));
                picker.setAction(Some(sel!(signInMethodChanged:)));
            }
            content.addSubview(&picker);
            picker
        });

        // Browser: the page, a way to move it, and a way to bring the
        // answer back by hand.
        let mut browser_views: Vec<Retained<NSView>> = Vec::new();
        let browser_intro = wrapping_label(
            &format!(
                "Your browser opens {}'s sign-in page. When the page says you can return to Omperor, you are done here.",
                provider.product_name()
            ),
            11.0,
            mtm,
        );
        browser_intro.setFrame(rect(margin, height - content_top - 34.0, inner, 34.0));
        browser_intro.setAutoresizingMask(top);
        content.addSubview(&browser_intro);
        browser_views.push(browser_intro.into_super().into_super());

        let row_y = height - content_top - 70.0;
        let open_width = 150.0;
        let copy_width = 64.0;
        let link_width = inner - open_width - copy_width - 16.0;
        let link_field = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            rect(margin, row_y, link_width, 24.0),
        );
        link_field.setEditable(false);
        link_field.setSelectable(true);
        link_field.setBezeled(true);
        link_field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            10.5,
            // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
            unsafe { NSFontWeightRegular },
        )));
        link_field.setTextColor(Some(&theme::secondary_text()));
        link_field.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
        link_field.setPlaceholderString(Some(ns_string!("Waiting for OMP to prepare the page…")));
        link_field.setToolTip(Some(ns_string!(
            "The sign-in page. Copy it to open it in any browser on this Mac."
        )));
        link_field.setAutoresizingMask(top);
        content.addSubview(&link_field);
        browser_views.push(link_field.clone().into_super().into_super());
        // SAFETY: AppDelegate implements both selectors and outlives the sheet.
        let copy_link = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Copy"),
                Some::<&AnyObject>(self),
                Some(sel!(signInCopyLink:)),
                mtm,
            )
        };
        copy_link.setBezelStyle(NSBezelStyle::Push);
        copy_link.setFrame(rect(
            margin + link_width + 8.0,
            row_y - 3.0,
            copy_width,
            30.0,
        ));
        copy_link.setAutoresizingMask(top);
        copy_link.setEnabled(false);
        content.addSubview(&copy_link);
        browser_views.push(copy_link.clone().into_super().into_super());
        let open_link = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&self.sign_in_browser_title()),
                Some::<&AnyObject>(self),
                Some(sel!(signInOpenBrowser:)),
                mtm,
            )
        };
        open_link.setBezelStyle(NSBezelStyle::Push);
        open_link.setFrame(rect(
            margin + link_width + copy_width + 16.0,
            row_y - 3.0,
            open_width,
            30.0,
        ));
        open_link.setAutoresizingMask(top);
        open_link.setEnabled(false);
        open_link.setToolTip(Some(ns_string!("Choose the browser in Settings › General")));
        content.addSubview(&open_link);
        browser_views.push(open_link.clone().into_super().into_super());

        let note = wrapping_label(
            "Signed in somewhere that cannot reach this Mac? Paste the address the browser ended up on, and Omperor will finish from it:",
            11.0,
            mtm,
        );
        note.setTextColor(Some(&theme::secondary_text()));
        note.setFrame(rect(margin, height - content_top - 118.0, inner, 34.0));
        note.setAutoresizingMask(top);
        content.addSubview(&note);
        browser_views.push(note.into_super().into_super());
        let use_width = 64.0;
        let callback_y = height - content_top - 154.0;
        let callback_field = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            rect(margin, callback_y, inner - use_width - 8.0, 24.0),
        );
        callback_field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            10.5,
            unsafe { NSFontWeightRegular },
        )));
        callback_field.setPlaceholderString(Some(ns_string!(
            "http://localhost:…/callback?code=…&state=…"
        )));
        callback_field.setAutoresizingMask(top);
        content.addSubview(&callback_field);
        browser_views.push(callback_field.clone().into_super().into_super());
        let use_callback = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Use"),
                Some::<&AnyObject>(self),
                Some(sel!(signInUseCallback:)),
                mtm,
            )
        };
        use_callback.setBezelStyle(NSBezelStyle::Push);
        use_callback.setFrame(rect(
            margin + inner - use_width,
            callback_y - 3.0,
            use_width,
            30.0,
        ));
        use_callback.setAutoresizingMask(top);
        use_callback.setEnabled(false);
        content.addSubview(&use_callback);
        browser_views.push(use_callback.clone().into_super().into_super());

        // A credential pasted in: a key, or a token from elsewhere.
        let mut secret_views: Vec<Retained<NSView>> = Vec::new();
        let secret_intro = wrapping_label(&sign_in_secret_intro(provider, method), 11.0, mtm);
        secret_intro.setFrame(rect(margin, height - content_top - 34.0, inner, 34.0));
        secret_intro.setAutoresizingMask(top);
        content.addSubview(&secret_intro);
        secret_views.push(secret_intro.clone().into_super().into_super());
        let secret_field = NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(mtm),
            rect(margin, height - content_top - 72.0, inner, 26.0),
        );
        secret_field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            12.0,
            unsafe { NSFontWeightRegular },
        )));
        secret_field.setAutoresizingMask(top);
        secret_field.setEnabled(false);
        content.addSubview(&secret_field);
        secret_views.push(secret_field.clone().into_super().into_super().into_super());

        // Footer, anchored to the bottom: what is happening, and the buttons.
        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            rect(margin, 26.0, 16.0, 16.0),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setIndeterminate(true);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        content.addSubview(&spinner);
        let status = text_label("", 11.0, false, mtm);
        status.setTextColor(Some(&theme::secondary_text()));
        status.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        status.setFrame(rect(margin + 22.0, 24.0, inner - 22.0 - 250.0, 20.0));
        content.addSubview(&status);
        let submit = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Sign in"),
                Some::<&AnyObject>(self),
                Some(sel!(signInSubmitSecret:)),
                mtm,
            )
        };
        submit.setBezelStyle(NSBezelStyle::Push);
        submit.setFrame(rect(width - margin - 88.0, 18.0, 88.0, 30.0));
        submit.setKeyEquivalent(ns_string!("\r"));
        submit.setEnabled(false);
        content.addSubview(&submit);
        secret_views.push(submit.clone().into_super().into_super());
        let retry = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Try again"),
                Some::<&AnyObject>(self),
                Some(sel!(signInRetry:)),
                mtm,
            )
        };
        retry.setBezelStyle(NSBezelStyle::Push);
        retry.setFrame(rect(width - margin - 88.0, 18.0, 88.0, 30.0));
        retry.setHidden(true);
        content.addSubview(&retry);
        let cancel = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Cancel"),
                Some::<&AnyObject>(self),
                Some(sel!(signInCancel:)),
                mtm,
            )
        };
        cancel.setBezelStyle(NSBezelStyle::Push);
        cancel.setFrame(rect(width - margin - 88.0 - 96.0, 18.0, 88.0, 30.0));
        cancel.setKeyEquivalent(ns_string!("\u{1b}"));
        content.addSubview(&cancel);

        host.beginSheet_completionHandler(&window, None);
        self.ivars().sign_in_sheet.replace(Some(SignInSheet {
            window,
            host: host.clone(),
            provider,
            method: Rc::new(Cell::new(method)),
            content_top,
            method_picker,
            browser_views,
            link_field,
            copy_link,
            open_link,
            callback_field,
            use_callback,
            secret_views,
            secret_intro,
            secret_field,
            submit,
            retry,
            cancel,
            spinner,
            status,
        }));
        self.show_sign_in_method(method);
    }

    /// Shows the views for `method`, sizes the sheet to them, and moves the
    /// Return key to the button that fits.
    fn show_sign_in_method(&self, method: AuthMethod) {
        let Some(sheet) = self
            .ivars()
            .sign_in_sheet
            .borrow()
            .as_ref()
            .map(SignInSheet::views)
        else {
            return;
        };
        let browser = method == AuthMethod::Browser;
        for view in &sheet.browser_views {
            view.setHidden(!browser);
        }
        for view in &sheet.secret_views {
            view.setHidden(browser);
        }
        sheet
            .secret_intro
            .setStringValue(&NSString::from_str(&sign_in_secret_intro(
                sheet.provider,
                method,
            )));
        sheet.use_callback.setKeyEquivalent(if browser {
            ns_string!("\r")
        } else {
            ns_string!("")
        });
        sheet.submit.setKeyEquivalent(if browser {
            ns_string!("")
        } else {
            ns_string!("\r")
        });
        sheet.method.set(method);
        if let Some(picker) = &sheet.method_picker
            && let Some(index) = sheet
                .provider
                .methods()
                .iter()
                .position(|candidate| *candidate == method)
        {
            picker.setSelectedSegment(index as isize);
        }
        let height = sign_in_sheet_height(sheet.content_top, method);
        let frame = sheet.window.frame();
        if (frame.size.height - height).abs() > 0.5 {
            // Grow or shrink from the top edge, which the sheet hangs from.
            let target = rect(
                frame.origin.x,
                frame.origin.y + frame.size.height - height,
                frame.size.width,
                height,
            );
            sheet.window.setFrame_display_animate(target, true, true);
        }
    }

    /// Brings the sheet up to date with the sign-in state: the page, the
    /// prompt, what is being waited on, or how it failed.
    fn refresh_sign_in_sheet(&self) {
        let ivars = self.ivars();
        let Some(sheet) = ivars
            .sign_in_sheet
            .borrow()
            .as_ref()
            .map(SignInSheet::views)
        else {
            return;
        };
        let busy = ivars.authentication_busy.get();
        let failure = ivars.authentication_failure.borrow().clone();
        let url = ivars.authentication_url.borrow().clone();
        let prompt = ivars.pending_authentication.borrow().clone();
        let submitting = ivars.authentication_submitting.get();
        let method = sheet.method.get();

        // The page, once OMP has prepared it.
        match &url {
            Some((url, _)) => {
                sheet.link_field.setStringValue(&NSString::from_str(url));
                sheet.copy_link.setEnabled(true);
                sheet.open_link.setEnabled(true);
            }
            None => {
                sheet.link_field.setStringValue(ns_string!(""));
                sheet.copy_link.setEnabled(false);
                sheet.open_link.setEnabled(false);
            }
        }
        sheet
            .open_link
            .setTitle(&NSString::from_str(&self.sign_in_browser_title()));
        // Whether OMP is waiting on an answer. What kind is the method's
        // business, not the prompt's: a browser sign-in may prompt with a
        // confirmation or with a request for the callback URL, depending on
        // how the provider's flow completes, and either is answered by the
        // pasted callback; a credential method's prompt is for the secret.
        let waiting = busy && prompt.is_some() && !submitting;
        let browser = method == AuthMethod::Browser;
        sheet.use_callback.setEnabled(waiting && browser);
        sheet.callback_field.setEnabled(waiting && browser);
        sheet.secret_field.setEnabled(waiting && !browser);
        sheet.submit.setEnabled(waiting && !browser);
        if waiting && !browser {
            sheet.window.makeFirstResponder(Some(&sheet.secret_field));
        }
        // What is happening.
        let (text, color, spinning) = if let Some(failure) = &failure {
            (failure.clone(), NSColor::systemRedColor(), false)
        } else if !busy {
            (
                "Sign-in cancelled".to_owned(),
                theme::secondary_text(),
                false,
            )
        } else if submitting {
            (
                "Checking with OMP…".to_owned(),
                theme::secondary_text(),
                true,
            )
        } else if let Some(device) = ivars.authentication_device.borrow().as_ref() {
            (
                format!("Enter {} at {} on any device", device.0, device.1),
                NSColor::labelColor(),
                true,
            )
        } else if method == AuthMethod::Browser {
            if url.is_some() {
                (
                    "Waiting for the browser…".to_owned(),
                    theme::secondary_text(),
                    true,
                )
            } else {
                (
                    "Preparing the sign-in page…".to_owned(),
                    theme::secondary_text(),
                    true,
                )
            }
        } else if waiting && !browser {
            let noun = match method {
                AuthMethod::Token => "access token",
                AuthMethod::ApiKey | AuthMethod::Browser => "API key",
            };
            (
                format!("Paste your {noun}, then press Sign in"),
                theme::secondary_text(),
                false,
            )
        } else {
            ("Starting…".to_owned(), theme::secondary_text(), true)
        };
        sheet.status.setStringValue(&NSString::from_str(&text));
        sheet.status.setTextColor(Some(&color));
        sheet.status.setToolTip(Some(&NSString::from_str(&text)));
        // SAFETY: a progress indicator accepts a missing sender for its
        // animation controls; both calls only touch the view.
        unsafe {
            if spinning {
                sheet.spinner.startAnimation(None);
            } else {
                sheet.spinner.stopAnimation(None);
            }
        }
        // After a failure the way forward is to try again; the Sign in
        // button gives way to it.
        let failed = failure.is_some();
        sheet.retry.setHidden(!failed);
        sheet
            .submit
            .setHidden(failed || method == AuthMethod::Browser);
        sheet.cancel.setTitle(if failed || !busy {
            ns_string!("Close")
        } else {
            ns_string!("Cancel")
        });
    }

    fn current_model_auth_provider(&self) -> Option<AuthProvider> {
        let owner = self.session();
        AuthProvider::for_model(&owner.current_model.borrow())
    }

    fn provider_is_authenticated(&self, provider: AuthProvider) -> bool {
        let owner = self.session();
        owner.authenticated_providers
            .borrow()
            .contains(&provider)
    }

    fn current_model_is_authenticated(&self) -> bool {
        self.current_model_auth_provider()
            .is_none_or(|provider| self.provider_is_authenticated(provider))
    }



    fn dispatch_session_prompt(&self, owner: &live_sessions::LiveSession) {
        if self.ivars().termination_pending.get() || owner.read_only_session.get() || self.fork_owns_session(owner.key) { return; }
        if owner.omp_runtime.get().is_none() && owner.pending_prompt.borrow().is_some() {
            self.resolve_session_account(owner);
            if !self.session_account_is_resolved(owner) {
                return;
            }
            let target = owner.live_session_id.borrow().clone().map_or(omp_runtime::SessionStartup::Fresh, omp_runtime::SessionStartup::Resume);
            self.launch_session_runtime(owner, owner.current_account.borrow().clone(), target);
        }
        if !owner.runtime_ready.get() && !owner.runtime_starting.get() && owner.pending_prompt.borrow().is_some() {
            if let Some(runtime) = owner.omp_runtime.get() {
                owner.runtime_starting.set(true);
                let started = if let Some(id) = owner.live_session_id.borrow().clone() {
                    owner.resuming_session_id.replace(Some(id.clone()));
                    runtime.resume_session(id, owner.current_project.borrow().clone())
                } else {
                    // Startup can fail before OMP creates a journal. There is
                    // no id to resume; explicitly retry the original launch.
                    runtime.retry_startup()
                };
                if !started {
                    owner.runtime_starting.set(false);
                    owner.resuming_session_id.borrow_mut().take();
                    owner.chat_busy.set(false);
                    owner.pending_prompt.borrow_mut().take();
                    owner.conversation.borrow_mut().notice("This runtime has retired; open a new conversation to continue.", true);
                }
            }
        }
        if !owner.runtime_ready.get() || owner.resuming_session_id.borrow().is_some() || owner.account_change_pending.get() { return; }
        let Some(prompt) = owner.pending_prompt.borrow_mut().take() else { return; };
        owner.chat_busy.set(true);
        if !owner.omp_runtime.get().is_some_and(|runtime| runtime.prompt(prompt)) {
            owner.chat_busy.set(false);
            owner.conversation.borrow_mut().fail_assistant("OMP stopped before accepting the prompt. Retry explicitly; it will not be replayed automatically.");
        }
        if self.session().key == owner.key { self.update_chat_controls(); self.mark_chat_dirty(); }
    }

    fn runtime_command_failed(&self, action: &str) {
        let owner = self.session();
        owner.runtime_ready.set(false);
        owner.runtime_starting.set(false);
        owner.new_session_pending.set(false);
        owner.pending_prompt.borrow_mut().take();
        self.ivars().authentication_busy.set(false);
        self.ivars().authentication_provider.set(None);
        self.ivars().pending_authentication.borrow_mut().take();
        owner.chat_busy.set(false);
        self.update_chat_controls();
        self.set_chat_status(&format!(
            "OMP RPC stopped while {action}; quit and reopen Omperor"
        ));
    }

    fn set_chat_busy(&self, busy: bool) {
        let owner = self.session();
        owner.chat_busy.set(busy);
        self.update_chat_controls();
        self.request_sidebar_state_refresh();
        self.request_agents_refresh();
    }

    /// The selected session owns the footer; background sessions keep their
    /// own sidebar spinners. Generic local jobs use the same footer when idle.
    fn sync_busy_indicator(&self) {
        let owner = self.session();
        let activity = owner.activity_label().or_else(|| {
            (!self.ivars().active_jobs.borrow().is_empty() || self.ivars().preference_saves_pending.get() > 0)
                .then_some("Updating…")
        });
        let in_conversation = !self.agent_view().desk.active;
        let visible = activity.is_some() && in_conversation;
        if let Some(spinner) = self.ivars().busy_spinner.get() {
            spinner.setHidden(!visible);
            // SAFETY: animation controls are called on AppKit's main thread.
            unsafe { if visible { spinner.startAnimation(None); } else { spinner.stopAnimation(None); } }
            spinner.setAccessibilityLabel(Some(&NSString::from_str(activity.unwrap_or("Idle"))));
        }
        if let Some(label) = self.ivars().busy_label.get() {
            label.setStringValue(&NSString::from_str(activity.unwrap_or("")));
            label.setHidden(!visible);
        }
        if let Some(hint) = self.ivars().composer_hint.get() { hint.setHidden(!in_conversation || visible); }
    }

    fn update_chat_controls(&self) {
        if let Some(badge) = self.ivars().cache_warning_badge.get() {
            let was_visible = !badge.isHidden();
            if self.sync_cache_warning_badge() != was_visible { self.refit_composer_chips(); }
        }
        self.sync_busy_indicator();
        let owner = self.session();
        self.sync_developer_settings();
        let mock = self.mock_backend_active();
        let ready = owner.runtime_ready.get();
        let starting = owner.runtime_starting.get();
        let authenticated = self.current_model_is_authenticated();
        let busy = owner.chat_busy.get();
        let authentication_busy = self.ivars().authentication_busy.get();
        let new_session_pending = owner.new_session_pending.get();
        let prompt_pending = owner.pending_prompt.borrow().is_some();
        let terminating = self.ivars().termination_pending.get();
        let read_only = owner.read_only_session.get();
        let resuming = owner.resuming_session_id.borrow().is_some();
        if let Some(attach) = self.ivars().attach_button.get() {
            attach.setHidden(busy || self.agent_view().desk.active);
            attach.setEnabled(!terminating && self.ivars().can_receive_attachments(&owner));
        }
        let can_submit_prompt = !read_only
            && !self.fork_owns_session(owner.key)
            && !resuming
            && !prompt_pending
            && !owner.attachment_submission.get()
            && owner.session_history.borrow().is_none()
            && !authentication_busy
            && (owner.omp_runtime.get().is_none() || matches!(owner.lifecycle.get(), live_sessions::Lifecycle::Failed | live_sessions::Lifecycle::History) || (self.current_account_is_resolved()
            && prompt_submission_enabled(authenticated, ready, starting, new_session_pending)));
        let can_submit = !terminating && can_submit_prompt;
        // Typing never waits for history, a resume, or startup: only sending
        // does, and the send action says why when it cannot go yet.
        self.ivars()
            .input
            .get()
            .expect("chat input must exist before changing its state")
            .setEditable(!terminating && !read_only && !self.fork_owns_session(owner.key));
        let button = self
            .ivars()
            .send_button
            .get()
            .expect("send button must exist before changing its state");
        let attachments = owner.pending_attachments.borrow();
        button.setEnabled(can_submit && !attachments.blocked() && !(busy && !attachments.items.is_empty()));
        button.setToolTip(Some(if busy {
            ns_string!("Steer active turn")
        } else {
            ns_string!("Send")
        }));
        self.ivars()
            .stop_button
            .get()
            .expect("stop button must exist before changing its state")
            .setEnabled(!read_only && ready && busy && !terminating);
        if let Some(stop) = self.ivars().stop_button.get() {
            stop.setHidden(!busy);
        }
        let connect_button = self
            .ivars()
            .connect_button
            .get()
            .expect("connect button must exist before changing its state");
        let model_provider = self.current_model_auth_provider();
        connect_button.setEnabled(
            model_provider.is_some()
                && ready
                && !authenticated
                && !authentication_busy
                && !terminating,
        );
        let connect_title = if let Some(provider) = model_provider {
            if authenticated {
                let current = owner.current_account
                    .borrow()
                    .as_deref()
                    .and_then(|id| self.account_entry(id));
                match current {
                    Some(entry) => {
                        format!("{} · {}", provider.product_name(), entry.display_name())
                    }
                    None => format!("{} Connected", provider.product_name()),
                }
            } else if self.ivars().authentication_provider.get() == Some(provider) {
                "Connecting…".into()
            } else {
                format!("Connect {}", provider.product_name())
            }
        } else {
            "Accounts in Settings".into()
        };
        connect_button.setTitle(&NSString::from_str(&connect_title));
        self.ivars()
            .project_button
            .get()
            .expect("project button must exist before changing its state")
            .setEnabled(!terminating);
        let new_chat_enabled = !terminating;
        self.ivars()
            .new_chat_button
            .get()
            .expect("new chat button must exist before changing its state")
            .setEnabled(new_chat_enabled);
        self.ivars()
            .new_chat_menu_item
            .get()
            .expect("new chat menu item must exist before changing its state")
            .setEnabled(new_chat_enabled);
        self.ivars()
            .model_picker
            .get()
            .expect("model picker must exist before changing its state")
            .setEnabled(!mock && !read_only && ready && !busy && !authentication_busy && !terminating);
        self.ivars()
            .thinking_picker
            .get()
            .expect("thinking picker must exist before changing its state")
            .setEnabled(!mock && !read_only && ready && !busy && !authentication_busy && !terminating);
        if let Some(button) = self.ivars().fast_button.get() {
            button.setEnabled(!mock && !read_only && ready && !busy && !authentication_busy && !terminating);
        }
        self.ivars()
            .account_picker
            .get()
            .expect("account picker must exist before changing its state")
            .setEnabled(
                !read_only
                    && ready
                    && !busy
                    && !authentication_busy
                    && !terminating
                    && !owner.account_change_pending.get(),
            );
        if let Some(settings) = self.ivars().settings_ui.get() {
            let agent_controls_enabled = !mock && ready && !busy && !authentication_busy && !terminating;
            settings.model_picker.setEnabled(agent_controls_enabled);
            settings.thinking_picker.setEnabled(agent_controls_enabled);
        }
        self.sync_account_controls();
        self.sync_settings_account_ui();
    }

    fn populate_model_picker(&self, models: &[String], active: &str) {
        let owner = self.session();
        let picker = self
            .ivars()
            .model_picker
            .get()
            .expect("model picker must exist before loading models");
        picker.removeAllItems();
        let authenticated = owner.authenticated_providers.borrow();
        let titles = model_picker_options(models, active, &authenticated)
            .into_iter()
            .map(NSString::from_str)
            .collect::<Vec<_>>();
        let titles = NSArray::from_retained_slice(&titles);
        picker.addItemsWithTitles(&titles);
        picker.selectItemWithTitle(&NSString::from_str(active));
        self.set_model_label(active);
        if let Some(settings) = self.ivars().settings_ui.get() {
            settings.model_picker.removeAllItems();
            settings.model_picker.addItemsWithTitles(&titles);
            settings
                .model_picker
                .selectItemWithTitle(&NSString::from_str(active));
            settings
                .model_picker
                .setEnabled(owner.runtime_ready.get() && !self.mock_backend_active());
        }
    }

    fn set_model_label(&self, model: &str) {
        let display = if model == mock_inference::MODEL { "Mock backend" } else { model.rsplit('/').next().unwrap_or(model) };
        self.ivars()
            .model_label
            .get()
            .expect("model label exists before model updates")
            .setStringValue(&NSString::from_str(&format!("{display}  ▾")));
        self.refit_composer_chips();
    }

    fn set_thinking_label(&self, level: &str) {
        // Keep the native selection (and its accessibility value) aligned with
        // the visible chip when readiness, session switches, or RPC updates
        // change the level without a user menu selection.
        if let Some(picker) = self.ivars().thinking_picker.get() {
            picker.selectItemWithTitle(&NSString::from_str(level));
        }
        if let Some(settings) = self.ivars().settings_ui.get() {
            settings.thinking_picker.selectItemWithTitle(&NSString::from_str(level));
        }
        self.ivars()
            .thinking_label
            .get()
            .expect("thinking label exists before reasoning updates")
            .setStringValue(&NSString::from_str(&format!("Reasoning: {level}  ▾")));
        self.refit_composer_chips();
    }

    /// Align the composer with the reading column. All inputs remain native
    /// controls; their visible chips and hit areas share the same geometry.
    fn refit_composer_chips(&self) {
        self.sync_header_identity();
        let owner = self.session();
        let ivars = self.ivars();
        let (Some(surface), Some(input), Some(model), Some(reasoning), Some(fast), Some(account)) = (
            ivars.composer_surface.get(),
            ivars.input.get(),
            ivars.model_label.get(),
            ivars.thinking_label.get(),
            ivars.fast_label.get(),
            ivars.account_label.get(),
        ) else {
            return;
        };
        // SAFETY: the composer owns a place in the conversation pane.
        let Some(pane) = (unsafe { surface.superview() }) else {
            return;
        };
        let prefs = ivars.preferences.borrow();
        let typography =
            native_markdown::Typography::new(prefs.chat_style, prefs.chat_text_size.scale())
                .with_sizes(prefs.chat_text_sizes);
        let measure = typography.measure();
        drop(prefs);
        let input_size = typography.size(TextRole::Composer);
        input.setFont(Some(&typography.interface_font(TextRole::Composer)));
        let input_height = layout::composer_input_height(input_size);
        for label in [model, reasoning, fast, account] {
            label.setFont(Some(&typography.interface_font(TextRole::Controls)));
        }
        let labels = [model, reasoning, fast, account];
        let natural = labels.map(|label| {
            label.sizeToFit();
            if label.isHidden() {
                0.0
            } else {
                label.frame().size.width + 18.0
            }
        });
        let mut natural = natural;
        if !fast.isHidden() {
            natural[2] += 16.0;
        }

        let column = layout::reading_column(pane.frame().size.width, measure);
        let cache_warning = self.sync_cache_warning_badge();
        let positions = layout::composer_chips(column.width, natural, cache_warning);
        let input_y = layout::composer_input_y(&positions);
        let attachment_height = if owner.pending_attachments.borrow().items.is_empty() { 0.0 } else { 140.0 };
        let extra_height = input_height - 62.0 + input_y - 86.0 + attachment_height;
        if let Some(scroll) = ivars
            .chat_transcript
            .get()
            .map(|transcript| transcript.scroll_view().clone())
        {
            // The frame-timing readout, when shown, takes a band above the
            // composer so nothing under it need be redrawn for its refresh.
            let band = ivars
                .frame_overlay
                .get()
                .map_or(0.0, |overlay| overlay.band());
            let mut frame = scroll.frame();
            frame.origin.y = 218.0 + extra_height + band;
            frame.size.height = (pane.bounds().size.height - 380.0 - layout::USAGE_STRIP_HEIGHT - extra_height - band
                - self.account_banner_space(pane.bounds().size)).max(100.0);
            if scroll.frame() != frame {
                scroll.setFrame(frame);
            }
            if let Some(overlay) = ivars.frame_overlay.get() {
                overlay.place(pane.bounds().size.width, 40.0 + layout::COMPOSER_HEIGHT + extra_height);
            }
        }
        let bottom = 40.0;
        surface.setFrame(rect(
            column.x,
            bottom,
            column.width,
            layout::COMPOSER_HEIGHT + extra_height,
        ));
        input.setFrame(rect(
            column.x + 20.0,
            bottom + input_y,
            column.width - 40.0,
            input_height,
        ));
        self.layout_pending_attachments(20.0, input_y + input_height + 8.0, column.width - 40.0);
        if let Some(hint) = ivars.composer_hint.get() {
            hint.setFont(Some(&typography.interface_font(TextRole::Metadata)));
            hint.setFrame(rect(column.x, 9.0, column.width, 24.0));
            hint.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            hint.setStringValue(if column.width < 550.0 {
                ns_string!("Return to send · Shift Return for a new line")
            } else {
                ns_string!("Return to send · Shift Return for a new line · Paste images or drop files")
            });
        }
        if let Some(spinner) = ivars.busy_spinner.get() {
            spinner.setFrame(rect(column.x + 4.0, 15.0, 12.0, 12.0));
        }
        if let Some(label) = ivars.busy_label.get() {
            label.setFont(Some(&typography.interface_font(TextRole::Metadata)));
            label.setFrame(rect(column.x + 24.0, 9.0, column.width - 24.0, 24.0));
        }
        self.sync_busy_indicator();
        let chips = [
            ivars.model_chip.get(),
            ivars.thinking_chip.get(),
            ivars.fast_chip.get(),
            ivars.account_chip.get(),
        ];
        for (index, ((label, chip), position)) in
            labels.into_iter().zip(chips).zip(positions).enumerate()
        {
            let Some(chip) = chip else {
                continue;
            };
            let x = column.x + position.x;
            let y = bottom + position.y;
            let leading = if index == 2 { 25.0 } else { 9.0 };
            let height = label.frame().size.height;
            chip.setFrame(rect(x, y, position.width, layout::CHIP_HEIGHT));
            chip.setHidden(label.isHidden());
            label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            label.setFrame(rect(
                x + leading,
                y + (layout::CHIP_HEIGHT - height) / 2.0,
                (position.width - leading - 9.0).max(1.0),
                height,
            ));
            label.setToolTip(Some(&label.stringValue()));
            if index == 2 {
                if let Some(icon) = ivars.fast_icon.get() {
                    icon.setFrame(rect(x + 9.0, y + 9.0, 12.0, 12.0));
                }
                if let Some(button) = ivars.fast_button.get() {
                    button.setFrame(chip.frame());
                }
            } else {
                let picker = match index {
                    0 => ivars.model_picker.get(),
                    1 => ivars.thinking_picker.get(),
                    _ => ivars.account_picker.get(),
                };
                if let Some(picker) = picker {
                    picker.setFrame(chip.frame());
                }
            }
        }
        if let Some(send) = ivars.send_button.get() {
            send.setFrame(rect(
                column.x + column.width - 52.0,
                bottom + 6.0,
                40.0,
                40.0,
            ));
        }
        if let Some(badge) = ivars.cache_warning_badge.get() {
            badge.setFrame(rect(column.x + column.width - 100.0 - layout::CACHE_BADGE_WIDTH, bottom + 12.0, layout::CACHE_BADGE_WIDTH, 28.0));
        }
        if let Some(attach) = ivars.attach_button.get() {
            attach.setFrame(rect(column.x + column.width - 92.0, bottom + 8.0, 36.0, 36.0));
        }
        if let Some(stop) = ivars.stop_button.get() {
            stop.setFrame(rect(
                column.x + column.width - 92.0,
                bottom + 8.0,
                36.0,
                36.0,
            ));
        }
        if self.file_picker_visible() {
            self.rebuild_file_picker();
        }
        self.layout_session_workspace();
    }

    fn request_model_change(&self, model: String) {
        let owner = self.session();
        if self.mock_backend_active() { return; }
        let current = owner.current_model.borrow().clone();
        if model_change_requires_new_session(&current, &model) {
            // A provider switch needs its own account settled before the
            // fresh session launches; a cancelled choice keeps the old model.
            let Some(account) = self.account_for_new_provider_session(&model) else {
                self.show_model_selection(&current);
                return;
            };
            self.start_new_session_with_model(model, account);
            return;
        }
        self.set_chat_status(&format!("Switching OMP to {model}…"));
        let sent = owner.omp_runtime
            .get()
            .is_some_and(|runtime| runtime.set_model(model.clone()));
        if sent {
            self.note_cache_action(cache_policy::CacheAction::ModelSwitch { from: &current, to: &model });
        } else {
            self.runtime_command_failed("changing the model");
        }
    }

    fn start_new_session_with_model(&self, model: String, account: Option<String>) {
        let previous = self.session();
        let warning = self.cache_warning_for_action(cache_policy::CacheAction::ModelSwitch {
            from: &previous.current_model.borrow(), to: &model,
        });
        let directory = previous.current_project.borrow().clone();
        let project = previous.active_project_id.borrow().clone();
        self.capture_selected_binding();
        let owner = self.ivars().live_sessions.borrow_mut().create();
        owner.current_project.replace(directory);
        owner.active_project_id.replace(project);
        owner.current_model.replace(model.clone());
        owner.current_account.replace(account.clone());
        owner.cache_warning.replace(warning);
        owner.new_session_pending.set(true);
        self.ivars().preferences.borrow_mut().model = Some(model);
        self.store_preferences();
        self.ivars().live_sessions.borrow_mut().select(owner.key);
        self.launch_session_runtime(&owner, account, omp_runtime::SessionStartup::Fresh);
        self.render_selected_binding();
    }

    fn request_thinking_change(&self, level: String) {
        let owner = self.session();
        if self.mock_backend_active() { return; }
        let current = owner.current_thinking_level.borrow().clone();
        self.set_thinking_label(&level);
        self.set_chat_status(&format!("Setting OMP reasoning to {level}…"));
        let sent = owner.omp_runtime
            .get()
            .is_some_and(|runtime| runtime.set_thinking_level(level.clone()));
        if sent {
            self.note_cache_action(cache_policy::CacheAction::ReasoningChange { from: &current, to: &level });
        } else {
            self.runtime_command_failed("changing the reasoning level");
        }
    }

    /// Asks OMP for, or releases, the OpenAI `priority` tier on this owner.
    /// The chip answers at once; OMP's confirmation settles it.
    fn request_fast_mode(&self, enabled: bool) {
        let owner = self.session();
        if self.mock_backend_active() { return; }
        let previous = owner.fast_mode.replace(enabled);
        self.refresh_fast_chip();
        self.set_chat_status(if enabled {
            "Turning Fast on…"
        } else {
            "Turning Fast off…"
        });
        let sent = owner.omp_runtime
            .get()
            .is_some_and(|runtime| runtime.set_fast_mode(enabled));
        if sent {
            self.note_cache_action(cache_policy::CacheAction::FastModeChange { from: previous, to: enabled });
        } else {
            self.runtime_command_failed("changing fast mode");
        }
    }

    /// Shows the Fast chip when the model in view is offered the tier, and
    /// colors it by whether the session asks for it: gold when on, the quiet
    /// chip surface when off.
    fn refresh_fast_chip(&self) {
        let owner = self.session();
        let ivars = self.ivars();
        let (Some(chip), Some(icon), Some(label), Some(button)) = (
            ivars.fast_chip.get(),
            ivars.fast_icon.get(),
            ivars.fast_label.get(),
            ivars.fast_button.get(),
        ) else {
            return;
        };
        let model = owner.current_model.borrow().clone();
        let offered = owner.fast_models.borrow().iter().any(|fast| fast == &model);
        let on = owner.fast_mode.get();
        label.setHidden(!offered);
        icon.setHidden(!offered);
        chip.setHidden(!offered);
        button.setHidden(!offered);
        if offered {
            let (fill, ink) = if on {
                (theme::accent_fill_strong(), theme::accent())
            } else {
                (theme::surface_inset(), theme::secondary_text())
            };
            chip.setFillColor(&fill);
            label.setTextColor(Some(&ink));
            icon.setContentTintColor(Some(&ink));
            button.setToolTip(Some(&NSString::from_str(if on {
                "Fast is on: OpenAI priority processing. Quicker replies; each turn uses more of your plan. Click to turn off."
            } else {
                "Turn Fast on: OpenAI priority processing. Quicker replies; each turn uses more of your plan."
            })));
        }
        self.refit_composer_chips();
    }

    fn apply_appearance(&self, preference: AppearancePreference) {
        let appearance = match preference {
            AppearancePreference::System => None,
            AppearancePreference::Light => {
                // SAFETY: AppKit exports this appearance name for the process lifetime.
                NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua })
            }
            AppearancePreference::Dark => {
                // SAFETY: AppKit exports this appearance name for the process lifetime.
                NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua })
            }
        };
        NSApplication::sharedApplication(self.mtm()).setAppearance(appearance.as_deref());
    }

    fn reveal_directory(&self, path: &std::path::Path) {
        self.prepare_reveal(path.to_owned(), true);
    }

    fn prepare_reveal(&self, path: std::path::PathBuf, directory: bool) {
        let job = self.begin_job("Opening folder…");
        if !self.jobs().submit(move || {
            let result = (|| {
                if !directory && path.is_file() { return Ok((path, false)); }
                let path = if directory { path } else { path.parent().unwrap_or(&path).to_owned() };
                std::fs::create_dir_all(&path).map_err(|error| format!("Could not create {}: {error}", path.display()))?;
                Ok((path, true))
            })();
            BackgroundEvent::RevealPrepared { job, result }
        }) {
            self.finish_job(job);
            self.set_chat_status("Background workers are unavailable");
        }
    }

    fn show_settings_section(&self, selected: usize) {
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        for (index, page) in settings.pages.iter().enumerate() {
            page.setHidden(index != selected);
        }
        if selected == SETTINGS_ACCOUNTS_PAGE {
            self.scroll_accounts_to_top();
            self.request_account_usage(false);
        }
        if selected == SETTINGS_ARCHIVES_PAGE {
            self.sync_settings_archives_ui();
            self.request_archives_list(true);
        }
        if selected == SETTINGS_METRICS_PAGE {
            self.choose_default_metrics_session();
            self.sync_settings_metrics_ui();
            self.scroll_metrics_to_top();
            self.request_metrics_read();
        }
        for button in &settings.category_buttons {
            let is_selected = button.tag() == selected as isize;
            let color = if is_selected {
                theme::accent()
            } else {
                NSColor::labelColor()
            };
            let font = if is_selected {
                NSFont::boldSystemFontOfSize(13.0)
            } else {
                NSFont::systemFontOfSize(13.0)
            };
            button.setContentTintColor(Some(&color));
            button.setFont(Some(&font));
        }
    }

    fn sync_settings_ui(&self) {
        let owner = self.session();
        self.sync_developer_settings();
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        settings.workspace_label.setStringValue(&NSString::from_str(
            &owner.current_project.borrow().display().to_string(),
        ));

        let active_model = owner.current_model.borrow().clone();
        let models = owner.available_models.borrow();
        settings.model_picker.removeAllItems();
        if active_model.is_empty() && models.is_empty() {
            settings
                .model_picker
                .addItemWithTitle(ns_string!("Loading models…"));
            settings.model_picker.setEnabled(false);
        } else {
            let preferred = if active_model.is_empty() {
                self.ivars()
                    .preferences
                    .borrow()
                    .model
                    .clone()
                    .unwrap_or_default()
            } else {
                active_model
            };
            let authenticated = owner.authenticated_providers.borrow();
            let titles = model_picker_options(&models, &preferred, &authenticated)
                .into_iter()
                .map(NSString::from_str)
                .collect::<Vec<_>>();
            settings
                .model_picker
                .addItemsWithTitles(&NSArray::from_retained_slice(&titles));
            settings
                .model_picker
                .selectItemWithTitle(&NSString::from_str(&preferred));
            settings
                .model_picker
                .setEnabled(owner.runtime_ready.get() && !self.mock_backend_active());
        }

        self.sync_text_size_settings();
        self.sync_preference_save_status();
        let preferences = self.ivars().preferences.borrow();
        let thinking = if owner.current_thinking_level.borrow().is_empty() {
            preferences
                .thinking_level
                .as_deref()
                .unwrap_or("medium")
                .to_owned()
        } else {
            owner.current_thinking_level.borrow().clone()
        };
        settings
            .thinking_picker
            .selectItemWithTitle(&NSString::from_str(&thinking));
        settings
            .thinking_picker
            .setEnabled(owner.runtime_ready.get() && !self.mock_backend_active());
        settings.computer_use.setState(if preferences.computer_use { NSControlStateValueOn } else { NSControlStateValueOff });
        settings
            .appearance_picker
            .selectItemWithTitle(&NSString::from_str(preferences.appearance.label()));
        settings
            .text_size_picker
            .selectItemWithTitle(&NSString::from_str(preferences.chat_text_size.label()));
        settings
            .chat_style_picker
            .selectItemWithTitle(&NSString::from_str(preferences.chat_style.label()));
        for (picker, kind, preference) in [
            (
                &settings.editor_picker,
                launchers::LauncherKind::Editor,
                preferences.editor.as_deref(),
            ),
            (
                &settings.terminal_picker,
                launchers::LauncherKind::Terminal,
                preferences.terminal.as_deref(),
            ),
            (
                &settings.browser_picker,
                launchers::LauncherKind::Browser,
                preferences.browser.as_deref(),
            ),
        ] {
            let installed = launchers::installed(kind);
            picker.removeAllItems();
            // A page with no browser chosen goes to the system default, not
            // to the first browser on Omperor's list.
            let bare = if kind == launchers::LauncherKind::Browser {
                SETTINGS_SYSTEM_BROWSER
            } else {
                SETTINGS_AUTOMATIC_APP
            };
            let automatic = match (kind, installed.first()) {
                (launchers::LauncherKind::Browser, _) => bare.to_owned(),
                (_, Some(first)) => format!("{bare} ({})", first.name),
                (_, None) => bare.to_owned(),
            };
            picker.addItemWithTitle(&NSString::from_str(&automatic));
            // The automatic item keeps its bare name for the handler, which
            // compares titles; the parenthetical is a menu-side courtesy.
            if let Some(item) = picker.itemAtIndex(0) {
                item.setTitle(&NSString::from_str(bare));
                item.setToolTip(Some(&NSString::from_str(&automatic)));
            }
            for launcher in &installed {
                picker.addItemWithTitle(&NSString::from_str(launcher.name));
            }
            let selected = preference
                .filter(|name| installed.iter().any(|launcher| launcher.name == *name))
                .unwrap_or(bare);
            picker.selectItemWithTitle(&NSString::from_str(selected));
            // The system default is always a choice for a browser.
            picker.setEnabled(kind == launchers::LauncherKind::Browser || !installed.is_empty());
        }
        settings
            .inline_images
            .setState(if preferences.show_inline_images {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        settings
            .remote_images
            .setState(if preferences.load_remote_images {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        settings.remote_images.setEnabled(preferences.show_inline_images);
        drop(preferences);
        self.sync_settings_account_ui();
        self.sync_settings_projects_ui();
    }

    /// Rebuilds the Accounts page: one card per provider listing every stored
    /// account with its label, identity, default marker, and actions.
    /// Asks every account's provider for its plan and quota windows, on the
    /// worker in the child. Skipped while a fetch is in flight and, unless
    /// `force`, while the last answer is younger than a couple of minutes.
    fn request_account_usage(&self, force: bool) {
        let owner = self.session();
        let ivars = self.ivars();
        if !owner.runtime_ready.get() || ivars.account_usage_loading.get() {
            return;
        }
        if !force
            && ivars
                .account_usage_fetched_at
                .get()
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(120))
        {
            return;
        }
        if ivars.accounts.borrow().is_empty() {
            return;
        }
        let sent = owner.omp_runtime
            .get()
            .is_some_and(|runtime| runtime.refresh_account_usage());
        if sent {
            ivars.account_usage_loading.set(true);
            ivars
                .account_usage_fetched_at
                .set(Some(std::time::Instant::now()));
            self.sync_settings_account_ui();
        }
    }

    fn sync_settings_account_ui(&self) {
        let owner = self.session();
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        // Control-state updates can arrive every frame while inference streams.
        // A retained but hidden Settings window needs no native card rebuilding.
        if !settings.window.isVisible() || settings.pages[SETTINGS_ACCOUNTS_PAGE].isHidden() {
            self.ivars().accounts_ui_dirty.set(true);
            return;
        }
        // Scroller geometry can synchronously reenter layout while cards are
        // being inserted. Do not append a second copy of the same controls.
        if self.ivars().settings_laying_out.replace(true) { return; }
        let _reset = ResetOnDrop(&self.ivars().settings_laying_out);
        let mtm = self.mtm();
        let content = &settings.account_content;
        // A rebuild (usage arriving, say) keeps the reader where they were,
        // measured from the top since the document is laid out top-down.
        let from_top = {
            let visible = settings.account_scroll.contentView().bounds();
            let old_height = content.frame().size.height;
            (old_height - (visible.origin.y + visible.size.height)).max(0.0)
        };
        let previous = settings.account_content.subviews();
        for index in 0..previous.count() {
            previous.objectAtIndex(index).removeFromSuperview();
        }
        let ready = owner.runtime_ready.get();
        let busy = self.ivars().authentication_busy.get();
        let active = self.ivars().authentication_provider.get();
        let accounts = self.ivars().accounts.borrow().clone();
        let usage = self.ivars().account_usage.borrow().clone();
        let expanded = self.ivars().accounts_usage_expanded.borrow().clone();
        let usage_loading = self.ivars().account_usage_loading.get();
        let tab = self
            .ivars()
            .accounts_tab
            .get()
            .min(AuthProvider::ALL.len() - 1);
        let provider = AuthProvider::ALL[tab];
        let provider_index = tab;
        settings.account_tabs.setSelectedSegment(tab as isize);

        const CARD_X: f64 = 40.0;
        const CARD_GAP: f64 = 14.0;
        const INTRO_HEIGHT: f64 = 30.0;
        const FOOTER_HEIGHT: f64 = 48.0;
        const TOP_PADDING: f64 = 6.0;
        // The cards widen with the window, within reason, so a bigger
        // window gives the bars and reset times more room.
        let content_width = settings.account_scroll.frame().size.width;
        let card_width = (content_width - 2.0 * CARD_X).clamp(600.0, 900.0);
        let right = card_width - 20.0;

        let rows = accounts
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.provider() == provider)
            .map(|(index, entry)| {
                let mut plan = account_card_plan(
                    provider,
                    usage.get(&entry.summary.id),
                    usage_loading,
                    entry.is_usable(),
                    expanded.contains(&entry.summary.id),
                );
                if let Some(alert) = owner.account_alerts.borrow().for_account(entry.id()) {
                    plan.note = Some(format!("Account needs attention: {}.", alert.reason));
                    plan.note_is_error = true;
                }
                (index, entry.clone(), plan)
            })
            .collect::<Vec<_>>();
        let content_height = TOP_PADDING
            + INTRO_HEIGHT
            + rows
                .iter()
                .map(|(_, _, plan)| plan.height() + CARD_GAP)
                .sum::<f64>()
            + FOOTER_HEIGHT;
        let visible_height = settings.account_scroll.frame().size.height;
        let document_height = content_height.max(visible_height);
        content.setFrame(rect(0.0, 0.0, content_width, document_height));

        // What this provider is, in a line above its accounts.
        let intro = text_label(provider_account_detail(provider), 12.0, false, mtm);
        intro.setTextColor(Some(&theme::secondary_text()));
        intro.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        intro.setFrame(rect(
            CARD_X + 2.0,
            document_height - TOP_PADDING - 22.0,
            card_width,
            18.0,
        ));
        content.addSubview(&intro);

        let highlight = self.ivars().account_highlight.borrow().clone();
        let mut highlight_card: Option<Retained<NSBox>> = None;
        let mut top = TOP_PADDING + INTRO_HEIGHT;
        for (account_index, entry, plan) in &rows {
            let height = plan.height();
            let card = settings_card(
                rect(CARD_X, document_height - top - height, card_width, height),
                mtm,
            );
            if highlight.as_deref() == Some(entry.id()) {
                card.setBorderColor(&theme::accent());
                card.setBorderWidth(2.0);
                highlight_card = Some(card.clone());
            }
            let mut y = height - 16.0;
            // The account: its name, when it signed in, and its controls.
            let name = text_label(&entry.display_name(), 13.0, true, mtm);
            name.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            name.setFrame(rect(20.0, y - 20.0, right - 290.0, 20.0));
            card.addSubview(&name);
            let default_button = settings_radio(
                "Default",
                rect(right - 250.0, y - 19.0, 84.0, 20.0),
                self,
                sel!(settingsMakeDefaultAccount:),
                mtm,
            );
            default_button.setTag(*account_index as isize);
            default_button.setState(if entry.is_default {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            default_button.setEnabled(entry.is_usable());
            card.addSubview(&default_button);
            let rename = settings_button(
                if entry.label.is_some() {
                    "Rename…"
                } else {
                    "Label…"
                },
                rect(right - 160.0, y - 24.0, 76.0, 28.0),
                self,
                sel!(settingsRenameAccount:),
                mtm,
            );
            rename.setTag(*account_index as isize);
            card.addSubview(&rename);
            let remove = settings_button(
                "Remove…",
                rect(right - 78.0, y - 24.0, 78.0, 28.0),
                self,
                sel!(settingsRemoveAccount:),
                mtm,
            );
            remove.setTag(*account_index as isize);
            remove.setEnabled(ready && !busy);
            card.addSubview(&remove);
            let mut identity = if entry.label.is_some() {
                entry.identity()
            } else {
                "Unlabeled".to_owned()
            };
            match entry.summary.state {
                omp_runtime::AccountState::Active => {}
                omp_runtime::AccountState::RefreshRequired => {
                    identity.push_str(" • needs refresh");
                }
                omp_runtime::AccountState::Disabled => identity.push_str(" • disabled"),
                omp_runtime::AccountState::LoggedOut => identity.push_str(" • signed out"),
            }
            let identity_label = text_label(&identity, 11.0, false, mtm);
            identity_label.setTextColor(Some(&theme::secondary_text()));
            identity_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
            identity_label.setFrame(rect(20.0, y - 40.0, right - 290.0, 16.0));
            card.addSubview(&identity_label);
            y -= 52.0;
            settings_separator(&card, y, mtm);
            y -= 14.0;

            // The plan the provider reports.
            let plan_heading = section_label("Plan", mtm);
            plan_heading.setFrame(rect(20.0, y - 14.0, 200.0, 14.0));
            card.addSubview(&plan_heading);
            y -= 20.0;
            // A sign-in OMP would no longer accept, or one whose credential
            // failed at runtime (a revoked token the account list still shows
            // as active), earns a re-authorize button beside its plan.
            let usage_auth_error = usage
                .get(&entry.summary.id)
                .and_then(|usage| usage.error.as_deref())
                .is_some_and(omp_runtime::is_account_auth_failure);
            let alerted = owner.account_alerts.borrow().needs_reauth(entry.id());
            let needs_reauth = matches!(
                entry.summary.state,
                omp_runtime::AccountState::LoggedOut
                    | omp_runtime::AccountState::Disabled
                    | omp_runtime::AccountState::RefreshRequired
            ) || usage_auth_error
                || alerted
                || owner.account_alerts.borrow().for_provider(entry.provider()).is_some();
            let plan_value = text_label(&plan.plan, 13.0, false, mtm);
            plan_value.setTextColor(Some(&plan.color()));
            plan_value.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            let plan_value_width = if needs_reauth {
                right - 20.0 - 150.0
            } else {
                right - 20.0
            };
            plan_value.setFrame(rect(20.0, y - 20.0, plan_value_width, 20.0));
            card.addSubview(&plan_value);
            if needs_reauth {
                let reauth = settings_button(
                    "Sign in again",
                    rect(right - 140.0, y - 23.0, 140.0, 27.0),
                    self,
                    sel!(settingsReauthAccount:),
                    mtm,
                );
                reauth.setTag(*account_index as isize);
                reauth.setEnabled(!busy && owner.omp_runtime.get().is_some());
                card.addSubview(&reauth);
            }
            y -= 22.0;
            if let Some(note) = &plan.note {
                let note_label = text_label(note, 11.0, false, mtm);
                let note_color = if plan.note_is_error {
                    theme::plum()
                } else {
                    NSColor::secondaryLabelColor()
                };
                note_label.setTextColor(Some(&note_color));
                note_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
                note_label.setToolTip(Some(&NSString::from_str(note)));
                note_label.setFrame(rect(20.0, y - 16.0, right - 20.0, 18.0));
                card.addSubview(&note_label);
                y -= 18.0;
            }

            // Each quota window as a bar: how much is left, and when it resets.
            // Folded, the card shows only the window that runs out first;
            // a chevron beside the heading unfolds every window. The figure
            // column is one width for the whole card so every bar ends on
            // the same line; money needs more room than "100% left".
            let shown = plan.shown_groups();
            let figure_width = if shown
                .iter()
                .any(|group| group.rows.iter().any(|row| row.figure.is_some()))
            {
                110.0
            } else {
                62.0
            };
            for (group_index, group) in shown.iter().enumerate() {
                y -= 12.0;
                let heading = section_label(&group.title, mtm);
                heading.setFrame(rect(20.0, y - 14.0, 400.0, 14.0));
                card.addSubview(&heading);
                if group_index == 0 && plan.foldable() {
                    let toggle = usage_details_toggle(
                        plan.expanded,
                        rect(right - 120.0, y - 17.0, 120.0, 18.0),
                        *account_index,
                        self,
                        mtm,
                    );
                    card.addSubview(&toggle);
                }
                y -= 20.0;
                for row in &group.rows {
                    // The bar and its figure hug the card's right edge; the
                    // text takes what is left.
                    let bar_width = (card_width * 0.3).clamp(140.0, 260.0);
                    let bar_x = right - figure_width - 6.0 - bar_width;
                    let title = text_label(&row.title, 12.0, false, mtm);
                    title.setFrame(rect(20.0, y - 18.0, bar_x - 32.0, 18.0));
                    card.addSubview(&title);
                    if let Some(detail) = &row.detail {
                        let detail_label = text_label(detail, 11.0, false, mtm);
                        detail_label.setTextColor(Some(&theme::secondary_text()));
                        detail_label.setFrame(rect(20.0, y - 34.0, bar_x - 32.0, 14.0));
                        card.addSubview(&detail_label);
                    }
                    if let Some(left) = row.left_percent {
                        let track = fill_box(
                            rect(bar_x, y - 16.0, bar_width, 6.0),
                            &theme::surface_inset(),
                            3.0,
                            mtm,
                        );
                        card.addSubview(&track);
                        let width = bar_width * f64::from(left) / 100.0;
                        if width >= 1.0 {
                            let fill_color = if left <= 10 {
                                NSColor::systemOrangeColor()
                            } else {
                                theme::accent()
                            };
                            let fill =
                                fill_box(rect(bar_x, y - 16.0, width, 6.0), &fill_color, 3.0, mtm);
                            card.addSubview(&fill);
                        }
                        let figure = row
                            .figure
                            .clone()
                            .unwrap_or_else(|| format!("{left}% left"));
                        let left_label = text_label(&figure, 12.0, false, mtm);
                        left_label.setAlignment(NSTextAlignment::Right);
                        let left_color = if left <= 10 {
                            NSColor::systemOrangeColor()
                        } else {
                            NSColor::labelColor()
                        };
                        left_label.setTextColor(Some(&left_color));
                        left_label.setFrame(rect(
                            right - figure_width,
                            y - 21.0,
                            figure_width,
                            18.0,
                        ));
                        card.addSubview(&left_label);
                    }
                    y -= AccountCardPlan::ROW_HEIGHT;
                }
            }
            content.addSubview(&card);
            top += height + CARD_GAP;
        }

        // The provider's standing and what can be done about it, under
        // its accounts.
        let footer_y = document_height - top - FOOTER_HEIGHT;
        let connecting = busy && active == Some(provider);
        let any_active = rows
            .iter()
            .any(|(_, entry, _)| entry.summary.state == omp_runtime::AccountState::Active);
        let (status_text, status_color) = if connecting {
            ("Connecting…".to_owned(), theme::secondary_text())
        } else if !ready {
            ("Waiting for OMP…".to_owned(), theme::secondary_text())
        } else if any_active {
            let count = rows.len();
            let noun = if count == 1 { "account" } else { "accounts" };
            (
                format!("Connected • {count} {noun}"),
                NSColor::systemGreenColor(),
            )
        } else {
            ("Not connected".to_owned(), theme::secondary_text())
        };
        let status = text_label(&status_text, 12.0, true, mtm);
        status.setTextColor(Some(&status_color));
        status.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        status.setFrame(rect(CARD_X + 2.0, footer_y + 14.0, 240.0, 20.0));
        content.addSubview(&status);
        let add_title = if connecting {
            "Continue sign-in…".to_owned()
        } else if rows.is_empty() {
            format!("Connect {}…", provider.product_name())
        } else {
            format!("Add {} account…", provider.product_name())
        };
        let add = settings_button(
            &add_title,
            rect(CARD_X + card_width - 168.0, footer_y + 9.0, 168.0, 30.0),
            self,
            sel!(settingsAddAccount:),
            mtm,
        );
        add.setTag(provider_index as isize);
        add.setEnabled(ready && (!busy || connecting));
        content.addSubview(&add);
        if connecting {
            // A sign-in must be abandonable from where it was started,
            // without waiting on a browser that may never come back.
            let cancel = settings_button(
                "Cancel",
                rect(
                    CARD_X + card_width - 168.0 - 88.0,
                    footer_y + 9.0,
                    80.0,
                    30.0,
                ),
                self,
                sel!(settingsCancelSignIn:),
                mtm,
            );
            content.addSubview(&cancel);
        } else if any_active {
            // Plans and quota windows are fetched when the page opens and
            // kept for a couple of minutes; this asks again now.
            let refresh = settings_button(
                if usage_loading {
                    "Checking…"
                } else {
                    "Refresh usage"
                },
                rect(
                    CARD_X + card_width - 168.0 - 118.0,
                    footer_y + 9.0,
                    110.0,
                    30.0,
                ),
                self,
                sel!(settingsRefreshUsage:),
                mtm,
            );
            refresh.setEnabled(ready && !usage_loading);
            content.addSubview(&refresh);
        }
        content.scrollPoint(NSPoint::new(
            0.0,
            (document_height - visible_height - from_top).max(0.0),
        ));
        if self.ivars().account_highlight_scroll.replace(false)
            && let Some(card) = highlight_card {
            card.scrollRectToVisible(card.bounds());
        }
    }

    /// Reads the metrics log off the main thread; the page fills in when
    /// the read lands.
    fn request_metrics_read(&self) {
        let ivars = self.ivars();
        if ivars.metrics_loading.replace(true) {
            return;
        }
        let Ok(directory) = preferences::omp_sessions_directory() else {
            ivars.metrics_loading.set(false);
            return;
        };
        let path = directory.join(metrics::FILE_NAME);
        let submitted = self.jobs().submit(move || BackgroundEvent::MetricsRead {
            records: metrics::read(&path),
        });
        if !submitted {
            ivars.metrics_loading.set(false);
        }
    }

    /// Lays the Metrics page out from what the log said, for the scope the
    /// segmented control selects.
    fn sync_settings_metrics_ui(&self) {
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        // Scroller geometry can synchronously reenter layout while cards are
        // being inserted. Do not append a second copy of the same controls.
        if self.ivars().settings_laying_out.replace(true) { return; }
        let _reset = ResetOnDrop(&self.ivars().settings_laying_out);
        let mtm = self.mtm();
        let content = &settings.metrics_content;
        let from_top = {
            let visible = settings.metrics_scroll.contentView().bounds();
            let old_height = content.frame().size.height;
            (old_height - (visible.origin.y + visible.size.height)).max(0.0)
        };
        let previous = content.subviews();
        for index in 0..previous.count() {
            previous.objectAtIndex(index).removeFromSuperview();
        }
        let loading = self.ivars().metrics_loading.get();
        let selection = self.ivars().metrics_selection.borrow().clone();
        let (summary, sessions) = {
            let records = self.ivars().metrics_records.borrow();
            let scope = match &selection {
                MetricsSelection::Session(id) => Some(id.as_str()),
                MetricsSelection::All | MetricsSelection::Unchosen => None,
            };
            (
                metrics::summarize(&records, scope),
                metrics::sessions(&records),
            )
        };
        let list = {
            let summaries = self.ivars().sessions.borrow();
            metrics_session_rows(&sessions, &summaries)
        };
        let scope_title = match &selection {
            MetricsSelection::Session(id) => list
                .iter()
                .find(|row| row.session.as_deref() == Some(id.as_str()))
                .map_or_else(|| "This session".to_owned(), |row| row.title.clone()),
            MetricsSelection::All | MetricsSelection::Unchosen => "All sessions".to_owned(),
        };
        let plan = metrics_page_plan(
            &summary,
            &scope_title,
            matches!(selection, MetricsSelection::Session(_)),
            loading,
        );
        self.sync_metrics_session_list(&list, &selection, mtm);

        const CARD_X: f64 = 0.0;
        const CARD_GAP: f64 = 14.0;
        const INTRO_HEIGHT: f64 = 30.0;
        const FOOTER_HEIGHT: f64 = 48.0;
        const TOP_PADDING: f64 = 6.0;
        let content_width = settings.metrics_scroll.frame().size.width;
        let card_width = (content_width - 40.0).clamp(380.0, 900.0);
        let right = card_width - 20.0;
        let content_height = TOP_PADDING
            + INTRO_HEIGHT
            + plan
                .cards
                .iter()
                .map(|card| card.height() + CARD_GAP)
                .sum::<f64>()
            + FOOTER_HEIGHT;
        let visible_height = settings.metrics_scroll.frame().size.height;
        let document_height = content_height.max(visible_height);
        content.setFrame(rect(0.0, 0.0, content_width, document_height));

        let intro = text_label(&plan.intro, 12.0, false, mtm);
        intro.setTextColor(Some(&theme::secondary_text()));
        intro.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        intro.setFrame(rect(
            CARD_X + 2.0,
            document_height - TOP_PADDING - 22.0,
            card_width,
            18.0,
        ));
        content.addSubview(&intro);

        let mut top = TOP_PADDING + INTRO_HEIGHT;
        for card_plan in &plan.cards {
            let height = card_plan.height();
            let card = settings_card(
                rect(CARD_X, document_height - top - height, card_width, height),
                mtm,
            );
            let mut y = height - 16.0;
            let heading = section_label(&card_plan.title, mtm);
            heading.setFrame(rect(20.0, y - 14.0, 400.0, 14.0));
            card.addSubview(&heading);
            y -= 22.0;
            if let Some(note) = &card_plan.note {
                // Two lines at most, so a narrow pane wraps rather than
                // cuts the sentence short.
                let note_label = text_label(note, 11.0, false, mtm);
                note_label.setTextColor(Some(&theme::secondary_text()));
                note_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
                note_label.setMaximumNumberOfLines(2);
                if let Some(cell) = note_label.cell() {
                    cell.setWraps(true);
                    cell.setUsesSingleLineMode(false);
                }
                note_label.setFrame(rect(20.0, y - 30.0, right - 20.0, 30.0));
                card.addSubview(&note_label);
                y -= MetricsCardPlan::NOTE_HEIGHT;
            }
            if let Some(bar) = &card_plan.bar {
                // The stacked bar: one segment per share, in order, with the
                // whole width standing for the wall time.
                let bar_width = right - 20.0;
                let track = fill_box(
                    rect(20.0, y - 10.0, bar_width, 10.0),
                    &theme::surface_inset(),
                    5.0,
                    mtm,
                );
                card.addSubview(&track);
                let mut x = 20.0;
                for (share, color) in bar.iter().zip(metrics_share_colors()) {
                    let width = bar_width * share;
                    if width >= 1.0 {
                        let segment = fill_box(rect(x, y - 10.0, width, 10.0), &color, 5.0, mtm);
                        card.addSubview(&segment);
                        x += width;
                    }
                }
                y -= MetricsCardPlan::BAR_HEIGHT;
            }
            if let Some(columns) = &card_plan.columns {
                // Column headings, right-aligned over their figures.
                let mut x = 20.0;
                for (index, (title, width)) in columns.iter().enumerate() {
                    let label = section_label(title, mtm);
                    let width = if index == 0 {
                        right - 20.0 - columns.iter().skip(1).map(|(_, width)| width).sum::<f64>()
                    } else {
                        *width
                    };
                    label.setFrame(rect(x, y - 14.0, width, 14.0));
                    if index > 0 {
                        label.setAlignment(NSTextAlignment::Right);
                    }
                    card.addSubview(&label);
                    x += width;
                }
                y -= MetricsCardPlan::COLUMNS_HEIGHT;
            }
            for (row_index, row) in card_plan.rows.iter().enumerate() {
                let mut x = 20.0;
                if let Some(bar) = &card_plan.bar {
                    let color = metrics_share_colors()
                        .get(row_index)
                        .cloned()
                        .unwrap_or_else(NSColor::tertiaryLabelColor);
                    let swatch = fill_box(rect(20.0, y - 15.0, 8.0, 8.0), &color, 4.0, mtm);
                    card.addSubview(&swatch);
                    x += 16.0;
                    let _ = bar;
                }
                let widths: Vec<f64> = match &card_plan.columns {
                    Some(columns) => {
                        let rest: f64 = columns.iter().skip(1).map(|(_, width)| width).sum();
                        std::iter::once(right - 20.0 - rest)
                            .chain(columns.iter().skip(1).map(|(_, width)| *width))
                            .collect()
                    }
                    None => vec![right - 20.0 - 190.0, 110.0, 80.0],
                };
                for (index, (cell, width)) in row.iter().zip(widths.iter()).enumerate() {
                    let label = text_label(cell, 12.0, false, mtm);
                    label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
                    if index > 0 {
                        label.setAlignment(NSTextAlignment::Right);
                        label.setTextColor(Some(&theme::secondary_text()));
                    }
                    let width = if index == 0 {
                        width - (x - 20.0)
                    } else {
                        *width
                    };
                    label.setFrame(rect(x, y - 18.0, width, 18.0));
                    card.addSubview(&label);
                    x += width;
                }
                y -= MetricsCardPlan::ROW_HEIGHT;
            }
            content.addSubview(&card);
            top += height + CARD_GAP;
        }

        let footer_y = document_height - top - FOOTER_HEIGHT;
        let status = text_label(&plan.footer, 12.0, false, mtm);
        status.setTextColor(Some(&theme::secondary_text()));
        status.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        status.setFrame(rect(
            CARD_X + 2.0,
            footer_y + 14.0,
            card_width - 260.0,
            18.0,
        ));
        content.addSubview(&status);
        let reveal = settings_button(
            "Reveal log…",
            rect(CARD_X + card_width - 110.0, footer_y + 9.0, 110.0, 30.0),
            self,
            sel!(revealMetricsLog:),
            mtm,
        );
        content.addSubview(&reveal);
        let refresh = settings_button(
            if loading { "Reading…" } else { "Refresh" },
            rect(
                CARD_X + card_width - 110.0 - 98.0,
                footer_y + 9.0,
                90.0,
                30.0,
            ),
            self,
            sel!(settingsRefreshMetrics:),
            mtm,
        );
        refresh.setEnabled(!loading);
        content.addSubview(&refresh);
        content.scrollPoint(NSPoint::new(
            0.0,
            (document_height - visible_height - from_top).max(0.0),
        ));
    }

    /// Lays the Metrics page's session list out: "All sessions", then each
    /// session the log knows, newest activity first, with a gold pill
    /// behind the chosen row.
    fn sync_metrics_session_list(
        &self,
        rows: &[MetricsSessionRow],
        selection: &MetricsSelection,
        mtm: MainThreadMarker,
    ) {
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let content = &settings.metrics_list_content;
        let previous = content.subviews();
        for index in 0..previous.count() {
            previous.objectAtIndex(index).removeFromSuperview();
        }
        let width = settings.metrics_list_scroll.frame().size.width;
        let visible_height = settings.metrics_list_scroll.frame().size.height;
        let document_height =
            (rows.len() as f64 * SETTINGS_METRICS_ROW_HEIGHT + 8.0).max(visible_height);
        content.setFrame(rect(0.0, 0.0, width, document_height));
        let mut tags = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let y = document_height - 4.0 - (index as f64 + 1.0) * SETTINGS_METRICS_ROW_HEIGHT;
            let frame = rect(0.0, y, width, SETTINGS_METRICS_ROW_HEIGHT);
            let chosen = match (&row.session, selection) {
                (None, MetricsSelection::All | MetricsSelection::Unchosen) => true,
                (Some(session), MetricsSelection::Session(id)) => session == id,
                _ => false,
            };
            if chosen {
                let pill = fill_box(
                    rect(0.0, y + 2.0, width - 8.0, SETTINGS_METRICS_ROW_HEIGHT - 4.0),
                    &theme::accent_fill_strong(),
                    9.0,
                    mtm,
                );
                content.addSubview(&pill);
            }
            let title = text_label(&row.title, 12.0, chosen, mtm);
            title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            title.setFrame(rect(12.0, y + 22.0, width - 28.0, 17.0));
            content.addSubview(&title);
            let detail = text_label(&row.detail, 10.5, false, mtm);
            detail.setTextColor(Some(&theme::secondary_text()));
            detail.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            detail.setFrame(rect(12.0, y + 6.0, width - 28.0, 15.0));
            content.addSubview(&detail);
            // The row is a transparent button over the two labels: it draws
            // nothing, so they stay visible, and it sits on top, so a click
            // anywhere on the row reaches it rather than a label.
            // SAFETY: the delegate implements selectMetricsSession: and
            // outlives the list.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    ns_string!(""),
                    Some::<&AnyObject>(self),
                    Some(sel!(selectMetricsSession:)),
                    mtm,
                )
            };
            button.setFrame(frame);
            button.setBordered(false);
            button.setTransparent(true);
            button.setTag(isize::try_from(index).unwrap_or(isize::MAX));
            button.setToolTip(Some(&NSString::from_str(&row.tooltip)));
            content.addSubview(&button);
            tags.push(row.session.clone());
        }
        self.ivars().metrics_rows.replace(tags);
    }

    fn scroll_metrics_to_top(&self) {
        if let Some(settings) = self.ivars().settings_ui.get() {
            let height = settings.metrics_content.frame().size.height;
            settings
                .metrics_content
                .scrollPoint(NSPoint::new(0.0, height));
        }
    }

    /// Shows the top of the Accounts page: another provider's tab, or the
    /// page opening afresh.
    fn scroll_accounts_to_top(&self) {
        if let Some(settings) = self.ivars().settings_ui.get() {
            let height = settings.account_content.frame().size.height;
            settings
                .account_content
                .scrollPoint(NSPoint::new(0.0, height));
        }
    }

    /// Stored accounts of `provider` that OMP would accept as a pin, in the
    /// order they are listed everywhere else.
    fn provider_accounts(&self, provider: Option<AuthProvider>) -> Vec<AccountEntry> {
        let Some(provider) = provider else {
            return Vec::new();
        };
        self.ivars()
            .accounts
            .borrow()
            .iter()
            .filter(|entry| entry.provider() == provider && entry.is_usable())
            .cloned()
            .collect()
    }

    fn account_entry(&self, account_id: &str) -> Option<AccountEntry> {
        self.ivars()
            .accounts
            .borrow()
            .iter()
            .find(|entry| entry.id() == account_id)
            .cloned()
    }

    fn sender_account(&self, sender: &AnyObject) -> Option<AccountEntry> {
        let button = sender.downcast_ref::<NSButton>()?;
        let index = usize::try_from(button.tag()).ok()?;
        self.ivars().accounts.borrow().get(index).cloned()
    }

    /// Replaces the in-memory account set with OMP's live list, persisting
    /// sightings, pruning metadata for accounts OMP dropped, and deriving which
    /// providers are usable.
    fn replace_accounts(&self, summaries: Vec<AccountSummary>) {
        let mut stored = self.ivars().account_metadata.borrow().clone();
        let live: HashSet<_> = summaries.iter().map(|summary| summary.id.as_str()).collect();
        stored.retain(|account| live.contains(account.account_id.as_str()));
        self.ivars().project_account_choices.borrow_mut().retain(|_, account| live.contains(account.as_str()));
        for summary in &summaries {
            if !stored.iter().any(|entry| entry.account_id == summary.id) {
                stored.push(StoredAccount {
                    account_id: summary.id.clone(), provider: summary.provider.id().into(),
                    principal: summary.principal.clone(), label: None,
                    authenticated_at_ms: preferences::now_ms(), is_default: false,
                });
            }
        }
        self.ivars().account_metadata.replace(stored.clone());
        let observed = summaries.clone();
        self.write_preferences(move || {
            for summary in &observed {
                preferences::record_account(&summary.id, summary.provider.id(), summary.principal.as_deref(), None)?;
            }
            preferences::retain_accounts(&observed.into_iter().map(|summary| summary.id).collect::<Vec<_>>())
        });
        let entries = summaries
            .into_iter()
            .map(|summary| AccountEntry::joined(summary, &stored))
            .collect::<Vec<_>>();
        let authenticated = entries
            .iter()
            .filter(|entry| entry.summary.state == omp_runtime::AccountState::Active)
            .map(AccountEntry::provider)
            .collect::<HashSet<_>>();
        for entry in self.ivars().live_sessions.borrow().entries() {
            entry.account_alerts.borrow_mut().retain_accounts(&entries);
            entry.authenticated_providers.replace(authenticated.clone());
        }
        self.ivars().accounts.replace(entries);
        self.sync_account_banner();
        self.sync_settings_account_ui();
        self.sync_account_controls();
    }

    /// Re-reads labels, defaults, and timestamps after a metadata edit without
    /// asking OMP for its account list again.
    fn reload_account_metadata(&self) {
        self.write_account_metadata(|| Ok(()));
    }

    fn apply_account_metadata(&self, stored: Vec<StoredAccount>) {
        self.ivars().account_metadata.replace(stored.clone());
        let entries = self
            .ivars()
            .accounts
            .borrow()
            .iter()
            .map(|entry| AccountEntry::joined(entry.summary.clone(), &stored))
            .collect::<Vec<_>>();
        self.ivars().accounts.replace(entries);
        self.sync_settings_account_ui();
        self.sync_account_controls();
        self.update_chat_controls();
    }

    /// Records a sign-in OMP just completed. The child is restarting, so the
    /// account is shown immediately from the completion event and labelled
    /// right away when the provider now holds more than one account.
    fn register_authenticated_account(
        &self,
        provider: AuthProvider,
        account_id: String,
        principal: Option<String>,
    ) {
        let saved_id = account_id.clone();
        let saved_principal = principal.clone();
        self.write_account_metadata(move || preferences::record_account(
            &saved_id, provider.id(), saved_principal.as_deref(), Some(preferences::now_ms())));
        let stored = self.ivars().account_metadata.borrow().clone();
        let summary = AccountSummary {
            id: account_id.clone(),
            provider,
            principal,
            state: omp_runtime::AccountState::Active,
        };
        {
            let mut accounts = self.ivars().accounts.borrow_mut();
            match accounts.iter_mut().find(|entry| entry.id() == account_id) {
                Some(existing) => *existing = AccountEntry::joined(summary, &stored),
                None => accounts.push(AccountEntry::joined(summary, &stored)),
            }
        }
        self.reload_account_metadata();
        self.prompt_for_missing_labels(provider);
    }

    /// Whether prompts may be sent as far as account selection is concerned:
    /// either the active provider's account is pinned, or there is nothing to
    /// pin because OMP has no usable account or never reported its list.
    fn current_account_is_resolved(&self) -> bool {
        self.session_account_is_resolved(&self.session())
    }

    fn session_account_is_resolved(&self, owner: &live_sessions::LiveSession) -> bool {
        let Some(provider) = AuthProvider::for_model(&owner.current_model.borrow()) else {
            return true;
        };
        if !owner.accounts_known.get() {
            return true;
        }
        if owner.account_change_pending.get() {
            return false;
        }
        let usable = self.provider_accounts(Some(provider));
        if usable.is_empty() {
            return true;
        }
        owner.current_account
            .borrow()
            .as_deref()
            .is_some_and(|id| usable.iter().any(|entry| entry.id() == id))
    }

    /// Settles which account the active provider should use and pins it,
    /// or asks the user to choose when nothing singles one out.
    fn resolve_session_account(&self, owner: &live_sessions::LiveSession) {
        let Some(provider) = AuthProvider::for_model(&owner.current_model.borrow()) else {
            return;
        };
        if !owner.accounts_known.get() {
            return;
        }
        let project = owner.active_project_id.borrow().clone().unwrap_or_else(|| owner.current_project.borrow().to_string_lossy().into_owned());
        let project_choice = self.ivars().project_account_choices.borrow()
            .get(&(project, provider.id().to_owned())).cloned();
        let current = owner.current_account.borrow().clone();
        let accounts = self.ivars().accounts.borrow().clone();
        match resolve_provider_account(
            &accounts,
            provider,
            project_choice.as_deref(),
            current.as_deref(),
        ) {
            AccountResolution::Unavailable => {
                owner.current_account.replace(None);
            }
            AccountResolution::Chosen(account_id) => {
                if current.as_deref() != Some(account_id.as_str()) {
                    if let Some(runtime) = owner.omp_runtime.get() {
                        owner.account_change_pending.set(true);
                        if !runtime.set_account(account_id) {
                            owner.account_change_pending.set(false);
                        }
                    } else {
                        // Pin launch intent before the child exists. Starting
                        // unpinned and correcting it after Ready creates a
                        // needless restart during which a submitted prompt can
                        // otherwise be stranded.
                        owner.current_account.replace(Some(account_id));
                    }
                }
            }
            AccountResolution::NeedsChoice => {
                owner.current_account.replace(None);
                if !owner.read_only_session.get() {
                    self.set_session_status(owner, &format!(
                        "Choose a {} account to continue",
                        provider.product_name()
                    ));
                }
            }
        }
        if self.session().key == owner.key { self.update_chat_controls(); }
    }

    /// Account to pin for a fresh session on `model`, asking the user when the
    /// provider has several and nothing decides. `None` means the user
    /// cancelled and the model change should not proceed.
    fn account_for_new_provider_session(&self, model: &str) -> Option<Option<String>> {
        let owner = self.session();
        let Some(provider) = AuthProvider::for_model(model) else {
            return Some(None);
        };
        if !owner.accounts_known.get() {
            return Some(None);
        }
        let project = self.active_project_key();
        let project_choice = self.ivars().project_account_choices.borrow()
            .get(&(project, provider.id().to_owned())).cloned();
        let accounts = self.ivars().accounts.borrow().clone();
        match resolve_provider_account(&accounts, provider, project_choice.as_deref(), None) {
            AccountResolution::Unavailable => Some(None),
            AccountResolution::Chosen(account_id) => {
                self.remember_project_account(provider, &account_id);
                Some(Some(account_id))
            }
            AccountResolution::NeedsChoice => {
                let account_id = self.prompt_for_account_choice(provider, model)?;
                self.remember_project_account(provider, &account_id);
                Some(Some(account_id))
            }
        }
    }

    fn remember_project_account(&self, provider: AuthProvider, account_id: &str) {
        self.remember_session_account(&self.session(), provider, account_id);
    }

    fn remember_session_account(&self, owner: &live_sessions::LiveSession, provider: AuthProvider, account_id: &str) {
        // An account acknowledgement may belong to an unselected session.
        // Persist its workspace choice before Ready resolves the restarted child.
        let project = owner.active_project_id.borrow().clone()
            .unwrap_or_else(|| owner.current_project.borrow().to_string_lossy().into_owned());
        let account_id = account_id.to_owned();
        self.ivars().project_account_choices.borrow_mut().insert(
            (project.clone(), provider.id().into()), account_id.clone());
        self.write_preferences(move || preferences::set_project_account(&project, provider.id(), &account_id));
    }

    fn request_account_change(&self, account_id: String) {
        let owner = self.session();
        if owner.current_account.borrow().as_deref() == Some(account_id.as_str())
            && !owner.account_change_pending.get()
        {
            self.sync_account_controls();
            return;
        }
        let current_account = owner.current_account.borrow().clone().unwrap_or_default();
        if owner.account_change_pending.replace(true) {
            return;
        }
        let name = self
            .account_entry(&account_id)
            .map_or_else(|| account_id.clone(), |entry| entry.display_name());
        if !owner.read_only_session.get() {
            self.set_chat_status(&format!("Switching to account {name}…"));
        }
        if owner.omp_runtime.get().is_none() {
            if let Some(account) = self.account_entry(&account_id) {
                self.remember_session_account(&owner, account.provider(), &account_id);
            }
            self.note_cache_action(cache_policy::CacheAction::AccountSwitch { from: &current_account, to: &account_id });
            owner.current_account.replace(Some(account_id));
            owner.account_change_pending.set(false);
            self.dispatch_session_prompt(&owner);
            self.sync_account_controls();
            self.update_chat_controls();
            return;
        }
        let sent = owner.omp_runtime
            .get()
            .is_some_and(|runtime| runtime.set_account(account_id.clone()));
        if sent {
            self.note_cache_action(cache_policy::CacheAction::AccountSwitch { from: &current_account, to: &account_id });
        } else {
            owner.account_change_pending.set(false);
            self.runtime_command_failed("selecting the account");
        }
        self.update_chat_controls();
    }

    /// Keeps the composer's account control in step with the active provider's
    /// accounts and the pinned account.
    fn sync_account_controls(&self) {
        let owner = self.session();
        if cfg!(feature = "design-preview") {
            return;
        }
        let (Some(picker), Some(label)) = (
            self.ivars().account_picker.get(),
            self.ivars().account_label.get(),
        ) else {
            return;
        };
        let accounts = self.provider_accounts(self.current_model_auth_provider());
        if accounts.is_empty() || !owner.accounts_known.get() {
            picker.setHidden(true);
            label.setHidden(true);
            if let Some(chip) = self.ivars().account_chip.get() {
                chip.setHidden(true);
            }
            return;
        }
        picker.removeAllItems();
        let titles = accounts
            .iter()
            .map(|entry| NSString::from_str(&entry.display_name()))
            .collect::<Vec<_>>();
        picker.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
        let current = owner.current_account.borrow().clone();
        let selected = current
            .as_deref()
            .and_then(|id| accounts.iter().position(|entry| entry.id() == id));
        match selected {
            Some(index) => {
                picker.selectItemAtIndex(index as isize);
                label.setStringValue(&NSString::from_str(&format!(
                    "Account: {}  ▾",
                    accounts[index].display_name()
                )));
            }
            None => {
                picker.selectItemAtIndex(-1);
                label.setStringValue(ns_string!("Choose account  ▾"));
            }
        }
        picker.setHidden(false);
        label.setHidden(false);
        self.refit_composer_chips();
    }

    fn show_model_selection(&self, model: &str) {
        self.ivars()
            .model_picker
            .get()
            .expect("model picker must exist before restoring its selection")
            .selectItemWithTitle(&NSString::from_str(model));
        if let Some(settings) = self.ivars().settings_ui.get() {
            settings
                .model_picker
                .selectItemWithTitle(&NSString::from_str(model));
        }
        self.set_model_label(model);
    }

    /// Obligatory once a provider holds more than one account: every account
    /// without a label is named before the user continues.
    fn prompt_for_missing_labels(&self, provider: AuthProvider) {
        let accounts = self
            .ivars()
            .accounts
            .borrow()
            .iter()
            .filter(|entry| entry.provider() == provider)
            .cloned()
            .collect::<Vec<_>>();
        if accounts.len() < 2 || accounts.iter().all(|entry| entry.label.is_some()) {
            return;
        }
        let Some(_prompt) = self.account_prompt_guard() else {
            return;
        };
        let unlabeled = accounts
            .iter()
            .filter(|entry| entry.label.is_none())
            .cloned()
            .collect::<Vec<_>>();
        let existing = accounts
            .iter()
            .filter_map(|entry| entry.label.clone())
            .collect::<Vec<_>>();
        let mtm = self.mtm();
        const ROW: f64 = 32.0;
        const WIDTH: f64 = 400.0;
        let form = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, WIDTH, unlabeled.len() as f64 * ROW),
        );
        let mut fields = Vec::with_capacity(unlabeled.len());
        for (index, entry) in unlabeled.iter().enumerate() {
            let y = (unlabeled.len() - 1 - index) as f64 * ROW;
            let identity = text_label(&entry.identity(), 12.0, false, mtm);
            identity.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
            identity.setFrame(rect(0.0, y + 6.0, 190.0, 20.0));
            form.addSubview(&identity);
            let field = NSTextField::initWithFrame(
                NSTextField::alloc(mtm),
                rect(198.0, y + 3.0, WIDTH - 198.0, 24.0),
            );
            field.setPlaceholderString(Some(ns_string!("Label, e.g. Work")));
            form.addSubview(&field);
            fields.push(field);
        }
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!(
            "Label your {} accounts",
            provider.product_name()
        )));
        alert.setInformativeText(ns_string!(
            "Give each account a short name so you can tell them apart. Every account needs its own label."
        ));
        alert.setAccessoryView(Some(&form));
        alert.addButtonWithTitle(ns_string!("Save Labels"));
        loop {
            if let Some(first) = fields.first() {
                alert
                    .window()
                    .makeFirstResponder(Some::<&NSResponder>(first));
            }
            alert.runModal();
            let labels = fields
                .iter()
                .map(|field| field.stringValue().to_string().trim().to_owned())
                .collect::<Vec<_>>();
            if labels_are_valid(&labels, &existing) {
                for (entry, label) in unlabeled.iter().zip(&labels) {
                    let id = entry.id().to_owned();
                    let label = label.clone();
                    self.write_preferences(move || preferences::set_account_label(&id, &label));
                }
                break;
            }
            alert.setInformativeText(ns_string!(
                "Each label must be filled in and different from every other label for this provider."
            ));
        }
        self.reload_account_metadata();
    }

    /// Asks which of several accounts a model's provider should use. Returns
    /// `None` when the user cancels.
    fn prompt_for_account_choice(&self, provider: AuthProvider, model: &str) -> Option<String> {
        let candidates = self.provider_accounts(Some(provider));
        if candidates.is_empty() {
            return None;
        }
        let _prompt = self.account_prompt_guard()?;
        let mtm = self.mtm();
        let form = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 380.0, 62.0));
        let picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(0.0, 32.0, 380.0, 26.0),
            false,
        );
        for entry in &candidates {
            picker.addItemWithTitle(&NSString::from_str(&entry.display_name()));
        }
        let preselect = candidates
            .iter()
            .position(|entry| entry.is_default)
            .unwrap_or(0);
        picker.selectItemAtIndex(preselect as isize);
        form.addSubview(&picker);
        // SAFETY: A checkbox without a target or action is inert; its state is
        // read back after the modal returns.
        let make_default = unsafe {
            NSButton::checkboxWithTitle_target_action(
                &NSString::from_str(&format!(
                    "Use this account by default for {}",
                    provider.product_name()
                )),
                None,
                None,
                mtm,
            )
        };
        make_default.setFrame(rect(0.0, 2.0, 380.0, 22.0));
        form.addSubview(&make_default);
        let alert = NSAlert::new(mtm);
        let model_name = model.rsplit('/').next().unwrap_or(model);
        alert.setMessageText(&NSString::from_str(&format!(
            "Which {} account should {model_name} use?",
            provider.product_name()
        )));
        alert.setInformativeText(ns_string!(
            "This workspace will remember your choice for future chats."
        ));
        alert.setAccessoryView(Some(&form));
        alert.addButtonWithTitle(ns_string!("Use Account"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return None;
        }
        let chosen = candidates.get(usize::try_from(picker.indexOfSelectedItem()).ok()?)?;
        if make_default.state() == NSControlStateValueOn {
            let id = chosen.id().to_owned();
            self.write_preferences(move || preferences::set_default_account(provider.id(), Some(&id)));
        }
        let account_id = chosen.id().to_owned();
        self.reload_account_metadata();
        Some(account_id)
    }

    fn rename_account(&self, entry: &AccountEntry) {
        let Some(_prompt) = self.account_prompt_guard() else {
            return;
        };
        let existing = self
            .ivars()
            .accounts
            .borrow()
            .iter()
            .filter(|other| other.provider() == entry.provider() && other.id() != entry.id())
            .filter_map(|other| other.label.clone())
            .collect::<Vec<_>>();
        let mtm = self.mtm();
        let field =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(0.0, 0.0, 320.0, 24.0));
        if let Some(label) = &entry.label {
            field.setStringValue(&NSString::from_str(label));
        }
        field.setPlaceholderString(Some(ns_string!("Label, e.g. Work")));
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!(
            "Rename {}",
            entry.display_name()
        )));
        alert.setInformativeText(&NSString::from_str(&format!(
            "{} • labels tell your {} accounts apart.",
            entry.identity(),
            entry.provider().product_name()
        )));
        alert.setAccessoryView(Some(&field));
        alert.addButtonWithTitle(ns_string!("Save"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        loop {
            alert
                .window()
                .makeFirstResponder(Some::<&NSResponder>(&field));
            if alert.runModal() != NSAlertFirstButtonReturn {
                return;
            }
            let label = field.stringValue().to_string().trim().to_owned();
            if labels_are_valid(std::slice::from_ref(&label), &existing) {
                let id = entry.id().to_owned();
                self.write_preferences(move || preferences::set_account_label(&id, &label));
                self.reload_account_metadata();
                return;
            }
            alert.setInformativeText(ns_string!(
                "The label must be filled in and different from this provider's other labels."
            ));
        }
    }

    /// Marks an account modal as open for its lifetime, or returns `None`
    /// when one is already running.
    fn account_prompt_guard(&self) -> Option<AccountPromptGuard<'_>> {
        if self.ivars().account_prompt_open.replace(true) {
            return None;
        }
        Some(AccountPromptGuard(&self.ivars().account_prompt_open))
    }

    fn confirm_remove_account(&self, entry: &AccountEntry) {
        let owner = self.session();
        let Some(_prompt) = self.account_prompt_guard() else {
            return;
        };
        let mtm = self.mtm();
        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(&format!(
            "Remove {} from OMP?",
            entry.display_name()
        )));
        alert.setInformativeText(&NSString::from_str(&format!(
            "Omperor signs this {} account out of OMP and forgets its label and workspace choices. You can add it again later.",
            entry.provider().product_name()
        )));
        alert.addButtonWithTitle(ns_string!("Remove"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return;
        }
        self.set_chat_status(&format!("Removing account {}…", entry.display_name()));
        let sent = owner.omp_runtime
            .get()
            .is_some_and(|runtime| runtime.logout_account(entry.id().to_owned()));
        if !sent {
            self.runtime_command_failed("removing the account");
        }
    }

    /// Key under which per-project choices are stored: the active project's
    /// id, or the working directory itself before the registry has been
    /// seeded.
    fn active_project_key(&self) -> String {
        let owner = self.session();
        owner.active_project_id
            .borrow()
            .clone()
            .unwrap_or_else(|| {
                owner.current_project
                    .borrow()
                    .to_string_lossy()
                    .into_owned()
            })
    }

    fn active_project(&self) -> Option<Project> {
        let owner = self.session();
        let active = owner.active_project_id.borrow();
        let active = active.as_deref()?;
        self.ivars()
            .projects
            .borrow()
            .iter()
            .find(|project| project.id == active)
            .cloned()
    }

    fn project_by_id(&self, project_id: &str) -> Option<Project> {
        self.ivars()
            .projects
            .borrow()
            .iter()
            .find(|project| project.id == project_id)
            .cloned()
    }

    /// The project that owns `directory`: one of a project's folders, or the
    /// directory of a session Omperor created for it, such as a worktree.
    fn project_for_directory(&self, directory: &std::path::Path) -> Option<Project> {
        let projects = self.ivars().projects.borrow();
        if let Some(project) = projects
            .iter()
            .find(|project| project.directories.iter().any(|path| path == directory))
        {
            return Some(project.clone());
        }
        let records = self.ivars().session_records.borrow();
        records
            .values()
            .find(|record| {
                record.directory == directory || record.worktree_path.as_deref() == Some(directory)
            })
            .and_then(|record| {
                projects
                    .iter()
                    .find(|project| project.id == record.project_id)
                    .cloned()
            })
    }

    fn project_for_session(&self, summary: &session::SessionSummary) -> Option<Project> {
        let recorded = self
            .ivars()
            .session_records
            .borrow()
            .get(&summary.id)
            .map(|record| record.project_id.clone());
        if let Some(project) = recorded.and_then(|id| self.project_by_id(&id)) {
            return Some(project);
        }
        summary
            .cwd
            .as_deref()
            .and_then(|directory| self.project_for_directory(directory))
    }

    /// Creates a project for a directory that none owns, named after the
    /// folder and disambiguated against existing project names.
    fn create_project_for_directory(&self, directory: &std::path::Path) -> Option<Project> {
        let name = unique_project_name(directory, &self.ivars().projects.borrow());
        let project = preferences::project_draft(&name, &[directory.to_owned()]);
        let saved = project.clone();
        let directory = directory.to_owned();
        // Insert before subsequent touch/session writes on the same ordered worker.
        self.write_preferences(move || {
            preferences::insert_project(&saved)?;
            preferences::adopt_directory_account_choices(&directory, &saved.id)
        });
        self.ivars().projects.borrow_mut().insert(0, project.clone());
        Some(project)
    }

    fn activate_project_for_directory(&self, directory: &std::path::Path) {
        let project = self
            .project_for_directory(directory)
            .or_else(|| self.create_project_for_directory(directory));
        if let Some(project) = project {
            self.activate_project(&project.id);
        }
    }

    fn activate_project_for_session(&self, summary: &session::SessionSummary) {
        match self.project_for_session(summary) {
            Some(project) => self.activate_project(&project.id),
            None => {
                if let Some(directory) = summary.cwd.as_deref() {
                    self.activate_project_for_directory(directory);
                }
            }
        }
    }

    fn activate_project(&self, project_id: &str) {
        let owner = self.session();
        owner.active_project_id
            .replace(Some(project_id.to_owned()));
        // Record the project as opened, but do not move it: the sidebar order is
        // stable and only the user reorders it, by dragging a project row.
        let project_id = project_id.to_owned();
        self.write_preferences(move || preferences::touch_project(&project_id));
        self.refresh_project_location();
    }

    /// Suspends sidebar rebuilds so a project drag can move a live button
    /// without it being torn out by a periodic refresh, and raises the drop
    /// indicator so it draws above the rows for the duration of the drag.
    fn begin_project_drag(&self) {
        self.ivars().sidebar_drag_active.set(true);
        if let Some(container) = self.ivars().sidebar_sessions_view.get() {
            let indicator = self.ensure_drop_indicator(container);
            indicator.removeFromSuperview();
            container.addSubview(&indicator);
            indicator.setHidden(true);
        }
    }

    /// Positions the drop indicator at the boundary where the dragged project
    /// would be inserted — above the project header it now sits over, or at the
    /// foot of the list — so the user sees where it will land before releasing.
    fn update_project_drag(&self, dragged: &SidebarProjectButton) {
        let Some(container) = self.ivars().sidebar_sessions_view.get() else {
            return;
        };
        let dragged_id = dragged.project_id();
        let dragged_frame = dragged.frame();
        let dragged_center = dragged_frame.origin.y + dragged_frame.size.height / 2.0;
        // Other headers, top of the list first (a larger y is higher, as the
        // list view is not flipped).
        let mut others: Vec<f64> = self
            .ivars()
            .sidebar_project_buttons
            .borrow()
            .iter()
            .filter(|button| button.project_id() != dragged_id)
            .map(|button| {
                let frame = button.frame();
                frame.origin.y + frame.size.height
            })
            .collect();
        if others.is_empty() {
            self.hide_drop_indicator();
            return;
        }
        others.sort_by(|left, right| right.partial_cmp(left).unwrap_or(std::cmp::Ordering::Equal));
        let index = others.iter().filter(|top| **top > dragged_center).count();
        let boundary = if index < others.len() {
            // Just above the header the dragged project would precede.
            others[index] + 2.0
        } else {
            // Below the lowest row: the foot of the list.
            self.sidebar_list_bottom(&dragged_id) - 2.0
        };
        let indicator = self.ensure_drop_indicator(container);
        indicator.setFrame(rect(10.0, boundary - 1.5, SIDEBAR_WIDTH - 44.0, 3.0));
        indicator.setHidden(false);
    }

    /// The y of the foot of the sidebar's rows, ignoring the dragged header,
    /// used to place the drop indicator when a project is dragged to the end.
    fn sidebar_list_bottom(&self, dragged_id: &str) -> f64 {
        let mut bottom = f64::INFINITY;
        for button in self.ivars().sidebar_row_buttons.borrow().iter() {
            bottom = bottom.min(button.frame().origin.y);
        }
        for button in self.ivars().sidebar_project_buttons.borrow().iter() {
            if button.project_id() != dragged_id {
                bottom = bottom.min(button.frame().origin.y);
            }
        }
        if bottom.is_finite() { bottom } else { 0.0 }
    }

    /// Returns the shared drop-indicator bar, creating it (hidden) on first use.
    fn ensure_drop_indicator(&self, container: &SessionListView) -> Retained<NSBox> {
        if let Some(existing) = self.ivars().drop_indicator.borrow().as_ref() {
            return existing.clone();
        }
        let mtm = self.mtm();
        let bar = NSBox::initWithFrame(NSBox::alloc(mtm), rect(10.0, 0.0, 10.0, 3.0));
        bar.setBoxType(NSBoxType::Custom);
        bar.setTitlePosition(NSTitlePosition::NoTitle);
        bar.setContentViewMargins(NSSize::new(0.0, 0.0));
        bar.setBorderWidth(0.0);
        bar.setBorderColor(&NSColor::clearColor());
        bar.setFillColor(&theme::accent());
        bar.setCornerRadius(1.5);
        bar.setHidden(true);
        container.addSubview(&bar);
        let retained = bar.clone();
        self.ivars().drop_indicator.replace(Some(bar));
        retained
    }

    fn hide_drop_indicator(&self) {
        if let Some(bar) = self.ivars().drop_indicator.borrow().as_ref() {
            bar.setHidden(true);
        }
    }

    /// Commits a project drag: the dragged header now sits at a new height, so
    /// re-derive the order from every header's vertical position, persist it,
    /// and rebuild the sidebar in that order.
    fn finish_project_drag(&self) {
        self.ivars().sidebar_drag_active.set(false);
        self.hide_drop_indicator();
        // The list view is not flipped, so a larger y is higher in the list and
        // therefore earlier in the order.
        let mut ranked: Vec<(String, f64)> = self
            .ivars()
            .sidebar_project_buttons
            .borrow()
            .iter()
            .map(|button| {
                let frame = button.frame();
                (
                    button.project_id(),
                    frame.origin.y + frame.size.height / 2.0,
                )
            })
            .collect();
        // Only persist a reorder when every project is shown; a search filter
        // hides some, and reordering the visible few would scramble the rest.
        let complete = ranked.len() == self.ivars().projects.borrow().len();
        if complete && ranked.len() > 1 {
            ranked.sort_by(|left, right| {
                right
                    .1
                    .partial_cmp(&left.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let order: Vec<String> = ranked.into_iter().map(|(id, _)| id).collect();
            self.ivars().projects.borrow_mut().sort_by_key(|project| {
                order
                    .iter()
                    .position(|id| id == &project.id)
                    .unwrap_or(usize::MAX)
            });
            self.write_preferences(move || preferences::set_project_order(&order));
        }
        self.rebuild_sidebar_sessions();
    }

    /// A session drag reuses the project drag's setup: suspend rebuilds and
    /// raise the drop indicator.
    fn begin_session_drag(&self) {
        self.begin_project_drag();
    }

    /// Positions the drop indicator at the boundary within the dragged
    /// session's project where it would land — the sibling it now sits over, or
    /// the foot of that project's block.
    fn update_session_drag(&self, dragged: &SidebarSessionButton) {
        let Some(container) = self.ivars().sidebar_sessions_view.get() else {
            return;
        };
        let project_id = dragged.project_id();
        let dragged_id = dragged.session_id();
        let dragged_frame = dragged.frame();
        let dragged_center = dragged_frame.origin.y + dragged_frame.size.height / 2.0;
        let buttons = self.ivars().sidebar_session_buttons.borrow();
        let mut others: Vec<f64> = buttons
            .iter()
            .filter(|button| button.project_id() == project_id && button.session_id() != dragged_id)
            .map(|button| {
                let frame = button.frame();
                frame.origin.y + frame.size.height
            })
            .collect();
        // Foot of this project's session block (its lowest sibling), for a drop
        // at the very end.
        let block_bottom = buttons
            .iter()
            .filter(|button| button.project_id() == project_id && button.session_id() != dragged_id)
            .map(|button| button.frame().origin.y)
            .fold(f64::INFINITY, f64::min);
        drop(buttons);
        if others.is_empty() {
            self.hide_drop_indicator();
            return;
        }
        others.sort_by(|left, right| right.partial_cmp(left).unwrap_or(std::cmp::Ordering::Equal));
        let index = others.iter().filter(|top| **top > dragged_center).count();
        let boundary = if index < others.len() {
            others[index] + 2.0
        } else {
            block_bottom - 2.0
        };
        let indicator = self.ensure_drop_indicator(container);
        // Indented and narrower than the project bar, to sit under session rows.
        indicator.setFrame(rect(20.0, boundary - 1.5, SIDEBAR_WIDTH - 54.0, 3.0));
        indicator.setHidden(false);
    }

    /// Commits a session drag: re-derive the order of the dragged session's
    /// project from every sibling's vertical position, persist it, and rebuild.
    fn finish_session_drag(&self, dragged: &SidebarSessionButton) {
        self.ivars().sidebar_drag_active.set(false);
        self.hide_drop_indicator();
        let project_id = dragged.project_id();
        // A search filters the list, so a reorder then would scramble the hidden
        // sessions; snap back instead of persisting.
        let searching = self
            .ivars()
            .session_search
            .get()
            .map(|search| !search.stringValue().to_string().trim().is_empty())
            .unwrap_or(false);
        if !searching {
            let mut ranked: Vec<(String, f64)> = self
                .ivars()
                .sidebar_session_buttons
                .borrow()
                .iter()
                .filter(|button| button.project_id() == project_id)
                .map(|button| {
                    let frame = button.frame();
                    (
                        button.session_id(),
                        frame.origin.y + frame.size.height / 2.0,
                    )
                })
                .collect();
            if ranked.len() > 1 {
                ranked.sort_by(|left, right| {
                    right
                        .1
                        .partial_cmp(&left.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let visible: Vec<String> = ranked.into_iter().map(|(id, _)| id).collect();
                let all = {
                    let sessions = self.ivars().sessions.borrow();
                    sidebar_rows_for(&self.ivars().projects.borrow(), &sessions, &self.ivars().session_records.borrow(),
                        &self.ivars().session_positions.borrow(), SessionSourceFilter::Omperor, "")
                        .into_iter().filter_map(|row| match row {
                            SidebarRow::Session { index, project_id: Some(id) } if id == project_id => Some(sessions[index].id.clone()),
                            _ => None,
                        }).collect()
                };
                let order = sidebar::reordered_project(all, &visible, &dragged.session_id());
                {
                    let mut positions = self.ivars().session_positions.borrow_mut();
                    for (index, id) in order.iter().enumerate() {
                        positions.insert(id.clone(), index as i64);
                    }
                }
                self.write_preferences(move || preferences::set_session_order(&order));
            }
        }
        self.rebuild_sidebar_sessions();
    }

    /// Seeds the registry once from the sessions that existed before projects:
    /// one project per distinct session directory, and a record binding each
    /// session to it.
    fn bootstrap_projects_if_needed(&self) {
        if !self.ivars().projects_bootstrap_pending.replace(false) { return; }
        let sessions = self.ivars().sessions.borrow().clone();
        self.write_project_registry(move || startup::bootstrap_sessions(&sessions));
    }

    fn store_session_record(&self, record: SessionRecord) {
        let saved = record.clone();
        self.write_preferences(move || preferences::record_session(&saved));
        self.ivars()
            .session_records
            .borrow_mut()
            .insert(record.session_id.clone(), record);
    }

    /// Binds the fresh session the child just reported to the project and
    /// directory it was started for.


    /// A live session listed for the first time becomes the selection when
    /// nothing else is: it is what the window shows, and the row should say so.
    fn select_live_session_if_unselected(&self) {
        let owner = self.session();
        let ivars = self.ivars();
        if owner.selected_session_path.borrow().is_some() || owner.read_only_session.get() {
            return;
        }
        let Some(live) = owner.live_session_id.borrow().clone() else {
            return;
        };
        let path = ivars
            .sessions
            .borrow()
            .iter()
            .find(|summary| summary.id == live)
            .map(|summary| summary.path.clone());
        if path.is_some() {
            owner.selected_session_path.replace(path);
        }
    }

    fn open_pending_session_if_listed(&self) {
        let owner = self.session();
        let pending = owner.pending_open_session_id.borrow().clone();
        let Some(session_id) = pending else {
            return;
        };
        let summary = self
            .ivars()
            .sessions
            .borrow()
            .iter()
            .find(|summary| summary.is_omperor_owned() && summary.id == session_id)
            .cloned();
        let Some(summary) = summary else {
            return;
        };
        owner.pending_open_session_id.borrow_mut().take();
        owner.selected_session_path
            .replace(Some(summary.path.clone()));
        self.rebuild_sidebar_sessions();
        self.resume_summary(summary);
    }

    /// The session a sidebar row control or context-menu item refers to.
    fn sender_session(&self, sender: &AnyObject) -> Option<session::SessionSummary> {
        let tag = sender
            .downcast_ref::<NSMenuItem>()
            .map(|item| item.tag())
            .or_else(|| sender.downcast_ref::<NSButton>().map(|button| button.tag()))?;
        let index = self.sidebar_row_session_index(tag)?;
        self.ivars().sessions.borrow().get(index).cloned()
    }

    /// Context menu for one session row, offered on right-click and from the
    /// hover button. Actions that cannot run right now are shown disabled.
    fn session_context_menu(
        &self,
        row_tag: isize,
        summary: &session::SessionSummary,
    ) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Session"));
        menu.setAutoenablesItems(false);
        let can_fork = summary.cwd.is_some() && self.ivars().live_sessions.borrow().find_path(&summary.path)
            .is_none_or(|entry| self.can_release_for_fork(&entry));
        let has_journal = journal::is_journal(&summary.path);
        for (title, action, enabled) in [
            ("Timeline…", sel!(showSidebarSessionTimeline:), has_journal),
            ("Fork Session", sel!(forkSidebarSession:), can_fork),
            (
                "Fork into New Worktree…",
                sel!(forkSidebarSessionIntoWorktree:),
                can_fork,
            ),
        ] {
            // SAFETY: The delegate implements both selectors and outlives the menu.
            let item = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(title),
                    Some(action),
                    ns_string!(""),
                )
            };
            // SAFETY: The delegate outlives every sidebar menu it targets.
            unsafe { item.setTarget(Some::<&AnyObject>(self)) };
            item.setTag(row_tag);
            item.setEnabled(enabled);
        }
        if let Some(entry) = self.ivars().live_sessions.borrow().find_path(&summary.path) {
            let item = unsafe { menu.addItemWithTitle_action_keyEquivalent(ns_string!("Stop session"), Some(sel!(stopSidebarSession:)), ns_string!("")) };
            unsafe { item.setTarget(Some::<&AnyObject>(self)) };
            item.setTag(entry.key.0 as isize);
            item.setEnabled(entry.chat_busy.get());
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let title = if summary.id.starts_with("draft-") { ns_string!("Discard Draft") } else { ns_string!("Delete Session…") };
        // SAFETY: the delegate implements deleteSidebarSession: and outlives
        // the menu; the represented object is an NSString the item keeps.
        let item = unsafe { menu.addItemWithTitle_action_keyEquivalent(title, Some(sel!(deleteSidebarSession:)), ns_string!("")) };
        unsafe {
            item.setTarget(Some::<&AnyObject>(self));
            item.setRepresentedObject(Some(&NSString::from_str(&summary.id)));
        }
        menu
    }

    /// Forks a session in place: a fresh session seeded with the parent's
    /// transcript, working in the same folder.
    fn fork_session_in_place(&self, summary: session::SessionSummary) {
        if self.mock_backend_active() { self.set_chat_status("Select OMP in Settings → General to use this action"); return; }
        let Some(directory) = summary.cwd.clone() else {
            self.set_chat_status("This session's folder is unavailable, so it cannot be forked");
            return;
        };
        let Some(project) = self
            .project_for_session(&summary)
            .or_else(|| self.create_project_for_directory(&directory))
        else {
            return;
        };
        self.start_chat_in(
            &project.id,
            directory,
            Some(ForkPlan {
                parent_session_id: summary.id,
                worktree: None,
            }),
        );
    }

    fn sidebar_row_session_index(&self, tag: isize) -> Option<usize> {
        let tag = usize::try_from(tag).ok()?;
        match self.ivars().sidebar_rows.borrow().get(tag)? {
            SidebarRow::Session { index, .. } => Some(*index),
            SidebarRow::Project { .. } | SidebarRow::ShowMore { .. } | SidebarRow::Unfiled => None,
        }
    }

    fn sidebar_row_project_id(&self, sender: &AnyObject) -> Option<String> {
        let button = sender.downcast_ref::<NSButton>()?;
        let tag = usize::try_from(button.tag()).ok()?;
        match self.ivars().sidebar_rows.borrow().get(tag)? {
            SidebarRow::Project { project_id } | SidebarRow::ShowMore { project_id, .. } => Some(project_id.clone()),
            SidebarRow::Session { .. } | SidebarRow::Unfiled => None,
        }
    }

    fn sender_project(&self, sender: &AnyObject) -> Option<Project> {
        let button = sender.downcast_ref::<NSButton>()?;
        let index = usize::try_from(button.tag()).ok()?;
        self.ivars().projects.borrow().get(index).cloned()
    }

    /// Re-reads the registry after an edit and refreshes every view of it.
    fn reload_project_registry(&self) {
        self.write_project_registry(|| Ok(()));
    }

    /// Starts a chat in `project_id`, asking which folder to use when the
    /// project has several.
    fn new_chat_in_project(&self, project_id: &str) {
        let Some(project) = self.project_by_id(project_id) else {
            return;
        };
        let directories = project
            .directories
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let directory = match directories.as_slice() {
            [] => {
                self.set_chat_status(&format!(
                    "{} has no folder on disk • add one in Settings → Projects",
                    project.name
                ));
                return;
            }
            [only] => only.clone(),
            _ => match self.prompt_for_directory(&project, &directories) {
                Some(directory) => directory,
                None => return,
            },
        };
        self.start_chat_in(project_id, directory, None);
    }

    /// Creates an independently owned session without replacing another worker.
    fn start_chat_in(&self, project_id: &str, directory: std::path::PathBuf, fork: Option<ForkPlan>) {
        self.create_session_in(project_id, directory, fork, true);
    }

    fn fork_owns_session(&self, key: live_sessions::SessionKey) -> bool {
        self.ivars().pending_forks.borrow().iter().any(|(source, (target, _, _))| *source == key || *target == key)
    }

    fn can_release_for_fork(&self, source: &live_sessions::LiveSession) -> bool {
        !self.ivars().termination_pending.get()
            && !self.fork_owns_session(source.key)
            && !source.chat_busy.get() && !source.runtime_starting.get()
            && !source.attachment_submission.get() && !source.pending_attachments.borrow().blocked()
            && source.pending_prompt.borrow().is_none() && !source.account_change_pending.get()
            && self.ivars().authentication_owner.get() != Some(source.key)
            && !matches!(source.lifecycle.get(), live_sessions::Lifecycle::Stopping | live_sessions::Lifecycle::Retiring)
            && !source.subagents.borrow().iter().any(|agent| agents::AgentState::from_status(&agent.status).is_busy())
    }

    fn create_session_in(&self, project_id: &str, directory: std::path::PathBuf, fork: Option<ForkPlan>, select: bool) {
        if self.ivars().termination_pending.get() { return; }
        let source = fork.as_ref().and_then(|plan| self.ivars().live_sessions.borrow().entries().into_iter()
            .find(|entry| entry.live_session_id.borrow().as_deref() == Some(&plan.parent_session_id)));
        if source.as_ref().is_some_and(|source| !self.can_release_for_fork(source)) {
            self.set_chat_status("Wait for the source and its delegated work to finish before forking");
            return;
        }
        let warning = if let Some(plan) = &fork {
            let model = self.ivars().sessions.borrow().iter()
                .find(|summary| summary.id == plan.parent_session_id)
                .and_then(|summary| summary.model.clone())
                .or_else(|| source.as_ref().map(|source| source.current_model.borrow().clone()));
            model.and_then(|model| cache_policy::CacheWarning::for_action(&model, cache_policy::CacheAction::ForkSession))
        } else {
            self.cache_warning_for_action(cache_policy::CacheAction::NewSession)
        };
        if select { self.capture_selected_binding(); }
        let owner = self.ivars().live_sessions.borrow_mut().create();
        owner.cache_warning.replace(warning);
        owner.current_project.replace(directory.clone());
        owner.current_model.replace(self.ivars().preferences.borrow().model.clone().unwrap_or_default());
        owner.active_project_id.replace(Some(project_id.to_owned()));
        owner.new_session_pending.set(true);
        owner.pending_session.replace(Some(PendingSession {
            project_id: project_id.to_owned(), directory: directory.clone(),
            parent_session_id: fork.as_ref().map(|plan| plan.parent_session_id.clone()),
            branch: fork.as_ref().and_then(|plan| plan.worktree.as_ref()).map(|worktree| worktree.branch.clone()),
            base_branch: fork.as_ref().and_then(|plan| plan.worktree.as_ref()).map(|worktree| worktree.base_branch.clone()),
            worktree_path: fork.as_ref().is_some_and(|plan| plan.worktree.is_some()).then_some(directory),
        }));
        if select { self.ivars().live_sessions.borrow_mut().select(owner.key); }
        if let Some(source) = source.filter(|source| source.omp_runtime.get().is_some()) {
            let parent = fork.as_ref().unwrap().parent_session_id.clone();
            let job = self.begin_job("Releasing the source journal for its fork…");
            self.ivars().pending_forks.borrow_mut().insert(source.key, (owner.key, parent, job));
            source.lifecycle.set(live_sessions::Lifecycle::Retiring);
            source.runtime_ready.set(false);
            if !source.omp_runtime.get().unwrap().release_session() {
                self.ivars().pending_forks.borrow_mut().remove(&source.key);
                self.finish_job(job);
                source.lifecycle.set(live_sessions::Lifecycle::Failed);
                owner.new_session_pending.set(false);
                owner.lifecycle.set(live_sessions::Lifecycle::Failed);
                self.set_session_status(&owner, "Source runtime is unavailable; choose Fork again to retry");
            }
        } else {
            let target = fork.map_or(omp_runtime::SessionStartup::Fresh, |plan| omp_runtime::SessionStartup::Fork(plan.parent_session_id));
            self.launch_session_runtime(&owner, None, target);
        }
        if select { self.render_selected_binding(); } else { self.request_sidebar_rebuild(); }
    }

    /// Forks a session: a new branch and worktree from the commit its folder
    /// has checked out, then a fresh OMP session seeded with its transcript.
    /// Forks a session in stages so git never runs on the main thread: probe
    /// the repository on the job pool, prompt for a branch, create the
    /// worktree on the pool, then launch the forked owner.
    fn fork_session_into_worktree(&self, summary: session::SessionSummary) {
        let key = self.session().key;
        if self.mock_backend_active() { self.set_chat_status("Select OMP in Settings → General to use this action"); return; }
        let Some(directory) = summary.cwd.clone() else {
            self.set_chat_status("This session's folder is unavailable, so it cannot be forked");
            return;
        };
        let Some(project) = self
            .project_for_session(&summary)
            .or_else(|| self.create_project_for_directory(&directory))
        else {
            return;
        };
        let job = self.begin_job(&format!(
            "Inspecting the repository in {} for a fork…",
            directory.display()
        ));
        let probe_directory = directory.clone();
        let submitted = self.jobs().submit(move || BackgroundEvent::ForkProbed {
                key,
            job,
            summary,
            directory: probe_directory.clone(),
            project_id: project.id,
            result: workspace::branch_point(&probe_directory),
        });
        if !submitted {
            self.finish_job(job);
            self.set_chat_status("Background workers are unavailable, so the fork cannot start");
        }
    }

    /// Second fork stage, on the main thread: the repository probe finished.
    fn continue_fork(
        &self,
        summary: session::SessionSummary,
        directory: std::path::PathBuf,
        project_id: String,
        result: Result<Option<workspace::BranchPoint>, workspace::WorkspaceError>,
    ) {
        let key = self.session().key;
        let point = match result {
            Ok(Some(point)) => point,
            Ok(None) => {
                self.alert_message(
                    "This session cannot be forked into a worktree",
                    &format!(
                        "{} is not inside a git repository. Worktrees need a repository to branch from.",
                        directory.display()
                    ),
                );
                return;
            }
            Err(error) => {
                self.set_chat_status(&format!(
                    "Could not inspect the session's repository: {error}"
                ));
                return;
            }
        };
        let Some(branch) = self.prompt_for_branch_name(&summary, &point) else {
            return;
        };
        let root = match preferences::worktrees_directory() {
            Ok(root) => root,
            Err(error) => {
                self.set_chat_status(&format!("Could not locate the worktree directory: {error}"));
                return;
            }
        };
        let path = root.join(&project_id).join(branch.replace('/', "-"));
        let plan = ForkPlan {
            parent_session_id: summary.id.clone(),
            worktree: Some(WorktreeFork {
                branch: branch.clone(),
                base_branch: point
                    .branch
                    .clone()
                    .unwrap_or_else(|| point.head.chars().take(12).collect()),
            }),
        };
        let job = self.begin_job(&format!("Creating a worktree on branch {branch}…"));
        let worktree = path.clone();
        let submitted = self.jobs().submit(move || BackgroundEvent::WorktreeAdded {
                key,
            job,
            project_id,
            worktree: worktree.clone(),
            plan,
            result: workspace::add_worktree(&directory, &branch, &worktree),
        });
        if !submitted {
            self.finish_job(job);
            self.set_chat_status("Background workers are unavailable, so the fork cannot start");
        }
    }

    fn jobs(&self) -> &jobs::JobPool<BackgroundEvent> {
        self.ivars()
            .jobs
            .get_or_init(|| jobs::JobPool::start_notifying(2, "omperor-jobs", gui_wake::notify))
    }

    /// Registers a background job and shows it in the header until it
    /// finishes. Returns the id its completion event must carry.
    fn begin_job(&self, label: &str) -> u64 {
        let job = self.ivars().next_job_id.get();
        self.ivars().next_job_id.set(job + 1);
        self.ivars()
            .active_jobs
            .borrow_mut()
            .push((job, label.to_owned()));
        self.set_chat_status(label);
        self.sync_activity_indicator();
        // The sidebar doesn't list jobs; only a state it shows can change.
        self.request_sidebar_state_refresh();
        self.request_agents_refresh();
        job
    }

    fn finish_job(&self, job: u64) {
        self.ivars()
            .active_jobs
            .borrow_mut()
            .retain(|(id, _)| *id != job);
        self.sync_activity_indicator();
        self.request_sidebar_state_refresh();
        self.request_agents_refresh();
    }

    fn sync_activity_indicator(&self) {
        self.sync_busy_indicator();
    }

    fn poll_background_jobs(&self) {
        let Some(jobs) = self.ivars().jobs.get() else {
            return;
        };
        let started = std::time::Instant::now();
        while let Some(event) = jobs.try_next() {
            self.apply_background_event(event);
            if started.elapsed() >= std::time::Duration::from_millis(4) {
                gui_wake::notify();
                break;
            }
        }
    }

    fn apply_background_event(&self, event: BackgroundEvent) {
        match event {
            BackgroundEvent::AttachmentPromptPrepared { job, key, generation, prompt } => {
                self.finish_job(job);
                let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
                if generation != owner.attachment_generation.get() { return; }
                owner.attachment_submission.set(false);
                owner.pending_prompt.replace(Some(prompt));
                self.dispatch_session_prompt(&owner);
                if self.session().key == key { self.update_chat_controls(); }
            }
            BackgroundEvent::AttachmentPrepared { job, key, id, result } => {
                self.finish_job(job);
                let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
                if owner.pending_attachments.borrow_mut().complete(id, result) {
                    if owner.chat_status_text.borrow().as_str() == "Preparing attachment…"
                        && owner.pending_attachments.borrow().items.iter().all(|item| item.result.is_some()) {
                        let status = if owner.pending_attachments.borrow().blocked() { "Check pending attachments before sending" } else { "Attachments ready" };
                        self.set_session_status(&owner, status);
                    }
                    if self.session().key == key {
                        self.refresh_pending_attachments();
                        self.update_chat_controls();
                    }
                }
            }
            BackgroundEvent::ProjectChosen { job, key, session, generation, result } => {
                let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
                self.ivars().project_choice_inflight.set(false);
                self.finish_job(job);
                match result {
                    Ok((project, directory)) => {
                        if self.project_by_id(&project.id).is_none() {
                            self.ivars().projects.borrow_mut().insert(0, project.clone());
                        }
                        if !self.ivars().termination_pending.get() && self.session().key == key
                            && generation == owner.evidence_generation.get()
                            && session == *owner.selected_session_path.borrow()
                        {
                            self.start_chat_in(&project.id, directory, None);
                        } else {
                            self.rebuild_sidebar_sessions();
                            self.set_chat_status("Project added. Start its conversation from the sidebar when ready.");
                        }
                    }
                    Err(error) => self.set_chat_status(&format!("Could not open the project: {error}")),
                }
            }
            BackgroundEvent::StartupLoaded(result) => self.apply_startup(result),
            BackgroundEvent::SessionViewsLoaded(result) => self.apply_saved_session_views(result),
            BackgroundEvent::RevealPrepared { job, result } => {
                self.finish_job(job);
                if self.ivars().termination_pending.get() { return; }
                match result {
                    Ok((path, false)) => self.reveal_in_finder(&path),
                    Ok((path, true)) => {
                        let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&path.to_string_lossy()), true);
                        if !NSWorkspace::sharedWorkspace().openURL(&url) {
                            self.set_chat_status(&format!("Could not open {}", path.display()));
                        }
                    }
                    Err(error) => self.set_chat_status(&error),
                }
            }
            BackgroundEvent::HeightsSaved(result) => {
                if let Err(error) = result {
                    eprintln!("omperor: could not save the transcript's heights: {error}");
                }
            }
            #[cfg(feature = "design-preview")]
            BackgroundEvent::PreviewPreferencesLoaded(mut preferences) => {
                if std::env::args().any(|arg| arg == "--light") {
                    preferences.appearance = AppearancePreference::Light;
                }
                self.ivars().preferences.replace(preferences.clone());
                self.ivars().preferences_loaded.set(true);
                self.apply_appearance(preferences.appearance);
                self.apply_chat_typography();
            }
            BackgroundEvent::ForkProbed {
                key,
                job,
                summary,
                directory,
                project_id,
                result,
            } => {
                let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
                self.finish_job(job);
                if self.ivars().termination_pending.get() { return; }
                if self.session().key == key { self.continue_fork(summary, directory, project_id, result); }
                else { self.set_session_status(&owner, "Repository inspected; choose Fork again to name the branch"); }
            }
            BackgroundEvent::WorktreeAdded {
                key,
                job,
                project_id,
                worktree,
                plan,
                result,
            } => {
                if self.ivars().live_sessions.borrow().get(key).is_none() { return; }
                self.finish_job(job);
                if self.ivars().termination_pending.get() { return; }
                match result {
                    Ok(()) => self.create_session_in(&project_id, worktree, Some(plan), self.session().key == key),
                    Err(error) => {
                        self.alert_message("Could not create the worktree", &error.to_string());
                    }
                }
            }
            BackgroundEvent::ProjectFilesIndexed {
                key,
                directory,
                files,
                filter,
            } => {
                let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
                owner.project_files_indexing.set(false);
                // Ignore a stale index if the project changed while it ran.
                if owner.current_project.borrow().as_path() == directory {
                    // The watcher judges changes by the same ignore files the
                    // walk just honored.
                    if let Some(watcher) = owner.project_watcher.borrow().as_ref()
                        && watcher.root() == directory
                    {
                        watcher.set_filter(filter);
                    }
                    owner.project_files.replace(Arc::new(files));
                    owner.project_files_dir.replace(Some(directory));
                    // An `@token` typed while the index was building is
                    // answered now, unless the picker was dismissed meanwhile.
                    if self.session().key == key && self.ivars().file_picker_pending_index.replace(false) {
                        self.update_file_picker();
                    }
                }
                // More changed while the walk ran: walk once more.
                if self.session().key == key && owner.project_files_dirty.replace(false) {
                    self.refresh_project_files();
                }
            }
            BackgroundEvent::Launched {
                key,
                app,
                target,
                result,
            } => { if self.session().key != key { return; } match result {
                Ok(()) => self.set_chat_status(&format!("Opened {} in {app}", path_leaf(&target))),
                Err(error) => self.set_chat_status(&format!(
                    "Could not open {} in {app}: {error}",
                    path_leaf(&target)
                )),
            } },
            BackgroundEvent::WorkspaceEvidenceRead {
                key,
                generation,
                session,
                id,
                result,
            } => {
                let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
                owner.workspace_reads
                    .borrow_mut()
                    .remove(&(generation, id.clone()));
                if generation != owner.evidence_generation.get()
                    || session != *owner.selected_session_path.borrow()
                {
                    return;
                }
                match result {
                    Ok(entry) => {
                        if !self.apply_session_transcript_updates(&owner, vec![transcripts::Update { id, entry }]) && self.session().key == key {
                            self.request_observatory_refresh();
                            self.refresh_agent_inspection();
                        }
                    },
                    Err(error) => {
                        owner.workspace_read_failures.borrow_mut().insert(id, error);
                        if self.session().key == key { self.request_observatory_refresh(); }
                    }
                }
            }
            BackgroundEvent::AgentInspectionRead {
                key,
                generation,
                session,
                id,
                result,
            } => {
                let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
                if generation != owner.agent_inspection_generation.get() {
                    return;
                }
                owner.agent_inspection_loading.replace(None);
                if session != *owner.selected_session_path.borrow() {
                    return;
                }
                match result {
                    Ok(entry) => {
                        if !self.apply_session_transcript_updates(&owner, vec![transcripts::Update { id, entry }]) && self.session().key == key {
                            self.request_observatory_refresh();
                            self.refresh_agent_inspection();
                        }
                    },
                    Err(error) => {
                        owner.agent_inspection_failure.replace(Some((
                            id,
                            format!("Could not read the task journal: {error}"),
                        )));
                        if self.session().key == key { self.refresh_agent_inspection(); }
                    }
                }
            }
            BackgroundEvent::UsageRead { key, generation, report } => {
                self.finish_usage_read(key, generation, report);
            }
            BackgroundEvent::MetricsRead { records } => {
                self.ivars().metrics_records.replace(records);
                self.ivars().metrics_loading.set(false);
                self.ivars().metrics_loaded.set(true);
                self.choose_default_metrics_session();
                self.sync_settings_metrics_ui();
            }
            BackgroundEvent::RemovalFinished { id, outcome } => self.apply_removal_finished(id, outcome),
            BackgroundEvent::ArchivesListed(result) => self.apply_archives_listed(result),
            BackgroundEvent::ArchiveDeleted { job, id, origin, result } => {
                self.apply_archive_deleted(job, id, origin, result);
            }
            BackgroundEvent::ArchiveRestored { job, id, origin, result, registry } => {
                self.apply_archive_restored(job, id, origin, result, registry);
            }
            BackgroundEvent::EmptySessionsSwept { job, origin, result } => {
                self.apply_empty_sessions_swept(job, origin, result);
            }
            BackgroundEvent::ToolAnswered { key, call, result } => {
                if let Some(owner) = self.ivars().live_sessions.borrow().get(key)
                    && let Some(runtime) = owner.omp_runtime.get()
                {
                    runtime.answer_host_tool(call, result);
                }
            }
            BackgroundEvent::SubagentTranscriptsSynced {
                key,
                generation,
                session,
                result,
            } => {
                let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return; };
                owner.subagent_sync_inflight.set(false);
                if generation == owner.evidence_generation.get()
                    && session == *owner.selected_session_path.borrow()
                {
                    let mut errors_changed = false;
                    {
                        let mut failures = owner.workspace_read_failures.borrow_mut();
                        for (id, error) in result.failures {
                            if failures.get(&id) != Some(&error) {
                                failures.insert(id, error);
                                errors_changed = true;
                            }
                        }
                    }
                    if !self.apply_session_transcript_updates(&owner, result.updates) && errors_changed && self.session().key == key {
                        self.request_observatory_refresh();
                    }
                }
            }
            BackgroundEvent::FilePickerFiltered {
                key,
                generation,
                query_start,
                matches,
            } => {
                // The composer moved past this query (another keystroke, a
                // dismissal, a commit, or a send) while it was being answered.
                if self.session().key != key || generation != self.ivars().file_picker_generation.get() {
                    return;
                }
                if matches.is_empty() {
                    self.hide_file_picker();
                    return;
                }
                self.ivars().file_picker_query_start.set(query_start);
                self.ivars().file_picker_selected.set(0);
                self.ivars().file_picker_matches.replace(matches);
                self.rebuild_file_picker();
            }
        }
    }

    /// Re-indexes the active project's files for the `@` picker on the worker
    /// pool; `file_picker::index_project_files` says what is listed. Runs with
    /// every workspace inspection — after each file-changing tool, the
    /// Refresh button, a project switch, and the watcher — coalesced so that
    /// a request while a walk is in flight queues exactly one more.
    fn refresh_project_files(&self) {
        let key = self.session().key;
        let owner = self.session();
        if owner.project_files_indexing.get() {
            owner.project_files_dirty.set(true);
            return;
        }
        let directory = owner.current_project.borrow().clone();
        owner.project_files_indexing.set(true);
        let submitted = self.jobs().submit(move || {
            let index = index_project_files(&directory);
            let filter = file_watcher::ChangeFilter::build(&directory, &index.ignore_dirs);
            BackgroundEvent::ProjectFilesIndexed {
                key,
                directory,
                files: index.files,
                filter,
            }
        });
        if !submitted {
            owner.project_files_indexing.set(false);
            // Without workers the picker simply has nothing to offer.
            owner.project_files.replace(Arc::default());
        }
    }

    /// Indexes the active project only if nothing is indexed for it yet —
    /// what a keystroke in the picker needs, without a walk per keystroke.
    fn ensure_project_files(&self) {
        let owner = self.session();
        let directory = owner.current_project.borrow().clone();
        if owner.project_files_dir
            .borrow()
            .as_deref()
            .is_some_and(|indexed| indexed == directory)
        {
            return;
        }
        self.refresh_project_files();
    }

    /// Keeps an FSEvents watcher on the active project so edits made outside
    /// Omperor — an editor, a terminal, git — refresh the Environment pane and
    /// the `@` index too. Replaced when the project changes; the previous
    /// watcher's thread winds down on its own.
    fn ensure_project_watcher(&self) {
        let owner = self.session();
        let root = owner.current_project.borrow().clone();
        if owner.project_watcher
            .borrow()
            .as_ref()
            .is_some_and(|watcher| watcher.root() == root)
        {
            return;
        }
        let (notices, _) = self.ivars().watch_notices.get_or_init(flume::unbounded);
        let watcher = match file_watcher::ProjectWatcher::start(root, notices.clone()) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                // Without a watcher the inspection triggers still cover the
                // agent's own edits; only out-of-band changes wait for Refresh.
                perf::event_with(
                    "gui.watcher.unavailable",
                    || serde_json::json!({ "error": error }),
                );
                None
            }
        };
        owner.project_watcher.replace(watcher);
    }

    /// Drains the watcher's notices from the poll timer and, once the tree
    /// has been quiet for `WATCH_QUIET_PERIOD`, inspects the workspace again —
    /// so a build writing thousands of files costs one refresh at its end.
    fn poll_project_watcher(&self) {
        let owner = self.session();
        let Some((_, notices)) = self.ivars().watch_notices.get() else {
            return;
        };
        let current = owner.current_project.borrow().clone();
        let mut noticed = false;
        while let Ok(root) = notices.try_recv() {
            noticed |= root == current;
        }
        if noticed {
            owner.workspace_change_seen_at
                .set(Some(std::time::Instant::now()));
        }
        if owner.workspace_change_seen_at
            .get()
            .is_some_and(|seen| seen.elapsed() >= WATCH_QUIET_PERIOD)
        {
            owner.workspace_change_seen_at.set(None);
            perf::event("gui.watcher.refresh");
            self.request_workspace_refresh();
        }
    }

    /// Replaces the composer with the adjacent submitted prompt for this
    /// session. Returning `false` leaves an arrow key to AppKit when there is
    /// no history movement to perform.
    /// Quitting stops every agent; ask first while any is working. Headless
    /// smokes and benchmarks (launched with arguments) never ask.
    fn confirm_quit_while_working(&self) -> bool {
        if std::env::args().len() > 1 {
            return true;
        }
        let working = self
            .ivars()
            .live_sessions
            .borrow()
            .entries()
            .iter()
            .filter(|entry| {
                entry.chat_busy.get()
                    || entry
                        .subagents
                        .borrow()
                        .iter()
                        .any(|agent| agents::AgentState::from_status(&agent.status).is_busy())
            })
            .count();
        if working == 0 {
            return true;
        }
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(&if working == 1 {
            "An agent is still working".to_owned()
        } else {
            format!("{working} sessions are still working")
        }));
        alert.setInformativeText(ns_string!(
            "Quitting stops them. Work already written to files is kept; the turn in progress is interrupted."
        ));
        alert.addButtonWithTitle(ns_string!("Quit"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        alert.runModal() == NSAlertFirstButtonReturn
    }

    /// Up recalls an earlier prompt only from the composer's first line, and
    /// Down moves forward only from its last line while a prompt is recalled,
    /// so arrow keys still move the caret through a multi-line draft.
    fn navigate_prompt_history(&self, direction: isize, editor: &NSTextView) -> bool {
        let owner = self.session();
        let text = editor.string().to_string();
        let caret = editor.selectedRange();
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let start = caret.location.min(utf16.len());
        let end = (caret.location + caret.length).min(utf16.len());
        let newline = u16::from(b'\n');
        let on_first_line = !utf16[..start].contains(&newline);
        let on_last_line = !utf16[end..].contains(&newline);
        let recall = if direction < 0 {
            on_first_line
        } else {
            on_last_line && owner.prompt_history.borrow().navigating()
        };
        if !recall {
            return false;
        }
        let Some(prompt) = owner.prompt_history.borrow_mut().navigate(direction, &text) else {
            return false;
        };
        let value = NSString::from_str(&prompt);
        editor.setString(&value);
        editor.setSelectedRange(NSRange::new(value.length(), 0));
        if let Some(input) = self.ivars().input.get() {
            input.setStringValue(&value);
        }
        owner.draft.replace(prompt);
        self.hide_file_picker();
        self.schedule_session_view_save();
        true
    }

    fn file_picker_visible(&self) -> bool {
        self.ivars()
            .file_picker
            .get()
            .is_some_and(|picker| !picker.isHidden())
    }

    /// Recomputes the `@` reference picker from the composer's current text and
    /// caret, showing it for an active `@token` and hiding it otherwise.
    fn update_file_picker(&self) {
        let key = self.session().key;
        let owner = self.session();
        let Some(input) = self.ivars().input.get() else {
            return;
        };
        let Some(editor) = input.currentEditor() else {
            self.hide_file_picker();
            return;
        };
        let caret = editor.selectedRange().location;
        let full = input.stringValue();
        if caret > full.length() {
            self.hide_file_picker();
            return;
        }
        let prefix = full.substringToIndex(caret).to_string();
        let Some((query, query_start)) = active_at_query(&prefix, caret) else {
            self.hide_file_picker();
            return;
        };
        self.ensure_project_files();
        let files = Arc::clone(&owner.project_files.borrow());
        if files.is_empty() {
            // Nothing indexed yet (or nothing to index): answer this token
            // when the index lands rather than making the user retype it.
            self.hide_file_picker();
            self.ivars().file_picker_pending_index.set(true);
            return;
        }
        // Filtering scans every path, which on a large monorepo takes several
        // milliseconds — too long for the main thread — so it runs on the
        // worker pool and reports back tagged with this request's generation.
        let generation = self.ivars().file_picker_generation.get() + 1;
        self.ivars().file_picker_generation.set(generation);
        let submitted = self.jobs().submit(move || {
            let matches = filter_project_files(&files, &query, FILE_PICKER_MAX_ROWS);
            BackgroundEvent::FilePickerFiltered {
                key,
                generation,
                query_start,
                matches,
            }
        });
        if !submitted {
            self.hide_file_picker();
        }
    }

    /// Builds (or rebuilds) the picker overlay for the current matches and shows
    /// it just above the composer.
    fn rebuild_file_picker(&self) {
        let mtm = self.mtm();
        let Some(input) = self.ivars().input.get() else {
            return;
        };
        // SAFETY: the composer field is installed in the chat pane for the
        // window's lifetime.
        let Some(superview) = (unsafe { input.superview() }) else {
            return;
        };
        let matches = self.ivars().file_picker_matches.borrow().clone();
        if matches.is_empty() {
            self.hide_file_picker();
            return;
        }
        let picker = self
            .ivars()
            .file_picker
            .get_or_init(|| {
                let container = card_box(rect(0.0, 0.0, 10.0, 10.0), 10.0, mtm);
                // The picker floats over the transcript, so it is opaque.
                container.setFillColor(&theme::popover());
                container.setContentViewMargins(NSSize::new(0.0, 0.0));
                container.setAutoresizingMask(NSAutoresizingMaskOptions::empty());
                superview.addSubview(&container);
                container
            })
            .clone();
        let content = picker.contentView().expect("box content view");
        // The selection highlight sits behind the rows; created once.
        let selection = self
            .ivars()
            .file_picker_selection
            .get_or_init(|| {
                let bar = NSBox::initWithFrame(
                    NSBox::alloc(mtm),
                    rect(0.0, 0.0, 10.0, FILE_PICKER_ROW_HEIGHT),
                );
                bar.setBoxType(NSBoxType::Custom);
                bar.setTitlePosition(NSTitlePosition::NoTitle);
                bar.setContentViewMargins(NSSize::new(0.0, 0.0));
                bar.setBorderWidth(0.0);
                bar.setBorderColor(&NSColor::clearColor());
                bar.setCornerRadius(6.0);
                bar.setFillColor(&theme::accent_fill());
                content.addSubview(&bar);
                bar
            })
            .clone();
        for row in self.ivars().file_picker_row_buttons.borrow_mut().drain(..) {
            row.removeFromSuperview();
        }
        let rows = matches.len().min(FILE_PICKER_MAX_ROWS);
        let height = rows as f64 * FILE_PICKER_ROW_HEIGHT + FILE_PICKER_PADDING * 2.0;
        let Some(composer) = self.ivars().composer_surface.get() else {
            return;
        };
        let frame = composer.frame();
        let width = frame.size.width;
        picker.setFrame(rect(
            frame.origin.x,
            frame.origin.y + frame.size.height + 8.0,
            width,
            height,
        ));
        let content_width = width - FILE_PICKER_PADDING * 2.0;
        let mut buttons = Vec::new();
        for (index, path) in matches.iter().take(rows).enumerate() {
            // The content view is not flipped, so the first row is highest.
            let y = height - FILE_PICKER_PADDING - FILE_PICKER_ROW_HEIGHT * (index as f64 + 1.0);
            let button = file_picker_row_button(path, index, self, mtm);
            button.setFrame(rect(
                FILE_PICKER_PADDING,
                y,
                content_width,
                FILE_PICKER_ROW_HEIGHT,
            ));
            content.addSubview(&button);
            buttons.push(button);
        }
        *self.ivars().file_picker_row_buttons.borrow_mut() = buttons;
        selection.setHidden(false);
        picker.setHidden(false);
        self.apply_file_picker_selection();
    }

    /// Moves the picker highlight to the current row.
    fn apply_file_picker_selection(&self) {
        let selected = self.ivars().file_picker_selected.get();
        let rows = self.ivars().file_picker_row_buttons.borrow();
        let (Some(selection), Some(button)) =
            (self.ivars().file_picker_selection.get(), rows.get(selected))
        else {
            return;
        };
        let frame = button.frame();
        selection.setFrame(rect(
            frame.origin.x - 2.0,
            frame.origin.y,
            frame.size.width + 4.0,
            frame.size.height,
        ));
    }

    fn move_file_picker_selection(&self, delta: isize) {
        let count = self.ivars().file_picker_matches.borrow().len();
        if count == 0 {
            return;
        }
        let current = self.ivars().file_picker_selected.get() as isize;
        let next = (current + delta).clamp(0, count as isize - 1) as usize;
        self.ivars().file_picker_selected.set(next);
        self.apply_file_picker_selection();
    }

    fn hide_file_picker(&self) {
        // A filter still running answers a question the composer has moved
        // past; bumping the generation discards its result on arrival.
        let generation = self.ivars().file_picker_generation.get() + 1;
        self.ivars().file_picker_generation.set(generation);
        self.ivars().file_picker_pending_index.set(false);
        if let Some(picker) = self.ivars().file_picker.get() {
            picker.setHidden(true);
        }
    }

    /// Inserts the picker's selected file path into the composer, replacing the
    /// `@token` that opened it.
    fn commit_file_picker(&self) {
        let index = self.ivars().file_picker_selected.get();
        let Some(path) = self
            .ivars()
            .file_picker_matches
            .borrow()
            .get(index)
            .cloned()
        else {
            self.hide_file_picker();
            return;
        };
        let Some(input) = self.ivars().input.get() else {
            return;
        };
        let Some(editor) = input.currentEditor() else {
            return;
        };
        let caret = editor.selectedRange().location;
        let query_start = self.ivars().file_picker_query_start.get();
        if query_start <= caret {
            let replacement = format!("{path} ");
            let range = NSRange::new(query_start, caret - query_start);
            // The range spans the `@token` within the live field editor; the
            // caret is then moved to just after the inserted path.
            editor.replaceCharactersInRange_withString(range, &NSString::from_str(&replacement));
            let new_caret = query_start + replacement.encode_utf16().count();
            editor.setSelectedRange(NSRange::new(new_caret, 0));
        }
        self.hide_file_picker();
    }

    fn prompt_for_branch_name(
        &self,
        summary: &session::SessionSummary,
        point: &workspace::BranchPoint,
    ) -> Option<String> {
        let _prompt = self.account_prompt_guard()?;
        let mtm = self.mtm();
        let field =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(0.0, 0.0, 380.0, 24.0));
        field.setStringValue(&NSString::from_str(&default_fork_branch_name(
            &summary.title,
            &summary.id,
        )));
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!(
            "Fork “{}” into a new worktree",
            summary.title
        )));
        let base = point.branch.as_deref().map_or_else(
            || format!("commit {}", &point.head[..point.head.len().min(12)]),
            |branch| format!("branch {branch}"),
        );
        alert.setInformativeText(&NSString::from_str(&format!(
            "Creates the branch below from {base} in {} and checks it out in its own worktree. The forked session continues from this session's transcript there.",
            point.repository_root.display()
        )));
        alert.setAccessoryView(Some(&field));
        alert.addButtonWithTitle(ns_string!("Fork"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        loop {
            alert
                .window()
                .makeFirstResponder(Some::<&NSResponder>(&field));
            if alert.runModal() != NSAlertFirstButtonReturn {
                return None;
            }
            let branch = field.stringValue().to_string().trim().to_owned();
            if workspace::branch_name_is_valid(&branch) {
                return Some(branch);
            }
            alert.setInformativeText(ns_string!(
                "That is not a valid git branch name. Use letters, digits, dots, dashes, and slashes."
            ));
        }
    }

    fn prompt_for_directory(
        &self,
        project: &Project,
        directories: &[std::path::PathBuf],
    ) -> Option<std::path::PathBuf> {
        let _prompt = self.account_prompt_guard()?;
        let mtm = self.mtm();
        let picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(0.0, 0.0, 380.0, 26.0),
            false,
        );
        for directory in directories {
            picker.addItemWithTitle(&NSString::from_str(&directory.display().to_string()));
        }
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!(
            "Which folder should this {} chat use?",
            project.name
        )));
        alert.setInformativeText(ns_string!(
            "The agent's tools and the Git inspector work in the folder you choose."
        ));
        alert.setAccessoryView(Some(&picker));
        alert.addButtonWithTitle(ns_string!("Start Chat"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return None;
        }
        directories
            .get(usize::try_from(picker.indexOfSelectedItem()).ok()?)
            .cloned()
    }

    fn rename_project_prompt(&self, project: &Project) {
        let Some(_prompt) = self.account_prompt_guard() else {
            return;
        };
        let mtm = self.mtm();
        let field =
            NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(0.0, 0.0, 320.0, 24.0));
        field.setStringValue(&NSString::from_str(&project.name));
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&format!("Rename {}", project.name)));
        alert.setInformativeText(ns_string!("The name groups sessions in the sidebar."));
        alert.setAccessoryView(Some(&field));
        alert.addButtonWithTitle(ns_string!("Save"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        loop {
            alert
                .window()
                .makeFirstResponder(Some::<&NSResponder>(&field));
            if alert.runModal() != NSAlertFirstButtonReturn {
                return;
            }
            let name = field.stringValue().to_string().trim().to_owned();
            if !name.is_empty() {
                let project_id = project.id.clone();
                self.write_project_registry(move || preferences::rename_project(&project_id, &name));
                return;
            }
            alert.setInformativeText(ns_string!("The project needs a name."));
        }
    }

    fn add_project_folder(&self, project: &Project) {
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        panel.setAllowsMultipleSelection(false);
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(path) = panel
            .URLs()
            .firstObject()
            .and_then(|url| url.path())
            .map(|path| std::path::PathBuf::from(path.to_string()))
        else {
            return;
        };
        let project_id = project.id.clone();
        self.write_project_registry(move || {
            let directory = std::fs::canonicalize(&path).unwrap_or(path);
            // The database enforces unique folder ownership on the worker.
            preferences::add_project_directory(&project_id, &directory)
        });
    }

    fn delete_project_prompt(&self, project: &Project) {
        let Some(_prompt) = self.account_prompt_guard() else {
            return;
        };
        let mtm = self.mtm();
        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(&format!(
            "Delete the project {}?",
            project.name
        )));
        alert.setInformativeText(ns_string!(
            "Sessions keep their transcripts and move to Other folders. Folders and worktrees on disk are not touched."
        ));
        alert.addButtonWithTitle(ns_string!("Delete Project"));
        alert.addButtonWithTitle(ns_string!("Cancel"));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return;
        }
        let project_id = project.id.clone();
        self.write_project_registry(move || preferences::delete_project(&project_id));
    }

    fn alert_message(&self, title: &str, message: &str) {
        let Some(_prompt) = self.account_prompt_guard() else {
            self.set_chat_status(title);
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(message));
        alert.addButtonWithTitle(ns_string!("OK"));
        alert.runModal();
    }

    /// Rebuilds the Projects page: one card per project with its folders,
    /// each removable, plus rename, add-folder, and delete actions.
    fn sync_settings_projects_ui(&self) {
        let owner = self.session();
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let mtm = self.mtm();
        let content = &settings.projects_content;
        let previous = content.subviews();
        for index in 0..previous.count() {
            previous.objectAtIndex(index).removeFromSuperview();
        }
        let projects = self.ivars().projects.borrow().clone();
        let records = self.ivars().session_records.borrow().clone();
        let active = owner.active_project_id.borrow().clone();

        const CARD_X: f64 = 40.0;
        const CARD_WIDTH: f64 = 600.0;
        const CARD_GAP: f64 = 16.0;
        const CARD_HEADER: f64 = 60.0;
        const CARD_FOOTER: f64 = 54.0;
        const ROW_HEIGHT: f64 = 30.0;
        const TOP_PADDING: f64 = 4.0;

        let card_height =
            |folders: usize| CARD_HEADER + folders.max(1) as f64 * ROW_HEIGHT + CARD_FOOTER;
        let content_height = projects
            .iter()
            .map(|project| card_height(project.directories.len()) + CARD_GAP)
            .sum::<f64>()
            + TOP_PADDING;
        let visible_height = settings.projects_scroll.frame().size.height;
        let document_height = content_height.max(visible_height);
        content.setFrame(rect(
            0.0,
            0.0,
            settings.projects_scroll.frame().size.width,
            document_height,
        ));

        let mut top = TOP_PADDING;
        for (project_index, project) in projects.iter().enumerate() {
            let height = card_height(project.directories.len());
            let card = settings_card(
                rect(CARD_X, document_height - top - height, CARD_WIDTH, height),
                mtm,
            );
            let session_count = records
                .values()
                .filter(|record| record.project_id == project.id)
                .count();
            let mut title = project.name.clone();
            if active.as_deref() == Some(project.id.as_str()) {
                title.push_str("  •  active");
            }
            let noun = if session_count == 1 {
                "session"
            } else {
                "sessions"
            };
            settings_row_text(
                &card,
                height - 30.0,
                &title,
                &format!("{session_count} {noun} recorded"),
                mtm,
            );
            if project.directories.is_empty() {
                let empty = text_label(
                    "No folders yet • add one to start chats here",
                    11.0,
                    false,
                    mtm,
                );
                empty.setTextColor(Some(&theme::secondary_text()));
                empty.setFrame(rect(
                    20.0,
                    height - CARD_HEADER - ROW_HEIGHT + 7.0,
                    400.0,
                    16.0,
                ));
                card.addSubview(&empty);
            }
            for (directory_index, directory) in project.directories.iter().enumerate() {
                let y = height - CARD_HEADER - (directory_index as f64 + 1.0) * ROW_HEIGHT;
                settings_separator(&card, y + ROW_HEIGHT, mtm);
                let path_label = settings_path_label(
                    &directory.display().to_string(),
                    rect(20.0, y + 7.0, 460.0, 16.0),
                    mtm,
                );
                if self.ivars().missing_project_directories.borrow().contains(directory) {
                    path_label.setTextColor(Some(&NSColor::systemRedColor()));
                    path_label.setToolTip(Some(ns_string!("This folder is missing on disk")));
                }
                card.addSubview(&path_label);
                let remove = settings_button(
                    "Remove",
                    rect(500.0, y + 2.0, 78.0, 26.0),
                    self,
                    sel!(settingsRemoveProjectFolder:),
                    mtm,
                );
                remove.setTag(
                    (project_index * SETTINGS_FOLDER_TAG_STRIDE + directory_index) as isize,
                );
                card.addSubview(&remove);
            }
            settings_separator(&card, CARD_FOOTER, mtm);
            let rename = settings_button(
                "Rename…",
                rect(20.0, 12.0, 96.0, 30.0),
                self,
                sel!(settingsRenameProject:),
                mtm,
            );
            rename.setTag(project_index as isize);
            card.addSubview(&rename);
            let add_folder = settings_button(
                "Add Folder…",
                rect(122.0, 12.0, 116.0, 30.0),
                self,
                sel!(settingsAddProjectFolder:),
                mtm,
            );
            add_folder.setTag(project_index as isize);
            card.addSubview(&add_folder);
            let archive = settings_button(
                "Archive…",
                rect(330.0, 12.0, 104.0, 30.0),
                self,
                sel!(settingsArchiveProject:),
                mtm,
            );
            archive.setTag(project_index as isize);
            archive.setToolTip(Some(ns_string!(
                "Put the project and its sessions away in a compressed archive; restore it from Settings › Archives"
            )));
            card.addSubview(&archive);
            let delete = settings_button(
                "Delete Project…",
                rect(440.0, 12.0, 136.0, 30.0),
                self,
                sel!(settingsDeleteProject:),
                mtm,
            );
            delete.setTag(project_index as isize);
            card.addSubview(&delete);
            content.addSubview(&card);
            top += height + CARD_GAP;
        }
        let clip = settings.projects_scroll.contentView();
        let clip_height = clip.bounds().size.height;
        clip.scrollToPoint(NSPoint::new(0.0, (document_height - clip_height).max(0.0)));
        settings.projects_scroll.reflectScrolledClipView(&clip);
    }

    fn refresh_project_location(&self) {
        let owner = self.session();
        let directory = owner.current_project.borrow().clone();
        let project = self.active_project();
        let location = project.as_ref().map_or_else(
            || directory.display().to_string(),
            |project| format!("{}  ·  {}", project.name, directory.display()),
        );
        self.ivars()
            .workspace_path_label
            .get()
            .expect("workspace path exists before showing a workspace")
            .setStringValue(&NSString::from_str(&location));
        self.layout_header();
        if let Some(settings) = self.ivars().settings_ui.get() {
            settings
                .workspace_label
                .setStringValue(&NSString::from_str(&location));
        }
    }

    fn store_preferences(&self) {
        if !self.ivars().preferences_loaded.get() {
            self.set_chat_status("Settings were not saved: Omperor's saved preferences have not loaded");
            return;
        }
        let snapshot = self.ivars().preferences.borrow().clone();
        self.write_preferences(move || preferences::store(&snapshot));
    }

    fn write_preferences(&self, work: impl FnOnce() -> Result<(), preferences::PreferencesError> + Send + 'static) {
        self.submit_storage(move || work().map(|()| StorageUpdate::None));
    }

    fn write_account_metadata(&self, work: impl FnOnce() -> Result<(), preferences::PreferencesError> + Send + 'static) {
        self.submit_storage(move || {
            work()?;
            preferences::load_accounts().map(StorageUpdate::Accounts)
        });
    }

    fn write_project_registry(&self, work: impl FnOnce() -> Result<(), preferences::PreferencesError> + Send + 'static) {
        self.submit_storage(move || {
            work()?;
            preferences::load_registry().map(StorageUpdate::Registry)
        });
    }

    fn submit_storage(&self, work: impl FnOnce() -> Result<StorageUpdate, preferences::PreferencesError> + Send + 'static) {
        // Ordered writes are drained before normal quit, without blocking AppKit.
        let writer = self.ivars().preference_writer.get_or_init(|| {
            jobs::JobPool::start_notifying(1, "omperor-preferences", gui_wake::notify)
        });
        if writer.submit(work) {
            self.ivars()
                .preference_saves_pending
                .set(self.ivars().preference_saves_pending.get() + 1);
            if self.ivars().preference_saves_pending.get() == 1 {
                self.ivars().preference_save_error.replace(None);
            }
        } else {
            self.ivars()
                .preference_save_error
                .replace(Some("The preferences worker is unavailable".into()));
        }
        self.sync_preference_save_status();
        self.sync_activity_indicator();
    }

    fn sync_preference_save_status(&self) {
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let error = self.ivars().preference_save_error.borrow();
        let label = if self.ivars().preference_saves_pending.get() > 0 {
            "Saving changes…"
        } else if error.is_some() {
            "Could not save changes"
        } else {
            "Changes save automatically"
        };
        settings
            .save_status
            .setStringValue(&NSString::from_str(label));
        settings.save_status.setToolTip(
            error
                .as_ref()
                .map(|error| NSString::from_str(error))
                .as_deref(),
        );
    }


}

/// The native message field, before any live conversation delegate or action
/// is attached. Settings examples use this same view in read-only form.
fn composer_input_field(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let input = NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(0.0, 0.0, 320.0, 62.0));
    input.setBezeled(false);
    input.setDrawsBackground(false);
    input.setEditable(false);
    input.setSelectable(true);
    input.setFont(Some(&theme::ui_font(theme::READING_SIZE, false)));
    input.setPlaceholderString(Some(ns_string!("What would you like to work on?")));
    input.setAccessibilityLabel(Some(ns_string!("Message")));
    input.setAccessibilityHelp(Some(ns_string!("Return sends your message. Shift Return inserts a new line. Type @ to reference a project file.")));
    input.setMaximumNumberOfLines(3);
    if let Some(cell) = input.cell() {
        cell.setUsesSingleLineMode(false);
        cell.setScrollable(false);
        cell.setWraps(true);
    }
    input
}

/// The same label treatment serves live composer chips and inert examples.
fn composer_control_label(value: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = text_label(value, theme::CONTROL_SIZE, false, mtm);
    label.setTextColor(Some(&theme::secondary_text()));
    label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    label
}

/// A label that wraps to its frame, for a sentence or two of guidance.
fn wrapping_label(value: &str, size: f64, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(value), mtm);
    label.setFont(Some(&theme::ui_font(size, false)));
    label.setTextColor(Some(&NSColor::labelColor()));
    label.setSelectable(false);
    label
}

fn text_label(value: &str, size: f64, bold: bool, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(value), mtm);
    let font = theme::ui_font(size, bold);
    label.setFont(Some(&font));
    label.setTextColor(Some(&NSColor::labelColor()));
    label
}

/// A section label at the shared caption size, in the secondary text color.
fn section_label(value: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = text_label(value, theme::CAPTION_SIZE, true, mtm);
    label.setTextColor(Some(&theme::secondary_text()));
    label
}

/// A text run with a font, a color, and letter spacing.
fn attributed(
    text: &str,
    font: &NSFont,
    color: &NSColor,
    kern: f64,
) -> Retained<NSMutableAttributedString> {
    let string = NSString::from_str(text);
    let run =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &string);
    let range = NSRange::new(0, string.length());
    // SAFETY: each attribute name is paired with the value type AppKit
    // documents for it, over exactly the run.
    unsafe {
        run.addAttribute_value_range(NSFontAttributeName, font, range);
        run.addAttribute_value_range(NSForegroundColorAttributeName, color, range);
        if kern != 0.0 {
            run.addAttribute_value_range(
                NSKernAttributeName,
                &NSNumber::numberWithDouble(kern),
                range,
            );
        }
    }
    run
}

/// A borderless box filled with `color` — the surface of a pane, the
/// pill behind a selected row, a chip behind a label.
/// Where a press plants its seed: a narrow bar of full gold inside the left
/// edge of the pill the row would get.
fn seed_frame(pill: NSRect) -> NSRect {
    rect(
        pill.origin.x,
        pill.origin.y + 4.0,
        3.0,
        (pill.size.height - 8.0).max(0.0),
    )
}

fn fill_box(frame: NSRect, color: &NSColor, radius: f64, mtm: MainThreadMarker) -> Retained<NSBox> {
    let view = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
    view.setBoxType(NSBoxType::Custom);
    view.setTitlePosition(NSTitlePosition::NoTitle);
    view.setContentViewMargins(NSSize::new(0.0, 0.0));
    view.setBorderWidth(0.0);
    view.setBorderColor(&NSColor::clearColor());
    view.setCornerRadius(radius);
    view.setFillColor(color);
    view
}

/// Lifts a card off the page on a soft, wide shadow, tinted plum-black so
/// it reads as depth on paper and on the plum-black page alike. The render
/// server draws it from the card's own shape; nothing is drawn per frame.
fn float_card(card: &NSView) {
    card.setWantsLayer(true);
    let Some(layer) = card.layer() else { return };
    let shadow = NSColor::colorWithSRGBRed_green_blue_alpha(0.10, 0.03, 0.10, 0.16);
    layer.setMasksToBounds(false);
    layer.setShadowColor(Some(&shadow.CGColor()));
    layer.setShadowOpacity(1.0);
    layer.setShadowRadius(16.0);
    layer.setShadowOffset(objc2_foundation::NSSize::new(0.0, -5.0));
}

/// A card: a lifted surface with a hairline edge and soft corners.
fn card_box(frame: NSRect, radius: f64, mtm: MainThreadMarker) -> Retained<NSBox> {
    let card = NSBox::initWithFrame(NSBox::alloc(mtm), frame);
    card.setBoxType(NSBoxType::Custom);
    card.setTitlePosition(NSTitlePosition::NoTitle);
    card.setBorderColor(&theme::hairline());
    card.setBorderWidth(1.0);
    card.setCornerRadius(radius);
    card.setFillColor(&theme::surface());
    card
}

/// The status line under the session title: a colored dot that reads the
/// state (green ready, amber busy, red trouble) followed by the message.
/// A confirmation on the status line: gold, the color of what the user
/// chose, so it reads as an answer to them rather than a state of the
/// session.
fn status_flash_attributed(value: &str, size: f64) -> Retained<NSMutableAttributedString> {
    let line = attributed(
        "●",
        &NSFont::systemFontOfSize(size * 7.0 / 12.0),
        &theme::accent(),
        0.0,
    );
    line.appendAttributedString(&attributed(
        &format!("  {value}"),
        &NSFont::systemFontOfSize(size),
        &theme::accent(),
        0.0,
    ));
    line
}

/// The header keeps its state dot; active work is shown beside the composer.
const STATUS_X: f64 = 28.0;
/// What the mock backend's status lines begin with; the header's pill shows
/// it instead of the line.
const MOCK_STATUS_PREFIX: &str = "Mock · temporary · ";

fn status_attributed(value: &str, size: f64) -> Retained<NSMutableAttributedString> {
    // The header's model pill says "temporary" for the mock; the line says
    // only the state (and its dot reads "Ready" as ready again).
    let value = value.strip_prefix(MOCK_STATUS_PREFIX).unwrap_or(value);
    let lower = value.to_lowercase();
    let dot = if lower.starts_with("ready") {
        NSColor::systemGreenColor()
    } else if lower.contains("fail")
        || lower.contains("error")
        || lower.contains("unavailable")
        || lower.contains("cannot")
        || lower.contains("could not")
    {
        NSColor::systemRedColor()
    } else if value.ends_with('…') {
        NSColor::systemYellowColor()
    } else {
        theme::secondary_text()
    };
    let line = attributed("●", &NSFont::systemFontOfSize(size * 7.0 / 12.0), &dot, 0.0);
    line.appendAttributedString(&attributed(
        &format!("  {value}"),
        &NSFont::systemFontOfSize(size),
        &theme::secondary_text(),
        0.0,
    ));
    line
}

/// One summary row in the Environment pane: a symbol, a quiet label, and a
/// value on the right; returns the value label so it can be set later.
fn inspector_row(
    pane: &NSView,
    y: f64,
    symbol: &str,
    label: &str,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let icon = NSImageView::initWithFrame(NSImageView::alloc(mtm), rect(18.0, y + 5.0, 16.0, 16.0));
    // SAFETY: AppKit exports NSFontWeightMedium for the process lifetime.
    icon.setImage(theme::symbol(symbol, 12.0, unsafe { NSFontWeightMedium }, None).as_deref());
    icon.setContentTintColor(Some(&theme::secondary_text()));
    icon.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
    let key = text_label(label, 12.0, false, mtm);
    key.setTextColor(Some(&theme::secondary_text()));
    key.setFrame(rect(42.0, y + 4.0, 90.0, 18.0));
    key.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
    let value = text_label("—", 12.0, false, mtm);
    // SAFETY: as above.
    value.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, unsafe {
        NSFontWeightMedium
    })));
    value.setAlignment(NSTextAlignment::Right);
    value.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    value.setFrame(rect(134.0, y + 4.0, INSPECTOR_WIDTH - 134.0 - 18.0, 18.0));
    value.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
    );
    pane.addSubview(&icon);
    pane.addSubview(&key);
    pane.addSubview(&value);
    value
}

/// A change tree file row: a colored status badge and the file name;
/// tagged with its row index in the tree.
fn change_row_button(
    change: &workspace::WorkspaceChange,
    name: &str,
    row: usize,
    target: &AppDelegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    // SAFETY: the delegate implements selectChange: and outlives the row.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(name),
            Some::<&AnyObject>(target),
            Some(sel!(selectChange:)),
            mtm,
        )
    };
    button.setBordered(false);
    button.setAlignment(NSTextAlignment::Left);
    button.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
    button.setTag(isize::try_from(row).unwrap_or(isize::MAX));
    let (badge, color) = match change.kind() {
        "New" | "Added" => ("A", NSColor::systemGreenColor()),
        "Deleted" => ("D", NSColor::systemRedColor()),
        "Renamed" => ("R", NSColor::systemBlueColor()),
        "Conflict" => ("U", NSColor::systemRedColor()),
        "Ignored" => ("I", theme::secondary_text()),
        _ => ("M", NSColor::systemOrangeColor()),
    };
    // SAFETY: AppKit exports NSFontWeightBold for the process lifetime.
    let badge_font = NSFont::monospacedSystemFontOfSize_weight(10.5, unsafe { NSFontWeightBold });
    let title = attributed(&format!("{badge}   "), &badge_font, &color, 0.0);
    title.appendAttributedString(&attributed(
        name,
        &theme::ui_font(theme::CONTROL_SIZE, false),
        &NSColor::labelColor(),
        0.0,
    ));
    button.setAttributedTitle(&title);
    button
}

/// A change tree directory row: a chevron that reads open or closed, the
/// directory (a collapsed chain reads `a/b/c`), and how many changed files
/// it holds; clicking it folds or unfolds the directory.
fn directory_row_button(
    name: &str,
    collapsed: bool,
    files: usize,
    row: usize,
    target: &AppDelegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    // SAFETY: the delegate implements toggleChangeDirectory: and outlives the row.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(name),
            Some::<&AnyObject>(target),
            Some(sel!(toggleChangeDirectory:)),
            mtm,
        )
    };
    button.setBordered(false);
    button.setAlignment(NSTextAlignment::Left);
    button.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
    button.setTag(isize::try_from(row).unwrap_or(isize::MAX));
    let chevron = if collapsed { "▸" } else { "▾" };
    let title = attributed(
        &format!("{chevron}  "),
        &theme::ui_font(theme::CAPTION_SIZE, false),
        &theme::secondary_text(),
        0.0,
    );
    // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
    title.appendAttributedString(&attributed(
        &format!("{name}/"),
        &NSFont::systemFontOfSize_weight(12.0, unsafe { NSFontWeightSemibold }),
        &NSColor::labelColor(),
        0.0,
    ));
    title.appendAttributedString(&attributed(
        &format!("  {files}"),
        &theme::ui_font(theme::CAPTION_SIZE, false),
        &theme::secondary_text(),
        0.0,
    ));
    button.setAttributedTitle(&title);
    button
}

/// A status dot for an agent or a sidebar row: filled gold while busy, a plum
/// ring while blocked, hollow while idle, green when done, red when failed.
/// A row's spinner: a session at work, from the moment its prompt leaves
/// until its turn settles.
fn row_spinner(frame: NSRect, mtm: MainThreadMarker) -> Retained<NSProgressIndicator> {
    let spinner = NSProgressIndicator::initWithFrame(NSProgressIndicator::alloc(mtm), frame);
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setIndeterminate(true);
    spinner.setControlSize(NSControlSize::Mini);
    spinner.setDisplayedWhenStopped(false);
    // SAFETY: startAnimation:/stopAnimation: take an optional sender and make no other demand.
    unsafe { spinner.startAnimation(None) };
    spinner
}

fn agent_dot(dot: agents::Dot, frame: NSRect, mtm: MainThreadMarker) -> Retained<NSBox> {
    let radius = frame.size.width / 2.0;
    let (fill, ring) = match dot {
        agents::Dot::Working => (Some(theme::accent()), None),
        agents::Dot::Blocked => (None, Some(theme::plum())),
        agents::Dot::Idle => (None, Some(theme::secondary_text())),
        agents::Dot::Done => (Some(NSColor::systemGreenColor()), None),
        agents::Dot::Errored => (Some(NSColor::systemRedColor()), None),
    };
    let view = fill_box(
        frame,
        &fill.unwrap_or_else(NSColor::clearColor),
        radius,
        mtm,
    );
    if let Some(ring) = ring {
        view.setBorderColor(&ring);
        view.setBorderWidth(1.5);
    }
    view
}

/// Whether a sidebar row earns a dot: only states that ask for attention or
/// mean something is happening; a session that simply finished stays quiet.
fn sidebar_dot(state: &agents::AgentState) -> Option<agents::Dot> {
    match state.dot() {
        agents::Dot::Working => Some(agents::Dot::Working),
        agents::Dot::Blocked => Some(agents::Dot::Blocked),
        agents::Dot::Errored => Some(agents::Dot::Errored),
        agents::Dot::Idle | agents::Dot::Done => None,
    }
}

/// Milliseconds since the Unix epoch, the clock OMP records job start times
/// against.
fn wall_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The wall clock at an `Instant` that has already passed.
fn wall_clock_ms_at(at: std::time::Instant) -> u64 {
    wall_clock_ms().saturating_sub(u64::try_from(at.elapsed().as_millis()).unwrap_or(0))
}

/// The last component of a path, for a status line.
fn path_leaf(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Line counts as `+added −removed`, green and red; a dash where they could
/// not be counted.
fn change_counts_attributed(
    insertions: Option<u64>,
    deletions: Option<u64>,
    size: f64,
) -> Retained<NSMutableAttributedString> {
    // SAFETY: AppKit exports NSFontWeightMedium for the process lifetime.
    let font = NSFont::monospacedSystemFontOfSize_weight(size, unsafe { NSFontWeightMedium });
    match (insertions, deletions) {
        (Some(added), Some(removed)) => {
            let line = attributed(
                &format!("+{added}"),
                &font,
                &NSColor::systemGreenColor(),
                0.0,
            );
            line.appendAttributedString(&attributed(
                &format!(" −{removed}"),
                &font,
                &NSColor::systemRedColor(),
                0.0,
            ));
            line
        }
        _ => attributed("—", &font, &theme::secondary_text(), 0.0),
    }
}

/// Right-aligns an attributed string: a label's own alignment does not
/// reach into attributed content, which carries its paragraph style.
fn aligned_right(text: &NSMutableAttributedString) -> Retained<NSMutableAttributedString> {
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(NSTextAlignment::Right);
    // SAFETY: the paragraph style is the documented value type, over the
    // whole string.
    unsafe {
        text.addAttribute_value_range(
            NSParagraphStyleAttributeName,
            &style,
            NSRange::new(0, text.length()),
        );
    }
    text.retain()
}

/// One session row's title.
///
/// Every row is set at the same weight, selected or not. Emboldening only the
/// open session re-measures the string, which moves where the ellipsis falls,
/// so titles jump as the user clicks down the list. Selection is carried by
/// the pill behind the row and the gold on its symbol instead, and the text
/// keeps the label color, which reads against both.
fn session_row_title(title: &str) -> Retained<NSMutableAttributedString> {
    attributed(
        title,
        &theme::ui_font(theme::NAVIGATION_SIZE, false),
        &NSColor::labelColor(),
        0.0,
    )
}

/// Project names stay separate from the native disclosure and folder symbols.
fn project_row_title(name: &str, active: bool) -> Retained<NSMutableAttributedString> {
    let color = if active { theme::accent() } else { NSColor::labelColor() };
    // SAFETY: AppKit exports this weight for the process lifetime.
    let font = NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightSemibold });
    attributed(name, &font, &color, 0.0)
}

fn settings_page_header(page: &NSView, title: &str, subtitle: &str, mtm: MainThreadMarker) {
    let heading = text_label(title, 24.0, true, mtm);
    // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
    heading.setFont(Some(&NSFont::systemFontOfSize_weight(24.0, unsafe {
        NSFontWeightSemibold
    })));
    heading.setFrame(rect(40.0, SETTINGS_HEIGHT - 64.0, 600.0, 32.0));
    heading.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
    let detail = text_label(subtitle, 12.0, false, mtm);
    detail.setTextColor(Some(&theme::secondary_text()));
    detail.setFrame(rect(40.0, SETTINGS_HEIGHT - 87.0, 600.0, 20.0));
    detail.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
    page.addSubview(&heading);
    page.addSubview(&detail);
}

fn settings_card(frame: NSRect, mtm: MainThreadMarker) -> Retained<NSBox> {
    card_box(frame, 12.0, mtm)
}

/// A settings row's title and explanation. The explanation stops short of
/// the control column on the right and wraps onto a second line when it
/// must; the caller gets it back to widen when the row has no control
/// beside it.
fn settings_row_text(
    card: &NSBox,
    title_y: f64,
    title: &str,
    detail: &str,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let title_label = text_label(title, 13.0, true, mtm);
    title_label.setFrame(rect(20.0, title_y, 330.0, 21.0));
    let detail_label = text_label(detail, 11.0, false, mtm);
    detail_label.setTextColor(Some(&theme::secondary_text()));
    // A label made by `labelWithString` clips at its edge; the explanation
    // must wrap onto a second line instead, and grow with the card.
    detail_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
    detail_label.setMaximumNumberOfLines(2);
    if let Some(cell) = detail_label.cell() {
        cell.setWraps(true);
        cell.setUsesSingleLineMode(false);
    }
    detail_label.setFrame(rect(20.0, title_y - 32.0, 352.0, 32.0));
    detail_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    card.addSubview(&title_label);
    card.addSubview(&detail_label);
    detail_label
}

fn settings_separator(card: &NSBox, y: f64, mtm: MainThreadMarker) {
    let separator = NSBox::initWithFrame(NSBox::alloc(mtm), rect(20.0, y, 556.0, 1.0));
    separator.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    separator.setBoxType(NSBoxType::Separator);
    card.addSubview(&separator);
}

fn settings_radio(
    title: &str,
    frame: NSRect,
    target: &AppDelegate,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    // SAFETY: AppDelegate implements every selector supplied by settings and
    // outlives the settings window retained in its ivars.
    let button = unsafe {
        NSButton::radioButtonWithTitle_target_action(
            &NSString::from_str(title),
            Some::<&AnyObject>(target),
            Some(action),
            mtm,
        )
    };
    button.setFrame(frame);
    button.setFont(Some(&theme::ui_font(theme::CONTROL_SIZE, false)));
    button
}

/// What an account's card says below its identity: the plan the provider
/// reports, a note when that is not known, and each quota window as a bar.
struct AccountCardPlan {
    plan: String,
    tone: PlanTone,
    note: Option<String>,
    note_is_error: bool,
    /// Every quota window, grouped; shown in full only when `expanded`.
    groups: Vec<UsageGroupPlan>,
    /// The one window that runs out first, which is all a folded card shows.
    summary: Option<UsageGroupPlan>,
    expanded: bool,
}

/// One set of quota windows: the account as a whole, or one metered feature.
struct UsageGroupPlan {
    title: String,
    rows: Vec<UsageRowPlan>,
}

/// One quota window: "Weekly limit", when it resets, and what is left.
#[derive(Clone)]
struct UsageRowPlan {
    title: String,
    /// Under the title: when the window resets, or what a billing window
    /// has spent against its cap.
    detail: Option<String>,
    left_percent: Option<u8>,
    /// The figure beside the bar; "{left}% left" unless the row says
    /// otherwise, as a billing window does with money.
    figure: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanTone {
    Known,
    Pending,
    Warning,
}

impl AccountCardPlan {
    /// The name row, the sign-in line, and the rule under them.
    const HEADER_HEIGHT: f64 = 82.0;
    /// The plan heading and its value.
    const PLAN_HEIGHT: f64 = 42.0;
    const NOTE_HEIGHT: f64 = 18.0;
    const GROUP_HEADING_HEIGHT: f64 = 32.0;
    const ROW_HEIGHT: f64 = 44.0;
    const BOTTOM_PADDING: f64 = 10.0;

    fn height(&self) -> f64 {
        Self::HEADER_HEIGHT
            + Self::PLAN_HEIGHT
            + if self.note.is_some() {
                Self::NOTE_HEIGHT
            } else {
                0.0
            }
            + self
                .shown_groups()
                .iter()
                .map(|group| {
                    Self::GROUP_HEADING_HEIGHT + group.rows.len() as f64 * Self::ROW_HEIGHT
                })
                .sum::<f64>()
            + Self::BOTTOM_PADDING
    }

    /// Whether folding hides anything: an account with a single window has
    /// nothing more to show, so it is laid out in full with no toggle.
    fn foldable(&self) -> bool {
        self.groups
            .iter()
            .map(|group| group.rows.len())
            .sum::<usize>()
            > 1
    }

    /// The groups the card lays out: all of them unfolded, the summary
    /// alone folded.
    fn shown_groups(&self) -> Vec<&UsageGroupPlan> {
        if self.expanded || !self.foldable() {
            self.groups.iter().collect()
        } else {
            self.summary.iter().collect()
        }
    }

    fn color(&self) -> Retained<NSColor> {
        match self.tone {
            PlanTone::Known => NSColor::labelColor(),
            PlanTone::Pending => theme::secondary_text(),
            PlanTone::Warning => NSColor::systemOrangeColor(),
        }
    }
}

/// The Metrics page as laid out: an intro line, cards, and a footer.
struct MetricsPagePlan {
    intro: String,
    cards: Vec<MetricsCardPlan>,
    footer: String,
}

/// One card on the Metrics page: a heading, an optional note, an optional
/// stacked bar (one share per row, in row order), optional column headings
/// (title and width; the first column takes what is left), and rows of
/// cells.
struct MetricsCardPlan {
    title: String,
    note: Option<String>,
    bar: Option<Vec<f64>>,
    columns: Option<Vec<(String, f64)>>,
    rows: Vec<Vec<String>>,
}

impl MetricsCardPlan {
    const HEADER_HEIGHT: f64 = 38.0;
    const NOTE_HEIGHT: f64 = 36.0;
    const BAR_HEIGHT: f64 = 22.0;
    const COLUMNS_HEIGHT: f64 = 22.0;
    const ROW_HEIGHT: f64 = 24.0;
    const BOTTOM_PADDING: f64 = 10.0;

    fn height(&self) -> f64 {
        Self::HEADER_HEIGHT
            + if self.note.is_some() {
                Self::NOTE_HEIGHT
            } else {
                0.0
            }
            + if self.bar.is_some() {
                Self::BAR_HEIGHT
            } else {
                0.0
            }
            + if self.columns.is_some() {
                Self::COLUMNS_HEIGHT
            } else {
                0.0
            }
            + self.rows.len() as f64 * Self::ROW_HEIGHT
            + Self::BOTTOM_PADDING
    }
}

/// The colours of the four shares of a turn, in the order the split lists
/// them: waiting for the first token, receiving tokens, tools, the rest.
fn metrics_share_colors() -> Vec<Retained<NSColor>> {
    vec![
        theme::accent_fill_strong(),
        theme::accent(),
        theme::plum(),
        theme::secondary_text(),
    ]
}

/// One row of the Metrics page's session list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MetricsSessionRow {
    /// `None` for the row that stands for every session.
    session: Option<String>,
    title: String,
    detail: String,
    tooltip: String,
}

/// The session list: "All sessions", then each session the log knows in
/// order of latest activity, titled the way the sidebar titles it when the
/// sidebar knows it and by a shortened id otherwise.
fn metrics_session_rows(
    sessions: &[metrics::SessionActivity],
    summaries: &[session::SessionSummary],
) -> Vec<MetricsSessionRow> {
    let total_turns: usize = sessions.iter().map(|session| session.turns).sum();
    let mut rows = vec![MetricsSessionRow {
        session: None,
        title: "All sessions".to_owned(),
        detail: format!(
            "{} session{} · {} turn{}",
            sessions.len(),
            if sessions.len() == 1 { "" } else { "s" },
            total_turns,
            if total_turns == 1 { "" } else { "s" }
        ),
        tooltip: "Everything the log holds".to_owned(),
    }];
    for activity in sessions {
        let summary = summaries.iter().find(|summary| summary.id == activity.id);
        let title = summary.map_or_else(
            || format!("Session {}", short_session_id(&activity.id)),
            |summary| summary.title.clone(),
        );
        let mut parts = Vec::new();
        if let Some(project) = summary.map(|summary| summary.project.as_str()) {
            if !project.is_empty() {
                parts.push(project.to_owned());
            }
        }
        parts.push(format!(
            "{} turn{}",
            activity.turns,
            if activity.turns == 1 { "" } else { "s" }
        ));
        parts.push(short_when(activity.latest_ms));
        rows.push(MetricsSessionRow {
            session: Some(activity.id.clone()),
            title,
            detail: parts.join(" · "),
            tooltip: format!(
                "{}\nLast activity {}",
                activity.id,
                format_timestamp(i64::try_from(activity.latest_ms).unwrap_or(i64::MAX))
            ),
        });
    }
    rows
}

/// The tail of a session id, enough to tell sessions apart.
fn short_session_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 8 {
        id.to_owned()
    } else {
        chars[chars.len() - 8..].iter().collect()
    }
}

/// "3:14 PM" within the day, otherwise the date.
fn short_when(at_ms: u64) -> String {
    let now = wall_clock_ms();
    let date = NSDate::dateWithTimeIntervalSince1970(at_ms as f64 / 1000.0);
    let within_day = now.abs_diff(at_ms) < 24 * 3600 * 1000;
    let (date_style, time_style) = if within_day {
        (
            NSDateFormatterStyle::NoStyle,
            NSDateFormatterStyle::ShortStyle,
        )
    } else {
        (
            NSDateFormatterStyle::MediumStyle,
            NSDateFormatterStyle::NoStyle,
        )
    };
    NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(&date, date_style, time_style)
        .to_string()
}

/// What the Metrics page says for `summary`, under `scope_title`.
fn metrics_page_plan(
    summary: &metrics::Summary,
    scope_title: &str,
    one_session: bool,
    loading: bool,
) -> MetricsPagePlan {
    // A long session title would push the counts off the line.
    let scope = shortened(scope_title, 44);
    let scope = scope.as_str();
    if summary.responses == 0 && summary.turns == 0 {
        let intro = if loading {
            "Reading the log…".to_owned()
        } else if one_session {
            "Nothing recorded for this session yet. Send a prompt and come back.".to_owned()
        } else {
            "Nothing recorded yet. Metrics are written as turns run.".to_owned()
        };
        return MetricsPagePlan {
            intro,
            cards: Vec::new(),
            footer: String::new(),
        };
    }
    let split = summary.split;
    let accounted = split.first_token_ms + split.receiving_ms + split.tools_ms + split.harness_ms;
    let share = |part: u64| {
        if accounted == 0 {
            0.0
        } else {
            part as f64 / accounted as f64
        }
    };
    let percent = |part: u64| format!("{}%", (share(part) * 100.0).round() as u64);
    let time_card = MetricsCardPlan {
        title: "Where the time went".to_owned(),
        note: Some(format!(
            "{} across {} turn{}: {} in inference, {} in tools.",
            duration_text(split.wall_ms.max(accounted)),
            summary.turns,
            if summary.turns == 1 { "" } else { "s" },
            duration_text(split.first_token_ms + split.receiving_ms),
            duration_text(split.tools_ms),
        )),
        bar: Some(vec![
            share(split.first_token_ms),
            share(split.receiving_ms),
            share(split.tools_ms),
            share(split.harness_ms),
        ]),
        columns: None,
        rows: vec![
            vec![
                "Waiting for the first token".to_owned(),
                duration_text(split.first_token_ms),
                percent(split.first_token_ms),
            ],
            vec![
                "Receiving tokens".to_owned(),
                duration_text(split.receiving_ms),
                percent(split.receiving_ms),
            ],
            vec![
                "Running tools".to_owned(),
                duration_text(split.tools_ms),
                percent(split.tools_ms),
            ],
            vec![
                "Harness and everything else".to_owned(),
                duration_text(split.harness_ms),
                percent(split.harness_ms),
            ],
        ],
    };
    let model_rows: Vec<Vec<String>> = summary
        .models
        .iter()
        .map(|model| {
            // The model without its provider prefix, then the provider.
            let name = model
                .model
                .strip_prefix(&format!("{}/", model.provider))
                .unwrap_or(&model.model);
            vec![
                if model.provider.is_empty() {
                    name.to_owned()
                } else {
                    format!("{name} · {}", model.provider)
                },
                model.responses.to_string(),
                model
                    .median_tps
                    .map_or_else(|| "—".to_owned(), |tps| format!("{}", tps.round() as u64)),
                model
                    .p90_tps
                    .map_or_else(|| "—".to_owned(), |tps| format!("{}", tps.round() as u64)),
                model
                    .median_ttft_ms
                    .map_or_else(|| "—".to_owned(), duration_text),
                if model.fast_responses == 0 {
                    "—".to_owned()
                } else {
                    format!("{}/{}", model.fast_responses, model.responses)
                },
            ]
        })
        .collect();
    let models_card = MetricsCardPlan {
        title: "Throughput by model".to_owned(),
        note: Some(
            "Median and p90 output tokens a second, first streamed token to last, from the provider's own counts (reasoning included); median time to first token."
                .to_owned(),
        ),
        bar: None,
        columns: Some(vec![
            ("Model".to_owned(), 0.0),
            ("Replies".to_owned(), 62.0),
            ("tok/s".to_owned(), 62.0),
            ("p90".to_owned(), 56.0),
            ("TTFT".to_owned(), 72.0),
            ("Fast".to_owned(), 48.0),
        ]),
        rows: if model_rows.is_empty() {
            vec![vec!["No responses yet.".to_owned()]]
        } else {
            model_rows
        },
    };
    let tool_rows: Vec<Vec<String>> = summary
        .tools
        .iter()
        .map(|tool| {
            vec![
                tool.name.clone(),
                tool.calls.to_string(),
                duration_text(tool.total_ms),
                duration_text(tool.median_ms),
                if tool.errors == 0 {
                    "—".to_owned()
                } else {
                    tool.errors.to_string()
                },
            ]
        })
        .collect();
    let tools_card = MetricsCardPlan {
        title: "Tools".to_owned(),
        note: None,
        bar: None,
        columns: Some(vec![
            ("Tool".to_owned(), 0.0),
            ("Calls".to_owned(), 62.0),
            ("Total".to_owned(), 84.0),
            ("Median".to_owned(), 84.0),
            ("Errors".to_owned(), 56.0),
        ]),
        rows: if tool_rows.is_empty() {
            vec![vec!["No tool calls yet.".to_owned()]]
        } else {
            tool_rows
        },
    };
    let period = match (summary.from_ms, summary.to_ms) {
        (Some(from), Some(to)) if to > from + 24 * 3600 * 1000 => format!(
            " · {} to {}",
            format_timestamp(i64::try_from(from).unwrap_or(i64::MAX)),
            format_timestamp(i64::try_from(to).unwrap_or(i64::MAX))
        ),
        _ => String::new(),
    };
    MetricsPagePlan {
        intro: format!(
            "{scope} · {} turn{} · {} response{} · {} tool call{}{period}",
            summary.turns,
            if summary.turns == 1 { "" } else { "s" },
            summary.responses,
            if summary.responses == 1 { "" } else { "s" },
            summary.tool_calls,
            if summary.tool_calls == 1 { "" } else { "s" },
        ),
        cards: vec![time_card, models_card, tools_card],
        footer: if loading {
            "Reading the log…".to_owned()
        } else {
            "Recorded as the turns ran; Refresh reads the log again.".to_owned()
        },
    }
}

/// `text` cut to `limit` characters with an ellipsis, at a word break when
/// one is near.
fn shortened(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let cut: String = text.chars().take(limit).collect();
    let cut = match cut.rfind(' ') {
        Some(space) if space > limit / 2 => &cut[..space],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end())
}

use metrics::duration_text;

use timeline::offset_text;

/// An event's sheet as one attributed text: each fact on its own line with
/// its name in the quiet tone, then each text behind the event under a
/// small heading; arguments and results in monospace, prose in the body
/// face.
fn timeline_event_text(info: &timeline::Info) -> Retained<NSMutableAttributedString> {
    let rendered = NSMutableAttributedString::initWithString(
        NSMutableAttributedString::alloc(),
        &NSString::from_str(""),
    );
    let body = theme::ui_font(15.0, false);
    // SAFETY: AppKit exports these weights for the process lifetime.
    let name_font = NSFont::systemFontOfSize_weight(12.0, unsafe { NSFontWeightSemibold });
    let heading_font = NSFont::systemFontOfSize_weight(12.0, unsafe { NSFontWeightSemibold });
    let mono = NSFont::monospacedSystemFontOfSize_weight(13.0, unsafe { NSFontWeightRegular });
    let width = info
        .facts
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    for (name, value) in &info.facts {
        rendered.appendAttributedString(&attributed(
            &format!("{name:<width$}  "),
            &NSFont::monospacedSystemFontOfSize_weight(13.0, unsafe { NSFontWeightSemibold }),
            &theme::secondary_text(),
            0.0,
        ));
        rendered.appendAttributedString(&attributed(
            &format!("{value}\n"),
            &body,
            &NSColor::labelColor(),
            0.0,
        ));
    }
    let _ = name_font;
    for (title, text) in &info.sections {
        rendered.appendAttributedString(&attributed(
            &format!("\n{}\n", title),
            &heading_font,
            &theme::secondary_text(),
            0.0,
        ));
        let monospaced = matches!(title.as_str(), "Arguments" | "Result");
        rendered.appendAttributedString(&attributed(
            &format!("{}\n", text.trim_end()),
            if monospaced { &mono } else { &body },
            &NSColor::labelColor(),
            0.0,
        ));
    }
    rendered
}

/// The line under the rate once a response settles: its time to first
/// token and how many tokens it was.
fn speedometer_detail_line(settled: &metrics::Settled) -> String {
    let mut parts = Vec::new();
    if let Some(ttft) = settled.ttft_ms {
        parts.push(format!("TTFT {}", duration_text(ttft)));
    }
    parts.push(format!("{} tokens", settled.tokens_out));
    if settled.flushed {
        // The rate above is over the whole turn, not a streamed span.
        parts.push("one flush".to_owned());
    }
    parts.join(" · ")
}

fn account_card_plan(
    provider: AuthProvider,
    usage: Option<&omp_runtime::AccountUsage>,
    loading: bool,
    usable: bool,
    expanded: bool,
) -> AccountCardPlan {
    let Some(usage) = usage else {
        return AccountCardPlan {
            plan: if !usable {
                "Unknown until the account is signed in".to_owned()
            } else if loading {
                "Checking with the provider…".to_owned()
            } else {
                "Not checked yet".to_owned()
            },
            tone: PlanTone::Pending,
            note: None,
            note_is_error: false,
            groups: Vec::new(),
            summary: None,
            expanded,
        };
    };
    if let Some(error) = &usage.error {
        // The provider's usage endpoint needs a credential OMP does not hold
        // for this sign-in (an API key, say); the account still works.
        let benign = error.contains("credential_unavailable");
        let note = if benign {
            "This sign-in has no credential OMP can ask the provider's usage service with."
                .to_owned()
        } else {
            error.clone()
        };
        return AccountCardPlan {
            plan: "Unavailable".to_owned(),
            tone: if benign { PlanTone::Pending } else { PlanTone::Warning },
            note: Some(note),
            note_is_error: !benign,
            groups: Vec::new(),
            summary: None,
            expanded,
        };
    }
    let mut groups: Vec<UsageGroupPlan> = Vec::new();
    let mut worst_used = 0u8;
    // The rate window closest to running out, which is what a folded card
    // shows; money is a cap, not a window, so it never stands in for one.
    let mut tightest: Option<(u8, UsageRowPlan)> = None;
    for window in &usage.windows {
        let title = usage_group_title(window.scope.as_deref(), &window.label);
        let row = match window.spend {
            Some(spend) => spend_row(spend, window.used_percent),
            None => UsageRowPlan {
                title: usage_row_title(&window.label),
                detail: window.resets_at_ms.map(reset_text),
                left_percent: window.used_percent.map(|used| 100u8.saturating_sub(used)),
                figure: None,
            },
        };
        if let Some(used) = window.used_percent {
            worst_used = worst_used.max(used);
            if window.spend.is_none() && tightest.as_ref().is_none_or(|(worst, _)| used > *worst) {
                let mut summary = row.clone();
                if title != GENERAL_USAGE_TITLE {
                    // Which feature's window it is, since the group heading
                    // that would say so is folded away.
                    let feature = title.trim_end_matches(" usage limits");
                    summary.title = format!("{} ({feature})", summary.title);
                }
                tightest = Some((used, summary));
            }
        }
        match groups.iter_mut().find(|group| group.title == title) {
            Some(group) => group.rows.push(row),
            None => groups.push(UsageGroupPlan {
                title,
                rows: vec![row],
            }),
        }
    }
    // The account as a whole comes before any one feature's limits.
    groups.sort_by_key(|group| group.title != GENERAL_USAGE_TITLE);
    let summary = tightest.map(|(_, row)| UsageGroupPlan {
        title: "Usage".to_owned(),
        rows: vec![row],
    });
    AccountCardPlan {
        plan: plan_display_name(provider, usage.plan.as_deref()),
        tone: if worst_used >= 90 {
            PlanTone::Warning
        } else {
            PlanTone::Known
        },
        note: None,
        note_is_error: false,
        groups,
        summary,
        expanded,
    }
}

/// The chevron beside a card's usage heading that unfolds every quota
/// window, or folds them back to the one that runs out first; tagged with
/// the account's index like the card's other controls.
fn usage_details_toggle(
    expanded: bool,
    frame: NSRect,
    account_index: usize,
    target: &AppDelegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let title = if expanded {
        "Hide details"
    } else {
        "Show details"
    };
    // SAFETY: the delegate implements toggleAccountUsageDetails: and
    // outlives the card.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some::<&AnyObject>(target),
            Some(sel!(toggleAccountUsageDetails:)),
            mtm,
        )
    };
    button.setFrame(frame);
    button.setBordered(false);
    button.setAlignment(NSTextAlignment::Right);
    button.setTag(isize::try_from(account_index).unwrap_or(isize::MAX));
    let chevron = if expanded { "▾" } else { "▸" };
    let attributed_title = attributed(
        &format!("{title}  {chevron}"),
        &theme::ui_font(theme::CAPTION_SIZE, false),
        &theme::secondary_text(),
        0.0,
    );
    button.setAttributedTitle(&attributed_title);
    button
}

const GENERAL_USAGE_TITLE: &str = "General usage limits";
const EXTRA_USAGE_TITLE: &str = "Extra usage";

/// "General usage limits" for the account's own windows; a metered feature
/// is named after the parenthetical its windows carry ("Spark usage limits",
/// "Fable usage limits"); pay-as-you-go spend past the plan is "Extra usage".
fn usage_group_title(scope: Option<&str>, label: &str) -> String {
    match scope {
        None | Some("shared") => GENERAL_USAGE_TITLE.to_owned(),
        Some("extra") => EXTRA_USAGE_TITLE.to_owned(),
        Some(scope) => match parenthetical(label) {
            Some(feature) => format!("{feature} usage limits"),
            None => format!("{} usage limits", capitalized(scope)),
        },
    }
}

/// "5 hours (Spark)" reads as "5-hour limit"; a week as "Weekly limit". A
/// provider's own name in front of the span ("Claude 7 Day") is dropped.
fn usage_row_title(label: &str) -> String {
    let span = match label.find(" (") {
        Some(end) => &label[..end],
        None => label,
    };
    let span = span.trim();
    let Some(start) = span.find(|c: char| c.is_ascii_digit()) else {
        return format!("{span} limit");
    };
    let span = span[start..].to_ascii_lowercase();
    match span.split_once(' ') {
        Some((count, unit)) if count.chars().all(|c| c.is_ascii_digit()) => {
            let unit = unit.trim().trim_end_matches('s');
            match (count, unit) {
                ("1", "day") => "Daily limit".to_owned(),
                ("7", "day") => "Weekly limit".to_owned(),
                ("30", "day") => "Monthly limit".to_owned(),
                _ => format!("{count}-{unit} limit"),
            }
        }
        _ => format!("{span} limit"),
    }
}

/// A billing window as money: what was spent against the cap, and what is
/// left of it. Without a cap there is nothing to show as remaining.
fn spend_row(spend: omp_runtime::Spend, used_percent: Option<u8>) -> UsageRowPlan {
    match spend.limit_cents {
        Some(limit) => UsageRowPlan {
            title: "Monthly spending limit".to_owned(),
            detail: Some(format!(
                "{} of {} spent",
                dollars(spend.used_cents),
                dollars(limit)
            )),
            left_percent: used_percent.map(|used| 100u8.saturating_sub(used)),
            figure: Some(format!(
                "{} left",
                dollars(limit.saturating_sub(spend.used_cents))
            )),
        },
        None => UsageRowPlan {
            title: "Spending, uncapped".to_owned(),
            detail: Some(format!("{} spent", dollars(spend.used_cents))),
            left_percent: None,
            figure: None,
        },
    }
}

/// "$1,234.56".
fn dollars(cents: u64) -> String {
    let whole = (cents / 100).to_string();
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("${grouped}.{:02}", cents % 100)
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn parenthetical(label: &str) -> Option<&str> {
    let start = label.find('(')? + 1;
    let end = label[start..].find(')')? + start;
    let inner = label[start..end].trim();
    (!inner.is_empty()).then_some(inner)
}

/// "Resets 3:14 AM" within the day, otherwise the date as well.
fn reset_text(at_ms: u64) -> String {
    let now = wall_clock_ms();
    let date = NSDate::dateWithTimeIntervalSince1970(at_ms as f64 / 1000.0);
    let within_day = at_ms > now && at_ms - now < 24 * 3600 * 1000;
    let text = NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
        &date,
        if within_day {
            NSDateFormatterStyle::NoStyle
        } else {
            NSDateFormatterStyle::MediumStyle
        },
        NSDateFormatterStyle::ShortStyle,
    );
    format!("Resets {text}")
}

/// The plan as a person names it. OpenAI reports ChatGPT plan slugs; OMP
/// reads Claude's from the account profile (`max_20x`, `pro`, `team`…).
fn plan_display_name(provider: AuthProvider, plan: Option<&str>) -> String {
    let plan = plan.map(str::to_ascii_lowercase);
    match provider {
        AuthProvider::OpenAi => match plan.as_deref() {
            Some("pro") => "ChatGPT Pro".to_owned(),
            Some("plus") => "ChatGPT Plus".to_owned(),
            Some("team") => "ChatGPT Team".to_owned(),
            Some("business") => "ChatGPT Business".to_owned(),
            Some("enterprise") => "ChatGPT Enterprise".to_owned(),
            Some("edu") => "ChatGPT Edu".to_owned(),
            Some("free") => "ChatGPT Free".to_owned(),
            Some(other) if !other.is_empty() => format!("ChatGPT {}", capitalized(other)),
            _ => "ChatGPT plan not reported".to_owned(),
        },
        AuthProvider::OpenAiApi => "API key, billed per token".to_owned(),
        AuthProvider::Anthropic => match plan.as_deref() {
            Some("max_20x") => "Claude Max 20×".to_owned(),
            Some("max_5x") => "Claude Max 5×".to_owned(),
            Some("max") => "Claude Max".to_owned(),
            Some("pro") => "Claude Pro".to_owned(),
            Some("team") => "Claude Team".to_owned(),
            Some("enterprise") => "Claude Enterprise".to_owned(),
            Some("free") => "Claude Free".to_owned(),
            Some(other) if !other.is_empty() => format!("Claude {}", capitalized(other)),
            _ => "Claude plan not reported".to_owned(),
        },
    }
}

const fn provider_account_detail(provider: AuthProvider) -> &'static str {
    match provider {
        AuthProvider::OpenAi => {
            "Your ChatGPT subscription: sign in in your browser, or paste an access token."
        }
        AuthProvider::OpenAiApi => "An OpenAI API key, billed per token.",
        AuthProvider::Anthropic => {
            "Your Claude subscription: sign in in your browser, or paste an API key."
        }
    }
}

fn sender_provider(sender: &AnyObject) -> Option<AuthProvider> {
    let button = sender.downcast_ref::<NSButton>()?;
    let index = usize::try_from(button.tag()).ok()?;
    AuthProvider::ALL.get(index).copied()
}

/// Replaces OMP's opaque account identifiers in a message with the names the
/// user knows the accounts by, so "no credential is stored for openai-codex
/// account openai-codex:83dd…" reads as "… account “Work”". An account the
/// user has neither labelled nor whose provider reports an email keeps its
/// identifier, which is still what the account menu shows.
fn name_accounts_in(text: &str, accounts: &[AccountEntry]) -> String {
    let mut named = text.to_owned();
    for entry in accounts {
        let Some(name) = entry
            .label
            .clone()
            .or_else(|| entry.summary.email().map(str::to_owned))
        else {
            continue;
        };
        if named.contains(entry.id()) {
            named = named.replace(entry.id(), &format!("“{name}”"));
        }
    }
    named
}

fn format_timestamp(unix_ms: i64) -> String {
    let date = NSDate::dateWithTimeIntervalSince1970(unix_ms as f64 / 1000.0);
    NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
        &date,
        NSDateFormatterStyle::MediumStyle,
        NSDateFormatterStyle::ShortStyle,
    )
    .to_string()
}

fn settings_path_label(value: &str, frame: NSRect, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = text_label(value, 10.5, false, mtm);
    label.setTextColor(Some(&theme::secondary_text()));
    label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
    label.setFrame(frame);
    label
}

fn settings_button(
    title: &str,
    frame: NSRect,
    target: &AppDelegate,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    // SAFETY: AppDelegate implements every selector supplied by settings and
    // outlives the settings window retained in its ivars.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some::<&AnyObject>(target),
            Some(action),
            mtm,
        )
    };
    button.setFrame(frame);
    button.setBezelStyle(NSBezelStyle::Push);
    button.setFont(Some(&theme::ui_font(theme::CONTROL_SIZE, false)));
    button
}

fn navigation_button(
    title: &str,
    symbol: &str,
    frame: NSRect,
    target: &AppDelegate,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    // SAFETY: Callers provide selectors implemented by the application
    // delegate, which outlives the complete view hierarchy.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some::<&AnyObject>(target),
            Some(action),
            mtm,
        )
    };
    button.setFrame(frame);
    button.setBordered(false);
    button.setAlignment(NSTextAlignment::Left);
    button.setFont(Some(&theme::ui_font(theme::NAVIGATION_SIZE, false)));
    button.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    button.setContentTintColor(Some(&NSColor::labelColor()));
    if let Some(image) = theme::row_symbol(symbol, 12.5, 7.0) {
        button.setImage(Some(&image));
        button.setImagePosition(NSCellImagePosition::ImageLeading);
    }
    button
}

/// A borderless row for the `@` file picker: the path, left-aligned, tagged with
/// its index into the current matches.
fn file_picker_row_button(
    path: &str,
    index: usize,
    target: &AppDelegate,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    // SAFETY: the delegate implements chooseFilePickerRow: and outlives the row.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(path),
            Some::<&AnyObject>(target),
            Some(sel!(chooseFilePickerRow:)),
            mtm,
        )
    };
    button.setBordered(false);
    button.setAlignment(NSTextAlignment::Left);
    button.setFont(Some(&NSFont::systemFontOfSize(12.5)));
    button.setContentTintColor(Some(&NSColor::labelColor()));
    // The directory in a quiet tone, the file name in the reading tone, so
    // the eye lands on what distinguishes each row.
    let (directory, name) = match path.rsplit_once('/') {
        Some((directory, name)) => (format!("{directory}/"), name),
        None => (String::new(), path),
    };
    let title = attributed(
        &directory,
        &NSFont::systemFontOfSize(12.5),
        &theme::secondary_text(),
        0.0,
    );
    title.appendAttributedString(&attributed(
        name,
        &NSFont::systemFontOfSize(12.5),
        &NSColor::labelColor(),
        0.0,
    ));
    button.setAttributedTitle(&title);
    button.setTag(isize::try_from(index).unwrap_or(isize::MAX));
    // Clicking a row must not end the composer's editing session, or its field
    // editor — where the path is inserted — would be gone by the time the action
    // fires.
    button.setRefusesFirstResponder(true);
    button
}

fn icon_button(
    symbol: &str,
    label: &str,
    frame: NSRect,
    target: &AppDelegate,
    action: objc2::runtime::Sel,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str(label)),
    );
    // Sidestep has its own symbol set. Keep an unsupported icon's action
    // usable with a text glyph and its original tooltip.
    let Some(image) = image else {
        let glyph = match symbol {
            "command" => "⌘",
            "sidebar.right" => "▥",
            "stop.circle" => "■",
            "arrow.left.and.right" => "↔",
            "line.3.horizontal.decrease" => "≡",
            _ => "⋯",
        };
        // SAFETY: the delegate implements the selector and owns the button.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(glyph),
                Some::<&AnyObject>(target),
                Some(action),
                mtm,
            )
        };
        button.setFrame(frame);
        button.setBordered(false);
        button.setToolTip(Some(&NSString::from_str(label)));
        return button;
    };
    let point_size = match symbol {
        "arrow.up.circle.fill" => 32.0,
        "stop.circle" => 28.0,
        _ => 15.0,
    };
    // SAFETY: AppKit exports the medium font weight for this process.
    let configuration =
        objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
            point_size,
            unsafe { NSFontWeightMedium },
        );
    let image = image
        .imageWithSymbolConfiguration(&configuration)
        .unwrap_or(image);
    // SAFETY: Callers provide selectors implemented by the application
    // delegate, which outlives the complete view hierarchy.
    let button = unsafe {
        NSButton::buttonWithImage_target_action(
            &image,
            Some::<&AnyObject>(target),
            Some(action),
            mtm,
        )
    };
    button.setFrame(frame);
    button.setBordered(false);
    button.setToolTip(Some(&NSString::from_str(label)));
    button
}

fn transcript_control(
    frame: NSRect,
    mtm: MainThreadMarker,
) -> (Retained<NSScrollView>, Retained<NSTextView>) {
    let scroll = NSTextView::scrollableTextView(mtm);
    scroll.setFrame(frame);
    scroll.setBorderType(NSBorderType::NoBorder);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );

    let transcript = transcript_view(&scroll);
    transcript.setEditable(false);
    transcript.setSelectable(true);
    transcript.setRichText(false);
    transcript.setDrawsBackground(false);
    transcript.setTextColor(Some(&NSColor::labelColor()));
    transcript.setFont(Some(&NSFont::systemFontOfSize(15.0)));
    transcript.setTextContainerInset(NSSize::new(18.0, 18.0));
    (scroll, transcript)
}

fn transcript_view(scroll_view: &NSScrollView) -> Retained<NSTextView> {
    scroll_view
        .documentView()
        .expect("AppKit scrollable text view must have a document view")
        .downcast::<NSTextView>()
        .expect("AppKit scrollable text view must contain NSTextView")
}

fn install_main_menu(app: &NSApplication, delegate: &AppDelegate, mtm: MainThreadMarker) {
    let main_menu = attachment_input::main_menu(mtm);
    let app_menu_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!(""),
            None,
            ns_string!(""),
        )
    };
    let app_name = app_state::Channel::current().app_name();
    let app_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(app_name));
    // SAFETY (this and the standard items below): each action is one
    // NSApplication or NSWindow implements, sent along the responder chain.
    unsafe {
        app_menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str(&format!("About {app_name}")),
            Some(sel!(orderFrontStandardAboutPanel:)),
            ns_string!(""),
        );
    }
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    let settings = unsafe {
        app_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Settings…"),
            Some(sel!(showSettings:)),
            ns_string!(","),
        )
    };
    unsafe { settings.setTarget(Some::<&AnyObject>(delegate)) };
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    unsafe {
        app_menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str(&format!("Hide {app_name}")),
            Some(sel!(hide:)),
            ns_string!("h"),
        );
        let others = app_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Hide Others"),
            Some(sel!(hideOtherApplications:)),
            ns_string!("h"),
        );
        others.setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Option);
        app_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Show All"),
            Some(sel!(unhideAllApplications:)),
            ns_string!(""),
        );
    }
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    unsafe {
        app_menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str(&format!("Quit {app_name}")),
            Some(sel!(terminate:)),
            ns_string!("q"),
        );
    }
    app_menu_item.setSubmenu(Some(&app_menu));
    main_menu.addItem(&app_menu_item);

    let file_menu_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!(""),
            None,
            ns_string!(""),
        )
    };
    let file_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("File"));
    file_menu.setAutoenablesItems(false);
    let new_chat = unsafe {
        file_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("New Conversation"),
            Some(sel!(newChat:)),
            ns_string!("n"),
        )
    };
    let open_workspace = unsafe {
        file_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("New Project"),
            Some(sel!(chooseProject:)),
            ns_string!("o"),
        )
    };
    let stop = unsafe {
        file_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Stop Response"),
            Some(sel!(stopOmp:)),
            ns_string!("."),
        )
    };
    // SAFETY: These menu actions are implemented by the application delegate,
    // which remains alive until NSApplication exits.
    unsafe {
        new_chat.setTarget(Some::<&AnyObject>(delegate));
        open_workspace.setTarget(Some::<&AnyObject>(delegate));
        stop.setTarget(Some::<&AnyObject>(delegate));
    }
    new_chat.setEnabled(false);
    delegate
        .ivars()
        .new_chat_menu_item
        .set(new_chat)
        .expect("new chat menu item once");
    file_menu.addItem(&NSMenuItem::separatorItem(mtm));
    // The project in the tools the user already has: an uppercase key
    // equivalent is Shift-Command, so these read ⇧⌘E, ⇧⌘T, and ⇧⌘R.
    let open_in_editor = unsafe {
        file_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Open Project in Editor"),
            Some(sel!(openProjectInEditor:)),
            ns_string!("E"),
        )
    };
    let open_in_terminal = unsafe {
        file_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Open Terminal in Project"),
            Some(sel!(openProjectInTerminal:)),
            ns_string!("T"),
        )
    };
    let reveal = unsafe {
        file_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Reveal Project in Finder"),
            Some(sel!(revealProject:)),
            ns_string!("R"),
        )
    };
    // SAFETY: the delegate implements all three selectors and outlives the menu.
    unsafe {
        open_in_editor.setTarget(Some::<&AnyObject>(delegate));
        open_in_terminal.setTarget(Some::<&AnyObject>(delegate));
        reveal.setTarget(Some::<&AnyObject>(delegate));
    }
    delegate
        .ivars()
        .editor_menu_item
        .set(open_in_editor)
        .expect("editor menu item once");
    delegate
        .ivars()
        .terminal_menu_item
        .set(open_in_terminal)
        .expect("terminal menu item once");
    delegate.sync_launcher_titles();
    file_menu_item.setSubmenu(Some(&file_menu));
    main_menu.addItem(&file_menu_item);

    let edit_menu_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!(""),
            None,
            ns_string!(""),
        )
    };
    let edit_menu = attachment_input::edit_menu(mtm);
    unsafe {
        edit_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Undo"),
            Some(sel!(undo:)),
            ns_string!("z"),
        );
        edit_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Redo"),
            Some(sel!(redo:)),
            ns_string!("Z"),
        );
    }
    edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
    unsafe {
        edit_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Cut"),
            Some(sel!(cut:)),
            ns_string!("x"),
        );
        edit_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Copy"),
            Some(sel!(copy:)),
            ns_string!("c"),
        );
        edit_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Paste"),
            Some(sel!(paste:)),
            ns_string!("v"),
        );
        edit_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Select All"),
            Some(sel!(selectAll:)),
            ns_string!("a"),
        );
    }
    edit_menu_item.setSubmenu(Some(&edit_menu));
    main_menu.addItem(&edit_menu_item);

    // Panels use ⌘1–2, session surfaces use ⌘3–4, and the timeline uses ⌘5.
    let view_menu_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!(""),
            None,
            ns_string!(""),
        )
    };
    let view_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("View"));
    for (title, action, key) in [
        ("Quick Actions…", sel!(showQuickActions:), "k"),
        ("Keep Agent Tab Open", sel!(openSelectedAgentTab:), "\r"),
        ("Changes", sel!(showChangesPanel:), "1"),
        ("Agents", sel!(showAgentsPanel:), "2"),
        ("Conversation", sel!(showConversationSurface:), "3"),
        ("Agent Observatory", sel!(showSessionWorkspace:), "4"),
        ("Session Timeline…", sel!(showSessionTimeline:), "5"),
    ] {
        // SAFETY: the delegate implements both selectors and outlives the menu.
        let item = unsafe {
            view_menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        unsafe { item.setTarget(Some::<&AnyObject>(delegate)) };
    }
    let toggle = unsafe {
        view_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Toggle Focus"),
            Some(sel!(toggleWorkbench:)),
            ns_string!("i"),
        )
    };
    toggle
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Shift);
    unsafe { toggle.setTarget(Some::<&AnyObject>(delegate)) };
    view_menu_item.setSubmenu(Some(&view_menu));
    main_menu.addItem(&view_menu_item);

    let window_menu_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!(""),
            None,
            ns_string!(""),
        )
    };
    let window_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Window"));
    unsafe {
        window_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Close Window"),
            Some(sel!(performClose:)),
            ns_string!("w"),
        );
        window_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Minimize"),
            Some(sel!(performMiniaturize:)),
            ns_string!("m"),
        );
        window_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Zoom"),
            Some(sel!(performZoom:)),
            ns_string!(""),
        );
    }
    window_menu.addItem(&NSMenuItem::separatorItem(mtm));
    unsafe {
        window_menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Bring All to Front"),
            Some(sel!(arrangeInFront:)),
            ns_string!(""),
        );
    }
    window_menu_item.setSubmenu(Some(&window_menu));
    main_menu.addItem(&window_menu_item);
    app.setWindowsMenu(Some(&window_menu));
    app.setMainMenu(Some(&main_menu));
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn project_tooltip(project: &Project) -> String {
    if project.directories.is_empty() {
        return format!("{} — no folders", project.name);
    }
    let folders = project
        .directories
        .iter()
        .map(|directory| directory.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n{folders}", project.name)
}

fn session_tooltip(summary: &session::SessionSummary, record: Option<&SessionRecord>) -> String {
    let mut tooltip = summary.menu_label();
    if let Some(model) = summary.model.as_deref() {
        tooltip.push_str(&format!("\nmodel {}", agents::short_model(model)));
    }
    if let Some(record) = record {
        if let Some(branch) = &record.branch {
            tooltip.push_str(&format!("\nbranch {branch}"));
            if let Some(base) = &record.base_branch {
                tooltip.push_str(&format!(" forked from {base}"));
            }
        }
        if let Some(worktree) = &record.worktree_path {
            tooltip.push_str(&format!("\nworktree {}", worktree.display()));
        }
    }
    tooltip
}

/// Groups the sidebar list. Omperor sessions sit under their project in
/// discovery order (forks are not nested; lineage lives in the tooltip), and
/// sessions no project owns under "Other folders"; the read-only OMP list
/// stays flat. A search hides projects
/// without a match and ignores collapse state.
/// Whether a finished tool could have changed the working tree. Unknown
/// tools count as mutating so the Environment pane errs toward refreshing.
fn tool_may_change_files(name: Option<&str>) -> bool {
    !matches!(
        name,
        Some(
            "read"
                | "grep"
                | "glob"
                | "think"
                | "debug"
                | "todo"
                | "ask"
                | "web_search"
                | "recall"
                | "reflect"
                | "goal"
                | "report_issue"
        )
    )
}

/// Which project a session lists under: the one its record names, else the
/// first whose folders, or whose sessions' folders and worktrees, include
/// the session's folder. `None` files it under Other folders.
fn session_project_position(
    summary: &session::SessionSummary,
    projects: &[Project],
    records: &HashMap<String, SessionRecord>,
) -> Option<usize> {
    if let Some(position) = records.get(&summary.id).and_then(|record| {
        projects
            .iter()
            .position(|project| project.id == record.project_id)
    }) {
        return Some(position);
    }
    let cwd = summary.cwd.as_deref()?;
    projects.iter().position(|project| {
        project.directories.iter().any(|directory| directory == cwd)
            || records.values().any(|record| {
                record.project_id == project.id
                    && (record.directory == cwd || record.worktree_path.as_deref() == Some(cwd))
            })
    })
}

fn sidebar_rows_for(
    projects: &[Project],
    sessions: &[session::SessionSummary],
    records: &HashMap<String, SessionRecord>,
    session_positions: &HashMap<String, i64>,
    filter: SessionSourceFilter,
    query: &str,
) -> Vec<SidebarRow> {
    let matches = |summary: &session::SessionSummary| {
        query.is_empty()
            || summary.title.to_lowercase().contains(query)
            || summary.project.to_lowercase().contains(query)
    };
    let mut rows = Vec::new();
    if filter == SessionSourceFilter::Omp {
        rows.extend(
            sessions
                .iter()
                .enumerate()
                .filter(|(_, summary)| {
                    filter.includes(summary.is_omperor_owned()) && matches(summary)
                })
                .map(|(index, _)| SidebarRow::Session {
                    index,
                    project_id: None,
                }),
        );
        return rows;
    }
    let project_of = |summary: &session::SessionSummary| session_project_position(summary, projects, records);
    let mut members = vec![Vec::new(); projects.len()];
    let mut unfiled = Vec::new();
    for (index, summary) in sessions.iter().enumerate() {
        if !summary.is_omperor_owned() || !matches(summary) {
            continue;
        }
        match project_of(summary) {
            Some(position) => members[position].push(index),
            None => unfiled.push(index),
        }
    }
    for (position, project) in projects.iter().enumerate() {
        let mut sessions_here = std::mem::take(&mut members[position]);
        if !query.is_empty() && sessions_here.is_empty() {
            continue;
        }
        // Sessions the user has dragged into an order hold that order. The
        // rest sit above them, newest created first: ordering by creation
        // rather than by last activity means a session finishing a turn never
        // jumps rows. Sessions with no record keep the list's order.
        let created = |index: usize| {
            records
                .get(&sessions[index].id)
                .map(|record| record.created_at_ms)
                .or_else(|| ulid_timestamp_ms(&sessions[index].id))
        };
        sessions_here.sort_by(|&left, &right| {
            match (
                session_positions.get(&sessions[left].id),
                session_positions.get(&sessions[right].id),
            ) {
                (None, None) => match (created(left), created(right)) {
                    (Some(left), Some(right)) => right.cmp(&left),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                },
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(left), Some(right)) => left.cmp(right),
            }
        });
        rows.push(SidebarRow::Project {
            project_id: project.id.clone(),
        });
        if project.collapsed && query.is_empty() {
            continue;
        }
        rows.extend(sessions_here.into_iter().map(|index| SidebarRow::Session {
            index,
            project_id: Some(project.id.clone()),
        }));
    }
    if !unfiled.is_empty() {
        rows.push(SidebarRow::Unfiled);
        rows.extend(unfiled.into_iter().map(|index| SidebarRow::Session {
            index,
            project_id: None,
        }));
    }
    rows
}

/// The creation time a ULID session id carries in its first ten characters.
fn ulid_timestamp_ms(id: &str) -> Option<i64> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    if id.len() != 26 {
        return None;
    }
    id.bytes().take(10).try_fold(0i64, |time, byte| {
        let digit = ALPHABET.iter().position(|&c| c == byte.to_ascii_uppercase())?;
        time.checked_mul(32)?.checked_add(digit as i64)
    })
}

/// Suggests a branch name for a fork: a slug of the session title plus a
/// short suffix from the session id so sibling forks do not collide.
fn default_fork_branch_name(title: &str, session_id: &str) -> String {
    const MAX_SLUG: usize = 32;
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character);
        } else {
            pending_dash = true;
        }
        if slug.len() > MAX_SLUG {
            break;
        }
    }
    // Cut an overlong slug at a word boundary rather than mid-word.
    if slug.len() > MAX_SLUG {
        let cut = slug[..MAX_SLUG].rfind('-').unwrap_or(MAX_SLUG);
        slug.truncate(cut);
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "session" } else { slug };
    let suffix = session_id
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .to_lowercase();
    format!("omperor/{slug}-{suffix}")
}

/// Names a new project after its folder, disambiguating against existing
/// project names with the parent folder and then a counter.
fn unique_project_name(directory: &std::path::Path, projects: &[Project]) -> String {
    let taken = |candidate: &str| {
        projects
            .iter()
            .any(|project| project.name.eq_ignore_ascii_case(candidate))
    };
    let base = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| directory.display().to_string());
    if !taken(&base) {
        return base;
    }
    if let Some(parent) = directory
        .parent()
        .and_then(std::path::Path::file_name)
        .map(|name| name.to_string_lossy())
    {
        let qualified = format!("{base} ({parent})");
        if !taken(&qualified) {
            return qualified;
        }
    }
    (2..)
        .map(|counter| format!("{base} {counter}"))
        .find(|candidate| !taken(candidate))
        .expect("an unused numbered project name exists")
}

fn model_change_requires_new_session(current: &str, next: &str) -> bool {
    let current = AuthProvider::for_model(current);
    let next = AuthProvider::for_model(next);
    current != next && (current.is_some() || next.is_some())
}

/// Picks the account a provider should serve from, in priority order: the
/// workspace's remembered choice, the account already pinned on the session,
/// the only usable account, then the provider default. Anything else needs
/// the user to choose.
fn resolve_provider_account(
    accounts: &[AccountEntry],
    provider: AuthProvider,
    project_choice: Option<&str>,
    current: Option<&str>,
) -> AccountResolution {
    let usable = accounts
        .iter()
        .filter(|entry| entry.provider() == provider && entry.is_usable())
        .collect::<Vec<_>>();
    if usable.is_empty() {
        return AccountResolution::Unavailable;
    }
    let is_usable = |id: &&str| usable.iter().any(|entry| entry.id() == *id);
    if let Some(id) = project_choice
        .filter(is_usable)
        .or_else(|| current.filter(is_usable))
    {
        return AccountResolution::Chosen(id.to_owned());
    }
    if let [only] = usable.as_slice() {
        return AccountResolution::Chosen(only.id().to_owned());
    }
    match usable.iter().find(|entry| entry.is_default) {
        Some(default) => AccountResolution::Chosen(default.id().to_owned()),
        None => AccountResolution::NeedsChoice,
    }
}

/// Labels are obligatory and must disambiguate: every new label is non-empty
/// and no two labels for a provider match, ignoring case.
fn labels_are_valid(new_labels: &[String], existing_labels: &[String]) -> bool {
    let mut seen = existing_labels
        .iter()
        .map(|label| label.trim().to_lowercase())
        .collect::<HashSet<_>>();
    new_labels.iter().all(|label| {
        let label = label.trim();
        !label.is_empty() && seen.insert(label.to_lowercase())
    })
}

fn model_picker_options<'a>(
    models: &'a [String],
    active: &'a str,
    authenticated_providers: &HashSet<AuthProvider>,
) -> Vec<&'a str> {
    if active == mock_inference::MODEL && models.iter().any(|model| model == active) { return vec![active]; }
    let mut options = Vec::with_capacity(MAX_MODEL_PICKER_ITEMS.min(models.len() + 1));
    let mut seen = std::collections::HashSet::with_capacity(options.capacity());
    let mut add = |model: &'a str| {
        if options.len() < MAX_MODEL_PICKER_ITEMS && !model.trim().is_empty() && seen.insert(model)
        {
            options.push(model);
        }
    };

    if models.iter().any(|model| model == active)
        && AuthProvider::for_model(active)
            .is_some_and(|provider| authenticated_providers.contains(&provider))
        && omp_runtime::is_supported_subscription_model(active)
    {
        add(active);
    }
    for provider in AuthProvider::ALL {
        if !authenticated_providers.contains(&provider) {
            continue;
        }
        for model in models.iter().map(String::as_str).filter(|model| {
            AuthProvider::for_model(model) == Some(provider)
                && omp_runtime::is_supported_subscription_model(model)
        }) {
            add(model);
        }
    }
    options
}

/// The window's public behaviour, for the headless Markdown benchmark and
/// dump; `displayed_answer` is the one method that reads the renderer.
impl markdown_bench::Host for Retained<AppDelegate> {
    fn window(&self) -> Option<Retained<NSWindow>> {
        self.ivars().window.get().cloned()
    }

    fn transcript_scroll_view(&self) -> Option<Retained<NSScrollView>> {
        self.ivars().chat_transcript.get().map(|transcript| transcript.scroll_view().clone())
    }

    fn ready(&self) -> bool {
        let owner = self.session();
        self.ivars().omperor_sessions_directory.get().is_some()
            && owner.backend.get() == mock_inference::Backend::Mock
            && owner.runtime_ready.get()
            && !owner.chat_busy.get()
            && !owner.attachment_submission.get()
            && owner.pending_prompt.borrow().is_none()
    }

    fn new_conversation(&self) -> u64 {
        let this: &AppDelegate = self;
        this.new_chat(sel!(newChat:), this);
        this.session().key.0
    }

    fn select_conversation(&self, key: u64) {
        self.capture_selected_binding();
        self.ivars().live_sessions.borrow_mut().select(live_sessions::SessionKey(key));
        self.render_selected_binding();
    }

    fn send_prompt(&self, prompt: &str) {
        let input = self.ivars().input.get().expect("the composer exists before prompts").clone();
        input.setStringValue(&NSString::from_str(prompt));
        self.send_message(sel!(sendMessage:), &input);
    }

    fn turn_running(&self) -> bool {
        let owner = self.session();
        owner.chat_busy.get() || owner.attachment_submission.get() || owner.pending_prompt.borrow().is_some()
    }

    fn inject_synthetic_accounts(&self, in_flight: bool) {
        self.inject_synthetic_account_usage(in_flight);
    }

    fn open_slash_list(&self, text: &str) {
        let Some(input) = self.ivars().input.get() else { return };
        if let Some(window) = self.ivars().window.get() {
            window.makeFirstResponder(Some(input));
        }
        let value = NSString::from_str(text);
        input.setStringValue(&value);
        if let Some(editor) = input.currentEditor() {
            editor.setString(&value);
            editor.setSelectedRange(NSRange::new(value.length(), 0));
        }
        self.update_slash_picker();
    }

    fn set_look(&self, style: &str, dark: bool) {
        let appearance = if dark { AppearancePreference::Dark } else { AppearancePreference::Light };
        {
            let mut preferences = self.ivars().preferences.borrow_mut();
            preferences.chat_style = theme::ChatStyle::from_label(style);
            preferences.appearance = appearance;
        }
        self.apply_appearance(appearance);
        self.apply_chat_typography();
    }

    fn displayed_answer(&self) -> Option<markdown_bench::Displayed> {
        self.ivars().chat_transcript.get()?.displayed_answer()
    }

    fn begin_equivalence(&self) -> bool {
        self.ivars().chat_transcript.get().is_some_and(|transcript| transcript.begin_equivalence())
    }

    fn capture_equivalence(&self, capture: &mut dyn FnMut(&NSView, NSRect)) {
        if let Some(transcript) = self.ivars().chat_transcript.get() {
            transcript.capture_equivalence(capture);
        }
    }

    fn swap_equivalence(&self) -> Option<transcript_stack::Equivalence> {
        self.ivars().chat_transcript.get()?.swap_equivalence()
    }

    fn end_equivalence(&self) {
        if let Some(transcript) = self.ivars().chat_transcript.get() {
            transcript.end_equivalence();
        }
    }

    fn replay_begin(&self) {
        {
            let owner = self.session();
            let mut conversation = owner.conversation.borrow_mut();
            conversation.begin_turn();
            conversation.submit_user("Replay");
            conversation.start_assistant();
        }
        self.refresh_chat_transcript();
    }

    fn replay_append(&self, delta: &str) {
        self.session().conversation.borrow_mut().append_assistant(delta);
        self.refresh_chat_transcript();
    }

    fn replay_finish(&self) {
        {
            let owner = self.session();
            let mut conversation = owner.conversation.borrow_mut();
            conversation.finish_assistant(None);
            conversation.finish_turn();
        }
        self.refresh_chat_transcript();
    }

    fn reload_answer(&self) {
        if let Some(transcript) = self.ivars().chat_transcript.get() {
            transcript.forget_parses();
        }
        self.refresh_chat_transcript();
    }

    fn replay_state(&self) -> Option<transcript_stack::ReplayState> {
        self.ivars().chat_transcript.get()?.replay_state()
    }

    fn open_history(&self, name: &str, messages: Vec<session::SessionMessage>) {
        let project = self.session().current_project.borrow().clone();
        let summary = session::SessionSummary::bench(&format!("bench-{name}"), name, &project);
        self.load_summary_from(summary, true, Some(messages));
    }

    fn history_loading(&self) -> bool {
        self.session().session_history.borrow().is_some()
    }

    fn probe(&self, bottom: bool) -> Option<(String, f64)> {
        self.ivars().chat_transcript.get()?.probe(bottom)
    }

    fn locate(&self, key: &str) -> Option<f64> {
        self.ivars().chat_transcript.get()?.locate(key)
    }

    fn viewport_gaps(&self) -> (usize, usize) {
        self.ivars().chat_transcript.get().map_or((0, 0), |transcript| transcript.viewport_gaps())
    }

    fn content_extent(&self) -> (usize, usize) {
        self.ivars().chat_transcript.get().map_or((0, 0), |transcript| transcript.content_extent())
    }

    fn open_journal(&self, id: &str) {
        let project = self.session().current_project.borrow().clone();
        let summary = session::SessionSummary::bench(id, id, &project);
        self.load_summary_from(summary, true, None);
    }

    fn layout_violations(&self) -> Vec<String> {
        self.ivars().chat_transcript.get().map_or_else(Vec::new, |transcript| transcript.layout_violations())
    }

    fn invalidate_visible(&self) {
        if let Some(transcript) = self.ivars().chat_transcript.get() {
            transcript.invalidate_visible();
        }
    }
}

thread_local! {
    /// The journal whose history the transcript shows, and when its heights
    /// are next due to be saved (after the history ends and rendering has
    /// had time to settle).
    static SAVED_HEIGHTS: std::cell::RefCell<(Option<std::path::PathBuf>, Option<std::time::Instant>)> = const { std::cell::RefCell::new((None, None)) };
}

/// Heights are saved this long after the history finished loading.
const SAVE_HEIGHTS_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

fn prompt_submission_enabled(
    authenticated: bool,
    runtime_ready: bool,
    runtime_starting: bool,
    new_session_pending: bool,
) -> bool {
    authenticated && (runtime_ready || runtime_starting || new_session_pending)
}

fn main() {
    // Headless execution must precede all AppKit and preference initialization.
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(code) = app_state::dispatch(&arguments).or_else(|| eval::dispatch(arguments)) {
        std::process::exit(code);
    }
    // `--markdown-bench` / `--markdown-dump`: refuse the real home and set up
    // the scratch one before anything reads it or any thread starts.
    markdown_bench::prepare();
    #[cfg(target_os = "linux")]
    if scroll_smoke::requested() {
        scroll_smoke::refuse_real_home();
        scroll_smoke::prepare_capture();
    }
    let _ = perf::initialize();
    // Always on: every main-thread pass over one 240 Hz frame is written down.
    frame_budget::install(preferences::omp_sessions_directory().ok());
    // The highlighter's syntax and theme sets deserialize slowly; a worker
    // loads them now so the first code block does not do so on the main thread.
    let _ = std::thread::Builder::new()
        .name("syntax-warm-up".into())
        .spawn(native_markdown::warm_up_highlighting);
    let mtm = MainThreadMarker::new().expect("AppKit must start on the main thread");
    // The Markdown renderer's start-up (vendor/upleft, docs/EMBEDDING.md):
    // Mermaid diagrams, and the math fonts the app bundle carries.
    upleft_mermaid::downright::mermaid_renderer_bridge::install_fragment_renderer();
    // A binary run outside the bundle (`cargo run`, `target/release`) has
    // none in its resources; it uses the checkout's copy, which
    // scripts/build-app.sh puts in the bundle.
    let bundled = objc2_foundation::NSBundle::mainBundle()
        .resourcePath()
        .map(|resources| std::path::Path::new(&resources.to_string()).join("mathFonts.bundle"));
    let checkout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/upleft/vendor/downright/Vendor/SwiftMath/Sources/SwiftMath/mathFonts.bundle");
    if let Some(fonts) = bundled.into_iter().chain([checkout]).find(|fonts| fonts.is_dir()) {
        upleft_math::math_bundle::math_resource_bundle::set_math_fonts_directory(fonts);
    }
    perf::event("gui.main_run_loop.enter");
    autoreleasepool(|_| {
        // The subclass that times every event it dispatches; it must be the
        // first to be asked for the shared application.
        let app = event_spans::shared_application(mtm);
        markdown_bench::hide_application(&app);
        if readme_shots::requested() {
            markdown_bench::keep_off_screen(&app);
        }
        let delegate = AppDelegate::new(mtm);
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        app.run();
    });
}

#[cfg(test)]
mod main_tests {
    use std::collections::HashSet;

    use std::collections::HashMap;

    use super::{
        AccountEntry, AccountResolution, AuthProvider, SessionSourceFilter, SidebarRow,
        default_fork_branch_name, labels_are_valid, model_change_requires_new_session,
        model_picker_options, name_accounts_in, prompt_submission_enabled,
        resolve_provider_account, sidebar_rows_for,
        tool_may_change_files,
        unique_project_name,
    };
    use crate::omp_runtime::{AccountState, AccountSummary};
    use crate::preferences::{Project, SessionRecord};
    use crate::session::SessionSummary;

    fn project(id: &str, name: &str, directories: &[&str], collapsed: bool) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            directories: directories.iter().map(Into::into).collect(),
            last_opened_at_ms: 0,
            collapsed,
        }
    }

    fn record(
        session_id: &str,
        project_id: &str,
        directory: &str,
        parent: Option<&str>,
    ) -> SessionRecord {
        SessionRecord {
            session_id: session_id.into(),
            project_id: project_id.into(),
            directory: directory.into(),
            parent_session_id: parent.map(str::to_owned),
            branch: parent.map(|_| "omperor/fork".to_owned()),
            base_branch: parent.map(|_| "main".to_owned()),
            worktree_path: parent.map(|_| directory.into()),
            created_at_ms: 0,
        }
    }

    #[test]
    fn ulid_session_ids_carry_their_creation_time() {
        assert_eq!(super::ulid_timestamp_ms("01M4A5FVJ4B2B29K6GJS3WZKAP"), Some(1_791_342_669_380));
        assert!(super::ulid_timestamp_ms("01M4A5FVJ4B2B29K6GJS3WZKAP") > super::ulid_timestamp_ms("01M30XF8KTV881T8J22B2W0K3B"));
        assert_eq!(super::ulid_timestamp_ms("research"), None);
    }

    #[test]
    fn sidebar_groups_sessions_under_projects_without_nesting_forks() {
        let projects = vec![
            project("p-app", "app", &["/work/app"], false),
            project("p-lib", "lib", &["/work/lib"], true),
        ];
        // Discovery lists newest first; forks stay in that order rather than
        // nesting under their parents.
        let sessions = vec![
            SessionSummary::for_test("fork", "Fork task", Some("/wt/app/fork"), true),
            SessionSummary::for_test("root", "Root task", Some("/work/app"), true),
            SessionSummary::for_test("grandchild", "Deeper", Some("/wt/app/deeper"), true),
            SessionSummary::for_test("lib-session", "Lib task", Some("/work/lib"), true),
            SessionSummary::for_test("stray", "Elsewhere", Some("/elsewhere"), true),
            SessionSummary::for_test("tui", "TUI chat", Some("/work/app"), false),
        ];
        let created = |mut record: SessionRecord, at| {
            record.created_at_ms = at;
            record
        };
        let records = [
            created(record("root", "p-app", "/work/app", None), 10),
            created(record("fork", "p-app", "/wt/app/fork", Some("root")), 20),
            created(record("grandchild", "p-app", "/wt/app/deeper", Some("fork")), 30),
        ]
        .into_iter()
        .map(|record| (record.session_id.clone(), record))
        .collect::<HashMap<_, _>>();

        let positions = HashMap::new();
        let session = |index: usize, project: Option<&str>| SidebarRow::Session {
            index,
            project_id: project.map(str::to_owned),
        };
        let rows = sidebar_rows_for(
            &projects,
            &sessions,
            &records,
            &positions,
            SessionSourceFilter::Omperor,
            "",
        );
        // Unarranged sessions are newest created first, whatever order
        // recent activity put the discovered list in.
        assert_eq!(
            rows,
            vec![
                SidebarRow::Project {
                    project_id: "p-app".into()
                },
                session(2, Some("p-app")),
                session(0, Some("p-app")),
                session(1, Some("p-app")),
                // Collapsed projects keep their heading and hide members.
                SidebarRow::Project {
                    project_id: "p-lib".into()
                },
                SidebarRow::Unfiled,
                session(4, None),
            ]
        );

        // A stored session order pins those sessions below the unpositioned
        // ones.
        let ordered = HashMap::from([("grandchild".to_owned(), 0), ("fork".to_owned(), 1)]);
        let rows = sidebar_rows_for(
            &projects,
            &sessions,
            &records,
            &ordered,
            SessionSourceFilter::Omperor,
            "",
        );
        let app_sessions: Vec<_> = rows
            .iter()
            .skip_while(|row| !matches!(row, SidebarRow::Session { .. }))
            .take_while(|row| matches!(row, SidebarRow::Session { .. }))
            .cloned()
            .collect();
        assert_eq!(
            app_sessions,
            vec![
                session(1, Some("p-app")), // root: unpositioned, above the arranged
                session(2, Some("p-app")), // grandchild: position 0
                session(0, Some("p-app")), // fork: position 1
            ]
        );

        // Searching ignores collapse and drops projects without a match.
        let rows = sidebar_rows_for(
            &projects,
            &sessions,
            &records,
            &positions,
            SessionSourceFilter::Omperor,
            "lib",
        );
        assert_eq!(
            rows,
            vec![
                SidebarRow::Project {
                    project_id: "p-lib".into()
                },
                session(3, Some("p-lib")),
            ]
        );

        // The read-only tab stays a flat list of foreign sessions.
        let rows = sidebar_rows_for(
            &projects,
            &sessions,
            &records,
            &positions,
            SessionSourceFilter::Omp,
            "",
        );
        assert_eq!(rows, vec![session(5, None)]);
    }

    #[test]
    fn fork_branch_names_are_slugs_with_a_session_suffix() {
        assert_eq!(
            default_fork_branch_name("Reply with exactly: pong!", "01M1MY90K25SRDQ29ZFCN2DGE9"),
            "omperor/reply-with-exactly-pong-dge9"
        );
        assert_eq!(
            default_fork_branch_name(
                "Reply with exactly: account pin verified",
                "01M1MY90K25SRDQ29ZFCN2DGE9"
            ),
            "omperor/reply-with-exactly-account-pin-dge9"
        );
        assert_eq!(
            default_fork_branch_name("   ", "ABCD"),
            "omperor/session-abcd"
        );
        let long = default_fork_branch_name(&"word ".repeat(20), "XYZ1");
        assert!(long.len() <= "omperor/".len() + 32 + 5, "{long}");
        assert!(!long.contains("--") && !long.ends_with('-'));
    }

    #[test]
    fn project_names_come_from_folders_and_stay_unique() {
        let existing = vec![
            project("a", "omperor", &["/work/omperor"], false),
            project("b", "omperor (other)", &["/other/omperor"], false),
        ];
        assert_eq!(
            unique_project_name(std::path::Path::new("/tmp/fresh"), &existing),
            "fresh"
        );
        assert_eq!(
            unique_project_name(std::path::Path::new("/third/omperor"), &existing),
            "omperor (third)"
        );
        assert_eq!(
            unique_project_name(std::path::Path::new("/other/omperor"), &existing),
            "omperor 2"
        );
        assert_eq!(
            unique_project_name(std::path::Path::new("/"), &existing),
            "/"
        );
    }

    fn account(
        id: &str,
        provider: AuthProvider,
        label: Option<&str>,
        is_default: bool,
        state: AccountState,
    ) -> AccountEntry {
        AccountEntry {
            summary: AccountSummary {
                id: id.into(),
                provider,
                principal: None,
                state,
            },
            label: label.map(str::to_owned),
            authenticated_at_ms: 0,
            is_default,
        }
    }

    #[test]
    fn account_resolution_prefers_workspace_memory_then_session_pin_then_default() {
        let accounts = vec![
            account(
                "anthropic:a",
                AuthProvider::Anthropic,
                Some("Personal"),
                false,
                AccountState::Active,
            ),
            account(
                "anthropic:b",
                AuthProvider::Anthropic,
                Some("Work"),
                true,
                AccountState::Active,
            ),
            account(
                "openai-codex:1",
                AuthProvider::OpenAi,
                None,
                false,
                AccountState::Active,
            ),
        ];
        let resolve = |project: Option<&str>, current: Option<&str>| {
            resolve_provider_account(&accounts, AuthProvider::Anthropic, project, current)
        };
        assert_eq!(
            resolve(Some("anthropic:a"), Some("anthropic:b")),
            AccountResolution::Chosen("anthropic:a".into())
        );
        assert_eq!(
            resolve(None, Some("anthropic:a")),
            AccountResolution::Chosen("anthropic:a".into())
        );
        assert_eq!(
            resolve(None, None),
            AccountResolution::Chosen("anthropic:b".into())
        );
        // A remembered account OMP no longer stores falls through to the pin.
        assert_eq!(
            resolve(Some("anthropic:missing"), Some("anthropic:a")),
            AccountResolution::Chosen("anthropic:a".into())
        );
        // A provider with exactly one usable account never asks.
        assert_eq!(
            resolve_provider_account(&accounts, AuthProvider::OpenAi, None, None),
            AccountResolution::Chosen("openai-codex:1".into())
        );
    }

    #[test]
    fn account_resolution_asks_when_nothing_singles_out_an_account() {
        let accounts = vec![
            account(
                "anthropic:a",
                AuthProvider::Anthropic,
                Some("Personal"),
                false,
                AccountState::Active,
            ),
            account(
                "anthropic:b",
                AuthProvider::Anthropic,
                Some("Work"),
                false,
                AccountState::RefreshRequired,
            ),
            account(
                "anthropic:c",
                AuthProvider::Anthropic,
                Some("Old"),
                true,
                AccountState::Disabled,
            ),
        ];
        // The disabled default cannot be pinned, so it does not decide.
        assert_eq!(
            resolve_provider_account(&accounts, AuthProvider::Anthropic, None, None),
            AccountResolution::NeedsChoice
        );
        assert_eq!(
            resolve_provider_account(
                &accounts,
                AuthProvider::Anthropic,
                Some("anthropic:c"),
                None
            ),
            AccountResolution::NeedsChoice
        );
        assert_eq!(
            resolve_provider_account(&accounts, AuthProvider::OpenAi, None, None),
            AccountResolution::Unavailable
        );
    }



    #[test]
    fn the_metrics_page_splits_a_turn_and_names_the_scope() {
        use super::{duration_text, metrics_page_plan};
        assert_eq!(duration_text(820), "820 ms");
        assert_eq!(duration_text(4_230), "4.2 s");
        assert_eq!(duration_text(72_000), "1m 12s");
        assert_eq!(duration_text(7_500_000), "2h 05m");

        let empty = metrics_page_plan(&crate::metrics::Summary::default(), "Fix it", true, false);
        assert!(empty.cards.is_empty());
        assert!(empty.intro.contains("Nothing recorded for this session"));
        let empty = metrics_page_plan(
            &crate::metrics::Summary::default(),
            "All sessions",
            false,
            true,
        );
        assert_eq!(empty.intro, "Reading the log…");

        let summary = crate::metrics::Summary {
            turns: 2,
            responses: 3,
            tool_calls: 4,
            split: crate::metrics::TurnSplit {
                wall_ms: 40_000,
                first_token_ms: 10_000,
                receiving_ms: 20_000,
                tools_ms: 6_000,
                harness_ms: 4_000,
            },
            models: vec![crate::metrics::ModelStats {
                provider: "openai-codex".into(),
                model: "openai-codex/gpt-5.6-sol".into(),
                responses: 3,
                tokens_out: 900,
                median_tps: Some(88.4),
                p90_tps: Some(120.0),
                median_ttft_ms: Some(1_200),
                fast_responses: 1,
            }],
            tools: vec![crate::metrics::ToolStats {
                name: "bash".into(),
                calls: 4,
                total_ms: 6_000,
                median_ms: 1_500,
                errors: 0,
            }],
            from_ms: Some(1),
            to_ms: Some(2),
        };
        let plan = metrics_page_plan(&summary, "All sessions", false, false);
        assert_eq!(
            plan.intro,
            "All sessions · 2 turns · 3 responses · 4 tool calls"
        );

        // The list: every session pinned under "All sessions", newest first,
        // titled by the sidebar when it knows the session.
        use super::{metrics_session_rows, short_session_id};
        let activity = vec![
            crate::metrics::SessionActivity {
                id: "01M2J8VJX2PHZF6JWM5Z1D43XD".into(),
                turns: 2,
                latest_ms: 2_000,
            },
            crate::metrics::SessionActivity {
                id: "01M2OLD".into(),
                turns: 1,
                latest_ms: 1_000,
            },
        ];
        let summaries = vec![crate::session::SessionSummary::for_test(
            "01M2J8VJX2PHZF6JWM5Z1D43XD",
            "Use the read tool on Cargo.toml",
            Some("/Users/example/work/omperor"),
            true,
        )];
        let rows = metrics_session_rows(&activity, &summaries);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].session, None);
        assert_eq!(rows[0].detail, "2 sessions · 3 turns");
        assert_eq!(rows[1].title, "Use the read tool on Cargo.toml");
        assert!(
            rows[1].detail.starts_with("omperor · 2 turns · "),
            "{}",
            rows[1].detail
        );
        assert_eq!(rows[2].title, "Session 01M2OLD");
        assert_eq!(short_session_id("01M2J8VJX2PHZF6JWM5Z1D43XD"), "5Z1D43XD");
        let long = metrics_page_plan(
            &summary,
            "I just want to test token speed, without using too many, just do some thinking",
            true,
            false,
        );
        assert_eq!(
            long.intro,
            "I just want to test token speed, without… · 2 turns · 3 responses · 4 tool calls"
        );
        assert_eq!(super::shortened("short", 44), "short");
        assert_eq!(plan.cards.len(), 3);
        let time = &plan.cards[0];
        let shares = time.bar.as_ref().expect("a stacked bar");
        assert!((shares.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert_eq!(time.rows[0][1], "10.0 s");
        assert_eq!(time.rows[0][2], "25%");
        assert_eq!(time.rows[3][0], "Harness and everything else");
        let models = &plan.cards[1];
        assert_eq!(models.rows[0][0], "gpt-5.6-sol · openai-codex");
        assert_eq!(models.rows[0][2], "88");
        assert_eq!(models.rows[0][4], "1.2 s");
        assert_eq!(models.rows[0][5], "1/3");
        assert_eq!(plan.cards[2].rows[0][2], "6.0 s");
        assert!(time.height() > 4.0 * 24.0);
    }

    #[test]
    fn an_account_card_names_its_plan_and_groups_its_windows() {
        use super::{
            PlanTone, account_card_plan, plan_display_name, usage_group_title, usage_row_title,
        };
        use crate::omp_runtime::{AccountUsage, UsageWindow};
        let window = |label: &str, scope: &str, used: u8| UsageWindow {
            label: label.into(),
            scope: Some(scope.into()),
            used_percent: Some(used),
            resets_at_ms: None,
            spend: None,
        };
        let usage = AccountUsage {
            account_id: "openai-codex:a".into(),
            plan: Some("pro".into()),
            windows: vec![
                window("5 hours (Spark)", "spark", 0),
                window("7 days", "shared", 10),
                window("7 days (Spark)", "spark", 93),
            ],
            error: None,
        };
        let card = account_card_plan(AuthProvider::OpenAi, Some(&usage), false, true, true);
        assert_eq!(card.plan, "ChatGPT Pro");
        assert_eq!(card.tone, PlanTone::Warning, "a nearly spent window warns");
        // The account's own windows lead; a feature's follow under its name.
        let titles = card
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titles, ["General usage limits", "Spark usage limits"]);
        assert_eq!(card.groups[0].rows[0].title, "Weekly limit");
        assert_eq!(card.groups[0].rows[0].left_percent, Some(90));
        assert_eq!(card.groups[1].rows[0].title, "5-hour limit");
        assert_eq!(card.groups[1].rows[1].left_percent, Some(7));
        assert!(card.height() > card.groups.len() as f64 * 44.0);
        assert_eq!(card.shown_groups().len(), 2, "unfolded, every group shows");

        // Folded, only the window that runs out first shows, named after
        // its feature since the heading that would say so is gone.
        let folded = account_card_plan(AuthProvider::OpenAi, Some(&usage), false, true, false);
        let shown = folded.shown_groups();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title, "Usage");
        assert_eq!(shown[0].rows.len(), 1);
        assert_eq!(shown[0].rows[0].title, "Weekly limit (Spark)");
        assert_eq!(shown[0].rows[0].left_percent, Some(7));
        assert!(folded.height() < card.height());

        // One window has nothing to fold away: it shows under its own
        // heading and no toggle is offered.
        let single = AccountUsage {
            account_id: "openai-codex:s".into(),
            plan: Some("free".into()),
            windows: vec![window("30 days", "shared", 1)],
            error: None,
        };
        let card = account_card_plan(AuthProvider::OpenAi, Some(&single), false, true, false);
        assert!(!card.foldable());
        assert_eq!(card.shown_groups()[0].title, "General usage limits");
        assert_eq!(card.shown_groups()[0].rows[0].title, "Monthly limit");

        let pending = account_card_plan(AuthProvider::OpenAi, None, true, true, false);
        assert_eq!(pending.plan, "Checking with the provider…");
        assert!(pending.groups.is_empty());
        assert!(pending.summary.is_none());
        let signed_out = account_card_plan(AuthProvider::OpenAi, None, false, false, false);
        assert_eq!(signed_out.plan, "Unknown until the account is signed in");

        let failed = AccountUsage {
            account_id: "anthropic:b".into(),
            plan: None,
            windows: Vec::new(),
            error: Some("protocol violation (console_usage_credential_unavailable)".into()),
        };
        let card = account_card_plan(AuthProvider::Anthropic, Some(&failed), false, true, false);
        assert_eq!(card.plan, "Unavailable");
        assert!(
            card.note
                .as_deref()
                .is_some_and(|note| note.contains("no credential"))
        );

        assert_eq!(usage_row_title("30 days"), "Monthly limit");
        assert_eq!(usage_row_title("1 day"), "Daily limit");
        assert_eq!(usage_row_title("12 hours (Spark)"), "12-hour limit");
        assert_eq!(usage_group_title(None, "7 days"), "General usage limits");
        assert_eq!(
            usage_group_title(Some("spark"), "7 days (Spark)"),
            "Spark usage limits"
        );
        assert_eq!(
            plan_display_name(AuthProvider::OpenAi, Some("team")),
            "ChatGPT Team"
        );
        assert_eq!(
            plan_display_name(AuthProvider::OpenAi, Some("startup")),
            "ChatGPT Startup"
        );
        assert_eq!(
            plan_display_name(AuthProvider::OpenAi, None),
            "ChatGPT plan not reported"
        );
        // A Claude account: the provider's name leads each window label, a
        // model-scoped week gets its own group, and extra usage is money.
        let claude = AccountUsage {
            account_id: "anthropic:c".into(),
            plan: Some("max_20x".into()),
            windows: vec![
                window("Claude 7 Day (Fable)", "fable", 0),
                window("Claude 5 Hour", "shared", 1),
                window("Claude 7 Day", "shared", 3),
                UsageWindow {
                    label: "Claude Extra Usage".into(),
                    scope: Some("extra".into()),
                    used_percent: Some(25),
                    resets_at_ms: None,
                    spend: Some(crate::omp_runtime::Spend {
                        used_cents: 100_25,
                        limit_cents: Some(400_00),
                    }),
                },
            ],
            error: None,
        };
        let card = account_card_plan(AuthProvider::Anthropic, Some(&claude), false, true, true);
        assert_eq!(card.plan, "Claude Max 20×");
        let titles = card
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            ["General usage limits", "Fable usage limits", "Extra usage"]
        );
        assert_eq!(card.groups[0].rows[0].title, "5-hour limit");
        assert_eq!(card.groups[0].rows[1].title, "Weekly limit");
        assert_eq!(card.groups[1].rows[0].title, "Weekly limit");
        let spend = &card.groups[2].rows[0];
        assert_eq!(spend.title, "Monthly spending limit");
        assert_eq!(spend.detail.as_deref(), Some("$100.25 of $400.00 spent"));
        assert_eq!(spend.figure.as_deref(), Some("$299.75 left"));
        assert_eq!(spend.left_percent, Some(75));
        assert_eq!(super::dollars(1_234_567), "$12,345.67");
        // Money is a cap, not a window: the folded card still leads with
        // the tightest rate window, the general week at 3 % used.
        let folded = account_card_plan(AuthProvider::Anthropic, Some(&claude), false, true, false);
        let summary = folded.summary.as_ref().expect("a summary");
        assert_eq!(summary.rows[0].title, "Weekly limit");
        assert_eq!(summary.rows[0].left_percent, Some(97));
        assert_eq!(
            plan_display_name(AuthProvider::Anthropic, Some("pro")),
            "Claude Pro"
        );
        assert_eq!(
            plan_display_name(AuthProvider::Anthropic, None),
            "Claude plan not reported"
        );
    }

    #[test]
    fn turn_error_reasons_name_accounts_the_user_knows() {
        let entry = |id: &str, label: Option<&str>, principal: Option<&str>| AccountEntry {
            label: label.map(str::to_owned),
            authenticated_at_ms: 0,
            is_default: false,
            summary: crate::omp_runtime::AccountSummary {
                id: id.to_owned(),
                provider: AuthProvider::OpenAi,
                principal: principal.map(str::to_owned),
                state: crate::omp_runtime::AccountState::Active,
            },
        };
        let accounts = vec![
            entry("openai-codex:work", Some("Work account"), None),
            entry("anthropic:cma@example.test", None, Some("cma@example.test")),
            entry("openai-codex:opaque", None, Some("0a1b2c3d")),
        ];
        assert_eq!(
            name_accounts_in(
                "no credential is stored for openai-codex account openai-codex:work; sign in again",
                &accounts
            ),
            "no credential is stored for openai-codex account “Work account”; sign in again"
        );
        assert_eq!(
            name_accounts_in("anthropic:cma@example.test is rate limited", &accounts),
            "“cma@example.test” is rate limited"
        );
        assert_eq!(
            name_accounts_in("openai-codex:opaque failed", &accounts),
            "openai-codex:opaque failed",
            "an account with neither label nor email keeps its identifier"
        );
    }

    #[test]
    fn account_labels_must_be_present_and_distinct_ignoring_case() {
        let existing = vec!["Work".to_owned()];
        assert!(labels_are_valid(&["Personal".into()], &existing));
        assert!(!labels_are_valid(&["".into()], &existing));
        assert!(!labels_are_valid(&["   ".into()], &existing));
        assert!(!labels_are_valid(&["work".into()], &existing));
        assert!(!labels_are_valid(&["Home".into(), "home ".into()], &[]));
        assert!(labels_are_valid(
            &["Home".into(), "Office".into()],
            &existing
        ));
    }

    #[test]
    fn model_picker_only_includes_supported_models_for_connected_accounts() {
        let mut models = (0..300)
            .map(|index| {
                if index == 250 {
                    "openai-codex/gpt-current".to_owned()
                } else {
                    format!("provider/model-{index}")
                }
            })
            .collect::<Vec<_>>();
        models.extend([
            "anthropic/claude-opus-5".to_owned(),
            "anthropic/claude-fable-5-1".to_owned(),
        ]);

        let authenticated = HashSet::from([AuthProvider::OpenAi, AuthProvider::Anthropic]);
        let options = model_picker_options(&models, "provider/model-42", &authenticated);

        assert_eq!(
            options,
            [
                "openai-codex/gpt-current",
                "anthropic/claude-opus-5",
                "anthropic/claude-fable-5-1",
            ]
        );
    }

    #[test]
    fn model_picker_uses_only_authenticated_runtime_rows() {
        let models = [
            "openai-codex/gpt-5.6-luna",
            "anthropic/claude-fable-5",
            "anthropic/claude-fable-5-1",
            "anthropic/claude-opus-5",
            "cursor/default",
            "aiand/deepseek-ai/deepseek-v4-pro",
            "aimlapi/alibaba/wan2.2-14b-animate-move",
        ]
        .map(str::to_owned);
        let authenticated = HashSet::from([AuthProvider::Anthropic]);

        let options = model_picker_options(&models, "anthropic/claude-opus-5", &authenticated);

        assert_eq!(
            options,
            [
                "anthropic/claude-opus-5",
                "anthropic/claude-fable-5",
                "anthropic/claude-fable-5-1",
            ]
        );
    }

    #[test]
    fn active_model_is_hidden_while_its_account_is_disconnected() {
        let models = [
            "openai-codex/gpt-5.6-luna".to_owned(),
            "anthropic/claude-sonnet-4-6".to_owned(),
        ];

        let options = model_picker_options(&models, "openai-codex/gpt-5.6-luna", &HashSet::new());

        assert!(options.is_empty());
    }

    #[test]
    fn changing_subscription_provider_starts_a_fresh_backend_session() {
        assert!(model_change_requires_new_session(
            "openai-codex/gpt-5.6-luna",
            "anthropic/claude-opus-4-8"
        ));
        assert!(model_change_requires_new_session(
            "anthropic/claude-opus-4-8",
            "openrouter/anthropic/claude-opus-4-8"
        ));
        assert!(!model_change_requires_new_session(
            "anthropic/claude-opus-4-8",
            "anthropic/claude-sonnet-4-6"
        ));
        assert!(!model_change_requires_new_session(
            "openai-codex/gpt-5.6-luna",
            "openai-codex/gpt-5.6-sol"
        ));
    }



    #[test]
    fn composer_waits_for_authentication_even_when_rpc_is_starting_or_ready() {
        assert!(!prompt_submission_enabled(false, false, true, false));
        assert!(!prompt_submission_enabled(false, true, false, false));
        assert!(prompt_submission_enabled(true, true, false, false));
        assert!(prompt_submission_enabled(true, false, true, true));
    }

    #[test]
    fn session_source_tabs_partition_native_and_external_sessions() {
        assert!(SessionSourceFilter::Omperor.includes(true));
        assert!(!SessionSourceFilter::Omperor.includes(false));
        assert!(!SessionSourceFilter::Omp.includes(true));
        assert!(SessionSourceFilter::Omp.includes(false));
        assert_eq!(
            SessionSourceFilter::from_segment(0),
            SessionSourceFilter::Omperor
        );
        assert_eq!(
            SessionSourceFilter::from_segment(1),
            SessionSourceFilter::Omp
        );
        assert_eq!(
            SessionSourceFilter::from_segment(99),
            SessionSourceFilter::Omperor
        );
    }

    #[test]
    fn file_changing_tools_refresh_the_environment_but_read_only_tools_do_not() {
        for tool in [
            "write",
            "edit",
            "ast_edit",
            "bash",
            "task",
            "checkpoint",
            "rewind",
        ] {
            assert!(tool_may_change_files(Some(tool)), "{tool} can change files");
        }
        for tool in ["read", "grep", "glob", "think", "todo", "web_search"] {
            assert!(!tool_may_change_files(Some(tool)), "{tool} is read-only");
        }
        // An unnamed or unknown tool errs toward refreshing.
        assert!(tool_may_change_files(None));
        assert!(tool_may_change_files(Some("some_future_tool")));
    }
}
