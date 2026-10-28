//! The windows, drawn from the program's state every time it changes:
//! the main window (the sidebar of projects and sessions, the conversation
//! and its composer, the panel of changes or agents beside it), Settings,
//! and the sheets that ask for something.
//!
//! Only the kit's widgets and the theme's colours: the frame is the kit's
//! (`sidebar_layout`, `toolbar`, `status_bar`), and Omperor draws its
//! transcript, composer and panels from them.

mod agents;
mod changes;
mod composer;
mod settings;
mod sheets;
mod sidebar;
mod timeline;
mod transcript;
mod usage;
mod usage_card;

use aizome_kit::prelude::*;
use aizome_kit::{column, row};

use crate::app::{App, Msg, Panel, Pane, Sheet};
use crate::launchers::LauncherKind;

/// The sidebar's width to start with.
const SIDEBAR: f64 = 264.0;
/// The panel beside the conversation.
const PANEL: f64 = 400.0;

pub(crate) fn view(app: &App, window: WindowKey) -> Element<Msg> {
    if let Some(sheet) = app.sheets.get(&window) {
        return match sheet {
            Sheet::SignIn => sheets::sign_in(app),
            Sheet::Prompt(prompt) => sheets::prompt(prompt),
            Sheet::Choice(choice) => sheets::choice(choice),
        };
    }
    if Some(window) == app.settings {
        return settings::window(app);
    }
    if app.timeline.as_ref().is_some_and(|t| t.window == window) {
        return timeline::window(app);
    }
    if app.usage_window.as_ref().is_some_and(|u| u.window == window) {
        return usage::window(app);
    }
    main_window(app)
}

fn main_window(app: &App) -> Element<Msg> {
    if let Some(error) = &app.boot_error {
        return container(
            column![
                icon(Icon::TriangleAlert).size(28.0).role(Role::Danger),
                text("Omperor could not start").scale(1.3).semibold(),
                text(error.clone()).wrap().role(Role::Text2).max_width(520.0),
            ]
            .spacing(10.0)
            .align(Align::Center),
        )
        .center()
        .width(Fill)
        .height(Fill)
        .into();
    }
    let session = app.session();
    let view = app.view_of(session.key);
    let conversation = column![transcript::view(app), composer::view(app)].width(Fill).height(Fill);
    // The conversation keeps its place in the tree whether a panel shows or
    // not, so its reading position, selection and composer survive.
    let panel = view.and_then(|v| v.panel());
    let side: Element<Msg> = match panel {
        Some(Panel::Changes) => changes::view(app),
        Some(Panel::Agents) => agents::view(app),
        None => space(0.0),
    };
    let body = split(conversation.key(("conversation", session.key.0)), side)
        .second_size(PANEL)
        .min_sizes(420.0, 300.0)
        .collapse_second(panel.is_none())
        .key("body");
    let mut main = column![tool_bar(app).key("toolbar")];
    if let Some(banner) = banner(app) {
        main = main.push(banner.key("banner"));
    }
    let main = main.push(body).push(status_line(app).key("status")).width(Fill).height(Fill);
    sidebar_layout(sidebar::view(app), main).first_size(SIDEBAR).min_sizes(200.0, 600.0).into()
}

/// A path with the home folder written `~`.
pub(crate) fn home_relative(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(std::path::PathBuf::from) {
        Some(home) if path.starts_with(&home) => format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display()),
        _ => path.display().to_string(),
    }
}

/// The title of the session in view.
pub(crate) fn session_title(app: &App) -> String {
    let session = app.session();
    if let Some((id, _)) = session.header_session.borrow().as_ref()
        && let Some(summary) = app.sessions.iter().find(|s| s.id == *id)
    {
        return summary.title.clone();
    }
    let id = session.live_session_id.borrow().clone();
    if let Some(summary) = id.and_then(|id| app.sessions.iter().find(|s| s.id == id)) {
        return summary.title.clone();
    }
    // Not listed yet: named after its first prompt, as OMP will name it.
    if let Some(prompt) = session.conversation.borrow().message(0).filter(|m| m.kind == crate::chat::TranscriptMessageKind::User) {
        return crate::session::title_from_prompt(&prompt.body);
    }
    "New session".to_owned()
}

