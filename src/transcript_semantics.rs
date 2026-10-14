//! What the transcript displays for one message, read back from its views
//! in the renderer-neutral form the conformance dump writes
//! (`docs/MARKDOWN_CONFORMANCE.md`). This is the renderer-specific half of
//! `--markdown-dump`: the fields come from what the layout shows (the
//! renderer's display map applied), not from its parser.
use objc2::{msg_send, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSAttachmentAttributeName, NSFont, NSFontAttributeName, NSFontDescriptorSymbolicTraits,
    NSFontTraitsAttribute, NSFontWeightSemibold, NSFontWeightTrait, NSLinkAttributeName,
    NSTextAttachment, NSTextLayoutFragment, NSTextLayoutFragmentEnumerationOptions, NSTextParagraph,
    NSTextSelectionDataSource,
};
use upleft_render::view::markdown_text_view::MarkdownTextView;
use objc2_foundation::{NSAttributedStringKey, NSNumber, NSRange, NSRect, NSString, NSURL};
use serde_json::json;

use crate::markdown_bench::Displayed;

/// An attachment at most this tall, in points, is an inline icon (a symbol,
/// a checkbox); anything taller is an image.
const ICON_MAX_HEIGHT: f64 = 24.0;

/// One message read back from the new transcript's views, piece by piece:
/// its labels as set, and from each hosted Markdown view what its layout
/// shows. That is the text of the paragraphs TextKit lays out (the
/// renderer's display map already applied: hidden markers and definitions
/// are gone, substitutions are in), with their links, fonts and inline
/// attachments; a diagram, display formula or image block is one attachment
/// (U+FFFC), as the old transcript's images were.
#[derive(Default)]
pub(crate) struct Collected {
    units: Vec<u16>,
    links: Vec<(usize, usize, String)>,
    attachments: Vec<serde_json::Value>,
    mono: Vec<bool>,
    bold: Vec<bool>,
    italic: Vec<bool>,
}

impl Collected {
    pub(crate) fn push_label(&mut self, text: &str, font: &NSFont) {
        let (mono, bold, italic) = font_traits(font);
        for unit in text.encode_utf16() {
            self.units.push(unit);
            self.mono.push(mono);
            self.bold.push(bold);
            self.italic.push(italic);
        }
    }

    fn push_object(&mut self, kind: &str, width: f64, height: f64) {
        self.attachments.push(json!({
            "kind": kind,
            "offset": self.units.len(),
            "width_pt": width.round(),
            "height_pt": height.round(),
        }));
        for unit in "\u{fffc}\n".encode_utf16() {
            self.units.push(unit);
            self.mono.push(false);
            self.bold.push(false);
            self.italic.push(false);
        }
    }

    fn push_attributed(&mut self, text: &objc2_foundation::NSAttributedString) {
        let base = self.units.len();
        let string = text.string().to_string();
        let units: Vec<u16> = string.encode_utf16().collect();
        let length = units.len();
        self.units.extend_from_slice(&units);
        self.mono.resize(base + length, false);
        self.bold.resize(base + length, false);
        self.italic.resize(base + length, false);
        let limit = NSRange::new(0, length);
        for (from, to, value) in attributed_runs(text, unsafe { NSLinkAttributeName }, limit) {
            let Some(value) = value else { continue };
            let url = if let Some(url) = value.downcast_ref::<NSURL>() {
                url.absoluteString().map(|s| s.to_string()).unwrap_or_default()
            } else if let Some(string) = value.downcast_ref::<NSString>() {
                string.to_string()
            } else {
                continue;
            };
            match self.links.last_mut() {
                Some(last) if last.1 == base + from && last.2 == url => last.1 = base + to,
                _ => self.links.push((base + from, base + to, url)),
            }
        }
        for (from, _to, value) in attributed_runs(text, unsafe { NSAttachmentAttributeName }, limit) {
            let Some(attachment) = value.and_then(|value| value.downcast::<NSTextAttachment>().ok()) else { continue };
            let size = attachment.bounds().size;
            let size = if size.height > 0.0 { size } else { attachment.image().map(|image| image.size()).unwrap_or(size) };
            let kind = if size.height > ICON_MAX_HEIGHT { "image" } else { "icon" };
            self.attachments.push(json!({
                "kind": kind,
                "offset": base + from,
                "width_pt": size.width.round(),
                "height_pt": size.height.round(),
            }));
        }
        for (from, to, value) in attributed_runs(text, unsafe { NSFontAttributeName }, limit) {
            let Some(font) = value.and_then(|value| value.downcast::<NSFont>().ok()) else { continue };
            let (mono, bold, italic) = font_traits(&font);
            self.mono[base + from..base + to].fill(mono);
            self.bold[base + from..base + to].fill(bold);
            self.italic[base + from..base + to].fill(italic);
        }
    }

