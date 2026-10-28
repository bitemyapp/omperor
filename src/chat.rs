use serde_json::{Map, Value};

use crate::session::{SessionMessage, Speaker};

pub(crate) const WELCOME: &str = "## What would you like to work on?\n\nExplore a codebase, work through a problem, or turn an idea into a change.\n\nChoose a project in the sidebar, then write a message below. Type **@** to bring a file into the conversation.";

const MAX_DETAIL_BYTES: usize = 8 * 1024;
const MAX_JSON_ARRAY_ITEMS: usize = 32;

/// Lifecycle state of a tool call, shown as a colored status pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToolStatus {
    Running,
    Completed,
    Failed,
}

impl ToolStatus {
    /// The status word, used by the test-only transcript renderer.
    #[cfg(test)]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    /// The model's streamed chain-of-thought.
    Thinking,
    /// A tool invocation and its result.
    Tool,
    /// A folded activity line replayed from a durable session.
    Event,
    /// A run summary or other non-actionable note.
    Notice,
    /// The `/usage` card.
    Usage,
}

impl Role {
    fn kind(self) -> TranscriptMessageKind {
        match self {
            Self::User => TranscriptMessageKind::User,
            Self::Assistant => TranscriptMessageKind::Assistant,
            Self::Thinking => TranscriptMessageKind::Thinking,
            Self::Tool => TranscriptMessageKind::Tool,
            Self::Event => TranscriptMessageKind::Event,
            Self::Notice => TranscriptMessageKind::Notice,
            Self::Usage => TranscriptMessageKind::Usage,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TranscriptMessageKind {
    User,
    Assistant,
    Thinking,
    Tool,
    Event,
    Notice,
    /// The `/usage` card: made by the app, never sent or saved; `body` is
    /// the card's data (`usage_card`).
    Usage,
}

/// One rendered transcript row. `body` is what appears inline (markdown for
/// chat, a compact one-liner for activity rows); `detail_id` addresses the
/// full payload shown in a modal when the row is clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptMessage {
    pub(crate) author: String,
    pub(crate) body: String,
    pub(crate) kind: TranscriptMessageKind,
    pub(crate) status: Option<ToolStatus>,
    pub(crate) detail_id: Option<usize>,
}

/// The full detail of one activity row, shown in a modal on click, split into
/// labeled sections so the modal can style each part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailView {
    pub(crate) title: String,
    pub(crate) sections: Vec<DetailSection>,
}

/// One labeled block of a detail modal: the model's intent, the call arguments,
/// its streamed updates, or its result. Reasoning has a single unlabeled block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailSection {
    /// The section label (`Arguments`, `Result`, …), or `None` for prose.
    pub(crate) heading: Option<String>,
    pub(crate) body: String,
    /// Whether the body is code-like (JSON, command output) and reads best
    /// monospaced, versus prose that reads best in the system font.
    pub(crate) code: bool,
}

/// Section labels `tool_detail` emits, recognized when re-parsing a stored
/// detail back into its sections. `Intent` is prose; the rest are code-like.
const DETAIL_HEADINGS: [&str; 4] = ["Intent", "Arguments", "Updates", "Result"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    role: Role,
    /// A short label for activity rows, e.g. the tool name.
    label: Option<String>,
    /// Inline text: markdown for chat, a compact summary for activity rows.
    text: String,
    /// Full payload shown in the detail modal; `None` when there is nothing
    /// beyond the inline text worth expanding.
    detail: Option<String>,
    status: Option<ToolStatus>,
}

impl Message {
    fn chat(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            label: None,
            text: text.into(),
            detail: None,
            status: None,
        }
    }

    fn notice(text: impl Into<String>, status: Option<ToolStatus>) -> Self {
        Self {
            role: Role::Notice,
            label: Some("turn".into()),
            text: text.into(),
            detail: None,
            status,
        }
    }

    fn event(text: impl Into<String>) -> Self {
        let text = text.into();
        // A replayed activity line is already compact; keep the full text
        // available so a click can show it untruncated.
        let detail = (text.len() > 200).then(|| text.clone());
        Self {
            role: Role::Event,
            label: Some("event".into()),
            text,
            detail,
            status: None,
        }
    }

    fn author(&self) -> String {
        match self.role {
            Role::User => "You".to_owned(),
            Role::Assistant => "omp".to_owned(),
            Role::Thinking => "omp · thinking".to_owned(),
            Role::Tool => format!("omp · {}", self.label.as_deref().unwrap_or("tool")),
            Role::Event => "omp · event".to_owned(),
            Role::Notice => format!("omp · {}", self.label.as_deref().unwrap_or("note")),
            Role::Usage => "Omperor · usage".to_owned(),
        }
    }
}

#[derive(Debug)]
struct ToolActivity {
    call_id: String,
    name: String,
    arguments: String,
    /// The model's stated intent for the call, shown in the detail modal.
    intent: Option<String>,
    updates: Vec<String>,
    result: Option<String>,
    finished: bool,
    failed: bool,
    message_index: usize,
}

#[derive(Debug)]
pub(crate) struct Conversation {
    messages: Vec<Message>,
    placeholder: bool,
    streaming: Option<usize>,
    thinking: Option<usize>,
    tools: Vec<ToolActivity>,
    /// What changed since the transcript last read the conversation, so a
    /// refresh touches only that rather than every message every time.
    changes: Changes,
}

/// The messages edited since the transcript last asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Changes {
    /// Everything: the conversation was replaced or shifted.
    All,
    /// These messages were edited in place or added at these indices; a
    /// change of length says the rest on its own.
    Messages(Vec<usize>),
    /// This many messages of history were put above the rest, and nothing
    /// else changed.
    Prepended(usize),
}

impl Default for Conversation {
    fn default() -> Self {
        Self {
            messages: vec![Message::chat(Role::Assistant, WELCOME)],
            placeholder: true,
            streaming: None,
            thinking: None,
            tools: Vec::new(),
            changes: Changes::All,
        }
    }
}

