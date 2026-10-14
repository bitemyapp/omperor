//! Shared visual tokens for the native interface. Neutral, opaque surfaces
//! keep text contrast stable regardless of the desktop behind the window.
//! Gold identifies an action or selection; the conversation owns the contrast.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSCompositingOperation,
    NSFont, NSFontDescriptorSystemDesignSerif, NSFontWeight, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};

/// How the transcript is set: which family carries the conversation and the
/// rhythm around it. Stored as a preference; `native_markdown::Typography`
/// turns it into fonts, sizes, and spacing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatStyle {
    /// San Francisco: the system's own voice, a comfortable measure.
    #[default]
    Modern,
    /// New York: a serif page with a book's measure and hyphenated rag.
    Editorial,
    /// SF Mono throughout: the conversation as a terminal session.
    Terminal,
}

impl ChatStyle {
    pub(crate) const ALL: [Self; 3] = [Self::Modern, Self::Editorial, Self::Terminal];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Modern => "Modern",
            Self::Editorial => "Editorial",
            Self::Terminal => "Terminal",
        }
    }

    pub(crate) fn from_label(label: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|style| style.label() == label)
            .unwrap_or_default()
    }

    pub(crate) const fn stored(self) -> &'static str {
        match self {
            Self::Modern => "modern",
            Self::Editorial => "editorial",
            Self::Terminal => "terminal",
        }
    }

    pub(crate) fn from_stored(value: Option<String>) -> Self {
        match value.as_deref() {
            Some("editorial") => Self::Editorial,
            Some("terminal") => Self::Terminal,
            _ => Self::Modern,
        }
    }
}

/// An sRGB color, channels in 0–255 and alpha in 0–1.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rgba(pub f64, pub f64, pub f64, pub f64);

// The crowned gate's gold and plum, brightened: a marigold that glows on
// the plum-black page, an amber deep enough to read as text on paper, and
// a plum with more red in it, so the two sit apart at a glance.
const GOLD: (f64, f64, f64) = (245.0, 199.0, 92.0);
const GOLD_DEEP: (f64, f64, f64) = (160.0, 108.0, 12.0);
const PLUM: (f64, f64, f64) = (124.0, 36.0, 100.0);
const PLUM_BRIGHT: (f64, f64, f64) = (228.0, 146.0, 206.0);

const fn with_alpha((red, green, blue): (f64, f64, f64), alpha: f64) -> Rgba {
    Rgba(red, green, blue, alpha)
}

fn srgb(Rgba(red, green, blue, alpha): Rgba) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(red / 255.0, green / 255.0, blue / 255.0, alpha)
}

fn is_dark(appearance: &NSAppearance) -> bool {
    // SAFETY: AppKit exports the appearance-name constants for the process.
    let (aqua, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
    appearance
        .bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[aqua, dark]))
        .is_some_and(|name| name.to_string() == dark.to_string())
}

/// A color that resolves to `light` under Aqua and `dark` under Dark Aqua at
/// draw time, so a view keeps the right value when the appearance changes.
pub(crate) fn dynamic(light: Rgba, dark: Rgba) -> Retained<NSColor> {
    let provider = RcBlock::new(
        move |appearance: NonNull<NSAppearance>| -> NonNull<NSColor> {
            // SAFETY: AppKit hands the provider a live appearance for the call.
            let appearance = unsafe { appearance.as_ref() };
            let color = if is_dark(appearance) {
                srgb(dark)
            } else {
                srgb(light)
            };
            // Handed back autoreleased, as an Objective-C provider would.
            NonNull::new(Retained::autorelease_return(color)).expect("a color")
        },
    );
    // SAFETY: AppKit copies the block into the color, so the provider outlives
    // this frame; it captures only plain values.
    unsafe { NSColor::colorWithName_dynamicProvider(None, &provider) }
}

/// Gold: the crown. Selected items, the user's own words, the send button.
pub(crate) fn accent() -> Retained<NSColor> {
    dynamic(with_alpha(GOLD_DEEP, 1.0), with_alpha(GOLD, 1.0))
}

/// Gold at fill strength, for selection pills and chips behind text.
pub(crate) fn accent_fill() -> Retained<NSColor> {
    dynamic(with_alpha(GOLD_DEEP, 0.09), with_alpha(GOLD, 0.10))
}

/// Gold at selection strength, for the row a sheet is showing or the open
/// session: enough to read as chosen rather than tinted. A press shows it at
/// `motion::PRESS_TINT` of its alpha.
pub(crate) fn accent_fill_strong() -> Retained<NSColor> {
    dynamic(with_alpha(GOLD_DEEP, 0.14), with_alpha(GOLD, 0.16))
}

/// Plum at full strength, for a mark that must read on its own: the ring
/// around an agent that is waiting on the user.
pub(crate) fn plum() -> Retained<NSColor> {
    dynamic(with_alpha(PLUM, 1.0), with_alpha(PLUM_BRIGHT, 1.0))
}