    /// What a hosted view lays out, fragment by fragment.
    pub(crate) fn push_hosted(&mut self, view: &MarkdownTextView) {
        let Some(layout) = view.textLayoutManager() else { return };
        let start = layout.documentRange().location();
        let fragments: std::cell::RefCell<Vec<(String, NSRect, Option<Retained<objc2_foundation::NSAttributedString>>)>> =
            std::cell::RefCell::new(Vec::new());
        let block = block2::RcBlock::new(|fragment: std::ptr::NonNull<NSTextLayoutFragment>| -> objc2::runtime::Bool {
            // SAFETY: TextKit hands the block a live fragment for the call.
            let fragment = unsafe { fragment.as_ref() };
            let class = fragment.class().name().to_string_lossy().into_owned();
            let text = fragment
                .textElement()
                .and_then(|element| element.downcast::<NSTextParagraph>().ok())
                .map(|paragraph| paragraph.attributedString());
            fragments.borrow_mut().push((class, fragment.layoutFragmentFrame(), text));
            objc2::runtime::Bool::YES
        });
        layout.enumerateTextLayoutFragmentsFromLocation_options_usingBlock(
            Some(&start),
            NSTextLayoutFragmentEnumerationOptions::EnsuresLayout,
            &block,
        );
        drop(block);
        let width = view.hosted_width().unwrap_or(0.0);
        for (class, frame, text) in fragments.into_inner() {
            let plain = text.as_ref().map(|text| visible(&text.string().to_string())).unwrap_or_default();
            match class.as_str() {
                "ElidedFragment" | "ElisionCueFragment" => {}
                // A block's later paragraphs lay out at no height.
                "MermaidFragment" | "MathFragment" | "ImageFragment" if frame.size.height < 1.0 => {}
                "MermaidFragment" => self.push_object("diagram", width, frame.size.height),
                "MathFragment" => self.push_object("math", width, frame.size.height),
                "ImageFragment" => {
                    self.push_object("image", width, frame.size.height);
                    // The alt text, drawn as the caption.
                    if let Some(alt) = plain.strip_prefix("![").and_then(|rest| rest.split("](").next())
                        && !alt.is_empty()
                    {
                        self.push_plain(&format!("{alt}\n"), false, false, false);
                    }
                }
                // The fences are not drawn; the opening one's language is,
                // as a label.
                "CodeBlockFragment" if plain.trim_start().starts_with("```") || plain.trim_start().starts_with("~~~") => {
                    let language = plain.trim().trim_start_matches(['`', '~']).trim();
                    if !language.is_empty() {
                        self.push_plain(&format!("{language}\n"), false, false, false);
                    }
                }
                // The card shows the fields, not the fences.
                "FrontMatterFragment" if plain.trim() == "---" => {}
                // A row draws its cells from the source row.
                "TableRowFragment" => self.push_table_row(&plain),
                _ => {
                    if let Some(text) = text {
                        self.push_attributed(&text);
                    }
                }
            }
        }
    }

    fn push_plain(&mut self, text: &str, mono: bool, bold: bool, italic: bool) {
        for unit in text.encode_utf16() {
            self.units.push(unit);
            self.mono.push(mono);
            self.bold.push(bold);
            self.italic.push(italic);
        }
    }

