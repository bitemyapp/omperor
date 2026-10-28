//! A parsed answer as kit elements: paragraphs as selectable rich text,
//! with formulas set in their lines; code in a sunken box with its
//! language and a copy button; formulas set on their own, centred;
//! diagrams in a box of their own; quotes and callouts edged, tables
//! lined, lists with their markers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use aizome_kit::prelude::*;
use aizome_kit::widgets::layout::{Container, column as column_of, row as row_of};
use aizome_kit::sidestep_ui::Cursor;
use aizome_kit::{column, row};

use super::diagram::Diagram;
use super::math::Formula;
use super::picture::{Picture, Source};
use super::{Align as CellAlign, Block, Callout, Doc, Item, Run};

/// How a document is drawn, and what its links and buttons send.
pub(crate) struct Look<Msg> {
    /// The body text's size, as a multiple of the theme's.
    pub(crate) scale: f64,
    pub(crate) open: Rc<dyn Fn(String) -> Msg>,
    pub(crate) copy: Rc<dyn Fn(String) -> Msg>,
    /// What its images show, where answers show pictures: none where they
    /// stay links.
    pub(crate) pictures: Option<Pictures<Msg>>,
    /// The text's colour: `Text` for an answer, quieter for reasoning.
    pub(crate) role: Role,
}

impl<Msg> Clone for Look<Msg> {
    fn clone(&self) -> Self {
        Look {
            scale: self.scale,
            open: self.open.clone(),
            copy: self.copy.clone(),
            pictures: self.pictures.clone(),
            role: self.role,
        }
    }
}

/// The pictures a document's images show.
pub(crate) struct Pictures<Msg> {
    /// As far as each has got, by where it comes from ([`Source::key`]).
    pub(crate) read: Rc<HashMap<u64, Picture>>,
    /// Where relative paths start: the session's folder.
    pub(crate) base: Option<PathBuf>,
    /// Read one on the web after all.
    pub(crate) load: Rc<dyn Fn(Source) -> Msg>,
    /// Open one in the desktop's viewer, by its link.
    pub(crate) show: Rc<dyn Fn(String) -> Msg>,
}

impl<Msg> Clone for Pictures<Msg> {
    fn clone(&self) -> Self {
        Pictures { read: self.read.clone(), base: self.base.clone(), load: self.load.clone(), show: self.show.clone() }
    }
}

const LINE_HEIGHT: f64 = 1.45;
/// Room for a list item's marker.
const MARKER: f64 = 22.0;
/// The widest a note about a picture not shown yet is.
const IMAGE_NOTE: f64 = 560.0;
/// Math's size against the text's, as KaTeX sets it: its fonts' small
/// letters are that much shorter than most text's.
const MATH_SCALE: f64 = 1.21;

/// A formula as something drawn in a line of text, copied as its LaTeX
/// between the delimiters it came in: none if its LaTeX couldn't be set.
fn formula(formula: &Arc<Formula>) -> Option<Span> {
    let picture = formula.set.as_ref().ok()?;
    let copied =
        if formula.display { format!("$$\n{}\n$$", formula.source) } else { format!("${}$", formula.source) };
    let set = formula.clone();
    let inline = Inline::new(picture.width, picture.ascent, picture.descent, copied, formula.id, move |c, place| {
        if let Ok(p) = &set.set {
            p.paint(c, place.origin(p.ascent), place.em, place.color);
        }
    });
    Some(Span::inline(inline).scale(MATH_SCALE))
}

fn span(run: &Run) -> Span {
    if let Some(f) = &run.math {
        // LaTeX that couldn't be set shows as it was written.
        return formula(f).unwrap_or_else(|| Span::new(run.text.clone()).mono().role(Role::Danger).scale(0.92));
    }
    let mut s = Span::new(run.text.clone());
    if run.bold {
        s = s.weight(650.0);
    }
    if run.italic {
        s = s.italic();
    }
    if run.strike {
        s = s.strikethrough();
    }
    if run.code {
        s = s.mono().role(Role::Success).scale(0.92);
    }
    if let Some(role) = run.role {
        s = s.role(role);
    }
    if let Some(link) = &run.link {
        s = s.link(link.clone()).role(Role::Accent).underline();
    }
    s
}

fn paragraph<Msg: Clone + Send + 'static>(runs: &[Run], look: &Look<Msg>, scale: f64) -> Element<Msg> {
    let open = look.open.clone();
    rich_text(runs.iter().map(span))
        .role(look.role)
        .scale(look.scale * scale)
        .line_height(LINE_HEIGHT)
        .wrap()
        .on_link(move |target| open(target))
        .into()
}

