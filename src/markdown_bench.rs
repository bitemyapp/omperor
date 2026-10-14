//! Headless Markdown benchmark and conformance dump: `--markdown-bench
//! <out.json>` and `--markdown-dump <out-dir> [inputs.md …]`; and the
//! presentation review's full-window captures, `--screenshot <out-dir>`.
//!
//! Both drive the real main window through its public behaviour only — a
//! new conversation, a prompt sent to the mock backend, the 30 Hz tick,
//! selecting a conversation, the window's frame, the transcript's scroll
//! view — so a build whose transcript differs inside is measured by the
//! same harness. The window never reaches a screen: the process is never
//! activated, never in the Dock, and its window sits far off every display,
//! never ordered in (checked on every step; a violation ends the run).
//!
//! Drawing: Core Animation's commit still draws what the app invalidates in
//! a layer-backed view of this window — in a pass of its own, the dirty rect
//! only, as on screen — and the harness names those draws
//! `gui.bench.appkit_draw`. It does not draw what the harness exposes by
//! scrolling, resizing, or switching conversations; the harness draws those
//! itself, in the same pass, inside `gui.bench.draw`. See
//! docs/MARKDOWN_CONFORMANCE.md.
//!
//! The hooks the app carries (all no-ops unless a mode is active):
//! [`prepare`] first thing in `main`; [`hide_application`] right after the
//! shared application exists; [`present`] instead of showing the main
//! window; [`launched`] from `applicationDidFinishLaunching:`; and
//! [`after_tick`] at the end of the runtime tick. The app implements
//! [`Host`]; only [`Host::displayed_answer`] knows the renderer.
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ffi::CStr,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{OnceLock, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use block2::RcBlock;
use objc2::{
    AnyThread, ClassType, MainThreadMarker, Message, msg_send,
    rc::Retained,
    runtime::{AnyObject, Imp, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBitmapImageRep, NSDeviceRGBColorSpace,
    NSRunningApplication, NSScreen, NSScrollView, NSView, NSWindow, NSWindowOcclusionState,
    NSWorkspace,
};
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer, kCFRunLoopCommonModes,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};
use serde_json::{Map, Value, json};

use crate::{frame_budget, mock_inference, perf, preferences};

/// The main window's size in every run: the renderer is compared at one
/// measure.
const WINDOW_WIDTH: f64 = 1440.0;
const WINDOW_HEIGHT: f64 = 900.0;
/// Far beyond any display arrangement; checked against every screen anyway.
const OFFSCREEN: NSPoint = NSPoint::new(-30_000.0, -30_000.0);
/// Bitmap pixels per point, fixed so runs agree whatever the displays are.
const SCALE: f64 = 2.0;
/// The transcript counts as settled after this long without a change —
/// longer than two of the app's idle ticks, so late work has landed.
const SETTLE: Duration = Duration::from_millis(1_200);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const STAGE_TIMEOUT: Duration = Duration::from_secs(150);
const RUN_TIMEOUT: Duration = Duration::from_secs(360);
const REALISTIC_PROMPT: &str = "/mock scenario=markdown rate=240";
const HEAVY_PROMPT: &str = "/mock scenario=markdown template=stress size=200000 rate=4000";
/// Window widths the resize phase steps through, from the full width.
const RESIZE_WIDTHS: [f64; 5] = [1320.0, 1200.0, 1080.0, 1260.0, 1440.0];
const RESIZE_MIN_STEP: Duration = Duration::from_millis(600);
/// The scroll phase moves this far a step, one step a 60 Hz frame.
const SCROLL_STEP_PT: f64 = 400.0;
const SCROLL_INTERVAL: f64 = 1.0 / 60.0;
const MAX_SCROLL_STEPS: usize = 2_000;
/// The conformance dump's captures are this far apart; two identical in a
/// row settle it.
const CAPTURE_INTERVAL: f64 = 0.6;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(12);
/// The transcript's canvas (theme::canvas), under a capture's transparency.
const CANVAS_LIGHT: [u8; 3] = [252, 249, 244];
const CANVAS_DARK: [u8; 3] = [22, 17, 26];
/// Chat styles by the label Settings shows, and the name a dump file carries.
const STYLES: [(&str, &str); 3] = [("Modern", "modern"), ("Editorial", "editorial"), ("Terminal", "terminal")];
/// The relative image the stress document shows, generated into the project.
const STRESS_IMAGE: &str = "markdown-stress.png";
/// Where dump inputs are copied, inside the project the mock reads `file=` from.
const INPUT_DIRECTORY: &str = "markdown-inputs";
const DEFAULT_INPUTS: [(&str, &str); 7] = [
    ("chat-answer", include_str!("../tests/markdown-conformance/chat-answer.md")),
    ("chat-review", include_str!("../tests/markdown-conformance/chat-review.md")),
    ("chat-explainer", include_str!("../tests/markdown-conformance/chat-explainer.md")),
    ("chat-edge", include_str!("../tests/markdown-conformance/chat-edge.md")),
    // Longer than one view (`transcript_parse::WHOLE_LIMIT`): set as segments.
    ("chat-long", CHAT_LONG),
    // Downright's own status page, as Upleft's corpus copies it.
    ("corpus-status", include_str!("../vendor/upleft/vendor/downright/Docs/STATUS.md")),
    // Upleft's generated agent document at 400 lines (`agent_document` in
    // vendor/upleft/scripts/build-corpus.py), kept here since the generated
    // corpus is not part of a checkout.
    ("corpus-agent", include_str!("../tests/markdown-conformance/corpus-agent-400.md")),
];
const CHAT_LONG: &str = include_str!("../tests/markdown-conformance/chat-long.md");

/// The dump's inputs when none are named: the stress document once and
/// twice over, the chat samples, and a long answer that stops inside its
/// diagram, as a stream cut off by Stop or an error does.
fn default_inputs() -> Vec<(String, String)> {
    let mut inputs = vec![("markdown-stress".to_owned(), mock_inference::STRESS_DOCUMENT.to_owned())];
    inputs.extend(DEFAULT_INPUTS.iter().map(|(name, text)| ((*name).to_owned(), (*text).to_owned())));
    inputs.push(("markdown-stress-2x".to_owned(), format!("{0}\n\n{0}", mock_inference::STRESS_DOCUMENT)));
    let stop = CHAT_LONG.find("  C --> W").unwrap_or(CHAT_LONG.len());
    inputs.push(("chat-long-stopped".to_owned(), CHAT_LONG[..stop].to_owned()));
    inputs
}

/// The stability phases, in order; every pass of each is looked at.
const STABILITY_PHASES: [&str; 7] = [
    "history-open",
    "history-middle",
    "scroll-up-normal",
    "scroll-up-fast",
    "stream-follow",
    "stream-scrolled",
    "width-history",
];
/// The two long sessions: turns, and the seed of what they say.
const HISTORY_OPEN: (usize, u64) = (70, 11);
const HISTORY_MIDDLE: (usize, u64) = (110, 29);
/// The middle phase scrolls once this many screens of history are in.
const MIDDLE_SCREENS: f64 = 4.0;
/// A streamed answer grows by about this many bytes a 30 Hz tick.
const STREAM_CHUNK: usize = 48;
/// Window widths the width phase steps through, scrolled into the history.
const HISTORY_WIDTHS: [f64; 4] = [1080.0, 900.0, 1260.0, 1440.0];
/// A pass whose reference content moved by more than this has drifted.
const DRIFT_EPSILON: f64 = 0.5;
/// Frame pairs written a phase, around drift or pop-in, when asked for.
const FRAME_EVENTS: usize = 6;

/// Which phases run: `OMPEROR_MARKDOWN_BENCH_PHASES` is `timing` (the
/// original five), `stability`, or `all` (the default).
fn phases_wanted() -> (bool, bool) {
    match std::env::var("OMPEROR_MARKDOWN_BENCH_PHASES").as_deref() {
        Ok("timing") => (true, false),
        Ok("stability") => (false, true),
        _ => (true, true),
    }
}

/// Where frame pairs around drift and pop-in go, when asked for.
fn frames_directory() -> Option<PathBuf> {
    std::env::var_os("OMPEROR_MARKDOWN_BENCH_FRAMES").filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// A long session as a person's might read: turns of a question, often
/// reasoning, runs of tool calls, sometimes a first reply between them, and
/// an answer — short, a list with code, one of the chat samples, a diagram,
/// display math, an image, a long answer cut into segments, now and then
/// the stress document. Diagrams and formulas differ from turn to turn, so
/// each renders afresh; tool rows repeat, as they do.
fn long_session(turns: usize, seed: u64) -> Vec<crate::session::SessionMessage> {
    use crate::session::{SessionMessage, Speaker};
    const TOOLS: [(&str, &str); 6] = [
        ("read", "Read src/transcript_stack.rs"),
        ("bash", "Ran cargo test --release"),
        ("grep", "Searched anchor in src"),
        ("edit", "Edited src/chat.rs"),
        ("find", "Found 12 files matching *.md"),
        ("bash", "Ran git status --short"),
    ];
    const PROSE: [&str; 6] = [
        "The reader's place is the content under the top of the viewport, and it keeps its position on screen whatever changes around it.",
        "Heights that were estimated are corrected in the same pass that learns them, before anything is drawn, so a correction never shows.",
        "History arrives in pages, newest first, and every page goes above the rows already shown; the view moves with the rows, not the other way round.",
        "A diagram renders on a worker and lands later; its size is reserved when it is known, and the anchor absorbs the rest.",
        "Views are made a screen or two ahead in the direction the reader is going, within the time a frame allows.",
        "Streaming output grows the last message; a reader at the end follows it, and a reader further up sees nothing move.",
    ];
    let samples = [
        include_str!("../tests/markdown-conformance/chat-answer.md"),
        include_str!("../tests/markdown-conformance/chat-review.md"),
        include_str!("../tests/markdown-conformance/chat-explainer.md"),
        include_str!("../tests/markdown-conformance/chat-edge.md"),
    ];
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut next = move |bound: usize| {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as usize % bound.max(1)
    };
    let prose = |at: usize| PROSE[at % PROSE.len()];
    let mut messages = Vec::new();
    let tools = |messages: &mut Vec<SessionMessage>, count: usize, next: &mut dyn FnMut(usize) -> usize| {
        for _ in 0..count {
            let (name, summary) = TOOLS[next(TOOLS.len())];
            let detail = format!("{summary}\n\n{}", "output line\n".repeat(1 + next(6)));
            messages.push(SessionMessage::tool(name, summary, Some(detail), next(20) == 0));
        }
    };
    for turn in 0..turns {
        let question = match next(4) {
            0 => format!("Why does the transcript move when history loads? (turn {turn})"),
            1 => format!(
                "Look at this and tell me what is wrong, turn {turn}:\n\n```rust\nfn restack(&self) {{\n    let y = self.anchor();\n    self.scroll_to(y + {turn}.0);\n}}\n```\n\n{}",
                prose(turn)
            ),
            2 => "continue".to_owned(),
            _ => format!("{} Can you check it for turn {turn}?\n\n{}", prose(turn + 1), prose(turn + 2)),
        };
        messages.push(SessionMessage::spoken(Speaker::User, question));
        if next(10) < 6 {
            let sentences = 2 + next(4);
            let thinking = (0..sentences).map(|at| prose(turn + at)).collect::<Vec<_>>().join(" ");
            messages.push(SessionMessage::spoken(Speaker::Thinking, thinking));
        }
        let count = next(5);
        tools(&mut messages, count, &mut next);
        if next(10) < 3 {
            messages.push(SessionMessage::spoken(Speaker::Assistant, format!("Let me look further. {}", prose(turn + 3))));
            let count = 1 + next(4);
            tools(&mut messages, count, &mut next);
        }
        let answer = match next(12) {
            0 | 1 => prose(turn + 4).to_owned(),
            2 => format!("{}\n\n{}", prose(turn), prose(turn + 1)),
            3 => format!(
                "Three things changed in turn {turn}:\n\n- The anchor is a line, not a row.\n- Views are made ahead of the reader.\n- Sizes are reserved.\n\n```rust\nlet place = transcript.place();\nassert_eq!(place.offset, {turn}.0);\n```\n\n{}",
                prose(turn + 2)
            ),
            4 | 5 => format!("{}\n\nTurn {turn}.", samples[next(samples.len())]),
            6 | 7 => format!(
                "Here is the flow for turn {turn}:\n\n```mermaid\nflowchart TD\n  A[Page {turn} arrives] --> B{{Reader at the end?}}\n  B -->|yes| C[Follow the end]\n  B -->|no| D[Keep line {} in place]\n  D --> E[Restack {} rows]\n  C --> E\n```\n\n{}",
                turn * 7 % 97,
                turn % 13 + 2,
                prose(turn + 5)
            ),
            8 => format!(
                "The drift after {turn} pages is bounded:\n\n$$\n\\sum_{{k=1}}^{{{}}} \\delta_k = 0, \\quad |\\delta_k| < {}\n$$\n\nInline, $h_{{{turn}}} = \\hat h_{{{turn}}} + \\epsilon$ holds for every row.\n\n{}",
                turn + 3,
                turn % 7 + 1,
                prose(turn + 1)
            ),
            9 => format!("{}\n\n![The crowned gate]({STRESS_IMAGE})\n\nThe image above is turn {turn}'s.", prose(turn + 2)),
            10 => format!("## Turn {turn}\n\n{CHAT_LONG}"),
            _ if next(3) == 0 => format!("# Stress, turn {turn}\n\n{}", mock_inference::STRESS_DOCUMENT),
            _ => (0..6 + next(10))
                .map(|at| format!("### Part {at}\n\n{}\n\n{}", prose(turn + at), prose(turn + at + 3)))
                .collect::<Vec<_>>()
                .join("\n\n"),
        };
        messages.push(SessionMessage::spoken(Speaker::Assistant, answer));
    }
    messages
}

/// The answer the stream phases stream: long, with a diagram and math.
fn stream_answer(name: &str) -> String {
    format!("## Streamed for {name}\n\n{CHAT_LONG}\n\n```mermaid\nflowchart LR\n  S[{name}] --> T[Tail] --> U[Seal]\n```\n\nDone.")
}

/// Appends of a streamed replay: 1 to this many characters, as tokens are.
const REPLAY_CHUNK: usize = 6;
/// Samples of a replay besides those at seals and completed constructs.
const REPLAY_SAMPLES: usize = 24;
/// At most this many equivalence failures are written out as images.
const MISMATCH_IMAGES: usize = 60;

/// What the app lets the harness do. Everything but `displayed_answer` is
/// the window's public behaviour; that one reads the renderer's output.
pub(crate) trait Host {
    fn window(&self) -> Option<Retained<NSWindow>>;
    /// The transcript's scroll view: what is drawn, scrolled, and captured.
    fn transcript_scroll_view(&self) -> Option<Retained<NSScrollView>>;
    /// Startup is done and the selected conversation, on the mock backend,
    /// can take a prompt.
    fn ready(&self) -> bool;
    /// Opens a new conversation in the current project and selects it, as
    /// New Chat does; returns its key.
    fn new_conversation(&self) -> u64;
    /// Selects a conversation, as its sidebar row does.
    fn select_conversation(&self, key: u64);
    /// Types `prompt` into the composer and sends it, as Return does.
    fn send_prompt(&self, prompt: &str);
    /// The selected conversation's turn is queued or running.
    fn turn_running(&self) -> bool;
    /// Sets the chat style (by its Settings label) and a light or dark
    /// appearance, without storing them, and re-renders.
    fn set_look(&self, style: &str, dark: bool);
    /// Types `text` into the composer as the user would (empty clears it),
    /// so a leading `/` opens the command list.
    fn open_slash_list(&self, text: &str);
    /// Gives the current session made-up accounts and usage (screenshots);
    /// with `in_flight`, a reading stays on its way so the spinner shows.
    fn inject_synthetic_accounts(&self, in_flight: bool);
    /// The selected conversation's last assistant message as displayed.
    fn displayed_answer(&self) -> Option<Displayed>;
    /// The equivalence check, in steps (`Transcript::begin_equivalence`):
    /// the last answer made whole; what is shown drawn over its text; one
    /// view of its text shown in place of its stack; the stack back.
    fn begin_equivalence(&self) -> bool;
    fn capture_equivalence(&self, capture: &mut dyn FnMut(&NSView, NSRect));
    fn swap_equivalence(&self) -> Option<crate::transcript_stack::Equivalence>;
    fn end_equivalence(&self);
    /// Streams an answer into the selected conversation as its events would,
    /// without the backend: a turn with an empty answer, appends, the end.
    fn replay_begin(&self);
    fn replay_append(&self, delta: &str);
    fn replay_finish(&self);
    fn replay_state(&self) -> Option<crate::transcript_stack::ReplayState>;
    /// Sets the selected conversation again as if opened from history,
    /// every parse forgotten.
    fn reload_answer(&self);
    /// Opens a stored session made of `messages`, as its sidebar row does:
    /// its history streams in pages, newest first, through the app's own
    /// loading path, and older pages are put above the rows shown.
    fn open_history(&self, name: &str, messages: Vec<crate::session::SessionMessage>);
    /// A session's history is still streaming in.
    fn history_loading(&self) -> bool;
    /// The content at the viewport's top edge (with `bottom`, its bottom
    /// edge): a name the renderer can find it by again, and its y in the
    /// viewport. The stability phases watch it pass by pass.
    fn probe(&self, bottom: bool) -> Option<(String, f64)>;
    /// Where the content `probe` named is now, in the viewport.
    fn locate(&self, name: &str) -> Option<f64>;
    /// What the viewport shows unfinished: rows not made (unparsed, no
    /// view, or laid out at another width), and diagrams, formulas or
    /// images drawn as placeholders.
    fn viewport_gaps(&self) -> (usize, usize);
    /// What the transcript holds: its rows, and the bytes of their text. A
    /// pass that changes it changed the content (a page of history, a
    /// streamed delta), not only how the content is measured.
    fn content_extent(&self) -> (usize, usize);
    /// Opens the session `<id>.oms` in the project, read from its journal
    /// as a session is when selected.
    fn open_journal(&self, id: &str);
    /// How the transcript's row views disagree with its rows.
    fn layout_violations(&self) -> Vec<String>;
    /// Asks every view in sight to draw all of itself again.
    fn invalidate_visible(&self);
}

/// One message as displayed: its rect in the transcript's document view,
/// and its semantic dump (docs/MARKDOWN_CONFORMANCE.md).
pub(crate) struct Displayed {
    pub(crate) rect: NSRect,
    pub(crate) semantics: Value,
}

#[derive(Debug, Clone)]
enum Mode {
    Bench { out: PathBuf },
    Dump { out_dir: PathBuf, inputs: Vec<(String, String)> },
    /// Full-window captures of a representative conversation, finished and
    /// streaming, in every chat style and appearance.
    Shots { out_dir: PathBuf },
}

/// The screenshot mode's conversation: a prompt the mock answers with
/// thinking, tools (one failing), more thinking, and the stress document.
const SHOWCASE_PROMPT: &str = "Find where the transcript sets its measure, run the tests, and show me the showcase document.";
/// The streaming capture is taken this long after the prompt is sent: the
/// tools are done and the answer is part way in.
const SHOT_STREAM_AT: Duration = Duration::from_millis(8_500);
/// `SHOT_STREAM_AT`, or `OMPEROR_SHOT_STREAM_MS` milliseconds: at 11200 the
/// showcase's first code block is part way in, its fence still open.
fn shot_stream_at() -> Duration {
    std::env::var("OMPEROR_SHOT_STREAM_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .map_or(SHOT_STREAM_AT, Duration::from_millis)
}
/// Where the finished conversation is captured: the top, and fractions of
/// the way down its scrollable height.
const SHOT_VIEWS: [(&str, f64); 6] =
    [("top", 0.0), ("callouts", 0.32), ("code", 0.36), ("diff", 0.44), ("middle", 0.55), ("end", 1.0)];

static MODE: OnceLock<Mode> = OnceLock::new();
/// `--transcript-check <session.oms> <out-dir>` (`transcript_check`): the
/// journal copied into the scratch project, and where the results go.
static CHECK: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
/// The frontmost application before the run touched anything.
static FRONTMOST_BEFORE: OnceLock<Value> = OnceLock::new();

/// Whether this process is a headless benchmark or dump.
pub(crate) fn active() -> bool {
    MODE.get().is_some() || CHECK.get().is_some()
}

/// Developer-only: `OMPEROR_SHOT_SYNTHETIC_ACCOUNTS=1` in `--screenshot`
/// mode gives the usage captures made-up accounts and usage, since the
/// scratch home has none. Never read outside the screenshot run.
pub(crate) fn synthetic_accounts() -> bool {
    matches!(MODE.get(), Some(Mode::Shots { .. }))
        && std::env::var_os("OMPEROR_SHOT_SYNTHETIC_ACCOUNTS").is_some_and(|value| value == "1")
}

/// `--transcript-check <session.oms> <out-dir>`, if asked for.
fn parse_check(args: &[String]) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let Some(index) = args.iter().position(|arg| arg == "--transcript-check") else {
        return Ok(None);
    };
    match (args.get(index + 1), args.get(index + 2)) {
        (Some(journal), Some(out)) if journal.ends_with(".oms") && !out.starts_with("--") => {
            Ok(Some((PathBuf::from(journal), PathBuf::from(out))))
        }
        _ => Err("usage: omperor --transcript-check <session.oms> <out-dir>".into()),
    }
}

/// Left in a home this harness set up. A home that already holds Omperor
/// data without it is refused, whatever the user database says — a second
/// guard in case the real-home check is ever wrong.
const SCRATCH_MARKER: &str = ".omperor-markdown-bench-home";

const USAGE: &str = "usage: omperor --markdown-bench <out.json>\n       omperor --markdown-dump <out-dir> [inputs.md …]\n       omperor --screenshot <out-dir>";

fn parse_mode(args: &[String]) -> Result<Option<Mode>, String> {
    let Some(index) = args.iter().position(|arg| arg == "--markdown-bench" || arg == "--markdown-dump" || arg == "--screenshot") else {
        return Ok(None);
    };
    let target = args
        .get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{} needs an output path\n{USAGE}", args[index]))?;
    if args[index] == "--markdown-bench" {
        return Ok(Some(Mode::Bench { out: PathBuf::from(target) }));
    }
    if args[index] == "--screenshot" {
        return Ok(Some(Mode::Shots { out_dir: PathBuf::from(target) }));
    }
    let mut inputs = Vec::new();
    for path in args[index + 2..].iter().filter(|arg| !arg.starts_with("--")) {
        let text = std::fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
        let stem = Path::new(path).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let name: String = stem
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect();
        if name.is_empty() || inputs.iter().any(|(existing, _)| *existing == name) {
            return Err(format!("{path}: inputs need distinct file names"));
        }
        inputs.push((name, text));
    }
    if inputs.is_empty() {
        inputs = default_inputs();
    }
    Ok(Some(Mode::Dump { out_dir: PathBuf::from(target), inputs }))
}

/// The user's real home, from the user database rather than `HOME`.
pub(crate) fn real_home() -> Option<PathBuf> {
    // SAFETY: getpwuid returns null or a pointer to static storage that stays
    // valid until the next call; it is read at once, before any thread starts.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_dir.is_null() {
            return None;
        }
        Some(PathBuf::from(CStr::from_ptr((*entry).pw_dir).to_string_lossy().into_owned()))
    }
}

