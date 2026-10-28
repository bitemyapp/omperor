//! Lock-free reader for OMP's journal-first `.oms` sessions.
//!
//! The live OMP child holds a journal's exclusive writer lock for its whole
//! lifetime, so the sidebar and history views never open a journal as a
//! writer. They read the committed entries through `Journal::scan`, which
//! takes no lock and never truncates, and fold the live chain (the branch
//! reachable from the file tail) into the same message shape the legacy JSONL
//! transcripts produce.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use omp_journal::{Entry, EntryId, Journal, data, kind, live_chain};
use serde_json::Value;

use crate::chat;
use crate::session::{SessionMessage, SessionOutcome, Speaker};

/// File extension of a journal-first OMP session.
pub(crate) const JOURNAL_EXTENSION: &str = "oms";

/// Everything the GUI needs from one journal without launching a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JournalTranscript {
    /// Project root recorded at session creation.
    pub(crate) cwd: Option<PathBuf>,
    /// First user prompt, the fallback title when the session was not named.
    pub(crate) first_prompt: Option<String>,
    /// Live-chain messages in chronological order.
    pub(crate) messages: Vec<SessionMessage>,
    /// The model the session last ran on, from the prompt facts OMP records
    /// at every launch.
    pub(crate) model: Option<String>,
    /// How the last turn ended.
    pub(crate) outcome: Option<SessionOutcome>,
    /// Ids of the sub-agents this session spawned; each has its own journal
    /// beside this one, named by its id.
    pub(crate) spawned: Vec<String>,
}

impl JournalTranscript {
    /// Whether a person or the assistant has said anything yet; tool events
    /// alone do not make a session worth listing or resuming.
    pub(crate) fn has_conversation(&self) -> bool {
        self.messages
            .iter()
            .any(|message| matches!(message.speaker, Speaker::User | Speaker::Assistant))
    }
}

/// Whether `path` names a journal, by its extension alone: safe on the
/// interface thread, which never touches the disk.
pub(crate) fn names_journal(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == JOURNAL_EXTENSION)
}

pub(crate) fn is_journal(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|extension| extension == JOURNAL_EXTENSION)
}

/// Reads a journal's committed live chain.
pub(crate) fn read(path: &Path) -> Result<JournalTranscript, String> {
    let entries = Journal::scan(path).map_err(|error| error.to_string())?;
    Ok(fold(&entries))
}

/// Whether the journal at `path` has persisted at least one conversational
/// message, which is what OMP requires before it will resume the session.
pub(crate) fn has_conversation(path: &Path) -> bool {
    read(path).is_ok_and(|transcript| transcript.has_conversation())
}

