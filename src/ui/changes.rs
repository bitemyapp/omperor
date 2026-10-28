//! The Changes panel: the branch and how it stands, the uncommitted files
//! as a tree, and the selected file's diff, coloured.

use std::rc::Rc;

use aizome_kit::prelude::*;
use aizome_kit::{column, row};

use crate::app::{App, Diff, Msg};
use crate::change_tree::{self, TreeRow};
use crate::launchers::LauncherKind;

pub(super) fn view(app: &App) -> Element<Msg> {
    let session = app.session();
    let snapshot = session.workspace_snapshot.borrow().clone();
    let failure = session.workspace_read_failures.borrow().get("").cloned();
    let reading = app.inspecting.contains(&session.key);
    let branch = snapshot.as_ref().and_then(|s| s.branch.clone());
    let mut header = vec![
        icon(Icon::GitBranch).size(15.0).role(Role::Text2).into(),
        text(branch.clone().unwrap_or_else(|| if snapshot.as_ref().is_some_and(|s| s.repository_root.is_some()) { "detached HEAD".into() } else { "Changes".into() }))
            .mono()
            .semibold()
            .into(),
    ];
    if let Some(upstream) = snapshot.as_ref().and_then(|s| s.upstream) {
        header.push(text(format!("↑{} ↓{}", upstream.ahead, upstream.behind)).mono().scale(0.9).role(Role::Text3).tooltip("Commits ahead of and behind its upstream").into());
    }
    header.push(spacer().into());
    if reading {
        header.push(icon(Icon::LoaderCircle).size(14.0).role(Role::Accent).tooltip("Reading the folder…").into());
    }
    let dir = session.current_project.borrow().clone();
    header.push(icon_button(Icon::SquareTerminal).on_press(Msg::OpenTerminal(dir.clone())).tooltip("Open a terminal here").into());
    header.push(icon_button(Icon::Code).on_press(Msg::OpenInEditor(dir)).tooltip("Open the project in your editor").into());
    header.push(icon_button(Icon::RefreshCw).on_press(Msg::RefreshChanges).tooltip("Read the changes again (❖R)").into());
    let header = container(aizome_kit::widgets::layout::row(header).spacing(6.0).width(Fill))
        .padding([8.0, 10.0])
        .edges(Edge::Line, Sides::BOTTOM)
        .width(Fill);
    let body: Element<Msg> = match (&snapshot, failure) {
        (None, Some(error)) => super::empty_state(Icon::CircleAlert, Role::Warning, "The folder could not be read", &error),
        (None, None) => super::empty_state(Icon::LoaderCircle, Role::Accent, "Reading the folder…", ""),
        (Some(snapshot), _) if snapshot.repository_root.is_none() => super::empty_state(
            Icon::FolderSearch,
            Role::Text3,
            "Not a git repository",
            "Changes show for folders in a git repository.",
        ),
        (Some(snapshot), _) if snapshot.changes.is_empty() => super::empty_state(
            Icon::CircleCheck,
            Role::Success,
            "No uncommitted changes",
            snapshot.head_subject.as_deref().unwrap_or(""),
        ),
        (Some(snapshot), _) => {
            let (added, removed) = snapshot.totals();
            let count = snapshot.changes.len();
            let summary = container(
                row![
                    text(format!("{count} file{} changed", if count == 1 { "" } else { "s" })).role(Role::Text2),
                    spacer(),
                    text(format!("+{added}")).mono().role(Role::Success),
                    text(format!("−{removed}")).mono().role(Role::Danger),
                ]
                .spacing(8.0),
            )
            .padding([6.0, 12.0])
            .width(Fill);
            let tree = tree_list(app, snapshot);
            split(column![summary, tree].width(Fill).height(Fill), diff_view(app)).vertical().ratio(0.42).min_sizes(120.0, 120.0).into()
        }
    };
    column![header, body].width(Fill).height(Fill).into()
}