impl Conversation {
    /// Hands over what changed since the last call, and starts afresh.
    pub(crate) fn take_changes(&mut self) -> Changes {
        std::mem::replace(&mut self.changes, Changes::Messages(Vec::new()))
    }

    pub(crate) fn len(&self) -> usize {
        self.messages.len()
    }

    /// The assistant message being streamed into, if any.
    pub(crate) fn streaming_index(&self) -> Option<usize> {
        self.streaming
    }

    fn touch(&mut self, index: usize) {
        match &mut self.changes {
            Changes::Messages(indices) if !indices.contains(&index) => indices.push(index),
            // History put above, then an edit: the transcript reads it all.
            Changes::Prepended(_) => self.changes = Changes::All,
            _ => {}
        }
    }

    fn touch_last(&mut self) {
        if let Some(index) = self.messages.len().checked_sub(1) {
            self.touch(index);
        }
    }

    pub(crate) fn reset(&mut self, message: impl Into<String>) {
        self.changes = Changes::All;
        self.messages = vec![Message::chat(Role::Assistant, message)];
        self.placeholder = true;
        self.streaming = None;
        self.thinking = None;
        self.tools.clear();
    }

    pub(crate) fn load_history(&mut self, history: &[SessionMessage]) {
        self.changes = Changes::All;
        self.messages = history.iter().map(message_from_session).collect();
        self.placeholder = self.messages.is_empty();
        if self.messages.is_empty() {
            self.messages.push(Message::chat(
                Role::Assistant,
                "This session has no messages to show yet.",
            ));
        }
        self.streaming = None;
        self.thinking = None;
        self.tools.clear();
    }

    pub(crate) fn prepend_history(&mut self, history: &[SessionMessage]) {
        if history.is_empty() {
            return;
        }
        if self.placeholder {
            self.messages.clear();
            self.placeholder = false;
            self.changes = Changes::All;
        }
        self.changes = match std::mem::replace(&mut self.changes, Changes::All) {
            Changes::Messages(indices) if indices.is_empty() && self.streaming.is_none() => Changes::Prepended(history.len()),
            Changes::Prepended(count) => Changes::Prepended(count + history.len()),
            _ => Changes::All,
        };
        self.messages
            .splice(0..0, history.iter().map(message_from_session));
        self.streaming = None;
        self.thinking = None;
        self.tools.clear();
    }

    pub(crate) fn begin_turn(&mut self) {
        self.close_assistant_segment();
        self.thinking = None;
    }

    pub(crate) fn submit_user(&mut self, input: &str) -> bool {
        let input = input.trim();
        if input.is_empty() {
            return false;
        }
        if self.placeholder {
            self.messages.clear();
            self.placeholder = false;
            self.changes = Changes::All;
        }
        self.messages.push(Message::chat(Role::User, input));
        self.touch_last();
        true
    }

    pub(crate) fn steer_user(&mut self, input: &str) -> bool {
        let input = input.trim();
        if input.is_empty() {
            return false;
        }
        self.close_assistant_segment();
        self.messages.push(Message::chat(Role::User, input));
        self.touch_last();
        true
    }

    pub(crate) fn start_assistant(&mut self) {
        if self.streaming.is_some() {
            return;
        }
        self.thinking = None;
        self.messages
            .push(Message::chat(Role::Assistant, String::new()));
        self.streaming = Some(self.messages.len() - 1);
        self.touch_last();
    }

    pub(crate) fn append_assistant(&mut self, delta: &str) {
        self.start_assistant();
        if let Some(index) = self.streaming
            && let Some(message) = self.messages.get_mut(index)
        {
            message.text.push_str(delta);
            self.touch(index);
        }
    }

    pub(crate) fn finish_assistant(&mut self, final_text: Option<&str>) {
        match self.streaming.take() {
            Some(index)
                if self
                    .messages
                    .get(index)
                    .is_some_and(|message| message.text.is_empty()) =>
            {
                if let Some(final_text) = final_text.filter(|text| !text.is_empty()) {
                    if let Some(message) = self.messages.get_mut(index) {
                        message.text = final_text.into();
                        self.touch(index);
                    }
                } else if index + 1 == self.messages.len() {
                    self.messages.pop();
                }
            }
            Some(_) => {}
            None => {
                if let Some(final_text) = final_text.filter(|text| !text.is_empty()) {
                    self.messages
                        .push(Message::chat(Role::Assistant, final_text));
                    self.touch_last();
                }
            }
        }
    }

    pub(crate) fn append_thinking(&mut self, delta: &str) {
        self.close_assistant_segment();
        let index = *self.thinking.get_or_insert_with(|| {
            self.messages
                .push(Message::chat(Role::Thinking, String::new()));
            self.messages.len() - 1
        });
        if let Some(message) = self.messages.get_mut(index) {
            message.text.push_str(delta);
            truncate_in_place(&mut message.text);
            self.touch(index);
        }
    }

    pub(crate) fn start_tool(&mut self, call_id: &str, name: &str) {
        self.close_assistant_segment();
        self.thinking = None;
        if self.tools.iter().any(|tool| tool.call_id == call_id) {
            return;
        }

        self.messages.push(Message {
            role: Role::Tool,
            label: Some(tool_label(name)),
            text: String::new(),
            detail: None,
            status: Some(ToolStatus::Running),
        });
        self.touch_last();
        self.tools.push(ToolActivity {
            call_id: call_id.into(),
            name: name.into(),
            arguments: String::new(),
            intent: None,
            updates: Vec::new(),
            result: None,
            finished: false,
            failed: false,
            message_index: self.messages.len() - 1,
        });
    }

