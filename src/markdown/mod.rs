//! Markdown, as the transcript shows it: answers parsed into a document of
//! blocks and styled runs ([`parse`]), with code coloured in its language
//! ([`highlight`]). The document holds roles, not colours, so it is
//! parsed once on a worker and drawn in whichever theme is current
//! ([`view`]).
//!
//! What it covers: paragraphs, headings, emphasis, strikethrough, inline
//! and fenced code, links, block quotes and GitHub's callouts, ordered,
//! bulleted and task lists, tables, rules, images, math (between `$`s,
//! `$$`s, `\(…\)` and `\[…\]`, and in `math` fences: set as KaTeX sets
//! it, [`math`]), and Mermaid diagrams ([`diagram`]). Math and diagrams
//! are set here too, so they are a worker's job like the code.

pub(crate) mod diagram;
pub(crate) mod highlight;
pub(crate) mod math;
pub(crate) mod view;

use std::borrow::Cow;
use std::sync::Arc;

use aizome_kit::Role;
use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// A parsed answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Doc {
    pub(crate) blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Block {
    Paragraph(Vec<Run>),
    Heading { level: u8, runs: Vec<Run> },
    /// Fenced or indented code: its language as written, its text, and
    /// its lines coloured.
    Code { lang: String, text: Arc<str>, lines: Arc<Vec<Vec<Run>>> },
    Quote(Vec<Block>),
    Callout { kind: Callout, blocks: Vec<Block> },
    List { start: Option<u64>, items: Vec<Item> },
    Table { aligns: Vec<Align>, head: Vec<Vec<Run>>, rows: Vec<Vec<Vec<Run>>> },
    Rule,
    /// Math set on its own.
    Math(Arc<math::Formula>),
    /// A Mermaid diagram.
    Diagram(Arc<diagram::Diagram>),
    Image { url: String, alt: String },
}

/// One list item: a task's box, when it has one, and what it holds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Item {
    pub(crate) task: Option<bool>,
    pub(crate) blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Callout {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Callout {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Callout::Note => "Note",
            Callout::Tip => "Tip",
            Callout::Important => "Important",
            Callout::Warning => "Warning",
            Callout::Caution => "Caution",
        }
    }

    pub(crate) fn role(self) -> Role {
        match self {
            Callout::Note => Role::Cyan,
            Callout::Tip => Role::Success,
            Callout::Important => Role::Violet,
            Callout::Warning => Role::Warning,
            Callout::Caution => Role::Danger,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Align {
    Start,
    Center,
    End,
}

/// A run of text in one style.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Run {
    /// Its text; a formula's LaTeX, between `$`s.
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) strike: bool,
    pub(crate) code: bool,
    /// A formula in the line: the run is it alone.
    pub(crate) math: Option<Arc<math::Formula>>,
    pub(crate) link: Option<Arc<str>>,
    /// A colour of its own (code's tokens).
    pub(crate) role: Option<Role>,
}

impl Run {
    pub(crate) fn plain(text: impl Into<String>) -> Run {
        Run { text: text.into(), ..Run::default() }
    }

    fn same_style(&self, other: &Run) -> bool {
        self.math.is_none()
            && other.math.is_none()
            && (self.bold, self.italic, self.strike, self.code, &self.link, self.role)
                == (other.bold, other.italic, other.strike, other.code, &other.link, other.role)
    }
}

/// The style the open inline tags give text.
#[derive(Debug, Clone, Default)]
struct Style {
    bold: u32,
    italic: u32,
    strike: u32,
    /// Inside `<kbd>`, `<code>` or `<tt>`: set as code.
    code: u32,
    link: Vec<Arc<str>>,
}

impl Style {
    fn run(&self, text: &str) -> Run {
        Run {
            text: text.to_owned(),
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            link: self.link.last().cloned(),
            code: self.code > 0,
            ..Run::default()
        }
    }
}

/// Where blocks are being gathered: the document, a quote, a list item.
#[derive(Debug)]
enum Frame {
    Root(Vec<Block>),
    Quote(Option<Callout>, Vec<Block>),
    List(Option<u64>, Vec<Item>),
    Item(Option<bool>, Vec<Block>),
}

