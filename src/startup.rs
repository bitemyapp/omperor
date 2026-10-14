//! Filesystem and registry bootstrap. Called only by the background job pool.
use super::*;

#[derive(Debug)]
pub(super) struct Startup {
    preferences: Preferences,
    directory: std::path::PathBuf,
    projects: Vec<Project>,
    missing_directories: HashSet<std::path::PathBuf>,
    records: Vec<SessionRecord>,
    positions: HashMap<String, i64>,
    bootstrap_pending: bool,
    active: String,
    data: std::path::PathBuf,
    sessions: std::path::PathBuf,
    account: Option<String>,
    account_metadata: Vec<StoredAccount>,
    account_choices: HashMap<(String, String), String>,
}

impl Startup {
    pub(super) fn load() -> Result<Self, preferences::PreferencesError> {
        let preferences = preferences::load()?;
        let directory = preferences
            .project
            .clone()
            .filter(|path| path.is_dir())
            .unwrap_or_else(omp_runtime::default_project_root);
        let mut projects = preferences::load_projects()?;
        let records = preferences::load_session_records()?;
        let owner = projects
            .iter()
            .find(|project| {
                project.directories.contains(&directory)
                    || records.iter().any(|record| {
                        record.project_id == project.id
                            && (record.directory == directory
                                || record.worktree_path.as_ref() == Some(&directory))
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
        let provider = preferences
            .model
            .as_deref()
            .and_then(AuthProvider::for_model);
        let account = match provider {
            Some(provider) => preferences::project_account(&project.id, provider.id())?.or(
                preferences::load_accounts()?
                    .into_iter()
                    .find(|account| account.provider == provider.id() && account.is_default)
                    .map(|account| account.account_id),
            ),
            None => None,
        };
        Ok(Self {
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
        })
    }
}

impl AppDelegate {
    pub(super) fn apply_startup(&self, result: Result<Startup, preferences::PreferencesError>) {
        let ivars = self.ivars();
        let owner = self.session();
        if ivars.termination_pending.get() {
            return;
        }
        let state = match result {
            Ok(state) => state,
            Err(error) => {
                owner.runtime_starting.set(false);
                self.set_chat_status(&format!("Could not load Omperor: {error}"));
                self.update_chat_controls();
                return;
            }
        };
        let appearance = state.preferences.appearance;
        owner.current_project.replace(state.directory);
        crate::native_markdown::allow_remote_images(state.preferences.load_remote_images);
        ivars.preferences.replace(state.preferences);
        ivars.preferences_loaded.set(true);
        ivars.projects.replace(state.projects);
        ivars.missing_project_directories.replace(state.missing_directories);
        ivars.account_metadata.replace(state.account_metadata);
        ivars.project_account_choices.replace(state.account_choices);
        ivars.session_records.replace(
            state
                .records
                .into_iter()
                .map(|record| (record.session_id.clone(), record))
                .collect(),
        );
        ivars.session_positions.replace(state.positions);
        ivars
            .projects_bootstrap_pending
            .set(state.bootstrap_pending);
        owner.active_project_id.replace(Some(state.active));
        ivars
            .omperor_data_directory
            .set(state.data)
            .expect("one startup");
        ivars
            .omperor_sessions_directory
            .set(state.sessions)
            .expect("one startup");
        self.apply_appearance(appearance);
        self.apply_chat_typography();
        self.refresh_project_location();
        self.sync_settings_ui();
        self.start_session_discovery();
        self.start_workspace_inspection();
        self.start_omp_runtime(state.account);
    }
}

/// The folder picker only supplies a path; resolution and registration happen here.
pub(super) fn prepare_project(
    path: std::path::PathBuf,
) -> Result<(Project, std::path::PathBuf), preferences::PreferencesError> {
    let directory = std::fs::canonicalize(&path).unwrap_or(path);
    let projects = preferences::load_projects()?;
    let records = preferences::load_session_records()?;
    if let Some(project) = projects.iter().find(|project| {
        project.directories.contains(&directory)
            || records.iter().any(|record| {
                record.project_id == project.id
                    && (record.directory == directory
                        || record.worktree_path.as_ref() == Some(&directory))
            })
    }) {
        return Ok((project.clone(), directory));
    }
    let name = unique_project_name(&directory, &projects);
    let project = preferences::create_project(&name, std::slice::from_ref(&directory))?;
    preferences::adopt_directory_account_choices(&directory, &project.id)?;
    Ok((project, directory))
}

/// Import the pre-project session registry without filesystem or SQL work in a GUI callback.
pub(super) fn bootstrap_sessions(sessions: &[session::SessionSummary]) -> Result<(), preferences::PreferencesError> {
    let recorded: HashSet<_> = preferences::load_session_records()?.into_iter().map(|record| record.session_id).collect();
    for summary in sessions.iter().filter(|summary| summary.is_omperor_owned() && !recorded.contains(&summary.id)) {
        let Some(directory) = summary.cwd.as_deref().filter(|path| path.is_dir()) else { continue; };
        let (project, directory) = prepare_project(directory.to_owned())?;
        preferences::record_session(&SessionRecord {
            session_id: summary.id.clone(), project_id: project.id, directory,
            parent_session_id: None, branch: None, base_branch: None, worktree_path: None,
            created_at_ms: preferences::now_ms(),
        })?;
    }
    preferences::mark_projects_bootstrapped()
}