fn refuse(message: &str) -> ! {
    eprintln!("markdown-bench: {message}");
    std::process::exit(2);
}

/// First thing in `main`, before any thread exists: when a mode is asked
/// for, refuses the real home, then sets up the scratch home's content —
/// preferences with Developer mode and the mock backend, the project, the
/// stress image, and the dump's inputs. Returns whether a mode is active.
pub(crate) fn prepare() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let check = parse_check(&args).unwrap_or_else(|message| refuse(&message));
    let mode = match parse_mode(&args) {
        Ok(Some(mode)) => mode,
        // The check keeps the scratch home's rules and set-up; it drives
        // itself (`transcript_check`), not the benchmark's stages.
        Ok(None) if check.is_some() => Mode::Bench { out: PathBuf::new() },
        Ok(None) => return false,
        Err(message) => refuse(&message),
    };
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from);
    let real = real_home();
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    match (&home, &real) {
        (Some(home), Some(real)) if canonical(home) != canonical(real) && !canonical(home).starts_with(canonical(real).join("Library")) => {}
        _ => refuse(&format!(
            "refusing to run with HOME={} — that is your real home, whose Omperor data this run would read and write.\n\
             Give it a scratch home; the run fills it in itself (preferences with Developer mode and the mock backend, a project):\n\
             \n    HOME=\"$(mktemp -d)\" {} {}\n",
            home.as_deref().map_or_else(|| "(unset)".into(), |home| home.display().to_string()),
            args[0],
            args[1..].join(" "),
        )),
    }
    let home = home.expect("checked above");
    // A scratch HOME alone is not isolation when an explicit state override
    // points elsewhere. Never let the benchmark populate an external store.
    if std::env::var_os("OMPEROR_STATE_DIR").is_some_and(|value| !value.is_empty()) {
        refuse("OMPEROR_STATE_DIR is set; unset it before running the scratch-home benchmark");
    }
    let state = preferences::application_support_directory().unwrap_or_else(|error| refuse(&error.to_string()));
    if state.exists() && !home.join(SCRATCH_MARKER).exists() {
        refuse(&format!(
            "refusing HOME={}: it already holds Omperor data and is not a scratch home this harness set up. Use an empty directory.",
            home.display()
        ));
    }
    let project = match std::env::var_os("OMPEROR_PROJECT").filter(|value| !value.is_empty()) {
        Some(project) => PathBuf::from(project),
        None => {
            let project = home.join("project");
            // SAFETY: called first in `main`, before any other thread exists.
            unsafe { std::env::set_var("OMPEROR_PROJECT", &project) };
            project
        }
    };
    if real.as_deref().is_some_and(|real| canonical(&project) == canonical(real)) {
        refuse("OMPEROR_PROJECT is the real home; point it into the scratch home or leave it unset");
    }
    let fresh = !state.exists();
    if let Err(error) = std::fs::write(home.join(SCRATCH_MARKER), "omperor --markdown-bench scratch home\n") {
        refuse(&format!("cannot mark the scratch home {}: {error}", home.display()));
    }
    if let Err(error) = populate(&mode, &project) {
        refuse(&format!("cannot set up the scratch home {}: {error}", home.display()));
    }
    if let Some((journal, _)) = &check {
        // A copy, in the scratch project, is what the check opens.
        let copy = journal.file_name().map(|name| project.join(name));
        if let Err(error) = copy.as_ref().map_or(Err(std::io::Error::other("no file name")), |copy| std::fs::copy(journal, copy)) {
            refuse(&format!("cannot copy {} into the scratch project: {error}", journal.display()));
        }
    }
    let stored = preferences::store(&preferences::Preferences {
        developer_mode: true,
        inference_backend: mock_inference::Backend::Mock,
        project: Some(project.clone()),
        appearance: preferences::AppearancePreference::Light,
        chat_style: crate::theme::ChatStyle::Modern,
        show_inline_images: true,
        frame_timing_overlay: false,
        ..preferences::Preferences::default()
    });
    if let Err(error) = stored {
        refuse(&format!("cannot store the scratch preferences: {error}"));
    }
    let _ = FRONTMOST_BEFORE.set(frontmost());
    eprintln!(
        "markdown-bench: scratch HOME {} ({}), project {}",
        home.display(),
        if fresh { "fresh" } else { "reused" },
        project.display()
    );
    match check {
        Some(check) => {
            let _ = CHECK.set(check);
        }
        None => {
            let _ = MODE.set(mode);
        }
    }
    true
}

/// The project's content: the stress image and, for a dump, its inputs.
fn populate(mode: &Mode, project: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(project)?;
    write_stress_image(&project.join(STRESS_IMAGE))?;
    if let Mode::Dump { inputs, .. } = mode {
        let directory = project.join(INPUT_DIRECTORY);
        std::fs::create_dir_all(&directory)?;
        for (name, text) in inputs {
            std::fs::write(directory.join(format!("{name}.md")), text)?;
        }
    }
    Ok(())
}

/// A small deterministic picture: a gold-to-plum field with a crown.
fn write_stress_image(path: &Path) -> std::io::Result<()> {
    let (width, height) = (320_u32, 160_u32);
    let gold = [201.0, 162.0, 39.0];
    let plum = [91.0, 42.0, 94.0];
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let t = f64::from(x) / f64::from(width - 1);
            let (fx, fy) = (f64::from(x), f64::from(y));
            // Three points over a band, in image coordinates (y down).
            let band = (100.0..=120.0).contains(&fy) && (90.0..=230.0).contains(&fx);
            let point = |centre: f64| (fx - centre).abs() <= (fy - 40.0).max(0.0) * 0.5 && (40.0..100.0).contains(&fy);
            let crown = band || point(110.0) || point(160.0) || point(210.0);
            for channel in 0..3 {
                let value = if crown { 250.0 } else { gold[channel] * (1.0 - t) + plum[channel] * t };
                pixels.push(value.round() as u8);
            }
        }
    }
    image::save_buffer_with_format(path, &pixels, width, height, image::ExtendedColorType::Rgb8, image::ImageFormat::Png)
        .map_err(std::io::Error::other)
}

/// `-[NSWindow orderWindow:relativeTo:]` as AppKit had it.
static ORDER_WINDOW: OnceLock<Imp> = OnceLock::new();
/// `-[NSView setNeedsDisplayInRect:]` as AppKit had it.
static SET_NEEDS_DISPLAY: OnceLock<Imp> = OnceLock::new();
/// The transcript document view's class's own `drawRect:`, and that class.
static DRAW_RECT: OnceLock<(usize, Imp)> = OnceLock::new();

thread_local! {
    /// The transcript's document view, whose invalidations are watched.
    static WATCHED: Cell<*const AnyObject> = const { Cell::new(std::ptr::null()) };
    /// Something in the transcript asked to be redrawn since the last draw:
    /// the renderer-neutral sign that a pass changed what it shows.
    static DIRTY: Cell<bool> = const { Cell::new(false) };
    /// The harness's own draw is running.
    static IN_BENCH_DRAW: Cell<bool> = const { Cell::new(false) };
    /// Draws of the transcript AppKit made by itself, outside the harness's.
    static APPKIT_DRAWS: Cell<u64> = const { Cell::new(0) };
    /// When each of those began and how long it took, until the harness looks.
    static APPKIT_DRAW_LOG: RefCell<Vec<(Instant, u32)>> = const { RefCell::new(Vec::new()) };
}

/// Marks the transcript dirty when its document view, or a view inside
/// it, is invalidated; then invalidates as AppKit would.
extern "C-unwind" fn watched_set_needs_display(view: &NSView, cmd: Sel, rect: NSRect) {
    let watched = WATCHED.get();
    if !watched.is_null() && !DIRTY.get() {
        let mut current = Some(view.retain());
        for _ in 0..6 {
            let Some(candidate) = current else { break };
            if std::ptr::eq(Retained::as_ptr(&candidate).cast::<AnyObject>(), watched) {
                DIRTY.set(true);
                break;
            }
            // SAFETY: reading a view's superview has no preconditions.
            current = unsafe { candidate.superview() };
        }
    }
    let original = *SET_NEEDS_DISPLAY.get().expect("replaced after it was kept");
    // SAFETY: the kept IMP is AppKit's own, with this signature.
    let original: extern "C-unwind" fn(&NSView, Sel, NSRect) = unsafe { std::mem::transmute(original) };
    original(view, cmd, rect);
}

/// Counts and names the transcript draws AppKit makes by itself.
extern "C-unwind" fn counted_draw_rect(view: &NSView, cmd: Sel, rect: NSRect) {
    let ours = IN_BENCH_DRAW.get();
    let watched = std::ptr::eq((view as *const NSView).cast::<AnyObject>(), WATCHED.get());
    let _span = (watched && !ours).then(|| {
        APPKIT_DRAWS.set(APPKIT_DRAWS.get() + 1);
        if APPKIT_DRAWS.get() == 1 {
            // Once: who draws the transcript besides the harness.
            let frames = objc2_foundation::NSThread::callStackSymbols();
            let frames: Vec<String> = (0..frames.count().min(24)).map(|index| frames.objectAtIndex(index).to_string()).collect();
            eprintln!("markdown-bench: the first draw of the transcript not made by the harness:\n  {}", frames.join("\n  "));
        }
        perf::span("gui.bench.appkit_draw")
    });
    let (_, original) = *DRAW_RECT.get().expect("replaced after it was kept");
    // SAFETY: the kept IMP is the class's own drawRect:, with this signature.
    let original: extern "C-unwind" fn(&NSView, Sel, NSRect) = unsafe { std::mem::transmute(original) };
    let started = Instant::now();
    original(view, cmd, rect);
    if _span.is_some() {
        let us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
        APPKIT_DRAW_LOG.with(|log| log.borrow_mut().push((started, us)));
    }
}

/// Watches `document`: its invalidations, and — when its class draws in a
/// `drawRect:` of its own — the draws AppKit makes of it.
fn watch(document: &NSView) {
    WATCHED.set(Retained::as_ptr(&document.retain()).cast());
    if DRAW_RECT.get().is_some() {
        return;
    }
    let class = document.class();
    let own = class.instance_methods().iter().find(|method| method.name() == sel!(drawRect:)).copied();
    let Some(method) = own else {
        eprintln!("markdown-bench: {} has no drawRect: of its own; AppKit's draws of it are not counted", class.name().to_string_lossy());
        return;
    };
    // SAFETY: the replacement has drawRect:'s C signature and calls the original.
    let original = unsafe {
        method.set_implementation(std::mem::transmute::<extern "C-unwind" fn(&NSView, Sel, NSRect), Imp>(counted_draw_rect))
    };
    let _ = DRAW_RECT.set((class as *const _ as usize, original));
}

/// Row classes whose own `drawRect:` is counted: for a transcript made of
/// row views inside its document view, their draws are AppKit drawing the
/// transcript. Each keeps the implementation it had.
static ROW_DRAW_RECTS: std::sync::Mutex<Vec<(usize, Imp)>> = std::sync::Mutex::new(Vec::new());