/// Gold as a rule rather than ink: Editorial's pull-quote bar.
pub(crate) fn accent_rule() -> Retained<NSColor> {
    dynamic(with_alpha(GOLD_DEEP, 0.55), with_alpha(GOLD, 0.6))
}

/// Plum at a whisper, for marks beside quiet text: thinking's margin rule.
pub(crate) fn plum_soft() -> Retained<NSColor> {
    dynamic(with_alpha(PLUM, 0.28), with_alpha(PLUM_BRIGHT, 0.34))
}

/// The user's bubble: the page washed with plum, one step toward the
/// crown's other colour, so the two voices never need a rule between them.
pub(crate) fn user_bubble() -> Retained<NSColor> {
    dynamic(Rgba(244.0, 234.0, 241.0, 1.0), Rgba(50.0, 35.0, 55.0, 1.0))
}

/// The bubble's edge: barely there, so the fill reads as paper, not a box.
pub(crate) fn user_bubble_edge() -> Retained<NSColor> {
    dynamic(with_alpha(PLUM, 0.10), with_alpha(PLUM_BRIGHT, 0.14))
}

/// A tool chip's fill: a plum-grey pill behind a finished call.
pub(crate) fn chip_fill() -> Retained<NSColor> {
    dynamic(with_alpha(PLUM, 0.055), Rgba(255.0, 235.0, 250.0, 0.065))
}

/// Success and failure, tuned to the palette rather than the system's
/// traffic lights: a jade and a rose, readable as text in both appearances.
pub(crate) fn success() -> Retained<NSColor> {
    dynamic(Rgba(22.0, 138.0, 96.0, 1.0), Rgba(108.0, 218.0, 164.0, 1.0))
}

pub(crate) fn danger() -> Retained<NSColor> {
    dynamic(Rgba(190.0, 44.0, 70.0, 1.0), Rgba(255.0, 126.0, 140.0, 1.0))
}

/// A failed chip's fill: rose at fill strength.
pub(crate) fn danger_fill() -> Retained<NSColor> {
    dynamic(Rgba(190.0, 44.0, 70.0, 0.08), Rgba(255.0, 126.0, 140.0, 0.12))
}

/// Minimum size for supporting interface copy. Never use tracking to make
/// small text appear larger; let San Francisco keep its optical spacing.
pub(crate) const CAPTION_SIZE: f64 = 12.0;
pub(crate) const CONTROL_SIZE: f64 = 13.0;
pub(crate) const NAVIGATION_SIZE: f64 = 14.0;
pub(crate) const READING_SIZE: f64 = 15.0;
pub(crate) const READING_MEASURE: f64 = 720.0;

pub(crate) fn ui_font(size: f64, emphasis: bool) -> Retained<NSFont> {
    // SAFETY: AppKit exports these font weights for the process lifetime.
    let weight = unsafe {
        if emphasis {
            NSFontWeightSemibold
        } else {
            NSFontWeightRegular
        }
    };
    NSFont::systemFontOfSize_weight(size.max(CAPTION_SIZE), weight)
}

/// Solid colors keep supporting text readable on every surface, including
/// inactive windows where AppKit's tertiary label color becomes very faint.
pub(crate) fn secondary_text() -> Retained<NSColor> {
    dynamic(Rgba(96.0, 86.0, 98.0, 1.0), Rgba(186.0, 174.0, 190.0, 1.0))
}

/// Warm paper in light; in dark, a plum-black rather than a neutral grey.
pub(crate) fn canvas() -> Retained<NSColor> {
    dynamic(Rgba(252.0, 249.0, 244.0, 1.0), Rgba(22.0, 17.0, 26.0, 1.0))
}

pub(crate) fn sidebar() -> Retained<NSColor> {
    dynamic(Rgba(244.0, 239.0, 240.0, 1.0), Rgba(30.0, 23.0, 35.0, 1.0))
}

pub(crate) fn inspector() -> Retained<NSColor> {
    navigation_surface()
}

/// The composer and settings cards sit one step above their surroundings.
pub(crate) fn surface() -> Retained<NSColor> {
    dynamic(Rgba(255.0, 254.0, 251.0, 1.0), Rgba(38.0, 30.0, 43.0, 1.0))
}

pub(crate) fn surface_inset() -> Retained<NSColor> {
    dynamic(Rgba(80.0, 30.0, 66.0, 0.05), Rgba(255.0, 235.0, 250.0, 0.06))
}

pub(crate) fn popover() -> Retained<NSColor> {
    dynamic(Rgba(255.0, 253.0, 250.0, 1.0), Rgba(42.0, 33.0, 48.0, 1.0))
}

pub(crate) fn hairline() -> Retained<NSColor> {
    dynamic(Rgba(70.0, 24.0, 58.0, 0.11), Rgba(255.0, 228.0, 248.0, 0.11))
}

