use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

const NATIVE_RULE: &str = "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextRange {
    pub(crate) location: usize,
    pub(crate) length: usize,
}

impl TextRange {
    fn from_bounds(start: usize, end: usize) -> Self {
        Self {
            location: start,
            length: end.saturating_sub(start),
        }
    }

    pub(crate) fn end(self) -> usize {
        self.location + self.length
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TextTone {
    #[default]
    Primary,
    Secondary,
    Accent,
    Separator,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InlineStyle {
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) code: bool,
    pub(crate) code_block: bool,
    pub(crate) code_label: bool,
    pub(crate) strikethrough: bool,
    pub(crate) heading: Option<u8>,
    pub(crate) link: Option<String>,
    pub(crate) tone: TextTone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StyledRun {
    pub(crate) range: TextRange,
    pub(crate) style: InlineStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParagraphKind {
    Body,
    Heading(u8),
    CodeBlock,
    TableCell,
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Paragraph {
    pub(crate) range: TextRange,
    pub(crate) kind: ParagraphKind,
    pub(crate) quote_depth: usize,
    pub(crate) list_depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeBlock {
    pub(crate) range: TextRange,
    pub(crate) code_range: TextRange,
    pub(crate) language: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TableCell {
    pub(crate) range: TextRange,
    pub(crate) row: usize,
    pub(crate) column: usize,
    pub(crate) header: bool,
    pub(crate) alignment: Alignment,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MarkdownTable {
    pub(crate) columns: usize,
    pub(crate) cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownImage {
    pub(crate) range: TextRange,
    pub(crate) destination: String,
    pub(crate) alt: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MarkdownDocument {
    pub(crate) text: String,
    pub(crate) runs: Vec<StyledRun>,
    pub(crate) paragraphs: Vec<Paragraph>,
    pub(crate) code_blocks: Vec<CodeBlock>,
    pub(crate) tables: Vec<MarkdownTable>,
    pub(crate) images: Vec<MarkdownImage>,
}

#[derive(Debug, Clone, Copy)]
struct ActiveParagraph {
    start: usize,
    kind: ParagraphKind,
    quote_depth: usize,
    list_depth: usize,
}

#[derive(Debug, Clone, Copy)]
struct ListState {
    next_number: Option<u64>,
}

#[derive(Debug)]
struct ActiveCodeBlock {
    code_start: usize,
    language: String,
}

#[derive(Debug)]
struct ActiveTable {
    alignments: Vec<Alignment>,
    cells: Vec<TableCell>,
    row: usize,
    column: usize,
    header: bool,
}

#[derive(Debug)]
struct ActiveImage {
    destination: String,
    alt: String,
}

#[derive(Debug, Default)]
struct StyleState {
    bold_depth: usize,
    italic_depth: usize,
    strike_depth: usize,
    code_depth: usize,
    code_block_depth: usize,
    heading: Option<u8>,
    links: Vec<String>,
}

impl StyleState {
    fn current(&self) -> InlineStyle {
        InlineStyle {
            bold: self.bold_depth > 0,
            italic: self.italic_depth > 0,
            code: self.code_depth > 0,
            code_block: self.code_block_depth > 0,
            code_label: false,
            strikethrough: self.strike_depth > 0,
            heading: self.heading,
            link: self.links.last().cloned(),
            tone: TextTone::Primary,
        }
    }
}

#[derive(Debug, Default)]
struct DocumentBuilder {
    document: MarkdownDocument,
    style: StyleState,
    active_paragraph: Option<ActiveParagraph>,
    quote_depth: usize,
    lists: Vec<ListState>,
    active_code_block: Option<ActiveCodeBlock>,
    active_table: Option<ActiveTable>,
    active_image: Option<ActiveImage>,
    /// The text's length in UTF-16 units, kept as it grows: recounting the
    /// whole text on every append made parsing quadratic in the message.
    utf16_length: usize,
}

impl DocumentBuilder {
    fn utf16_len(&self) -> usize {
        self.utf16_length
    }

    fn append(&mut self, value: &str) {
        let style = self.style.current();
        self.append_with_style(value, style);
    }

    fn append_with_style(&mut self, value: &str, style: InlineStyle) {
        if value.is_empty() {
            return;
        }
        let start = self.utf16_len();
        self.document.text.push_str(value);
        self.utf16_length += value.encode_utf16().count();
        let end = self.utf16_len();
        if let Some(last) = self.document.runs.last_mut()
            && last.range.end() == start
            && last.style == style
        {
            last.range.length += end - start;
            return;
        }
        self.document.runs.push(StyledRun {
            range: TextRange::from_bounds(start, end),
            style,
        });
    }

    fn ensure_line_boundary(&mut self) {
        if !self.document.text.is_empty() && !self.document.text.ends_with('\n') {
            self.append("\n");
        }
    }

    fn begin_paragraph(&mut self, kind: ParagraphKind) {
        if self.active_paragraph.is_some() {
            return;
        }
        self.ensure_line_boundary();
        let start = self.utf16_len();
        // A quotation carries no marker in the text: the renderer draws a
        // rule down its left edge from `quote_depth`.
        self.active_paragraph = Some(ActiveParagraph {
            start,
            kind,
            quote_depth: self.quote_depth,
            list_depth: self.lists.len(),
        });
    }

    fn update_paragraph_kind(&mut self, kind: ParagraphKind) {
        if let Some(paragraph) = self.active_paragraph.as_mut() {
            paragraph.kind = kind;
        } else {
            self.begin_paragraph(kind);
        }
    }

    fn finish_paragraph(&mut self) {
        let Some(active) = self.active_paragraph.take() else {
            return;
        };
        // A paragraph nothing was written into leaves no line behind, so a
        // block parsed alone reads as it does inside the whole.
        if self.utf16_len() == active.start {
            return;
        }
        if !self.document.text.ends_with('\n') {
            self.append("\n");
        }
        let end = self.utf16_len();
        if end > active.start {
            self.document.paragraphs.push(Paragraph {
                range: TextRange::from_bounds(active.start, end),
                kind: active.kind,
                quote_depth: active.quote_depth,
                list_depth: active.list_depth,
            });
        }
    }

    fn start_list_item(&mut self) {
        self.finish_paragraph();
        self.begin_paragraph(ParagraphKind::Body);
        let depth = self.lists.len();
        let marker = if let Some(list) = self.lists.last_mut() {
            if let Some(number) = list.next_number.as_mut() {
                let marker = format!("{number}.\t");
                *number = number.saturating_add(1);
                marker
            } else if depth.is_multiple_of(2) {
                "◦\t".into()
            } else {
                "•\t".into()
            }
        } else {
            "•\t".into()
        };
        let mut style = self.style.current();
        style.bold = true;
        style.tone = TextTone::Secondary;
        self.append_with_style(&marker, style);
    }

    fn start_code_block(&mut self, kind: CodeBlockKind<'_>) {
        self.update_paragraph_kind(ParagraphKind::CodeBlock);
        self.style.code_block_depth += 1;
        let language = match kind {
            CodeBlockKind::Indented => String::new(),
            CodeBlockKind::Fenced(info) => info
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned(),
        };
        if !language.is_empty() {
            let mut style = self.style.current();
            style.bold = true;
            style.code_label = true;
            style.tone = TextTone::Secondary;
            self.append_with_style(&language, style);
            self.append("\n");
        }
        self.active_code_block = Some(ActiveCodeBlock {
            code_start: self.utf16_len(),
            language,
        });
    }

    fn finish_code_block(&mut self) {
        let Some(active) = self.active_code_block.take() else {
            return;
        };
        let code_end = self.utf16_len();
        self.style.code_block_depth = self.style.code_block_depth.saturating_sub(1);
        self.finish_paragraph();
        let Some(paragraph) = self.document.paragraphs.last() else {
            return;
        };
        self.document.code_blocks.push(CodeBlock {
            range: paragraph.range,
            code_range: TextRange::from_bounds(active.code_start, code_end),
            language: active.language,
        });
    }

    fn start_table(&mut self, alignments: Vec<Alignment>) {
        self.finish_paragraph();
        self.active_table = Some(ActiveTable {
            alignments,
            cells: Vec::new(),
            row: 0,
            column: 0,
            header: false,
        });
    }

    fn start_table_row(&mut self) {
        if let Some(table) = self.active_table.as_mut() {
            table.column = 0;
        }
    }

    fn start_table_cell(&mut self) {
        self.finish_paragraph();
        self.begin_paragraph(ParagraphKind::TableCell);
    }

    fn finish_table_cell(&mut self) {
        self.finish_paragraph();
        let Some(range) = self
            .document
            .paragraphs
            .last()
            .map(|paragraph| paragraph.range)
        else {
            return;
        };
        let Some(table) = self.active_table.as_mut() else {
            return;
        };
        let alignment = table
            .alignments
            .get(table.column)
            .copied()
            .unwrap_or(Alignment::None);
        table.cells.push(TableCell {
            range,
            row: table.row,
            column: table.column,
            header: table.header,
            alignment,
        });
        table.column += 1;
    }

    fn finish_table_row(&mut self) {
        self.finish_paragraph();
        if let Some(table) = self.active_table.as_mut() {
            table.row += 1;
            table.column = 0;
        }
    }

    fn finish_table(&mut self) {
        self.finish_paragraph();
        let Some(table) = self.active_table.take() else {
            return;
        };
        self.document.tables.push(MarkdownTable {
            columns: table.alignments.len(),
            cells: table.cells,
        });
    }

    fn start_image(&mut self, destination: String) {
        self.active_image = Some(ActiveImage {
            destination,
            alt: String::new(),
        });
    }

    fn append_image_alt(&mut self, value: &str) -> bool {
        let Some(image) = self.active_image.as_mut() else {
            return false;
        };
        image.alt.push_str(value);
        true
    }

    fn finish_image(&mut self) {
        let Some(image) = self.active_image.take() else {
            return;
        };
        let alt = image.alt.trim();
        let alt = if alt.is_empty() { "Image" } else { alt };
        let start = self.utf16_len();
        let mut style = self.style.current();
        style.italic = true;
        style.tone = TextTone::Accent;
        style.link = Some(image.destination.clone());
        self.append_with_style(&format!("▧\u{a0}{alt}"), style);
        let end = self.utf16_len();
        self.document.images.push(MarkdownImage {
            range: TextRange::from_bounds(start, end),
            destination: image.destination,
            alt: alt.to_owned(),
        });
    }

    fn finish(mut self) -> MarkdownDocument {
        self.finish_paragraph();
        self.document
    }
}

pub(crate) fn parse(markdown: &str) -> MarkdownDocument {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    let mut builder = DocumentBuilder::default();

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => builder.begin_paragraph(ParagraphKind::Body),
                Tag::Heading { level, .. } => {
                    let level = heading_level(level);
                    builder.style.heading = Some(level);
                    builder.begin_paragraph(ParagraphKind::Heading(level));
                }
                Tag::BlockQuote(_) => builder.quote_depth += 1,
                Tag::CodeBlock(kind) => builder.start_code_block(kind),
                Tag::HtmlBlock => {
                    builder.begin_paragraph(ParagraphKind::CodeBlock);
                    builder.style.code_depth += 1;
                }
                Tag::List(first) => builder.lists.push(ListState { next_number: first }),
                Tag::Item => builder.start_list_item(),
                Tag::FootnoteDefinition(label) => {
                    builder.begin_paragraph(ParagraphKind::Body);
                    builder.append(&format!("[{label}]\u{a0}"));
                }
                Tag::DefinitionList
                | Tag::DefinitionListTitle
                | Tag::DefinitionListDefinition
                | Tag::MetadataBlock(_) => {}
                Tag::Table(alignments) => builder.start_table(alignments),
                Tag::TableHead => {
                    builder.style.bold_depth += 1;
                    if let Some(table) = builder.active_table.as_mut() {
                        table.header = true;
                    }
                }
                Tag::TableRow => builder.start_table_row(),
                Tag::TableCell => builder.start_table_cell(),
                Tag::Emphasis => builder.style.italic_depth += 1,
                Tag::Strong => builder.style.bold_depth += 1,
                Tag::Strikethrough => builder.style.strike_depth += 1,
                Tag::Superscript | Tag::Subscript => {}
                Tag::Link { dest_url, .. } => builder.style.links.push(dest_url.into_string()),
                Tag::Image { dest_url, .. } => builder.start_image(dest_url.into_string()),
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => builder.finish_paragraph(),
                TagEnd::Heading(_) => {
                    builder.finish_paragraph();
                    builder.style.heading = None;
                }
                TagEnd::BlockQuote(_) => {
                    builder.finish_paragraph();
                    builder.quote_depth = builder.quote_depth.saturating_sub(1);
                }
                TagEnd::CodeBlock => builder.finish_code_block(),
                TagEnd::HtmlBlock => {
                    builder.finish_paragraph();
                    builder.style.code_depth = builder.style.code_depth.saturating_sub(1);
                }
                TagEnd::List(_) => {
                    builder.finish_paragraph();
                    builder.lists.pop();
                }
                TagEnd::Item => builder.finish_paragraph(),
                TagEnd::FootnoteDefinition => builder.finish_paragraph(),
                TagEnd::DefinitionList
                | TagEnd::DefinitionListTitle
                | TagEnd::DefinitionListDefinition
                | TagEnd::MetadataBlock(_) => {}
                TagEnd::Table => builder.finish_table(),
                TagEnd::TableHead => {
                    builder.finish_table_row();
                    builder.style.bold_depth = builder.style.bold_depth.saturating_sub(1);
                    if let Some(table) = builder.active_table.as_mut() {
                        table.header = false;
                    }
                }
                TagEnd::TableRow => builder.finish_table_row(),
                TagEnd::TableCell => builder.finish_table_cell(),
                TagEnd::Emphasis => {
                    builder.style.italic_depth = builder.style.italic_depth.saturating_sub(1);
                }
                TagEnd::Strong => {
                    builder.style.bold_depth = builder.style.bold_depth.saturating_sub(1);
                }
                TagEnd::Strikethrough => {
                    builder.style.strike_depth = builder.style.strike_depth.saturating_sub(1);
                }
                TagEnd::Superscript | TagEnd::Subscript => {}
                TagEnd::Link => {
                    builder.style.links.pop();
                }
                TagEnd::Image => builder.finish_image(),
            },
            Event::Text(text) => {
                if !builder.append_image_alt(&text) {
                    builder.append(&text);
                }
            }
            Event::Code(code) | Event::InlineMath(code) | Event::DisplayMath(code) => {
                if !builder.append_image_alt(&code) {
                    builder.style.code_depth += 1;
                    builder.append(&code);
                    builder.style.code_depth = builder.style.code_depth.saturating_sub(1);
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                builder.style.code_depth += 1;
                builder.append(&html);
                builder.style.code_depth = builder.style.code_depth.saturating_sub(1);
            }
            Event::FootnoteReference(label) => builder.append(&format!("[{label}]")),
            Event::SoftBreak => {
                if !builder.document.text.ends_with(char::is_whitespace) {
                    builder.append(" ");
                }
            }
            Event::HardBreak => builder.append("\n"),
            Event::Rule => {
                builder.finish_paragraph();
                builder.begin_paragraph(ParagraphKind::Rule);
                let mut style = builder.style.current();
                style.tone = TextTone::Separator;
                builder.append_with_style(NATIVE_RULE, style);
                builder.finish_paragraph();
            }
            Event::TaskListMarker(checked) => {
                builder.append(if checked { "☑︎\u{a0}" } else { "☐\u{a0}" });
            }
        }
    }

    builder.finish()
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

/// Where a markdown body can be cut into blocks that parse the same alone
/// as together: at a blank line outside a fence, unless what follows
/// continues the block before it (a list's next item or continuation, an
/// indented line, an HTML block). The ranges are contiguous and cover the
/// input, each block keeping the blank lines after it; a body still being
/// written keeps its last block open, since only the next line can say
/// where it ends.
pub(crate) fn split_blocks(markdown: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut block_start = 0usize;
    let mut shape: Option<BlockShape> = None;
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0usize;
    while offset < markdown.len() {
        let end = markdown[offset..]
            .find('\n')
            .map_or(markdown.len(), |newline| offset + newline + 1);
        let content = markdown[offset..end].trim_end_matches(['\n', '\r']);
        if let Some((marker, length)) = fence {
            if closes_fence(content, marker, length) {
                fence = None;
            }
            offset = end;
            continue;
        }
        if let Some(opened) = opens_fence(content) {
            fence = Some(opened);
            shape.get_or_insert(BlockShape::Other);
            offset = end;
            continue;
        }
        if content.trim().is_empty() {
            let Some(next) = next_content_line(markdown, end) else {
                break;
            };
            let next_line = markdown[next..].lines().next().unwrap_or("");
            let continues = match shape {
                Some(BlockShape::Html) => true,
                Some(BlockShape::List) => indent_of(next_line) >= 2 || is_list_item(next_line),
                Some(BlockShape::Other) | None => indent_of(next_line) >= 4,
            };
            if !continues && next > block_start {
                ranges.push(block_start..next);
                block_start = next;
                shape = None;
            }
            offset = end;
            continue;
        }
        shape.get_or_insert(shape_of(content));
        offset = end;
    }
    if block_start < markdown.len() || ranges.is_empty() {
        ranges.push(block_start..markdown.len());
    }
    ranges
}

/// Link reference definitions anywhere in the body — `[label]: url` — which
/// a link elsewhere may use, so a block rendered alone must see them too.
pub(crate) fn link_definitions(markdown: &str) -> Vec<&str> {
    if !markdown.contains("]:") {
        return Vec::new();
    }
    markdown
        .lines()
        .filter(|line| {
            let line = line.trim_start_matches(' ');
            let Some(rest) = line.strip_prefix('[') else {
                return false;
            };
            !rest.starts_with('^')
                && rest
                    .find("]:")
                    .is_some_and(|close| close > 0 && !rest[close + 2..].trim().is_empty())
        })
        .collect()
}

/// Whether the body ends inside a code fence that has not closed yet.
pub(crate) fn ends_in_open_fence(markdown: &str) -> bool {
    let mut fence: Option<(u8, usize)> = None;
    for line in markdown.lines() {
        match fence {
            Some((marker, length)) if closes_fence(line, marker, length) => fence = None,
            Some(_) => {}
            None => fence = opens_fence(line),
        }
    }
    fence.is_some()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockShape {
    List,
    Html,
    Other,
}

fn shape_of(line: &str) -> BlockShape {
    let trimmed = line.trim_start_matches(' ');
    let lower = trimmed.to_ascii_lowercase();
    if ["<pre", "<script", "<style", "<textarea", "<!--", "<?", "<![cdata["]
        .iter()
        .any(|opener| lower.starts_with(opener))
    {
        BlockShape::Html
    } else if is_list_item(line) {
        BlockShape::List
    } else {
        BlockShape::Other
    }
}

fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

fn is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some('-' | '+' | '*') => matches!(chars.next(), Some(' ' | '\t') | None),
        Some(digit) if digit.is_ascii_digit() => {
            let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
            let rest = &trimmed[digits..];
            digits <= 9
                && (rest.starts_with('.') || rest.starts_with(')'))
                && matches!(rest[1..].chars().next(), Some(' ' | '\t') | None)
        }
        _ => false,
    }
}

fn opens_fence(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = *trimmed.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = trimmed.bytes().take_while(|byte| *byte == marker).count();
    // An info string after a backtick fence may not contain a backtick.
    if length < 3 || (marker == b'`' && trimmed[length..].contains('`')) {
        return None;
    }
    Some((marker, length))
}

fn closes_fence(line: &str, marker: u8, length: usize) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let run = trimmed.bytes().take_while(|byte| *byte == marker).count();
    run >= length && trimmed[run..].trim().is_empty()
}

/// The byte offset of the first line at or after `from` with any content.
fn next_content_line(markdown: &str, mut from: usize) -> Option<usize> {
    while from < markdown.len() {
        let end = markdown[from..]
            .find('\n')
            .map_or(markdown.len(), |newline| from + newline + 1);
        if !markdown[from..end].trim().is_empty() {
            return Some(from);
        }
        from = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ParagraphKind, TextTone, ends_in_open_fence, link_definitions, parse, split_blocks};

    fn blocks(markdown: &str) -> Vec<&str> {
        let ranges = split_blocks(markdown);
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges.last().unwrap().end, markdown.len());
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "blocks must be contiguous");
        }
        ranges.into_iter().map(|range| &markdown[range]).collect()
    }

    #[test]
    fn blocks_cut_at_blank_lines_and_keep_fences_and_lists_whole() {
        assert_eq!(blocks("one\n\ntwo\n\nthree"), ["one\n\n", "two\n\n", "three"]);
        assert_eq!(blocks("```rust\nfn a() {}\n\nfn b() {}\n```\n\nafter"), ["```rust\nfn a() {}\n\nfn b() {}\n```\n\n", "after"]);
        assert_eq!(blocks("- a\n\n- b\n\n  more of b\n\npara"), ["- a\n\n- b\n\n  more of b\n\n", "para"]);
        assert_eq!(blocks("1. a\n\n2. b\n\n# heading"), ["1. a\n\n2. b\n\n", "# heading"]);
        assert_eq!(blocks("para\n\n    indented code\n\n    more"), ["para\n\n    indented code\n\n    more"]);
        assert_eq!(blocks("<pre>\nx\n\ny\n</pre>\n\nafter"), ["<pre>\nx\n\ny\n</pre>\n\nafter"]);
    }

    #[test]
    fn a_body_still_being_written_keeps_its_last_block_open() {
        assert_eq!(blocks("one\n\n"), ["one\n\n"]);
        assert_eq!(blocks("one\n\ntwo"), ["one\n\n", "two"]);
        assert_eq!(blocks("one\n\ntwo\n\n\n"), ["one\n\n", "two\n\n\n"]);
        assert_eq!(blocks(""), [""]);
        assert_eq!(blocks("```\nopen fence\n\nstill code"), ["```\nopen fence\n\nstill code"]);
    }

    #[test]
    fn an_open_fence_is_known() {
        assert!(ends_in_open_fence("text\n\n```rust\nfn a() {}"));
        assert!(!ends_in_open_fence("```\ncode\n```\n\nafter"));
        assert!(!ends_in_open_fence("plain"));
    }

    #[test]
    fn link_definitions_are_found_and_footnotes_are_not() {
        let body = "See [docs][d] and [^1].\n\n[d]: https://example.com\n[^1]: a footnote\n  [e]: https://e.example 'title'\nnot [a]: thing? yes it is\n";
        assert_eq!(link_definitions(body), ["[d]: https://example.com", "  [e]: https://e.example 'title'"]);
        assert!(link_definitions("no definitions here").is_empty());
    }

    #[test]
    fn blocks_parse_the_same_alone_as_together() {
        let sample = "## A repeatable review\n\nThe **same input** produces the same output. Use this stream to inspect wrapping, selection, and scrolling.\n\n- [x] Stable content\n- [ ] Review the next section\n\n> Keep the reader's place as new text arrives.\n\nInline `code` and [a reference](https://example.com) share the line.\n\n```rust\nfn total(values: &[u64]) -> u64 {\n    values.iter().sum()\n}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n1. first\n\n2. second\n   continued\n\n---\n\nText\n===\n\n* loose\n\n* list\n\n```\nfenced\n\nwith blank\n```\n\n    indented\n\n    code\n\n- item with fence\n\n  ```\n  inner\n\n  more\n  ```\n\nlast paragraph\n";
        let mut corpus = vec![sample.to_owned(), "single paragraph".to_owned(), String::new()];
        corpus.push(sample.repeat(3));
        // Every prefix of the sample, as a stream would present it.
        let mut cut = 0;
        while cut < sample.len() {
            cut += 7;
            while !sample.is_char_boundary(cut.min(sample.len())) {
                cut += 1;
            }
            corpus.push(sample[..cut.min(sample.len())].to_owned());
        }
        for body in corpus {
            let whole = parse(&body);
            let ranges = split_blocks(&body);
            let mut text = String::new();
            let mut paragraphs = Vec::new();
            let mut code_blocks = 0;
            for range in ranges {
                let part = parse(&body[range]);
                let shift = text.encode_utf16().count();
                text.push_str(&part.text);
                paragraphs.extend(part.paragraphs.iter().map(|paragraph| {
                    (paragraph.range.location + shift, paragraph.range.length, paragraph.kind, paragraph.quote_depth, paragraph.list_depth)
                }));
                code_blocks += part.code_blocks.iter().filter(|block| block.code_range.length > 0).count();
            }
            assert_eq!(text, whole.text, "text differs for {body:?}");
            let expected = whole
                .paragraphs
                .iter()
                .map(|paragraph| (paragraph.range.location, paragraph.range.length, paragraph.kind, paragraph.quote_depth, paragraph.list_depth))
                .collect::<Vec<_>>();
            assert_eq!(paragraphs, expected, "paragraphs differ for {body:?}");
            // An empty fence still being typed is bookkept differently alone;
            // it sets nothing either way.
            assert_eq!(code_blocks, whole.code_blocks.iter().filter(|block| block.code_range.length > 0).count());
        }
    }

    #[test]
    fn produces_semantic_runs_without_markdown_delimiters() {
        let document = parse("# Heading\n\nNormal **bold _and italic_** with `code`.");

        assert_eq!(
            document.text,
            "Heading\nNormal bold and italic with code.\n"
        );
        assert!(
            document
                .paragraphs
                .iter()
                .any(|paragraph| paragraph.kind == ParagraphKind::Heading(1))
        );
        let italic = document
            .runs
            .iter()
            .find(|run| {
                let start = document
                    .text
                    .char_indices()
                    .find(|(index, _)| *index == 20)
                    .map_or(0, |(index, _)| index);
                run.style.bold && run.style.italic && run.range.location <= start
            })
            .expect("nested emphasis should retain both styles");
        assert!(italic.style.bold && italic.style.italic);
        assert!(document.runs.iter().any(|run| run.style.code));
    }

    #[test]
    fn preserves_utf16_ranges_for_native_text_apis() {
        let document = parse("😀 **bold**");
        let bold = document
            .runs
            .iter()
            .find(|run| run.style.bold)
            .expect("bold run");

        assert_eq!(&document.text, "😀 bold\n");
        assert_eq!(bold.range.location, 3);
        assert_eq!(bold.range.length, 4);
    }

    #[test]
    fn preserves_lists_quotes_table_cells_tasks_links_images_and_rules() {
        let document = parse(
            "> quote\n\n1. first\n2. [x] second\n\n| A | B |\n| --- | --- |\n| x | y |\n\n[link](https://example.com)\n\n![diagram](https://example.com/diagram.png)\n\n---",
        );

        // A quotation keeps its text clean; its depth marks the paragraph.
        let quote = document
            .paragraphs
            .iter()
            .find(|paragraph| paragraph.quote_depth == 1)
            .expect("quoted paragraph");
        assert_eq!(&document.text[..quote.range.end()].trim_end(), &"quote");
        assert!(document.text.contains("1.\tfirst"));
        assert!(document.text.contains("2.\t☑︎\u{a0}second"));
        assert!(document.text.contains("A\nB\nx\ny\n"));
        assert_eq!(document.tables.len(), 1);
        assert_eq!(document.tables[0].columns, 2);
        assert_eq!(document.tables[0].cells.len(), 4);
        assert!(document.tables[0].cells[0].header);
        assert_eq!(document.tables[0].cells[3].row, 1);
        assert_eq!(document.tables[0].cells[3].column, 1);
        assert!(!document.text.contains("https://example.com"));
        assert!(
            document
                .runs
                .iter()
                .any(|run| { run.style.link.as_deref() == Some("https://example.com") })
        );
        assert!(
            document
                .runs
                .iter()
                .any(|run| run.style.tone == TextTone::Separator)
        );
        assert_eq!(document.images.len(), 1);
        assert_eq!(document.images[0].alt, "diagram");
        assert_eq!(
            document.images[0].destination,
            "https://example.com/diagram.png"
        );
        assert!(document.text.contains("▧\u{a0}diagram"));
    }

    #[test]
    fn preserves_fenced_code_language_and_source_range() {
        let document = parse("```rust\nfn main() {}\n```\n");
        let block = document.code_blocks.first().expect("fenced code block");

        assert_eq!(block.language, "rust");
        assert_eq!(document.text, "rust\nfn main() {}\n");
        let code_start = document
            .text
            .find("fn main")
            .expect("code starts after language label");
        assert_eq!(block.code_range.location, code_start);
        assert_eq!(block.code_range.length, "fn main() {}\n".len());
    }

    #[test]
    fn leaves_html_visible_and_never_interprets_it() {
        let document = parse("before <span>unsafe</span> after");

        assert_eq!(document.text, "before <span>unsafe</span> after\n");
        assert!(document.runs.iter().any(|run| run.style.code));
    }
}
