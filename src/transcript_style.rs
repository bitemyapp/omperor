//! The transcript's look for the Markdown renderer (vendor/upleft): the
//! chat style, text sizes and palette of Settings, as a theme and a style
//! sheet per voice. A style sheet resolves its colours against one
//! appearance when it is built, so the transcript builds new ones when the
//! appearance changes (`Styles::for_appearance`).

use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSColorSpace};
use objc2_foundation::NSArray;
use upleft_render::render_contracts::{CodeTheme, Theme, ThemeAppearance, ThemeColor};
use upleft_render::theme::style_sheet::StyleSheet;

use crate::chat_typography::TextRole;
use crate::native_markdown::Typography;
use crate::theme;

/// The two voices' style sheets for one typography and appearance.
#[derive(Clone)]
pub(crate) struct Styles {
    pub(crate) assistant: Rc<StyleSheet>,
    pub(crate) user: Rc<StyleSheet>,
    pub(crate) typography: Typography,
    pub(crate) dark: bool,
    /// What decides a message's height: the typography, not the colours.
    pub(crate) metrics_key: u64,
}

thread_local! {
    /// The last style sheets built, which the transcript and Settings'
    /// examples share: building one sets up fonts, a few milliseconds.
    static SHARED: std::cell::RefCell<Option<Styles>> = const { std::cell::RefCell::new(None) };
}

impl Styles {
    /// The style sheets for `typography` under `appearance`, reused while
    /// neither changes.
    pub(crate) fn shared(typography: Typography, appearance: &NSAppearance) -> Styles {
        let dark = is_dark(appearance);
        if let Some(styles) = SHARED.with(|shared| shared.borrow().clone())
            && styles.typography == typography
            && styles.dark == dark
        {
            return styles;
        }
        let styles = Styles::for_appearance(typography, appearance);
        SHARED.with(|shared| *shared.borrow_mut() = Some(styles.clone()));
        styles
    }

    pub(crate) fn for_appearance(typography: Typography, appearance: &NSAppearance) -> Styles {
        let _span = crate::perf::span("gui.transcript.style_sheets");
        let dark = is_dark(appearance);
        let theme = omperor_theme(appearance, dark);
        // Reduce Motion only drives Downright's own animations, which a
        // transcript in Read mode does not run.
        let sheet = |role| Rc::new(StyleSheet::for_host(theme.clone(), appearance, true, typography.host_typography(role)));
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{typography:?}").hash(&mut hasher);
        Styles {
            assistant: sheet(TextRole::Assistant),
            user: sheet(TextRole::User),
            typography,
            dark,
            metrics_key: hasher.finish(),
        }
    }
}

pub(crate) fn is_dark(appearance: &NSAppearance) -> bool {
    // SAFETY: AppKit exports both appearance names for the process lifetime.
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    appearance
        .bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[aqua, dark]))
        .is_some_and(|name| name.to_string() == dark.to_string())
}

/// Downright's System theme in Omperor's palette: the page, the lifted
/// surface, the gold accent, the quiet secondary ink, hairlines; system
/// labels and link colour; One Half code colours, as the old transcript's
/// syntax themes had them.
fn omperor_theme(appearance: &NSAppearance, dark: bool) -> Theme {
    let mut theme = Theme::fallback();
    theme.name = "Omperor".to_owned();
    theme.appearance = if dark { ThemeAppearance::Dark } else { ThemeAppearance::Light };
    let fixed = |color: Retained<NSColor>| ThemeColor::new(&hex_in(&color, appearance));
    let system = ThemeColor::new;
    let palette = &mut theme.palette;
    palette.background = fixed(theme::canvas());
    palette.surface = fixed(theme::surface());
    palette.text = system("system:label");
    palette.text_secondary = fixed(theme::secondary_text());
    palette.heading = system("system:label");
    palette.accent = fixed(theme::accent());
    palette.link = system("system:link");
    // Rules keep the separator's alpha (a `system:` colour would lose it).
    // Code's edges are the interface's hairline laid on the page: opaque,
    // since the inline code pill strokes it at half alpha.
    palette.rule = fixed(NSColor::separatorColor());
    palette.code_rule = ThemeColor::new(if dark { "#3a2c40" } else { "#eadfd8" });
    palette.quote_rule = fixed(theme::hairline());
    palette.inline_code_background = fixed(NSColor::tertiarySystemFillColor());
    // Code sits on its own sheet: a warm vellum on paper, a lifted plum on
    // the plum-black page.
    palette.code_background = ThemeColor::new(if dark { "#211a26" } else { "#f5efe7" });
    // Callouts in the crown's colours and their neighbours, each deep enough
    // to read as text on paper and bright enough to glow on plum-black:
    // Note plum, Tip jade, Important marigold, Warning copper, Caution rose.
    let pick = |light: &str, dark_hex: &str| ThemeColor::new(if dark { dark_hex } else { light });
    palette.callout_note = pick("#8a2c70", "#e492ce");
    palette.callout_success = pick("#16875f", "#6cdaa4");
    palette.callout_important = Some(pick("#9a6608", "#f5c75c"));
    palette.callout_warning = pick("#b4521a", "#ff9f66");
    palette.callout_danger = pick("#be2c46", "#ff7e8c");
    theme.code = one_half(dark);
    theme
}

