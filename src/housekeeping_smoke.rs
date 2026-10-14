//! `--housekeeping-smoke`: drives deleting, archiving, restoring, the
//! empty-session sweep, and the agent's `omperor_` tools through the real
//! main-thread paths, headless, in a scratch home, and says what happened
//! on stderr. The window stays off every display and the app is never
//! activated. Refuses to run unless the home holds [`MARKER`], so it can
//! never touch real data.

use super::*;
use std::time::{Duration, Instant};

const FLAG: &str = "--housekeeping-smoke";
const MARKER: &str = ".omperor-housekeeping-smoke-home";
const PHASE_LIMIT: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub(super) struct Smoke {
    phase: u32,
    phase_started: Option<Instant>,
    agent: Option<live_sessions::SessionKey>,
    drafts: Vec<live_sessions::SessionKey>,
    deleted: Option<String>,
    archived_project: Option<String>,
    archived_sessions: Vec<String>,
    empties: Vec<String>,
}

pub(super) fn requested() -> bool {
    std::env::args().any(|argument| argument == FLAG)
}

/// Checked once at launch, before anything is loaded.
pub(super) fn refuse_unless_scratch_home() {
    if !requested() {
        return;
    }
    let marked = preferences::database_path()
        .ok()
        .and_then(|database| database.parent().map(|directory| directory.join(MARKER)))
        .is_some_and(|marker| marker.is_file());
    if !marked {
        eprintln!(
            "housekeeping-smoke refused: the home has no {MARKER}; it only runs in a scratch home"
        );
        std::process::exit(2);
    }
}

/// Places the window off every display without ordering it in.
pub(super) fn hide(window: &NSWindow) {
    window.setFrame_display(
        NSRect::new(
            NSPoint::new(-30_000.0, -30_000.0),
            NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        ),
        false,
    );
}

fn fail(message: &str) -> ! {
    eprintln!("housekeeping-smoke FAIL {message}");
    std::process::exit(1);
}

/// The first button titled `title` in `view` or beneath it.
fn find_button(view: &NSView, title: &str) -> Option<Retained<NSButton>> {
    let subviews = view.subviews();
    for index in 0..subviews.count() {
        let child = subviews.objectAtIndex(index);
        if let Some(button) = child.downcast_ref::<NSButton>()
            && button.title().to_string() == title
        {
            return Some(button.retain());
        }
        if let Some(found) = find_button(&child, title) {
            return Some(found);
        }
    }
    None
}

impl AppDelegate {
    /// Confirms the sheet on the main window whose first button is
    /// `button`, as a click would. `false` until it is up.
    fn smoke_confirm_sheet(&self, button: &str) -> bool {
        let Some(window) = self.ivars().window.get() else {
            return false;
        };
        let Some(sheet) = window.attachedSheet() else {
            return false;
        };
        let title = find_button(
            &sheet
                .contentView()
                .unwrap_or_else(|| fail("a sheet without content")),
            button,
        );
        if title.is_none() {
            fail(&format!("the sheet up is not the one with {button}"));
        }
        eprintln!("housekeeping-smoke confirmed the {button} sheet");
        window.endSheet_returnCode(&sheet, NSAlertFirstButtonReturn);
        true
    }

    fn smoke_conversations(&self) -> Vec<session::SessionSummary> {
        self.ivars()
            .sessions
            .borrow()
            .iter()
            .filter(|summary| {
                summary.is_omperor_owned()
                    && !summary.is_mock()
                    && !summary.id.starts_with("draft-")
            })
            .cloned()
            .collect()
    }

    fn smoke_journal_exists(&self, id: &str) -> bool {
        self.ivars()
            .omperor_sessions_directory
            .get()
            .is_some_and(|directory| housekeeping::journal_path(directory, id).exists())
    }

