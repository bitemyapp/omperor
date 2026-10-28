//! Projects (a name, the folders it holds, its sessions) and their
//! archives: a project put away into one compressed file with everything
//! of its sessions, and brought back later. Registry writes and archive
//! work run on workers.

use std::path::PathBuf;
use std::sync::Arc;

use aizome_kit::prelude::*;

use super::{App, Msg, Pane, Prompt, PromptAction, Sheet};
use crate::live_sessions::{Lifecycle, SessionKey};
use crate::preferences::{self, Project};
use crate::{archive, rules};

/// Who asked for an archive operation: the user, or an agent's tool call
/// waiting for the answer.
pub(crate) type Origin = Option<(SessionKey, u64)>;

impl App {
    pub(super) fn ask_rename_project(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(project) = self.project_by_id(id).cloned() else { return };
        let prompt = Prompt {
            title: "Rename project".into(),
            message: rules::project_tooltip(&project),
            value: project.name.clone(),
            confirm: "Rename".into(),
            action: PromptAction::RenameProject(id.to_owned()),
            error: None,
        };
        let sheet = cx.show_sheet(self.main_or_settings());
        self.sheets.insert(sheet, Sheet::Prompt(prompt));
    }

    pub(crate) fn rename_project(&mut self, id: String, name: String, cx: &mut Cx<Msg>) {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return;
        }
        if let Some(project) = self.projects.iter_mut().find(|p| p.id == id) {
            project.name = name.clone();
        }
        self.write(cx, move || preferences::rename_project(&id, &name));
    }

    pub(super) fn folder_chosen(&mut self, id: &str, paths: Option<Vec<PathBuf>>, cx: &mut Cx<Msg>) {
        let Some(folder) = paths.and_then(|p| p.into_iter().next()) else { return };
        if let Some(project) = self.projects.iter_mut().find(|p| p.id == id)
            && !project.directories.contains(&folder)
        {
            project.directories.push(folder.clone());
            self.missing_directories.remove(&folder);
            let id = id.to_owned();
            self.write(cx, move || preferences::add_project_directory(&id, &folder));
        }
    }

    pub(super) fn remove_folder(&mut self, id: &str, folder: PathBuf, cx: &mut Cx<Msg>) {
        if let Some(project) = self.projects.iter_mut().find(|p| p.id == id) {
            project.directories.retain(|d| *d != folder);
            let id = id.to_owned();
            self.write(cx, move || preferences::remove_project_directory(&id, &folder));
        }
    }

    pub(super) fn ask_remove_project(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(project) = self.project_by_id(id).cloned() else { return };
        cx.alert(
            self.main_or_settings(),
            Alert::new(
                format!("Remove “{}” from Omperor?", project.name),
                "Its sessions stay on disk and show under Other folders. Its folders and code are untouched.",
            )
            .destructive_button("Remove", Msg::ConfirmRemoveProject(id.to_owned()))
            .cancel_button("Cancel"),
        );
    }

    pub(super) fn remove_project(&mut self, id: &str, cx: &mut Cx<Msg>) {
        self.projects.retain(|p| p.id != id);
        let id = id.to_owned();
        self.write(cx, move || preferences::delete_project(&id));
    }

    pub(super) fn registry_reloaded(&mut self, result: &Result<preferences::Registry, String>) {
        if let Ok(registry) = result {
            self.projects = registry.projects.clone();
            self.records = registry.records.iter().map(|r| (r.session_id.clone(), r.clone())).collect();
            self.positions = registry.positions.clone();
            self.missing_directories = registry.missing_directories.clone();
        }
    }

    fn reload_registry(&mut self, cx: &mut Cx<Msg>) {
        cx.spawn(|| Msg::RegistryReloaded(None, Arc::new(preferences::load_registry().map_err(|e| e.to_string()))));
    }

    // Archives.

    /// What archiving `id` would take: its members, or why it can't now.
    fn plan_archive(&self, id: &str, requester: Option<SessionKey>) -> Result<(Project, Vec<archive::Member>, Vec<PathBuf>), String> {
        let position = self.projects.iter().position(|p| p.id == id).ok_or_else(|| format!("there is no project with the id {id}"))?;
        let project = self.projects[position].clone();
        let mut members = Vec::new();
        let mut busy = Vec::new();
        for summary in self.sessions.iter().filter(|s| {
            s.is_omperor_owned() && rules::session_project_position(s, &self.projects, &self.records) == Some(position)
        }) {
            let live = self.live.entries().into_iter().find(|s| s.live_session_id.borrow().as_deref() == Some(summary.id.as_str()));
            if let Some(live) = &live {
                if Some(live.key) == requester {
                    return Err("the session making this request is one of its sessions".into());
                }
                if live.chat_busy.get() || live.runtime_starting.get() {
                    busy.push(format!("“{}”", summary.title));
                    continue;
                }
            }
            let view = self.saved_views.get(&Some(summary.path.clone())).and_then(|v| serde_json::to_value(v).ok());
            members.push(archive::Member {
                id: summary.id.clone(),
                title: summary.title.clone(),
                journal: !summary.is_mock(),
                view,
                draft_key: None,
            });
        }
        if !busy.is_empty() {
            return Err(format!("some of its sessions are working: {}", busy.join(", ")));
        }
        let mut worktrees: Vec<PathBuf> =
            self.records.values().filter(|r| r.project_id == project.id).filter_map(|r| r.worktree_path.clone()).collect();
        worktrees.sort();
        worktrees.dedup();
        Ok((project, members, worktrees))
    }

    pub(super) fn ask_archive_project(&mut self, id: &str, cx: &mut Cx<Msg>) {
        match self.plan_archive(id, None) {
            Ok((project, members, worktrees)) => {
                let mut message = format!(
                    "Its {} session{} go into one compressed file in Omperor's archives, to bring back later from Settings → Archives.",
                    members.len(),
                    crate::housekeeping::plural(members.len())
                );
                message.push_str(" Its folders and code stay where they are");
                if !worktrees.is_empty() {
                    message.push_str(", and so do its worktrees");
                }
                message.push('.');
                cx.alert(
                    self.main_or_settings(),
                    Alert::new(format!("Archive “{}”?", project.name), message)
                        .default_button("Archive", Msg::ConfirmArchiveProject(id.to_owned()))
                        .cancel_button("Cancel"),
                );
            }
            Err(why) => self.note(format!("The project can't be archived now: {why}"), cx),
        }
    }

    pub(super) fn archive_project(&mut self, id: &str, cx: &mut Cx<Msg>) {
        self.archive_project_for(id, None, cx);
    }

    pub(crate) fn archive_project_for(&mut self, id: &str, origin: Origin, cx: &mut Cx<Msg>) {
        let requester = origin.map(|(key, _)| key);
        let (project, members, worktrees) = match self.plan_archive(id, requester) {
            Ok(plan) => plan,
            Err(why) => {
                self.answer_origin(origin, Err(format!("The project was not archived: {why}.")), cx);
                return;
            }
        };
        let (Some(sessions_dir), Ok(database), Ok(archives_dir)) =
            (self.sessions_dir.clone(), preferences::database_path(), preferences::archives_directory())
        else {
            self.answer_origin(origin, Err("Omperor's storage is unavailable".into()), cx);
            return;
        };
        // Its live sessions let go of their journals first.
        let ids: Vec<String> = members.iter().map(|m| m.id.clone()).collect();
        for session in self.live.entries() {
            let id = session.live_session_id.borrow().clone();
            if id.as_ref().is_some_and(|id| ids.contains(id)) {
                if self.session().key == session.key {
                    self.select_other(session.key, cx);
                }
                if let Some(runtime) = session.omp_runtime.get() {
                    runtime.begin_shutdown();
                }
                session.lifecycle.set(Lifecycle::Retiring);
                self.live.remove(session.key);
                self.views.remove(&session.key);
            }
        }
        let fallback = self
            .projects
            .iter()
            .find(|other| other.id != project.id && !other.directories.is_empty())
            .map(|other| other.directories[0].clone());
        let metrics = self.metrics_log().map(crate::metrics::Log::editor);
        let request = archive::Request {
            archives_dir,
            sessions_dir,
            database,
            project_id: project.id.clone(),
            members,
            worktrees,
            active_fallback: fallback,
            metrics,
        };
        self.projects.retain(|p| p.id != project.id);
        self.sessions.retain(|s| !ids.contains(&s.id));
        self.removed_session_ids.extend(ids);
        let job = self.begin_job(format!("Archiving “{}”…", project.name));
        self.archive_origins.insert(job, origin);
        cx.spawn(move || {
            let mut result = archive::archive(request.clone());
            // A child just told to stop may hold its journal a moment.
            for _ in 0..40 {
                match &result {
                    Err(error) if error.contains("has it open") => {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        result = archive::archive(request.clone());
                    }
                    _ => break,
                }
            }
            Msg::Archived(job, Arc::new(result))
        });
    }

    pub(super) fn archived(&mut self, job: u64, result: &Result<archive::Archived, String>, cx: &mut Cx<Msg>) {
        self.finish_job(job);
        let origin = self.archive_origins.remove(&job).flatten();
        match result {
            Ok(archived) => {
                let manifest = &archived.listing.manifest;
                let summary = format!("Archived “{}” with {} sessions", manifest.project.name, manifest.session_count());
                self.note(summary.clone(), cx);
                self.answer_origin(
                    origin,
                    Ok(serde_json::json!({ "summary": summary, "archive": super::tools::listing_answer(&archived.listing), "warnings": archived.warnings })),
                    cx,
                );
            }
            Err(error) => {
                self.note(format!("The project was not archived: {error}"), cx);
                self.answer_origin(origin, Err(error.clone()), cx);
            }
        }
        self.reload_registry(cx);
        self.start_discovery();
        self.list_archives(cx);
    }

    pub(crate) fn list_archives(&mut self, cx: &mut Cx<Msg>) {
        let Ok(dir) = preferences::archives_directory() else { return };
        cx.spawn(move || Msg::ArchivesListed(Arc::new(archive::list(&dir))));
    }

    pub(super) fn restore_archive(&mut self, id: &str, cx: &mut Cx<Msg>) {
        self.restore_archive_for(id, None, cx);
    }

    pub(crate) fn restore_archive_for(&mut self, id: &str, origin: Origin, cx: &mut Cx<Msg>) {
        let (Some(sessions_dir), Ok(database), Ok(archives_dir)) =
            (self.sessions_dir.clone(), preferences::database_path(), preferences::archives_directory())
        else {
            self.answer_origin(origin, Err("Omperor's storage is unavailable".into()), cx);
            return;
        };
        let metrics = self.metrics_log().map(crate::metrics::Log::editor);
        let id = id.to_owned();
        let job = self.begin_job("Restoring an archived project…");
        self.archive_origins.insert(job, origin);
        cx.spawn(move || {
            let result = archive::restore(&archives_dir, &id, &sessions_dir, &database, metrics.as_ref());
            Msg::Restored(job, Arc::new(result))
        });
    }

    pub(super) fn restored(&mut self, job: u64, result: &Result<archive::Restored, String>, cx: &mut Cx<Msg>) {
        self.finish_job(job);
        let origin = self.archive_origins.remove(&job).flatten();
        match result {
            Ok(restored) => {
                let ids: Vec<&String> = restored.manifest.sessions.iter().map(|s| &s.id).collect();
                self.removed_session_ids.retain(|id| !ids.contains(&id));
                let mut summary = format!("Restored “{}”", restored.manifest.project.name);
                match restored.taken_directories.len() {
                    0 => {}
                    1 => summary.push_str(" · one of its folders stays with the project that took it since"),
                    n => summary.push_str(&format!(" · {n} of its folders stay with the projects that took them since")),
                }
                self.note(summary.clone(), cx);
                self.answer_origin(origin, Ok(serde_json::json!({ "summary": summary, "warnings": restored.warnings })), cx);
            }
            Err(error) => {
                self.note(format!("The archive was not restored: {error}"), cx);
                self.answer_origin(origin, Err(error.clone()), cx);
            }
        }
        self.reload_registry(cx);
        self.start_discovery();
        self.list_archives(cx);
    }

    pub(super) fn ask_delete_archive(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let name = self.archive_name(id);
        cx.alert(
            self.main_or_settings(),
            Alert::new(
                format!("Delete the archive of {name}?"),
                "The archived project and its sessions are deleted for good and can't be restored.",
            )
            .destructive_button("Delete", Msg::ConfirmDeleteArchive(id.to_owned()))
            .cancel_button("Cancel"),
        );
    }

    pub(crate) fn archive_name(&self, id: &str) -> String {
        self.archives
            .as_ref()
            .and_then(|a| a.as_ref().ok())
            .and_then(|list| list.iter().find(|l| l.manifest.id == id))
            .map_or_else(|| "this project".to_owned(), |l| format!("“{}”", l.manifest.project.name))
    }

    pub(super) fn delete_archive(&mut self, id: &str, cx: &mut Cx<Msg>) {
        self.delete_archive_for(id, None, cx);
    }

    pub(crate) fn delete_archive_for(&mut self, id: &str, origin: Origin, cx: &mut Cx<Msg>) {
        let Ok(dir) = preferences::archives_directory() else {
            self.answer_origin(origin, Err("Omperor's storage is unavailable".into()), cx);
            return;
        };
        let id = id.to_owned();
        let job = self.begin_job("Deleting an archive…");
        self.archive_origins.insert(job, origin);
        cx.spawn(move || Msg::ArchiveDeleted(job, Arc::new(archive::delete(&dir, &id))));
    }

    pub(super) fn archive_deleted(&mut self, job: u64, result: &Result<u64, String>, cx: &mut Cx<Msg>) {
        self.finish_job(job);
        let origin = self.archive_origins.remove(&job).flatten();
        match result {
            Ok(bytes) => {
                let summary = format!("Archive deleted, {} freed", crate::housekeeping::byte_text(*bytes));
                self.note(summary.clone(), cx);
                self.answer_origin(origin, Ok(serde_json::json!({ "summary": summary, "freed_bytes": bytes })), cx);
            }
            Err(error) => {
                self.note(format!("The archive was not deleted: {error}"), cx);
                self.answer_origin(origin, Err(error.clone()), cx);
            }
        }
        self.list_archives(cx);
    }

    /// Settings, open at `pane`.
    pub(crate) fn open_settings(&mut self, pane: Pane, cx: &mut Cx<Msg>) {
        match self.settings {
            Some(window) => cx.activate(window),
            None => {
                let window = cx.open_window(WindowSpec::new("Omperor Settings").size(860.0, 600.0).min_size(720.0, 460.0));
                self.settings = Some(window);
            }
        }
        self.show_pane(pane, cx);
    }

    pub(super) fn show_pane(&mut self, pane: Pane, cx: &mut Cx<Msg>) {
        self.pane = pane;
        match pane {
            Pane::Accounts => self.request_account_usage(false, cx),
            Pane::Archives => self.list_archives(cx),
            Pane::Metrics => self.load_metrics(cx),
            _ => {}
        }
    }
}