/// A paragraph of plain text: what an answer shows until it is parsed.
pub(crate) fn plain<Msg: Clone + Send + 'static>(text: &str, look: &Look<Msg>) -> Element<Msg> {
    rich_text([Span::new(text.to_owned())]).role(look.role).scale(look.scale).line_height(LINE_HEIGHT).wrap().selectable().into()
}

/// The document.
pub(crate) fn doc<Msg: Clone + Send + 'static>(doc: &Doc, look: &Look<Msg>) -> Element<Msg> {
    blocks(&doc.blocks, look)
}

fn blocks<Msg: Clone + Send + 'static>(blocks: &[Block], look: &Look<Msg>) -> Element<Msg> {
    column_of(blocks.iter().map(|b| block(b, look)).collect()).spacing(10.0).width(Fill).into()
}

fn block<Msg: Clone + Send + 'static>(block: &Block, look: &Look<Msg>) -> Element<Msg> {
    match block {
        Block::Paragraph(runs) => paragraph(runs, look, 1.0),
        Block::Heading { level, runs } => {
            let (scale, weight, role) = match level {
                1 => (1.45, 700.0, Role::Text),
                2 => (1.25, 680.0, Role::Text),
                3 => (1.1, 650.0, Role::Text),
                _ => (1.0, 650.0, Role::Text2),
            };
            let open = look.open.clone();
            let heading = rich_text(runs.iter().map(|r| span(r).weight(weight)))
                .scale(look.scale * scale)
                .role(role)
                .wrap()
                .on_link(move |target| open(target));
            container(heading).padding([if *level <= 2 { 6.0 } else { 2.0 }, 0.0, 0.0, 0.0]).width(Fill).into()
        }
        Block::Code { lang, text, lines } => code_block(lang, text, lines, look),
        Block::Quote(inner) => {
            container(blocks(inner, look)).edges(Edge::Line2, Sides::LEFT).padding([2.0, 0.0, 2.0, 14.0]).width(Fill).into()
        }
        Block::Callout { kind, blocks: inner } => callout(*kind, inner, look),
        Block::List { start, items } => list(*start, items, look),
        Block::Table { aligns, head, rows } => table_block(aligns, head, rows, look),
        Block::Rule => container(divider()).padding([4.0, 0.0]).width(Fill).into(),
        Block::Math(f) => display_math(f, look),
        Block::Diagram(d) => diagram(d, look),
        Block::Image { url, alt } => picture(url, alt, look),
    }
}

/// An image: its picture, as large as it is, smaller in a narrower column,
/// opened in the desktop's viewer with a click. Until then, a note of how
/// it's going: loading, kept off the web by Settings (with a button to
/// load it anyway), or why it can't be shown. Where answers don't show
/// pictures, a link to it.
fn picture<Msg: Clone + Send + 'static>(url: &str, alt: &str, look: &Look<Msg>) -> Element<Msg> {
    let label = match alt.trim() {
        "" if url.starts_with("data:") => "A picture".to_owned(),
        "" => url.to_owned(),
        alt => alt.to_owned(),
    };
    let Some(pictures) = &look.pictures else {
        let open = look.open.clone();
        return row![
            icon(Icon::Image).size(16.0).role(Role::Text3),
            rich_text([Span::new(label).link(url.to_owned()).role(Role::Accent).underline()])
                .scale(look.scale)
                .wrap()
                .on_link(move |target| open(target)),
        ]
        .spacing(8.0)
        .into();
    };
    let source = Source::of(url, pictures.base.as_deref());
    let key = source.key();
    // As wide as it is (in points) or the column, whichever is less.
    let drawn: Element<Msg> = match pictures.read.get(&key) {
        Some(Picture::Shown(image)) => {
            let (shown, size) = (image.clone(), image.size());
            canvas(key, move |c, bounds, _, _| c.draw_image(&shown, bounds))
                .aspect(size.width / size.height.max(1.0))
                .max_width(size.width.max(1.0))
                .width(Shrink)
                .into()
        }
        Some(Picture::Drawn(svg)) => {
            let (drawn, size) = (svg.clone(), svg.size);
            canvas(key, move |c, bounds, _, _| drawn.paint(c, bounds.origin(), bounds.width() / size.width.max(1.0)))
                .aspect(size.width / size.height.max(1.0))
                .max_width(size.width.max(1.0))
                .width(Shrink)
                .into()
        }
        state => {
            let (glyph, role, note) = match state {
                Some(Picture::Blocked) => {
                    let host = url.split("//").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or(url);
                    let note = format!("From {host}. Settings keeps answers from loading pictures from the web.");
                    (Icon::Image, Role::Text3, note)
                }
                Some(Picture::Failed(why)) => (Icon::Image, Role::Warning, why.clone()),
                _ => (Icon::LoaderCircle, Role::Accent, "Loading…".to_owned()),
            };
            let load: Element<Msg> = match state {
                Some(Picture::Blocked) => button("Load").compact().on_press((pictures.load)(source)).into(),
                _ => space(0.0),
            };
            let words = column![text(label).max_lines(1), text(note).scale(0.88).role(Role::Text3).wrap()].spacing(2.0);
            return container(row![icon(glyph).size(16.0).role(role), words.width(Fill), load].spacing(10.0))
                .padding([8.0, 12.0])
                .fill(Tone::Chrome)
                .border(Edge::Line)
                .max_width(IMAGE_NOTE)
                .into();
        }
    };
    let framed = container(drawn).border(Edge::Line).padding(1.0).tooltip(label).id("picture");
    match source.link() {
        Some(link) => mouse_area(framed).on_press((pictures.show)(link)).cursor(Cursor::Pointer).into(),
        None => framed.into(),
    }
}

