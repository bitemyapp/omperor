//! Inference and tool timing, recorded beside the sessions.
//!
//! Every settled inference response, every tool call, and every turn is one
//! line of JSON appended to `metrics.jsonl` in OMP's sessions directory. The
//! token counts are the provider's own (from the receipt OMP journals), the
//! inference timings are OMP's clock at the point it consumes the stream, and
//! the tool and turn spans are measured here as their events arrive. Zero and
//! absent fields are left out, so a line is only as long as what it knows.
//!
//! Output throughput follows the conventional definition: output tokens
//! (reasoning included, as every provider counts them) over the span from the
//! first streamed token to the last, so it matches what the providers' own
//! dashboards and the public benchmarks report. The one departure is honest
//! rather than flattering: when a provider counts reasoning tokens but never
//! streamed any reasoning, those tokens were produced before the first token
//! arrived and are left out of the rate instead of being credited to the
//! visible span.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};

/// The log's file name, beside the `.oms` journals.
pub(crate) const FILE_NAME: &str = "metrics.jsonl";

/// One settled inference response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Inference {
    pub(crate) usage: crate::usage::Evidence,
    /// Unix milliseconds when the response settled.
    pub(crate) at_ms: u64,
    pub(crate) session: String,
    /// The turn the response belongs to, named by the unix milliseconds it
    /// started at, which stays unique across relaunches of a session.
    pub(crate) turn_ms: u64,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) tokens_in: u64,
    /// Output tokens as the provider counts them, reasoning included.
    pub(crate) tokens_out: u64,
    pub(crate) tokens_reasoning: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
    /// Request start to the first streamed token.
    pub(crate) ttft_ms: Option<u64>,
    /// Request start to the last streamed token.
    pub(crate) last_token_ms: Option<u64>,
    /// Request start to completion.
    pub(crate) duration_ms: u64,
    /// Whether the request asked for OpenAI's priority tier.
    pub(crate) fast: bool,
    /// The provider counted reasoning tokens but streamed none of the
    /// reasoning: it all happened before the first token arrived, so those
    /// tokens are left out of the rate rather than credited to the visible
    /// span.
    pub(crate) reasoning_hidden: bool,
}

impl Inference {
    /// The span the provider spent producing tokens: first token to last.
    pub(crate) fn decode_ms(&self) -> Option<u64> {
        let (first, last) = (self.ttft_ms?, self.last_token_ms?);
        (last >= first).then_some(last - first)
    }

    /// The output tokens the decode span accounts for: every one the
    /// provider counted, unless the reasoning never streamed.
    pub(crate) fn timed_tokens(&self) -> u64 {
        if self.reasoning_hidden {
            self.tokens_out.saturating_sub(self.tokens_reasoning)
        } else {
            self.tokens_out
        }
    }

    /// Whether the provider delivered the response in one flush: a decode
    /// span too short to have streamed anything (under 50 ms) at the end of
    /// a request that itself took long enough to time. Codex holds a reply
    /// through its reasoning and sends it whole, so first-to-last-token
    /// spans a few milliseconds on nearly every turn.
    pub(crate) fn flushed_whole(&self) -> bool {
        matches!(self.decode_ms(), Some(decode) if decode < 50)
            && self.last_token_ms.is_some_and(|last| last >= 50)
    }

    /// Output tokens per second. Over the decode span when the tokens
    /// streamed; over the whole generation — request to last token, every
    /// token counted, since the reasoning happened inside it — when they
    /// arrived in one flush, which is the rate the provider actually
    /// produced them at. A response too short to time either way has no
    /// rate rather than an absurd one.
    pub(crate) fn tokens_per_second(&self) -> Option<f64> {
        let decode = self.decode_ms()?;
        if decode >= 50 {
            let tokens = self.timed_tokens();
            return (tokens >= 2).then(|| tokens as f64 * 1000.0 / decode as f64);
        }
        if !self.flushed_whole() || self.tokens_out < 2 {
            return None;
        }
        let whole = self.last_token_ms?;
        Some(self.tokens_out as f64 * 1000.0 / whole as f64)
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("k".into(), json!("inference"));
        if self.usage != crate::usage::Evidence::default() {
            object.insert("usage".into(), json!(self.usage));
        }
        object.insert("at".into(), json!(self.at_ms));
        object.insert("session".into(), json!(self.session));
        object.insert("turn".into(), json!(self.turn_ms));
        if let Some(provider) = &self.provider {
            object.insert("provider".into(), json!(provider));
        }
        if let Some(model) = &self.model {
            object.insert("model".into(), json!(model));
        }
        object.insert("tokens_in".into(), json!(self.tokens_in));
        object.insert("tokens_out".into(), json!(self.tokens_out));
        insert_nonzero(&mut object, "tokens_reasoning", self.tokens_reasoning);
        insert_nonzero(&mut object, "cache_read", self.cache_read);
        insert_nonzero(&mut object, "cache_write", self.cache_write);
        if let Some(ttft) = self.ttft_ms {
            object.insert("ttft_ms".into(), json!(ttft));
        }
        if let Some(last) = self.last_token_ms {
            object.insert("last_token_ms".into(), json!(last));
        }
        object.insert("duration_ms".into(), json!(self.duration_ms));
        if self.fast {
            object.insert("fast".into(), json!(true));
        }
        if self.reasoning_hidden {
            object.insert("reasoning_hidden".into(), json!(true));
        }
        Value::Object(object)
    }

