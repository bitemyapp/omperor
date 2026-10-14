use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};

use crate::{chat_typography::TextSizes, theme::ChatStyle};

/// Current schema of the Omperor preferences database. Bumped whenever a
/// migration in [`migrate`] adds or changes tables beyond the original
/// key/value `preferences` table, which is created unconditionally.
const SCHEMA_VERSION: i64 = 4;

/// Key/value flag set once the project registry has been seeded from the
/// sessions that existed before projects were introduced.
const PROJECTS_BOOTSTRAPPED_KEY: &str = "projects_bootstrapped";

const PROJECT_KEY: &str = "active_project";
const MODEL_KEY: &str = "model";
const THINKING_LEVEL_KEY: &str = "thinking_level";
const APPEARANCE_KEY: &str = "appearance";
const CHAT_TEXT_SIZE_KEY: &str = "chat_text_size";
const SHOW_INLINE_IMAGES_KEY: &str = "show_inline_images";
const LOAD_REMOTE_IMAGES_KEY: &str = "load_remote_images";
const FRAME_TIMING_OVERLAY_KEY: &str = "frame_timing_overlay";
const CHAT_TEXT_SIZES_KEY: &str = "chat_text_sizes";
const CHAT_STYLE_KEY: &str = "chat_style";
const EDITOR_KEY: &str = "editor_app";
const TERMINAL_KEY: &str = "terminal_app";
const BROWSER_KEY: &str = "browser_app";
const AGENTS_FILTER_KEY: &str = "agents_filter";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppearancePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl AppearancePreference {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub(crate) fn from_stored(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            _ => Self::System,
        }
    }

    const fn stored(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatTextSize {
    Compact,
    #[default]
    Default,
    Large,
}

impl ChatTextSize {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Default => "Default",
            Self::Large => "Large",
        }
    }

    pub(crate) const fn scale(self) -> f64 {
        match self {
            Self::Compact => 0.9,
            Self::Default => 1.0,
            Self::Large => 1.12,
        }
    }

    pub(crate) fn from_stored(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("compact") => Self::Compact,
            Some("large") => Self::Large,
            _ => Self::Default,
        }
    }

    const fn stored(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Default => "default",
            Self::Large => "large",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preferences {
    pub(crate) developer_mode: bool,
    /// Enable desktop input/capture in newly launched sessions.
    pub(crate) computer_use: bool,
    pub(crate) inference_backend: crate::mock_inference::Backend,
    /// Whether the main thread's frame timing is shown live in the window;
    /// takes effect only while Developer mode is on.
    pub(crate) frame_timing_overlay: bool,
    pub(crate) project: Option<PathBuf>,
    pub(crate) model: Option<String>,
    pub(crate) thinking_level: Option<String>,
    pub(crate) appearance: AppearancePreference,
    pub(crate) chat_text_size: ChatTextSize,
    pub(crate) chat_style: ChatStyle,
    pub(crate) chat_text_sizes: TextSizes,
    pub(crate) show_inline_images: bool,
    /// Whether images on the web that a reply links to are fetched. Off by
    /// default: fetching tells the image's server that the reply was read,
    /// and a prompt-injected agent could put data in the URL.
    pub(crate) load_remote_images: bool,
    /// The application "Open in editor" uses, by name; `None` picks the
    /// first installed editor Omperor knows.
    pub(crate) editor: Option<String>,
    /// The application "Open terminal here" uses, by name; `None` picks the
    /// first installed terminal Omperor knows.
    pub(crate) terminal: Option<String>,
    /// The browser sign-in pages open in, by name; `None` is the system's
    /// default browser.
    pub(crate) browser: Option<String>,
    /// Which finished sub-agents the Agents panel shows.
    pub(crate) agents_filter: crate::agents::FinishedFilter,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            developer_mode: false,
            computer_use: false,
            inference_backend: crate::mock_inference::Backend::Omp,
            frame_timing_overlay: false,
            project: None,
            model: None,
            thinking_level: None,
            appearance: AppearancePreference::System,
            chat_text_size: ChatTextSize::Default,
            chat_style: ChatStyle::Modern,
            chat_text_sizes: TextSizes::default(),
            show_inline_images: true,
            load_remote_images: false,
            editor: None,
            terminal: None,
            browser: None,
            agents_filter: crate::agents::FinishedFilter::All,
        }
    }
}

