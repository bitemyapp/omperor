//! The `/usage` card: a transcript row the app makes, never a model message.
//! Its data is a snapshot taken when the command runs (the receipts'
//! figures from `usage::Card`, the account's limit windows if OMP has
//! reported them, the tokens-a-second readings), carried in the row's body
//! as JSON. Layout is pure arithmetic over that data and the width, so the
//! row's height is exact before any view exists and the same at every
//! pass; only `view` touches AppKit.
use super::*;

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Title,
    Section,
    Label,
    Value,
    Figure { lead: bool },
    Caption,
    /// A failure said plainly, in the danger colour.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Gold,
    Plum,
    Danger,
}

/// One thing drawn in the card, in a top-down frame.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Item {
    Text { frame: [f64; 4], text: String, role: Role, right: bool },
    Bar { frame: [f64; 4], fraction: f64, tone: Tone },
    Chip { frame: [f64; 4], lead: bool },
    Rule { frame: [f64; 4] },
    Spark { frame: [f64; 4] },
    Spinner { frame: [f64; 4] },
    Copy { frame: [f64; 4] },
    /// Asks OMP for the accounts' usage again.
    Retry { frame: [f64; 4] },
}

const PAD: f64 = 18.0;
const LINE: f64 = 20.0;
const GAP: f64 = 14.0;
const CHIP: f64 = 50.0;
/// Most agents listed by name; the rest are summed on one line.
const AGENTS: usize = 6;
/// A caption wraps at about this many points a character.
const CAPTION_CHAR: f64 = 6.6;
/// A caption's width a character, on average, for placing beside it.
const STAMP_CHAR: f64 = 6.3;

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
    super::dollars(amount)
}

