//! A session's events in the order they happened, each with how long it
//! took, folded from the session journal.
//!
//! The journal stamps every entry with the moment it was committed, so a
//! prompt, a response (from its start entry to its receipt, with the first
//! and last streamed fragment in between), a tool call (from the response
//! that produced it settling to its result), and an error notice all have a
//! place on one clock. Where a receipt carries OMP's own timings they win
//! over the entry stamps, since they were taken where the stream was read.

use omp_journal::{Entry, EntryId, data, kind, live_chain};
use serde_json::Value;
use std::collections::HashMap;

/// What kind of thing an event is, which decides how its row is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The user said something; a moment, not a span.
    Prompt,
    /// A model response: waiting for the first token, then receiving.
    Response,
    /// A tool the harness ran for the model.
    Tool,
    /// The turn ended with an error; a moment.
    Notice,
}

/// What a click on an event opens: the facts the journal recorded about
/// it, in display order, and the texts behind it (the prompt, a response's
/// reasoning and reply, a call's arguments and result, an error's reason).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Info {
    pub(crate) facts: Vec<(String, String)>,
    pub(crate) sections: Vec<(String, String)>,
}

/// One event on the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) kind: Kind,
    /// Everything the journal knows about the event, for its detail sheet.
    pub(crate) info: Info,
    /// The sub-agent whose journal the event came from, when it did not
    /// come from the session's own; such events sit one level in.
    pub(crate) agent: Option<String>,
    /// The model behind the event: a response's own, or for a tool call the
    /// model that made it. A prompt or a notice has none.
    pub(crate) model: Option<String>,
    /// Unix milliseconds.
    pub(crate) started_ms: u64,
    pub(crate) ended_ms: u64,
    /// The part of a response spent waiting for its first token.
    pub(crate) waiting_ms: u64,
    /// Who or what: "You", the model, the tool's name.
    pub(crate) label: String,
    /// What was said, or the figures: tokens and rate for a response, the
    /// intent for a tool, the reason for a notice.
    pub(crate) detail: String,
    pub(crate) failed: bool,
    /// What the search matches against, lowercased, once the session's
    /// start is known; empty until then.
    pub(crate) search: String,
}

impl Event {
    pub(crate) fn duration_ms(&self) -> u64 {
        self.ended_ms.saturating_sub(self.started_ms)
    }

    /// Everything a search word can match: the kind, who or what, the
    /// detail, the sub-agent, the duration, and the offset from `origin_ms`.
    fn haystack(&self, origin_ms: u64) -> String {
        format!(
            "{} {} {} {} {} +{}",
            self.kind.name(),
            self.label,
            self.detail,
            self.agent.as_deref().unwrap_or(""),
            crate::metrics::duration_text(self.duration_ms()),
            offset_text(self.started_ms.saturating_sub(origin_ms)),
        )
        .to_lowercase()
    }
}

/// A whole session on one clock.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Timeline {
    /// When the first event started; offsets are measured from here.
    pub(crate) origin_ms: u64,
    pub(crate) events: Vec<Event>,
}

impl Timeline {
    /// Every model the events used, in order of first appearance, across
    /// the session and its sub-agents.
    pub(crate) fn models(&self) -> Vec<String> {
        let mut models: Vec<String> = Vec::new();
        for event in &self.events {
            if let Some(model) = &event.model
                && !models.contains(model)
            {
                models.push(model.clone());
            }
        }
        models
    }

    /// From the first event's start to the last event's end.
    pub(crate) fn span_ms(&self) -> u64 {
        self.events
            .iter()
            .map(|event| event.ended_ms)
            .max()
            .unwrap_or(self.origin_ms)
            .saturating_sub(self.origin_ms)
    }

    /// The longest single event, which the duration bars are drawn against.
    pub(crate) fn longest_ms(&self) -> u64 {
        self.events
            .iter()
            .map(Event::duration_ms)
            .max()
            .unwrap_or(0)
    }
}

/// A response being assembled from its start entry onwards.
struct OpenResponse {
    started_ms: u64,
    model: String,
    provider: String,
    route: String,
    first_token_ms: Option<u64>,
    last_token_ms: Option<u64>,
    ended_ms: Option<u64>,
    stop_reason: Option<String>,
    /// Its text and reasoning streams by stream id, and what they said.
    streams: HashMap<u32, ResponseStream>,
    text: String,
    thinking: String,
    /// The tools it called, in order.
    calls: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseStream {
    Text,
    Thinking,
}

/// A tool call whose result has not been read yet. Its arguments arrive
/// either whole on the call, or streamed (`sid`) and then confirmed by a
/// `kernel: ready` update, which is also where the intent lands.
struct PendingTool {
    name: String,
    call_id: String,
    /// The model of the response that made the call.
    model: Option<String>,
    intent: Option<String>,
    args: Option<String>,
    arg_buffer: String,
    /// When the model called it; execution starts once its response settles.
    called_ms: u64,
    started_ms: Option<u64>,
}

impl PendingTool {
    /// The arguments as the model gave them: whole on the call, or as
    /// streamed.
    fn arguments(&self) -> Option<&str> {
        self.args
            .as_deref()
            .filter(|args| !args.trim().is_empty())
            .or_else(|| (!self.arg_buffer.trim().is_empty()).then_some(self.arg_buffer.as_str()))
    }