/// New York for the optional Editorial transcript style. Interface chrome
/// always uses the system sans face.
pub(crate) fn display_font(size: f64, weight: NSFontWeight) -> Retained<NSFont> {
    let base = NSFont::systemFontOfSize_weight(size, weight);
    // SAFETY: AppKit exports the design name for the process lifetime.
    let design = unsafe { NSFontDescriptorSystemDesignSerif };
    let Some(descriptor) = base.fontDescriptor().fontDescriptorWithDesign(design) else {
        return base;
    };
    NSFont::fontWithDescriptor_size(&descriptor, size).unwrap_or(base)
}

/// An SF Symbol for a row's leading icon: a template image (so the button's
/// tint colors it) drawn centered in a fixed cell with `gap` points of clear
/// space after it, so titles never crowd the glyph and every row's text
/// starts at the same place whatever the symbol's own width.
pub(crate) fn row_symbol(name: &str, point_size: f64, gap: f64) -> Option<Retained<NSImage>> {
    // SAFETY: AppKit exports NSFontWeightMedium for the process lifetime.
    let symbol = symbol(name, point_size, unsafe { NSFontWeightMedium }, None)?;
    let size = symbol.size();
    let cell = (point_size * 1.45).max(size.width);
    let padded = NSSize::new(cell + gap, size.height.max(point_size * 1.3));
    let handler = RcBlock::new(move |_bounds: NSRect| -> Bool {
        let origin = NSPoint::new(
            (cell - size.width) / 2.0,
            (padded.height - size.height) / 2.0,
        );
        symbol.drawInRect_fromRect_operation_fraction(
            NSRect::new(origin, size),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
            NSCompositingOperation::SourceOver,
            1.0,
        );
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(padded, false, &handler);
    image.setTemplate(true);
    Some(image)
}

/// A project header's disclosure comes before its folder, in fixed-width cells.
/// Drawing native symbols at render time keeps both crisp at any display scale.
pub(crate) fn project_row_symbol(expanded: bool) -> Option<Retained<NSImage>> {
    // SAFETY: AppKit exports this weight for the process lifetime.
    let weight = unsafe { NSFontWeightMedium };
    let chevron = symbol(
        if expanded {
            "chevron.down"
        } else {
            "chevron.right"
        },
        11.0,
        weight,
        None,
    )?;
    let folder = symbol("folder", 13.0, weight, None)?;
    let size = NSSize::new(44.0, 20.0);
    let handler = RcBlock::new(move |_bounds: NSRect| -> Bool {
        for (image, x, width) in [(&chevron, 0.0, 14.0), (&folder, 18.0, 20.0)] {
            let image_size = image.size();
            let origin = NSPoint::new(
                x + (width - image_size.width) / 2.0,
                (size.height - image_size.height) / 2.0,
            );
            image.drawInRect_fromRect_operation_fraction(
                NSRect::new(origin, image_size),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
                NSCompositingOperation::SourceOver,
                1.0,
            );
        }
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler);
    image.setTemplate(true);
    Some(image)
}

/// An SF Symbol at a point size and weight, optionally tinted (hierarchically,
/// so multi-layer symbols keep their depth).
pub(crate) fn symbol(
    name: &str,
    point_size: f64,
    weight: NSFontWeight,
    color: Option<&NSColor>,
) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let sized = NSImageSymbolConfiguration::configurationWithPointSize_weight(point_size, weight);
    let image = image.imageWithSymbolConfiguration(&sized)?;
    match color {
        Some(color) => image.imageWithSymbolConfiguration(
            &NSImageSymbolConfiguration::configurationWithHierarchicalColor(color),
        ),
        None => Some(image),
    }
}

/// Navigation follows the window appearance with a restrained plum undertone.
pub(crate) fn navigation_surface() -> Retained<NSColor> {
    dynamic(Rgba(244.0, 239.0, 240.0, 1.0), Rgba(30.0, 23.0, 35.0, 1.0))
}

/// Cyan identifies retained evidence and workspace navigation in both appearances.
pub(crate) fn signal() -> Retained<NSColor> {
    dynamic(Rgba(0.0, 105.0, 119.0, 1.0), Rgba(105.0, 223.0, 219.0, 1.0))
}
pub(crate) fn signal_fill() -> Retained<NSColor> {
    dynamic(
        Rgba(0.0, 105.0, 119.0, 0.08),
        Rgba(105.0, 223.0, 219.0, 0.08),
    )
}

/// The crown's gold at full strength in both appearances, for a solid
/// selection that carries its own label: the header's surface switch.
pub(crate) fn crown_gold() -> Retained<NSColor> {
    dynamic(with_alpha(GOLD, 1.0), with_alpha(GOLD, 1.0))
}

/// Ink on crown gold: the plum-black of the dark page, about 11:1 on it.
pub(crate) fn on_gold() -> Retained<NSColor> {
    dynamic(Rgba(34.0, 20.0, 30.0, 1.0), Rgba(22.0, 17.0, 26.0, 1.0))
}

/// Plum at fill strength, behind a plum mark: a stat's badge, the header's
/// model pill.
pub(crate) fn plum_fill() -> Retained<NSColor> {
    dynamic(with_alpha(PLUM, 0.10), with_alpha(PLUM_BRIGHT, 0.14))
}