impl Preferences {
    pub(crate) fn effective_backend(&self) -> crate::mock_inference::Backend {
        if self.developer_mode {
            self.inference_backend
        } else {
            crate::mock_inference::Backend::Omp
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PreferencesError {
    #[error("{path} already belongs to project {project}")]
    DirectoryOwned { path: PathBuf, project: String },
    #[error("invalid saved session state: {0}")]
    SessionState(String),
    #[error(transparent)]
    StateRoot(#[from] crate::app_state::Error),
    #[error("could not create preferences directory {path}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not access preferences database {path}")]
    Database {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
}

pub(crate) fn load() -> Result<Preferences, PreferencesError> {
    load_from(&database_path()?)
}

pub(crate) fn store(preferences: &Preferences) -> Result<(), PreferencesError> {
    store_in(&database_path()?, preferences)
}

/// Session reading metadata lives beside preferences, never in OMP's journal.
/// Both functions are worker-only. The existing key/value table keeps this
/// addition compatible with older app builds and their preference writes.
pub(crate) fn load_session_views() -> Result<crate::session_state::SavedViews, PreferencesError> {
    load_session_views_from(&database_path()?)
}

fn load_session_views_from(
    path: &Path,
) -> Result<crate::session_state::SavedViews, PreferencesError> {
    let db = connection(path)?;
    let json: Option<String> = db
        .query_row(
            "SELECT value FROM preferences WHERE key = 'session_views_v1'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error(path))?;
    json.map(|json| {
        crate::session_state::SavedViews::decode(&json).map_err(PreferencesError::SessionState)
    })
    .transpose()
    .map(Option::unwrap_or_default)
}

pub(crate) fn store_session_views(
    views: &crate::session_state::SavedViews,
) -> Result<(), PreferencesError> {
    store_session_views_in(&database_path()?, views)
}

fn store_session_views_in(
    path: &Path,
    views: &crate::session_state::SavedViews,
) -> Result<(), PreferencesError> {
    let json = serde_json::to_string(views)
        .map_err(|error| PreferencesError::SessionState(error.to_string()))?;
    let db = connection(path)?;
    db.execute(
        "INSERT INTO preferences (key, value) VALUES ('session_views_v1', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value WHERE value != excluded.value",
        [json],
    )
    .map_err(database_error(path))?;
    Ok(())
}

pub(crate) fn omp_sessions_directory() -> Result<PathBuf, PreferencesError> {
    Ok(application_support_directory()?.join("omp/sessions"))
}

pub(crate) fn omp_data_directory() -> Result<PathBuf, PreferencesError> {
    Ok(application_support_directory()?.join("omp/data"))
}

pub(crate) fn database_path() -> Result<PathBuf, PreferencesError> {
    Ok(application_support_directory()?.join("omperor.sqlite3"))
}

/// Where archived projects are kept: private to Omperor, and outside every
/// directory it loads sessions or projects from.
pub(crate) fn archives_directory() -> Result<PathBuf, PreferencesError> {
    Ok(application_support_directory()?.join("archives"))
}

pub(crate) fn application_support_directory() -> Result<PathBuf, PreferencesError> {
    Ok(crate::app_state::root()?)
}

fn connection(path: &Path) -> Result<Connection, PreferencesError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| PreferencesError::CreateDirectory {
            path: parent.to_owned(),
            source,
        })?;
    }
    let connection = Connection::open(path).map_err(|source| PreferencesError::Database {
        path: path.to_owned(),
        source,
    })?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 3000;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS preferences (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL
             );",
        )
        .map_err(|source| PreferencesError::Database {
            path: path.to_owned(),
            source,
        })?;
    migrate(&connection).map_err(|source| PreferencesError::Database {
        path: path.to_owned(),
        source,
    })?;
    Ok(connection)
}

/// Applies versioned schema migrations. SQLite's `user_version` pragma records
/// the schema generation so each step runs exactly once; every step executes
/// inside its own transaction together with the version bump.
fn migrate(connection: &Connection) -> Result<(), rusqlite::Error> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 1 {
        connection.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS accounts (
                 account_id TEXT PRIMARY KEY NOT NULL,
                 provider TEXT NOT NULL,
                 principal TEXT,
                 label TEXT,
                 authenticated_at_ms INTEGER NOT NULL,
                 is_default INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS accounts_provider ON accounts(provider);
             CREATE TABLE IF NOT EXISTS project_accounts (
                 project TEXT NOT NULL,
                 provider TEXT NOT NULL,
                 account_id TEXT NOT NULL,
                 chosen_at_ms INTEGER NOT NULL,
                 PRIMARY KEY (project, provider)
             );
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    if version < 2 {
        // Projects group sessions; each session is bound to exactly one
        // directory, which is either one of the project's folders or a git
        // worktree Omperor created for a forked session. `project_accounts`
        // keeps its shape but its `project` column now holds a project id.
        connection.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS projects (
                 id TEXT PRIMARY KEY NOT NULL,
                 name TEXT NOT NULL,
                 created_at_ms INTEGER NOT NULL,
                 last_opened_at_ms INTEGER NOT NULL,
                 collapsed INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS project_directories (
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 path TEXT NOT NULL UNIQUE,
                 position INTEGER NOT NULL,
                 PRIMARY KEY (project_id, path)
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 session_id TEXT PRIMARY KEY NOT NULL,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 directory TEXT NOT NULL,
                 parent_session_id TEXT,
                 branch TEXT,
                 base_branch TEXT,
                 worktree_path TEXT,
                 created_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id);
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
    }
    if version < 3 {
        // Projects gained an explicit sidebar order. Seed it from the previous
        // recency ordering (last opened first) so the sidebar looks unchanged
        // on first launch, then only ever changes when the user drags a
        // project.
        connection.execute_batch(
            "BEGIN;
             ALTER TABLE projects ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
             UPDATE projects SET position = (
                 SELECT COUNT(*) FROM projects AS other
                 WHERE other.last_opened_at_ms > projects.last_opened_at_ms
                    OR (other.last_opened_at_ms = projects.last_opened_at_ms
                        AND other.name < projects.name)
                    OR (other.last_opened_at_ms = projects.last_opened_at_ms
                        AND other.name = projects.name
                        AND other.id < projects.id)
             );
             PRAGMA user_version = 3;
             COMMIT;",
        )?;
    }
    if version < 4 {
        // A user-chosen order for the sessions within a project, keyed by
        // session id so it covers sessions that have no full record (ones
        // filed under a project by their working directory alone). Sessions
        // absent here keep sorting by recency.
        connection.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS session_positions (
                 session_id TEXT PRIMARY KEY NOT NULL,
                 position INTEGER NOT NULL
             );
             PRAGMA user_version = 4;
             COMMIT;",
        )?;
    }
    debug_assert!(
        SCHEMA_VERSION == 4,
        "add a migration step for the new schema version"
    );
    Ok(())
}

fn load_from(path: &Path) -> Result<Preferences, PreferencesError> {
    let connection = connection(path)?;
    let get = |key: &str| {
        connection
            .query_row(
                "SELECT value FROM preferences WHERE key = ?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|source| PreferencesError::Database {
                path: path.to_owned(),
                source,
            })
    };
    Ok(Preferences {
        developer_mode: get("developer_mode")?.as_deref() == Some("true"),
        computer_use: get("computer_use")?.as_deref() == Some("true"),
        inference_backend: crate::mock_inference::Backend::from_stored(
            get("inference_backend")?.as_deref(),
        ),
        frame_timing_overlay: get(FRAME_TIMING_OVERLAY_KEY)?.as_deref() == Some("true"),
        project: get(PROJECT_KEY)?.map(PathBuf::from),
        model: get(MODEL_KEY)?,
        thinking_level: get(THINKING_LEVEL_KEY)?,
        appearance: AppearancePreference::from_stored(get(APPEARANCE_KEY)?),
        chat_text_size: ChatTextSize::from_stored(get(CHAT_TEXT_SIZE_KEY)?),
        chat_style: ChatStyle::from_stored(get(CHAT_STYLE_KEY)?),
        chat_text_sizes: TextSizes::from_stored(get(CHAT_TEXT_SIZES_KEY)?),
        show_inline_images: get(SHOW_INLINE_IMAGES_KEY)?
            .and_then(|value| value.parse().ok())
            .unwrap_or(true),
        load_remote_images: get(LOAD_REMOTE_IMAGES_KEY)?
            .and_then(|value| value.parse().ok())
            .unwrap_or(false),
        editor: get(EDITOR_KEY)?,
        terminal: get(TERMINAL_KEY)?,
        browser: get(BROWSER_KEY)?,
        agents_filter: crate::agents::FinishedFilter::from_stored(
            get(AGENTS_FILTER_KEY)?.as_deref(),
        ),
    })
}

fn store_in(path: &Path, preferences: &Preferences) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection
        .transaction()
        .map_err(|source| PreferencesError::Database {
            path: path.to_owned(),
            source,
        })?;
    for (key, value) in [
        ("computer_use", Some(preferences.computer_use.to_string())),
        (
            "developer_mode",
            Some(preferences.developer_mode.to_string()),
        ),
        (
            "inference_backend",
            Some(preferences.effective_backend().stored().to_owned()),
        ),
        (
            FRAME_TIMING_OVERLAY_KEY,
            Some(preferences.frame_timing_overlay.to_string()),
        ),
        (
            PROJECT_KEY,
            preferences
                .project
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        ),
        (MODEL_KEY, preferences.model.clone()),
        (THINKING_LEVEL_KEY, preferences.thinking_level.clone()),
        (
            APPEARANCE_KEY,
            Some(preferences.appearance.stored().to_owned()),
        ),
        (
            CHAT_TEXT_SIZE_KEY,
            Some(preferences.chat_text_size.stored().to_owned()),
        ),
        (
            CHAT_STYLE_KEY,
            Some(preferences.chat_style.stored().to_owned()),
        ),
        (
            SHOW_INLINE_IMAGES_KEY,
            Some(preferences.show_inline_images.to_string()),
        ),
        (
            LOAD_REMOTE_IMAGES_KEY,
            Some(preferences.load_remote_images.to_string()),
        ),
        (
            CHAT_TEXT_SIZES_KEY,
            Some(preferences.chat_text_sizes.stored()),
        ),
        (EDITOR_KEY, preferences.editor.clone()),
        (TERMINAL_KEY, preferences.terminal.clone()),
        (BROWSER_KEY, preferences.browser.clone()),
        (
            AGENTS_FILTER_KEY,
            Some(preferences.agents_filter.stored().to_owned()),
        ),
    ] {
        if let Some(value) = value {
            transaction
                .execute(
                    "INSERT INTO preferences (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )
                .map_err(|source| PreferencesError::Database {
                    path: path.to_owned(),
                    source,
                })?;
        } else {
            transaction
                .execute("DELETE FROM preferences WHERE key = ?1", [key])
                .map_err(|source| PreferencesError::Database {
                    path: path.to_owned(),
                    source,
                })?;
        }
    }
    transaction
        .commit()
        .map_err(|source| PreferencesError::Database {
            path: path.to_owned(),
            source,
        })
}

/// Omperor-owned metadata for one account held in OMP's credential store.
///
/// OMP owns the credential itself and reports the account's identity and
/// lifecycle state; Omperor records the user-facing label, the time the account
/// was authenticated through the app, and the per-provider default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredAccount {
    pub(crate) account_id: String,
    pub(crate) provider: String,
    pub(crate) principal: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) authenticated_at_ms: i64,
    pub(crate) is_default: bool,
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

pub(crate) fn load_accounts() -> Result<Vec<StoredAccount>, PreferencesError> {
    load_accounts_from(&database_path()?)
}

/// Records an account reported by OMP. A new row is stamped with
/// `authenticated_at_ms` when supplied, otherwise with the current time; an
/// existing row keeps its label and default, refreshes its principal when one is
/// supplied, and moves its timestamp only when `authenticated_at_ms` is given.
pub(crate) fn record_account(
    account_id: &str,
    provider: &str,
    principal: Option<&str>,
    authenticated_at_ms: Option<i64>,
) -> Result<(), PreferencesError> {
    record_account_in(
        &database_path()?,
        account_id,
        provider,
        principal,
        authenticated_at_ms,
    )
}

pub(crate) fn set_account_label(account_id: &str, label: &str) -> Result<(), PreferencesError> {
    set_account_label_in(&database_path()?, account_id, label)
}

pub(crate) fn set_default_account(
    provider: &str,
    account_id: Option<&str>,
) -> Result<(), PreferencesError> {
    set_default_account_in(&database_path()?, provider, account_id)
}

/// Drops metadata for accounts that OMP no longer stores. Only call this with a
/// complete, successfully fetched account list so a transient listing failure
/// cannot erase labels.
pub(crate) fn retain_accounts(live_account_ids: &[String]) -> Result<(), PreferencesError> {
    retain_accounts_in(&database_path()?, live_account_ids)
}

/// Load all non-secret account choices on the startup worker for GUI lookups.
pub(crate) fn load_project_account_choices()
-> Result<HashMap<(String, String), String>, PreferencesError> {
    let path = database_path()?;
    let db = connection(&path)?;
    let mut statement = db
        .prepare("SELECT project, provider, account_id FROM project_accounts")
        .map_err(database_error(&path))?;
    statement
        .query_map([], |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)))
        .map_err(database_error(&path))?
        .collect::<Result<_, _>>()
        .map_err(database_error(&path))
}

