//! Settings: General (reading, the apps Omperor hands things to, what new
//! sessions start with), Accounts (signing in, each account's plan and
//! limits), Projects, Archives, Sessions & Data (where things are kept,
//! tidying), Metrics (where the time went), and Developer.

use aizome_kit::prelude::*;
use aizome_kit::widgets::layout::column as column_of;
use aizome_kit::{column, row};

use crate::app::{App, Msg, Pane};
use crate::launchers::LauncherKind;
use crate::omp_runtime::{AccountState, AuthProvider};
use crate::preferences::ChatTextSize;
use crate::rules::{self, PlanTone};

pub(super) fn window(app: &App) -> Element<Msg> {
    let places = column_of(
        Pane::ALL
            .iter()
            .map(|pane| sidebar_item(pane.icon(), pane.title(), app.pane == *pane).on_press(Msg::ShowPane(*pane)).into())
            .collect(),
    )
    .spacing(1.0)
    .padding([10.0, 6.0]);
    let page = match app.pane {
        Pane::General => general(app),
        Pane::Accounts => accounts(app),
        Pane::Projects => projects(app),
        Pane::Archives => archives(app),
        Pane::Data => data(app),
        Pane::Metrics => metrics(app),
        Pane::Developer => developer(app),
    };
    let content = container(page.max_width(640.0).width(Fill)).align_x(Align::Center).padding([20.0, 24.0, 28.0, 24.0]).width(Fill);
    sidebar_layout(places, scroll(content).key(app.pane)).first_size(190.0).into()
}

fn heading(title: &str, detail: &str) -> Element<Msg> {
    column![text(title.to_owned()).scale(1.45).weight(700.0), text(detail.to_owned()).role(Role::Text3).wrap()]
        .spacing(4.0)
        .padding([0.0, 0.0, 6.0, 0.0])
        .width(Fill)
        .into()
}

fn general(app: &App) -> Element<Msg> {
    let prefs = &app.preferences;
    let size = match prefs.chat_text_size {
        ChatTextSize::Compact => 0,
        ChatTextSize::Default => 1,
        ChatTextSize::Large => 2,
    };
    let launcher = |kind: LauncherKind, preference: &Option<String>, automatic: &str| -> Element<Msg> {
        let installed = app.launchers.of(kind);
        let mut options = vec![automatic.to_owned()];
        options.extend(installed.iter().map(|l| l.name.to_owned()));
        let selected = preference.as_deref().and_then(|name| installed.iter().position(|l| l.name == name)).map_or(0, |i| i + 1);
        dropdown(options, selected).on_select(move |i| Msg::SetLauncher(kind, i)).width(220.0).into()
    };
    let first_editor = app.launchers.of(LauncherKind::Editor).first().map_or("none found".to_owned(), |l| l.name.to_owned());
    let first_terminal = app.launchers.of(LauncherKind::Terminal).first().map_or("none found".to_owned(), |l| l.name.to_owned());
    column![
        heading("General", "How conversations read, and the apps Omperor hands files and pages to."),
        form_section(
            "Conversation",
            vec![
                form_row("Text size", segmented(["Compact", "Default", "Large"], size).on_select(Msg::SetTextSize))
                    .description("Answers and prompts; the desktop's Size setting scales everything else.")
                    .into(),
                form_row("Show pictures in answers", toggle(prefs.show_inline_images).on_toggle(Msg::SetInlineImages)).into(),
                form_row("Load pictures from the web", toggle(prefs.load_remote_images).on_toggle(Msg::SetRemoteImages))
                    .description("Off by default: fetching tells the picture's server that the answer was read.")
                    .into(),
            ],
        ),
        form_section(
            "Apps",
            vec![
                form_row("Editor", launcher(LauncherKind::Editor, &prefs.editor, &format!("Automatic ({first_editor})"))).into(),
                form_row("Terminal", launcher(LauncherKind::Terminal, &prefs.terminal, &format!("Automatic ({first_terminal})"))).into(),
                form_row("Browser for signing in", launcher(LauncherKind::Browser, &prefs.browser, "The desktop's default")).into(),
            ],
        ),
        form_section("New sessions", new_sessions(app)),
    ]
    .spacing(18.0)
    .width(Fill)
    .into()
}

