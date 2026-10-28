//! SVG as shapes the kit draws: usvg reads it (setting its text in the
//! faces it names, Aizome's where it can, and turning the text into
//! outlines), and what it holds becomes filled and stroked paths drawn at
//! any size. Diagrams come this way, and so do SVG pictures in answers.
//!
//! What's drawn is what a picture's paths make: gradients take their first
//! stop's colour, and clips, masks, filters and pictures inside the SVG are
//! left out.

use std::sync::{Arc, OnceLock};

use aizome_kit::Color;
use aizome_kit::sidestep_ui::kurbo::{Affine, BezPath, Point, Size};
use aizome_kit::sidestep_ui::{Canvas, Font, LineCap, LineJoin, StrokeStyle};

/// An SVG's size in points, and its shapes.
pub(crate) struct Picture {
    pub(crate) size: Size,
    pub(super) shapes: Vec<Shape>,
}

pub(super) enum Shape {
    /// Filled (by the even-odd rule when it says so).
    Fill(BezPath, Color, bool),
    Stroke(BezPath, StrokeStyle, Color),
}

impl Picture {
    /// Draw it with its top left at `origin`, `scale` times its size.
    pub(crate) fn paint(&self, canvas: &mut Canvas, origin: Point, scale: f64) {
        canvas.saved(|c| {
            c.concat(Affine::translate(origin.to_vec2()) * Affine::scale(scale));
            for shape in &self.shapes {
                match shape {
                    Shape::Fill(path, color, false) => c.fill(path, *color),
                    Shape::Fill(path, color, true) => c.fill_even_odd(path, *color),
                    Shape::Stroke(path, style, color) => c.stroke(path, style, *color),
                }
            }
        });
    }
}

/// `data`, an SVG document, as shapes; text with no face of its
/// own set in `family` (the system's sans by default). A worker's job.
pub(crate) fn read(data: &[u8], family: Option<&str>) -> Result<Picture, String> {
    let mut options = usvg::Options { fontdb: fonts(), ..usvg::Options::default() };
    if let Some(family) = family {
        options.font_family = family.to_owned();
    }
    let tree = usvg::Tree::from_data(data, &options).map_err(|e| e.to_string())?;
    let mut shapes = Vec::new();
    collect(tree.root(), 1.0, &mut shapes);
    let size = tree.size();
    Ok(Picture { size: Size::new(f64::from(size.width()), f64::from(size.height())), shapes })
}

/// Whether `data` looks like an SVG document rather than a bitmap.
pub(crate) fn is_svg(data: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&data[..data.len().min(512)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    let prologue = ["<?xml", "<!--", "<!doctype"].iter().any(|p| head.starts_with(p));
    head.starts_with("<svg") || (prologue && head.contains("<svg"))
}

/// The faces usvg sets text in: Aizome's, where the kit finds them, then
/// the system's. Read once, by the first SVG.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            if let Some(dir) = std::env::var_os("AIZOME_FONT_DIR") {
                db.load_fonts_dir(dir);
            }
            for dir in aizome_kit::fonts::font_dirs() {
                db.load_fonts_dir(dir);
            }
            db.load_system_fonts();
            db.set_sans_serif_family(Font::system(13.0).family_name());
            Arc::new(db)
        })
        .clone()
}

/// The shapes `group` holds, as drawn, at `opacity`.
fn collect(group: &usvg::Group, opacity: f32, shapes: &mut Vec<Shape>) {
    let opacity = opacity * group.opacity().get();
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => collect(g, opacity, shapes),
            usvg::Node::Text(t) => collect(t.flattened(), opacity, shapes),
            usvg::Node::Path(p) if p.is_visible() => {
                let t = p.abs_transform();
                let place = Affine::new([t.sx, t.ky, t.kx, t.sy, t.tx, t.ty].map(f64::from));
                let path = place * bez(p.data());
                let fill = p.fill().and_then(|f| {
                    let color = color(f.paint(), f.opacity().get() * opacity)?;
                    Some(Shape::Fill(path.clone(), color, f.rule() == usvg::FillRule::EvenOdd))
                });
                let stroke = p.stroke().and_then(|s| {
                    let color = color(s.paint(), s.opacity().get() * opacity)?;
                    let scale = place.determinant().abs().sqrt();
                    let mut style = StrokeStyle::new(f64::from(s.width().get()) * scale)
                        .with_cap(match s.linecap() {
                            usvg::LineCap::Butt => LineCap::Butt,
                            usvg::LineCap::Round => LineCap::Round,
                            usvg::LineCap::Square => LineCap::Square,
                        })
                        .with_join(match s.linejoin() {
                            usvg::LineJoin::Round => LineJoin::Round,
                            usvg::LineJoin::Bevel => LineJoin::Bevel,
                            _ => LineJoin::Miter,
                        });
                    if let Some(dashes) = s.dasharray() {
                        let dashes = dashes.iter().map(|d| f64::from(*d) * scale).collect();
                        style = style.with_dash(dashes, f64::from(s.dashoffset()) * scale);
                    }
                    Some(Shape::Stroke(path.clone(), style, color))
                });
                let both = match p.paint_order() {
                    usvg::PaintOrder::FillAndStroke => [fill, stroke],
                    usvg::PaintOrder::StrokeAndFill => [stroke, fill],
                };
                shapes.extend(both.into_iter().flatten());
            }
            _ => {}
        }
    }
}

/// A paint's colour (a gradient's first), at `opacity`.
fn color(paint: &usvg::Paint, opacity: f32) -> Option<Color> {
    let (c, a) = match paint {
        usvg::Paint::Color(c) => (*c, 1.0),
        usvg::Paint::LinearGradient(g) => g.stops().first().map(|s| (s.color(), s.opacity().get()))?,
        usvg::Paint::RadialGradient(g) => g.stops().first().map(|s| (s.color(), s.opacity().get()))?,
        usvg::Paint::Pattern(_) => return None,
    };
    let alpha = (a * opacity).clamp(0.0, 1.0);
    (alpha > 0.0).then(|| Color::rgba8(c.red, c.green, c.blue, (alpha * 255.0).round() as u8))
}

fn bez(path: &usvg::tiny_skia_path::Path) -> BezPath {
    use usvg::tiny_skia_path::PathSegment;
    let p = |p: usvg::tiny_skia_path::Point| (f64::from(p.x), f64::from(p.y));
    let mut out = BezPath::new();
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(a) => out.move_to(p(a)),
            PathSegment::LineTo(a) => out.line_to(p(a)),
            PathSegment::QuadTo(a, b) => out.quad_to(p(a), p(b)),
            PathSegment::CubicTo(a, b, c) => out.curve_to(p(a), p(b), p(c)),
            PathSegment::Close => out.close_path(),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_svg_is_its_shapes_at_its_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
            <rect width="40" height="20" fill="#7aa2f7"/><path d="M0 0 L40 20" fill="none" stroke="#e0af68" stroke-width="2"/>
        </svg>"##;
        assert!(is_svg(svg) && is_svg(&[b"<?xml version=\"1.0\"?>\n".as_slice(), svg].concat()));
        assert!(!is_svg(b"\x89PNG\r\n\x1a\n"));
        let picture = read(svg, None).expect("read");
        assert_eq!(picture.size, Size::new(40.0, 20.0));
        assert!(matches!(&picture.shapes[..], [Shape::Fill(_, fill, false), Shape::Stroke(_, stroke, line)]
            if fill.b > 0.9 && stroke.width == 2.0 && line.r > 0.8));
        assert!(read(b"<svg", None).is_err());
    }
}
