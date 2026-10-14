//! Deleting sessions, archiving projects, and the Archives page: who asked
//! (the user from a menu, or the agent through an `omperor_` tool), the
//! confirmation, releasing any live session's journal, and the interface
//! catching up once a worker (`housekeeping`, `archive`) has done the work.
//! Nothing here touches the disk.

use super::*;
use objc2_app_kit::{NSModalResponse, NSModalResponseAbort};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Who asked for a removal, so the outcome goes back to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Origin {
    User,
    Agent {
        key: live_sessions::SessionKey,
        call: u64,
    },
}

#[derive(Debug)]
enum Work {
    Delete(housekeeping::Request),
    Archive(archive::Request),
}

/// A deletion or archive in flight.
#[derive(Debug)]
pub(super) struct Removal {
    job: u64,
    origin: Origin,
    /// What the user calls it: a session's title, a project's name.
    subject: String,
    /// Live sessions that must let go of their journals before the work runs.
    waiting: HashSet<live_sessions::SessionKey>,
    started: Instant,
    /// The work, until a worker takes it.
    work: Option<Work>,
    members: Vec<Member>,
    /// Sessions asked for that cannot go, and why.
    refused: Vec<Refusal>,
    /// For an archive: where the selection moves if it is in the project.
    fallback: Option<(String, PathBuf)>,
}

/// One session a removal takes: its id, its live entry if it has one, and
/// the path its saved view is kept under.
#[derive(Debug, Clone)]
struct Member {
    id: String,
    key: Option<live_sessions::SessionKey>,
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct Refusal {
    id: String,
    title: String,
    reason: String,
}

// One per removal, moved once from a worker: not worth boxing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(super) enum Outcome {
    Deleted(housekeeping::Report),
    /// With the registry as the archive left it, read on the worker.
    Archived(
        Result<archive::Archived, String>,
        Option<preferences::Registry>,
    ),
}

struct ArchivePlan {
    project: Project,
    members: Vec<(archive::Member, Member)>,
    worktrees: Vec<PathBuf>,
    fallback: Option<(String, PathBuf)>,
}

/// How long a removal waits for live sessions to let go of their journals
/// before running anyway: the worker's lock check refuses any journal that
/// is still held, so waiting longer only keeps the spinner up.
const RELEASE_WAIT: Duration = Duration::from_secs(20);
/// Titles a confirmation lists before "and N more".
const LISTED_TITLES: usize = 12;

fn draft_key_of(id: &str) -> Option<u64> {
    id.strip_prefix("draft-").and_then(|key| key.parse().ok())
}

fn plural(count: usize) -> &'static str {
    housekeeping::plural(count)
}

/// Titles as a list for a confirmation, the first few and a count.
fn title_list<'a>(titles: impl Iterator<Item = &'a str>) -> String {
    let titles: Vec<&str> = titles.collect();
    let mut lines: Vec<String> = titles
        .iter()
        .take(LISTED_TITLES)
        .map(|title| format!("• {title}"))
        .collect();
    if titles.len() > LISTED_TITLES {
        lines.push(format!("…and {} more", titles.len() - LISTED_TITLES));
    }
    lines.join("\n")
}

fn deletion_detail(targets: &[&housekeeping::Target]) -> String {
    let conversations = targets.iter().filter(|target| target.journal).count();
    let mut text = if targets.len() == 1 {
        "Its transcript, the sub-agent transcripts only it refers to, its saved view, and its metrics are removed from Omperor.".to_owned()
    } else if conversations == 0 {
        "They hold nothing on disk; they are removed from the sidebar.".to_owned()
    } else {
        "Their transcripts, the sub-agent transcripts only they refer to, their saved views, and their metrics are removed from Omperor.".to_owned()
    };
    let worktrees: Vec<&Path> = targets
        .iter()
        .filter_map(|target| target.worktree.as_deref())
        .collect();
    match worktrees.as_slice() {
        [] => {}
        [only] => text.push_str(&format!(
            "\n\nIts worktree at {} is removed too if git finds nothing uncommitted or untracked in it; ignored files such as build output go with it. Its branch and commits are kept.",
            only.display()
        )),
        many => text.push_str(&format!(
            "\n\nTheir {} worktrees are removed too where git finds nothing uncommitted or untracked; ignored files such as build output go with them. Branches and commits are kept.",
            many.len()
        )),
    }
    text.push_str(" This can't be undone.");
    text
}

fn worktree_answer(path: &Path, outcome: &workspace::WorktreeRemoval) -> Value {
    match outcome {
        workspace::WorktreeRemoval::Removed { branch } => {
            json!({ "path": path, "removed": true, "branch_kept": branch })
        }
        workspace::WorktreeRemoval::Missing => {
            json!({ "path": path, "removed": false, "reason": "it was already gone" })
        }
        workspace::WorktreeRemoval::Kept(reason) => {
            json!({ "path": path, "removed": false, "reason": reason })
        }
    }
}

fn deletion_answer(report: &housekeeping::Report, refused: &[Refusal]) -> Value {
    let deleted: Vec<Value> = report
        .removed
        .iter()
        .map(|removed| {
            let mut entry =
                json!({ "id": removed.id, "title": removed.title, "bytes": removed.bytes });
            if removed.children > 0 {
                entry["sub_agent_transcripts"] = json!(removed.children);
            }
            if let Some((path, outcome)) = &removed.worktree {
                entry["worktree"] = worktree_answer(path, outcome);
            }
            entry
        })
        .collect();
    let not_deleted: Vec<Value> = report
        .failed
        .iter()
        .map(|failed| json!({ "id": failed.id, "title": failed.title, "reason": failed.reason }))
        .chain(refused.iter().map(
            |refusal| json!({ "id": refusal.id, "title": refusal.title, "reason": refusal.reason }),
        ))
        .collect();
    let mut answer =
        json!({ "summary": report.summary(), "deleted": deleted, "freed_bytes": report.bytes() });
    if !not_deleted.is_empty() {
        answer["not_deleted"] = json!(not_deleted);
    }
    if report.shared_children > 0 {
        answer["shared_sub_agent_transcripts_kept"] = json!(report.shared_children);
    }
    if !report.warnings.is_empty() {
        answer["warnings"] = json!(report.warnings);
    }
    answer
}

fn listing_answer(listing: &archive::Listing) -> Value {
    let manifest = &listing.manifest;
    json!({
        "id": manifest.id,
        "project": manifest.project.name,
        "folders": manifest.project.directories,
        "archived_ms": manifest.archived_at_ms,
        "sessions": manifest.session_count(),
        "bytes": manifest.bytes,
        "compressed_bytes": manifest.compressed_bytes,
        "worktrees_left_on_disk": manifest.worktrees,
    })
}

impl AppDelegate {
    fn sessions_directory(&self) -> Option<PathBuf> {
        self.ivars().omperor_sessions_directory.get().cloned()
    }