fn accounts(app: &App) -> Element<Msg> {
    let mut sections: Vec<Element<Msg>> = vec![heading(
        "Accounts",
        "The subscriptions and keys the agent runs on. Each project remembers the account it uses.",
    )];
    let ready = app.session().runtime_ready.get();
    sections.push(
        row![
            text(match app.usage_fetched_ms {
                Some(at) => crate::usage_card::updated_ago(at, rules::wall_clock_ms()),
                None if app.usage_loading => "Asking the providers…".to_owned(),
                None => "Plans and limits not read yet".to_owned(),
            })
            .role(Role::Text3),
            spacer(),
            button("Refresh").icon(Icon::RefreshCw).compact().enabled(ready && !app.usage_loading).on_press(Msg::RefreshUsage),
        ]
        .spacing(8.0)
        .width(Fill)
        .into(),
    );
    let session = app.session();
    let alerts = session.account_alerts.borrow();
    for provider in AuthProvider::ALL {
        let entries: Vec<_> = app.accounts.iter().filter(|a| a.provider() == provider).collect();
        let mut rows: Vec<Element<Msg>> = Vec::new();
        if let Some(alert) = alerts.for_provider(provider) {
            rows.push(alert_line(alert.message(), provider));
        }
        rows.extend(entries.iter().map(|entry| account_card(app, entry, alerts.for_account(entry.id()).map(|a| a.message()))));
        rows.push(
            container(
                row![
                    text(rules::provider_account_detail(provider)).role(Role::Text3).wrap().width(Fill),
                    button(if entries.is_empty() { "Sign In…" } else { "Add Another…" })
                        .icon(Icon::KeyRound)
                        .compact()
                        .on_press(Msg::SignIn(provider)),
                ]
                .spacing(12.0),
            )
            .padding([8.0, 12.0])
            .width(Fill)
            .into(),
        );
        sections.push(form_section(provider.product_name(), rows));
    }
    column_of(sections).spacing(18.0).width(Fill).into()
}

fn account_card(app: &App, entry: &rules::AccountEntry, alert: Option<String>) -> Element<Msg> {
    let usable = entry.is_usable();
    let expanded = app.expanded_accounts.contains(entry.id());
    let plan = rules::account_card_plan(entry.provider(), app.account_usage.get(entry.id()), app.usage_loading, usable, expanded);
    let state = match entry.summary.state {
        AccountState::Active => None,
        AccountState::RefreshRequired => Some(("Sign in again", Role::Warning)),
        AccountState::Disabled => Some(("Disabled", Role::Danger)),
        AccountState::LoggedOut => Some(("Signed out", Role::Text3)),
    };
    let mut title = vec![
        icon(Icon::UserRound).size(15.0).role(Role::Text2).into(),
        text(entry.display_name()).semibold().into(),
    ];
    if entry.is_default {
        title.push(badge("default").role(Role::Accent).into());
    }
    if let Some((label, role)) = state {
        title.push(badge(label).role(role).into());
    }
    title.push(spacer().into());
    let id = entry.id().to_owned();
    let menu = Menu::new("")
        .item(MenuItem::new("Rename…", Msg::RenameAccount(id.clone())))
        .item(MenuItem::new("Make Default", Msg::MakeDefault(id.clone())).enabled(!entry.is_default))
        .separator()
        .item(MenuItem::new("Sign In Again…", Msg::SignIn(entry.provider())))
        .item(MenuItem::new("Remove…", Msg::RemoveAccount(id.clone())));
    title.push(context_menu(button("Manage").compact().on_press(Msg::RenameAccount(id.clone())), menu).tooltip("Right-click for more").into());
    let mut lines: Vec<Element<Msg>> = vec![aizome_kit::widgets::layout::row(title).spacing(8.0).width(Fill).into()];
    if entry.label.is_some() {
        lines.push(text(entry.identity()).scale(0.88).role(Role::Text3).into());
    }
    if let Some(alert) = alert {
        lines.push(alert_line(alert, entry.provider()));
    }
    let plan_role = match plan.tone {
        PlanTone::Known => Role::Text2,
        PlanTone::Pending => Role::Text3,
        PlanTone::Warning => Role::Warning,
    };
    lines.push(text(plan.plan.clone()).role(plan_role).into());
    if let Some(note) = &plan.note {
        lines.push(text(note.clone()).scale(0.88).role(if plan.note_is_error { Role::Danger } else { Role::Text3 }).wrap().into());
    }
    for group in plan.shown_groups() {
        if plan.groups.len() > 1 || plan.summary.is_some() {
            lines.push(text(group.title.to_uppercase()).mono().scale(0.75).letter_spacing(1.0).role(Role::Text3).into());
        }
        for usage in &group.rows {
            let mut line = vec![text(usage.title.clone()).role(Role::Text2).width(Fill).into()];
            if let Some(detail) = &usage.detail {
                line.push(text(detail.clone()).scale(0.85).role(Role::Text3).into());
            }
            if let Some(figure) = &usage.figure {
                line.push(text(figure.clone()).mono().scale(0.85).into());
            } else if let Some(left) = usage.left_percent {
                line.push(text(format!("{left}% left")).mono().tabular().scale(0.85).role(if left <= 10 { Role::Danger } else { Role::Text2 }).into());
            }
            lines.push(aizome_kit::widgets::layout::row(line).spacing(10.0).width(Fill).into());
            if let Some(left) = usage.left_percent {
                lines.push(progress(f64::from(left) / 100.0).width(Fill).into());
            }
        }
    }
    if plan.foldable() {
        lines.push(
            button(if expanded { "Fewer details" } else { "All limits" })
                .plain()
                .compact()
                .on_press(Msg::ToggleAccount(id))
                .into(),
        );
    }
    container(column_of(lines).spacing(5.0).width(Fill)).padding([10.0, 12.0]).width(Fill).into()
}