    fn from_object(object: &Map<String, Value>) -> Option<Self> {
        Some(Self {
            usage: object.get("usage").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default(),
            at_ms: u64_field(object, "at")?,
            session: string_field(object, "session")?,
            turn_ms: u64_field(object, "turn").unwrap_or(0),
            provider: string_field(object, "provider"),
            model: string_field(object, "model"),
            tokens_in: u64_field(object, "tokens_in").unwrap_or(0),
            tokens_out: u64_field(object, "tokens_out").unwrap_or(0),
            tokens_reasoning: u64_field(object, "tokens_reasoning").unwrap_or(0),
            cache_read: u64_field(object, "cache_read").unwrap_or(0),
            cache_write: u64_field(object, "cache_write").unwrap_or(0),
            ttft_ms: u64_field(object, "ttft_ms"),
            last_token_ms: u64_field(object, "last_token_ms"),
            duration_ms: u64_field(object, "duration_ms").unwrap_or(0),
            fast: object.get("fast").and_then(Value::as_bool).unwrap_or(false),
            reasoning_hidden: object
                .get("reasoning_hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// One tool call the harness ran for the model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Tool {
    /// Unix milliseconds when the call finished.
    pub(crate) at_ms: u64,
    pub(crate) session: String,
    pub(crate) turn_ms: u64,
    pub(crate) name: String,
    pub(crate) duration_ms: u64,
    pub(crate) error: bool,
}

impl Tool {
    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("k".into(), json!("tool"));
        object.insert("at".into(), json!(self.at_ms));
        object.insert("session".into(), json!(self.session));
        object.insert("turn".into(), json!(self.turn_ms));
        object.insert("tool".into(), json!(self.name));
        object.insert("duration_ms".into(), json!(self.duration_ms));
        if self.error {
            object.insert("error".into(), json!(true));
        }
        Value::Object(object)
    }

    fn from_object(object: &Map<String, Value>) -> Option<Self> {
        Some(Self {
            at_ms: u64_field(object, "at")?,
            session: string_field(object, "session")?,
            turn_ms: u64_field(object, "turn").unwrap_or(0),
            name: string_field(object, "tool")?,
            duration_ms: u64_field(object, "duration_ms").unwrap_or(0),
            error: object.get("error").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// One turn, from the prompt leaving the composer to OMP handing control
/// back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Turn {
    /// Unix milliseconds when the turn ended.
    pub(crate) at_ms: u64,
    pub(crate) session: String,
    /// Unix milliseconds when it started; what its responses and tools
    /// name.
    pub(crate) turn_ms: u64,
    pub(crate) duration_ms: u64,
    /// `completed`, `errored`, or `interrupted`.
    pub(crate) outcome: String,
}

impl Turn {
    fn to_value(&self) -> Value {
        json!({
            "k": "turn",
            "at": self.at_ms,
            "session": self.session,
            "turn": self.turn_ms,
            "duration_ms": self.duration_ms,
            "outcome": self.outcome,
        })
    }

    fn from_object(object: &Map<String, Value>) -> Option<Self> {
        Some(Self {
            at_ms: u64_field(object, "at")?,
            session: string_field(object, "session")?,
            turn_ms: u64_field(object, "turn").unwrap_or(0),
            duration_ms: u64_field(object, "duration_ms").unwrap_or(0),
            outcome: string_field(object, "outcome").unwrap_or_else(|| "completed".to_owned()),
        })
    }
}

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Record {
    Inference(Inference),
    Tool(Tool),
    Turn(Turn),
}

impl Record {
    pub(crate) fn to_line(&self) -> String {
        let value = match self {
            Self::Inference(inference) => inference.to_value(),
            Self::Tool(tool) => tool.to_value(),
            Self::Turn(turn) => turn.to_value(),
        };
        value.to_string()
    }

    pub(crate) fn parse(line: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(line.trim()).ok()?;
        let object = value.as_object()?;
        match object.get("k").and_then(Value::as_str)? {
            "inference" => Inference::from_object(object).map(Self::Inference),
            "tool" => Tool::from_object(object).map(Self::Tool),
            "turn" => Turn::from_object(object).map(Self::Turn),
            _ => None,
        }
    }

    pub(crate) fn session(&self) -> &str {
        match self {
            Self::Inference(inference) => &inference.session,
            Self::Tool(tool) => &tool.session,
            Self::Turn(turn) => &turn.session,
        }
    }

    fn at_ms(&self) -> u64 {
        match self {
            Self::Inference(inference) => inference.at_ms,
            Self::Tool(tool) => tool.at_ms,
            Self::Turn(turn) => turn.at_ms,
        }
    }
}

fn insert_nonzero(object: &mut Map<String, Value>, key: &str, value: u64) {
    if value != 0 {
        object.insert(key.into(), json!(value));
    }
}

fn u64_field(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key).and_then(Value::as_u64)
}

fn string_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// What the log's thread is asked to do, in the order asked.
#[derive(Debug)]
enum Message {
    Record(Box<Record>),
    /// Take every line about these sessions out of the log, answering with
    /// the lines taken.
    Take {
        sessions: HashSet<String>,
        done: flume::Sender<std::io::Result<Vec<String>>>,
    },
    /// Put lines taken earlier back.
    Restore {
        lines: Vec<String>,
        done: flume::Sender<std::io::Result<()>>,
    },
}

/// Appends records on its own thread, so the interface never waits on the
/// disk. Records are written in the order they are sent.
#[derive(Debug)]
pub(crate) struct Log {
    sender: flume::Sender<Message>,
}

impl Log {
    pub(crate) fn open(path: PathBuf) -> Self {
        let (sender, receiver) = flume::unbounded::<Message>();
        std::thread::Builder::new()
            .name("metrics-log".to_owned())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Record(record) => {
                            if let Err(error) = append(&path, &record) {
                                eprintln!(
                                    "metrics: could not append to {}: {error}",
                                    path.display()
                                );
                            }
                        }
                        Message::Take { sessions, done } => {
                            let _ = done.send(take_sessions(&path, &sessions));
                        }
                        Message::Restore { lines, done } => {
                            let _ = done.send(append_lines(&path, &lines));
                        }
                    }
                }
            })
            .expect("metrics log thread must start");
        Self { sender }
    }

    pub(crate) fn record(&self, record: Record) {
        let _ = self.sender.send(Message::Record(Box::new(record)));
    }

    /// A handle a worker can use to take sessions' lines out of the log, or
    /// put them back, through the one thread that writes it.
    pub(crate) fn editor(&self) -> Editor {
        Editor {
            sender: self.sender.clone(),
        }
    }
}

/// Edits the log on its own thread, so an edit never races an append.
#[derive(Debug, Clone)]
pub(crate) struct Editor {
    sender: flume::Sender<Message>,
}

impl Editor {
    /// Removes every line about `sessions` and returns them. Waits for the
    /// log's thread, so it belongs on a worker.
    pub(crate) fn take(&self, sessions: HashSet<String>) -> std::io::Result<Vec<String>> {
        let (done, answer) = flume::bounded(1);
        self.sender
            .send(Message::Take { sessions, done })
            .map_err(|_| std::io::Error::other("the metrics log has stopped"))?;
        answer
            .recv()
            .map_err(|_| std::io::Error::other("the metrics log has stopped"))?
    }