    /// A table row as its cells show it: separated by tabs, inline markers
    /// taken as the styles they mark.
    fn push_table_row(&mut self, row: &str) {
        let row = row.trim();
        let row = row.strip_prefix('|').unwrap_or(row);
        let row = row.strip_suffix('|').unwrap_or(row);
        let mut cells = Vec::new();
        let (mut cell, mut code) = (String::new(), false);
        let mut chars = row.chars().peekable();
        while let Some(char) = chars.next() {
            match char {
                '\\' if chars.peek() == Some(&'|') => cell.push(chars.next().expect("peeked")),
                '`' => {
                    code = !code;
                    cell.push(char);
                }
                '|' if !code => cells.push(std::mem::take(&mut cell)),
                _ => cell.push(char),
            }
        }
        cells.push(cell);
        for (index, cell) in cells.iter().enumerate() {
            if index > 0 {
                self.push_plain("\t", false, false, false);
            }
            let (mut code, mut bold, mut italic) = (false, false, false);
            let mut chars = cell.trim().chars().peekable();
            while let Some(char) = chars.next() {
                match char {
                    '`' => code = !code,
                    '*' | '_' if !code && chars.peek() == Some(&char) => {
                        chars.next();
                        bold = !bold;
                    }
                    '*' | '_' if !code => italic = !italic,
                    _ => self.push_plain(&char.to_string(), code, bold, italic),
                }
            }
        }
        self.push_plain("\n", false, false, false);
    }

    pub(crate) fn finish(mut self, rect: NSRect) -> Displayed {
        // Zero-width joiners stand where hidden markers were: laid out, but
        // nothing a reader sees.
        let mut map = Vec::with_capacity(self.units.len() + 1);
        let mut kept = 0;
        for unit in &self.units {
            map.push(kept);
            if !is_zero_width(*unit) {
                kept += 1;
            }
        }
        map.push(kept);
        let keep = |units: &[u16], flags: &[bool]| -> Vec<bool> {
            units.iter().zip(flags).filter(|(unit, _)| !is_zero_width(**unit)).map(|(_, flag)| *flag).collect()
        };
        self.mono = keep(&self.units, &self.mono);
        self.bold = keep(&self.units, &self.bold);
        self.italic = keep(&self.units, &self.italic);
        self.units.retain(|unit| !is_zero_width(*unit));
        for link in &mut self.links {
            link.0 = map[link.0];
            link.1 = map[link.1];
        }
        for attachment in &mut self.attachments {
            if let Some(offset) = attachment["offset"].as_u64() {
                attachment["offset"] = json!(map[offset as usize]);
            }
        }
        let text_of = |from: usize, to: usize| String::from_utf16_lossy(&self.units[from..to]);
        let links = self
            .links
            .iter()
            .map(|(from, to, url)| json!({"text": text_of(*from, *to), "url": url}))
            .collect::<Vec<_>>();
        let semantics = json!({
            "visible_text": text_of(0, self.units.len()),
            "links": links,
            "attachments": {"count": self.attachments.len(), "items": self.attachments},
            "code_runs": paragraph_runs(&self.units, &self.mono),
            "bold_runs": paragraph_runs(&self.units, &self.bold),
            "italic_runs": paragraph_runs(&self.units, &self.italic),
            "height_pt": rect.size.height,
        });
        Displayed { rect, semantics }
    }
}

fn is_zero_width(unit: u16) -> bool {
    matches!(unit, 0x200B | 0x200C | 0x200D | 0x2060 | 0xFEFF)
}

/// `text` without its zero-width joiners.
fn visible(text: &str) -> String {
    text.chars().filter(|char| !(*char as u32 <= 0xFFFF && is_zero_width(*char as u16))).collect()
}

