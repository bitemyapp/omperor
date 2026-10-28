//! Math, set as KaTeX sets it: RaTeX lays the LaTeX out in KaTeX's fonts,
//! and each glyph becomes its outline (skrifa), so a formula is a picture
//! of filled shapes, in ems, that the transcript draws at the text's size
//! and in its colour. Setting is a worker's job, done as an answer is
//! parsed. Formulas set once are kept, so an answer streaming in doesn't
//! set its formulas again with each word.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use aizome_kit::sidestep_ui::kurbo::{Affine, BezPath, Point, Rect, Shape as _};
use aizome_kit::sidestep_ui::{Canvas, Font, StrokeStyle, TextLayout, TextStyle};
use aizome_kit::Color;
use ratex_types::display_item::DisplayItem;
use ratex_types::path_command::PathCommand;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider};

/// A formula: its LaTeX, and how it's set.
pub(crate) struct Formula {
    pub(crate) source: String,
    /// Set on its own (`$$…$$`), rather than in a line of text.
    pub(crate) display: bool,
    /// The picture, or what's wrong with the LaTeX.
    pub(crate) set: Result<Picture, String>,
    /// Tells it from other formulas: its source and how it's set.
    pub(crate) id: u64,
}

impl std::fmt::Debug for Formula {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Formula").field("source", &self.source).field("display", &self.display).finish()
    }
}

impl PartialEq for Formula {
    fn eq(&self, other: &Formula) -> bool {
        self.id == other.id
    }
}

/// A set formula, in ems from its baseline's left end, y down.
pub(crate) struct Picture {
    pub(crate) width: f64,
    /// How far it reaches above the baseline and below it.
    pub(crate) ascent: f64,
    pub(crate) descent: f64,
    shapes: Vec<Shape>,
}

enum Shape {
    /// Glyphs, rules and a radical's sign: in the text's colour, or one
    /// of their own (`\color`).
    Fill(BezPath, Option<Color>),
    /// A line drawn rather than filled (`\cancel`), this many ems wide.
    Stroke(BezPath, f64, Option<Color>),
    /// A character KaTeX's fonts don't have, set in the system's serif:
    /// its baseline's left end, and its size in ems.
    Text { text: String, at: Point, size: f64, color: Option<Color> },
}

/// What a worker made, kept for the next to find.
type Kept<K, V> = OnceLock<Mutex<HashMap<K, V>>>;

/// The colour formulas are laid out in: shapes in it take the text's.
const TEXT_COLOR: ratex_types::color::Color = ratex_types::color::Color::new(0.123, 0.456, 0.789, 1.0);

/// Formulas already set, by source and whether set on their own: an answer
/// is parsed again and again as it streams in.
static SET: Kept<(String, bool), Arc<Formula>> = OnceLock::new();
const KEPT: usize = 512;

/// `source` set (`display`: on its own), or as it was set before.
pub(crate) fn set(source: &str, display: bool) -> Arc<Formula> {
    let source = source.trim();
    let kept = SET.get_or_init(Mutex::default);
    let key = (source.to_owned(), display);
    if let Some(formula) = kept.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return formula.clone();
    }
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    let formula = Arc::new(Formula { source: key.0.clone(), display, set: lay_out(source, display), id: h.finish() });
    let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
    if kept.len() >= KEPT {
        kept.clear();
    }
    kept.insert(key, formula.clone());
    formula
}