fn projects(app: &App) -> Element<Msg> {
    let mut sections: Vec<Element<Msg>> = vec![heading(
        "Projects",
        "A project groups the folders an agent works in, and the sessions held there.",
    )];
    if app.projects.is_empty() {
        sections.push(text("No projects yet.").role(Role::Text3).into());
    }
    for project in &app.projects {
        let mut rows: Vec<Element<Msg>> = project
            .directories
            .iter()
            .map(|folder| {
                let missing = app.missing_directories.contains(folder);
                row![
                    icon(if missing { Icon::FolderSearch } else { Icon::Folder }).size(15.0).role(if missing { Role::Warning } else { Role::Text2 }),
                    text(folder.display().to_string()).mono().scale(0.92).width(Fill).tooltip(if missing { "Not found on disk" } else { "" }),
                    icon_button(Icon::Minus)
                        .on_press(Msg::RemoveFolder(project.id.clone(), folder.clone()))
                        .tooltip("Remove this folder from the project"),
                ]
                .spacing(8.0)
                .padding([6.0, 12.0])
                .width(Fill)
                .into()
            })
            .collect();
        rows.push(
            row![
                button("Rename…").compact().on_press(Msg::RenameProject(project.id.clone())),
                button("Add Folder…").compact().icon(Icon::FolderPlus).on_press(Msg::AddFolder(project.id.clone())),
                spacer(),
                button("Archive…").compact().icon(Icon::Archive).on_press(Msg::ArchiveProject(project.id.clone())),
                button("Remove…").compact().destructive().on_press(Msg::RemoveProject(project.id.clone())),
            ]
            .spacing(8.0)
            .padding([8.0, 12.0])
            .width(Fill)
            .into(),
        );
        sections.push(form_section(project.name.clone(), rows));
    }
    sections.push(button("New Project…").icon(Icon::FolderPlus).on_press(Msg::NewProject).into());
    column_of(sections).spacing(18.0).width(Fill).into()
}