pub(crate) fn project_account(
    project_id: &str,
    provider: &str,
) -> Result<Option<String>, PreferencesError> {
    project_account_in(&database_path()?, project_id, provider)
}

pub(crate) fn set_project_account(
    project_id: &str,
    provider: &str,
    account_id: &str,
) -> Result<(), PreferencesError> {
    set_project_account_in(&database_path()?, project_id, provider, account_id)
}

/// A group of sessions. Every session runs in exactly one directory: one of
/// the project's folders, or a git worktree Omperor created when a session
/// was forked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Project {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) directories: Vec<PathBuf>,
    pub(crate) last_opened_at_ms: i64,
    pub(crate) collapsed: bool,
}

/// Omperor's record of one session it created: which project it belongs to,
/// where it runs, and its fork lineage when it was forked into a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionRecord {
    pub(crate) session_id: String,
    pub(crate) project_id: String,
    pub(crate) directory: PathBuf,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) base_branch: Option<String>,
    pub(crate) worktree_path: Option<PathBuf>,
    pub(crate) created_at_ms: i64,
}

pub(crate) fn worktrees_directory() -> Result<PathBuf, PreferencesError> {
    Ok(application_support_directory()?.join("worktrees"))
}

pub(crate) fn load_projects() -> Result<Vec<Project>, PreferencesError> {
    load_projects_from(&database_path()?)
}

pub(crate) fn create_project(
    name: &str,
    directories: &[PathBuf],
) -> Result<Project, PreferencesError> {
    create_project_in(&database_path()?, name, directories)
}

pub(crate) fn rename_project(project_id: &str, name: &str) -> Result<(), PreferencesError> {
    rename_project_in(&database_path()?, project_id, name)
}

pub(crate) fn set_project_collapsed(
    project_id: &str,
    collapsed: bool,
) -> Result<(), PreferencesError> {
    set_project_collapsed_in(&database_path()?, project_id, collapsed)
}

pub(crate) fn touch_project(project_id: &str) -> Result<(), PreferencesError> {
    touch_project_in(&database_path()?, project_id)
}

/// Persists the sidebar order of projects: each id is written its index in
/// `ordered_ids` as its `position`. Ids not present keep their stored position.
pub(crate) fn set_project_order(ordered_ids: &[String]) -> Result<(), PreferencesError> {
    set_project_order_in(&database_path()?, ordered_ids)
}

/// Loads every stored session ordering, keyed by session id. Sessions absent
/// from the result sort by recency within their project.
pub(crate) fn load_session_positions() -> Result<HashMap<String, i64>, PreferencesError> {
    load_session_positions_from(&database_path()?)
}

/// Persists the sidebar order of one project's sessions: each id is written its
/// index in `ordered_ids` as its `position`.
pub(crate) fn set_session_order(ordered_ids: &[String]) -> Result<(), PreferencesError> {
    set_session_order_in(&database_path()?, ordered_ids)
}

pub(crate) fn add_project_directory(
    project_id: &str,
    directory: &Path,
) -> Result<(), PreferencesError> {
    add_project_directory_in(&database_path()?, project_id, directory)
}

pub(crate) fn remove_project_directory(
    project_id: &str,
    directory: &Path,
) -> Result<(), PreferencesError> {
    remove_project_directory_in(&database_path()?, project_id, directory)
}

pub(crate) fn delete_project(project_id: &str) -> Result<(), PreferencesError> {
    delete_project_in(&database_path()?, project_id)
}

pub(crate) fn record_session(record: &SessionRecord) -> Result<(), PreferencesError> {
    record_session_in(&database_path()?, record)
}

pub(crate) fn load_session_records() -> Result<Vec<SessionRecord>, PreferencesError> {
    load_session_records_from(&database_path()?)
}

pub(crate) fn projects_bootstrapped() -> Result<bool, PreferencesError> {
    projects_bootstrapped_in(&database_path()?)
}

pub(crate) fn mark_projects_bootstrapped() -> Result<(), PreferencesError> {
    mark_projects_bootstrapped_in(&database_path()?)
}

/// Moves account choices recorded against a directory path, from before
/// projects existed, onto the project that now owns that directory.
pub(crate) fn adopt_directory_account_choices(
    directory: &Path,
    project_id: &str,
) -> Result<(), PreferencesError> {
    adopt_directory_account_choices_in(&database_path()?, directory, project_id)
}

fn database_error(path: &Path) -> impl Fn(rusqlite::Error) -> PreferencesError + '_ {
    move |source| PreferencesError::Database {
        path: path.to_owned(),
        source,
    }
}

pub(crate) fn load_accounts_from(path: &Path) -> Result<Vec<StoredAccount>, PreferencesError> {
    let connection = connection(path)?;
    let mut statement = connection
        .prepare(
            "SELECT account_id, provider, principal, label, authenticated_at_ms, is_default
             FROM accounts
             ORDER BY provider, authenticated_at_ms, account_id",
        )
        .map_err(database_error(path))?;
    let rows = statement
        .query_map([], |row| {
            Ok(StoredAccount {
                account_id: row.get(0)?,
                provider: row.get(1)?,
                principal: row.get(2)?,
                label: row.get(3)?,
                authenticated_at_ms: row.get(4)?,
                is_default: row.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(database_error(path))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(database_error(path))
}

pub(crate) fn record_account_in(
    path: &Path,
    account_id: &str,
    provider: &str,
    principal: Option<&str>,
    authenticated_at_ms: Option<i64>,
) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "INSERT INTO accounts (account_id, provider, principal, label, authenticated_at_ms, is_default)
             VALUES (?1, ?2, ?3, NULL, ?4, 0)
             ON CONFLICT(account_id) DO UPDATE SET
                 provider = excluded.provider,
                 principal = COALESCE(excluded.principal, accounts.principal),
                 authenticated_at_ms = COALESCE(?5, accounts.authenticated_at_ms)",
            params![
                account_id,
                provider,
                principal,
                authenticated_at_ms.unwrap_or_else(now_ms),
                authenticated_at_ms,
            ],
        )
        .map_err(database_error(path))?;
    Ok(())
}

pub(crate) fn set_account_label_in(
    path: &Path,
    account_id: &str,
    label: &str,
) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "UPDATE accounts SET label = ?2 WHERE account_id = ?1",
            params![account_id, label],
        )
        .map_err(database_error(path))?;
    Ok(())
}