/// A window's name as a person reads it: "5-hour limit", "Weekly · all
/// models", "Weekly · Fable", "Extra usage". OMP labels a window by its
/// span ("5 hours", "Claude 7 Day"), with a feature in parentheses when it
/// is scoped to one; the scope is `shared` for the account as a whole.
pub(crate) fn window_title(label: &str, scope: Option<&str>, money: bool) -> String {
    if scope == Some("extra") {
        return "Extra usage".into();
    }
    let span = super::usage_row_title(label);
    let has_span = label.chars().any(|c| c.is_ascii_digit());
    // "Weekly limit" → "Weekly"; "5-hour limit" → "5-hour".
    let short = span.strip_suffix(" limit").unwrap_or(&span).to_owned();
    let feature = match scope {
        None | Some("shared") => None,
        Some(scope) => Some(super::parenthetical(label).map_or_else(|| super::capitalized(scope), str::to_owned)),
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
        .map(|plan| super::plan_display_name(provider, Some(plan)));
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
    if fraction >= 0.85 { Tone::Danger } else { Tone::Gold }
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

/// The Subscription section (or, for `/usage all`, Accounts): each account
/// with its plan and every window it reported. Returns the new `y`.
fn accounts_section(items: &mut Vec<Item>, body: &Body, x: f64, w: f64, mut y: f64) -> f64 {
    let legacy = legacy_blocks(body);
    let blocks = if body.accounts.is_empty() { &legacy } else { &body.accounts };
    if blocks.is_empty() && !body.all && body.accounts_note.is_none() {
        return y;
    }
    let text = |items: &mut Vec<Item>, frame: [f64; 4], text: String, role: Role, right: bool| {
        items.push(Item::Text { frame, text, role, right });
    };
    let heading = if body.all || blocks.len() > 1 { "Accounts" } else { "Subscription" };
    text(items, [x, y, w * 0.5, LINE], heading.into(), Role::Section, false);
    // The freshness line and its spinner keep their place whether or not a
    // reading is on its way, so the answer never moves anything.
    let stamp = body.fetched_ms.map(|at| updated_ago(at, body.at_ms)).unwrap_or_default();
    let stamp_w = 150.0_f64.min(w * 0.5 - 20.0);
    text(items, [x + w - stamp_w, y + 1.0, stamp_w, LINE], stamp.clone(), Role::Caption, true);
    if body.refreshing {
        // Just before the stamp's words, estimated from their length.
        let words = (stamp.chars().count() as f64 * STAMP_CHAR).min(stamp_w);
        let gap = if stamp.is_empty() { 0.0 } else { 6.0 };
        items.push(Item::Spinner { frame: [x + w - words - gap - 12.0, y + 4.0, 12.0, 12.0] });
    }
    y += LINE + 4.0;
    if let Some(note) = &body.accounts_note {
        let lines = wrapped_lines(note, w) as f64;
        text(items, [x, y, w, lines * 16.0 + 2.0], note.clone(), Role::Caption, false);
        y += lines * 16.0 + 6.0;
    }
    if blocks.is_empty() {
        let none = if body.all { "No accounts are signed in." } else { "This session has no provider account." };
        text(items, [x, y, w, LINE], none.into(), Role::Caption, false);
        return y + LINE + GAP;
    }
    // One column for every compact figure, wide enough for "$1,234.56 of
    // $5,000.00" when any window is money, so the bars line up.
    let money = blocks.iter().flat_map(|block| &block.windows).any(|window| window.spent_cents.is_some());
    let compact_figure_w = if money { 150.0 } else { 56.0 };
    let mut provider: Option<&str> = None;
    for (index, block) in blocks.iter().enumerate() {
        if body.all && provider != Some(block.provider.as_str()) {
            // `/usage all` groups by provider under a small heading.
            if index > 0 {
                y += 4.0;
            }
            provider = Some(block.provider.as_str());
            let group = if block.kind.is_empty() { block.provider.clone() } else { format!("{}s", block.kind) };
            text(items, [x, y, w, 16.0], group.to_uppercase(), Role::Caption, false);
            y += 18.0;
            items.push(Item::Rule { frame: [x, y, w, 1.0] });
            y += 5.0;
        } else if index > 0 {
            y += 6.0;
        }
        // Who: the account's name and kind; its plan on the right.
        let who = if body.all || block.kind.is_empty() { block.name.clone() } else { format!("{} · {}", block.name, block.kind) };
        let plan_w = if block.plan.is_some() { (w * 0.4).min(200.0) } else { 0.0 };
        text(items, [x, y, w - plan_w - 8.0, LINE], who, Role::Value, false);
        if let Some(plan) = &block.plan {
            text(items, [x + w - plan_w, y, plan_w, LINE], plan.clone(), Role::Label, true);
        }
        y += LINE + 4.0;
        if let Some(error) = &block.error {
            let message = format!("Couldn't read usage: {error}");
            let text_w = w - 64.0;
            let lines = wrapped_lines(&message, text_w) as f64;
            text(items, [x, y, text_w, lines * 16.0 + 2.0], message, Role::Error, false);
            items.push(Item::Retry { frame: [x + w - 56.0, y - 2.0, 56.0, 20.0] });
            y += lines * 16.0 + 8.0;
        } else if !block.reported {
            let said = if body.refreshing { "Asking the provider…" } else { "Not read yet." };
            text(items, [x, y, w, LINE], said.into(), Role::Caption, false);
            y += LINE + 2.0;
        } else if block.windows.is_empty() {
            text(items, [x, y, w, LINE], "The provider reported no usage windows.".into(), Role::Caption, false);
            y += LINE + 2.0;
        }
        if body.all {
            // Compact: one line a window, a short bar before its figure.
            let bar_w = (w * 0.24).clamp(60.0, 150.0);
            let figure_w = compact_figure_w;
            for window in &block.windows {
                let fraction = window_fraction(window);
                let resets = window.resets_at_ms.map(|at| resets_in(at, body.at_ms)).unwrap_or_default();
                let title_w = (w - bar_w - figure_w - 12.0) * 0.5;
                text(items, [x, y, title_w, LINE], window.title.clone(), Role::Label, false);
                text(items, [x + title_w, y, w - bar_w - figure_w - 12.0 - title_w, LINE], resets, Role::Caption, true);
                if let Some(fraction) = fraction {
                    items.push(Item::Bar {
                        frame: [x + w - bar_w - figure_w, y + 7.0, bar_w, 6.0],
                        fraction,
                        tone: tone_for(fraction),
                    });
                }
                text(items, [x + w - figure_w + 6.0, y, figure_w - 6.0, LINE], window_figure(window).unwrap_or_default(), Role::Value, true);
                y += LINE + 3.0;
            }
        } else {
            for window in &block.windows {
                let fraction = window_fraction(window);
                let figure = window_figure(window).unwrap_or_default();
                let figure_w = if window.spent_cents.is_some() { 150.0_f64.min(w * 0.45) } else { 48.0 };
                let resets = window.resets_at_ms.map(|at| resets_in(at, body.at_ms)).unwrap_or_default();
                text(items, [x, y, w * 0.5, LINE], window.title.clone(), Role::Label, false);
                text(items, [x + w * 0.35, y, w * 0.65 - figure_w - 8.0, LINE], resets, Role::Caption, true);
                text(items, [x + w - figure_w, y, figure_w, LINE], figure, Role::Value, true);
                y += LINE + 2.0;
                match fraction {
                    Some(fraction) => {
                        items.push(Item::Bar { frame: [x, y, w, 7.0], fraction, tone: tone_for(fraction) });
                        y += 7.0 + 12.0;
                    }
                    None => y += 6.0,
                }
            }
        }
    }
    y + GAP - 8.0
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

fn wrapped_lines(text: &str, width: f64) -> usize {
    let per_line = (width / CAPTION_CHAR).floor().max(10.0) as usize;
    text.split('\n')
        .map(|line| line.chars().count().div_ceil(per_line).max(1))
        .sum()
}

/// Lays the card out at `width`: what to draw and the card's height.
pub(crate) fn layout(body: &Body, width: f64) -> (Vec<Item>, f64) {
    let mut items = Vec::new();
    let x = PAD;
    let w = (width - 2.0 * PAD).max(160.0);
    let mut y = PAD;
    let text = |items: &mut Vec<Item>, frame: [f64; 4], text: String, role: Role, right: bool| {
        items.push(Item::Text { frame, text, role, right });
    };
    text(&mut items, [x, y, w - 40.0, 22.0], "Usage".into(), Role::Title, false);
    items.push(Item::Copy { frame: [x + w - 24.0, y, 24.0, 22.0] });
    y += 24.0;
    let Some(card) = &body.card else {
        items.push(Item::Spinner { frame: [x, y + 3.0, 14.0, 14.0] });
        text(&mut items, [x + 22.0, y, w - 22.0, LINE], "Reading recorded requests…".into(), Role::Caption, false);
        y += LINE + PAD;
        return (items, y);
    };
    let children = card.agents.iter().filter(|a| !a.root).count();
    let mut scope = if children > 0 {
        format!("This session and {children} sub-agent{}", if children == 1 { "" } else { "s" })
    } else {
        "This session".into()
    };
    scope.push_str(&format!(" · {} request{}", card.requests, if card.requests == 1 { "" } else { "s" }));
    text(&mut items, [x, y, w, LINE], scope, Role::Caption, false);
    y += LINE;
    if let Some(note) = &body.note {
        let lines = wrapped_lines(note, w) as f64;
        text(&mut items, [x, y, w, lines * 16.0 + 2.0], note.clone(), Role::Caption, false);
        y += lines * 16.0 + 2.0;
    }
    y += GAP - 4.0;
    // The account's plan and limit windows, when the provider has said.
    y = accounts_section(&mut items, body, x, w, y);
    if card.requests == 0 {
        text(&mut items, [x, y, w, LINE], "No requests recorded for this session yet.".into(), Role::Caption, false);
        y += LINE + PAD;
        return (items, y);
    }
    // The four chips: what the header strip used to hold.
    // Cache reads only when the receipts reported them.
    let mut chips = vec![
        ("Input tokens", usage::compact_number(card.input)),
        ("Output tokens", usage::compact_number(card.output)),
    ];
    if card.cache_read_known {
        chips.push(("Cache reads", usage::compact_number(card.cache_read)));
    }
    chips.push(("Requests", card.requests.to_string()));
    let columns = if w >= 420.0 { chips.len() } else { 2 };
    let chip_w = ((w - (columns - 1) as f64 * 8.0) / columns as f64).floor();
    for (i, (label, value)) in chips.iter().enumerate() {
        let cx = x + (i % columns) as f64 * (chip_w + 8.0);
        let cy = y + (i / columns) as f64 * (CHIP + 8.0);
        items.push(Item::Chip { frame: [cx, cy, chip_w, CHIP], lead: i == 0 });
        text(&mut items, [cx + 12.0, cy + 7.0, chip_w - 20.0, 24.0], value.clone(), Role::Figure { lead: i == 0 }, false);
        text(&mut items, [cx + 12.0, cy + 30.0, chip_w - 20.0, 14.0], (*label).into(), Role::Caption, false);
    }
    y += chips.len().div_ceil(columns) as f64 * (CHIP + 8.0) - 8.0 + GAP + 4.0;
    // This session: a grid of label and figure.
    let cells = session_cells(card);
    if !cells.is_empty() {
        text(&mut items, [x, y, w, LINE], "This session".into(), Role::Section, false);
        y += LINE + 4.0;
        let columns = if w >= 480.0 { 3 } else { 2 };
        let cell_w = w / columns as f64;
        for (i, (label, value)) in cells.iter().enumerate() {
            let cx = x + (i % columns) as f64 * cell_w;
            if i % columns == 0 && i > 0 {
                y += LINE + 6.0;
            }
            if i % columns == 0 {
                items.push(Item::Rule { frame: [x, y + LINE + 2.0, w, 1.0] });
            }
            // The figure takes what it needs; the label the rest.
            let figure = 70.0_f64.min(cell_w * 0.45);
            let gutter = if i % columns + 1 == columns { 0.0 } else { 18.0 };
            text(&mut items, [cx, y, cell_w - figure - gutter - 4.0, LINE], label.clone(), Role::Label, false);
            text(&mut items, [cx + cell_w - figure - gutter, y, figure, LINE], value.clone(), Role::Value, true);
        }
        y += LINE + 6.0 + GAP;
    }
    // Breakdown by model.
    for model in card.models.iter().take(3) {
        text(&mut items, [x, y, w * 0.4, LINE], "Breakdown".into(), Role::Section, false);
        text(&mut items, [x + w * 0.4, y, w * 0.6, LINE], model.model.clone(), Role::Caption, true);
        y += LINE + 4.0;
        let mut rows = vec![("Input", usage::compact_number(model.input)), ("Output", usage::compact_number(model.output))];
        if card.cache_read_known {
            rows.push(("Cache read", usage::compact_number(model.cache_read)));
        }
        if card.cache_write_known {
            rows.push(("Cache write", usage::compact_number(model.cache_write)));
        }
        rows.push(("Requests", model.requests.to_string()));
        for (label, value) in rows {
            items.push(Item::Rule { frame: [x, y, w, 1.0] });
            text(&mut items, [x, y + 3.0, w * 0.6, LINE], label.into(), Role::Label, false);
            text(&mut items, [x + w * 0.6, y + 3.0, w * 0.4, LINE], value, Role::Value, true);
            y += LINE + 4.0;
        }
        items.push(Item::Rule { frame: [x, y, w, 1.0] });
        y += GAP;
    }
    // What the tokens went to: the session and each sub-agent.
    if children > 0 {
        text(&mut items, [x, y, w, LINE], "What's using this session?".into(), Role::Section, false);
        y += LINE + 4.0;
        let total = card.tokens().max(1) as f64;
        let named = card.agents.len().min(AGENTS);
        let mut lines: Vec<(String, f64, bool)> = card.agents[..named]
            .iter()
            .map(|a| (agent_name(body, a), a.tokens as f64 / total, a.root))
            .collect();
        if card.agents.len() > named {
            let rest: u64 = card.agents[named..].iter().map(|a| a.tokens).sum();
            lines.push((format!("{} more sub-agents", card.agents.len() - named), rest as f64 / total, false));
        }
        let bar_w = (w * 0.3).clamp(60.0, 180.0);
        for (name, share, root) in lines {
            text(&mut items, [x, y, w - bar_w - 64.0, LINE], name, Role::Label, false);
            items.push(Item::Bar {
                frame: [x + w - bar_w - 52.0, y + 7.0, bar_w, 6.0],
                fraction: share,
                tone: if root { Tone::Gold } else { Tone::Plum },
            });
            text(&mut items, [x + w - 46.0, y, 46.0, LINE], percent(share), Role::Value, true);
            y += LINE + 4.0;
        }
        y += GAP - 4.0;
    }
    // Output speed, from the speedometer's readings.
    if !body.rates.is_empty() {
        let latest = body.rates.last().copied().unwrap_or(0.0).round();
        let peak = body.rates.iter().copied().fold(0.0_f64, f64::max).round();
        text(&mut items, [x, y, w * 0.5, LINE], "Output speed".into(), Role::Section, false);
        text(&mut items, [x + w * 0.4, y, w * 0.6, LINE], format!("{latest:.0} tok/s · peak {peak:.0}"), Role::Value, true);
        y += LINE + 4.0;
        items.push(Item::Spark { frame: [x, y, w, 30.0] });
        y += 30.0 + GAP;
    }
    // A tip, from the figures above.
    if let Some(tip) = tip(card) {
        text(&mut items, [x, y, w, LINE], "Tip".into(), Role::Section, false);
        y += LINE + 2.0;
        let lines = wrapped_lines(&tip, w) as f64;
        text(&mut items, [x, y, w, lines * 16.0 + 2.0], tip, Role::Caption, false);
        y += lines * 16.0 + 2.0 + GAP;
    }
    // What is not shown, and why.
    let mut notes = Vec::new();
    if !body.all && body.accounts.is_empty() && body.limits.is_empty() && body.accounts_note.is_none() {
        notes.push("No subscription usage: this session has no provider account.".to_owned());
    }
    if card.cost_nano_usd.is_some() {
        notes.push(format!(
            "Cost is the model catalog's estimate over {} of {} requests, not an invoice.",
            card.cost_requests, card.requests
        ));
    }
    if card.warnings > 0 {
        notes.push(format!("{} journal note{}: totals may be partial (Usage window has details).", card.warnings, if card.warnings == 1 { "" } else { "s" }));
    }
    if !notes.is_empty() {
        let note = notes.join("\n");
        let lines = wrapped_lines(&note, w) as f64;
        text(&mut items, [x, y, w, lines * 15.0 + 2.0], note, Role::Caption, false);
        y += lines * 15.0 + 2.0;
    }
    y = y - GAP.min(y) + PAD;
    (items, y.ceil())
}

/// The card as plain text: what Copy puts on the pasteboard and what the
/// transcript's text holds.
pub(crate) fn plain_text(body: &Body) -> String {
    let (items, _) = layout(body, 640.0);
    let mut lines: Vec<String> = Vec::new();
    let mut row_y = f64::NAN;
    for item in items {
        let Item::Text { frame, text, .. } = item else { continue };
        if text.is_empty() {
            continue;
        }
        if (frame[1] - row_y).abs() < 6.0 {
            if let Some(last) = lines.last_mut() {
                last.push_str("  ");
                last.push_str(&text);
                continue;
            }
        }
        row_y = frame[1];
        lines.push(text);
    }
    lines.join("\n")
}

#[derive(Debug, Default)]
pub(crate) struct CardIvars {
    text: RefCell<String>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond its designated
    //   initializer, which `new` calls.
    // - MainThreadOnly matches AppKit's threading model.
    // - UsageCardView does not implement Drop.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = CardIvars]
    #[derive(Debug)]
    pub(crate) struct UsageCardView;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for UsageCardView {}

    // SAFETY: the overrides keep AppKit's declared signatures.
    impl UsageCardView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(copyUsageReport:))]
        fn copy_usage_report(&self, sender: Option<&AnyObject>) {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // SAFETY: NSPasteboardTypeString is an AppKit static.
            pasteboard.setString_forType(&NSString::from_str(&self.ivars().text.borrow()), unsafe { NSPasteboardTypeString });
            if let Some(button) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
                motion::hop(button);
            }
        }
    }
);