    /// What the row says under the tool's name.
    fn detail(&self) -> String {
        tool_detail(&self.name, self.intent.as_deref(), self.arguments())
    }
}

/// Reads the journal at `path` and folds its live chain, with the journals
/// of the sub-agents it spawned (beside it, named by their ids) folded in
/// one level down, so a wait on them shows what they were doing.
///
/// This touches the filesystem and belongs on a worker thread.
pub(crate) fn read(path: &std::path::Path) -> Result<Timeline, String> {
    let entries = omp_journal::Journal::scan(path).map_err(|error| error.to_string())?;
    let mut children = Vec::new();
    if let Some(directory) = path.parent() {
        for id in spawned_ids(&entries) {
            let child = directory.join(format!("{id}.oms"));
            if let Ok(child_entries) = omp_journal::Journal::scan(&child) {
                children.push((id, child_entries));
            }
        }
    }
    Ok(fold_with_children(&entries, &children))
}

/// The ids of every sub-agent the session spawned, in spawn order.
fn spawned_ids(entries: &[Entry]) -> Vec<String> {
    let mut ids = Vec::new();
    for entry in live_chain(entries) {
        if entry.kind.name.as_str() == kind::PATCH && entry.label.as_deref() == Some("jobs.insert")
        {
            for id in crate::journal::inserted_subagent_ids(entry.data.as_str()) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

/// Folds a session and its sub-agents' journals onto one clock: the
/// session's events at the top level, each sub-agent's one level in and
/// named after it, all in the order they happened.
pub(crate) fn fold_with_children(entries: &[Entry], children: &[(String, Vec<Entry>)]) -> Timeline {
    let mut timeline = fold(entries);
    for (id, child_entries) in children {
        let child = fold(child_entries);
        timeline
            .events
            .extend(child.events.into_iter().map(|mut event| {
                event.agent = Some(id.clone());
                event
            }));
    }
    timeline.events.sort_by_key(|event| event.started_ms);
    timeline.origin_ms = timeline
        .events
        .first()
        .map_or(timeline.origin_ms, |event| event.started_ms);
    // The offsets are final now, so each event's search text can be set
    // once here rather than on every keystroke.
    let origin_ms = timeline.origin_ms;
    for event in &mut timeline.events {
        event.search = event.haystack(origin_ms);
    }
    timeline
}

/// Folds committed entries into events, oldest first.
pub(crate) fn fold(entries: &[Entry]) -> Timeline {
    let mut events: Vec<Event> = Vec::new();
    let mut open: Option<OpenResponse> = None;
    // The kind of the content node the next stream opens on, from the
    // `assistant.content` patch that inserted it.
    let mut next_content_kind: Option<String> = None;
    let mut pending: Vec<PendingTool> = Vec::new();
    let mut tool_by_entry: HashMap<EntryId, usize> = HashMap::new();
    let mut tool_by_sid: HashMap<u32, usize> = HashMap::new();
    let mut settled: Vec<bool> = Vec::new();
    // The model of the response now streaming, which its tool calls take.
    let mut current_model: Option<String> = None;
    for entry in live_chain(entries) {
        let at = entry.id.as_ulid().timestamp_ms();
        match entry.kind.name.as_str() {
            kind::MSG_USER => {
                let Ok(payload) = serde_json::from_str::<data::MsgUser>(entry.data.as_str())
                else {
                    continue;
                };
                let mut facts = vec![("Kind".to_owned(), "Prompt".to_owned())];
                if !payload.attachments.is_empty() {
                    facts.push(("Attachments".to_owned(), payload.attachments.len().to_string()));
                }
                facts.push((
                    "Length".to_owned(),
                    format!("{} characters", payload.text.chars().count()),
                ));
                events.push(Event {
                    kind: Kind::Prompt,
                    info: Info {
                        facts,
                        sections: vec![("Prompt".to_owned(), payload.text.to_string())],
                    },
                    agent: None,
                    model: None,
                    started_ms: at,
                    ended_ms: at,
                    waiting_ms: 0,
                    label: "You".to_owned(),
                    detail: excerpt(payload.text.as_str(), 120),
                    failed: false,
                    search: String::new(),
                });
            }
            kind::MSG_ASSISTANT_START => {
                next_content_kind = None;
                let payload =
                    serde_json::from_str::<data::MsgAssistantStart>(entry.data.as_str()).ok();
                let model = payload
                    .as_ref()
                    .map_or_else(|| "model".to_owned(), |payload| short_model(&payload.model));
                current_model = Some(model.clone());
                open = Some(OpenResponse {
                    started_ms: at,
                    model,
                    provider: payload
                        .as_ref()
                        .map(|payload| payload.provider.to_string())
                        .unwrap_or_default(),
                    route: payload
                        .as_ref()
                        .map(|payload| payload.route.to_string())
                        .unwrap_or_default(),
                    first_token_ms: None,
                    last_token_ms: None,
                    ended_ms: None,
                    stop_reason: None,
                    streams: HashMap::new(),
                    text: String::new(),
                    thinking: String::new(),
                    calls: Vec::new(),
                });
            }
            kind::STREAM => {
                let Ok(payload) = serde_json::from_str::<data::Stream>(entry.data.as_str())
                else {
                    continue;
                };
                if payload.op == data::StreamOp::Open
                    && matches!(payload.prop.as_deref(), Some("text" | "thinking"))
                    && let Some(response) = open.as_mut()
                {
                    // Reasoning and reply both stream into a `text` property;
                    // the content node inserted just before the open says
                    // which this is.
                    let thinking = next_content_kind.take().as_deref() == Some("thinking");
                    match payload.prop.as_deref() {
                        Some("text") if !thinking => {
                            response.streams.insert(payload.sid, ResponseStream::Text);
                        }
                        Some("text") | Some("thinking") => {
                            response.streams.insert(payload.sid, ResponseStream::Thinking);
                        }
                        _ => {}
                    }
                }
                if payload.op == data::StreamOp::Append {
                    if let Some(response) = open.as_mut() {
                        response.first_token_ms.get_or_insert(at);
                        response.last_token_ms = Some(at);
                        if let Some(text) = payload.text.as_deref() {
                            match response.streams.get(&payload.sid) {
                                Some(ResponseStream::Text) => response.text.push_str(text),
                                Some(ResponseStream::Thinking) => {
                                    response.thinking.push_str(text);
                                }
                                None => {}
                            }
                        }
                    }
                    if let Some(index) = tool_by_sid.get(&payload.sid)
                        && let Some(tool) = pending.get_mut(*index)
                        && let Some(text) = payload.text.as_deref()
                    {
                        tool.arg_buffer.push_str(text);
                    }
                }
            }
            kind::MSG_ASSISTANT_END => {
                next_content_kind = None;
                if let Some(response) = open.as_mut() {
                    response.ended_ms = Some(at);
                    response.stop_reason =
                        serde_json::from_str::<data::MsgAssistantEnd>(entry.data.as_str())
                            .ok()
                            .map(|payload| payload.stop_reason.to_string());
                }
            }
            kind::TURN_RECEIPT => {
                let receipt = serde_json::from_str::<data::TurnReceipt>(entry.data.as_str()).ok();
                if let Some(response) = open.take() {
                    events.push(response_event(response, at, receipt.as_ref()));
                }
                // The calls this response made run from here.
                for (index, tool) in pending.iter_mut().enumerate() {
                    if !settled[index] && tool.started_ms.is_none() {
                        tool.started_ms = Some(at);
                    }
                }
            }
            kind::TOOL_CALL => {
                let Ok(payload) = serde_json::from_str::<data::ToolCall>(entry.data.as_str())
                else {
                    continue;
                };
                let intent = payload
                    .i
                    .as_deref()
                    .map(str::trim)
                    .filter(|intent| !intent.is_empty())
                    .map(str::to_owned);
                tool_by_entry.insert(entry.id, pending.len());
                if let Some(sid) = payload.sid {
                    tool_by_sid.insert(sid, pending.len());
                }
                if let Some(response) = open.as_mut() {
                    response.calls.push(payload.name.to_string());
                }
                pending.push(PendingTool {
                    name: payload.name.to_string(),
                    call_id: payload.call_id.to_string(),
                    model: current_model.clone(),
                    intent,
                    args: payload.args.as_ref().map(|args| args.get().to_owned()),
                    arg_buffer: String::new(),
                    called_ms: at,
                    started_ms: None,
                });
                settled.push(false);
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
                        tool.args = Some(args.to_string());
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
                // The transcript's own reading of a result decides whether
                // the call failed, and what it said.
                let (failed, result) = serde_json::from_str::<data::ToolResult>(entry.data.as_str())
                    .map(|payload| crate::journal::tool_result_text(&payload))
                    .unwrap_or((false, String::new()));
                if let Some(tool) = pending.get(index)
                    && !settled[index]
                {
                    settled[index] = true;
                    let started = tool.started_ms.unwrap_or(tool.called_ms).min(at);
                    let mut facts = vec![
                        ("Tool".to_owned(), tool.name.clone()),
                        ("Call".to_owned(), tool.call_id.clone()),
                    ];
                    if let Some(model) = &tool.model {
                        facts.push(("Called by".to_owned(), model.clone()));
                    }
                    if let Some(intent) = &tool.intent {
                        facts.push(("Intent".to_owned(), intent.clone()));
                    }
                    facts.push((
                        "Outcome".to_owned(),
                        if failed { "failed" } else { "ok" }.to_owned(),
                    ));
                    facts.push((
                        "Called".to_owned(),
                        format!("{} before it ran", crate::metrics::duration_text(started.saturating_sub(tool.called_ms))),
                    ));
                    facts.push(("Took".to_owned(), crate::metrics::duration_text(at.saturating_sub(started))));
                    let mut sections = Vec::new();
                    if let Some(args) = tool.arguments() {
                        sections.push(("Arguments".to_owned(), pretty_json(args)));
                    }
                    if !result.trim().is_empty() {
                        sections.push(("Result".to_owned(), pretty_json(&result)));
                    }
                    events.push(Event {
                        kind: Kind::Tool,
                        info: Info { facts, sections },
                        agent: None,
                        model: tool.model.clone(),
                        started_ms: started,
                        ended_ms: at,
                        waiting_ms: 0,
                        label: tool.name.clone(),
                        detail: tool.detail(),
                        failed,
                        search: String::new(),
                    });
                }
            }
            kind::PATCH if entry.label.as_deref() == Some("assistant.content") => {
                next_content_kind = crate::journal::content_kind(entry.data.as_str());
            }
            kind::PATCH if entry.label.as_deref() == Some("kernel.notice") => {
                if let Some(reason) = crate::journal::error_notice_text(entry.data.as_str()) {
                    events.push(Event {
                        kind: Kind::Notice,
                        info: Info {
                            facts: vec![("Kind".to_owned(), "Error notice".to_owned())],
                            sections: vec![("Reason".to_owned(), reason.clone())],
                        },
                        agent: None,
                        model: None,
                        started_ms: at,
                        ended_ms: at,
                        waiting_ms: 0,
                        label: "Error".to_owned(),
                        detail: excerpt(&reason, 160),
                        failed: true,
                        search: String::new(),
                    });
                }
            }
            _ => {}
        }
    }
    // A response the child never settled (it was cut off) still shows what
    // it streamed.
    if let Some(response) = open.take() {
        let at = response
            .ended_ms
            .or(response.last_token_ms)
            .unwrap_or(response.started_ms);
        events.push(response_event(response, at, None));
    }
    events.sort_by_key(|event| event.started_ms);
    let origin_ms = events.first().map_or(0, |event| event.started_ms);
    Timeline { origin_ms, events }
}

/// A settled response: the receipt's timings when it has them, since those
/// were taken where the stream was read; the entry stamps otherwise.
fn response_event(
    response: OpenResponse,
    settled_ms: u64,
    receipt: Option<&data::TurnReceipt>,
) -> Event {
    let (started, first, last, ended) = match receipt {
        Some(receipt) if receipt.duration_ms.is_some() => {
            let duration = receipt.duration_ms.unwrap_or(0);
            let started = settled_ms.saturating_sub(duration);
            (
                started,
                receipt.ttft_ms.map(|ttft| started + ttft),
                receipt.last_token_ms.map(|last| started + last),
                settled_ms,
            )
        }
        _ => (
            response.started_ms,
            response.first_token_ms,
            response.last_token_ms,
            response.ended_ms.unwrap_or(settled_ms).max(response.started_ms),
        ),
    };
    let waiting_ms = first.map_or(ended.saturating_sub(started), |first| {
        first.saturating_sub(started)
    });
    let duration_text = crate::metrics::duration_text;
    let mut parts = Vec::new();
    let mut facts = vec![
        ("Model".to_owned(), response.model.clone()),
        ("Provider".to_owned(), response.provider.clone()),
    ];
    if !response.route.is_empty() && response.route != response.provider {
        facts.push(("Route".to_owned(), response.route.clone()));
    }
    facts.push(("Took".to_owned(), duration_text(ended.saturating_sub(started))));
    if let Some(first) = first {
        facts.push(("First token".to_owned(), format!("after {}", duration_text(first - started))));
    }
    if let (Some(first), Some(last)) = (first, last) {
        facts.push(("Receiving".to_owned(), duration_text(last.saturating_sub(first))));
    }
    if let Some(receipt) = receipt {
        if let Some(first) = first {
            parts.push(format!("TTFT {}", duration_text(first - started)));
        }
        parts.push(format!("{} tokens", receipt.tokens_out));
        if let (Some(first), Some(last)) = (first, last) {
            // The same rule as the header's dial: a streamed response is
            // timed first token to last; one that arrived in a single flush
            // (Codex holds a reply through its reasoning) is timed over the
            // whole turn, and says so.
            let decode = last.saturating_sub(first);
            let whole = last.saturating_sub(started);
            let tokens = receipt.tokens_out;
            let timed = if decode >= 50 {
                Some((decode, false))
            } else if whole >= 50 {
                Some((whole, true))
            } else {
                None
            };
            if let Some((span, flushed)) = timed
                && tokens >= 2
            {
                let rate = (tokens as f64 * 1000.0 / span as f64).round() as u64;
                parts.push(format!("{rate} tok/s"));
                if flushed {
                    parts.push("one flush".to_owned());
                }
                facts.push((
                    "Throughput".to_owned(),
                    if flushed {
                        format!("{rate} tokens a second over the whole turn (one flush)")
                    } else {
                        format!("{rate} tokens a second")
                    },
                ));
            }
        }
        facts.push(("Tokens in".to_owned(), receipt.tokens_in.to_string()));
        let out = if receipt.tokens_reasoning > 0 {
            format!("{} ({} reasoning)", receipt.tokens_out, receipt.tokens_reasoning)
        } else {
            receipt.tokens_out.to_string()
        };
        facts.push(("Tokens out".to_owned(), out));
        if receipt.cache_read > 0 || receipt.cache_write > 0 {
            facts.push((
                "Cache".to_owned(),
                format!("{} read · {} written", receipt.cache_read, receipt.cache_write),
            ));
        }
        if receipt.cost_nano_usd > 0 {
            facts.push((
                "Cost".to_owned(),
                format!("${:.4}", receipt.cost_nano_usd as f64 / 1_000_000_000.0),
            ));
        }
        if !receipt.recoveries.is_empty() {
            facts.push((
                "Recoveries".to_owned(),
                receipt
                    .recoveries
                    .iter()
                    .map(|recovery| recovery.kind.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
    } else {
        parts.push("no receipt".to_owned());
        facts.push(("Receipt".to_owned(), "none: the response never settled".to_owned()));
    }
    if let Some(stop) = &response.stop_reason {
        facts.push(("Stopped".to_owned(), stop.clone()));
    }
    if !response.calls.is_empty() {
        facts.push(("Called".to_owned(), response.calls.join(", ")));
    }
    let mut sections = Vec::new();
    if !response.thinking.trim().is_empty() {
        sections.push(("Reasoning".to_owned(), response.thinking.clone()));
    }
    if !response.text.trim().is_empty() {
        sections.push(("Reply".to_owned(), response.text.clone()));
    }
    Event {
        kind: Kind::Response,
        info: Info { facts, sections },
        agent: None,
        model: Some(response.model.clone()),
        started_ms: started,
        ended_ms: ended.max(started),
        waiting_ms,
        label: response.model,
        detail: parts.join(" · "),
        failed: false,
        search: String::new(),
    }
}

/// `text` as indented JSON when it parses, itself otherwise.
fn pretty_json(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| text.to_owned())
}

/// The model without its provider prefix.
fn short_model(model: &str) -> String {
    model
        .rsplit_once('/')
        .map_or(model, |(_, name)| name)
        .to_owned()
}

/// One line of `text`, cut to `limit` characters.
fn excerpt(text: &str, limit: usize) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= limit {
        line
    } else {
        format!("{}…", line.chars().take(limit).collect::<String>().trim_end())
    }
}

/// What a tool row says: for the coordination tools, what was asked of
/// whom (`hub` waits on named jobs; `task` spawns named sub-agents), then
/// the model's stated intent; for the rest, the intent or the most telling
/// argument.
fn tool_detail(name: &str, intent: Option<&str>, args: Option<&str>) -> String {
    let parsed = args.and_then(|args| serde_json::from_str::<Value>(args).ok());
    let mut parts: Vec<String> = Vec::new();
    match (name, parsed.as_ref()) {
        ("hub", Some(value)) => {
            let op = value
                .get("op")
                .and_then(Value::as_str)
                .unwrap_or("hub");
            let ids: Vec<&str> = value
                .get("ids")
                .and_then(Value::as_array)
                .map(|ids| ids.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let name = value
                .get("name")
                .or_else(|| value.get("to"))
                .and_then(Value::as_str);
            let what = if !ids.is_empty() {
                ids.join(", ")
            } else if let Some(name) = name {
                name.to_owned()
            } else {
                String::new()
            };
            parts.push(match (op, what.is_empty()) {
                ("wait", false) => format!("wait for {what}"),
                (op, false) => format!("{op} {what}"),
                (op, true) => op.to_owned(),
            });
        }
        ("task", Some(value)) => {
            let names: Vec<&str> = value
                .get("tasks")
                .and_then(Value::as_array)
                .map(|tasks| {
                    tasks
                        .iter()
                        .filter_map(|task| task.get("name").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            if !names.is_empty() {
                parts.push(format!("spawn {}", names.join(", ")));
            }
        }
        _ => {}
    }
    if let Some(intent) = intent {
        parts.push(excerpt(intent, 120));
    } else if parts.is_empty()
        && let Some(args) = args
    {
        parts.push(args_excerpt(args));
    }
    parts.join(" · ")
}

/// The most telling of a call's arguments: a path, a command, a pattern, a
/// prompt; the whole object cut short otherwise.
fn args_excerpt(args: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(args) else {
        return excerpt(args, 80);
    };
    for key in [
        "path",
        "file_path",
        "command",
        "cmd",
        "pattern",
        "query",
        "prompt",
        "description",
        "url",
    ] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return excerpt(text, 80);
        }
    }
    excerpt(args, 80)
}

impl Kind {
    /// The kind as the filter menu and the search name it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Response => "response",
            Self::Tool => "tool",
            Self::Notice => "error",
        }
    }
}

/// What the timeline is narrowed to. Every field's default admits every
/// event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Filter {
    /// Words that must all appear somewhere in an event: its kind, who or
    /// what, its detail, its sub-agent, its duration, or its offset.
    pub(crate) query: String,
    /// The shortest event still shown.
    pub(crate) min_ms: u64,
    /// Only events of this kind.
    pub(crate) kind: Option<Kind>,
    /// Only events behind this model: its responses and the tool calls
    /// they made, in the session or a sub-agent.
    pub(crate) model: Option<String>,
}

impl Filter {
    pub(crate) fn is_active(&self) -> bool {
        self.query_active() || self.min_ms > 0 || self.kind.is_some() || self.model.is_some()
    }

    pub(crate) fn query_active(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Whether `event` stays in view. `origin_ms` is the session's start,
    /// which the offset is searched against.
    #[cfg(test)]
    pub(crate) fn admits(&self, event: &Event, origin_ms: u64) -> bool {
        self.admits_words(event, origin_ms, &self.words())
    }

    /// The query's words, lowercased once for a whole pass over the events.
    fn words(&self) -> Vec<String> {
        self.query.split_whitespace().map(str::to_lowercase).collect()
    }

    fn admits_words(&self, event: &Event, origin_ms: u64, words: &[String]) -> bool {
        if self.kind.is_some_and(|kind| kind != event.kind) {
            return false;
        }
        if self
            .model
            .as_deref()
            .is_some_and(|model| event.model.as_deref() != Some(model))
        {
            return false;
        }
        if event.duration_ms() < self.min_ms {
            return false;
        }
        if words.is_empty() {
            return true;
        }
        // Events from a fold carry their search text; one built by hand
        // has it made here.
        let made;
        let haystack = if event.search.is_empty() {
            made = event.haystack(origin_ms);
            &made
        } else {
            &event.search
        };
        words.iter().all(|word| haystack.contains(word.as_str()))
    }
}

/// An offset from the session's start: "0:00.0", "1:23.4", "1:02:03".
pub(crate) fn offset_text(ms: u64) -> String {
    let seconds = ms / 1000;
    let tenths = (ms % 1000) / 100;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}.{tenths}", seconds / 60, seconds % 60)
    }
}

/// A duration typed into the filter: "2", "2.5", "2.5s", "500ms", "1m",
/// "1m30s", "1:30". Empty means no minimum.
pub(crate) fn parse_duration_ms(text: &str) -> Option<u64> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return Some(0);
    }
    if let Some((minutes, seconds)) = text.split_once(':') {
        let minutes: f64 = minutes.trim().parse().ok()?;
        let seconds: f64 = seconds.trim().parse().ok()?;
        return Some(((minutes * 60.0 + seconds) * 1000.0).round() as u64);
    }
    let mut total = 0.0;
    let mut number = String::new();
    let mut unit = String::new();
    let flush = |number: &mut String, unit: &mut String, total: &mut f64| -> Option<()> {
        if number.is_empty() {
            return if unit.is_empty() { Some(()) } else { None };
        }
        let value: f64 = number.parse().ok()?;
        let scale = match unit.as_str() {
            "" | "s" | "sec" | "secs" | "second" | "seconds" => 1000.0,
            "ms" | "millis" | "milliseconds" => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60_000.0,
            "h" | "hr" | "hour" | "hours" => 3_600_000.0,
            _ => return None,
        };
        *total += value * scale;
        number.clear();
        unit.clear();
        Some(())
    };
    for character in text.chars() {
        if character.is_ascii_digit() || character == '.' {
            if !unit.is_empty() {
                flush(&mut number, &mut unit, &mut total)?;
            }
            number.push(character);
        } else if character.is_ascii_alphabetic() {
            unit.push(character);
        } else if character.is_whitespace() {
            continue;
        } else {
            return None;
        }
    }
    flush(&mut number, &mut unit, &mut total)?;
    Some(total.round() as u64)
}

/// The `fraction` percentile of the events' durations (0.9 for p90), for
/// the minimum-duration hint.
pub(crate) fn percentile_ms(events: &[Event], fraction: f64) -> Option<u64> {
    let mut durations: Vec<u64> = events.iter().map(Event::duration_ms).collect();
    if durations.is_empty() {
        return None;
    }
    durations.sort_unstable();
    let index = ((durations.len() - 1) as f64 * fraction).round() as usize;
    durations.get(index).copied()
}

/// How many of the newest events the first page carries: enough to fill
/// the window straight away.
const FIRST_PAGE_EVENTS: usize = 48;
/// How many older events each later page carries.
const PAGE_EVENTS: usize = 256;

/// The whole session's figures, which the fold knows before any row is
/// sent: the totals line, the minimum hint, the model menu, and the scale
/// every duration bar is drawn against, so none of them move as older
/// events arrive.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Summary {
    pub(crate) origin_ms: u64,
    pub(crate) span_ms: u64,
    pub(crate) longest_ms: u64,
    pub(crate) p90_ms: Option<u64>,
    pub(crate) events: usize,
    pub(crate) responses: usize,
    pub(crate) tools: usize,
    pub(crate) agents: usize,
    pub(crate) models: Vec<String>,
}

impl Summary {
    fn of(timeline: &Timeline) -> Self {
        let count = |kind| timeline.events.iter().filter(|event| event.kind == kind).count();
        let mut agents: Vec<&str> = Vec::new();
        for event in &timeline.events {
            if let Some(agent) = event.agent.as_deref()
                && !agents.contains(&agent)
            {
                agents.push(agent);
            }
        }
        Self {
            origin_ms: timeline.origin_ms,
            span_ms: timeline.span_ms(),
            longest_ms: timeline.longest_ms(),
            p90_ms: percentile_ms(&timeline.events, 0.9),
            events: timeline.events.len(),
            responses: count(Kind::Response),
            tools: count(Kind::Tool),
            agents: agents.len(),
            models: timeline.models(),
        }
    }
}

/// What the timeline worker sends, newest first, the way the chat's
/// history arrives: the most recent events straight away, then older ones
/// page by page until the session's start.
#[derive(Debug, Clone)]
pub(crate) enum Update {
    /// The fold finished: the session's figures and its newest events,
    /// oldest first.
    Newest { summary: Summary, events: Vec<Event> },
    /// Events from just before every page sent so far, oldest first.
    Older(Vec<Event>),
    Complete,
    Failed(String),
}

/// Reads and folds the journal at `path`, then sends it newest first in
/// pages through `send`, stopping as soon as `send` says no one is
/// listening (the window closed).
///
/// This touches the filesystem and belongs on a worker thread.
pub(crate) fn stream(path: &std::path::Path, send: impl Fn(Update) -> bool) {
    let _span = crate::perf::span("gui.timeline.stream");
    let timeline = match read(path) {
        Ok(timeline) => timeline,
        Err(error) => {
            send(Update::Failed(error));
            return;
        }
    };
    let summary = Summary::of(&timeline);
    let mut remaining = timeline.events;
    let newest = remaining.split_off(remaining.len().saturating_sub(FIRST_PAGE_EVENTS));
    if !send(Update::Newest {
        summary,
        events: newest,
    }) {
        return;
    }
    while !remaining.is_empty() {
        let page = remaining.split_off(remaining.len().saturating_sub(PAGE_EVENTS));
        if !send(Update::Older(page)) {
            return;
        }
    }
    send(Update::Complete);
}

/// The part of a session's timeline that has arrived: its figures, and
/// its newest events, reaching further back as older pages land. Events
/// are indexed by their place in the whole session, so an index, a row,
/// and an opened run all stay put while older events are put in front.
#[derive(Debug, Clone, Default)]
pub(crate) struct Loaded {
    pub(crate) summary: Summary,
    /// The session-wide index of `events[0]`.
    offset: usize,
    events: Vec<Event>,
}

impl Loaded {
    pub(crate) fn new(summary: Summary, newest: Vec<Event>) -> Self {
        Self {
            offset: summary.events.saturating_sub(newest.len()),
            summary,
            events: newest,
        }
    }

