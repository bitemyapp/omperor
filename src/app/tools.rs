//! The `omperor_` tools: Omperor's own workspace (its sessions, projects
//! and archives) offered to the agent of a session where the user turned
//! them on with `/omperor`. Reading is answered at once; anything that
//! deletes or moves data asks the user first.

use std::sync::Arc;

use aizome_kit::prelude::*;
use serde_json::{Map, Value, json};

use super::projects::Origin;
use super::{App, Msg};
use crate::live_sessions::{Lifecycle, LiveSession, SessionKey};
use crate::omperor_tools::{self, Request, SessionFacts, SessionKind};
use crate::{archive, housekeeping, preferences, rules, session};

/// Titles a confirmation lists before "and N more".
const LISTED_TITLES: usize = 12;

pub(crate) fn listing_answer(listing: &archive::Listing) -> Value {
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

fn deletion_answer(report: &housekeeping::Report) -> Value {
    let deleted: Vec<Value> = report
        .removed
        .iter()
        .map(|removed| {
            let mut entry = json!({ "id": removed.id, "title": removed.title, "bytes": removed.bytes });
            if removed.children > 0 {
                entry["sub_agent_transcripts"] = json!(removed.children);
            }
            entry
        })
        .collect();
    let not_deleted: Vec<Value> =
        report.failed.iter().map(|f| json!({ "id": f.id, "title": f.title, "reason": f.reason })).collect();
    let mut answer = json!({ "summary": report.summary(), "deleted": deleted, "freed_bytes": report.bytes() });
    if !not_deleted.is_empty() {
        answer["not_deleted"] = json!(not_deleted);
    }
    if !report.warnings.is_empty() {
        answer["warnings"] = json!(report.warnings);
    }
    answer
}

fn title_list<'a>(titles: impl Iterator<Item = &'a str>) -> String {
    let titles: Vec<&str> = titles.collect();
    let mut lines: Vec<String> = titles.iter().take(LISTED_TITLES).map(|t| format!("• {t}")).collect();
    if titles.len() > LISTED_TITLES {
        lines.push(format!("…and {} more", titles.len() - LISTED_TITLES));
    }
    lines.join("\n")
}

impl App {
    fn session_title(&self, session: &LiveSession) -> String {
        let id = session.live_session_id.borrow().clone().unwrap_or_default();
        self.sessions.iter().find(|s| s.id == id).map_or_else(|| "this session".to_owned(), |s| s.title.clone())
    }

    fn session_facts(&self, requester: SessionKey) -> Vec<SessionFacts> {
        let now = preferences::now_ms();
        self.sessions
            .iter()
            .filter(|s| s.is_omperor_owned())
            .map(|summary| {
                let live =
                    self.live.entries().into_iter().find(|s| s.live_session_id.borrow().as_deref() == Some(summary.id.as_str()));
                let kind = if summary.is_mock() { SessionKind::Mock } else { SessionKind::Conversation };
                let state = match &live {
                    Some(s) if s.chat_busy.get() => "running",
                    Some(s)
                        if s.omp_runtime.get().is_some()
                            && matches!(s.lifecycle.get(), Lifecycle::Starting | Lifecycle::Ready | Lifecycle::Running) =>
                    {
                        "open"
                    }
                    _ => "closed",
                };
                let record = self.records.get(&summary.id);
                let journals = match (&self.sessions_dir, kind) {
                    (Some(dir), SessionKind::Conversation) => std::iter::once(summary.id.as_str())
                        .chain(summary.spawned.iter().map(String::as_str))
                        .map(|id| housekeeping::journal_path(dir, id))
                        .collect(),
                    _ => Vec::new(),
                };
                SessionFacts {
                    id: summary.id.clone(),
                    title: summary.title.clone(),
                    kind,
                    project_id: rules::session_project_position(summary, &self.projects, &self.records)
                        .map(|i| self.projects[i].id.clone()),
                    directory: summary.cwd.clone(),
                    updated_ms: if kind == SessionKind::Conversation { summary.modified_ms() } else { now },
                    model: summary.model.clone().filter(|m| !m.is_empty()),
                    last_turn: summary.outcome.map(|outcome| match outcome {
                        session::SessionOutcome::Completed => "completed",
                        session::SessionOutcome::Errored => "errored",
                        session::SessionOutcome::Interrupted => "interrupted",
                        session::SessionOutcome::InProgress => "in_progress",
                    }),
                    state,
                    sub_agents: summary.spawned.len(),
                    worktree: record.and_then(|r| r.worktree_path.clone()),
                    branch: record.and_then(|r| r.branch.clone()),
                    parent_id: record.and_then(|r| r.parent_session_id.clone()),
                    current: live.is_some_and(|s| s.key == requester),
                    journals,
                }
            })
            .collect()
    }

