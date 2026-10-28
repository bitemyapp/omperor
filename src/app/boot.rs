//! Starting up: what is read from disk before the window can work, on a
//! worker, and what the program does with it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use aizome_kit::Cx;

use super::{App, Msg};
use crate::agents;
use crate::live_sessions::SessionKey;
use crate::omp_runtime::{self, AuthProvider};
use crate::preferences::{self, Preferences, Project, SessionRecord, StoredAccount};
use crate::rules::unique_project_name;

/// What startup read.
#[derive(Debug)]
pub(crate) struct Boot {
    preferences: Preferences,
    directory: PathBuf,
    projects: Vec<Project>,
    missing_directories: HashSet<PathBuf>,
    records: Vec<SessionRecord>,
    positions: HashMap<String, i64>,
    bootstrap_pending: bool,
    active: String,
    data: PathBuf,
    sessions: PathBuf,
    account: Option<String>,
    account_metadata: Vec<StoredAccount>,
    account_choices: HashMap<(String, String), String>,
    views: Vec<(Option<PathBuf>, agents::ViewState)>,
    launchers: crate::launchers::Installed,
    helper: PathBuf,
}

impl Boot {
    /// Read the preferences and the registry, registering the folder the
    /// app starts in as a project when no project holds it.
    pub(crate) fn load() -> Result<Boot, preferences::PreferencesError> {
        let preferences = preferences::load()?;
        let directory =
            preferences.project.clone().filter(|path| path.is_dir()).unwrap_or_else(omp_runtime::default_project_root);
        let mut projects = preferences::load_projects()?;
        let records = preferences::load_session_records()?;
        let owner = projects
            .iter()
            .find(|project| {
                project.directories.contains(&directory)
                    || records.iter().any(|record| {
                        record.project_id == project.id
                            && (record.directory == directory || record.worktree_path.as_ref() == Some(&directory))
                    })
            })
            .cloned();
        let project = match owner {
            Some(project) => project,
            None => {
                let name = unique_project_name(&directory, &projects);
                let project = preferences::create_project(&name, std::slice::from_ref(&directory))?;
                preferences::adopt_directory_account_choices(&directory, &project.id)?;
                projects.insert(0, project.clone());
                project
            }
        };
        preferences::touch_project(&project.id)?;
        let provider = preferences.model.as_deref().and_then(AuthProvider::for_model);
        let account = match provider {
            Some(provider) => preferences::project_account(&project.id, provider.id())?.or(
                preferences::load_accounts()?
                    .into_iter()
                    .find(|account| account.provider == provider.id() && account.is_default)
                    .map(|account| account.account_id),
            ),
            None => None,
        };
        // Saved arrangements that no longer parse are left as they are
        // and not loaded; the app starts without them.
        let views = preferences::load_session_views().map(|v| v.into_states().collect()).unwrap_or_default();
        Ok(Boot {
            account_metadata: preferences::load_accounts()?,
            account_choices: preferences::load_project_account_choices()?,
            preferences,
            directory,
            missing_directories: preferences::missing_project_directories(&projects),
            projects,
            records,
            active: project.id,
            account,
            positions: preferences::load_session_positions()?,
            bootstrap_pending: !preferences::projects_bootstrapped()?,
            data: preferences::omp_data_directory()?,
            sessions: preferences::omp_sessions_directory()?,
            views,
            launchers: crate::launchers::Installed::detect(),
            helper: omp_runtime::omp_executable(),
        })
    }
}

/// The folder picker only supplies a path; resolution and registration happen here, on a worker.
pub(crate) fn prepare_project(path: PathBuf) -> Result<(Project, PathBuf), preferences::PreferencesError> {
    let directory = std::fs::canonicalize(&path).unwrap_or(path);
    if !directory.is_dir() {
        return Err(preferences::PreferencesError::NotAFolder(directory));
    }
    let projects = preferences::load_projects()?;
    let records = preferences::load_session_records()?;
    if let Some(project) = projects.iter().find(|project| {
        project.directories.contains(&directory)
            || records.iter().any(|record| {
                record.project_id == project.id
                    && (record.directory == directory || record.worktree_path.as_ref() == Some(&directory))
            })
    }) {
        return Ok((project.clone(), directory));
    }
    let name = unique_project_name(&directory, &projects);
    let project = preferences::create_project(&name, std::slice::from_ref(&directory))?;
    preferences::adopt_directory_account_choices(&directory, &project.id)?;
    Ok((project, directory))
}