    /// Puts `older` in front of everything that has arrived.
    pub(crate) fn prepend(&mut self, mut older: Vec<Event>) {
        self.offset = self.offset.saturating_sub(older.len());
        older.append(&mut self.events);
        self.events = older;
    }

    pub(crate) fn get(&self, index: usize) -> Option<&Event> {
        index
            .checked_sub(self.offset)
            .and_then(|local| self.events.get(local))
    }

    /// How many events have arrived.
    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    /// The arrived events with their session-wide indices, oldest first.
    fn indexed(&self) -> impl Iterator<Item = (usize, &Event)> {
        self.events
            .iter()
            .enumerate()
            .map(|(local, event)| (self.offset + local, event))
    }
}

/// One row of the list as laid out: an event, or a run of events the
/// filter took out, folded into one bar that opens when clicked. A row is
/// its own identity, so a view drawn for it can outlive a relayout.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Row {
    Event {
        /// The event's session-wide index.
        index: usize,
        /// The event is in a run the filter took out that the user opened.
        dimmed: bool,
    },
    Hidden {
        /// The first event of the run.
        first: usize,
        count: usize,
        /// How long the hidden events took, together.
        total_ms: u64,
        open: bool,
    },
}

impl Row {

    /// What names a hidden run in the set of opened ones: the index just
    /// past its last event, which stays put when older events arrive and
    /// lengthen the run at its front.
    pub(crate) const fn run_key(first: usize, count: usize) -> usize {
        first + count
    }
}