pub(crate) fn set_default_account_in(
    path: &Path,
    provider: &str,
    account_id: Option<&str>,
) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    transaction
        .execute(
            "UPDATE accounts SET is_default = 0 WHERE provider = ?1",
            [provider],
        )
        .map_err(database_error(path))?;
    if let Some(account_id) = account_id {
        transaction
            .execute(
                "UPDATE accounts SET is_default = 1 WHERE account_id = ?1 AND provider = ?2",
                params![account_id, provider],
            )
            .map_err(database_error(path))?;
    }
    transaction.commit().map_err(database_error(path))
}

fn forget_account_in(path: &Path, account_id: &str) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    transaction
        .execute("DELETE FROM accounts WHERE account_id = ?1", [account_id])
        .map_err(database_error(path))?;
    transaction
        .execute(
            "DELETE FROM project_accounts WHERE account_id = ?1",
            [account_id],
        )
        .map_err(database_error(path))?;
    transaction.commit().map_err(database_error(path))
}

fn retain_accounts_in(path: &Path, live_account_ids: &[String]) -> Result<(), PreferencesError> {
    let stale = load_accounts_from(path)?
        .into_iter()
        .map(|account| account.account_id)
        .filter(|account_id| !live_account_ids.contains(account_id))
        .collect::<Vec<_>>();
    for account_id in stale {
        forget_account_in(path, &account_id)?;
    }
    Ok(())
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn project_account_in(
    path: &Path,
    project_id: &str,
    provider: &str,
) -> Result<Option<String>, PreferencesError> {
    let connection = connection(path)?;
    connection
        .query_row(
            "SELECT account_id FROM project_accounts WHERE project = ?1 AND provider = ?2",
            params![project_id, provider],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error(path))
}

fn set_project_account_in(
    path: &Path,
    project_id: &str,
    provider: &str,
    account_id: &str,
) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "INSERT INTO project_accounts (project, provider, account_id, chosen_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project, provider) DO UPDATE SET
                 account_id = excluded.account_id,
                 chosen_at_ms = excluded.chosen_at_ms",
            params![project_id, provider, account_id, now_ms()],
        )
        .map_err(database_error(path))?;
    Ok(())
}

fn adopt_directory_account_choices_in(
    path: &Path,
    directory: &Path,
    project_id: &str,
) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    transaction
        .execute(
            "UPDATE OR IGNORE project_accounts SET project = ?2 WHERE project = ?1",
            params![path_key(directory), project_id],
        )
        .map_err(database_error(path))?;
    transaction
        .execute(
            "DELETE FROM project_accounts WHERE project = ?1",
            [path_key(directory)],
        )
        .map_err(database_error(path))?;
    transaction.commit().map_err(database_error(path))
}

/// Generates a project identifier that is unique on this machine without
/// pulling in an id crate: the creation time plus per-process random bits.
fn new_project_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let now = now_ms();
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_i64(now);
    format!("project-{now:x}-{:016x}", hasher.finish())
}

fn load_projects_from(path: &Path) -> Result<Vec<Project>, PreferencesError> {
    let connection = connection(path)?;
    let mut statement = connection
        .prepare(
            "SELECT id, name, last_opened_at_ms, collapsed FROM projects
             ORDER BY position, name, id",
        )
        .map_err(database_error(path))?;
    let mut projects = statement
        .query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                directories: Vec::new(),
                last_opened_at_ms: row.get(2)?,
                collapsed: row.get::<_, i64>(3)? != 0,
            })
        })
        .map_err(database_error(path))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error(path))?;
    let mut statement = connection
        .prepare("SELECT project_id, path FROM project_directories ORDER BY position, path")
        .map_err(database_error(path))?;
    let directories = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error(path))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error(path))?;
    for (project_id, directory) in directories {
        if let Some(project) = projects.iter_mut().find(|project| project.id == project_id) {
            project.directories.push(PathBuf::from(directory));
        }
    }
    Ok(projects)
}

/// Allocates metadata only; the ordered GUI storage worker persists it.
pub(crate) fn project_draft(name: &str, directories: &[PathBuf]) -> Project {
    Project {
        id: new_project_id(), name: name.to_owned(), directories: directories.to_vec(),
        last_opened_at_ms: now_ms(), collapsed: false,
    }
}

pub(crate) fn insert_project(project: &Project) -> Result<(), PreferencesError> {
    insert_project_in(&database_path()?, project)
}

fn create_project_in(path: &Path, name: &str, directories: &[PathBuf]) -> Result<Project, PreferencesError> {
    let project = project_draft(name, directories);
    insert_project_in(path, &project)?;
    Ok(project)
}

fn insert_project_in(path: &Path, project: &Project) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    transaction.execute(
        "INSERT INTO projects (id, name, created_at_ms, last_opened_at_ms, collapsed, position)
         VALUES (?1, ?2, ?3, ?3, ?4, (SELECT COALESCE(MAX(position) + 1, 0) FROM projects))",
        params![project.id, project.name, project.last_opened_at_ms, project.collapsed],
    ).map_err(database_error(path))?;
    for (position, directory) in project.directories.iter().enumerate() {
        transaction.execute(
            "INSERT INTO project_directories (project_id, path, position) VALUES (?1, ?2, ?3)",
            params![project.id, path_key(directory), position as i64],
        ).map_err(database_error(path))?;
    }
    transaction.commit().map_err(database_error(path))
}

fn rename_project_in(path: &Path, project_id: &str, name: &str) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "UPDATE projects SET name = ?2 WHERE id = ?1",
            params![project_id, name],
        )
        .map_err(database_error(path))?;
    Ok(())
}

fn set_project_collapsed_in(
    path: &Path,
    project_id: &str,
    collapsed: bool,
) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "UPDATE projects SET collapsed = ?2 WHERE id = ?1",
            params![project_id, i64::from(collapsed)],
        )
        .map_err(database_error(path))?;
    Ok(())
}

fn touch_project_in(path: &Path, project_id: &str) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "UPDATE projects SET last_opened_at_ms = ?2 WHERE id = ?1",
            params![project_id, now_ms()],
        )
        .map_err(database_error(path))?;
    Ok(())
}

fn set_project_order_in(path: &Path, ordered_ids: &[String]) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    for (position, id) in ordered_ids.iter().enumerate() {
        transaction
            .execute(
                "UPDATE projects SET position = ?2 WHERE id = ?1",
                params![id, position as i64],
            )
            .map_err(database_error(path))?;
    }
    transaction.commit().map_err(database_error(path))?;
    Ok(())
}

fn load_session_positions_from(path: &Path) -> Result<HashMap<String, i64>, PreferencesError> {
    let connection = connection(path)?;
    let mut statement = connection
        .prepare("SELECT session_id, position FROM session_positions")
        .map_err(database_error(path))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(database_error(path))?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(database_error(path))?;
    Ok(rows)
}

fn set_session_order_in(path: &Path, ordered_ids: &[String]) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    for (position, id) in ordered_ids.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO session_positions (session_id, position) VALUES (?1, ?2)
                 ON CONFLICT(session_id) DO UPDATE SET position = excluded.position",
                params![id, position as i64],
            )
            .map_err(database_error(path))?;
    }
    transaction.commit().map_err(database_error(path))?;
    Ok(())
}

fn add_project_directory_in(
    path: &Path,
    project_id: &str,
    directory: &Path,
) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    let owner: Option<(String, String)> = transaction.query_row(
        "SELECT p.id, p.name FROM project_directories d JOIN projects p ON p.id = d.project_id WHERE d.path = ?1",
        [path_key(directory)], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(database_error(path))?;
    if let Some((id, project)) = owner {
        return if id == project_id { Ok(()) } else {
            Err(PreferencesError::DirectoryOwned { path: directory.to_owned(), project })
        };
    }
    transaction
        .execute(
            "INSERT INTO project_directories (project_id, path, position)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM project_directories WHERE project_id = ?1))
             ON CONFLICT(path) DO NOTHING",
            params![project_id, path_key(directory)],
        )
        .map_err(database_error(path))?;
    transaction.commit().map_err(database_error(path))
}

