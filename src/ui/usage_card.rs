//! The `/usage` card in the transcript: the session's tokens, cache, time
//! and sub-agents, and the account's plan and limits.

use aizome_kit::prelude::*;
use aizome_kit::widgets::layout::{column as column_of, row as row_of};
use aizome_kit::{column, row};

use crate::app::Msg;
use crate::usage_card::{Body, Item, Tone as BarTone, items, plain_text};

fn bar_role(tone: BarTone) -> Role {
    match tone {
        BarTone::Accent => Role::Accent,
        BarTone::Agent => Role::Violet,
        BarTone::Danger => Role::Danger,
    }
}

fn pair(label: &str, value: &str) -> Element<Msg> {
    row![text(label.to_owned()).role(Role::Text2).width(Fill), text(value.to_owned()).mono().tabular()].spacing(10.0).width(Fill).into()
}

pub(super) fn card(body: &Body) -> Element<Msg> {
    let mut parts: Vec<Element<Msg>> = vec![
        row![
            icon(Icon::ChartBar).size(15.0).role(Role::Accent),
            text("Usage").scale(1.1).weight(700.0),
            spacer(),
            icon_button(Icon::Copy).on_press(Msg::Copy(plain_text(body))).tooltip("Copy the card as text"),
        ]
        .spacing(8.0)
        .into(),
    ];
    for item in items(body) {
        parts.push(match item {
            Item::Loading(text_) => row![icon(Icon::LoaderCircle).size(14.0).role(Role::Accent), text(text_).role(Role::Text3)].spacing(8.0).into(),
            Item::Caption(text_) => text(text_).scale(0.9).role(Role::Text3).wrap().into(),
            Item::Section { title, aside, refreshing } => {
                let mut line = vec![text(title).semibold().width(Fill).into()];
                if refreshing {
                    line.push(icon(Icon::LoaderCircle).size(12.0).role(Role::Accent).into());
                }
                if let Some(aside) = aside {
                    line.push(text(aside).scale(0.85).role(Role::Text3).into());
                }
                container(row_of(line).spacing(6.0).width(Fill)).padding([8.0, 0.0, 0.0, 0.0]).width(Fill).into()
            }
            Item::Group(name) => text(name).mono().scale(0.78).letter_spacing(1.0).role(Role::Text3).into(),
            Item::Account { name, plan } => row![
                text(name).semibold().width(Fill),
                text(plan.unwrap_or_default()).role(Role::Text2),
            ]
            .spacing(8.0)
            .width(Fill)
            .into(),
            Item::Error(error) => row![
                text(error).role(Role::Danger).wrap().width(Fill),
                button("Retry").compact().on_press(Msg::RefreshUsage),
            ]
            .spacing(8.0)
            .width(Fill)
            .into(),
            Item::Window { title, resets, figure, fraction, tone, compact } => {
                let line = row![
                    text(title).role(Role::Text2).width(Fill),
                    text(resets).scale(0.85).role(Role::Text3),
                    text(figure).mono().tabular(),
                ]
                .spacing(10.0)
                .width(Fill);
                match fraction {
                    Some(fraction) if !compact => column![line, meter(vec![fraction as f32]).role(bar_role(tone)).width(Fill)].spacing(3.0).width(Fill).into(),
                    _ => line.into(),
                }
            }
            Item::Chips(pairs) => row_of(
                pairs
                    .into_iter()
                    .enumerate()
                    .map(|(i, (label, value))| {
                        container(
                            column![
                                text(value).mono().tabular().scale(1.25).weight(if i == 0 { 700.0 } else { 600.0 }).role(if i == 0 { Role::Accent } else { Role::Text }),
                                text(label).scale(0.8).role(Role::Text3),
                            ]
                            .spacing(2.0),
                        )
                        .fill(Tone::Sunk)
                        .border(Edge::Line)
                        .padding([8.0, 10.0])
                        .width(Fill)
                        .into()
                    })
                    .collect(),
            )
            .spacing(8.0)
            .width(Fill)
            .into(),
            Item::Cells(pairs) => column_of(pairs.iter().map(|(label, value)| pair(label, value)).collect()).spacing(3.0).width(Fill).into(),
            Item::Breakdown { model, rows } => {
                let mut lines: Vec<Element<Msg>> =
                    vec![row![text("Breakdown").semibold().width(Fill), text(model).mono().scale(0.85).role(Role::Text3)].spacing(8.0).into()];
                lines.extend(rows.iter().map(|(label, value)| pair(label, value)));
                container(column_of(lines).spacing(3.0).width(Fill)).padding([6.0, 0.0, 0.0, 0.0]).width(Fill).into()
            }
            Item::Share { name, fraction, tone } => row![
                text(name).role(Role::Text2).width(Fill),
                meter(vec![fraction as f32]).role(bar_role(tone)).width(120.0),
                text(format!("{:.0}%", fraction * 100.0)).mono().tabular().width(44.0),
            ]
            .spacing(10.0)
            .width(Fill)
            .into(),
            Item::Speed { latest, peak, rates } => {
                let peak_rate = rates.iter().copied().fold(1.0_f64, f64::max);
                column![
                    row![text("Output speed").semibold().width(Fill), text(format!("{latest:.0} tok/s · peak {peak:.0}")).mono().tabular()].spacing(8.0),
                    meter(rates.iter().map(|r| (r / peak_rate) as f32).collect::<Vec<f32>>()).role(Role::Accent).width(Fill),
                ]
                .spacing(4.0)
                .width(Fill)
                .into()
            }
            Item::Tip(tip) => row![icon(Icon::Info).size(14.0).role(Role::Cyan), text(tip).role(Role::Text2).wrap()].spacing(8.0).align(Align::Start).into(),
        });
    }
    container(column_of(parts).spacing(6.0).width(Fill)).fill(Tone::Chrome).border(Edge::Line).padding([10.0, 14.0, 12.0, 14.0]).width(Fill).into()
}
