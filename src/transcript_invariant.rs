//! The safe-cut invariant. A long message is shown as several hosted views
//! (`transcript_parse`), and the cut between two of them must not show: a
//! view of a segment must present its text exactly as one view of the whole
//! message presents the same text. Everything the renderer reads is compared:
//! the block tree (kinds, ranges after the shift, markers, list tightness,
//! depths), inlines with their resolved references, safe-HTML annotations
//! (whose `<details>` pairing reaches across blocks), the definitions Read
//! mode hides, and the heading contexts decoration keys on the document: the
//! message's first heading gives up the space above it, and a heading right
//! after another takes less. A segment is parsed with what the rest of the
//! message gives it (`SegmentContext`), and must still match.

use std::fmt::Write;

use upleft_core::safe_html::SafeHTMLDocument;
use upleft_core::{BlockContent, BlockRef, InlineKind, InlineSpan, NSRange, ParsedDocument, swift_text};

/// The top-level blocks of `document` from `start` to `end` (UTF-16), as a
/// view presents them. `continues` is the view's hosted continuation flag:
/// a heading of the message comes before this part, so its first heading is
/// not the message's first.
pub(crate) struct Part<'a> {
    pub(crate) document: &'a ParsedDocument,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) continues: bool,
}

/// Whether a view of `piece` (a segment, `piece.start == 0`) presents its
/// text as the view of the whole message presents `whole`'s part. `Err`
/// says where not.
pub(crate) fn verify(whole: &Part, piece: &Part) -> Result<(), String> {
    debug_assert_eq!(piece.start, 0);
    if whole.end - whole.start != piece.end || piece.end != piece.document.utf16.len() {
        return Err(format!("lengths {}, {} and {}", whole.end - whole.start, piece.end, piece.document.utf16.len()));
    }
    let a = describe(whole);
    let b = describe(piece);
    if a == b {
        return Ok(());
    }
    let at = a.iter().zip(&b).position(|(a, b)| a != b).unwrap_or(a.len().min(b.len()));
    Err(format!(
        "differs at item {at}: whole {:?} / segment {:?}",
        a.get(at).map(String::as_str).unwrap_or("(end)"),
        b.get(at).map(String::as_str).unwrap_or("(end)")
    ))
}

/// Whether a heading of `document` starts before `offset`.
pub(crate) fn heading_before(document: &ParsedDocument, offset: usize) -> bool {
    document.headings.first().is_some_and(|heading| (heading.range.location.max(0) as usize) < offset)
}

/// The message's text up to `end` ends with a heading, whitespace aside, at
/// any depth: a heading after it would be one that follows another.
pub(crate) fn ends_with_heading(document: &ParsedDocument, end: usize) -> bool {
    let Some(last) = document.headings.iter().rev().find(|heading| (heading.range.location.max(0) as usize) < end) else {
        return false;
    };
    let from = (last.range.upper_bound().max(0) as usize).min(end);
    document.utf16[from..end].iter().all(|&unit| matches!(unit, 0x09..=0x0D | 0x20))
}

/// What a view presents of `part`, as comparable lines: ranges relative to
/// the part's start.
pub(crate) fn describe(part: &Part) -> Vec<String> {
    let document = part.document;
    let (start, end) = (part.start as isize, part.end as isize);
    let within = |range: NSRange| range.location >= start && range.location < end;
    let mut out = Vec::new();
    for block in document.root.children.iter().filter(|block| within(block.range)) {
        let mut line = String::new();
        block_line(block, start, &mut line);
        out.push(line);
    }
    // Read mode hides definitions, by their ranges.
    let mut hidden: Vec<String> = document
        .link_references
        .iter()
        .filter(|(_, reference)| within(reference.range))
        .map(|(label, reference)| format!("definition {label:?} {}", span(reference.range, start)))
        .chain(
            document
                .footnotes
                .iter()
                .filter(|(_, block)| within(block.range))
                .map(|(label, block)| format!("footnote {label:?} {}", span(block.range, start))),
        )
        .collect();
    hidden.sort();
    out.extend(hidden);
    // The heading contexts decoration reads from the document.
    for (index, heading) in document.headings.iter().enumerate() {
        if !within(heading.range) {
            continue;
        }
        let first = !part.continues && index == 0;
        out.push(format!("heading {} first {first} follows {}", span(heading.range, start), follows_another_heading(document, index)));
    }
    out
}

/// `follows_another_heading` of the decoration engine: nothing but
/// whitespace between this heading and the one before.
fn follows_another_heading(document: &ParsedDocument, index: usize) -> bool {
    if index == 0 {
        return false;
    }
    let previous = &document.headings[index - 1];
    let heading = &document.headings[index];
    let between =
        NSRange::new(previous.range.upper_bound(), 0.max(heading.range.location - previous.range.upper_bound()));
    if !(between.length > 0 && between.upper_bound() <= document.length) {
        return true;
    }
    swift_text::trim_whitespaces_and_newlines(&document.substring(between)).is_empty()
}

fn span(range: NSRange, base: isize) -> String {
    format!("{}+{}", range.location - base, range.length)
}

