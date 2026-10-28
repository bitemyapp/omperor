//! Sessions as the sidebar offers them: opening one, starting one in a
//! project, forking one (in place, or into a branch and worktree of its
//! own), deleting one, stopping one.

use std::path::PathBuf;
use std::sync::Arc;

use aizome_kit::prelude::*;

use super::{App, Choice, ChoiceAction, Msg, Prompt, PromptAction, Sheet};
use crate::cache_policy::{CacheAction, CacheWarning};
use crate::live_sessions::{Lifecycle, LiveSession, PendingSession, SessionKey};
use crate::omp_runtime;
use crate::preferences::{self, Project};
use crate::session::SessionSummary;
use crate::{housekeeping, workspace};

/// A fork's source and, for a worktree, its branch.
#[derive(Debug, Clone)]
pub(crate) struct ForkPlan {
    pub(crate) parent_session_id: String,
    pub(crate) worktree: Option<(String, String)>,
}

impl App {
    pub(super) fn toggle_project(&mut self, id: String, cx: &mut Cx<Msg>) {
        let Some(project) = self.projects.iter_mut().find(|p| p.id == id) else { return };
        project.collapsed = !project.collapsed;
        let collapsed = project.collapsed;
        self.write(cx, move || preferences::set_project_collapsed(&id, collapsed));
    }

    /// Open the session `id` from the sidebar: the live one if it is, else
    /// its history (resumable when it is Omperor's).
    pub(super) fn open_session(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(summary) = self.sessions.iter().find(|s| s.id == id).cloned() else { return };
        let current = self.session();
        if current.selected_session_path.borrow().as_ref() == Some(&summary.path) {
            return;
        }
        self.capture_views();
        let retained = self.live.find_path(&summary.path);
        let existing = retained.is_some();
        let next = retained.unwrap_or_else(|| self.live.create());
        if !existing {
            seed_context(&next, &current, summary.model.as_deref(), &self.preferences);
            next.selected_session_path.replace(Some(summary.path.clone()));
        }
        self.live.select(next.key);
        self.view_mut(next.key);
        if !existing {
            if summary.is_omperor_owned() {
                if let Some(project) = summary.cwd.clone() {
                    next.current_project.replace(project);
                }
                let project = crate::rules::session_project_position(&summary, &self.projects, &self.records)
                    .map(|i| self.projects[i].id.clone());
                next.active_project_id.replace(project);
                next.live_session_id.replace(Some(summary.id.clone()));
                self.resolve_account(&next);
                self.load_history(&next, summary, false);
                next.chat_status_text.replace("Ready to continue • sending starts this session's agent".into());
            } else {
                self.load_history(&next, summary, true);
            }
            self.inspect_workspace(&next, cx);
            self.index_project_files(&next, cx);
        }
        self.restore_attachments(next.key, cx);
        self.schedule_save_views(cx);
    }

    /// Start a session in `project` (else the one in view).
    pub(super) fn new_session(&mut self, project: Option<String>, cx: &mut Cx<Msg>) {
        let session = self.session();
        let project = project.or_else(|| session.active_project_id.borrow().clone());
        let Some(project) = project.and_then(|id| self.project_by_id(&id).cloned()) else {
            cx.send(Msg::NewProject);
            return;
        };
        match project.directories.as_slice() {
            [] => self.note(format!("{} has no folder • add one in Settings → Projects", project.name), cx),
            [only] => {
                let only = only.clone();
                self.create_session(&project.id, only, None, cx);
            }
            folders => {
                let choice = Choice {
                    title: format!("New session in {}", project.name),
                    message: "Which folder does the agent work in?".into(),
                    options: folders.iter().map(|f| f.display().to_string()).collect(),
                    selected: 0,
                    confirm: "Start".into(),
                    action: ChoiceAction::Folder { project_id: project.id.clone(), folders: folders.to_vec() },
                };
                let sheet = cx.show_sheet(self.main);
                self.sheets.insert(sheet, Sheet::Choice(choice));
            }
        }
    }