fn tool_bar(app: &App) -> Element<Msg> {
    let session = app.session();
    let project = session
        .active_project_id
        .borrow()
        .as_deref()
        .and_then(|id| app.project_by_id(id))
        .map(|p| p.name.clone())
        .unwrap_or_else(|| crate::rules::path_leaf(&session.current_project.borrow()));
    let panel = app.view_of(session.key).and_then(|v| v.panel());
    let changes = session.workspace_snapshot.borrow().as_ref().map_or(0, |s| s.changes.len());
    let running = session
        .subagents
        .borrow()
        .iter()
        .filter(|a| crate::agents::AgentState::from_status(&a.status).is_busy())
        .count();
    let title = container(text(session_title(app)).scale(1.05).semibold().max_lines(1)).padding([0.0, 4.0, 0.0, 6.0]).max_width(420.0);
    let mut items: Vec<Element<Msg>> = vec![
        context_menu(title, session_actions(app, Menu::new(""))).tooltip("Right-click for the session's ID and journal").into(),
        // The folder takes what room is left, cut short when there is little.
        text(format!("{project} · {}", home_relative(&session.current_project.borrow())))
            .mono()
            .tabular()
            .size(11.0)
            .role(Role::Text3)
            .width(Fill)
            .into(),
    ];
    let mut changes_button = button(if changes > 0 { format!("Changes {changes}") } else { "Changes".to_owned() })
        .icon(Icon::FileDiff)
        .toolbar()
        .selected(panel == Some(Panel::Changes))
        .on_press(Msg::ShowPanel(Panel::Changes));
    changes_button = changes_button.tooltip("The project's uncommitted changes (❖1)");
    items.push(changes_button.into());
    let agents_label = if running > 0 { format!("Agents {running}") } else { "Agents".to_owned() };
    items.push(
        button(agents_label)
            .icon(Icon::Bot)
            .toolbar()
            .selected(panel == Some(Panel::Agents))
            .on_press(Msg::ShowPanel(Panel::Agents))
            .tooltip("The session's sub-agents and their transcripts (❖2)")
            .into(),
    );
    items.push(toolbar_separator());
    items.push(icon_button(Icon::ChartGantt).toolbar().on_press(Msg::ShowTimeline(None)).tooltip("Timeline: every prompt, response and tool call on one clock (❖5)").into());
    items.push(icon_button(Icon::ChartBar).toolbar().on_press(Msg::ShowUsage).tooltip("Usage: tokens, cache and requests").into());
    items.push(toolbar_separator());
    items.push(icon_button(Icon::Settings).toolbar().on_press(Msg::OpenSettings(Pane::General)).tooltip("Settings (❖,)").into());
    toolbar(items)
}

/// A strip under the toolbar when something needs the reader: an account
/// that must sign in again, a prompt cache about to lapse.
fn banner(app: &App) -> Option<Element<Msg>> {
    let session = app.session();
    let alerts = session.account_alerts.borrow();
    let alert = alerts.visible(std::time::Instant::now())?.clone();
    drop(alerts);
    let provider = alert.provider;
    let action = match provider {
        Some(provider) => button("Sign In Again").compact().on_press(Msg::SignIn(provider)),
        None => button("Accounts…").compact().on_press(Msg::OpenSettings(Pane::Accounts)),
    };
    Some(
        container(
            row![
                icon(Icon::TriangleAlert).size(15.0).role(Role::Warning),
                text(alert.message()).role(Role::Text2).wrap().width(Fill),
                action,
                button("Later").plain().compact().on_press(Msg::SnoozeAlert(true)).tooltip("Hide this for half an hour"),
                icon_button(Icon::X).on_press(Msg::SnoozeAlert(false)).tooltip("Hide this until Omperor starts again"),
            ]
            .spacing(10.0),
        )
        .fill(Tone::Chrome)
        .edges(Edge::Line, Sides::BOTTOM)
        .padding([6.0, 12.0])
        .width(Fill)
        .into(),
    )
}

fn status_line(app: &App) -> Element<Msg> {
    let session = app.session();
    let mut items: Vec<Element<Msg>> = Vec::new();
    if let Some(activity) = app.activity() {
        items.push(progress_indeterminate().width(64.0).into());
        items.push(status_text(activity));
    } else if let Some((note, _)) = &app.note {
        items.push(status_text(note.clone()));
    } else {
        let status = session.chat_status_text.borrow().clone();
        items.push(status_text(status));
    }
    items.push(spacer().into());
    if let Some(warning) = session.cache_clock.borrow().warning(crate::rules::wall_clock_ms()) {
        items.push(
            text(warning.badge_label.clone())
                .mono()
                .tabular()
                .size(11.0)
                .role(if warning.urgent { Role::Warning } else { Role::Text3 })
                .tooltip(warning.tooltip.clone())
                .into(),
        );
    }
    if let Some(warning) = session.cache_warning.borrow().as_ref() {
        items.push(
            row![icon(Icon::TriangleAlert).size(12.0).role(Role::Warning), status_text(warning.badge_label.clone())]
                .spacing(4.0)
                .tooltip(format!("{}\n{}", warning.headline, warning.explanation))
                .into(),
        );
    }
    let model = session.current_model.borrow().clone();
    if !model.is_empty() {
        items.push(status_text(crate::agents::short_model(&model)));
    }
    status_bar(items)
}