fn archives(app: &App) -> Element<Msg> {
    let mut sections: Vec<Element<Msg>> = vec![heading(
        "Archives",
        "Projects put away with their sessions, each in one compressed file. Restoring brings everything back.",
    )];
    match &app.archives {
        None => sections.push(text("Reading the archives…").role(Role::Text3).into()),
        Some(Err(error)) => sections.push(text(format!("The archives could not be read: {error}")).role(Role::Danger).wrap().into()),
        Some(Ok(list)) if list.is_empty() => sections.push(text("No archived projects.").role(Role::Text3).into()),
        Some(Ok(list)) => {
            let rows = list
                .iter()
                .map(|listing| {
                    let m = &listing.manifest;
                    row![
                        icon(Icon::Archive).size(16.0).role(Role::Text2),
                        column![
                            text(m.project.name.clone()).semibold(),
                            text(format!(
                                "{} · {} session{} · {}",
                                rules::format_timestamp(m.archived_at_ms),
                                m.session_count(),
                                crate::housekeeping::plural(m.session_count()),
                                crate::housekeeping::byte_text(m.compressed_bytes.max(m.bytes)),
                            ))
                            .scale(0.88)
                            .role(Role::Text3),
                        ]
                        .spacing(2.0)
                        .width(Fill),
                        button("Restore").compact().on_press(Msg::RestoreArchive(m.id.clone())),
                        icon_button(Icon::Trash).on_press(Msg::DeleteArchive(m.id.clone())).tooltip("Delete this archive for good"),
                    ]
                    .spacing(10.0)
                    .padding([8.0, 12.0])
                    .width(Fill)
                    .into()
                })
                .collect();
            sections.push(form_section("Archived projects", rows));
        }
    }
    column_of(sections).spacing(18.0).width(Fill).into()
}

fn developer(app: &App) -> Element<Msg> {
    let prefs = &app.preferences;
    let backend = usize::from(prefs.effective_backend() == crate::mock_inference::Backend::Mock);
    let root = crate::app_state::root().map_or_else(|e| e.to_string(), |p| p.display().to_string());
    let helper = app.helper.as_ref().map_or_else(|| "…".to_owned(), |p| p.display().to_string());
    column![
        heading("Developer", "For working on Omperor itself."),
        form_section(
            "Testing",
            vec![
                form_row("Developer mode", toggle(prefs.developer_mode).on_toggle(Msg::SetDeveloper)).into(),
                form_row(
                    "Inference",
                    segmented(["OMP", "Mock"], backend).on_select(Msg::SetBackend).enabled(prefs.developer_mode),
                )
                .description("The mock backend scripts turns locally: no provider, account or tools. New sessions use it.")
                .into(),
                form_row("Example", button("Insert Example Prompt").compact().enabled(prefs.developer_mode).on_press(Msg::InsertExample))
                    .description("Puts a mock turn with sub-agents in the composer, to send.")
                    .into(),
            ],
        ),
        form_section(
            "Where things are",
            vec![
                form_row("State", text(root).mono().scale(0.88).selectable()).into(),
                form_row("OMP helper", text(helper).mono().scale(0.88).selectable()).into(),
            ],
        ),
    ]
    .spacing(18.0)
    .width(Fill)
    .into()
}

/// What new sessions start with: the model, its reasoning, desktop control.
fn new_sessions(app: &App) -> Vec<Element<Msg>> {
    let prefs = &app.preferences;
    let session = app.session();
    let current = prefs.model.clone().unwrap_or_default();
    let models: Vec<String> = rules::model_picker_options(
        &session.available_models.borrow(),
        &current,
        &session.authenticated_providers.borrow(),
    )
    .into_iter()
    .map(str::to_owned)
    .collect();
    let model: Element<Msg> = if models.is_empty() {
        text("Sign in to an account to choose one").role(Role::Text3).into()
    } else {
        let selected = models.iter().position(|m| *m == current);
        let shown = models.clone();
        dropdown(models.iter().map(|m| crate::agents::short_model(m)), selected)
            .placeholder("Choose a model")
            .on_select(move |i| Msg::SetDefaultModel(shown[i].clone()))
            .width(220.0)
            .into()
    };
    let level = prefs.thinking_level.clone().unwrap_or_default();
    let thinking = super::composer::THINKING;
    vec![
        form_row("Model", model).into(),
        form_row(
            "Reasoning",
            dropdown(thinking.iter().map(|l| (*l).to_owned()), thinking.iter().position(|l| *l == level))
                .placeholder("Model default")
                .on_select(move |i| Msg::SetDefaultThinking(thinking[i].to_owned()))
                .width(220.0),
        )
        .into(),
        form_row("Models offered", button("Refresh Models").icon(Icon::RefreshCw).compact().on_press(Msg::RefreshModels))
            .description("Asks each signed-in account's provider which models it offers now. The session in view restarts its agent to take them in.")
            .into(),
        form_row("Desktop control", toggle(prefs.computer_use).on_toggle(Msg::SetComputerUse))
            .description("New sessions may take screenshots and use the keyboard and pointer.")
            .into(),
    ]
}

