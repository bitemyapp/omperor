//! The sidebar: Omperor's mark, a search over sessions, Omperor's own
//! sessions under their projects (or OMP's other sessions, read-only), and
//! a way to start a project.

use std::collections::HashSet;

use aizome_kit::prelude::*;
use aizome_kit::widgets::layout::column as column_of;
use aizome_kit::{column, row};

use crate::agents::AgentState;
use crate::app::{App, Msg};
use crate::rules::{self, SessionSourceFilter, SidebarRow};

/// The most of OMP's own sessions the read-only list shows.
const OMP_LIMIT: usize = 300;

pub(super) fn view(app: &App) -> Element<Msg> {
    let brand = row![
        container(icon(Icon::Crown).size(18.0).role(Role::Accent)).padding(5.0).fill(Tone::Chrome).border(Edge::Line),
        column![
            text(app.info_name()).scale(1.15).weight(700.0),
            text("Coding agents · OMP").scale(0.85).role(Role::Text3),
        ]
        .spacing(1.0),
        spacer(),
        icon_button(Icon::MessageSquarePlus).on_press(Msg::NewSession(None)).tooltip("New session (❖N)"),
    ]
    .spacing(10.0)
    .padding([14.0, 10.0, 10.0, 12.0]);
    let search = container(search_field(&app.sidebar_query).placeholder("Search sessions").on_change(Msg::Search).id("session-search").width(Fill))
        .padding([0.0, 10.0, 6.0, 10.0]);
    let source = container(
        segmented(["Omperor", "OMP"], match app.sidebar_source {
            SessionSourceFilter::Omperor => 0,
            SessionSourceFilter::Omp => 1,
        })
        .on_select(Msg::Source)
        .width(Fill),
    )
    .padding([0.0, 10.0, 8.0, 10.0]);
    let foot = container(
        row![
            button("New Project…").icon(Icon::FolderPlus).compact().on_press(Msg::NewProject),
            spacer(),
            discovery_state(app),
        ]
        .spacing(8.0),
    )
    .padding([8.0, 10.0])
    .edges(Edge::Line, Sides::TOP)
    .width(Fill);
    column![brand, search, source, divider(), scroll(list(app)).width(Fill).height(Fill), foot]
        .width(Fill)
        .height(Fill)
        .into()
}

fn discovery_state(app: &App) -> Element<Msg> {
    if app.discovering {
        return text("Reading…").scale(0.85).role(Role::Text3).into();
    }
    match &app.discovery_error {
        Some(error) => icon(Icon::CircleAlert).size(14.0).role(Role::Warning).tooltip(error.clone()).into(),
        None => space(0.0),
    }
}

fn list(app: &App) -> Element<Msg> {
    let query = app.sidebar_query.trim().to_lowercase();
    let rows = rules::sidebar_rows_for(&app.projects, &app.sessions, &app.records, &app.positions, app.sidebar_source, &query);
    // Where each session stands among its project's, for Move Up and Down.
    let mut places: std::collections::HashMap<usize, (usize, usize)> = std::collections::HashMap::new();
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for row in &rows {
        if let SidebarRow::Session { index, project_id: Some(project) } = row {
            let count = counts.entry(project.as_str()).or_default();
            places.insert(*index, (*count, 0));
            *count += 1;
        }
    }
    for row in &rows {
        if let SidebarRow::Session { index, project_id: Some(project) } = row
            && let Some(place) = places.get_mut(index)
        {
            place.1 = counts[project.as_str()];
        }
    }
    let selected_id = app.session().live_session_id.borrow().clone();
    let selected_path = app.session().selected_session_path.borrow().clone();
    // The selected session and the busy ones stay in view when a project folds.
    let keep: HashSet<usize> = app
        .sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            selected_path.as_ref() == Some(&s.path)
                || app.sidebar_state(&s.id).is_some_and(|state| state.is_busy())
        })
        .map(|(i, _)| i)
        .collect();
    let rows = if query.is_empty() { rules::fold_rows(rows, &app.expanded_projects, &keep) } else { rows };
    if rows.is_empty() {
        return container(
            text(if app.discovering {
                "Reading sessions…"
            } else if query.is_empty() {
                "No sessions yet. Write a message to start one."
            } else {
                "No session matches."
            })
            .wrap()
            .role(Role::Text3),
        )
        .padding([12.0, 14.0])
        .width(Fill)
        .into();
    }
    let mut live = live_rows(app, &query);
    let mut items: Vec<Element<Msg>> = Vec::new();
    for row in rows.into_iter().take(OMP_LIMIT) {
        match row {
            SidebarRow::Project { project_id } => {
                let Some(project) = app.project_by_id(&project_id) else { continue };
                items.push(project_row(app, project).key(("project", project_id.clone())));
                // Sessions started here that OMP hasn't listed yet come first.
                if !project.collapsed || !query.is_empty() {
                    items.extend(live.remove(&project_id).unwrap_or_default());
                }
            }
            SidebarRow::Unfiled => items.push(aizome_kit::caption("Other folders")),
            SidebarRow::ShowMore { project_id, hidden, expanded } => {
                let label = if expanded { "Show fewer".to_owned() } else { format!("Show {hidden} more") };
                items.push(
                    container(button(label).plain().compact().on_press(Msg::ShowMore(project_id.clone())))
                        .padding([0.0, 0.0, 0.0, 30.0])
                        .key(("more", project_id))
                        .into(),
                );
            }
            SidebarRow::Session { index, .. } => {
                let Some(summary) = app.sessions.get(index) else { continue };
                let selected = selected_id.as_deref() == Some(summary.id.as_str())
                    || selected_path.as_ref() == Some(&summary.path);
                let place = places.get(&index).copied();
                items.push(session_row(app, summary, selected, place).key(("session", summary.id.clone())));
            }
        }
    }
    column_of(items).spacing(1.0).padding([4.0, 6.0, 8.0, 6.0]).width(Fill).into()
}

