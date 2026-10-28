//! The composer: what to send next, with its attachments, the `@` and `/`
//! pickers above it, and the choices that go with it: model, reasoning,
//! account, Fast; Send (or Stop while the agent works).

use aizome_kit::prelude::*;
use aizome_kit::sidestep_ui::{Key, KeyEvent, NamedKey};
use aizome_kit::widgets::layout::{column as column_of, row as row_of};
use aizome_kit::{column, row};

use crate::app::{App, Msg, PickerKind};
use crate::omp_runtime::AuthProvider;
use crate::rules;

/// The reasoning levels OMP takes.
pub(super) const THINKING: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];

pub(super) fn view(app: &App) -> Element<Msg> {
    let session = app.session();
    let Some(view) = app.view_of(session.key) else { return space(0.0) };
    let mut parts: Vec<Element<Msg>> = Vec::new();
    if let Some(picker) = &view.picker {
        parts.push(picker_view(picker).key("picker"));
    }
    let attachments = session.pending_attachments.borrow();
    if !attachments.is_empty() {
        let chips = attachments
            .items
            .iter()
            .map(|item| {
                let state: Element<Msg> = match &item.result {
                    None => icon(Icon::LoaderCircle).size(13.0).role(Role::Accent).into(),
                    Some(Err(error)) => icon(Icon::CircleAlert).size(13.0).role(Role::Danger).tooltip(error.clone()).into(),
                    Some(Ok(_)) => match &item.thumbnail {
                        Some(picture) => image((**picture).clone()).fit(Fit::Cover).width(28.0).height(28.0).into(),
                        None => icon(Icon::File).size(14.0).role(Role::Text2).into(),
                    },
                };
                container(
                    row![
                        state,
                        text(item.name.clone()).scale(0.9).max_width(180.0),
                        icon_button(Icon::X).on_press(Msg::RemoveAttachment(item.id)).tooltip("Remove"),
                    ]
                    .spacing(6.0),
                )
                .fill(Tone::Raised)
                .border(Edge::Line)
                .padding([2.0, 2.0, 2.0, 8.0])
                .key(("attachment", item.id))
                .into()
            })
            .collect();
        parts.push(scroll(row_of(chips).spacing(6.0).padding([0.0, 0.0, 6.0, 0.0])).horizontal().height(Shrink).key("attachments").into());
    }
    drop(attachments);
    let read_only = session.read_only_session.get();
    let picker_open = view.picker.is_some();
    // Where the caret is decides what ↑ and ↓ do: at the top and foot they
    // walk the prompts sent before.
    let selection = view.editor.selection();
    let rope = view.editor.rope();
    let caret_line = rope.char_to_line(selection.head.min(rope.len_chars()));
    let at_top = selection.is_empty() && caret_line == 0;
    let at_foot = selection.is_empty() && caret_line + 1 >= rope.len_lines();
    let recalling = session.prompt_history.borrow().navigating();
    let busy = session.chat_busy.get();
    let placeholder = if read_only {
        "This session is read-only: OMP's terminal made it. Start a new session to write."
    } else if session.chat_busy.get() {
        "Steer the agent: what you send now guides the turn under way"
    } else {
        "Write to the agent… @ mentions a file, / runs a command"
    };
    let editor = text_editor(&view.editor)
        .mono(false)
        .wrap(true)
        .current_line(false)
        .line_height(1.5)
        .padding(12.0)
        .fit_lines(2, 12)
        .placeholder(placeholder)
        .on_action(Msg::Edit)
        .on_key(move |key: &KeyEvent| composer_key(key, picker_open, busy, at_top, at_foot, recalling))
        .on_paste(|types: &[String]| {
            types
                .iter()
                .any(|t| t.starts_with("image/") || t == "text/uri-list")
                .then(|| Msg::PasteData(types.to_vec()))
        })
        .height(Shrink)
        .id("composer");
    let editor = if read_only { editor.read_only() } else { editor };
    parts.push(editor.key("editor").into());
    parts.push(controls(app).key("controls"));
    let boxed = container(column_of(parts).width(Fill)).fill(Tone::Raised).border(Edge::Line).padding([2.0, 2.0, 6.0, 2.0]);
    container(boxed.max_width(super::transcript::READING + 24.0).width(Fill))
        .align_x(Align::Center)
        .padding([0.0, 16.0, 14.0, 16.0])
        .width(Fill)
        .into()
}

/// What a key in the composer does before the editor sees it.
fn composer_key(key: &KeyEvent, picker: bool, busy: bool, at_top: bool, at_foot: bool, recalling: bool) -> Option<Msg> {
    let m = key.modifiers;
    if !key.pressed {
        return None;
    }
    match &key.key {
        Key::Named(NamedKey::Enter) if picker && !m.shift() => Some(Msg::PickerChoose(None)),
        Key::Named(NamedKey::Tab) if picker => Some(Msg::PickerChoose(None)),
        Key::Named(NamedKey::Enter) if !m.shift() && !m.alt() && !m.control() => Some(Msg::Send),
        Key::Named(NamedKey::Escape) if picker => Some(Msg::PickerDismiss),
        Key::Named(NamedKey::Escape) if busy => Some(Msg::Stop),
        Key::Named(NamedKey::ArrowUp) if picker => Some(Msg::PickerMove(-1)),
        Key::Named(NamedKey::ArrowDown) if picker => Some(Msg::PickerMove(1)),
        Key::Named(NamedKey::ArrowUp) if at_top && !m.shift() && !m.super_key() => Some(Msg::HistoryBack),
        Key::Named(NamedKey::ArrowDown) if at_foot && recalling && !m.shift() && !m.super_key() => Some(Msg::HistoryForward),
        _ => None,
    }
}