/// Counts AppKit's draws of views of `class` that sit inside the
/// transcript's document view, as `watch` does for the document view: a
/// renderer whose transcript is a stack of row views calls this for the
/// classes that draw. Does nothing unless this is a benchmark or dump run.
pub(crate) fn count_row_draws(class: &objc2::runtime::AnyClass) {
    if !active() {
        return;
    }
    let mut kept = ROW_DRAW_RECTS.lock().unwrap_or_else(|poison| poison.into_inner());
    let key = class as *const _ as usize;
    if kept.iter().any(|(kept, _)| *kept == key) {
        return;
    }
    let Some(method) = class.instance_methods().iter().find(|method| method.name() == sel!(drawRect:)).copied() else {
        eprintln!("markdown-bench: {} has no drawRect: of its own; its draws are not counted", class.name().to_string_lossy());
        return;
    };
    // SAFETY: the replacement has drawRect:'s C signature and calls the original.
    let original = unsafe {
        method.set_implementation(std::mem::transmute::<extern "C-unwind" fn(&NSView, Sel, NSRect), Imp>(counted_row_draw_rect))
    };
    kept.push((key, original));
}

/// `counted_draw_rect` for the row classes: a draw of a view inside the
/// watched document view, outside the harness's own, is AppKit's.
extern "C-unwind" fn counted_row_draw_rect(view: &NSView, cmd: Sel, rect: NSRect) {
    let original = {
        let kept = ROW_DRAW_RECTS.lock().unwrap_or_else(|poison| poison.into_inner());
        let mut class = Some(view.class());
        let mut found = None;
        while let Some(current) = class {
            let key = current as *const _ as usize;
            if let Some((_, imp)) = kept.iter().find(|(kept, _)| *kept == key) {
                found = Some(*imp);
                break;
            }
            class = current.superclass();
        }
        found.expect("a counted row class")
    };
    let watched = WATCHED.get();
    let inside = !watched.is_null() && {
        // SAFETY: reading a view's superview has no preconditions.
        let mut current = unsafe { view.superview() };
        let mut found = false;
        for _ in 0..8 {
            let Some(candidate) = current else { break };
            if std::ptr::eq(Retained::as_ptr(&candidate).cast::<AnyObject>(), watched) {
                found = true;
                break;
            }
            // SAFETY: as above.
            current = unsafe { candidate.superview() };
        }
        found
    };
    let counted = inside && !IN_BENCH_DRAW.get();
    let _span = counted.then(|| {
        APPKIT_DRAWS.set(APPKIT_DRAWS.get() + 1);
        perf::span("gui.bench.appkit_draw")
    });
    // SAFETY: the kept IMP is the class's own drawRect:, with this signature.
    let original: extern "C-unwind" fn(&NSView, Sel, NSRect) = unsafe { std::mem::transmute(original) };
    let started = Instant::now();
    original(view, cmd, rect);
    if counted {
        let us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
        APPKIT_DRAW_LOG.with(|log| log.borrow_mut().push((started, us)));
    }
}

/// Every window ordered in goes off every display first — an alert or a
/// panel AppKit would centre included.
extern "C-unwind" fn offscreen_order(window: &NSWindow, cmd: Sel, place: isize, other: isize) {
    // NSWindowOut is zero: ordering out needs no move.
    if place != 0 && placement_violation(window.frame(), MainThreadMarker::from(window)).is_some() {
        window.setFrameOrigin(OFFSCREEN);
    }
    let original = *ORDER_WINDOW.get().expect("replaced after it was kept");
    // SAFETY: the kept IMP is AppKit's own implementation of this selector,
    // with this signature.
    let original: extern "C-unwind" fn(&NSWindow, Sel, isize, isize) = unsafe { std::mem::transmute(original) };
    original(window, cmd, place, other);
}

extern "C-unwind" fn unconstrained_frame(_window: &AnyObject, _cmd: Sel, frame: NSRect, _screen: *mut AnyObject) -> NSRect {
    frame
}

/// Right after the shared application exists: [`keep_off_screen`], and
/// every invalidation is watched.
pub(crate) fn hide_application(app: &NSApplication) {
    if !active() {
        return;
    }
    keep_off_screen(app);
    let invalidate = NSView::class()
        .instance_method(sel!(setNeedsDisplayInRect:))
        .expect("NSView invalidates");
    // SAFETY: the replacement has the method's C signature — self, _cmd and
    // an NSRect — and calls the original.
    let original = unsafe {
        invalidate.set_implementation(std::mem::transmute::<extern "C-unwind" fn(&NSView, Sel, NSRect), Imp>(
            watched_set_needs_display,
        ))
    };
    let _ = SET_NEEDS_DISPLAY.set(original);
    frame_budget::set_quiet(true);
}

/// Never Regular, never in the Dock, never activatable; every window keeps
/// the frame it is given and goes off every display before it is ordered
/// in. Shared with `--readme-shots`.
pub(crate) fn keep_off_screen(app: &NSApplication) {
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    let method = NSWindow::class()
        .instance_method(sel!(constrainFrameRect:toScreen:))
        .expect("NSWindow constrains frames");
    // SAFETY: the replacement has the method's C signature — self, _cmd, an
    // NSRect and a nullable NSScreen, returning an NSRect — and does less.
    unsafe {
        method.set_implementation(std::mem::transmute::<
            extern "C-unwind" fn(&AnyObject, Sel, NSRect, *mut AnyObject) -> NSRect,
            Imp,
        >(unconstrained_frame));
    }
    let order = NSWindow::class()
        .instance_method(sel!(orderWindow:relativeTo:))
        .expect("NSWindow orders windows");
    // SAFETY: the replacement has the method's C signature — self, _cmd, an
    // NSWindowOrderingMode and a window number — and calls the original.
    let original = unsafe {
        order.set_implementation(std::mem::transmute::<
            extern "C-unwind" fn(&NSWindow, Sel, isize, isize),
            Imp,
        >(offscreen_order))
    };
    let _ = ORDER_WINDOW.set(original);
}

/// Instead of centring and showing the main window: places it off every
/// display, then orders it in without activating. Returns whether it did.
pub(crate) fn present(window: &NSWindow) -> bool {
    if !active() {
        return false;
    }
    let mtm = MainThreadMarker::from(window);
    let frame = NSRect::new(OFFSCREEN, NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));
    window.setFrame_display(frame, false);
    if let Some(violation) = placement_violation(window.frame(), mtm) {
        fatal(&format!("the window would be on screen before it is shown: {violation}"));
    }
    // Ordered in, even off every display, AppKit's display cycle draws what
    // a tick invalidates — but not what a scroll or a switch exposes — so
    // the measured drawing would depend on who drew. By default the window
    // is never ordered in: AppKit draws none of it and the harness draws the
    // transcript itself, the same way for every renderer. The environment
    // variable orders it in, off screen, to look at AppKit's own drawing.
    if ordered_in() {
        window.orderFrontRegardless();
    }
    if window.frame() != frame {
        fatal(&format!("AppKit moved the window to {:?}", window.frame()));
    }
    if let Some(violation) = offscreen_violation(mtm) {
        fatal(&violation);
    }
    eprintln!(
        "markdown-bench: main window at {}, visible {}; screens {}",
        rect_text(window.frame()),
        window.isVisible(),
        screens(mtm).iter().map(|screen| rect_text(*screen)).collect::<Vec<_>>().join(", ")
    );
    true
}

/// `OMPEROR_MARKDOWN_BENCH_ORDER_IN=1`: order the (off-screen) window in.
fn ordered_in() -> bool {
    std::env::var_os("OMPEROR_MARKDOWN_BENCH_ORDER_IN").is_some_and(|value| value == "1")
}

/// From `applicationDidFinishLaunching:`: the harness starts driving.
pub(crate) fn launched(host: Rc<dyn Host>) {
    if let Some((journal, out)) = CHECK.get() {
        crate::transcript_check::start(host, journal.clone(), out.clone());
        return;
    }
    let Some(mode) = MODE.get() else {
        return;
    };
    let mtm = MainThreadMarker::new().expect("the harness runs on the main thread");
    let block = RcBlock::new(|_timer: *mut CFRunLoopTimer| {
        HARNESS.with(|harness| {
            if let Ok(mut harness) = harness.try_borrow_mut()
                && let Some(harness) = harness.as_mut()
            {
                harness.act();
            }
        });
    });
    // SAFETY: the handler is copied by Core Foundation and runs only on the
    // main run loop; the timer is retained by the harness for the process.
    let timer = unsafe {
        CFRunLoopTimer::with_handler(None, CFAbsoluteTimeGetCurrent() + 0.1, 1.0e9, 0, 0, Some(&block))
    }
    .expect("a run-loop timer");
    if let Some(run_loop) = CFRunLoop::main() {
        // SAFETY: the mode is a process-lifetime CF constant.
        run_loop.add_timer(Some(&timer), unsafe { kCFRunLoopCommonModes });
    }
    let (writer, jobs) = mpsc::channel::<Job>();
    let writer_thread = std::thread::Builder::new()
        .name("markdown-bench-writer".into())
        .spawn(move || write_jobs(jobs))
        .expect("the writer thread starts");
    let build = std::thread::Builder::new()
        .name("markdown-bench-build-info".into())
        .spawn(build_info)
        .expect("the build-info thread starts");
    let now = Instant::now();
    let harness = Harness {
        host,
        mtm,
        mode: mode.clone(),
        timer,
        stage: Stage::Starting,
        stage_since: now,
        launched: now,
        last_change: now,
        viewport: None,
        run: None,
        phases: Vec::new(),
        realistic: None,
        heavy: None,
        previous_capture: None,
        written: Vec::new(),
        writer: Some((writer, writer_thread)),
        build: Some(build),
        checks: 0,
        ticks: 0,
        window_visible: None,
        occlusion_visible: None,
        equivalence: Vec::new(),
        replay: None,
        mismatch_images: 0,
        settled_passes: 0,
        pending_equivalence: None,
        stream_text: String::new(),
        frames: frames_directory(),
    };
    HARNESS.with(|slot| slot.replace(Some(harness)));
    frame_budget::set_pass_hook(Some(pass_ended));
}

/// From the end of the runtime tick: draws the transcript if the tick
/// changed it — in the same pass, as a display would — and advances.
pub(crate) fn after_tick() {
    if !active() {
        return;
    }
    HARNESS.with(|harness| {
        if let Ok(mut harness) = harness.try_borrow_mut()
            && let Some(harness) = harness.as_mut()
        {
            harness.on_tick();
        }
    });
}

thread_local! {
    static HARNESS: RefCell<Option<Harness>> = const { RefCell::new(None) };
}

/// The frame budget's look at the end of every pass.
fn pass_ended(started: Instant, us: u32) {
    HARNESS.with(|harness| {
        if let Ok(mut harness) = harness.try_borrow_mut()
            && let Some(harness) = harness.as_mut()
        {
            harness.look(started, us);
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Realistic,
    Heavy,
}

impl Stream {
    fn phase(self) -> &'static str {
        match self {
            Self::Realistic => "stream-realistic",
            Self::Heavy => "stream-heavy",
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::Realistic => REALISTIC_PROMPT,
            Self::Heavy => HEAVY_PROMPT,
        }
    }
}

/// Where the run is. An action stage runs on the harness's timer, in a pass
/// of its own; a waiting stage is checked after every tick and action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Starting,
    Open(Stream),
    AwaitReady(Stream),
    Streaming(Stream),
    SwitchAway,
    AwayQuiet,
    SwitchBack,
    OpenSettling,
    ResizeStep(usize),
    ResizeSettling(usize),
    ScrollTop,
    ScrollTopQuiet,
    ScrollStep(usize),
    ScrollSettling,
    DumpOpen(usize),
    DumpAwaitReady(usize),
    DumpStreaming(usize),
    DumpLook(usize, usize),
    DumpCapture(usize, usize),
    /// The stack and one view of the same text, compared: as the stream
    /// left it, then as the message opened from history is set.
    DumpEquivalence(usize, usize),
    DumpReload(usize, usize),
    DumpReloaded(usize, usize),
    /// A streamed replay of the input in a new conversation, compared at
    /// sampled appends.
    DumpReplayOpen(usize, usize),
    DumpReplayReady(usize, usize),
    DumpReplay(usize, usize),
    /// The stability phases (docs/MARKDOWN_CONFORMANCE.md): a long mixed
    /// session opened from its history at the end, and the reader's place
    /// watched through everything that follows.
    HistoryOpen,
    HistoryOpenSettling,
    MiddleOpen,
    MiddleScroll,
    MiddleSettling,
    ScrollUpSetup,
    ScrollUpQuiet,
    ScrollUp(Speed, usize),
    ScrollUpSettling(Speed),
    StreamSetup(Place),
    StreamQuiet(Place),
    StreamAppend(Place, usize),
    StreamSettling(Place),
    WidthSetup,
    WidthQuiet,
    WidthStep(usize),
    WidthSettling(usize),
    /// The screenshot mode: a conversation opened and its prompt sent
    /// (0 is captured finished, 1 while it streams), then each look.
    ShotOpen(usize),
    ShotAwaitReady(usize),
    ShotRunning(usize),
    ShotLook(usize, usize),
    ShotScroll(usize, usize, usize),
    ShotCapture(usize, usize, usize),
    /// Then a mock session with sub-agents answers `/usage`: its card, and
    /// the `/` list open over the composer, in every style, light and dark.
    ShotUsageOpen,
    ShotUsageReady,
    ShotUsageRunning,
    ShotUsageCard,
    ShotUsageLook(usize),
    ShotUsageCapture(usize, bool),
    /// With synthetic accounts (`OMPEROR_SHOT_SYNTHETIC_ACCOUNTS=1`), then
    /// `/usage all` in every look.
    ShotUsageAllCard,
    ShotUsageAllLook(usize),
    ShotUsageAllCapture(usize),
    Finish,
}

/// How fast the stability phases scroll up through unmade rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Speed {
    Normal,
    Fast,
}

impl Speed {
    fn phase(self) -> &'static str {
        match self {
            Self::Normal => "scroll-up-normal",
            Self::Fast => "scroll-up-fast",
        }
    }

    /// Points a 60 Hz frame: about 1 800 pt/s, a reader's trackpad scroll,
    /// and 15 000 pt/s, a fling.
    fn step(self) -> f64 {
        match self {
            Self::Normal => 30.0,
            Self::Fast => 250.0,
        }
    }

    fn steps(self) -> usize {
        match self {
            Self::Normal => 360,
            Self::Fast => 400,
        }
    }
}

/// Where the reader is while an answer streams in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// At the end: the view follows the answer.
    Bottom,
    /// Scrolled up into the history: nothing may move.
    Scrolled,
}

impl Place {
    fn phase(self) -> &'static str {
        match self {
            Self::Bottom => "stream-follow",
            Self::Scrolled => "stream-scrolled",
        }
    }
}

struct Draw {
    at: Instant,
    us: u32,
    hash: Option<u64>,
}

/// One phase being measured.
struct PhaseRun {
    name: &'static str,
    started: Instant,
    /// What latencies are measured from: the prompt sent, the conversation selected.
    t0: Instant,
    /// The harness's own draws.
    draws: Vec<Draw>,
    /// AppKit's draws of the transcript, when its class can be watched.
    appkit_draws: Vec<(Instant, u32)>,
    /// Passes in which something in the transcript was invalidated.
    invalidated_passes: usize,
    hash_draws: bool,
    turn_ended: Option<Instant>,
    steps: Vec<(String, Instant)>,
    notes: Map<String, Value>,
    /// The reader's place and what the viewport shows, pass by pass, in a
    /// stability phase.
    stability: Option<Stability>,
}

/// The content at the reader's reference point at the end of a pass: named
/// and placed in the viewport, or, for a reader at the end, the distance
/// from the end.
struct Probe {
    name: Option<String>,
    y: f64,
    follow: bool,
}

/// One pass of a stability phase.
struct PassLook {
    at: Instant,
    us: u32,
    /// How far the content at the reference point moved on screen, beyond
    /// what the harness scrolled; `None` when it could not be found again.
    drift: Option<f64>,
    /// The harness scrolled, resized or opened something in this pass.
    input: bool,
    follow: bool,
    unmade: usize,
    placeholders: usize,
    /// A probe was taken at the end of the pass before.
    measurable: bool,
    /// How far the scroll bar's thumb moved since the pass before, in
    /// points on screen, and how much its proportion changed.
    thumb: Option<(f64, f64)>,
    /// The transcript's rows or text changed in this pass.
    content: bool,
    /// How far the harness's own scroll in this pass moves the thumb.
    scrolled_thumb: f64,
}

#[derive(Default)]
struct Stability {
    probe: Option<Probe>,
    /// What the harness scrolled by in the pass under way.
    scrolled: f64,
    input: bool,
    looks: Vec<PassLook>,
    /// Frames kept for the PNG sequences (`OMPEROR_MARKDOWN_BENCH_FRAMES`).
    previous_frame: Option<Capture>,
    frames_written: usize,
    frame_index: usize,
    /// The instrument checking itself: a probe not found where it was just
    /// taken, and probes not found again a pass later.
    mismatches: Vec<Value>,
    lost: Vec<String>,
    /// The thumb at the end of the pass before: its proportion and its top.
    thumb: Option<(f64, f64)>,
    /// What the transcript held at the end of the pass before.
    extent: Option<(usize, usize)>,
}

enum Job {
    Png { path: PathBuf, width: usize, height: usize, stride: usize, pixels: Vec<u8>, canvas: [u8; 3] },
    Text { path: PathBuf, text: String },
}

struct Capture {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    stride: usize,
}