/// A folder, in full, with a way to open it.
fn folder_row(label: &str, path: Option<std::path::PathBuf>) -> Element<Msg> {
    let shown = path.as_ref().map_or_else(|| "…".to_owned(), |p| p.display().to_string());
    let open = button("Show").icon(Icon::FolderOpen).compact().enabled(path.is_some());
    let open = match path {
        Some(path) => open.on_press(Msg::ShowInFiles(path)),
        None => open,
    };
    form_row(label, row![text(shown).mono().scale(0.85).selectable().wrap().width(Fill), open].spacing(10.0).width(Fill)).into()
}

fn data(app: &App) -> Element<Msg> {
    let sweeping = app.jobs.values().any(|job| job.label.starts_with("Removing empty"));
    column![
        heading("Sessions & Data", "Where Omperor keeps its sessions and OMP's data, and tidying them."),
        form_section(
            "Folders",
            vec![
                folder_row("Sessions", app.sessions_dir.clone()),
                folder_row("OMP data", app.data_dir.clone()),
                folder_row("Everything else", crate::app_state::root().ok()),
            ],
        ),
        form_section(
            "Tidying",
            vec![
                form_row(
                    "Empty sessions",
                    button(if sweeping { "Removing…" } else { "Remove Empty Sessions" })
                        .icon(if sweeping { Icon::LoaderCircle } else { Icon::Trash })
                        .compact()
                        .enabled(app.booted && !sweeping)
                        .on_press(Msg::SweepEmpty),
                )
                .description("Sessions OMP started that never held a conversation. No list shows them; open ones stay.")
                .into(),
                form_row("Ask the agent", text("Type /omperor in a session").mono().scale(0.9))
                    .description("Its agent can then list, archive and delete sessions and projects for you, asking before anything is removed.")
                    .into(),
            ],
        ),
    ]
    .spacing(18.0)
    .width(Fill)
    .into()
}

fn metrics(app: &App) -> Element<Msg> {
    let page = &app.metrics_page;
    let title_of = |id: &str| {
        app.sessions.iter().find(|s| s.id == id).map_or_else(|| format!("Session {}", &id[..id.len().min(8)]), |s| s.title.clone())
    };
    let mut options = vec!["All sessions".to_owned()];
    options.extend(page.sessions.iter().map(|s| {
        format!("{} · {} turn{}", title_of(&s.id), s.turns, if s.turns == 1 { "" } else { "s" })
    }));
    let selected = match &page.scope {
        None => 0,
        Some(id) => page.sessions.iter().position(|s| &s.id == id).map_or(0, |i| i + 1),
    };
    let controls = row![
        dropdown(options.clone(), Some(selected)).on_select(Msg::MetricsScope).enabled(page.records.is_some()).width(Fill),
        if page.loading {
            Element::from(icon(Icon::LoaderCircle).size(15.0).role(Role::Accent).tooltip("Reading the log…"))
        } else {
            space(0.0)
        },
        button("Refresh").icon(Icon::RefreshCw).compact().enabled(!page.loading).on_press(Msg::LoadMetrics),
        icon_button(Icon::FolderOpen)
            .enabled(app.sessions_dir.is_some())
            .on_press_maybe(app.sessions_dir.clone().map(Msg::ShowInFiles))
            .tooltip("Show the log in Files"),
    ]
    .spacing(8.0)
    .width(Fill);
    let mut parts: Vec<Element<Msg>> =
        vec![heading("Metrics", "Where the time went in the turns Omperor has run, from its metrics log."), controls.into()];
    match page.summary.as_deref() {
        None => parts.push(text("Reading the log…").role(Role::Text3).into()),
        Some(summary) if summary.turns == 0 && summary.responses == 0 && summary.tool_calls == 0 => {
            parts.push(text("Nothing recorded yet. Metrics are written as turns run.").role(Role::Text3).into());
        }
        Some(summary) => parts.extend(metrics_cards(summary, &options[selected])),
    }
    column_of(parts).spacing(18.0).width(Fill).into()
}