/// The rows the list shows for what has arrived under `filter`: every
/// admitted event where it falls, and each run of events the filter took
/// out as a bar saying how many, opened (its events shown, dimmed) when
/// its key is in `open_runs`.
pub(crate) fn rows(
    loaded: &Loaded,
    filter: &Filter,
    open_runs: &std::collections::HashSet<usize>,
) -> Vec<Row> {
    if !filter.is_active() {
        return loaded
            .indexed()
            .map(|(index, _)| Row::Event {
                index,
                dimmed: false,
            })
            .collect();
    }
    let origin_ms = loaded.summary.origin_ms;
    let words = filter.words();
    let admitted: Vec<bool> = loaded
        .events
        .iter()
        .map(|event| filter.admits_words(event, origin_ms, &words))
        .collect();
    let mut rows = Vec::new();
    let mut local = 0;
    while local < loaded.events.len() {
        if admitted[local] {
            rows.push(Row::Event {
                index: loaded.offset + local,
                dimmed: false,
            });
            local += 1;
            continue;
        }
        let start = local;
        let mut total_ms = 0;
        while local < loaded.events.len() && !admitted[local] {
            total_ms += loaded.events[local].duration_ms();
            local += 1;
        }
        let first = loaded.offset + start;
        let count = local - start;
        let open = open_runs.contains(&Row::run_key(first, count));
        rows.push(Row::Hidden {
            first,
            count,
            total_ms,
            open,
        });
        if open {
            rows.extend((first..first + count).map(|index| Row::Event {
                index,
                dimmed: true,
            }));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use omp_journal::{Kind as JournalKind, KindName};
    use serde_json::json;
    use std::str::FromStr;

    /// A ULID whose timestamp is `at_ms`, with a serial tail so ids differ.
    fn id(at_ms: u64, serial: u32) -> EntryId {
        const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let mut text = String::with_capacity(26);
        let mut time = at_ms;
        let mut head = ['0'; 10];
        for slot in head.iter_mut().rev() {
            *slot = ALPHABET[(time & 31) as usize] as char;
            time >>= 5;
        }
        text.extend(head);
        let mut tail = ['0'; 16];
        let mut serial = u64::from(serial);
        for slot in tail.iter_mut().rev() {
            *slot = ALPHABET[(serial & 31) as usize] as char;
            serial >>= 5;
        }
        text.extend(tail);
        EntryId::from_str(&text).expect("a valid ulid")
    }

    /// Everything in `events` as if every page had arrived.
    fn arrived(events: &[Event]) -> Loaded {
        Loaded::new(
            Summary {
                events: events.len(),
                ..Summary::default()
            },
            events.to_vec(),
        )
    }

    fn entry(
        at_ms: u64,
        serial: u32,
        kind: KindName,
        label: Option<&str>,
        data: serde_json::Value,
        by: Option<EntryId>,
    ) -> Entry {
        Entry {
            id: id(at_ms, serial),
            kind: JournalKind::known(kind),
            by,
            prior: None,
            label: label.map(|label| label.to_owned().into()),
            data: data.to_string().into(),
        }
    }

    #[test]
    fn a_turn_folds_into_a_prompt_a_response_a_tool_and_a_second_response() {
        let start = 1_000_000;
        let turn = entry(start, 1, KindName::TurnStart, None, json!({}), None);
        let user = entry(
            start + 10,
            2,
            KindName::MsgUser,
            None,
            json!({ "text": "Read   Cargo.toml\nand tell me the name" }),
            Some(turn.id),
        );
        let assistant = entry(
            start + 20,
            3,
            KindName::MsgAssistantStart,
            None,
            json!({ "model": "openai-codex/gpt-5.6-sol", "provider": "openai-codex", "route": "r" }),
            Some(user.id),
        );
        let call = entry(
            start + 2_000,
            4,
            KindName::ToolCall,
            None,
            json!({ "name": "read", "rev": 1, "call_id": "c1", "sid": 3 }),
            Some(assistant.id),
        );
        let appended = entry(
            start + 2_050,
            5,
            KindName::Stream,
            None,
            json!({ "sid": 3, "op": "append", "text": "{\"path\":\"Cargo.toml\",\"i\":\"Read the manifest\"}" }),
            Some(assistant.id),
        );
        // The kernel's ready update is where streamed arguments and the
        // intent settle.
        let ready = entry(
            start + 2_060,
            13,
            KindName::ToolUpdate,
            None,
            json!({ "kernel": "ready", "args": { "path": "Cargo.toml" }, "i": "Read the manifest" }),
            Some(call.id),
        );
        let end = entry(
            start + 2_080,
            6,
            KindName::MsgAssistantEnd,
            None,
            json!({ "stop_reason": "tool_calls" }),
            Some(assistant.id),
        );
        // The receipt's own timings are what the row shows.
        let receipt = entry(
            start + 2_100,
            7,
            KindName::TurnReceipt,
            None,
            json!({ "tokens_in": 7006, "tokens_out": 32, "cost_nano_usd": 0,
                    "ttft_ms": 2059, "last_token_ms": 2067, "duration_ms": 2078 }),
            Some(user.id),
        );
        let result = entry(
            start + 2_160,
            8,
            KindName::ToolResult,
            None,
            json!({ "outcome": { "kind": "call_outcome", "value": "[package]" },
                    "prompt_parts": [{ "kind": "text", "text": "[package]" }] }),
            Some(call.id),
        );
        let second = entry(
            start + 2_200,
            9,
            KindName::MsgAssistantStart,
            None,
            json!({ "model": "openai-codex/gpt-5.6-sol", "provider": "openai-codex", "route": "r" }),
            Some(user.id),
        );
        let opened = entry(
            start + 5_490,
            14,
            KindName::Stream,
            None,
            json!({ "sid": 4, "op": "open", "prop": "text" }),
            Some(second.id),
        );
        let text = entry(
            start + 5_500,
            10,
            KindName::Stream,
            None,
            json!({ "sid": 4, "op": "append", "text": "omperor" }),
            Some(second.id),
        );
        let second_end = entry(
            start + 5_600,
            11,
            KindName::MsgAssistantEnd,
            None,
            json!({ "stop_reason": "stop" }),
            Some(second.id),
        );
        let second_receipt = entry(
            start + 5_640,
            12,
            KindName::TurnReceipt,
            None,
            json!({ "tokens_in": 8260, "tokens_out": 25, "cost_nano_usd": 0,
                    "ttft_ms": 3357, "last_token_ms": 3428, "duration_ms": 3440 }),
            Some(user.id),
        );
        let entries = vec![
            turn,
            user,
            assistant,
            call,
            appended,
            ready,
            end,
            receipt,
            result,
            second,
            opened,
            text,
            second_end,
            second_receipt,
        ];
        let timeline = fold(&entries);
        assert_eq!(timeline.origin_ms, start + 10);
        let kinds: Vec<Kind> = timeline.events.iter().map(|event| event.kind).collect();
        assert_eq!(kinds, [Kind::Prompt, Kind::Response, Kind::Tool, Kind::Response]);
        let prompt = &timeline.events[0];
        assert_eq!(prompt.detail, "Read Cargo.toml and tell me the name");
        assert_eq!(prompt.duration_ms(), 0);
        let first = &timeline.events[1];
        assert_eq!(first.label, "gpt-5.6-sol");
        assert_eq!(first.started_ms, start + 2_100 - 2_078);
        assert_eq!(first.duration_ms(), 2_078);
        assert_eq!(first.waiting_ms, 2_059);
        // The first response landed in one flush 2.1 s in, so its rate is over
        // the whole turn and says so.
        assert_eq!(first.detail, "TTFT 2.1 s · 32 tokens · 15 tok/s · one flush");
        // Its sheet: the facts the receipt and the start entry carry, and
        // the call it made.
        let fact = |event: &Event, key: &str| {
            event
                .info
                .facts
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(fact(first, "Provider").as_deref(), Some("openai-codex"));
        assert_eq!(fact(first, "Tokens in").as_deref(), Some("7006"));
        assert_eq!(fact(first, "First token").as_deref(), Some("after 2.1 s"));
        assert_eq!(fact(first, "Stopped").as_deref(), Some("tool_calls"));
        assert_eq!(fact(first, "Called").as_deref(), Some("read"));
        assert_eq!(fact(prompt, "Length").as_deref(), Some("38 characters"));
        assert_eq!(
            prompt.info.sections,
            [("Prompt".to_owned(), "Read   Cargo.toml\nand tell me the name".to_owned())]
        );
        let tool = &timeline.events[2];
        assert_eq!(tool.label, "read");
        assert_eq!(tool.detail, "Read the manifest");
        assert_eq!(tool.started_ms, start + 2_100, "runs once its response settled");
        assert_eq!(tool.duration_ms(), 60);
        assert!(!tool.failed);
        assert_eq!(fact(tool, "Call").as_deref(), Some("c1"));
        assert_eq!(fact(tool, "Called by").as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(fact(tool, "Outcome").as_deref(), Some("ok"));
        assert_eq!(fact(tool, "Took").as_deref(), Some("60 ms"));
        assert_eq!(
            tool.info.sections,
            [
                ("Arguments".to_owned(), "{\n  \"path\": \"Cargo.toml\"\n}".to_owned()),
                ("Result".to_owned(), "[package]".to_owned()),
            ]
        );
        let second = &timeline.events[3];
        assert_eq!(second.waiting_ms, 3_357);
        assert_eq!(second.detail, "TTFT 3.4 s · 25 tokens · 352 tok/s");
        assert_eq!(fact(second, "Throughput").as_deref(), Some("352 tokens a second"));
        assert_eq!(
            second.info.sections,
            [("Reply".to_owned(), "omperor".to_owned())],
            "the streamed text is the reply"
        );
        assert_eq!(timeline.span_ms(), 5_640 - 10);
        assert_eq!(timeline.longest_ms(), 3_440);
    }

    #[test]
    fn a_cut_off_response_and_a_failed_tool_still_show() {
        let start = 5_000;
        let user = entry(start, 1, KindName::MsgUser, None, json!({ "text": "go" }), None);
        let assistant = entry(
            start + 100,
            2,
            KindName::MsgAssistantStart,
            None,
            json!({ "model": "claude-opus-5", "provider": "anthropic", "route": "r" }),
            Some(user.id),
        );
        let call = entry(
            start + 400,
            3,
            KindName::ToolCall,
            None,
            json!({ "name": "bash", "rev": 1, "call_id": "c1", "args": { "command": "make test" } }),
            Some(assistant.id),
        );
        let result = entry(
            start + 900,
            4,
            KindName::ToolResult,
            None,
            json!({ "fault": { "kind": "faulted", "value": { "message": "exit 2" } } }),
            Some(call.id),
        );
        let notice = entry(
            start + 950,
            5,
            KindName::Patch,
            Some("kernel.notice"),
            json!({ "ops": [["ins", 4, null, { "tag": "notice", "props": [["kind", "error"]], "content": "boom" }]] }),
            Some(user.id),
        );
        let timeline = fold(&[user, assistant, call, result, notice]);
        let kinds: Vec<Kind> = timeline.events.iter().map(|event| event.kind).collect();
        assert_eq!(kinds, [Kind::Prompt, Kind::Response, Kind::Tool, Kind::Notice]);
        let response = &timeline.events[1];
        assert_eq!(response.label, "claude-opus-5");
        assert_eq!(response.detail, "no receipt");
        assert!(response.info.facts.iter().any(|(name, value)| name == "Receipt" && value.starts_with("none")));
        assert_eq!(timeline.events[3].info.sections, [("Reason".to_owned(), "boom".to_owned())]);
        assert_eq!(response.duration_ms(), 0, "nothing streamed, nothing settled");
        let tool = &timeline.events[2];
        assert_eq!(tool.detail, "make test");
        assert!(tool.failed);
        assert_eq!(tool.started_ms, start + 400, "no receipt, so from the call");
        assert_eq!(timeline.events[3].detail, "boom");
    }

    #[test]
    fn a_hub_wait_names_its_jobs_and_the_sub_agents_fold_in_beneath_it() {
        let start = 10_000;
        let user = entry(start, 1, KindName::MsgUser, None, json!({ "text": "go" }), None);
        let spawn = entry(
            start + 100,
            2,
            KindName::Patch,
            Some("jobs.insert"),
            json!({ "ops": [["ins", 5, null, {
                "tag": "subagent",
                "props": [["id", "counter-one"], ["kind", "subagent"], ["status", "running"]]
            }]] }),
            Some(user.id),
        );
        let assistant = entry(
            start + 200,
            3,
            KindName::MsgAssistantStart,
            None,
            json!({ "model": "gpt-6-astra", "provider": "openai-codex", "route": "r" }),
            Some(user.id),
        );
        let call = entry(
            start + 300,
            4,
            KindName::ToolCall,
            None,
            json!({ "name": "hub", "rev": 2, "call_id": "c1", "sid": 2 }),
            Some(assistant.id),
        );
        let args = entry(
            start + 310,
            5,
            KindName::Stream,
            None,
            json!({ "sid": 2, "op": "append", "text": "{\"op\":\"wait\",\"ids\":[\"counter-one\"],\"i\":\"Waiting for the counter\"}" }),
            Some(assistant.id),
        );
        let ready = entry(
            start + 320,
            6,
            KindName::ToolUpdate,
            None,
            json!({ "kernel": "ready", "args": { "op": "wait", "ids": ["counter-one"], "i": "Waiting for the counter" }, "i": "Waiting for the counter" }),
            Some(call.id),
        );
        let receipt = entry(
            start + 400,
            7,
            KindName::TurnReceipt,
            None,
            json!({ "tokens_in": 1, "tokens_out": 1, "cost_nano_usd": 0, "duration_ms": 200 }),
            Some(user.id),
        );
        let result = entry(
            start + 60_400,
            8,
            KindName::ToolResult,
            None,
            json!({ "outcome": { "kind": "call_outcome", "value": "done" } }),
            Some(call.id),
        );
        // The sub-agent's own journal: its prompt, one response, one bash.
        let child_user = entry(
            start + 1_000,
            1,
            KindName::MsgUser,
            None,
            json!({ "text": "Count to 60" }),
            None,
        );
        let child_assistant = entry(
            start + 1_100,
            2,
            KindName::MsgAssistantStart,
            None,
            json!({ "model": "gpt-6-astra", "provider": "openai-codex", "route": "r" }),
            Some(child_user.id),
        );
        let child_call = entry(
            start + 1_200,
            3,
            KindName::ToolCall,
            None,
            json!({ "name": "bash", "rev": 1, "call_id": "b1", "args": { "command": "for i in $(seq 60); do sleep 1; done" } }),
            Some(child_assistant.id),
        );
        let child_receipt = entry(
            start + 1_300,
            4,
            KindName::TurnReceipt,
            None,
            json!({ "tokens_in": 1, "tokens_out": 1, "cost_nano_usd": 0, "duration_ms": 200 }),
            Some(child_user.id),
        );
        let child_result = entry(
            start + 60_000,
            5,
            KindName::ToolResult,
            None,
            json!({ "outcome": { "kind": "call_outcome", "value": "60" } }),
            Some(child_call.id),
        );
        let timeline = fold_with_children(
            &[user, spawn, assistant, call, args, ready, receipt, result],
            &[(
                "counter-one".to_owned(),
                vec![child_user, child_assistant, child_call, child_receipt, child_result],
            )],
        );
        let rows: Vec<(Option<&str>, Kind, &str)> = timeline
            .events
            .iter()
            .map(|event| (event.agent.as_deref(), event.kind, event.label.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                (None, Kind::Prompt, "You"),
                (None, Kind::Response, "gpt-6-astra"),
                (None, Kind::Tool, "hub"),
                (Some("counter-one"), Kind::Prompt, "You"),
                (Some("counter-one"), Kind::Response, "gpt-6-astra"),
                (Some("counter-one"), Kind::Tool, "bash"),
            ]
        );
        let hub = &timeline.events[2];
        assert_eq!(hub.detail, "wait for counter-one · Waiting for the counter");
        assert_eq!(hub.duration_ms(), 60_000);
        assert_eq!(hub.model.as_deref(), Some("gpt-6-astra"), "a call takes its response's model");
        assert_eq!(timeline.events[0].model, None);
        assert_eq!(timeline.models(), ["gpt-6-astra"]);
        let bash = &timeline.events[5];
        assert_eq!(bash.detail, "for i in $(seq 60); do sleep 1; done");
        assert_eq!(bash.duration_ms(), 60_000 - 1_300);
        assert_eq!(timeline.origin_ms, start);

        assert_eq!(
            tool_detail("task", None, Some(r#"{"tasks":[{"name":"a","task":"x"},{"name":"b","task":"y"}]}"#)),
            "spawn a, b"
        );
        assert_eq!(tool_detail("hub", None, Some(r#"{"op":"jobs"}"#)), "jobs");
        assert_eq!(
            tool_detail("hub", Some("Stop it"), Some(r#"{"op":"stop","name":"server"}"#)),
            "stop server · Stop it"
        );
        assert_eq!(tool_detail("read", None, Some(r#"{"path":"a.rs"}"#)), "a.rs");
        assert_eq!(tool_detail("read", Some("Look"), Some(r#"{"path":"a.rs"}"#)), "Look");
    }


    #[test]
    fn filters_fold_what_they_take_out_into_runs_that_open_on_request() {
        let event = |kind, agent: Option<&str>, label: &str, detail: &str, started, ended| Event {
            kind,
            info: Info::default(),
            agent: agent.map(str::to_owned),
            model: match kind {
                Kind::Response => Some(label.to_owned()),
                Kind::Tool => Some(if agent.is_some() { "gpt-5.5" } else { "gpt-6-astra" }.to_owned()),
                _ => None,
            },
            started_ms: started,
            ended_ms: ended,
            waiting_ms: 0,
            label: label.to_owned(),
            detail: detail.to_owned(),
            failed: false,
            search: String::new(),
        };
        let events = vec![
            event(Kind::Prompt, None, "You", "Spawn two sub-agents", 0, 0),
            event(Kind::Response, None, "gpt-6-astra", "TTFT 8.9 s · 219 tokens", 0, 8_900),
            event(Kind::Tool, None, "task", "spawn counter-one", 8_900, 8_916),
            event(Kind::Tool, None, "hub", "wait for counter-one", 12_300, 242_300),
            event(Kind::Tool, Some("counter-one"), "bash", "Counting to 60", 66_000, 96_000),
            event(Kind::Response, None, "gpt-6-astra", "TTFT 8.4 s · 193 tokens", 242_400, 250_900),
        ];
        // The models used, and a filter down to one of them: its responses
        // and the calls they made, nothing else.
        let timeline = Timeline {
            origin_ms: 0,
            events: events.clone(),
        };
        assert_eq!(timeline.models(), ["gpt-6-astra", "gpt-5.5"]);
        let loaded = arrived(&events);
        let by_model = Filter {
            model: Some("gpt-5.5".into()),
            ..Filter::default()
        };
        assert!(by_model.is_active());
        let shown: Vec<usize> = rows(&loaded, &by_model, &Default::default())
            .iter()
            .filter_map(|row| match row {
                Row::Event { index, .. } => Some(*index),
                Row::Hidden { .. } => None,
            })
            .collect();
        assert_eq!(shown, [4]);
        let permissive = Filter::default();
        assert!(!permissive.is_active());
        assert_eq!(rows(&loaded, &permissive, &Default::default()).len(), 6);

        // Words match across every dimension, all of them, any case.
        let by_agent = Filter {
            query: "COUNTER-one bash".into(),
            ..Filter::default()
        };
        assert!(by_agent.is_active());
        let folded = rows(&loaded, &by_agent, &Default::default());
        assert_eq!(
            folded,
            [
                Row::Hidden {
                    first: 0,
                    count: 4,
                    total_ms: 8_900 + 16 + 230_000,
                    open: false
                },
                Row::Event {
                    index: 4,
                    dimmed: false
                },
                Row::Hidden {
                    first: 5,
                    count: 1,
                    total_ms: 8_500,
                    open: false
                },
            ]
        );
        // The offset and the duration are searchable too.
        let by_offset = Filter {
            query: "+0:12".into(),
            ..Filter::default()
        };
        assert!(by_offset.admits(&events[3], 0));
        assert!(!by_offset.admits(&events[2], 0));
        let by_duration = Filter {
            query: "3m 50s".into(),
            ..Filter::default()
        };
        assert!(by_duration.admits(&events[3], 0));

        // A minimum keeps the slow ones; a kind keeps its own.
        let slow = Filter {
            min_ms: 10_000,
            ..Filter::default()
        };
        let shown: Vec<usize> = rows(&loaded, &slow, &Default::default())
            .iter()
            .filter_map(|row| match row {
                Row::Event { index, .. } => Some(*index),
                Row::Hidden { .. } => None,
            })
            .collect();
        assert_eq!(shown, [3, 4]);
        let tools = Filter {
            kind: Some(Kind::Tool),
            ..Filter::default()
        };
        assert_eq!(
            rows(&loaded, &tools, &Default::default())
                .iter()
                .filter(|row| matches!(row, Row::Event { .. }))
                .count(),
            3
        );

        // Opening a run shows its events, dimmed, under the bar. A run is
        // named by where it ends.
        let open: std::collections::HashSet<usize> = [Row::run_key(0, 4)].into_iter().collect();
        let opened = rows(&loaded, &by_agent, &open);
        assert_eq!(opened.len(), 3 + 4);
        assert!(matches!(opened[0], Row::Hidden { open: true, .. }));
        assert!(matches!(opened[1], Row::Event { index: 0, dimmed: true }));
        assert!(matches!(opened[5], Row::Event { index: 4, dimmed: false }));


        // Nearest rank: six durations, rank round(5 × 0.9) = 5, the longest.
        assert_eq!(percentile_ms(&events, 0.9), Some(230_000));
        assert_eq!(percentile_ms(&events, 0.5), Some(8_900));
        assert_eq!(percentile_ms(&[], 0.9), None);
        assert_eq!(parse_duration_ms(""), Some(0));
        assert_eq!(parse_duration_ms("2"), Some(2_000));
        assert_eq!(parse_duration_ms("2.5s"), Some(2_500));
        assert_eq!(parse_duration_ms("500ms"), Some(500));
        assert_eq!(parse_duration_ms("1m30s"), Some(90_000));
        assert_eq!(parse_duration_ms("1:30"), Some(90_000));
        assert_eq!(parse_duration_ms("1 min"), Some(60_000));
        assert_eq!(parse_duration_ms("fast"), None);
        assert_eq!(offset_text(3_723_400), "1:02:03");
    }

    /// A moment at `at` whose label says which it is, as a fold makes it.
    fn moment(at: u64, agent: Option<&str>) -> Event {
        let mut event = Event {
            kind: Kind::Prompt,
            info: Info::default(),
            agent: agent.map(str::to_owned),
            model: None,
            started_ms: at,
            ended_ms: at,
            waiting_ms: 0,
            label: format!("event {at}"),
            detail: String::new(),
            failed: false,
            search: String::new(),
        };
        event.search = event.haystack(0);
        event
    }

    #[test]
    fn the_stream_sends_the_newest_events_first_then_pages_back_to_the_start() {
        let start = 1_000_000;
        let mut entries = vec![entry(
            start - 1,
            0,
            KindName::Journal,
            None,
            json!({ "version": 1, "cwd": "/tmp/project", "created": "2026-09-22T00:00:00Z" }),
            None,
        )];
        for turn in 0..600u32 {
            let by = entries.last().map(|entry: &Entry| entry.id);
            entries.push(entry(
                start + u64::from(turn) * 10,
                turn + 1,
                KindName::MsgUser,
                None,
                json!({ "text": format!("prompt {turn}") }),
                by,
            ));
        }
        let directory = std::env::temp_dir().join(format!(
            "omperor-timeline-stream-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("session.oms");
        let mut bytes = Vec::new();
        for entry in &entries {
            omp_journal::sse::encode(entry, &mut bytes).expect("encode");
        }
        std::fs::write(&path, bytes).expect("write journal");

        let (sender, receiver) = flume::bounded(1);
        let wakes = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                stream(&path, |update| {
                    let sent = sender.send(update).is_ok();
                    wakes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    sent
                });
                drop(sender);
            });
            let Update::Newest { summary, events } = receiver.recv().expect("first page") else {
                panic!("the first page carries the newest events");
            };
            assert_eq!(summary.events, 600);
            assert_eq!(summary.origin_ms, start);
            assert_eq!(events.len(), FIRST_PAGE_EVENTS);
            assert_eq!(events.last().map(|event| event.detail.as_str()), Some("prompt 599"));
            let mut loaded = Loaded::new(summary, events);
            assert_eq!(loaded.get(599).map(|event| event.detail.as_str()), Some("prompt 599"));
            assert!(loaded.get(0).is_none(), "the oldest have not arrived");
            let mut pages = 0;
            loop {
                match receiver.recv().expect("an update") {
                    Update::Older(older) => {
                        assert!(older.len() <= PAGE_EVENTS);
                        loaded.prepend(older);
                        pages += 1;
                    }
                    Update::Complete => break,
                    other => panic!("unexpected {other:?}"),
                }
            }
            assert_eq!(pages, 3, "552 older events: 256, 256, then the last 40");
            assert_eq!(loaded.len(), 600);
            assert_eq!(loaded.get(0).map(|event| event.detail.as_str()), Some("prompt 0"));
            assert_eq!(loaded.get(599).map(|event| event.detail.as_str()), Some("prompt 599"));
            assert!(
                loaded.get(150).is_some_and(|event| event.search.contains("+0:01.5")),
                "search text is set against the session's start"
            );
        });
        assert_eq!(wakes.into_inner(), 5, "one send per update");

        // A reader that goes away stops the worker at its next page.
        let (sender, receiver) = flume::bounded(1);
        drop(receiver);
        stream(&path, |update| sender.send(update).is_ok());
        let (sender, receiver) = flume::bounded(1);
        stream(&directory.join("missing.oms"), |update| sender.send(update).is_ok());
        assert!(matches!(receiver.try_recv(), Ok(Update::Failed(_))));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn indices_and_opened_runs_hold_while_older_events_arrive() {
        // The session: ten moments, the sub-agent's at 3..=6; the filter is
        // looking for the sub-agent, so everything else folds into runs.
        let all: Vec<Event> = (0..10)
            .map(|at| moment(at, (3..=6).contains(&at).then_some("helper")))
            .collect();
        let summary = Summary {
            events: all.len(),
            ..Summary::default()
        };
        let mut loaded = Loaded::new(summary, all[8..].to_vec());
        let filter = Filter {
            query: "helper".into(),
            ..Filter::default()
        };
        assert_eq!(
            rows(&loaded, &filter, &Default::default()),
            [Row::Hidden { first: 8, count: 2, total_ms: 0, open: false }]
        );
        // The user opens the newest run; an older page that lengthens it at
        // its front leaves it open.
        let open: std::collections::HashSet<usize> = [Row::run_key(8, 2)].into_iter().collect();
        loaded.prepend(all[4..8].to_vec());
        assert_eq!(
            rows(&loaded, &filter, &open),
            [
                Row::Event { index: 4, dimmed: false },
                Row::Event { index: 5, dimmed: false },
                Row::Event { index: 6, dimmed: false },
                Row::Hidden { first: 7, count: 3, total_ms: 0, open: true },
                Row::Event { index: 7, dimmed: true },
                Row::Event { index: 8, dimmed: true },
                Row::Event { index: 9, dimmed: true },
            ],
        );
        loaded.prepend(all[..4].to_vec());
        let rows = rows(&loaded, &filter, &open);
        assert_eq!(rows[0], Row::Hidden { first: 0, count: 3, total_ms: 0, open: false });
        assert_eq!(rows[5], Row::Hidden { first: 7, count: 3, total_ms: 0, open: true });
        assert_eq!(rows[6], Row::Event { index: 7, dimmed: true });
        assert_eq!(loaded.get(4).map(|event| event.started_ms), Some(4));
    }


    #[test]
    fn argument_excerpts_pick_the_telling_field() {
        assert_eq!(args_excerpt(r#"{"path":"src/main.rs","limit":20}"#), "src/main.rs");
        assert_eq!(args_excerpt(r#"{"command":"cargo test"}"#), "cargo test");
        assert_eq!(args_excerpt(r#"{"other":1}"#), r#"{"other":1}"#);
        assert_eq!(args_excerpt("not json"), "not json");
        assert_eq!(excerpt("a  b\n c", 10), "a b c");
        assert_eq!(excerpt("abcdefghijk", 5), "abcde…");
        assert_eq!(short_model("openai-codex/gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(pretty_json(r#"{"a":1}"#), "{\n  \"a\": 1\n}");
        assert_eq!(pretty_json("plain"), "plain");
    }
}

