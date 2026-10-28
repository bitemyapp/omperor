//! The Agents panel: the session's agent and the sub-agents it started,
//! running and finished, each with its state in words and a glyph; the one
//! selected opens its recorded transcript below.

use std::rc::Rc;

use aizome_kit::prelude::*;
use aizome_kit::widgets::layout::column as column_of;
use aizome_kit::{column, row};

use crate::agents::{self, AgentRow, AgentState, RootAgent, WorkScope};
use crate::app::{App, Msg};

pub(super) fn view(app: &App) -> Element<Msg> {
    let session = app.session();
    let Some(view) = app.view_of(session.key) else { return space(0.0) };
    let subagents = session.subagents.borrow().clone();
    let now_ms = crate::rules::wall_clock_ms();
    let root = RootAgent {
        title: super::session_title(app),
        state: session.live_state.borrow().clone(),
        elapsed: session.turn_started_at.get().map(|at| at.elapsed()),
        model: session.current_model.borrow().clone(),
    };
    let candidates: Vec<_> = agents::matching_work(&subagents, &view.view).into_iter().cloned().collect();
    let rows = Rc::new(agents::agent_rows(&root, &candidates, now_ms, agents::FinishedFilter::All));
    let summary = agents::summary_line(&subagents);
    let header = container(
        column![
            row![
                icon(Icon::Bot).size(15.0).role(Role::Text2),
                text("Agents").semibold(),
                spacer(),
                text(if summary.is_empty() { "No sub-agents yet".to_owned() } else { summary }).scale(0.88).role(Role::Text3),
            ]
            .spacing(8.0),
            row![
                search_field(&view.view.query).placeholder("Search agents").on_change(Msg::AgentQuery).width(Fill),
                segmented(["All", "Active", "Attention"], match view.view.scope {
                    WorkScope::All => 0,
                    WorkScope::Active => 1,
                    WorkScope::Attention => 2,
                })
                .on_select(Msg::AgentScope),
            ]
            .spacing(8.0),
        ]
        .spacing(8.0),
    )
    .padding([8.0, 10.0])
    .edges(Edge::Line, Sides::BOTTOM)
    .width(Fill);
    let selected_id = view.view.selected.clone();
    let selected = rows.iter().position(|r| matches!(r, AgentRow::Subagent { id, .. } if Some(id) == selected_id.as_ref()));
    let build = rows.clone();
    let pick = rows.clone();
    let list = virtual_list(rows.len(), 40.0, move |i| agent_row(&build[i]))
        .selected(selected)
        .on_select(move |picked: Vec<usize>| match picked.first().and_then(|i| pick.get(*i)) {
            Some(AgentRow::Subagent { id, .. }) => Msg::SelectAgent(Some(id.clone())),
            _ => Msg::SelectAgent(None),
        })
        .width(Fill)
        .height(Fill);
    let Some(id) = selected_id.filter(|id| subagents.iter().any(|a| a.id == *id)) else {
        return column![header, list].width(Fill).height(Fill).into();
    };
    let body = split(list, evidence(app, &id)).vertical().ratio(0.38).min_sizes(100.0, 160.0);
    column![header, body].width(Fill).height(Fill).into()
}

fn state_glyph(state: &AgentState) -> (Icon, Role) {
    match state {
        AgentState::Working | AgentState::Thinking | AgentState::RunningTool(_) => (Icon::LoaderCircle, Role::Accent),
        AgentState::Done => (Icon::CircleCheck, Role::Success),
        AgentState::Errored => (Icon::CircleX, Role::Danger),
        AgentState::Interrupted => (Icon::CircleStop, Role::Text3),
        AgentState::Blocked(_) => (Icon::CircleAlert, Role::Warning),
        _ => (Icon::Circle, Role::Text3),
    }
}