    /// Appends lines taken by [`Editor::take`]. Waits like it.
    pub(crate) fn restore(&self, lines: Vec<String>) -> std::io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let (done, answer) = flume::bounded(1);
        self.sender
            .send(Message::Restore { lines, done })
            .map_err(|_| std::io::Error::other("the metrics log has stopped"))?;
        answer
            .recv()
            .map_err(|_| std::io::Error::other("the metrics log has stopped"))?
    }
}

/// The session a raw log line is about, without parsing the whole record.
fn line_session(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    value.get("session")?.as_str().map(str::to_owned)
}

/// The log's lines about `sessions`, read without changing the log.
/// Touches the filesystem; worker only.
pub(crate) fn lines_for(path: &Path, sessions: &HashSet<String>) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| line_session(line).is_some_and(|session| sessions.contains(&session)))
        .map(str::to_owned)
        .collect()
}

/// Rewrites the log without the lines about `sessions`, replacing it in one
/// rename, and returns the lines it left out.
fn take_sessions(path: &Path, sessions: &HashSet<String>) -> std::io::Result<Vec<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut kept = String::with_capacity(text.len());
    let mut taken = Vec::new();
    for line in text.lines() {
        if line_session(line).is_some_and(|session| sessions.contains(&session)) {
            taken.push(line.to_owned());
        } else {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    if taken.is_empty() {
        return Ok(taken);
    }
    let staged = path.with_extension("jsonl.tmp");
    std::fs::write(&staged, kept)?;
    std::fs::rename(&staged, path)?;
    Ok(taken)
}

fn append_lines(path: &Path, lines: &[String]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;
    let mut text = String::new();
    for line in lines {
        text.push_str(line.trim_end());
        text.push('\n');
    }
    file.write_all(text.as_bytes())
}

fn append(path: &Path, record: &Record) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;
    let mut line = record.to_line();
    line.push('\n');
    file.write_all(line.as_bytes())
}

/// Reads every record in the log, skipping lines it cannot read.
///
/// This touches the filesystem and belongs on a worker thread.
pub(crate) fn read(path: &Path) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(Record::parse).collect()
}

/// What one turn was spent on, from its records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TurnSplit {
    /// Turn wall time, from the prompt leaving to control returning.
    pub(crate) wall_ms: u64,
    /// Request start to first token, summed over the turn's responses.
    pub(crate) first_token_ms: u64,
    /// First token to completion, summed over the turn's responses.
    pub(crate) receiving_ms: u64,
    /// Time inside tool calls (overlapping calls counted once).
    pub(crate) tools_ms: u64,
    /// What is left of the wall time once inference and tools are taken out:
    /// prompt assembly, journaling, and everything else the harness does.
    pub(crate) harness_ms: u64,
}