struct Harness {
    host: Rc<dyn Host>,
    mtm: MainThreadMarker,
    mode: Mode,
    timer: CFRetained<CFRunLoopTimer>,
    stage: Stage,
    stage_since: Instant,
    launched: Instant,
    last_change: Instant,
    viewport: Option<Retained<NSBitmapImageRep>>,
    run: Option<PhaseRun>,
    phases: Vec<Value>,
    realistic: Option<u64>,
    heavy: Option<u64>,
    previous_capture: Option<Capture>,
    written: Vec<String>,
    writer: Option<(mpsc::Sender<Job>, std::thread::JoinHandle<()>)>,
    build: Option<std::thread::JoinHandle<Value>>,
    checks: u64,
    ticks: u64,
    window_visible: Option<bool>,
    occlusion_visible: Option<bool>,
    /// Equivalence results, and the replay under way.
    equivalence: Vec<Value>,
    replay: Option<Replay>,
    mismatch_images: usize,
    settled_passes: usize,
    /// A comparison whose single view the display cycle is drawing.
    pending_equivalence: Option<PendingEquivalence>,
    /// The answer a stream phase is streaming.
    stream_text: String,
    /// Where frame pairs around drift and pop-in are written, if anywhere.
    frames: Option<PathBuf>,
}

/// A comparison in steps, each after the display cycle has drawn: the
/// stack drawn until two draws agree, then the single view likewise.
struct PendingEquivalence {
    record: Map<String, Value>,
    dark: bool,
    stem: String,
    /// When the next draw may be made.
    next: Instant,
    previous: Option<Capture>,
    draws: usize,
    stack: Option<(Capture, bool)>,
    info: Option<crate::transcript_stack::Equivalence>,
    after: Stage,
}

/// Draws of one side of a comparison before it counts as settled anyway.
const COMPARISON_DRAWS: usize = 12;

/// A streamed replay of a dump input (`Stage::DumpReplay`).
struct Replay {
    text: String,
    /// Where each append ends, in bytes.
    ends: Vec<usize>,
    /// Appends made.
    next: usize,
    since_sample: usize,
    every: usize,
    /// Sealed heads seen at the last look.
    sealed: usize,
    /// A sample is due, and why; it is taken once the stack is settled.
    pending: Option<&'static str>,
    settled_passes: usize,
    finished: bool,
    samples: usize,
}

impl Replay {
    fn new(text: &str, seed: u64) -> Replay {
        let mut state = seed | 1;
        let mut ends = Vec::new();
        let characters: Vec<usize> = text.char_indices().map(|(byte, _)| byte).chain(std::iter::once(text.len())).collect();
        let mut at = 0;
        while at + 1 < characters.len() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            at = (at + 1 + (state >> 33) as usize % REPLAY_CHUNK).min(characters.len() - 1);
            ends.push(characters[at]);
        }
        let every = (ends.len() / REPLAY_SAMPLES).max(1);
        Replay { text: text.to_owned(), ends, next: 0, since_sample: 0, every, sealed: 0, pending: None, settled_passes: 0, finished: false, samples: 0 }
    }

    /// Whether the append from `from` to `to` completes a line that opens or
    /// closes a construct: a fence, a `<details>` tag, display math, a
    /// definition, a heading, a callout.
    fn completes_construct(&self, from: usize, to: usize) -> bool {
        let chunk = &self.text[from..to];
        let Some(newline) = chunk.rfind('\n') else { return false };
        let end = from + newline;
        let start = self.text[..end].rfind('\n').map_or(0, |at| at + 1);
        let line = self.text[start..end].trim();
        line.starts_with("```")
            || line.starts_with("~~~")
            || line.starts_with("<details")
            || line == "</details>"
            || line == "$$"
            || (line.starts_with('[') && line.contains("]:"))
            || line.starts_with('#')
            || line.starts_with("> [!")
    }
}

impl Harness {
    fn arm(&self, after: f64) {
        self.timer.set_next_fire_date(CFAbsoluteTimeGetCurrent() + after);
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.stage_since = Instant::now();
    }

    fn quiet(&self, span: Duration) -> bool {
        self.last_change.elapsed() >= span
    }

    fn on_tick(&mut self) {
        self.ticks += 1;
        if self.ticks % 15 == 0 {
            self.isolate();
        }
        self.note_changes();
        self.check();
    }

    /// A harness pass: the stage's action, the draw it calls for, the check.
    fn act(&mut self) {
        self.isolate();
        let host = Rc::clone(&self.host);
        if self.pending_equivalence.is_some() {
            self.comparison_step();
            self.check();
            return;
        }
        match self.stage {
            Stage::Starting => {
                if let Some(document) = self.document_view() {
                    watch(&document);
                }
                self.note_changes();
                if host.ready() && self.quiet(Duration::from_secs(1)) && self.stage_since.elapsed() >= Duration::from_secs(1) {
                    self.record_window();
                    self.enter(match self.mode {
                        Mode::Bench { .. } if phases_wanted().0 => Stage::Open(Stream::Realistic),
                        Mode::Bench { .. } => Stage::HistoryOpen,
                        Mode::Dump { .. } => Stage::DumpOpen(0),
                        Mode::Shots { .. } => Stage::ShotOpen(0),
                    });
                    self.arm(0.05);
                } else if self.stage_since.elapsed() > STARTUP_TIMEOUT {
                    fatal("the app never became ready: is the scratch home's backend the mock?");
                } else {
                    self.arm(0.1);
                }
            }
            Stage::Open(stream) => {
                let key = host.new_conversation();
                match stream {
                    Stream::Realistic => self.realistic = Some(key),
                    Stream::Heavy => self.heavy = Some(key),
                }
                self.note_changes();
                self.enter(Stage::AwaitReady(stream));
                self.arm(0.05);
            }
            Stage::DumpOpen(input) => {
                host.new_conversation();
                self.note_changes();
                self.enter(Stage::DumpAwaitReady(input));
                self.arm(0.05);
            }
            Stage::AwaitReady(stream) => {
                if host.ready() && self.quiet(Duration::from_millis(300)) {
                    self.begin_phase(stream.phase());
                    host.send_prompt(stream.prompt());
                    self.note_changes();
                    self.enter(Stage::Streaming(stream));
                } else {
                    self.note_changes();
                    self.arm(0.05);
                }
            }
            Stage::SwitchAway => {
                host.select_conversation(self.realistic.expect("the realistic conversation"));
                self.note_changes();
                self.enter(Stage::AwayQuiet);
                self.arm(0.1);
            }
            Stage::AwayQuiet => {
                self.note_changes();
                if self.quiet(SETTLE) {
                    self.enter(Stage::SwitchBack);
                    self.arm(0.02);
                } else {
                    self.arm(0.1);
                }
            }
            Stage::SwitchBack => {
                self.begin_phase("open-long");
                if let Some(run) = &mut self.run {
                    run.hash_draws = true;
                    run.t0 = Instant::now();
                }
                host.select_conversation(self.heavy.expect("the heavy conversation"));
                DIRTY.set(false);
                self.draw_viewport();
                self.enter(Stage::OpenSettling);
            }
            Stage::ResizeStep(step) => {
                if step == 0 {
                    self.begin_phase("resize");
                }
                let width = RESIZE_WIDTHS[step];
                if let Some(run) = &mut self.run {
                    run.steps.push((format!("width {width}"), Instant::now()));
                }
                if let Some(window) = host.window() {
                    window.setFrame_display(NSRect::new(OFFSCREEN, NSSize::new(width, WINDOW_HEIGHT)), false);
                }
                self.draw_viewport();
                self.enter(Stage::ResizeSettling(step));
            }
            Stage::ScrollTop => {
                self.scroll_to(0.0);
                self.draw_viewport();
                self.enter(Stage::ScrollTopQuiet);
                self.arm(0.1);
            }
            Stage::ScrollTopQuiet => {
                self.note_changes();
                if self.quiet(Duration::from_millis(600)) {
                    self.begin_phase("scroll");
                    self.enter(Stage::ScrollStep(0));
                    self.arm(0.0);
                } else {
                    self.arm(0.1);
                }
            }
            Stage::ScrollStep(step) => {
                let target = (step as f64 + 1.0) * SCROLL_STEP_PT;
                let reached = self.scroll_to(target);
                self.draw_viewport();
                if reached || step + 1 >= MAX_SCROLL_STEPS {
                    let height = self.document_height();
                    if let Some(run) = &mut self.run {
                        run.notes.insert("steps".into(), json!(step + 1));
                        run.notes.insert("step_pt".into(), json!(SCROLL_STEP_PT));
                        run.notes.insert("document_height_pt".into(), json!(height));
                    }
                    self.enter(Stage::ScrollSettling);
                } else {
                    self.stage = Stage::ScrollStep(step + 1);
                    self.arm(SCROLL_INTERVAL);
                }
            }
            Stage::DumpAwaitReady(input) => {
                if host.ready() && self.quiet(Duration::from_millis(300)) {
                    let name = self.input_name(input);
                    host.send_prompt(&format!("/mock scenario=markdown file={INPUT_DIRECTORY}/{name}.md rate=0 ttft=0"));
                    self.note_changes();
                    self.enter(Stage::DumpStreaming(input));
                }
                self.note_changes();
                self.arm(0.1);
            }
            Stage::DumpStreaming(_) => {
                self.note_changes();
                self.arm(0.1);
            }
            Stage::DumpLook(input, look) => {
                let (style, _) = STYLES[look / 2];
                host.set_look(style, look % 2 == 1);
                self.previous_capture = None;
                self.note_changes();
                self.enter(Stage::DumpCapture(input, look));
                self.arm(CAPTURE_INTERVAL);
            }
            Stage::DumpCapture(input, look) => {
                self.note_changes();
                self.capture(input, look);
            }
            Stage::DumpEquivalence(input, look) => {
                self.note_changes();
                self.open_comparison(input, look, "streamed", None, Stage::DumpReload(input, look));
                self.arm(0.05);
            }
            Stage::DumpReload(input, look) => {
                host.reload_answer();
                self.settled_passes = 0;
                self.enter(Stage::DumpReloaded(input, look));
                self.arm(0.05);
            }
            Stage::DumpReloaded(input, look) => {
                self.note_changes();
                let settled = host.replay_state().is_some_and(|state| state.settled)
                    && upleft_render::fragments::async_objects::pending_count() == 0;
                self.settled_passes = if settled { self.settled_passes + 1 } else { 0 };
                if self.settled_passes < 3 {
                    self.arm(0.05);
                    return;
                }
                let inputs = self.input_count();
                let after = if look + 1 < looks() {
                    Stage::DumpLook(input, look + 1)
                } else if replays() {
                    Stage::DumpReplayOpen(input, 0)
                } else if input + 1 < inputs {
                    Stage::DumpOpen(input + 1)
                } else {
                    Stage::Finish
                };
                self.open_comparison(input, look, "static", None, after);
                self.arm(0.05);
            }
            Stage::DumpReplayOpen(input, look) => {
                let (style, _) = STYLES[look / 2];
                host.set_look(style, look % 2 == 1);
                host.new_conversation();
                self.note_changes();
                self.enter(Stage::DumpReplayReady(input, look));
                self.arm(0.1);
            }
            Stage::DumpReplayReady(input, look) => {
                self.note_changes();
                if host.ready() && self.quiet(Duration::from_millis(300)) {
                    let Mode::Dump { inputs, .. } = &self.mode else { unreachable!() };
                    self.replay = Some(Replay::new(&inputs[input].1, 0x5eed ^ (input as u64) << 8 ^ look as u64));
                    host.replay_begin();
                    self.enter(Stage::DumpReplay(input, look));
                    self.arm(0.02);
                } else {
                    self.arm(0.1);
                }
            }
            Stage::DumpReplay(input, look) => self.replay_step(input, look),
            Stage::ShotOpen(conversation) => {
                host.new_conversation();
                self.note_changes();
                self.enter(Stage::ShotAwaitReady(conversation));
                self.arm(0.1);
            }
            Stage::ShotAwaitReady(conversation) => {
                self.note_changes();
                if host.ready() && self.quiet(Duration::from_millis(300)) {
                    host.send_prompt(SHOWCASE_PROMPT);
                    self.enter(Stage::ShotRunning(conversation));
                }
                self.arm(0.1);
            }
            Stage::ShotRunning(conversation) => {
                self.note_changes();
                let due = if conversation == 0 {
                    !host.turn_running() && self.quiet(SETTLE) && self.stage_since.elapsed() > Duration::from_secs(1)
                } else {
                    self.stage_since.elapsed() >= shot_stream_at()
                };
                if due {
                    self.enter(Stage::ShotLook(conversation, 0));
                    self.arm(0.0);
                } else if self.stage_since.elapsed() > STAGE_TIMEOUT {
                    fatal("the showcase turn never finished");
                } else {
                    self.arm(0.1);
                }
            }
            Stage::ShotLook(conversation, look) => {
                let (style, _) = STYLES[look / 2];
                host.set_look(style, look % 2 == 1);
                self.note_changes();
                self.enter(Stage::ShotScroll(conversation, look, 0));
                self.arm(0.4);
            }
            Stage::ShotScroll(conversation, look, view) => {
                // The streaming capture follows the stream at the bottom.
                let fraction = if conversation == 0 { SHOT_VIEWS[view].1 } else { 1.0 };
                let bottom = self.host.transcript_scroll_view().map_or(0.0, |scroll| {
                    (self.document_height() - scroll.contentView().bounds().size.height).max(0.0)
                });
                self.scroll_to((bottom * fraction).round());
                self.note_changes();
                self.enter(Stage::ShotCapture(conversation, look, view));
                self.arm(if conversation == 0 { 0.9 } else { 0.35 });
            }
            Stage::ShotCapture(conversation, look, view) => {
                self.note_changes();
                self.capture_window(conversation, look, view);
                let views = if conversation == 0 { SHOT_VIEWS.len() } else { 1 };
                let next = if view + 1 < views {
                    Stage::ShotScroll(conversation, look, view + 1)
                } else if look + 1 < looks() {
                    Stage::ShotLook(conversation, look + 1)
                } else if conversation == 0 {
                    Stage::ShotOpen(1)
                } else {
                    Stage::ShotUsageOpen
                };
                self.enter(next);
                self.arm(0.05);
            }
            Stage::ShotUsageOpen => {
                host.new_conversation();
                self.note_changes();
                self.enter(Stage::ShotUsageReady);
                self.arm(0.1);
            }
            Stage::ShotUsageReady => {
                self.note_changes();
                if host.ready() && self.quiet(Duration::from_millis(300)) {
                    host.send_prompt("/mock scenario=subagents ttft=200 rate=0");
                    self.enter(Stage::ShotUsageRunning);
                }
                self.arm(0.1);
            }
            Stage::ShotUsageRunning => {
                self.note_changes();
                if !host.turn_running() && self.quiet(SETTLE) && self.stage_since.elapsed() > Duration::from_secs(1) {
                    if synthetic_accounts() {
                        host.inject_synthetic_accounts(false);
                    }
                    host.send_prompt("/usage");
                    self.enter(Stage::ShotUsageCard);
                } else if self.stage_since.elapsed() > STAGE_TIMEOUT {
                    fatal("the sub-agent turn never finished");
                }
                self.arm(0.1);
            }
            Stage::ShotUsageCard => {
                self.note_changes();
                if self.quiet(SETTLE) && self.stage_since.elapsed() > Duration::from_secs(1) {
                    self.enter(Stage::ShotUsageLook(0));
                    self.arm(0.0);
                } else {
                    self.arm(0.1);
                }
            }
            Stage::ShotUsageLook(look) => {
                let (style, _) = STYLES[look / 2];
                host.set_look(style, look % 2 == 1);
                let bottom = self.host.transcript_scroll_view().map_or(0.0, |scroll| {
                    (self.document_height() - scroll.contentView().bounds().size.height).max(0.0)
                });
                // With synthetic accounts the card's top holds what is under
                // review; otherwise its end, as before.
                let lift = if synthetic_accounts() { self.viewport_height() * 0.25 } else { 0.0 };
                self.scroll_to((bottom - lift).max(0.0));
                self.note_changes();
                self.enter(Stage::ShotUsageCapture(look, false));
                self.arm(0.6);
            }
            Stage::ShotUsageCapture(look, list) => {
                self.note_changes();
                let (_, style) = STYLES[look / 2];
                let mode = if look % 2 == 1 { "dark" } else { "light" };
                self.capture_named(&format!("{}-{style}-{mode}.png", if list { "slash" } else { "usage" }), look % 2 == 1);
                if !list {
                    host.open_slash_list("/");
                    self.enter(Stage::ShotUsageCapture(look, true));
                    self.arm(0.4);
                } else {
                    host.open_slash_list("");
                    self.enter(if look + 1 < STYLES.len() * 2 {
                        Stage::ShotUsageLook(look + 1)
                    } else if synthetic_accounts() {
                        // This card shows a refresh still on its way.
                        host.inject_synthetic_accounts(true);
                        host.send_prompt("/usage all");
                        Stage::ShotUsageAllCard
                    } else {
                        Stage::Finish
                    });
                    self.arm(0.05);
                }
            }
            Stage::ShotUsageAllCard => {
                self.note_changes();
                if self.quiet(SETTLE) && self.stage_since.elapsed() > Duration::from_secs(1) {
                    self.enter(Stage::ShotUsageAllLook(0));
                    self.arm(0.0);
                } else {
                    self.arm(0.1);
                }
            }
            Stage::ShotUsageAllLook(look) => {
                let (style, _) = STYLES[look / 2];
                host.set_look(style, look % 2 == 1);
                // The card's top, where the accounts are, rather than its end.
                let viewport = self.viewport_height();
                let bottom = (self.document_height() - viewport).max(0.0);
                self.scroll_to((bottom - viewport * 0.6).max(0.0));
                self.note_changes();
                self.enter(Stage::ShotUsageAllCapture(look));
                self.arm(0.6);
            }
            Stage::ShotUsageAllCapture(look) => {
                self.note_changes();
                let (_, style) = STYLES[look / 2];
                let mode = if look % 2 == 1 { "dark" } else { "light" };
                self.capture_named(&format!("usage-all-{style}-{mode}.png"), look % 2 == 1);
                self.enter(if look + 1 < STYLES.len() * 2 { Stage::ShotUsageAllLook(look + 1) } else { Stage::Finish });
                self.arm(0.05);
            }
            Stage::HistoryOpen => {
                self.begin_phase("history-open");
                self.note_input();
                let (turns, seed) = HISTORY_OPEN;
                host.open_history("history-open", long_session(turns, seed));
                self.note_changes();
                self.enter(Stage::HistoryOpenSettling);
                self.arm(0.1);
            }
            Stage::MiddleOpen => {
                self.begin_phase("history-middle");
                self.note_input();
                let (turns, seed) = HISTORY_MIDDLE;
                host.open_history("history-middle", long_session(turns, seed));
                self.note_changes();
                self.enter(Stage::MiddleScroll);
                self.arm(0.05);
            }
            Stage::MiddleScroll => {
                self.note_changes();
                let viewport = self.viewport_height();
                let height = self.document_height();
                if height >= viewport * MIDDLE_SCREENS || !host.history_loading() {
                    self.scroll_to(((height - viewport) / 2.0).max(0.0));
                    self.draw_viewport();
                    if let Some(run) = &mut self.run {
                        run.notes.insert("scrolled_at_document_pt".into(), json!(height));
                    }
                    self.enter(Stage::MiddleSettling);
                    self.arm(0.1);
                } else {
                    self.arm(0.02);
                }
            }
            Stage::ScrollUpSetup => {
                // The same start whatever the phase before left: 70 % down
                // the whole history, settled.
                let bottom = (self.document_height() - self.viewport_height()).max(0.0);
                self.scroll_to(bottom * 0.7);
                self.draw_viewport();
                self.enter(Stage::ScrollUpQuiet);
                self.arm(0.1);
            }
            Stage::ScrollUpQuiet => {
                self.note_changes();
                if self.quiet(Duration::from_millis(600)) && upleft_render::fragments::async_objects::pending_count() == 0 {
                    self.enter(Stage::ScrollUp(Speed::Normal, 0));
                    self.arm(0.0);
                } else {
                    self.arm(0.1);
                }
            }
            Stage::ScrollUp(speed, step) => {
                if step == 0 {
                    self.begin_phase(speed.phase());
                }
                let origin = self.origin();
                let target = (origin - speed.step()).max(0.0);
                self.scroll_to(target);
                self.draw_viewport();
                if target <= 0.0 || step + 1 >= speed.steps() {
                    let height = self.document_height();
                    if let Some(run) = &mut self.run {
                        run.notes.insert("steps".into(), json!(step + 1));
                        run.notes.insert("step_pt".into(), json!(speed.step()));
                        run.notes.insert("document_height_pt".into(), json!(height));
                    }
                    self.enter(Stage::ScrollUpSettling(speed));
                    self.arm(0.1);
                } else {
                    self.stage = Stage::ScrollUp(speed, step + 1);
                    self.arm(SCROLL_INTERVAL);
                }
            }
            Stage::StreamSetup(place) => {
                let bottom = (self.document_height() - self.viewport_height()).max(0.0);
                let target = match place {
                    Place::Bottom => bottom,
                    Place::Scrolled => (bottom - self.viewport_height() * 1.5).max(0.0),
                };
                self.scroll_to(target);
                self.draw_viewport();
                self.enter(Stage::StreamQuiet(place));
                self.arm(0.1);
            }
            Stage::StreamQuiet(place) => {
                self.note_changes();
                if self.quiet(Duration::from_millis(600)) && upleft_render::fragments::async_objects::pending_count() == 0 {
                    self.begin_phase(place.phase());
                    self.stream_text = stream_answer(place.phase());
                    host.replay_begin();
                    self.note_changes();
                    self.enter(Stage::StreamAppend(place, 0));
                    self.arm(1.0 / 30.0);
                } else {
                    self.arm(0.1);
                }
            }
            Stage::StreamAppend(place, at) => {
                if at >= self.stream_text.len() {
                    host.replay_finish();
                    self.note_changes();
                    self.enter(Stage::StreamSettling(place));
                    self.arm(0.1);
                } else {
                    let mut end = (at + STREAM_CHUNK).min(self.stream_text.len());
                    while !self.stream_text.is_char_boundary(end) {
                        end += 1;
                    }
                    host.replay_append(&self.stream_text[at..end]);
                    self.note_changes();
                    self.stage = Stage::StreamAppend(place, end);
                    self.arm(1.0 / 30.0);
                }
            }
            Stage::WidthSetup => {
                // 40 % into the history, with a line of a paragraph at the
                // top (not its first): what a new width rewraps.
                let bottom = (self.document_height() - self.viewport_height()).max(0.0);
                let mut target = bottom * 0.4;
                for _ in 0..400 {
                    self.scroll_to(target);
                    let inside = host.probe(false).is_some_and(|(name, _)| {
                        name.split_once('|').is_some_and(|(_, mark)| mark.starts_with('L') && !mark.ends_with(":0"))
                    });
                    if inside {
                        break;
                    }
                    target += 11.0;
                }
                self.draw_viewport();
                self.enter(Stage::WidthQuiet);
                self.arm(0.1);
            }
            Stage::WidthQuiet => {
                self.note_changes();
                if self.quiet(Duration::from_millis(600)) && upleft_render::fragments::async_objects::pending_count() == 0 {
                    self.begin_phase("width-history");
                    self.enter(Stage::WidthStep(0));
                    self.arm(0.0);
                } else {
                    self.arm(0.1);
                }
            }
            Stage::WidthStep(step) => {
                let width = HISTORY_WIDTHS[step];
                self.note_input();
                if let Some(run) = &mut self.run {
                    run.steps.push((format!("width {width}"), Instant::now()));
                }
                if let Some(window) = host.window() {
                    window.setFrame_display(NSRect::new(OFFSCREEN, NSSize::new(width, WINDOW_HEIGHT)), false);
                }
                self.draw_viewport();
                let pane = host.transcript_scroll_view().map_or(0.0, |scroll| scroll.contentView().bounds().size.width);
                if let Some(run) = &mut self.run {
                    let widths = run.notes.entry("transcript_widths").or_insert_with(|| json!([]));
                    if let Some(widths) = widths.as_array_mut() {
                        widths.push(json!([width, pane]));
                    }
                }
                self.enter(Stage::WidthSettling(step));
            }
            Stage::HistoryOpenSettling | Stage::MiddleSettling | Stage::ScrollUpSettling(_) | Stage::StreamSettling(_) => {
                self.note_changes();
                self.arm(0.1);
            }
            Stage::Finish => self.finish(),
            _ => {}
        }
        self.check();
    }