/// Folds committed entries into messages. Assistant text arrives as stream
/// appends after the start entry, so each open text stream is attached to
/// the assistant message that opened it.
pub(crate) fn fold(entries: &[Entry]) -> JournalTranscript {
    let cwd = entries
        .first()
        .and_then(|genesis| serde_json::from_str::<data::Genesis>(genesis.data.as_str()).ok())
        .map(|genesis| PathBuf::from(genesis.cwd.as_str()))
        .filter(|cwd| !cwd.as_os_str().is_empty());
    let mut messages: Vec<SessionMessage> = Vec::new();
    let mut first_prompt = None;
    let mut open_assistant: Option<usize> = None;
    let mut text_streams: HashMap<u32, usize> = HashMap::new();
    // OMP streams a reasoning summary and the reply alike into a `text`
    // property; what tells them apart is the `assistant.content` patch that
    // inserts the node just before its stream opens, so the kind it carries
    // is held until that open arrives.
    let mut next_content_kind: Option<String> = None;
    // A tool call resolves across several entries: the call names the tool, an
    // optional `kernel: ready` update carries streamed arguments, and the
    // result settles its status. Every later entry is caused `by` the call, so
    // pending tools are keyed by the call's entry id and finalized in place
    // once the whole chain has been read.
    let mut pending: Vec<PendingTool> = Vec::new();
    let mut tool_by_entry: HashMap<EntryId, usize> = HashMap::new();
    let mut arg_streams: HashMap<u32, usize> = HashMap::new();
    // The session's state: a turn opens at `turn.start` and closes at its
    // receipt, an error notice marks the turn failed, and a session exit
    // while a turn is open means the child was cut off.
    let mut model = None;
    let mut outcome = None;
    let mut turn_open = false;
    let mut turn_failed = false;
    let mut spawned = Vec::new();
    for entry in live_chain(entries) {
        match entry.kind.name.as_str() {
            kind::TURN_START => {
                turn_open = true;
                turn_failed = false;
                outcome = Some(SessionOutcome::InProgress);
            }
            kind::TURN_RECEIPT => {
                turn_open = false;
                outcome = Some(if turn_failed {
                    SessionOutcome::Errored
                } else {
                    SessionOutcome::Completed
                });
            }
            kind::PATCH => match entry.label.as_deref() {
                Some("prompt.facts") => {
                    if let Some(identifier) = prompt_facts_model(entry.data.as_str()) {
                        model = Some(identifier);
                    }
                }
                Some("assistant.content") => {
                    next_content_kind = content_kind(entry.data.as_str());
                }
                Some("kernel.notice") => {
                    // An error notice is how a failed turn ends: no receipt
                    // follows, and the session's later exit must not read as
                    // an interruption. Its text is the reason, which the
                    // replayed history says the way the live view did.
                    if let Some(reason) = error_notice_text(entry.data.as_str()) {
                        turn_failed = true;
                        turn_open = false;
                        outcome = Some(SessionOutcome::Errored);
                        // Dated, so a failure replayed from last week does
                        // not read as something that just happened.
                        let at_ms =
                            i64::try_from(entry.id.as_ulid().timestamp_ms()).unwrap_or(i64::MAX);
                        messages.push(SessionMessage::spoken(
                            Speaker::Event,
                            turn_error_line(&reason, Some(&crate::rules::format_timestamp(at_ms))),
                        ));
                    }
                }
                Some("session.exit") if turn_open => {
                    turn_open = false;
                    outcome = Some(SessionOutcome::Interrupted);
                }
                Some("jobs.insert") => {
                    spawned.extend(inserted_subagent_ids(entry.data.as_str()));
                }
                _ => {}
            },
            kind::MSG_USER => {
                let Ok(payload) = serde_json::from_str::<data::MsgUser>(entry.data.as_str()) else {
                    continue;
                };
                let text = payload.text.to_string();
                if first_prompt.is_none() && !text.trim().is_empty() {
                    first_prompt = Some(text.clone());
                }
                messages.push(SessionMessage::spoken(Speaker::User, text));
            }
            kind::MSG_ASSISTANT_START => {
                next_content_kind = None;
                messages.push(SessionMessage::spoken(Speaker::Assistant, String::new()));
                open_assistant = Some(messages.len() - 1);
            }
            kind::STREAM => {
                let Ok(payload) = serde_json::from_str::<data::Stream>(entry.data.as_str()) else {
                    continue;
                };
                match payload.op {
                    data::StreamOp::Open => {
                        if payload.prop.as_deref() == Some("text")
                            && let Some(owner) = open_assistant
                        {
                            let target = if next_content_kind.take().as_deref()
                                == Some("thinking")
                            {
                                // Reasoning gets its own row, as the live
                                // transcript gives it one.
                                messages.push(SessionMessage::spoken(
                                    Speaker::Thinking,
                                    String::new(),
                                ));
                                messages.len() - 1
                            } else if owner + 1 == messages.len() {
                                owner
                            } else {
                                // A reply resumed after a reasoning or tool row
                                // is a new segment; running it into the earlier
                                // text would put it before rows it followed.
                                messages.push(SessionMessage::spoken(
                                    Speaker::Assistant,
                                    String::new(),
                                ));
                                open_assistant = Some(messages.len() - 1);
                                messages.len() - 1
                            };
                            text_streams.insert(payload.sid, target);
                        }
                    }
                    data::StreamOp::Append => {
                        let Some(delta) = payload.text else { continue };
                        if let Some(owner) = text_streams.get(&payload.sid) {
                            if let Some(message) = messages.get_mut(*owner) {
                                message.text.push_str(delta.as_str());
                            }
                        } else if let Some(index) = arg_streams.get(&payload.sid)
                            && let Some(tool) = pending.get_mut(*index)
                        {
                            tool.arg_buffer.push_str(delta.as_str());
                        }
                    }
                    data::StreamOp::Close => {
                        text_streams.remove(&payload.sid);
                        arg_streams.remove(&payload.sid);
                    }
                }
            }
            kind::MSG_ASSISTANT_END => {
                open_assistant = None;
                next_content_kind = None;
            }
            kind::TOOL_CALL => {
                let Ok(payload) = serde_json::from_str::<data::ToolCall>(entry.data.as_str())
                else {
                    continue;
                };
                let index = pending.len();
                let message_index = messages.len();
                // A placeholder holds the row's position; it is overwritten once
                // the call's arguments and result have been folded.
                messages.push(SessionMessage::spoken(Speaker::Event, String::new()));
                if let Some(sid) = payload.sid {
                    arg_streams.insert(sid, index);
                }
                pending.push(PendingTool {
                    name: payload.name.to_string(),
                    message_index,
                    intent: payload
                        .i
                        .as_deref()
                        .map(str::trim)
                        .filter(|intent| !intent.is_empty())
                        .map(str::to_owned),
                    inline_args: payload.args.map(|args| args.get().to_owned()),
                    ready_args: None,
                    arg_buffer: String::new(),
                    result: None,
                    failed: false,
                });
                tool_by_entry.insert(entry.id, index);
            }
            kind::TOOL_UPDATE => {
                let Some(index) = entry.by.and_then(|by| tool_by_entry.get(&by).copied()) else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(entry.data.as_str()) else {
                    continue;
                };
                if value.get("kernel").and_then(Value::as_str) == Some("ready")
                    && let Some(tool) = pending.get_mut(index)
                {
                    if let Some(args) = value.get("args") {
                        tool.ready_args = Some(args.to_string());
                    }
                    if tool.intent.is_none()
                        && let Some(intent) = value
                            .get("i")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|intent| !intent.is_empty())
                    {
                        tool.intent = Some(intent.to_owned());
                    }
                }
            }
            kind::TOOL_RESULT => {
                let Some(index) = entry.by.and_then(|by| tool_by_entry.get(&by).copied()) else {
                    continue;
                };
                let Ok(payload) = serde_json::from_str::<data::ToolResult>(entry.data.as_str())
                else {
                    continue;
                };
                if let Some(tool) = pending.get_mut(index) {
                    let (failed, text) = tool_result_text(&payload);
                    tool.failed = failed;
                    tool.result = Some(text);
                }
            }
            _ => {}
        }
    }
    for tool in &pending {
        finalize_tool(&mut messages, tool);
    }
    // An assistant turn that never produced text (aborted, tool-only) has
    // nothing to render, and neither does a reasoning node that stayed empty.
    messages.retain(|message| {
        !matches!(message.speaker, Speaker::Assistant | Speaker::Thinking)
            || !message.text.is_empty()
    });
    JournalTranscript {
        cwd,
        first_prompt,
        messages,
        model,
        outcome,
        spawned,
    }
}