/// One model's responses.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelStats {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) responses: usize,
    pub(crate) tokens_out: u64,
    /// Median output tokens per second across the responses that had one.
    pub(crate) median_tps: Option<f64>,
    pub(crate) p90_tps: Option<f64>,
    pub(crate) median_ttft_ms: Option<u64>,
    /// How many responses asked for the priority tier.
    pub(crate) fast_responses: usize,
}

/// One tool's calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolStats {
    pub(crate) name: String,
    pub(crate) calls: usize,
    pub(crate) total_ms: u64,
    pub(crate) median_ms: u64,
    pub(crate) errors: usize,
}

/// The Metrics page's numbers for one scope.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Summary {
    pub(crate) turns: usize,
    pub(crate) responses: usize,
    pub(crate) tool_calls: usize,
    pub(crate) split: TurnSplit,
    /// Models by responses, most first.
    pub(crate) models: Vec<ModelStats>,
    /// Tools by total time, longest first.
    pub(crate) tools: Vec<ToolStats>,
    /// The earliest and latest record in the scope.
    pub(crate) from_ms: Option<u64>,
    pub(crate) to_ms: Option<u64>,
}

/// One session the log knows: how many turns it ran and when it was last
/// heard from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionActivity {
    pub(crate) id: String,
    pub(crate) turns: usize,
    pub(crate) latest_ms: u64,
}

/// Every session in the log, newest activity first.
pub(crate) fn sessions(records: &[Record]) -> Vec<SessionActivity> {
    let mut by_id: BTreeMap<&str, SessionActivity> = BTreeMap::new();
    for record in records {
        let entry = by_id
            .entry(record.session())
            .or_insert_with(|| SessionActivity {
                id: record.session().to_owned(),
                turns: 0,
                latest_ms: 0,
            });
        entry.latest_ms = entry.latest_ms.max(record.at_ms());
        if matches!(record, Record::Turn(_)) {
            entry.turns += 1;
        }
    }
    let mut sessions: Vec<SessionActivity> = by_id.into_values().collect();
    sessions.sort_by(|a, b| b.latest_ms.cmp(&a.latest_ms).then(a.id.cmp(&b.id)));
    sessions
}

/// Folds the records for `session` (or every session when `None`).
pub(crate) fn summarize(records: &[Record], session: Option<&str>) -> Summary {
    let scoped: Vec<&Record> = records
        .iter()
        .filter(|record| session.is_none_or(|session| record.session() == session))
        .collect();
    let mut summary = Summary::default();
    // Each turn's own spans, keyed by session and turn start, so wall time
    // is compared with the work that happened inside it.
    let mut turns: BTreeMap<(&str, u64), (Option<&Turn>, Vec<&Inference>, Vec<&Tool>)> =
        BTreeMap::new();
    for record in &scoped {
        summary.from_ms = Some(summary.from_ms.map_or(record.at_ms(), |from| from.min(record.at_ms())));
        summary.to_ms = Some(summary.to_ms.map_or(record.at_ms(), |to| to.max(record.at_ms())));
        match record {
            Record::Inference(inference) => {
                turns
                    .entry((inference.session.as_str(), inference.turn_ms))
                    .or_default()
                    .1
                    .push(inference);
            }
            Record::Tool(tool) => {
                turns
                    .entry((tool.session.as_str(), tool.turn_ms))
                    .or_default()
                    .2
                    .push(tool);
            }
            Record::Turn(turn) => {
                turns
                    .entry((turn.session.as_str(), turn.turn_ms))
                    .or_default()
                    .0 = Some(turn);
            }
        }
    }
    let mut models: BTreeMap<(String, String), (Vec<f64>, Vec<u64>, u64, usize, usize)> =
        BTreeMap::new();
    let mut tools: BTreeMap<&str, (Vec<u64>, usize)> = BTreeMap::new();
    for (turn, inferences, calls) in turns.values() {
        summary.turns += usize::from(turn.is_some());
        summary.responses += inferences.len();
        summary.tool_calls += calls.len();
        let split = turn_split(*turn, inferences, calls);
        summary.split.wall_ms += split.wall_ms;
        summary.split.first_token_ms += split.first_token_ms;
        summary.split.receiving_ms += split.receiving_ms;
        summary.split.tools_ms += split.tools_ms;
        summary.split.harness_ms += split.harness_ms;
        for inference in inferences {
            let key = (
                inference.provider.clone().unwrap_or_default(),
                inference.model.clone().unwrap_or_else(|| "unknown".to_owned()),
            );
            let entry = models.entry(key).or_default();
            if let Some(tps) = inference.tokens_per_second() {
                entry.0.push(tps);
            }
            if let Some(ttft) = inference.ttft_ms {
                entry.1.push(ttft);
            }
            entry.2 += inference.tokens_out;
            entry.3 += 1;
            entry.4 += usize::from(inference.fast);
        }
        for call in calls {
            let entry = tools.entry(call.name.as_str()).or_default();
            entry.0.push(call.duration_ms);
            entry.1 += usize::from(call.error);
        }
    }
    summary.models = models
        .into_iter()
        .map(
            |((provider, model), (mut rates, mut ttfts, tokens_out, responses, fast))| {
                rates.sort_by(f64::total_cmp);
                ttfts.sort_unstable();
                ModelStats {
                    provider,
                    model,
                    responses,
                    tokens_out,
                    median_tps: percentile(&rates, 0.5),
                    p90_tps: percentile(&rates, 0.9),
                    median_ttft_ms: percentile_u64(&ttfts, 0.5),
                    fast_responses: fast,
                }
            },
        )
        .collect();
    summary
        .models
        .sort_by(|a, b| b.responses.cmp(&a.responses).then(a.model.cmp(&b.model)));
    summary.tools = tools
        .into_iter()
        .map(|(name, (mut durations, errors))| {
            durations.sort_unstable();
            ToolStats {
                name: name.to_owned(),
                calls: durations.len(),
                total_ms: durations.iter().sum(),
                median_ms: percentile_u64(&durations, 0.5).unwrap_or(0),
                errors,
            }
        })
        .collect();
    summary
        .tools
        .sort_by(|a, b| b.total_ms.cmp(&a.total_ms).then(a.name.cmp(&b.name)));
    summary
}

