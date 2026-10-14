//! `--readme-shots <directory>`: stages the README's screenshots against the
//! mock backend's tour and writes them to `<directory>` as PNGs. It runs in a
//! scratch home only (it refuses one without [`MARKER`] beside the
//! preferences), keeps every window off every display, and never activates
//! the app, so it can run while the user works.
//!
//! The project is whatever `OMPEROR_PROJECT` names; the shots look best with
//! a small git repository holding uncommitted changes to `src/chat.rs`, the
//! file the Changes panel opens.

use super::*;
use std::time::{Duration, Instant};

const FLAG: &str = "--readme-shots";
const MARKER: &str = ".omperor-readme-shots-home";
/// Far from every display, as `markdown_bench` places its window.
const OFFSCREEN: NSPoint = NSPoint::new(-30_000.0, -30_000.0);
/// How long a step may wait for what it needs before the run gives up.
const STEP_LIMIT: Duration = Duration::from_secs(90);
/// Time for background work a step starts (diffs, Markdown, usage) to land
/// and be drawn before the picture is taken.
const SETTLE: Duration = Duration::from_millis(1_800);

/// The tour's prompts: a change, a question, and a change still running when
/// the main window is shot.
const CHANGE: &str = "Replies split CJK and emoji mid-character. Wrap them by display width, and add a way to get the last few messages for the model's context.";
const QUESTION: &str = "How does the cost of wrap() grow with the length of a reply?";
const RUNNING: &str = "Add a --model flag that overrides the default model.";

#[derive(Debug, Default)]
pub(super) struct Shots {
    step: usize,
    step_started: Option<Instant>,
    /// When the current step's condition first held.
    ready_at: Option<Instant>,
    change: Option<live_sessions::SessionKey>,
    question: Option<live_sessions::SessionKey>,
    running: Option<live_sessions::SessionKey>,
    /// Sessions seen busy: a turn has finished once its session was busy
    /// and is no longer.
    seen_busy: Vec<live_sessions::SessionKey>,
    /// Whether this step has asked for the diff it waits on.
    diff_opened: bool,
}

pub(super) fn requested() -> bool {
    std::env::args().any(|argument| argument == FLAG)
}

fn directory() -> std::path::PathBuf {
    let mut arguments = std::env::args();
    arguments
        .position(|argument| argument == FLAG)
        .and_then(|_| arguments.next())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| fail("usage: --readme-shots <directory>"))
}

/// Checked once at launch, before anything is loaded. Turns the mock
/// backend's tour on.
pub(super) fn refuse_unless_scratch_home() {
    if !requested() {
        return;
    }
    let marked = preferences::database_path()
        .ok()
        .and_then(|database| database.parent().map(|directory| directory.join(MARKER)))
        .is_some_and(|marker| marker.is_file());
    if !marked {
        eprintln!("readme-shots refused: the home has no {MARKER}; it only runs in a scratch home");
        std::process::exit(2);
    }
    std::fs::create_dir_all(directory()).unwrap_or_else(|error| fail(&format!("cannot create the output directory: {error}")));
    mock_inference::start_tour();
}

/// Orders `window` in far from every display, without activating, so AppKit
/// draws it as it would on screen.
pub(super) fn place(window: &NSWindow) {
    let frame = NSRect::new(OFFSCREEN, window.frame().size);
    window.setFrame_display(frame, false);
    window.orderFrontRegardless();
    if window.frame().origin != OFFSCREEN {
        fail(&format!("AppKit moved a window to {:?}", window.frame()));
    }
}

fn fail(message: &str) -> ! {
    eprintln!("readme-shots FAIL {message}");
    std::process::exit(1);
}

impl AppDelegate {
    fn shots_entry(&self, key: Option<live_sessions::SessionKey>) -> Rc<live_sessions::LiveSession> {
        key.and_then(|key| self.ivars().live_sessions.borrow().get(key))
            .unwrap_or_else(|| fail("a tour session is gone"))
    }

    /// Types `prompt` into the composer and sends it, as the user would:
    /// in a new conversation unless `fresh` is the one already open.
    fn shots_start(&self, prompt: &str, fresh: bool) -> live_sessions::SessionKey {
        // SAFETY: newChat: and sendMessage: are implemented on this
        // delegate with the standard action signature.
        if !fresh {
            let _: () = unsafe { msg_send![self, newChat: std::ptr::null::<AnyObject>()] };
        }
        let input = self.ivars().input.get().unwrap_or_else(|| fail("no composer"));
        input.setStringValue(&NSString::from_str(prompt));
        let _: () = unsafe { msg_send![self, sendMessage: std::ptr::null::<AnyObject>()] };
        self.session().key
    }

    fn shots_select(&self, key: Option<live_sessions::SessionKey>) {
        let entry = self.shots_entry(key);
        if self.session().key == entry.key {
            return;
        }
        self.capture_selected_binding();
        self.ivars().live_sessions.borrow_mut().select(entry.key);
        self.render_selected_binding();
    }

    /// Whether `key`'s turn has run and finished.
    fn shots_finished(&self, shots: &Shots, key: Option<live_sessions::SessionKey>) -> bool {
        let entry = self.shots_entry(key);
        shots.seen_busy.contains(&entry.key) && !entry.chat_busy.get()
    }