    /// Register `path` as a project (or find the one holding it) on a
    /// worker, then start a session there.
    pub(crate) fn open_directory(&mut self, path: PathBuf, cx: &mut Cx<Msg>) {
        let job = self.begin_job(format!("Opening {}…", crate::rules::path_leaf(&path)));
        cx.spawn(move || {
            let result = super::boot::prepare_project(path).map_err(|e| e.to_string());
            Msg::ProjectPrepared(job, Arc::new(result))
        });
    }

    pub(super) fn project_prepared(
        &mut self,
        job: u64,
        result: &Result<(Project, PathBuf), String>,
        cx: &mut Cx<Msg>,
    ) {
        self.finish_job(job);
        match result {
            Ok((project, directory)) => {
                if self.project_by_id(&project.id).is_none() {
                    self.projects.insert(0, project.clone());
                }
                self.create_session(&project.id, directory.clone(), None, cx);
            }
            Err(error) => self.note(format!("Could not open the folder: {error}"), cx),
        }
    }

    /// A new session of its own, selected, its child started at once.
    pub(crate) fn create_session(&mut self, project_id: &str, directory: PathBuf, fork: Option<ForkPlan>, cx: &mut Cx<Msg>) {
        if self.quitting {
            return;
        }
        let source = fork.as_ref().and_then(|plan| {
            self.live
                .entries()
                .into_iter()
                .find(|s| s.live_session_id.borrow().as_deref() == Some(plan.parent_session_id.as_str()))
        });
        if source.as_ref().is_some_and(|source| !self.can_release(source)) {
            self.note("Wait for the source and its delegated work to finish before forking", cx);
            return;
        }
        let model = self.preferences.model.clone().unwrap_or_default();
        let warning = match &fork {
            Some(plan) => self
                .sessions
                .iter()
                .find(|s| s.id == plan.parent_session_id)
                .and_then(|s| s.model.clone())
                .and_then(|model| CacheWarning::for_action(&model, CacheAction::ForkSession)),
            None => CacheWarning::for_action(&model, CacheAction::NewSession),
        };
        self.capture_views();
        let previous = self.session();
        let session = self.live.create();
        seed_context(&session, &previous, None, &self.preferences);
        session.cache_warning.replace(warning);
        session.current_project.replace(directory.clone());
        session.active_project_id.replace(Some(project_id.to_owned()));
        session.new_session_pending.set(true);
        let worktree = fork.as_ref().and_then(|plan| plan.worktree.clone());
        session.pending_session.replace(Some(PendingSession {
            project_id: project_id.to_owned(),
            directory: directory.clone(),
            parent_session_id: fork.as_ref().map(|plan| plan.parent_session_id.clone()),
            branch: worktree.as_ref().map(|(branch, _)| branch.clone()),
            base_branch: worktree.as_ref().map(|(_, base)| base.clone()),
            worktree_path: worktree.is_some().then(|| directory.clone()),
        }));
        self.live.select(session.key);
        self.view_mut(session.key);
        match (source.filter(|s| s.omp_runtime.get().is_some()), fork) {
            (Some(source), Some(plan)) => {
                // The source's child must let go of its journal before the
                // fork's child can read it.
                self.pending_forks.insert(source.key, (session.key, plan.parent_session_id));
                source.lifecycle.set(Lifecycle::Retiring);
                source.runtime_ready.set(false);
                if !source.omp_runtime.get().is_some_and(|r| r.release_session()) {
                    self.pending_forks.remove(&source.key);
                    source.lifecycle.set(Lifecycle::Failed);
                    session.new_session_pending.set(false);
                    session.lifecycle.set(Lifecycle::Failed);
                    session.chat_status_text.replace("The source's agent is unavailable; choose Fork again".into());
                }
            }
            (_, fork) => {
                let target = fork.map_or(omp_runtime::SessionStartup::Fresh, |plan| {
                    omp_runtime::SessionStartup::Fork(plan.parent_session_id)
                });
                self.resolve_account(&session);
                let account = session.current_account.borrow().clone();
                self.launch_runtime(&session, account, target);
            }
        }
        self.inspect_workspace(&session, cx);
        self.index_project_files(&session, cx);
        cx.focus("composer");
    }