    /// Why a live session cannot be deleted or archived right now.
    fn removal_blocker(&self, owner: &live_sessions::LiveSession) -> Option<&'static str> {
        let ivars = self.ivars();
        if ivars.termination_pending.get() {
            return Some("Omperor is quitting");
        }
        if ivars.removals.borrow().values().any(|removal| {
            removal
                .members
                .iter()
                .any(|member| member.key == Some(owner.key))
        }) {
            return Some("it is already being removed");
        }
        if self.fork_owns_session(owner.key) {
            return Some("it is being forked");
        }
        if owner.chat_busy.get()
            || owner.pending_prompt.borrow().is_some()
            || owner.attachment_submission.get()
        {
            return Some("it is running a turn");
        }
        if owner.runtime_starting.get()
            || (owner.new_session_pending.get() && owner.omp_runtime.get().is_some())
        {
            return Some("it is still starting");
        }
        if owner.account_change_pending.get() || ivars.authentication_owner.get() == Some(owner.key)
        {
            return Some("it is changing accounts");
        }
        if matches!(
            owner.lifecycle.get(),
            live_sessions::Lifecycle::Stopping | live_sessions::Lifecycle::Retiring
        ) {
            return Some("it is stopping");
        }
        if owner
            .subagents
            .borrow()
            .iter()
            .any(|agent| agents::AgentState::from_status(&agent.status).is_busy())
        {
            return Some("its sub-agents are still working");
        }
        None
    }

    /// What to call a session in a confirmation or to the agent: a draft
    /// by what was written in it, since every draft row reads "Draft
    /// conversation".
    fn display_title(&self, summary: &session::SessionSummary) -> String {
        let Some(key) = draft_key_of(&summary.id) else {
            return summary.title.clone();
        };
        let mut text = self
            .ivars()
            .live_sessions
            .borrow()
            .get(live_sessions::SessionKey(key))
            .map(|owner| owner.draft.borrow().clone())
            .unwrap_or_default();
        if self.session().key.0 == key
            && let Some(input) = self.ivars().input.get()
        {
            text = input.stringValue().to_string();
        }
        match text.lines().map(str::trim).find(|line| !line.is_empty()) {
            Some(line) if line.chars().count() > 60 => {
                format!("Draft: {}…", line.chars().take(59).collect::<String>())
            }
            Some(line) => format!("Draft: {line}"),
            None => "Empty draft".to_owned(),
        }
    }

    /// The live entry of a listed session, if it has one.
    fn owner_of(
        &self,
        summary: &session::SessionSummary,
    ) -> Option<Rc<live_sessions::LiveSession>> {
        let live = self.ivars().live_sessions.borrow();
        match draft_key_of(&summary.id) {
            Some(key) => live.get(live_sessions::SessionKey(key)),
            None => live.find_path(&summary.path),
        }
    }

    /// Whether a worktree Omperor created is used only by sessions being
    /// removed, so it can go with them.
    fn worktree_only_for(&self, path: &Path, removing: &HashSet<String>) -> bool {
        let Ok(root) = preferences::worktrees_directory() else {
            return false;
        };
        if !path.starts_with(&root) {
            return false;
        }
        let used = self
            .ivars()
            .session_records
            .borrow()
            .values()
            .any(|record| {
                !removing.contains(&record.session_id)
                    && (record.directory == path || record.worktree_path.as_deref() == Some(path))
            })
            || self.ivars().sessions.borrow().iter().any(|summary| {
                !removing.contains(&summary.id) && summary.cwd.as_deref() == Some(path)
            });
        if used {
            return false;
        }
        !self
            .ivars()
            .live_sessions
            .borrow()
            .entries()
            .iter()
            .any(|entry| {
                let id = entry
                    .live_session_id
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| format!("draft-{}", entry.key.0));
                !removing.contains(&id) && entry.current_project.borrow().as_path() == path
            })
    }

    /// What deleting `ids` would take, and which of them cannot go.
    fn plan_deletion(
        &self,
        ids: &[String],
        requester: Option<live_sessions::SessionKey>,
    ) -> (Vec<(housekeeping::Target, Member)>, Vec<Refusal>) {
        let mut targets = Vec::new();
        let mut refused = Vec::new();
        for id in ids {
            let summary = self
                .ivars()
                .sessions
                .borrow()
                .iter()
                .find(|summary| &summary.id == id)
                .cloned();
            let Some(summary) = summary else {
                refused.push(Refusal {
                    id: id.clone(),
                    title: id.clone(),
                    reason: "no session in Omperor has this id".into(),
                });
                continue;
            };
            let title = self.display_title(&summary);
            let refuse = |reason: &str| Refusal {
                id: id.clone(),
                title: title.clone(),
                reason: reason.to_owned(),
            };
            if !summary.is_omperor_owned() {
                refused.push(refuse(
                    "it is a read-only OMP session, not one of Omperor's",
                ));
                continue;
            }
            let owner = self.owner_of(&summary);
            if let Some(owner) = &owner {
                if Some(owner.key) == requester {
                    refused.push(refuse("it is the session making this request"));
                    continue;
                }
                if let Some(reason) = self.removal_blocker(owner) {
                    refused.push(refuse(reason));
                    continue;
                }
            }
            let draft = draft_key_of(id).is_some();
            let worktree = self
                .ivars()
                .session_records
                .borrow()
                .get(id)
                .and_then(|record| record.worktree_path.clone());
            targets.push((
                housekeeping::Target {
                    id: id.clone(),
                    title,
                    journal: !draft && !summary.is_mock(),
                    worktree,
                },
                Member {
                    id: id.clone(),
                    key: owner.map(|owner| owner.key),
                    path: summary.path.clone(),
                },
            ));
        }
        // Refused sessions still own their directories. Check worktrees
        // only after the accepted set is complete.
        let removing: HashSet<String> = targets
            .iter()
            .map(|(target, _)| target.id.clone())
            .collect();
        for (target, _) in &mut targets {
            target.worktree = target
                .worktree
                .take()
                .filter(|path| self.worktree_only_for(path, &removing));
        }
        (targets, refused)
    }

    /// Asks the user on a sheet, so the rest of the window keeps working;
    /// `then` learns whether they confirmed.
    fn confirm_sheet(
        &self,
        window: &NSWindow,
        title: &str,
        detail: &str,
        confirm: &str,
        then: impl FnOnce(&AppDelegate, bool) + 'static,
    ) -> Retained<NSAlert> {
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(detail));
        alert
            .addButtonWithTitle(&NSString::from_str(confirm))
            .setHasDestructiveAction(true);
        alert.addButtonWithTitle(ns_string!("Cancel"));
        let delegate = self.retain();
        let then = Cell::new(Some(then));
        let handler = block2::RcBlock::new(move |response: NSModalResponse| {
            if let Some(then) = then.take() {
                then(&delegate, response == NSAlertFirstButtonReturn);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
        alert
    }

    /// Tells the user something on a sheet over whichever window they are
    /// in, so nothing else waits for them to read it.
    fn inform(&self, title: &str, detail: &str) {
        let window = self
            .ivars()
            .settings_ui
            .get()
            .map(|settings| settings.window.clone())
            .filter(|window| window.isKeyWindow())
            .or_else(|| self.ivars().window.get().cloned());
        let Some(window) = window else { return };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(detail));
        alert.addButtonWithTitle(ns_string!("OK"));
        alert.beginSheetModalForWindow_completionHandler(&window, None);
    }

    /// A confirmation an agent's call waits on; withdrawn if its turn stops.
    fn confirm_for_agent(
        &self,
        origin: Origin,
        title: &str,
        detail: &str,
        confirm: &str,
        then: impl FnOnce(&AppDelegate, bool) + 'static,
    ) {
        let Origin::Agent { key, call } = origin else {
            return;
        };
        let Some(window) = self.ivars().window.get().cloned() else {
            self.answer_tool(
                origin,
                Err("Omperor has no window to ask the user in".into()),
            );
            return;
        };
        let alert = self.confirm_sheet(
            &window,
            title,
            detail,
            confirm,
            move |delegate, confirmed| {
                delegate
                    .ivars()
                    .tool_confirmations
                    .borrow_mut()
                    .remove(&(key, call));
                then(delegate, confirmed);
            },
        );
        self.ivars()
            .tool_confirmations
            .borrow_mut()
            .insert((key, call), alert);
    }

    /// The agent's turn stopped while it waited on the user: take the
    /// question away.
    pub(super) fn withdraw_tool_confirmation(&self, key: live_sessions::SessionKey, call: u64) {
        let alert = self
            .ivars()
            .tool_confirmations
            .borrow_mut()
            .remove(&(key, call));
        if let (Some(alert), Some(window)) = (alert, self.ivars().window.get()) {
            window.endSheet_returnCode(&alert.window(), NSModalResponseAbort);
        }
    }

    /// Delivers a result to the agent's call that asked for it.
    fn answer_tool(&self, origin: Origin, result: Result<Value, String>) {
        let Origin::Agent { key, call } = origin else {
            return;
        };
        if let Some(owner) = self.ivars().live_sessions.borrow().get(key)
            && let Some(runtime) = owner.omp_runtime.get()
        {
            runtime.answer_host_tool(call, result);
        }
    }

    // MARK: Deleting sessions

    /// "Delete Session…" from a session's menu.
    pub(super) fn confirm_session_deletion(&self, id: String) {
        let (targets, refused) = self.plan_deletion(std::slice::from_ref(&id), None);
        if let Some(refusal) = refused.first() {
            self.inform(
                &format!("“{}” can't be deleted now", refusal.title),
                &format!("Omperor can't delete it because {}.", refusal.reason),
            );
            return;
        }
        let Some((target, member)) = targets.first() else {
            return;
        };
        if let Some(key) = draft_key_of(&target.id) {
            // A draft with nothing in it has nothing to lose.
            let empty =
                member
                    .key
                    .and_then(|key| self.ivars().live_sessions.borrow().get(key))
                    .is_none_or(|owner| {
                        owner.draft.borrow().trim().is_empty()
                            && owner.pending_attachments.borrow().items.is_empty()
                    })
                    && !(self.session().key.0 == key
                        && self.ivars().input.get().is_some_and(|input| {
                            !input.stringValue().to_string().trim().is_empty()
                        }));
            if empty {
                self.delete_sessions(vec![id], Origin::User);
                return;
            }
        }
        let Some(window) = self.ivars().window.get().cloned() else {
            return;
        };
        let (title, detail, button) = if draft_key_of(&target.id).is_some() {
            (
                "Discard this draft?".to_owned(),
                "What you wrote in it is discarded.".to_owned(),
                "Discard Draft",
            )
        } else {
            (
                format!("Delete “{}”?", target.title),
                deletion_detail(&[target]),
                "Delete Session",
            )
        };
        self.confirm_sheet(
            &window,
            &title,
            &detail,
            button,
            move |delegate, confirmed| {
                if confirmed {
                    delegate.delete_sessions(vec![id], Origin::User);
                }
            },
        );
    }

    /// Deletes what of `ids` can be deleted now; the rest are reported.
    pub(super) fn delete_sessions(&self, ids: Vec<String>, origin: Origin) {
        let requester = match origin {
            Origin::Agent { key, .. } => Some(key),
            Origin::User => None,
        };
        let (targets, refused) = self.plan_deletion(&ids, requester);
        if targets.is_empty() {
            match origin {
                Origin::User => {
                    if let Some(refusal) = refused.first() {
                        self.inform(
                            &format!("“{}” can't be deleted now", refusal.title),
                            &format!("Omperor can't delete it because {}.", refusal.reason),
                        );
                    }
                }
                Origin::Agent { .. } => {
                    self.answer_tool(
                        origin,
                        Ok(deletion_answer(&housekeeping::Report::default(), &refused)),
                    );
                }
            }
            return;
        }
        let (Some(sessions_dir), Ok(database)) =
            (self.sessions_directory(), preferences::database_path())
        else {
            self.answer_tool(origin, Err("Omperor's storage is unavailable".into()));
            self.set_chat_status("Omperor's storage is unavailable, so nothing was deleted");
            return;
        };
        let subject = if targets.len() == 1 {
            format!("“{}”", targets[0].0.title)
        } else {
            format!("{} sessions", targets.len())
        };
        let label = format!("Deleting {subject}…");
        let members = targets.iter().map(|(_, member)| member.clone()).collect();
        let request = housekeeping::Request {
            sessions_dir,
            database,
            targets: targets.into_iter().map(|(target, _)| target).collect(),
            metrics: self.metrics_log().map(metrics::Log::editor),
        };
        self.start_removal(
            Work::Delete(request),
            subject,
            members,
            refused,
            None,
            origin,
            &label,
        );
    }

    // MARK: Archiving projects

    fn plan_archive(
        &self,
        project_id: &str,
        requester: Option<live_sessions::SessionKey>,
    ) -> Result<ArchivePlan, String> {
        let projects = self.ivars().projects.borrow().clone();
        let position = projects
            .iter()
            .position(|project| project.id == project_id)
            .ok_or_else(|| format!("there is no project with the id {project_id}"))?;
        let project = projects[position].clone();
        let records = self.ivars().session_records.borrow().clone();
        let listed: Vec<session::SessionSummary> = self
            .ivars()
            .sessions
            .borrow()
            .iter()
            .filter(|summary| {
                summary.is_omperor_owned()
                    && session_project_position(summary, &projects, &records) == Some(position)
            })
            .cloned()
            .collect();
        let mut members = Vec::new();
        let mut busy = Vec::new();
        for summary in &listed {
            let owner = self.owner_of(summary);
            if let Some(owner) = &owner {
                if Some(owner.key) == requester {
                    return Err("the session making this request is one of its sessions".into());
                }
                if let Some(reason) = self.removal_blocker(owner) {
                    busy.push(format!("“{}” ({reason})", self.display_title(summary)));
                    continue;
                }
            }
            let draft_key = draft_key_of(&summary.id);
            let view = self
                .ivars()
                .agent_view_states
                .borrow()
                .get(&Some(summary.path.clone()))
                .and_then(|view| serde_json::to_value(view).ok());
            members.push((
                archive::Member {
                    id: summary.id.clone(),
                    title: self.display_title(summary),
                    journal: draft_key.is_none() && !summary.is_mock(),
                    view,
                    draft_key,
                },
                Member {
                    id: summary.id.clone(),
                    key: owner.map(|owner| owner.key),
                    path: summary.path.clone(),
                },
            ));
        }
        if !busy.is_empty() {
            return Err(format!(
                "some of its sessions are busy: {}",
                busy.join(", ")
            ));
        }
        let mut worktrees: Vec<PathBuf> = records
            .values()
            .filter(|record| record.project_id == project.id)
            .filter_map(|record| record.worktree_path.clone())
            .collect();
        worktrees.sort();
        worktrees.dedup();
        let fallback = projects
            .iter()
            .find(|other| other.id != project.id && !other.directories.is_empty())
            .map(|other| (other.id.clone(), other.directories[0].clone()));
        Ok(ArchivePlan {
            project,
            members,
            worktrees,
            fallback,
        })
    }

    fn archive_detail(plan: &ArchivePlan) -> String {
        let conversations = plan
            .members
            .iter()
            .filter(|(member, _)| member.journal)
            .count();
        let mut text = format!(
            "Its {conversations} session{}, their sub-agent transcripts, saved views, and metrics are compressed into an archive in Omperor's private storage, and the project leaves the sidebar.",
            plural(conversations)
        );
        text.push_str(" Its folders, worktrees, and code are not touched. Restore it any time from Settings › Archives.");
        text
    }

    /// "Archive Project…" from a project's menu or Settings › Projects.
    pub(super) fn confirm_project_archive(&self, project_id: &str) {
        let plan = match self.plan_archive(project_id, None) {
            Ok(plan) => plan,
            Err(reason) => {
                self.inform(
                    "This project can't be archived now",
                    &format!("Omperor can't archive it because {reason}."),
                );
                return;
            }
        };
        let window = self
            .ivars()
            .settings_ui
            .get()
            .map(|settings| settings.window.clone())
            .filter(|window| window.isVisible())
            .or_else(|| self.ivars().window.get().cloned());
        let Some(window) = window else { return };
        let project_id = project_id.to_owned();
        self.confirm_sheet(
            &window,
            &format!("Archive “{}”?", plan.project.name),
            &Self::archive_detail(&plan),
            "Archive Project",
            move |delegate, confirmed| {
                if confirmed {
                    delegate.archive_project(&project_id, Origin::User);
                }
            },
        );
    }

    pub(super) fn archive_project(&self, project_id: &str, origin: Origin) {
        let requester = match origin {
            Origin::Agent { key, .. } => Some(key),
            Origin::User => None,
        };
        let plan = match self.plan_archive(project_id, requester) {
            Ok(plan) => plan,
            Err(reason) => {
                self.answer_tool(
                    origin,
                    Err(format!("The project was not archived: {reason}.")),
                );
                if origin == Origin::User {
                    self.inform(
                        "This project can't be archived now",
                        &format!("Omperor can't archive it because {reason}."),
                    );
                }
                return;
            }
        };
        let (Some(sessions_dir), Ok(database), Ok(archives_dir)) = (
            self.sessions_directory(),
            preferences::database_path(),
            preferences::archives_directory(),
        ) else {
            self.answer_tool(origin, Err("Omperor's storage is unavailable".into()));
            self.set_chat_status("Omperor's storage is unavailable, so nothing was archived");
            return;
        };
        let name = plan.project.name.clone();
        let members = plan
            .members
            .iter()
            .map(|(_, member)| member.clone())
            .collect();
        let request = archive::Request {
            archives_dir,
            sessions_dir,
            database,
            project_id: plan.project.id.clone(),
            members: plan.members.into_iter().map(|(member, _)| member).collect(),
            worktrees: plan.worktrees,
            active_fallback: plan
                .fallback
                .as_ref()
                .map(|(_, directory)| directory.clone()),
            metrics: self.metrics_log().map(metrics::Log::editor),
        };
        self.start_removal(
            Work::Archive(request),
            format!("“{name}”"),
            members,
            Vec::new(),
            plan.fallback,
            origin,
            &format!("Archiving “{name}”…"),
        );
    }

    // MARK: The removal pipeline

    #[allow(clippy::too_many_arguments)]
    fn start_removal(
        &self,
        work: Work,
        subject: String,
        members: Vec<Member>,
        refused: Vec<Refusal>,
        fallback: Option<(String, PathBuf)>,
        origin: Origin,
        label: &str,
    ) {
        let job = self.begin_job(label);
        let id = self.ivars().next_removal_id.get() + 1;
        self.ivars().next_removal_id.set(id);
        // A live session holds its journal's lock: ask each to let go, and
        // run once they all have.
        let mut waiting = HashSet::new();
        for member in &members {
            let Some(owner) = member
                .key
                .and_then(|key| self.ivars().live_sessions.borrow().get(key))
            else {
                continue;
            };
            let Some(runtime) = owner.omp_runtime.get() else {
                continue;
            };
            if matches!(
                owner.lifecycle.get(),
                live_sessions::Lifecycle::Starting
                    | live_sessions::Lifecycle::Ready
                    | live_sessions::Lifecycle::Running
            ) {
                owner.lifecycle.set(live_sessions::Lifecycle::Retiring);
                owner.runtime_ready.set(false);
                if runtime.release_session() {
                    waiting.insert(owner.key);
                }
            }
        }
        let ready = waiting.is_empty();
        self.ivars().removals.borrow_mut().insert(
            id,
            Removal {
                job,
                origin,
                subject,
                waiting,
                started: Instant::now(),
                work: Some(work),
                members,
                refused,
                fallback,
            },
        );
        if ready {
            self.run_removal(id);
        }
        self.update_chat_controls();
    }

    /// A live session let go of its journal, or stopped.
    pub(super) fn removal_released(&self, key: live_sessions::SessionKey) {
        let ready: Vec<u64> = self
            .ivars()
            .removals
            .borrow_mut()
            .iter_mut()
            .filter_map(|(id, removal)| {
                (removal.waiting.remove(&key)
                    && removal.waiting.is_empty()
                    && removal.work.is_some())
                .then_some(*id)
            })
            .collect();
        for id in ready {
            self.run_removal(id);
        }
    }

    /// Runs removals whose sessions have not let go in time; the worker
    /// leaves any journal that is still held.
    pub(super) fn poll_removals(&self) {
        let overdue: Vec<u64> = self
            .ivars()
            .removals
            .borrow_mut()
            .iter_mut()
            .filter(|(_, removal)| {
                removal.work.is_some() && removal.started.elapsed() >= RELEASE_WAIT
            })
            .map(|(id, removal)| {
                removal.waiting.clear();
                *id
            })
            .collect();
        for id in overdue {
            self.run_removal(id);
        }
    }

    fn run_removal(&self, id: u64) {
        let work = self
            .ivars()
            .removals
            .borrow_mut()
            .get_mut(&id)
            .and_then(|removal| removal.work.take());
        let Some(work) = work else { return };
        let submitted = self
            .jobs()
            .submit(move || BackgroundEvent::RemovalFinished {
                id,
                outcome: match work {
                    Work::Delete(request) => Outcome::Deleted(housekeeping::delete(request)),
                    Work::Archive(request) => {
                        let archived = archive::archive(request);
                        Outcome::Archived(archived, preferences::load_registry().ok())
                    }
                },
            });
        if !submitted {
            self.apply_removal_finished(
                id,
                Outcome::Archived(Err("background workers are unavailable".into()), None),
            );
        }
    }

    /// Forgets everything the interface holds for the sessions a removal
    /// took: their rows, records, saved views, live entries, and metrics.
    fn forget_members(&self, members: &[Member], fallback: Option<&(String, PathBuf)>) {
        if members.is_empty() {
            return;
        }
        let ids: HashSet<String> = members.iter().map(|member| member.id.clone()).collect();
        let keys: Vec<live_sessions::SessionKey> =
            members.iter().filter_map(|member| member.key).collect();
        if keys.contains(&self.session().key) {
            self.select_replacement_draft(fallback);
        }
        let removed: Vec<_> = {
            let mut live = self.ivars().live_sessions.borrow_mut();
            keys.iter().filter_map(|key| live.remove(*key)).collect()
        };
        // Dropping the last handle shuts each released runtime down.
        drop(removed);
        self.ivars()
            .sessions
            .borrow_mut()
            .retain(|row| !ids.contains(&row.id));
        {
            let mut views = self.ivars().agent_view_states.borrow_mut();
            for member in members {
                views.remove(&Some(member.path.clone()));
            }
        }
        self.ivars()
            .session_records
            .borrow_mut()
            .retain(|id, _| !ids.contains(id));
        self.ivars()
            .session_positions
            .borrow_mut()
            .retain(|id, _| !ids.contains(id));
        self.ivars()
            .metrics_records
            .borrow_mut()
            .retain(|record| !ids.contains(record.session()));
        self.ivars().metrics_ui_dirty.set(true);
        self.ivars().removed_session_ids.borrow_mut().extend(ids);
        self.schedule_session_view_save();
        self.request_sidebar_rebuild();
        self.request_agents_refresh();
    }

    /// The open session is going: show a new draft in its place, in the
    /// same folder, or in `fallback` when the folder's project is going too.
    fn select_replacement_draft(&self, fallback: Option<&(String, PathBuf)>) {
        let previous = self.session();
        let owner = self.ivars().live_sessions.borrow_mut().create();
        match fallback {
            Some((project_id, directory)) => {
                owner.current_project.replace(directory.clone());
                owner.active_project_id.replace(Some(project_id.clone()));
            }
            None => {
                owner
                    .current_project
                    .replace(previous.current_project.borrow().clone());
                owner
                    .active_project_id
                    .replace(previous.active_project_id.borrow().clone());
            }
        }
        owner
            .current_model
            .replace(previous.current_model.borrow().clone());
        owner
            .current_account
            .replace(previous.current_account.borrow().clone());
        owner
            .current_thinking_level
            .replace(previous.current_thinking_level.borrow().clone());
        self.ivars().live_sessions.borrow_mut().select(owner.key);
        self.render_selected_binding();
    }

    pub(super) fn apply_removal_finished(&self, id: u64, outcome: Outcome) {
        let Some(removal) = self.ivars().removals.borrow_mut().remove(&id) else {
            return;
        };
        self.finish_job(removal.job);
        match outcome {
            Outcome::Deleted(report) => {
                let deleted: HashSet<&str> = report
                    .removed
                    .iter()
                    .map(|removed| removed.id.as_str())
                    .collect();
                let members: Vec<Member> = removal
                    .members
                    .iter()
                    .filter(|member| deleted.contains(member.id.as_str()))
                    .cloned()
                    .collect();
                self.forget_members(&members, None);
                self.sync_settings_projects_ui();
                self.set_chat_status(&report.summary());
                match removal.origin {
                    Origin::User => {
                        let problems: Vec<String> = report
                            .failed
                            .iter()
                            .map(|failed| format!("“{}”: {}", failed.title, failed.reason))
                            .chain(report.warnings.iter().cloned())
                            .collect();
                        if !problems.is_empty() {
                            self.inform(&report.summary(), &problems.join("\n"));
                        }
                    }
                    Origin::Agent { .. } => {
                        self.answer_tool(
                            removal.origin,
                            Ok(deletion_answer(&report, &removal.refused)),
                        );
                    }
                }
            }
            Outcome::Archived(Ok(archived), registry) => {
                self.forget_members(&removal.members, removal.fallback.as_ref());
                match registry {
                    Some(registry) => self.apply_project_registry(registry),
                    None => self.reload_project_registry(),
                }
                self.request_archives_list(true);
                let manifest = &archived.listing.manifest;
                let summary = format!(
                    "Archived “{}”: {} session{}, {}",
                    manifest.project.name,
                    manifest.session_count(),
                    plural(manifest.session_count()),
                    housekeeping::byte_text(manifest.compressed_bytes)
                );
                self.set_chat_status(&summary);
                match removal.origin {
                    Origin::User => {
                        if !archived.warnings.is_empty() {
                            self.inform(&summary, &archived.warnings.join("\n"));
                        }
                    }
                    Origin::Agent { .. } => {
                        let mut answer = json!({ "summary": summary, "archive": listing_answer(&archived.listing) });
                        if !archived.warnings.is_empty() {
                            answer["warnings"] = json!(archived.warnings);
                        }
                        self.answer_tool(removal.origin, Ok(answer));
                    }
                }
            }
            Outcome::Archived(Err(error), _) => {
                // Released sessions stay closed but listed, as they were.
                self.request_sidebar_rebuild();
                let title = format!("{} was not archived", removal.subject);
                self.set_chat_status(&format!("{title}: {error}"));
                match removal.origin {
                    Origin::User => self.inform(&title, &error),
                    Origin::Agent { .. } => {
                        self.answer_tool(removal.origin, Err(format!("{title}: {error}")))
                    }
                }
            }
        }
        self.update_chat_controls();
    }

    /// Takes a registry a worker read after changing it, and refreshes
    /// every view of it, as `reload_project_registry` does without reading.
    pub(super) fn apply_project_registry(&self, registry: preferences::Registry) {
        let owner = self.session();
        let active = owner.active_project_id.borrow().clone();
        self.ivars().projects.replace(registry.projects);
        self.ivars().missing_project_directories.replace(registry.missing_directories);
        self.ivars().session_records.replace(
            registry
                .records
                .into_iter()
                .map(|record| (record.session_id.clone(), record))
                .collect(),
        );
        self.ivars().session_positions.replace(registry.positions);
        if active
            .as_deref()
            .is_some_and(|id| self.project_by_id(id).is_none())
        {
            owner.active_project_id.replace(None);
        }
        self.refresh_project_location();
        self.rebuild_sidebar_sessions();
        self.sync_settings_projects_ui();
    }

    // MARK: Menus

    /// A project heading's right-click menu.
    pub(super) fn project_context_menu(&self, project_id: &str) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Project"));
        menu.setAutoenablesItems(false);
        let project = NSString::from_str(project_id);
        for (title, action) in [
            ("New Chat", Some(sel!(newChatInSidebarProject:))),
            ("", None),
            ("Archive Project…", Some(sel!(archiveSidebarProject:))),
        ] {
            let Some(action) = action else {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
                continue;
            };
            // SAFETY: the delegate implements both selectors and outlives the menu.
            let item = unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(title),
                    Some(action),
                    ns_string!(""),
                )
            };
            // SAFETY: the delegate outlives every sidebar menu it targets; the
            // represented object is an NSString the item keeps.
            unsafe {
                item.setTarget(Some::<&AnyObject>(self));
                item.setRepresentedObject(Some(&project));
            }
        }
        menu
    }

    /// The id a menu item carries as its represented object.
    pub(super) fn represented_id(sender: &AnyObject) -> Option<String> {
        let item = sender.downcast_ref::<NSMenuItem>()?;
        let object = item.representedObject()?;
        object.downcast::<NSString>().ok().map(|id| id.to_string())
    }

    // MARK: The agent's tools

    /// `/omperor`, `/omperor off`: offers the agent Omperor's own tools in
    /// the open session, or stops.
    pub(super) fn set_omperor_tools(&self, enabled: bool) {
        let owner = self.session();
        let changed = owner.omperor_tools.replace(enabled) != enabled;
        if let Some(runtime) = owner.omp_runtime.get() {
            runtime.set_omperor_tools(enabled);
        }
        let note = match (enabled, changed) {
            (true, true) => {
                "Omperor's tools are on for this session. The agent can list sessions, projects, and archives, delete sessions, and archive or restore projects; it asks you before removing anything. /omperor off turns them off."
            }
            (true, false) => "Omperor's tools are already on for this session.",
            (false, true) => "Omperor's tools are off for this session.",
            (false, false) => "Omperor's tools were not on in this session.",
        };
        owner.conversation.borrow_mut().notice(note, false);
        self.refresh_chat_transcript();
        self.schedule_session_view_save();
    }

    fn session_title(&self, owner: &live_sessions::LiveSession) -> String {
        let id = owner
            .live_session_id
            .borrow()
            .clone()
            .unwrap_or_else(|| format!("draft-{}", owner.key.0));
        self.ivars()
            .sessions
            .borrow()
            .iter()
            .find(|summary| summary.id == id)
            .map_or_else(
                || "this session".to_owned(),
                |summary| summary.title.clone(),
            )
    }

    fn session_facts(
        &self,
        requester: live_sessions::SessionKey,
    ) -> Vec<omperor_tools::SessionFacts> {
        let projects = self.ivars().projects.borrow().clone();
        let records = self.ivars().session_records.borrow().clone();
        let sessions = self.ivars().sessions.borrow().clone();
        let sessions_dir = self.sessions_directory();
        let now = preferences::now_ms();
        sessions
            .iter()
            .filter(|summary| summary.is_omperor_owned())
            .map(|summary| {
                let owner = self.owner_of(summary);
                let kind = if draft_key_of(&summary.id).is_some() {
                    omperor_tools::SessionKind::Draft
                } else if summary.is_mock() {
                    omperor_tools::SessionKind::Mock
                } else {
                    omperor_tools::SessionKind::Conversation
                };
                let state = match &owner {
                    Some(owner) if owner.chat_busy.get() => "running",
                    Some(owner)
                        if owner.omp_runtime.get().is_some()
                            && matches!(
                                owner.lifecycle.get(),
                                live_sessions::Lifecycle::Starting
                                    | live_sessions::Lifecycle::Ready
                                    | live_sessions::Lifecycle::Running
                            ) =>
                    {
                        "open"
                    }
                    _ => "closed",
                };
                let record = records.get(&summary.id);
                let journals = match (&sessions_dir, kind) {
                    (Some(dir), omperor_tools::SessionKind::Conversation) => {
                        std::iter::once(summary.id.as_str())
                            .chain(summary.spawned.iter().map(String::as_str))
                            .map(|id| housekeeping::journal_path(dir, id))
                            .collect()
                    }
                    _ => Vec::new(),
                };
                omperor_tools::SessionFacts {
                    id: summary.id.clone(),
                    title: self.display_title(summary),
                    kind,
                    project_id: session_project_position(summary, &projects, &records)
                        .map(|position| projects[position].id.clone()),
                    directory: summary.cwd.clone(),
                    updated_ms: if kind == omperor_tools::SessionKind::Conversation {
                        summary.modified_ms()
                    } else {
                        now
                    },
                    model: summary.model.clone().filter(|model| !model.is_empty()),
                    last_turn: summary.outcome.map(|outcome| match outcome {
                        session::SessionOutcome::Completed => "completed",
                        session::SessionOutcome::Errored => "errored",
                        session::SessionOutcome::Interrupted => "interrupted",
                        session::SessionOutcome::InProgress => "in_progress",
                    }),
                    state,
                    sub_agents: summary.spawned.len(),
                    worktree: record.and_then(|record| record.worktree_path.clone()),
                    branch: record.and_then(|record| record.branch.clone()),
                    parent_id: record.and_then(|record| record.parent_session_id.clone()),
                    current: owner.is_some_and(|owner| owner.key == requester),
                    journals,
                }
            })
            .collect()
    }

    pub(super) fn projects_answer(&self, requester: live_sessions::SessionKey) -> Value {
        let facts = self.session_facts(requester);
        let projects = self.ivars().projects.borrow().clone();
        let count = |project: Option<&str>, kind: omperor_tools::SessionKind| {
            facts
                .iter()
                .filter(|session| session.project_id.as_deref() == project && session.kind == kind)
                .count()
        };
        let listed: Vec<omperor_tools::ProjectFacts> = projects
            .iter()
            .map(|project| omperor_tools::ProjectFacts {
                id: project.id.clone(),
                name: project.name.clone(),
                directories: project.directories.clone(),
                conversations: count(Some(&project.id), omperor_tools::SessionKind::Conversation),
                drafts: count(Some(&project.id), omperor_tools::SessionKind::Draft),
                running: facts
                    .iter()
                    .filter(|session| {
                        session.project_id.as_deref() == Some(&project.id)
                            && session.state == "running"
                    })
                    .count(),
                current: facts.iter().any(|session| {
                    session.current && session.project_id.as_deref() == Some(&project.id)
                }),
            })
            .collect();
        omperor_tools::projects_answer(
            &listed,
            (
                count(None, omperor_tools::SessionKind::Conversation),
                count(None, omperor_tools::SessionKind::Draft),
            ),
        )
    }

    /// An `omperor_` tool call from the agent in `owner`.
    pub(super) fn handle_omperor_tool_call(
        &self,
        owner: &live_sessions::LiveSession,
        call: u64,
        tool: &str,
        arguments: &serde_json::Map<String, Value>,
    ) {
        let key = owner.key;
        let origin = Origin::Agent { key, call };
        if !owner.omperor_tools.get() {
            self.answer_tool(origin, Err("Omperor's tools are off in this session; the user turns them on with /omperor.".into()));
            return;
        }
        let request = match omperor_tools::parse(tool, arguments) {
            Ok(request) => request,
            Err(error) => {
                self.answer_tool(origin, Err(error));
                return;
            }
        };
        let asker = self.session_title(owner);
        match request {
            omperor_tools::Request::ListProjects => {
                let answer = self.projects_answer(key);
                self.answer_tool(origin, Ok(answer));
            }
            omperor_tools::Request::ListSessions {
                project_id,
                kind,
                limit,
                offset,
            } => {
                let (page, total) = omperor_tools::select_sessions(
                    self.session_facts(key),
                    project_id.as_deref(),
                    kind,
                    limit,
                    offset,
                );
                let submitted = self.jobs().submit(move || {
                    let sizes: Vec<u64> = page
                        .iter()
                        .map(|session| {
                            session
                                .journals
                                .iter()
                                .map(|journal| {
                                    housekeeping::size_of(journal)
                                        + housekeeping::size_of(&crate::saved_heights::path_for(
                                            journal,
                                        ))
                                        + housekeeping::size_of(&journal.with_extension(""))
                                })
                                .sum()
                        })
                        .collect();
                    BackgroundEvent::ToolAnswered {
                        key,
                        call,
                        result: Ok(omperor_tools::sessions_answer(&page, &sizes, total, offset)),
                    }
                });
                if !submitted {
                    self.answer_tool(
                        origin,
                        Err("Omperor's background workers are unavailable".into()),
                    );
                }
            }
            omperor_tools::Request::DeleteSessions {
                session_ids,
                reason,
            } => {
                let (targets, refused) = self.plan_deletion(&session_ids, Some(key));
                if targets.is_empty() {
                    self.answer_tool(
                        origin,
                        Ok(deletion_answer(&housekeeping::Report::default(), &refused)),
                    );
                    return;
                }
                let target_refs: Vec<&housekeeping::Target> =
                    targets.iter().map(|(target, _)| target).collect();
                let mut detail = format!(
                    "“{reason}”\n\n{}",
                    title_list(target_refs.iter().map(|target| target.title.as_str()))
                );
                if !refused.is_empty() {
                    detail.push_str(&format!(
                        "\n\n{} more can't be deleted now and will be left.",
                        refused.len()
                    ));
                }
                detail.push_str("\n\n");
                detail.push_str(&deletion_detail(&target_refs));
                let count = targets.len();
                let title = format!(
                    "The agent in “{asker}” wants to delete {count} session{}",
                    plural(count)
                );
                let button = if count == 1 {
                    "Delete Session".to_owned()
                } else {
                    format!("Delete {count} Sessions")
                };
                self.confirm_for_agent(
                    origin,
                    &title,
                    &detail,
                    &button,
                    move |delegate, confirmed| {
                        if confirmed {
                            delegate.delete_sessions(session_ids, origin);
                        } else {
                            delegate.answer_tool(
                                origin,
                                Err("The user declined; no sessions were deleted.".into()),
                            );
                        }
                    },
                );
            }
            omperor_tools::Request::ArchiveProject { project_id, reason } => {
                let plan = match self.plan_archive(&project_id, Some(key)) {
                    Ok(plan) => plan,
                    Err(why) => {
                        self.answer_tool(
                            origin,
                            Err(format!("The project was not archived: {why}.")),
                        );
                        return;
                    }
                };
                let title = format!(
                    "The agent in “{asker}” wants to archive “{}”",
                    plan.project.name
                );
                let detail = format!("“{reason}”\n\n{}", Self::archive_detail(&plan));
                self.confirm_for_agent(
                    origin,
                    &title,
                    &detail,
                    "Archive Project",
                    move |delegate, confirmed| {
                        if confirmed {
                            delegate.archive_project(&project_id, origin);
                        } else {
                            delegate.answer_tool(
                                origin,
                                Err("The user declined; the project was not archived.".into()),
                            );
                        }
                    },
                );
            }
            omperor_tools::Request::ListArchives => {
                let Ok(directory) = preferences::archives_directory() else {
                    self.answer_tool(origin, Err("Omperor's storage is unavailable".into()));
                    return;
                };
                let submitted = self.jobs().submit(move || BackgroundEvent::ToolAnswered {
                    key,
                    call,
                    result: archive::list(&directory)
                        .map(|listings| json!({ "archives": listings.iter().map(listing_answer).collect::<Vec<_>>() })),
                });
                if !submitted {
                    self.answer_tool(
                        origin,
                        Err("Omperor's background workers are unavailable".into()),
                    );
                }
            }
            omperor_tools::Request::DeleteArchive { archive_id, reason } => {
                let name = self.archive_name(&archive_id);
                let title = format!("The agent in “{asker}” wants to delete the archive of {name}");
                let detail = format!(
                    "“{reason}”\n\nThe archived project and its sessions are deleted for good and can't be restored."
                );
                self.confirm_for_agent(
                    origin,
                    &title,
                    &detail,
                    "Delete Archive",
                    move |delegate, confirmed| {
                        if confirmed {
                            delegate.delete_archive(archive_id, origin);
                        } else {
                            delegate.answer_tool(
                                origin,
                                Err("The user declined; the archive was kept.".into()),
                            );
                        }
                    },
                );
            }
            omperor_tools::Request::RestoreArchive { archive_id } => {
                self.restore_archive(archive_id, origin)
            }
            omperor_tools::Request::RemoveEmptySessions => self.remove_empty_sessions(origin),
        }
    }

    // MARK: Empty sessions

    /// Removes the sessions OMP started that never received a message.
    pub(super) fn remove_empty_sessions(&self, origin: Origin) {
        if self.ivars().sweeping.get() {
            self.answer_tool(
                origin,
                Err("Omperor is already removing empty sessions.".into()),
            );
            return;
        }
        let (Some(sessions_dir), Ok(database)) =
            (self.sessions_directory(), preferences::database_path())
        else {
            self.answer_tool(origin, Err("Omperor's storage is unavailable".into()));
            return;
        };
        // A live session that has not been sent anything yet is empty too,
        // and must keep its journal.
        let keep: HashSet<String> = self
            .ivars()
            .live_sessions
            .borrow()
            .entries()
            .iter()
            .filter_map(|entry| entry.live_session_id.borrow().clone())
            .collect();
        let metrics = self.metrics_log().map(metrics::Log::editor);
        self.ivars().sweeping.set(true);
        let job = self.begin_job("Removing empty sessions…");
        let submitted = self
            .jobs()
            .submit(move || BackgroundEvent::EmptySessionsSwept {
                job,
                origin,
                result: housekeeping::sweep_empty(
                    &sessions_dir,
                    &database,
                    &keep,
                    metrics.as_ref(),
                ),
            });
        if !submitted {
            self.ivars().sweeping.set(false);
            self.finish_job(job);
            self.answer_tool(
                origin,
                Err("Omperor's background workers are unavailable".into()),
            );
        }
    }

    pub(super) fn apply_empty_sessions_swept(
        &self,
        job: u64,
        origin: Origin,
        result: Result<housekeeping::Sweep, String>,
    ) {
        self.ivars().sweeping.set(false);
        self.finish_job(job);
        match result {
            Ok(sweep) => {
                let ids: HashSet<String> = sweep.ids.iter().cloned().collect();
                self.ivars()
                    .sessions
                    .borrow_mut()
                    .retain(|row| !ids.contains(&row.id));
                self.ivars()
                    .session_records
                    .borrow_mut()
                    .retain(|id, _| !ids.contains(id));
                self.ivars().removed_session_ids.borrow_mut().extend(ids);
                let summary = sweep.summary();
                self.set_chat_status(&summary);
                match origin {
                    Origin::User => self.inform(&summary, "Only sessions that never received a message were removed."),
                    Origin::Agent { .. } => self.answer_tool(
                        origin,
                        Ok(json!({ "summary": summary, "removed": sweep.ids.len(), "freed_bytes": sweep.bytes, "left_open": sweep.open })),
                    ),
                }
                self.request_sidebar_rebuild();
            }
            Err(error) => {
                self.set_chat_status(&format!("Empty sessions were not removed: {error}"));
                match origin {
                    Origin::User => self.inform("Empty sessions were not removed", &error),
                    Origin::Agent { .. } => self.answer_tool(origin, Err(error)),
                }
            }
        }
    }

    // MARK: Archives

    fn archive_name(&self, id: &str) -> String {
        self.ivars()
            .archives
            .borrow()
            .as_ref()
            .and_then(|listed| listed.as_ref().ok())
            .and_then(|listings| listings.iter().find(|listing| listing.manifest.id == id))
            .map_or_else(
                || id.to_owned(),
                |listing| format!("“{}”", listing.manifest.project.name),
            )
    }

    /// Reads the archive list on a worker; `force` reads it again even if
    /// one is shown.
    pub(super) fn request_archives_list(&self, force: bool) {
        let ivars = self.ivars();
        if ivars.archives_loading.get() || (!force && ivars.archives.borrow().is_some()) {
            return;
        }
        let Ok(directory) = preferences::archives_directory() else {
            return;
        };
        ivars.archives_loading.set(true);
        if !self
            .jobs()
            .submit(move || BackgroundEvent::ArchivesListed(archive::list(&directory)))
        {
            ivars.archives_loading.set(false);
        }
    }

    pub(super) fn apply_archives_listed(&self, result: Result<Vec<archive::Listing>, String>) {
        self.ivars().archives_loading.set(false);
        self.ivars().archives.replace(Some(result));
        self.sync_settings_archives_ui();
    }

    pub(super) fn archive_at(&self, index: isize) -> Option<archive::Listing> {
        let index = usize::try_from(index).ok()?;
        self.ivars()
            .archives
            .borrow()
            .as_ref()?
            .as_ref()
            .ok()?
            .get(index)
            .cloned()
    }

    pub(super) fn confirm_archive_deletion(&self, index: isize) {
        let Some(listing) = self.archive_at(index) else {
            return;
        };
        let Some(window) = self
            .ivars()
            .settings_ui
            .get()
            .map(|settings| settings.window.clone())
        else {
            return;
        };
        let manifest = listing.manifest;
        let detail = format!(
            "Its {} session{} and everything archived with them are deleted for good and can't be restored. The project's folders, worktrees, and code are not touched.",
            manifest.session_count(),
            plural(manifest.session_count())
        );
        self.confirm_sheet(
            &window,
            &format!("Delete the archive of “{}”?", manifest.project.name),
            &detail,
            "Delete Archive",
            move |delegate, confirmed| {
                if confirmed {
                    delegate.delete_archive(manifest.id, Origin::User);
                }
            },
        );
    }

    fn begin_archive_operation(&self, id: &str, label: &str, origin: Origin) -> Option<u64> {
        if !self
            .ivars()
            .archive_operations
            .borrow_mut()
            .insert(id.to_owned())
        {
            self.answer_tool(
                origin,
                Err("That archive is already being restored or deleted.".into()),
            );
            return None;
        }
        self.sync_settings_archives_ui();
        Some(self.begin_job(label))
    }

    fn end_archive_operation(&self, id: &str, job: u64) {
        self.ivars().archive_operations.borrow_mut().remove(id);
        self.finish_job(job);
    }

    pub(super) fn delete_archive(&self, id: String, origin: Origin) {
        let Ok(directory) = preferences::archives_directory() else {
            return;
        };
        let Some(job) = self.begin_archive_operation(&id, "Deleting an archive…", origin) else {
            return;
        };
        let target = id.clone();
        if !self.jobs().submit(move || BackgroundEvent::ArchiveDeleted {
            job,
            result: archive::delete(&directory, &target),
            id: target,
            origin,
        }) {
            self.end_archive_operation(&id, job);
        }
    }

    pub(super) fn apply_archive_deleted(
        &self,
        job: u64,
        id: String,
        origin: Origin,
        result: Result<u64, String>,
    ) {
        let name = self.archive_name(&id);
        self.end_archive_operation(&id, job);
        match result {
            Ok(freed) => {
                if let Some(Ok(listings)) = self.ivars().archives.borrow_mut().as_mut() {
                    listings.retain(|listing| listing.manifest.id != id);
                }
                let summary = format!(
                    "Deleted the archive of {name}, freeing {}",
                    housekeeping::byte_text(freed)
                );
                self.set_chat_status(&summary);
                self.answer_tool(
                    origin,
                    Ok(json!({ "summary": summary, "freed_bytes": freed })),
                );
            }
            Err(error) => {
                self.set_chat_status(&format!("Could not delete the archive: {error}"));
                if origin == Origin::User {
                    self.inform("The archive was not deleted", &error);
                }
                self.answer_tool(origin, Err(error));
            }
        }
        self.sync_settings_archives_ui();
    }

    pub(super) fn restore_archive(&self, id: String, origin: Origin) {
        let (Ok(directory), Some(sessions_dir), Ok(database)) = (
            preferences::archives_directory(),
            self.sessions_directory(),
            preferences::database_path(),
        ) else {
            self.answer_tool(origin, Err("Omperor's storage is unavailable".into()));
            return;
        };
        let label = format!("Restoring {}…", self.archive_name(&id));
        let Some(job) = self.begin_archive_operation(&id, &label, origin) else {
            return;
        };
        let metrics = self.metrics_log().map(metrics::Log::editor);
        let target = id.clone();
        if !self.jobs().submit(move || {
            let result = archive::restore(
                &directory,
                &target,
                &sessions_dir,
                &database,
                metrics.as_ref(),
            );
            let registry = result
                .as_ref()
                .ok()
                .and_then(|_| preferences::load_registry().ok());
            BackgroundEvent::ArchiveRestored {
                job,
                result,
                registry,
                id: target,
                origin,
            }
        }) {
            self.end_archive_operation(&id, job);
        }
    }

    pub(super) fn apply_archive_restored(
        &self,
        job: u64,
        id: String,
        origin: Origin,
        result: Result<archive::Restored, String>,
        registry: Option<preferences::Registry>,
    ) {
        self.end_archive_operation(&id, job);
        let restored = match result {
            Ok(restored) => restored,
            Err(error) => {
                self.set_chat_status(&format!("Could not restore the archive: {error}"));
                if origin == Origin::User {
                    self.inform("The project was not restored", &error);
                }
                self.answer_tool(origin, Err(error));
                self.sync_settings_archives_ui();
                return;
            }
        };
        let manifest = &restored.manifest;
        {
            let mut removed = self.ivars().removed_session_ids.borrow_mut();
            for entry in &manifest.sessions {
                removed.remove(&entry.id);
            }
        }
        let sessions_dir = self.sessions_directory();
        for entry in &manifest.sessions {
            let Some(view) = entry
                .view
                .clone()
                .and_then(|view| serde_json::from_value::<agents::ViewState>(view).ok())
            else {
                continue;
            };
            if let Some(key) = entry.draft_key {
                // A draft comes back as the same draft.
                let owner = self
                    .ivars()
                    .live_sessions
                    .borrow_mut()
                    .restore_draft(live_sessions::SessionKey(key));
                if let Some(context) = &view.desk.draft_context {
                    owner.current_project.replace(context.project.clone());
                    owner.active_project_id.replace(context.project_id.clone());
                    owner.current_model.replace(context.model.clone());
                    owner.current_account.replace(context.account.clone());
                    owner
                        .current_thinking_level
                        .replace(context.thinking_level.clone());
                    owner.fast_mode.set(context.fast_mode);
                }
                owner.draft.replace(view.desk.draft.clone());
                self.ivars()
                    .agent_view_states
                    .borrow_mut()
                    .insert(Some(session_state::draft_path(key)), view);
            } else if let (Some(stem), Some(directory)) = (&entry.journal, &sessions_dir) {
                self.ivars().agent_view_states.borrow_mut().insert(
                    Some(directory.join(format!("{stem}.{}", journal::JOURNAL_EXTENSION))),
                    view,
                );
            }
        }
        if restored.archive_removed
            && let Some(Ok(listings)) = self.ivars().archives.borrow_mut().as_mut()
        {
            listings.retain(|listing| listing.manifest.id != id);
        }
        self.schedule_session_view_save();
        match registry {
            Some(registry) => self.apply_project_registry(registry),
            None => self.reload_project_registry(),
        }
        self.start_session_discovery();
        self.sync_settings_archives_ui();
        let mut summary = format!(
            "Restored “{}” with {} session{}",
            manifest.project.name,
            manifest.session_count(),
            plural(manifest.session_count())
        );
        if !restored.taken_directories.is_empty() {
            summary.push_str(&format!(
                "; {} of its folders now belong to other projects and stay there",
                restored.taken_directories.len()
            ));
        }
        self.set_chat_status(&summary);
        if origin == Origin::User && !restored.warnings.is_empty() {
            self.inform(&summary, &restored.warnings.join("\n"));
        }
        let mut answer = json!({ "summary": summary, "project_id": manifest.project.id });
        if !restored.taken_directories.is_empty() {
            answer["folders_kept_by_other_projects"] = json!(restored.taken_directories);
        }
        if !restored.warnings.is_empty() {
            answer["warnings"] = json!(restored.warnings);
        }
        self.answer_tool(origin, Ok(answer));
    }

    pub(super) fn reveal_archive(&self, index: isize) {
        let Some(listing) = self.archive_at(index) else {
            return;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(&listing.path.display().to_string()));
        NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    }

    // MARK: The Archives page

    /// Lays out the Archives page's scrolling list and its note; the cards
    /// are made by [`Self::sync_settings_archives_ui`].
    pub(super) fn build_archives_page(
        &self,
        page: &NSView,
        mtm: MainThreadMarker,
    ) -> (Retained<NSScrollView>, Retained<NSView>) {
        settings_page_header(
            page,
            "Archives",
            "Projects put away with their sessions; restore one to bring it back",
            mtm,
        );
        let note = text_label(
            "Archives are zstd-compressed and private to Omperor. Archiving never touches a project's folders, worktrees, or code.",
            11.0,
            false,
            mtm,
        );
        note.setTextColor(Some(&theme::secondary_text()));
        note.setMaximumNumberOfLines(2);
        note.setFrame(rect(42.0, 18.0, 440.0, 38.0));
        note.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        page.addSubview(&note);
        let reveal = settings_button(
            "Show Folder",
            rect(500.0, 22.0, 140.0, 30.0),
            self,
            sel!(revealArchives:),
            mtm,
        );
        reveal.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        page.addSubview(&reveal);
        let height = SETTINGS_HEIGHT - SETTINGS_PAGE_HEADER_HEIGHT - SETTINGS_ACCOUNT_NOTE_HEIGHT;
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(
                0.0,
                SETTINGS_ACCOUNT_NOTE_HEIGHT,
                SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH,
                height,
            ),
        );
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0.0, 0.0, SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH, height),
        );
        scroll.setDocumentView(Some(&content));
        page.addSubview(&scroll);
        (scroll, content)
    }

    /// Rebuilds the Archives page from the last list read: one card per
    /// archive, with Restore, Show in Finder, and Delete.
    pub(super) fn sync_settings_archives_ui(&self) {
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let mtm = self.mtm();
        let content = &settings.archives_content;
        let previous = content.subviews();
        for index in 0..previous.count() {
            previous.objectAtIndex(index).removeFromSuperview();
        }
        const CARD_X: f64 = 40.0;
        const CARD_WIDTH: f64 = 600.0;
        const CARD_GAP: f64 = 16.0;
        const CARD_HEADER: f64 = 70.0;
        const CARD_FOOTER: f64 = 54.0;
        const ROW_HEIGHT: f64 = 22.0;
        const TOP_PADDING: f64 = 4.0;
        const SHOWN_FOLDERS: usize = 3;
        let state = self.ivars().archives.borrow().clone();
        let busy = self.ivars().archive_operations.borrow().clone();
        let listings = match &state {
            Some(Ok(listings)) => listings.clone(),
            _ => Vec::new(),
        };
        let message = match &state {
            None => Some("Reading archives…".to_owned()),
            Some(Err(error)) => Some(format!("Could not read the archives: {error}")),
            Some(Ok(listings)) if listings.is_empty() => Some(
                "No archived projects. Archive one from its menu in the sidebar or from Settings › Projects.".to_owned(),
            ),
            Some(Ok(_)) => None,
        };
        let rows = |listing: &archive::Listing| {
            let folders = listing
                .manifest
                .project
                .directories
                .len()
                .clamp(1, SHOWN_FOLDERS + 1);
            folders + usize::from(!listing.manifest.worktrees.is_empty())
        };
        let card_height = |listing: &archive::Listing| {
            CARD_HEADER + rows(listing) as f64 * ROW_HEIGHT + CARD_FOOTER
        };
        let content_height = if message.is_some() {
            100.0
        } else {
            listings
                .iter()
                .map(|listing| card_height(listing) + CARD_GAP)
                .sum::<f64>()
                + TOP_PADDING
        };
        let document_height = content_height.max(settings.archives_scroll.frame().size.height);
        content.setFrame(rect(
            0.0,
            0.0,
            settings.archives_scroll.frame().size.width,
            document_height,
        ));
        if let Some(message) = message {
            let card = settings_card(
                rect(
                    CARD_X,
                    document_height - TOP_PADDING - 80.0,
                    CARD_WIDTH,
                    80.0,
                ),
                mtm,
            );
            let label = text_label(&message, 12.0, false, mtm);
            label.setTextColor(Some(&theme::secondary_text()));
            label.setMaximumNumberOfLines(2);
            label.setFrame(rect(20.0, 20.0, 520.0, 40.0));
            card.addSubview(&label);
            if state.is_none() {
                let spinner = row_spinner(rect(552.0, 30.0, 18.0, 18.0), mtm);
                card.addSubview(&spinner);
            }
            content.addSubview(&card);
        }
        let mut top = TOP_PADDING;
        for (index, listing) in listings.iter().enumerate() {
            let manifest = &listing.manifest;
            let height = card_height(listing);
            let card = settings_card(
                rect(CARD_X, document_height - top - height, CARD_WIDTH, height),
                mtm,
            );
            let detail = format!(
                "Archived {} • {} session{} • {}",
                format_timestamp(manifest.archived_at_ms),
                manifest.session_count(),
                plural(manifest.session_count()),
                housekeeping::byte_text(manifest.compressed_bytes)
            );
            settings_row_text(&card, height - 30.0, &manifest.project.name, &detail, mtm);
            let mut y = height - CARD_HEADER;
            let folders = &manifest.project.directories;
            let shown: Vec<String> = if folders.len() > SHOWN_FOLDERS + 1 {
                folders
                    .iter()
                    .take(SHOWN_FOLDERS)
                    .map(|folder| folder.display().to_string())
                    .chain(std::iter::once(format!(
                        "and {} more folders",
                        folders.len() - SHOWN_FOLDERS
                    )))
                    .collect()
            } else if folders.is_empty() {
                vec!["No folders".to_owned()]
            } else {
                folders
                    .iter()
                    .map(|folder| folder.display().to_string())
                    .collect()
            };
            for line in shown {
                y -= ROW_HEIGHT;
                card.addSubview(&settings_path_label(
                    &line,
                    rect(20.0, y + 4.0, 556.0, 16.0),
                    mtm,
                ));
            }
            if !manifest.worktrees.is_empty() {
                y -= ROW_HEIGHT;
                let count = manifest.worktrees.len();
                let label = settings_path_label(
                    &format!("{count} worktree{} left on disk", plural(count)),
                    rect(20.0, y + 4.0, 556.0, 16.0),
                    mtm,
                );
                label.setToolTip(Some(&NSString::from_str(
                    &manifest
                        .worktrees
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                )));
                card.addSubview(&label);
            }
            settings_separator(&card, CARD_FOOTER, mtm);
            let working = busy.contains(&manifest.id);
            let reveal = format!("Show in {FILE_MANAGER}");
            for (title, frame, action) in [
                (
                    "Restore",
                    rect(20.0, 12.0, 96.0, 30.0),
                    sel!(settingsRestoreArchive:),
                ),
                (
                    reveal.as_str(),
                    rect(122.0, 12.0, 130.0, 30.0),
                    sel!(settingsRevealArchive:),
                ),
                (
                    "Delete…",
                    rect(476.0, 12.0, 100.0, 30.0),
                    sel!(settingsDeleteArchive:),
                ),
            ] {
                let button = settings_button(title, frame, self, action, mtm);
                button.setTag(index as isize);
                button.setEnabled(!working);
                card.addSubview(&button);
            }
            if working {
                card.addSubview(&row_spinner(rect(262.0, 18.0, 18.0, 18.0), mtm));
            }
            content.addSubview(&card);
            top += height + CARD_GAP;
        }
        let clip = settings.archives_scroll.contentView();
        let clip_height = clip.bounds().size.height;
        clip.scrollToPoint(NSPoint::new(0.0, (document_height - clip_height).max(0.0)));
        settings.archives_scroll.reflectScrolledClipView(&clip);
    }

    // MARK: Drafts

    /// Drops drafts nothing was written in, other than the open one, so new
    /// chats left empty do not pile up in the sidebar. Returns whether any
    /// went; the caller rebuilds the sidebar.
    pub(super) fn discard_empty_drafts(&self) -> bool {
        let selected = self.session().key;
        let empty: Vec<live_sessions::SessionKey> = {
            let views = self.ivars().agent_view_states.borrow();
            self.ivars()
                .live_sessions
                .borrow()
                .entries()
                .iter()
                .filter(|entry| {
                    entry.key != selected
                        && entry.lifecycle.get() == live_sessions::Lifecycle::Draft
                        && entry.omp_runtime.get().is_none()
                        && entry.live_session_id.borrow().is_none()
                        && entry.pending_session.borrow().is_none()
                        && entry.pending_prompt.borrow().is_none()
                        && !entry.new_session_pending.get()
                        && entry.draft.borrow().trim().is_empty()
                        && entry.pending_attachments.borrow().items.is_empty()
                        && views
                            .get(&Some(session_state::draft_path(entry.key.0)))
                            .is_none_or(|view| {
                                view.desk.draft.trim().is_empty()
                                    && view.desk.attachments.is_empty()
                            })
                })
                .map(|entry| entry.key)
                .collect()
        };
        if empty.is_empty() {
            return false;
        }
        {
            let mut live = self.ivars().live_sessions.borrow_mut();
            let mut views = self.ivars().agent_view_states.borrow_mut();
            for key in &empty {
                live.remove(*key);
                views.remove(&Some(session_state::draft_path(key.0)));
            }
        }
        let ids: HashSet<String> = empty.iter().map(|key| format!("draft-{}", key.0)).collect();
        self.ivars()
            .sessions
            .borrow_mut()
            .retain(|row| !ids.contains(&row.id));
        self.schedule_session_view_save();
        true
    }
}