/// A formula set on its own: centred, scrolling sideways if it's wider than
/// the column, with a button to copy its LaTeX. LaTeX that couldn't be set
/// shows as written, with what's wrong.
fn display_math<Msg: Clone + Send + 'static>(f: &Arc<Formula>, look: &Look<Msg>) -> Element<Msg> {
    let copy = icon_button(Icon::Copy).on_press((look.copy)(f.source.clone())).tooltip("Copy the LaTeX");
    let shown: Element<Msg> = match (formula(f), &f.set) {
        (Some(span), _) => {
            let set = rich_text([span]).role(look.role).scale(look.scale).selectable();
            scroll(container(set).align_x(Align::Center).padding([2.0, 0.0]).width(Fill)).horizontal().height(Shrink).into()
        }
        (None, error) => column![
            text(f.source.clone()).mono().scale(0.92).wrap().selectable(),
            text(format!("This LaTeX can't be set: {}", error.as_ref().err().map_or("", String::as_str)))
                .scale(0.85)
                .role(Role::Danger)
                .wrap(),
        ]
        .spacing(4.0)
        .width(Fill)
        .into(),
    };
    row![shown, copy].spacing(8.0).padding([4.0, 0.0]).width(Fill).into()
}

/// A Mermaid diagram, in a box with its kind and a button to copy its
/// source, drawn in the palette that shows: as large as it was laid out,
/// smaller in a narrower column. One that couldn't be drawn shows its
/// source, with what's wrong.
fn diagram<Msg: Clone + Send + 'static>(d: &Arc<Diagram>, look: &Look<Msg>) -> Element<Msg> {
    let header = row![
        icon(Icon::Network).size(14.0).role(Role::Text3),
        text(d.kind().to_owned()).mono().scale(0.85).role(Role::Text3),
        spacer(),
        icon_button(Icon::Copy).on_press((look.copy)(d.source.clone())).tooltip("Copy the diagram's source"),
    ]
    .spacing(6.0)
    .padding([2.0, 4.0, 2.0, 10.0]);
    let body: Element<Msg> = match &d.drawn {
        Ok([night, ..]) => {
            let size = night.size;
            let drawn = d.clone();
            let picture = canvas(d.id, move |c, bounds, _, theme| {
                if let Some(p) = drawn.picture(theme) {
                    p.paint(c, bounds.origin(), bounds.width() / p.size.width.max(1.0));
                }
            })
            .aspect(size.width / size.height.max(1.0))
            .max_width(size.width)
            .width(Shrink);
            container(picture).align_x(Align::Center).padding([14.0, 16.0]).width(Fill).into()
        }
        Err(error) => column![
            text(d.source.clone()).mono().scale(0.92).selectable(),
            text(format!("This diagram can't be drawn: {error}")).scale(0.85).role(Role::Danger).wrap(),
        ]
        .spacing(8.0)
        .padding([8.0, 12.0, 10.0, 12.0])
        .width(Fill)
        .into(),
    };
    container(column![header, divider(), body].width(Fill)).border(Edge::Line).width(Fill).into()
}

fn code_block<Msg: Clone + Send + 'static>(
    lang: &str,
    source: &Arc<str>,
    lines: &Arc<Vec<Vec<Run>>>,
    look: &Look<Msg>,
) -> Element<Msg> {
    let mut spans = Vec::with_capacity(lines.iter().map(Vec::len).sum::<usize>() + lines.len());
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            spans.push(Span::new("\n"));
        }
        spans.extend(line.iter().map(|run| {
            let s = Span::new(run.text.clone());
            match run.role {
                Some(role) => s.role(role),
                None => s,
            }
        }));
    }
    let label = if lang.is_empty() { "code".to_owned() } else { lang.to_owned() };
    let header = row![
        text(label).mono().scale(0.85).role(Role::Text3),
        spacer(),
        icon_button(Icon::Copy).on_press((look.copy)(source.to_string())).tooltip("Copy the code"),
    ]
    .spacing(6.0)
    .padding([2.0, 4.0, 2.0, 10.0]);
    let code = rich_text(spans)
        .font(FontSpec::mono(13.0))
        .scale(look.scale * 0.95)
        .line_height(1.35)
        .selectable();
    let body = scroll(container(code).padding([8.0, 12.0, 10.0, 12.0])).horizontal().height(Shrink);
    container(column![header, divider(), body].width(Fill)).fill(Tone::Sunk).border(Edge::Line).width(Fill).into()
}