fn lay_out(source: &str, display: bool) -> Result<Picture, String> {
    let nodes = ratex_parser::parser::parse(source).map_err(|e| e.message)?;
    let style = if display { ratex_types::math_style::MathStyle::Display } else { ratex_types::math_style::MathStyle::Text };
    let options = ratex_layout::LayoutOptions { style, color: TEXT_COLOR, ..Default::default() };
    let laid = ratex_layout::layout(&nodes, &options);
    ratex_layout::validate_layout(&laid)?;
    let list = ratex_layout::to_display_list(&laid);
    // The list's y runs down from its top; the picture's from its baseline.
    let top = list.height;
    let own = |c: &ratex_types::color::Color| (*c != TEXT_COLOR).then(|| Color::rgba(c.r, c.g, c.b, c.a));
    let mut shapes = Vec::with_capacity(list.items.len());
    for item in &list.items {
        match item {
            DisplayItem::GlyphPath { x, y, scale, font, char_code, color } => match glyph(font, *char_code) {
                Some(outline) => {
                    let place = Affine::translate((*x, y - top)) * Affine::scale(*scale);
                    shapes.push(Shape::Fill(place * &*outline, own(color)));
                }
                None => {
                    if let Some(c) = char::from_u32(*char_code).filter(|c| !c.is_whitespace()) {
                        let text = c.to_string();
                        shapes.push(Shape::Text { text, at: Point::new(*x, y - top), size: *scale, color: own(color) });
                    }
                }
            },
            DisplayItem::Line { x, y, width, thickness, color, dashed } => {
                // A rule's y is its middle.
                let (y0, y1) = (y - top - thickness / 2.0, y - top + thickness / 2.0);
                let dash = if *dashed { 4.0 * thickness } else { *width };
                let mut at = *x;
                while at < x + width {
                    let end = (at + dash).min(x + width);
                    shapes.push(Shape::Fill(Rect::new(at, y0, end, y1).to_path(0.01), own(color)));
                    at = end + dash;
                }
            }
            DisplayItem::Rect { x, y, width, height, color } => {
                shapes.push(Shape::Fill(Rect::new(*x, y - top, x + width, y - top + height).to_path(0.01), own(color)));
            }
            DisplayItem::Path { x, y, commands, fill, color } => {
                let place = Affine::translate((*x, y - top));
                if *fill {
                    // Each piece on its own: KaTeX's stretched arrows are
                    // pieces wound opposite ways, which filled as one
                    // would cancel out.
                    let starts: Vec<usize> = (0..commands.len())
                        .filter(|&i| i == 0 || matches!(commands[i], PathCommand::MoveTo { .. }))
                        .chain([commands.len()])
                        .collect();
                    for piece in starts.windows(2) {
                        shapes.push(Shape::Fill(place * path(&commands[piece[0]..piece[1]]), own(color)));
                    }
                } else {
                    shapes.push(Shape::Stroke(place * path(commands), 0.05, own(color)));
                }
            }
        }
    }
    Ok(Picture { width: list.width, ascent: list.height, descent: list.depth, shapes })
}

fn path(commands: &[PathCommand]) -> BezPath {
    let mut p = BezPath::new();
    for c in commands {
        match *c {
            PathCommand::MoveTo { x, y } => p.move_to((x, y)),
            PathCommand::LineTo { x, y } => p.line_to((x, y)),
            PathCommand::QuadTo { x1, y1, x, y } => p.quad_to((x1, y1), (x, y)),
            PathCommand::CubicTo { x1, y1, x2, y2, x, y } => p.curve_to((x1, y1), (x2, y2), (x, y)),
            PathCommand::Close => p.close_path(),
        }
    }
    p
}

/// Glyph outlines, by KaTeX font and character, in ems from the glyph's
/// origin, y down; none where the font hasn't the character.
static GLYPHS: Kept<(String, u32), Option<Arc<BezPath>>> = OnceLock::new();

fn glyph(font: &str, code: u32) -> Option<Arc<BezPath>> {
    let glyphs = GLYPHS.get_or_init(Mutex::default);
    let key = (font.to_owned(), code);
    if let Some(found) = glyphs.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return found.clone();
    }
    let found = outline(font, code).map(Arc::new);
    glyphs.lock().unwrap_or_else(|e| e.into_inner()).insert(key, found.clone());
    found
}