fn font(role: Role, terminal: bool) -> Retained<NSFont> {
    // SAFETY: AppKit exports the font weight constants for the process lifetime.
    let (size, weight) = match role {
        Role::Title => (15.0, unsafe { NSFontWeightSemibold }),
        Role::Section => (13.0, unsafe { NSFontWeightSemibold }),
        Role::Label => (13.0, unsafe { NSFontWeightRegular }),
        Role::Value => (13.0, unsafe { NSFontWeightMedium }),
        Role::Figure { .. } => (19.0, unsafe { NSFontWeightSemibold }),
        Role::Caption | Role::Error => (11.5, unsafe { NSFontWeightRegular }),
    };
    // Figures in tabular numerals; Terminal sets everything monospaced.
    if terminal {
        NSFont::monospacedSystemFontOfSize_weight(size - 0.5, weight)
    } else if matches!(role, Role::Value | Role::Figure { .. }) {
        NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)
    } else {
        NSFont::systemFontOfSize_weight(size, weight)
    }
}

/// The card's view at `width`: `layout`'s height with `margin` above and
/// below, so the view fills the transcript row it is placed in.
pub(crate) fn view(body: &Body, width: f64, margin: f64, style: theme::ChatStyle, mtm: MainThreadMarker) -> (Retained<NSView>, f64) {
    let (items, card_height) = layout(body, width);
    let height = card_height + 2.0 * margin;
    let frame_of = |frame: [f64; 4]| rect(frame[0], frame[1] + margin, frame[2], frame[3]);
    let this = UsageCardView::alloc(mtm).set_ivars(CardIvars { text: RefCell::new(plain_text(body)) });
    // SAFETY: NSView's designated initializer takes a frame rectangle.
    let card: Retained<UsageCardView> = unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, width, height)] };
    // The ground's own rectangle, inside the margins.
    let (top, bottom, height) = (margin, margin + card_height, card_height);
    let terminal = style == theme::ChatStyle::Terminal;
    // The card's ground, by chat style: Modern's rounded surface, Editorial's
    // gold rule and hairlines, Terminal's square lit band.
    match style {
        theme::ChatStyle::Modern => {
            let ground = card_box(rect(0.0, top, width, height), 14.0, mtm);
            ground.setFillColor(&theme::surface());
            card.addSubview(&ground);
        }
        theme::ChatStyle::Editorial => {
            card.addSubview(&fill_box(rect(0.0, top, 2.0, height), &theme::accent_rule(), 0.0, mtm));
            card.addSubview(&fill_box(rect(0.0, top, width, 1.0), &theme::hairline(), 0.0, mtm));
            card.addSubview(&fill_box(rect(0.0, bottom - 1.0, width, 1.0), &theme::hairline(), 0.0, mtm));
        }
        theme::ChatStyle::Terminal => {
            let ground = fill_box(rect(0.0, top, width, height), &theme::surface_inset(), 0.0, mtm);
            ground.setBorderWidth(1.0);
            ground.setBorderColor(&theme::hairline());
            card.addSubview(&ground);
            card.addSubview(&fill_box(rect(0.0, top, 3.0, height), &theme::accent(), 0.0, mtm));
        }
    }
    let radius = |h: f64| if terminal { 0.0 } else { (h / 2.0).floor() };
    for item in &items {
        match item {
            Item::Chip { frame, lead } => {
                let chip = fill_box(
                    frame_of(*frame),
                    &*if *lead { theme::accent_fill_strong() } else { theme::chip_fill() },
                    if terminal { 0.0 } else { 12.0 },
                    mtm,
                );
                chip.setBorderWidth(1.0);
                chip.setBorderColor(&*if *lead { theme::accent_rule() } else { theme::hairline() });
                card.addSubview(&chip);
            }
            Item::Rule { frame } => {
                card.addSubview(&fill_box(frame_of(*frame), &theme::hairline(), 0.0, mtm));
            }
            Item::Bar { frame, fraction, tone } => {
                let track = fill_box(frame_of(*frame), &theme::plum_fill(), radius(frame[3]), mtm);
                track.setAccessibilityElement(false);
                card.addSubview(&track);
                let filled = (frame[2] * fraction.clamp(0.0, 1.0)).max(if *fraction > 0.0 { frame[3] } else { 0.0 });
                if filled > 0.0 {
                    let color = match tone {
                        Tone::Gold => theme::crown_gold(),
                        Tone::Plum => theme::plum(),
                        Tone::Danger => theme::danger(),
                    };
                    let fill = fill_box(frame_of([frame[0], frame[1], filled, frame[3]]), &color, radius(frame[3]), mtm);
                    fill.setAccessibilityElement(false);
                    card.addSubview(&fill);
                }
            }
            Item::Spark { frame } => {
                let spark = header_ui::Sparkline::new(frame_of(*frame), mtm);
                spark.set_samples(&body.rates);
                spark.setAccessibilityElement(false);
                card.addSubview(&spark);
            }
            Item::Spinner { frame } => {
                let spinner = NSProgressIndicator::initWithFrame(NSProgressIndicator::alloc(mtm), frame_of(*frame));
                spinner.setStyle(NSProgressIndicatorStyle::Spinning);
                spinner.setControlSize(NSControlSize::Small);
                // SAFETY: AppKit animations are started on the main thread.
                unsafe { spinner.startAnimation(None) };
                card.addSubview(&spinner);
            }
            Item::Copy { frame } => {
                let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    ns_string!("doc.on.doc"),
                    Some(ns_string!("Copy usage report")),
                );
                // SAFETY: the card implements copyUsageReport: and outlives its button.
                let button = unsafe {
                    NSButton::buttonWithTitle_target_action(ns_string!(""), Some::<&AnyObject>(&card), Some(sel!(copyUsageReport:)), mtm)
                };
                if let Some(image) = image {
                    button.setImage(Some(&image));
                }
                button.setBordered(false);
                button.setImagePosition(NSCellImagePosition::ImageOnly);
                button.setContentTintColor(Some(&theme::secondary_text()));
                button.setToolTip(Some(ns_string!("Copy the usage report as text")));
                button.setAccessibilityLabel(Some(ns_string!("Copy usage report")));
                button.setFrame(frame_of(*frame));
                card.addSubview(&button);
            }
            Item::Retry { frame } => {
                // Sent up the responder chain to the app delegate, which
                // asks OMP again off the main thread.
                // SAFETY: a nil target is valid; the action is a selector.
                let button = unsafe {
                    NSButton::buttonWithTitle_target_action(ns_string!("Retry"), None, Some(sel!(retryAccountUsage:)), mtm)
                };
                button.setBordered(false);
                button.setAlignment(NSTextAlignment::Right);
                button.setAttributedTitle(&attributed("Retry", &font(Role::Caption, terminal), &theme::accent(), 0.0));
                button.setToolTip(Some(ns_string!("Ask the provider for this account's usage again")));
                button.setFrame(frame_of(*frame));
                card.addSubview(&button);
            }
            Item::Text { .. } => {}
        }
    }
    // Text last, above chips and rules; selectable so a figure can be
    // copied on its own.
    for item in &items {
        let Item::Text { frame, text, role, right } = item else { continue };
        let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        label.setFont(Some(&font(*role, terminal)));
        let color = match role {
            Role::Caption => theme::secondary_text(),
            Role::Error => theme::danger(),
            Role::Label => NSColor::secondaryLabelColor(),
            Role::Figure { lead: true } => theme::accent(),
            _ => NSColor::labelColor(),
        };
        label.setTextColor(Some(&color));
        if *right {
            label.setAlignment(NSTextAlignment::Right);
        }
        let wraps = frame[3] > LINE + 4.0 && matches!(role, Role::Caption | Role::Error);
        if wraps {
            label.setUsesSingleLineMode(false);
            label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
            label.setMaximumNumberOfLines(0);
        } else {
            label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        }
        label.setSelectable(true);
        label.setFrame(frame_of(*frame));
        card.addSubview(&label);
    }
    card.setAccessibilityElement(true);
    // SAFETY: NSAccessibilityGroupRole is an AppKit static.
    card.setAccessibilityRole(Some(unsafe { objc2_app_kit::NSAccessibilityGroupRole }));
    card.setAccessibilityLabel(Some(ns_string!("Usage report")));
    card.setToolTip(None);
    (Retained::into_super(card), card_height + 2.0 * margin)
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
    fn height_is_the_same_every_time_and_grows_narrower() {
        let body = sample();
        let (_, wide) = layout(&body, 720.0);
        assert_eq!(layout(&body, 720.0).1, wide);
        let (_, narrow) = layout(&body, 360.0);
        assert!(narrow > wide);
        // A round trip through the row's body keeps it.
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

    fn texts(body: &Body) -> Vec<(String, Role)> {
        layout(body, 640.0).0.into_iter().filter_map(|item| match item {
            Item::Text { text, role, .. } => Some((text, role)),
            _ => None,
        }).collect()
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
        // Nothing to map: the provider's own words.
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
        // Bars: gold below 85%, rose from it.
        let bars: Vec<(f64, Tone)> = layout(&body, 640.0).0.into_iter().filter_map(|item| match item {
            Item::Bar { fraction, tone, frame } if frame[2] > 400.0 => Some((fraction, tone)),
            _ => None,
        }).collect();
        assert_eq!(bars[0], (0.34, Tone::Gold));
        assert_eq!(bars[2], (0.88, Tone::Danger));
    }

    #[test]
    fn spend_windows_with_and_without_a_cap() {
        let capped = account_block("anthropic:w", "Work", AuthProvider::Anthropic, Some(&claude()));
        let body = with_accounts(vec![capped], false);
        let text = plain_text(&body);
        assert!(text.contains("Extra usage") && text.contains("$12.40 of $50.00"), "{text}");
        assert!(layout(&body, 640.0).0.iter().any(|item| matches!(item, Item::Bar { fraction, .. } if (*fraction - 0.248).abs() < 1e-9)));
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
        // Nothing to measure against: no bar (the card's own bars are the
        // sub-agents' short ones).
        assert!(!layout(&body, 640.0).0.iter().any(|item| matches!(item, Item::Bar { frame, .. } if frame[2] > 400.0)));
    }

    #[test]
    fn an_error_is_said_plainly_with_a_retry() {
        let failed = omp_runtime::AccountUsage {
            account_id: "anthropic:h".into(),
            plan: None,
            windows: Vec::new(),
            error: Some("usage endpoint answered 429".into()),
        };
        let body = with_accounts(vec![account_block("anthropic:h", "Home", AuthProvider::Anthropic, Some(&failed))], false);
        assert!(texts(&body).contains(&("Couldn't read usage: usage endpoint answered 429".into(), Role::Error)));
        assert!(layout(&body, 640.0).0.iter().any(|item| matches!(item, Item::Retry { .. })));
    }

    #[test]
    fn no_windows_and_not_yet_read_say_only_that() {
        let empty = omp_runtime::AccountUsage { account_id: "openai-codex:s".into(), plan: None, windows: Vec::new(), error: None };
        let body = with_accounts(vec![account_block("openai-codex:s", "Side", AuthProvider::OpenAi, Some(&empty))], false);
        let text = plain_text(&body);
        assert!(text.contains("The provider reported no usage windows."), "{text}");
        assert!(!texts(&body).iter().any(|(t, _)| t.starts_with("ChatGPT") && t != "ChatGPT subscription"), "no plan was reported: {text}");
        // Not answered yet: waiting, or not read.
        let mut body = with_accounts(vec![account_block("openai-codex:s", "Side", AuthProvider::OpenAi, None)], false);
        body.refreshing = true;
        assert!(plain_text(&body).contains("Asking the provider…"));
        assert!(layout(&body, 640.0).0.iter().any(|item| matches!(item, Item::Spinner { .. })));
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
        let texts = texts(&body);
        let position = |text: &str| texts.iter().position(|(t, _)| t == text).unwrap_or_else(|| panic!("{text} missing: {texts:?}"));
        assert!(position("Accounts") < position("CHATGPT SUBSCRIPTIONS"));
        assert!(position("CHATGPT SUBSCRIPTIONS") < position("Side"));
        assert!(position("Side") < position("CLAUDE SUBSCRIPTIONS"));
        assert!(position("CLAUDE SUBSCRIPTIONS") < position("Work"));
        assert!(position("Work") < position("Home"));
        assert_eq!(texts.iter().filter(|(t, _)| t == "CLAUDE SUBSCRIPTIONS").count(), 1);
        assert!(texts.iter().any(|(t, _)| t == "ChatGPT Plus"));
        assert!(texts.iter().any(|(t, _)| t == "Weekly · Spark"));
        // Compact: every money figure fits its column.
        assert!(texts.iter().any(|(t, _)| t == "$12.40 of $50.00"));
        // None signed in says so.
        let none = plain_text(&with_accounts(Vec::new(), true));
        assert!(none.contains("No accounts are signed in."), "{none}");
    }

    #[test]
    fn height_is_exact_before_any_view_and_steady_while_refreshing() {
        let blocks = vec![account_block("anthropic:w", "Work", AuthProvider::Anthropic, Some(&claude()))];
        for all in [false, true] {
            let mut body = with_accounts(blocks.clone(), all);
            for width in [360.0, 640.0, 900.0] {
                let (items, height) = layout(&body, width);
                assert_eq!(layout(&body, width).1, height);
                // Everything drawn lies inside the height the stack reserves.
                for item in &items {
                    let frame = match item {
                        Item::Text { frame, .. } | Item::Bar { frame, .. } | Item::Chip { frame, .. } | Item::Rule { frame } | Item::Spark { frame } | Item::Spinner { frame } | Item::Copy { frame } | Item::Retry { frame } => frame,
                    };
                    assert!(frame[1] >= 0.0 && frame[1] + frame[3] <= height + 0.5, "{item:?} outside {height}");
                    assert!(frame[0] >= 0.0 && frame[0] + frame[2] <= width + 0.5, "{item:?} wider than {width}");
                }
                // A reading on its way, or a newer stamp, moves nothing.
                body.refreshing = true;
                assert_eq!(layout(&body, width).1, height);
                body.fetched_ms = Some(NOW);
                assert_eq!(layout(&body, width).1, height);
                body.refreshing = false;
                body.fetched_ms = Some(NOW - 2 * 60_000);
            }
            // The row's body round-trips.
            assert_eq!(Body::decode(&body.encode()), body);
        }
    }
}