/// Import the sessions made before projects existed, on a worker.
pub(crate) fn bootstrap_sessions(
    sessions: &[crate::session::SessionSummary],
) -> Result<(), preferences::PreferencesError> {
    let recorded: HashSet<_> =
        preferences::load_session_records()?.into_iter().map(|record| record.session_id).collect();
    for summary in sessions.iter().filter(|summary| summary.is_omperor_owned() && !recorded.contains(&summary.id)) {
        let Some(directory) = summary.cwd.as_deref().filter(|path| path.is_dir()) else { continue };
        let (project, directory) = prepare_project(directory.to_owned())?;
        preferences::record_session(&SessionRecord {
            session_id: summary.id.clone(),
            project_id: project.id,
            directory,
            parent_session_id: None,
            branch: None,
            base_branch: None,
            worktree_path: None,
            created_at_ms: preferences::now_ms(),
        })?;
    }
    preferences::mark_projects_bootstrapped()
}

impl App {
    pub(super) fn booted(&mut self, result: Arc<Result<Boot, String>>, cx: &mut Cx<Msg>) {
        self.jobs.retain(|_, job| job.label != "Opening Omperor…");
        let boot = match Arc::try_unwrap(result) {
            Ok(Ok(boot)) => boot,
            Ok(Err(error)) => {
                self.boot_error = Some(error);
                return;
            }
            Err(shared) => match &*shared {
                Ok(_) => return,
                Err(error) => {
                    self.boot_error = Some(error.clone());
                    return;
                }
            },
        };
        self.booted = true;
        self.preferences = boot.preferences;
        self.projects = boot.projects;
        self.missing_directories = boot.missing_directories;
        self.account_metadata = boot.account_metadata;
        self.account_choices = boot.account_choices;
        self.records = boot.records.into_iter().map(|record| (record.session_id.clone(), record)).collect();
        self.positions = boot.positions;
        self.bootstrap_pending = boot.bootstrap_pending;
        self.data_dir = Some(boot.data);
        self.sessions_dir = Some(boot.sessions);
        self.saved_views = boot.views.into_iter().collect();
        self.launchers = boot.launchers;
        self.helper = Some(boot.helper);
        let session = self.session();
        session.current_project.replace(boot.directory);
        session.active_project_id.replace(Some(boot.active));
        session.current_model.replace(self.preferences.model.clone().unwrap_or_default());
        session.current_thinking_level.replace(self.preferences.thinking_level.clone().unwrap_or_default());
        session.new_session_pending.set(true);
        self.view_mut(session.key);
        self.restore_drafts(cx);
        self.restore_attachments(session.key, cx);
        self.start_discovery();
        self.inspect_workspace(&session, cx);
        self.index_project_files(&session, cx);
        self.launch_runtime(&session, boot.account, omp_runtime::SessionStartup::Fresh);
        for folder in std::mem::take(&mut self.pending_folders) {
            self.open_directory(folder, cx);
        }
        cx.focus("composer");
    }
}

impl App {
    /// The drafts not sent when Omperor last quit come back as the new
    /// sessions they were written in, each in its project and on its model;
    /// empty ones are let go.
    fn restore_drafts(&mut self, cx: &mut Cx<Msg>) {
        let drafts: Vec<(u64, agents::ViewState)> = self
            .saved_views
            .iter()
            .filter_map(|(path, view)| Some((crate::session_state::draft_key(path.as_deref()?)?, view.clone())))
            .collect();
        for (key, view) in drafts {
            let path = Some(crate::session_state::draft_path(key));
            if view.desk.draft.trim().is_empty() && view.desk.attachments.is_empty() {
                self.saved_views.remove(&path);
                continue;
            }
            let entry = self.live.restore_draft(SessionKey(key));
            if let Some(context) = &view.desk.draft_context {
                entry.current_project.replace(context.project.clone());
                entry.active_project_id.replace(context.project_id.clone());
                entry.current_model.replace(context.model.clone());
                entry.current_account.replace(context.account.clone());
                entry.current_thinking_level.replace(context.thinking_level.clone());
                entry.fast_mode.set(context.fast_mode);
            }
            self.view_mut(entry.key);
            self.restore_attachments(entry.key, cx);
        }
    }

    /// A session's saved attachments, prepared again on workers. Once per
    /// session: what it holds now wins over what was saved.
    pub(crate) fn restore_attachments(&mut self, key: SessionKey, cx: &mut Cx<Msg>) {
        let Some(session) = self.live.get(key) else { return };
        if session.attachments_restored.replace(true) || !session.pending_attachments.borrow().is_empty() {
            return;
        }
        let saved = self.view_mut(key).view.desk.attachments.clone();
        let dir = self.attachment_dir();
        for descriptor in &saved {
            let (id, source) = session.pending_attachments.borrow_mut().restore(descriptor);
            if let Some(source) = source {
                let dir = dir.clone();
                cx.spawn(move || Msg::AttachmentReady(key, id, Arc::new(crate::attachments::prepare(source, &dir))));
            }
        }
    }
}
