//! The `omperor_` tools: Omperor's own workspace (its sessions, projects,
//! and archives) offered to the agent as host tools, answered by the app.
//! They are never part of a session by default. The user turns them on
//! for one session with `/omperor`, and they stay on for it until
//! `/omperor off`.
//!
//! This module is the catalog: each tool's name, description, and schema,
//! how a call's arguments are read, and the JSON an answer is shaped into.
//! Pure, so it is tested on its own; `housekeeping_ui` does the work.

use std::path::PathBuf;

use omp_rpc::protocol::HostToolDefinition;
use serde_json::{Map, Value, json};

/// Every tool's name starts with this. Providers allow only letters,
/// digits, `_`, and `-` in a tool name, so the namespace is a prefix.
pub(crate) const PREFIX: &str = "omperor_";

const LIST_PROJECTS: &str = "omperor_list_projects";
const LIST_SESSIONS: &str = "omperor_list_sessions";
const DELETE_SESSIONS: &str = "omperor_delete_sessions";
const ARCHIVE_PROJECT: &str = "omperor_archive_project";
const LIST_ARCHIVES: &str = "omperor_list_archives";
const DELETE_ARCHIVE: &str = "omperor_delete_archive";
const RESTORE_ARCHIVE: &str = "omperor_restore_archive";
const REMOVE_EMPTY_SESSIONS: &str = "omperor_remove_empty_sessions";

/// The most sessions one call lists or deletes.
pub(crate) const MAX_SESSIONS: usize = 500;

/// What every description opens with, so the model knows which workspace
/// these tools act on.
const SCOPE: &str = "Omperor is the app this conversation runs in. The omperor_ tools manage the user's Omperor workspace (its sessions, projects and archives), never the code in the working directory.";

/// A call the agent made, its arguments read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    ListProjects,
    ListSessions {
        /// A project id, or [`UNFILED`] for sessions in no project.
        project_id: Option<String>,
        kind: Option<SessionKind>,
        limit: usize,
        offset: usize,
    },
    DeleteSessions {
        session_ids: Vec<String>,
        reason: String,
    },
    ArchiveProject {
        project_id: String,
        reason: String,
    },
    ListArchives,
    DeleteArchive {
        archive_id: String,
        reason: String,
    },
    RestoreArchive {
        archive_id: String,
    },
    RemoveEmptySessions,
}

/// `project_id` for sessions filed under no project.
pub(crate) const UNFILED: &str = "unfiled";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionKind {
    /// A conversation with a journal on disk.
    Conversation,
    /// A new chat nothing was sent from yet.
    Draft,
    /// A session of the mock backend, which keeps nothing on disk.
    Mock,
}

impl SessionKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Draft => "draft",
            Self::Mock => "mock",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [Self::Conversation, Self::Draft, Self::Mock]
            .into_iter()
            .find(|kind| kind.name() == name)
    }
}