    /// Records the full arguments and stated intent OMP reports for a call up
    /// front (RPC mode delivers both on `tool_execution_start`, never as
    /// streamed deltas). Empty values are ignored so a later, fuller report is
    /// not overwritten.
    pub(crate) fn set_tool_context(
        &mut self,
        call_id: &str,
        arguments: Option<&str>,
        intent: Option<&str>,
    ) {
        self.ensure_tool(call_id, "tool");
        if let Some(tool) = self.tools.iter_mut().find(|tool| tool.call_id == call_id) {
            if let Some(arguments) = arguments.filter(|arguments| !arguments.trim().is_empty()) {
                tool.arguments = arguments.to_owned();
                truncate_in_place(&mut tool.arguments);
            }
            if let Some(intent) = intent
                .map(str::trim)
                .filter(|intent| !intent.is_empty())
                .filter(|_| tool.intent.is_none())
            {
                tool.intent = Some(intent.to_owned());
            }
        }
        self.refresh_tool(call_id);
    }

    pub(crate) fn append_tool_arguments(&mut self, call_id: &str, delta: &str) {
        self.ensure_tool(call_id, "tool");
        if let Some(tool) = self.tools.iter_mut().find(|tool| tool.call_id == call_id) {
            tool.arguments.push_str(delta);
            truncate_in_place(&mut tool.arguments);
        }
        self.refresh_tool(call_id);
    }

    pub(crate) fn append_tool_update(&mut self, call_id: &str, content: &str) {
        self.ensure_tool(call_id, "tool");
        if let Some(tool) = self.tools.iter_mut().find(|tool| tool.call_id == call_id) {
            let update = normalize_json_text(content);
            if !update.is_empty() && tool.updates.last() != Some(&update) {
                tool.updates.push(update);
                if tool.updates.len() > 8 {
                    tool.updates.remove(0);
                }
            }
        }
        self.refresh_tool(call_id);
    }

    pub(crate) fn finish_tool(
        &mut self,
        call_id: &str,
        name: Option<&str>,
        is_error: Option<bool>,
        result: Option<&Value>,
    ) {
        self.ensure_tool(call_id, name.unwrap_or("tool"));
        if let Some(tool) = self.tools.iter_mut().find(|tool| tool.call_id == call_id) {
            if let Some(name) = name.filter(|name| !name.is_empty()) {
                tool.name = name.into();
            }
            tool.finished = true;
            tool.failed = is_error.unwrap_or(tool.failed);
            if let Some(result) = result.and_then(tool_result_text) {
                tool.result = Some(result);
            }
        }
        self.refresh_tool(call_id);
    }

    /// Closes out a turn. A failed tool call along the way is not surfaced here
    /// — the model routinely retries them, and each failed step already shows a
    /// red status — so a completed turn adds no note. Whether the turn itself
    /// ended abnormally is reported separately through [`note_turn_end`].
    pub(crate) fn finish_turn(&mut self) {
        self.close_assistant_segment();
        self.thinking = None;
    }

    /// Adds a turn-level note only when OMP reports the turn ended abnormally:
    /// an error (with OMP's reason) or an interruption. A normal completion
    /// leaves no note.
    pub(crate) fn note_turn_end(&mut self, failed: bool, reason: Option<&str>) {
        let reason = reason.map(str::trim).filter(|reason| !reason.is_empty());
        let text = if failed {
            match reason {
                Some(reason) => format!("The turn ended with an error: {reason}"),
                None => "The turn ended with an error.".to_owned(),
            }
        } else {
            match reason {
                Some(reason) => format!("Turn interrupted: {reason}"),
                None => "Stopped before the turn finished.".to_owned(),
            }
        };
        let status = failed.then_some(ToolStatus::Failed);
        self.messages.push(Message::notice(text, status));
        self.touch_last();
    }

    /// A standalone notice in the transcript, marked as a failure when it is
    /// one: what the app itself has to say about the session, as opposed to
    /// what OMP said during a turn.
    pub(crate) fn notice(&mut self, text: impl Into<String>, failed: bool) {
        self.messages.push(Message::notice(
            text.into(),
            failed.then_some(ToolStatus::Failed),
        ));
        self.touch_last();
    }

    /// Appends a `/usage` card; returns its index for `set_usage`. It lives
    /// only in this conversation: nothing journals it.
    pub(crate) fn push_usage(&mut self, body: impl Into<String>) -> usize {
        self.messages.push(Message {
            role: Role::Usage,
            label: Some("usage".into()),
            text: body.into(),
            detail: None,
            status: None,
        });
        self.touch_last();
        self.messages.len() - 1
    }

    /// Fills in the card at `index` once its figures are read; false when
    /// that row is no longer a card (the conversation was replaced).
    pub(crate) fn set_usage(&mut self, index: usize, body: impl Into<String>) -> bool {
        let Some(message) = self.messages.get_mut(index).filter(|m| m.role == Role::Usage) else {
            return false;
        };
        message.text = body.into();
        self.touch(index);
        true
    }

    pub(crate) fn fail_assistant(&mut self, error: &str) {
        self.start_assistant();
        if let Some(index) = self.streaming.take() {
            let Some(message) = self.messages.get_mut(index) else {
                return;
            };
            if !message.text.is_empty() {
                message.text.push_str("\n\n");
            }
            message.text.push_str("Error: ");
            message.text.push_str(error);
            self.touch(index);
        }
    }