    /// The waiting stages' conditions, after every tick and action.
    fn check(&mut self) {
        // A dump streams every input again in every look.
        let (run_limit, stage_limit) = match self.mode {
            Mode::Bench { .. } | Mode::Shots { .. } => (RUN_TIMEOUT, STAGE_TIMEOUT),
            Mode::Dump { .. } => (RUN_TIMEOUT * 20, STAGE_TIMEOUT * 4),
        };
        if self.launched.elapsed() > run_limit {
            fatal(&format!("the run took longer than {} s (stage {:?})", run_limit.as_secs(), self.stage));
        }
        if !matches!(self.stage, Stage::Starting) && self.stage_since.elapsed() > stage_limit {
            fatal(&format!("stage {:?} took longer than {} s", self.stage, stage_limit.as_secs()));
        }
        let host = Rc::clone(&self.host);
        match self.stage {
            Stage::Streaming(stream) => {
                let quiet = self.quiet(SETTLE);
                let Some(run) = &mut self.run else { return };
                if run.turn_ended.is_none() && !host.turn_running() {
                    run.turn_ended = Some(Instant::now());
                }
                if run.turn_ended.is_some() && quiet {
                    self.end_phase();
                    self.enter(match stream {
                        Stream::Realistic => Stage::Open(Stream::Heavy),
                        Stream::Heavy => Stage::SwitchAway,
                    });
                    self.arm(0.1);
                }
            }
            Stage::OpenSettling if self.quiet(SETTLE) => {
                self.end_phase();
                self.enter(Stage::ResizeStep(0));
                self.arm(0.05);
            }
            Stage::ResizeSettling(step)
                if self.stage_since.elapsed() >= RESIZE_MIN_STEP && self.quiet(SETTLE) =>
            {
                if step + 1 < RESIZE_WIDTHS.len() {
                    self.enter(Stage::ResizeStep(step + 1));
                } else {
                    self.end_phase();
                    self.enter(Stage::ScrollTop);
                }
                self.arm(0.0);
            }
            Stage::ScrollSettling if self.quiet(Duration::from_millis(600)) => {
                self.end_phase();
                self.enter(if phases_wanted().1 { Stage::HistoryOpen } else { Stage::Finish });
                self.arm(0.0);
            }
            Stage::HistoryOpenSettling | Stage::MiddleSettling
                if !host.history_loading()
                    && self.quiet(SETTLE)
                    && upleft_render::fragments::async_objects::pending_count() == 0 =>
            {
                self.end_phase();
                self.enter(if self.stage == Stage::HistoryOpenSettling { Stage::MiddleOpen } else { Stage::ScrollUpSetup });
                self.arm(0.05);
            }
            Stage::ScrollUpSettling(speed) if self.quiet(Duration::from_millis(600)) => {
                self.end_phase();
                self.enter(match speed {
                    Speed::Normal => Stage::ScrollUp(Speed::Fast, 0),
                    Speed::Fast => Stage::StreamSetup(Place::Bottom),
                });
                self.arm(0.0);
            }
            Stage::StreamSettling(place)
                if !host.turn_running()
                    && self.quiet(SETTLE)
                    && upleft_render::fragments::async_objects::pending_count() == 0 =>
            {
                self.end_phase();
                self.enter(match place {
                    Place::Bottom => Stage::StreamSetup(Place::Scrolled),
                    Place::Scrolled => Stage::WidthSetup,
                });
                self.arm(0.0);
            }
            Stage::WidthSettling(step) if self.stage_since.elapsed() >= RESIZE_MIN_STEP && self.quiet(SETTLE) => {
                if step + 1 < HISTORY_WIDTHS.len() {
                    self.enter(Stage::WidthStep(step + 1));
                } else {
                    self.end_phase();
                    self.enter(Stage::Finish);
                }
                self.arm(0.0);
            }
            Stage::DumpStreaming(input) if !host.turn_running() && self.quiet(Duration::from_millis(600)) => {
                self.enter(Stage::DumpLook(input, 0));
                self.arm(0.0);
            }
            _ => {}
        }
    }

    fn input_name(&self, input: usize) -> String {
        match &self.mode {
            Mode::Dump { inputs, .. } => inputs[input].0.clone(),
            Mode::Bench { .. } | Mode::Shots { .. } => unreachable!(),
        }
    }

    fn document_view(&self) -> Option<Retained<NSView>> {
        self.host.transcript_scroll_view()?.documentView()
    }

    fn viewport_height(&self) -> f64 {
        self.host.transcript_scroll_view().map_or(0.0, |scroll| scroll.contentView().bounds().size.height)
    }

    fn origin(&self) -> f64 {
        self.host.transcript_scroll_view().map_or(0.0, |scroll| scroll.contentView().bounds().origin.y)
    }

    /// The pass under way was the harness's doing: a session opened, the
    /// window resized.
    fn note_input(&mut self) {
        if let Some(stability) = self.run.as_mut().and_then(|run| run.stability.as_mut()) {
            stability.input = true;
        }
    }

    /// The end of a pass in a stability phase: how far the content at the
    /// reference point moved beyond what the harness scrolled, what the
    /// viewport shows unfinished, and the reference point for the next pass.
    fn look(&mut self, started: Instant, us: u32) {
        let Some(scroll) = self.host.transcript_scroll_view() else { return };
        let Some(document) = scroll.documentView() else { return };
        if self.run.as_ref().is_none_or(|run| run.stability.is_none()) {
            return;
        }
        let host = Rc::clone(&self.host);
        let visible = scroll.contentView().bounds();
        let height = document.frame().size.height;
        let distance = (height - visible.size.height).max(0.0) - visible.origin.y;
        let (unmade, placeholders) = host.viewport_gaps();
        let extent = host.content_extent();
        let follow = height - (visible.origin.y + visible.size.height) <= 36.0;
        let probe = if follow {
            Some(Probe { name: None, y: distance, follow: true })
        } else {
            host.probe(false).map(|(name, y)| Probe { name: Some(name), y, follow: false })
        };
        let mismatch = probe.as_ref().and_then(|probe| {
            let name = probe.name.as_ref()?;
            let found = host.locate(name);
            (found.is_none_or(|found| (found - probe.y).abs() > 0.01)).then(|| json!([name, probe.y, found]))
        });
        let frame = self.frames.is_some().then(|| {
            IN_BENCH_DRAW.set(true);
            let frame = grab(&scroll, scroll.bounds());
            IN_BENCH_DRAW.set(false);
            // What the capture laid out and invalidated is not a change.
            DIRTY.set(false);
            frame
        });
        let run = self.run.as_mut().expect("checked above");
        let name = run.name;
        let stability = run.stability.as_mut().expect("checked above");
        let scrolled = std::mem::take(&mut stability.scrolled);
        let input = std::mem::take(&mut stability.input);
        let previous = stability.probe.take();
        let followed = previous.as_ref().is_some_and(|probe| probe.follow);
        let mut lost = None;
        let measurable = previous.is_some();
        let drift = previous.and_then(|probe| match probe.name {
            // A reader the harness scrolled away from the end is no longer
            // following it: nothing to compare.
            None if scrolled != 0.0 => None,
            None => Some(distance - (probe.y - scrolled)),
            Some(name) => {
                let found = host.locate(&name).map(|y| y - (probe.y - scrolled));
                if found.is_none() {
                    lost = Some(name);
                }
                found
            }
        });
        if let Some(mismatch) = mismatch
            && stability.mismatches.len() < 20
        {
            stability.mismatches.push(mismatch);
        }
        if let Some(name) = lost
            && stability.lost.len() < 20
        {
            stability.lost.push(name);
        }
        let thumb = thumb_now(&scroll);
        let moved = stability.thumb.zip(thumb).map(|((proportion, y), (now_proportion, now_y))| (now_y - y, now_proportion - proportion));
        stability.thumb = thumb;
        let content = stability.extent.replace(extent).is_some_and(|before| before != extent);
        if let Some((jump, proportion)) = moved
            && jump.abs() > 2.0
            && !input
            && !content
        {
            perf::event_with("bench.thumb_jump", || json!({ "jump": jump, "proportion": proportion, "phase": name }));
        }
        let scrolled_thumb = if height > 0.0 { scrolled * visible.size.height / height } else { 0.0 };
        stability.looks.push(PassLook {
            at: started,
            us,
            drift,
            input,
            follow: followed,
            unmade,
            placeholders,
            measurable,
            thumb: moved,
            content,
            scrolled_thumb,
        });
        stability.probe = probe;
        let Some(frame) = frame else { return };
        let index = stability.frame_index;
        stability.frame_index += 1;
        let event = drift.is_some_and(|drift| drift.abs() > DRIFT_EPSILON) || unmade > 0 || placeholders > 0;
        let before = std::mem::replace(&mut stability.previous_frame, Some(frame));
        if !event || stability.frames_written >= FRAME_EVENTS {
            return;
        }
        stability.frames_written += 1;
        let (Some(directory), Some((jobs, _))) = (&self.frames, &self.writer) else { return };
        let label = format!(
            "{name}-{index:05}-drift{:+.0}-unmade{unmade}-objects{placeholders}{}",
            drift.unwrap_or(0.0),
            if input { "-input" } else { "" }
        );
        let after = stability.previous_frame.as_ref().expect("kept above");
        for (capture, suffix) in [(before.as_ref(), "a-before"), (Some(after), "b-after")] {
            let Some(capture) = capture else { continue };
            let _ = jobs.send(Job::Png {
                path: directory.join(format!("{label}-{suffix}.png")),
                width: capture.width,
                height: capture.height,
                stride: capture.stride,
                pixels: capture.pixels.clone(),
                canvas: CANVAS_LIGHT,
            });
        }
    }