    fn projects_answer(&self, requester: SessionKey) -> Value {
        let facts = self.session_facts(requester);
        let count = |project: Option<&str>, kind: SessionKind| {
            facts.iter().filter(|s| s.project_id.as_deref() == project && s.kind == kind).count()
        };
        let listed: Vec<omperor_tools::ProjectFacts> = self
            .projects
            .iter()
            .map(|project| omperor_tools::ProjectFacts {
                id: project.id.clone(),
                name: project.name.clone(),
                directories: project.directories.clone(),
                conversations: count(Some(&project.id), SessionKind::Conversation),
                drafts: count(Some(&project.id), SessionKind::Draft),
                running: facts.iter().filter(|s| s.project_id.as_deref() == Some(&project.id) && s.state == "running").count(),
                current: facts.iter().any(|s| s.current && s.project_id.as_deref() == Some(&project.id)),
            })
            .collect();
        omperor_tools::projects_answer(
            &listed,
            (count(None, SessionKind::Conversation), count(None, SessionKind::Draft)),
        )
    }

    /// An `omperor_` tool call from the agent in `session`.
    pub(crate) fn host_tool_call(
        &mut self,
        session: &LiveSession,
        call: u64,
        tool: &str,
        arguments: &Map<String, Value>,
        cx: &mut Cx<Msg>,
    ) {
        let key = session.key;
        let origin = Some((key, call));
        if !session.omperor_tools.get() {
            self.answer_origin(origin, Err("Omperor's tools are off in this session; the user turns them on with /omperor.".into()), cx);
            return;
        }
        let request = match omperor_tools::parse(tool, arguments) {
            Ok(request) => request,
            Err(error) => {
                self.answer_origin(origin, Err(error), cx);
                return;
            }
        };
        let asker = self.session_title(session);
        match request {
            Request::ListProjects => {
                let answer = self.projects_answer(key);
                self.answer_origin(origin, Ok(answer), cx);
            }
            Request::ListSessions { project_id, kind, limit, offset } => {
                let (page, total) =
                    omperor_tools::select_sessions(self.session_facts(key), project_id.as_deref(), kind, limit, offset);
                cx.spawn(move || {
                    let sizes: Vec<u64> = page
                        .iter()
                        .map(|s| {
                            s.journals
                                .iter()
                                .map(|j| housekeeping::size_of(j) + housekeeping::size_of(&j.with_extension("")))
                                .sum()
                        })
                        .collect();
                    Msg::ToolAnswered(key, call, Arc::new(Ok(omperor_tools::sessions_answer(&page, &sizes, total, offset))))
                });
            }
            Request::ListArchives => {
                let Ok(dir) = preferences::archives_directory() else {
                    self.answer_origin(origin, Err("Omperor's storage is unavailable".into()), cx);
                    return;
                };
                cx.spawn(move || {
                    let result = archive::list(&dir)
                        .map(|listings| json!({ "archives": listings.iter().map(listing_answer).collect::<Vec<_>>() }));
                    Msg::ToolAnswered(key, call, Arc::new(result))
                });
            }
            ref destructive => {
                let (title, detail, button) = match destructive {
                    Request::DeleteSessions { session_ids, reason } => {
                        let titles: Vec<String> = session_ids
                            .iter()
                            .map(|id| self.sessions.iter().find(|s| s.id == *id).map_or_else(|| id.clone(), |s| s.title.clone()))
                            .collect();
                        let count = titles.len();
                        (
                            format!("The agent in “{asker}” wants to delete {count} session{}", housekeeping::plural(count)),
                            format!(
                                "“{reason}”\n\n{}\n\nTheir transcripts, the sub-agent transcripts only they refer to, and their metrics are removed. This can't be undone.",
                                title_list(titles.iter().map(String::as_str))
                            ),
                            if count == 1 { "Delete Session".to_owned() } else { format!("Delete {count} Sessions") },
                        )
                    }
                    Request::ArchiveProject { project_id, reason } => {
                        let name = self.project_by_id(project_id).map_or_else(|| project_id.clone(), |p| p.name.clone());
                        (
                            format!("The agent in “{asker}” wants to archive “{name}”"),
                            format!("“{reason}”\n\nIts sessions go into one compressed file, to restore later. Its folders stay."),
                            "Archive Project".to_owned(),
                        )
                    }
                    Request::DeleteArchive { archive_id, reason } => (
                        format!("The agent in “{asker}” wants to delete the archive of {}", self.archive_name(archive_id)),
                        format!("“{reason}”\n\nThe archived project and its sessions are deleted for good."),
                        "Delete Archive".to_owned(),
                    ),
                    Request::RestoreArchive { archive_id } => (
                        format!("The agent in “{asker}” wants to restore {}", self.archive_name(archive_id)),
                        "Its project and sessions come back into Omperor.".to_owned(),
                        "Restore".to_owned(),
                    ),
                    Request::RemoveEmptySessions => (
                        format!("The agent in “{asker}” wants to remove empty sessions"),
                        "Sessions that never received a message are deleted.".to_owned(),
                        "Remove Empty Sessions".to_owned(),
                    ),
                    _ => unreachable!("reads are answered above"),
                };
                self.pending_tools.insert((key, call), destructive.clone());
                cx.alert(
                    self.main,
                    Alert::new(title, detail)
                        .destructive_button(button, Msg::ConfirmTool(key, call, true))
                        .cancel_with("Don't Allow", Msg::ConfirmTool(key, call, false)),
                );
            }
        }
    }