pub(crate) fn menus(app: &App) -> MenuBar<Msg> {
    let info = app.info_for_menus();
    let role = |label: &str, role: MenuRole| MenuItem::role_item(label, role);
    let name = info.name.clone();
    let app_menu = Menu::new(name.clone())
        .item(role(&format!("About {name}"), MenuRole::About))
        .separator()
        .item(MenuItem::new("Settings…", Msg::OpenSettings(Pane::General)).shortcut(Shortcut::cmd(',')))
        .item(MenuItem::new("Accounts…", Msg::OpenSettings(Pane::Accounts)))
        .separator()
        .submenu(Menu::new("Services").item(role("No Services Apply", MenuRole::Services).enabled(false)))
        .separator()
        .item(role(&format!("Hide {name}"), MenuRole::Hide).shortcut(Shortcut::cmd('H')))
        .item(role("Hide Others", MenuRole::HideOthers).shortcut(Shortcut::cmd('H').alt()))
        .separator()
        .item(role(&format!("Quit {name}"), MenuRole::Quit).shortcut(Shortcut::cmd('Q')));
    let session = app.session();
    let id = session.live_session_id.borrow().clone();
    let owned = id.as_ref().is_some_and(|id| app.sessions.iter().any(|s| &s.id == id && s.is_omperor_owned()));
    let busy = session.chat_busy.get();
    let folder = session.current_project.borrow().clone();
    let editor = app.launchers.choose(LauncherKind::Editor, app.preferences.editor.as_deref()).map_or("Editor", |l| l.name);
    let terminal = app.launchers.choose(LauncherKind::Terminal, app.preferences.terminal.as_deref()).map_or("Terminal", |l| l.name);
    let file = Menu::new("File")
        .item(MenuItem::new("New Session", Msg::NewSession(None)).shortcut(Shortcut::cmd('N')).icon(Icon::MessageSquarePlus))
        .item(MenuItem::new("New Project…", Msg::NewProject).shortcut(Shortcut::cmd('N').shift()).icon(Icon::FolderPlus))
        .separator()
        .item(MenuItem::new("Attach Files…", Msg::Attach).shortcut(Shortcut::cmd('O')).icon(Icon::Paperclip))
        .separator()
        .item(MenuItem::new(format!("Open Project in {editor}"), Msg::OpenInEditor(folder.clone())).shortcut(Shortcut::cmd('E').shift()).icon(Icon::Code))
        .item(MenuItem::new(format!("Open {terminal} Here"), Msg::OpenTerminal(folder.clone())).shortcut(Shortcut::cmd('T').shift()).icon(Icon::SquareTerminal))
        .item(MenuItem::new("Show Project in Files", Msg::ShowInFiles(folder)).icon(Icon::FolderOpen));
    let mut session_menu = Menu::new("Session")
        .item(MenuItem::new("Send", Msg::Send).shortcut(Shortcut::cmd_key(ShortcutKey::Enter)))
        .item(MenuItem::new("Stop", Msg::Stop).shortcut(Shortcut::cmd('.')).enabled(busy))
        .separator();
    if let Some(id) = id.clone().filter(|_| owned) {
        session_menu = session_menu
            .item(MenuItem::new("Fork", Msg::Fork(id.clone())).icon(Icon::GitFork))
            .item(MenuItem::new("Fork into Worktree…", Msg::ForkIntoWorktree(id.clone())).icon(Icon::GitBranch))
            .separator()
            .item(MenuItem::new("Delete Session…", Msg::DeleteSession(id)).icon(Icon::Trash))
            .separator();
    }
    let session_menu = session_actions(app, session_menu);
    let panel = app.view_of(session.key).and_then(|v| v.panel());
    let view = Menu::new("View")
        .item(
            MenuItem::new("Changes", Msg::ShowPanel(Panel::Changes))
                .shortcut(Shortcut::cmd('1'))
                .checked(panel == Some(Panel::Changes)),
        )
        .item(
            MenuItem::new("Agents", Msg::ShowPanel(Panel::Agents))
                .shortcut(Shortcut::cmd('2'))
                .checked(panel == Some(Panel::Agents)),
        )
        .item(MenuItem::new(if panel.is_some() { "Hide Panel" } else { "Show Panel" }, Msg::TogglePanel).shortcut(Shortcut::cmd('0')))
        .separator()
        .item(MenuItem::new("Search Sessions", Msg::FocusSearch).shortcut(Shortcut::cmd('F').shift()).icon(Icon::Search))
        .item(MenuItem::new("Refresh Changes", Msg::RefreshChanges).shortcut(Shortcut::cmd('R')));
    MenuBar::standard(&info)
        .replace(&name, app_menu)
        .with_file_menu(file)
        .with_menu(session_menu)
        .with_menu(view)
}