    /// The whole main window — sidebar, transcript, composer — drawn off
    /// screen into a PNG at `SCALE`, named by look and view.
    fn capture_window(&mut self, conversation: usize, look: usize, view: usize) {
        let Mode::Shots { out_dir } = &self.mode else { return };
        let Some(window) = self.host.window() else { return };
        let Some(content) = window.contentView() else { return };
        let (_, style) = STYLES[look / 2];
        let dark = look % 2 == 1;
        let name = if conversation == 0 {
            format!("{style}-{}-{}.png", if dark { "dark" } else { "light" }, SHOT_VIEWS[view].0)
        } else {
            format!("{style}-{}-streaming.png", if dark { "dark" } else { "light" })
        };
        // The frame's own view draws the title bar's backdrop too.
        let root = unsafe { content.superview() }.unwrap_or(content);
        let capture = grab(&root, root.bounds());
        let path = out_dir.join(&name);
        let canvas = if dark { CANVAS_DARK } else { CANVAS_LIGHT };
        let jobs = &self.writer.as_ref().expect("the writer runs until the end").0;
        let _ = jobs.send(Job::Png { path, width: capture.width, height: capture.height, stride: capture.stride, pixels: capture.pixels, canvas });
        eprintln!("markdown-bench: captured {name}");
        self.written.push(name);
    }

    /// The whole window, off screen, as `name`: the transcript and any
    /// overlay over the composer, which lives in the window's own views.
    fn capture_named(&mut self, name: &str, dark: bool) {
        let Mode::Shots { out_dir } = &self.mode else { return };
        let Some(window) = self.host.window() else { return };
        let Some(content) = window.contentView() else { return };
        let root = unsafe { content.superview() }.unwrap_or(content);
        let capture = grab(&root, root.bounds());
        let path = out_dir.join(name);
        let canvas = if dark { CANVAS_DARK } else { CANVAS_LIGHT };
        let jobs = &self.writer.as_ref().expect("the writer runs until the end").0;
        let _ = jobs.send(Job::Png { path, width: capture.width, height: capture.height, stride: capture.stride, pixels: capture.pixels, canvas });
        eprintln!("markdown-bench: captured {name}");
        self.written.push(name.to_owned());
    }

    fn document_height(&self) -> f64 {
        self.document_view().map_or(0.0, |document| document.frame().size.height)
    }

    /// Scrolls the transcript so `y` is the top of the viewport, clamped;
    /// returns whether that reached the bottom.
    fn scroll_to(&mut self, y: f64) -> bool {
        let Some(scroll) = self.host.transcript_scroll_view() else { return true };
        let clip = scroll.contentView();
        let bottom = (self.document_height() - clip.bounds().size.height).max(0.0);
        let target = y.min(bottom);
        // The stability phases measure what moved beyond this scroll.
        if let Some(stability) = self.run.as_mut().and_then(|run| run.stability.as_mut()) {
            stability.scrolled += target - clip.bounds().origin.y;
            stability.input = true;
        }
        clip.scrollToPoint(NSPoint::new(0.0, target));
        scroll.reflectScrolledClipView(&clip);
        target >= bottom
    }

    /// What the app changed since the last look: an invalidation anywhere
    /// in the transcript, and the draws AppKit made of it. While a phase
    /// follows frames (open-long), each invalidation is also drawn here.
    fn note_changes(&mut self) {
        let dirty = DIRTY.replace(false);
        let drawn = APPKIT_DRAW_LOG.with(|log| std::mem::take(&mut *log.borrow_mut()));
        if dirty || !drawn.is_empty() {
            self.last_change = Instant::now();
        }
        let mut follow = false;
        if let Some(run) = &mut self.run {
            run.invalidated_passes += usize::from(dirty);
            run.appkit_draws.extend(drawn);
            follow = dirty && run.hash_draws;
        }
        if follow {
            self.draw_viewport();
        }
    }

    /// Draws the transcript's visible rect into a bitmap, in `gui.bench.draw`,
    /// in the pass that called for it. AppKit's layer commit draws what the
    /// app invalidates, even for this window, but not what the harness
    /// exposes by scrolling, resizing, or switching; those it draws here.
    fn draw_viewport(&mut self) {
        let Some(scroll) = self.host.transcript_scroll_view() else { return };
        let bounds = scroll.bounds();
        let started = Instant::now();
        let hashing = self.run.as_ref().is_some_and(|run| run.hash_draws);
        if !hashing {
            // What AppKit does after a scroll or a resize: display what
            // needs it, and commit, rather than redrawing all of the view.
            let _span = perf::span("gui.bench.display");
            if let Some(window) = self.host.window() {
                window.displayIfNeeded();
            }
            if let Some(transaction) = objc2::runtime::AnyClass::get(c"CATransaction") {
                // SAFETY: +[CATransaction flush] takes no arguments.
                let _: () = unsafe { msg_send![transaction, flush] };
            }
        } else {
            let _span = perf::span("gui.bench.draw");
            let reuse = self.viewport.as_ref().is_some_and(|rep| rep.size() == bounds.size);
            if !reuse {
                self.viewport = Some(bitmap(bounds.size));
            }
            let rep = self.viewport.as_ref().expect("made above");
            IN_BENCH_DRAW.set(true);
            scroll.cacheDisplayInRect_toBitmapImageRep(bounds, rep);
            IN_BENCH_DRAW.set(false);
        }
        let us = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
        // What the draw itself laid out and invalidated is not a change.
        DIRTY.set(false);
        self.last_change = Instant::now();
        if let Some(run) = &mut self.run {
            let hash = run.hash_draws.then(|| sampled_hash(self.viewport.as_ref().expect("drawn")));
            run.draws.push(Draw { at: started, us, hash });
        }
    }

    fn begin_phase(&mut self, name: &'static str) {
        let now = Instant::now();
        frame_budget::collect_passes();
        self.run = Some(PhaseRun {
            name,
            started: now,
            t0: now,
            draws: Vec::new(),
            appkit_draws: Vec::new(),
            invalidated_passes: 0,
            hash_draws: false,
            turn_ended: None,
            steps: Vec::new(),
            notes: Map::new(),
            stability: STABILITY_PHASES.contains(&name).then(Stability::default),
        });
    }

    fn end_phase(&mut self) {
        let passes = frame_budget::take_passes();
        let Some(run) = self.run.take() else { return };
        let summary = summarize(&run, &passes);
        eprintln!("markdown-bench: {} done: {} passes, {} draws", run.name, passes.len(), run.draws.len());
        self.phases.push(summary);
    }

    fn record_window(&mut self) {
        if let Some(window) = self.host.window() {
            self.window_visible = Some(window.isVisible());
            self.occlusion_visible = Some(window.occlusionState().contains(NSWindowOcclusionState::Visible));
        }
    }

    /// Every window off every screen, the app never active or frontmost.
    fn isolate(&mut self) {
        self.checks += 1;
        if let Some(violation) = offscreen_violation(self.mtm) {
            fatal(&violation);
        }
        if NSRunningApplication::currentApplication().isActive() {
            fatal("the process became the active application");
        }
        let own = std::process::id() as i32;
        if NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .is_some_and(|app| pid(&app) == own)
        {
            fatal("the process became the frontmost application");
        }
    }

    /// One capture of the dump: the whole last answer, full height. Two
    /// identical in a row settle it and write it out.
    fn capture(&mut self, input: usize, look: usize) {
        let Some(displayed) = self.host.displayed_answer() else {
            self.arm(CAPTURE_INTERVAL);
            return;
        };
        let Some(document) = self.document_view() else { return };
        let rect = displayed.rect;
        let rep = bitmap(rect.size);
        let length = rep.bytesPerRow() as usize * rep.pixelsHigh() as usize;
        // SAFETY: the rep owns `length` bytes of pixel data.
        unsafe { std::ptr::write_bytes(rep.bitmapData(), 0, length) };
        document.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
        // SAFETY: as above; copied out before the rep is released.
        let pixels = unsafe { std::slice::from_raw_parts(rep.bitmapData(), length) }.to_vec();
        let capture = Capture {
            pixels,
            width: rep.pixelsWide() as usize,
            height: rep.pixelsHigh() as usize,
            stride: rep.bytesPerRow() as usize,
        };
        let settled = self.previous_capture.as_ref().is_some_and(|previous| {
            previous.width == capture.width && previous.height == capture.height && previous.pixels == capture.pixels
        });
        let timed_out = self.stage_since.elapsed() > CAPTURE_TIMEOUT;
        if !settled && !timed_out {
            self.previous_capture = Some(capture);
            self.arm(CAPTURE_INTERVAL);
            return;
        }
        let name = self.input_name(input);
        let (style, style_file) = STYLES[look / 2];
        let dark = look % 2 == 1;
        let appearance = if dark { "dark" } else { "light" };
        let stem = format!("{name}.{style_file}.{appearance}");
        let Mode::Dump { out_dir, .. } = &self.mode else { unreachable!() };
        let mut semantics = displayed.semantics;
        if let Value::Object(fields) = &mut semantics {
            fields.insert("input".into(), json!(name));
            fields.insert("style".into(), json!(style));
            fields.insert("appearance".into(), json!(appearance));
            fields.insert("height_pt".into(), json!(rect.size.height));
            fields.insert("width_pt".into(), json!(rect.size.width));
            fields.insert("png".into(), json!(format!("{stem}.png")));
            fields.insert("png_pixels".into(), json!([capture.width, capture.height]));
            fields.insert("settled".into(), json!(settled));
        }
        let jobs = &self.writer.as_ref().expect("the writer runs until the end").0;
        let _ = jobs.send(Job::Png {
            path: out_dir.join(format!("{stem}.png")),
            width: capture.width,
            height: capture.height,
            stride: capture.stride,
            pixels: capture.pixels,
            canvas: if dark { CANVAS_DARK } else { CANVAS_LIGHT },
        });
        let _ = jobs.send(Job::Text {
            path: out_dir.join(format!("{stem}.json")),
            text: serde_json::to_string_pretty(&semantics).unwrap_or_default(),
        });
        eprintln!("markdown-bench: {stem}{}", if settled { "" } else { " (did not settle)" });
        self.written.push(stem);
        self.previous_capture = None;
        self.enter(Stage::DumpEquivalence(input, look));
        self.arm(0.0);
    }

    fn input_count(&self) -> usize {
        match &self.mode {
            Mode::Dump { inputs, .. } => inputs.len(),
            Mode::Bench { .. } | Mode::Shots { .. } => 0,
        }
    }

    /// One pass of a streamed replay: a sample when one is due and the stack
    /// has settled, else the next append, else the end of the stream.
    fn replay_step(&mut self, input: usize, look: usize) {
        let host = Rc::clone(&self.host);
        let Some(state) = host.replay_state() else {
            self.arm(0.01);
            return;
        };
        let replay = self.replay.as_mut().expect("a replay under way");
        if state.sealed != replay.sealed {
            replay.sealed = state.sealed;
            replay.pending.get_or_insert("seal");
        }
        if let Some(reason) = replay.pending {
            // Settled: every append shown, and no diagram or formula still
            // rendering, twice running.
            if state.settled && upleft_render::fragments::async_objects::pending_count() == 0 {
                replay.settled_passes += 1;
            } else {
                replay.settled_passes = 0;
            }
            if replay.settled_passes < 2 {
                self.arm(0.01);
                return;
            }
            replay.settled_passes = 0;
            replay.pending = None;
            replay.samples += 1;
            let sample = (replay.samples, replay.next, replay.finished);
            let after = if replay.finished {
                let (appends, samples) = (replay.ends.len(), replay.samples);
                eprintln!("markdown-bench: replay {} look {look}: {appends} appends, {samples} samples", self.input_name(input));
                self.replay = None;
                let inputs = self.input_count();
                if look + 1 < looks() {
                    Stage::DumpReplayOpen(input, look + 1)
                } else if input + 1 < inputs {
                    Stage::DumpOpen(input + 1)
                } else {
                    Stage::Finish
                }
            } else {
                Stage::DumpReplay(input, look)
            };
            self.open_comparison(input, look, reason, Some(sample), after);
            self.arm(0.05);
            return;
        }
        if replay.next < replay.ends.len() {
            let from = if replay.next == 0 { 0 } else { replay.ends[replay.next - 1] };
            let to = replay.ends[replay.next];
            replay.next += 1;
            replay.since_sample += 1;
            let construct = replay.completes_construct(from, to);
            let delta = replay.text[from..to].to_owned();
            if replay.since_sample >= replay.every || construct {
                replay.since_sample = 0;
                replay.pending = Some(if construct { "construct" } else { "every" });
            }
            host.replay_append(&delta);
        } else if !replay.finished {
            replay.finished = true;
            replay.pending = Some("final");
            host.replay_finish();
        }
        self.arm(0.0);
    }

    /// Starts comparing the last answer's stack with one view of its text;
    /// `after` is the stage once they are compared.
    fn open_comparison(&mut self, input: usize, look: usize, kind: &str, sample: Option<(usize, usize, bool)>, after: Stage) {
        let name = self.input_name(input);
        let (_, style_file) = STYLES[look / 2];
        let dark = look % 2 == 1;
        let appearance = if dark { "dark" } else { "light" };
        let mut record = Map::new();
        record.insert("input".into(), json!(name));
        record.insert("style".into(), json!(style_file));
        record.insert("appearance".into(), json!(appearance));
        record.insert("kind".into(), json!(kind));
        if let Some((index, appends, finished)) = sample {
            record.insert("sample".into(), json!(index));
            record.insert("appends".into(), json!(appends));
            record.insert("finished".into(), json!(finished));
        }
        let suffix = sample.map_or_else(String::new, |(index, ..)| format!("-{index}"));
        let stem = format!("equivalence/{name}.{style_file}.{appearance}.{kind}{suffix}");
        if !self.host.begin_equivalence() {
            record.insert("identical".into(), json!(false));
            record.insert("error".into(), json!("no answer"));
            eprintln!("markdown-bench: {stem}: no answer to compare");
            self.equivalence.push(Value::Object(record));
            self.enter(after);
            return;
        }
        self.pending_equivalence = Some(PendingEquivalence {
            record,
            dark,
            stem,
            next: Instant::now() + Duration::from_millis(100),
            previous: None,
            draws: 0,
            stack: None,
            info: None,
            after,
        });
    }

    /// One step of a comparison: a draw of the side shown, once the display
    /// cycle has drawn it and nothing on it is still rendering; two alike in
    /// a row settle it.
    fn comparison_step(&mut self) {
        let host = Rc::clone(&self.host);
        let pending = self.pending_equivalence.as_mut().expect("a comparison");
        if Instant::now() < pending.next || upleft_render::fragments::async_objects::pending_count() > 0 {
            self.arm(0.03);
            return;
        }
        let mut captures: Vec<Capture> = Vec::new();
        host.capture_equivalence(&mut |view: &NSView, rect: NSRect| captures.push(grab(view, rect)));
        let Some(capture) = captures.pop() else {
            host.end_equivalence();
            let mut pending = self.pending_equivalence.take().expect("a comparison");
            pending.record.insert("identical".into(), json!(false));
            pending.record.insert("error".into(), json!("no capture"));
            self.equivalence.push(Value::Object(pending.record));
            self.enter(pending.after);
            self.arm(0.0);
            return;
        };
        pending.draws += 1;
        let settled = pending.previous.as_ref().is_some_and(|previous| {
            previous.width == capture.width && previous.height == capture.height && previous.pixels == capture.pixels
        });
        if !settled && pending.draws < COMPARISON_DRAWS {
            pending.previous = Some(capture);
            pending.next = Instant::now() + Duration::from_millis(60);
            self.arm(0.03);
            return;
        }
        pending.previous = None;
        pending.draws = 0;
        if pending.stack.is_none() {
            pending.stack = Some((capture, settled));
            pending.info = host.swap_equivalence();
            pending.next = Instant::now() + Duration::from_millis(100);
            self.arm(0.03);
            return;
        }
        host.end_equivalence();
        let pending = self.pending_equivalence.take().expect("a comparison");
        let after = pending.after;
        self.close_comparison(pending, capture, settled);
        self.enter(after);
        self.arm(0.0);
    }

    /// Compares a comparison's two sides pixel for pixel and records the
    /// result; a difference is written out as the two images and a map of it.
    fn close_comparison(&mut self, pending: PendingEquivalence, single: Capture, single_settled: bool) {
        let PendingEquivalence { mut record, dark, stem, stack, info, .. } = pending;
        let (Some((stack, stack_settled)), Some(info)) = (stack, info) else {
            record.insert("identical".into(), json!(false));
            record.insert("error".into(), json!("no single view"));
            self.equivalence.push(Value::Object(record));
            return;
        };
        let difference = difference(&stack, &single);
        let identical = difference.pixels == 0 && stack.width == single.width && stack.height == single.height;
        record.insert("identical".into(), json!(identical));
        record.insert("settled".into(), json!(stack_settled && single_settled));
        record.insert("segments".into(), json!(info.segments));
        record.insert("sealed".into(), json!(info.sealed));
        record.insert("streaming".into(), json!(info.streaming));
        record.insert("units".into(), json!(info.units));
        record.insert("stack_height_pt".into(), json!(info.stack_height));
        record.insert("single_height_pt".into(), json!(info.single_height));
        record.insert("stack_px".into(), json!([stack.width, stack.height]));
        record.insert("single_px".into(), json!([single.width, single.height]));
        record.insert("differing_px".into(), json!(difference.pixels));
        record.insert("first_differing_row_px".into(), json!(difference.first_row));
        record.insert("first_differing_pixel".into(), json!(difference.first));
        record.insert("differing_rows_pt".into(), json!(difference.rows.iter().map(|(from, to)| [*from as f64 / SCALE, *to as f64 / SCALE]).collect::<Vec<_>>()));
        record.insert("views".into(), json!(info.views));
        if let Some((at, stack_place, single_place)) = info.layout {
            record.insert("layout".into(), json!({ "offset": at, "stack": [stack_place.0, stack_place.1], "single": [single_place.0, single_place.1] }));
        }
        if !identical {
            eprintln!(
                "markdown-bench: {stem}: {} px differ (first row {:?}), {} segments, heights {} / {} pt, settled {}/{}, layout {:?}",
                difference.pixels, difference.first_row, info.segments, info.stack_height, info.single_height, stack_settled, single_settled, info.layout
            );
            if self.mismatch_images < MISMATCH_IMAGES {
                self.mismatch_images += 1;
                let Mode::Dump { out_dir, .. } = &self.mode else { unreachable!() };
                let canvas = if dark { CANVAS_DARK } else { CANVAS_LIGHT };
                let jobs = &self.writer.as_ref().expect("the writer runs until the end").0;
                for (which, capture) in [("stack", &stack), ("single", &single)] {
                    let _ = jobs.send(Job::Png {
                        path: out_dir.join(format!("{stem}-{which}.png")),
                        width: capture.width,
                        height: capture.height,
                        stride: capture.stride,
                        pixels: capture.pixels.clone(),
                        canvas,
                    });
                }
                let _ = jobs.send(Job::Png {
                    path: out_dir.join(format!("{stem}-diff.png")),
                    width: difference.width,
                    height: difference.height,
                    stride: difference.width * 4,
                    pixels: difference.map,
                    canvas,
                });
                record.insert("images".into(), json!(stem));
            }
        }
        self.equivalence.push(Value::Object(record));
    }

