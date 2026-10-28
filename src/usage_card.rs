//! The `/usage` card: a transcript row the app makes, never a model message.
//! Its data is a snapshot taken when the command runs (the receipts'
//! figures from `usage::Card`, the account's limit windows if OMP has
//! reported them, the tokens-a-second readings), carried in the row's body
//! as JSON. [`items`] says what the card shows, top to bottom; the
//! transcript draws them and [`plain_text`] copies them.
use serde::{Deserialize, Serialize};

use crate::omp_runtime::{self, AuthProvider};
use crate::usage;

/// One provider limit window, as OMP's account usage reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Limit {
    pub(crate) label: String,
    pub(crate) used_percent: u8,
    pub(crate) resets_at_ms: Option<u64>,
}

/// One of an account's windows as the card shows it: a friendly title and
/// only the figures the provider reported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Window {
    pub(crate) title: String,
    pub(crate) used_percent: Option<u8>,
    pub(crate) resets_at_ms: Option<u64>,
    /// Money spent in the window, and its cap, in cents.
    pub(crate) spent_cents: Option<u64>,
    pub(crate) cap_cents: Option<u64>,
}

/// One provider account on the card: who it is, its plan, and its windows,
/// as OMP's `get_account_usage` last reported them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AccountBlock {
    pub(crate) id: String,
    /// The user's label for the account, or its identity.
    pub(crate) name: String,
    /// "Claude subscription", "ChatGPT subscription", "OpenAI API key".
    pub(crate) kind: String,
    /// Which provider, for grouping `/usage all`.
    pub(crate) provider: String,
    /// The plan as a person names it; `None` when the provider did not say.
    pub(crate) plan: Option<String>,
    pub(crate) windows: Vec<Window>,
    /// Why the provider's usage could not be read.
    pub(crate) error: Option<String>,
    /// Whether OMP has answered for this account at all.
    pub(crate) reported: bool,
}

/// What a card row carries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Body {
    /// When the snapshot was taken, for "resets in".
    pub(crate) at_ms: u64,
    /// The receipts' figures; `None` while they are being read.
    pub(crate) card: Option<usage::Card>,
    /// Journal stem to the name the Agents panel gives it.
    pub(crate) labels: Vec<(String, String)>,
    /// Recent output tokens a second, oldest first.
    pub(crate) rates: Vec<f64>,
    pub(crate) limits: Vec<Limit>,
    /// Which account the limits are for.
    pub(crate) account: Option<String>,
    /// Said above the figures, e.g. that `/usage` takes no arguments.
    pub(crate) note: Option<String>,
    /// `/usage all`: every signed-in account, not only the session's.
    pub(crate) all: bool,
    /// The accounts the Subscription (or Accounts) section shows.
    pub(crate) accounts: Vec<AccountBlock>,
    /// A fresh reading of the accounts is on its way.
    pub(crate) refreshing: bool,
    /// When OMP was last asked for the accounts' usage, wall-clock ms.
    pub(crate) fetched_ms: Option<u64>,
    /// Said under the section's heading, e.g. that the session must start.
    pub(crate) accounts_note: Option<String>,
}

impl Body {
    pub(crate) fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub(crate) fn decode(text: &str) -> Body {
        serde_json::from_str(text).unwrap_or_default()
    }
}

pub(crate) fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0 => format!("{:.1}s", ms as f64 / 1000.0),
        1..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

fn percent(fraction: f64) -> String {
    let p = fraction * 100.0;
    if p > 0.0 && p < 1.0 { "<1%".into() } else { format!("{:.0}%", p) }
}

fn dollars(nano: u64) -> String {
    let usd = nano as f64 / 1e9;
    if usd < 0.01 && nano > 0 { "<$0.01".into() } else { format!("${usd:.2}") }
}

