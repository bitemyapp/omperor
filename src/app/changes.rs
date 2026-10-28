//! The Changes panel: the project's uncommitted work, read with git on a
//! worker whenever a tool may have changed files or the folder's watcher
//! says something did, and the diff of the file selected.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aizome_kit::prelude::*;

use super::{App, Diff, Msg, Panel};
use crate::file_watcher::{ChangeFilter, ProjectWatcher};
use crate::live_sessions::{LiveSession, SessionKey};
use crate::workspace;

/// How long the folder must be quiet before it is read again.
const WATCH_QUIET: Duration = Duration::from_millis(900);

impl App {
    /// Read the session's working tree again (unless a read is under way;
    /// then once more after it).
    pub(crate) fn inspect_workspace(&mut self, session: &LiveSession, cx: &mut Cx<Msg>) {
        if self.inspecting.contains(&session.key) {
            session.workspace_refresh_pending.set(true);
            return;
        }
        self.inspecting.insert(session.key);
        let generation = self.inspection_generation.entry(session.key).or_default();
        *generation += 1;
        let generation = *generation;
        let key = session.key;
        let project = session.current_project.borrow().clone();
        let covered = crate::file_watcher::current_event_id();
        if let Some(watcher) = session.project_watcher.borrow().as_ref() {
            watcher.mark_covered_through(covered);
        }
        cx.spawn(move || {
            let result = workspace::inspect(&project).map_err(|e| e.to_string());
            Msg::Inspected(key, generation, Arc::new(result))
        });
    }

    pub(super) fn inspected(
        &mut self,
        key: SessionKey,
        generation: u64,
        result: &Result<workspace::WorkspaceSnapshot, String>,
        cx: &mut Cx<Msg>,
    ) {
        self.inspecting.remove(&key);
        let Some(session) = self.live.get(key) else { return };
        if self.inspection_generation.get(&key) != Some(&generation) {
            return;
        }
        match result {
            Ok(snapshot) => {
                session.workspace_read_failures.borrow_mut().clear();
                session.workspace_snapshot.replace(Some(snapshot.clone()));
            }
            Err(error) => {
                session.workspace_read_failures.borrow_mut().insert(String::new(), error.clone());
            }
        }
        // The diff in view follows: read again, or gone with its file.
        let selected = session.selected_change.borrow().clone();
        if let Some(path) = selected {
            let still = session
                .workspace_snapshot
                .borrow()
                .as_ref()
                .is_some_and(|s| s.changes.iter().any(|c| c.path == path));
            if still {
                self.load_diff(&session, path, cx);
            } else {
                session.selected_change.replace(None);
                self.view_mut(key).diff = None;
            }
        }
        if session.workspace_refresh_pending.replace(false) {
            self.inspect_workspace(&session, cx);
        }
    }

    pub(super) fn select_change(&mut self, path: PathBuf, cx: &mut Cx<Msg>) {
        let session = self.session();
        session.selected_change.replace(Some(path.clone()));
        self.view_mut(session.key).view.desk.selected_change = Some(path.clone());
        self.load_diff(&session, path, cx);
    }

    fn load_diff(&mut self, session: &LiveSession, path: PathBuf, cx: &mut Cx<Msg>) {
        let Some(snapshot) = session.workspace_snapshot.borrow().clone() else { return };
        let Some(change) = snapshot.changes.iter().find(|c| c.path == path).cloned() else { return };
        let key = session.key;
        let view = self.view_mut(key);
        if !matches!(&view.diff, Some((p, _)) if *p == path) {
            view.diff = Some((path.clone(), Diff::Loading));
        }
        cx.spawn(move || match workspace::diff(&snapshot, &change) {
            // The file went since the folder was read: read it again.
            Err(error) if error.is_stale() => Msg::RefreshChanges,
            result => {
                let result = result.map(|text| crate::markdown::highlight::lines("diff", &text)).map_err(|e| e.to_string());
                Msg::DiffLoaded(key, path, Arc::new(result))
            }
        });
    }