    #[cfg(test)]
    pub(crate) fn messages(&self) -> impl ExactSizeIterator<Item = TranscriptMessage> + '_ {
        (0..self.messages.len()).map(|index| self.transcript_message(index))
    }

    /// The message at `index` as the transcript shows it.
    pub(crate) fn message(&self, index: usize) -> Option<TranscriptMessage> {
        (index < self.messages.len()).then(|| self.transcript_message(index))
    }

    fn transcript_message(&self, index: usize) -> TranscriptMessage {
        let message = &self.messages[index];
        let kind = message.role.kind();
        let detail_id = match message.role {
            Role::Thinking | Role::Tool => Some(index),
            Role::Event => message.detail.is_some().then_some(index),
            Role::User | Role::Assistant | Role::Notice | Role::Usage => None,
        };
        TranscriptMessage {
            author: message.author(),
            body: message.text.clone(),
            kind,
            status: message.status,
            detail_id,
        }
    }

    /// The full payload for the activity row at `index`, for the detail modal.
    pub(crate) fn detail(&self, index: usize) -> Option<DetailView> {
        let message = self.messages.get(index)?;
        if matches!(message.role, Role::User | Role::Assistant) {
            return Some(DetailView {
                title: "Message Markdown".into(),
                sections: vec![DetailSection {
                    heading: None,
                    body: message.text.clone(),
                    code: true,
                }],
            });
        }
        let title = match message.role {
            Role::Thinking => "Reasoning".to_owned(),
            Role::Tool => format!("Tool · {}", message.label.as_deref().unwrap_or("tool")),
            Role::Event => "Activity".to_owned(),
            Role::User | Role::Assistant | Role::Notice | Role::Usage => return None,
        };
        let body = message
            .detail
            .clone()
            .unwrap_or_else(|| message.text.clone());
        Some(DetailView {
            title,
            sections: detail_sections(&body, message.role),
        })
    }

    #[cfg(test)]
    pub(crate) fn render(&self) -> String {
        self.messages
            .iter()
            .map(|message| {
                let status = message
                    .status
                    .map(|status| format!(" [{}]", status.label()))
                    .unwrap_or_default();
                format!("{}{status}\n{}", message.author(), message.text)
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn close_assistant_segment(&mut self) {
        let Some(index) = self.streaming.take() else {
            return;
        };
        if self
            .messages
            .get(index)
            .is_some_and(|message| message.text.is_empty())
            && index + 1 == self.messages.len()
        {
            self.messages.pop();
        }
    }

    fn ensure_tool(&mut self, call_id: &str, name: &str) {
        if !self.tools.iter().any(|tool| tool.call_id == call_id) {
            self.start_tool(call_id, name);
        }
    }

    fn refresh_tool(&mut self, call_id: &str) {
        let Some(tool) = self.tools.iter().find(|tool| tool.call_id == call_id) else {
            return;
        };
        let status = if !tool.finished {
            ToolStatus::Running
        } else if tool.failed {
            ToolStatus::Failed
        } else {
            ToolStatus::Completed
        };

        // Inline summary and detail-modal body, built the same way a resumed
        // session builds them so the live and reopened transcripts read alike.
        let (summary, detail) = tool_row(
            &tool.name,
            &tool.arguments,
            &tool.updates,
            tool.result.as_deref(),
            tool.intent.as_deref(),
        );

        let index = tool.message_index;
        if let Some(message) = self.messages.get_mut(index) {
            message.label = Some(tool_label(&tool.name));
            message.text = summary;
            message.detail = detail;
            message.status = Some(status);
            self.touch(index);
        }
    }
}

fn message_from_session(message: &SessionMessage) -> Message {
    match message.speaker {
        Speaker::User => Message::chat(Role::User, &message.text),
        Speaker::Assistant => Message::chat(Role::Assistant, &message.text),
        Speaker::Thinking => Message::chat(Role::Thinking, &message.text),
        Speaker::Event => {
            let mut event = Message::event(&message.text);
            if message.detail.is_some() {
                event.detail = message.detail.clone();
            }
            event
        }
        // A resumed tool call arrives fully resolved: its status is settled and
        // its full payload is already folded, so it renders like a live row.
        Speaker::Tool => Message {
            role: Role::Tool,
            label: Some(tool_label(message.tool.as_deref().unwrap_or("tool"))),
            text: message.text.clone(),
            detail: message.detail.clone(),
            status: Some(if message.failed {
                ToolStatus::Failed
            } else {
                ToolStatus::Completed
            }),
        },
    }
}

/// The display label for a tool row: `think` presents as "thinking".
fn tool_label(name: &str) -> String {
    if name == "think" {
        "thinking".to_owned()
    } else {
        name.to_owned()
    }
}

/// A one-line, past-tense description of what a tool call did, derived from its
/// arguments — e.g. `Read src/main.rs`, `Ran cargo test`, `Searched TODO`. Used
/// by both the live transcript and resumed sessions so the two read alike.
pub(crate) fn describe_tool(name: &str, arguments: &str) -> String {
    let value = serde_json::from_str::<Value>(arguments).ok();
    // `think` is the model's reasoning; its summary is the thought itself.
    if name == "think"
        && let Some(thoughts) = value
            .as_ref()
            .and_then(|value| value.get("thoughts"))
            .and_then(Value::as_str)
    {
        return truncate_summary(thoughts);
    }
    // `hub` is the parent's desk for background work: waiting on jobs and
    // sub-agents, and running named processes. Its `op` says what happened.
    if name == "hub"
        && let Some(line) = describe_hub(value.as_ref())
    {
        return truncate_summary(&line);
    }
    // A batch `task` call names every child it spawned.
    if name == "task"
        && let Some(line) = describe_task_batch(value.as_ref())
    {
        return truncate_summary(&line);
    }
    if let Some(line) = describe_omperor(name, value.as_ref()) {
        return line;
    }
    let object = tool_object(name, value.as_ref());
    match (tool_verb(name), object) {
        (Some(verb), Some(object)) => truncate_summary(&format!("{verb} {object}")),
        (Some(verb), None) => verb.to_owned(),
        (None, Some(object)) => truncate_summary(&format!("{name} {object}")),
        (None, None) => name.to_owned(),
    }
}

/// What an `omperor_` tool call did to Omperor's own workspace.
fn describe_omperor(name: &str, value: Option<&Value>) -> Option<String> {
    let sessions = value
        .and_then(|value| value.get("session_ids"))
        .and_then(Value::as_array)
        .map(Vec::len);
    Some(match name {
        "omperor_list_projects" => "Listed Omperor's projects".to_owned(),
        "omperor_list_sessions" => "Listed Omperor's sessions".to_owned(),
        "omperor_list_archives" => "Listed Omperor's archives".to_owned(),
        "omperor_delete_sessions" => match sessions {
            Some(1) => "Deleted an Omperor session".to_owned(),
            Some(count) => format!("Deleted {count} Omperor sessions"),
            None => "Deleted Omperor sessions".to_owned(),
        },
        "omperor_archive_project" => "Archived an Omperor project".to_owned(),
        "omperor_restore_archive" => "Restored an Omperor project".to_owned(),
        "omperor_delete_archive" => "Deleted an Omperor archive".to_owned(),
        "omperor_remove_empty_sessions" => "Removed empty Omperor sessions".to_owned(),
        _ => return None,
    })
}

/// What a `hub` call did, from its `op` and whichever of `ids`, `name`, or
/// `to` it addressed: "Waited for job-3", "Started dev-server", "Checked
/// background jobs".
fn describe_hub(value: Option<&Value>) -> Option<String> {
    let fields = value?.as_object()?;
    let op = fields.get("op").and_then(Value::as_str)?;
    let name = fields
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty());
    let peer = fields
        .get("to")
        .and_then(Value::as_str)
        .filter(|peer| !peer.trim().is_empty());
    let ids = fields
        .get("ids")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|ids| !ids.is_empty());
    let target = name.or(peer).map(str::to_owned).or(ids);
    Some(match (op, target) {
        ("wait", Some(target)) => format!("Waited for {target}"),
        ("wait", None) => "Waited for background work".to_owned(),
        ("jobs", _) => "Checked background jobs".to_owned(),
        ("cancel", Some(target)) => format!("Cancelled {target}"),
        ("cancel", None) => "Cancelled background jobs".to_owned(),
        ("send", Some(target)) => format!("Sent a message to {target}"),
        ("send", None) => "Sent a message".to_owned(),
        ("inbox", _) => "Checked the inbox".to_owned(),
        ("list", _) => "Listed peer agents".to_owned(),
        ("ps", _) => "Listed named processes".to_owned(),
        ("start", Some(target)) => format!("Started {target}"),
        ("stop", Some(target)) => format!("Stopped {target}"),
        ("restart", Some(target)) => format!("Restarted {target}"),
        ("logs", Some(target)) => format!("Read the logs of {target}"),
        ("describe", Some(target)) => format!("Inspected {target}"),
        (op, Some(target)) => format!("hub {op} {target}"),
        (op, None) => format!("hub {op}"),
    })
}