/// "Resets in 1 hr 12 min", "Resets in 3 days 4 hr", "Resets in 40 min".
pub(crate) fn resets_in(at_ms: u64, now_ms: u64) -> String {
    if at_ms <= now_ms {
        return "Resets now".into();
    }
    let minutes = (at_ms - now_ms).div_ceil(60_000);
    let (days, hours, minutes) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    let plural = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let span = match (days, hours, minutes) {
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} hr"),
        (0, h, m) => format!("{h} hr {m} min"),
        (d, 0, _) => plural(d, "day", "days"),
        (d, h, _) => format!("{} {h} hr", plural(d, "day", "days")),
    };
    format!("Resets in {span}")
}

/// "Updated just now", "Updated 2 min ago", "Updated 3 hr ago".
pub(crate) fn updated_ago(fetched_ms: u64, now_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(fetched_ms) / 60_000;
    match minutes {
        0 => "Updated just now".into(),
        1..=59 => format!("Updated {minutes} min ago"),
        60..=1439 => format!("Updated {} hr ago", minutes / 60),
        _ => {
            let days = minutes / 1440;
            format!("Updated {days} day{} ago", if days == 1 { "" } else { "s" })
        }
    }
}

/// "$12.40", with thousands grouped.
fn cents(amount: u64) -> String {
    crate::rules::dollars(amount)
}

/// A window's name as a person reads it: "5-hour limit", "Weekly · all
/// models", "Weekly · Fable", "Extra usage". OMP labels a window by its
/// span ("5 hours", "Claude 7 Day"), with a feature in parentheses when it
/// is scoped to one; the scope is `shared` for the account as a whole.
pub(crate) fn window_title(label: &str, scope: Option<&str>, money: bool) -> String {
    if scope == Some("extra") {
        return "Extra usage".into();
    }
    let span = crate::rules::usage_row_title(label);
    let has_span = label.chars().any(|c| c.is_ascii_digit());
    // "Weekly limit" → "Weekly"; "5-hour limit" → "5-hour".
    let short = span.strip_suffix(" limit").unwrap_or(&span).to_owned();
    let feature = match scope {
        None | Some("shared") => None,
        Some(scope) => Some(crate::rules::parenthetical(label).map_or_else(|| crate::rules::capitalized(scope), str::to_owned)),
    };
    match (feature, has_span) {
        (Some(feature), true) => format!("{short} · {feature}"),
        (Some(feature), false) => format!("{feature} · {}", if money { "spending" } else { "limit" }),
        (None, true) if short == "Weekly" => "Weekly · all models".into(),
        (None, true) => span,
        // No span to name: the provider's own words.
        (None, false) if money => "Spending".into(),
        (None, false) => label.trim().to_owned(),
    }
}

/// The block for one account, from what OMP last reported for it (`None`
/// when it has not answered yet). Only reported figures are carried.
pub(crate) fn account_block(
    id: &str,
    name: &str,
    provider: AuthProvider,
    usage: Option<&omp_runtime::AccountUsage>,
) -> AccountBlock {
    let mut block = AccountBlock {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: provider.account_noun().to_owned(),
        provider: provider.product_name().to_owned(),
        ..AccountBlock::default()
    };
    let Some(usage) = usage else { return block };
    block.reported = true;
    block.plan = usage
        .plan
        .as_deref()
        .filter(|plan| !plan.trim().is_empty())
        .map(|plan| crate::rules::plan_display_name(provider, Some(plan)));
    block.error = usage.error.as_ref().map(|error| {
        if error.contains("credential_unavailable") {
            "this sign-in has no credential OMP can ask the provider's usage service with".to_owned()
        } else {
            error.trim().to_owned()
        }
    });
    block.windows = usage
        .windows
        .iter()
        .map(|window| Window {
            title: window_title(&window.label, window.scope.as_deref(), window.spend.is_some()),
            used_percent: window.used_percent,
            resets_at_ms: window.resets_at_ms,
            spent_cents: window.spend.map(|spend| spend.used_cents),
            cap_cents: window.spend.and_then(|spend| spend.limit_cents),
        })
        .collect();
    block
}

/// A window's used fraction: the provider's percent, or spend over cap.
fn window_fraction(window: &Window) -> Option<f64> {
    if let Some(percent) = window.used_percent {
        return Some(f64::from(percent.min(100)) / 100.0);
    }
    match (window.spent_cents, window.cap_cents) {
        (Some(spent), Some(cap)) if cap > 0 => Some((spent as f64 / cap as f64).min(1.0)),
        _ => None,
    }
}

