//! The app driven as a person would, on the kit's stand-in platform: the
//! mock backend (Developer → Mock) answers, so no provider, account or OMP
//! child is involved. With `OMPEROR_SHOTS=<dir>` set, each test also saves
//! pictures of what it drove the window to.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aizome_kit::sidestep_ui::{Modifiers, NamedKey};
use aizome_kit::testing::Harness;
use aizome_kit::{Program, WindowKey};

use super::{App, Msg, Panel, Pane};
use crate::preferences::{self, Preferences};

/// The app reads its state root and project from the environment: one
/// app test at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Run {
    h: Harness<App>,
    w: WindowKey,
    root: PathBuf,
}

impl Drop for Run {
    fn drop(&mut self) {
        for session in self.h.program().live.entries() {
            if let Some(runtime) = session.omp_runtime.get() {
                runtime.begin_shutdown();
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let _ = std::process::Command::new(crate::workspace::git())
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .output();
}

/// Start the app on a scratch state root and a small git project, the mock
/// backend answering, and wait for its session to be ready.
fn boot(label: &str) -> Run {
    let root = std::env::temp_dir().join(format!("omperor-app-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    boot_in(root)
}

/// [`boot`] in `root`, which may already hold saved state, and a project
/// (else one is made: a file committed, then changed, and one new).
fn boot_in(root: PathBuf) -> Run {
    let (state, project) = (root.join("state"), root.join("project"));
    if !project.exists() {
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&project, &["init", "-q"]);
        git(&project, &["add", "-A"]);
        git(&project, &["commit", "-qm", "init"]);
        std::fs::write(project.join("src/main.rs"), "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
        std::fs::write(project.join("notes.md"), "# Notes\n").unwrap();
    }
    // SAFETY: the app tests hold `ONE_AT_A_TIME`; nothing else in the test
    // binary reads these variables.
    unsafe {
        std::env::set_var("OMPEROR_STATE_DIR", &state);
        std::env::set_var("OMPEROR_PROJECT", &project);
    }
    let preferences =
        Preferences { developer_mode: true, inference_backend: crate::mock_inference::Backend::Mock, ..Preferences::default() };
    preferences::store(&preferences).expect("preferences");
    let h = Harness::new(App::new(None));
    let w = h.window();
    let mut run = Run { h, w, root };
    run.until("the session is ready", |app| app.booted && app.session().runtime_ready.get());
    run
}

impl Run {
    /// Let the workers, the runtime and the timers run until `done` holds.
    fn until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        self.until_within(Duration::from_secs(20), what, done);
    }

    fn until_within(&mut self, limit: Duration, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + limit;
        while !done(self.h.program()) {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(15));
            self.h.send(Msg::Wake);
            self.h.advance(Duration::from_millis(40));
        }
    }

    fn texts(&self) -> String {
        self.h.texts(self.w).join("\n")
    }

    fn shot(&mut self, name: &str) {
        if let Some(dir) = std::env::var_os("OMPEROR_SHOTS") {
            let _ = std::fs::create_dir_all(&dir);
            let window = self.w;
            self.h.save_png(window, Path::new(&dir).join(format!("{name}.png")), 1.0);
        }
    }

    fn prompt(&mut self, text: &str) {
        let w = self.w;
        self.h.type_text(w, text);
        let view = self.h.program().view_of(self.h.program().session().key).unwrap();
        assert_eq!(view.editor.text(), text, "typed into the composer");
        assert!(view.picker.is_none(), "no picker open: {:?}", view.picker);
        self.h.named(w, NamedKey::Enter, Modifiers::NONE);
    }
}

fn idle_with_answer(app: &App) -> bool {
    let session = app.session();
    !session.chat_busy.get()
        && session.pending_prompt.borrow().is_none()
        && (0..session.conversation.borrow().len()).any(|i| {
            session
                .conversation
                .borrow()
                .message(i)
                .is_some_and(|m| m.kind == crate::chat::TranscriptMessageKind::Assistant && !m.body.is_empty())
        })
}

#[test]
fn a_turn_streams_its_tools_and_answer_and_the_composer_comes_back() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = boot("turn");
    run.shot("welcome");
    assert!(run.texts().contains("What would you like to work on?"), "{}", run.texts());
    // The showcase, written at once rather than at a reader's pace.
    const PROMPT: &str = "/mock scenario=markdown rate=0 ttft=0";
    run.prompt(PROMPT);
    run.until("the prompt is in the transcript", |app| {
        app.session().conversation.borrow().message(0).is_some_and(|m| m.kind == crate::chat::TranscriptMessageKind::User)
    });
    assert!(run.texts().contains(PROMPT), "{}", run.texts());
    run.until("the turn is done", idle_with_answer);
    // The parse lands on a worker a moment after the text.
    run.until("the answer is parsed", |app| {
        app.view_of(app.session().key).is_some_and(|v| v.rows.iter().all(|r| !r.parsing))
    });
    let texts = run.texts();
    assert!(texts.contains("Markdown stress test"), "{texts}");
    assert!(texts.contains("Callouts"), "{texts}");
    assert!(run.h.program().view_of(run.h.program().session().key).unwrap().editor.text().is_empty());
    // The answer's start, scrolled back to.
    let w = run.w;
    run.h.scroll(w, aizome_kit::Point::new(700.0, 300.0), aizome_kit::Vec2::new(0.0, -1_000_000.0));
    run.shot("markdown");
    // Its math and its diagrams, each section scrolled to the top.
    // (The transcript makes only the rows it shows: scroll until it's there.)
    for (heading, name) in [("Math", "math"), ("Diagrams", "diagrams")] {
        let mut found = run.h.find(w, heading);
        for _ in 0..80 {
            if found.is_some_and(|at| at.y0 > 0.0) {
                break;
            }
            run.h.scroll(w, aizome_kit::Point::new(700.0, 300.0), aizome_kit::Vec2::new(0.0, 300.0));
            run.h.advance(Duration::from_millis(20));
            found = run.h.find(w, heading);
        }
        let at = found.unwrap_or_else(|| panic!("the {heading} heading"));
        run.h.scroll(w, aizome_kit::Point::new(700.0, 300.0), aizome_kit::Vec2::new(0.0, at.y0 - 70.0));
        run.h.advance(Duration::from_millis(20));
        run.shot(name);
        if name == "math" {
            // The formulas in a sentence copy as their LaTeX.
            let line = run.h.texts(w).into_iter().find(|t| t.contains("share the line")).expect("the sentence");
            assert!(line.contains('\u{FFFC}'), "set in the line: {line}");
            let at = run.h.find(w, &line).expect("the sentence's place");
            run.h.focus_window(w, true);
            run.h.press(w, at.center(), aizome_kit::sidestep_ui::PointerButton::Left, 3);
            run.h.release(w, at.center(), aizome_kit::sidestep_ui::PointerButton::Left);
            run.h.command(w, 'c');
            let copied = run.h.platform.clipboard.clone().unwrap_or_default();
            assert!(copied.contains("Inline $e^{i\\pi} + 1 = 0$ and $\\sum_{k=1}^{n} k"), "{copied}");
        }
    }
    // ↑ in the empty composer brings the prompt back.
    let composer = run.h.find_id(w, "composer").expect("the composer");
    run.h.click(w, composer.center());
    run.h.named(w, NamedKey::ArrowUp, Modifiers::NONE);
    assert_eq!(run.h.program().view_of(run.h.program().session().key).unwrap().editor.text(), PROMPT);
}

#[test]
fn sub_agents_show_in_the_agents_panel_and_open_their_record() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = boot("agents");
    run.prompt("Spawn two sub-agents to review the change");
    let w = run.w;
    run.h.command(w, '2');
    assert_eq!(run.h.program().view_of(run.h.program().session().key).and_then(|v| v.panel()), Some(Panel::Agents));
    run.until("the sub-agents have started", |app| app.session().subagents.borrow().len() == 2);
    assert!(run.texts().contains("RUNNING"), "{}", run.texts());
    assert!(run.h.click_text(w, "scout-layout"), "the agent's row is there to click");
    run.until("the agent is selected", |app| {
        app.view_of(app.session().key).is_some_and(|v| v.view.selected.is_some())
    });
    assert!(run.texts().contains("Discuss"), "{}", run.texts());
    run.shot("subagents");
    // Discuss quotes the agent into the draft, sending nothing.
    assert!(run.h.click_text(w, "Discuss"));
    let draft = run.h.program().view_of(run.h.program().session().key).unwrap().editor.text();
    assert!(draft.starts_with("From scout"), "{draft}");
    // The mock's board empties when its turn ends.
    run.until("the turn is done", idle_with_answer);
}

#[test]
fn the_changes_panel_lists_the_projects_uncommitted_files_and_their_diff() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = boot("changes");
    let w = run.w;
    run.h.command(w, '1');
    run.until("the folder is read", |app| app.session().workspace_snapshot.borrow().is_some());
    let texts = run.texts();
    assert!(texts.contains("main.rs") && texts.contains("notes.md"), "{texts}");
    assert!(texts.contains("2 files changed"), "{texts}");
    assert!(run.h.click_text(w, "main.rs"));
    run.until("the diff is read", |app| {
        app.view_of(app.session().key).is_some_and(|v| matches!(v.diff, Some((_, super::Diff::Ready(_)))))
    });
    assert!(run.texts().contains("println"), "{}", run.texts());
    run.shot("changes");
}