/// "Delegated scout, reviewer" for a batch `task` call, or `None` for the
/// flat single-child form, which `tool_object` already names.
fn describe_task_batch(value: Option<&Value>) -> Option<String> {
    let tasks = value?.as_object()?.get("tasks")?.as_array()?;
    let names = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| {
            task.get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map_or_else(|| format!("sub-agent {}", index + 1), str::to_owned)
        })
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Some("Delegated".to_owned());
    }
    Some(format!("Delegated {}", names.join(", ")))
}

/// The past-tense verb that reads best for a known tool, or `None` for a tool
/// we describe generically as `<name> <argument>`.
fn tool_verb(name: &str) -> Option<&'static str> {
    Some(match name {
        "read" => "Read",
        "write" => "Wrote",
        "edit" | "ast_edit" | "memory_edit" => "Edited",
        "bash" => "Ran",
        "grep" | "ast_grep" => "Searched",
        "glob" => "Listed",
        "web_search" => "Searched the web for",
        "todo" => "Updated the plan",
        "task" => "Delegated",
        "recall" => "Recalled",
        "retain" | "reflect" => "Saved a memory",
        "read_url" | "url" | "open" | "fetch" => "Fetched",
        "github" => "Queried GitHub for",
        "lsp" => "Queried the language server",
        _ => return None,
    })
}

/// Extracts the single most identifying argument of a tool call — the file it
/// touched, the command it ran, the pattern it searched — as the object of the
/// summary verb.
fn tool_object(name: &str, value: Option<&Value>) -> Option<String> {
    let object = value.and_then(Value::as_object);
    // `edit`'s payload is a section DSL whose first `[path#…]` header names the
    // file; `ast_edit` addresses files by an array of `{path}` entries.
    match name {
        "edit" => return edit_path(object).map(readable_path),
        "ast_edit" => {
            return object
                .and_then(|fields| fields.get("files"))
                .and_then(Value::as_array)
                .and_then(|files| files.first())
                .and_then(Value::as_object)
                .and_then(|file| file.get("path"))
                .and_then(Value::as_str)
                .map(readable_path);
        }
        _ => {}
    }
    let keys: &[&str] = match name {
        "read" | "write" | "glob" => &["path"],
        "grep" => &["pattern"],
        "ast_grep" => &["pat"],
        "bash" => &["command", "cmd"],
        "web_search" | "recall" => &["query"],
        "read_url" | "url" | "open" | "fetch" => &["url"],
        _ => &[
            "command",
            "cmd",
            "path",
            "file_path",
            "file",
            "pattern",
            "query",
            "glob",
            "url",
            "prompt",
            "selector",
            "name",
            "pat",
        ],
    };
    let fields = object?;
    for key in keys {
        if let Some(text) = fields.get(*key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return Some(readable_path(text));
        }
    }
    for key in ["paths", "files", "targets"] {
        if let Some(items) = fields.get(key).and_then(Value::as_array) {
            let joined = items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            if !joined.trim().is_empty() {
                return Some(joined);
            }
        }
    }
    None
}

/// Pulls the file path out of the first `[path#anchor]` header of an `edit`
/// section DSL.
fn edit_path(object: Option<&Map<String, Value>>) -> Option<&str> {
    let input = object?.get("input").and_then(Value::as_str)?;
    let start = input.find('[')? + 1;
    let rest = &input[start..];
    let end = rest.find(['#', ']'])?;
    let path = rest[..end].trim();
    (!path.is_empty()).then_some(path)
}