impl AppDelegate {
    /// Exercise the real planner in the isolated housekeeping smoke run.
    /// Only cached records are changed; restore them before the smoke continues.
    pub(super) fn smoke_check_shared_worktree_plan(
        &self,
        summaries: &[session::SessionSummary],
    ) -> Result<(), String> {
        let shared = preferences::worktrees_directory()
            .map_err(|error| error.to_string())?
            .join("smoke-shared");
        let project_id = self.ivars().projects.borrow()[0].id.clone();
        let saved: Vec<_> = summaries
            .iter()
            .map(|summary| {
                let record = preferences::SessionRecord {
                    session_id: summary.id.clone(),
                    project_id: project_id.clone(),
                    directory: shared.clone(),
                    parent_session_id: None,
                    branch: None,
                    base_branch: None,
                    worktree_path: Some(shared.clone()),
                    created_at_ms: 0,
                };
                let old = self
                    .ivars()
                    .session_records
                    .borrow_mut()
                    .insert(summary.id.clone(), record);
                (summary.id.clone(), old)
            })
            .collect();
        let busy = self.ivars().live_sessions.borrow_mut().create();
        busy.selected_session_path
            .replace(Some(summaries[1].path.clone()));
        busy.live_session_id.replace(Some(summaries[1].id.clone()));
        busy.current_project.replace(shared);
        busy.chat_busy.set(true);
        let ids: Vec<_> = summaries.iter().map(|summary| summary.id.clone()).collect();
        let (targets, refused) = self.plan_deletion(&ids, None);
        let keeps_busy =
            targets.len() == 1 && refused.len() == 1 && targets[0].0.worktree.is_none();
        busy.chat_busy.set(false);
        let (targets, refused) = self.plan_deletion(&ids, Some(busy.key));
        let keeps_requester =
            targets.len() == 1 && refused.len() == 1 && targets[0].0.worktree.is_none();
        let (targets, refused) = self.plan_deletion(&ids, None);
        let removes_when_idle = targets.len() == 2
            && refused.is_empty()
            && targets.iter().all(|(target, _)| target.worktree.is_some());
        self.ivars().live_sessions.borrow_mut().remove(busy.key);
        for (id, old) in saved {
            let mut records = self.ivars().session_records.borrow_mut();
            if let Some(old) = old {
                records.insert(id, old);
            } else {
                records.remove(&id);
            }
        }
        if keeps_busy && keeps_requester && removes_when_idle {
            Ok(())
        } else {
            Err(format!(
                "shared worktree plan: busy={keeps_busy}, requester={keeps_requester}, idle={removes_when_idle}"
            ))
        }
    }
}