impl App {
    /// Move project `id` up (`by` < 0) or down the sidebar.
    pub(super) fn move_project(&mut self, id: &str, by: isize, cx: &mut Cx<Msg>) {
        let Some(at) = self.projects.iter().position(|p| p.id == id) else { return };
        let Some(to) = at.checked_add_signed(by).filter(|to| *to < self.projects.len()) else { return };
        self.projects.swap(at, to);
        let order: Vec<String> = self.projects.iter().map(|p| p.id.clone()).collect();
        self.write(cx, move || preferences::set_project_order(&order));
    }

    /// Move session `id` up or down among its project's sessions, as the
    /// sidebar lists them; the whole project's order is kept from then on.
    pub(super) fn move_session(&mut self, id: &str, by: isize, cx: &mut Cx<Msg>) {
        let rows = rules::sidebar_rows_for(
            &self.projects,
            &self.sessions,
            &self.records,
            &self.positions,
            rules::SessionSourceFilter::Omperor,
            "",
        );
        let project_of = |row: &rules::SidebarRow| match row {
            rules::SidebarRow::Session { index, project_id: Some(project) } => Some((*index, project.clone())),
            _ => None,
        };
        let Some(project) = rows.iter().filter_map(project_of).find(|(i, _)| self.sessions[*i].id == id).map(|(_, p)| p) else {
            return;
        };
        let mut order: Vec<String> = rows
            .iter()
            .filter_map(project_of)
            .filter(|(_, p)| *p == project)
            .map(|(i, _)| self.sessions[i].id.clone())
            .collect();
        let Some(at) = order.iter().position(|s| s == id) else { return };
        let Some(to) = at.checked_add_signed(by).filter(|to| *to < order.len()) else { return };
        order.swap(at, to);
        for (position, id) in order.iter().enumerate() {
            self.positions.insert(id.clone(), position as i64);
        }
        self.write(cx, move || preferences::set_session_order(&order));
    }

    /// Ask the providers again which models they offer; the session in
    /// view restarts its child to take them in.
    pub(super) fn refresh_models(&mut self, cx: &mut Cx<Msg>) {
        let session = self.session();
        if session.omp_runtime.get().is_some_and(|runtime| runtime.refresh_models()) {
            session.chat_status_text.replace("Asking the providers for their models…".into());
        } else {
            self.note("Models are read when a session starts; start one first", cx);
        }
    }
}