/// One turn's split. A turn whose end was never recorded (the app quit
/// first) contributes its responses and tools but no wall or harness time.
fn turn_split(turn: Option<&Turn>, inferences: &[&Inference], calls: &[&Tool]) -> TurnSplit {
    let mut split = TurnSplit::default();
    let mut busy: Vec<(u64, u64)> = Vec::new();
    for inference in inferences {
        let first = inference.ttft_ms.unwrap_or(inference.duration_ms);
        split.first_token_ms += first.min(inference.duration_ms);
        split.receiving_ms += inference.duration_ms.saturating_sub(first);
        busy.push((
            inference.at_ms.saturating_sub(inference.duration_ms),
            inference.at_ms,
        ));
    }
    let tool_spans: Vec<(u64, u64)> = calls
        .iter()
        .map(|call| (call.at_ms.saturating_sub(call.duration_ms), call.at_ms))
        .collect();
    split.tools_ms = union_ms(&tool_spans);
    busy.extend(tool_spans);
    if let Some(turn) = turn {
        split.wall_ms = turn.duration_ms;
        split.harness_ms = turn.duration_ms.saturating_sub(union_ms(&busy));
    }
    split
}

/// The length of the union of half-open spans, so overlapping work is not
/// counted twice.
fn union_ms(spans: &[(u64, u64)]) -> u64 {
    let mut spans: Vec<(u64, u64)> = spans.iter().copied().filter(|(a, b)| b > a).collect();
    spans.sort_unstable();
    let mut total = 0;
    let mut current: Option<(u64, u64)> = None;
    for (start, end) in spans {
        match current {
            Some((cur_start, cur_end)) if start <= cur_end => {
                current = Some((cur_start, cur_end.max(end)));
            }
            Some((cur_start, cur_end)) => {
                total += cur_end - cur_start;
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    if let Some((start, end)) = current {
        total += end - start;
    }
    total
}

fn percentile(sorted: &[f64], fraction: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted.get(index).copied()
}

fn percentile_u64(sorted: &[u64], fraction: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted.get(index).copied()
}

/// "820 ms", "4.2 s", "1m 12s", "2h 05m".
pub(crate) fn duration_text(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else if ms < 3_600_000 {
        format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1000)
    } else {
        format!("{}h {:02}m", ms / 3_600_000, (ms % 3_600_000) / 60_000)
    }
}

/// How long the live needle looks back: long enough to steady a bursty
/// stream, short enough to follow it.
pub(crate) const LIVE_WINDOW: Duration = Duration::from_millis(2000);

/// Characters per output token to assume until a response teaches better.
const DEFAULT_CHARS_PER_TOKEN: f64 = 4.0;

/// What the speedometer knows about the response in flight and the last one
/// that settled.
///
/// While tokens stream, the needle follows an estimate: characters over the
/// last [`LIVE_WINDOW`], divided by the characters per token the previous
/// response of this model taught. The moment OMP's receipt arrives the
/// needle snaps to the exact figure, provider token counts over the span
/// from the first token to the last.
#[derive(Debug, Clone)]
pub(crate) struct LiveRate {
    /// When the response in flight started streaming.
    streaming_since: Option<Instant>,
    /// (arrival, characters) of every delta inside the window.
    recent: VecDeque<(Instant, usize)>,
    text_chars: usize,
    /// Reasoning characters streamed for the response in flight; none means
    /// any reasoning the provider counts happened out of sight.
    thinking_chars: usize,
    chars_per_token: f64,
    /// Whether a response is in flight (from its start until its receipt).
    in_flight: bool,
    settled: Option<Settled>,
}

/// The last settled response's exact figures.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settled {
    pub(crate) tokens_per_second: Option<f64>,
    pub(crate) ttft_ms: Option<u64>,
    pub(crate) tokens_out: u64,
    pub(crate) model: Option<String>,
    /// The response came in one flush, so the rate is over the whole
    /// generation rather than a streamed span; the reader is told.
    pub(crate) flushed: bool,
}

/// What the gauge should show right now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Reading {
    /// Nothing has streamed yet in this session.
    Idle,
    /// A request is out and no token has arrived.
    Waiting,
    /// Tokens are streaming; the rate is an estimate.
    Streaming { estimate_tps: f64 },
    /// The last response's exact rate.
    Settled(Settled),
}