/// What a window's figure says: "34%", "$12.40 of $50.00", "$3.10 spent".
fn window_figure(window: &Window) -> Option<String> {
    match (window.spent_cents, window.cap_cents) {
        (Some(spent), Some(cap)) => Some(format!("{} of {}", cents(spent), cents(cap))),
        (Some(spent), None) => Some(format!("{} spent", cents(spent))),
        _ => window.used_percent.map(|percent| format!("{percent}%")),
    }
}

fn tone_for(fraction: f64) -> Tone {
    if fraction >= 0.85 { Tone::Danger } else { Tone::Accent }
}

/// The cards made before accounts had blocks carried bare limit windows.
fn legacy_blocks(body: &Body) -> Vec<AccountBlock> {
    if body.limits.is_empty() {
        return Vec::new();
    }
    vec![AccountBlock {
        id: body.account.clone().unwrap_or_default(),
        name: body.account.clone().unwrap_or_default(),
        reported: true,
        windows: body
            .limits
            .iter()
            .map(|limit| Window {
                title: limit.label.clone(),
                used_percent: Some(limit.used_percent),
                resets_at_ms: limit.resets_at_ms,
                ..Window::default()
            })
            .collect(),
        ..AccountBlock::default()
    }]
}

fn agent_name(body: &Body, agent: &usage::AgentUse) -> String {
    if agent.root {
        return "This session".into();
    }
    body.labels
        .iter()
        .find(|(stem, _)| *stem == agent.session)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| format!("Sub-agent {}", agent.session.chars().take(8).collect::<String>()))
}

/// The "This session" grid: (label, value), only figures the receipts carry.
fn session_cells(card: &usage::Card) -> Vec<(String, String)> {
    let mut cells = Vec::new();
    if let Some(cost) = card.cost_nano_usd {
        let partial = if card.cost_requests < card.requests { "*" } else { "" };
        cells.push(("Cost (est.)".into(), format!("{}{partial}", dollars(cost))));
    }
    if let Some(api) = card.api_ms {
        cells.push(("API time".into(), duration(api)));
    }
    if let Some(active) = card.active_ms {
        cells.push(("Active".into(), duration(active)));
    }
    if let Some(hit) = card.cache_hit() {
        cells.push(("Cache hit".into(), percent(hit)));
    }
    let total = card.tokens().max(1) as f64;
    for model in card.models.iter().take(4) {
        cells.push((model.model.clone(), percent(model.tokens() as f64 / total)));
    }
    cells
}

fn tip(card: &usage::Card) -> Option<String> {
    if let Some(share) = card.subagent_share()
        && share >= 0.5
    {
        return Some("Sub-agents made most of this session's tokens. Each runs its own requests: a cheaper model or a tighter brief for simple ones goes furthest.".into());
    }
    if let Some(hit) = card.cache_hit()
        && hit < 0.5
        && card.requests >= 3
    {
        return Some("Under half of the input came from the cache. Switching model, account or tools mid-session starts the cache over.".into());
    }
    None
}


/// Most agents listed by name; the rest are summed on one line.
const AGENTS: usize = 6;

/// How a bar is coloured: the accent, a sub-agent's share, or nearly spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Accent,
    Agent,
    Danger,
}

/// One thing the card shows, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Item {
    /// What is being read, with a spinner.
    Loading(String),
    /// A line in the quieter caption style.
    Caption(String),
    /// A section's heading, with a note at its right (when the accounts
    /// were read) and whether a reading is on its way.
    Section { title: String, aside: Option<String>, refreshing: bool },
    /// `/usage all`'s heading over one provider's accounts.
    Group(String),
    /// An account: who, and its plan at the right.
    Account { name: String, plan: Option<String> },
    /// The provider would not say; a Retry asks again.
    Error(String),
    /// A limit window. `compact` lays it on one line (`/usage all`).
    Window { title: String, resets: String, figure: String, fraction: Option<f64>, tone: Tone, compact: bool },
    /// The figures that lead the card: (label, value).
    Chips(Vec<(String, String)>),
    /// A grid of (label, value).
    Cells(Vec<(String, String)>),
    /// One model's figures.
    Breakdown { model: String, rows: Vec<(String, String)> },
    /// What part of the tokens one agent made.
    Share { name: String, fraction: f64, tone: Tone },
    /// Output speed: the latest and peak readings, and all of them.
    Speed { latest: f64, peak: f64, rates: Vec<f64> },
    Tip(String),
}