fn optional(range: Option<NSRange>, base: isize) -> String {
    range.map_or_else(|| "-".to_owned(), |range| span(range, base))
}

fn kind_name(content: &BlockContent) -> &'static str {
    match content {
        BlockContent::Document => "document",
        BlockContent::Heading { .. } => "heading",
        BlockContent::Paragraph => "paragraph",
        BlockContent::BlockQuote => "quote",
        BlockContent::Callout { .. } => "callout",
        BlockContent::List { .. } => "list",
        BlockContent::ListItem { .. } => "item",
        BlockContent::CodeBlock { .. } => "code block",
        BlockContent::Mermaid { .. } => "diagram",
        BlockContent::MathBlock { .. } => "formula",
        BlockContent::Table(_) => "table",
        BlockContent::ThematicBreak => "break",
        BlockContent::HtmlBlock => "HTML block",
        BlockContent::FrontMatter(_) => "front matter",
        BlockContent::FootnoteDefinition { .. } => "footnote",
    }
}

fn block_line(block: &BlockRef, base: isize, out: &mut String) {
    let _ = write!(out, "[{} ", kind_name(&block.content));
    match &block.content {
        BlockContent::Heading { level } => {
            let _ = write!(out, "{level} ");
        }
        BlockContent::Callout { kind, title } => {
            let _ = write!(out, "{kind:?} {title:?} ");
        }
        BlockContent::List { ordered, start, tight, marker } => {
            let _ = write!(out, "{ordered} {start} {tight} {marker:?} ");
        }
        BlockContent::ListItem { ordinal, checkbox } => {
            let _ = write!(out, "{ordinal:?} ");
            if let Some(checkbox) = checkbox {
                let _ = write!(out, "box {} {} ", checkbox.is_checked, span(checkbox.mark_range, base));
            }
        }
        BlockContent::CodeBlock { language, is_fenced, content_range } => {
            let _ = write!(out, "{language:?} {is_fenced} {} ", span(*content_range, base));
        }
        BlockContent::Mermaid { source_range } => {
            let _ = write!(out, "{} ", span(*source_range, base));
        }
        BlockContent::MathBlock { latex_range } => {
            let _ = write!(out, "{} ", span(*latex_range, base));
        }
        BlockContent::Table(table) => {
            let _ = write!(out, "{:?} {} ", table.alignments, span(table.delimiter_range, base));
            for row in &table.rows {
                let _ = write!(out, "(row {} {} ", row.is_header, span(row.range, base));
                for cell in &row.cells {
                    let _ = write!(out, "(cell {} {} ", span(cell.range, base), span(cell.content_range, base));
                    inlines(&cell.inlines, base, out);
                    out.push(')');
                }
                out.push(')');
            }
        }
        BlockContent::FrontMatter(front) => {
            let _ = write!(out, "{} {} ", span(front.range, base), span(front.body_range, base));
            for field in &front.fields {
                let _ =
                    write!(out, "({:?} {:?} {} {}) ", field.key, field.value, span(field.key_range, base), span(field.value_range, base));
            }
        }
        BlockContent::FootnoteDefinition { identifier } => {
            let _ = write!(out, "{identifier:?} ");
        }
        BlockContent::Document
        | BlockContent::Paragraph
        | BlockContent::BlockQuote
        | BlockContent::ThematicBreak
        | BlockContent::HtmlBlock => {}
    }
    let _ = write!(
        out,
        "{} {} {} {} d{} q{} ",
        span(block.range, base),
        span(block.content_range, base),
        optional(block.marker_range, base),
        optional(block.trailing_marker_range, base),
        block.depth,
        block.quote_depth
    );
    if let Some(html) = &block.safe_html {
        safe_html(html, base, out);
    }
    inlines(&block.inlines, base, out);
    for child in &block.children {
        block_line(child, base, out);
    }
    out.push(']');
}

fn safe_html(html: &SafeHTMLDocument, base: isize, out: &mut String) {
    let _ = write!(out, "(html {} {} ", span(html.range, base), html.is_safe);
    for annotation in &html.annotations {
        let _ = write!(out, "({:?} {} {}", annotation.kind, span(annotation.range, base), span(annotation.content_range, base));
        for tag in &annotation.tag_ranges {
            let _ = write!(out, " {}", span(*tag, base));
        }
        out.push(')');
    }
    out.push_str(") ");
}

fn inlines(spans: &[InlineSpan], base: isize, out: &mut String) {
    for inline in spans {
        out.push('{');
        match &inline.kind {
            InlineKind::InlineMath { latex_range } => {
                let _ = write!(out, "math {}", span(*latex_range, base));
            }
            kind => {
                let _ = write!(out, "{kind:?}");
            }
        }
        let _ = write!(
            out,
            " {} {} {} {}",
            span(inline.range, base),
            span(inline.content_range, base),
            optional(inline.leading_marker_range, base),
            optional(inline.trailing_marker_range, base)
        );
        inlines(&inline.children, base, out);
        out.push('}');
    }
}