/// Presents a workspace-relative path readably: the workspace root reads as a
/// word rather than a bare `.`.
fn readable_path(path: &str) -> String {
    match path.trim() {
        "." | "./" | "" => "the workspace".to_owned(),
        other => other.to_owned(),
    }
}

/// Collapses `text` to its first line, bounded for a single transcript row.
fn truncate_summary(text: &str) -> String {
    const MAX_CHARS: usize = 160;
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let first = first.trim();
    if first.chars().count() <= MAX_CHARS {
        return first.to_owned();
    }
    let mut summary = first.chars().take(MAX_CHARS).collect::<String>();
    summary.push('…');
    summary
}

/// Re-splits a stored detail body into the sections `tool_detail` built it
/// from, so the modal can style each. Reasoning has no headings and stays one
/// prose block; a chunk that opens with no known heading continues the section
/// before it (the payload itself held a blank line).
fn detail_sections(body: &str, role: Role) -> Vec<DetailSection> {
    if role != Role::Tool {
        return vec![DetailSection {
            heading: None,
            body: body.trim_end().to_owned(),
            code: false,
        }];
    }
    let mut sections: Vec<DetailSection> = Vec::new();
    for chunk in body.split("\n\n") {
        let (first, rest) = chunk.split_once('\n').unwrap_or((chunk, ""));
        if DETAIL_HEADINGS.contains(&first) {
            sections.push(DetailSection {
                heading: Some(first.to_owned()),
                body: rest.to_owned(),
                code: first != "Intent",
            });
        } else if let Some(last) = sections.last_mut() {
            last.body.push_str("\n\n");
            last.body.push_str(chunk);
        } else {
            sections.push(DetailSection {
                heading: None,
                body: chunk.to_owned(),
                code: false,
            });
        }
    }
    for section in &mut sections {
        let trimmed = section.body.trim_end().len();
        section.body.truncate(trimmed);
    }
    if sections.is_empty() {
        sections.push(DetailSection {
            heading: None,
            body: body.trim_end().to_owned(),
            code: false,
        });
    }
    sections
}

/// Builds a tool row's inline summary and detail-modal body from everything
/// known about the call. Shared by the live transcript and resumed sessions so
/// the two read identically: the summary is the past-tense action, or the
/// model's intent when no arguments were captured; the detail opens with the
/// intent (when present) above the arguments, updates, and result.
pub(crate) fn tool_row(
    name: &str,
    arguments: &str,
    updates: &[String],
    result: Option<&str>,
    intent: Option<&str>,
) -> (String, Option<String>) {
    let intent = intent.map(str::trim).filter(|intent| !intent.is_empty());
    let summary = if arguments.trim().is_empty() {
        intent
            .map(str::to_owned)
            .unwrap_or_else(|| describe_tool(name, arguments))
    } else {
        describe_tool(name, arguments)
    };
    let mut detail = tool_detail(name, arguments, updates, result);
    if let Some(intent) = intent {
        let heading = format!("Intent\n{intent}");
        detail = Some(match detail {
            Some(body) => format!("{heading}\n\n{body}"),
            None => heading,
        });
    }
    (summary, detail)
}

/// Builds the full detail-modal body for a tool row: its arguments, any
/// streamed updates, and its result, each under a heading. `None` when there is
/// nothing beyond the summary worth expanding. Shared by the live transcript
/// and resumed sessions so both expand to the same shape.
pub(crate) fn tool_detail(
    name: &str,
    arguments: &str,
    updates: &[String],
    result: Option<&str>,
) -> Option<String> {
    let mut sections = Vec::new();
    if !arguments.trim().is_empty() {
        sections.push(format_tool_arguments(name, arguments));
    }
    if !updates.is_empty() {
        sections.push(format!("Updates\n{}", updates.join("\n\n")));
    }
    if let Some(result) = result.filter(|result| !result.trim().is_empty()) {
        sections.push(format!("Result\n{result}"));
    }
    (!sections.is_empty()).then(|| truncate(&sections.join("\n\n")))
}

fn format_tool_arguments(name: &str, arguments: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(arguments) else {
        return format!("Arguments\n{}", truncate(arguments));
    };
    if name == "think"
        && let Some(thoughts) = value.get("thoughts").and_then(Value::as_str)
    {
        return truncate(thoughts);
    }
    format!("Arguments\n{}", pretty_value(&value))
}

pub(crate) fn normalize_json_text(content: &str) -> String {
    serde_json::from_str::<Value>(content)
        .map_or_else(|_| truncate(content), |value| pretty_value(&value))
}

fn tool_result_text(result: &Value) -> Option<String> {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if !content.trim().is_empty() {
        // Tool result text is often itself a JSON string (including OMP's
        // out-of-line blob envelope); normalize simplifies it, and leaves
        // prose untouched.
        return Some(normalize_json_text(&content));
    }
    result
        .get("details")
        .filter(|details| !details.is_null())
        .map(pretty_value)
}

fn pretty_value(value: &Value) -> String {
    let compact = compact_value(value, 0);
    truncate(
        &serde_json::to_string_pretty(&compact)
            .unwrap_or_else(|_| "(structured result could not be rendered)".into()),
    )
}