fn accounts_section(items: &mut Vec<Item>, body: &Body) {
    let legacy = legacy_blocks(body);
    let blocks = if body.accounts.is_empty() { &legacy } else { &body.accounts };
    if blocks.is_empty() && !body.all && body.accounts_note.is_none() {
        return;
    }
    let title = if body.all || blocks.len() > 1 { "Accounts" } else { "Subscription" };
    items.push(Item::Section {
        title: title.into(),
        aside: body.fetched_ms.map(|at| updated_ago(at, body.at_ms)),
        refreshing: body.refreshing,
    });
    if let Some(note) = &body.accounts_note {
        items.push(Item::Caption(note.clone()));
    }
    if blocks.is_empty() {
        let none = if body.all { "No accounts are signed in." } else { "This session has no provider account." };
        items.push(Item::Caption(none.into()));
        return;
    }
    let mut provider: Option<&str> = None;
    for block in blocks {
        if body.all && provider != Some(block.provider.as_str()) {
            // `/usage all` groups by provider under a small heading.
            provider = Some(block.provider.as_str());
            let group = if block.kind.is_empty() { block.provider.clone() } else { format!("{}s", block.kind) };
            items.push(Item::Group(group.to_uppercase()));
        }
        let name = if body.all || block.kind.is_empty() { block.name.clone() } else { format!("{} · {}", block.name, block.kind) };
        items.push(Item::Account { name, plan: block.plan.clone() });
        if let Some(error) = &block.error {
            items.push(Item::Error(format!("Couldn't read usage: {error}")));
        } else if !block.reported {
            let said = if body.refreshing { "Asking the provider…" } else { "Not read yet." };
            items.push(Item::Caption(said.into()));
        } else if block.windows.is_empty() {
            items.push(Item::Caption("The provider reported no usage windows.".into()));
        }
        for window in &block.windows {
            let fraction = window_fraction(window);
            items.push(Item::Window {
                title: window.title.clone(),
                resets: window.resets_at_ms.map(|at| resets_in(at, body.at_ms)).unwrap_or_default(),
                figure: window_figure(window).unwrap_or_default(),
                fraction,
                tone: fraction.map_or(Tone::Accent, tone_for),
                compact: body.all,
            });
        }
    }
}

