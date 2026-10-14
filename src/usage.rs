//! Provider usage is a ledger of completed requests, not a context-window meter.
//! Read and aggregate journals on the job pool; never scan them on AppKit.
use crate::{
    cache_policy::{CacheAction, CacheClock, CacheObservation, CachePolicy},
    omp_runtime::InferenceReceipt,
};
use omp_journal::{Journal, kind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Evidence {
    pub receipt_id: Option<String>,
    pub recorded_at_ms: Option<u64>,
    pub usage_source: Option<String>,
    pub cost_source: Option<String>,
    pub cost_nano_usd: Option<u64>,
    pub cache_write_1h: Option<u64>,
    pub cache_read_reported: Option<bool>,
    pub cache_write_reported: Option<bool>,
    pub cache_write_1h_reported: Option<bool>,
    pub prompt_cache_ttl_seconds: Option<u64>,
    pub prompt_cache_retention: Option<String>,
    pub response_completed_at_ms: Option<u64>,
}

impl From<&InferenceReceipt> for Evidence {
    fn from(r: &InferenceReceipt) -> Self {
        Self {
            receipt_id: r.receipt_id.clone(),
            recorded_at_ms: r.recorded_at_ms,
            usage_source: r.usage_source.clone(),
            cost_source: r.cost_source.clone(),
            cost_nano_usd: r.cost_nano_usd,
            cache_write_1h: r.cache_write_1h,
            cache_read_reported: r.cache_read_reported,
            cache_write_reported: r.cache_write_reported,
            cache_write_1h_reported: r.cache_write_1h_reported,
            prompt_cache_ttl_seconds: r.prompt_cache_ttl_seconds,
            prompt_cache_retention: r.prompt_cache_retention.clone(),
            response_completed_at_ms: r.response_completed_at_ms,
        }
    }
}

#[derive(Debug, Clone)]
struct Receipt {
    id: String,
    session: String,
    model: String,
    at_ms: u64,
    request_started_ms: Option<u64>,
    input: u64,
    output: u64,
    read: u64,
    write: u64,
    evidence: Evidence,
}

impl Receipt {
    fn from_value(v: &Value, id: String, session: String, at_ms: u64) -> Option<Self> {
        // Ignore malformed records instead of manufacturing a zero-token request.
        let input = v.get("tokens_in")?.as_u64()?;
        let output = v.get("tokens_out")?.as_u64()?;
        let mut evidence: Evidence = serde_json::from_value(v.clone()).unwrap_or_default();
        // Older journals contain a catalog estimate but predate cost_source.
        if evidence.cost_nano_usd.is_some() && evidence.cost_source.is_none() {
            evidence.cost_source = Some("catalog_estimate".into());
        }
        let at_ms = evidence.recorded_at_ms.unwrap_or(at_ms);
        let request_started_ms = v
            .get("duration_ms")
            .and_then(Value::as_u64)
            .and_then(|duration| at_ms.checked_sub(duration));
        Some(Self {
            id: evidence.receipt_id.clone().unwrap_or(id),
            session,
            model: v
                .pointer("/identity/model")
                .and_then(Value::as_str)
                .unwrap_or("Unknown model")
                .into(),
            at_ms,
            request_started_ms,
            input,
            output,
            read: v.get("cache_read").and_then(Value::as_u64).unwrap_or(0),
            write: v.get("cache_write").and_then(Value::as_u64).unwrap_or(0),
            evidence,
        })
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Totals {
    requests: u64,
    input: u64,
    output: u64,
    read: u64,
    write: u64,
    write_1h: u64,
    reported: u64,
    estimated: u64,
    unknown: u64,
    known_reads: u64,
    known_writes: u64,
    cost: u64,
    cost_requests: u64,
}

impl Totals {
    fn add(&mut self, r: &Receipt) {
        self.requests += 1;
        self.input = self.input.saturating_add(r.input);
        self.output = self.output.saturating_add(r.output);
        self.read = self.read.saturating_add(r.read);
        self.write = self.write.saturating_add(r.write);
        self.write_1h = self
            .write_1h
            .saturating_add(r.evidence.cache_write_1h.unwrap_or(0));
        match r.evidence.usage_source.as_deref() {
            Some("provider") => self.reported += 1,
            Some("estimated" | "mixed" | "measured") => self.estimated += 1,
            _ => self.unknown += 1,
        }
        self.known_reads += u64::from(r.evidence.cache_read_reported == Some(true) || r.read > 0);
        self.known_writes +=
            u64::from(r.evidence.cache_write_reported == Some(true) || r.write > 0);
        if let Some(cost) = r.evidence.cost_nano_usd
            && r.evidence.cost_source.as_deref() == Some("catalog_estimate")
        {
            self.cost = self.cost.saturating_add(cost);
            self.cost_requests += 1;
        }
    }
    fn total_input(&self) -> u64 {
        self.input
            .saturating_add(self.read)
            .saturating_add(self.write)
    }
}

/// Presentation data is prepared with the ledger on a worker. AppKit only lays
/// out a bounded page of native views; it never parses a text report.
#[derive(Debug, Clone)]
pub(crate) struct Metric {
    pub title: &'static str,
    pub value: String,
    pub compact: String,
    pub caption: String,
    pub tooltip: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Section {
    pub title: String,
    pub note: String,
    pub columns: Vec<String>,
    pub weights: Vec<f64>,
    pub rows: Vec<Vec<String>>,
    pub tooltips: Vec<String>,
}
impl Section {
    fn new(title: &str, note: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            note: note.into(),
            columns: vec![],
            weights: vec![],
            rows: vec![],
            tooltips: vec![],
        }
    }
    fn table(mut self, columns: &[&str], weights: &[f64], rows: Vec<Vec<String>>) -> Self {
        self.columns = columns.iter().map(|s| (*s).into()).collect();
        self.weights = weights.to_vec();
        self.rows = rows;
        self
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    /// Latest root response for the selected model; computed off AppKit.
    pub cache_observation: Option<CacheObservation>,
    /// The `/usage` card's figures, from the same receipts.
    pub card: Card,
    pub metrics: [Metric; 4],
    pub scope: String,
    pub composition: [u64; 3],
    pub pages: [Vec<Section>; 3],
}

/// Committed receipts on ALL branches count as spend. A fork may copy receipts;
/// globally stable receipt IDs (or legacy entry IDs) prevent double counting.
/// Discovery follows only recorded child IDs, never an unbounded directory walk.
pub(crate) fn read(path: Option<PathBuf>, children: Vec<PathBuf>, model: String) -> Report {
    let mut receipts = Vec::new();
    let mut warnings = Vec::new();
    let mut queue = VecDeque::new();
    let mut seen_paths = HashSet::new();
    let mut seen_ids = HashSet::new();
    let root = path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|p| p.to_string_lossy().into_owned());
    if let Some(path) = path {
        queue.push_back(path);
    }
    queue.extend(children);
    let mut journal_count = 0;
    let mut bytes = 0u64;
    while let Some(path) = queue.pop_front() {
        if !seen_paths.insert(path.clone()) {
            continue;
        }
        if path.extension().is_none_or(|e| e != "oms") {
            warnings.push(format!(
                "{} is a legacy transcript without a compatible receipt ledger.",
                short_path(&path)
            ));
            continue;
        }
        if seen_paths.len() > 256 {
            warnings.push("Stopped at the 256-journal safety limit; totals are partial.".into());
            break;
        }
        let size = match std::fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(e) => {
                warnings.push(format!("Cannot read {}: {e}", short_path(&path)));
                continue;
            }
        };
        bytes = bytes.saturating_add(size);
        if size > 128 * 1024 * 1024 || bytes > 512 * 1024 * 1024 {
            warnings.push(format!(
                "{} exceeds the analysis size limit; totals are partial.",
                short_path(&path)
            ));
            continue;
        }
        let entries = match Journal::scan(&path) {
            Ok(e) => e,
            Err(e) => {
                warnings.push(format!("Cannot scan {}: {e}", short_path(&path)));
                continue;
            }
        };
        journal_count += 1;
        let session = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for entry in entries {
            if entry.kind.name.as_str() == kind::TURN_RECEIPT {
                let at = u64::try_from(entry.id.as_ulid().timestamp_ms()).unwrap_or(u64::MAX);
                let receipt = serde_json::from_str(&entry.data).ok().and_then(|value| {
                    Receipt::from_value(&value, entry.id.to_string(), session.clone(), at)
                });
                match receipt {
                    Some(receipt) if seen_ids.insert(receipt.id.clone()) => receipts.push(receipt),
                    Some(_) => {}
                    None => warnings.push(format!(
                        "Malformed receipt {} in {}; totals are partial.",
                        entry.id,
                        short_path(&path)
                    )),
                }
            } else if entry.label.as_deref() == Some("jobs.insert") {
                for id in crate::journal::inserted_subagent_ids(&entry.data) {
                    if valid_child_id(&id)
                        && let Some(parent) = path.parent()
                    {
                        queue.push_back(parent.join(format!("{id}.oms")));
                    }
                }
            }
        }
    }
    receipts.sort_by_key(|r| r.at_ms);
    let mut report = render(&receipts, root.as_deref(), journal_count, &warnings, &model);
    report.card = card(&receipts, root.as_deref(), journal_count, warnings.len());
    report
}

fn short_path(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
fn valid_child_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(c);
    }
    result
}
fn count(value: u64, reported: Option<bool>) -> String {
    if reported == Some(true) || value > 0 {
        number(value)
    } else {
        "unavailable".into()
    }
}

fn aggregate_count(value: u64, known: u64, requests: u64) -> String {
    if requests == 0 {
        "—".into()
    } else if known == 0 && value == 0 {
        "unavailable".into()
    } else if known < requests {
        format!("{} (partial)", number(value))
    } else {
        number(value)
    }
}

fn cache_observation(
    receipts: &[Receipt],
    root: Option<&str>,
    model: &str,
) -> Option<CacheObservation> {
    receipts
        .iter()
        .filter(|r| Some(r.session.as_str()) == root && r.model == model)
        .map(|r| CacheObservation {
            completed_at_ms: r.evidence.response_completed_at_ms.unwrap_or(r.at_ms),
            ttl_seconds: r.evidence.prompt_cache_ttl_seconds,
            retention: r.evidence.prompt_cache_retention.clone(),
        })
        .max_by_key(|r| r.completed_at_ms)
}

fn cache_window_summary(
    receipts: &[Receipt],
    root: Option<&str>,
    model: &str,
    now_ms: u64,
) -> String {
    let mut clock = CacheClock::default();
    clock.observe(cache_observation(receipts, root, model), now_ms);
    match clock.warning(now_ms) {
        Some(warning) => format!("{} · {}\n{}\nRefresh updates this snapshot.", warning.badge_label, warning.headline, warning.explanation),
        None => "Cache timer unavailable: no valid completed response recorded for this session and model.".into(),
    }
}

fn render(
    receipts: &[Receipt],
    root: Option<&str>,
    journals: usize,
    warnings: &[String],
    model: &str,
) -> Report {
    let mut total = Totals::default();
    let mut primary = Totals::default();
    let mut workers = Totals::default();
    let mut routes: BTreeMap<&str, Totals> = BTreeMap::new();
    for r in receipts {
        total.add(r);
        if Some(r.session.as_str()) == root {
            primary.add(r);
        } else {
            workers.add(r);
        }
        routes.entry(&r.model).or_default().add(r);
    }
    let metric = |title, value: u64, caption: &str, tooltip: &str| Metric {
        title,
        value: number(value),
        compact: compact_number(value),
        caption: caption.into(),
        tooltip: format!("{}: {}. {tooltip}", title, number(value)),
    };
    let mut cache_metric = metric(
        "Cache reads",
        total.read,
        "Included in input",
        "Recorded reused input tokens; included in Input tokens.",
    );
    if total.requests == 0 || total.known_reads == 0 {
        cache_metric.value = "—".into();
        cache_metric.compact = "—".into();
        cache_metric.caption = "No read telemetry".into();
    } else if total.known_reads < total.requests {
        cache_metric.caption = "Partial telemetry".into();
        cache_metric.compact.push('+');
    }
    cache_metric.tooltip = format!(
        "Cache reads: {}. Read telemetry in {}/{} requests. Missing values are not cache misses.",
        aggregate_count(total.read, total.known_reads, total.requests),
        total.known_reads,
        total.requests
    );
    let scope = format!(
        "Session + workers · {journals} {} · Recorded requests only",
        if journals == 1 { "journal" } else { "journals" }
    );
    let metrics = [
        metric(
            "Input tokens",
            total.total_input(),
            "Includes cache reads / writes",
            "Cumulative ordinary input + cache reads + cache writes across this session and recorded workers. Repeated requests count again.",
        ),
        metric(
            "Output tokens",
            total.output,
            "Includes reasoning",
            "Cumulative output across this session and recorded workers, including reasoning.",
        ),
        cache_metric,
        metric(
            "Requests",
            total.requests,
            "Session + workers",
            "Committed request receipts, including abandoned branches; copied receipt IDs count once.",
        ),
    ];
    let mut overview = vec![
        Section::new(
            "Input composition",
            format!(
                "Read telemetry: {}/{} requests. Write telemetry: {}/{}. {}",
                total.known_reads,
                total.requests,
                total.known_writes,
                total.requests,
                if total.known_reads > 0 && total.total_input() > 0 {
                    format!(
                        "Recorded cache-read share: {:.1}% of input.{}",
                        total.read as f64 / total.total_input() as f64 * 100.0,
                        if total.known_reads < total.requests || total.known_writes < total.requests
                        {
                            " Partial telemetry; not a complete hit rate."
                        } else {
                            ""
                        }
                    )
                } else {
                    "Cache-read share unavailable. Missing values are not cache misses.".into()
                }
            ),
        )
        .table(
            &["Token class", "Recorded tokens"],
            &[3.0, 2.0],
            vec![
                vec!["Ordinary input".into(), number(total.input)],
                vec![
                    "Cache reads".into(),
                    aggregate_count(total.read, total.known_reads, total.requests),
                ],
                vec![
                    "Cache writes".into(),
                    aggregate_count(total.write, total.known_writes, total.requests),
                ],
                vec![
                    "1-hour writes (subset of writes)".into(),
                    if total.write_1h > 0 {
                        number(total.write_1h)
                    } else {
                        "—".into()
                    },
                ],
            ],
        ),
    ];
    overview.push(Section::new("Spend by owner", "Includes recorded worker sessions and abandoned branches; copied receipt IDs count once.").table(
        &["Owner", "Requests", "Input", "Output"], &[2.5, 1.0, 1.5, 1.3],
        [("This session", &primary), ("Delegated workers", &workers)].into_iter().map(|(name, t)| vec![name.into(), number(t.requests), number(t.total_input()), number(t.output)]).collect()));
    overview.push(
        Section::new(
            "By model",
            if routes.len() > 15 {
                "Showing the first 15 model groups; totals include all groups."
            } else {
                "Input includes cache reads and writes."
            },
        )
        .table(
            &["Model", "Requests", "Input", "Output", "Cache reads"],
            &[3.0, 1.0, 1.4, 1.2, 1.7],
            routes
                .into_iter()
                .take(15)
                .map(|(model, t)| {
                    vec![
                        model.into(),
                        number(t.requests),
                        number(t.total_input()),
                        number(t.output),
                        aggregate_count(t.read, t.known_reads, t.requests),
                    ]
                })
                .collect(),
        ),
    );
    overview.push(Section::new("Cost & provenance", if total.cost_requests > 0 {
        format!("Catalog estimate: ${:.6} for {}/{} requests. May include retry estimates not itemized in token receipts. This is not an invoice, measured savings, or subscription quota consumption.", total.cost as f64 / 1_000_000_000.0, total.cost_requests, total.requests)
    } else { "Cost estimate and subscription quota impact are unavailable.".into() }).table(
        &["Usage source", "Requests"], &[3.0, 1.0], vec![
            vec!["Provider reported".into(), number(total.reported)],
            vec!["Local estimates / mixed".into(), number(total.estimated)],
            vec!["Legacy or unavailable".into(), number(total.unknown)],
        ]));
    let mut coverage = Section::new(
        "Coverage",
        "Only committed receipts can be counted. Interrupted streams, failed attempts without usage, auxiliary calls without receipts, missing worker journals, and activity outside this session may consume additional tokens. Older journals omitted cache-write detail and provenance; no cache zero or expiry is inferred from that absence. Totals measure repeated requests, not unique context tokens.",
    );
    if !warnings.is_empty() {
        coverage.note.push_str(&format!(
            " {} coverage warnings; showing up to 20.",
            warnings.len()
        ));
        coverage = coverage.table(
            &["Incomplete data"],
            &[1.0],
            warnings.iter().take(20).map(|w| vec![w.clone()]).collect(),
        );
    }
    if total.requests == 0 {
        coverage.note.insert_str(
            0,
            "No completed request receipts yet. No provider calls are made to inspect usage. ",
        );
    }
    overview.push(coverage);

    let mut requests = Section::new("Recent requests", "Latest 20 recorded requests, newest first. Input here is ordinary input; reads and writes are separate. Hover a row for its receipt ID and usage source.").table(
        &["Model", "Owner", "Input", "Read", "Write", "Output"], &[3.0, 1.0, 1.25, 1.5, 1.5, 1.1],
        receipts.iter().rev().take(20).map(|r| vec![r.model.clone(), if Some(r.session.as_str()) == root { "Session".into() } else { "Worker".into() }, number(r.input), count(r.read, r.evidence.cache_read_reported), count(r.write, r.evidence.cache_write_reported), number(r.output)]).collect());
    requests.tooltips = receipts
        .iter()
        .rev()
        .take(20)
        .map(|r| {
            format!(
                "Receipt {} · {} · Session {}",
                r.id,
                r.evidence
                    .usage_source
                    .as_deref()
                    .unwrap_or("legacy source unknown"),
                r.session
            )
        })
        .collect();

    let provider = model.split('/').next().unwrap_or("");
    let policy = CachePolicy::for_route(provider, model, None);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    let mut cache = vec![
        Section::new("Retention policy", policy.summary()).table(
            &["Selected route", "Minimum eligible prefix"],
            &[3.0, 2.0],
            vec![vec![
                model.into(),
                policy
                    .minimum_tokens
                    .map(number)
                    .unwrap_or_else(|| "Unknown".into()),
            ]],
        ),
        Section::new(
            "KV cache timer",
            cache_window_summary(receipts, root, model, now_ms),
        ),
        Section::new("Pricing semantics", policy.pricing_summary()),
    ];
    if !policy.supported_retention.is_empty() {
        cache.push(
            Section::new(
                "Provider API retention options",
                "Reference options; these do not confirm the TTL used by this session.",
            )
            .table(
                &["Option", "Retention & pricing"],
                &[1.0, 5.0],
                policy
                    .supported_retention
                    .iter()
                    .map(|option| {
                        vec![
                            option.value.into(),
                            format!(
                                "{}\n{}",
                                option.description,
                                CachePolicy::for_route(provider, model, Some(option.value))
                                    .pricing_summary()
                            ),
                        ]
                    })
                    .collect(),
            ),
        );
    }
    let mut config = String::from(
        "Provider-reported minimum TTLs are carried from API responses into receipts. When absent, the KV timer assumes five minutes. The countdown starts at response completion; neither a reported TTL nor the fallback confirms a live cache entry.",
    );
    if provider == "anthropic" {
        config.push_str(" Automatic retention currently supplies no explicit Anthropic cache markers. The documented 5-minute default applies when caching is requested; it is not proof this session enabled caching.");
    }
    cache.push(Section::new("What the app can observe", config));
    cache.push(Section::new("What a change does", "Normal appended messages can reuse an unchanged eligible prefix. A mismatch prevents reuse from the changed position onward; it need not erase every stored prefix. A miss shifts tokens into ordinary input or cache writes, possibly at a higher input price. Output tokens are still counted. Provider receipts do not identify the cause of a miss.").table(&["Change", "Effect on reuse"], &[1.4, 4.6], [
        ("Model", CacheAction::ModelSwitch { from: "current", to: "different" }),
        ("Account", CacheAction::AccountSwitch { from: "current", to: "different" }),
        ("Reasoning", CacheAction::ReasoningChange { from: "current", to: "different" }),
        ("Instructions", CacheAction::SystemInstructionsChange), ("Tools", CacheAction::ToolsChange),
        ("History / compaction", CacheAction::Compaction), ("Fork", CacheAction::ForkSession),
    ].into_iter().filter_map(|(label, action)| policy.impact(action).map(|impact| vec![label.into(), impact.detail.into()])).collect()));
    cache.push(
        Section::new(
            "Policy references",
            format!("Reviewed {}", crate::cache_policy::VERIFIED_ON),
        )
        .table(
            &["Provider", "Documentation"],
            &[1.0, 4.0],
            vec![
                vec![
                    "OpenAI".into(),
                    "https://developers.openai.com/api/docs/guides/prompt-caching".into(),
                ],
                vec![
                    "Anthropic".into(),
                    "https://platform.claude.com/docs/en/build-with-claude/prompt-caching".into(),
                ],
            ],
        ),
    );
    Report {
        cache_observation: cache_observation(receipts, root, model),
        card: Card::default(),
        metrics,
        scope,
        composition: [total.input, total.read, total.write],
        pages: [overview, vec![requests], cache],
    }
}

/// One model's part of a session: its requests and token classes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ModelUse {
    pub model: String,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl ModelUse {
    /// Every token the model read or wrote: fresh input, cache reads and
    /// writes, and output.
    pub fn tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

/// One journal's part of a session: the session itself or one sub-agent,
/// named by its journal's stem (the window names it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AgentUse {
    pub session: String,
    pub root: bool,
    pub requests: u64,
    pub tokens: u64,
}

/// What `/usage` shows, from recorded receipts only. A figure the receipts
/// do not carry is `None` (or a `false` flag) and its section is left out;
/// nothing here is estimated but the cost, which says so.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Card {
    pub journals: usize,
    pub warnings: usize,
    pub requests: u64,
    /// Requests with provider-reported, estimated/measured, or unknown usage.
    pub reported_requests: u64,
    pub estimated_requests: u64,
    pub unknown_requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Whether any receipt reported a cache read (or write): zero reported
    /// is a figure, never reported is not.
    pub cache_read_known: bool,
    pub cache_write_known: bool,
    /// The catalog's estimate, over `cost_requests` of the requests.
    pub cost_nano_usd: Option<u64>,
    pub cost_requests: u64,
    /// Summed request time, over `timed_requests` of the requests.
    pub api_ms: Option<u64>,
    /// Wall time during which at least one request was running.
    pub active_ms: Option<u64>,
    pub timed_requests: u64,
    /// First request's start to last receipt.
    pub span_ms: Option<u64>,
    /// By tokens, most first.
    pub models: Vec<ModelUse>,
    /// The session first, then its sub-agents by tokens, most first.
    pub agents: Vec<AgentUse>,
}