fn tree_list(app: &App, snapshot: &crate::workspace::WorkspaceSnapshot) -> Element<Msg> {
    let session = app.session();
    let collapsed = session.change_tree_collapsed.borrow().clone();
    let rows = Rc::new(change_tree::tree_rows(&snapshot.changes, &collapsed));
    let changes = Rc::new(snapshot.changes.clone());
    let root = snapshot.repository_root.clone().unwrap_or_else(|| snapshot.project.clone());
    let selected_path = session.selected_change.borrow().clone();
    let selected = rows.iter().position(|row| match row {
        TreeRow::File { change, .. } => Some(&changes[*change].path) == selected_path.as_ref(),
        TreeRow::Directory { .. } => false,
    });
    let (build_rows, build_changes) = (rows.clone(), changes.clone());
    let (select_rows, select_changes) = (rows.clone(), changes.clone());
    let (menu_rows, menu_changes, menu_root) = (rows.clone(), changes.clone(), root.clone());
    let editor = format!("Open in {}", launcher_name(app, LauncherKind::Editor));
    virtual_list(rows.len(), 24.0, move |i| change_row(&build_rows[i], &build_changes))
        .selected(selected)
        .on_select(move |picked: Vec<usize>| {
            match picked.first().and_then(|i| select_rows.get(*i)) {
                Some(TreeRow::Directory { path, .. }) => Msg::ToggleChangeDir(path.clone()),
                Some(TreeRow::File { change, .. }) => Msg::SelectChange(select_changes[*change].path.clone()),
                None => Msg::ClearNote,
            }
        })
        .on_activate({
            let (rows, changes, root) = (rows.clone(), changes.clone(), root.clone());
            move |i| match rows.get(i) {
                Some(TreeRow::File { change, .. }) => Msg::OpenInEditor(root.join(&changes[*change].path)),
                Some(TreeRow::Directory { path, .. }) => Msg::ToggleChangeDir(path.clone()),
                None => Msg::ClearNote,
            }
        })
        .context_menu(move |i| {
            let relative = match menu_rows.get(i) {
                Some(TreeRow::File { change, .. }) => menu_changes[*change].path.clone(),
                Some(TreeRow::Directory { path, .. }) => path.clone(),
                None => std::path::PathBuf::new(),
            };
            let (path, folder) = match menu_rows.get(i) {
                Some(TreeRow::File { change, .. }) => {
                    let path = menu_root.join(&menu_changes[*change].path);
                    let folder = path.parent().map_or_else(|| menu_root.clone(), std::path::Path::to_path_buf);
                    (path, folder)
                }
                Some(TreeRow::Directory { path, .. }) => (menu_root.join(path), menu_root.join(path)),
                None => (menu_root.clone(), menu_root.clone()),
            };
            Menu::new("")
                .item(MenuItem::new(editor.clone(), Msg::OpenInEditor(path.clone())))
                .item(MenuItem::new("Open Terminal Here", Msg::OpenTerminal(folder.clone())))
                .item(MenuItem::new("Show in Files", Msg::ShowInFiles(folder.clone())))
                .separator()
                .item(MenuItem::new("Copy Path", Msg::Copy(path.display().to_string())))
                .item(MenuItem::new("Copy Relative Path", Msg::Copy(relative.display().to_string())))
        })
        .width(Fill)
        .height(Fill)
        .into()
}

fn launcher_name(app: &App, kind: LauncherKind) -> String {
    app.launchers.of(kind).first().map_or_else(|| "Editor".to_owned(), |l| l.name.to_owned())
}

fn change_row(row: &TreeRow, changes: &[crate::workspace::WorkspaceChange]) -> Element<Msg> {
    match row {
        TreeRow::Directory { name, depth, collapsed, files, .. } => aizome_kit::row![
            space(*depth as f64 * 14.0),
            icon(if *collapsed { Icon::ChevronRight } else { Icon::ChevronDown }).size(12.0).role(Role::Text3),
            icon(Icon::Folder).size(14.0).role(Role::Text3),
            text(name.clone()).role(Role::Text2),
            spacer(),
            text(if *collapsed { format!("{files}") } else { String::new() }).mono().scale(0.85).role(Role::Text3),
        ]
        .spacing(6.0)
        .padding([0.0, 10.0])
        .into(),
        TreeRow::File { change, name, depth } => {
            let change = &changes[*change];
            let (letter, role) = match change.kind() {
                "New" => ("?", Role::Success),
                "Added" => ("A", Role::Success),
                "Deleted" => ("D", Role::Danger),
                "Renamed" => ("R", Role::Cyan),
                "Conflict" => ("U", Role::Danger),
                _ => ("M", Role::Warning),
            };
            let counts = match (change.insertions, change.deletions) {
                (Some(a), Some(d)) => format!("+{a} −{d}"),
                _ => String::new(),
            };
            aizome_kit::row![
                space(*depth as f64 * 14.0 + 18.0),
                text(letter).mono().semibold().role(role).tooltip(change.kind()),
                text(name.clone()),
                spacer(),
                text(counts).mono().scale(0.85).role(Role::Text3),
            ]
            .spacing(6.0)
            .padding([0.0, 10.0])
            .into()
        }
    }
}

fn diff_view(app: &App) -> Element<Msg> {
    let session = app.session();
    let Some(view) = app.view_of(session.key) else { return space(0.0) };
    match &view.diff {
        None => container(text("Select a file to see its changes").role(Role::Text3)).center().width(Fill).height(Fill).into(),
        Some((_, Diff::Loading)) => container(text("Reading the diff…").role(Role::Text3)).center().width(Fill).height(Fill).into(),
        Some((_, Diff::Failed(error))) => container(text(error.clone()).role(Role::Danger).wrap()).padding(12.0).width(Fill).into(),
        Some((path, Diff::Ready(lines))) => {
            let mut spans = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::new("\n"));
                }
                for run in line {
                    let s = Span::new(run.text.clone());
                    spans.push(match run.role {
                        Some(role) => s.role(role),
                        None => s,
                    });
                }
            }
            let title = container(
                row![
                    icon(Icon::FileDiff).size(14.0).role(Role::Text3),
                    text(path.display().to_string()).mono().scale(0.9).role(Role::Text2),
                ]
                .spacing(6.0),
            )
            .padding([6.0, 10.0])
            .edges(Edge::Line, Sides::BOTTOM)
            .width(Fill);
            let code = rich_text(spans).font(FontSpec::mono(12.5)).line_height(1.35).selectable();
            column![
                title,
                container(scroll(container(code).padding([8.0, 12.0])).both()).fill(Tone::Sunk).width(Fill).height(Fill),
            ]
            .width(Fill)
            .height(Fill)
            .into()
        }
    }
}