/// What the card shows, top to bottom.
pub(crate) fn items(body: &Body) -> Vec<Item> {
    let mut items = Vec::new();
    let Some(card) = &body.card else {
        items.push(Item::Loading("Reading recorded requests…".into()));
        return items;
    };
    let children = card.agents.iter().filter(|a| !a.root).count();
    let mut scope = if children > 0 {
        format!("This session and {children} sub-agent{}", if children == 1 { "" } else { "s" })
    } else {
        "This session".into()
    };
    scope.push_str(&format!(" · {} request{}", card.requests, if card.requests == 1 { "" } else { "s" }));
    items.push(Item::Caption(scope));
    if let Some(note) = &body.note {
        items.push(Item::Caption(note.clone()));
    }
    // The account's plan and limit windows, when the provider has said.
    accounts_section(&mut items, body);
    if card.requests == 0 {
        items.push(Item::Caption("No requests recorded for this session yet.".into()));
        return items;
    }
    // Cache reads only when the receipts reported them.
    let mut chips =
        vec![("Input tokens".into(), usage::compact_number(card.input)), ("Output tokens".into(), usage::compact_number(card.output))];
    if card.cache_read_known {
        chips.push(("Cache reads".into(), usage::compact_number(card.cache_read)));
    }
    chips.push(("Requests".into(), card.requests.to_string()));
    items.push(Item::Chips(chips));
    let cells = session_cells(card);
    if !cells.is_empty() {
        items.push(Item::Section { title: "This session".into(), aside: None, refreshing: false });
        items.push(Item::Cells(cells));
    }
    for model in card.models.iter().take(3) {
        let mut rows = vec![("Input".into(), usage::compact_number(model.input)), ("Output".into(), usage::compact_number(model.output))];
        if card.cache_read_known {
            rows.push(("Cache read".into(), usage::compact_number(model.cache_read)));
        }
        if card.cache_write_known {
            rows.push(("Cache write".into(), usage::compact_number(model.cache_write)));
        }
        rows.push(("Requests".into(), model.requests.to_string()));
        items.push(Item::Breakdown { model: model.model.clone(), rows });
    }
    // What the tokens went to: the session and each sub-agent.
    if children > 0 {
        items.push(Item::Section { title: "What's using this session?".into(), aside: None, refreshing: false });
        let total = card.tokens().max(1) as f64;
        let named = card.agents.len().min(AGENTS);
        for agent in &card.agents[..named] {
            items.push(Item::Share {
                name: agent_name(body, agent),
                fraction: agent.tokens as f64 / total,
                tone: if agent.root { Tone::Accent } else { Tone::Agent },
            });
        }
        if card.agents.len() > named {
            let rest: u64 = card.agents[named..].iter().map(|a| a.tokens).sum();
            items.push(Item::Share {
                name: format!("{} more sub-agents", card.agents.len() - named),
                fraction: rest as f64 / total,
                tone: Tone::Agent,
            });
        }
    }
    if !body.rates.is_empty() {
        let latest = body.rates.last().copied().unwrap_or(0.0).round();
        let peak = body.rates.iter().copied().fold(0.0_f64, f64::max).round();
        items.push(Item::Speed { latest, peak, rates: body.rates.clone() });
    }
    if let Some(tip) = tip(card) {
        items.push(Item::Tip(tip));
    }
    // What is not shown, and why.
    if !body.all && body.accounts.is_empty() && body.limits.is_empty() && body.accounts_note.is_none() {
        items.push(Item::Caption("No subscription usage: this session has no provider account.".into()));
    }
    if card.cost_nano_usd.is_some() {
        items.push(Item::Caption(format!(
            "Cost is the model catalog's estimate over {} of {} requests, not an invoice.",
            card.cost_requests, card.requests
        )));
    }
    if card.warnings > 0 {
        items.push(Item::Caption(format!(
            "{} journal note{}: totals may be partial (the Usage window has details).",
            card.warnings,
            if card.warnings == 1 { "" } else { "s" }
        )));
    }
    items
}