fn remove_project_directory_in(
    path: &Path,
    project_id: &str,
    directory: &Path,
) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "DELETE FROM project_directories WHERE project_id = ?1 AND path = ?2",
            params![project_id, path_key(directory)],
        )
        .map_err(database_error(path))?;
    Ok(())
}

fn delete_project_in(path: &Path, project_id: &str) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    transaction
        .execute("DELETE FROM projects WHERE id = ?1", [project_id])
        .map_err(database_error(path))?;
    transaction
        .execute(
            "DELETE FROM project_accounts WHERE project = ?1",
            [project_id],
        )
        .map_err(database_error(path))?;
    transaction.commit().map_err(database_error(path))
}

fn record_session_in(path: &Path, record: &SessionRecord) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "INSERT INTO sessions (session_id, project_id, directory, parent_session_id, branch, base_branch, worktree_path, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(session_id) DO UPDATE SET
                 project_id = excluded.project_id,
                 directory = excluded.directory,
                 parent_session_id = COALESCE(excluded.parent_session_id, sessions.parent_session_id),
                 branch = COALESCE(excluded.branch, sessions.branch),
                 base_branch = COALESCE(excluded.base_branch, sessions.base_branch),
                 worktree_path = COALESCE(excluded.worktree_path, sessions.worktree_path)",
            params![
                record.session_id,
                record.project_id,
                path_key(&record.directory),
                record.parent_session_id,
                record.branch,
                record.base_branch,
                record.worktree_path.as_deref().map(path_key),
                record.created_at_ms,
            ],
        )
        .map_err(database_error(path))?;
    Ok(())
}

fn load_session_records_from(path: &Path) -> Result<Vec<SessionRecord>, PreferencesError> {
    let connection = connection(path)?;
    let mut statement = connection
        .prepare(
            "SELECT session_id, project_id, directory, parent_session_id, branch, base_branch, worktree_path, created_at_ms
             FROM sessions ORDER BY created_at_ms, session_id",
        )
        .map_err(database_error(path))?;
    let records = statement
        .query_map([], |row| {
            Ok(SessionRecord {
                session_id: row.get(0)?,
                project_id: row.get(1)?,
                directory: PathBuf::from(row.get::<_, String>(2)?),
                parent_session_id: row.get(3)?,
                branch: row.get(4)?,
                base_branch: row.get(5)?,
                worktree_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
                created_at_ms: row.get(7)?,
            })
        })
        .map_err(database_error(path))?;
    records
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error(path))
}

/// Forgets sessions that were deleted: their records and sidebar positions.
/// A fork recorded against one of them keeps its record; only the parent is
/// gone.
pub(crate) fn forget_sessions_in(path: &Path, session_ids: &[String]) -> Result<(), PreferencesError> {
    if session_ids.is_empty() {
        return Ok(());
    }
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    for id in session_ids {
        transaction
            .execute("DELETE FROM sessions WHERE session_id = ?1", [id])
            .map_err(database_error(path))?;
        transaction
            .execute("DELETE FROM session_positions WHERE session_id = ?1", [id])
            .map_err(database_error(path))?;
    }
    transaction.commit().map_err(database_error(path))
}

/// Everything the registry keeps for one project, as an archive stores it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ProjectRows {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at_ms: i64,
    pub(crate) last_opened_at_ms: i64,
    pub(crate) collapsed: bool,
    pub(crate) directories: Vec<PathBuf>,
    pub(crate) accounts: Vec<AccountChoiceRow>,
    pub(crate) sessions: Vec<SessionRow>,
    /// Sidebar positions of the project's sessions, including ones that
    /// have no record (sessions filed by their folder).
    pub(crate) positions: Vec<(String, i64)>,
}

/// The account a project chose for one provider.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct AccountChoiceRow {
    pub(crate) provider: String,
    pub(crate) account_id: String,
    pub(crate) chosen_at_ms: i64,
}

/// A [`SessionRecord`] as an archive stores it; its project is the archive's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SessionRow {
    pub(crate) session_id: String,
    pub(crate) directory: PathBuf,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) base_branch: Option<String>,
    pub(crate) worktree_path: Option<PathBuf>,
    pub(crate) created_at_ms: i64,
}

impl ProjectRows {
    /// Whether `directory` is one of the project's folders or a folder one
    /// of its sessions ran in.
    fn owns_directory(&self, directory: &Path) -> bool {
        self.directories.iter().any(|owned| owned == directory)
            || self.sessions.iter().any(|session| {
                session.directory == directory || session.worktree_path.as_deref() == Some(directory)
            })
    }
}

/// Reads a project's rows: the project, its folders and account choices, the
/// records of every session recorded under it, and the sidebar positions of
/// those sessions and of `member_ids` (sessions filed by folder).
pub(crate) fn project_rows_in(
    path: &Path,
    project_id: &str,
    member_ids: &[String],
) -> Result<ProjectRows, PreferencesError> {
    let connection = connection(path)?;
    let (name, created_at_ms, last_opened_at_ms, collapsed) = connection
        .query_row(
            "SELECT name, created_at_ms, last_opened_at_ms, collapsed FROM projects WHERE id = ?1",
            [project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)? != 0,
                ))
            },
        )
        .map_err(database_error(path))?;
    let mut statement = connection
        .prepare("SELECT path FROM project_directories WHERE project_id = ?1 ORDER BY position, path")
        .map_err(database_error(path))?;
    let directories = statement
        .query_map([project_id], |row| row.get::<_, String>(0).map(PathBuf::from))
        .map_err(database_error(path))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error(path))?;
    let mut statement = connection
        .prepare("SELECT provider, account_id, chosen_at_ms FROM project_accounts WHERE project = ?1")
        .map_err(database_error(path))?;
    let accounts = statement
        .query_map([project_id], |row| {
            Ok(AccountChoiceRow {
                provider: row.get(0)?,
                account_id: row.get(1)?,
                chosen_at_ms: row.get(2)?,
            })
        })
        .map_err(database_error(path))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error(path))?;
    let mut statement = connection
        .prepare(
            "SELECT session_id, directory, parent_session_id, branch, base_branch, worktree_path, created_at_ms
             FROM sessions WHERE project_id = ?1 ORDER BY created_at_ms, session_id",
        )
        .map_err(database_error(path))?;
    let sessions = statement
        .query_map([project_id], |row| {
            Ok(SessionRow {
                session_id: row.get(0)?,
                directory: PathBuf::from(row.get::<_, String>(1)?),
                parent_session_id: row.get(2)?,
                branch: row.get(3)?,
                base_branch: row.get(4)?,
                worktree_path: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
                created_at_ms: row.get(6)?,
            })
        })
        .map_err(database_error(path))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error(path))?;
    let mut ids: Vec<&str> = sessions.iter().map(|session| session.session_id.as_str()).collect();
    ids.extend(member_ids.iter().map(String::as_str));
    ids.sort_unstable();
    ids.dedup();
    let mut positions = Vec::new();
    for id in ids {
        let position = connection
            .query_row(
                "SELECT position FROM session_positions WHERE session_id = ?1",
                [id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(database_error(path))?;
        if let Some(position) = position {
            positions.push((id.to_owned(), position));
        }
    }
    Ok(ProjectRows {
        id: project_id.to_owned(),
        name,
        created_at_ms,
        last_opened_at_ms,
        collapsed,
        directories,
        accounts,
        sessions,
        positions,
    })
}

/// Removes an archived project's rows: the project with its folders and
/// session records (by cascade), its account choices, and the positions of
/// `member_ids`. When the remembered active folder belonged to the project,
/// it moves to `fallback`, or is forgotten, so the next launch does not file
/// a new project for the archived folder.
pub(crate) fn remove_project_rows_in(
    path: &Path,
    rows: &ProjectRows,
    member_ids: &[String],
    fallback: Option<&Path>,
) -> Result<(), PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    transaction
        .execute("DELETE FROM projects WHERE id = ?1", [&rows.id])
        .map_err(database_error(path))?;
    transaction
        .execute("DELETE FROM project_accounts WHERE project = ?1", [&rows.id])
        .map_err(database_error(path))?;
    for id in member_ids
        .iter()
        .chain(rows.sessions.iter().map(|session| &session.session_id))
    {
        transaction
            .execute("DELETE FROM session_positions WHERE session_id = ?1", [id])
            .map_err(database_error(path))?;
        // A session filed by folder has no record; one recorded under
        // another project keeps it.
        transaction
            .execute(
                "DELETE FROM sessions WHERE session_id = ?1 AND project_id = ?2",
                params![id, rows.id],
            )
            .map_err(database_error(path))?;
    }
    let active = transaction
        .query_row(
            "SELECT value FROM preferences WHERE key = ?1",
            [PROJECT_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error(path))?;
    if active.is_some_and(|active| rows.owns_directory(Path::new(&active))) {
        match fallback {
            Some(fallback) => transaction
                .execute(
                    "INSERT INTO preferences (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![PROJECT_KEY, path_key(fallback)],
                )
                .map_err(database_error(path))?,
            None => transaction
                .execute("DELETE FROM preferences WHERE key = ?1", [PROJECT_KEY])
                .map_err(database_error(path))?,
        };
    }
    transaction.commit().map_err(database_error(path))
}

/// Puts an archived project's rows back, at the end of the project list.
/// A folder another project has taken since stays with that project and is
/// returned; the restored sessions keep their records, so they still list
/// under the restored project. Fails if the project is already present.
pub(crate) fn restore_project_rows_in(
    path: &Path,
    rows: &ProjectRows,
) -> Result<Vec<PathBuf>, PreferencesError> {
    let mut connection = connection(path)?;
    let transaction = connection.transaction().map_err(database_error(path))?;
    transaction
        .execute(
            "INSERT INTO projects (id, name, created_at_ms, last_opened_at_ms, collapsed, position)
             VALUES (?1, ?2, ?3, ?4, ?5, (SELECT COALESCE(MAX(position) + 1, 0) FROM projects))",
            params![
                rows.id,
                rows.name,
                rows.created_at_ms,
                rows.last_opened_at_ms,
                i64::from(rows.collapsed)
            ],
        )
        .map_err(database_error(path))?;
    let mut taken = Vec::new();
    for (position, directory) in rows.directories.iter().enumerate() {
        let inserted = transaction
            .execute(
                "INSERT INTO project_directories (project_id, path, position) VALUES (?1, ?2, ?3)
                 ON CONFLICT(path) DO NOTHING",
                params![rows.id, path_key(directory), position as i64],
            )
            .map_err(database_error(path))?;
        if inserted == 0 {
            taken.push(directory.clone());
        }
    }
    for account in &rows.accounts {
        transaction
            .execute(
                "INSERT OR IGNORE INTO project_accounts (project, provider, account_id, chosen_at_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![rows.id, account.provider, account.account_id, account.chosen_at_ms],
            )
            .map_err(database_error(path))?;
    }
    for session in &rows.sessions {
        transaction
            .execute(
                "INSERT OR IGNORE INTO sessions (session_id, project_id, directory, parent_session_id, branch, base_branch, worktree_path, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    session.session_id,
                    rows.id,
                    path_key(&session.directory),
                    session.parent_session_id,
                    session.branch,
                    session.base_branch,
                    session.worktree_path.as_deref().map(path_key),
                    session.created_at_ms,
                ],
            )
            .map_err(database_error(path))?;
    }
    for (id, position) in &rows.positions {
        transaction
            .execute(
                "INSERT OR IGNORE INTO session_positions (session_id, position) VALUES (?1, ?2)",
                params![id, position],
            )
            .map_err(database_error(path))?;
    }
    transaction.commit().map_err(database_error(path))?;
    Ok(taken)
}