    fn can_release(&self, source: &LiveSession) -> bool {
        !self.quitting
            && !self.fork_owns(source.key)
            && !source.chat_busy.get()
            && !source.runtime_starting.get()
            && !source.attachment_submission.get()
            && source.pending_prompt.borrow().is_none()
            && !source.account_change_pending.get()
            && !matches!(source.lifecycle.get(), Lifecycle::Stopping | Lifecycle::Retiring)
            && !source.subagents.borrow().iter().any(|a| crate::agents::AgentState::from_status(&a.status).is_busy())
    }

    fn summary(&self, id: &str) -> Option<SessionSummary> {
        self.sessions.iter().find(|s| s.id == id).cloned()
    }

    /// The project a session belongs to, or one made for its folder.
    fn project_of(&self, summary: &SessionSummary) -> Option<Project> {
        crate::rules::session_project_position(summary, &self.projects, &self.records).map(|i| self.projects[i].clone())
    }

    /// Fork in place: a new session in the same folder, seeded with the
    /// source's transcript.
    pub(super) fn fork_in_place(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(summary) = self.summary(id) else { return };
        let Some(directory) = summary.cwd.clone() else {
            self.note("This session's folder is unavailable, so it cannot be forked", cx);
            return;
        };
        let Some(project) = self.project_of(&summary) else {
            self.note("This session belongs to no project; open its folder as a project first", cx);
            return;
        };
        self.create_session(
            &project.id,
            directory,
            Some(ForkPlan { parent_session_id: summary.id.clone(), worktree: None }),
            cx,
        );
    }

    /// Fork into a worktree: probe the repository on a worker, ask for a
    /// branch, add the worktree on a worker, then start the fork there.
    pub(super) fn fork_into_worktree(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(summary) = self.summary(id) else { return };
        let Some(directory) = summary.cwd.clone() else {
            self.note("This session's folder is unavailable, so it cannot be forked", cx);
            return;
        };
        let Some(project) = self.project_of(&summary) else {
            self.note("This session belongs to no project; open its folder as a project first", cx);
            return;
        };
        let job = self.begin_job(format!("Inspecting the repository in {}…", directory.display()));
        let project_id = project.id;
        cx.spawn(move || {
            let result = workspace::branch_point(&directory).map_err(|e| e.to_string());
            Msg::ForkProbed(job, Arc::new((summary, directory, project_id, result)))
        });
    }

    pub(super) fn fork_probed(
        &mut self,
        job: u64,
        probe: &(SessionSummary, PathBuf, String, Result<Option<workspace::BranchPoint>, String>),
        cx: &mut Cx<Msg>,
    ) {
        self.finish_job(job);
        let (summary, directory, project_id, result) = probe;
        let point = match result {
            Ok(Some(point)) => point.clone(),
            Ok(None) => {
                cx.alert(
                    self.main,
                    Alert::new(
                        "This session cannot be forked into a worktree",
                        format!("{} is not inside a git repository. Worktrees need a repository to branch from.", directory.display()),
                    )
                    .default_button("OK", Msg::ClearNote),
                );
                return;
            }
            Err(error) => {
                self.note(format!("Could not inspect the session's repository: {error}"), cx);
                return;
            }
        };
        let prompt = Prompt {
            title: "Fork into a new worktree".into(),
            message: format!(
                "A new branch from {} and a worktree for it; the fork continues the conversation there.",
                point.branch.clone().unwrap_or_else(|| point.head.chars().take(12).collect())
            ),
            value: crate::rules::default_fork_branch_name(&summary.title, &summary.id),
            confirm: "Fork".into(),
            action: PromptAction::ForkBranch {
                summary: Box::new(summary.clone()),
                directory: directory.clone(),
                project_id: project_id.clone(),
                point,
            },
            error: None,
        };
        let sheet = cx.show_sheet(self.main);
        self.sheets.insert(sheet, Sheet::Prompt(prompt));
    }

