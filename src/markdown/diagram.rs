//! Mermaid diagrams, laid out as Mermaid lays them out (merman), in the
//! desktop's type and colours. merman writes SVG for a theme made from
//! Aizome's palette, its labels measured in Archivo as the kit sets it;
//! usvg reads the SVG, setting the labels in Archivo and turning them into
//! outlines; and what it holds becomes shapes the kit fills and strokes at
//! any size. Each diagram is drawn in every palette as its answer is
//! parsed (a worker's job), so Night and Day switch without drawing again,
//! and a diagram drawn once is kept.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use aizome_kit::sidestep_ui::kurbo::{Affine, BezPath, Point, Size};
use aizome_kit::sidestep_ui::{Canvas, Font, LineCap, LineJoin, StrokeStyle, TextStyle};
use aizome_kit::theme::ThemeName;
use aizome_kit::{Color, FontSpec, Role, Theme};
use merman::svg::{
    DeterministicTextMeasurer, HostTheme, HostThemeAppearance, MeasurementProfileId, Presentation, SvgPipeline,
    TextMeasurementPolicy, TextMeasurementProfile, TextMeasurementProfileIdentity, ThemeRole,
};
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment, SvgRequest};

/// The palettes a diagram is drawn in, in [`Diagram::drawn`]'s order.
const PALETTES: [ThemeName; 3] = [ThemeName::Night, ThemeName::Day, ThemeName::HighContrast];

/// Labels' size, in points: the transcript's small text.
const LABEL_SIZE: f64 = 14.0;

/// A diagram: its source, and how it's drawn.
pub(crate) struct Diagram {
    pub(crate) source: String,
    /// Drawn in each palette, or why it couldn't be.
    pub(crate) drawn: Result<[Picture; 3], String>,
    /// Tells it from other diagrams: its source.
    pub(crate) id: u64,
}

impl std::fmt::Debug for Diagram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Diagram").field("source", &self.source).field("drawn", &self.drawn.is_ok()).finish()
    }
}

impl PartialEq for Diagram {
    fn eq(&self, other: &Diagram) -> bool {
        self.id == other.id
    }
}

impl Diagram {
    /// How it's drawn in `theme`'s palette.
    pub(crate) fn picture(&self, theme: &Theme) -> Option<&Picture> {
        let drawn = self.drawn.as_ref().ok()?;
        PALETTES.iter().position(|p| *p == theme.palette.name).map(|i| &drawn[i])
    }

    /// Its kind, as its source names it (`flowchart`, `sequenceDiagram`).
    pub(crate) fn kind(&self) -> &str {
        self.source
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("%%") && !l.starts_with("---") && !l.contains(':'))
            .and_then(|l| l.split_whitespace().next())
            .unwrap_or("mermaid")
    }
}

/// A diagram in one palette: its size in points, and its shapes.
pub(crate) struct Picture {
    pub(crate) size: Size,
    shapes: Vec<Shape>,
}

enum Shape {
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

static DRAWN: OnceLock<Mutex<HashMap<String, Arc<Diagram>>>> = OnceLock::new();
const KEPT: usize = 64;

/// `source` drawn in every palette, or as it was drawn before.
pub(crate) fn draw(source: &str) -> Arc<Diagram> {
    let drawn = DRAWN.get_or_init(Mutex::default);
    if let Some(diagram) = drawn.lock().unwrap_or_else(|e| e.into_inner()).get(source) {
        return diagram.clone();
    }
    let pictures: Result<Vec<Picture>, String> = PALETTES.iter().map(|p| picture(source, *p)).collect();
    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    let diagram = Arc::new(Diagram {
        source: source.to_owned(),
        drawn: pictures.map(|p| p.try_into().unwrap_or_else(|_| unreachable!("one per palette"))),
        id: h.finish(),
    });
    let mut drawn = drawn.lock().unwrap_or_else(|e| e.into_inner());
    if drawn.len() >= KEPT {
        drawn.clear();
    }
    drawn.insert(source.to_owned(), diagram.clone());
    diagram
}

fn theme(palette: ThemeName) -> Theme {
    match palette {
        ThemeName::Night => Theme::night(),
        ThemeName::Day => Theme::day(),
        ThemeName::HighContrast => Theme::high_contrast(),
    }
}

fn hex(c: Color) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
}

/// Mermaid's theme, from Aizome's palette: nodes raised from the page as
/// panes are, lines and edges in the palette's, notes amber.
fn host_theme(t: &Theme) -> Result<HostTheme, String> {
    let roles = [
        (ThemeRole::Canvas, t.surface()),
        (ThemeRole::Surface, t.chrome()),
        (ThemeRole::SurfaceAlt, t.raised()),
        (ThemeRole::SurfaceMuted, t.sunk()),
        (ThemeRole::Text, t.text()),
        (ThemeRole::SubtleText, t.text3()),
        (ThemeRole::Border, t.role(Role::Accent).mix(t.line(), 0.55)),
        (ThemeRole::Line, t.text3()),
        (ThemeRole::EdgeLabelBackground, t.surface()),
        (ThemeRole::ClusterBackground, t.sunk()),
        (ThemeRole::ClusterBorder, t.line2()),
        (ThemeRole::NoteBackground, t.role(Role::Warning).mix(t.surface(), 0.82)),
        (ThemeRole::NoteBorder, t.role(Role::Warning)),
        (ThemeRole::NoteText, t.text()),
        (ThemeRole::ActorBackground, t.chrome()),
        (ThemeRole::ActorBorder, t.role(Role::Accent)),
        (ThemeRole::ActorText, t.text()),
        (ThemeRole::ActivationBackground, t.raised()),
        (ThemeRole::ActivationBorder, t.role(Role::Accent)),
        (ThemeRole::Error, t.role(Role::Danger)),
        (ThemeRole::Warning, t.role(Role::Warning)),
        (ThemeRole::Success, t.role(Role::Success)),
    ];
    let mut host = HostTheme::new()
        .with_appearance(if t.is_dark() { HostThemeAppearance::Dark } else { HostThemeAppearance::Light })
        .try_with_font_family(format!("{}, sans-serif", ui_family(t)))
        .and_then(|h| h.try_with_font_size(format!("{LABEL_SIZE}px")))
        .map_err(|e| e.to_string())?;
    for (role, color) in roles {
        host = host.try_with_role(role, hex(color)).map_err(|e| e.to_string())?;
    }
    let series = [Role::Accent, Role::Cyan, Role::Violet, Role::Success, Role::Warning, Role::Danger, Role::Text3];
    host.try_with_series_palette(series.map(|r| hex(t.role(r)))).map_err(|e| e.to_string())
}

