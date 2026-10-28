//! The Usage window: a session's tokens and requests from its receipts,
//! the input split into what was new, read from the cache and written to
//! it, and three pages of detail (an overview, the recent requests, and
//! what the cache keeps and why).

use std::sync::Arc;

use aizome_kit::prelude::*;
use aizome_kit::widgets::layout::column as column_of;
use aizome_kit::{column, row};

use crate::app::{App, Msg};
use crate::usage::{Report, Section};

pub(super) fn window(app: &App) -> Element<Msg> {
    let Some(window) = &app.usage_window else { return space(0.0) };
    let header = container(
        row![
            icon(Icon::ChartBar).size(18.0).role(Role::Accent),
            column![
                text(window.title.clone()).semibold().max_lines(1),
                text(window.report.as_ref().map_or_else(String::new, |r| r.scope.clone())).scale(0.88).role(Role::Text3).max_lines(1),
            ]
            .spacing(2.0)
            .width(Fill),
            if window.loading {
                Element::from(icon(Icon::LoaderCircle).size(15.0).role(Role::Accent).tooltip("Reading the receipts…"))
            } else {
                space(0.0)
            },
            button("Refresh").icon(Icon::RefreshCw).compact().enabled(!window.loading).on_press(Msg::RefreshUsageReport),
        ]
        .spacing(10.0),
    )
    .padding([10.0, 14.0])
    .width(Fill);
    let body: Element<Msg> = match &window.report {
        None => super::empty_state(Icon::LoaderCircle, Role::Accent, "Reading the receipts…", ""),
        Some(report) => report_view(report, window.page),
    };
    column![header, divider(), body].width(Fill).height(Fill).into()
}

fn report_view(report: &Arc<Report>, page: usize) -> Element<Msg> {
    let figures = aizome_kit::widgets::layout::row(
        report
            .metrics
            .iter()
            .map(|metric| {
                container(
                    column![
                        text(metric.title).scale(0.85).role(Role::Text3),
                        text(metric.value.clone()).scale(1.45).weight(700.0).mono().tabular(),
                        text(metric.caption.clone()).scale(0.85).role(Role::Text3).max_lines(1),
                    ]
                    .spacing(2.0),
                )
                .padding([10.0, 12.0])
                .fill(Tone::Chrome)
                .border(Edge::Line)
                .width(Fill)
                .tooltip(metric.tooltip.clone())
                .into()
            })
            .collect(),
    )
    .spacing(10.0)
    .width(Fill);
    let [fresh, read, write] = report.composition;
    let parts = [("New input", fresh, Role::Text3), ("Read from the cache", read, Role::Success), ("Written to the cache", write, Role::Warning)];
    let total = (fresh + read + write).max(1);
    let bar = canvas(parts.map(|p| (p.1, p.2)), move |c, bounds, parts, theme| {
        let mut x = bounds.x0;
        for (tokens, role) in parts {
            let width = bounds.width() * *tokens as f64 / total as f64;
            c.fill_rect(Rect::new(x, bounds.y0, x + width, bounds.y1), theme.role(*role));
            x += width;
        }
    })
    .height(10.0)
    .width(Fill);
    let legend = aizome_kit::widgets::layout::row(
        parts
            .iter()
            .map(|(label, tokens, role)| {
                row![
                    super::swatch(*role),
                    text(format!("{label} {:.0}%", *tokens as f64 * 100.0 / total as f64)).scale(0.88).role(Role::Text2),
                ]
                .spacing(6.0)
                .into()
            })
            .collect(),
    )
    .spacing(16.0);
    let sections = &report.pages[page.min(2)];
    let mut parts: Vec<Element<Msg>> = vec![
        figures.into(),
        column![aizome_kit::caption("Input tokens"), bar, legend].spacing(6.0).width(Fill).into(),
        container(segmented(["Overview", "Requests", "Cache policy"], page.min(2)).on_select(Msg::UsagePage)).align_x(Align::Center).width(Fill).into(),
    ];
    parts.extend(sections.iter().map(section));
    scroll(container(column_of(parts).spacing(16.0).max_width(860.0).width(Fill)).align_x(Align::Center).padding([14.0, 18.0, 20.0, 18.0]).width(Fill))
        .key(page)
        .into()
}

/// One section: what it says, then its table, a column set at the right
/// when every value in it is a figure.
fn section(section: &Section) -> Element<Msg> {
    let mut rows: Vec<Element<Msg>> = Vec::new();
    if !section.note.is_empty() {
        rows.push(container(text(section.note.clone()).role(Role::Text3).wrap()).padding([8.0, 12.0, 4.0, 12.0]).width(Fill).into());
    }
    if !section.columns.is_empty() {
        let numeric = |column: usize| {
            column > 0
                && section.rows.iter().all(|row| {
                    row.get(column).is_none_or(|cell| {
                        cell.is_empty() || cell.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '$' | '—' | '-' | '<' | '~'))
                    })
                })
        };
        let columns: Vec<(&str, f64, bool)> = section
            .columns
            .iter()
            .enumerate()
            .map(|(i, title)| (title.as_str(), section.weights.get(i).copied().unwrap_or(1.0), numeric(i)))
            .collect();
        rows.push(super::grid(&columns, section.rows.clone()));
    }
    form_section(section.title.clone(), rows)
}