fn compact_value(value: &Value, depth: usize) -> Value {
    if depth >= 8 {
        return Value::String("…".into());
    }
    match value {
        Value::Array(values)
            if values.len() > 16
                && values
                    .iter()
                    .all(|value| value.as_u64().is_some_and(|byte| byte <= 255)) =>
        {
            Value::String(format!("<{} bytes>", values.len()))
        }
        Value::Array(values) => {
            let mut compact = values
                .iter()
                .take(MAX_JSON_ARRAY_ITEMS)
                .map(|value| compact_value(value, depth + 1))
                .collect::<Vec<_>>();
            if values.len() > MAX_JSON_ARRAY_ITEMS {
                compact.push(Value::String(format!(
                    "… {} more item(s)",
                    values.len() - MAX_JSON_ARRAY_ITEMS
                )));
            }
            Value::Array(compact)
        }
        // OMP offloads large tool payloads to a content-addressed blob store
        // and reports only this envelope. The hash is noise to a reader, so
        // collapse it to the size it stands in for.
        Value::Object(values)
            if values.get("storage").and_then(Value::as_str) == Some("spilled") =>
        {
            let bytes = values
                .get("byte_len")
                .or_else(|| {
                    values
                        .get("blob")
                        .and_then(Value::as_object)
                        .and_then(|blob| blob.get("byte_len"))
                })
                .and_then(Value::as_u64);
            match bytes {
                Some(bytes) => Value::String(format!("<{bytes} bytes, stored out of line>")),
                None => Value::String("<stored out of line>".into()),
            }
        }
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), compact_value(value, depth + 1)))
                .collect::<Map<_, _>>(),
        ),
        _ => value.clone(),
    }
}

fn truncate(value: &str) -> String {
    if value.len() <= MAX_DETAIL_BYTES {
        return value.into();
    }
    let mut end = MAX_DETAIL_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… detail truncated", &value[..end])
}

