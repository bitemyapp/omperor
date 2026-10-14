#![allow(dead_code)]

#[cfg(target_os = "linux")]
use sidestep as _;

#[path = "../src/app_state.rs"]
mod app_state;
#[path = "../src/chat.rs"]
mod chat;
#[path = "../src/chat_typography.rs"]
mod chat_typography;
#[path = "../src/frame_budget.rs"]
mod frame_budget;
#[path = "../src/markdown.rs"]
mod markdown;
#[path = "../src/native_markdown.rs"]
mod native_markdown;
#[path = "../src/perf.rs"]
mod perf;
#[path = "../src/theme.rs"]
mod theme;

mod session {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Speaker {
        User,
        Assistant,
        Thinking,
        Tool,
        Event,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct SessionMessage {
        pub(crate) speaker: Speaker,
        pub(crate) text: String,
        pub(crate) tool: Option<String>,
        pub(crate) detail: Option<String>,
        pub(crate) failed: bool,
    }
}

use std::path::{Path, PathBuf};

use chat::{TranscriptMessage, TranscriptMessageKind};
use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSColor, NSLayoutManager, NSTextView,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRange, NSRect, NSSize};

const WIDTH: f64 = 900.0;
const MAX_HEIGHT: f64 = 10_000.0;

fn main() {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/markdown-snapshot.pdf"));
    let dark = std::env::args_os()
        .nth(2)
        .is_some_and(|value| value == "dark");
    // The chat style to set the sample in: modern (default), editorial, or
    // terminal.
    let style = std::env::args()
        .nth(3)
        .map(|value| theme::ChatStyle::from_stored(Some(value)))
        .unwrap_or_default();
    let mtm = MainThreadMarker::new().expect("snapshot renderer must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: The named AppKit appearance constant lives for the process.
    let appearance_name = unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    let appearance = NSAppearance::appearanceNamed(appearance_name).expect("named appearance");
    app.setAppearance(Some(&appearance));
    let message = TranscriptMessage {
        author: "omp".into(),
        status: None,
        detail_id: None,
        body: concat!(
            "# Code\n\n",
            "Rendering happens *entirely* in TextKit, so **selection**, ",
            "[links](https://example.com), and accessibility come for free. ",
            "The measure is capped so a line never runs past what the eye can ",
            "return from comfortably, and the rag is left ragged — no rivers.\n\n",
            "## Structure\n\n",
            "- Headings step down a tight scale\n",
            "- Lists hang their wrapped lines under the text, not the marker, ",
            "which matters once an item runs to a second line like this one does\n",
            "  - Nested items indent again\n",
            "- Emphasis stays in the family: *italic*, **bold**, ~~struck~~\n\n",
            "1. Numbered lists count\n",
            "2. And keep counting\n\n",
            "> A quotation is set off by a rule down its left edge rather than a ",
            "glyph on its first line, so a long quotation reads as one piece.\n",
            ">\n",
            "> Its second paragraph shares the rule.\n\n",
            "### Details\n\n",
            "Inline Swift code: `let attributedString = renderer.render(markdown)`.\n\n",
            "```swift\n",
            "import AppKit\n\n",
            "struct MarkdownRenderer {\n",
            "    func render(_ source: String) -> NSAttributedString {\n",
            "        NSAttributedString(string: source)\n",
            "    }\n",
            "}\n",
            "```\n\n",
            "A shell command:\n\n",
            "```bash\n",
            "xcodebuild \\\n",
            "  -scheme ExampleApp \\\n",
            "  -destination 'platform=iOS Simulator,name=iPhone 16 Pro' \\\n",
            "  test\n",
            "```\n\n",
            "A JSON payload:\n\n",
            "```json\n",
            "{\n",
            "  \"type\": \"markdown\",\n",
            "  \"source\": \"# Hello\",\n",
            "  \"features\": [\"headings\", \"lists\", \"code\"]\n",
            "}\n",
            "```\n\n",
            "A diff:\n\n",
            "```diff\n",
            "- let color = UIColor.label\n",
            "+ let color = traitCollection.userInterfaceStyle == .dark\n",
            "+     ? UIColor.white\n",
            "+     : UIColor.black\n",
            "```\n\n",
            "| Element | Markdown syntax | Typical use |\n",
            "| :--- | :---: | ---: |\n",
            "| Heading | `# Title` | Section labels |\n",
            "| Code | `` `value` `` | Identifiers |\n\n",
            "The paragraph after a table gets its air back.\n\n",
            "---\n\n",
            "And a rule, drawn as a hairline, closes the sample.\n",
        )
        .into(),
        kind: TranscriptMessageKind::Assistant,
    };
    let typography = native_markdown::Typography::new(style, 1.0);
    let rendered = native_markdown::render_message(
        &message,
        typography.measure(),
        Path::new("."),
        typography,
        true,
    );
    let view = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, MAX_HEIGHT)),
    );
    view.setAppearance(Some(&appearance));
    view.setDrawsBackground(true);
    view.setBackgroundColor(&NSColor::windowBackgroundColor());
    view.setTextContainerInset(NSSize::new(24.0, 24.0));
    view.setHorizontallyResizable(false);
    view.setVerticallyResizable(true);
    // SAFETY: NSTextView retains its text-system objects for its lifetime.
    let container = unsafe { view.textContainer() }.expect("text container");
    // Set at the style's measure, as the app does.
    container.setContainerSize(NSSize::new(typography.measure(), MAX_HEIGHT));
    container.setWidthTracksTextView(false);
    let storage = unsafe { view.textStorage() }.expect("text storage");
    storage.setAttributedString(&rendered);
    let layout: objc2::rc::Retained<NSLayoutManager> =
        unsafe { view.layoutManager() }.expect("layout manager");
    layout.ensureLayoutForTextContainer(&container);
    let rendered_text = rendered.string().to_string();
    // Foundation ranges count UTF-16 units; the sample has bullets and dashes
    // before the code, so byte offsets from `find` are converted.
    let utf16_at = |byte_index: usize| rendered_text[..byte_index].encode_utf16().count();
    let code_start = utf16_at(rendered_text.find("import AppKit").expect("fixture code"));
    // SAFETY: The fixture is ASCII through the queried range, and the layout
    // manager accepts a null actual-range pointer.
    let glyphs = unsafe {
        layout.glyphRangeForCharacterRange_actualCharacterRange(
            NSRange::new(code_start, "import AppKit".len()),
            std::ptr::null_mut(),
        )
    };
    let code_bounds = layout.boundingRectForGlyphRange_inTextContainer(glyphs, &container);
    assert!(
        code_bounds.size.width > 80.0 && code_bounds.size.height < 40.0,
        "code collapsed during TextKit layout: {code_bounds:?}"
    );
    let struct_start = utf16_at(
        rendered_text
            .find("struct MarkdownRenderer")
            .expect("second fixture code line"),
    );
    // SAFETY: The queried fixture prefix is ASCII and the null output range is
    // explicitly accepted by NSLayoutManager.
    let struct_glyphs = unsafe {
        layout.glyphRangeForCharacterRange_actualCharacterRange(
            NSRange::new(struct_start, "struct MarkdownRenderer".len()),
            std::ptr::null_mut(),
        )
    };
    let struct_bounds = layout.boundingRectForGlyphRange_inTextContainer(struct_glyphs, &container);
    let baseline_grid = (struct_bounds.origin.y - code_bounds.origin.y).abs();
    assert!(
        (32.0..=44.0).contains(&baseline_grid),
        "code baseline grid is too loose: {baseline_grid:.1}pt"
    );
    let function_start = utf16_at(
        rendered_text
            .find("func render")
            .expect("adjacent fixture code line"),
    );
    // SAFETY: The queried fixture prefix is ASCII and the null output range is
    // explicitly accepted by NSLayoutManager.
    let function_glyphs = unsafe {
        layout.glyphRangeForCharacterRange_actualCharacterRange(
            NSRange::new(function_start, "func render".len()),
            std::ptr::null_mut(),
        )
    };
    let function_bounds =
        layout.boundingRectForGlyphRange_inTextContainer(function_glyphs, &container);
    let adjacent_baseline = (function_bounds.origin.y - struct_bounds.origin.y).abs();
    assert!(
        (17.0..=21.0).contains(&adjacent_baseline),
        "adjacent code lines escaped the baseline grid: {adjacent_baseline:.1}pt"
    );
    let used = layout.usedRectForTextContainer(&container);
    view.setFrameSize(NSSize::new(WIDTH, used.size.height + 48.0));
    layout.ensureLayoutForTextContainer(&container);
    let pdf = view.dataWithPDFInsideRect(view.bounds());
    std::fs::write(&output, pdf.to_vec()).expect("write Markdown snapshot PDF");
    println!("{}", output.display());
}