    /// Opens `path`'s diff in the Changes panel, as a click on its row would.
    fn shots_open_change(&self, path: &str) -> bool {
        let owner = self.session();
        let Some(snapshot) = owner.workspace_snapshot.borrow().clone() else {
            return false;
        };
        let Some(index) = snapshot.changes.iter().position(|change| change.path.ends_with(path)) else {
            eprintln!("readme-shots: {path} is not among {:?}", snapshot.changes.iter().map(|change| &change.path).collect::<Vec<_>>());
            return false;
        };
        let Some(row) = self.ivars().change_tree_rows.borrow().iter().position(|row| {
            matches!(row, change_tree::TreeRow::File { change, .. } if *change == index)
        }) else {
            eprintln!("readme-shots: no row for {path} among {} rows", self.ivars().change_tree_rows.borrow().len());
            return false;
        };
        let change = snapshot.changes[index].clone();
        owner.selected_change.replace(Some(change.path.clone()));
        self.highlight_change_row(Some(row));
        self.load_change_diff(snapshot, change);
        true
    }

    /// The change's conversation beside the Changes panel, from the top.
    fn shots_show_change(&self, shots: &Shots) {
        self.shots_select(shots.change);
        self.set_workbench_visible(true);
        self.show_workbench_panel(0);
        self.request_workspace_refresh();
    }

    /// Opens `src/chat.rs` once the workspace is inspected, and holds the
    /// transcript at its top; true once the diff has loaded.
    fn shots_diff_ready(&self, shots: &mut Shots) -> bool {
        self.transcript().restore_place(None, 0.0);
        if !shots.diff_opened {
            shots.diff_opened = self.shots_open_change("src/chat.rs");
            return false;
        }
        self.session().diff_discovery.borrow().is_none()
    }

    fn shoot(&self, window: &NSWindow, name: &str) {
        let path = directory().join(format!("{name}.png"));
        attachment_input::smoke_capture_window(window, &path.to_string_lossy());
        eprintln!("readme-shots wrote {}", path.display());
    }

    pub(super) fn poll_readme_shots(&self) {
        if !requested() || self.ivars().termination_pending.get() {
            return;
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        // Taking a picture pumps the run loop, which ticks again: that tick
        // finds the step in progress and leaves it alone.
        let Ok(mut shots) = self.ivars().readme_shots.try_borrow_mut() else {
            return;
        };
        let shots = shots.get_or_insert_with(Shots::default);
        for key in [shots.change, shots.question, shots.running].into_iter().flatten() {
            if !shots.seen_busy.contains(&key) && self.shots_entry(Some(key)).chat_busy.get() {
                shots.seen_busy.push(key);
            }
        }
        let started = *shots.step_started.get_or_insert_with(Instant::now);
        if started.elapsed() > STEP_LIMIT {
            fail(&format!("step {} timed out; status: {}", shots.step, self.session().chat_status_text.borrow()));
        }
        // A step is a condition and an action. The action runs once the
        // condition has held for `settle`, then the next step begins.
        let settled = |shots: &mut Shots, ready: bool, settle: Duration| -> bool {
            if !ready {
                shots.ready_at = None;
                return false;
            }
            let at = *shots.ready_at.get_or_insert_with(Instant::now);
            at.elapsed() >= settle
        };
        let done = match shots.step {
            0 => {
                if settled(shots, true, Duration::from_millis(500)) {
                    self.apply_appearance(AppearancePreference::Dark);
                    shots.question = Some(self.shots_start(QUESTION, true));
                    shots.change = Some(self.shots_start(CHANGE, false));
                    true
                } else {
                    false
                }
            }
            // The two finished turns, then the one still running.
            1 => {
                let ready = self.shots_finished(shots, shots.change) && self.shots_finished(shots, shots.question);
                if settled(shots, ready, Duration::from_millis(300)) {
                    shots.running = Some(self.shots_start(RUNNING, false));
                    true
                } else {
                    false
                }
            }
            // The running turn's scouts in the Agents panel.
            2 => {
                let ready = self.shots_entry(shots.running).subagents.borrow().len() == 2;
                if settled(shots, ready, Duration::from_millis(1_200)) {
                    self.shots_select(shots.running);
                    self.show_workbench_panel(1);
                    true
                } else {
                    false
                }
            }
            3 => {
                if settled(shots, true, Duration::from_millis(900)) {
                    self.shoot(window, "agents");
                    self.shots_show_change(shots);
                    true
                } else {
                    false
                }
            }
            // The finished change beside its diff, while the third session
            // still runs in the sidebar.
            4 => {
                let ready = self.shots_diff_ready(shots);
                if settled(shots, ready, SETTLE) {
                    self.shoot(window, "main-dark");
                    self.shots_select(shots.question);
                    self.set_workbench_visible(false);
                    true
                } else {
                    false
                }
            }
            5 => {
                self.transcript().restore_place(None, 0.0);
                if settled(shots, true, SETTLE) {
                    self.shoot(window, "markdown");
                    self.apply_appearance(AppearancePreference::Light);
                    self.shots_show_change(shots);
                    true
                } else {
                    false
                }
            }
            6 => {
                let ready = self.shots_diff_ready(shots);
                if settled(shots, ready, SETTLE) {
                    self.shoot(window, "main-light");
                    true
                } else {
                    false
                }
            }
            _ => {
                eprintln!("readme-shots PASS");
                NSApplication::sharedApplication(self.mtm()).terminate(None);
                return;
            }
        };
        if done {
            shots.diff_opened = false;
            shots.step += 1;
            shots.step_started = None;
            shots.ready_at = None;
        }
    }
}