    pub(crate) fn host_tool_cancelled(&mut self, key: SessionKey, call: u64) {
        self.pending_tools.remove(&(key, call));
    }

    pub(super) fn confirm_tool(&mut self, key: SessionKey, call: u64, approved: bool, cx: &mut Cx<Msg>) {
        let Some(request) = self.pending_tools.remove(&(key, call)) else { return };
        let origin: Origin = Some((key, call));
        if !approved {
            self.answer_origin(origin, Err("The user declined.".into()), cx);
            return;
        }
        match request {
            Request::DeleteSessions { session_ids, .. } => self.delete_for_tool(key, call, session_ids, cx),
            Request::ArchiveProject { project_id, .. } => self.archive_project_for(&project_id, origin, cx),
            Request::DeleteArchive { archive_id, .. } => self.delete_archive_for(&archive_id, origin, cx),
            Request::RestoreArchive { archive_id } => self.restore_archive_for(&archive_id, origin, cx),
            Request::RemoveEmptySessions => self.sweep_for_tool(key, call, cx),
            _ => {}
        }
    }

    fn delete_for_tool(&mut self, key: SessionKey, call: u64, ids: Vec<String>, cx: &mut Cx<Msg>) {
        let (Some(sessions_dir), Ok(database)) = (self.sessions_dir.clone(), preferences::database_path()) else {
            self.answer_origin(Some((key, call)), Err("Omperor's storage is unavailable".into()), cx);
            return;
        };
        let mut targets = Vec::new();
        for id in &ids {
            let Some(summary) = self.sessions.iter().find(|s| s.id == *id).cloned() else { continue };
            let live = self.live.entries().into_iter().find(|s| s.live_session_id.borrow().as_deref() == Some(id.as_str()));
            if let Some(live) = live {
                if live.key == key || live.chat_busy.get() {
                    continue;
                }
                if self.session().key == live.key {
                    self.select_other(live.key, cx);
                }
                if let Some(runtime) = live.omp_runtime.get() {
                    runtime.begin_shutdown();
                }
                self.live.remove(live.key);
                self.views.remove(&live.key);
            }
            let worktree = self.records.get(id).and_then(|r| r.worktree_path.clone());
            targets.push(housekeeping::Target {
                id: summary.id.clone(),
                title: summary.title.clone(),
                journal: summary.is_omperor_owned(),
                worktree,
            });
            self.removed_session_ids.insert(summary.id.clone());
        }
        self.sessions.retain(|s| !self.removed_session_ids.contains(&s.id));
        let metrics = self.metrics_log().map(crate::metrics::Log::editor);
        cx.spawn(move || {
            let request = housekeeping::Request { sessions_dir, database, targets, metrics };
            let mut report = housekeeping::delete(request.clone());
            for _ in 0..40 {
                if !report.failed.iter().any(|f| f.reason.contains("has it open")) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                report = housekeeping::delete(request.clone());
            }
            Msg::ToolAnswered(key, call, Arc::new(Ok(deletion_answer(&report))))
        });
    }