impl Card {
    /// Every token of the session: fresh input, cache reads and writes, and
    /// output.
    pub fn tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    /// Cache reads over all input read (fresh, cached and written), when
    /// the receipts reported cache reads at all.
    pub fn cache_hit(&self) -> Option<f64> {
        let input = self
            .input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write);
        (self.cache_read_known && input > 0).then(|| self.cache_read as f64 / input as f64)
    }

    /// Sub-agents' share of the session's tokens, when it had any.
    pub fn subagent_share(&self) -> Option<f64> {
        let total = self.tokens();
        let children: u64 = self
            .agents
            .iter()
            .filter(|a| !a.root)
            .map(|a| a.tokens)
            .sum();
        (total > 0 && self.agents.iter().any(|a| !a.root)).then(|| children as f64 / total as f64)
    }
}

/// A receipt as it arrived on the live stream: what a session that keeps
/// no journal (the mock's temporary sessions) has of its requests.
#[derive(Debug, Clone)]
pub(crate) struct LiveReceipt(Receipt);

/// Completion timestamp is generated in the helper, never at UI delivery.
/// Older helpers used receipt creation time, the closest available completion.
pub(crate) fn live_cache_observation(
    r: &InferenceReceipt,
    model: &str,
) -> Option<CacheObservation> {
    let receipt_model = r.model.as_deref()?;
    let matches = receipt_model == model
        || model.split_once('/').is_some_and(|(provider, name)| {
            r.provider.as_deref() == Some(provider) && receipt_model == name
        });
    if !matches {
        return None;
    }
    Some(CacheObservation {
        completed_at_ms: r.response_completed_at_ms.or(r.recorded_at_ms)?,
        ttl_seconds: r.prompt_cache_ttl_seconds,
        retention: r.prompt_cache_retention.clone(),
    })
}