fn definition(
    name: &str,
    label: &str,
    description: String,
    parameters: Value,
) -> HostToolDefinition {
    HostToolDefinition {
        name: name.to_owned(),
        label: Some(label.to_owned()),
        description,
        parameters,
        hidden: false,
        load_mode: None,
    }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// The tools, as advertised to OMP.
pub(crate) fn definitions() -> Vec<HostToolDefinition> {
    let reason = json!({
        "type": "string",
        "description": "One sentence, shown to the user in the confirmation dialog, saying why.",
    });
    vec![
        definition(
            LIST_PROJECTS,
            "List Omperor projects",
            format!(
                "{SCOPE} Lists the projects in Omperor's sidebar: each one's id, name, folders, and how many conversations and drafts it holds, plus the sessions filed under no project. Start here; the ids work with the other omperor_ tools."
            ),
            object(json!({}), &[]),
        ),
        definition(
            LIST_SESSIONS,
            "List Omperor sessions",
            format!(
                "{SCOPE} Lists sessions newest first with what each holds: kind (\"conversation\" has a journal on disk, \"draft\" is a new chat nothing was sent from, \"mock\" is a test session of the mock backend), title, project, last activity, model, how its last turn ended, whether it is running, its sub-agents, its size on disk, and any worktree it was forked into. The session making the call is marked current."
            ),
            object(
                json!({
                    "project_id": { "type": "string", "description": "Only this project's sessions; \"unfiled\" for sessions in no project." },
                    "kind": { "type": "string", "enum": ["conversation", "draft", "mock"], "description": "Only sessions of this kind." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SESSIONS, "description": "At most this many (default 100)." },
                    "offset": { "type": "integer", "minimum": 0, "description": "Skip this many, to page through a long list." },
                }),
                &[],
            ),
        ),
        definition(
            DELETE_SESSIONS,
            "Delete Omperor sessions",
            format!(
                "{SCOPE} Deletes sessions for good: each one's journal, the sub-agent transcripts only it refers to, its saved view, metrics and registry record. A worktree Omperor created when the session was forked is removed too if git finds nothing uncommitted or untracked in it; its branch and commits are always kept. Omperor asks the user to confirm all of them at once. The answer says what was deleted, what was not, and why. A session running a turn, and the current session, cannot be deleted. To keep a project's sessions but clear them from the sidebar, archive the project instead."
            ),
            object(
                json!({
                    "session_ids": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": MAX_SESSIONS, "description": "Ids from omperor_list_sessions." },
                    "reason": reason,
                }),
                &["session_ids", "reason"],
            ),
        ),
        definition(
            ARCHIVE_PROJECT,
            "Archive an Omperor project",
            format!(
                "{SCOPE} Archives a project: its sessions, their sub-agent transcripts, saved views, metrics and registry rows go into one compressed archive private to Omperor, and the project leaves the sidebar. Its folders, worktrees and code are not touched. omperor_restore_archive brings it back. Omperor asks the user to confirm. A project with a session running a turn, or holding the current session, cannot be archived."
            ),
            object(
                json!({
                    "project_id": { "type": "string", "description": "An id from omperor_list_projects." },
                    "reason": reason,
                }),
                &["project_id", "reason"],
            ),
        ),
        definition(
            REMOVE_EMPTY_SESSIONS,
            "Remove empty Omperor sessions",
            format!(
                "{SCOPE} Removes the sessions OMP started that never received a message. No list shows them, omperor_list_sessions included, and they cannot be resumed, so nothing is lost; ones still open are left. The answer says how many went and the space freed."
            ),
            object(json!({}), &[]),
        ),
        definition(
            LIST_ARCHIVES,
            "List Omperor archives",
            format!(
                "{SCOPE} Lists archived projects, newest first: each archive's id, the project's name and folders, when it was archived, how many sessions it holds, and its size."
            ),
            object(json!({}), &[]),
        ),
        definition(
            DELETE_ARCHIVE,
            "Delete an Omperor archive",
            format!(
                "{SCOPE} Deletes an archived project for good; it can no longer be restored. Omperor asks the user to confirm."
            ),
            object(
                json!({
                    "archive_id": { "type": "string", "description": "An id from omperor_list_archives." },
                    "reason": reason,
                }),
                &["archive_id", "reason"],
            ),
        ),
        definition(
            RESTORE_ARCHIVE,
            "Restore an Omperor archive",
            format!(
                "{SCOPE} Puts an archived project back in the sidebar with all its sessions, and removes the archive."
            ),
            object(
                json!({ "archive_id": { "type": "string", "description": "An id from omperor_list_archives." } }),
                &["archive_id"],
            ),
        ),
    ]
}

fn string(arguments: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.trim().to_owned())),
        Some(Value::String(_)) => Err(format!("`{key}` is empty")),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn required(arguments: &Map<String, Value>, key: &str) -> Result<String, String> {
    string(arguments, key)?.ok_or_else(|| format!("`{key}` is required"))
}