#[test]
fn settings_open_on_their_pages() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = boot("settings");
    run.h.send(Msg::OpenSettings(Pane::Accounts));
    let settings = run.h.program().settings.expect("the Settings window");
    let texts = run.h.texts(settings).join("\n");
    assert!(texts.contains("Accounts") && texts.contains("Claude subscription"), "{texts}");
    if let Some(dir) = std::env::var_os("OMPEROR_SHOTS") {
        run.h.save_png(settings, Path::new(&dir).join("settings-accounts.png"), 1.0);
    }
    run.h.send(Msg::ShowPane(Pane::General));
    let texts = run.h.texts(settings).join("\n");
    assert!(texts.contains("Text size") && texts.contains("Editor"), "{texts}");
    if let Some(dir) = std::env::var_os("OMPEROR_SHOTS") {
        run.h.save_png(settings, Path::new(&dir).join("settings-general.png"), 1.0);
    }
    run.h.send(Msg::ShowPane(Pane::Data));
    let texts = run.h.texts(settings).join("\n");
    assert!(texts.contains("Remove Empty Sessions"), "{texts}");
    // Two turns of one session in the log: responses, tools, one failing.
    let log = run.h.program().sessions_dir.clone().expect("the sessions folder").join(crate::metrics::FILE_NAME);
    let at = |offset: u64| crate::rules::wall_clock_ms() - 3_600_000 + offset;
    let records = [
        format!(r#"{{"k":"inference","at":{},"session":"s1","turn":{},"provider":"anthropic","model":"claude-opus-5-5","tokens_in":900,"tokens_out":510,"ttft_ms":1000,"last_token_ms":6100,"duration_ms":6100}}"#, at(6_100), at(0)),
        format!(r#"{{"k":"tool","at":{},"session":"s1","turn":{},"tool":"bash","duration_ms":4000}}"#, at(10_500), at(0)),
        format!(r#"{{"k":"tool","at":{},"session":"s1","turn":{},"tool":"read","duration_ms":120}}"#, at(11_200), at(0)),
        format!(r#"{{"k":"inference","at":{},"session":"s1","turn":{},"provider":"anthropic","model":"claude-opus-5-5","tokens_in":1400,"tokens_out":610,"ttft_ms":1100,"last_token_ms":7200,"duration_ms":7200,"fast":true}}"#, at(18_000), at(0)),
        format!(r#"{{"k":"turn","at":{},"session":"s1","turn":{},"duration_ms":20000,"outcome":"completed"}}"#, at(20_000), at(0)),
        format!(r#"{{"k":"tool","at":{},"session":"s1","turn":{},"tool":"bash","duration_ms":900,"error":true}}"#, at(31_000), at(25_000)),
        format!(r#"{{"k":"inference","at":{},"session":"s1","turn":{},"provider":"openai-codex","model":"gpt-5.6-sol","tokens_in":800,"tokens_out":300,"ttft_ms":700,"last_token_ms":3000,"duration_ms":3000}}"#, at(34_000), at(25_000)),
        format!(r#"{{"k":"turn","at":{},"session":"s1","turn":{},"duration_ms":10000,"outcome":"completed"}}"#, at(35_000), at(25_000)),
    ];
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, records.join("\n") + "\n").unwrap();
    run.h.send(Msg::ShowPane(Pane::Metrics));
    run.until("the metrics log is read", |app| app.metrics_page.summary.is_some());
    let texts = run.h.texts(settings).join("\n");
    assert!(texts.contains("Where the time went") && texts.contains("bash"), "{texts}");
    if let Some(dir) = std::env::var_os("OMPEROR_SHOTS") {
        run.h.save_png(settings, Path::new(&dir).join("settings-metrics.png"), 1.0);
        run.h.send(Msg::ShowPane(Pane::Data));
        run.h.save_png(settings, Path::new(&dir).join("settings-data.png"), 1.0);
    }
    assert_eq!(run.h.program().info().app_id, crate::APP_ID);
}

/// A saved OMP journal with one turn: a prompt, a reply in Markdown, and a
/// `read` call with its result, all in `project`.
fn write_saved_session(sessions: &Path, project: &Path) -> String {
    use omp_journal::{EntryDraft, Journal, Kind, KindName};
    use serde_json::json;
    let draft = |kind, data: serde_json::Value, by| EntryDraft {
        kind: Kind::known(kind),
        by,
        prior: None,
        label: None,
        data: data.to_string().into(),
    };
    let id = "01M4WCRZ12FTJRSYETKAGAXWYW";
    std::fs::create_dir_all(sessions).unwrap();
    let mut journal = Journal::create(sessions.join(format!("{id}.oms"))).expect("journal");
    let genesis = journal
        .append(draft(KindName::Journal, json!({ "version": 1, "cwd": project, "created": "2026-10-01T00:00:00Z" }), None))
        .unwrap();
    let turn = journal.append(draft(KindName::TurnStart, json!({}), Some(genesis.id))).unwrap();
    let user = journal.append(draft(KindName::MsgUser, json!({ "text": "What does main print?" }), Some(turn.id))).unwrap();
    let assistant = journal.append(draft(KindName::MsgAssistantStart, json!({}), Some(user.id))).unwrap();
    let call = journal
        .append(draft(
            KindName::ToolCall,
            json!({ "name": "read", "rev": 1, "call_id": "call-1", "i": "Read src/main.rs", "args": { "path": "src/main.rs" } }),
            Some(assistant.id),
        ))
        .unwrap();
    journal
        .append(draft(KindName::ToolResult, json!({ "outcome": { "text": "fn main() {}" } }), Some(call.id)))
        .unwrap();
    for op in [
        json!({ "sid": 1, "op": "open", "prop": "text" }),
        json!({ "sid": 1, "op": "append", "text": "## Saved answer\n\nIt prints nothing yet:\n\n```rust\nfn main() {}\n```\n" }),
        json!({ "sid": 1, "op": "close" }),
    ] {
        journal.append(draft(KindName::Stream, op, Some(assistant.id))).unwrap();
    }
    let end = journal.append(draft(KindName::MsgAssistantEnd, json!({ "stop_reason": "stop" }), Some(assistant.id))).unwrap();
    journal
        .append(draft(
            KindName::TurnReceipt,
            json!({ "tokens_in": 4200, "tokens_out": 380, "cost_nano_usd": 21_000_000, "cache_read": 3600, "cache_write": 600, "ttft_ms": 900, "last_token_ms": 2400, "duration_ms": 2400 }),
            Some(end.id),
        ))
        .unwrap();
    id.to_owned()
}

#[test]
fn a_saved_session_is_listed_under_its_project_and_opens_with_its_history() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!("omperor-app-saved-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    let id = write_saved_session(&root.join("state/omp/sessions"), &project);
    let mut run = boot_in(root);
    run.until("the saved session is listed", |app| app.sessions.iter().any(|s| s.id == id));
    let w = run.w;
    assert!(run.h.click_text(w, "What does main print?"), "the session's row: {}", run.texts());
    run.until("its history is read", |app| {
        app.view_of(app.session().key).is_some_and(|v| {
            v.rows.iter().any(|r| r.message.body.contains("Saved answer")) && v.rows.iter().all(|r| !r.parsing)
        })
    });
    let texts = run.texts();
    assert!(texts.contains("Saved answer") && texts.contains("Read src/main.rs"), "{texts}");
    run.shot("saved");
    // Its timeline: the prompt, the response and the call, on one clock.
    run.h.command(w, '5');
    run.until("the timeline is read", |app| app.timeline.as_ref().is_some_and(|t| t.complete));
    let timeline = run.h.program().timeline.as_ref().map(|t| t.window).expect("the Timeline window");
    let texts = run.h.texts(timeline).join("\n");
    assert!(texts.contains("Read src/main.rs") && texts.contains("What does main print?"), "{texts}");
    if let Some(dir) = std::env::var_os("OMPEROR_SHOTS") {
        run.h.save_png(timeline, Path::new(&dir).join("timeline.png"), 1.0);
    }
    // Its usage, from the receipt.
    run.h.send(Msg::ShowUsage);
    run.until("the receipts are read", |app| app.usage_window.as_ref().is_some_and(|u| u.report.is_some()));
    let usage = run.h.program().usage_window.as_ref().map(|u| u.window).expect("the Usage window");
    let texts = run.h.texts(usage).join("\n");
    assert!(texts.contains("Output tokens") && texts.contains("380"), "{texts}");
    if let Some(dir) = std::env::var_os("OMPEROR_SHOTS") {
        run.h.save_png(usage, Path::new(&dir).join("usage.png"), 1.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Puts plain prompts back to the mock's ordinary scenarios.
struct EndTour;

impl Drop for EndTour {
    fn drop(&mut self) {
        crate::mock_inference::set_tour(false);
    }
}

/// The README's pictures, drawn only with `OMPEROR_SHOTS` set (`just
/// shots`): the mock's tour of a change to the demo project
/// (`tests/readme-demo`), its sub-agents at work, and the result in Night
/// and Day.
#[test]
fn readme_pictures() {
    let Some(dir) = std::env::var_os("OMPEROR_SHOTS").map(PathBuf::from) else { return };
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!("omperor-app-readme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/readme-demo");
    copy_tree(&demo.join("base"), &project);
    git(&project, &["init", "-q"]);
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-qm", "harbor"]);
    git(&project, &["apply", demo.join("work.patch").to_str().unwrap()]);
    crate::mock_inference::set_tour(true);
    let _tour = EndTour;
    let mut run = boot_in(root);
    let w = run.w;
    run.prompt(
        "Replies split CJK and emoji mid-character. Wrap them by display width, and add a way to get the last few messages for the model's context.",
    );
    run.h.command(w, '2');
    run.until_within(Duration::from_secs(60), "the scouts are out", |app| app.session().subagents.borrow().len() == 2);
    let first = run.h.program().session().subagents.borrow()[0].id.clone();
    run.h.send(Msg::SelectAgent(Some(first)));
    run.until("the scout is selected", |app| app.view_of(app.session().key).is_some_and(|v| v.view.selected.is_some()));
    run.h.save_png(w, dir.join("agents.png"), 1.0);
    run.until_within(Duration::from_secs(120), "the change is made", idle_with_answer);
    run.until("the answer is parsed", |app| app.view_of(app.session().key).is_some_and(|v| v.rows.iter().all(|r| !r.parsing)));
    run.h.command(w, '1');
    run.until("the folder is read", |app| app.session().workspace_snapshot.borrow().is_some());
    run.h.save_png(w, dir.join("main-dark.png"), 1.0);
    run.h.set_appearance(aizome_kit::theme::Appearance { theme: aizome_kit::theme::ThemeChoice::Day, ..Default::default() });
    run.h.save_png(w, dir.join("main-light.png"), 1.0);
}

#[test]
fn a_pasted_picture_is_read_and_attached_on_a_worker() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut run = boot("paste");
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(8, 6, image::Rgba([200, 60, 60, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    run.h.platform.clipboard = None;
    run.h.platform.offer = vec![("image/png".into(), png)];
    let w = run.w;
    run.h.focus_window(w, true);
    run.h.command(w, 'v');
    run.until("the picture is attached", |app| {
        let attachments = app.session().pending_attachments.borrow().items.iter().map(|item| item.result.is_some()).collect::<Vec<_>>();
        attachments == [true]
    });
    let session = run.h.program().session();
    let attachments = session.pending_attachments.borrow();
    assert_eq!(attachments.items[0].name, "Pasted image.png");
    assert!(!attachments.blocked(), "{:?}", attachments.items[0].result);
    assert!(run.h.program().view_of(session.key).unwrap().editor.text().is_empty(), "no text was pasted");
}
