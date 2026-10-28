//! The conversation: prompts, answers rendered from Markdown, reasoning
//! and tool calls folded to a line each (opened with a click), notices,
//! and `/usage` cards; kept at its end while the agent writes, unless the
//! reader has scrolled up.

use std::rc::Rc;

use aizome_kit::prelude::*;
use aizome_kit::widgets::layout::column as column_of;
use aizome_kit::{column, row};

use crate::app::{App, Msg, Row};
use crate::chat::{DetailView, ToolStatus, TranscriptMessageKind};
use crate::markdown::view::{self as md, Look};

/// The widest the reading column gets.
pub(super) const READING: f64 = 820.0;

pub(super) fn look(app: &App, role: Role) -> Look<Msg> {
    Look {
        scale: app.preferences.chat_text_size.scale() * 1.08,
        open: Rc::new(Msg::OpenLink),
        copy: Rc::new(Msg::Copy),
        pictures: app.preferences.show_inline_images.then(|| md::Pictures {
            read: app.pictures.clone(),
            base: app.picture_base(app.session().key),
            load: Rc::new(Msg::LoadPicture),
            show: Rc::new(Msg::OpenUrl),
        }),
        role,
    }
}

pub(super) fn view(app: &App) -> Element<Msg> {
    let session = app.session();
    let Some(view) = app.view_of(session.key) else {
        return space(0.0).height(Fill);
    };
    if view.rows.len() <= 1 && view.rows.first().is_none_or(|r| r.message.body == crate::chat::WELCOME) {
        return welcome(app);
    }
    let answer = look(app, Role::Text);
    let quiet = look(app, Role::Text3);
    let streaming = session.conversation.borrow().streaming_index();
    let busy = session.chat_busy.get();
    let last = view.rows.len().saturating_sub(1);
    let mut children: Vec<Element<Msg>> = Vec::with_capacity(view.rows.len() + 1);
    let mut previous: Option<TranscriptMessageKind> = None;
    for (index, row) in view.rows.iter().enumerate() {
        let expanded = view.expanded.contains(&index);
        let detail = if expanded { session.conversation.borrow().detail(index) } else { None };
        let live = busy && (streaming == Some(index) || index == last);
        let element = row_view(row, Some(index), expanded, detail, live, &answer, &quiet);
        let gap = gap(previous, row.message.kind);
        previous = Some(row.message.kind);
        children.push(container(element).padding([gap, 0.0, 0.0, 0.0]).width(Fill).key(index).into());
    }
    if busy && !matches!(previous, Some(TranscriptMessageKind::Assistant | TranscriptMessageKind::Thinking)) {
        children.push(
            container(row![icon(Icon::LoaderCircle).size(14.0).role(Role::Accent), text(working_label(app)).role(Role::Text3)].spacing(8.0))
                .padding([14.0, 0.0, 0.0, 0.0])
                .key("working")
                .into(),
        );
    }
    let content = container(column_of(children).max_width(READING).width(Fill))
        .align_x(Align::Center)
        .padding([18.0, 28.0, 28.0, 28.0])
        .width(Fill);
    let scroller = scroll(content).follow_end().scroll_to_end(view.scroll_token).key(("transcript", session.key.0));
    mouse_area(scroller).on_drop(|dropped| Msg::Dropped(dropped.urls, dropped.text)).width(Fill).height(Fill).into()
}