impl Frame {
    fn blocks(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Frame::Root(b) | Frame::Quote(_, b) | Frame::Item(_, b) => Some(b),
            Frame::List(..) => None,
        }
    }
}

struct Builder {
    frames: Vec<Frame>,
    /// Runs of the paragraph, heading or cell being written.
    runs: Option<Vec<Run>>,
    style: Style,
    heading: Option<u8>,
    code: Option<(String, String)>,
    image: Option<(String, String)>,
    /// Inside a metadata block (front matter): left out.
    metadata: bool,
    table: Option<TableDraft>,
}

/// A table being read: its columns' alignment, its head, its rows, the
/// row being read, and whether the head is being read.
type TableDraft = (Vec<Align>, Vec<Vec<Run>>, Vec<Vec<Vec<Run>>>, Vec<Vec<Run>>, bool);

impl Builder {
    fn push_block(&mut self, block: Block) {
        // Text directly in a list item (a tight list) belongs to it; so does
        // anything else: find the nearest frame that holds blocks.
        for frame in self.frames.iter_mut().rev() {
            if let Some(blocks) = frame.blocks() {
                blocks.push(block);
                return;
            }
        }
    }

    fn push_run(&mut self, run: Run) {
        if run.text.is_empty() {
            return;
        }
        let runs = self.runs.get_or_insert_with(Vec::new);
        match runs.last_mut() {
            Some(last) if last.same_style(&run) => last.text.push_str(&run.text),
            _ => runs.push(run),
        }
    }

    /// The runs written so far become a paragraph (text in a tight list
    /// item has no paragraph tags of its own).
    fn flush_runs(&mut self) {
        if let Some(runs) = self.runs.take() {
            let runs = trim_runs(runs);
            if !runs.is_empty() {
                self.push_block(Block::Paragraph(runs));
            }
        }
    }
}

fn trim_runs(mut runs: Vec<Run>) -> Vec<Run> {
    if let Some(first) = runs.first_mut() {
        first.text = first.text.trim_start().to_owned();
    }
    if let Some(last) = runs.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }
    runs.retain(|r| !r.text.is_empty());
    runs
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn callout(kind: Option<BlockQuoteKind>) -> Option<Callout> {
    kind.map(|k| match k {
        BlockQuoteKind::Note => Callout::Note,
        BlockQuoteKind::Tip => Callout::Tip,
        BlockQuoteKind::Important => Callout::Important,
        BlockQuoteKind::Warning => Callout::Warning,
        BlockQuoteKind::Caution => Callout::Caution,
    })
}

/// `\(…\)` and `\[…\]`, the delimiters LaTeX uses (and models write), as
/// the `$…$` and `$$…$$` the parser reads. Code, fenced or between
/// backticks, stays as it is, and so does a paren after an escaped
/// backslash (`\\[2pt]`).
fn latex_delimiters(text: &str) -> Cow<'_, str> {
    if !text.contains("\\(") && !text.contains("\\[") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    // The open fence's character and length.
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if i == 0 || text.as_bytes()[i - 1] == b'\n' {
            let line = &rest[..rest.find('\n').map_or(rest.len(), |n| n + 1)];
            let marker = line.trim_start_matches(' ');
            let run = |c: char| marker.chars().take_while(|m| *m == c).count();
            // (A backtick fence's info string has no backticks.)
            let opens = ['`', '~']
                .into_iter()
                .map(|c| (c, run(c)))
                .find(|(c, n)| *n >= 3 && (*c == '~' || !marker[*n..].contains('`')));
            match (fence, opens) {
                (Some((c, n)), Some((m, k))) if c == m && k >= n && marker[k..].trim().is_empty() => fence = None,
                (None, Some(open)) => fence = Some(open),
                _ => {}
            }
            if fence.is_some() || opens.is_some() {
                out.push_str(line);
                i += line.len();
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or_default();
        if c == '`' {
            // A code span, to its closing run of as many backticks.
            let n = rest.chars().take_while(|m| *m == '`').count();
            let ticks = &rest[..n];
            let close = rest[n..].match_indices(ticks).find(|(at, _)| {
                let after = &rest[n + at + n..];
                !after.starts_with('`') && !rest[..n + at].ends_with('`')
            });
            let end = close.map_or(n, |(at, _)| n + at + n);
            out.push_str(&rest[..end]);
            i += end;
            continue;
        }
        if c == '\\' {
            let (open, close, dollars) = match rest.as_bytes().get(1) {
                Some(b'(') => ("\\(", "\\)", "$"),
                Some(b'[') => ("\\[", "\\]", "$$"),
                Some(b'\\') => {
                    out.push_str("\\\\");
                    i += 2;
                    continue;
                }
                _ => ("", "", ""),
            };
            let body = &rest[open.len()..];
            if let Some(end) = (!open.is_empty()).then(|| body.find(close)).flatten()
                && !body[..end].contains("\n\n")
                && !body[..end].trim().is_empty()
            {
                let math = &body[..end];
                out.push_str(dollars);
                out.push_str(if dollars == "$" { math.trim() } else { math });
                out.push_str(dollars);
                i += open.len() + end + close.len();
                continue;
            }
        }
        out.push(c);
        i += c.len_utf8();
    }
    Cow::Owned(out)
}