/// Rows for the live sessions OMP hasn't listed yet (a session just started,
/// before its first answer is written), by project: the one in view, and
/// any with something written in them.
fn live_rows(app: &App, query: &str) -> std::collections::HashMap<String, Vec<Element<Msg>>> {
    let listed: HashSet<&str> = app.sessions.iter().map(|s| s.id.as_str()).collect();
    let current = app.session().key;
    let mut rows: std::collections::HashMap<String, Vec<Element<Msg>>> = std::collections::HashMap::new();
    for session in app.live.entries() {
        if session.read_only_session.get() {
            continue;
        }
        let id = session
            .live_session_id
            .borrow()
            .clone()
            .or_else(|| session.header_session.borrow().as_ref().map(|(id, _)| id.clone()));
        if id.as_deref().is_some_and(|id| listed.contains(id)) {
            continue;
        }
        let Some(project) = session.active_project_id.borrow().clone() else { continue };
        let prompt = session.conversation.borrow().message(0).filter(|m| m.kind == crate::chat::TranscriptMessageKind::User);
        let drafted = app.view_of(session.key).is_some_and(|v| !v.editor.text().trim().is_empty());
        let selected = session.key == current;
        if !selected && prompt.is_none() && !drafted {
            continue;
        }
        let draft = app.view_of(session.key).map(|v| v.editor.text()).unwrap_or_default();
        let title = match prompt {
            Some(m) => crate::session::title_from_prompt(&m.body),
            None if !draft.trim().is_empty() => crate::session::title_from_prompt(&draft),
            None => "New session".to_owned(),
        };
        if !query.is_empty() && !title.to_lowercase().contains(query) {
            continue;
        }
        let state = session.sidebar_state();
        let detail = if state.is_busy() { state.label() } else { "new".to_owned() };
        let row = container(sidebar_item(state_icon(Some(&state)), title, selected).on_press(Msg::SelectLive(session.key)).detail(detail))
            .padding([0.0, 0.0, 0.0, 14.0])
            .width(Fill)
            .key(("live", session.key.0))
            .into();
        rows.entry(project).or_default().push(row);
    }
    rows
}