fn truncate_in_place(value: &mut String) {
    if value.len() > MAX_DETAIL_BYTES {
        *value = truncate(value);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn prepending_history_replaces_placeholder_without_losing_real_messages() {
        let mut conversation = Conversation::default();
        conversation.prepend_history(&[SessionMessage::spoken(Speaker::Assistant, super::WELCOME)]);
        conversation.submit_user("continue");
        assert_eq!(conversation.messages.len(), 2);
        assert_eq!(conversation.messages[0].text, super::WELCOME);
    }

    #[test]
    fn message_detail_preserves_complete_markdown() {
        let mut conversation = Conversation::default();
        let markdown = "| Item | Description |\n| --- | --- |\n| A | ".to_owned() + &"Long cell content. ".repeat(100) + "|";
        conversation.load_history(&[SessionMessage::spoken(Speaker::Assistant, &markdown)]);
        let detail = conversation.detail(0).expect("message detail");
        assert_eq!(detail.sections[0].body, markdown);
    }

    #[test]
    fn a_refresh_learns_only_what_changed_since_the_last_one() {
        let mut conversation = super::Conversation::default();
        assert_eq!(conversation.take_changes(), super::Changes::All);
        conversation.submit_user("hello");
        conversation.append_assistant("wor");
        conversation.append_assistant("ld");
        assert_eq!(conversation.take_changes(), super::Changes::All);
        assert_eq!(conversation.take_changes(), super::Changes::Messages(Vec::new()));
        conversation.start_tool("c1", "read");
        conversation.append_tool_update("c1", "done");
        assert_eq!(conversation.take_changes(), super::Changes::Messages(vec![2]));
        conversation.load_history(&[]);
        assert_eq!(conversation.take_changes(), super::Changes::All);
    }
    use serde_json::json;

    use crate::session::{SessionMessage, Speaker};

    use super::{Conversation, describe_tool};

    #[test]
    fn a_hub_call_reads_as_what_the_parent_did() {
        assert_eq!(
            describe_tool("hub", r#"{"op":"wait","ids":["job-3","job-4"]}"#),
            "Waited for job-3, job-4"
        );
        assert_eq!(
            describe_tool("hub", r#"{"op":"wait"}"#),
            "Waited for background work"
        );
        assert_eq!(
            describe_tool("hub", r#"{"op":"jobs"}"#),
            "Checked background jobs"
        );
        assert_eq!(
            describe_tool("hub", r#"{"op":"start","name":"dev-server"}"#),
            "Started dev-server"
        );
        assert_eq!(
            describe_tool("hub", r#"{"op":"ps"}"#),
            "Listed named processes"
        );
    }

    #[test]
    fn a_batch_task_call_names_every_child() {
        assert_eq!(
            describe_tool(
                "task",
                r#"{"context":"shared","tasks":[{"task":"a","name":"scout"},{"task":"b"}]}"#
            ),
            "Delegated scout, sub-agent 2"
        );
        assert_eq!(
            describe_tool("task", r#"{"task":"look around","name":"scout"}"#),
            "Delegated scout"
        );
    }

    #[test]
    fn starts_with_actionable_workspace_guidance() {
        let transcript = Conversation::default().render();

        assert!(transcript.contains("What would you like to work on?"));
        assert!(transcript.contains("Choose a project"));
    }

    #[test]
    fn ignores_blank_messages() {
        let mut conversation = Conversation::default();
        let before = conversation.render();

        assert!(!conversation.submit_user("  \n "));
        assert_eq!(conversation.render(), before);
    }

    #[test]
    fn streams_an_assistant_reply() {
        let mut conversation = Conversation::default();

        assert!(conversation.submit_user("  Hello from Rust  "));
        assert!(!conversation.render().contains(super::WELCOME));
        conversation.start_assistant();
        conversation.append_assistant("Hello");
        conversation.append_assistant(" back");
        conversation.finish_assistant(Some("Hello back"));

        assert!(
            conversation
                .render()
                .ends_with("You\nHello from Rust\n\nomp\nHello back")
        );
    }

    #[test]
    fn uses_final_text_when_no_delta_arrives() {
        let mut conversation = Conversation::default();
        conversation.start_assistant();
        conversation.finish_assistant(Some("Complete response"));

        assert!(conversation.render().ends_with("omp\nComplete response"));
    }

    #[test]
    fn renders_thinking_and_tool_arguments_in_order() {
        let mut conversation = Conversation::default();
        conversation.begin_turn();
        conversation.start_tool("call-1", "think");
        conversation.append_tool_arguments(
            "call-1",
            r#"{"thoughts":"Inspect the project, then write the file."}"#,
        );
        conversation.finish_tool(
            "call-1",
            Some("think"),
            Some(false),
            Some(&json!({"content": []})),
        );

        let transcript = conversation.render();
        assert!(transcript.contains("omp · thinking [Completed]"));
        // The reasoning summary appears inline; the full arguments are in the
        // detail modal.
        assert!(transcript.contains("Inspect the project, then write the file."));
    }

    #[test]
    fn a_failed_tool_flags_its_row_but_a_completed_turn_adds_no_note() {
        let mut conversation = Conversation::default();
        conversation.begin_turn();
        conversation.start_tool("call-1", "bash");
        conversation.finish_tool(
            "call-1",
            Some("bash"),
            Some(true),
            Some(&json!({
                "details": {"kind":"faulted","value":{"message":"README.md missing"}}
            })),
        );
        // The turn itself completed, so no turn-level note is added — the model
        // routinely retries a failed step.
        conversation.finish_turn();

        let transcript = conversation.render();
        // The row still carries a failed status pill.
        assert!(transcript.contains("omp · bash [Failed]"));
        // A completed turn adds no "turn" note.
        assert!(!transcript.contains("omp · turn"));
        // The fault message lives in the row's detail payload (index 1: after
        // the greeting).
        let detail = conversation.detail(1).expect("bash tool detail");
        assert_eq!(detail.title, "Tool · bash");
        let result = detail
            .sections
            .iter()
            .find(|section| section.heading.as_deref() == Some("Result"))
            .expect("a result section");
        assert!(result.code, "a tool result renders as code");
        assert!(result.body.contains("README.md missing"));
    }

    #[test]
    fn surfaces_the_reason_a_turn_errored_or_was_interrupted() {
        let mut errored = Conversation::default();
        errored.begin_turn();
        errored.finish_turn();
        errored.note_turn_end(true, Some("rate limit exceeded"));
        let transcript = errored.render();
        assert!(transcript.contains("omp · turn [Failed]"));
        assert!(transcript.contains("error: rate limit exceeded"));

        let mut interrupted = Conversation::default();
        interrupted.finish_turn();
        interrupted.note_turn_end(false, None);
        let transcript = interrupted.render();
        assert!(transcript.contains("omp · turn"));
        assert!(transcript.contains("Stopped before the turn finished"));
    }

    #[test]
    fn splits_a_tool_detail_into_labeled_sections() {
        use super::{Role, detail_sections};

        // A blank line inside the result keeps that content in the Result
        // section rather than starting a spurious one.
        let body =
            "Intent\nList files\n\nArguments\n{\n  \"path\": \".\"\n}\n\nResult\nfirst\n\nsecond";
        let sections = detail_sections(body, Role::Tool);
        let headings: Vec<_> = sections
            .iter()
            .map(|section| section.heading.as_deref())
            .collect();
        assert_eq!(
            headings,
            vec![Some("Intent"), Some("Arguments"), Some("Result")]
        );
        assert!(!sections[0].code, "intent is prose");
        assert!(sections[1].code, "arguments are code");
        assert_eq!(sections[2].body, "first\n\nsecond");

        // Reasoning has no headings and stays a single prose block.
        let reasoning = detail_sections("Think\n\nabout it", Role::Thinking);
        assert_eq!(reasoning.len(), 1);
        assert_eq!(reasoning[0].heading, None);
        assert!(!reasoning[0].code);
    }

    #[test]
    fn suppresses_empty_assistant_placeholders_before_tools() {
        let mut conversation = Conversation::default();
        conversation.start_assistant();
        conversation.start_tool("call-1", "glob");

        assert!(!conversation.render().contains("OMP returned no text"));
    }

    #[test]
    fn loads_session_history_with_readable_structure() {
        let mut conversation = Conversation::default();
        conversation.load_history(&[
            SessionMessage::spoken(Speaker::User, "Inspect the workspace"),
            SessionMessage::spoken(Speaker::Assistant, "I found **three** files."),
        ]);

        assert_eq!(
            conversation.render(),
            "You\nInspect the workspace\n\nomp\nI found **three** files."
        );
    }

    #[test]
    fn history_put_above_says_so_until_anything_else_changes() {
        let mut conversation = Conversation::default();
        conversation.load_history(&[SessionMessage::spoken(Speaker::Assistant, "Recent answer")]);
        assert_eq!(conversation.take_changes(), super::Changes::All);
        conversation.prepend_history(&[SessionMessage::spoken(Speaker::User, "Older question")]);
        conversation.prepend_history(&[
            SessionMessage::spoken(Speaker::User, "Oldest question"),
            SessionMessage::tool("read", "Read a file", None, false),
        ]);
        assert_eq!(conversation.take_changes(), super::Changes::Prepended(3));
        conversation.prepend_history(&[SessionMessage::spoken(Speaker::User, "Older still")]);
        conversation.submit_user("A new question");
        conversation.prepend_history(&[SessionMessage::spoken(Speaker::User, "Older again")]);
        assert_eq!(conversation.take_changes(), super::Changes::All);
    }

    #[test]
    fn prepends_older_session_history_without_reordering_the_recent_page() {
        let mut conversation = Conversation::default();
        conversation.load_history(&[SessionMessage::spoken(Speaker::Assistant, "Recent answer")]);
        conversation.prepend_history(&[SessionMessage::spoken(Speaker::User, "Older question")]);

        assert_eq!(
            conversation.render(),
            "You\nOlder question\n\nomp\nRecent answer"
        );
    }

    #[test]
    fn steering_splits_streaming_output_around_the_user_message() {
        let mut conversation = Conversation::default();
        conversation.start_assistant();
        conversation.append_assistant("First direction");
        assert!(conversation.steer_user("Change course"));
        conversation.append_assistant("Second direction");

        assert!(
            conversation
                .render()
                .ends_with("omp\nFirst direction\n\nYou\nChange course\n\nomp\nSecond direction")
        );
    }
}