    pub(super) fn diff_loaded(
        &mut self,
        key: SessionKey,
        path: PathBuf,
        result: &Result<Vec<Vec<crate::markdown::Run>>, String>,
    ) {
        let Some(view) = self.views.get_mut(&key) else { return };
        if !matches!(&view.diff, Some((p, _)) if *p == path) {
            return;
        }
        view.diff = Some((
            path,
            match result {
                Ok(lines) => Diff::Ready(Arc::new(lines.clone())),
                Err(error) => Diff::Failed(error.clone()),
            },
        ));
    }

    pub(super) fn show_panel(&mut self, panel: Panel, cx: &mut Cx<Msg>) {
        let session = self.session();
        let view = self.view_mut(session.key);
        let showing = view.panel() == Some(panel);
        view.view.desk.panel = panel.index();
        view.view.desk.workspace_inspector = !showing;
        if panel == Panel::Changes && !showing {
            self.inspect_workspace(&session, cx);
        }
        if panel == Panel::Agents && !showing {
            self.request_transcripts(&session, true, cx);
        }
        self.schedule_save_views(cx);
    }

    /// Watch the session's folder for changes made outside the agent (a
    /// terminal, an editor), so the panel follows them.
    pub(crate) fn watch_project(&mut self, session: &LiveSession, dir: PathBuf, ignore_dirs: &[PathBuf], cx: &mut Cx<Msg>) {
        let _ = cx;
        let same = session.project_watcher.borrow().as_ref().is_some_and(|w| w.root() == dir);
        if !same {
            match ProjectWatcher::start(dir.clone(), self.watch_sender.clone()) {
                Ok(watcher) => {
                    session.project_watcher.replace(Some(watcher));
                }
                Err(_) => return,
            }
        }
        let ignore_dirs = ignore_dirs.to_vec();
        if let Some(watcher) = session.project_watcher.borrow().as_ref() {
            // The ignore files are read here: they are few and small, and
            // the index walk just listed them.
            watcher.set_filter(ChangeFilter::build(&dir, &ignore_dirs));
        }
    }

    /// The watchers' notices: read the folders again once they are quiet.
    pub(crate) fn poll_watchers(&mut self, cx: &mut Cx<Msg>) {
        let mut roots = Vec::new();
        while let Ok(root) = self.watch_notices.try_recv() {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        if roots.is_empty() {
            return;
        }
        let now = Instant::now();
        for session in self.live.entries() {
            if roots.iter().any(|root| *root == *session.current_project.borrow()) {
                session.workspace_change_seen_at.set(Some(now));
            }
        }
        cx.after(WATCH_QUIET, Msg::WatchSettled);
    }

    pub(super) fn watch_settled(&mut self, cx: &mut Cx<Msg>) {
        for session in self.live.entries() {
            let quiet = session.workspace_change_seen_at.get().is_some_and(|at| at.elapsed() >= WATCH_QUIET - Duration::from_millis(50));
            if quiet {
                session.workspace_change_seen_at.set(None);
                self.inspect_workspace(&session, cx);
                // New files belong in the `@` picker too.
                session.project_files_dir.replace(None);
                self.index_project_files(&session, cx);
            }
        }
    }

    pub(super) fn open_link(&mut self, target: &str, cx: &mut Cx<Msg>) {
        if target.starts_with("http://") || target.starts_with("https://") || target.starts_with("mailto:") {
            cx.open_url(target);
            return;
        }
        // A path the agent wrote: relative to the session's folder.
        let session = self.session();
        let path = target.strip_prefix("file://").unwrap_or(target);
        let path = path.split_once('#').map_or(path, |(p, _)| p);
        let path = Path::new(path);
        let path = if path.is_relative() { session.current_project.borrow().join(path) } else { path.to_owned() };
        let target = target.to_owned();
        cx.spawn(move || if path.exists() { Msg::OpenInEditor(path) } else { Msg::OpenUrl(target) });
    }
}
