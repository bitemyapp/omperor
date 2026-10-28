//! The Timeline window: a session's prompts, responses, tool calls and
//! errors on one clock, each with when it started and how long it took,
//! narrowed by a search, a minimum duration, a kind and a model. A run of
//! events the filter takes out folds into one row that opens with a click;
//! an event opens its record beside the list.

use std::rc::Rc;

use aizome_kit::prelude::*;
use aizome_kit::sidestep_ui::Alignment;
use aizome_kit::widgets::layout::column as column_of;
use aizome_kit::{column, row};

use crate::app::{App, Msg, TimelineChange, TimelineWindow};
use crate::metrics::duration_text;
use crate::timeline::{self, Event, Kind, Loaded, Row};

const KINDS: [&str; 5] = ["All types", "Prompts", "Responses", "Tools", "Errors"];

pub(super) fn window(app: &App) -> Element<Msg> {
    let Some(window) = &app.timeline else { return space(0.0) };
    let body: Element<Msg> = match (&window.loaded, &window.error) {
        (None, Some(error)) => super::empty_state(Icon::TriangleAlert, Role::Danger, "The timeline could not be read", error),
        (None, None) => super::empty_state(Icon::LoaderCircle, Role::Accent, "Reading the journal…", ""),
        (Some(loaded), _) => {
            let selected = window.selected.and_then(|index| loaded.get(index));
            let detail: Element<Msg> = match selected {
                Some(event) => detail(event, loaded.summary.origin_ms),
                None => space(0.0),
            };
            split(list(window, loaded).key("events"), detail)
                .second_size(380.0)
                .min_sizes(380.0, 280.0)
                .collapse_second(selected.is_none())
                .into()
        }
    };
    column![header(window).key("header"), filters(window).key("filters"), divider(), body, status(window).key("status")]
        .width(Fill)
        .height(Fill)
        .into()
}

fn header(window: &TimelineWindow) -> Element<Msg> {
    let line = window.loaded.as_ref().map_or_else(String::new, |loaded| summary_line(&loaded.summary));
    container(
        row![
            icon(Icon::ChartGantt).size(18.0).role(Role::Accent),
            column![text(window.title.clone()).semibold().max_lines(1), text(line).scale(0.88).role(Role::Text3).max_lines(1)]
                .spacing(2.0)
                .width(Fill),
            button("Next Slowest")
                .icon(Icon::Timer)
                .compact()
                .enabled(window.loaded.is_some())
                .on_press(Msg::TimelineSlowest)
                .tooltip("Select the slowest events in turn"),
        ]
        .spacing(10.0),
    )
    .padding([10.0, 14.0])
    .width(Fill)
    .into()
}

/// "120 events over 14m 02s · 31 responses · 80 tool calls · 2 sub-agents,
/// shown one level in · longest 2m 10s".
fn summary_line(summary: &timeline::Summary) -> String {
    let plural = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    let mut line = format!(
        "{} over {} · {} · {}",
        plural(summary.events, "event"),
        duration_text(summary.span_ms),
        plural(summary.responses, "response"),
        plural(summary.tools, "tool call")
    );
    if summary.agents > 0 {
        line.push_str(&format!(" · {}, shown one level in", plural(summary.agents, "sub-agent")));
    }
    line.push_str(&format!(" · longest {}", duration_text(summary.longest_ms)));
    line
}

fn filters(window: &TimelineWindow) -> Element<Msg> {
    let filter = &window.filter;
    let kind = match filter.kind {
        None => 0,
        Some(Kind::Prompt) => 1,
        Some(Kind::Response) => 2,
        Some(Kind::Tool) => 3,
        Some(Kind::Notice) => 4,
    };
    let models = window.loaded.as_ref().map(|l| l.summary.models.clone()).unwrap_or_default();
    let model = filter.model.as_ref().and_then(|m| models.iter().position(|x| x == m)).map_or(0, |i| i + 1);
    let mut model_options = vec!["All models".to_owned()];
    model_options.extend(models.iter().map(|m| crate::agents::short_model(m)));
    let hint = window.loaded.as_ref().map(|loaded| {
        let p90 = loaded.summary.p90_ms.map_or_else(|| "—".to_owned(), duration_text);
        format!("p90 {p90} · longest {}", duration_text(loaded.summary.longest_ms))
    });
    container(
        row![
            search_field(&filter.query)
                .placeholder("Search events")
                .on_change(|q| Msg::TimelineFilter(TimelineChange::Query(q)))
                .width(Fill),
            text("At least").role(Role::Text3),
            text_field(&window.min_text)
                .placeholder("0 s")
                .on_change(|s| Msg::TimelineFilter(TimelineChange::Min(s)))
                .error(window.min_invalid().then_some("Try 2s, 500ms or 1m30s"))
                .width(84.0)
                .tooltip(hint.unwrap_or_default()),
            dropdown(KINDS.map(str::to_owned), Some(kind)).on_select(|i| Msg::TimelineFilter(TimelineChange::Kind(i))).compact(),
            dropdown(model_options, Some(model)).on_select(|i| Msg::TimelineFilter(TimelineChange::Model(i))).compact(),
            button("Clear").plain().compact().enabled(filter.is_active()).on_press(Msg::TimelineFilter(TimelineChange::Clear)),
        ]
        .spacing(8.0),
    )
    .padding([0.0, 14.0, 10.0, 14.0])
    .width(Fill)
    .into()
}