impl Default for LiveRate {
    fn default() -> Self {
        Self {
            streaming_since: None,
            recent: VecDeque::new(),
            text_chars: 0,
            thinking_chars: 0,
            chars_per_token: DEFAULT_CHARS_PER_TOKEN,
            in_flight: false,
            settled: None,
        }
    }
}

impl LiveRate {
    /// A response started: the needle waits for its first token.
    pub(crate) fn response_started(&mut self) {
        self.streaming_since = None;
        self.recent.clear();
        self.text_chars = 0;
        self.thinking_chars = 0;
        self.in_flight = true;
    }

    /// `chars` of assistant text arrived at `now`.
    pub(crate) fn delta(&mut self, chars: usize, now: Instant) {
        self.arrived(chars, now);
        self.text_chars += chars;
    }

    /// `chars` of streamed reasoning arrived at `now`.
    pub(crate) fn thinking_delta(&mut self, chars: usize, now: Instant) {
        self.arrived(chars, now);
        self.thinking_chars += chars;
    }

    fn arrived(&mut self, chars: usize, now: Instant) {
        if !self.in_flight {
            self.response_started();
        }
        self.streaming_since.get_or_insert(now);
        self.recent.push_back((now, chars));
        self.trim(now);
    }

    /// Whether the response in flight streamed any of its reasoning; what
    /// `Inference::reasoning_hidden` is decided from when the receipt
    /// counts reasoning tokens.
    pub(crate) fn streamed_reasoning(&self) -> bool {
        self.thinking_chars > 0
    }

    /// The receipt arrived: the exact rate replaces the estimate, and the
    /// response teaches how many characters a token of this model was.
    pub(crate) fn settle(&mut self, inference: &Inference) {
        let visible_tokens = inference
            .tokens_out
            .saturating_sub(inference.tokens_reasoning);
        if visible_tokens >= 20 && self.text_chars >= 80 {
            let learned = self.text_chars as f64 / visible_tokens as f64;
            self.chars_per_token = learned.clamp(1.5, 8.0);
        }
        self.settled = Some(Settled {
            tokens_per_second: inference.tokens_per_second(),
            ttft_ms: inference.ttft_ms,
            tokens_out: inference.tokens_out,
            model: inference.model.clone(),
            flushed: inference.flushed_whole(),
        });
        self.in_flight = false;
        self.streaming_since = None;
        self.recent.clear();
    }

    /// The turn ended without a receipt (an error, a stop): nothing is in
    /// flight any more.
    pub(crate) fn response_abandoned(&mut self) {
        self.in_flight = false;
        self.streaming_since = None;
        self.recent.clear();
    }

    pub(crate) fn reading(&mut self, now: Instant) -> Reading {
        if self.in_flight {
            self.trim(now);
            let Some(since) = self.streaming_since else {
                return Reading::Waiting;
            };
            let window_start = now.checked_sub(LIVE_WINDOW).unwrap_or(now).max(since);
            let seconds = now.duration_since(window_start).as_secs_f64().max(0.25);
            let chars: usize = self.recent.iter().map(|(_, chars)| *chars).sum();
            return Reading::Streaming {
                estimate_tps: chars as f64 / self.chars_per_token / seconds,
            };
        }
        match &self.settled {
            Some(settled) => Reading::Settled(settled.clone()),
            None => Reading::Idle,
        }
    }