fn project_row(app: &App, project: &crate::preferences::Project) -> Element<Msg> {
    let missing = project.directories.iter().any(|d| app.missing_directories.contains(d));
    let chevron = if project.collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
    let id = project.id.clone();
    let heading = mouse_area(
        row![
            icon(chevron).size(13.0).role(Role::Text3),
            icon(if missing { Icon::FolderSearch } else { Icon::Folder }).size(15.0).role(if missing { Role::Warning } else { Role::Text2 }),
            text(project.name.clone()).semibold().role(Role::Text2),
            spacer(),
        ]
        .spacing(6.0)
        .padding([7.0, 6.0, 5.0, 6.0])
        .width(Fill),
    )
    .on_press(Msg::ToggleProject(id.clone()))
    .tooltip(rules::project_tooltip(project));
    let at = app.projects.iter().position(|p| p.id == id).unwrap_or(0);
    let menu = Menu::new(project.name.clone())
        .item(MenuItem::new("New Session", Msg::NewSession(Some(id.clone()))).icon(Icon::MessageSquarePlus))
        .separator()
        .item(MenuItem::new("Move Up", Msg::MoveProject(id.clone(), -1)).enabled(at > 0))
        .item(MenuItem::new("Move Down", Msg::MoveProject(id.clone(), 1)).enabled(at + 1 < app.projects.len()))
        .separator()
        .item(MenuItem::new("Rename…", Msg::RenameProject(id.clone())))
        .item(MenuItem::new("Add Folder…", Msg::AddFolder(id.clone())))
        .item(MenuItem::new("Archive…", Msg::ArchiveProject(id.clone())).icon(Icon::Archive))
        .separator()
        .item(MenuItem::new("Remove from Omperor…", Msg::RemoveProject(id.clone())));
    let row = row![
        heading,
        icon_button(Icon::Plus).on_press(Msg::NewSession(Some(id))).tooltip("New session in this project"),
    ]
    .spacing(2.0)
    .width(Fill);
    context_menu(row, menu).into()
}

/// A session's glyph: what it is doing, or how it ended.
pub(super) fn state_icon(state: Option<&AgentState>) -> Icon {
    match state {
        Some(AgentState::Working | AgentState::Thinking | AgentState::RunningTool(_)) => Icon::LoaderCircle,
        Some(AgentState::Blocked(_)) => Icon::CircleAlert,
        Some(AgentState::Errored) => Icon::CircleX,
        Some(AgentState::Interrupted) => Icon::CircleStop,
        _ => Icon::MessageSquare,
    }
}

/// A session's row. `place` is where it stands among its project's
/// sessions, and how many there are.
fn session_row(
    app: &App,
    summary: &crate::session::SessionSummary,
    selected: bool,
    place: Option<(usize, usize)>,
) -> Element<Msg> {
    // A session no child serves shows how it ended.
    let state = app
        .sidebar_state(&summary.id)
        .or_else(|| summary.outcome.map(|outcome| AgentState::from_outcome(Some(outcome))));
    let detail = match &state {
        Some(state) if state.is_busy() => state.label(),
        _ => rules::ago(summary.modified_ms(), crate::preferences::now_ms()),
    };
    let item = sidebar_item(state_icon(state.as_ref()), summary.title.clone(), selected)
        .on_press(Msg::Open(summary.id.clone()))
        .detail(detail);
    let tooltip = rules::session_tooltip(summary, app.records.get(&summary.id));
    let indent = if summary.is_omperor_owned() { 14.0 } else { 0.0 };
    let element = container(item).padding([0.0, 0.0, 0.0, indent]).width(Fill).tooltip(tooltip);
    if !summary.is_omperor_owned() {
        return element.into();
    }
    let id = summary.id.clone();
    let (at, count) = place.unwrap_or((0, 1));
    let mut menu = Menu::new(summary.title.clone())
        .item(MenuItem::new("Open", Msg::Open(id.clone())))
        .item(MenuItem::new("Timeline…", Msg::ShowTimeline(Some(id.clone()))).icon(Icon::ChartGantt))
        .separator()
        .item(MenuItem::new("Fork", Msg::Fork(id.clone())).icon(Icon::GitFork))
        .item(MenuItem::new("Fork into Worktree…", Msg::ForkIntoWorktree(id.clone())).icon(Icon::GitBranch))
        .separator()
        .item(MenuItem::new("Move Up", Msg::MoveSession(id.clone(), -1)).enabled(place.is_some() && at > 0))
        .item(MenuItem::new("Move Down", Msg::MoveSession(id.clone(), 1)).enabled(place.is_some() && at + 1 < count))
        .separator()
        .item(MenuItem::new("Copy Session ID", Msg::Copy(id.clone())));
    if let Some(dir) = summary.cwd.clone() {
        menu = menu.separator().item(MenuItem::new("Show Folder in Files", Msg::ShowInFiles(dir)));
    }
    let live = app.live.entries().into_iter().find(|s| s.live_session_id.borrow().as_deref() == Some(id.as_str()));
    if let Some(live) = live.filter(|s| s.chat_busy.get()) {
        menu = menu.separator().item(MenuItem::new("Stop", Msg::StopSession(live.key)).icon(Icon::CircleStop));
    }
    menu = menu.separator().item(MenuItem::new("Delete…", Msg::DeleteSession(id)).icon(Icon::Trash));
    context_menu(element, menu).into()
}

impl App {
    fn info_name(&self) -> String {
        crate::app_state::Channel::current().app_name().to_owned()
    }
}