fn count(
    arguments: &Map<String, Value>,
    key: &str,
    default: usize,
    max: usize,
) -> Result<usize, String> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value <= max)
            .ok_or_else(|| format!("`{key}` must be a whole number from 0 to {max}")),
    }
}

/// Reads a call's arguments, refusing anything the schema does not allow.
pub(crate) fn parse(tool: &str, arguments: &Map<String, Value>) -> Result<Request, String> {
    if !tool.starts_with(PREFIX) {
        return Err(format!("{tool} is not one of Omperor's tools"));
    }
    let allowed: &[&str] = match tool {
        LIST_PROJECTS | LIST_ARCHIVES | REMOVE_EMPTY_SESSIONS => &[],
        LIST_SESSIONS => &["project_id", "kind", "limit", "offset"],
        DELETE_SESSIONS => &["session_ids", "reason"],
        ARCHIVE_PROJECT => &["project_id", "reason"],
        DELETE_ARCHIVE => &["archive_id", "reason"],
        RESTORE_ARCHIVE => &["archive_id"],
        _ => return Err(format!("Omperor has no tool named {tool}")),
    };
    if let Some(unknown) = arguments
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(format!(
            "{tool} takes no `{unknown}`; it takes {}",
            if allowed.is_empty() {
                "no arguments".to_owned()
            } else {
                allowed.join(", ")
            }
        ));
    }
    Ok(match tool {
        LIST_PROJECTS => Request::ListProjects,
        LIST_ARCHIVES => Request::ListArchives,
        REMOVE_EMPTY_SESSIONS => Request::RemoveEmptySessions,
        LIST_SESSIONS => Request::ListSessions {
            project_id: string(arguments, "project_id")?,
            kind: string(arguments, "kind")?
                .map(|name| {
                    SessionKind::parse(&name).ok_or_else(|| {
                        format!("`kind` must be conversation, draft or mock, not {name}")
                    })
                })
                .transpose()?,
            limit: count(arguments, "limit", 100, MAX_SESSIONS)?.max(1),
            offset: count(arguments, "offset", 0, usize::MAX)?,
        },
        DELETE_SESSIONS => {
            let ids = arguments
                .get("session_ids")
                .and_then(Value::as_array)
                .ok_or("`session_ids` must be a list of session ids")?;
            if ids.is_empty() || ids.len() > MAX_SESSIONS {
                return Err(format!(
                    "`session_ids` must name 1 to {MAX_SESSIONS} sessions"
                ));
            }
            let mut session_ids = Vec::with_capacity(ids.len());
            for id in ids {
                let id = id
                    .as_str()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or("each session id must be a string")?;
                if !session_ids.iter().any(|seen| seen == id) {
                    session_ids.push(id.to_owned());
                }
            }
            Request::DeleteSessions {
                session_ids,
                reason: required(arguments, "reason")?,
            }
        }
        ARCHIVE_PROJECT => Request::ArchiveProject {
            project_id: required(arguments, "project_id")?,
            reason: required(arguments, "reason")?,
        },
        DELETE_ARCHIVE => Request::DeleteArchive {
            archive_id: required(arguments, "archive_id")?,
            reason: required(arguments, "reason")?,
        },
        RESTORE_ARCHIVE => Request::RestoreArchive {
            archive_id: required(arguments, "archive_id")?,
        },
        _ => unreachable!("checked above"),
    })
}

/// What the app knows about one session, for `omperor_list_sessions`.
#[derive(Debug, Clone)]
pub(crate) struct SessionFacts {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) kind: SessionKind,
    pub(crate) project_id: Option<String>,
    pub(crate) directory: Option<PathBuf>,
    pub(crate) updated_ms: i64,
    pub(crate) model: Option<String>,
    pub(crate) last_turn: Option<&'static str>,
    /// `running` (a turn), `open` (a live OMP child, idle), or `closed`.
    pub(crate) state: &'static str,
    pub(crate) sub_agents: usize,
    pub(crate) worktree: Option<PathBuf>,
    pub(crate) branch: Option<String>,
    pub(crate) parent_id: Option<String>,
    pub(crate) current: bool,
    /// The journal and its sub-agents' journals, for sizing on a worker.
    pub(crate) journals: Vec<PathBuf>,
}