fn metrics_cards(summary: &crate::metrics::Summary, scope: &str) -> Vec<Element<Msg>> {
    use crate::metrics::duration_text;
    let plural = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    let mut intro = format!(
        "{scope} · {} · {} · {}",
        plural(summary.turns, "turn"),
        plural(summary.responses, "response"),
        plural(summary.tool_calls, "tool call")
    );
    if let (Some(from), Some(to)) = (summary.from_ms, summary.to_ms) {
        intro.push_str(&format!(" · {} – {}", rules::format_timestamp(from as i64), rules::format_timestamp(to as i64)));
    }
    let split = summary.split;
    let parts = [
        ("Waiting for the first token", split.first_token_ms, Role::Text3),
        ("Receiving tokens", split.receiving_ms, Role::Accent),
        ("Running tools", split.tools_ms, Role::Cyan),
        ("The harness and everything else", split.harness_ms, Role::Warning),
    ];
    let total = split.wall_ms.max(parts.iter().map(|p| p.1).sum()).max(1);
    let bar = canvas(parts.map(|p| (p.1, p.2)), move |c, bounds, parts, theme| {
        let mut x = bounds.x0;
        for (ms, role) in parts {
            let width = bounds.width() * *ms as f64 / total as f64;
            c.fill_rect(Rect::new(x, bounds.y0, x + width, bounds.y1), theme.role(*role));
            x += width;
        }
    })
    .height(10.0)
    .width(Fill);
    let mut time: Vec<Element<Msg>> = vec![container(bar).padding([10.0, 12.0, 4.0, 12.0]).width(Fill).into()];
    for (label, ms, role) in parts {
        time.push(
            container(
                row![
                    super::swatch(role),
                    text(label).role(Role::Text2).width(Fill),
                    text(duration_text(ms)).mono().tabular().scale(0.9),
                    text(format!("{:.0}%", ms as f64 * 100.0 / total as f64)).mono().tabular().scale(0.9).role(Role::Text3).width(44.0).align(aizome_kit::sidestep_ui::Alignment::Right),
                ]
                .spacing(10.0),
            )
            .padding([2.0, 12.0])
            .width(Fill)
            .into(),
        );
    }
    let ms_or_dash = |ms: Option<u64>| ms.map_or_else(|| "—".to_owned(), duration_text);
    let rate = |tps: Option<f64>| tps.map_or_else(|| "—".to_owned(), |t| format!("{t:.0}"));
    let models = super::grid(
        &[("Model", 3.0, false), ("Replies", 1.0, true), ("tok/s", 1.0, true), ("p90", 1.0, true), ("TTFT", 1.2, true), ("Fast", 1.0, true)],
        summary
            .models
            .iter()
            .map(|m| {
                vec![
                    format!("{} · {}", crate::agents::short_model(&m.model), m.provider),
                    m.responses.to_string(),
                    rate(m.median_tps),
                    rate(m.p90_tps),
                    ms_or_dash(m.median_ttft_ms),
                    format!("{}/{}", m.fast_responses, m.responses),
                ]
            })
            .collect(),
    );
    let tools = super::grid(
        &[("Tool", 3.0, false), ("Calls", 1.0, true), ("Total", 1.2, true), ("Median", 1.2, true), ("Errors", 1.0, true)],
        summary
            .tools
            .iter()
            .map(|t| vec![t.name.clone(), t.calls.to_string(), duration_text(t.total_ms), duration_text(t.median_ms), t.errors.to_string()])
            .collect(),
    );
    vec![
        text(intro).role(Role::Text2).wrap().into(),
        form_section("Where the time went", time),
        form_section("Throughput by model", vec![models]),
        form_section("Tools", vec![tools]),
    ]
}

/// What went wrong with an account or a provider, and the way out.
fn alert_line(message: String, provider: AuthProvider) -> Element<Msg> {
    container(
        row![
            icon(Icon::TriangleAlert).size(14.0).role(Role::Warning),
            text(message).scale(0.9).role(Role::Text2).wrap().width(Fill),
            button("Sign In Again…").compact().on_press(Msg::SignIn(provider)),
        ]
        .spacing(8.0),
    )
    .padding([6.0, 12.0])
    .width(Fill)
    .into()
}