    fn trim(&mut self, now: Instant) {
        while let Some((at, _)) = self.recent.front() {
            if now.duration_since(*at) > LIVE_WINDOW {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inference(session: &str, turn_ms: u64, at_ms: u64) -> Inference {
        Inference {
            at_ms,
            session: session.to_owned(),
            turn_ms,
            provider: Some("openai-codex".to_owned()),
            model: Some("gpt-5.6-sol".to_owned()),
            tokens_in: 1_000,
            tokens_out: 500,
            tokens_reasoning: 100,
            ttft_ms: Some(1_000),
            last_token_ms: Some(6_000),
            duration_ms: 6_100,
            ..Inference::default()
        }
    }

    #[test]
    fn an_editor_takes_sessions_lines_out_and_puts_them_back_through_the_log_thread() {
        let directory = std::env::temp_dir().join(format!(
            "omperor-metrics-editor-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(FILE_NAME);
        let log = Log::open(path.clone());
        for (session, at) in [("keep", 1), ("gone", 2), ("keep", 3), ("gone", 4)] {
            log.record(Record::Inference(inference(session, 0, at)));
        }
        let editor = log.editor();
        let gone = HashSet::from(["gone".to_owned()]);
        // The take is queued behind the appends, so it sees all four.
        let taken = editor.take(gone.clone()).unwrap();
        assert_eq!(taken.len(), 2);
        let left = read(&path);
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|record| record.session() == "keep"));
        assert!(lines_for(&path, &gone).is_empty());
        editor.restore(taken).unwrap();
        assert_eq!(lines_for(&path, &gone).len(), 2);
        assert_eq!(read(&path).len(), 4);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn throughput_is_output_tokens_over_the_span_between_first_and_last_token() {
        let response = inference("s", 0, 10_000);
        assert_eq!(response.decode_ms(), Some(5_000));
        assert_eq!(response.tokens_per_second(), Some(100.0));
        // One flush after 900 ms: the 500 tokens were produced over that
        // whole window, not the zero-length span they landed in.
        let single_chunk = Inference {
            ttft_ms: Some(900),
            last_token_ms: Some(900),
            ..response.clone()
        };
        assert!(single_chunk.flushed_whole());
        let rate = single_chunk.tokens_per_second().expect("a flush is timed over the turn");
        assert!((rate - 555.555).abs() < 0.01, "{rate}");
        // The exact Codex receipt the header showed "— tok/s" for: 594
        // tokens, first at 15 301 ms, last at 15 330 ms.
        let codex = Inference {
            tokens_out: 594,
            tokens_reasoning: 406,
            ttft_ms: Some(15_301),
            last_token_ms: Some(15_330),
            ..response.clone()
        };
        assert!(codex.flushed_whole());
        let rate = codex.tokens_per_second().expect("timed over the turn");
        assert!((rate - 38.747).abs() < 0.01, "{rate}");
        // A flush that itself took under 50 ms is still nothing to time.
        let instant = Inference {
            ttft_ms: Some(20),
            last_token_ms: Some(20),
            ..response.clone()
        };
        assert!(!instant.flushed_whole());
        assert_eq!(instant.tokens_per_second(), None);
        let untimed = Inference {
            last_token_ms: None,
            ..response.clone()
        };
        assert_eq!(untimed.tokens_per_second(), None);
        assert!(!untimed.flushed_whole());
        // Reasoning that never streamed happened before the first token, so
        // only the visible 400 tokens are credited to the span.
        let hidden = Inference {
            reasoning_hidden: true,
            ..response
        };
        assert_eq!(hidden.timed_tokens(), 400);
        assert_eq!(hidden.tokens_per_second(), Some(80.0));
    }

    #[test]
    fn a_record_round_trips_and_leaves_out_what_it_does_not_know() {
        let full = Record::Inference(Inference {
            usage: crate::usage::Evidence {
                receipt_id: Some("receipt-1".into()),
                recorded_at_ms: Some(10_000),
                usage_source: Some("provider".into()),
                cache_read_reported: Some(true),
                cache_write_reported: Some(false),
                cache_write_1h: Some(0),
                cost_nano_usd: Some(123_000),
                cost_source: Some("catalog_estimate".into()),
                ..Default::default()
            },
            fast: true,
            ..inference("s", 5, 10_000)
        });
        let line = full.to_line();
        assert!(line.contains(r#""k":"inference""#));
        assert!(line.contains(r#""fast":true"#));
        assert!(!line.contains("reasoning_hidden"));
        assert_eq!(Record::parse(&line), Some(full));
        let hidden = Record::Inference(Inference {
            reasoning_hidden: true,
            ..inference("s", 5, 10_000)
        });
        let line = hidden.to_line();
        assert!(line.contains(r#""reasoning_hidden":true"#));
        assert_eq!(Record::parse(&line), Some(hidden));

        let bare = Record::Inference(Inference {
            at_ms: 1,
            session: "s".into(),
            tokens_out: 3,
            duration_ms: 40,
            ..Inference::default()
        });
        let line = bare.to_line();
        assert!(!line.contains("tokens_reasoning"));
        assert!(!line.contains("cache_read"));
        assert!(!line.contains("ttft_ms"));
        assert!(!line.contains("fast"));
        assert!(!line.contains("provider"));
        assert_eq!(Record::parse(&line), Some(bare));

        let tool = Record::Tool(Tool {
            at_ms: 9,
            session: "s".into(),
            turn_ms: 5,
            name: "bash".into(),
            duration_ms: 1_500,
            error: false,
        });
        let line = tool.to_line();
        assert!(!line.contains("error"));
        assert_eq!(Record::parse(&line), Some(tool));
        let turn = Record::Turn(Turn {
            at_ms: 20,
            session: "s".into(),
            turn_ms: 5,
            duration_ms: 15,
            outcome: "completed".into(),
        });
        assert_eq!(Record::parse(&turn.to_line()), Some(turn));
        assert_eq!(Record::parse("not json"), None);
        assert_eq!(Record::parse(r#"{"k":"other","at":1}"#), None);
    }

    #[test]
    fn a_turn_is_split_into_waiting_receiving_tools_and_the_rest() {
        // A 20 s turn: two responses of 6.1 s (1 s waiting, 5.1 s receiving)
        // and a 4 s tool between them, leaving 3.8 s to the harness.
        let records = vec![
            Record::Inference(inference("s", 100_000, 106_100)),
            Record::Tool(Tool {
                at_ms: 110_500,
                session: "s".into(),
                turn_ms: 100_000,
                name: "bash".into(),
                duration_ms: 4_000,
                error: false,
            }),
            Record::Inference(inference("s", 100_000, 118_000)),
            Record::Turn(Turn {
                at_ms: 120_000,
                session: "s".into(),
                turn_ms: 100_000,
                duration_ms: 20_000,
                outcome: "completed".into(),
            }),
            // Another session's turn does not leak into a session scope.
            Record::Inference(inference("other", 1, 7_000)),
        ];
        let summary = summarize(&records, Some("s"));
        assert_eq!(summary.turns, 1);
        assert_eq!(summary.responses, 2);
        assert_eq!(summary.tool_calls, 1);
        assert_eq!(summary.split.wall_ms, 20_000);
        assert_eq!(summary.split.first_token_ms, 2_000);
        assert_eq!(summary.split.receiving_ms, 10_200);
        assert_eq!(summary.split.tools_ms, 4_000);
        assert_eq!(summary.split.harness_ms, 3_800);
        assert_eq!(summary.models.len(), 1);
        assert_eq!(summary.models[0].model, "gpt-5.6-sol");
        assert_eq!(summary.models[0].responses, 2);
        assert_eq!(summary.models[0].median_tps, Some(100.0));
        assert_eq!(summary.models[0].median_ttft_ms, Some(1_000));
        assert_eq!(summary.tools[0].name, "bash");
        assert_eq!(summary.tools[0].total_ms, 4_000);
        assert_eq!(summary.from_ms, Some(106_100));
        assert_eq!(summary.to_ms, Some(120_000));

        let everything = summarize(&records, None);
        assert_eq!(everything.responses, 3);
        assert_eq!(everything.turns, 1, "the other session's turn never ended");
    }

    #[test]
    fn the_sessions_the_log_knows_come_newest_first_with_their_turn_counts() {
        let records = vec![
            Record::Inference(inference("old", 1, 5_000)),
            Record::Turn(Turn {
                at_ms: 6_000,
                session: "old".into(),
                turn_ms: 1,
                duration_ms: 5,
                outcome: "completed".into(),
            }),
            Record::Turn(Turn {
                at_ms: 9_000,
                session: "new".into(),
                turn_ms: 2,
                duration_ms: 5,
                outcome: "completed".into(),
            }),
            Record::Turn(Turn {
                at_ms: 9_500,
                session: "new".into(),
                turn_ms: 3,
                duration_ms: 5,
                outcome: "errored".into(),
            }),
        ];
        let sessions = sessions(&records);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "new");
        assert_eq!(sessions[0].turns, 2);
        assert_eq!(sessions[0].latest_ms, 9_500);
        assert_eq!(sessions[1].id, "old");
        assert_eq!(sessions[1].turns, 1);
    }

    #[test]
    fn overlapping_tool_calls_are_counted_once() {
        assert_eq!(union_ms(&[(0, 10), (5, 15), (20, 25)]), 20);
        assert_eq!(union_ms(&[(5, 5), (3, 1)]), 0);
    }

    #[test]
    fn the_needle_estimates_while_streaming_and_snaps_to_the_receipt() {
        let mut live = LiveRate::default();
        let start = Instant::now();
        assert_eq!(live.reading(start), Reading::Idle);
        live.response_started();
        assert_eq!(live.reading(start), Reading::Waiting);
        // 400 characters over one second at four characters a token: ~100
        // tokens a second.
        live.delta(200, start);
        live.delta(200, start + Duration::from_millis(1000));
        match live.reading(start + Duration::from_millis(1000)) {
            Reading::Streaming { estimate_tps } => {
                assert!((estimate_tps - 100.0).abs() < 1.0, "{estimate_tps}");
            }
            other => panic!("expected an estimate, got {other:?}"),
        }
        let response = Inference {
            tokens_out: 200,
            tokens_reasoning: 0,
            ..inference("s", 0, 1)
        };
        live.settle(&response);
        assert_eq!(
            live.reading(start + Duration::from_secs(2)),
            Reading::Settled(Settled {
                tokens_per_second: Some(40.0),
                ttft_ms: Some(1_000),
                tokens_out: 200,
                model: Some("gpt-5.6-sol".to_owned()),
            
                flushed: false,
            })
        );
        // 400 characters were 200 visible tokens: two characters a token
        // from now on, so the next estimate doubles for the same text.
        assert!((live.chars_per_token - 2.0).abs() < f64::EPSILON);
        live.response_started();
        live.delta(400, start + Duration::from_secs(3));
        match live.reading(start + Duration::from_secs(4)) {
            Reading::Streaming { estimate_tps } => {
                assert!((estimate_tps - 200.0).abs() < 1.0, "{estimate_tps}");
            }
            other => panic!("expected an estimate, got {other:?}"),
        }
        live.response_abandoned();
        assert!(matches!(live.reading(start), Reading::Settled(_)));
        // Reasoning that streams is counted in the estimate, and says so.
        live.response_started();
        assert!(!live.streamed_reasoning());
        live.thinking_delta(40, start + Duration::from_secs(5));
        assert!(live.streamed_reasoning());
    }
}