/// Parse `text` into a document. Code is coloured, math set and diagrams
/// drawn here too, so this is a worker's job for anything long.
pub(crate) fn parse(text: &str) -> Doc {
    let text = latex_delimiters(text);
    let text = text.as_ref();
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut b = Builder {
        frames: vec![Frame::Root(Vec::new())],
        runs: None,
        style: Style::default(),
        heading: None,
        code: None,
        image: None,
        metadata: false,
        table: None,
    };
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => b.flush_runs(),
                Tag::Heading { level, .. } => {
                    b.flush_runs();
                    b.heading = Some(heading_level(level));
                    b.runs = Some(Vec::new());
                }
                Tag::BlockQuote(kind) => {
                    b.flush_runs();
                    b.frames.push(Frame::Quote(callout(kind), Vec::new()));
                }
                Tag::CodeBlock(kind) => {
                    b.flush_runs();
                    let lang = match kind {
                        CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_owned(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    b.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    b.flush_runs();
                    b.frames.push(Frame::List(start, Vec::new()));
                }
                Tag::Item => {
                    b.flush_runs();
                    b.frames.push(Frame::Item(None, Vec::new()));
                }
                Tag::Table(aligns) => {
                    b.flush_runs();
                    let aligns = aligns
                        .iter()
                        .map(|a| match a {
                            Alignment::Center => Align::Center,
                            Alignment::Right => Align::End,
                            _ => Align::Start,
                        })
                        .collect();
                    b.table = Some((aligns, Vec::new(), Vec::new(), Vec::new(), false));
                }
                Tag::TableHead => {
                    if let Some(t) = &mut b.table {
                        t.4 = true;
                        t.3.clear();
                    }
                }
                Tag::TableRow => {
                    if let Some(t) = &mut b.table {
                        t.3.clear();
                    }
                }
                Tag::TableCell => b.runs = Some(Vec::new()),
                Tag::Emphasis => b.style.italic += 1,
                Tag::Strong => b.style.bold += 1,
                Tag::Strikethrough => b.style.strike += 1,
                Tag::Link { dest_url, .. } => b.style.link.push(Arc::from(dest_url.as_ref())),
                Tag::Image { dest_url, .. } => {
                    b.image = Some((dest_url.to_string(), String::new()));
                }
                Tag::MetadataBlock(_) => b.metadata = true,
                Tag::FootnoteDefinition(label) => {
                    b.flush_runs();
                    b.push_run(Run { text: format!("[{label}] "), bold: true, ..Run::default() });
                }
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph => b.flush_runs(),
                TagEnd::Heading(_) => {
                    let runs = trim_runs(b.runs.take().unwrap_or_default());
                    let level = b.heading.take().unwrap_or(1);
                    if !runs.is_empty() {
                        b.push_block(Block::Heading { level, runs });
                    }
                }
                TagEnd::BlockQuote(_) => {
                    b.flush_runs();
                    if let Some(Frame::Quote(kind, blocks)) = b.frames.pop() {
                        b.push_block(match kind {
                            Some(kind) => Block::Callout { kind, blocks },
                            None => Block::Quote(blocks),
                        });
                    }
                }
                TagEnd::CodeBlock => {
                    // Math and diagrams are set once their fence closes: until
                    // then (an answer streaming in) they show as written.
                    let fence = text[range].trim_end();
                    let closed = fence.ends_with("```") || fence.ends_with("~~~");
                    if let Some((lang, mut text)) = b.code.take() {
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        let block = if closed && matches!(lang.as_str(), "math" | "latex" | "tex") {
                            Block::Math(math::set(&text, true))
                        } else if closed && lang.eq_ignore_ascii_case("mermaid") {
                            Block::Diagram(diagram::draw(&text))
                        } else {
                            let lines = highlight::lines(&lang, &text);
                            Block::Code { lang, text: Arc::from(text), lines: Arc::new(lines) }
                        };
                        b.push_block(block);
                    }
                }
                TagEnd::List(_) => {
                    b.flush_runs();
                    if let Some(Frame::List(start, items)) = b.frames.pop() {
                        b.push_block(Block::List { start, items });
                    }
                }
                TagEnd::Item => {
                    b.flush_runs();
                    if let Some(Frame::Item(task, blocks)) = b.frames.pop()
                        && let Some(Frame::List(_, items)) = b.frames.last_mut()
                    {
                        items.push(Item { task, blocks });
                    }
                }
                TagEnd::TableCell => {
                    let runs = trim_runs(b.runs.take().unwrap_or_default());
                    if let Some(t) = &mut b.table {
                        t.3.push(runs);
                    }
                }
                TagEnd::TableHead => {
                    if let Some(t) = &mut b.table {
                        t.1 = std::mem::take(&mut t.3);
                        t.4 = false;
                    }
                }
                TagEnd::TableRow => {
                    if let Some(t) = &mut b.table {
                        let row = std::mem::take(&mut t.3);
                        t.2.push(row);
                    }
                }
                TagEnd::Table => {
                    if let Some((aligns, head, rows, _, _)) = b.table.take() {
                        b.push_block(Block::Table { aligns, head, rows });
                    }
                }
                TagEnd::Emphasis => b.style.italic = b.style.italic.saturating_sub(1),
                TagEnd::Strong => b.style.bold = b.style.bold.saturating_sub(1),
                TagEnd::Strikethrough => b.style.strike = b.style.strike.saturating_sub(1),
                TagEnd::Link => {
                    b.style.link.pop();
                }
                TagEnd::Image => {
                    if let Some((url, alt)) = b.image.take() {
                        // An image in a line of text stays a link to it; on
                        // its own it is shown.
                        if b.runs.as_ref().is_some_and(|r| r.iter().any(|run| !run.text.trim().is_empty())) {
                            let label = if alt.is_empty() { url.clone() } else { alt };
                            b.push_run(Run { text: label, link: Some(Arc::from(url.as_str())), ..Run::default() });
                        } else {
                            b.flush_runs();
                            b.push_block(Block::Image { url, alt });
                        }
                    }
                }
                TagEnd::FootnoteDefinition => b.flush_runs(),
                TagEnd::MetadataBlock(_) => b.metadata = false,
                _ => {}
            },
            Event::Text(_) if b.metadata => {}
            Event::Text(text) => {
                if let Some((_, code)) = &mut b.code {
                    code.push_str(&text);
                } else if let Some((_, alt)) = &mut b.image {
                    alt.push_str(&text);
                } else {
                    let run = b.style.run(&text);
                    b.push_run(run);
                }
            }
            Event::Code(text) => {
                let mut run = b.style.run(&text);
                run.code = true;
                b.push_run(run);
            }
            Event::InlineMath(source) => {
                let mut run = b.style.run(&format!("${}$", source.trim()));
                run.math = Some(math::set(&source, false));
                b.push_run(run);
            }
            Event::DisplayMath(source) => {
                b.flush_runs();
                b.push_block(Block::Math(math::set(&source, true)));
            }
            Event::InlineHtml(html) => match inline_tag(&html) {
                // The few tags answers use for type: kept as their look.
                Some(("kbd" | "code" | "tt", open)) => {
                    b.style.code = if open { b.style.code + 1 } else { b.style.code.saturating_sub(1) };
                }
                Some(("br", _)) => {
                    let run = b.style.run("\n");
                    b.push_run(run);
                }
                Some(("b" | "strong", open)) => b.style.bold = if open { b.style.bold + 1 } else { b.style.bold.saturating_sub(1) },
                Some(("i" | "em", open)) => b.style.italic = if open { b.style.italic + 1 } else { b.style.italic.saturating_sub(1) },
                Some(("sup" | "sub" | "span" | "small" | "mark" | "u", _)) => {}
                // Anything else is shown as written: markup is text to read.
                _ => {
                    let run = b.style.run(&html);
                    b.push_run(run);
                }
            },
            Event::Html(html) => {
                for line in block_html(&html) {
                    match line {
                        HtmlLine::Summary(text) => {
                            b.flush_runs();
                            b.style.bold += 1;
                            let run = b.style.run(&text);
                            b.push_run(run);
                            b.style.bold -= 1;
                            b.flush_runs();
                        }
                        HtmlLine::Rule => {
                            b.flush_runs();
                            b.push_block(Block::Rule);
                        }
                        HtmlLine::Text(text) => {
                            let run = b.style.run(&text);
                            b.push_run(run);
                        }
                    }
                }
            }
            Event::SoftBreak => {
                let run = b.style.run(" ");
                b.push_run(run);
            }
            Event::HardBreak => {
                let run = b.style.run("\n");
                b.push_run(run);
            }
            Event::Rule => {
                b.flush_runs();
                b.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item(task, _)) = b.frames.last_mut() {
                    *task = Some(done);
                }
            }
            Event::FootnoteReference(label) => {
                let mut run = b.style.run(&format!("[{label}]"));
                run.role = Some(Role::Accent);
                b.push_run(run);
            }
        }
    }
    b.flush_runs();
    // Unclosed frames (a reply cut off mid-stream) give back what they hold.
    while b.frames.len() > 1 {
        match b.frames.pop() {
            Some(Frame::Quote(kind, blocks)) => b.push_block(match kind {
                Some(kind) => Block::Callout { kind, blocks },
                None => Block::Quote(blocks),
            }),
            Some(Frame::List(start, items)) => b.push_block(Block::List { start, items }),
            Some(Frame::Item(task, blocks)) => {
                if let Some(Frame::List(_, items)) = b.frames.last_mut() {
                    items.push(Item { task, blocks });
                } else {
                    for block in blocks {
                        b.push_block(block);
                    }
                }
            }
            _ => {}
        }
    }
    match b.frames.pop() {
        Some(Frame::Root(blocks)) => Doc { blocks },
        _ => Doc::default(),
    }
}