/// What `omperor_list_projects` reports for one project.
#[derive(Debug, Clone)]
pub(crate) struct ProjectFacts {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) directories: Vec<PathBuf>,
    pub(crate) conversations: usize,
    pub(crate) drafts: usize,
    pub(crate) running: usize,
    pub(crate) current: bool,
}

pub(crate) fn projects_answer(projects: &[ProjectFacts], unfiled: (usize, usize)) -> Value {
    json!({
        "projects": projects.iter().map(|project| json!({
            "id": project.id,
            "name": project.name,
            "folders": project.directories,
            "conversations": project.conversations,
            "drafts": project.drafts,
            "running": project.running,
            "current": project.current,
        })).collect::<Vec<_>>(),
        "unfiled": { "project_id": UNFILED, "conversations": unfiled.0, "drafts": unfiled.1 },
    })
}

/// The `omperor_list_sessions` answer: `page` already filtered and cut,
/// `sizes` its bytes on disk in the same order.
pub(crate) fn sessions_answer(
    page: &[SessionFacts],
    sizes: &[u64],
    total: usize,
    offset: usize,
) -> Value {
    let sessions: Vec<Value> = page
        .iter()
        .zip(sizes.iter().copied().chain(std::iter::repeat(0)))
        .map(|(session, bytes)| {
            let mut entry = json!({
                "id": session.id,
                "title": session.title,
                "kind": session.kind.name(),
                "project_id": session.project_id.as_deref().unwrap_or(UNFILED),
                "updated_ms": session.updated_ms,
                "state": session.state,
                "bytes": bytes,
            });
            let fields = entry.as_object_mut().expect("an object");
            let mut set = |key: &str, value: Value| {
                fields.insert(key.into(), value);
            };
            if let Some(directory) = &session.directory {
                set("folder", json!(directory));
            }
            if let Some(model) = &session.model {
                set("model", json!(model));
            }
            if let Some(last_turn) = session.last_turn {
                set("last_turn", json!(last_turn));
            }
            if session.sub_agents > 0 {
                set("sub_agents", json!(session.sub_agents));
            }
            if let Some(worktree) = &session.worktree {
                set(
                    "worktree",
                    json!({ "path": worktree, "branch": session.branch }),
                );
            }
            if let Some(parent) = &session.parent_id {
                set("forked_from", json!(parent));
            }
            if session.current {
                set("current", json!(true));
            }
            entry
        })
        .collect();
    let mut answer = json!({ "total": total, "offset": offset, "sessions": sessions });
    if offset + page.len() < total {
        answer["next_offset"] = json!(offset + page.len());
    }
    answer
}