/// A conversation not yet begun: what to do, and what's needed first.
fn welcome(app: &App) -> Element<Msg> {
    let session = app.session();
    let signed_in = !session.authenticated_providers.borrow().is_empty();
    let project = session
        .active_project_id
        .borrow()
        .as_deref()
        .and_then(|id| app.project_by_id(id))
        .map(|p| p.name.clone());
    let mut parts: Vec<Element<Msg>> = vec![
        container(icon(Icon::Crown).size(30.0).role(Role::Accent)).padding(12.0).fill(Tone::Chrome).border(Edge::Line).into(),
        text("What would you like to work on?").scale(1.6).weight(700.0).into(),
        text(match &project {
            Some(name) => format!("Explore {name}, work through a problem, or turn an idea into a change."),
            None => "Explore a codebase, work through a problem, or turn an idea into a change.".to_owned(),
        })
        .role(Role::Text2)
        .wrap()
        .center()
        .into(),
    ];
    let hints = [
        (Icon::AtSign, "Type @ to bring a file into the conversation"),
        (Icon::SquareTerminal, "/usage shows the session's tokens, cache and limits"),
        (Icon::Paperclip, "Paste a screenshot, or drop files, to attach them"),
    ];
    parts.push(
        column_of(
            hints
                .iter()
                .map(|(glyph, hint)| row![icon(*glyph).size(14.0).role(Role::Text3), text(*hint).role(Role::Text3)].spacing(8.0).into())
                .collect(),
        )
        .spacing(6.0)
        .padding([8.0, 0.0, 0.0, 0.0])
        .into(),
    );
    if !signed_in && session.accounts_known.get() {
        parts.push(button("Sign In to an Account…").icon(Icon::KeyRound).primary().on_press(Msg::OpenSettings(crate::app::Pane::Accounts)).into());
    }
    container(column_of(parts).spacing(12.0).align(Align::Center).max_width(520.0)).center().width(Fill).height(Fill).into()
}

fn working_label(app: &App) -> String {
    app.session().activity_label().unwrap_or("Working…").to_owned()
}

/// Room above a row: chat apart, activity together.
fn gap(previous: Option<TranscriptMessageKind>, kind: TranscriptMessageKind) -> f64 {
    use TranscriptMessageKind::*;
    match (previous, kind) {
        (None, _) => 0.0,
        (Some(Tool | Event | Thinking), Tool | Event | Thinking) => 2.0,
        (_, User) => 22.0,
        (Some(User), _) => 18.0,
        _ => 12.0,
    }
}

/// One row. `index` is `None` for rows that can't be opened (a
/// sub-agent's, in the Agents panel).
pub(super) fn row_view(
    row: &Row,
    index: Option<usize>,
    expanded: bool,
    detail: Option<DetailView>,
    live: bool,
    answer: &Look<Msg>,
    quiet: &Look<Msg>,
) -> Element<Msg> {
    let message = &row.message;
    let body = |look: &Look<Msg>| -> Element<Msg> {
        match &row.doc {
            Some((_, doc)) => md::doc(doc, look),
            None => md::plain(&message.body, look),
        }
    };
    match message.kind {
        TranscriptMessageKind::User => container(
            column![
                row![
                    icon(Icon::UserRound).size(14.0).role(Role::Text3),
                    text("You").scale(0.85).semibold().role(Role::Text3),
                    spacer(),
                    icon_button(Icon::Copy).on_press(Msg::Copy(message.body.clone())).tooltip("Copy the prompt"),
                ]
                .spacing(6.0),
                body(answer),
            ]
            .spacing(4.0)
            .width(Fill),
        )
        .fill(Tone::Chrome)
        .border(Edge::Line)
        .padding([8.0, 14.0, 12.0, 14.0])
        .width(Fill)
        .into(),
        TranscriptMessageKind::Assistant => column![
            row![
                icon(Icon::Sparkles).size(14.0).role(Role::Accent),
                text("Agent").scale(0.85).semibold().role(Role::Text3),
                spacer(),
                icon_button(Icon::Copy).on_press(Msg::Copy(message.body.clone())).tooltip("Copy the answer as Markdown"),
            ]
            .spacing(6.0),
            body(answer),
        ]
        .spacing(4.0)
        .width(Fill)
        .into(),
        TranscriptMessageKind::Thinking => {
            let excerpt = message.body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").trim_matches('*').to_owned();
            let head = activity_head(
                if live { Icon::LoaderCircle } else { Icon::Brain },
                if live { Role::Accent } else { Role::Text3 },
                if live { "Thinking" } else { "Thought" },
                excerpt,
                index,
                expanded,
                true,
            );
            if expanded {
                column![head, container(body(quiet)).padding([4.0, 0.0, 6.0, 22.0]).width(Fill)].width(Fill).into()
            } else {
                head
            }
        }
        TranscriptMessageKind::Tool => {
            let (glyph, role) = match message.status {
                Some(ToolStatus::Running) => (Icon::LoaderCircle, Role::Accent),
                Some(ToolStatus::Failed) => (Icon::CircleX, Role::Danger),
                _ => (Icon::Check, Role::Success),
            };
            let label = message.author.strip_prefix("omp · ").unwrap_or(&message.author).to_owned();
            let head = activity_head(glyph, role, &label, message.body.clone(), index, expanded, detail.is_some() || index.is_some());
            match (expanded, detail) {
                (true, Some(detail)) => column![head, detail_view(&detail)].spacing(4.0).width(Fill).into(),
                _ => head,
            }
        }
        TranscriptMessageKind::Event => row![
            icon(Icon::Dot).size(14.0).role(Role::Faint),
            text(message.body.clone()).scale(0.92).role(Role::Text3).wrap().selectable(),
        ]
        .spacing(6.0)
        .align(Align::Start)
        .width(Fill)
        .into(),
        TranscriptMessageKind::Notice => {
            let failed = message.status == Some(ToolStatus::Failed) || message.body.starts_with("Error");
            container(
                row![
                    icon(if failed { Icon::TriangleAlert } else { Icon::Info }).size(15.0).role(if failed { Role::Danger } else { Role::Text3 }),
                    text(message.body.clone()).role(if failed { Role::Danger } else { Role::Text2 }).wrap().selectable(),
                ]
                .spacing(8.0)
                .align(Align::Start),
            )
            .padding([2.0, 0.0])
            .width(Fill)
            .into()
        }
        TranscriptMessageKind::Usage => super::usage_card::card(&crate::usage_card::Body::decode(&message.body)),
    }
}