pub(crate) fn live_receipt(r: &InferenceReceipt, at_ms: u64) -> LiveReceipt {
    LiveReceipt(Receipt {
        id: r
            .receipt_id
            .clone()
            .unwrap_or_else(|| format!("live-{at_ms}")),
        session: "live".into(),
        model: r.model.clone().unwrap_or_else(|| "Unknown model".into()),
        at_ms,
        request_started_ms: r
            .duration_ms
            .and_then(|duration| at_ms.checked_sub(duration)),
        input: r.tokens_in,
        output: r.tokens_out,
        read: r.cache_read,
        write: r.cache_write,
        evidence: Evidence::from(r),
    })
}

/// The `/usage` card from live receipts, oldest first.
pub(crate) fn live_card(receipts: &[LiveReceipt]) -> Card {
    let receipts: Vec<Receipt> = receipts.iter().map(|r| r.0.clone()).collect();
    card(&receipts, Some("live"), 0, 0)
}

/// Builds the `/usage` card from receipts sorted by time. `root` is the
/// session's own journal stem; every other stem is a sub-agent.
fn card(receipts: &[Receipt], root: Option<&str>, journals: usize, warnings: usize) -> Card {
    let mut card = Card {
        journals,
        warnings,
        ..Card::default()
    };
    let mut models: BTreeMap<&str, ModelUse> = BTreeMap::new();
    let mut agents: BTreeMap<&str, AgentUse> = BTreeMap::new();
    let mut intervals = Vec::new();
    let (mut api, mut cost) = (0u64, 0u64);
    let (mut first, mut last) = (None::<u64>, None::<u64>);
    for r in receipts {
        card.requests += 1;
        match r.evidence.usage_source.as_deref() {
            Some("provider") => card.reported_requests += 1,
            Some("estimated" | "mixed" | "measured") => card.estimated_requests += 1,
            _ => card.unknown_requests += 1,
        }
        card.input = card.input.saturating_add(r.input);
        card.output = card.output.saturating_add(r.output);
        card.cache_read = card.cache_read.saturating_add(r.read);
        card.cache_write = card.cache_write.saturating_add(r.write);
        card.cache_read_known |= r.evidence.cache_read_reported == Some(true) || r.read > 0;
        card.cache_write_known |= r.evidence.cache_write_reported == Some(true) || r.write > 0;
        if let Some(nano) = r.evidence.cost_nano_usd
            && r.evidence.cost_source.as_deref() == Some("catalog_estimate")
        {
            cost = cost.saturating_add(nano);
            card.cost_requests += 1;
        }
        if let Some(start) = r.request_started_ms {
            api = api.saturating_add(r.at_ms - start);
            card.timed_requests += 1;
            intervals.push((start, r.at_ms));
            first = Some(first.map_or(start, |f: u64| f.min(start)));
        }
        last = Some(last.map_or(r.at_ms, |l: u64| l.max(r.at_ms)));
        let model = models.entry(r.model.as_str()).or_insert_with(|| ModelUse {
            model: r.model.clone(),
            ..ModelUse::default()
        });
        model.requests += 1;
        model.input = model.input.saturating_add(r.input);
        model.output = model.output.saturating_add(r.output);
        model.cache_read = model.cache_read.saturating_add(r.read);
        model.cache_write = model.cache_write.saturating_add(r.write);
        let tokens = r
            .input
            .saturating_add(r.output)
            .saturating_add(r.read)
            .saturating_add(r.write);
        let agent = agents
            .entry(r.session.as_str())
            .or_insert_with(|| AgentUse {
                session: r.session.clone(),
                root: Some(r.session.as_str()) == root,
                ..AgentUse::default()
            });
        agent.requests += 1;
        agent.tokens = agent.tokens.saturating_add(tokens);
    }
    if card.cost_requests > 0 {
        card.cost_nano_usd = Some(cost);
    }
    if card.timed_requests > 0 {
        card.api_ms = Some(api);
        // Overlapping requests (sub-agents at once) count once.
        intervals.sort_unstable();
        let mut active = 0u64;
        let mut open: Option<(u64, u64)> = None;
        for (start, end) in intervals {
            match open {
                Some((s, e)) if start <= e => open = Some((s, e.max(end))),
                Some((s, e)) => {
                    active += e - s;
                    open = Some((start, end));
                }
                None => open = Some((start, end)),
            }
        }
        if let Some((s, e)) = open {
            active += e - s;
        }
        card.active_ms = Some(active);
        if let (Some(first), Some(last)) = (first, last) {
            card.span_ms = Some(last.saturating_sub(first));
        }
    }
    card.models = models.into_values().collect();
    card.models.sort_by(|a, b| {
        b.tokens()
            .cmp(&a.tokens())
            .then_with(|| a.model.cmp(&b.model))
    });
    card.agents = agents.into_values().collect();
    card.agents.sort_by(|a, b| {
        b.root
            .cmp(&a.root)
            .then(b.tokens.cmp(&a.tokens))
            .then_with(|| a.session.cmp(&b.session))
    });
    card
}