/// Filters and orders sessions for one `omperor_list_sessions` call:
/// newest first, then the page asked for. Returns the page and the total.
pub(crate) fn select_sessions(
    mut sessions: Vec<SessionFacts>,
    project_id: Option<&str>,
    kind: Option<SessionKind>,
    limit: usize,
    offset: usize,
) -> (Vec<SessionFacts>, usize) {
    sessions.retain(|session| {
        kind.is_none_or(|kind| session.kind == kind)
            && project_id
                .is_none_or(|wanted| session.project_id.as_deref().unwrap_or(UNFILED) == wanted)
    });
    sessions.sort_by(|a, b| {
        b.updated_ms
            .cmp(&a.updated_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    let total = sessions.len();
    let page = sessions.into_iter().skip(offset).take(limit).collect();
    (page, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn every_tool_is_in_the_namespace_with_a_closed_object_schema() {
        let definitions = definitions();
        assert_eq!(definitions.len(), 8);
        for definition in &definitions {
            assert!(definition.name.starts_with(PREFIX), "{}", definition.name);
            // What OpenAI and Anthropic accept as a tool name.
            assert!(definition.name.len() <= 64);
            assert!(
                definition
                    .name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            );
            assert_eq!(definition.parameters["type"], "object");
            assert_eq!(definition.parameters["additionalProperties"], false);
            assert!(definition.description.starts_with(SCOPE));
            // Every declared argument is one `parse` accepts.
            let properties = definition.parameters["properties"].as_object().unwrap();
            for key in properties.keys() {
                let error = parse(&definition.name, &arguments(json!({ key: 7 })))
                    .err()
                    .unwrap_or_default();
                assert!(
                    !error.contains("takes no"),
                    "{} rejects its own `{key}`",
                    definition.name
                );
            }
        }
    }

    #[test]
    fn calls_are_read_strictly() {
        assert_eq!(parse(LIST_PROJECTS, &Map::new()), Ok(Request::ListProjects));
        assert_eq!(
            parse(
                LIST_SESSIONS,
                &arguments(json!({ "kind": "draft", "limit": 20 }))
            ),
            Ok(Request::ListSessions {
                project_id: None,
                kind: Some(SessionKind::Draft),
                limit: 20,
                offset: 0
            })
        );
        assert!(parse(LIST_SESSIONS, &arguments(json!({ "kind": "journal" }))).is_err());
        assert!(parse(LIST_SESSIONS, &arguments(json!({ "limit": 501 }))).is_err());
        assert_eq!(
            parse(
                DELETE_SESSIONS,
                &arguments(
                    json!({ "session_ids": ["a", " b ", "a"], "reason": "Empty mock runs" })
                )
            ),
            Ok(Request::DeleteSessions {
                session_ids: vec!["a".into(), "b".into()],
                reason: "Empty mock runs".into()
            })
        );
        assert!(
            parse(DELETE_SESSIONS, &arguments(json!({ "session_ids": ["a"] })))
                .unwrap_err()
                .contains("reason")
        );
        assert!(
            parse(
                DELETE_SESSIONS,
                &arguments(json!({ "session_ids": [], "reason": "x" }))
            )
            .is_err()
        );
        assert!(
            parse(
                DELETE_SESSIONS,
                &arguments(json!({ "session_ids": [1], "reason": "x" }))
            )
            .is_err()
        );
        assert!(
            parse(
                ARCHIVE_PROJECT,
                &arguments(json!({ "project_id": "p", "reason": "done", "force": true }))
            )
            .unwrap_err()
            .contains("force")
        );
        assert!(parse("omperor_format_disk", &Map::new()).is_err());
    }

    #[test]
    fn sessions_list_newest_first_and_page() {
        let facts =
            |id: &str, updated_ms: i64, kind: SessionKind, project: Option<&str>| SessionFacts {
                id: id.into(),
                title: id.into(),
                kind,
                project_id: project.map(Into::into),
                directory: None,
                updated_ms,
                model: None,
                last_turn: None,
                state: "closed",
                sub_agents: 0,
                worktree: None,
                branch: None,
                parent_id: None,
                current: false,
                journals: Vec::new(),
            };
        let all = vec![
            facts("old", 1, SessionKind::Conversation, Some("p")),
            facts("new", 3, SessionKind::Conversation, Some("p")),
            facts("draft", 2, SessionKind::Draft, None),
        ];
        let (page, total) = select_sessions(all.clone(), None, None, 2, 0);
        assert_eq!(total, 3);
        assert_eq!(
            page.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["new", "draft"]
        );
        let answer = sessions_answer(&page, &[10, 0], total, 0);
        assert_eq!(answer["next_offset"], 2);
        assert_eq!(answer["sessions"][1]["project_id"], UNFILED);
        assert_eq!(answer["sessions"][0]["bytes"], 10);
        let (page, total) = select_sessions(all, Some(UNFILED), None, 10, 0);
        assert_eq!((page.len(), total), (1, 1));
    }
}