/// A folded activity line: its glyph, its name, what it did, and a
/// chevron when it opens.
fn activity_head(
    glyph: Icon,
    role: Role,
    label: &str,
    summary: String,
    index: Option<usize>,
    expanded: bool,
    opens: bool,
) -> Element<Msg> {
    let line = row![
        icon(glyph).size(14.0).role(role),
        text(label.to_owned()).mono().scale(0.9).semibold().role(Role::Text2),
        text(summary.replace('\n', " ")).scale(0.92).role(Role::Text3).width(Fill),
        if opens && index.is_some() {
            icon(if expanded { Icon::ChevronDown } else { Icon::ChevronRight }).size(13.0).role(Role::Faint).into()
        } else {
            space(0.0)
        },
    ]
    .spacing(8.0)
    .padding([3.0, 0.0])
    .width(Fill);
    match index.filter(|_| opens) {
        Some(index) => mouse_area(line).on_press(Msg::ToggleRow(index)).cursor(aizome_kit::sidestep_ui::Cursor::Pointer).into(),
        None => line.into(),
    }
}

/// A tool call's full record: its intent, arguments, updates and result.
fn detail_view(detail: &DetailView) -> Element<Msg> {
    let sections = detail
        .sections
        .iter()
        .map(|section| {
            let mut parts: Vec<Element<Msg>> = Vec::new();
            if let Some(heading) = &section.heading {
                parts.push(aizome_kit::caption(heading.clone()));
            }
            let body = if section.code {
                rich_text([Span::new(section.body.clone()).mono()]).scale(0.9).wrap().selectable()
            } else {
                rich_text([Span::new(section.body.clone())]).wrap().selectable()
            };
            parts.push(container(body).padding([0.0, 10.0, 6.0, 10.0]).width(Fill).into());
            column_of(parts).width(Fill).into()
        })
        .collect();
    container(scroll(column_of(sections).width(Fill)).max_height(420.0).height(Shrink))
        .fill(Tone::Sunk)
        .border(Edge::Line)
        .padding([2.0, 0.0, 6.0, 0.0])
        .width(Fill)
        .into()
}