pub(crate) fn compact_number(value: u64) -> String {
    for (threshold, suffix) in [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "k")] {
        if value >= threshold {
            return format!("{:.1}{suffix}", value as f64 / threshold as f64);
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn receipt(session: &str, _model: &str, at: u64, v: Value) -> Receipt {
        Receipt::from_value(&v, format!("{session}-{at}"), session.into(), at).unwrap()
    }

    #[test]
    fn card_totals_come_only_from_receipts() {
        let receipts = vec![
            receipt(
                "root",
                "m",
                10_000,
                json!({"tokens_in":100,"tokens_out":20,"cache_read":300,"cache_write":100,"cache_read_reported":true,"duration_ms":4_000,"identity":{"model":"opus"}}),
            ),
            receipt(
                "child",
                "m",
                12_000,
                json!({"tokens_in":50,"tokens_out":30,"duration_ms":3_000,"identity":{"model":"haiku"}}),
            ),
            receipt(
                "root",
                "m",
                20_000,
                json!({"tokens_in":10,"tokens_out":5,"cache_read":90,"duration_ms":1_000,"cost_nano_usd":2_000_000,"cost_source":"catalog_estimate","identity":{"model":"opus"}}),
            ),
        ];
        let card = card(&receipts, Some("root"), 2, 0);
        assert_eq!(
            (
                card.requests,
                card.input,
                card.output,
                card.cache_read,
                card.cache_write
            ),
            (3, 160, 55, 390, 100)
        );
        // Cost only from the one request that carried an estimate.
        assert_eq!(
            (card.cost_nano_usd, card.cost_requests),
            (Some(2_000_000), 1)
        );
        // 4 s + 3 s + 1 s of requests; 6–10 s and 9–12 s overlap to 6 s.
        assert_eq!(card.api_ms, Some(8_000));
        assert_eq!(card.active_ms, Some(7_000));
        assert_eq!(card.span_ms, Some(14_000));
        let hit = card.cache_hit().unwrap();
        assert!((hit - 390.0 / 650.0).abs() < 1e-9);
        assert_eq!(card.models[0].model, "opus");
        assert_eq!(card.models[0].tokens(), 100 + 20 + 300 + 100 + 10 + 5 + 90);
        assert_eq!(card.models[1].tokens(), 80);
        assert!(card.agents[0].root);
        assert_eq!(
            card.agents[1],
            AgentUse {
                session: "child".into(),
                root: false,
                requests: 1,
                tokens: 80
            }
        );
        assert!((card.subagent_share().unwrap() - 80.0 / 705.0).abs() < 1e-9);
    }

    #[test]
    fn card_keeps_reported_estimated_and_unknown_usage_distinct() {
        let receipts = vec![
            receipt(
                "root",
                "m",
                1,
                json!({"tokens_in":10,"tokens_out":1,"usage_source":"provider"}),
            ),
            receipt(
                "root",
                "m",
                2,
                json!({"tokens_in":20,"tokens_out":2,"usage_source":"estimated"}),
            ),
            receipt(
                "root",
                "m",
                3,
                json!({"tokens_in":30,"tokens_out":3,"usage_source":"mixed"}),
            ),
            receipt("root", "m", 4, json!({"tokens_in":40,"tokens_out":4})),
        ];
        let card = card(&receipts, Some("root"), 1, 0);
        assert_eq!(
            (
                card.reported_requests,
                card.estimated_requests,
                card.unknown_requests
            ),
            (1, 2, 1)
        );
        assert_eq!(card.requests, 4);
        assert_eq!((card.input, card.output), (100, 10));
    }

    #[test]
    fn card_leaves_unknown_figures_unknown() {
        // No duration, no cost, no cache reporting, no children.
        let receipts = vec![receipt(
            "root",
            "m",
            5,
            json!({"tokens_in":7,"tokens_out":3}),
        )];
        let card = card(&receipts, Some("root"), 1, 0);
        assert_eq!(card.cost_nano_usd, None);
        assert_eq!(
            (card.api_ms, card.active_ms, card.span_ms),
            (None, None, None)
        );
        assert_eq!(card.cache_hit(), None);
        assert_eq!(card.subagent_share(), None);
        assert_eq!(card.models[0].model, "Unknown model");
        // And no receipts at all is an empty card, not zeros dressed as data.
        let empty = super::card(&[], Some("root"), 0, 0);
        assert_eq!(empty.requests, 0);
        assert!(empty.models.is_empty() && empty.agents.is_empty());
    }
    #[test]
    fn exclusive_token_classes_and_one_hour_subset() {
        let r = Receipt::from_value(&json!({"tokens_in":100,"tokens_out":20,"cache_read":700,"cache_write":200,"cache_write_1h":150,"usage_source":"provider","cache_read_reported":true,"cache_write_reported":true}), "r".into(), "s".into(), 0).unwrap();
        let mut total = Totals::default();
        total.add(&r);
        assert_eq!(total.total_input(), 1000);
        assert_eq!(total.write_1h, 150);
        assert_eq!(total.reported, 1);
        assert_eq!(total.known_reads, 1);
    }
    #[test]
    fn absent_and_reported_zero_are_distinct() {
        assert_eq!(count(0, None), "unavailable");
        assert_eq!(count(0, Some(false)), "unavailable");
        assert_eq!(count(0, Some(true)), "0");
        assert_eq!(count(42, None), "42");
        assert_eq!(aggregate_count(0, 0, 3), "unavailable");
        assert_eq!(aggregate_count(0, 3, 3), "0");
        assert_eq!(aggregate_count(42, 1, 3), "42 (partial)");
        assert!(Receipt::from_value(&json!({}), "r".into(), "s".into(), 0).is_none());
    }

    #[test]
    fn history_timer_uses_completion_without_requiring_cache_hits_or_duration() {
        let route = "openai-codex/gpt-6-luna";
        let make = |session: &str, model: &str, metadata: Value| {
            let mut v = json!({"tokens_in":1,"tokens_out":1,"identity":{"model":model}});
            v.as_object_mut()
                .unwrap()
                .extend(metadata.as_object().unwrap().clone());
            Receipt::from_value(&v, "r".into(), session.into(), 120_000).unwrap()
        };
        let valid = make("root", route, json!({"duration_ms":600_000}));
        assert!(
            cache_window_summary(&[valid.clone()], Some("root"), route, 120_000)
                .contains("KV ~05:00")
        );
        assert!(
            cache_window_summary(&[valid.clone()], Some("root"), route, 420_000)
                .contains("window elapsed")
        );
        let reported = make(
            "root",
            route,
            json!({"response_completed_at_ms":110_000,"prompt_cache_ttl_seconds":1800}),
        );
        assert!(
            cache_window_summary(&[reported], Some("root"), route, 120_000).contains("KV 29:50")
        );
        for other in [
            make("worker", route, json!({})),
            make("root", "other", json!({})),
        ] {
            assert!(
                cache_window_summary(&[other], Some("root"), route, 130_000)
                    .contains("unavailable")
            );
        }
        assert!(cache_window_summary(&[valid], Some("root"), route, 0).contains("unavailable"));
    }

    #[test]
    fn live_cache_clock_carries_completion_and_provider_metadata_without_counters() {
        let model = "openai-codex/gpt-6-luna";
        let valid = InferenceReceipt {
            model: Some("gpt-6-luna".into()),
            provider: Some("openai-codex".into()),
            recorded_at_ms: Some(500_000),
            response_completed_at_ms: Some(490_000),
            prompt_cache_ttl_seconds: Some(1800),
            prompt_cache_retention: Some("24h".into()),
            ..Default::default()
        };
        assert_eq!(
            live_cache_observation(&valid, model),
            Some(CacheObservation {
                completed_at_ms: 490_000,
                ttl_seconds: Some(1800),
                retention: Some("24h".into()),
            })
        );
        let old = InferenceReceipt {
            response_completed_at_ms: None,
            ..valid.clone()
        };
        assert_eq!(
            live_cache_observation(&old, model).unwrap().completed_at_ms,
            500_000
        );
        let no_time = InferenceReceipt {
            recorded_at_ms: None,
            ..old
        };
        assert!(live_cache_observation(&no_time, model).is_none());
        assert!(live_cache_observation(&valid, "openai/gpt-6-luna").is_none());
        let evidence = Evidence::from(&valid);
        let decoded: Evidence =
            serde_json::from_str(&serde_json::to_string(&evidence).unwrap()).unwrap();
        assert_eq!(decoded, evidence);
    }
    #[test]
    fn workers_and_legacy_costs_remain_visible() {
        let a = Receipt::from_value(
            &json!({"tokens_in":100,"tokens_out":10,"cost_nano_usd":1000000}),
            "a".into(),
            "root".into(),
            1,
        )
        .unwrap();
        let b = Receipt::from_value(
            &json!({"tokens_in":200,"tokens_out":20}),
            "b".into(),
            "worker".into(),
            2,
        )
        .unwrap();
        let report = render(&[a, b], Some("root"), 2, &[], "openai-codex/gpt-6-luna");
        assert_eq!(report.metrics[0].value, "300");
        assert_eq!(
            report.pages[0][1].rows[1],
            ["Delegated workers", "1", "200", "20"]
        );
        assert!(
            report.pages[0][3]
                .note
                .contains("$0.001000 for 1/2 requests")
        );
        assert_eq!(report.pages[0][3].rows[2], ["Legacy or unavailable", "2"]);
        assert_eq!(report.metrics[2].value, "—");
    }
    #[test]
    fn dashboard_totals_include_workers_and_mark_partial_cache_evidence() {
        let make = |value, id: &str, session: &str| {
            Receipt::from_value(&value, id.into(), session.into(), 0).unwrap()
        };
        let report = render(
            &[
                make(
                    json!({"tokens_in":1000,"tokens_out":200,"cache_read":8000,"cache_write":1000,"cache_write_1h":600,"cache_read_reported":true,"cache_write_reported":true}),
                    "a",
                    "root",
                ),
                make(json!({"tokens_in":500,"tokens_out":100}), "b", "worker"),
            ],
            Some("root"),
            2,
            &[],
            "openai-codex/gpt-6-luna",
        );
        assert_eq!(report.metrics[0].value, "10,500");
        assert_eq!(report.metrics[0].compact, "10.5k");
        assert_eq!(report.metrics[1].value, "300");
        assert_eq!(report.metrics[2].compact, "8.0k+");
        assert_eq!(report.metrics[2].caption, "Partial telemetry");
        assert!(report.metrics[2].tooltip.contains("1/2 requests"));
        assert_eq!(report.composition, [1500, 8000, 1000]);
        assert_eq!(report.pages[1][0].rows[1][2], "1,000");
        assert_eq!(report.pages[0][0].rows[3][1], "600");
        let zero = render(
            &[make(
                json!({"tokens_in":100,"tokens_out":10,"cache_read":0,"cache_read_reported":true}),
                "z",
                "root",
            )],
            Some("root"),
            1,
            &[],
            "unknown/model",
        );
        assert_eq!(zero.metrics[2].value, "0");
        assert_eq!(zero.metrics[2].compact, "0");
        assert_eq!(zero.metrics[2].caption, "Included in input");
    }

    #[test]
    fn child_paths_cannot_escape_sessions() {
        assert!(valid_child_id("01M356YHMPFF9J4F0BPQ3SN4A9"));
        assert!(!valid_child_id("../outside"));
        assert!(!valid_child_id("/tmp/other"));
    }

    #[test]
    fn journal_reader_includes_workers_and_deduplicates_copied_receipts() {
        let dir = std::env::temp_dir().join(format!("omperor-usage-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let genesis = "event: journal@1\nid: 01M356YHMV689DGP5RJ40BV25X\ndata: {\"version\":1,\"cwd\":\"/tmp\",\"created\":\"1790102685339\"}\n\n";
        let receipt = "event: turn.receipt@1\nid: 01M356Z2RRZ7TF760C1MRQ90QQ\nby: 01M356YHMV689DGP5RJ40BV25X\ndata: {\"tokens_in\":100,\"tokens_out\":10,\"receipt_id\":\"shared\"}\n\n";
        let spawn = ": jobs.insert\nevent: patch@1\nid: 01M356YHMYGVNFVPQFZG77QCH8\nby: 01M356YHMV689DGP5RJ40BV25X\ndata: {\"ops\":[[\"ins\",1,null,{\"tag\":\"subagent\",\"props\":[[\"id\",\"worker\"]]}]]}\n\n";
        let extra = "event: turn.receipt@1\nid: 01M3570166SBFNP1AETRCR54YR\nby: 01M356YHMV689DGP5RJ40BV25X\ndata: {\"tokens_in\":200,\"tokens_out\":20,\"receipt_id\":\"worker-only\"}\n\n";
        std::fs::write(dir.join("root.oms"), format!("{genesis}{spawn}{receipt}")).unwrap();
        std::fs::write(dir.join("worker.oms"), format!("{genesis}{receipt}{extra}")).unwrap();
        let report = read(
            Some(dir.join("root.oms")),
            vec![dir.join("worker.oms")],
            "anthropic/claude-opus-5".into(),
        );
        assert_eq!(report.metrics[0].value, "300");
        assert_eq!(report.metrics[1].value, "30");
        assert_eq!(report.metrics[3].value, "2");
        assert_eq!(
            report.pages[0][1].rows[1],
            ["Delegated workers", "1", "200", "20"]
        );
        assert!(report.scope.contains("2 journals"));
        assert!(
            report.pages[2]
                .iter()
                .flat_map(|s| &s.rows)
                .any(|r| r[0] == "1h" && r[1].contains("Minimum 1 hour"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
