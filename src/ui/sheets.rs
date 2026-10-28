//! Sheets: signing in to a provider, and the short questions (a name, a
//! branch, which folder, which account).

use aizome_kit::prelude::*;
use aizome_kit::widgets::layout::{column as column_of, row as row_of};
use aizome_kit::{column, row};

use crate::app::{App, Choice, Msg, Prompt};
use crate::omp_runtime::AuthMethod;

const WIDTH: f64 = 460.0;

fn frame(content: Element<Msg>) -> Element<Msg> {
    container(content).padding([20.0, 22.0, 18.0, 22.0]).width(WIDTH).into()
}

pub(super) fn sign_in(app: &App) -> Element<Msg> {
    let Some(sign_in) = &app.sign_in else {
        return frame(text("Signing in…").into());
    };
    let provider = sign_in.provider;
    let methods = provider.methods();
    let mut parts: Vec<Element<Msg>> = vec![
        row![icon(Icon::KeyRound).size(20.0).role(Role::Accent), text(format!("Sign in to {}", provider.product_name())).scale(1.3).weight(700.0)]
            .spacing(10.0)
            .into(),
    ];
    if methods.len() > 1 {
        let selected = methods.iter().position(|m| *m == sign_in.method).unwrap_or(0);
        let shown = methods.to_vec();
        parts.push(segmented(methods.iter().map(|m| m.label()), selected).on_select(move |i| Msg::SignInMethod(shown[i])).into());
    }
    match sign_in.method {
        AuthMethod::Browser => {
            parts.push(text("Omperor opens the provider's sign-in page in your browser. Finish there, and this closes by itself.").wrap().role(Role::Text2).into());
            if let Some((code, url)) = &sign_in.device {
                parts.push(
                    container(
                        column![
                            text("Enter this code on the page:").role(Role::Text3),
                            row![text(code.clone()).mono().scale(1.6).weight(700.0).selectable(), spacer(), button("Copy").compact().on_press(Msg::SignInCopyCode)]
                                .spacing(8.0),
                            text(url.clone()).mono().scale(0.85).role(Role::Text3).selectable(),
                        ]
                        .spacing(6.0),
                    )
                    .fill(Tone::Sunk)
                    .border(Edge::Line)
                    .padding(12.0)
                    .width(Fill)
                    .into(),
                );
            }
            if sign_in.url.is_some() {
                parts.push(button("Open the Sign-In Page Again").icon(Icon::ExternalLink).on_press(Msg::SignInOpenPage).into());
            }
        }
        AuthMethod::ApiKey | AuthMethod::Token => {
            let what = if sign_in.method == AuthMethod::ApiKey { "API key" } else { "access token" };
            parts.push(text(format!("Paste the {what}. It is kept by OMP in its credential store, not in Omperor.")).wrap().role(Role::Text2).into());
        }
    }
    if let Some(prompt) = &sign_in.prompt {
        parts.push(text(prompt.message.clone()).wrap().into());
        let field = text_field(&sign_in.secret)
            .placeholder(match sign_in.method {
                AuthMethod::ApiKey => "API key",
                AuthMethod::Token => "Access token",
                AuthMethod::Browser => "Answer",
            })
            .on_change(Msg::SignInSecret)
            .on_submit(Msg::SignInSubmit)
            .width(Fill)
            .id("sign-in-secret");
        let field = if sign_in.method == AuthMethod::Browser { field } else { field.password() };
        parts.push(field.into());
    }
    if let Some(failure) = &sign_in.failure {
        parts.push(row![icon(Icon::CircleAlert).size(15.0).role(Role::Danger), text(failure.clone()).role(Role::Danger).wrap()].spacing(8.0).align(Align::Start).into());
    } else if let Some(waiting) = &sign_in.waiting {
        parts.push(row![progress_indeterminate().width(48.0), text(waiting.clone()).role(Role::Text3).wrap()].spacing(10.0).into());
    }
    let mut buttons: Vec<Element<Msg>> = vec![spacer().into(), button("Cancel").cancel().on_press(Msg::SignInCancel).into()];
    if sign_in.failure.is_some() {
        buttons.push(button("Try Again").default().on_press(Msg::SignInMethod(sign_in.method)).into());
    } else if sign_in.prompt.is_some() {
        buttons.push(
            button(if sign_in.submitting { "Connecting…" } else { "Connect" })
                .default()
                .enabled(!sign_in.submitting && !sign_in.secret.trim().is_empty())
                .on_press(Msg::SignInSubmit)
                .into(),
        );
    }
    parts.push(row_of(buttons).spacing(8.0).width(Fill).into());
    frame(column_of(parts).spacing(14.0).width(Fill).into())
}

pub(super) fn prompt(prompt: &Prompt) -> Element<Msg> {
    let mut parts: Vec<Element<Msg>> = vec![
        text(prompt.title.clone()).scale(1.25).weight(700.0).into(),
        text(prompt.message.clone()).wrap().role(Role::Text2).into(),
        text_field(&prompt.value)
            .on_change(Msg::SheetValue)
            .on_submit(Msg::SheetConfirm)
            .error(prompt.error.clone())
            .width(Fill)
            .id("sheet-field")
            .into(),
    ];
    parts.push(
        row![
            spacer(),
            button("Cancel").cancel().on_press(Msg::SheetCancel),
            button(prompt.confirm.clone()).default().on_press(Msg::SheetConfirm),
        ]
        .spacing(8.0)
        .width(Fill)
        .into(),
    );
    frame(column_of(parts).spacing(12.0).width(Fill).into())
}

pub(super) fn choice(choice: &Choice) -> Element<Msg> {
    let options = choice
        .options
        .iter()
        .enumerate()
        .map(|(i, option)| radio(option.clone(), i, Some(choice.selected)).on_select(Msg::SheetChoose).into())
        .collect();
    frame(
        column![
            text(choice.title.clone()).scale(1.25).weight(700.0),
            text(choice.message.clone()).wrap().role(Role::Text2),
            column_of(options).spacing(8.0),
            row![
                spacer(),
                button("Cancel").cancel().on_press(Msg::SheetCancel),
                button(choice.confirm.clone()).default().on_press(Msg::SheetConfirm),
            ]
            .spacing(8.0)
            .width(Fill),
        ]
        .spacing(12.0)
        .width(Fill)
        .into(),
    )
}