fn list(window: &TimelineWindow, loaded: &Loaded) -> Element<Msg> {
    if window.rows.is_empty() {
        return super::empty_state(Icon::Search, Role::Text3, "No event matches", "Loosen the filter to see more.");
    }
    let rows: Rc<Vec<Row>> = window.rows.clone();
    let events = Rc::new(loaded.clone());
    let longest = loaded.summary.longest_ms;
    let origin = loaded.summary.origin_ms;
    let build = {
        let (rows, events) = (rows.clone(), events.clone());
        move |i: usize| -> Element<Msg> {
            match &rows[i] {
                Row::Event { index, dimmed } => match events.get(*index) {
                    Some(event) => event_row(event, origin, longest, *dimmed),
                    None => space(0.0),
                },
                Row::Hidden { count, total_ms, open, .. } => hidden_row(*count, *total_ms, *open),
            }
        }
    };
    let mut list = virtual_list(rows.len(), 40.0, build)
        .selected(window.selected_row())
        .on_select(|picked: Vec<usize>| Msg::TimelinePick(picked.first().copied()));
    if let Some(row) = window.selected_row() {
        list = list.reveal(row);
    }
    list.width(Fill).height(Fill).into()
}

fn event_row(event: &Event, origin: u64, longest: u64, dimmed: bool) -> Element<Msg> {
    let quiet = if dimmed { Role::Faint } else { Role::Text3 };
    let label_role = match (dimmed, event.failed) {
        (true, _) => Role::Faint,
        (false, true) => Role::Danger,
        (false, false) => Role::Text,
    };
    let label = match &event.agent {
        Some(agent) => format!("↳ {agent} · {}", event.label),
        None => event.label.clone(),
    };
    let indent = if event.agent.is_some() { 14.0 } else { 0.0 };
    row![
        text(timeline::offset_text(event.started_ms.saturating_sub(origin)))
            .mono()
            .tabular()
            .scale(0.85)
            .role(quiet)
            .width(72.0),
        space(indent),
        column![
            text(label).semibold().role(label_role).max_lines(1),
            text(event.detail.replace('\n', " ")).scale(0.88).role(quiet).max_lines(1),
        ]
        .spacing(1.0)
        .width(Fill),
        bar(event, longest, dimmed),
        text(if matches!(event.kind, Kind::Prompt | Kind::Notice) { String::new() } else { duration_text(event.duration_ms()) })
            .mono()
            .tabular()
            .scale(0.88)
            .role(if dimmed { Role::Faint } else { Role::Text2 })
            .align(Alignment::Right)
            .width(72.0),
    ]
    .spacing(10.0)
    .padding([0.0, 12.0])
    .width(Fill)
    .into()
}

/// How long an event took, against the longest, on a square-root scale so
/// short ones still show; a response's wait for its first token in a quieter
/// colour. A prompt or an error is a moment: a dot.
fn bar(event: &Event, longest: u64, dimmed: bool) -> Element<Msg> {
    let kind = match event.kind {
        Kind::Prompt => 0u8,
        Kind::Response => 1,
        Kind::Tool => 2,
        Kind::Notice => 3,
    };
    let state = (kind, event.duration_ms(), event.waiting_ms, longest, event.failed, dimmed);
    canvas(state, |c, bounds, &(kind, duration, waiting, longest, failed, dimmed), theme| {
        let colour = |role: Role| theme.role(if dimmed { Role::Faint } else { role });
        let middle = (bounds.y0 + bounds.y1) / 2.0;
        if kind == 0 || kind == 3 {
            let dot = Rect::new(bounds.x0, middle - 4.0, bounds.x0 + 8.0, middle + 4.0);
            c.fill_rect(dot, colour(if kind == 3 { Role::Danger } else { Role::Accent }));
            return;
        }
        let share = if longest == 0 { 0.0 } else { (duration as f64 / longest as f64).sqrt() };
        let width = (bounds.width() * share).max(if duration > 0 { 2.0 } else { 0.0 });
        let (top, bottom) = (middle - 4.0, middle + 4.0);
        if kind == 1 {
            let wait = if duration == 0 { 0.0 } else { width * waiting.min(duration) as f64 / duration as f64 };
            c.fill_rect(Rect::new(bounds.x0, top, bounds.x0 + wait, bottom), colour(Role::Text3));
            c.fill_rect(Rect::new(bounds.x0 + wait, top, bounds.x0 + width, bottom), colour(Role::Accent));
        } else {
            c.fill_rect(Rect::new(bounds.x0, top, bounds.x0 + width, bottom), colour(if failed { Role::Danger } else { Role::Cyan }));
        }
    })
    .width(180.0)
    .height(16.0)
    .into()
}