/// The One Half palettes (Son A. Pham's), as syntect's One Half themes map
/// them onto scopes.
fn one_half(dark: bool) -> CodeTheme {
    let c = ThemeColor::new;
    if dark {
        CodeTheme {
            keyword: c("#c678dd"),
            string: c("#98c379"),
            number: c("#e5c07b"),
            comment: c("#5c6370"),
            r#type: c("#e5c07b"),
            function: c("#61afef"),
            variable: c("#e06c75"),
            constant: c("#e5c07b"),
            operator: c("#56b6c2"),
            punctuation: c("#dcdfe4"),
            attribute: c("#e5c07b"),
            diff_added: c("#98c379"),
            diff_removed: c("#e06c75"),
            diff_header: c("#61afef"),
        }
    } else {
        CodeTheme {
            keyword: c("#a626a4"),
            string: c("#50a14f"),
            number: c("#c18401"),
            comment: c("#a0a1a7"),
            r#type: c("#c18401"),
            function: c("#0184bc"),
            variable: c("#e45649"),
            constant: c("#c18401"),
            operator: c("#0997b3"),
            punctuation: c("#383a42"),
            attribute: c("#c18401"),
            diff_added: c("#50a14f"),
            diff_removed: c("#e45649"),
            diff_header: c("#0184bc"),
        }
    }
}

/// `#rrggbbaa` for `color` as drawn under `appearance`.
fn hex_in(color: &NSColor, appearance: &NSAppearance) -> String {
    let components = Cell::new((0.0, 0.0, 0.0, 1.0));
    let resolve = RcBlock::new(|| {
        let srgb = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace());
        if let Some(srgb) = srgb {
            components.set((srgb.redComponent(), srgb.greenComponent(), srgb.blueComponent(), srgb.alphaComponent()));
        }
    });
    appearance.performAsCurrentDrawingAppearance(&resolve);
    let (red, green, blue, alpha) = components.get();
    let byte = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}{:02x}", byte(red), byte(green), byte(blue), byte(alpha))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ChatStyle;

    #[test]
    fn a_dynamic_colour_resolves_per_appearance() {
        let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
        let light = NSAppearance::appearanceNamed(aqua).unwrap();
        let dark = NSAppearance::appearanceNamed(dark).unwrap();
        assert_eq!(hex_in(&theme::surface(), &light), "#fffefbff");
        assert_eq!(hex_in(&theme::surface(), &dark), "#261e2bff");
        assert!(!is_dark(&light));
        assert!(is_dark(&dark));
    }

    #[test]
    fn each_voice_gets_its_own_body_size() {
        let (aqua, _) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
        let light = NSAppearance::appearanceNamed(aqua).unwrap();
        let typography = Typography::new(ChatStyle::Editorial, 1.0);
        let styles = Styles::for_appearance(typography, &light);
        assert_eq!(styles.assistant.host.body_size, Some(15.5));
        assert_eq!(styles.assistant.host.hyphenation_factor, Some(0.9));
        assert!(!styles.dark);
        let modern = Styles::for_appearance(Typography::new(ChatStyle::Modern, 1.0), &light);
        assert_ne!(modern.metrics_key, styles.metrics_key);
    }
}