    /// The branch was named: add the worktree on a worker.
    pub(super) fn add_worktree(
        &mut self,
        summary: &SessionSummary,
        directory: PathBuf,
        project_id: String,
        point: workspace::BranchPoint,
        branch: String,
        cx: &mut Cx<Msg>,
    ) {
        let root = match preferences::worktrees_directory() {
            Ok(root) => root,
            Err(error) => {
                self.note(format!("Could not find the worktree directory: {error}"), cx);
                return;
            }
        };
        let path = root.join(&project_id).join(branch.replace('/', "-"));
        let job = self.begin_job(format!("Creating a worktree on branch {branch}…"));
        let parent = summary.id.clone();
        cx.spawn(move || {
            let result = workspace::add_worktree(&directory, &branch, &path).map_err(|e| e.to_string());
            Msg::WorktreeAdded(job, Arc::new((project_id, path, format!("{branch}\n{parent}"), result, point)))
        });
    }

    pub(super) fn worktree_added(
        &mut self,
        job: u64,
        added: &(String, PathBuf, String, Result<(), String>, workspace::BranchPoint),
        cx: &mut Cx<Msg>,
    ) {
        self.finish_job(job);
        let (project_id, path, names, result, point) = added;
        if let Err(error) = result {
            self.note(format!("Could not create the worktree: {error}"), cx);
            return;
        }
        let (branch, parent) = names.split_once('\n').unwrap_or((names.as_str(), ""));
        let base = point.branch.clone().unwrap_or_else(|| point.head.chars().take(12).collect());
        let plan = ForkPlan { parent_session_id: parent.to_owned(), worktree: Some((branch.to_owned(), base)) };
        self.create_session(project_id, path.clone(), Some(plan), cx);
    }

    pub(super) fn ask_delete_session(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(summary) = self.summary(id) else { return };
        let busy = self
            .live
            .entries()
            .iter()
            .any(|s| s.live_session_id.borrow().as_deref() == Some(id) && (s.chat_busy.get() || s.runtime_starting.get()));
        if busy {
            self.note("Stop the session before deleting it", cx);
            return;
        }
        let worktree = self.records.get(id).and_then(|r| r.worktree_path.clone());
        let mut message = "Its transcript, its sub-agents' transcripts and its metrics are deleted. Files it changed stay.".to_owned();
        if let Some(worktree) = &worktree {
            message.push_str(&format!(" Its worktree at {} is removed if it has no changes; its branch stays.", worktree.display()));
        }
        let id = id.to_owned();
        cx.alert(
            self.main,
            Alert::new(format!("Delete “{}”?", summary.title), message)
                .destructive_button("Delete", Msg::ConfirmDeleteSession(id))
                .cancel_button("Cancel"),
        );
    }