/// A simple inline tag: its name, lowercased, and whether it opens.
fn inline_tag(html: &str) -> Option<(&'static str, bool)> {
    let inner = html.trim().strip_prefix('<')?.strip_suffix('>')?.trim_end_matches('/').trim();
    let (open, name) = match inner.strip_prefix('/') {
        Some(name) => (false, name),
        None => (true, inner),
    };
    let name = name.split_whitespace().next()?.to_ascii_lowercase();
    ["kbd", "code", "tt", "br", "b", "strong", "i", "em", "sup", "sub", "span", "small", "mark", "u"]
        .into_iter()
        .find(|tag| *tag == name)
        .map(|tag| (tag, open))
}

/// A line of a block of HTML, as an answer means it.
enum HtmlLine {
    /// A `<summary>`: the heading of a folded part, shown bold.
    Summary(String),
    Rule,
    /// Anything else, as written: markup is text to read.
    Text(String),
}

/// What a block of HTML shows: the wrappers answers use (`<details>`,
/// `<div>`, `<p>`, `<center>`) left out, so what they hold reads as text.
fn block_html(html: &str) -> Vec<HtmlLine> {
    let mut lines = Vec::new();
    for line in html.lines() {
        let trimmed = line.trim();
        if let Some(summary) = trimmed.strip_prefix("<summary>").and_then(|s| s.strip_suffix("</summary>")) {
            lines.push(HtmlLine::Summary(summary.trim().to_owned()));
            continue;
        }
        let tag = trimmed
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .and_then(|inner| inner.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next())
            .map(str::to_ascii_lowercase);
        match tag.as_deref() {
            Some("hr") => lines.push(HtmlLine::Rule),
            Some("details" | "summary" | "div" | "p" | "br" | "center" | "section") => {}
            _ if trimmed.is_empty() => {}
            _ => lines.push(HtmlLine::Text(format!("{line}\n"))),
        }
    }
    lines
}