fn agent_row(row: &AgentRow) -> Element<Msg> {
    match row {
        AgentRow::Section { section, count, shown } => {
            let label = if count == shown { format!("{} · {count}", section.title()) } else { format!("{} · {shown} of {count}", section.title()) };
            container(text(label.to_uppercase()).mono().scale(0.78).letter_spacing(1.0).role(Role::Text3))
                .padding([14.0, 12.0, 0.0, 12.0])
                .into()
        }
        _ => {
            let (glyph, role) = state_glyph(row.state());
            let attention = matches!(row, AgentRow::Subagent { error: Some(_), .. } | AgentRow::Subagent { warning: Some(_), .. });
            let indent = if matches!(row, AgentRow::Root { .. }) { 0.0 } else { 14.0 };
            let elapsed = row.elapsed().map(agents::elapsed_text).unwrap_or_default();
            aizome_kit::row![
                space(indent),
                icon(glyph).size(15.0).role(role),
                column![
                    text(row.title().to_owned()).semibold(),
                    text(row.subtitle().to_owned()).scale(0.85).role(if attention { Role::Warning } else { Role::Text3 }),
                ]
                .spacing(1.0)
                .width(Fill),
                text(elapsed).mono().tabular().scale(0.85).role(Role::Text3),
            ]
            .spacing(8.0)
            .padding([0.0, 12.0])
            .into()
        }
    }
}

/// The selected agent's recorded work.
fn evidence(app: &App, id: &str) -> Element<Msg> {
    let session = app.session();
    let info = session.subagents.borrow().iter().find(|a| a.id == id).cloned();
    let Some(info) = info else { return space(0.0) };
    let view = app.view_of(session.key);
    let rows = view.and_then(|v| v.agent_rows.get(id));
    let failure = session.workspace_read_failures.borrow().get(id).cloned();
    let answer = super::transcript::look(app, Role::Text);
    let quiet = super::transcript::look(app, Role::Text3);
    let mut parts: Vec<Element<Msg>> = Vec::new();
    if let Some(error) = &info.error {
        parts.push(note(Icon::CircleX, Role::Danger, error));
    }
    if let Some(warning) = &info.warning {
        parts.push(note(Icon::TriangleAlert, Role::Warning, warning));
    }
    match (rows, failure) {
        (Some(rows), _) if !rows.is_empty() => {
            for (index, row) in rows.iter().enumerate() {
                parts.push(container(super::transcript::row_view(row, None, false, None, false, &answer, &quiet)).padding([6.0, 0.0, 0.0, 0.0]).key(index).into());
            }
        }
        (_, Some(error)) => parts.push(note(Icon::CircleAlert, Role::Warning, &format!("Its journal could not be read: {error}"))),
        _ if info.session_file.is_none() => parts.push(text("It hasn't written a journal yet.").role(Role::Text3).into()),
        _ => parts.push(text("Reading its journal…").role(Role::Text3).into()),
    }
    let title = container(
        row![
            text(if info.label.is_empty() { info.id.clone() } else { info.label.clone() }).semibold().width(Fill),
            button("Discuss").icon(Icon::MessageSquareText).compact().on_press(Msg::DiscussAgent(info.id.clone())).tooltip("Quote its last answer into your draft, for the lead agent"),
            icon_button(Icon::X).on_press(Msg::SelectAgent(None)).tooltip("Close"),
        ]
        .spacing(6.0),
    )
    .padding([6.0, 10.0])
    .edges(Edge::Line, Sides::BOTTOM)
    .width(Fill);
    let busy = AgentState::from_status(&info.status).is_busy();
    let mut transcript = scroll(container(column_of(parts).width(Fill)).padding([6.0, 12.0, 12.0, 12.0])).key(("agent", id.to_owned()));
    if busy {
        transcript = transcript.follow_end();
    }
    column![title, transcript].width(Fill).height(Fill).into()
}

fn note(glyph: Icon, role: Role, text_: &str) -> Element<Msg> {
    row![icon(glyph).size(14.0).role(role), text(text_.to_owned()).role(role).wrap().selectable()]
        .spacing(8.0)
        .align(Align::Start)
        .padding([4.0, 0.0])
        .width(Fill)
        .into()
}