/// The card as plain text: what Copy puts on the clipboard.
pub(crate) fn plain_text(body: &Body) -> String {
    let mut lines = vec!["Usage".to_owned()];
    for item in items(body) {
        match item {
            Item::Loading(text) | Item::Caption(text) | Item::Group(text) | Item::Error(text) => lines.push(text),
            Item::Section { title, aside, .. } => {
                lines.push(match aside {
                    Some(aside) => format!("{title}  {aside}"),
                    None => title,
                });
            }
            Item::Account { name, plan } => lines.push(match plan {
                Some(plan) => format!("{name}  {plan}"),
                None => name,
            }),
            Item::Window { title, resets, figure, .. } => {
                let parts: Vec<String> = [title, resets, figure].into_iter().filter(|s| !s.is_empty()).collect();
                lines.push(parts.join("  "));
            }
            Item::Chips(pairs) | Item::Cells(pairs) => {
                lines.extend(pairs.into_iter().map(|(label, value)| format!("{label}  {value}")));
            }
            Item::Breakdown { model, rows } => {
                lines.push(format!("Breakdown  {model}"));
                lines.extend(rows.into_iter().map(|(label, value)| format!("{label}  {value}")));
            }
            Item::Share { name, fraction, .. } => lines.push(format!("{name}  {}", percent(fraction))),
            Item::Speed { latest, peak, .. } => lines.push(format!("Output speed  {latest:.0} tok/s · peak {peak:.0}")),
            Item::Tip(tip) => lines.push(format!("Tip  {tip}")),
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Body {
        let card = usage::Card {
            journals: 3,
            requests: 12,
            input: 1_700,
            output: 81_000,
            cache_read: 382_700,
            cache_write: 8_900,
            cache_read_known: true,
            cache_write_known: true,
            api_ms: Some(125_000),
            active_ms: Some(90_000),
            timed_requests: 12,
            models: vec![usage::ModelUse { model: "opus".into(), requests: 12, input: 1_700, output: 81_000, cache_read: 382_700, cache_write: 8_900 }],
            agents: vec![
                usage::AgentUse { session: "root".into(), root: true, requests: 4, tokens: 100_000 },
                usage::AgentUse { session: "kid".into(), root: false, requests: 8, tokens: 374_300 },
            ],
            ..usage::Card::default()
        };
        Body { at_ms: 0, card: Some(card), labels: vec![("kid".into(), "scout · map the repo".into())], rates: vec![40.0, 80.0, 60.0], ..Body::default() }
    }

    #[test]
    fn a_body_round_trips_through_its_row() {
        let body = sample();
        assert_eq!(Body::decode(&body.encode()), body);
    }

    #[test]
    fn plain_text_says_only_what_the_receipts_do() {
        let text = plain_text(&sample());
        assert!(text.contains("Input tokens"), "{text}");
        assert!(text.contains("1.7k"), "{text}");
        assert!(text.contains("scout · map the repo"), "{text}");
        assert!(text.contains("Cache hit"), "{text}");
        assert!(text.contains("peak 80"), "{text}");
        // No cost in the receipts: no cost line.
        assert!(!text.contains("Cost"), "{text}");
        assert!(text.contains("No subscription usage: this session has no provider account"), "{text}");
        // A card saved before account blocks keeps its bare limit windows.
        let mut limited = sample();
        limited.at_ms = 1_000;
        limited.limits = vec![Limit { label: "5 hours".into(), used_percent: 30, resets_at_ms: Some(1_000 + 72 * 60_000) }];
        let text = plain_text(&limited);
        assert!(text.contains("5 hours") && text.contains("30%") && text.contains("Resets in 1 hr 12 min"), "{text}");
        assert!(!text.contains("No subscription usage"), "{text}");
        // Loading says so and nothing else.
        let loading = plain_text(&Body::default());
        assert!(loading.contains("Reading recorded requests"));
        assert!(!loading.contains("tokens"));
    }

    const NOW: u64 = 1_800_000_000_000;

    fn window(label: &str, scope: &str, used: Option<u8>, resets_in_min: Option<u64>, spend: Option<(u64, Option<u64>)>) -> omp_runtime::UsageWindow {
        omp_runtime::UsageWindow {
            label: label.into(),
            scope: Some(scope.into()),
            used_percent: used,
            resets_at_ms: resets_in_min.map(|m| NOW + m * 60_000),
            spend: spend.map(|(used_cents, limit_cents)| omp_runtime::Spend { used_cents, limit_cents }),
        }
    }

    fn claude() -> omp_runtime::AccountUsage {
        omp_runtime::AccountUsage {
            account_id: "anthropic:w".into(),
            plan: Some("max_20x".into()),
            windows: vec![
                window("Claude 5 Hour", "shared", Some(34), Some(72), None),
                window("Claude 7 Day", "shared", Some(61), Some(3 * 1440 + 4 * 60), None),
                window("Claude 7 Day (Fable)", "fable", Some(88), Some(3 * 1440 + 4 * 60), None),
                window("Claude Extra Usage", "extra", None, None, Some((1_240, Some(5_000)))),
            ],
            error: None,
        }
    }

    fn with_accounts(accounts: Vec<AccountBlock>, all: bool) -> Body {
        Body { at_ms: NOW, all, accounts, fetched_ms: Some(NOW - 2 * 60_000), ..sample() }
    }

    fn windows(body: &Body) -> Vec<(f64, Tone)> {
        items(body)
            .into_iter()
            .filter_map(|item| match item {
                Item::Window { fraction: Some(fraction), tone, .. } => Some((fraction, tone)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn friendly_window_titles() {
        assert_eq!(window_title("Claude 5 Hour", Some("shared"), false), "5-hour limit");
        assert_eq!(window_title("5 hours", None, false), "5-hour limit");
        assert_eq!(window_title("Claude 7 Day", Some("shared"), false), "Weekly · all models");
        assert_eq!(window_title("7 days", Some("shared"), false), "Weekly · all models");
        assert_eq!(window_title("Claude 7 Day (Fable)", Some("fable"), false), "Weekly · Fable");
        assert_eq!(window_title("7 days (Spark)", Some("spark"), false), "Weekly · Spark");
        assert_eq!(window_title("5 hours (Spark)", Some("spark"), false), "5-hour · Spark");
        assert_eq!(window_title("7 days", Some("opus"), false), "Weekly · Opus");
        assert_eq!(window_title("30 days", Some("shared"), false), "Monthly limit");
        assert_eq!(window_title("Claude Extra Usage", Some("extra"), true), "Extra usage");
        assert_eq!(window_title("Monthly spend", Some("shared"), true), "Spending");
        assert_eq!(window_title("Burst credits", None, false), "Burst credits");
    }

    #[test]
    fn reset_and_freshness_times() {
        assert_eq!(resets_in(NOW + 72 * 60_000, NOW), "Resets in 1 hr 12 min");
        assert_eq!(resets_in(NOW + 40 * 60_000, NOW), "Resets in 40 min");
        assert_eq!(resets_in(NOW + 30_000, NOW), "Resets in 1 min");
        assert_eq!(resets_in(NOW + 3 * 3_600_000, NOW), "Resets in 3 hr");
        assert_eq!(resets_in(NOW + (1440 + 125) * 60_000, NOW), "Resets in 1 day 2 hr");
        assert_eq!(resets_in(NOW + 17 * 1440 * 60_000, NOW), "Resets in 17 days");
        assert_eq!(resets_in(NOW - 1, NOW), "Resets now");
        assert_eq!(updated_ago(NOW - 20_000, NOW), "Updated just now");
        assert_eq!(updated_ago(NOW - 2 * 60_000 - 5_000, NOW), "Updated 2 min ago");
        assert_eq!(updated_ago(NOW - 3 * 3_600_000, NOW), "Updated 3 hr ago");
        assert_eq!(updated_ago(NOW - 26 * 3_600_000, NOW), "Updated 1 day ago");
    }

    #[test]
    fn percent_windows_show_every_window_with_bar_reset_and_percent() {
        let block = account_block("anthropic:w", "Work", AuthProvider::Anthropic, Some(&claude()));
        assert_eq!(block.plan.as_deref(), Some("Claude Max 20×"));
        assert_eq!(block.windows.len(), 4);
        let body = with_accounts(vec![block], false);
        let text = plain_text(&body);
        for expected in ["Subscription", "Updated 2 min ago", "Work · Claude subscription", "Claude Max 20×", "5-hour limit", "Resets in 1 hr 12 min", "34%", "Weekly · all models", "61%", "Weekly · Fable", "88%", "Resets in 3 days 4 hr"] {
            assert!(text.contains(expected), "{expected} missing from {text}");
        }
        // Bars: the accent below 85%, the danger colour from it.
        let bars = windows(&body);
        assert_eq!(bars[0], (0.34, Tone::Accent));
        assert_eq!(bars[2], (0.88, Tone::Danger));
    }

    #[test]
    fn spend_windows_with_and_without_a_cap() {
        let capped = account_block("anthropic:w", "Work", AuthProvider::Anthropic, Some(&claude()));
        let body = with_accounts(vec![capped], false);
        let text = plain_text(&body);
        assert!(text.contains("Extra usage") && text.contains("$12.40 of $50.00"), "{text}");
        assert!(windows(&body).iter().any(|(fraction, _)| (*fraction - 0.248).abs() < 1e-9));
        let uncapped = omp_runtime::AccountUsage {
            account_id: "openai:k".into(),
            plan: None,
            windows: vec![window("Monthly spend", "shared", None, None, Some((310, None)))],
            error: None,
        };
        let block = account_block("openai:k", "Batch", AuthProvider::OpenAiApi, Some(&uncapped));
        // No plan reported: none shown, nothing made up.
        assert_eq!(block.plan, None);
        let body = with_accounts(vec![block], false);
        let text = plain_text(&body);
        assert!(text.contains("Spending  $3.10 spent\n"), "{text}");
        // Nothing to measure against: no bar.
        assert!(windows(&body).is_empty());
    }

    #[test]
    fn an_error_is_said_plainly() {
        let failed = omp_runtime::AccountUsage {
            account_id: "anthropic:h".into(),
            plan: None,
            windows: Vec::new(),
            error: Some("usage endpoint answered 429".into()),
        };
        let body = with_accounts(vec![account_block("anthropic:h", "Home", AuthProvider::Anthropic, Some(&failed))], false);
        assert!(items(&body).contains(&Item::Error("Couldn't read usage: usage endpoint answered 429".into())));
    }

    #[test]
    fn no_windows_and_not_yet_read_say_only_that() {
        let empty = omp_runtime::AccountUsage { account_id: "openai-codex:s".into(), plan: None, windows: Vec::new(), error: None };
        let body = with_accounts(vec![account_block("openai-codex:s", "Side", AuthProvider::OpenAi, Some(&empty))], false);
        let text = plain_text(&body);
        assert!(text.contains("The provider reported no usage windows."), "{text}");
        assert!(items(&body).contains(&Item::Account { name: "Side · ChatGPT subscription".into(), plan: None }), "no plan was reported: {text}");
        // Not answered yet: waiting, or not read.
        let mut body = with_accounts(vec![account_block("openai-codex:s", "Side", AuthProvider::OpenAi, None)], false);
        body.refreshing = true;
        assert!(plain_text(&body).contains("Asking the provider…"));
        assert!(items(&body).iter().any(|item| matches!(item, Item::Section { refreshing: true, .. })));
        // A history-only session says why nothing is fresh.
        body.refreshing = false;
        body.accounts_note = Some("Start the session to read current usage.".into());
        let text = plain_text(&body);
        assert!(text.contains("Start the session to read current usage.") && text.contains("Not read yet."), "{text}");
    }

    #[test]
    fn usage_all_groups_several_accounts_by_provider() {
        let codex = omp_runtime::AccountUsage {
            account_id: "openai-codex:s".into(),
            plan: Some("plus".into()),
            windows: vec![window("5 hours", "shared", Some(12), Some(215), None), window("7 days (Spark)", "spark", Some(5), Some(7000), None)],
            error: None,
        };
        let blocks = vec![
            account_block("openai-codex:s", "Side", AuthProvider::OpenAi, Some(&codex)),
            account_block("anthropic:w", "Work", AuthProvider::Anthropic, Some(&claude())),
            account_block("anthropic:h", "Home", AuthProvider::Anthropic, None),
        ];
        let body = with_accounts(blocks, true);
        let lines: Vec<String> = plain_text(&body).lines().map(str::to_owned).collect();
        let position = |text: &str| lines.iter().position(|l| l.starts_with(text)).unwrap_or_else(|| panic!("{text} missing: {lines:?}"));
        assert!(position("Accounts") < position("CHATGPT SUBSCRIPTIONS"));
        assert!(position("CHATGPT SUBSCRIPTIONS") < position("Side"));
        assert!(position("Side") < position("CLAUDE SUBSCRIPTIONS"));
        assert!(position("CLAUDE SUBSCRIPTIONS") < position("Work"));
        assert!(position("Work") < position("Home"));
        assert_eq!(lines.iter().filter(|l| *l == "CLAUDE SUBSCRIPTIONS").count(), 1);
        assert!(lines.iter().any(|l| l == "Side  ChatGPT Plus"));
        assert!(lines.iter().any(|l| l.starts_with("Weekly · Spark")));
        assert!(items(&body).iter().all(|item| !matches!(item, Item::Window { compact: false, .. })));
        // None signed in says so.
        let none = plain_text(&with_accounts(Vec::new(), true));
        assert!(none.contains("No accounts are signed in."), "{none}");
    }
}