/// The ids of the `<subagent>` elements a `jobs.insert` patch adds:
/// `["ins", parent, after, {"tag": "subagent", "props": [["id", …], …]}]`.
pub(crate) fn inserted_subagent_ids(data: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return Vec::new();
    };
    value
        .get("ops")
        .and_then(Value::as_array)
        .map(|ops| {
            ops.iter()
                .filter_map(Value::as_array)
                .filter(|op| op.first().and_then(Value::as_str) == Some("ins"))
                .filter_map(|op| op.last())
                .filter(|node| node.get("tag").and_then(Value::as_str) == Some("subagent"))
                .filter_map(|node| {
                    node.get("props")?
                        .as_array()?
                        .iter()
                        .filter_map(Value::as_array)
                        .find(|pair| pair.first().and_then(Value::as_str) == Some("id"))
                        .and_then(|pair| pair.get(1)?.as_str())
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `kind` of the content node an `assistant.content` patch inserts:
/// `["ins", assistant, after, {"tag": "tool:assistant-content", "props":
/// [["kind", "thinking" | "text"], …]}]`. OMP inserts the node right before
/// it opens the node's `text` stream, so the kind names that stream.
pub(crate) fn content_kind(data: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(data).ok()?;
    value
        .get("ops")?
        .as_array()?
        .iter()
        .filter_map(Value::as_array)
        .filter(|op| op.first().and_then(Value::as_str) == Some("ins"))
        .filter_map(|op| op.last())
        .filter(|node| node.get("tag").and_then(Value::as_str) == Some("tool:assistant-content"))
        .find_map(|node| {
            node.get("props")?
                .as_array()?
                .iter()
                .filter_map(Value::as_array)
                .find(|pair| pair.first().and_then(Value::as_str) == Some("kind"))
                .and_then(|pair| pair.get(1)?.as_str())
                .map(str::to_owned)
        })
}

/// The model identifier from a `prompt.facts` patch:
/// `["set", …, "custom:prompt-facts", {"model": {"identifier": …}}]`.
fn prompt_facts_model(data: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(data).ok()?;
    value
        .get("ops")?
        .as_array()?
        .iter()
        .filter_map(Value::as_array)
        .filter(|op| op.first().and_then(Value::as_str) == Some("set"))
        .filter_map(|op| op.last())
        .find_map(|facts| facts.get("model")?.get("identifier")?.as_str())
        .map(str::to_owned)
}

/// The text of the error notice a `kernel.notice` patch inserts, if it
/// inserts one. Empty text still counts: the turn failed either way.
pub(crate) fn error_notice_text(data: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(data).ok()?;
    value
        .get("ops")?
        .as_array()?
        .iter()
        .filter_map(Value::as_array)
        .find_map(|op| {
            if op.first().and_then(Value::as_str) != Some("ins") {
                return None;
            }
            let node = op.last()?;
            if node.get("tag").and_then(Value::as_str) != Some("notice") {
                return None;
            }
            let is_error = node
                .get("props")
                .and_then(Value::as_array)
                .is_some_and(|props| {
                    props.iter().any(|prop| {
                        prop.as_array().is_some_and(|pair| {
                            pair.first().and_then(Value::as_str) == Some("kind")
                                && pair.get(1).and_then(Value::as_str) == Some("error")
                        })
                    })
                });
            is_error.then(|| {
                node.get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            })
        })
}

/// The line the replayed history shows for a turn OMP ended with an error,
/// worded as the live transcript words it and dated when the journal says
/// when. OMP's own notice indents its cause on a second line; one line reads
/// better in a transcript row.
fn turn_error_line(reason: &str, when: Option<&str>) -> String {
    let reason = reason
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let head = match when {
        Some(when) => format!("OMP ended the turn with an error ({when})"),
        None => "OMP ended the turn with an error".to_owned(),
    };
    if reason.is_empty() {
        format!("{head}.")
    } else {
        format!("{head}: {reason}")
    }
}

/// A tool call being reassembled from the several journal entries that describe
/// it before it is finalized into a single row.
struct PendingTool {
    name: String,
    message_index: usize,
    intent: Option<String>,
    /// Arguments carried inline on the call entry, when not streamed.
    inline_args: Option<String>,
    /// Arguments reassembled by the `kernel: ready` update.
    ready_args: Option<String>,
    /// Arguments accumulated from the argument stream, a fallback for a session
    /// cut off before the ready update landed.
    arg_buffer: String,
    /// Model-facing text of the tool's result.
    result: Option<String>,
    failed: bool,
}

impl PendingTool {
    /// The best available rendering of the call's arguments as a JSON string.
    fn arguments(&self) -> String {
        for candidate in [self.inline_args.as_deref(), self.ready_args.as_deref()] {
            if let Some(args) = candidate.filter(|args| !args.trim().is_empty()) {
                return args.to_owned();
            }
        }
        self.arg_buffer.clone()
    }
}

/// Overwrites a tool's placeholder row with its resolved summary, detail, and
/// status. `think` calls are the model reasoning aloud, so they render as a
/// reasoning row rather than a tool row, matching the live transcript.
fn finalize_tool(messages: &mut [SessionMessage], tool: &PendingTool) {
    let Some(slot) = messages.get_mut(tool.message_index) else {
        return;
    };
    let arguments = tool.arguments();
    if tool.name == "think" {
        let thoughts = serde_json::from_str::<Value>(&arguments)
            .ok()
            .as_ref()
            .and_then(|value| value.get("thoughts"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| tool.intent.clone())
            .unwrap_or_default();
        *slot = SessionMessage::spoken(Speaker::Thinking, thoughts);
        return;
    }
    let (summary, detail) = chat::tool_row(
        &tool.name,
        &arguments,
        &[],
        tool.result.as_deref(),
        tool.intent.as_deref(),
    );
    *slot = SessionMessage::tool(&tool.name, summary, detail, tool.failed);
}

/// Projects a journaled tool result into `(failed, model-facing text)`. Prefers
/// the durable `prompt_parts` projection; falls back to the raw outcome JSON,
/// which collapses out-of-line blob envelopes to their size.
pub(crate) fn tool_result_text(result: &data::ToolResult) -> (bool, String) {
    let (failed, raw, prompt_parts) = match result {
        data::ToolResult::Outcome {
            outcome,
            prompt_parts,
            ..
        } => (false, outcome, prompt_parts),
        data::ToolResult::Fault {
            fault,
            prompt_parts,
            ..
        } => (true, fault, prompt_parts),
    };
    let text = prompt_parts
        .as_deref()
        .and_then(|parts| prompt_parts_text(parts.get()))
        .unwrap_or_else(|| chat::normalize_json_text(raw.get()));
    (failed, text)
}

/// Concatenates the text of a tool result's `prompt_parts` array, the model's
/// own projection of its outcome. `None` when the parts carry no text.
fn prompt_parts_text(raw: &str) -> Option<String> {
    let parts = serde_json::from_str::<Vec<Value>>(raw).ok()?;
    let mut text = String::new();
    for part in &parts {
        if let Some(chunk) = part
            .get("text")
            .or_else(|| part.get("alt"))
            .and_then(Value::as_str)
        {
            text.push_str(chunk);
        }
    }
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use omp_journal::{EntryDraft, Kind, KindName};
    use serde_json::json;

    use super::*;

    fn draft(
        kind: KindName,
        data: serde_json::Value,
        by: Option<omp_journal::EntryId>,
    ) -> EntryDraft {
        EntryDraft {
            kind: Kind::known(kind),
            by,
            prior: None,
            label: None,
            data: data.to_string().into(),
        }
    }

    fn labeled(
        kind: KindName,
        label: &str,
        data: serde_json::Value,
        by: Option<omp_journal::EntryId>,
    ) -> EntryDraft {
        EntryDraft {
            kind: Kind::known(kind),
            by,
            prior: None,
            label: Some(label.to_owned().into()),
            data: data.to_string().into(),
        }
    }

    fn genesis_payload() -> serde_json::Value {
        json!({ "version": 1, "cwd": "/tmp/project", "created": "2026-09-04T00:00:00Z" })
    }

    fn prompt_facts(model: &str) -> serde_json::Value {
        json!({ "ops": [["set", 2, "custom:prompt-facts", {
            "cwd": "/tmp/project",
            "model": { "identifier": model, "codex_task_policy": false }
        }]] })
    }

    fn error_notice(text: &str) -> serde_json::Value {
        json!({ "ops": [["ins", 19, 20, {
            "tag": "notice", "props": [["kind", "error"]], "content": text
        }]] })
    }

    fn session_exit() -> serde_json::Value {
        json!({ "ops": [["set", 16, "custom:process-exited", true]] })
    }

    /// A journal whose one turn ends the way `ending` says: with a receipt,
    /// an error notice, a session exit while running, or not at all.
    fn journal_ending_with(dir: &Path, ending: &str) -> PathBuf {
        let path = dir.join(format!("01TESTJOURNAL{ending:0>13}.oms"));
        let mut journal = Journal::create(&path).expect("journal");
        let genesis = journal
            .append(draft(KindName::Journal, genesis_payload(), None))
            .expect("genesis");
        let facts = journal
            .append(labeled(
                KindName::Patch,
                "prompt.facts",
                prompt_facts("openai-codex/gpt-6-astra"),
                Some(genesis.id),
            ))
            .expect("facts");
        let turn = journal
            .append(draft(KindName::TurnStart, json!({}), Some(facts.id)))
            .expect("turn");
        let user = journal
            .append(draft(
                KindName::MsgUser,
                json!({ "text": "Make some changes" }),
                Some(turn.id),
            ))
            .expect("user");
        match ending {
            "receipt" => {
                let spawn = journal
                    .append(labeled(
                        KindName::Patch,
                        "jobs.insert",
                        json!({ "ops": [["ins", 5, null, {
                            "tag": "subagent",
                            "props": [["id", "scout"], ["kind", "subagent"], ["status", "running"],
                                      ["custom:owner", "01TESTJOURNAL"], ["name", "task"],
                                      ["label", "scout"], ["custom:agent", "scout"]]
                        }]] }),
                        Some(user.id),
                    ))
                    .expect("spawn");
                journal
                    .append(draft(
                        KindName::TurnReceipt,
                        json!({ "tokens_in": 1, "tokens_out": 1 }),
                        Some(spawn.id),
                    ))
                    .expect("receipt");
            }
            "error" => {
                let notice = journal
                    .append(labeled(
                        KindName::Patch,
                        "kernel.notice",
                        error_notice("inference Authentication error during Authentication"),
                        Some(user.id),
                    ))
                    .expect("notice");
                journal
                    .append(labeled(
                        KindName::Patch,
                        "session.exit",
                        session_exit(),
                        Some(notice.id),
                    ))
                    .expect("exit");
            }
            "exit" => {
                journal
                    .append(labeled(
                        KindName::Patch,
                        "session.exit",
                        session_exit(),
                        Some(user.id),
                    ))
                    .expect("exit");
            }
            _ => {}
        }
        path
    }

    #[test]
    fn a_turn_error_reads_on_one_line_with_its_cause_and_its_date() {
        assert_eq!(
            turn_error_line(
                "session tool JSON is invalid\n  caused by: data did not match any variant of untagged enum Params",
                Some("Sep 10, 2026 at 11:10 PM"),
            ),
            "OMP ended the turn with an error (Sep 10, 2026 at 11:10 PM): session tool JSON is invalid caused by: data did not match any variant of untagged enum Params"
        );
        assert_eq!(
            turn_error_line("  \n", None),
            "OMP ended the turn with an error."
        );
    }

    #[test]
    fn the_fold_reads_the_model_and_how_the_last_turn_ended() {
        let dir =
            std::env::temp_dir().join(format!("omperor-journal-outcome-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let expectations = [
            ("receipt", Some(SessionOutcome::Completed)),
            ("error", Some(SessionOutcome::Errored)),
            ("exit", Some(SessionOutcome::Interrupted)),
            ("open", Some(SessionOutcome::InProgress)),
        ];
        for (ending, outcome) in expectations {
            let path = journal_ending_with(&dir, ending);
            let transcript = read(&path).expect("readable journal");
            assert_eq!(transcript.outcome, outcome, "ending {ending}");
            assert_eq!(
                transcript.model.as_deref(),
                Some("openai-codex/gpt-6-astra"),
                "ending {ending}"
            );
            let expected_spawned: &[&str] = if ending == "receipt" { &["scout"] } else { &[] };
            assert_eq!(transcript.spawned, expected_spawned, "ending {ending}");
            // The reason a turn failed is replayed as a line of the history,
            // worded as the live transcript words it.
            let error_lines = transcript
                .messages
                .iter()
                .filter(|message| message.speaker == Speaker::Event)
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>();
            if ending == "error" {
                assert_eq!(error_lines.len(), 1, "ending {ending}: {error_lines:?}");
                let line = error_lines[0];
                assert!(
                    line.starts_with("OMP ended the turn with an error ("),
                    "{line}"
                );
                assert!(
                    line.ends_with("): inference Authentication error during Authentication"),
                    "{line}"
                );
            } else {
                assert!(error_lines.is_empty(), "ending {ending}: {error_lines:?}");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn journal_with_one_turn(dir: &Path) -> PathBuf {
        let path = dir.join("01TESTJOURNAL0000000000000.oms");
        let mut journal = Journal::create(&path).expect("journal");
        let genesis = journal
            .append(draft(KindName::Journal, genesis_payload(), None))
            .expect("genesis");
        let turn = journal
            .append(draft(KindName::TurnStart, json!({}), Some(genesis.id)))
            .expect("turn");
        let user = journal
            .append(draft(
                KindName::MsgUser,
                json!({ "text": "Reply with exactly: pong" }),
                Some(turn.id),
            ))
            .expect("user");
        let assistant = journal
            .append(draft(KindName::MsgAssistantStart, json!({}), Some(user.id)))
            .expect("assistant");
        journal
            .append(draft(
                KindName::Stream,
                json!({ "sid": 1, "op": "open", "prop": "text" }),
                Some(assistant.id),
            ))
            .expect("open");
        journal
            .append(draft(
                KindName::Stream,
                json!({ "sid": 1, "op": "append", "text": "po" }),
                Some(assistant.id),
            ))
            .expect("append");
        journal
            .append(draft(
                KindName::Stream,
                json!({ "sid": 1, "op": "append", "text": "ng" }),
                Some(assistant.id),
            ))
            .expect("append");
        journal
            .append(draft(
                KindName::Stream,
                json!({ "sid": 1, "op": "close" }),
                Some(assistant.id),
            ))
            .expect("close");
        journal
            .append(draft(
                KindName::ToolCall,
                json!({ "name": "read", "rev": 1, "call_id": "call-1", "i": "Read Cargo.toml" }),
                Some(assistant.id),
            ))
            .expect("tool");
        journal
            .append(draft(
                KindName::MsgAssistantEnd,
                json!({ "stop_reason": "stop" }),
                Some(assistant.id),
            ))
            .expect("end");
        path
    }

    #[test]
    fn folds_the_live_chain_into_messages_without_locking() {
        let dir = std::env::temp_dir().join(format!("omperor-journal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = journal_with_one_turn(&dir);

        let transcript = read(&path).expect("scan");
        assert_eq!(transcript.cwd.as_deref(), Some(Path::new("/tmp/project")));
        assert_eq!(
            transcript.first_prompt.as_deref(),
            Some("Reply with exactly: pong")
        );
        assert_eq!(
            transcript.messages,
            vec![
                SessionMessage::spoken(Speaker::User, "Reply with exactly: pong"),
                SessionMessage::spoken(Speaker::Assistant, "pong"),
                // The `read` call carried no arguments, so the model's intent
                // stands in as the summary and the modal shows it verbatim.
                SessionMessage::tool(
                    "read",
                    "Read Cargo.toml",
                    Some("Intent\nRead Cargo.toml".into()),
                    false,
                ),
            ]
        );
        assert!(transcript.has_conversation());
        assert!(has_conversation(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `assistant.content` patch OMP writes before opening a content
    /// node's stream, with the node's kind.
    fn content_node(kind: &str, index: u32) -> serde_json::Value {
        json!({ "ops": [["ins", 21, null, {
            "tag": "tool:assistant-content",
            "props": [["kind", kind], ["custom:index", index], ["text", ""]]
        }]] })
    }

    #[test]
    fn a_reasoning_summary_replays_as_its_own_row_before_the_reply() {
        let dir = std::env::temp_dir().join(format!(
            "omperor-journal-reasoning-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("01TESTJOURNALREASON0000000.oms");
        let mut journal = Journal::create(&path).expect("journal");
        let genesis = journal
            .append(draft(KindName::Journal, genesis_payload(), None))
            .expect("genesis");
        let turn = journal
            .append(draft(KindName::TurnStart, json!({}), Some(genesis.id)))
            .expect("turn");
        let user = journal
            .append(draft(
                KindName::MsgUser,
                json!({ "text": "Write a limerick" }),
                Some(turn.id),
            ))
            .expect("user");
        let assistant = journal
            .append(draft(KindName::MsgAssistantStart, json!({}), Some(user.id)))
            .expect("assistant");
        // Codex streams its reasoning summary and the reply as two content
        // nodes, both through a `text` property.
        let mut cause = assistant.id;
        for (kind, index, sid, text) in [
            ("thinking", 0, 1, "**Planning limerick structure**"),
            ("text", 1, 2, "A keeper who lived by the bay"),
        ] {
            cause = journal
                .append(labeled(
                    KindName::Patch,
                    "assistant.content",
                    content_node(kind, index),
                    Some(cause),
                ))
                .expect("content")
                .id;
            journal
                .append(draft(
                    KindName::Stream,
                    json!({ "sid": sid, "op": "open", "node": 22 + index, "prop": "text" }),
                    Some(assistant.id),
                ))
                .expect("open");
            journal
                .append(draft(
                    KindName::Stream,
                    json!({ "sid": sid, "op": "append", "text": text }),
                    Some(assistant.id),
                ))
                .expect("append");
            journal
                .append(draft(
                    KindName::Stream,
                    json!({ "sid": sid, "op": "close" }),
                    Some(assistant.id),
                ))
                .expect("close");
        }
        journal
            .append(draft(
                KindName::MsgAssistantEnd,
                json!({ "stop_reason": "stop" }),
                Some(assistant.id),
            ))
            .expect("end");

        let transcript = read(&path).expect("scan");
        assert_eq!(
            transcript.messages,
            vec![
                SessionMessage::spoken(Speaker::User, "Write a limerick"),
                SessionMessage::spoken(Speaker::Thinking, "**Planning limerick structure**"),
                SessionMessage::spoken(Speaker::Assistant, "A keeper who lived by the bay"),
            ]
        );
        let timeline = crate::timeline::read(&path).expect("timeline");
        let response = timeline.events.iter().find(|e| e.kind == crate::timeline::Kind::Response).expect("response");
        assert!(response.info.sections.iter().any(|(title, body)| title == "Reasoning" && body == "**Planning limerick structure**"));
        assert!(response.info.sections.iter().any(|(title, body)| title == "Reply" && body == "A keeper who lived by the bay"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folds_streamed_tool_calls_with_their_results_and_status() {
        let dir =
            std::env::temp_dir().join(format!("omperor-journal-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("01TESTJOURNALTOOLS00000000.oms");
        let mut journal = Journal::create(&path).expect("journal");
        let genesis = journal
            .append(draft(KindName::Journal, genesis_payload(), None))
            .expect("genesis");
        let turn = journal
            .append(draft(KindName::TurnStart, json!({}), Some(genesis.id)))
            .expect("turn");
        let user = journal
            .append(draft(
                KindName::MsgUser,
                json!({ "text": "List the files" }),
                Some(turn.id),
            ))
            .expect("user");
        let assistant = journal
            .append(draft(KindName::MsgAssistantStart, json!({}), Some(user.id)))
            .expect("assistant");
        // A bash call whose arguments were streamed: the reassembled arguments
        // arrive on the `kernel: ready` update, and it succeeds.
        let bash = journal
            .append(draft(
                KindName::ToolCall,
                json!({ "name": "bash", "rev": 1, "call_id": "call-bash", "sid": 7 }),
                Some(assistant.id),
            ))
            .expect("bash call");
        journal
            .append(draft(
                KindName::ToolUpdate,
                json!({ "kernel": "ready", "args": { "command": "ls -la" }, "i": "List files" }),
                Some(bash.id),
            ))
            .expect("bash ready");
        journal
            .append(draft(
                KindName::ToolResult,
                json!({
                    "outcome": { "kind": "call_outcome", "value": "a.txt\nb.txt" },
                    "prompt_parts": [{ "kind": "text", "text": "a.txt\nb.txt" }],
                }),
                Some(bash.id),
            ))
            .expect("bash result");
        // A read call that faults; its status must surface as a failure.
        let read_call = journal
            .append(draft(
                KindName::ToolCall,
                json!({ "name": "read", "rev": 2, "call_id": "call-read", "args": { "path": "missing.txt" } }),
                Some(assistant.id),
            ))
            .expect("read call");
        journal
            .append(draft(
                KindName::ToolResult,
                json!({
                    "fault": { "kind": "faulted", "value": { "message": "missing.txt not found" } },
                    "prompt_parts": [{ "kind": "text", "text": "missing.txt not found" }],
                }),
                Some(read_call.id),
            ))
            .expect("read result");
        journal
            .append(draft(
                KindName::MsgAssistantEnd,
                json!({ "stop_reason": "stop" }),
                Some(assistant.id),
            ))
            .expect("end");

        let transcript = read(&path).expect("scan");
        // The empty assistant turn is dropped; only the prompt and two tool rows
        // remain.
        assert_eq!(transcript.messages.len(), 3);
        assert_eq!(
            transcript.messages[0],
            SessionMessage::spoken(Speaker::User, "List the files")
        );

        let bash_row = &transcript.messages[1];
        assert_eq!(bash_row.speaker, Speaker::Tool);
        assert_eq!(bash_row.tool.as_deref(), Some("bash"));
        assert_eq!(bash_row.text, "Ran ls -la");
        assert!(!bash_row.failed);
        let bash_detail = bash_row.detail.as_deref().expect("bash detail");
        assert!(bash_detail.contains("ls -la"), "arguments in detail");
        assert!(bash_detail.contains("a.txt"), "result in detail");
        assert!(bash_detail.contains("List files"), "intent in detail");

        let read_row = &transcript.messages[2];
        assert_eq!(read_row.speaker, Speaker::Tool);
        assert_eq!(read_row.text, "Read missing.txt");
        assert!(read_row.failed, "a faulted call reads as failed");
        assert!(
            read_row
                .detail
                .as_deref()
                .expect("read detail")
                .contains("missing.txt not found")
        );

        assert!(transcript.has_conversation());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_opened_but_unused_journal_has_no_conversation() {
        let dir =
            std::env::temp_dir().join(format!("omperor-journal-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("01TESTJOURNAL0000000000001.oms");
        let mut journal = Journal::create(&path).expect("journal");
        journal
            .append(draft(KindName::Journal, genesis_payload(), None))
            .expect("genesis");
        assert!(!has_conversation(&path));
        assert!(is_journal(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