fn callout<Msg: Clone + Send + 'static>(kind: Callout, inner: &[Block], look: &Look<Msg>) -> Element<Msg> {
    let glyph = match kind {
        Callout::Note => Icon::Info,
        Callout::Tip => Icon::Sparkles,
        Callout::Important => Icon::CircleAlert,
        Callout::Warning => Icon::TriangleAlert,
        Callout::Caution => Icon::CircleX,
    };
    let title = row![icon(glyph).size(15.0).role(kind.role()), text(kind.title()).semibold().role(kind.role())].spacing(8.0);
    container(column![title, blocks(inner, look)].spacing(6.0).width(Fill))
        .fill(Tone::Chrome)
        .border(Edge::Line)
        .padding([10.0, 14.0])
        .width(Fill)
        .into()
}

fn list<Msg: Clone + Send + 'static>(start: Option<u64>, items: &[Item], look: &Look<Msg>) -> Element<Msg> {
    let rows = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let marker: Element<Msg> = match (item.task, start) {
                (Some(done), _) => icon(if done { Icon::SquareCheck } else { Icon::Square })
                    .size(15.0)
                    .role(if done { Role::Success } else { Role::Text3 })
                    .into(),
                (None, Some(first)) => text(format!("{}.", first + index as u64))
                    .scale(look.scale)
                    .line_height(LINE_HEIGHT)
                    .tabular()
                    .role(Role::Text3)
                    .into(),
                (None, None) => text("•").scale(look.scale).line_height(LINE_HEIGHT).role(Role::Text3).into(),
            };
            let top = if item.task.is_some() { 4.0 } else { 0.0 };
            row![container(marker).width(MARKER).padding([top, 0.0, 0.0, 0.0]), blocks(&item.blocks, look)]
                .align(Align::Start)
                .width(Fill)
                .into()
        })
        .collect();
    column_of(rows).spacing(6.0).width(Fill).into()
}

fn table_block<Msg: Clone + Send + 'static>(
    aligns: &[CellAlign],
    head: &[Vec<Run>],
    rows: &[Vec<Vec<Run>>],
    look: &Look<Msg>,
) -> Element<Msg> {
    let columns = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return space(0.0);
    }
    // Each column's share of the width, by the longest text it holds.
    let widest = |c: usize| {
        std::iter::once(head.get(c))
            .chain(rows.iter().map(|r| r.get(c)))
            .flatten()
            .map(|runs| super::runs_text(runs).chars().count())
            .max()
            .unwrap_or(1)
    };
    let portions: Vec<u16> = (0..columns).map(|c| (widest(c) as u16).clamp(4, 40)).collect();
    let cell = |runs: Option<&Vec<Run>>, c: usize, header: bool| -> Element<Msg> {
        let runs = runs.cloned().unwrap_or_default();
        let open = look.open.clone();
        let content = rich_text(runs.iter().map(|r| if header { span(r).weight(650.0) } else { span(r) }))
            .scale(look.scale * 0.96)
            .wrap()
            .on_link(move |target| open(target));
        let align = match aligns.get(c) {
            Some(CellAlign::Center) => Align::Center,
            Some(CellAlign::End) => Align::End,
            _ => Align::Start,
        };
        let boxed: Container<Msg> = container(content).padding([5.0, 10.0]).align_x(align);
        boxed.width(Length::FillPortion(portions[c])).into()
    };
    let line = |cells: &[Vec<Run>], header: bool| -> Element<Msg> {
        row_of((0..columns).map(|c| cell(cells.get(c), c, header)).collect()).align(Align::Start).width(Fill).into()
    };
    let mut lines: Vec<Element<Msg>> = vec![container(line(head, true)).fill(Tone::Chrome).width(Fill).into()];
    for r in rows {
        lines.push(divider().into());
        lines.push(line(r, false));
    }
    let table = container(column_of(lines).width(Fill)).border(Edge::Line).padding(1.0).width(Fill);
    if columns > 5 {
        scroll(table.min_width(columns as f64 * 120.0)).horizontal().height(Shrink).into()
    } else {
        table.into()
    }
}