/// The family the kit sets interface text in (Archivo, where it's there).
fn ui_family(t: &Theme) -> String {
    t.font(FontSpec::ui(LABEL_SIZE)).family_name().to_owned()
}

fn picture(source: &str, palette: ThemeName) -> Result<Picture, String> {
    let t = theme(palette);
    let resolved = Presentation::new().with_theme(host_theme(&t)?).resolve();
    let renderer = Renderer::new().with_engine(resolved.materialize_engine(merman::Engine::new()));
    let request = SvgRequest {
        environment: SvgEnvironment::deterministic().with_text_measurement_policy(measured_as_drawn(&t)?),
        pipeline: Some(SvgPipeline::resvg_safe()),
        presentation: resolved.render_policy(),
        ..Default::default()
    };
    let output = renderer.render(RenderRequest::svg(source, OperationControl::new(), request)).map_err(|e| e.to_string())?;
    let RenderOutput::Svg(Some(svg)) = output else { return Err("This isn't a diagram Mermaid knows.".into()) };
    let options = usvg::Options { fontdb: fonts(), font_family: ui_family(&t), ..usvg::Options::default() };
    let tree = usvg::Tree::from_str(svg.svg(), &options).map_err(|e| e.to_string())?;
    let mut shapes = Vec::new();
    collect(tree.root(), 1.0, &mut shapes);
    let size = tree.size();
    Ok(Picture { size: Size::new(f64::from(size.width()), f64::from(size.height())), shapes })
}

/// Labels measured as the kit lays text out, in the face it draws them in.
fn measured_as_drawn(t: &Theme) -> Result<TextMeasurementPolicy, String> {
    let (ui, mono) = (t.font(FontSpec::ui(LABEL_SIZE)), t.font(FontSpec::mono(LABEL_SIZE)));
    let identity = format!("{}:{}", ui.family_name(), mono.family_name());
    let measurer = DeterministicTextMeasurer::default().with_width_callback(move |text, style| {
        let family = style.font_family.as_deref().unwrap_or_default().to_ascii_lowercase();
        let font = if family.contains("mono") || family.contains("courier") { &mono } else { &ui };
        let mut font = font.with_size(style.font_size.max(1.0));
        if let Some(weight) = style.font_weight.as_deref() {
            font = match weight {
                "bold" | "bolder" => font.bold(),
                w => w.parse::<f32>().map_or(font.clone(), |w| font.with_weight(w)),
            };
        }
        if style.font_style.as_deref() == Some("italic") {
            font = font.italic();
        }
        aizome_kit::sidestep_ui::measure(text, &TextStyle::new(font, Color::BLACK)).width
    });
    let id = MeasurementProfileId::new("org.aizome.omperor.kit-text").map_err(|e| e.to_string())?;
    let identity = TextMeasurementProfileIdentity::new(id, identity).map_err(|e| e.to_string())?;
    Ok(TextMeasurementPolicy::uniform(TextMeasurementProfile::new(identity, measurer)))
}

/// The faces usvg sets labels in: Aizome's, where the kit finds them, then
/// the system's. Read once, by the first diagram.
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
            db.set_sans_serif_family(Font::system(LABEL_SIZE).family_name());
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
    fn a_flowchart_is_drawn_in_every_palette_with_its_labels() {
        let diagram = draw("flowchart LR\n    P[Prompt] --> T{Tools?}\n    T -- yes --> R[Run tools]\n");
        assert_eq!(diagram.kind(), "flowchart");
        let drawn = diagram.drawn.as_ref().expect("drawn");
        for picture in drawn {
            assert!(picture.size.width > picture.size.height && picture.size.height > 20.0, "{:?}", picture.size);
            // Boxes, edges, arrowheads and labels.
            assert!(picture.shapes.len() > 10, "{}", picture.shapes.len());
        }
        let fills = |p: &Picture| p.shapes.iter().filter_map(|s| if let Shape::Fill(_, c, _) = s { Some(*c) } else { None }).collect::<Vec<_>>();
        assert_ne!(fills(&drawn[0]), fills(&drawn[1]), "Night and Day differ");
        assert!(Arc::ptr_eq(&diagram, &draw("flowchart LR\n    P[Prompt] --> T{Tools?}\n    T -- yes --> R[Run tools]\n")));
    }

    #[test]
    fn what_isnt_mermaid_says_so() {
        assert!(draw("this is not a diagram").drawn.is_err());
    }
}