impl App {
    fn info_for_menus(&self) -> AppInfo {
        <App as Program>::info(self)
    }
}

/// A small square of `role`'s colour, a key to a chart.
pub(super) fn swatch(role: Role) -> Element<Msg> {
    canvas(role, |c, bounds, role, theme| c.fill_rect(bounds, theme.role(*role))).width(10.0).height(10.0).into()
}

/// Figures in columns: `(title, share of the width, numeric)` for each
/// column, numbers set at the right; a line under the titles.
pub(super) fn grid(columns: &[(&str, f64, bool)], rows: Vec<Vec<String>>) -> Element<Msg> {
    let cell = |value: String, (weight, numeric): (f64, bool), head: bool| -> Element<Msg> {
        let mut t = text(value).max_lines(1);
        if numeric && !head {
            t = t.mono().tabular().scale(0.9);
        }
        if head {
            t = t.scale(0.85).semibold().role(Role::Text3);
        }
        let boxed = container(t).width(Length::FillPortion((weight * 10.0) as u16));
        if numeric { boxed.align_x(Align::End).into() } else { boxed.into() }
    };
    let shape: Vec<(f64, bool)> = columns.iter().map(|c| (c.1, c.2)).collect();
    let line = |values: Vec<String>, head: bool| -> Element<Msg> {
        aizome_kit::widgets::layout::row(values.into_iter().zip(shape.iter()).map(|(v, s)| cell(v, *s, head)).collect())
            .spacing(10.0)
            .padding([4.0, 12.0])
            .width(Fill)
            .into()
    };
    let mut lines = vec![line(columns.iter().map(|c| c.0.to_owned()).collect(), true), divider().into()];
    if rows.is_empty() {
        lines.push(container(text("None").role(Role::Text3)).padding([4.0, 12.0]).into());
    }
    lines.extend(rows.into_iter().map(|r| line(r, false)));
    aizome_kit::widgets::layout::column(lines).padding([4.0, 0.0, 6.0, 0.0]).width(Fill).into()
}

/// The session in view's own actions, added to `menu`: its timeline and
/// usage, and its ID and journal for working with it elsewhere.
fn session_actions(app: &App, menu: Menu<Msg>) -> Menu<Msg> {
    let session = app.session();
    let id = session.live_session_id.borrow().clone();
    let journal = app.journal_in_view();
    let mut menu = menu
        .item(MenuItem::new("Timeline…", Msg::ShowTimeline(None)).shortcut(Shortcut::cmd('5')).icon(Icon::ChartGantt))
        .item(MenuItem::new("Usage…", Msg::ShowUsage).icon(Icon::ChartBar))
        .separator()
        .item(MenuItem::new("Copy Session ID", Msg::Copy(id.clone().unwrap_or_default())).enabled(id.is_some()));
    menu = match journal {
        Some(path) => menu
            .item(MenuItem::new("Copy Journal Path", Msg::Copy(path.display().to_string())))
            .item(MenuItem::new(
                "Show Journal in Files",
                Msg::ShowInFiles(path.parent().map(std::path::Path::to_path_buf).unwrap_or_default()),
            )),
        None => menu
            .item(MenuItem::new("Copy Journal Path", Msg::ClearNote).enabled(false))
            .item(MenuItem::new("Show Journal in Files", Msg::ClearNote).enabled(false)),
    };
    menu
}

/// A pane with nothing to list yet: a glyph, what is going on, and what to do.
pub(super) fn empty_state(glyph: Icon, role: Role, title: &str, detail: &str) -> Element<Msg> {
    container(
        column![
            icon(glyph).size(22.0).role(role),
            text(title.to_owned()).semibold().role(Role::Text2),
            text(detail.to_owned()).role(Role::Text3).wrap().center(),
        ]
        .spacing(8.0)
        .align(Align::Center)
        .max_width(300.0),
    )
    .center()
    .width(Fill)
    .height(Fill)
    .into()
}