    pub(super) fn poll_housekeeping_smoke(&self) {
        if !requested() || self.ivars().termination_pending.get() {
            return;
        }
        let mut smoke = self.ivars().housekeeping_smoke.borrow_mut();
        let smoke = smoke.get_or_insert_with(Smoke::default);
        let started = *smoke.phase_started.get_or_insert_with(Instant::now);
        if started.elapsed() > PHASE_LIMIT {
            fail(&format!(
                "phase {} timed out; status: {}",
                smoke.phase,
                self.session().chat_status_text.borrow()
            ));
        }
        let advance = |smoke: &mut Smoke| {
            smoke.phase += 1;
            smoke.phase_started = Some(Instant::now());
        };
        let idle = self.ivars().removals.borrow().is_empty()
            && self.ivars().archive_operations.borrow().is_empty()
            && !self.ivars().sweeping.get();
        match smoke.phase {
            // Discovery has listed the seeded sessions.
            0 => {
                let conversations = self.smoke_conversations();
                if conversations.len() < 9
                    || self.ivars().projects.borrow().is_empty()
                    || !self.ivars().session_views_loaded.get()
                {
                    return;
                }
                if let Err(error) = self.smoke_check_shared_worktree_plan(&conversations[..2]) {
                    fail(&error);
                }
                eprintln!("housekeeping-smoke PASS refused sessions keep their shared worktree");
                let projects = self.projects_answer(self.session().key);
                eprintln!(
                    "housekeeping-smoke listed conversations={} projects={}",
                    conversations.len(),
                    projects["projects"]
                );
                let directory = self.session().current_project.borrow().clone();
                // The agent's session, on the mock backend.
                let agent = self.ivars().live_sessions.borrow_mut().create();
                agent.current_project.replace(directory.clone());
                self.launch_session_runtime(&agent, None, omp_runtime::SessionStartup::Fresh);
                smoke.agent = Some(agent.key);
                // Two drafts with something in them, for the agent to clear.
                for text in ["a draft to clear", "another draft"] {
                    let draft = self.ivars().live_sessions.borrow_mut().create();
                    draft.current_project.replace(directory.clone());
                    draft.draft.replace(text.into());
                    smoke.drafts.push(draft.key);
                }
                self.ivars().live_sessions.borrow_mut().select(agent.key);
                self.render_selected_binding();
                advance(smoke);
            }
            // `/omperor tidy up…` once the agent's session is ready.
            1 => {
                let agent = self.session();
                if Some(agent.key) != smoke.agent || !agent.runtime_ready.get() {
                    return;
                }
                if let Some(input) = self.ivars().input.get() {
                    input.setStringValue(ns_string!("/omperor tidy up my Omperor workspace"));
                    // SAFETY: the delegate's sendMessage: action exists.
                    unsafe {
                        let _: () = msg_send![self, sendMessage: &**input];
                    }
                }
                advance(smoke);
            }
            // The agent asks to delete the drafts and mock sessions: the
            // sheet says what and why, and the user confirms.
            2 => {
                let confirmation = self
                    .ivars()
                    .tool_confirmations
                    .borrow()
                    .values()
                    .next()
                    .cloned();
                let Some(alert) = confirmation else { return };
                let (title, detail) = (
                    alert.messageText().to_string(),
                    alert.informativeText().to_string(),
                );
                eprintln!("housekeeping-smoke sheet title={title:?}");
                eprintln!("housekeeping-smoke sheet detail={detail:?}");
                if !title.contains("wants to delete") || !detail.contains("Draft: a draft to clear")
                {
                    fail("the confirmation does not name the drafts");
                }
                if let Some(window) = self.ivars().window.get() {
                    window.endSheet_returnCode(&alert.window(), NSAlertFirstButtonReturn);
                }
                advance(smoke);
            }
            // The turn finishes with the drafts gone and every conversation kept.
            3 => {
                let agent = self.session();
                if agent.chat_busy.get() || !idle {
                    return;
                }
                let live = self.ivars().live_sessions.borrow();
                if smoke.drafts.iter().any(|key| live.get(*key).is_some()) {
                    fail("the drafts the agent deleted are still listed");
                }
                drop(live);
                let answer = {
                    let conversation = agent.conversation.borrow();
                    (0..conversation.len())
                        .rev()
                        .filter_map(|index| conversation.message(index))
                        .find(|message| message.kind == chat::TranscriptMessageKind::Assistant)
                        .map(|message| message.body)
                        .unwrap_or_default()
                };
                eprintln!(
                    "housekeeping-smoke agent_answer={answer:?} conversations={}",
                    self.smoke_conversations().len()
                );
                if self.smoke_conversations().len() < 9 {
                    fail("the agent's cleanup removed a conversation");
                }
                advance(smoke);
            }
            // Delete one conversation from its sidebar menu, confirming the sheet.
            4 => {
                let target = self
                    .smoke_conversations()
                    .into_iter()
                    .find(|summary| {
                        summary
                            .cwd
                            .as_deref()
                            .is_some_and(|cwd| cwd.ends_with("omperor"))
                    })
                    .unwrap_or_else(|| fail("no conversation to delete"));
                let menu = self.session_context_menu(0, &target);
                let item = menu
                    .itemWithTitle(ns_string!("Delete Session…"))
                    .unwrap_or_else(|| fail("the session menu has no Delete Session…"));
                eprintln!(
                    "housekeeping-smoke deleting id={} title={:?} via the session menu",
                    target.id, target.title
                );
                smoke.deleted = Some(target.id.clone());
                // SAFETY: the item's action is the delegate's deleteSidebarSession:.
                unsafe {
                    let _: () = msg_send![self, deleteSidebarSession: &*item];
                }
                advance(smoke);
            }
            5 => {
                if !self.smoke_confirm_sheet("Delete Session") {
                    return;
                }
                advance(smoke);
            }
            6 => {
                if !idle {
                    return;
                }
                let id = smoke.deleted.clone().unwrap_or_default();
                if self.smoke_journal_exists(&id)
                    || self
                        .smoke_conversations()
                        .iter()
                        .any(|summary| summary.id == id)
                {
                    fail("the deleted session is still on disk or listed");
                }
                eprintln!(
                    "housekeeping-smoke deleted status={:?}",
                    self.session().chat_status_text.borrow()
                );
                advance(smoke);
            }
            // Archive a project from its sidebar menu, confirming the sheet.
            7 => {
                let project = self
                    .ivars()
                    .projects
                    .borrow()
                    .iter()
                    .find(|project| project.name == "omperor")
                    .cloned()
                    .unwrap_or_else(|| fail("no project to archive"));
                let records = self.ivars().session_records.borrow().clone();
                let projects = self.ivars().projects.borrow().clone();
                smoke.archived_sessions = self
                    .smoke_conversations()
                    .iter()
                    .filter(|summary| {
                        session_project_position(summary, &projects, &records)
                            .is_some_and(|position| projects[position].id == project.id)
                    })
                    .map(|summary| summary.id.clone())
                    .collect();
                eprintln!(
                    "housekeeping-smoke archiving project={:?} sessions={} via the project menu",
                    project.name,
                    smoke.archived_sessions.len()
                );
                smoke.archived_project = Some(project.id.clone());
                let menu = self.project_context_menu(&project.id);
                let item = menu
                    .itemWithTitle(ns_string!("Archive Project…"))
                    .unwrap_or_else(|| fail("the project menu has no Archive Project…"));
                // SAFETY: the item's action is the delegate's archiveSidebarProject:.
                unsafe {
                    let _: () = msg_send![self, archiveSidebarProject: &*item];
                }
                advance(smoke);
            }
            8 => {
                if !self.smoke_confirm_sheet("Archive Project") {
                    return;
                }
                advance(smoke);
            }
            9 => {
                if !idle {
                    return;
                }
                let project = smoke.archived_project.clone().unwrap_or_default();
                if self.project_by_id(&project).is_some() {
                    fail("the archived project is still listed");
                }
                if smoke
                    .archived_sessions
                    .iter()
                    .any(|id| self.smoke_journal_exists(id))
                {
                    fail("an archived session's journal is still in the sessions directory");
                }
                // The Settings window, built but never shown.
                if self.ivars().settings_ui.get().is_none() {
                    let settings = self.build_settings_window(self.mtm());
                    hide(&settings.window);
                    let _ = self.ivars().settings_ui.set(settings);
                }
                self.sync_settings_ui();
                self.show_settings_section(SETTINGS_ARCHIVES_PAGE);
                advance(smoke);
            }
            // The Archives page lists the archive; restore it from its button.
            10 => {
                let listing = match self.ivars().archives.borrow().as_ref() {
                    Some(Ok(listings)) => listings.first().cloned(),
                    Some(Err(error)) => fail(&format!("archives could not be listed: {error}")),
                    None => return,
                };
                let Some(listing) = listing else {
                    fail("the archive is not listed")
                };
                let manifest = &listing.manifest;
                eprintln!(
                    "housekeeping-smoke archive id={} sessions={} bytes={} compressed={} blobs={} path={}",
                    manifest.id,
                    manifest.session_count(),
                    manifest.bytes,
                    manifest.compressed_bytes,
                    manifest.blobs,
                    listing.path.display()
                );
                let settings = self
                    .ivars()
                    .settings_ui
                    .get()
                    .unwrap_or_else(|| fail("no Settings window"));
                let restore = find_button(&settings.archives_content, "Restore")
                    .unwrap_or_else(|| fail("the Archives page has no Restore button"));
                find_button(&settings.archives_content, "Delete…")
                    .unwrap_or_else(|| fail("the Archives page has no Delete button"));
                find_button(&settings.pages[SETTINGS_PROJECTS_PAGE], "Archive…")
                    .unwrap_or_else(|| fail("Settings › Projects has no Archive button"));
                // SAFETY: the button targets the delegate's settingsRestoreArchive:.
                unsafe { restore.performClick(None) };
                advance(smoke);
            }
            // The project and its sessions come back.
            11 => {
                let project = smoke.archived_project.clone().unwrap_or_default();
                if !idle || self.project_by_id(&project).is_none() {
                    return;
                }
                let listed = self.smoke_conversations();
                if smoke
                    .archived_sessions
                    .iter()
                    .any(|id| !listed.iter().any(|summary| &summary.id == id))
                {
                    return;
                }
                if smoke
                    .archived_sessions
                    .iter()
                    .any(|id| !self.smoke_journal_exists(id))
                {
                    fail("a restored session's journal is missing");
                }
                eprintln!(
                    "housekeeping-smoke restored sessions={} status={:?}",
                    smoke.archived_sessions.len(),
                    self.session().chat_status_text.borrow()
                );
                advance(smoke);
            }
            // Sweep the sessions that never held a conversation, from the
            // Sessions & Data page.
            12 => {
                let Some(directory) = self.ivars().omperor_sessions_directory.get().cloned() else {
                    return;
                };
                smoke.empties = [
                    "01M1Q6V3ABZAG6PT3TRET3N7J2",
                    "01M1Q8QBP9EMF1725YB1PDFNNQ",
                    "01M1QA0RJTF3VFWXV59SJ8B1BC",
                    "01M1QA5R4ERBC8C5SAPA1VPRED",
                ]
                .into_iter()
                .filter(|id| housekeeping::journal_path(&directory, id).exists())
                .map(str::to_owned)
                .collect();
                let settings = self
                    .ivars()
                    .settings_ui
                    .get()
                    .unwrap_or_else(|| fail("no Settings window"));
                let sweep = find_button(&settings.pages[4], "Remove Empty Sessions")
                    .unwrap_or_else(|| fail("Sessions & Data has no Remove Empty Sessions button"));
                // SAFETY: the button targets the delegate's removeEmptySessions:.
                unsafe { sweep.performClick(None) };
                advance(smoke);
            }
            13 => {
                if !idle {
                    return;
                }
                if smoke.empties.iter().any(|id| self.smoke_journal_exists(id)) {
                    fail("an empty session survived the sweep");
                }
                if self
                    .smoke_conversations()
                    .iter()
                    .any(|summary| !self.smoke_journal_exists(&summary.id))
                {
                    fail("a listed conversation lost its journal");
                }
                eprintln!(
                    "housekeeping-smoke swept empties={} status={:?}",
                    smoke.empties.len(),
                    self.session().chat_status_text.borrow()
                );
                eprintln!("housekeeping-smoke pass");
                advance(smoke);
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
            _ => {}
        }
    }
}