    /// Writes the report, waits for the writer, and ends the process.
    fn finish(&mut self) {
        self.isolate();
        let build = self.build.take().and_then(|handle| handle.join().ok()).unwrap_or(Value::Null);
        let frontmost_after = frontmost();
        let window = self.host.window();
        let isolation = json!({
            "own_pid": std::process::id(),
            "frontmost_before": FRONTMOST_BEFORE.get().cloned().unwrap_or(Value::Null),
            "frontmost_after": frontmost_after,
            "ever_active_or_frontmost": false,
            "checks": self.checks,
            "window_frame": window.as_ref().map(|window| rect_json(window.frame())),
            "window_ordered_in": ordered_in(),
            "window_visible": self.window_visible,
            "window_occlusion_visible": self.occlusion_visible,
            "appkit_transcript_draws": if DRAW_RECT.get().is_some() { json!(APPKIT_DRAWS.get()) } else { Value::Null },
            "screens": screens(self.mtm).into_iter().map(rect_json).collect::<Vec<_>>(),
        });
        let (report_path, report) = match &self.mode {
            Mode::Bench { out } => {
                print_table(&self.phases);
                (out.clone(), json!({
                    "tool": "omperor --markdown-bench",
                    "format": 1,
                    "build": build,
                    "budget_us": frame_budget::BUDGET_US as u64,
                    "window_pt": [WINDOW_WIDTH, WINDOW_HEIGHT],
                    "draw_scale": SCALE,
                    "prompts": {"stream-realistic": REALISTIC_PROMPT, "stream-heavy": HEAVY_PROMPT},
                    "isolation": isolation,
                    "phases": self.phases,
                }))
            }
            Mode::Shots { out_dir } => (out_dir.join("index.json"), json!({
                "tool": "omperor --screenshot",
                "format": 1,
                "build": build,
                "window_pt": [WINDOW_WIDTH, WINDOW_HEIGHT],
                "scale": SCALE,
                "prompt": SHOWCASE_PROMPT,
                "captures": self.written,
                "isolation": isolation,
            })),
            Mode::Dump { out_dir, inputs } => {
                let identical = self.equivalence.iter().filter(|record| record["identical"] == json!(true)).count();
                let count = |kinds: &[&str], same: bool| {
                    self.equivalence
                        .iter()
                        .filter(|record| kinds.iter().any(|kind| record["kind"] == json!(kind)) == same)
                        .filter(|record| record["identical"] == json!(true))
                        .count()
                };
                let total = |kinds: &[&str], same: bool| {
                    self.equivalence.iter().filter(|record| kinds.iter().any(|kind| record["kind"] == json!(kind)) == same).count()
                };
                let whole = ["static", "streamed"];
                let summary = json!({
                    "compared": self.equivalence.len(),
                    "identical": identical,
                    "different": self.equivalence.len() - identical,
                    "finished": {"compared": total(&whole, true), "identical": count(&whole, true)},
                    "streaming": {"compared": total(&whole, false), "identical": count(&whole, false)},
                });
                eprintln!("markdown-bench: equivalence {summary}");
                let jobs = &self.writer.as_ref().expect("the writer runs until the end").0;
                let _ = jobs.send(Job::Text {
                    path: out_dir.join("equivalence.json"),
                    text: serde_json::to_string_pretty(&json!({ "summary": summary, "results": self.equivalence })).unwrap_or_default(),
                });
                (out_dir.join("index.json"), json!({
                    "tool": "omperor --markdown-dump",
                    "format": 1,
                    "build": build,
                    "window_pt": [WINDOW_WIDTH, WINDOW_HEIGHT],
                    "scale": SCALE,
                    "inputs": inputs.iter().map(|(name, _)| name).collect::<Vec<_>>(),
                    "styles": STYLES.iter().map(|(label, _)| label).collect::<Vec<_>>(),
                    "appearances": ["light", "dark"],
                    "captures": self.written,
                    "equivalence": summary,
                    "isolation": isolation,
                }))
            }
        };
        let (jobs, writer) = self.writer.take().expect("the writer runs until the end");
        let _ = jobs.send(Job::Text { path: report_path.clone(), text: serde_json::to_string_pretty(&report).unwrap_or_default() });
        drop(jobs);
        let _ = writer.join();
        eprintln!(
            "markdown-bench: wrote {}; frontmost before {}, after {}",
            report_path.display(),
            FRONTMOST_BEFORE.get().cloned().unwrap_or(Value::Null),
            report["isolation"]["frontmost_after"],
        );
        std::process::exit(0);
    }
}

fn items<T: Message>(array: &NSArray<T>) -> Vec<Retained<T>> {
    (0..array.count()).map(|index| array.objectAtIndex(index)).collect()
}

fn pid(app: &NSRunningApplication) -> i32 {
    // SAFETY: `processIdentifier` returns a pid_t, a 32-bit integer.
    unsafe { msg_send![app, processIdentifier] }
}

fn fatal(message: &str) -> ! {
    eprintln!("markdown-bench: stopping: {message}");
    std::process::exit(3);
}

/// How many looks (style and appearance, in `STYLES` order, light then
/// dark) a dump covers: all six unless `OMPEROR_MARKDOWN_DUMP_LOOKS` says
/// fewer.
fn looks() -> usize {
    std::env::var("OMPEROR_MARKDOWN_DUMP_LOOKS").ok().and_then(|value| value.parse().ok()).unwrap_or(STYLES.len() * 2).clamp(1, STYLES.len() * 2)
}

/// Whether a dump replays its inputs as streams too; `OMPEROR_MARKDOWN_DUMP_REPLAY=0`
/// leaves them out.
fn replays() -> bool {
    std::env::var_os("OMPEROR_MARKDOWN_DUMP_REPLAY").is_none_or(|value| value != "0")
}

/// `rect` of `view` drawn into a fresh bitmap, transparent where nothing is:
/// its size rounded up to whole pixels, so every capture is drawn at exactly
/// `SCALE`.
fn grab(view: &NSView, rect: NSRect) -> Capture {
    let whole = |points: f64| (points * SCALE).ceil() / SCALE;
    let rect = NSRect::new(rect.origin, NSSize::new(whole(rect.size.width), whole(rect.size.height)));
    let rep = bitmap(rect.size);
    let length = rep.bytesPerRow() as usize * rep.pixelsHigh() as usize;
    // SAFETY: the rep owns `length` bytes of pixel data.
    unsafe { std::ptr::write_bytes(rep.bitmapData(), 0, length) };
    view.cacheDisplayInRect_toBitmapImageRep(rect, &rep);
    // SAFETY: as above; copied out before the rep is released.
    let pixels = unsafe { std::slice::from_raw_parts(rep.bitmapData(), length) }.to_vec();
    Capture { pixels, width: rep.pixelsWide() as usize, height: rep.pixelsHigh() as usize, stride: rep.bytesPerRow() as usize }
}

/// Where two captures differ, over the larger of them: a pixel counts when
/// either has it and they are not equal.
struct Difference {
    pixels: usize,
    first_row: Option<usize>,
    /// Runs of rows that differ, as pixel rows from and to.
    rows: Vec<(usize, usize)>,
    /// The first pixel that differs: where, and both values (RGBA).
    first: Option<(usize, usize, [u8; 4], [u8; 4])>,
    width: usize,
    height: usize,
    /// Red where they differ, the first capture faded elsewhere; RGBA.
    map: Vec<u8>,
}

fn difference(a: &Capture, b: &Capture) -> Difference {
    let width = a.width.max(b.width);
    let height = a.height.max(b.height);
    let pixel = |capture: &Capture, x: usize, y: usize| -> [u8; 4] {
        if x < capture.width && y < capture.height {
            let at = y * capture.stride + x * 4;
            [capture.pixels[at], capture.pixels[at + 1], capture.pixels[at + 2], capture.pixels[at + 3]]
        } else {
            [0; 4]
        }
    };
    let mut map = vec![0u8; width * height * 4];
    let mut pixels = 0;
    let mut first_row = None;
    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut first = None;
    for y in 0..height {
        for x in 0..width {
            let (one, other) = (pixel(a, x, y), pixel(b, x, y));
            let at = (y * width + x) * 4;
            if one != other {
                pixels += 1;
                first_row.get_or_insert(y);
                first.get_or_insert((x, y, one, other));
                match rows.last_mut() {
                    Some((_, to)) if *to + 4 >= y => *to = y + 1,
                    _ => rows.push((y, y + 1)),
                }
                map[at..at + 4].copy_from_slice(&[230, 0, 0, 255]);
            } else {
                map[at..at + 4].copy_from_slice(&[one[0] / 4, one[1] / 4, one[2] / 4, one[3] / 4]);
            }
        }
    }
    Difference { pixels, first_row, rows, first, width, height, map }
}

/// An RGBA bitmap of `size` points at `SCALE`, premultiplied, alpha last.
fn bitmap(size: NSSize) -> Retained<NSBitmapImageRep> {
    let wide = (size.width * SCALE).ceil().max(1.0) as isize;
    let high = (size.height * SCALE).ceil().max(1.0) as isize;
    // SAFETY: null planes ask AppKit to allocate the pixel buffer; the color
    // space name is an AppKit constant.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            wide,
            high,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }
    .expect("an RGBA bitmap");
    rep.setSize(size);
    rep
}

/// A cheap fingerprint of a drawn frame: every 61st word, so a pass is not
/// made longer by hashing megabytes.
fn sampled_hash(rep: &NSBitmapImageRep) -> u64 {
    let length = rep.bytesPerRow() as usize * rep.pixelsHigh() as usize;
    let data = rep.bitmapData() as *const u8;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut offset = 0;
    while offset + 8 <= length {
        // SAFETY: `offset + 8` lies within the rep's pixel buffer.
        let word = unsafe { std::ptr::read_unaligned(data.add(offset) as *const u64) };
        hash = (hash ^ word).wrapping_mul(0x0100_0000_01b3);
        offset += 61 * 8;
    }
    hash
}

fn screens(mtm: MainThreadMarker) -> Vec<NSRect> {
    items(&NSScreen::screens(mtm)).iter().map(|screen| screen.frame()).collect()
}

fn intersects(a: NSRect, b: NSRect) -> bool {
    a.size.width > 0.0
        && a.size.height > 0.0
        && a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

fn placement_violation(frame: NSRect, mtm: MainThreadMarker) -> Option<String> {
    screens(mtm)
        .into_iter()
        .find(|screen| intersects(frame, *screen))
        .map(|screen| format!("frame {} meets screen {}", rect_text(frame), rect_text(screen)))
}

/// The first visible window of this process that meets a screen, if any.
fn offscreen_violation(mtm: MainThreadMarker) -> Option<String> {
    let app = NSApplication::sharedApplication(mtm);
    items(&app.windows()).into_iter().filter(|window| window.isVisible()).find_map(|window| {
        placement_violation(window.frame(), mtm).map(|violation| {
            format!("window {:?} is visible on a screen: {violation}", window.title().to_string())
        })
    })
}

fn rect_text(rect: NSRect) -> String {
    format!("({}, {}, {}×{})", rect.origin.x, rect.origin.y, rect.size.width, rect.size.height)
}

fn rect_json(rect: NSRect) -> Value {
    json!([rect.origin.x, rect.origin.y, rect.size.width, rect.size.height])
}

fn frontmost() -> Value {
    NSWorkspace::sharedWorkspace().frontmostApplication().map_or(Value::Null, |app| {
        json!({
            "pid": pid(&app),
            "bundle": app.bundleIdentifier().map(|id| id.to_string()),
            "name": app.localizedName().map(|name| name.to_string()),
        })
    })
}

/// Which source and binary this is: the source tree's commit (read now,
/// on this worker), whether it had uncommitted changes, and the profile.
fn build_info() -> Value {
    let source = env!("CARGO_MANIFEST_DIR");
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(source)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let executable = std::env::current_exe().ok();
    let metadata = executable.as_ref().and_then(|path| std::fs::metadata(path).ok());
    json!({
        "source": source,
        "commit": git(&["rev-parse", "HEAD"]),
        "branch": git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        "uncommitted_files": git(&["status", "--porcelain", "--untracked-files=no"]).map(|status| status.lines().count()),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "binary": executable.map(|path| path.display().to_string()),
        "binary_bytes": metadata.as_ref().map(std::fs::Metadata::len),
        "binary_modified": metadata
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_secs()),
        "ran_at": SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or(0),
    })
}

fn write_jobs(jobs: mpsc::Receiver<Job>) {
    for job in jobs {
        match job {
            Job::Text { path, text } => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(error) = std::fs::write(&path, text) {
                    eprintln!("markdown-bench: cannot write {}: {error}", path.display());
                }
            }
            Job::Png { path, width, height, stride, pixels, canvas } => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // Premultiplied RGBA over the canvas, as the window shows it.
                let mut rgb = Vec::with_capacity(width * height * 3);
                for row in pixels.chunks(stride).take(height) {
                    for pixel in row[..width * 4].chunks_exact(4) {
                        let alpha = u32::from(pixel[3]);
                        for channel in 0..3 {
                            let over = u32::from(pixel[channel]) + u32::from(canvas[channel]) * (255 - alpha) / 255;
                            rgb.push(over.min(255) as u8);
                        }
                    }
                }
                let saved = image::save_buffer_with_format(
                    &path,
                    &rgb,
                    width as u32,
                    height as u32,
                    image::ExtendedColorType::Rgb8,
                    image::ImageFormat::Png,
                );
                if let Err(error) = saved {
                    eprintln!("markdown-bench: cannot write {}: {error}", path.display());
                }
            }
        }
    }
}

/// The nearest-rank percentile of sorted values.
fn percentile(sorted: &[u32], fraction: f64) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn distribution(values: &[u32]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let total: u64 = sorted.iter().map(|value| u64::from(*value)).sum();
    json!({
        "p50": percentile(&sorted, 0.50),
        "p90": percentile(&sorted, 0.90),
        "p99": percentile(&sorted, 0.99),
        "max": sorted.last().copied().unwrap_or(0),
        "mean": if sorted.is_empty() { 0 } else { total / sorted.len() as u64 },
        "total": total,
    })
}