/// Creates a project with one folder, for other modules' tests.
#[cfg(test)]
pub(crate) fn create_project_for_test(path: &Path, name: &str, directory: &Path) -> String {
    create_project_in(path, name, &[directory.to_path_buf()]).expect("test project").id
}

/// The project registry as the interface holds it: projects, session
/// records, and the sidebar order.
#[derive(Debug, Clone)]
pub(crate) struct Registry {
    pub(crate) projects: Vec<Project>,
    pub(crate) records: Vec<SessionRecord>,
    pub(crate) positions: HashMap<String, i64>,
    pub(crate) missing_directories: std::collections::HashSet<PathBuf>,
}

/// Reads the whole registry, for a worker that has just changed it.
pub(crate) fn load_registry() -> Result<Registry, PreferencesError> {
    let path = database_path()?;
    let projects = load_projects_from(&path)?;
    let missing_directories = missing_project_directories(&projects);
    Ok(Registry {
        projects,
        records: load_session_records_from(&path)?,
        positions: load_session_positions_from(&path)?,
        missing_directories,
    })
}

/// Filesystem probes belong with registry loading on a background worker.
pub(crate) fn missing_project_directories(projects: &[Project]) -> std::collections::HashSet<PathBuf> {
    projects.iter().flat_map(|project| &project.directories).filter(|path| !path.is_dir()).cloned().collect()
}

/// Whether the registry has a project with this id.
pub(crate) fn project_exists_in(path: &Path, project_id: &str) -> Result<bool, PreferencesError> {
    let connection = connection(path)?;
    connection
        .query_row("SELECT 1 FROM projects WHERE id = ?1", [project_id], |_| Ok(()))
        .optional()
        .map(|found| found.is_some())
        .map_err(database_error(path))
}

