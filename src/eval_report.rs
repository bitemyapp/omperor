//! Eval suite results: one normalized record per attempt, aggregated by task
//! and model into success rates and token statistics, and compared between
//! two suite runs. Pure data and formatting; the suite owns all I/O ordering.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

/// How an attempt counts toward a success rate. `Infrastructure` and
/// `Skipped` attempts are kept visible but never enter a rate's denominator:
/// a failed sign-in or fixture setup says nothing about the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Passed,
    Failed,
    Ungraded,
    Infrastructure,
    Skipped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CheckOutcome {
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AttemptRecord {
    pub schema_version: u32,
    pub task: String,
    pub model: String,
    pub account: Option<String>,
    pub thinking_level: Option<String>,
    pub trial: u32,
    pub outcome: Outcome,
    /// The run's status (`completed`, `error`, `timed_out`, ...) or the suite's
    /// own (`setup_failed`, `skipped`, `infrastructure_error`).
    pub status: String,
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// input + output + cache_read + cache_write, the ledger's recorded tokens.
    pub tokens: u64,
    /// Requests whose usage the provider did not report.
    pub unknown_usage_requests: u64,
    pub usage_warnings: u64,
    pub subagents: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub files_changed: Option<u64>,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
    pub checks: Vec<CheckOutcome>,
    pub directory: PathBuf,
}

impl AttemptRecord {
    pub(crate) fn new(
        task: &str,
        model: &str,
        account: Option<String>,
        thinking_level: Option<String>,
        trial: u32,
        directory: PathBuf,
    ) -> Self {
        Self {
            schema_version: 1,
            task: task.into(),
            model: model.into(),
            account,
            thinking_level,
            trial,
            outcome: Outcome::Infrastructure,
            status: "not_started".into(),
            error: None,
            elapsed_ms: 0,
            requests: 0,
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            tokens: 0,
            unknown_usage_requests: 0,
            usage_warnings: 0,
            subagents: 0,
            tool_calls: 0,
            tool_errors: 0,
            files_changed: None,
            insertions: None,
            deletions: None,
            checks: vec![],
            directory,
        }
    }

    /// Fills the record from `omperor eval run`'s exit code and result file.
    /// A missing, malformed, or exit-inconsistent result is an infrastructure
    /// failure, never a pass.
    pub(crate) fn absorb_result(&mut self, exit_code: Option<i32>, result: Result<Value, String>) {
        let result = match result.and_then(|value| check_consistency(exit_code, value)) {
            Ok(result) => result,
            Err(error) => {
                self.outcome = Outcome::Infrastructure;
                self.status = "infrastructure_error".into();
                self.error = Some(error);
                return;
            }
        };
        let text = |key: &str| result.get(key).and_then(Value::as_str).map(str::to_owned);
        let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
        self.status = text("status").unwrap_or_default();
        self.error = text("error");
        if let Some(account) = text("account") {
            self.account = Some(account);
        }
        if let Some(level) = text("thinking_level") {
            self.thinking_level = Some(level);
        }
        self.elapsed_ms = number(&result, "elapsed_ms");
        self.tool_calls = number(&result, "tool_calls");
        self.tool_errors = number(&result, "tool_errors");
        let usage = result.get("usage").cloned().unwrap_or(Value::Null);
        self.requests = number(&usage, "requests");
        self.input = number(&usage, "input");
        self.output = number(&usage, "output");
        self.cache_read = number(&usage, "cache_read");
        self.cache_write = number(&usage, "cache_write");
        self.tokens = self
            .input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write);
        self.unknown_usage_requests = number(&usage, "unknown_requests");
        self.usage_warnings = number(&usage, "warnings");
        self.subagents = usage
            .get("agents")
            .and_then(Value::as_array)
            .map_or(0, |agents| agents.iter().filter(|a| a.get("root") == Some(&Value::Bool(false))).count() as u64);
        if let Some(changes) = result.get("changes").filter(|changes| changes.is_object()) {
            self.files_changed = Some(number(changes, "files"));
            self.insertions = Some(number(changes, "insertions"));
            self.deletions = Some(number(changes, "deletions"));
        }
        self.checks = result
            .get("checks")
            .and_then(Value::as_array)
            .map(|checks| {
                checks
                    .iter()
                    .map(|check| CheckOutcome {
                        passed: check.get("passed").and_then(Value::as_bool).unwrap_or(false),
                        detail: check.get("detail").and_then(Value::as_str).unwrap_or_default().into(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.outcome = match (self.status.as_str(), result.get("success").and_then(Value::as_bool)) {
            // The account never let the prompt through, or the operator
            // stopped the run: neither says anything about the agent.
            ("authentication_required" | "interrupted", _) => Outcome::Infrastructure,
            (_, Some(true)) => Outcome::Passed,
            (_, Some(false)) => Outcome::Failed,
            (_, None) => Outcome::Ungraded,
        };
    }
}

fn check_consistency(exit_code: Option<i32>, result: Value) -> Result<Value, String> {
    if !result.is_object() || !result.get("id").is_some_and(Value::is_string) {
        return Err("result.json is not an eval result".into());
    }
    let success = match result.get("success") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err("result.json has a non-boolean success".into()),
    };
    let status = result.get("status").and_then(Value::as_str).unwrap_or_default();
    let expected = crate::eval::exit_code(status, success);
    if exit_code != Some(expected) {
        return Err(format!(
            "eval run exit {exit_code:?} disagrees with result status {status} success {success:?}"
        ));
    }
    if status != "completed" && success != Some(false) {
        return Err("a run that did not complete claims success or no verdict".into());
    }
    Ok(result)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Stats {
    pub mean: f64,
    pub median: f64,
    pub min: u64,
    pub max: u64,
    pub total: u64,
}

impl Stats {
    fn of(mut values: Vec<u64>) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        values.sort_unstable();
        let total = values.iter().fold(0u64, |sum, value| sum.saturating_add(*value));
        let middle = values.len() / 2;
        let median = if values.len().is_multiple_of(2) {
            (values[middle - 1] as f64 + values[middle] as f64) / 2.0
        } else {
            values[middle] as f64
        };
        Some(Self {
            mean: total as f64 / values.len() as f64,
            median,
            min: values[0],
            max: values[values.len() - 1],
            total,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Group {
    /// `None` for a model's totals across every task.
    pub task: Option<String>,
    pub model: String,
    pub attempts: u64,
    pub passed: u64,
    pub failed: u64,
    pub ungraded: u64,
    pub infrastructure: u64,
    pub skipped: u64,
    /// passed / (passed + failed); `None` with no graded attempt.
    pub pass_rate: Option<f64>,
    /// Wilson 95% score interval of the pass rate.
    pub pass_rate_low: Option<f64>,
    pub pass_rate_high: Option<f64>,
    /// Over attempts that reached the agent (passed, failed, ungraded).
    pub tokens: Option<Stats>,
    pub requests: Option<Stats>,
    pub elapsed_ms: Option<Stats>,
    pub tool_calls: Option<Stats>,
    pub tool_errors: u64,
    /// Every token spent, failures included, per passing attempt.
    pub tokens_per_pass: Option<f64>,
    /// cache_read / (input + cache_read + cache_write).
    pub cache_read_share: Option<f64>,
    /// Attempts whose usage is incomplete (unreported requests or ledger warnings).
    pub partial_usage: u64,
}

fn group(task: Option<String>, model: String, records: &[&AttemptRecord]) -> Group {
    let count = |outcome| records.iter().filter(|r| r.outcome == outcome).count() as u64;
    let (passed, failed) = (count(Outcome::Passed), count(Outcome::Failed));
    let graded = passed + failed;
    let reached: Vec<_> = records
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Passed | Outcome::Failed | Outcome::Ungraded))
        .collect();
    let field = |f: fn(&AttemptRecord) -> u64| Stats::of(reached.iter().map(|r| f(r)).collect());
    let tokens = field(|r| r.tokens);
    let prompt: u64 = reached.iter().map(|r| r.input + r.cache_read + r.cache_write).sum();
    let read: u64 = reached.iter().map(|r| r.cache_read).sum();
    let (low, high) = wilson(passed, graded).unzip();
    Group {
        task,
        model,
        attempts: records.len() as u64,
        passed,
        failed,
        ungraded: count(Outcome::Ungraded),
        infrastructure: count(Outcome::Infrastructure),
        skipped: count(Outcome::Skipped),
        pass_rate: (graded > 0).then(|| passed as f64 / graded as f64),
        pass_rate_low: low,
        pass_rate_high: high,
        tokens_per_pass: tokens
            .as_ref()
            .filter(|_| passed > 0)
            .map(|stats| stats.total as f64 / passed as f64),
        tokens,
        requests: field(|r| r.requests),
        elapsed_ms: field(|r| r.elapsed_ms),
        tool_calls: field(|r| r.tool_calls),
        tool_errors: reached.iter().map(|r| r.tool_errors).sum(),
        cache_read_share: (prompt > 0).then(|| read as f64 / prompt as f64),
        partial_usage: reached
            .iter()
            .filter(|r| r.unknown_usage_requests > 0 || r.usage_warnings > 0 || r.requests == 0)
            .count() as u64,
    }
}

/// The Wilson score interval at 95% confidence; small samples stay honest.
fn wilson(passed: u64, graded: u64) -> Option<(f64, f64)> {
    if graded == 0 {
        return None;
    }
    let (n, p, z) = (graded as f64, passed as f64 / graded as f64, 1.959_964);
    let denominator = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denominator;
    let margin = z * ((p * (1.0 - p) + z * z / (4.0 * n)) / n).sqrt() / denominator;
    Some(((centre - margin).max(0.0), (centre + margin).min(1.0)))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Summary {
    pub schema_version: u32,
    pub attempts: u64,
    pub passed: u64,
    pub failed: u64,
    pub ungraded: u64,
    pub infrastructure: u64,
    pub skipped: u64,
    /// One row per task and model, in task order.
    pub tasks: Vec<Group>,
    /// One row per model across all tasks.
    pub models: Vec<Group>,
}

pub(crate) fn summarize(records: &[AttemptRecord]) -> Summary {
    let mut by_task: BTreeMap<(String, String), Vec<&AttemptRecord>> = BTreeMap::new();
    let mut by_model: BTreeMap<String, Vec<&AttemptRecord>> = BTreeMap::new();
    for record in records {
        by_task
            .entry((record.task.clone(), record.model.clone()))
            .or_default()
            .push(record);
        by_model.entry(record.model.clone()).or_default().push(record);
    }
    let all: Vec<_> = records.iter().collect();
    let totals = group(None, String::new(), &all);
    Summary {
        schema_version: 1,
        attempts: totals.attempts,
        passed: totals.passed,
        failed: totals.failed,
        ungraded: totals.ungraded,
        infrastructure: totals.infrastructure,
        skipped: totals.skipped,
        tasks: by_task
            .into_iter()
            .map(|((task, model), records)| group(Some(task), model, &records))
            .collect(),
        models: by_model
            .into_iter()
            .map(|(model, records)| group(None, model, &records))
            .collect(),
    }
}

/// Suite exit status: 0 when every graded attempt passed and nothing broke,
/// 1 on any failure or infrastructure problem, 3 when nothing was graded.
pub(crate) fn exit_status(summary: &Summary) -> i32 {
    if summary.failed > 0 || summary.infrastructure > 0 || summary.skipped > 0 {
        1
    } else if summary.passed == 0 {
        3
    } else {
        0
    }
}

pub(crate) fn load_records(output: &Path) -> Result<Vec<AttemptRecord>, String> {
    let attempts = output.join("attempts");
    let mut records = Vec::new();
    let mut stack = vec![attempts.clone()];
    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if directory == attempts => {
                return Err(format!("{}: {error}", attempts.display()));
            }
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() && path.file_name().is_none_or(|name| name != "workspace" && name != "run") {
                stack.push(path);
            } else if kind.is_file() && path.file_name().is_some_and(|name| name == "attempt.json") {
                let record = fs::read(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                records.push(record);
            }
        }
    }
    records.sort_by(|a: &AttemptRecord, b| (&a.task, &a.model, a.trial).cmp(&(&b.task, &b.model, b.trial)));
    Ok(records)
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{:.0}%", value * 100.0))
}

fn thousands(value: f64) -> String {
    let rounded = value.round() as u64;
    let digits = rounded.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn seconds(stats: &Option<Stats>) -> String {
    stats
        .as_ref()
        .map_or_else(|| "—".into(), |stats| format!("{:.1} s", stats.median / 1000.0))
}

fn rate(group: &Group) -> String {
    match (group.pass_rate, group.pass_rate_low, group.pass_rate_high) {
        (Some(rate), Some(low), Some(high)) => format!(
            "{} ({}–{})",
            percent(Some(rate)),
            percent(Some(low)),
            percent(Some(high))
        ),
        _ => "—".into(),
    }
}

fn row(out: &mut String, label: &str, group: &Group) {
    let excluded = group.infrastructure + group.skipped + group.ungraded;
    let _ = writeln!(
        out,
        "| {label} | {} | {}/{}{} | {} | {} | {} | {} | {} | {} | {} |",
        group.model,
        group.passed,
        group.passed + group.failed,
        if excluded > 0 { format!(" (+{excluded} not graded)") } else { String::new() },
        rate(group),
        group.tokens.as_ref().map_or_else(|| "—".into(), |s| thousands(s.median)),
        group.tokens_per_pass.map_or_else(|| "—".into(), thousands),
        percent(group.cache_read_share),
        group.requests.as_ref().map_or_else(|| "—".into(), |s| format!("{:.1}", s.mean)),
        group
            .tool_calls
            .as_ref()
            .map_or_else(|| "—".into(), |s| format!("{:.1} ({})", s.mean, group.tool_errors)),
        seconds(&group.elapsed_ms),
    );
}

const HEADER: &str = "| Task | Model | Passed | Pass rate (95% CI) | Median tokens | Tokens per pass | Cached | Requests | Tool calls (errors) | Median time |\n|---|---|---|---|---:|---:|---:|---:|---:|---:|\n";

pub(crate) fn markdown(summary: &Summary, records: &[AttemptRecord]) -> String {
    let mut out = String::from("# Eval summary\n\n");
    let _ = writeln!(
        out,
        "{} attempts: {} passed, {} failed, {} ungraded, {} infrastructure, {} skipped.\n",
        summary.attempts, summary.passed, summary.failed, summary.ungraded, summary.infrastructure, summary.skipped
    );
    out.push_str(HEADER);
    for group in &summary.tasks {
        row(&mut out, group.task.as_deref().unwrap_or_default(), group);
    }
    if summary.tasks.len() > summary.models.len() {
        for group in &summary.models {
            row(&mut out, "**all tasks**", group);
        }
    }
    out.push_str(
        "\nTokens are input + output + cache reads + cache writes from provider receipts, \
         sub-agents included; they measure repeated inference, not unique context. \
         Tokens per pass divides every token spent, failures included, by the passes. \
         Cached is the share of prompt tokens read from the provider's cache.\n",
    );
    let problems: Vec<_> = records
        .iter()
        .filter(|r| !matches!(r.outcome, Outcome::Passed))
        .collect();
    if !problems.is_empty() {
        out.push_str("\n## Attempts that did not pass\n\n");
        for record in problems {
            let detail = record
                .error
                .clone()
                .or_else(|| {
                    record
                        .checks
                        .iter()
                        .find(|check| !check.passed)
                        .map(|check| format!("check failed: {}", check.detail))
                })
                .unwrap_or_else(|| "no checks".into());
            let _ = writeln!(
                out,
                "- `{}` {} #{}: {:?} ({}): {} — `{}`",
                record.task,
                record.model,
                record.trial,
                record.outcome,
                record.status,
                one_line(&detail, 240),
                record.directory.display()
            );
        }
    }
    let partial: u64 = summary.models.iter().map(|g| g.partial_usage).sum();
    if partial > 0 {
        let _ = writeln!(
            out,
            "\n{partial} attempt(s) have incomplete usage (unreported requests, ledger warnings, or no receipts); their token figures are lower bounds."
        );
    }
    out
}

fn one_line(text: &str, limit: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        flat
    } else {
        format!("{}…", flat.chars().take(limit).collect::<String>())
    }
}

/// Baseline → candidate for every task and model present in both runs.
pub(crate) fn compare(baseline: &Summary, candidate: &Summary) -> String {
    let mut out = String::from(
        "# Eval comparison\n\n| Task | Model | Pass rate | Median tokens | Tokens per pass | Median time |\n|---|---|---|---:|---:|---:|\n",
    );
    let index: BTreeMap<_, _> = baseline
        .tasks
        .iter()
        .chain(baseline.models.iter())
        .map(|g| ((g.task.clone(), g.model.clone()), g))
        .collect();
    for group in candidate.tasks.iter().chain(candidate.models.iter()) {
        let Some(before) = index.get(&(group.task.clone(), group.model.clone())) else {
            continue;
        };
        let change = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) if a > 0.0 => format!(
                "{} → {} ({:+.0}%)",
                thousands(a),
                thousands(b),
                (b - a) / a * 100.0
            ),
            (a, b) => format!(
                "{} → {}",
                a.map_or_else(|| "—".into(), thousands),
                b.map_or_else(|| "—".into(), thousands)
            ),
        };
        let median = |g: &Group| g.tokens.as_ref().map(|s| s.median);
        let time = |g: &Group| g.elapsed_ms.as_ref().map(|s| s.median / 1000.0);
        let _ = writeln!(
            out,
            "| {} | {} | {} → {} | {} | {} | {} |",
            group.task.as_deref().unwrap_or("**all tasks**"),
            group.model,
            percent(before.pass_rate),
            percent(group.pass_rate),
            change(median(before), median(group)),
            change(before.tokens_per_pass, group.tokens_per_pass),
            match (time(before), time(group)) {
                (Some(a), Some(b)) => format!("{a:.1} s → {b:.1} s"),
                _ => "—".into(),
            },
        );
    }
    out.push_str("\nSmall samples vary a lot between runs; compare the confidence intervals in each summary before drawing conclusions.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(task: &str, outcome: Outcome, tokens: u64) -> AttemptRecord {
        let mut record = AttemptRecord::new(task, "openai/x", None, None, 1, PathBuf::from("/x"));
        record.outcome = outcome;
        record.input = tokens;
        record.tokens = tokens;
        record.requests = 1;
        record
    }

    #[test]
    fn infrastructure_and_ungraded_attempts_stay_out_of_the_rate() {
        let records = vec![
            record("a", Outcome::Passed, 100),
            record("a", Outcome::Failed, 300),
            record("a", Outcome::Infrastructure, 0),
            record("a", Outcome::Ungraded, 200),
        ];
        let summary = summarize(&records);
        let group = &summary.tasks[0];
        assert_eq!(group.pass_rate, Some(0.5));
        assert_eq!(group.infrastructure, 1);
        // Tokens cover the attempts that reached the agent, failures included.
        assert_eq!(group.tokens.as_ref().unwrap().total, 600);
        assert_eq!(group.tokens_per_pass, Some(600.0));
        assert_eq!(exit_status(&summary), 1);
    }

    #[test]
    fn nothing_graded_has_no_rate_and_exits_three() {
        let summary = summarize(&[record("a", Outcome::Ungraded, 5)]);
        assert_eq!(summary.tasks[0].pass_rate, None);
        assert_eq!(summary.tasks[0].pass_rate_low, None);
        assert_eq!(exit_status(&summary), 3);
        assert_eq!(exit_status(&summarize(&[record("a", Outcome::Passed, 5)])), 0);
    }

    #[test]
    fn wilson_interval_is_wide_for_small_samples() {
        let (low, high) = wilson(3, 3).unwrap();
        assert!(low > 0.4 && low < 0.5, "{low}");
        assert!((high - 1.0).abs() < 1e-9);
        let (low, high) = wilson(50, 100).unwrap();
        assert!((low - 0.404).abs() < 0.01 && (high - 0.596).abs() < 0.01);
    }

    #[test]
    fn medians_handle_even_counts() {
        assert_eq!(Stats::of(vec![1, 3, 2, 10]).unwrap().median, 2.5);
        assert_eq!(Stats::of(vec![]), None);
    }

    #[test]
    fn inconsistent_or_malformed_results_are_infrastructure_failures() {
        let mut attempt = AttemptRecord::new("a", "m", None, None, 1, PathBuf::new());
        attempt.absorb_result(Some(0), Ok(json!({"id":"a","status":"completed","success":false})));
        assert_eq!(attempt.outcome, Outcome::Infrastructure);
        attempt.absorb_result(Some(0), Ok(json!([])));
        assert_eq!(attempt.outcome, Outcome::Infrastructure);
        attempt.absorb_result(Some(1), Ok(json!({"id":"a","status":"error","success":true})));
        assert_eq!(attempt.outcome, Outcome::Infrastructure);
        attempt.absorb_result(None, Err("missing".into()));
        assert_eq!(attempt.outcome, Outcome::Infrastructure);
    }

    #[test]
    fn a_result_fills_usage_and_counts_subagents() {
        let mut attempt = AttemptRecord::new("a", "m", None, None, 1, PathBuf::new());
        attempt.absorb_result(
            Some(0),
            Ok(json!({"id":"a","status":"completed","success":true,"elapsed_ms":1500,
                "account":"p:1","tool_calls":3,"tool_errors":1,
                "usage":{"requests":2,"input":10,"output":5,"cache_read":20,"cache_write":1,
                    "agents":[{"root":true},{"root":false}]},
                "checks":[{"index":0,"passed":true,"detail":"ok"}]})),
        );
        assert_eq!(attempt.outcome, Outcome::Passed);
        assert_eq!(attempt.tokens, 36);
        assert_eq!(attempt.subagents, 1);
        assert_eq!(attempt.account.as_deref(), Some("p:1"));
        assert_eq!(attempt.checks.len(), 1);
    }

    #[test]
    fn a_rejected_sign_in_is_not_an_agent_failure() {
        let mut attempt = AttemptRecord::new("a", "m", None, None, 1, PathBuf::new());
        attempt.absorb_result(
            Some(1),
            Ok(json!({"id":"a","status":"authentication_required","success":false})),
        );
        assert_eq!(attempt.outcome, Outcome::Infrastructure);
    }

    #[test]
    fn comparison_lists_shared_rows() {
        let base = summarize(&[record("a", Outcome::Passed, 100)]);
        let next = summarize(&[record("a", Outcome::Passed, 50)]);
        let text = compare(&base, &next);
        assert!(text.contains("100 → 50 (-50%)"), "{text}");
    }
}