    /// Removing the sessions that never held a conversation, as work for a
    /// worker: the live sessions' own stay.
    fn sweep_work(&mut self) -> Result<impl FnOnce() -> Result<housekeeping::Sweep, String> + Send + 'static, String> {
        let (Some(sessions_dir), Ok(database)) = (self.sessions_dir.clone(), preferences::database_path()) else {
            return Err("Omperor's storage is unavailable".into());
        };
        let keep: std::collections::HashSet<String> =
            self.live.entries().iter().filter_map(|s| s.live_session_id.borrow().clone()).collect();
        let metrics = self.metrics_log().map(crate::metrics::Log::editor);
        Ok(move || housekeeping::sweep_empty(&sessions_dir, &database, &keep, metrics.as_ref()))
    }

    /// Settings' Remove Empty Sessions.
    pub(super) fn sweep_empty(&mut self, cx: &mut Cx<Msg>) {
        match self.sweep_work() {
            Ok(work) => {
                let job = self.begin_job("Removing empty sessions…");
                cx.spawn(move || Msg::Swept(job, Arc::new(work())));
            }
            Err(error) => self.note(error, cx),
        }
    }

    pub(super) fn swept(&mut self, job: u64, result: &Result<housekeeping::Sweep, String>, cx: &mut Cx<Msg>) {
        self.finish_job(job);
        match result {
            Ok(sweep) => self.note(sweep.summary(), cx),
            Err(error) => self.note(format!("Empty sessions were not removed: {error}"), cx),
        }
        self.start_discovery();
    }

    fn sweep_for_tool(&mut self, key: SessionKey, call: u64, cx: &mut Cx<Msg>) {
        let work = match self.sweep_work() {
            Ok(work) => work,
            Err(error) => {
                self.answer_origin(Some((key, call)), Err(error), cx);
                return;
            }
        };
        cx.spawn(move || {
            let result = work().map(|sweep| {
                json!({ "summary": sweep.summary(), "removed": sweep.ids.len(), "freed_bytes": sweep.bytes, "left_open": sweep.open })
            });
            Msg::ToolAnswered(key, call, Arc::new(result))
        });
    }

    /// Answer a tool call, or, for the user, say how it went.
    pub(crate) fn answer_origin(&mut self, origin: Origin, result: Result<Value, String>, cx: &mut Cx<Msg>) {
        match origin {
            Some((key, call)) => self.tool_answered(key, call, result, cx),
            None => {
                if let Err(error) = result {
                    self.note(error, cx);
                }
            }
        }
    }

    pub(super) fn tool_answered(&mut self, key: SessionKey, call: u64, result: Result<Value, String>, cx: &mut Cx<Msg>) {
        if let Some(session) = self.live.get(key)
            && let Some(runtime) = session.omp_runtime.get()
        {
            runtime.answer_host_tool(call, result);
        }
        // What the agent changed shows in the sidebar.
        self.start_discovery();
        let _ = cx;
    }
}