fn hidden_row(count: usize, total_ms: u64, open: bool) -> Element<Msg> {
    let plural = if count == 1 { "" } else { "s" };
    let words = if open {
        format!("{count} event{plural} the filter took out, shown dimmed · click to fold")
    } else {
        format!("{count} event{plural} hidden · {} · click to show", duration_text(total_ms))
    };
    row![
        space(72.0),
        icon(if open { Icon::ChevronDown } else { Icon::ChevronRight }).size(13.0).role(Role::Text3),
        text(words).scale(0.88).italic().role(Role::Text3).max_lines(1),
    ]
    .spacing(8.0)
    .padding([0.0, 12.0])
    .width(Fill)
    .into()
}

/// An event's record: the facts the journal holds about it, then the
/// texts behind it.
fn detail(event: &Event, origin: u64) -> Element<Msg> {
    let glyph = match event.kind {
        Kind::Prompt => Icon::UserRound,
        Kind::Response => Icon::Sparkles,
        Kind::Tool => Icon::Wrench,
        Kind::Notice => Icon::TriangleAlert,
    };
    let mut parts: Vec<Element<Msg>> = vec![
        row![
            icon(glyph).size(16.0).role(if event.failed { Role::Danger } else { Role::Accent }),
            text(event.label.clone()).semibold().max_lines(1).width(Fill),
            icon_button(Icon::X).on_press(Msg::TimelinePick(None)).tooltip("Close"),
        ]
        .spacing(8.0)
        .into(),
        text(format!(
            "At +{} · took {}",
            timeline::offset_text(event.started_ms.saturating_sub(origin)),
            duration_text(event.duration_ms())
        ))
        .scale(0.88)
        .role(Role::Text3)
        .into(),
    ];
    let facts: Vec<Element<Msg>> = event
        .info
        .facts
        .iter()
        .map(|(name, value)| {
            row![
                text(name.clone()).scale(0.88).role(Role::Text3).width(110.0),
                text(value.clone()).scale(0.88).selectable().wrap().width(Fill),
            ]
            .spacing(8.0)
            .align(Align::Start)
            .into()
        })
        .collect();
    if !facts.is_empty() {
        parts.push(column_of(facts).spacing(3.0).padding([6.0, 0.0]).width(Fill).into());
    }
    for (title, body) in &event.info.sections {
        let code = matches!(title.as_str(), "Arguments" | "Result" | "Output");
        let spans = [if code { Span::new(body.clone()).mono() } else { Span::new(body.clone()) }];
        parts.push(aizome_kit::caption(title.clone()));
        parts.push(
            container(rich_text(spans).scale(if code { 0.88 } else { 1.0 }).wrap().selectable())
                .padding([6.0, 8.0])
                .fill(Tone::Sunk)
                .border(Edge::Line)
                .width(Fill)
                .into(),
        );
    }
    container(scroll(column_of(parts).spacing(6.0).padding([12.0, 14.0]).width(Fill)).width(Fill).height(Fill))
        .edges(Edge::Line, Sides::LEFT)
        .width(Fill)
        .height(Fill)
        .into()
}

fn status(window: &TimelineWindow) -> Element<Msg> {
    let mut items: Vec<Element<Msg>> = Vec::new();
    if let Some(loaded) = &window.loaded {
        let shown = window.rows.iter().filter(|r| matches!(r, Row::Event { dimmed: false, .. })).count();
        items.push(
            text(if window.filter.is_active() {
                format!("{shown} shown of {}", loaded.len())
            } else {
                format!("{} events", loaded.len())
            })
            .into(),
        );
        if !window.complete {
            items.push(
                row![
                    icon(Icon::LoaderCircle).size(13.0).role(Role::Accent),
                    text(format!("Loading earlier events · {} of {}", loaded.len(), loaded.summary.events)),
                ]
                .spacing(6.0)
                .into(),
            );
        }
    }
    if let Some(error) = window.error.as_ref().filter(|_| window.loaded.is_some()) {
        items.push(text(error.clone()).role(Role::Danger).into());
    }
    status_bar(items)
}