fn outline(font: &str, code: u32) -> Option<BezPath> {
    let bytes = ratex_katex_fonts::ttf_bytes(&format!("KaTeX_{font}.ttf"))?;
    let face = FontRef::new(&bytes).ok()?;
    let id = face.charmap().map(code)?;
    if id.to_u32() == 0 {
        return None;
    }
    let glyph = face.outline_glyphs().get(id)?;
    let em = f64::from(face.head().ok()?.units_per_em().max(1));
    let mut pen = Pen(BezPath::new(), 1.0 / em);
    glyph.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).ok()?;
    Some(pen.0)
}

/// An outline in font units (y up) drawn as a path in ems (y down).
struct Pen(BezPath, f64);

impl Pen {
    fn at(&self, x: f32, y: f32) -> Point {
        Point::new(f64::from(x) * self.1, -f64::from(y) * self.1)
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.0.move_to(p);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.0.line_to(p);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (c, p) = (self.at(cx0, cy0), self.at(x, y));
        self.0.quad_to(c, p);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (self.at(cx0, cy0), self.at(cx1, cy1), self.at(x, y));
        self.0.curve_to(c0, c1, p);
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

impl Picture {
    /// Draw it with its baseline's left end at `origin`, `em` points to the
    /// em, in `color` where it has none of its own.
    pub(crate) fn paint(&self, canvas: &mut Canvas, origin: Point, em: f64, color: Color) {
        canvas.saved(|c| {
            c.concat(Affine::translate(origin.to_vec2()) * Affine::scale(em));
            for shape in &self.shapes {
                match shape {
                    Shape::Fill(path, own) => c.fill(path, own.unwrap_or(color)),
                    Shape::Stroke(path, width, own) => c.stroke(path, &StrokeStyle::new(*width), own.unwrap_or(color)),
                    Shape::Text { .. } => {}
                }
            }
        });
        for shape in &self.shapes {
            if let Shape::Text { text, at, size, color: own } = shape {
                let style = TextStyle::new(Font::serif(size * em), own.unwrap_or(color));
                let baseline = TextLayout::new(text, &style).first_baseline();
                let top = Point::new(origin.x + at.x * em, origin.y + at.y * em - baseline);
                canvas.draw_label(text, &style, top);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_are_set_as_katex_sets_them() {
        let inline = set(r"\sum_{k=1}^{n} k = \frac{n(n+1)}{2}", false);
        let shown = set(r"\sum_{k=1}^{n} k = \frac{n(n+1)}{2}", true);
        let (a, b) = (inline.set.as_ref().unwrap(), shown.set.as_ref().unwrap());
        assert!(a.width > 3.0 && a.ascent > 0.7 && a.descent > 0.2, "{} {} {}", a.width, a.ascent, a.descent);
        // On its own, the sum's limits go above and below it: taller.
        assert!(b.ascent + b.descent > a.ascent + a.descent + 0.5);
        assert!(a.shapes.iter().all(|s| matches!(s, Shape::Fill(_, None))), "glyphs and the bar, in the text's colour");
        assert_ne!(inline.id, shown.id);
        assert!(Arc::ptr_eq(&inline, &set(r"\sum_{k=1}^{n} k = \frac{n(n+1)}{2}", false)), "kept");
    }

    #[test]
    fn matrices_colours_and_mistakes() {
        let m = set(r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}^{-1}", true);
        let m = m.set.as_ref().unwrap();
        assert!(m.ascent > 1.0 && m.descent > 0.7, "two rows in tall parentheses");
        let red = set(r"\color{red}{x}", false);
        assert!(red.set.as_ref().unwrap().shapes.iter().any(|s| matches!(s, Shape::Fill(_, Some(c)) if c.r > 0.9)));
        assert!(set(r"\frac{1}{", false).set.is_err());
        // What KaTeX's fonts haven't is set in the system's.
        let text = set(r"x = \text{日本}", false);
        let shapes = &text.set.as_ref().unwrap().shapes;
        assert!(shapes.iter().any(|s| matches!(s, Shape::Text { text, .. } if text == "日")), "{}", shapes.len());
    }
}