/// The text of `runs`, as copied.
pub(crate) fn runs_text(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: &Doc) -> Vec<String> {
        doc.blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph(r) | Block::Heading { runs: r, .. } => runs_text(r),
                Block::Code { text, .. } => text.to_string(),
                other => format!("{other:?}").chars().take(12).collect(),
            })
            .collect()
    }

    #[test]
    fn paragraphs_headings_and_emphasis() {
        let doc = parse("# Title\n\nSome **bold** and *italic* and `code` and [a link](https://x.test).\n\nNext.");
        assert_eq!(texts(&doc), ["Title", "Some bold and italic and code and a link.", "Next."]);
        let Block::Paragraph(runs) = &doc.blocks[1] else { panic!() };
        assert!(runs.iter().any(|r| r.bold && r.text == "bold"));
        assert!(runs.iter().any(|r| r.italic && r.text == "italic"));
        assert!(runs.iter().any(|r| r.code && r.text == "code"));
        assert!(runs.iter().any(|r| r.link.as_deref() == Some("https://x.test") && r.text == "a link"));
    }

    #[test]
    fn folded_html_reads_as_its_summary_and_its_text() {
        let doc = parse("<details>\n<summary>A folded detail</summary>\n\nHidden text with **Markdown** inside.\n\n</details>\n\n<custom-tag>kept</custom-tag>");
        assert_eq!(texts(&doc), ["A folded detail", "Hidden text with Markdown inside.", "<custom-tag>kept</custom-tag>"]);
        let Block::Paragraph(summary) = &doc.blocks[0] else { panic!("{doc:?}") };
        assert!(summary.iter().all(|r| r.bold));
    }

    #[test]
    fn lists_nest_and_tasks_keep_their_boxes() {
        let doc = parse("- one\n- two\n  - deeper\n\n1. first\n2. second\n\n- [x] done\n- [ ] open");
        let Block::List { start: None, items } = &doc.blocks[0] else { panic!("{doc:?}") };
        assert_eq!(items.len(), 2);
        assert!(matches!(items[1].blocks.last(), Some(Block::List { .. })));
        assert!(matches!(&doc.blocks[1], Block::List { start: Some(1), items } if items.len() == 2));
        let Block::List { items, .. } = &doc.blocks[2] else { panic!() };
        assert_eq!(items.iter().map(|i| i.task).collect::<Vec<_>>(), [Some(true), Some(false)]);
    }

    #[test]
    fn code_is_kept_whole_and_coloured_by_language() {
        let doc = parse("```rust\nfn main() {\n    // hi\n}\n```");
        let Block::Code { lang, text, lines } = &doc.blocks[0] else { panic!("{doc:?}") };
        assert_eq!(lang, "rust");
        assert_eq!(&**text, "fn main() {\n    // hi\n}");
        assert_eq!(lines.len(), 3);
        assert!(lines[0].iter().any(|r| r.text.contains("fn") && r.role == Some(Role::Violet)), "{lines:?}");
        assert!(lines[1].iter().any(|r| r.text.contains("// hi") && r.role == Some(Role::Text3)), "{lines:?}");
    }

    #[test]
    fn tables_quotes_callouts_and_math() {
        let doc = parse(
            "| A | B |\n|:--|--:|\n| 1 | 2 |\n\n> quoted\n\n> [!WARNING]\n> careful\n\n$$\\alpha + \\beta$$\n\n---",
        );
        let Block::Table { aligns, head, rows } = &doc.blocks[0] else { panic!("{doc:?}") };
        assert_eq!(aligns, &[Align::Start, Align::End]);
        assert_eq!(runs_text(&head[1]), "B");
        assert_eq!(runs_text(&rows[0][0]), "1");
        assert!(matches!(&doc.blocks[1], Block::Quote(b) if b.len() == 1));
        assert!(matches!(&doc.blocks[2], Block::Callout { kind: Callout::Warning, .. }));
        assert!(matches!(&doc.blocks[3], Block::Math(f) if f.source == "\\alpha + \\beta" && f.set.is_ok()));
        assert_eq!(doc.blocks[4], Block::Rule);
    }

    #[test]
    fn math_between_any_of_its_delimiters_is_set_but_not_in_code() {
        let doc = parse(
            "Inline $e^{i\\pi}$ and \\(a^2 + b^2\\) and \\[\\int_0^1 x\\,dx\\] then `\\(code\\)`.\n\n\
             ```\n\\(fenced\\)\n```\n\n\\[\n\\begin{aligned} a &= b \\\\[2pt] c &= d \\end{aligned}\n\\]",
        );
        let Block::Paragraph(runs) = &doc.blocks[0] else { panic!("{doc:?}") };
        let inline: Vec<&str> = runs.iter().filter_map(|r| r.math.as_ref().map(|f| f.source.as_str())).collect();
        assert_eq!(inline, ["e^{i\\pi}", "a^2 + b^2"]);
        assert!(runs.iter().all(|r| r.math.as_ref().is_none_or(|f| f.set.is_ok() && !f.display)));
        assert!(matches!(&doc.blocks[1], Block::Math(f) if f.source == "\\int_0^1 x\\,dx" && f.display));
        assert!(matches!(&doc.blocks[2], Block::Paragraph(r) if r.iter().any(|r| r.code && r.text == "\\(code\\)")), "{doc:?}");
        assert!(matches!(&doc.blocks[3], Block::Code { text, .. } if &**text == "\\(fenced\\)"));
        assert!(matches!(&doc.blocks[4], Block::Math(f) if f.source.contains("\\\\[2pt]") && f.set.is_ok()), "{doc:?}");
    }

    #[test]
    fn mermaid_fences_are_drawn() {
        let doc = parse("```mermaid\nsequenceDiagram\n    A->>B: Ask\n```");
        let Block::Diagram(d) = &doc.blocks[0] else { panic!("{doc:?}") };
        assert_eq!(d.kind(), "sequenceDiagram");
        assert!(d.drawn.is_ok());
        // Still streaming in: shown as written until the fence closes.
        let doc = parse("```mermaid\nsequenceDiagram\n    A->>B: As");
        assert!(matches!(&doc.blocks[0], Block::Code { lang, .. } if lang == "mermaid"), "{doc:?}");
    }

    #[test]
    fn a_reply_cut_off_mid_list_still_shows() {
        let doc = parse("Intro\n\n- one\n- tw");
        assert_eq!(doc.blocks.len(), 2);
        let doc = parse("```py\nprint(");
        assert!(matches!(&doc.blocks[0], Block::Code { text, .. } if &**text == "print("));
    }

    #[test]
    fn an_image_alone_is_shown_and_in_text_is_a_link() {
        let doc = parse("![diagram](shot.png)\n\nSee ![this](a.png) here.");
        assert_eq!(doc.blocks[0], Block::Image { url: "shot.png".into(), alt: "diagram".into() });
        let Block::Paragraph(runs) = &doc.blocks[1] else { panic!() };
        assert!(runs.iter().any(|r| r.link.as_deref() == Some("a.png")));
    }
}