    pub(super) fn delete_session(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(summary) = self.summary(id) else { return };
        let (Some(sessions_dir), Ok(database)) = (self.sessions_dir.clone(), preferences::database_path()) else { return };
        // A live session's child is stopped first; the worker waits for it
        // to let go of the journal.
        let live = self.live.entries().into_iter().find(|s| s.live_session_id.borrow().as_deref() == Some(id));
        if let Some(session) = &live {
            if self.session().key == session.key {
                self.select_other(session.key, cx);
            }
            if let Some(runtime) = session.omp_runtime.get() {
                runtime.begin_shutdown();
            }
            self.live.remove(session.key);
            self.views.remove(&session.key);
        }
        let worktree = self.records.get(id).and_then(|r| r.worktree_path.clone()).filter(|path| {
            // Only a worktree no other session uses.
            !self.records.values().any(|r| r.session_id != id && r.worktree_path.as_ref() == Some(path))
        });
        let target = housekeeping::Target {
            id: summary.id.clone(),
            title: summary.title.clone(),
            journal: summary.is_omperor_owned(),
            worktree,
        };
        let metrics = self.metrics_log().map(crate::metrics::Log::editor);
        self.removed_session_ids.insert(summary.id.clone());
        self.sessions.retain(|s| s.id != summary.id);
        let job = self.begin_job(format!("Deleting “{}”…", summary.title));
        cx.spawn(move || {
            let request = housekeeping::Request { sessions_dir, database, targets: vec![target], metrics };
            let mut report = housekeeping::delete(request.clone());
            // The child just told to stop may hold the journal a moment.
            for _ in 0..40 {
                if !report.failed.iter().any(|f| f.reason.contains("has it open")) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                report = housekeeping::delete(request.clone());
            }
            Msg::Deleted(job, Arc::new(report))
        });
    }

    pub(super) fn deleted(&mut self, job: u64, report: &housekeeping::Report, cx: &mut Cx<Msg>) {
        self.finish_job(job);
        for removed in &report.removed {
            self.records.remove(&removed.id);
        }
        for failed in &report.failed {
            self.removed_session_ids.remove(&failed.id);
        }
        self.note(report.summary(), cx);
        self.start_discovery();
    }

    /// Select a session other than `key`: a draft of its project.
    pub(crate) fn select_other(&mut self, key: SessionKey, cx: &mut Cx<Msg>) {
        if let Some(other) = self.live.entries().into_iter().find(|s| s.key != key) {
            self.live.select(other.key);
            self.view_mut(other.key);
            return;
        }
        let session = self.session();
        let project = session.active_project_id.borrow().clone();
        let directory = session.current_project.borrow().clone();
        if let Some(project) = project {
            self.create_session(&project, directory, None, cx);
        }
    }

    pub(super) fn stop(&mut self, key: SessionKey, cx: &mut Cx<Msg>) {
        let Some(session) = self.live.get(key) else { return };
        // A stop with no turn running would stay queued in OMP and cancel
        // the next prompt; a second one while the first is in flight too.
        if !session.chat_busy.get() || session.lifecycle.get() == Lifecycle::Stopping {
            return;
        }
        if let Some(runtime) = session.omp_runtime.get() {
            if runtime.abort() {
                session.lifecycle.set(Lifecycle::Stopping);
                session.chat_status_text.replace("Stopping…".into());
            } else {
                self.note("The agent stopped answering; start a new session", cx);
            }
        }
    }
}

/// A new session starts with what the one in view knows: its model, its
/// reasoning level, the accounts and models OMP reported.
fn seed_context(target: &LiveSession, source: &LiveSession, model: Option<&str>, prefs: &preferences::Preferences) {
    let model = model
        .filter(|m| !m.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let m = source.current_model.borrow();
            (!m.trim().is_empty()).then(|| m.clone())
        })
        .or_else(|| prefs.model.clone())
        .unwrap_or_default();
    target.current_model.replace(model.clone());
    let mut models = source.available_models.borrow().clone();
    if !model.is_empty() && !models.contains(&model) {
        models.insert(0, model);
    }
    target.available_models.replace(models);
    target.fast_models.replace(source.fast_models.borrow().clone());
    target.fast_mode.set(source.fast_mode.get());
    target.current_thinking_level.replace(source.current_thinking_level.borrow().clone());
    target.authenticated_providers.replace(source.authenticated_providers.borrow().clone());
    target.accounts_known.set(source.accounts_known.get());
    target.current_account.replace(source.current_account.borrow().clone());
}