/// A phase's report: every pass, their distribution, and the spans in them.
fn summarize(run: &PhaseRun, passes: &[frame_budget::Pass]) -> Value {
    let ended = Instant::now();
    let durations: Vec<u32> = passes.iter().map(|pass| pass.us).collect();
    let mut spans: BTreeMap<&str, (u64, u64, u32)> = BTreeMap::new();
    for pass in passes {
        for (name, us) in &pass.spans {
            let entry = spans.entry(*name).or_default();
            entry.0 += 1;
            entry.1 += u64::from(*us);
            entry.2 = entry.2.max(*us);
        }
        // Past the ledger's thousand a pass keeps sums by name, not spans.
        for (name, total, count) in &pass.overflow {
            let entry = spans.entry(*name).or_default();
            entry.0 += u64::from(*count);
            entry.1 += total;
        }
    }
    let mut span_list: Vec<_> = spans.into_iter().collect();
    span_list.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.0.cmp(b.0)));
    let span_totals: Map<String, Value> = span_list
        .iter()
        .map(|(name, (count, total, max))| ((*name).to_owned(), json!({"count": count, "total_us": total, "max_us": max})))
        .collect();
    let pass_list: Vec<Value> = passes
        .iter()
        .map(|pass| {
            let mut top = pass.spans.clone();
            top.sort_by(|a, b| b.1.cmp(&a.1));
            top.truncate(3);
            json!([pass.us, top.iter().map(|(name, us)| json!([name, us])).collect::<Vec<_>>()])
        })
        .collect();
    let ms = |from: Instant, to: Instant| to.saturating_duration_since(from).as_secs_f64() * 1_000.0;
    let draw_us: Vec<u32> = run.draws.iter().map(|draw| draw.us).collect();
    let appkit_us: Vec<u32> = run.appkit_draws.iter().map(|(_, us)| *us).collect();
    let end_of = |at: Instant, us: u32| at + Duration::from_micros(u64::from(us));
    let first_draw = run
        .draws
        .iter()
        .map(|draw| end_of(draw.at, draw.us))
        .chain(run.appkit_draws.iter().map(|(at, us)| end_of(*at, *us)))
        .filter(|end| *end >= run.t0)
        .min();
    let mut report = json!({
        "name": run.name,
        "wall_ms": ms(run.started, ended),
        "passes": passes.len(),
        "pass_us": distribution(&durations),
        "over_4ms": durations.iter().filter(|us| **us > 4_000).count(),
        "over_8_33ms": durations.iter().filter(|us| **us > 8_333).count(),
        "draws": {
            "harness": {"count": run.draws.len(), "us": distribution(&draw_us)},
            "appkit": {"count": run.appkit_draws.len(), "us": distribution(&appkit_us)},
        },
        "invalidated_passes": run.invalidated_passes,
        "first_draw_ms": first_draw.map(|end| ms(run.t0, end)),
        "spans": span_totals,
        "pass_list": pass_list,
    });
    let fields = report.as_object_mut().expect("an object");
    if let Some(ended) = run.turn_ended {
        fields.insert("turn_ms".into(), json!(ms(run.t0, ended)));
    }
    if run.hash_draws && let Some(last) = run.draws.last() {
        // The first frame that already looks like the settled one, and
        // every frame after it does too.
        let first = run
            .draws
            .iter()
            .rposition(|draw| draw.hash != last.hash)
            .map_or(0, |index| index + 1);
        let draw = &run.draws[first];
        fields.insert(
            "first_complete_frame_ms".into(),
            json!(ms(run.t0, draw.at + Duration::from_micros(u64::from(draw.us)))),
        );
        fields.insert("frames_until_complete".into(), json!(first + 1));
    }
    if !run.steps.is_empty() {
        // The last step keeps every pass after its mark.
        let marks: Vec<Instant> =
            run.steps.iter().map(|(_, at)| *at).chain([ended + Duration::from_secs(3_600)]).collect();
        let steps: Vec<Value> = run
            .steps
            .iter()
            .enumerate()
            .map(|(index, (label, _))| {
                let (from, to) = (marks[index], marks[index + 1]);
                let inside: Vec<u32> = passes
                    .iter()
                    .filter(|pass| {
                        let end = pass.started + Duration::from_micros(u64::from(pass.us));
                        end > from && end <= to
                    })
                    .map(|pass| pass.us)
                    .collect();
                json!({
                    "step": label,
                    "passes": inside.len(),
                    "total_us": inside.iter().map(|us| u64::from(*us)).sum::<u64>(),
                    "max_us": inside.iter().max().copied().unwrap_or(0),
                    "first_pass_us": inside.first().copied().unwrap_or(0),
                })
            })
            .collect();
        fields.insert("steps".into(), json!(steps));
    }
    for (key, value) in &run.notes {
        fields.insert(key.clone(), value.clone());
    }
    if let Some(stability) = &run.stability {
        fields.insert("stability".into(), stability_report(stability));
    }
    report
}

/// The vertical scroll bar's thumb: its proportion of the track, and its
/// top on screen in points, from the scroller's value and proportion over a
/// track as long as the viewport.
fn thumb_now(scroll: &NSScrollView) -> Option<(f64, f64)> {
    let scroller = scroll.verticalScroller()?;
    let proportion = scroller.knobProportion();
    let track = scroll.contentView().bounds().size.height;
    Some((proportion, scroller.doubleValue() * track * (1.0 - proportion)))
}

/// A stability phase's findings: drift of the reference content, in the
/// passes the harness caused nothing in and in those it scrolled or resized,
/// and pop-in: passes whose viewport shows unmade rows or placeholders.
fn stability_report(stability: &Stability) -> Value {
    let drifted = |looks: &[&PassLook]| {
        let drifts: Vec<f64> = looks.iter().filter_map(|look| look.drift).collect();
        let drifting: Vec<&&PassLook> = looks.iter().filter(|look| look.drift.is_some_and(|drift| drift.abs() > DRIFT_EPSILON)).collect();
        json!({
            "passes": looks.len(),
            "measured": drifts.len(),
            "lost": looks.iter().filter(|look| look.measurable && look.drift.is_none()).count(),
            "drifting": drifting.len(),
            "max_pt": drifts.iter().fold(0.0_f64, |max, drift| max.max(drift.abs())),
            "total_pt": drifts.iter().map(|drift| drift.abs()).sum::<f64>(),
            "while_following": drifting.iter().filter(|look| look.follow).count(),
            "first": drifting.iter().take(12).map(|look| json!([look.drift, look.us, look.follow])).collect::<Vec<_>>(),
        })
    };
    let quiet: Vec<&PassLook> = stability.looks.iter().filter(|look| !look.input).collect();
    let input: Vec<&PassLook> = stability.looks.iter().filter(|look| look.input).collect();
    let rows = stability.looks.iter().filter(|look| look.unmade > 0).count();
    let objects = stability.looks.iter().filter(|look| look.placeholders > 0).count();
    let _ = stability.looks.first().map(|look| look.at);
    // The scroll bar in the passes the harness did not scroll in: how far
    // the thumb jumped in one pass, and how often it moved beyond a point;
    // over the passes that changed only measurements, and over those that
    // changed the content too (a page of history, a streamed delta).
    let bar = |looks: &[&&PassLook]| {
        let thumbs: Vec<(f64, f64)> = looks.iter().filter_map(|look| look.thumb).collect();
        let jumps: Vec<f64> = thumbs.iter().map(|(y, _)| y.abs()).collect();
        json!({
            "passes": thumbs.len(),
            "max_jump_pt": jumps.iter().fold(0.0_f64, |max, jump| max.max(*jump)),
            "over_1pt": jumps.iter().filter(|jump| **jump > 1.0).count(),
            "over_2pt": jumps.iter().filter(|jump| **jump > 2.0).count(),
            "total_pt": jumps.iter().sum::<f64>(),
            "max_proportion_change": thumbs.iter().fold(0.0_f64, |max, (_, proportion)| max.max(proportion.abs())),
            "proportion_changes": thumbs.iter().filter(|(_, proportion)| proportion.abs() > 1.0e-4).count(),
        })
    };
    let measured: Vec<&&PassLook> = quiet.iter().filter(|look| !look.content).collect();
    let changed: Vec<&&PassLook> = quiet.iter().filter(|look| look.content).collect();
    let mut scroll_bar = bar(&quiet.iter().collect::<Vec<_>>());
    scroll_bar["measurements_only"] = bar(&measured);
    scroll_bar["content_changed"] = bar(&changed);
    // In the passes the harness scrolled in, how far the thumb moved beyond
    // what that scroll moves it: the rows made on the way correcting their
    // guesses.
    let excess: Vec<f64> = input
        .iter()
        .filter(|look| look.scrolled_thumb != 0.0 && !look.content)
        .filter_map(|look| look.thumb.map(|(y, _)| (y - look.scrolled_thumb).abs()))
        .collect();
    scroll_bar["scrolled_excess"] = json!({
        "passes": excess.len(),
        "max_pt": excess.iter().fold(0.0_f64, |max, excess| max.max(*excess)),
        "over_2pt": excess.iter().filter(|excess| **excess > 2.0).count(),
        "total_pt": excess.iter().sum::<f64>(),
    });
    json!({
        "scroll_bar": scroll_bar,
        "probe_mismatches": stability.mismatches,
        "lost_names": stability.lost,
        "drift": drifted(&quiet),
        "input_drift": drifted(&input),
        "pop_in": {
            "passes": stability.looks.iter().filter(|look| look.unmade > 0 || look.placeholders > 0).count(),
            "row_passes": rows,
            "object_passes": objects,
            "max_unmade": stability.looks.iter().map(|look| look.unmade).max().unwrap_or(0),
            "max_placeholders": stability.looks.iter().map(|look| look.placeholders).max().unwrap_or(0),
        },
    })
}

/// Spans the table names, in the order of the transcript's pipeline.
const TABLE_SPANS: [(&str, &str); 9] = [
    ("gui.render_chat_transcript", "render"),
    ("gui.markdown.render_message", "md.msg"),
    ("gui.markdown.parse", "md.parse"),
    ("gui.transcript.storage_edit", "storage"),
    ("gui.transcript.ensure_layout", "layout"),
    ("gui.transcript.scroll", "scroll"),
    ("gui.bench.draw", "draw"),
    ("gui.bench.display", "display"),
    ("gui.bench.appkit_draw", "appkit"),
];

fn print_table(phases: &[Value]) {
    let ms = |us: &Value| us.as_u64().map_or("-".to_owned(), |us| format!("{:.2}", us as f64 / 1_000.0));
    println!(
        "{:<17} {:>6} {:>7} {:>7} {:>7} {:>7} {:>6} {:>7} {:>10} {:>7} {:>9}",
        "phase", "passes", "p50 ms", "p90 ms", "p99 ms", "max ms", ">4ms", ">8.33ms", "draws h/a", "wall s", "first ms"
    );
    for phase in phases {
        let first = phase
            .get("first_complete_frame_ms")
            .or_else(|| phase.get("first_draw_ms"))
            .and_then(Value::as_f64)
            .map_or("-".to_owned(), |ms| format!("{ms:.1}"));
        println!(
            "{:<17} {:>6} {:>7} {:>7} {:>7} {:>7} {:>6} {:>7} {:>10} {:>7.1} {:>9}",
            phase["name"].as_str().unwrap_or("?"),
            phase["passes"],
            ms(&phase["pass_us"]["p50"]),
            ms(&phase["pass_us"]["p90"]),
            ms(&phase["pass_us"]["p99"]),
            ms(&phase["pass_us"]["max"]),
            phase["over_4ms"],
            phase["over_8_33ms"],
            format!("{}/{}", phase["draws"]["harness"]["count"], phase["draws"]["appkit"]["count"]),
            phase["wall_ms"].as_f64().unwrap_or(0.0) / 1_000.0,
            first,
        );
    }
    println!();
    println!(
        "{:<17} {:>8} {:>9} {:>9} {:>6} {:>9} {:>9} {:>9} {:>9}",
        "stability", "drift px", "drifting", "max pt", "lost", "in-drift", "in-max", "pop rows", "pop objs"
    );
    for phase in phases {
        let Some(stability) = phase.get("stability") else { continue };
        println!(
            "{:<17} {:>8} {:>9} {:>9.1} {:>6} {:>9} {:>9.1} {:>9} {:>9}",
            phase["name"].as_str().unwrap_or("?"),
            stability["drift"]["measured"],
            stability["drift"]["drifting"],
            stability["drift"]["max_pt"].as_f64().unwrap_or(0.0),
            stability["drift"]["lost"],
            stability["input_drift"]["drifting"],
            stability["input_drift"]["max_pt"].as_f64().unwrap_or(0.0),
            stability["pop_in"]["row_passes"],
            stability["pop_in"]["object_passes"],
        );
    }
    println!();
    println!(
        "{:<17} {:>8} {:>9} {:>6} {:>6} {:>9} {:>8} | {:>8} {:>9} {:>6} {:>9} | {:>8} {:>9} {:>6} {:>9}",
        "scroll bar",
        "passes",
        "max pt",
        ">1pt",
        ">2pt",
        "total pt",
        "prop chg",
        "content",
        "max pt",
        ">2pt",
        "total pt",
        "scrolled",
        "excess",
        ">2pt",
        "total pt"
    );
    let count = |value: &Value| value.as_u64().unwrap_or(0);
    let points = |value: &Value| value.as_f64().unwrap_or(0.0);
    for phase in phases {
        let Some(bar) = phase.get("stability").and_then(|stability| stability.get("scroll_bar")) else { continue };
        let (only, changed, excess) = (&bar["measurements_only"], &bar["content_changed"], &bar["scrolled_excess"]);
        println!(
            "{:<17} {:>8} {:>9.1} {:>6} {:>6} {:>9.1} {:>8} | {:>8} {:>9.1} {:>6} {:>9.1} | {:>8} {:>9.1} {:>6} {:>9.1}",
            phase["name"].as_str().unwrap_or("?"),
            count(&only["passes"]),
            points(&only["max_jump_pt"]),
            count(&only["over_1pt"]),
            count(&only["over_2pt"]),
            points(&only["total_pt"]),
            count(&only["proportion_changes"]),
            count(&changed["passes"]),
            points(&changed["max_jump_pt"]),
            count(&changed["over_2pt"]),
            points(&changed["total_pt"]),
            count(&excess["passes"]),
            points(&excess["max_pt"]),
            count(&excess["over_2pt"]),
            points(&excess["total_pt"]),
        );
    }
    println!();
    print!("{:<17}", "span totals (ms)");
    for (_, label) in TABLE_SPANS {
        print!(" {label:>9}");
    }
    println!();
    for phase in phases {
        print!("{:<17}", phase["name"].as_str().unwrap_or("?"));
        for (span, _) in TABLE_SPANS {
            let total = phase["spans"].get(span).and_then(|span| span["total_us"].as_u64());
            print!(" {:>9}", total.map_or("-".to_owned(), |us| format!("{:.1}", us as f64 / 1_000.0)));
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_and_default_to_the_stress_document_and_the_chat_inputs() {
        let args = |list: &[&str]| list.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        assert!(parse_mode(&args(&["omperor"])).unwrap().is_none());
        assert!(matches!(
            parse_mode(&args(&["omperor", "--markdown-bench", "out.json"])).unwrap(),
            Some(Mode::Bench { out }) if out == Path::new("out.json")
        ));
        assert!(parse_mode(&args(&["omperor", "--markdown-bench"])).is_err());
        let Some(Mode::Dump { out_dir, inputs }) = parse_mode(&args(&["omperor", "--markdown-dump", "dump"])).unwrap() else {
            panic!("a dump");
        };
        assert_eq!(out_dir, Path::new("dump"));
        let names: Vec<&str> = inputs.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "markdown-stress",
                "chat-answer",
                "chat-review",
                "chat-explainer",
                "chat-edge",
                "chat-long",
                "corpus-status",
                "corpus-agent",
                "markdown-stress-2x",
                "chat-long-stopped"
            ]
        );
        // The long inputs are longer than one view, and the stopped one ends
        // inside its diagram.
        for (name, text) in &inputs[5..] {
            assert!(text.len() > crate::transcript_parse::WHOLE_LIMIT, "{name}");
        }
        assert!(inputs[9].1.trim_end().ends_with("[Consumers]"));
        assert_eq!(inputs[0].1, mock_inference::STRESS_DOCUMENT);
        assert!(parse_mode(&args(&["omperor", "--markdown-dump", "dump", "/no/such/file.md"])).is_err());
    }

    #[test]
    fn the_real_home_comes_from_the_user_database() {
        // A wrong `struct passwd` layout reads another field (the GECOS name,
        // say) as the home; libc supplies the platform's actual layout.
        let home = real_home().expect("the user has a home");
        assert!(home.is_absolute(), "{home:?}");
        assert!(home.is_dir(), "{home:?}");
        #[cfg(target_os = "macos")]
        assert!(home.join("Library").is_dir(), "{home:?}");
    }

    #[test]
    fn percentiles_are_nearest_rank_and_steps_see_their_own_passes() {
        let values: Vec<u32> = (1..=100).collect();
        assert_eq!(percentile(&values, 0.5), 50);
        assert_eq!(percentile(&values, 0.99), 99);
        assert_eq!(percentile(&values, 1.0), 100);
        assert_eq!(percentile(&[7], 0.9), 7);
        assert_eq!(percentile(&[], 0.9), 0);
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let pass = |started_ms: u64, us: u32| frame_budget::Pass {
            started: at(started_ms),
            us,
            spans: vec![("gui.tick", us)],
            overflow: Vec::new(),
        };
        let run = PhaseRun {
            name: "resize",
            started: start,
            t0: start,
            draws: vec![
                Draw { at: at(1), us: 500, hash: Some(1) },
                Draw { at: at(20), us: 500, hash: Some(2) },
                Draw { at: at(40), us: 500, hash: Some(2) },
            ],
            appkit_draws: vec![(at(0), 300)],
            invalidated_passes: 1,
            hash_draws: true,
            turn_ended: None,
            steps: vec![("a".into(), at(0)), ("b".into(), at(10))],
            notes: Map::new(),
            stability: None,
        };
        // The pass that acts for step b starts before its mark and ends after.
        let passes = [pass(0, 2_000), pass(9, 3_000), pass(30, 1_000)];
        let report = summarize(&run, &passes);
        assert_eq!(report["passes"], 3);
        assert_eq!(report["steps"][0]["passes"], 1);
        assert_eq!(report["steps"][1]["passes"], 2);
        assert_eq!(report["steps"][1]["first_pass_us"], 3_000);
        assert_eq!(report["spans"]["gui.tick"]["count"], 3);
        assert_eq!(report["frames_until_complete"], 2);
        assert!((report["first_complete_frame_ms"].as_f64().unwrap() - 20.5).abs() < 0.01);
        // The first draw is whichever ended first, AppKit's included.
        assert!((report["first_draw_ms"].as_f64().unwrap() - 0.3).abs() < 0.01);
        assert_eq!(report["draws"]["appkit"]["count"], 1);
    }
}