fn projects_bootstrapped_in(path: &Path) -> Result<bool, PreferencesError> {
    let connection = connection(path)?;
    connection
        .query_row(
            "SELECT value FROM preferences WHERE key = ?1",
            [PROJECTS_BOOTSTRAPPED_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(database_error(path))
}

fn mark_projects_bootstrapped_in(path: &Path) -> Result<(), PreferencesError> {
    let connection = connection(path)?;
    connection
        .execute(
            "INSERT INTO preferences (key, value) VALUES (?1, '1')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [PROJECTS_BOOTSTRAPPED_KEY],
        )
        .map_err(database_error(path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    #[test]
    fn an_archived_projects_rows_come_out_whole_and_go_back_in() {
        let (scratch, path) = scratch_database("archive-rows");
        let demo = super::create_project_in(&path, "demo", &[PathBuf::from("/work/demo")]).unwrap();
        let other = super::create_project_in(&path, "other", &[PathBuf::from("/work/other")]).unwrap();
        let record = |id: &str, project: &str, directory: &str, worktree: Option<&str>| SessionRecord {
            session_id: id.into(),
            project_id: project.into(),
            directory: directory.into(),
            parent_session_id: None,
            branch: worktree.map(|_| "omperor/fork".into()),
            base_branch: worktree.map(|_| "main".into()),
            worktree_path: worktree.map(PathBuf::from),
            created_at_ms: 1,
        };
        super::record_session_in(&path, &record("a", &demo.id, "/wt/fork", Some("/wt/fork"))).unwrap();
        super::record_session_in(&path, &record("b", &other.id, "/work/other", None)).unwrap();
        super::set_session_order_in(&path, &["a".into(), "loose".into(), "b".into()]).unwrap();
        super::set_project_account_in(&path, &demo.id, "openai-codex", "acct-1").unwrap();
        // The remembered folder is the archived project's fork worktree.
        super::connection(&path)
            .unwrap()
            .execute(
                "INSERT INTO preferences (key, value) VALUES ('active_project', '/wt/fork')",
                [],
            )
            .unwrap();

        let rows = super::project_rows_in(&path, &demo.id, &["loose".into()]).unwrap();
        assert_eq!(rows.name, "demo");
        assert_eq!(rows.directories, [PathBuf::from("/work/demo")]);
        assert_eq!(rows.sessions.len(), 1);
        assert_eq!(rows.accounts.len(), 1);
        assert_eq!(rows.positions, [("a".to_owned(), 0), ("loose".to_owned(), 1)]);

        let members = ["a".to_owned(), "loose".to_owned()];
        super::remove_project_rows_in(&path, &rows, &members, Some(Path::new("/work/other"))).unwrap();
        let projects = super::load_projects_from(&path).unwrap();
        assert_eq!(projects.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["other"]);
        let records = super::load_session_records_from(&path).unwrap();
        assert_eq!(records.iter().map(|r| r.session_id.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(super::load_session_positions_from(&path).unwrap().len(), 1);
        assert_eq!(super::load_from(&path).unwrap().project, Some(PathBuf::from("/work/other")));
        assert!(!super::project_exists_in(&path, &demo.id).unwrap());

        // Meanwhile another project took the archived folder.
        super::add_project_directory_in(&path, &other.id, Path::new("/work/demo")).unwrap();
        let taken = super::restore_project_rows_in(&path, &rows).unwrap();
        assert_eq!(taken, [PathBuf::from("/work/demo")]);
        assert!(super::project_exists_in(&path, &demo.id).unwrap());
        let records = super::load_session_records_from(&path).unwrap();
        assert!(records.iter().any(|r| r.session_id == "a" && r.project_id == demo.id));
        assert_eq!(super::load_session_positions_from(&path).unwrap().len(), 3);
        assert_eq!(
            super::project_account_in(&path, &demo.id, "openai-codex").unwrap().as_deref(),
            Some("acct-1")
        );
        // A second restore would duplicate the project.
        assert!(super::restore_project_rows_in(&path, &rows).is_err());

        super::forget_sessions_in(&path, &["a".into(), "b".into()]).unwrap();
        assert!(super::load_session_records_from(&path).unwrap().is_empty());
        assert_eq!(super::load_session_positions_from(&path).unwrap().len(), 1);
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn session_views_survive_preference_edits_and_do_not_duplicate_transcripts() {
        let (scratch, path) = scratch_database("session-views");
        let mut view = crate::agents::ViewState::default();
        view.desk.draft = "Unsent draft 🦀".into();
        view.desk.inspect("child");
        view.desk.keep(0);
        view.desk.pins[0].snapshot.body = "journal only".into();
        let key = Some(crate::session_state::draft_path(42));
        let views =
            crate::session_state::SavedViews::capture(&HashMap::from([(key.clone(), view)]));
        super::store_session_views_in(&path, &views).unwrap();
        super::store_in(&path, &Preferences::default()).unwrap();
        let restored: HashMap<_, _> = super::load_session_views_from(&path)
            .unwrap()
            .into_states()
            .collect();
        assert_eq!(restored[&key].desk.draft, "Unsent draft 🦀");
        assert!(restored[&key].desk.pins[0].kept);
        assert!(restored[&key].desk.pins[0].snapshot.body.is_empty());
        std::fs::remove_dir_all(scratch).unwrap();
    }

    use std::{
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        Preferences, SessionRecord, StoredAccount, add_project_directory_in,
        adopt_directory_account_choices_in, create_project_in, delete_project_in,
        forget_account_in, load_accounts_from, load_from, load_projects_from,
        load_session_positions_from, load_session_records_from, mark_projects_bootstrapped_in,
        project_account_in, projects_bootstrapped_in, record_account_in, record_session_in,
        remove_project_directory_in, rename_project_in, retain_accounts_in, set_account_label_in,
        set_default_account_in, set_project_account_in, set_project_collapsed_in,
        set_project_order_in, set_session_order_in, store_in, touch_project_in,
    };

    static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

    fn scratch_database(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!(
            "omperor-preferences-{label}-{}-{sequence}",
            std::process::id()
        ));
        let path = scratch.join("preferences.sqlite3");
        (scratch, path)
    }

    #[test]
    fn new_database_uses_native_display_defaults() {
        let (scratch, path) = scratch_database("defaults");
        assert_eq!(
            load_from(&path).expect("new preferences load"),
            Preferences::default()
        );
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn preferences_round_trip_and_clear_optional_values() {
        let (scratch, path) = scratch_database("round-trip");
        let preferences = Preferences {
            developer_mode: true,
            computer_use: true,
            inference_backend: crate::mock_inference::Backend::Mock,
            frame_timing_overlay: true,
            project: Some("/work/project".into()),
            model: Some("openai-codex/gpt-5.6-luna".into()),
            thinking_level: Some("high".into()),
            appearance: super::AppearancePreference::Dark,
            chat_text_size: super::ChatTextSize::Large,
            chat_style: super::ChatStyle::Editorial,
            chat_text_sizes: super::TextSizes::from_stored(Some(
                r#"{"user":19.5,"assistant":18,"code_block":16}"#.into(),
            )),
            show_inline_images: false,
            load_remote_images: true,
            editor: Some("Zed".into()),
            terminal: Some("Ghostty".into()),
            browser: Some("Firefox".into()),
            agents_filter: crate::agents::FinishedFilter::Failed,
        };

        store_in(&path, &preferences).expect("preferences store");
        assert_eq!(load_from(&path).expect("preferences load"), preferences);

        // A stale Mock selection cannot activate testing with Developer mode off.
        let disabled = Preferences {
            developer_mode: false,
            ..preferences.clone()
        };
        assert_eq!(
            disabled.effective_backend(),
            crate::mock_inference::Backend::Omp
        );
        store_in(&path, &disabled).expect("disable developer mode");
        let reloaded = load_from(&path).expect("disabled preferences load");
        assert!(!reloaded.developer_mode);
        assert_eq!(
            reloaded.inference_backend,
            crate::mock_inference::Backend::Omp
        );
        assert_eq!(reloaded.model, preferences.model);

        store_in(&path, &Preferences::default()).expect("preferences clear");
        assert_eq!(
            load_from(&path).expect("cleared preferences load"),
            Preferences::default()
        );
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn schema_migration_runs_once_and_is_idempotent() {
        let (scratch, path) = scratch_database("migration");
        let version = |path: &Path| -> i64 {
            super::connection(path)
                .expect("connection")
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .expect("user_version")
        };
        assert_eq!(version(&path), super::SCHEMA_VERSION);
        assert_eq!(version(&path), super::SCHEMA_VERSION);
        assert!(load_accounts_from(&path).expect("accounts").is_empty());
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn accounts_keep_labels_and_defaults_across_repeated_records() {
        let (scratch, path) = scratch_database("accounts");
        record_account_in(&path, "anthropic:a@x", "anthropic", Some("a@x"), Some(10))
            .expect("first record");
        record_account_in(&path, "anthropic:b@x", "anthropic", Some("b@x"), Some(20))
            .expect("second record");
        set_account_label_in(&path, "anthropic:a@x", "Personal").expect("label");
        set_default_account_in(&path, "anthropic", Some("anthropic:a@x")).expect("default");

        // A later sighting without a timestamp keeps the original timestamp,
        // label, and default while still refreshing the principal.
        record_account_in(&path, "anthropic:a@x", "anthropic", Some("A@x"), None)
            .expect("re-record");
        let accounts = load_accounts_from(&path).expect("accounts");
        assert_eq!(
            accounts,
            vec![
                StoredAccount {
                    account_id: "anthropic:a@x".into(),
                    provider: "anthropic".into(),
                    principal: Some("A@x".into()),
                    label: Some("Personal".into()),
                    authenticated_at_ms: 10,
                    is_default: true,
                },
                StoredAccount {
                    account_id: "anthropic:b@x".into(),
                    provider: "anthropic".into(),
                    principal: Some("b@x".into()),
                    label: None,
                    authenticated_at_ms: 20,
                    is_default: false,
                },
            ]
        );

        // A fresh authentication moves the timestamp; the default moves to the
        // newly chosen account and never applies to two accounts at once.
        record_account_in(&path, "anthropic:b@x", "anthropic", None, Some(30)).expect("reauth");
        set_default_account_in(&path, "anthropic", Some("anthropic:b@x")).expect("default");
        let accounts = load_accounts_from(&path).expect("accounts");
        assert_eq!(accounts[0].is_default, false);
        assert_eq!(accounts[1].authenticated_at_ms, 30);
        assert!(accounts[1].is_default);
        set_default_account_in(&path, "anthropic", None).expect("clear default");
        assert!(
            load_accounts_from(&path)
                .expect("accounts")
                .iter()
                .all(|account| !account.is_default)
        );
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn project_choices_follow_account_lifecycle() {
        let (scratch, path) = scratch_database("projects");
        let project = "project-1";
        record_account_in(&path, "openai-codex:1", "openai-codex", None, Some(1)).expect("one");
        record_account_in(&path, "openai-codex:2", "openai-codex", None, Some(2)).expect("two");
        assert_eq!(
            project_account_in(&path, project, "openai-codex").expect("empty"),
            None
        );
        set_project_account_in(&path, project, "openai-codex", "openai-codex:1").expect("choose");
        set_project_account_in(&path, project, "openai-codex", "openai-codex:2")
            .expect("re-choose");
        assert_eq!(
            project_account_in(&path, project, "openai-codex").expect("choice"),
            Some("openai-codex:2".into())
        );

        // Forgetting an account also forgets every project choice naming it.
        forget_account_in(&path, "openai-codex:2").expect("forget");
        assert_eq!(
            project_account_in(&path, project, "openai-codex").expect("cleared"),
            None
        );
        assert_eq!(load_accounts_from(&path).expect("accounts").len(), 1);

        // Pruning against OMP's live list removes rows OMP no longer stores.
        retain_accounts_in(&path, &[]).expect("prune all");
        assert!(load_accounts_from(&path).expect("accounts").is_empty());
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn projects_group_directories_and_cascade_to_sessions_and_accounts() {
        let (scratch, path) = scratch_database("project-registry");
        let work = create_project_in(&path, "work", &["/work/app".into(), "/work/lib".into()])
            .expect("create work");
        let side = create_project_in(&path, "side", &["/side".into()]).expect("create side");
        rename_project_in(&path, &side.id, "Side quest").expect("rename");
        set_project_collapsed_in(&path, &side.id, true).expect("collapse");
        touch_project_in(&path, &work.id).expect("touch");
        add_project_directory_in(&path, &work.id, Path::new("/work/docs")).expect("add dir");
        // A directory belongs to one project; a competing owner gets a visible error.
        assert!(matches!(add_project_directory_in(&path, &side.id, Path::new("/work/docs")), Err(super::PreferencesError::DirectoryOwned { .. })));
        add_project_directory_in(&path, &work.id, Path::new("/work/docs")).expect("same owner is idempotent");
        remove_project_directory_in(&path, &work.id, Path::new("/work/lib")).expect("remove dir");

        let projects = load_projects_from(&path).expect("projects");
        assert_eq!(projects.len(), 2);
        assert_eq!(
            projects[0].id, work.id,
            "projects keep their creation order until explicitly reordered, so \
             touching one does not move it"
        );
        assert_eq!(
            projects[0].directories,
            vec![PathBuf::from("/work/app"), PathBuf::from("/work/docs")]
        );
        assert_eq!(projects[1].name, "Side quest");
        assert!(projects[1].collapsed);
        assert_eq!(projects[1].directories, vec![PathBuf::from("/side")]);

        record_session_in(
            &path,
            &SessionRecord {
                session_id: "s1".into(),
                project_id: side.id.clone(),
                directory: "/side".into(),
                parent_session_id: None,
                branch: None,
                base_branch: None,
                worktree_path: None,
                created_at_ms: 1,
            },
        )
        .expect("session");
        set_project_account_in(&path, &side.id, "anthropic", "anthropic:a").expect("account");

        delete_project_in(&path, &side.id).expect("delete");
        assert_eq!(load_projects_from(&path).expect("projects").len(), 1);
        assert!(
            load_session_records_from(&path)
                .expect("sessions")
                .is_empty()
        );
        assert_eq!(
            project_account_in(&path, &side.id, "anthropic").expect("choice"),
            None
        );
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn project_order_is_stable_until_explicitly_reordered() {
        let (scratch, path) = scratch_database("project-order");
        let first = create_project_in(&path, "first", &["/a".into()]).expect("first");
        let second = create_project_in(&path, "second", &["/b".into()]).expect("second");
        let third = create_project_in(&path, "third", &["/c".into()]).expect("third");

        // New projects append; opening one does not move it.
        touch_project_in(&path, &first.id).expect("touch");
        let ids = |path: &Path| {
            load_projects_from(path)
                .expect("projects")
                .into_iter()
                .map(|project| project.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(&path),
            vec![first.id.clone(), second.id.clone(), third.id.clone()],
            "creation order is preserved regardless of last-opened"
        );

        // A drag reorders explicitly and durably.
        set_project_order_in(
            &path,
            &[third.id.clone(), first.id.clone(), second.id.clone()],
        )
        .expect("reorder");
        assert_eq!(
            ids(&path),
            vec![third.id.clone(), first.id.clone(), second.id.clone()]
        );

        // A project created after a reorder still appends to the end.
        let fourth = create_project_in(&path, "fourth", &["/d".into()]).expect("fourth");
        assert_eq!(ids(&path).last(), Some(&fourth.id));
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn session_order_persists_by_id_including_unrecorded_sessions() {
        let (scratch, path) = scratch_database("session-order");
        // Sessions need not have a full record — positions key on the id alone.
        set_session_order_in(&path, &["s3".into(), "s1".into(), "s2".into()]).expect("order");
        let positions = load_session_positions_from(&path).expect("positions");
        assert_eq!(positions.get("s3"), Some(&0));
        assert_eq!(positions.get("s1"), Some(&1));
        assert_eq!(positions.get("s2"), Some(&2));
        assert_eq!(positions.get("unranked"), None);

        // Re-ordering overwrites existing positions.
        set_session_order_in(&path, &["s1".into(), "s2".into(), "s3".into()]).expect("reorder");
        let positions = load_session_positions_from(&path).expect("positions");
        assert_eq!(positions.get("s1"), Some(&0));
        assert_eq!(positions.get("s3"), Some(&2));
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn session_records_upsert_without_losing_fork_lineage() {
        let (scratch, path) = scratch_database("session-records");
        let project = create_project_in(&path, "app", &["/app".into()]).expect("create");
        let fork = SessionRecord {
            session_id: "child".into(),
            project_id: project.id.clone(),
            directory: "/worktrees/app/feature".into(),
            parent_session_id: Some("parent".into()),
            branch: Some("omperor/feature".into()),
            base_branch: Some("main".into()),
            worktree_path: Some("/worktrees/app/feature".into()),
            created_at_ms: 5,
        };
        record_session_in(&path, &fork).expect("record fork");
        // A later plain sighting keeps the lineage it does not know about.
        record_session_in(
            &path,
            &SessionRecord {
                parent_session_id: None,
                branch: None,
                base_branch: None,
                worktree_path: None,
                created_at_ms: 9,
                ..fork.clone()
            },
        )
        .expect("re-record");
        let records = load_session_records_from(&path).expect("records");
        assert_eq!(
            records,
            vec![SessionRecord {
                created_at_ms: 5,
                ..fork
            }]
        );
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }

    #[test]
    fn bootstrap_flag_and_directory_account_adoption() {
        let (scratch, path) = scratch_database("bootstrap");
        assert!(!projects_bootstrapped_in(&path).expect("flag"));
        mark_projects_bootstrapped_in(&path).expect("mark");
        assert!(projects_bootstrapped_in(&path).expect("flag"));

        // Choices recorded against a path before projects existed move to the
        // project that now owns the directory; an existing project-keyed row wins.
        set_project_account_in(&path, "/work/app", "anthropic", "anthropic:old").expect("old");
        set_project_account_in(&path, "/work/app", "openai-codex", "openai-codex:1").expect("old");
        let project = create_project_in(&path, "app", &["/work/app".into()]).expect("create");
        set_project_account_in(&path, &project.id, "anthropic", "anthropic:new").expect("new");
        adopt_directory_account_choices_in(&path, Path::new("/work/app"), &project.id)
            .expect("adopt");
        assert_eq!(
            project_account_in(&path, &project.id, "anthropic").expect("anthropic"),
            Some("anthropic:new".into())
        );
        assert_eq!(
            project_account_in(&path, &project.id, "openai-codex").expect("codex"),
            Some("openai-codex:1".into())
        );
        assert_eq!(
            project_account_in(&path, "/work/app", "openai-codex").expect("path"),
            None
        );
        std::fs::remove_dir_all(scratch).expect("scratch cleanup");
    }
}