/// Every run of `name` in `text` inside `limit`, as offsets from its start.
fn attributed_runs(
    text: &objc2_foundation::NSAttributedString,
    name: &NSAttributedStringKey,
    limit: NSRange,
) -> Vec<(usize, usize, Option<Retained<AnyObject>>)> {
    let end = limit.location + limit.length;
    let mut at = limit.location;
    let mut runs = Vec::new();
    while at < end {
        let mut effective = NSRange::new(at, 0);
        // SAFETY: `effective` is a valid out-pointer and `at` lies in the text.
        let value = unsafe { text.attribute_atIndex_longestEffectiveRange_inRange(name, at, &mut effective, limit) };
        let run_end = (effective.location + effective.length).clamp(at + 1, end);
        runs.push((at - limit.location, run_end - limit.location, value));
        at = run_end;
    }
    runs
}

/// Monospaced, bold (semibold or heavier), and italic, from the font itself.
fn font_traits(font: &NSFont) -> (bool, bool, bool) {
    let descriptor = font.fontDescriptor();
    let symbolic = descriptor.symbolicTraits();
    // SAFETY: the traits attribute is a dictionary of NSNumbers by contract.
    let weight = unsafe {
        descriptor
            .objectForKey(NSFontTraitsAttribute)
            .and_then(|traits| {
                let weight: Option<Retained<NSNumber>> =
                    msg_send![&*traits, objectForKey: NSFontWeightTrait];
                weight
            })
            .map_or(0.0, |weight| weight.doubleValue())
    };
    let mono = font.isFixedPitch() || symbolic.contains(NSFontDescriptorSymbolicTraits::TraitMonoSpace);
    let bold = symbolic.contains(NSFontDescriptorSymbolicTraits::TraitBold)
        // SAFETY: an AppKit constant.
        || weight >= unsafe { NSFontWeightSemibold } - 0.01;
    let italic = symbolic.contains(NSFontDescriptorSymbolicTraits::TraitItalic);
    (mono, bold, italic)
}

/// Maximal runs of flagged text, cut at paragraph separators; runs of only
/// whitespace are dropped.
fn paragraph_runs(units: &[u16], flagged: &[bool]) -> Vec<String> {
    let separator = |unit: u16| matches!(unit, 0x0A | 0x0D | 0x2028 | 0x2029);
    let mut runs = Vec::new();
    let mut current: Vec<u16> = Vec::new();
    let mut flush = |current: &mut Vec<u16>| {
        let text = String::from_utf16_lossy(current);
        if !text.trim().is_empty() {
            runs.push(text);
        }
        current.clear();
    };
    for (unit, flag) in units.iter().zip(flagged) {
        if *flag && !separator(*unit) {
            current.push(*unit);
        } else {
            flush(&mut current);
        }
    }
    flush(&mut current);
    runs
}

#[cfg(test)]
mod tests {
    use super::{Collected, paragraph_runs};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    #[test]
    fn a_table_row_reads_as_its_cells_and_joiners_are_not_text() {
        let mut collected = Collected::default();
        collected.push_plain("A\u{2060}\u{2060}b\n", false, false, false);
        collected.push_table_row("| Decorate | `render` | **Bold** and *it* |");
        let displayed = collected.finish(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 10.0)));
        let semantics = displayed.semantics;
        assert_eq!(semantics["visible_text"], "Ab\nDecorate\trender\tBold and it\n");
        assert_eq!(semantics["code_runs"], serde_json::json!(["render"]));
        assert_eq!(semantics["bold_runs"], serde_json::json!(["Bold"]));
        assert_eq!(semantics["italic_runs"], serde_json::json!(["it"]));
    }

    #[test]
    fn runs_merge_within_a_paragraph_and_split_at_separators() {
        let units: Vec<u16> = "ab cd\nef g".encode_utf16().collect();
        let flags = [true, true, true, true, false, false, true, true, true, true];
        assert_eq!(paragraph_runs(&units, &flags), ["ab c", "ef g"]);
        // Whitespace alone is no run.
        let units: Vec<u16> = "a  b".encode_utf16().collect();
        assert_eq!(paragraph_runs(&units, &[false, true, true, false]), Vec::<String>::new());
    }
}