fn picker_view(picker: &crate::app::Picker) -> Element<Msg> {
    let rows: Vec<Element<Msg>> = picker
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let selected = index == picker.selected;
            let glyph = match picker.kind {
                PickerKind::File => Icon::FileText,
                PickerKind::Command => Icon::SquareTerminal,
            };
            let line = row![
                icon(glyph).size(14.0).role(if selected { Role::Accent } else { Role::Text3 }),
                text(item.label.clone()).mono().scale(0.95),
                text(item.detail.clone()).scale(0.88).role(Role::Text3).width(Fill),
            ]
            .spacing(8.0)
            .padding([4.0, 10.0])
            .width(Fill);
            let line = container(line).fill(if selected { Tone::Selection } else { Tone::Raised }).width(Fill);
            mouse_area(line).on_press(Msg::PickerChoose(Some(index))).key(index).into()
        })
        .collect();
    let caption = match picker.kind {
        PickerKind::File => "Files",
        PickerKind::Command => "Commands",
    };
    container(column![aizome_kit::caption(caption), column_of(rows).width(Fill)].width(Fill))
        .edges(Edge::Line, Sides::BOTTOM)
        .padding([0.0, 0.0, 4.0, 0.0])
        .width(Fill)
        .into()
}

fn controls(app: &App) -> Element<Msg> {
    let session = app.session();
    let busy = session.chat_busy.get();
    let mut items: Vec<Element<Msg>> = vec![
        icon_button(Icon::Paperclip).on_press(Msg::Attach).tooltip("Attach files (❖O), or paste a picture").enabled(!session.read_only_session.get()).into(),
    ];
    // The model: the supported models of the accounts signed in.
    let models = session.available_models.borrow().clone();
    let current = session.current_model.borrow().clone();
    let authenticated = session.authenticated_providers.borrow().clone();
    let options: Vec<String> = rules::model_picker_options(&models, &current, &authenticated).into_iter().map(str::to_owned).collect();
    if options.is_empty() {
        let label = if authenticated.is_empty() { "Sign in to choose a model" } else { "No models yet" };
        let provider = AuthProvider::for_model(&current).unwrap_or(AuthProvider::Anthropic);
        items.push(button(label).compact().icon(Icon::KeyRound).on_press(Msg::SignIn(provider)).into());
    } else {
        let selected = options.iter().position(|m| *m == current);
        let shown = options.clone();
        items.push(
            dropdown(options.iter().map(|m| crate::agents::short_model(m)), selected)
                .placeholder(if current.is_empty() { "Model".to_owned() } else { crate::agents::short_model(&current) })
                .on_select(move |i| Msg::PickModel(shown[i].clone()))
                .compact()
                .tooltip("The model")
                .into(),
        );
    }
    let level = session.current_thinking_level.borrow().clone();
    items.push(
        dropdown(THINKING.iter().map(|l| format!("Reasoning: {l}")), THINKING.iter().position(|l| *l == level))
            .placeholder("Reasoning")
            .on_select(|i| Msg::PickThinking(THINKING[i].to_owned()))
            .compact()
            .tooltip("How much the model reasons before it answers")
            .into(),
    );
    // The account, when the provider has several.
    if let Some(provider) = AuthProvider::for_model(&current) {
        let accounts: Vec<_> = app.accounts.iter().filter(|a| a.provider() == provider && a.is_usable()).collect();
        if accounts.len() > 1 {
            let ids: Vec<String> = accounts.iter().map(|a| a.id().to_owned()).collect();
            let selected = session.current_account.borrow().as_deref().and_then(|id| ids.iter().position(|i| i == id));
            items.push(
                dropdown(accounts.iter().map(|a| a.display_name()), selected)
                    .placeholder("Choose account")
                    .on_select(move |i| Msg::PickAccount(ids[i].clone()))
                    .compact()
                    .tooltip("The account this session uses")
                    .into(),
            );
        }
    }
    if session.fast_models.borrow().contains(&current) {
        let on = session.fast_mode.get();
        items.push(
            button("Fast")
                .icon(Icon::Zap)
                .toolbar()
                .selected(on)
                .on_press(Msg::ToggleFast(!on))
                .tooltip(if on {
                    "Fast is on: OpenAI priority processing. Quicker replies; each turn uses more of your plan."
                } else {
                    "Turn Fast on: OpenAI priority processing. Quicker replies; each turn uses more of your plan."
                })
                .into(),
        );
    }
    items.push(spacer().into());
    if let Some(rate) = app.view_of(session.key).and_then(|v| v.rates.back().copied()).filter(|r| busy && *r >= 1.0) {
        items.push(text(format!("{rate:.0} tok/s")).mono().tabular().scale(0.85).role(Role::Text3).into());
    }
    if busy {
        items.push(button("Stop").icon(Icon::CircleStop).compact().on_press(Msg::Stop).tooltip("Stop the turn (❖.)").into());
    }
    let signed_out = session.signed_out();
    items.push(
        button(if busy { "Steer" } else { "Send" })
            .icon(Icon::ArrowUp)
            .compact()
            .primary()
            .enabled(!session.read_only_session.get() && !signed_out)
            .on_press(Msg::Send)
            .tooltip(if signed_out { "Sign in to an account first" } else { "Send (Return); a new line is ⇧Return" })
            .into(),
    );
    row_of(items).spacing(6.0).padding([0.0, 8.0, 0.0, 6.0]).width(Fill).into()
}
