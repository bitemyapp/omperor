//! The chat transcript: a stack of rows in the transcript's scroll view.
//!
//! Each user or assistant message is a hosted `MarkdownTextView` from the
//! Markdown renderer (vendor/upleft, its docs/EMBEDDING.md), sized to its
//! content; a long one is several, cut between top-level blocks
//! (`transcript_parse`). Consecutive tool, event, thinking and notice rows
//! share one small TextKit 1 view with the attributed rows
//! `native_markdown` has always set. The stack is virtual: only the rows
//! within about a screen of the viewport have views. The rest take a height
//! from a cache (keyed by content, width and type) or an estimate, and are
//! made in passes of bounded length as the reader scrolls toward them.
//!
//! Text is parsed on the transcript's parse worker. The message being
//! streamed is the only view that changes: each growth is parsed and diffed
//! on the worker, then applied on the main thread with the storage edit and
//! `update` in the same turn, which relays out only the tail.
//!
//! Heights change synchronously, from `update` and from the relayout after
//! a diagram, formula or image lands, and the rows below move in the same
//! pass. The reader's place holds: a reader at the bottom stays there, and
//! otherwise the first row in view keeps its place on screen.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAppearanceCustomization, NSAttributedStringNSStringDrawing, NSColor, NSCursor, NSCursorAttributeName, NSEventModifierFlags, NSFont, NSLayoutManager, NSMenu, NSMenuItem,
    NSParagraphStyle, NSParagraphStyleAttributeName, NSPasteboard, NSPasteboardTypeString, NSResponder, NSScrollView,
    NSLinkAttributeName, NSTextContainer, NSTextStorage, NSTextView, NSTextViewDelegate, NSUnderlineStyleAttributeName, NSView,
    NSViewBoundsDidChangeNotification, NSWorkspace, NSEvent, NSScrollerPart,
};
use objc2_foundation::{
    NSDictionary, NSMutableAttributedString, NSNotification, NSNotificationCenter, NSNumber, NSObject,
    NSObjectProtocol, NSPoint, NSRange, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer, NSURL,
    ns_string,
};
use upleft_core::{DirtySet, ParsedDocument};
use upleft_render::fragments::fragment_base::RemoteImage;
use upleft_render::view::markdown_text_view::{HostedLine, MarkdownTextView};
use upleft_render::view::markdown_text_view_delegate::{ContextTarget, ContextTargetKind, MarkdownTextViewDelegate};

use crate::chat::{Conversation, TranscriptMessage, TranscriptMessageKind};
use crate::markdown_bench::Displayed;
use crate::native_markdown::{self, Typography};
use crate::perf;
use crate::transcript_parse::{Done, Job, Parser, SealedSource, Segment};
use crate::transcript_style::Styles;
use crate::theme::ChatStyle;
use crate::{scroll_reserve, theme, transcript_semantics};

/// Space above the first row and below the last.
const TOP_PADDING: f64 = 28.0;
const BOTTOM_PADDING: f64 = 28.0;
/// A reader within this distance of the end is following it.
const NEAR_BOTTOM: f64 = 36.0;
/// The user's turn sits in a soft, rounded, plum-washed bubble, set in from
/// the column's left so the two voices are told apart by place as well as
/// by colour; no rule.
const USER_RULE: f64 = 0.0;
const USER_LEFT: f64 = 16.0;
const USER_RIGHT: f64 = 16.0;
const USER_TOP: f64 = 11.0;
const USER_BOTTOM: f64 = 11.0;
const USER_MARGIN: f64 = 4.0;
const USER_RADIUS: f64 = 16.0;

/// How far the user's bubble sits in from the column's left: a share of
/// the column, so a narrow window keeps its measure. Only Modern sets the
/// user's turn aside; Editorial and Terminal keep it on the column.
fn user_indent(style: ChatStyle, column: f64) -> f64 {
    match style {
        ChatStyle::Modern => (column * 0.09).clamp(0.0, 64.0).round(),
        ChatStyle::Editorial | ChatStyle::Terminal => 0.0,
    }
}

/// The user's turn, drawn under its text, by style: Modern's rounded plum
/// bubble; Editorial's pull-quote, a gold rule down the left and nothing
/// else; Terminal's command line, a square, faintly lit band with a gold
/// bar at its left edge. `frame` is the whole box.
fn user_decor(style: ChatStyle, segment: usize, count: usize, frame: NSRect, mtm: MainThreadMarker) -> Vec<Retained<TranscriptFillView>> {
    let bar = |width: f64, color: Retained<NSColor>| {
        TranscriptFillView::new(color, NSRect::new(frame.origin, NSSize::new(width, frame.size.height)), mtm)
    };
    match style {
        ChatStyle::Modern => vec![TranscriptFillView::bubble(segment, count, frame, mtm)],
        ChatStyle::Editorial => vec![bar(USER_BAR, theme::accent_rule())],
        ChatStyle::Terminal => vec![TranscriptFillView::new(theme::surface_inset(), frame, mtm), bar(USER_BAR + 1.0, theme::accent())],
    }
}

/// The width of Editorial's and Terminal's bar down the user's turn.
const USER_BAR: f64 = 2.0;

/// The bubble's corners for one segment of a message: a message set as
/// several views rounds only its outer corners, so the stack reads as one.
/// Core Animation's mask, in the fill's unflipped layer: bit 0 and 1 are
/// the bottom corners, 2 and 3 the top.
fn user_corners(segment: usize, count: usize) -> usize {
    let top = if segment == 0 { 4 | 8 } else { 0 };
    let bottom = if segment + 1 == count { 1 | 2 } else { 0 };
    top | bottom
}
/// Room below the speaker's name.
const SPEAKER_GAP: f64 = 8.0;
/// Space above and below a `/usage` card.
const CARD_MARGIN: f64 = 10.0;
/// Most activity rows one view holds.
const ACTIVITY_GROUP: usize = 48;
/// How long one pass may spend making views. Rows in sight are made in
/// the pass that shows them, all of them: a row left for the next pass
/// shows as a blank that fills in (a session opened, a jump); this only
/// bounds a pathological pass. Views ahead of the reader get a frame's
/// share.
const VISIBLE_BUDGET: Duration = Duration::from_millis(100);
const NEARBY_BUDGET: Duration = Duration::from_micros(3_000);
/// How long restoring a place may spend making its message's views, from
/// the message's top down; the rows in sight then follow as usual.
const PLACE_BUDGET: Duration = Duration::from_millis(12);
/// What making a view costs, roughly (measured in release builds over the
/// benchmark's long sessions): a Markdown view by its text, a run of
/// activity rows by its rows.
const MAKE_MARKDOWN_BASE: Duration = Duration::from_micros(1_000);
const MAKE_MARKDOWN_KB: Duration = Duration::from_micros(2_200);
const MAKE_ACTIVITY_BASE: Duration = Duration::from_micros(500);
const MAKE_ACTIVITY_ROW: Duration = Duration::from_micros(150);
/// Screens of views made ahead of the reader in the direction they last
/// scrolled, and behind.
const PREPARE_AHEAD: f64 = 3.5;
/// At most this many far pieces are looked at to hold one correction.
const FAR_PIECES: usize = 32;
const PREPARE_BEHIND: f64 = 1.0;
/// A message in sight at most this long (bytes) is parsed on the main
/// thread when the worker has not got to it: one view, a fraction of a
/// millisecond.
const PARSE_NOW_LIMIT: usize = crate::transcript_parse::WHOLE_LIMIT;
/// Activity runs' text kept, at most.
const ACTIVITY_TEXTS: usize = 2_048;
/// Heights remembered, at most.
const HEIGHT_CACHE: usize = 20_000;
/// Parsed messages remembered, by fingerprint, and the text they may hold
/// in all: a conversation shown again needs no parse to show.
const SEGMENTATIONS: usize = 4096;
const SEGMENTATION_BYTES: usize = 8 << 20;

/// What the transcript asks of the window.
pub(crate) enum TranscriptEvent {
    /// Open the activity row's detail (`omperor-detail:<n>`).
    OpenDetail(usize),
}

/// How the transcript is set, from Settings and the session.
#[derive(Clone, PartialEq)]
pub(crate) struct Settings {
    pub(crate) typography: Typography,
    pub(crate) image_base: PathBuf,
    pub(crate) show_inline_images: bool,
}

/// Where the reader was, to put them back.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Place {
    /// At the end, following it.
    pub(crate) at_bottom: bool,
    /// The message at the top of the view, and how far into it.
    pub(crate) message: usize,
    pub(crate) offset: f64,
}

/// A selection in one message, in the message's own UTF-16 offsets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Selection {
    pub(crate) message: usize,
    pub(crate) location: usize,
    pub(crate) length: usize,
}

// MARK: - Views

define_class!(
    /// The transcript's document view: rows stack from the top.
    // SAFETY: NSView's initialiser is `initWithFrame:`, untouched; the
    // overrides keep AppKit's signatures. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OmperorTranscriptDocumentView"]
    pub(crate) struct TranscriptDocumentView;

    unsafe impl NSObjectProtocol for TranscriptDocumentView {}

    impl TranscriptDocumentView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Dark and light have different style sheets: restyle the rows.
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn view_did_change_effective_appearance(&self) {
            // SAFETY: NSView implements the method; calling it keeps its behaviour.
            let _: () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            with_current(|transcript| transcript.appearance_changed());
        }

        /// A frame of the display link: a step of the held corrections.
        #[unsafe(method(transcriptBleed:))]
        fn transcript_bleed(&self, _link: &AnyObject) {
            with_current(|transcript| transcript.bleed());
        }

        #[unsafe(method(copyTranscriptMessage:))]
        fn copy_transcript_message(&self, sender: &NSMenuItem) {
            let message = sender.tag() as usize;
            with_current(|transcript| transcript.copy_message(message));
        }

        #[unsafe(method(viewTranscriptMessage:))]
        fn view_transcript_message(&self, sender: &NSMenuItem) {
            let message = sender.tag() as usize;
            with_current(|transcript| {
                if let Some(handler) = transcript.events.borrow().as_ref() {
                    handler(TranscriptEvent::OpenDetail(message));
                }
            });
        }

        #[unsafe(method(selectTranscriptMessage:))]
        fn select_transcript_message(&self, sender: &NSMenuItem) {
            let message = sender.tag() as usize;
            with_current(|transcript| transcript.select_message(message));
        }

        #[unsafe(method(copyTranscriptLink:))]
        fn copy_transcript_link(&self, sender: &NSMenuItem) {
            if let Some(link) = sender.representedObject().and_then(|object| object.downcast::<NSString>().ok()) {
                copy_text(&link.to_string());
            }
        }

        /// Copy LaTeX on a formula: its LaTeX alone, without delimiters.
        #[unsafe(method(copyTranscriptLatex:))]
        fn copy_transcript_latex(&self, sender: &NSMenuItem) {
            if let Some(latex) = sender.representedObject().and_then(|object| object.downcast::<NSString>().ok()) {
                copy_text(&latex.to_string());
            }
        }

        #[unsafe(method(openTranscriptLink:))]
        fn open_transcript_link(&self, sender: &NSMenuItem) {
            if let Some(link) = sender.representedObject().and_then(|object| object.downcast::<NSString>().ok()) {
                with_current(|transcript| transcript.open_link(&link.to_string()));
            }
        }
    }
);

define_class!(
    /// One row of the stack: a message's views, or a run of activity rows.
    // SAFETY: as above. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OmperorTranscriptRowView"]
    struct TranscriptRowView;

    unsafe impl NSObjectProtocol for TranscriptRowView {}

    impl TranscriptRowView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

/// The ivars of a label: its text, drawn at its origin.
struct LabelIvars {
    text: RefCell<Option<Retained<objc2_foundation::NSAttributedString>>>,
}

define_class!(
    /// A speaker's name above a message.
    // SAFETY: as above; drawRect: draws inside the bounds. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = LabelIvars]
    #[name = "OmperorTranscriptLabelView"]
    struct TranscriptLabelView;

    unsafe impl NSObjectProtocol for TranscriptLabelView {}

    impl TranscriptLabelView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let _span = perf::span("gui.transcript.draw_label");
            if let Some(text) = self.ivars().text.borrow().as_ref() {
                text.drawAtPoint(NSPoint::new(0.0, 0.0));
            }
        }
    }
);

/// The ivars of an activity run: its text, laid out lazily by a layout
/// manager of its own, and the text view it becomes when first clicked.
struct ActivityIvars {
    storage: Retained<NSTextStorage>,
    layout: Retained<NSLayoutManager>,
    container: Retained<NSTextContainer>,
    text_view: RefCell<Option<Retained<NSTextView>>>,
    delegate: Option<Retained<ProtocolObject<dyn NSTextViewDelegate>>>,
    link_attributes: Option<Retained<NSDictionary<NSString, AnyObject>>>,
}

define_class!(
    /// A run of tool, thinking, and notice rows. Scrolling only draws the
    /// glyphs in the dirty rect from its own layout manager, laid out as far
    /// as drawing needs: no text view until a click wants the text to
    /// select, copy, or follow a link, when a TextKit 1 text view on the same
    /// layout takes its place and the click.
    // SAFETY: as above; drawRect: draws inside the bounds, and the mouse
    // overrides hand the event to the text view they make. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ActivityIvars]
    #[name = "OmperorTranscriptActivityView"]
    struct TranscriptActivityView;

    unsafe impl NSObjectProtocol for TranscriptActivityView {}

    impl TranscriptActivityView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            let _span = perf::span("gui.transcript.draw_activity");
            let ivars = self.ivars();
            let glyphs = ivars.layout.glyphRangeForBoundingRect_inTextContainer(dirty, &ivars.container);
            // The chips stay under the text view a click puts in the run's
            // place; the glyphs are then that view's to draw.
            self.draw_chips(glyphs);
            if ivars.text_view.borrow().is_some() {
                return;
            }
            let origin = NSPoint::new(0.0, 0.0);
            ivars.layout.drawBackgroundForGlyphRange_atPoint(glyphs, origin);
            ivars.layout.drawGlyphsForGlyphRange_atPoint(glyphs, origin);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &objc2_app_kit::NSEvent) {
            self.text_view().mouseDown(event);
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &objc2_app_kit::NSEvent) {
            self.text_view().rightMouseDown(event);
        }
    }
);

impl TranscriptActivityView {
    fn new(
        text: &objc2_foundation::NSAttributedString,
        width: f64,
        delegate: Option<Retained<ProtocolObject<dyn NSTextViewDelegate>>>,
        link_attributes: Option<Retained<NSDictionary<NSString, AnyObject>>>,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let storage = NSTextStorage::new();
        storage.setAttributedString(text);
        // Drawn without a text view, a link would take the system's blue and
        // underline; its target waits under a key of its own until a click
        // makes the text view, which styles links as the run asks.
        move_attribute(&storage, unsafe { NSLinkAttributeName }, &NSString::from_str(PARKED_LINK));
        let layout = NSLayoutManager::new();
        // Laid out from the top as far as a draw reaches, never around it: a
        // non-contiguous layout places the lines it has not reached from
        // estimates, and AppKit draws a run in the parts that come into
        // sight, so a part drawn from estimates stays on screen beside
        // parts drawn after the lines above were laid out — rows over rows,
        // cut where each part ended.
        layout.setAllowsNonContiguousLayout(false);
        let container = NSTextContainer::initWithSize(NSTextContainer::alloc(), NSSize::new(width, 1.0e7));
        container.setLineFragmentPadding(0.0);
        container.setWidthTracksTextView(false);
        layout.addTextContainer(&container);
        storage.addLayoutManager(&layout);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, 1.0));
        let this = Self::alloc(mtm).set_ivars(ActivityIvars {
            storage,
            layout,
            container,
            text_view: RefCell::new(None),
            delegate,
            link_attributes,
        });
        // SAFETY: NSView's designated initialiser takes a frame.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Behind each tool row, a pill hugging its line (tinted by how the call
    /// ended); beside each thinking row, a plum margin rule. Only the rows
    /// in `glyphs`, from the layout drawing already needs.
    fn draw_chips(&self, glyphs: NSRange) {
        let ivars = self.ivars();
        let storage = &ivars.storage;
        let length = storage.length();
        if glyphs.length == 0 || length == 0 {
            return;
        }
        // SAFETY: a null actual range is documented as allowed.
        let characters = unsafe { ivars.layout.characterRangeForGlyphRange_actualGlyphRange(glyphs, std::ptr::null_mut()) };
        let key = CHIP_KEY_STRING.with(|key| key.clone());
        let mut at = characters.location;
        let end = (characters.location + characters.length).min(length);
        // The drawn range is whole lines, and a chip is a line or a
        // paragraph: runs are looked for only inside it.
        let within = NSRange::new(characters.location, end - characters.location);
        while at < end {
            let mut run = NSRange::new(0, 0);
            // SAFETY: `at` lies inside the string; `run` is written by the call.
            let value = unsafe {
                storage.attribute_atIndex_longestEffectiveRange_inRange(&key, at, &mut run, within)
            };
            let next = (run.location + run.length).max(at + 1);
            if let Some(kind) = value.and_then(|value| value.downcast::<NSString>().ok()) {
                // SAFETY: as above.
                let chip_glyphs = unsafe { ivars.layout.glyphRangeForCharacterRange_actualCharacterRange(run, std::ptr::null_mut()) };
                let rect = ivars.layout.boundingRectForGlyphRange_inTextContainer(chip_glyphs, &ivars.container);
                draw_chip(&kind.to_string(), rect, self.bounds().size.width);
            }
            at = next;
        }
    }

    /// The run's live marks, over the status glyphs the text draws: a gold
    /// arc turning round a running call's dotted ring, and a bloom — the
    /// press's gold seed, spent — from a call in `landed` (row ids) that
    /// has just ended. Core Animation turns and blooms them; the text is
    /// never drawn again for it.
    fn animate_chips(&self, landed: &[u64]) {
        let ivars = self.ivars();
        let storage = &ivars.storage;
        let length = storage.length();
        let key = NSString::from_str(native_markdown::CHIP_KEY);
        let parked = NSString::from_str(PARKED_LINK);
        let mut at = 0;
        while at < length {
            let mut run = NSRange::new(0, 0);
            // SAFETY: `at` lies inside the string; `run` is written by the call.
            let value = unsafe { storage.attribute_atIndex_longestEffectiveRange_inRange(&key, at, &mut run, NSRange::new(0, length)) };
            at = (run.location + run.length).max(at + 1);
            let Some(kind) = value.and_then(|value| value.downcast::<NSString>().ok()).map(|kind| kind.to_string()) else { continue };
            let kind = kind.rsplit(':').next().unwrap_or_default().to_owned();
            let blooming = (kind == "done" || kind == "failed") && !landed.is_empty() && {
                // SAFETY: as above; the parked link is an NSURL.
                let link = unsafe { storage.attribute_atIndex_effectiveRange(&parked, run.location, std::ptr::null_mut()) };
                let id = link
                    .and_then(|link| link.downcast::<NSURL>().ok())
                    .and_then(|url| url.absoluteString())
                    .and_then(|url| url.to_string().rsplit(':').next().and_then(|id| id.parse::<u64>().ok()));
                id.is_some_and(|id| landed.contains(&id))
            };
            if kind != "running" && !blooming {
                continue;
            }
            // SAFETY: a null actual range is documented as allowed.
            let glyph = unsafe { ivars.layout.glyphRangeForCharacterRange_actualCharacterRange(NSRange::new(run.location, 1), std::ptr::null_mut()) };
            let mark = ivars.layout.boundingRectForGlyphRange_inTextContainer(glyph, &ivars.container);
            if mark.size.width <= 0.0 {
                continue;
            }
            let color = if kind == "failed" { theme::danger() } else if kind == "done" { theme::success() } else { theme::accent() };
            let running = kind == "running";
            let side = if running { mark.size.width.max(mark.size.height) + 2.0 } else { mark.size.width.max(mark.size.height) * 2.6 };
            let center = NSPoint::new(mark.origin.x + mark.size.width / 2.0, mark.origin.y + mark.size.height / 2.0);
            let frame = NSRect::new(NSPoint::new(center.x - side / 2.0, center.y - side / 2.0), NSSize::new(side, side));
            if let Some(overlay) = chip_overlay(frame, running, color, self.mtm()) {
                self.addSubview(&overlay);
            }
        }
    }

    /// The height of the whole text, laid out: only when no height is
    /// remembered for it.
    fn measure(&self) -> f64 {
        let _span = perf::span("gui.transcript.measure_activity");
        let ivars = self.ivars();
        ivars.layout.ensureLayoutForTextContainer(&ivars.container);
        ivars.layout.usedRectForTextContainer(&ivars.container).size.height.ceil()
    }

    /// The height of the whole text, if it is laid out whole already.
    fn laid_out_height(&self) -> Option<f64> {
        let ivars = self.ivars();
        let whole = !ivars.layout.hasNonContiguousLayout() && ivars.layout.firstUnlaidCharacterIndex() >= ivars.storage.length();
        whole.then(|| ivars.layout.usedRectForTextContainer(&ivars.container).size.height.ceil())
    }

    /// The text view in the run's place, made the first time it is wanted,
    /// on the layout the run already has.
    fn text_view(&self) -> Retained<NSTextView> {
        if let Some(view) = self.ivars().text_view.borrow().as_ref() {
            return view.clone();
        }
        let _span = perf::span("gui.transcript.activity_text_view");
        let ivars = self.ivars();
        let view = NSTextView::initWithFrame_textContainer(NSTextView::alloc(self.mtm()), self.bounds(), Some(&ivars.container));
        view.setEditable(false);
        view.setSelectable(true);
        view.setRichText(true);
        view.setDrawsBackground(false);
        view.setTextContainerInset(NSSize::new(0.0, 0.0));
        view.setVerticallyResizable(false);
        view.unregisterDraggedTypes();
        if let Some(delegate) = ivars.delegate.as_ref() {
            view.setDelegate(Some(delegate));
        }
        if let Some(attributes) = ivars.link_attributes.as_ref() {
            // SAFETY: the keys are AppKit attribute names paired with the
            // value types AppKit documents for them.
            unsafe { view.setLinkTextAttributes(Some(attributes)) };
        }
        move_attribute(&ivars.storage, &NSString::from_str(PARKED_LINK), unsafe { NSLinkAttributeName });
        self.addSubview(&view);
        *ivars.text_view.borrow_mut() = Some(view.clone());
        view
    }
}

/// The ivars of a fill: its colour, and for the user's bubble its corners
/// and edge.
struct FillIvars {
    color: RefCell<Retained<NSColor>>,
    radius: Cell<f64>,
    corners: Cell<usize>,
    edge: RefCell<Option<Retained<NSColor>>>,
}

define_class!(
    /// A solid colour, as a layer's background: the user's box and its rule.
    // SAFETY: as above; updateLayer only sets the layer's colour. No Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = FillIvars]
    #[name = "OmperorTranscriptFillView"]
    struct TranscriptFillView;

    unsafe impl NSObjectProtocol for TranscriptFillView {}

    impl TranscriptFillView {
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            let Some(layer) = self.layer() else { return };
            let color = self.ivars().color.borrow().clone();
            let edge = self.ivars().edge.borrow().clone();
            let appearance = self.effectiveAppearance();
            let resolved = Cell::new(None);
            let resolved_edge = Cell::new(None);
            let resolve = block2::RcBlock::new(|| {
                resolved.set(Some(color.CGColor()));
                resolved_edge.set(edge.as_ref().map(|edge| edge.CGColor()));
            });
            appearance.performAsCurrentDrawingAppearance(&resolve);
            if let Some(cg) = resolved.take() {
                layer.setBackgroundColor(Some(&cg));
            }
            let radius = self.ivars().radius.get();
            if radius > 0.0 {
                layer.setCornerRadius(radius);
                // SAFETY: maskedCorners takes a CACornerMask, an NSUInteger.
                let _: () = unsafe { msg_send![&*layer, setMaskedCorners: self.ivars().corners.get()] };
            }
            if let Some(cg) = resolved_edge.take() {
                layer.setBorderWidth(1.0);
                layer.setBorderColor(Some(&cg));
            }
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn view_did_change_effective_appearance(&self) {
            // SAFETY: NSView implements the method; calling it keeps its behaviour.
            let _: () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            self.setNeedsDisplay(true);
        }
    }
);

impl TranscriptLabelView {
    fn new(text: Retained<objc2_foundation::NSAttributedString>, frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(LabelIvars { text: RefCell::new(Some(text)) });
        // SAFETY: NSView's designated initialiser takes a frame.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

impl TranscriptFillView {
    fn new(color: Retained<NSColor>, frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        Self::rounded(color, None, 0.0, 0, frame, mtm)
    }

    /// The user's bubble: a fill with rounded `corners` and a fine edge.
    fn bubble(segment: usize, count: usize, frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        Self::rounded(theme::user_bubble(), Some(theme::user_bubble_edge()), USER_RADIUS, user_corners(segment, count), frame, mtm)
    }

    fn rounded(color: Retained<NSColor>, edge: Option<Retained<NSColor>>, radius: f64, corners: usize, frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FillIvars {
            color: RefCell::new(color),
            radius: Cell::new(radius),
            corners: Cell::new(corners),
            edge: RefCell::new(edge),
        });
        // SAFETY: NSView's designated initialiser takes a frame.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setWantsLayer(true);
        view
    }
}

/// Where an activity run keeps its links while it draws itself.
const PARKED_LINK: &str = "OmperorParkedLink";

/// Moves every run of attribute `from` in `storage` to `to`, values kept.
fn move_attribute(storage: &NSTextStorage, from: &NSString, to: &NSString) {
    let length = storage.length();
    let mut runs = Vec::new();
    let mut at = 0;
    while at < length {
        let mut run = NSRange::new(0, 0);
        // SAFETY: `at` lies inside the string; `run` is written by the call.
        let value = unsafe { storage.attribute_atIndex_longestEffectiveRange_inRange(from, at, &mut run, NSRange::new(0, length)) };
        if let Some(value) = value {
            runs.push((run, value));
        }
        at = (run.location + run.length).max(at + 1);
    }
    if runs.is_empty() {
        return;
    }
    storage.beginEditing();
    for (run, value) in runs {
        // SAFETY: the value moves between two keys that take the same type.
        unsafe {
            storage.removeAttribute_range(from, run);
            storage.addAttribute_value_range(to, &value, run);
        }
    }
    storage.endEditing();
}

/// A layer-hosting view over a chip's status glyph: an arc that turns
/// while the call runs, or a ring that blooms out and fades as it ends.
/// Only its sublayer moves; the view is inert to the mouse.
fn chip_overlay(frame: NSRect, running: bool, color: Retained<NSColor>, mtm: MainThreadMarker) -> Option<Retained<NSView>> {
    use objc2::runtime::AnyClass;
    use objc2_app_kit::{NSBezierPath, NSLineCapStyle};
    let layer_class = AnyClass::get(c"CALayer")?;
    let side = frame.size.width;
    let handler = block2::RcBlock::new(move |bounds: NSRect| -> objc2::runtime::Bool {
        let inset = if running { 1.6 } else { 1.0 };
        let radius = bounds.size.width / 2.0 - inset;
        let center = NSPoint::new(bounds.size.width / 2.0, bounds.size.height / 2.0);
        let path = NSBezierPath::bezierPath();
        if running {
            path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(center, radius, 90.0, 200.0);
            path.setLineWidth(1.8);
        } else {
            path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(center, radius, 0.0, 360.0);
            path.setLineWidth(1.4);
        }
        path.setLineCapStyle(NSLineCapStyle::Round);
        color.setStroke();
        path.stroke();
        objc2::runtime::Bool::YES
    });
    let image = objc2_app_kit::NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(side, side), false, &handler);
    let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
    // SAFETY: CALayer's `layer` makes a plain layer; the setters take the
    // documented types; an NSImage is valid layer contents on macOS.
    unsafe {
        let host: Retained<AnyObject> = msg_send![layer_class, layer];
        let mark: Retained<AnyObject> = msg_send![layer_class, layer];
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(side, side));
        let _: () = msg_send![&*mark, setFrame: bounds];
        let _: () = msg_send![&*mark, setContents: &*image];
        let _: () = msg_send![&*host, addSublayer: &*mark];
        let _: () = msg_send![&view, setLayer: &*host];
        view.setWantsLayer(true);
        if running {
            crate::motion::spin(&mark);
        } else {
            // Spent after the bloom: its resting state is invisible.
            let _: () = msg_send![&*mark, setOpacity: 0.0f32];
            crate::motion::bloom(&mark);
        }
    }
    Some(view)
}

/// The streaming orb: a gold dot on a layer of its own, breathing.
fn streaming_orb(frame: NSRect, mtm: MainThreadMarker) -> Option<Retained<NSView>> {
    use objc2::runtime::AnyClass;
    let layer_class = AnyClass::get(c"CALayer")?;
    let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
    let gold = Cell::new(None);
    let appearance = view.effectiveAppearance();
    let resolve = block2::RcBlock::new(|| gold.set(Some(theme::accent().CGColor())));
    appearance.performAsCurrentDrawingAppearance(&resolve);
    // SAFETY: CALayer's `layer` makes a plain layer; the setters take the
    // documented types.
    unsafe {
        let host: Retained<AnyObject> = msg_send![layer_class, layer];
        let dot: Retained<AnyObject> = msg_send![layer_class, layer];
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), frame.size);
        let _: () = msg_send![&*dot, setFrame: bounds];
        let _: () = msg_send![&*dot, setCornerRadius: frame.size.width / 2.0];
        if let Some(gold) = gold.take() {
            let _: () = msg_send![&*dot, setBackgroundColor: &*gold];
        }
        let _: () = msg_send![&*host, addSublayer: &*dot];
        let _: () = msg_send![&view, setLayer: &*host];
        view.setWantsLayer(true);
        crate::motion::breathe(&dot);
    }
    Some(view)
}

thread_local! {
    /// The chip attribute's name, made once rather than every draw.
    static CHIP_KEY_STRING: Retained<NSString> = NSString::from_str(native_markdown::CHIP_KEY);
    /// The chips' colours, made once: each resolves per appearance as drawn.
    static CHIP_COLORS: [Retained<NSColor>; 7] = [
        theme::plum_soft(),
        theme::chip_fill(),
        theme::danger_fill(),
        theme::accent_fill(),
        theme::accent().colorWithAlphaComponent(0.35),
        theme::danger(),
        theme::hairline(),
    ];
}

/// One of the chips' colours: 0 the thinking rule, 1 a chip, 2 a failed
/// chip, 3 a running chip, 4 its edge, 5 rose, 6 a hairline.
fn chip_color(index: usize) -> Retained<NSColor> {
    CHIP_COLORS.with(|colors| colors[index].clone())
}

/// One chip, in the activity view's flipped coordinates: a pill around a
/// tool row's text, or a margin rule down a thinking row.
fn draw_chip(kind: &str, text: NSRect, width: f64) {
    use objc2_app_kit::NSBezierPath;
    // Editorial's and Terminal's chips carry their style: "terminal:done".
    let (style, kind) = kind.split_once(':').unwrap_or(("modern", kind));
    if style == "editorial" && kind != "thinking" {
        // The book's margin: no box, only a hairline under a failure.
        if kind == "failed" {
            chip_color(5).colorWithAlphaComponent(0.45).setFill();
            NSBezierPath::fillRect(NSRect::new(NSPoint::new(text.origin.x, text.origin.y + text.size.height), NSSize::new(text.size.width, 1.0)));
        }
        return;
    }
    if style == "terminal" && kind != "thinking" {
        // A bracketed line: a square outline, lit gold while it runs.
        let pad = native_markdown::CHIP_INDENT - 4.0;
        let x = (text.origin.x - pad).max(0.5);
        let right = (text.origin.x + text.size.width + pad).min(width - 0.5);
        let frame = NSRect::new(NSPoint::new(x + 0.5, text.origin.y - 2.5), NSSize::new((right - x - 1.0).max(0.0), text.size.height + 5.0));
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(frame, 3.0, 3.0);
        path.setLineWidth(1.0);
        match kind {
            "failed" => chip_color(5).colorWithAlphaComponent(0.55).setStroke(),
            "running" => chip_color(4).setStroke(),
            _ => chip_color(6).setStroke(),
        }
        path.stroke();
        return;
    }
    if kind == "thinking" {
        let rule = NSRect::new(NSPoint::new(1.0, text.origin.y + 1.0), NSSize::new(2.0, (text.size.height - 2.0).max(2.0)));
        chip_color(0).setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rule, 1.0, 1.0).fill();
        return;
    }
    let pad = native_markdown::CHIP_INDENT - 2.0;
    let x = (text.origin.x - pad).max(0.5);
    let right = (text.origin.x + text.size.width + pad).min(width - 0.5);
    let pill = NSRect::new(NSPoint::new(x, text.origin.y - 3.0), NSSize::new((right - x).max(0.0), text.size.height + 6.0));
    let radius = (pill.size.height / 2.0).min(11.0);
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(pill, radius, radius);
    match kind {
        "failed" => {
            chip_color(2).setFill();
            path.fill();
        }
        "running" => {
            chip_color(3).setFill();
            path.fill();
            chip_color(4).setStroke();
            path.setLineWidth(1.0);
            path.stroke();
        }
        _ => {
            chip_color(1).setFill();
            path.fill();
        }
    }
}

impl TranscriptRowView {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: NSView's designated initialiser takes a frame.
        unsafe { msg_send![Self::alloc(mtm), initWithFrame: frame] }
    }
}

impl TranscriptDocumentView {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: NSView's designated initialiser takes a frame.
        unsafe { msg_send![Self::alloc(mtm), initWithFrame: frame] }
    }
}

// MARK: - Model

/// One message.
struct Row {
    /// Unique for the transcript's life: worker answers name rows by it.
    id: u64,
    fingerprint: u64,
    message: TranscriptMessage,
    markdown: Option<MarkdownRow>,
    /// When a message added below the others arrived: its view settles in
    /// on a spring if it is made soon after. None for rows set whole.
    arrived: Option<Instant>,
    /// When a tool call that was running ended, this row replacing its
    /// row: its chip blooms once if it is made soon after.
    landed: Option<Instant>,
}

/// A row made this soon after it arrived settles in; later it just shows.
const SETTLE_WINDOW: Duration = Duration::from_millis(1_500);

struct MarkdownRow {
    /// The views the message is set as, once the worker has parsed it.
    segments: Option<Rc<[Rc<Segment>]>>,
    /// The worker has been asked for them.
    requested: bool,
    /// The message being streamed into, or one that was.
    live: Option<Live>,
}

/// A message streamed into one view, append by append.
struct Live {
    /// Heads sealed off the tail as it grew (`transcript_parse::Seal`),
    /// each its own view, with what the worker needs of it. Only a sealed
    /// segment's parse changes, when what the rest gives it does.
    sealed: Vec<(Rc<Segment>, SealedSource)>,
    /// Where the tail starts in the message, in UTF-16 units and bytes.
    tail_start: usize,
    tail_byte: usize,
    /// The tail the view shows, and its parse; `None` before the first.
    applied: Option<(String, Arc<ParsedDocument>)>,
    /// A parse is on the worker.
    in_flight: bool,
    /// The next job must be answered wholesale: no view has the last parse.
    reset: bool,
    /// The conversation still streams into it.
    streaming: bool,
    /// A sealed segment holds a heading: the tail's view is a continuation.
    heading_before: bool,
    /// The sealed segments as the worker needs them, for every job.
    sources: Arc<[SealedSource]>,
}

impl Live {
    fn new() -> Live {
        Live {
            sealed: Vec::new(),
            tail_start: 0,
            tail_byte: 0,
            applied: None,
            in_flight: false,
            reset: true,
            streaming: true,
            heading_before: false,
            sources: Arc::from(Vec::new()),
        }
    }

    /// The tail must start inside the message: a message that was replaced
    /// rather than grown starts streaming afresh.
    fn check_tail(&mut self, body: &str) {
        if body.len() >= self.tail_byte && body.is_char_boundary(self.tail_byte) {
            return;
        }
        let streaming = self.streaming;
        *self = Live::new();
        self.streaming = streaming;
    }

    /// The job for the tail as the message reads now.
    fn job(&mut self, row: u64, generation: u64, body: &str) -> Job {
        self.in_flight = true;
        let reset = std::mem::replace(&mut self.reset, false);
        Job::Stream {
            row,
            generation,
            start: self.tail_start,
            text: body[self.tail_byte..].to_owned(),
            reset,
            heading_before: self.heading_before,
            sealed: self.sources.clone(),
        }
    }

    /// `sources` again, after `sealed` changed.
    fn refresh_sources(&mut self) {
        self.sources = self.sealed.iter().map(|(_, source)| source.clone()).collect();
    }
}

/// A worker's answer for a streamed tail (`Done::Streamed`).
struct Streamed {
    start: usize,
    text: String,
    document: Arc<ParsedDocument>,
    dirty: DirtySet,
    seal: Option<crate::transcript_parse::Seal>,
    updates: Vec<crate::transcript_parse::SealedUpdate>,
    unseal: Option<usize>,
}

#[derive(Debug, Clone)]
enum PieceKind {
    /// Segment `segment` of `count` of a user or assistant message.
    Markdown { row: usize, segment: usize, count: usize },
    /// A run of activity rows.
    Activity { rows: Range<usize> },
}

/// One view's worth of the stack.
struct Piece {
    kind: PieceKind,
    /// What it shows: views and heights are kept by it.
    key: u64,
    y: f64,
    /// Its own height: measured, remembered or estimated.
    height: f64,
    exact: bool,
    view: Option<RowViews>,
    /// A correction held for the scroll bar (`scroll_reserve`), on a piece
    /// with no view far from the reader; laid out at `shown`.
    reserve: f64,
    /// Laid out by a restack: its height has been in the document.
    placed: bool,
}

impl Piece {
    /// The height it is laid out at.
    fn shown(&self) -> f64 {
        scroll_reserve::shown(self.height, self.reserve)
    }
}

impl scroll_reserve::Rows for Vec<Piece> {
    fn base(&self, index: usize) -> f64 {
        self[index].height
    }
    fn reserve(&self, index: usize) -> f64 {
        self[index].reserve
    }
    fn set_reserve(&mut self, index: usize, reserve: f64) {
        self[index].reserve = reserve;
    }
}

struct RowViews {
    container: Retained<TranscriptRowView>,
    content: Content,
    /// The column width it was laid out at.
    width: f64,
}

enum Content {
    Markdown {
        view: Retained<MarkdownTextView>,
        storage: Retained<NSTextStorage>,
        /// The view's offset from the top of the row.
        top: f64,
        /// The space the row adds around the view: above, below.
        extra: f64,
        /// Of `top`, the space a continuing segment's first heading asks for.
        gap: f64,
        /// The part of `top` the view's text takes below the view's own top
        /// (`set_hosted_content_offset`): a continuing segment's view sits on
        /// a whole device pixel from its message's first, and its text where
        /// one view's would be.
        offset: f64,
        /// For the height cache: the text's key.
        text_key: u64,
    },
    Activity,
}

/// Where the rows go across the pane.
#[derive(Clone, Copy, PartialEq)]
struct Column {
    x: f64,
    width: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct HeightKey {
    text: u64,
    width: u64,
    metrics: u64,
}

/// A place in the transcript's content that can be found again after the
/// rows around it, its own text's layout, or the width changed: what keeps
/// the reader's place.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mark {
    /// A line of a message: the paragraph at UTF-16 `offset` of the
    /// message, and where the line starts in the paragraph as laid out.
    Line { row: u64, offset: usize, element: isize, character: isize },
    /// The top of the piece of a message that starts at `offset`.
    Piece { row: u64, offset: usize },
    /// The top of the activity piece that holds row `row`.
    Activity { row: u64 },
}

impl Mark {
    fn row(self) -> u64 {
        match self {
            Mark::Line { row, .. } | Mark::Piece { row, .. } | Mark::Activity { row } => row,
        }
    }

    fn with_row(self, id: u64) -> Mark {
        match self {
            Mark::Line { offset, element, character, .. } => Mark::Line { row: id, offset, element, character },
            Mark::Piece { offset, .. } => Mark::Piece { row: id, offset },
            Mark::Activity { .. } => Mark::Activity { row: id },
        }
    }

    fn name(self) -> String {
        match self {
            Mark::Line { row, offset, element, character } => format!("L{row}:{offset}:{element}:{character}"),
            Mark::Piece { row, offset } => format!("P{row}:{offset}"),
            Mark::Activity { row } => format!("A{row}"),
        }
    }

    fn parse(name: &str) -> Option<Mark> {
        let (kind, rest) = name.split_at_checked(1)?;
        let mut parts = rest.split(':');
        let row = parts.next()?.parse().ok()?;
        let mut offset = || parts.next().and_then(|offset| offset.parse().ok());
        match kind {
            "L" => {
                let offset = offset()?;
                let element = parts.next()?.parse().ok()?;
                Some(Mark::Line { row, offset, element, character: parts.next()?.parse().ok()? })
            }
            "P" => Some(Mark::Piece { row, offset: offset()? }),
            "A" => Some(Mark::Activity { row }),
            _ => None,
        }
    }
}

struct State {
    settings: Option<Settings>,
    styles: Option<Styles>,
    rows: Vec<Row>,
    pieces: Vec<Piece>,
    column: Column,
    heights: HashMap<HeightKey, f64>,
    /// Activity runs' text, by what their height is remembered by.
    activity_texts: HashMap<(HeightKey, u64), Retained<objc2_foundation::NSAttributedString>>,
    /// Parsed messages by fingerprint, with when each was last used.
    segmentations: HashMap<u64, (Rc<[Rc<Segment>]>, u64)>,
    segmentation_bytes: usize,
    clock: u64,
    next_id: u64,
    /// Bumped when the rows are replaced: older worker answers are stale.
    generation: u64,
    /// A row whose views the benchmark keeps made, to read them back.
    pinned: Option<usize>,
    /// A selection to set when its message gets a view.
    pending_selection: Option<Selection>,
    /// The heights the session had when last shown (`saved_heights`).
    saved: Option<SavedState>,
    /// The session's history is still arriving, older pages above.
    history_loading: bool,
    /// While it does: the saved height of the history not here yet, laid
    /// out above the first row so the document has its whole height.
    above: f64,
}

/// A session's saved heights, as looked up.
struct SavedState {
    width: u64,
    metrics: u64,
    total: f64,
    pieces: HashMap<u64, f64>,
    rows: HashMap<u64, f64>,
}

pub(crate) struct Transcript {
    this: Weak<Transcript>,
    mtm: MainThreadMarker,
    scroll: Retained<NSScrollView>,
    document: Retained<TranscriptDocumentView>,
    state: RefCell<State>,
    /// Inside a batch of changes: height reports wait for its end.
    busy: Cell<bool>,
    /// How deep the batches are nested.
    depth: Cell<u32>,
    /// Height reports that arrived during a batch.
    reported: RefCell<Vec<usize>>,
    /// The Markdown dump's equivalence check under way.
    equivalence: RefCell<Option<EquivalenceCheck>>,
    /// Whether the reader was at the end when the batch began.
    following: Cell<Option<bool>>,
    /// Otherwise the content the reader saw at the top of the view then,
    /// and how far below it the view began: it keeps that place on screen.
    anchor: Cell<Option<(Mark, f64)>>,
    /// Where the reader last scrolled the view to, and which way it went:
    /// views are made ahead of the reader in that direction.
    last_origin: Cell<f64>,
    direction: Cell<f64>,
    /// A pass of view making is scheduled.
    work_scheduled: Cell<bool>,
    /// The pane changed width while the rows were being placed.
    viewport_pending: Cell<bool>,
    delegate: OnceCell<Rc<RowDelegate>>,
    parser: Parser,
    events: RefCell<Option<Box<dyn Fn(TranscriptEvent)>>>,
    text_delegate: RefCell<Option<Retained<ProtocolObject<dyn NSTextViewDelegate>>>>,
    /// The streaming orb: a gold dot pulsing under the message being
    /// streamed, in its trailing space, for as long as it streams.
    orb: RefCell<Option<Retained<NSView>>>,
    link_attributes: OnceCell<Retained<NSDictionary<NSString, AnyObject>>>,
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    /// Gives held corrections back a step a frame while any are held.
    ticker: RefCell<Option<Ticker>>,
    /// When the last step was given back.
    bleed_last: Cell<Option<Instant>>,
    /// A width, type or appearance change is being applied in this batch:
    /// heights it changes are the content's own, not corrections to hold.
    restyling: Cell<bool>,
}

/// What drives the give-back: the display, or a timer for a window on no
/// screen (the headless benchmark's).
enum Ticker {
    Display(Retained<objc2_quartz_core::CADisplayLink>),
    Timer(Retained<NSTimer>),
}

impl std::fmt::Debug for Transcript {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.try_borrow();
        formatter
            .debug_struct("Transcript")
            .field("rows", &state.as_ref().map_or(0, |state| state.rows.len()))
            .field("pieces", &state.as_ref().map_or(0, |state| state.pieces.len()))
            .finish()
    }
}

thread_local! {
    static CURRENT: RefCell<Weak<Transcript>> = const { RefCell::new(Weak::new()) };
    /// A layout manager for measuring lines of TextKit 1 text.
    static MEASURE: OnceCell<Retained<NSLayoutManager>> = const { OnceCell::new() };
}

fn with_current(body: impl FnOnce(&Transcript)) {
    if let Some(transcript) = CURRENT.with(|current| current.borrow().upgrade()) {
        body(&transcript);
    }
}

/// The worker's wake-up, on the main thread.
fn drain_parses() {
    with_current(|transcript| transcript.drain());
}

impl Transcript {
    /// The transcript in `frame`, as a scroll view to add to the pane.
    pub(crate) fn new(frame: NSRect, mtm: MainThreadMarker) -> Rc<Transcript> {
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        // The pane's canvas, drawn by the clip view: the rows draw no
        // background of their own, and text is set on an opaque page.
        scroll.setDrawsBackground(true);
        scroll.setBackgroundColor(&theme::canvas());
        scroll.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let size = scroll.contentSize();
        let document = TranscriptDocumentView::new(NSRect::new(NSPoint::new(0.0, 0.0), size), mtm);
        document.setAccessibilityLabel(Some(ns_string!("Conversation")));
        scroll.setDocumentView(Some(&document));
        let transcript = Rc::new_cyclic(|this| Transcript {
            this: this.clone(),
            mtm,
            scroll,
            document,
            state: RefCell::new(State {
                settings: None,
                styles: None,
                rows: Vec::new(),
                pieces: Vec::new(),
                column: Column { x: 0.0, width: 0.0 },
                heights: HashMap::new(),
                activity_texts: HashMap::new(),
                segmentations: HashMap::new(),
                segmentation_bytes: 0,
                clock: 0,
                next_id: 1,
                generation: 1,
                pinned: None,
                pending_selection: None,
                saved: None,
                history_loading: false,
                above: 0.0,
            }),
            busy: Cell::new(false),
            depth: Cell::new(0),
            reported: RefCell::new(Vec::new()),
            equivalence: RefCell::new(None),
            following: Cell::new(None),
            anchor: Cell::new(None),
            last_origin: Cell::new(0.0),
            direction: Cell::new(0.0),
            work_scheduled: Cell::new(false),
            viewport_pending: Cell::new(false),
            delegate: OnceCell::new(),
            parser: Parser::start(drain_parses),
            events: RefCell::new(None),
            text_delegate: RefCell::new(None),
            orb: RefCell::new(None),
            link_attributes: OnceCell::new(),
            observers: RefCell::new(Vec::new()),
            ticker: RefCell::new(None),
            bleed_last: Cell::new(None),
            restyling: Cell::new(false),
        });
        let _ = transcript.delegate.set(Rc::new(RowDelegate { transcript: Rc::downgrade(&transcript) }));
        // What AppKit's display cycle does for the transcript is named in
        // the frame-budget ledger, always.
        crate::display_spans::wrap(
            MarkdownTextView::class(),
            "gui.transcript.message_layout",
            "gui.transcript.message_will_draw",
            "gui.transcript.message_draw",
        );
        crate::display_spans::wrap(
            TranscriptDocumentView::class(),
            "gui.transcript.document_layout",
            "gui.transcript.will_draw",
            "gui.transcript.document_draw",
        );
        // The headless benchmark counts AppKit's draws of the rows.
        crate::markdown_bench::count_row_draws(MarkdownTextView::class());
        crate::markdown_bench::count_row_draws(TranscriptLabelView::class());
        CURRENT.with(|current| *current.borrow_mut() = Rc::downgrade(&transcript));
        // Scrolling makes the rows coming into view.
        let clip = transcript.scroll.contentView();
        clip.setPostsBoundsChangedNotifications(true);
        let weak = Rc::downgrade(&transcript);
        let block = block2::RcBlock::new(move |_note: std::ptr::NonNull<NSNotification>| {
            if let Some(transcript) = weak.upgrade() {
                transcript.scrolled();
            }
        });
        // SAFETY: the name is an AppKit static; the block runs where the
        // clip view posts, on the main thread, and holds the transcript weakly.
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
                None,
                &block,
            )
        };
        transcript.observers.borrow_mut().push(observer);
        transcript
    }

    pub(crate) fn scroll_view(&self) -> &Retained<NSScrollView> {
        &self.scroll
    }

    pub(crate) fn set_event_handler(&self, handler: impl Fn(TranscriptEvent) + 'static) {
        *self.events.borrow_mut() = Some(Box::new(handler));
    }

    /// The delegate of the activity rows' text views, which opens their
    /// detail links.
    pub(crate) fn set_text_delegate(&self, delegate: Retained<ProtocolObject<dyn NSTextViewDelegate>>) {
        *self.text_delegate.borrow_mut() = Some(delegate);
    }

    // MARK: Showing a conversation

    /// Forgets every row: another conversation is about to be shown.
    pub(crate) fn reset(&self) {
        let _span = perf::span("gui.transcript.reset");
        self.direction.set(0.0);
        let mut state = self.state.borrow_mut();
        for row in &state.rows {
            if row.markdown.as_ref().is_some_and(|markdown| markdown.live.is_some()) {
                self.parser.submit(Job::Forget { row: row.id });
            }
        }
        for piece in &mut state.pieces {
            if let Some(views) = piece.view.take() {
                views.container.removeFromSuperview();
            }
        }
        state.rows.clear();
        state.pieces.clear();
        state.generation += 1;
        state.pinned = None;
        state.pending_selection = None;
        state.saved = None;
        state.above = 0.0;
    }

    /// Shows `conversation` as it is now. Only what changed since the last
    /// call is read again.
    pub(crate) fn sync(&self, conversation: &mut Conversation, settings: Settings) {
        let _span = perf::span("gui.render_chat_transcript");
        self.begin();
        let changes = conversation.take_changes();
        self.apply_settings(settings);
        let streaming = conversation.streaming_index();
        let count = conversation.len();
        {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            let old_rows = std::mem::take(&mut state.rows);
            let fingerprint_span = perf::span("gui.transcript.fingerprints");
            let mut rows: Vec<Option<Row>> = Vec::with_capacity(count);
            // History put above the rows: only the new messages are read.
            let (changes, old_rows) = match changes {
                crate::chat::Changes::Prepended(added) if added + old_rows.len() == count => {
                    for index in 0..added {
                        let Some(message) = conversation.message(index) else { continue };
                        let print = fingerprint(&message);
                        rows.push(Some(new_row(state, message, print)));
                    }
                    rows.extend(old_rows.into_iter().map(Some));
                    (None, Vec::new())
                }
                crate::chat::Changes::Prepended(_) => (Some(crate::chat::Changes::All), old_rows),
                changes => (Some(changes), old_rows),
            };
            match &changes {
                None => {}
                Some(crate::chat::Changes::Prepended(_)) => unreachable!("read above"),
                Some(crate::chat::Changes::All) => {
                    // Keep the rows that are still there: history loaded above
                    // them, or messages added below.
                    let messages: Vec<TranscriptMessage> =
                        (0..count).filter_map(|index| conversation.message(index)).collect();
                    let fingerprints: Vec<u64> = messages.iter().map(fingerprint).collect();
                    let mut old = old_rows.into_iter().map(Some).collect::<Vec<_>>();
                    let prepended = count >= old.len()
                        && !old.is_empty()
                        && old
                            .iter()
                            .zip(&fingerprints[count - old.len()..])
                            .all(|(row, fingerprint)| row.as_ref().is_some_and(|row| row.fingerprint == *fingerprint));
                    let shift = if prepended { count - old.len() } else { 0 };
                    for (index, message) in messages.into_iter().enumerate() {
                        let reuse = index
                            .checked_sub(shift)
                            .and_then(|old_index| old.get_mut(old_index))
                            .and_then(|slot| slot.take_if(|row| row.fingerprint == fingerprints[index]));
                        rows.push(Some(reuse.unwrap_or_else(|| {
                            new_row(state, message, fingerprints[index])
                        })));
                    }
                    for row in old.into_iter().flatten() {
                        if row.markdown.as_ref().is_some_and(|markdown| markdown.live.is_some()) {
                            self.parser.submit(Job::Forget { row: row.id });
                        }
                    }
                }
                Some(changes @ crate::chat::Changes::Messages(_)) => {
                    let mut old = old_rows.into_iter();
                    for index in 0..count {
                        let kept = old.next();
                        let changed = changes.covers(index) || kept.is_none();
                        match kept {
                            Some(row) if !changed => rows.push(Some(row)),
                            kept => {
                                let Some(message) = conversation.message(index) else { continue };
                                let print = fingerprint(&message);
                                rows.push(Some(match kept {
                                    Some(row) if row.fingerprint == print => row,
                                    // The streamed message keeps its row and view.
                                    Some(mut row)
                                        if row.markdown.as_ref().is_some_and(|markdown| markdown.live.is_some())
                                            && row.message.kind == message.kind =>
                                    {
                                        row.fingerprint = print;
                                        row.message = message;
                                        row
                                    }
                                    Some(row) => {
                                        if row.markdown.as_ref().is_some_and(|markdown| markdown.live.is_some()) {
                                            self.parser.submit(Job::Forget { row: row.id });
                                        }
                                        let was_running = row.message.status == Some(crate::chat::ToolStatus::Running);
                                        let mut replaced = new_row(state, message, print);
                                        let ended = replaced.message.status.is_some_and(|status| status != crate::chat::ToolStatus::Running);
                                        replaced.landed = (was_running && ended).then(Instant::now);
                                        replaced
                                    }
                                    None => {
                                        let mut added = new_row(state, message, print);
                                        added.arrived = Some(Instant::now());
                                        added
                                    }
                                }));
                            }
                        }
                    }
                }
            }
            drop(fingerprint_span);
            state.rows = rows.into_iter().flatten().collect();
            // The streamed message: parse its growth; one that stopped
            // streaming finishes once its last text is shown.
            let generation = state.generation;
            let mut finished = Vec::new();
            let mut unparsed = Vec::new();
            for (index, row) in state.rows.iter_mut().enumerate() {
                let is_streaming = streaming == Some(index);
                let Some(markdown) = row.markdown.as_mut() else { continue };
                // The message being streamed is set by appends, even if a
                // message with the same text was set whole before (an empty
                // one, when the stream starts).
                if is_streaming && markdown.live.is_none() {
                    markdown.segments = None;
                    markdown.live = Some(Live::new());
                }
                let Some(live) = markdown.live.as_mut() else {
                    if markdown.segments.is_none() && !markdown.requested {
                        markdown.requested = true;
                        unparsed.push(index);
                    }
                    continue;
                };
                live.streaming = is_streaming;
                live.check_tail(&row.message.body);
                let tail = &row.message.body[live.tail_byte..];
                let applied = live.applied.as_ref().map(|(text, _)| text.as_str());
                if applied != Some(tail) {
                    if !live.in_flight {
                        self.parser.submit(live.job(row.id, generation, &row.message.body));
                    }
                } else if !is_streaming {
                    finished.push(index);
                }
            }
            // A conversation opens at its end: the worker parses the last
            // messages first, so the rows in sight arrive first.
            for &index in unparsed.iter().rev() {
                let row = &state.rows[index];
                self.parser.submit(Job::Segment { row: row.id, generation, text: row.message.body.clone() });
            }
            self.rebuild_pieces(state);
            for index in finished {
                self.finish_live(state, index);
            }
        }
        self.restack();
        // The rows in sight are made in this pass, so it draws them, and in
        // this batch, so the reader's place holds through both.
        self.work();
        self.end();
    }

    /// New typography, width or image settings.
    fn apply_settings(&self, settings: Settings) {
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let column = self.column_for(settings.typography);
        let typography_changed = state.settings.as_ref().is_none_or(|old| old.typography != settings.typography);
        let images_changed = state
            .settings
            .as_ref()
            .is_some_and(|old| old.image_base != settings.image_base || old.show_inline_images != settings.show_inline_images);
        if typography_changed || state.styles.is_none() {
            state.styles = Some(Styles::shared(settings.typography, &self.document.effectiveAppearance()));
        }
        state.settings = Some(settings);
        if typography_changed || images_changed || column != state.column {
            // What is laid out anew is the content's own change: nothing
            // held stays held, and nothing it changes is held.
            self.restyling.set(true);
            for piece in &mut state.pieces {
                piece.reserve = 0.0;
            }
        }
        if typography_changed || images_changed {
            // Every view is set anew, in the new type, as the reader reaches it.
            for piece in &mut state.pieces {
                if let Some(views) = piece.view.take() {
                    views.container.removeFromSuperview();
                }
                piece.exact = false;
            }
            if typography_changed {
                for row in &mut state.rows {
                    let Some(markdown) = row.markdown.as_mut() else { continue };
                    match &mut markdown.live {
                        // Streamed afresh into a new view.
                        Some(live) if live.streaming => {
                            live.reset = true;
                            live.applied = None;
                        }
                        // A finished stream is set like any other message.
                        Some(_) => {
                            markdown.live = None;
                            markdown.requested = false;
                        }
                        None => {}
                    }
                }
                self.rebuild_pieces(state);
            }
        }
        if column != state.column {
            state.column = column;
            self.relayout_width(state);
        }
    }

    /// The reading column for the pane's width: the measure, centred.
    fn column_for(&self, typography: Typography) -> Column {
        let viewport = self.scroll.contentSize().width;
        let available = (viewport - 18.0).min(typography.measure()).max(240.0);
        Column { x: ((viewport - available) / 2.0).floor().max(0.0), width: available }
    }

    /// The pane was resized.
    pub(crate) fn viewport_changed(&self) {
        // From inside a restack (the scroller came or went): the next pass
        // takes the new width.
        let Ok(state) = self.state.try_borrow_mut() else {
            self.viewport_pending.set(true);
            self.schedule_work();
            return;
        };
        let Some(typography) = state.settings.as_ref().map(|settings| settings.typography) else { return };
        drop(state);
        let column = self.column_for(typography);
        let nested = self.depth.get() > 0;
        self.begin();
        {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            if column != state.column {
                state.column = column;
                self.restyling.set(true);
                self.relayout_width(state);
            }
        }
        self.restack();
        // Rows a new width brings into sight are made in this batch; inside
        // another, that one makes them.
        if !nested {
            self.work();
        }
        self.end();
    }

    /// The column changed width. Views in sight lay out again now; the rest
    /// are laid out when they are next made, and until then keep the height
    /// they had, or the one remembered for this width.
    fn relayout_width(&self, state: &mut State) {
        let _span = perf::span("gui.transcript.relayout_width");
        for piece in &mut state.pieces {
            piece.reserve = 0.0;
        }
        let visible = self.visible_rect();
        let Some(styles) = state.styles.clone() else { return };
        let column = state.column;
        for index in 0..state.pieces.len() {
            let piece = &state.pieces[index];
            let in_sight = piece.y < visible.origin.y + visible.size.height && piece.y + piece.shown() > visible.origin.y;
            let whole_and_long = matches!(piece.kind, PieceKind::Markdown { count: 1, .. })
                && piece.height > 0.0
                && self.piece_text_length(state, index) > crate::transcript_parse::WHOLE_LIMIT;
            if piece.view.is_some() && in_sight && !whole_and_long {
                self.relayout_piece(state, index, &styles);
                continue;
            }
            // Out of sight, or a long streamed message that is better cut
            // into segments than laid out whole again.
            let piece = &mut state.pieces[index];
            if let Some(views) = piece.view.take() {
                views.container.removeFromSuperview();
            }
            piece.exact = false;
            if whole_and_long {
                let PieceKind::Markdown { row, .. } = piece.kind else { continue };
                self.convert_to_segments(state, row);
            }
        }
        let _ = column;
        let keys: Vec<(usize, u64)> = state
            .pieces
            .iter()
            .enumerate()
            .filter(|(_, piece)| piece.view.is_none())
            .map(|(index, piece)| (index, piece.key))
            .collect();
        for (index, _) in keys {
            if let Some(height) = self.remembered_height(state, index) {
                let piece = &mut state.pieces[index];
                piece.height = height;
                piece.exact = true;
            }
        }
    }

    /// A long message streamed into one view becomes segments.
    fn convert_to_segments(&self, state: &mut State, row: usize) {
        let Some(markdown) = state.rows[row].markdown.as_mut() else { return };
        if markdown.live.as_ref().is_some_and(|live| live.streaming) {
            return;
        }
        markdown.live = None;
        markdown.segments = None;
        let fingerprint = state.rows[row].fingerprint;
        if let Some(segments) = cached_segmentation(state, fingerprint) {
            state.rows[row].markdown.as_mut().expect("a Markdown row").segments = Some(segments);
        } else {
            let id = state.rows[row].id;
            let text = state.rows[row].message.body.clone();
            self.parser.submit(Job::Segment { row: id, generation: state.generation, text });
        }
        self.rebuild_pieces(state);
    }

    // MARK: Worker answers

    /// Applies what the parse worker finished.
    fn drain(&self) {
        let _span = perf::span("gui.transcript.apply");
        self.begin();
        let mut segmented = false;
        // Rows whose one guess becomes a guess per segment: the change is
        // held like a measurement's.
        let mut resegmented: Vec<(u64, f64, f64)> = Vec::new();
        while let Some(done) = self.parser.try_next() {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            match done {
                Done::Segmented { row, generation, segments } => {
                    let segments: Rc<[Rc<Segment>]> = segments.into_iter().map(Rc::new).collect();
                    let Some(index) = state.rows.iter().position(|candidate| candidate.id == row) else {
                        continue;
                    };
                    let fingerprint = state.rows[index].fingerprint;
                    remember_segmentation(state, fingerprint, segments.clone());
                    if generation != state.generation {
                        continue;
                    }
                    let Some(markdown) = state.rows[index].markdown.as_mut() else { continue };
                    match &markdown.live {
                        // A finished stream of sealed segments is set as
                        // any message is: its segments share definitions.
                        Some(live) if !live.streaming && !live.in_flight => markdown.live = None,
                        Some(_) => continue,
                        None => {}
                    }
                    markdown.segments = Some(segments);
                    segmented = true;
                    if let Some((height, top)) = row_extent(state, index) {
                        resegmented.push((row, height, top));
                    }
                }
                Done::Streamed { row, generation, start, text, document, dirty, seal, updates, unseal } => {
                    if generation != state.generation {
                        continue;
                    }
                    let Some(index) = state.rows.iter().position(|candidate| candidate.id == row) else { continue };
                    if segmented {
                        self.rebuild_pieces(state);
                        self.hold_resegmented(state, &mut resegmented);
                        segmented = false;
                    }
                    self.apply_stream(state, index, Streamed { start, text, document, dirty, seal, updates, unseal });
                }
            }
        }
        if segmented {
            let mut state = self.state.borrow_mut();
            self.rebuild_pieces(&mut state);
            self.hold_resegmented(&mut state, &mut resegmented);
        }
        self.restack();
        // What arrived is shown in this pass, in this batch.
        self.work();
        self.end();
    }

    /// One parse of the streamed message's tail: the storage edit and
    /// `update`, in this turn, then the next parse if the message grew
    /// meanwhile. Sealed segments that carry other definitions now change in
    /// the same turn. A sealed tail keeps its head and a new view takes the
    /// rest; sealed segments joined to the tail again become its view.
    fn apply_stream(&self, state: &mut State, index: usize, streamed: Streamed) {
        let Streamed { start, text, document, dirty, seal, updates, unseal } = streamed;
        let _span = perf::span_with("gui.transcript.stream_update", || {
            serde_json::json!({ "bytes": text.len(), "seal": seal.is_some(), "updates": updates.len(), "unseal": unseal.is_some() })
        });
        let generation = state.generation;
        let row_id = state.rows[index].id;
        let Some(live) = state.rows[index].markdown.as_mut().and_then(|markdown| markdown.live.as_mut()) else { return };
        live.in_flight = false;
        let expected = match unseal {
            Some(first) => live.sealed.get(first).map(|(segment, ..)| segment.start),
            None => Some(live.tail_start),
        };
        if expected != Some(start) || updates.iter().any(|update| update.index >= live.sealed.len()) {
            // Answered for a tail that has moved on: ask again.
            self.submit_tail(state, index, generation, row_id);
            return;
        }
        // Sealed segments parsed again in a new context: their text is the
        // same, their parse not.
        let mut edits: Vec<(usize, String, Arc<ParsedDocument>, DirtySet)> = Vec::new();
        for update in updates {
            let (segment, source) = &mut live.sealed[update.index];
            let fresh = Rc::new(Segment {
                text: segment.text.clone(),
                document: update.document.clone(),
                heading_before: segment.heading_before,
                start: segment.start,
                length: segment.length,
            });
            source.document = update.document.clone();
            edits.push((update.index, segment.text.clone(), update.document, update.dirty));
            *segment = fresh;
        }
        if !edits.is_empty() {
            live.refresh_sources();
        }
        for (segment, text, document, dirty) in edits {
            if let Some(piece) = self.piece_of(state, index, segment) {
                self.edit_hosted(state, piece, &text, &text, document, &dirty);
            }
        }
        if let Some(first) = unseal {
            self.apply_unseal(state, index, first, text, document);
        } else {
            self.apply_tail(state, index, text, document, dirty, seal);
        }
        let live = state.rows[index].markdown.as_ref().and_then(|markdown| markdown.live.as_ref()).expect("live");
        let body = &state.rows[index].message.body;
        let current = live.applied.as_ref().map(|(text, _)| text.as_str());
        if body.get(live.tail_byte..) != current {
            self.submit_tail(state, index, generation, row_id);
        } else if !live.streaming {
            self.finish_live(state, index);
        }
    }

    /// The tail's new parse, and with `seal` the cut: the view that showed
    /// the tail shows the head, and the rest gets a view of its own when the
    /// rows are next made.
    fn apply_tail(&self, state: &mut State, index: usize, text: String, document: Arc<ParsedDocument>, dirty: DirtySet, seal: Option<crate::transcript_parse::Seal>) {
        let live = state.rows[index].markdown.as_mut().and_then(|markdown| markdown.live.as_mut()).expect("live");
        let tail_index = live.sealed.len();
        let (shown, shown_document, shown_dirty) = match &seal {
            // The head is set again whole: cutting a streamed view back to
            // it is an edit the incremental update is not exact for.
            Some(seal) => (text[..seal.bytes].to_owned(), seal.head.clone(), DirtySet::wholesale()),
            None => (text.clone(), document.clone(), dirty),
        };
        let previous = live.applied.replace((shown.clone(), shown_document.clone()));
        let piece = self.piece_of(state, index, tail_index);
        let hosted = piece.is_some_and(|piece| state.pieces[piece].view.is_some());
        match (piece, previous) {
            (Some(piece), Some((old, _))) if hosted => {
                self.edit_hosted(state, piece, &old, &shown, shown_document, &shown_dirty);
                if seal.is_some()
                    && let Some(RowViews { content: Content::Markdown { view, .. }, .. }) = &state.pieces[piece].view
                {
                    let view = view.clone();
                    view.set_streaming(false);
                    view.set_hosted_continued(true);
                    self.note_hosted_height(state, piece, false);
                }
            }
            (Some(piece), None) if hosted => {
                // A view that has not seen the previous parse: set anew.
                if let Some(views) = state.pieces[piece].view.take() {
                    views.container.removeFromSuperview();
                }
            }
            _ => {}
        }
        let Some(seal) = seal else { return };
        let live = state.rows[index].markdown.as_mut().and_then(|markdown| markdown.live.as_mut()).expect("live");
        let (head, _) = live.applied.take().expect("the head was just applied");
        let source = SealedSource::sealed(&head, &seal, live.heading_before);
        live.sealed.push((
            Rc::new(Segment {
                text: head,
                document: seal.head,
                heading_before: live.heading_before,
                start: live.tail_start,
                length: seal.units,
            }),
            source,
        ));
        live.refresh_sources();
        live.heading_before |= seal.head_heading;
        live.tail_start += seal.units;
        live.tail_byte += seal.bytes;
        live.applied = Some((text[seal.bytes..].to_owned(), seal.rest));
        self.rebuild_pieces(state);
        if let Some(piece) = self.piece_of(state, index, tail_index) {
            self.refresh_extra(state, piece);
        }
    }

    /// Sealed segments from `first` on are the tail again, with the tail
    /// after them: the view of segment `first` shows the joined `text`, and
    /// the views after it go.
    fn apply_unseal(&self, state: &mut State, index: usize, first: usize, text: String, document: Arc<ParsedDocument>) {
        perf::event_with("gui.transcript.unseal", || serde_json::json!({ "segment": first }));
        let live = state.rows[index].markdown.as_mut().and_then(|markdown| markdown.live.as_mut()).expect("live");
        let (segment, ..) = live.sealed[first].clone();
        let back: usize = live.sealed[..first].iter().map(|(_, source)| source.own.len()).sum();
        live.heading_before = segment.heading_before;
        live.tail_start = segment.start;
        live.tail_byte = back;
        live.sealed.truncate(first);
        live.refresh_sources();
        live.applied = Some((text.clone(), document.clone()));
        let streaming = live.streaming;
        if let Some(piece) = self.piece_of(state, index, first) {
            self.edit_hosted(state, piece, &segment.text, &text, document, &DirtySet::wholesale());
            if let Some(RowViews { content: Content::Markdown { view, .. }, .. }) = &state.pieces[piece].view {
                view.set_streaming(streaming);
                view.set_hosted_continued(false);
            }
        }
        self.rebuild_pieces(state);
        if let Some(piece) = self.piece_of(state, index, first) {
            self.refresh_extra(state, piece);
        }
    }

    /// The piece that shows segment `segment` of row `row`.
    fn piece_of(&self, state: &State, row: usize, segment: usize) -> Option<usize> {
        state.pieces.iter().position(
            |piece| matches!(piece.kind, PieceKind::Markdown { row: other, segment: which, .. } if other == row && which == segment),
        )
    }

    /// A hosted piece's text goes from `old` to `new`: the storage edit
    /// after the common prefix, usually an append, and `update`, in the same
    /// turn; then the gap above a continuing segment and the row's height.
    fn edit_hosted(&self, state: &mut State, piece: usize, old: &str, new: &str, document: Arc<ParsedDocument>, dirty: &DirtySet) {
        let Some(RowViews { content: Content::Markdown { view, storage, .. }, .. }) = &state.pieces[piece].view else { return };
        let (view, storage) = (view.clone(), storage.clone());
        let old_units: Vec<u16> = old.encode_utf16().collect();
        let new_units: Vec<u16> = new.encode_utf16().collect();
        let prefix = old_units.iter().zip(&new_units).take_while(|(a, b)| a == b).count();
        let replacement = String::from_utf16_lossy(&new_units[prefix..]);
        if old != new {
            let _span = perf::span("gui.transcript.storage_edit");
            storage.beginEditing();
            storage.replaceCharactersInRange_withString(NSRange::new(prefix, old_units.len() - prefix), &NSString::from_str(&replacement));
            storage.endEditing();
        }
        view.update(document, dirty, true);
        self.refresh_gap(state, piece);
        self.note_hosted_height(state, piece, false);
    }

    /// A segment after the first has the space above its first paragraph
    /// that TextKit drops at the top of a view; the paragraph may change.
    fn refresh_gap(&self, state: &mut State, index: usize) {
        let PieceKind::Markdown { row, segment, .. } = state.pieces[index].kind else { return };
        let continues = self.segment_start(state, row, segment) > 0;
        let Some(RowViews { content: Content::Markdown { storage, gap, .. }, .. }) = &mut state.pieces[index].view else { return };
        let fresh = if continues { first_spacing_before(storage) } else { 0.0 };
        if fresh == *gap {
            return;
        }
        *gap = fresh;
        self.refresh_extra(state, index);
    }

    /// Asks the worker for the parse of row `index`'s tail as it is now.
    fn submit_tail(&self, state: &mut State, index: usize, generation: u64, row_id: u64) {
        let row = &mut state.rows[index];
        let body = &row.message.body;
        let Some(live) = row.markdown.as_mut().and_then(|markdown| markdown.live.as_mut()) else { return };
        live.check_tail(body);
        self.parser.submit(live.job(row_id, generation, body));
    }

    /// A Markdown piece became the first or last of its message, or stopped
    /// being so: its speaker line, spacer and box follow.
    fn refresh_extra(&self, state: &mut State, index: usize) {
        let PieceKind::Markdown { row, segment, count } = state.pieces[index].kind else { return };
        let Some(RowViews { content: Content::Markdown { gap, .. }, .. }) = &state.pieces[index].view else { return };
        let (top, extra) = self.extra_height(state, row, segment, count, *gap);
        let y = state.pieces[index].y;
        let scale = self.backing_scale();
        if let Some(RowViews { content: Content::Markdown { top: old_top, extra: old_extra, view, offset, .. }, .. }) = &mut state.pieces[index].view {
            if *old_top != top {
                let (frame_y, wanted) = pixel_place(y + top, scale);
                *offset = wanted;
                place_view(view, frame_y - y, wanted);
            }
            *old_top = top;
            *old_extra = extra;
        }
        self.note_hosted_height(state, index, false);
    }

    /// The stream into row `index` ended and its view shows all of it. Its
    /// segments, each checked against the message whole as it was sealed,
    /// are the message's from now on, views and all, and are remembered for
    /// the next time it is opened.
    fn finish_live(&self, state: &mut State, index: usize) {
        let Some(live) = state.rows[index].markdown.as_mut().and_then(|markdown| markdown.live.as_mut()) else { return };
        if live.streaming || live.in_flight || live.applied.is_none() {
            return;
        }
        let tail = live.sealed.len();
        if let Some(piece) = self.piece_of(state, index, tail)
            && let Some(RowViews { content: Content::Markdown { view, .. }, .. }) = &state.pieces[piece].view
            && view.is_streaming()
        {
            let view = view.clone();
            view.set_streaming(false);
            self.note_hosted_height(state, piece, false);
        }
        let row = &mut state.rows[index];
        let fingerprint = row.fingerprint;
        let length = row.message.body.encode_utf16().count();
        let markdown = row.markdown.as_mut().expect("a Markdown row");
        let live = markdown.live.take().expect("live");
        let (text, document) = live.applied.expect("applied");
        let mut segments: Vec<Rc<Segment>> = live.sealed.into_iter().map(|(segment, ..)| segment).collect();
        segments.push(Rc::new(Segment { text, document, heading_before: live.heading_before, start: live.tail_start, length: length - live.tail_start }));
        let segments: Rc<[Rc<Segment>]> = segments.into();
        markdown.segments = Some(segments.clone());
        let count = segments.len();
        // The pieces keep their views under the keys of a finished message.
        let row = &state.rows[index];
        for piece in &mut state.pieces {
            if let PieceKind::Markdown { row: other, segment, .. } = piece.kind
                && other == index
            {
                piece.key = piece_key(row, segment, Some(count));
            }
        }
        remember_segmentation(state, fingerprint, segments);
        self.rebuild_pieces(state);
    }

    // MARK: Pieces

    /// Lays the rows out as pieces again, keeping every view and height
    /// whose content is unchanged.
    fn rebuild_pieces(&self, state: &mut State) {
        let _span = perf::span("gui.transcript.pieces");
        let mut old: HashMap<u64, Piece> = HashMap::with_capacity(state.pieces.len());
        for piece in state.pieces.drain(..) {
            if let Some(duplicate) = old.insert(piece.key, piece)
                && let Some(views) = duplicate.view
            {
                views.container.removeFromSuperview();
            }
        }
        let mut pieces = Vec::with_capacity(state.rows.len());
        let mut index = 0;
        while index < state.rows.len() {
            let row = &state.rows[index];
            match &row.markdown {
                Some(markdown) => {
                    let count = match &markdown.live {
                        Some(live) => live.sealed.len() + 1,
                        None => markdown.segments.as_ref().map_or(1, |segments| segments.len()),
                    };
                    for segment in 0..count {
                        let key = piece_key(row, segment, markdown.live.is_none().then_some(count));
                        pieces.push(old.remove(&key).map(|mut piece| {
                            piece.kind = PieceKind::Markdown { row: index, segment, count };
                            piece
                        }).unwrap_or(Piece {
                            kind: PieceKind::Markdown { row: index, segment, count },
                            key,
                            y: 0.0,
                            height: 0.0,
                            exact: false,
                            view: None,
                            reserve: 0.0,
                            placed: false,
                        }));
                    }
                    index += 1;
                }
                None => {
                    let start = index;
                    // A `/usage` card is a run of its own: it is drawn by its
                    // own view, not set as activity text.
                    let card = |row: &Row| row.message.kind == TranscriptMessageKind::Usage;
                    if card(row) {
                        index += 1;
                    } else {
                        while index < state.rows.len()
                            && state.rows[index].markdown.is_none()
                            && index - start < ACTIVITY_GROUP
                            && !card(&state.rows[index])
                        {
                            index += 1;
                        }
                    }
                    // By the rows' ids as well as what they say: two runs of
                    // the same tool calls are two pieces.
                    let mut hasher = DefaultHasher::new();
                    "activity".hash(&mut hasher);
                    for row in &state.rows[start..index] {
                        row.id.hash(&mut hasher);
                        row.fingerprint.hash(&mut hasher);
                    }
                    let key = hasher.finish();
                    pieces.push(old.remove(&key).map(|mut piece| {
                        piece.kind = PieceKind::Activity { rows: start..index };
                        piece
                    }).unwrap_or(Piece {
                        kind: PieceKind::Activity { rows: start..index },
                        key,
                        y: 0.0,
                        height: 0.0,
                        exact: false,
                        view: None,
                        reserve: 0.0,
                        placed: false,
                    }));
                }
            }
        }
        for piece in old.into_values() {
            if let Some(views) = piece.view {
                views.container.removeFromSuperview();
            }
        }
        state.pieces = pieces;
        for index in 0..state.pieces.len() {
            if state.pieces[index].view.is_some() || state.pieces[index].exact {
                continue;
            }
            if let Some(height) = self.remembered_height(state, index) {
                state.pieces[index].height = height;
                state.pieces[index].exact = true;
            } else if let Some(height) = saved_height(state, index) {
                // As it was when the session was last shown: made, it
                // measures the same unless its text or the look changed.
                state.pieces[index].height = height;
            } else {
                state.pieces[index].height = self.estimate(state, index);
            }
        }
    }

    /// The text a Markdown piece shows, and its segment.
    fn piece_text<'a>(&self, state: &'a State, index: usize) -> Option<(&'a str, Option<&'a Rc<Segment>>)> {
        let PieceKind::Markdown { row, segment, .. } = state.pieces[index].kind else { return None };
        let markdown = state.rows[row].markdown.as_ref()?;
        if let Some(live) = &markdown.live {
            if let Some((sealed, ..)) = live.sealed.get(segment) {
                return Some((sealed.text.as_str(), Some(sealed)));
            }
            let body = &state.rows[row].message.body;
            return Some((live.applied.as_ref().map_or(body.get(live.tail_byte..).unwrap_or(""), |(text, _)| text), None));
        }
        match &markdown.segments {
            Some(segments) => segments.get(segment).map(|piece| (piece.text.as_str(), Some(piece))),
            None => Some((state.rows[row].message.body.as_str(), None)),
        }
    }

    fn piece_text_length(&self, state: &State, index: usize) -> usize {
        self.piece_text(state, index).map_or(0, |(text, _)| text.len())
    }

    /// The height remembered for a piece's content at this width and type.
    fn remembered_height(&self, state: &State, index: usize) -> Option<f64> {
        let styles = state.styles.as_ref()?;
        if let PieceKind::Activity { rows } = &state.pieces[index].kind {
            return state.heights.get(&activity_height_key(state, rows.clone(), styles)).copied();
        }
        let PieceKind::Markdown { row, segment, count } = state.pieces[index].kind else { return None };
        let markdown = state.rows[row].markdown.as_ref()?;
        if markdown.live.is_some() || markdown.segments.is_none() {
            return None;
        }
        let (text, _) = self.piece_text(state, index)?;
        let key = HeightKey { text: text_key(text, state.rows[row].message.kind), width: state.column.width.to_bits(), metrics: styles.metrics_key };
        let height = state.heights.get(&key)?;
        Some(height + self.extra_height(state, row, segment, count, 0.0).1)
    }

    /// A guess at a piece's height from its text, until it has a view.
    fn estimate(&self, state: &State, index: usize) -> f64 {
        let Some(styles) = state.styles.as_ref() else { return 40.0 };
        let typography = styles.typography;
        let width = state.column.width.max(200.0);
        match state.pieces[index].kind.clone() {
            PieceKind::Activity { rows } if self.card_row(state, &rows).is_some() => {
                let body = self.card_row(state, &rows).expect("a card");
                crate::usage_card::layout(&body, state.column.width).1 + CARD_MARGIN * 2.0
            }
            PieceKind::Activity { rows } => {
                // Measured over long mixed sessions: about 2.2 lines a row
                // and as much again for the run's own space.
                let line = line_height(&typography.ui_font(typography.size(crate::chat_typography::TextRole::Activity), 0.0));
                (rows.len() + 1) as f64 * line * 2.2
            }
            PieceKind::Markdown { row, segment, count } => {
                let text = self.piece_text(state, index).map_or("", |(text, _)| text);
                let body = typography.size(crate::chat_typography::TextRole::Assistant);
                estimate_markdown(text, width, body) + self.extra_height(state, row, segment, count, 0.0).1
            }
        }
    }

    /// Space a Markdown piece adds around its text: (above, above + below).
    fn extra_height(&self, state: &State, row: usize, segment: usize, count: usize, gap: f64) -> (f64, f64) {
        let Some(styles) = state.styles.as_ref() else { return (0.0, 0.0) };
        let typography = styles.typography;
        let first = segment == 0;
        let last = segment + 1 == count;
        let speaker = if first { line_height(&typography.speaker_font()) + SPEAKER_GAP } else { 0.0 };
        let spacer = if last { line_height(&typography.spacer_font()) } else { 0.0 };
        let user = state.rows[row].message.kind == TranscriptMessageKind::User;
        let top = if user { if first { USER_TOP } else { 0.0 } } else { 0.0 } + speaker + gap;
        let bottom = if user && last { USER_BOTTOM + USER_MARGIN } else { 0.0 } + spacer;
        (top, top + bottom)
    }

    // MARK: Layout

    /// Places every piece under the one above it. A reader at the end stays
    /// there; otherwise the content the reader saw at the top of the view
    /// when the batch began keeps its place on screen, to the line: heights
    /// corrected, views made or dropped, rows added above or below, a
    /// diagram landing, a new width. The scroll is corrected here, in the
    /// pass that changed the heights, before anything is drawn.
    fn restack(&self) {
        let _span = perf::span("gui.transcript.restack");
        let clip = self.scroll.contentView();
        let visible = clip.bounds();
        let mut state = self.state.borrow_mut();
        let old_height = self.document.frame().size.height;
        let following = self
            .following
            .get()
            .unwrap_or_else(|| near_bottom(old_height, visible.origin.y + visible.size.height));
        let anchor = if following { None } else { self.anchor.get().or_else(|| self.reference(&state, visible)) };
        let column = state.column;
        let scale = self.backing_scale();
        state.above = saved_above(&state, scale);
        let mut y = TOP_PADDING + state.above;
        for piece in &mut state.pieces {
            y = piece_top(&piece.kind, y, scale);
            piece.y = y;
            piece.placed = true;
            if let Some(views) = &piece.view {
                let frame = NSRect::new(NSPoint::new(column.x, y), NSSize::new(column.width, piece.shown()));
                if views.container.frame() != frame {
                    views.container.setFrame(frame);
                }
            }
            y += piece.shown();
        }
        // The document and every place the transcript scrolls to on whole
        // device pixels: TextKit sets what it draws on the window's pixels
        // when it lays a view out, and a scroll by part of a pixel would
        // leave what it drew before off them.
        let content = ((y + BOTTOM_PADDING) * scale).ceil() / scale;
        let placed = self.place_views(&mut state);
        drop(state);
        let span = perf::span("gui.transcript.restack.place");
        for (view, y, offset) in placed {
            place_view(&view, y, offset);
        }
        drop(span);
        // Sizing the document may show or hide the scroller, which resizes
        // the clip view and comes back here: nothing is borrowed now.
        let viewport = self.scroll.contentSize();
        let size = NSSize::new(viewport.width, content.max(viewport.height));
        if self.document.frame().size != size {
            let _span = perf::span("gui.transcript.restack.document");
            self.document.setFrameSize(size);
        }
        // Where the anchored content is now that every view is placed. Gone
        // (its row was replaced): the view stays where it is.
        let anchored = anchor.and_then(|(mark, delta)| {
            let state = self.state.borrow();
            self.mark_y(&state, mark).map(|y| y + delta)
        });
        // A reader rubber-banding past either end keeps the overscroll: the
        // view moves with the content and is not pulled back in.
        let before = visible.origin.y;
        let old_maximum = (old_height - visible.size.height).max(0.0);
        let visible = clip.bounds();
        let maximum = (size.height - visible.size.height).max(0.0);
        let beyond = (before - old_maximum).max(0.0);
        let target = if following {
            Some(maximum + beyond)
        } else {
            anchored.map(|y| y.clamp(before.min(0.0), maximum + beyond))
        };
        let target = target.map(|target| (target * scale).round() / scale);
        if let Some(target) = target
            && (target - visible.origin.y).abs() > 0.25
        {
            let _span = perf::span("gui.transcript.restack.scroll");
            clip.scrollToPoint(NSPoint::new(0.0, target));
            self.scroll.reflectScrolledClipView(&clip);
        }
        // The transcript's own correction is not the reader scrolling.
        self.last_origin.set(clip.bounds().origin.y);
        self.schedule_work();
    }

    /// Places the pieces one under another, and returns where the view will
    /// be when `restack` has corrected it, without touching a view: for a
    /// pass that makes several views and restacks once.
    fn projected_rect(&self, state: &mut State, visible: NSRect) -> NSRect {
        let scale = self.backing_scale();
        let mut y = TOP_PADDING + state.above;
        for piece in &mut state.pieces {
            y = piece_top(&piece.kind, y, scale);
            piece.y = y;
            y += piece.shown();
        }
        let maximum = (y + BOTTOM_PADDING - visible.size.height).max(0.0);
        let origin = match (self.following.get(), self.anchor.get()) {
            (Some(true), _) => maximum,
            (_, Some((mark, delta))) => self.mark_y(state, mark).map_or(visible.origin.y, |y| (y + delta).clamp(0.0, maximum)),
            _ => visible.origin.y,
        };
        NSRect::new(NSPoint::new(visible.origin.x, origin), visible.size)
    }

    /// The content the reader sees at the top of the view, and how far below
    /// it the view begins. A row at the top edge not made yet shows nothing
    /// to hold, so the first row in sight that shows something holds the
    /// place, and the row above it is sized around it.
    fn reference(&self, state: &State, visible: NSRect) -> Option<(Mark, f64)> {
        let top = visible.origin.y;
        let bottom = top + visible.size.height;
        let index = piece_at(state, top)?;
        let shown = state.pieces[index..]
            .iter()
            .take_while(|piece| piece.y < bottom)
            .position(|piece| piece.view.is_some())
            .map(|offset| index + offset);
        let (mark, y) = match shown {
            Some(found) if found == index => self.mark_at(state, index, top),
            Some(found) => self.mark_at(state, found, state.pieces[found].y),
            None => self.mark_at(state, index, top),
        };
        Some((mark, top - y))
    }

    /// Every hosted view's frame on whole device pixels of the document, and
    /// its text below that by the rest of the distance (its content offset).
    /// TextKit draws each layout fragment in a view of its own, set on the
    /// window's pixels when it lays the viewport out, and moves what it drew
    /// without drawing it again; the transcript scrolls by whole pixels. So
    /// a view's pixels depend only on where its text is, not on where its
    /// row was when it was laid out, and every line of a message set as
    /// several views falls on the pixels one view, so placed, puts it on.
    /// Returns the views to move: frame origin in the row, content offset.
    fn place_views(&self, state: &mut State) -> Vec<(Retained<MarkdownTextView>, f64, f64)> {
        let scale = self.backing_scale();
        let mut moves = Vec::new();
        for piece in &mut state.pieces {
            let y = piece.y;
            let Some(RowViews { content: Content::Markdown { view, top, offset, .. }, .. }) = &mut piece.view else { continue };
            let (frame_y, wanted) = pixel_place(y + *top, scale);
            let origin = view.frame().origin.y;
            if (wanted - *offset).abs() > 1.0e-9 || (origin - (frame_y - y)).abs() > 1.0e-9 {
                *offset = wanted;
                moves.push((view.clone(), frame_y - y, wanted));
            }
        }
        moves
    }

    fn backing_scale(&self) -> f64 {
        self.document.window().map_or(2.0, |window| window.backingScaleFactor()).max(1.0)
    }

    fn visible_rect(&self) -> NSRect {
        self.scroll.contentView().bounds()
    }

    /// The reader scrolled (or a restack did).
    fn scrolled(&self) {
        if self.busy.get() {
            return;
        }
        let origin = self.visible_rect().origin.y;
        let moved = origin - self.last_origin.replace(origin);
        if moved.abs() > 0.5 {
            self.direction.set(moved.signum());
        }
        self.work();
    }

    /// Where views are made ahead of the reader: a screen behind, and
    /// `PREPARE_AHEAD` screens in the direction the reader last scrolled
    /// (both ways before they have).
    fn prepared_rect(&self, visible: NSRect, margin: f64) -> (f64, f64) {
        let height = visible.size.height;
        let (above, below) = match self.direction.get() {
            direction if direction < 0.0 => (PREPARE_AHEAD, PREPARE_BEHIND),
            direction if direction > 0.0 => (PREPARE_BEHIND, PREPARE_AHEAD),
            _ => (PREPARE_BEHIND + 0.5, PREPARE_BEHIND + 0.5),
        };
        (visible.origin.y - (above + margin) * height, visible.origin.y + (1.0 + below + margin) * height)
    }

    fn schedule_work(&self) {
        self.schedule_work_in(0.0);
    }

    /// A pass of view making `delay` seconds from now, unless one is due.
    fn schedule_work_in(&self, delay: f64) {
        if self.work_scheduled.replace(true) {
            return;
        }
        let weak = self.this.clone();
        let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
            if let Some(transcript) = weak.upgrade() {
                transcript.work_scheduled.set(false);
                transcript.work();
            }
        });
        // SAFETY: the block holds the transcript weakly and runs on the main
        // run loop; common modes keep it firing during scrolls and resizes.
        let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(delay, false, &block) };
        // SAFETY: the main run loop and the mode constant live for the process.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    }

    /// Makes the views in sight, then those ahead of the reader nearest
    /// first, within this pass's budget, and lets go of those far from
    /// sight. A row in sight the worker has not parsed yet is parsed here
    /// when it is short; a long one waits for the worker, whose answer
    /// schedules the next pass.
    fn work(&self) {
        if self.viewport_pending.replace(false) {
            self.viewport_changed();
        }
        let _span = perf::span("gui.transcript.realize");
        let started = Instant::now();
        self.begin();
        let mut more = false;
        let mut held = false;
        let mut made_views = Vec::new();
        let mut waiting: Vec<usize> = Vec::new();
        // Where the view will be once the rows are placed: made views are
        // placed, and the scroll corrected, once at the end of the pass.
        let mut visible = self.visible_rect();
        loop {
            let (low, high) = self.prepared_rect(visible, 0.0);
            let next = {
                let state = self.state.borrow();
                let pinned = state.pinned;
                let needs = |index: usize, piece: &Piece| {
                    !waiting.contains(&index) && piece.view.as_ref().is_none_or(|views| views.width != state.column.width)
                };
                let top = visible.origin.y;
                let bottom = top + visible.size.height;
                let first = piece_at(&state, top).unwrap_or(state.pieces.len());
                let in_sight = state.pieces[first.min(state.pieces.len())..]
                    .iter()
                    .enumerate()
                    .take_while(|(_, piece)| piece.y < bottom)
                    .map(|(offset, piece)| (first + offset, piece))
                    .find(|(index, piece)| needs(*index, piece))
                    .map(|(index, _)| index);
                let ahead = || {
                    // Nearest the view first, the reader's direction winning
                    // ties by a screen.
                    let bias = visible.size.height * self.direction.get();
                    state
                        .pieces
                        .iter()
                        .enumerate()
                        .filter(|(index, piece)| {
                            (matches!(piece.kind, PieceKind::Markdown { row, .. } if Some(row) == pinned)
                                || (piece.y < high && piece.y + piece.shown() > low))
                                && needs(*index, piece)
                        })
                        .min_by(|(_, a), (_, b)| {
                            let distance = |piece: &Piece| {
                                let gap = if piece.y + piece.shown() <= top {
                                    top - (piece.y + piece.shown()) + bias.max(0.0)
                                } else {
                                    piece.y - bottom - bias.min(0.0)
                                };
                                gap.max(0.0)
                            };
                            distance(a).total_cmp(&distance(b))
                        })
                        .map(|(index, _)| index)
                };
                match in_sight {
                    Some(index) => Some((index, VISIBLE_BUDGET)),
                    // A diagram or formula in sight still rendering goes
                    // first: views made ahead would queue their own renders
                    // before it. Its landing restacks, which comes back here.
                    None if objects_in_sight(&state, top, bottom) => {
                        held = true;
                        None
                    }
                    None => ahead().map(|index| (index, NEARBY_BUDGET)),
                }
            };
            let Some((index, budget)) = next else { break };
            // A view ahead of the reader is started only if it should fit
            // what is left of the pass; the first of a pass always is.
            let cost = if budget == NEARBY_BUDGET && !made_views.is_empty() {
                self.making_cost(&self.state.borrow(), index)
            } else {
                Duration::ZERO
            };
            if started.elapsed() + cost >= budget {
                more = true;
                break;
            }
            let made = {
                let mut state = self.state.borrow_mut();
                let in_sight = budget == VISIBLE_BUDGET;
                if in_sight {
                    self.parse_now(&mut state, index);
                }
                self.realize(&mut state, index)
            };
            if !made {
                // Waiting on the worker; its answer schedules the next pass.
                waiting.push(index);
                continue;
            }
            made_views.push(index);
            visible = {
                let mut state = self.state.borrow_mut();
                self.projected_rect(&mut state, visible)
            };
        }
        if !made_views.is_empty() {
            self.restack_quietly();
        }
        self.release_far();
        self.end();
        // A view made in sight lays out and draws its visible part now, so
        // this pass's frame shows it whole rather than the next one's.
        let visible = self.visible_rect();
        let fresh: Vec<Retained<MarkdownTextView>> = {
            let state = self.state.borrow();
            made_views
                .into_iter()
                .filter_map(|index| state.pieces.get(index))
                .filter(|piece| piece.y < visible.origin.y + visible.size.height && piece.y + piece.shown() > visible.origin.y)
                .filter_map(|piece| match &piece.view {
                    Some(RowViews { content: Content::Markdown { view, .. }, .. }) => Some(view.clone()),
                    _ => None,
                })
                .collect()
        };
        for view in fresh {
            view.prepare_for_display();
        }
        perf::event_with("gui.transcript.pending_objects", || {
            serde_json::json!({ "pending": upleft_render::fragments::async_objects::pending_count() })
        });
        if more {
            self.schedule_work();
        } else if held {
            // A landing that keeps its reserved size reports no new height:
            // look again in a frame.
            self.schedule_work_in(1.0 / 60.0);
        }
    }

    /// About how long making piece `index`'s view takes, from its text.
    fn making_cost(&self, state: &State, index: usize) -> Duration {
        match &state.pieces[index].kind {
            PieceKind::Activity { rows } => MAKE_ACTIVITY_BASE + MAKE_ACTIVITY_ROW * rows.len() as u32,
            PieceKind::Markdown { .. } => {
                let bytes = self.piece_text_length(state, index) as u32;
                MAKE_MARKDOWN_BASE + MAKE_MARKDOWN_KB * bytes / 1024
            }
        }
    }

    /// A short message in sight that the worker has not parsed yet is
    /// parsed here: a few kilobytes cost a fraction of the view it becomes,
    /// and the reader sees it in this pass rather than a blank row.
    fn parse_now(&self, state: &mut State, index: usize) {
        let PieceKind::Markdown { row, .. } = state.pieces[index].kind else { return };
        let Some(markdown) = state.rows[row].markdown.as_ref() else { return };
        if markdown.live.is_some() || markdown.segments.is_some() || state.rows[row].message.body.len() > PARSE_NOW_LIMIT {
            return;
        }
        let _span = perf::span("gui.transcript.parse_now");
        let segments: Rc<[Rc<Segment>]> = crate::transcript_parse::segment(&state.rows[row].message.body).into_iter().map(Rc::new).collect();
        let fingerprint = state.rows[row].fingerprint;
        remember_segmentation(state, fingerprint, segments.clone());
        state.rows[row].markdown.as_mut().expect("a Markdown row").segments = Some(segments);
        self.rebuild_pieces(state);
    }

    /// `restack` inside a pass of view making, which is already a batch.
    fn restack_quietly(&self) {
        let scheduled = self.work_scheduled.replace(true);
        self.restack();
        self.work_scheduled.set(scheduled);
    }

    /// Lets go of views well outside where they are made ahead: a screen
    /// beyond it either way, so views near its edge are not made and let go
    /// again as the reader moves.
    fn release_far(&self) {
        let (low, high) = self.prepared_rect(self.visible_rect(), 1.0);
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let pinned = state.pinned;
        for piece in &mut state.pieces {
            let far = piece.y + piece.shown() < low || piece.y > high;
            // The streamed message keeps its view: every append would
            // otherwise set all of it again once it came back into sight.
            let kept = matches!(piece.kind, PieceKind::Markdown { row, segment, .. }
                if Some(row) == pinned
                    || state.rows[row].markdown.as_ref().and_then(|markdown| markdown.live.as_ref())
                        .is_some_and(|live| live.streaming && segment == live.sealed.len()));
            if far && !kept
                && let Some(views) = piece.view.take()
            {
                views.container.removeFromSuperview();
            }
        }
    }

    /// Makes piece `index`'s views, or lays them out at the new width.
    /// Returns false when its text is not parsed yet.
    fn realize(&self, state: &mut State, index: usize) -> bool {
        let Some(styles) = state.styles.clone() else { return false };
        if state.pieces[index].view.is_some() {
            self.relayout_piece(state, index, &styles);
            return true;
        }
        // A piece about to be made shows its own height: what it held goes
        // to another far piece on its side.
        self.pass_reserve_on(state, index);
        let old = state.pieces[index].shown();
        let made = match state.pieces[index].kind.clone() {
            PieceKind::Activity { rows } => {
                self.make_activity(state, index, rows, &styles);
                true
            }
            PieceKind::Markdown { row, segment, count } => self.make_markdown(state, index, row, segment, count, &styles),
        };
        if made {
            self.hold_correction(state, index, old);
        }
        made
    }

    fn make_markdown(&self, state: &mut State, index: usize, row: usize, segment: usize, count: usize, styles: &Styles) -> bool {
        let (text, document, heading_before, start) = {
            let markdown = state.rows[row].markdown.as_ref().expect("a Markdown row");
            if let Some(live) = &markdown.live {
                if let Some((sealed, ..)) = live.sealed.get(segment) {
                    (sealed.text.clone(), sealed.document.clone(), sealed.heading_before, sealed.start)
                } else {
                    let Some((text, document)) = &live.applied else {
                        return false;
                    };
                    (text.clone(), document.clone(), live.heading_before, live.tail_start)
                }
            } else {
                let Some(segments) = &markdown.segments else {
                    return false;
                };
                let Some(piece) = segments.get(segment) else { return false };
                (piece.text.clone(), piece.document.clone(), piece.heading_before, piece.start)
            }
        };
        let _span = perf::span_with("gui.transcript.make_view", || serde_json::json!({ "bytes": text.len(), "piece": index, "estimate": state.pieces[index].height, "exact": state.pieces[index].exact }));
        let message_kind = state.rows[row].message.kind;
        let user = message_kind == TranscriptMessageKind::User;
        let column = state.column;
        let indent = user_indent(styles.typography.style, column.width);
        let text_width = if user { column.width - indent - USER_RULE - USER_LEFT - USER_RIGHT } else { column.width };
        let sheet = if user { styles.user.clone() } else { styles.assistant.clone() };
        let storage: Retained<NSTextStorage> =
            // SAFETY: NSTextStorage's initialiser takes a string.
            unsafe { msg_send![NSTextStorage::alloc(), initWithString: &*NSString::from_str(&text)] };
        let view = MarkdownTextView::new_hosted(&storage, sheet, text_width.max(80.0), self.mtm);
        // Copy and drags write each formula as its TeX: work done when the
        // reader copies, never when a message renders.
        view.set_math_copy_as_tex(true);
        let delegate: Rc<dyn MarkdownTextViewDelegate> = self.delegate.get().expect("the row delegate").clone();
        view.set_markdown_delegate(Some(Rc::downgrade(&delegate)));
        view.setDrawsBackground(false);
        view.unregisterDraggedTypes();
        if heading_before {
            view.set_hosted_continuation(true);
        }
        // Another segment follows: the text's last line break opens no line.
        if segment + 1 < count {
            view.set_hosted_continued(true);
        }
        if let Some(settings) = &state.settings {
            configure_images(&view, settings);
        }
        let settle = segment == 0 && state.rows[row].arrived.take().is_some_and(|arrived| arrived.elapsed() < SETTLE_WINDOW);
        let live = state.rows[row].markdown.as_ref().and_then(|markdown| markdown.live.as_ref());
        if let Some(live) = live
            && segment == live.sealed.len()
        {
            view.set_streaming(live.streaming);
        }
        let container = TranscriptRowView::new(NSRect::new(NSPoint::new(column.x, state.pieces[index].y), NSSize::new(column.width, 10.0)), self.mtm);
        container.addSubview(&view);
        self.document.addSubview(&container);
        if settle {
            crate::motion::settle_in(&container);
        }
        view.update(document, &DirtySet::wholesale(), false);
        let gap = if start > 0 { first_spacing_before(&storage) } else { 0.0 };
        let (top, extra) = self.extra_height(state, row, segment, count, gap);
        // On whole device pixels, the text the rest below (`place_views`),
        // before the view is first drawn.
        let (frame_y, offset) = pixel_place(state.pieces[index].y + top, self.backing_scale());
        view.set_hosted_content_offset(offset);
        let text_height = view.frame().size.height - offset;
        let height = text_height + extra;
        let first = segment == 0;
        let last = segment + 1 == count;
        let typography = styles.typography;
        let x = if user { indent + USER_RULE + USER_LEFT } else { 0.0 };
        view.setFrameOrigin(NSPoint::new(x, frame_y - state.pieces[index].y));
        if user {
            let box_top = 0.0;
            let box_bottom = height - if last { USER_MARGIN + line_height(&typography.spacer_font()) } else { 0.0 };
            let frame = NSRect::new(NSPoint::new(indent, box_top), NSSize::new(column.width - indent, box_bottom - box_top));
            for fill in user_decor(styles.typography.style, segment, count, frame, self.mtm) {
                container.addSubview_positioned_relativeTo(&fill, objc2_app_kit::NSWindowOrderingMode::Below, Some(&view));
            }
        }
        if first {
            let author = speaker_name(&state.rows[row].message);
            let color = if user { theme::accent() } else { theme::secondary_text() };
            let label = speaker_text(&author, &typography.speaker_font(), &color);
            let label_height = line_height(&typography.speaker_font());
            let label_top = if user { USER_TOP } else { 0.0 };
            let label_view = TranscriptLabelView::new(label, NSRect::new(NSPoint::new(x, label_top), NSSize::new(text_width, label_height)), self.mtm);
            container.addSubview(&label_view);
        }
        container.setFrameSize(NSSize::new(column.width, height));
        let key = text_key(&text, message_kind);
        if live.is_none() {
            remember_height(state, HeightKey { text: key, width: column.width.to_bits(), metrics: styles.metrics_key }, text_height);
        }
        let piece = &mut state.pieces[index];
        piece.height = height;
        piece.exact = true;
        piece.view = Some(RowViews {
            container,
            content: Content::Markdown { view: view.clone(), storage, top, extra, gap, offset, text_key: key },
            width: column.width,
        });
        // A selection the reader left here.
        if let Some(selection) = state.pending_selection
            && selection.message == row
        {
            let start = self.segment_start(state, row, segment);
            let end = start + self.segment_length(state, row, segment).unwrap_or_else(|| text.encode_utf16().count());
            if selection.location >= start && selection.location < end.max(start + 1) {
                let local = selection.location - start;
                let length = selection.length.min(end - selection.location);
                view.set_source_selected_ranges(&[upleft_core::NSRange::new(local as isize, length as isize)]);
                state.pending_selection = None;
            }
        }
        true
    }

    /// The card a run of one `/usage` row shows.
    fn card_row(&self, state: &State, rows: &Range<usize>) -> Option<crate::usage_card::Body> {
        (rows.len() == 1 && state.rows[rows.start].message.kind == TranscriptMessageKind::Usage)
            .then(|| crate::usage_card::Body::decode(&state.rows[rows.start].message.body))
    }

    /// A `/usage` card: its height is its layout's, exact before the view.
    fn make_card(&self, state: &mut State, index: usize, rows: Range<usize>, body: crate::usage_card::Body, styles: &Styles) {
        let _span = perf::span("gui.transcript.make_card");
        let column = state.column;
        // The card fills its row, its margins inside it, as an activity
        // run's text does.
        let (card, height) = crate::usage_card::view(&body, column.width, CARD_MARGIN, styles.typography.style, self.mtm);
        card.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(column.width, height)));
        let row = TranscriptRowView::new(NSRect::new(NSPoint::new(column.x, state.pieces[index].y), NSSize::new(column.width, height)), self.mtm);
        row.addSubview(&card);
        self.document.addSubview(&row);
        let fresh = state.rows[rows.start].arrived.take().is_some_and(|arrived| arrived.elapsed() < SETTLE_WINDOW);
        state.rows[rows.start].landed = None;
        if fresh {
            crate::motion::settle_in(&row);
        }
        let key = activity_height_key(state, rows, styles);
        remember_height(state, key, height);
        let piece = &mut state.pieces[index];
        piece.height = height;
        piece.exact = true;
        piece.view = Some(RowViews { container: row, content: Content::Activity, width: column.width });
    }

    fn make_activity(&self, state: &mut State, index: usize, rows: Range<usize>, styles: &Styles) {
        if let Some(body) = self.card_row(state, &rows) {
            return self.make_card(state, index, rows, body, styles);
        }
        let _span = perf::span("gui.transcript.make_activity");
        let column = state.column;
        let key = activity_height_key(state, rows.clone(), styles);
        // The text is kept for the run: a run let go of and made again as
        // the reader scrolls back sets it from here. Its detail links name
        // its rows.
        let text_key = (key, state.rows[rows.clone()].iter().map(|row| row.id).fold(0u64, |hash, id| hash.rotate_left(7) ^ id));
        let text = match state.activity_texts.get(&text_key) {
            Some(text) => text.clone(),
            None => {
                let _span = perf::span("gui.transcript.activity_text");
                let text = Retained::into_super(self.activity_text(state, rows.clone(), styles));
                if state.activity_texts.len() >= ACTIVITY_TEXTS {
                    state.activity_texts.clear();
                }
                state.activity_texts.insert(text_key, text.clone());
                text
            }
        };
        let delegate = self.text_delegate.borrow().clone();
        let view = TranscriptActivityView::new(&text, column.width, delegate, self.link_attributes(), self.mtm);
        let height = match state.heights.get(&key) {
            Some(height) => *height,
            None => view.measure(),
        };
        view.setFrameSize(NSSize::new(column.width, height));
        let row = TranscriptRowView::new(NSRect::new(NSPoint::new(column.x, state.pieces[index].y), NSSize::new(column.width, height)), self.mtm);
        row.addSubview(&view);
        self.document.addSubview(&row);
        // A new run settles in; a run that grew does not move again.
        let fresh = state.rows[rows.start].arrived.is_some_and(|arrived| arrived.elapsed() < SETTLE_WINDOW);
        let mut landed = Vec::new();
        for item in &mut state.rows[rows.clone()] {
            item.arrived = None;
            if item.landed.take().is_some_and(|ended| ended.elapsed() < SETTLE_WINDOW) {
                landed.push(item.id);
            }
        }
        if fresh {
            crate::motion::settle_in(&row);
        }
        view.animate_chips(&landed);
        remember_height(state, key, height);
        let piece = &mut state.pieces[index];
        piece.height = height;
        piece.exact = true;
        piece.view = Some(RowViews { container: row, content: Content::Activity, width: column.width });
    }

    /// Activity rows carry links only so a click opens their detail: no
    /// underline, a pointing hand, and each run keeps its colour.
    fn link_attributes(&self) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
        Some(
            self.link_attributes
                .get_or_init(|| {
                    let no_underline = NSNumber::numberWithInteger(0);
                    let cursor = NSCursor::pointingHandCursor();
                    // SAFETY: the attribute names are AppKit statics.
                    let keys: [&NSString; 2] = unsafe { [NSUnderlineStyleAttributeName, NSCursorAttributeName] };
                    let values: [&AnyObject; 2] = [&no_underline, &cursor];
                    NSDictionary::from_slices(&keys, &values)
                })
                .clone(),
        )
    }

    fn activity_text(&self, state: &State, rows: Range<usize>, styles: &Styles) -> Retained<NSMutableAttributedString> {
        let text = NSMutableAttributedString::new();
        let settings = state.settings.clone();
        let image_base = settings.as_ref().map(|settings| settings.image_base.clone()).unwrap_or_default();
        let show = settings.as_ref().is_some_and(|settings| settings.show_inline_images);
        for row in &state.rows[rows] {
            // The detail link names the row, which history loaded above
            // does not renumber (`open_link` finds its message).
            let mut message = row.message.clone();
            message.detail_id = message.detail_id.map(|_| row.id as usize);
            let rendered = native_markdown::render_message(&message, state.column.width, &image_base, styles.typography, show);
            text.appendAttributedString(&rendered);
        }
        text
    }

    /// Lays a piece's views out at the column's width, or sets its text
    /// again after its rows changed.
    fn relayout_piece(&self, state: &mut State, index: usize, styles: &Styles) {
        let _span = perf::span("gui.transcript.relayout");
        let column = state.column;
        let kind = state.pieces[index].kind.clone();
        let Some(views) = state.pieces[index].view.as_ref() else { return };
        if views.width == column.width {
            return;
        }
        match (&views.content, kind) {
            (Content::Markdown { view, .. }, PieceKind::Markdown { row, .. }) => {
                // The text lays out again at the new width, and the row's
                // label and box follow; the height arrives with it.
                let (view, container) = (view.clone(), views.container.clone());
                let user = state.rows[row].message.kind == TranscriptMessageKind::User;
                let indent = state.styles.as_ref().map_or(0.0, |styles| user_indent(styles.typography.style, column.width));
                let text_width = if user { column.width - indent - USER_RULE - USER_LEFT - USER_RIGHT } else { column.width };
                view.set_hosted_width(text_width.max(80.0));
                if user {
                    view.setFrameOrigin(NSPoint::new(indent + USER_RULE + USER_LEFT, view.frame().origin.y));
                }
                for subview in container.subviews().iter() {
                    let frame = subview.frame();
                    if subview.isKindOfClass(TranscriptLabelView::class()) {
                        let label_x = if user { indent + USER_RULE + USER_LEFT } else { frame.origin.x };
                        subview.setFrame(NSRect::new(NSPoint::new(label_x, frame.origin.y), NSSize::new(text_width, frame.size.height)));
                    } else if subview.isKindOfClass(TranscriptFillView::class()) {
                        // Bars keep their width; boxes follow the column.
                        let width = if frame.size.width > USER_BAR + 1.0 { column.width - indent } else { frame.size.width };
                        subview.setFrame(NSRect::new(NSPoint::new(indent, frame.origin.y), NSSize::new(width, frame.size.height)));
                    }
                }
                if let Some(views) = state.pieces[index].view.as_mut() {
                    views.width = column.width;
                }
                container.setFrameSize(NSSize::new(column.width, container.frame().size.height));
                self.note_hosted_height(state, index, false);
            }
            (Content::Activity, PieceKind::Activity { rows }) => {
                if let Some(views) = state.pieces[index].view.take() {
                    views.container.removeFromSuperview();
                }
                self.make_activity(state, index, rows, styles);
            }
            _ => {}
        }
    }

    /// A hosted view's height may have changed: size its row. With `hold`,
    /// a change not the reader's streamed text (a diagram, formula or image
    /// landing) is held for the scroll bar.
    fn note_hosted_height(&self, state: &mut State, index: usize, hold: bool) {
        let Some(RowViews { container, content: Content::Markdown { view, top, extra, offset, text_key, .. }, width }) =
            &state.pieces[index].view
        else {
            return;
        };
        let text_height = view.frame().size.height - offset;
        let height = text_height + extra;
        let (container, top, width, text_key) = (container.clone(), *top, *width, *text_key);
        let _ = top;
        if (state.pieces[index].height - height).abs() > 0.01 {
            perf::event_with("gui.transcript.height_changed", || {
                serde_json::json!({ "piece": index, "from": state.pieces[index].height, "to": height, "busy": self.busy.get() })
            });
            let old = state.pieces[index].shown();
            state.pieces[index].height = height;
            container.setFrameSize(NSSize::new(width, height));
            self.resize_user_box(state, index, height);
            if hold {
                self.hold_correction(state, index, old);
            }
        }
        let live = match state.pieces[index].kind {
            PieceKind::Markdown { row, .. } => state.rows[row].markdown.as_ref().is_some_and(|markdown| markdown.live.is_some()),
            _ => false,
        };
        if !live && let Some(styles) = &state.styles {
            let key = HeightKey { text: text_key, width: width.to_bits(), metrics: styles.metrics_key };
            remember_height(state, key, text_height);
        }
    }

    /// The user's box follows its row's height.
    fn resize_user_box(&self, state: &State, index: usize, height: f64) {
        let PieceKind::Markdown { row, segment, count } = state.pieces[index].kind else { return };
        if state.rows[row].message.kind != TranscriptMessageKind::User {
            return;
        }
        let Some(views) = &state.pieces[index].view else { return };
        let last = segment + 1 == count;
        let bottom = height
            - if last {
                USER_MARGIN + state.styles.as_ref().map_or(0.0, |styles| line_height(&styles.typography.spacer_font()))
            } else {
                0.0
            };
        let subviews = views.container.subviews();
        for subview in subviews.iter() {
            if subview.isKindOfClass(TranscriptFillView::class()) {
                let frame = subview.frame();
                subview.setFrameSize(NSSize::new(frame.size.width, bottom - frame.origin.y));
            }
        }
    }

    fn segment_start(&self, state: &State, row: usize, segment: usize) -> usize {
        let Some(markdown) = state.rows[row].markdown.as_ref() else { return 0 };
        if let Some(live) = &markdown.live {
            return live.sealed.get(segment).map_or(live.tail_start, |(sealed, ..)| sealed.start);
        }
        markdown.segments.as_ref().and_then(|segments| segments.get(segment)).map_or(0, |piece| piece.start)
    }

    /// A segment's own length, without the definitions it carries.
    fn segment_length(&self, state: &State, row: usize, segment: usize) -> Option<usize> {
        let markdown = state.rows[row].markdown.as_ref()?;
        if let Some(live) = &markdown.live {
            return live.sealed.get(segment).map(|(sealed, ..)| sealed.length);
        }
        markdown.segments.as_ref().and_then(|segments| segments.get(segment)).map(|piece| piece.length)
    }

    // MARK: Held corrections

    /// Where the reader's place is kept, in the document: a piece wholly
    /// above it moves the scroll origin when its height changes. A reader
    /// following the end has everything above.
    fn anchor_line(&self, state: &State, visible: NSRect) -> f64 {
        let following = self
            .following
            .get()
            .unwrap_or_else(|| near_bottom(self.document.frame().size.height, visible.origin.y + visible.size.height));
        if following {
            return f64::INFINITY;
        }
        self.anchor.get().and_then(|(mark, _)| self.mark_y(state, mark)).unwrap_or(visible.origin.y)
    }

    /// Pieces with no view well beyond where views are made, on one side of
    /// the reader, farthest first: where a correction is held.
    fn far_pieces(&self, state: &State, above: bool, visible: NSRect, except: usize) -> Vec<usize> {
        let (low, high) = self.prepared_rect(visible, 1.0);
        let pinned = state.pinned;
        let holds = |index: usize, piece: &Piece| {
            index != except
                && piece.view.is_none()
                && piece.placed
                && !matches!(piece.kind, PieceKind::Markdown { row, .. }
                    if Some(row) == pinned || state.rows[row].markdown.as_ref().is_some_and(|markdown| markdown.live.is_some()))
        };
        let pieces = state.pieces.iter().enumerate();
        if above {
            pieces
                .take_while(|(_, piece)| !piece.placed || piece.y + piece.shown() < low)
                .filter(|(index, piece)| holds(*index, piece))
                .map(|(index, _)| index)
                .take(FAR_PIECES)
                .collect()
        } else {
            pieces
                .rev()
                .take_while(|(_, piece)| !piece.placed || piece.y > high)
                .filter(|(index, piece)| holds(*index, piece))
                .map(|(index, _)| index)
                .take(FAR_PIECES)
                .collect()
        }
    }

    /// Piece `index` went from `old` to its height now because a
    /// measurement replaced a guess (its view made, a diagram, formula or
    /// image landed): the change is held, with the opposite sign, in far
    /// pieces on its side of the reader, so neither the scroll origin (above)
    /// nor the document's height (below) jumps, and given back a frame at a
    /// time. Streamed text, new rows, and new widths or styles change what
    /// the transcript holds and show at once.
    fn hold_correction(&self, state: &mut State, index: usize, old: f64) {
        let piece = &state.pieces[index];
        let live = matches!(piece.kind, PieceKind::Markdown { row, .. }
            if state.rows[row].markdown.as_ref().is_some_and(|markdown| markdown.live.is_some()));
        if self.restyling.get() || !piece.placed || live {
            return;
        }
        let (top, now) = (piece.y, piece.shown());
        self.hold_change(state, top, old, now, index);
    }

    /// Content at `top` went from `old` to `now` points tall without the
    /// transcript's content changing: holds the difference in far pieces on
    /// its side of the reader, other than piece `except`.
    fn hold_change(&self, state: &mut State, top: f64, old: f64, now: f64, except: usize) {
        let scale = self.backing_scale();
        let amount = scroll_reserve::whole_pixels(old - now, scale);
        if amount == 0.0 {
            return;
        }
        let visible = self.visible_rect();
        let above = top + old <= self.anchor_line(state, visible);
        let far = self.far_pieces(state, above, visible, except);
        let left = scroll_reserve::absorb(&mut state.pieces, &far, amount, scale);
        perf::event_with("gui.transcript.hold", || {
            serde_json::json!({ "top": top, "old": old, "now": now, "above": above, "amount": amount, "left": left, "far": far.len(), "except": except })
        });
        if left != amount {
            self.start_bleed();
        }
    }

    /// Rows the worker cut into segments since the pieces were last rebuilt:
    /// the segments' guesses replace the row's, which is held.
    fn hold_resegmented(&self, state: &mut State, resegmented: &mut Vec<(u64, f64, f64)>) {
        if self.restyling.get() {
            resegmented.clear();
            return;
        }
        for (id, old, top) in resegmented.drain(..) {
            let Some(index) = state.rows.iter().position(|row| row.id == id) else { continue };
            let now: f64 = state
                .pieces
                .iter()
                .filter(|piece| matches!(piece.kind, PieceKind::Markdown { row, .. } if row == index))
                .map(Piece::shown)
                .sum();
            self.hold_change(state, top, old, now, usize::MAX);
        }
    }

    /// Piece `index` is about to be made: what it holds moves to another far
    /// piece on its side, or, with none, is settled at once.
    fn pass_reserve_on(&self, state: &mut State, index: usize) {
        let piece = &state.pieces[index];
        if piece.reserve == 0.0 {
            return;
        }
        let visible = self.visible_rect();
        let above = piece.y + piece.shown() <= self.anchor_line(state, visible);
        let far = self.far_pieces(state, above, visible, index);
        scroll_reserve::transfer(&mut state.pieces, index, &far, self.backing_scale());
    }

    /// Runs the give-back, on the display's frames when the window is on a
    /// screen.
    fn start_bleed(&self) {
        if self.ticker.borrow().is_some() {
            return;
        }
        self.bleed_last.set(None);
        let on_screen = self.document.window().is_some_and(|window| window.isVisible() && window.screen().is_some());
        let ticker = if on_screen {
            // SAFETY: the document view answers `transcriptBleed:` with the
            // link as its argument; the link is invalidated when idle.
            let link = unsafe { self.document.displayLinkWithTarget_selector(&self.document, sel!(transcriptBleed:)) };
            // SAFETY: the main run loop and the mode constant live for the
            // process; common modes keep it running during a scroll.
            unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
            Ticker::Display(link)
        } else {
            let weak = self.this.clone();
            let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
                if let Some(transcript) = weak.upgrade() {
                    transcript.bleed();
                }
            });
            // SAFETY: the block holds the transcript weakly and runs on the
            // main run loop.
            let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(1.0 / 60.0, true, &block) };
            // SAFETY: as above.
            unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
            Ticker::Timer(timer)
        };
        *self.ticker.borrow_mut() = Some(ticker);
    }

    fn stop_bleed(&self) {
        let ticker = self.ticker.borrow_mut().take();
        match ticker {
            Some(Ticker::Display(link)) => link.invalidate(),
            Some(Ticker::Timer(timer)) => timer.invalidate(),
            None => {}
        }
    }

    /// The reader holds the scroll bar's thumb: it stays under the pointer.
    fn dragging_thumb(&self) -> bool {
        NSEvent::pressedMouseButtons() & 1 != 0
            && self.scroll.verticalScroller().is_some_and(|scroller| {
                let part = scroller.hitPart();
                part == NSScrollerPart::Knob || part == NSScrollerPart::KnobSlot
            })
    }

    /// A frame's step of every held correction: a piece's reserve above the
    /// reader shrinks with the scroll origin (the anchor keeps the text
    /// still), one below with the document. One restack.
    fn bleed(&self) {
        if self.busy.get() || self.state.try_borrow_mut().is_err() {
            return;
        }
        if self.dragging_thumb() {
            self.bleed_last.set(None);
            return;
        }
        let now = Instant::now();
        let elapsed = self.bleed_last.replace(Some(now)).map_or(1.0 / 60.0, |last| now.duration_since(last).as_secs_f64());
        let _span = perf::span("gui.transcript.bleed");
        let scale = self.backing_scale();
        self.begin();
        let held = {
            let mut state = self.state.borrow_mut();
            let mut held = false;
            for piece in &mut state.pieces {
                if piece.reserve != 0.0 {
                    piece.reserve -= scroll_reserve::bleed_step(piece.reserve, elapsed, scale);
                    if piece.reserve.abs() < 1.0e-9 {
                        piece.reserve = 0.0;
                    }
                    held |= piece.reserve != 0.0;
                }
            }
            held
        };
        self.restack_quietly();
        self.end();
        if !held {
            self.stop_bleed();
        }
    }

    // MARK: Batches

    fn begin(&self) {
        // Batches nest (a restack that shows the scroller comes back through
        // `viewport_changed`): the outermost one keeps the place.
        self.depth.set(self.depth.get() + 1);
        if self.busy.replace(true) {
            return;
        }
        let visible = self.visible_rect();
        let height = self.document.frame().size.height;
        let following = near_bottom(height, visible.origin.y + visible.size.height);
        self.following.set(Some(following));
        let anchor = (!following)
            .then(|| self.state.try_borrow().ok())
            .flatten()
            .and_then(|state| self.reference(&state, visible));
        self.anchor.set(anchor);
    }

    fn end(&self) {
        let depth = self.depth.get().saturating_sub(1);
        self.depth.set(depth);
        if depth > 0 {
            return;
        }
        // Heights reported inside the batch, by views not sized since.
        loop {
            let reported = std::mem::take(&mut *self.reported.borrow_mut());
            if reported.is_empty() {
                break;
            }
            {
                let mut state = self.state.borrow_mut();
                let state = &mut *state;
                for pointer in reported {
                    if let Some(index) = find_view(state, pointer) {
                        self.note_hosted_height(state, index, true);
                    }
                }
            }
            self.restack();
        }
        self.busy.set(false);
        self.following.set(None);
        self.anchor.set(None);
        self.restyling.set(false);
        self.place_orb();
    }

    /// Puts the streaming orb under the streamed message's last line, in
    /// the space the row leaves below it, or takes it away when nothing
    /// streams. Moving it is one frame set; its pulse is Core Animation's.
    fn place_orb(&self) {
        const SIDE: f64 = 8.0;
        let spot = self.state.try_borrow().ok().and_then(|state| {
            let streaming = state.rows.iter().position(|row| {
                row.markdown.as_ref().and_then(|markdown| markdown.live.as_ref()).is_some_and(|live| live.streaming)
            })?;
            let piece = state.pieces.iter().rev().find(|piece| matches!(piece.kind, PieceKind::Markdown { row, .. } if row == streaming))?;
            let spacer = state.styles.as_ref().map_or(16.0, |styles| line_height(&styles.typography.spacer_font()));
            let y = piece.y + piece.height - spacer + (spacer - SIDE) / 2.0 - 2.0;
            Some(NSPoint::new(state.column.x + 1.0, y.round()))
        });
        let mut orb = self.orb.borrow_mut();
        match (spot, orb.as_ref()) {
            (Some(origin), Some(view)) => view.setFrameOrigin(origin),
            (Some(origin), None) => {
                if let Some(view) = streaming_orb(NSRect::new(origin, NSSize::new(SIDE, SIDE)), self.mtm) {
                    self.document.addSubview(&view);
                    *orb = Some(view);
                }
            }
            (None, Some(view)) => {
                view.removeFromSuperview();
                *orb = None;
            }
            (None, None) => {}
        }
    }

    /// A hosted view reported a new height.
    fn height_reported(&self, view: &MarkdownTextView) {
        let pointer = view as *const MarkdownTextView as usize;
        if self.busy.get() {
            self.reported.borrow_mut().push(pointer);
            return;
        }
        self.begin();
        self.reported.borrow_mut().push(pointer);
        self.end();
    }

    // MARK: Appearance

    fn appearance_changed(&self) {
        let _span = perf::span("gui.transcript.appearance");
        let typography = {
            let state = self.state.borrow();
            let Some(styles) = state.styles.as_ref() else { return };
            let dark = crate::transcript_style::is_dark(&self.document.effectiveAppearance());
            if dark == styles.dark {
                return;
            }
            styles.typography
        };
        self.begin();
        self.restyling.set(true);
        let styles = Styles::shared(typography, &self.document.effectiveAppearance());
        let views: Vec<(Retained<MarkdownTextView>, bool)> = {
            let mut state = self.state.borrow_mut();
            state.styles = Some(styles.clone());
            state.activity_texts.clear();
            let mut views = Vec::new();
            for piece in &mut state.pieces {
                match (&piece.view, &piece.kind) {
                    (Some(RowViews { content: Content::Markdown { view, .. }, .. }), _) => {
                        views.push((view.clone(), false));
                    }
                    (Some(RowViews { content: Content::Activity, .. }), _) => {
                        // Symbols and rows are tinted when set: set them again.
                        if let Some(views) = piece.view.take() {
                            views.container.removeFromSuperview();
                        }
                    }
                    _ => {}
                }
            }
            views
        };
        {
            let state = self.state.borrow();
            for (view, _) in &views {
                let user = state.pieces.iter().any(|piece| {
                    matches!((&piece.view, &piece.kind), (Some(RowViews { content: Content::Markdown { view: other, .. }, .. }), PieceKind::Markdown { row, .. })
                        if std::ptr::eq(&**other, &**view) && state.rows[*row].message.kind == TranscriptMessageKind::User)
                });
                let sheet = if user { styles.user.clone() } else { styles.assistant.clone() };
                // Heights reported here wait for the batch's end.
                view.set_style_sheet(sheet);
            }
        }
        self.end();
        self.schedule_work();
    }

    // MARK: Reader actions

    fn open_link(&self, destination: &str) {
        if let Some(id) = destination
            .strip_prefix(native_markdown::DETAIL_URL_SCHEME)
            .and_then(|rest| rest.strip_prefix(':'))
            .and_then(|id| id.trim().parse::<u64>().ok())
        {
            let index = self.state.borrow().rows.iter().position(|row| row.id == id);
            if let Some(index) = index
                && let Some(handler) = self.events.borrow().as_ref()
            {
                handler(TranscriptEvent::OpenDetail(index));
            }
            return;
        }
        if !native_markdown::safe_link(destination) {
            return;
        }
        if let Some(url) = NSURL::URLWithString(&NSString::from_str(destination)) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }

    /// The message's text, as it was written, to the pasteboard.
    fn copy_message(&self, message: usize) {
        let state = self.state.borrow();
        if let Some(row) = state.rows.get(message) {
            copy_text(&row.message.body);
        }
    }

    /// Selects all of a message that has one view.
    fn select_message(&self, message: usize) {
        let state = self.state.borrow();
        for piece in &state.pieces {
            if let (PieceKind::Markdown { row, .. }, Some(RowViews { content: Content::Markdown { view, .. }, .. })) = (&piece.kind, &piece.view)
                && *row == message
            {
                // SAFETY: a nil sender is allowed.
                unsafe { view.selectAll(None) };
                if let Some(window) = self.document.window() {
                    window.makeFirstResponder(Some(view));
                }
                return;
            }
        }
    }

    /// One selection at a time: a new one clears the others.
    fn selection_changed(&self, changed: &MarkdownTextView) {
        if self.busy.get() {
            return;
        }
        let Some(range) = changed.source_selected_ranges().first().copied() else { return };
        if range.length == 0 {
            return;
        }
        let Ok(state) = self.state.try_borrow() else { return };
        for piece in &state.pieces {
            if let Some(RowViews { content: Content::Markdown { view, .. }, .. }) = &piece.view
                && !std::ptr::eq(&**view, changed)
                && view.source_selected_ranges().first().is_some_and(|range| range.length > 0)
            {
                view.set_source_selected_ranges(&[upleft_core::NSRange::new(0, 0)]);
            }
        }
    }

    fn context_menu(&self, view: &MarkdownTextView, target: &ContextTarget) -> Option<Retained<NSMenu>> {
        let state = self.state.try_borrow().ok()?;
        let pointer = view as *const MarkdownTextView as usize;
        let index = find_view(&state, pointer)?;
        let PieceKind::Markdown { row, count, .. } = state.pieces[index].kind else { return None };
        let menu = NSMenu::new(self.mtm);
        let item = |title: &str, action: Sel, target: &AnyObject| {
            // SAFETY: the action is a method of `target`, which outlives the menu.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(self.mtm), &NSString::from_str(title), Some(action), ns_string!(""))
            };
            // SAFETY: as above.
            unsafe { item.setTarget(Some(target)) };
            item
        };
        let document: &AnyObject = &self.document;
        if let ContextTargetKind::Link(link) = &target.kind {
            let open = item("Open Link", sel!(openTranscriptLink:), document);
            // SAFETY: the represented object is an NSString the menu keeps.
            unsafe { open.setRepresentedObject(Some(&NSString::from_str(link))) };
            menu.addItem(&open);
            let copy = item("Copy Link", sel!(copyTranscriptLink:), document);
            // SAFETY: as above.
            unsafe { copy.setRepresentedObject(Some(&NSString::from_str(link))) };
            menu.addItem(&copy);
            menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        }
        let has_selection = view.source_selected_ranges().first().is_some_and(|range| range.length > 0);
        if has_selection {
            let view_object: &AnyObject = view;
            menu.addItem(&item("Copy", sel!(copy:), view_object));
        }
        if let Some(latex) = target.hit_offset.and_then(|offset| view.math_latex_at_source_offset(offset)) {
            let copy = item("Copy LaTeX", sel!(copyTranscriptLatex:), document);
            // SAFETY: the represented object is an NSString the menu keeps.
            unsafe { copy.setRepresentedObject(Some(&NSString::from_str(&latex))) };
            menu.addItem(&copy);
        }
        let copy_message = item("Copy Message", sel!(copyTranscriptMessage:), document);
        copy_message.setTag(row as isize);
        menu.addItem(&copy_message);
        let source = item("View Markdown", sel!(viewTranscriptMessage:), document);
        source.setTag(row as isize);
        menu.addItem(&source);
        if count == 1 {
            let select = item("Select Message", sel!(selectTranscriptMessage:), document);
            select.setTag(row as isize);
            menu.addItem(&select);
        }
        Some(menu)
    }

    // MARK: Place and selection

    /// Where the reader is, to restore it later.
    pub(crate) fn place(&self) -> Place {
        let visible = self.visible_rect();
        let height = self.document.frame().size.height;
        let at_bottom = near_bottom(height, visible.origin.y + visible.size.height);
        let state = self.state.borrow();
        let top = state.pieces.iter().find(|piece| piece.y + piece.shown() > visible.origin.y);
        let (message, offset) = match top {
            Some(piece) => {
                let row = match &piece.kind {
                    PieceKind::Markdown { row, .. } => *row,
                    PieceKind::Activity { rows } => rows.start,
                };
                let first = state.pieces.iter().find(|candidate| match &candidate.kind {
                    PieceKind::Markdown { row: other, .. } => *other == row,
                    PieceKind::Activity { rows } => rows.start == row,
                });
                let start = first.map_or(piece.y, |first| first.y);
                (row, visible.origin.y - start)
            }
            None => (0, 0.0),
        };
        Place { at_bottom, message, offset }
    }

    /// Puts the reader back at `place` (or, without one, at `y`).
    pub(crate) fn restore_place(&self, place: Option<&Place>, y: f64) {
        // The message's views are made first, so its offset lands on the
        // content it was taken from, not on an estimate of it.
        if let Some(place) = place.filter(|place| !place.at_bottom) {
            self.make_place(place);
        }
        let clip = self.scroll.contentView();
        let visible = clip.bounds();
        let maximum = (self.document.frame().size.height - visible.size.height).max(0.0);
        let target = match place {
            Some(place) if place.at_bottom => maximum,
            Some(place) => {
                let state = self.state.borrow();
                state
                    .pieces
                    .iter()
                    .find(|piece| match &piece.kind {
                        PieceKind::Markdown { row, .. } => *row == place.message,
                        PieceKind::Activity { rows } => rows.contains(&place.message),
                    })
                    .map_or(y, |piece| piece.y + place.offset)
            }
            None => y,
        };
        let scale = self.backing_scale();
        let target = (target.clamp(0.0, maximum) * scale).round() / scale;
        self.last_origin.set(target);
        clip.scrollToPoint(NSPoint::new(0.0, target));
        self.scroll.reflectScrolledClipView(&clip);
    }

    /// Makes the views of the message at `place` from its top to a screen
    /// past its offset, within a pass's budget for rows in sight.
    fn make_place(&self, place: &Place) {
        let started = Instant::now();
        let reach = place.offset + self.visible_rect().size.height;
        self.begin();
        {
            let mut state = self.state.borrow_mut();
            let first = state.pieces.iter().position(|piece| match &piece.kind {
                PieceKind::Markdown { row, .. } => *row == place.message,
                PieceKind::Activity { rows } => rows.contains(&place.message),
            });
            if let Some(first) = first {
                let top = state.pieces[first].y;
                let mut index = first;
                while index < state.pieces.len() && started.elapsed() < PLACE_BUDGET {
                    let piece = &state.pieces[index];
                    if piece.y - top > reach {
                        break;
                    }
                    if piece.view.as_ref().is_none_or(|views| views.width != state.column.width) {
                        self.parse_now(&mut state, index);
                        self.realize(&mut state, index);
                    }
                    let mut y = state.pieces[first].y;
                    let scale = self.backing_scale();
                    for piece in &mut state.pieces[first..] {
                        y = piece_top(&piece.kind, y, scale);
                        piece.y = y;
                        y += piece.shown();
                    }
                    index += 1;
                }
            }
        }
        self.restack();
        self.end();
    }

    /// The reader's selection, in its message's own offsets.
    pub(crate) fn selection(&self) -> Option<Selection> {
        let state = self.state.borrow();
        for piece in &state.pieces {
            if let (PieceKind::Markdown { row, segment, .. }, Some(RowViews { content: Content::Markdown { view, .. }, .. })) = (&piece.kind, &piece.view)
                && let Some(range) = view.source_selected_ranges().first().copied()
                && range.length > 0
            {
                let start = self.segment_start(&state, *row, *segment);
                return Some(Selection { message: *row, location: start + range.location as usize, length: range.length as usize });
            }
        }
        state.pending_selection
    }

    /// Sets the selection now, or when its message next gets a view.
    pub(crate) fn restore_selection(&self, selection: Option<Selection>) {
        let mut state = self.state.borrow_mut();
        state.pending_selection = selection;
        let Some(selection) = selection else { return };
        for index in 0..state.pieces.len() {
            let PieceKind::Markdown { row, segment, .. } = state.pieces[index].kind else { continue };
            if row != selection.message {
                continue;
            }
            let Some(RowViews { content: Content::Markdown { view, storage, .. }, .. }) = &state.pieces[index].view else { continue };
            let start = self.segment_start(&state, row, segment);
            let end = start + self.segment_length(&state, row, segment).unwrap_or_else(|| storage.length());
            if selection.location >= start && selection.location < end.max(start + 1) {
                let length = selection.length.min(end - selection.location);
                view.set_source_selected_ranges(&[upleft_core::NSRange::new((selection.location - start) as isize, length as isize)]);
                state.pending_selection = None;
                return;
            }
        }
    }

    // MARK: Marks

    /// The content of piece `index` at document `y`, and where it is: the
    /// line there when the piece has a view, else the piece's top.
    fn mark_at(&self, state: &State, index: usize, y: f64) -> (Mark, f64) {
        let piece = &state.pieces[index];
        match &piece.kind {
            PieceKind::Activity { rows } => (Mark::Activity { row: state.rows[rows.start].id }, piece.y),
            PieceKind::Markdown { row, segment, .. } => {
                let id = state.rows[*row].id;
                let start = self.segment_start(state, *row, *segment);
                if let Some(RowViews { content: Content::Markdown { view, .. }, .. }) = &piece.view {
                    let origin = piece.y + view.frame().origin.y;
                    if let Some((line, top)) = view.hosted_line_at(y - origin) {
                        let mark = Mark::Line {
                            row: id,
                            offset: start + line.paragraph.max(0) as usize,
                            element: line.element,
                            character: line.character,
                        };
                        return (mark, origin + top);
                    }
                }
                (Mark::Piece { row: id, offset: start }, piece.y)
            }
        }
    }

    /// Where `mark` is in the document now, or `None` when its row is gone.
    fn mark_y(&self, state: &State, mark: Mark) -> Option<f64> {
        let (id, offset) = match mark {
            Mark::Line { row, offset, .. } | Mark::Piece { row, offset } => (row, offset),
            Mark::Activity { row } => (row, 0),
        };
        let row = state.rows.iter().position(|candidate| candidate.id == id)?;
        let first = state.pieces.partition_point(|piece| match &piece.kind {
            PieceKind::Markdown { row: other, .. } => *other < row,
            PieceKind::Activity { rows } => rows.end <= row,
        });
        let mut found = None;
        for index in first..state.pieces.len() {
            match &state.pieces[index].kind {
                PieceKind::Activity { rows } if rows.contains(&row) => return Some(state.pieces[index].y),
                PieceKind::Markdown { row: other, segment, .. } if *other == row => {
                    let start = self.segment_start(state, row, *segment);
                    if start > offset && found.is_some() {
                        break;
                    }
                    found = Some((index, start));
                }
                _ => break,
            }
        }
        let (index, start) = found?;
        let piece = &state.pieces[index];
        if let (Mark::Line { element, character, .. }, Some(RowViews { content: Content::Markdown { view, .. }, .. })) = (mark, &piece.view)
            && let Some(top) = view.hosted_line_top(HostedLine { paragraph: offset.saturating_sub(start) as isize, element, character })
        {
            return Some(piece.y + view.frame().origin.y + top);
        }
        Some(piece.y)
    }

    // MARK: Reading back

    /// For the headless benchmark: the content at the viewport's top edge
    /// (or, with `bottom`, its bottom edge), named, and its y in the
    /// viewport.
    pub(crate) fn probe(&self, bottom: bool) -> Option<(String, f64)> {
        let visible = self.visible_rect();
        let state = self.state.try_borrow().ok()?;
        let y = if bottom { visible.origin.y + visible.size.height - 1.0 } else { visible.origin.y };
        let index = piece_at(&state, y)?;
        let (mark, at) = self.mark_at(&state, index, y);
        // With where the row is from the end and what it says, so the
        // content is found again even if its row was replaced.
        let row = mark.row();
        let position = state.rows.iter().position(|candidate| candidate.id == row)?;
        let name = format!("{}:{}|{}", state.rows.len() - 1 - position, content_hash(&state.rows[position]), mark.name());
        Some((name, at - visible.origin.y))
    }

    /// Where the content `probe` named is now, in the viewport.
    pub(crate) fn locate(&self, name: &str) -> Option<f64> {
        let (row, name) = name.split_once('|')?;
        let (from_end, hash) = row.split_once(':')?;
        let (from_end, hash): (usize, u64) = (from_end.parse().ok()?, hash.parse().ok()?);
        let mut mark = Mark::parse(name)?;
        let state = self.state.try_borrow().ok()?;
        if !state.rows.iter().any(|candidate| candidate.id == mark.row()) {
            let index = state.rows.len().checked_sub(from_end + 1)?;
            let replaced = &state.rows[index];
            if content_hash(replaced) != hash {
                return None;
            }
            mark = mark.with_row(replaced.id);
        }
        Some(self.mark_y(&state, mark)? - self.visible_rect().origin.y)
    }

    /// For the headless benchmark: what the viewport shows unfinished — rows
    /// with no view (or one of another width), and diagrams or formulas
    /// drawn as placeholders.
    /// For the headless benchmark: the rows, and the bytes of their text.
    pub(crate) fn content_extent(&self) -> (usize, usize) {
        let Ok(state) = self.state.try_borrow() else { return (0, 0) };
        (state.rows.len(), state.rows.iter().map(|row| row.message.body.len()).sum())
    }

    pub(crate) fn viewport_gaps(&self) -> (usize, usize) {
        let visible = self.visible_rect();
        let Ok(state) = self.state.try_borrow() else { return (0, 0) };
        let (top, bottom) = (visible.origin.y, visible.origin.y + visible.size.height);
        let mut unmade = 0;
        let mut placeholders = 0;
        for piece in &state.pieces {
            if piece.y >= bottom || piece.y + piece.shown() <= top || piece.shown() <= 0.0 {
                continue;
            }
            match &piece.view {
                Some(views) if views.width == state.column.width => {
                    if let Content::Markdown { view, .. } = &views.content {
                        let origin = piece.y + view.frame().origin.y;
                        placeholders += view
                            .hosted_pending_objects()
                            .iter()
                            .filter(|(from, to)| origin + from < bottom && origin + to > top)
                            .count();
                    }
                }
                _ => unmade += 1,
            }
        }
        (unmade, placeholders)
    }

    /// The heights the session had when last shown, read off the main
    /// thread as its history is: rows not laid out yet take them, and while
    /// older history arrives the rest of the saved total waits above.
    pub(crate) fn use_saved_heights(&self, saved: crate::saved_heights::SavedHeights) {
        self.begin();
        {
            let mut state = self.state.borrow_mut();
            state.saved = Some(SavedState {
                width: saved.width,
                metrics: saved.metrics,
                total: saved.total,
                pieces: saved.pieces.into_iter().collect(),
                rows: saved.rows.into_iter().collect(),
            });
            let state = &mut *state;
            for index in 0..state.pieces.len() {
                let piece = &state.pieces[index];
                if piece.view.is_none() && !piece.exact && let Some(height) = saved_height(state, index) {
                    state.pieces[index].height = height;
                }
            }
        }
        self.restack();
        self.end();
    }

    /// Whether the session's history is still arriving above.
    pub(crate) fn set_history_loading(&self, loading: bool) {
        let changed = self.state.try_borrow_mut().map(|mut state| std::mem::replace(&mut state.history_loading, loading) != loading).unwrap_or(false);
        if changed && !loading {
            self.begin();
            self.restack();
            self.end();
        }
    }

    /// Every piece's height as laid out now, to be saved for the next time
    /// the session is shown; none before the look is known.
    pub(crate) fn saved_heights(&self) -> Option<crate::saved_heights::SavedHeights> {
        let state = self.state.try_borrow().ok()?;
        let styles = state.styles.as_ref()?;
        if state.history_loading || state.pieces.is_empty() {
            return None;
        }
        let mut pieces = Vec::with_capacity(state.pieces.len());
        let mut rows: HashMap<u64, f64> = HashMap::new();
        let mut total = 0.0;
        for (index, piece) in state.pieces.iter().enumerate() {
            pieces.push((saved_key(&state, index), piece.height));
            total += piece.height;
            if let PieceKind::Markdown { row, .. } = piece.kind {
                *rows.entry(state.rows[row].fingerprint).or_default() += piece.height;
            }
        }
        Some(crate::saved_heights::SavedHeights {
            width: state.column.width.to_bits(),
            metrics: styles.metrics_key,
            total,
            pieces,
            rows: rows.into_iter().collect(),
        })
    }

    /// For the transcript check: how the row views disagree with the rows.
    /// Each made piece's container must sit at the piece's place and shown
    /// height, an activity run's text fill it, no two containers overlap,
    /// and no row view be left in the document that no piece owns. Empty
    /// when all is well.
    pub(crate) fn layout_violations(&self) -> Vec<String> {
        const EPSILON: f64 = 0.01;
        let Ok(state) = self.state.try_borrow() else { return Vec::new() };
        let column = state.column;
        let mut found = Vec::new();
        let mut frames: Vec<(usize, NSRect)> = Vec::new();
        let mut containers = std::collections::HashSet::new();
        let differs = |a: f64, b: f64| (a - b).abs() > EPSILON;
        for (index, piece) in state.pieces.iter().enumerate() {
            let Some(views) = &piece.view else { continue };
            containers.insert(Retained::as_ptr(&views.container) as usize);
            let kind = match &piece.kind {
                PieceKind::Markdown { row, segment, .. } => format!("markdown {row}.{segment}"),
                PieceKind::Activity { rows } => format!("activity {}..{}", rows.start, rows.end),
            };
            let frame = views.container.frame();
            if differs(frame.origin.x, column.x)
                || differs(frame.origin.y, piece.y)
                || differs(frame.size.width, column.width)
                || differs(frame.size.height, piece.shown())
            {
                found.push(format!(
                    "piece {index} ({kind}): view at y {:.2} h {:.2} x {:.2} w {:.2}, piece at y {:.2} h {:.2} (reserve {:.2}) x {:.2} w {:.2}",
                    frame.origin.y, frame.size.height, frame.origin.x, frame.size.width,
                    piece.y, piece.shown(), piece.reserve, column.x, column.width,
                ));
            }
            if let Content::Activity = views.content {
                for child in views.container.subviews().iter() {
                    // The text must fit the row it is drawn in: nothing clips it.
                    // Read only once laid out whole, so the check lays nothing out.
                    if let Some(activity) = child.downcast_ref::<TranscriptActivityView>()
                        && let Some(text) = activity.laid_out_height()
                    {
                        if differs(text, piece.height) {
                            found.push(format!("piece {index} ({kind}): activity text {text:.2} tall in a row of h {:.2}", piece.height));
                        }
                    }
                    let inner = child.frame();
                    if differs(inner.origin.y, 0.0) || differs(inner.size.height, piece.height) || differs(inner.size.width, frame.size.width) {
                        found.push(format!(
                            "piece {index} ({kind}): activity text at y {:.2} h {:.2} w {:.2} in a row of h {:.2} w {:.2}",
                            inner.origin.y, inner.size.height, inner.size.width, piece.height, frame.size.width,
                        ));
                    }
                }
            }
            frames.push((index, frame));
        }
        frames.sort_by(|a, b| a.1.origin.y.total_cmp(&b.1.origin.y));
        for pair in frames.windows(2) {
            let (above, below) = (pair[0], pair[1]);
            if above.1.origin.y + above.1.size.height > below.1.origin.y + EPSILON {
                found.push(format!(
                    "pieces {} and {} overlap: {:.2}..{:.2} and {:.2}..{:.2}",
                    above.0,
                    below.0,
                    above.1.origin.y,
                    above.1.origin.y + above.1.size.height,
                    below.1.origin.y,
                    below.1.origin.y + below.1.size.height,
                ));
            }
        }
        for view in self.document.subviews().iter() {
            if view.isHidden() || containers.contains(&(Retained::as_ptr(&view) as usize)) {
                continue;
            }
            if view.class().name().to_str().is_ok_and(|name| name == "OmperorTranscriptRowView") {
                let frame = view.frame();
                found.push(format!("a row view no piece owns at y {:.2} h {:.2}", frame.origin.y, frame.size.height));
            }
        }
        found
    }

    /// For the transcript check: every view in sight asked to draw all of
    /// itself again, from its own state rather than the pixels it kept, and
    /// every activity run's text laid out whole first.
    pub(crate) fn invalidate_visible(&self) {
        fn invalidate(view: &NSView) {
            // An activity run's text laid out whole, as it is when first
            // measured: what it draws does not rest on estimates.
            if let Some(activity) = view.downcast_ref::<TranscriptActivityView>() {
                activity.measure();
            }
            view.setNeedsDisplay(true);
            for child in view.subviews().iter() {
                invalidate(&child);
            }
        }
        let visible = self.visible_rect();
        self.document.setNeedsDisplay(true);
        for view in self.document.subviews().iter() {
            let frame = view.frame();
            if frame.origin.y < visible.origin.y + visible.size.height && frame.origin.y + frame.size.height > visible.origin.y {
                invalidate(&view);
            }
        }
    }

    /// Every message's text, in order: what the transcript holds.
    pub(crate) fn plain_text(&self) -> String {
        let state = self.state.borrow();
        state
            .rows
            .iter()
            .map(|row| match row.message.kind {
                TranscriptMessageKind::Usage => crate::usage_card::plain_text(&crate::usage_card::Body::decode(&row.message.body)),
                _ => row.message.body.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Images that finished loading: the views showing them lay out again.
    pub(crate) fn images_landed(&self, destinations: &[String]) {
        if destinations.is_empty() {
            return;
        }
        self.begin();
        {
            let state = self.state.borrow();
            for piece in &state.pieces {
                if let Some(RowViews { content: Content::Markdown { view, storage, .. }, .. }) = &piece.view {
                    let text = storage.string().to_string();
                    for destination in destinations {
                        if text.contains(destination.as_str()) {
                            view.refresh_remote_image(destination);
                        }
                    }
                }
            }
        }
        self.end();
    }

    /// The last assistant message as displayed, for the Markdown benchmark
    /// and dump: every piece of it made and kept, its rect in the document
    /// view, and what its views show.
    pub(crate) fn displayed_answer(&self) -> Option<Displayed> {
        let row = {
            let state = self.state.borrow();
            state.rows.iter().rposition(|row| row.message.kind == TranscriptMessageKind::Assistant)?
        };
        self.begin();
        {
            let mut state = self.state.borrow_mut();
            state.pinned = Some(row);
            let indices: Vec<usize> = state
                .pieces
                .iter()
                .enumerate()
                .filter(|(_, piece)| matches!(piece.kind, PieceKind::Markdown { row: other, .. } if other == row))
                .map(|(index, _)| index)
                .collect();
            for index in indices {
                if state.pieces[index].view.as_ref().is_none_or(|views| views.width != state.column.width) && !self.realize(&mut state, index) {
                    drop(state);
                    self.end();
                    return None;
                }
            }
        }
        self.restack();
        self.end();
        let state = self.state.borrow();
        let pieces: Vec<&Piece> = state
            .pieces
            .iter()
            .filter(|piece| matches!(piece.kind, PieceKind::Markdown { row: other, .. } if other == row))
            .collect();
        let first = pieces.first()?;
        let height: f64 = pieces.iter().map(|piece| piece.height).sum();
        let rect = NSRect::new(
            NSPoint::new(0.0, first.y.floor()),
            NSSize::new(self.document.frame().size.width, height.ceil()),
        );
        let styles = state.styles.as_ref()?;
        let mut collected = transcript_semantics::Collected::default();
        collected.push_label(&format!("{}\n", speaker_name(&state.rows[row].message)), &styles.typography.speaker_font());
        for piece in &pieces {
            if let Some(RowViews { content: Content::Markdown { view, .. }, .. }) = &piece.view {
                // Drawn whole when captured, not only the part in sight.
                view.set_hosted_full_viewport(true);
                collected.push_hosted(view);
            }
        }
        collected.push_label("\n", &styles.typography.spacer_font());
        Some(collected.finish(rect))
    }
}

/// The Markdown dump's equivalence check under way (`begin_equivalence`).
struct EquivalenceCheck {
    text: String,
    views: Vec<(f64, f64, f64)>,
    /// For a message still set as a stream, whether it streams.
    streaming: Option<bool>,
    sealed: usize,
    first_row: Retained<TranscriptRowView>,
    rows: Vec<Retained<TranscriptRowView>>,
    /// The message's text in the document view: first view's top to last
    /// view's bottom.
    rect: NSRect,
    /// The stack's layout fragments: where each one's text starts in the
    /// message, its top from the text's top, and its height.
    fragments: Vec<(isize, f64, f64)>,
    /// The single view's row, and its text's rect.
    single: Option<(Retained<TranscriptRowView>, NSRect)>,
}

/// What `Transcript::swap_equivalence` found.
pub(crate) struct Equivalence {
    /// The views the message is set as, and how many of them are sealed
    /// heads of a stream.
    pub(crate) segments: usize,
    pub(crate) sealed: usize,
    pub(crate) streaming: bool,
    /// The text shown, in UTF-16 units.
    pub(crate) units: usize,
    /// The first layout fragment whose place differs, by where its text
    /// starts in the message: `(offset, stack y and height, single y and
    /// height)`, in points from the top of the text.
    pub(crate) layout: Option<(isize, (f64, f64), (f64, f64))>,
    /// The stack's text, first view's top to last view's bottom, and the
    /// single view's height.
    pub(crate) stack_height: f64,
    pub(crate) single_height: f64,
    /// Each view of the stack: its text's top from the first's, its height,
    /// and its content offset.
    pub(crate) views: Vec<(f64, f64, f64)>,
}

/// A hosted view's lines, laid out: for each line of each layout fragment,
/// where the fragment's text starts in the view's source (plus the line's
/// index, in thousandths), and the line's top and height in the view —
/// without the empty line after a final line break when another view
/// continues the text (`set_hosted_continued`).
fn fragment_places(view: &MarkdownTextView, continued: bool) -> Vec<(isize, f64, f64)> {
    use objc2_app_kit::NSTextSelectionDataSource;
    let Some(manager) = view.textLayoutManager() else { return Vec::new() };
    let document = manager.documentRange();
    manager.ensureLayoutForRange(&document);
    let origin = view.textContainerOrigin();
    let places = RefCell::new(Vec::new());
    let block = block2::StackBlock::new(|fragment: std::ptr::NonNull<objc2_app_kit::NSTextLayoutFragment>| -> objc2::runtime::Bool {
        // SAFETY: TextKit passes a live fragment for the call.
        let fragment = unsafe { fragment.as_ref() };
        let range = upleft_render::fragments::fragment_base::element_source_range(fragment, || upleft_core::NSRange::new(-1, 0));
        let frame = fragment.layoutFragmentFrame();
        let lines = fragment.textLineFragments();
        for index in 0..lines.count() {
            let line = lines.objectAtIndex(index);
            let bounds = line.typographicBounds();
            let empty = line.characterRange().length == 0 && index > 0;
            // Where the glyphs sit in the line, in thousandths of a point,
            // rides along in the height.
            let glyphs = line.glyphOrigin().y / 100.0;
            places.borrow_mut().push((range.location * 1000 + index as isize, origin.y + frame.origin.y + bounds.origin.y, bounds.size.height + glyphs, empty));
        }
        objc2::runtime::Bool::YES
    });
    manager.enumerateTextLayoutFragmentsFromLocation_options_usingBlock(
        Some(&document.location()),
        objc2_app_kit::NSTextLayoutFragmentEnumerationOptions::EnsuresLayout,
        &block,
    );
    let mut places = places.into_inner();
    if continued && places.last().is_some_and(|place| place.3) {
        places.pop();
    }
    places.into_iter().map(|(at, y, height, _)| (at, y, height)).collect()
}

/// Where the streamed message stands, for the Markdown dump's replay.
pub(crate) struct ReplayState {
    /// Every append is parsed and shown.
    pub(crate) settled: bool,
    pub(crate) sealed: usize,
}

impl Transcript {
    /// For the Markdown dump's equivalence check (docs/MARKDOWN_CONFORMANCE.md),
    /// in steps the harness spaces out so the display cycle draws between
    /// them: `begin_equivalence` makes every view of the last assistant
    /// message and has it drawn whole; `capture_equivalence` draws what is
    /// shown over the message's text; `swap_equivalence` shows one hosted
    /// view of the same text in place of the stack, set as the transcript
    /// sets a message; `end_equivalence` puts the stack back. Headless runs
    /// only: the single view is parsed and laid out on the main thread.
    pub(crate) fn begin_equivalence(&self) -> bool {
        self.end_equivalence();
        if self.displayed_answer().is_none() {
            return false;
        }
        let state = self.state.borrow();
        let Some(row) = state.rows.iter().rposition(|row| row.message.kind == TranscriptMessageKind::Assistant) else { return false };
        let Some(markdown) = state.rows[row].markdown.as_ref() else { return false };
        let body = &state.rows[row].message.body;
        let (text, streaming, sealed) = match &markdown.live {
            Some(live) => {
                let Some((tail, _)) = live.applied.as_ref() else { return false };
                (format!("{}{tail}", &body[..live.tail_byte]), Some(live.streaming), live.sealed.len())
            }
            None => (body.clone(), None, 0),
        };
        let mut views = Vec::new();
        let mut fragments = Vec::new();
        let mut extents = Vec::new();
        for (index, piece) in state.pieces.iter().filter(|piece| matches!(piece.kind, PieceKind::Markdown { row: other, .. } if other == row)).enumerate() {
            let Some(RowViews { container, content: Content::Markdown { view, top, offset, .. }, .. }) = &piece.view else { return false };
            let start = self.segment_start(&state, row, index) as isize;
            let continued = matches!(piece.kind, PieceKind::Markdown { segment, count, .. } if segment + 1 < count);
            let frame = view.frame();
            fragments.extend(fragment_places(view, continued).into_iter().map(|(at, y, height)| (start * 1000 + at, piece.y + frame.origin.y + y, height)));
            views.push((container.clone(), piece.y + top, frame.size.height - offset));
            extents.push((piece.y + top, frame.size.height - offset, *offset));
        }
        let Some((first, top, _)) = views.first().cloned() else { return false };
        let (_, last_top, last_height) = views.last().cloned().expect("a first view");
        let column = state.column;
        let rect = NSRect::new(NSPoint::new(column.x, top), NSSize::new(column.width, last_top + last_height - top));
        let fragments = fragments.into_iter().map(|(at, y, height)| (at, y - top, height)).collect();
        let extents = extents.into_iter().map(|(y, height, offset)| (y - top, height, offset)).collect();
        *self.equivalence.borrow_mut() = Some(EquivalenceCheck {
            text,
            views: extents,
            streaming,
            sealed,
            first_row: first,
            rows: views.into_iter().map(|(view, ..)| view).collect(),
            rect,
            fragments,
            single: None,
        });
        true
    }

    /// Draws what the check shows — the stack, or after `swap_equivalence`
    /// the single view — over the message's text and a margin below it, the
    /// same rect for both.
    pub(crate) fn capture_equivalence(&self, capture: &mut dyn FnMut(&NSView, NSRect)) {
        let check = self.equivalence.borrow();
        let Some(check) = check.as_ref() else { return };
        let rect = check.rect;
        capture(&self.document, NSRect::new(rect.origin, NSSize::new(rect.size.width, rect.size.height + 24.0)));
    }

    /// Shows one view of the message's text where its stack was.
    pub(crate) fn swap_equivalence(&self) -> Option<Equivalence> {
        let mut check = self.equivalence.borrow_mut();
        let check = check.as_mut()?;
        let state = self.state.borrow();
        let styles = state.styles.as_ref()?;
        let settings = state.settings.as_ref()?;
        let width = state.column.width;
        let storage: Retained<NSTextStorage> =
            // SAFETY: NSTextStorage's initialiser takes a string.
            unsafe { msg_send![NSTextStorage::alloc(), initWithString: &*NSString::from_str(&check.text)] };
        let single = MarkdownTextView::new_hosted(&storage, styles.assistant.clone(), width.max(80.0), self.mtm);
        single.setDrawsBackground(false);
        single.unregisterDraggedTypes();
        configure_images(&single, settings);
        if let Some(streaming) = check.streaming {
            single.set_streaming(streaming);
        }
        drop(state);
        // In a row where the first segment's row is, where its view is.
        let row_frame = check.first_row.frame();
        let inset = check.rect.origin.y - row_frame.origin.y;
        let first_offset = check.views.first().map_or(0.0, |view| view.2);
        let container = TranscriptRowView::new(row_frame, self.mtm);
        container.addSubview(&single);
        for row in &check.rows {
            row.setHidden(true);
        }
        self.document.addSubview(&container);
        single.setFrameOrigin(NSPoint::new(0.0, inset - first_offset));
        single.update(upleft_core::parser::MarkdownParser::parse(&check.text), &DirtySet::wholesale(), false);
        single.set_hosted_content_offset(first_offset);
        single.set_hosted_full_viewport(true);
        let height = single.frame().size.height - first_offset;
        container.setFrameSize(NSSize::new(row_frame.size.width, inset + height));
        single.prepare_for_display();
        let places: Vec<(isize, f64, f64)> = fragment_places(&single, false).into_iter().map(|(at, y, height)| (at, y - first_offset, height)).collect();
        let layout = places
            .iter()
            .zip(&check.fragments)
            .find_map(|(&(at, y, height), &(other_at, other_y, other_height))| {
                (at != other_at || (other_y - y).abs() > 1e-6 || (height - other_height).abs() > 1e-6)
                    .then_some((at.min(other_at), (other_y, other_height), (y, height)))
            })
            .or_else(|| {
                (places.len() != check.fragments.len()).then(|| {
                    let at = places.len().min(check.fragments.len());
                    let one = places.get(at).map_or((f64::NAN, 0.0), |f| (f.1, f.2));
                    let other = check.fragments.get(at).map_or((f64::NAN, 0.0), |f| (f.1, f.2));
                    (-1, other, one)
                })
            });
        check.single = Some((container, NSRect::new(check.rect.origin, NSSize::new(check.rect.size.width, height))));
        Some(Equivalence {
            segments: check.rows.len(),
            sealed: check.sealed,
            streaming: check.streaming == Some(true),
            units: check.text.encode_utf16().count(),
            layout,
            stack_height: check.rect.size.height,
            single_height: height,
            views: check.views.clone(),
        })
    }

    /// Puts the stack back.
    pub(crate) fn end_equivalence(&self) {
        let Some(check) = self.equivalence.borrow_mut().take() else { return };
        if let Some((container, _)) = check.single {
            container.removeFromSuperview();
        }
        for row in check.rows {
            row.setHidden(false);
        }
    }

    /// For the Markdown dump: forgets every row and every remembered parse
    /// and height, so the next `sync` sets each message as one opened from
    /// history is, cut by `transcript_parse::segment`.
    pub(crate) fn forget_parses(&self) {
        self.reset();
        let mut state = self.state.borrow_mut();
        state.segmentations.clear();
        state.segmentation_bytes = 0;
        state.heights.clear();
    }

    /// The last assistant message's stream, for the Markdown dump's replay.
    pub(crate) fn replay_state(&self) -> Option<ReplayState> {
        let state = self.state.borrow();
        let row = state.rows.iter().rposition(|row| row.message.kind == TranscriptMessageKind::Assistant)?;
        let markdown = state.rows[row].markdown.as_ref()?;
        Some(match &markdown.live {
            Some(live) => ReplayState {
                settled: !live.in_flight
                    && live.applied.as_ref().map(|(text, _)| text.as_str()) == state.rows[row].message.body.get(live.tail_byte..),
                sealed: live.sealed.len(),
            },
            None => ReplayState { settled: markdown.segments.is_some(), sealed: 0 },
        })
    }
}

/// A reader whose view ends within `NEAR_BOTTOM` of the document's end
/// follows it as it grows.
/// A Markdown row's laid-out height and top, when all its pieces have been
/// laid out and none has a view.
fn row_extent(state: &State, row: usize) -> Option<(f64, f64)> {
    let mut pieces = state.pieces.iter().filter(|piece| matches!(piece.kind, PieceKind::Markdown { row: other, .. } if other == row)).peekable();
    let top = pieces.peek()?.y;
    let mut height = 0.0;
    for piece in pieces {
        if !piece.placed || piece.view.is_some() {
            return None;
        }
        height += piece.shown();
    }
    Some((height, top))
}

fn near_bottom(document_height: f64, visible_max_y: f64) -> bool {
    document_height - visible_max_y <= NEAR_BOTTOM
}

// MARK: - Examples

/// A container for `set_example`, flipped as the transcript's rows are.
pub(crate) fn example_container(mtm: MainThreadMarker) -> Retained<NSView> {
    Retained::into_super(TranscriptRowView::new(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 100.0)), mtm))
}

/// `message` set in `container` as the transcript sets a user or assistant
/// message, `width` wide, with a margin of `inset` all round: Settings' live
/// type examples. Inert: no delegate, so no links, menus or selection
/// handling beyond the view's own. Returns the height.
pub(crate) fn set_example(container: &NSView, message: &TranscriptMessage, typography: Typography, width: f64, inset: f64) -> f64 {
    let mtm = MainThreadMarker::from(container);
    for subview in container.subviews().iter() {
        subview.removeFromSuperview();
    }
    let styles = Styles::shared(typography, &container.effectiveAppearance());
    let user = message.kind == TranscriptMessageKind::User;
    let column = (width - inset * 2.0).max(120.0);
    let indent = user_indent(typography.style, column);
    let text_width = if user { column - indent - USER_RULE - USER_LEFT - USER_RIGHT } else { column };
    let sheet = if user { styles.user.clone() } else { styles.assistant.clone() };
    let storage: Retained<NSTextStorage> =
        // SAFETY: NSTextStorage's initialiser takes a string.
        unsafe { msg_send![NSTextStorage::alloc(), initWithString: &*NSString::from_str(&message.body)] };
    let view = MarkdownTextView::new_hosted(&storage, sheet, text_width, mtm);
    view.setDrawsBackground(false);
    view.unregisterDraggedTypes();
    // The samples are a few words of fixed text.
    view.update(upleft_core::parser::MarkdownParser::parse(&message.body), &DirtySet::wholesale(), false);
    let speaker_font = typography.speaker_font();
    let label_height = line_height(&speaker_font);
    let top = if user { USER_TOP } else { 0.0 } + label_height + SPEAKER_GAP;
    let text_height = view.frame().size.height;
    let box_height = top + text_height + if user { USER_BOTTOM } else { 0.0 };
    let x = inset + if user { indent + USER_RULE + USER_LEFT } else { 0.0 };
    view.setFrameOrigin(NSPoint::new(x, inset + top));
    if user {
        let frame = NSRect::new(NSPoint::new(inset + indent, inset), NSSize::new(column - indent, box_height));
        for fill in user_decor(typography.style, 0, 1, frame, mtm) {
            container.addSubview(&fill);
        }
    }
    let color = if user { theme::accent() } else { theme::secondary_text() };
    let label = speaker_text(&speaker_name(message), &speaker_font, &color);
    let label_top = inset + if user { USER_TOP } else { 0.0 };
    container.addSubview(&TranscriptLabelView::new(label, NSRect::new(NSPoint::new(x, label_top), NSSize::new(text_width, label_height)), mtm));
    container.addSubview(&view);
    let height = (box_height + inset * 2.0).ceil();
    container.setFrameSize(NSSize::new(width, height));
    height
}

// MARK: - Rows

fn new_row(state: &mut State, message: TranscriptMessage, fingerprint: u64) -> Row {
    let id = state.next_id;
    state.next_id += 1;
    let markdown = matches!(message.kind, TranscriptMessageKind::User | TranscriptMessageKind::Assistant).then(|| MarkdownRow {
        segments: cached_segmentation(state, fingerprint),
        requested: false,
        live: None,
    });
    Row { id, fingerprint, message, markdown, arrived: None, landed: None }
}

fn fingerprint(message: &TranscriptMessage) -> u64 {
    let mut hasher = DefaultHasher::new();
    message.kind.hash(&mut hasher);
    message.author.hash(&mut hasher);
    message.body.hash(&mut hasher);
    message.status.hash(&mut hasher);
    // Not the detail's id: it is the message's index, which history loaded
    // above changes, and every row below would be set again. Activity rows
    // link their detail by row id instead.
    message.detail_id.is_some().hash(&mut hasher);
    hasher.finish()
}

/// A piece's identity: the row it belongs to and which segment of how many.
/// A streamed row's segments keep theirs as the tail is sealed (no count):
/// the view that showed the tail goes on showing its head.
fn piece_key(row: &Row, segment: usize, count: Option<usize>) -> u64 {
    let mut hasher = DefaultHasher::new();
    row.id.hash(&mut hasher);
    segment.hash(&mut hasher);
    count.hash(&mut hasher);
    hasher.finish()
}

fn text_key(text: &str, kind: TranscriptMessageKind) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    kind.hash(&mut hasher);
    hasher.finish()
}

/// A run of activity rows' height key: what they say, the width, the type,
/// and whether images show inline.
/// A piece's key in a session's saved heights: what it shows, stable from
/// one run of the app to the next (not its rows' ids).
fn saved_key(state: &State, index: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    match &state.pieces[index].kind {
        PieceKind::Markdown { row, segment, count } => {
            let parsed = state.rows[*row].markdown.as_ref().is_some_and(|markdown| markdown.segments.is_some() && markdown.live.is_none());
            "markdown".hash(&mut hasher);
            state.rows[*row].fingerprint.hash(&mut hasher);
            segment.hash(&mut hasher);
            parsed.then_some(*count).hash(&mut hasher);
        }
        PieceKind::Activity { rows } => {
            "activity".hash(&mut hasher);
            for row in &state.rows[rows.clone()] {
                row.fingerprint.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// The saved heights, if they were measured at this width and look.
fn saved_state(state: &State) -> Option<&SavedState> {
    let saved = state.saved.as_ref()?;
    let styles = state.styles.as_ref()?;
    (saved.width == state.column.width.to_bits() && saved.metrics == styles.metrics_key).then_some(saved)
}

/// The height piece `index` had when the session was last shown: its own,
/// or, for a Markdown row not cut into pieces yet, all of the row's.
fn saved_height(state: &State, index: usize) -> Option<f64> {
    let saved = saved_state(state)?;
    if let Some(height) = saved.pieces.get(&saved_key(state, index)) {
        return Some(*height);
    }
    let PieceKind::Markdown { row, .. } = state.pieces[index].kind else { return None };
    let unparsed = state.rows[row].markdown.as_ref().is_none_or(|markdown| markdown.segments.is_none());
    unparsed.then(|| saved.rows.get(&state.rows[row].fingerprint).copied()).flatten()
}

/// While older history arrives: the saved total less what is here, on whole
/// device pixels, so the document is its whole height from the first page.
///
/// Only the look has to match, not the width: before the first page, the
/// transcript is too short for a scroller, and the width a scroller takes is
/// what makes it match. The total is near enough to show one; rows take
/// their saved heights once the width is theirs. What is here counts at its
/// shown height, a correction held on it (`scroll_reserve`) included, so a
/// change anywhere in what has loaded moves the history above, not the end.
fn saved_above(state: &State, scale: f64) -> f64 {
    if !state.history_loading {
        return 0.0;
    }
    let Some(saved) = state.saved.as_ref() else { return 0.0 };
    if state.styles.as_ref().is_none_or(|styles| styles.metrics_key != saved.metrics) {
        return 0.0;
    }
    let here: f64 = state.pieces.iter().map(Piece::shown).sum();
    scroll_reserve::whole_pixels((saved.total - here).max(0.0), scale)
}

fn activity_height_key(state: &State, rows: Range<usize>, styles: &Styles) -> HeightKey {
    let mut hasher = DefaultHasher::new();
    "activity".hash(&mut hasher);
    for row in &state.rows[rows] {
        row.fingerprint.hash(&mut hasher);
    }
    state.settings.as_ref().is_some_and(|settings| settings.show_inline_images).hash(&mut hasher);
    HeightKey { text: hasher.finish(), width: state.column.width.to_bits(), metrics: styles.metrics_key }
}

fn remember_height(state: &mut State, key: HeightKey, height: f64) {
    if state.heights.len() >= HEIGHT_CACHE && !state.heights.contains_key(&key) {
        state.heights.clear();
    }
    state.heights.insert(key, height);
}

fn cached_segmentation(state: &mut State, fingerprint: u64) -> Option<Rc<[Rc<Segment>]>> {
    state.clock += 1;
    let clock = state.clock;
    let entry = state.segmentations.get_mut(&fingerprint)?;
    entry.1 = clock;
    Some(entry.0.clone())
}

/// Keeps a message's parse for the next time it is shown, within a bound on
/// how many and how much text; the least recently used go first.
fn remember_segmentation(state: &mut State, fingerprint: u64, segments: Rc<[Rc<Segment>]>) {
    let bytes = |segments: &Rc<[Rc<Segment>]>| segments.iter().map(|piece| piece.text.len()).sum::<usize>();
    state.clock += 1;
    state.segmentation_bytes += bytes(&segments);
    if let Some((old, _)) = state.segmentations.insert(fingerprint, (segments, state.clock)) {
        state.segmentation_bytes -= bytes(&old);
    }
    while state.segmentations.len() > SEGMENTATIONS || state.segmentation_bytes > SEGMENTATION_BYTES {
        let Some(oldest) = state.segmentations.iter().min_by_key(|(_, (_, used))| *used).map(|(print, _)| *print) else { break };
        if oldest == fingerprint && state.segmentations.len() == 1 {
            break;
        }
        if let Some((old, _)) = state.segmentations.remove(&oldest) {
            state.segmentation_bytes -= bytes(&old);
        }
    }
}

/// Where a piece starts, `y` being where the one above it ends: a message
/// or a run of activity rows starts on a whole device pixel, so a height
/// changing above it (history loaded, a row made, a diagram landing) moves
/// it by whole pixels, and its views keep the pixels they were drawn on
/// (`place_views`) rather than laying out and drawing again. The segments
/// of a message follow one another exactly, as one view's text would.
fn piece_top(kind: &PieceKind, y: f64, scale: f64) -> f64 {
    match kind {
        PieceKind::Markdown { segment, .. } if *segment > 0 => y,
        _ => (y * scale - 1.0e-6).ceil() / scale,
    }
}

/// Whether a view in `top..bottom` draws a diagram or formula placeholder.
fn objects_in_sight(state: &State, top: f64, bottom: f64) -> bool {
    if upleft_render::fragments::async_objects::pending_count() == 0 {
        return false;
    }
    let first = piece_at(state, top).unwrap_or(state.pieces.len());
    state.pieces[first..].iter().take_while(|piece| piece.y < bottom).any(|piece| match &piece.view {
        Some(RowViews { content: Content::Markdown { view, .. }, .. }) => {
            let origin = piece.y + view.frame().origin.y;
            view.hosted_pending_objects().iter().any(|(from, to)| origin + from < bottom && origin + to > top)
        }
        _ => false,
    })
}

/// What a row says, for the benchmark's probe.
fn content_hash(row: &Row) -> u64 {
    let mut hasher = DefaultHasher::new();
    row.message.kind.hash(&mut hasher);
    row.message.body.hash(&mut hasher);
    hasher.finish()
}

/// The piece at document `y`.
fn piece_at(state: &State, y: f64) -> Option<usize> {
    let index = state.pieces.partition_point(|piece| piece.y + piece.shown() <= y);
    (index < state.pieces.len()).then_some(index)
}

fn find_view(state: &State, pointer: usize) -> Option<usize> {
    state.pieces.iter().position(|piece| {
        matches!(&piece.view, Some(RowViews { content: Content::Markdown { view, .. }, .. })
            if Retained::as_ptr(view) as usize == pointer)
    })
}

fn speaker_name(message: &TranscriptMessage) -> String {
    if message.author == "omp" { "Omperor".to_owned() } else { message.author.clone() }
}

fn speaker_text(name: &str, font: &NSFont, color: &NSColor) -> Retained<objc2_foundation::NSAttributedString> {
    // Omperor's turns wear the crown: the gate's gold mark before the name.
    let crowned = name == "Omperor";
    let text = NSMutableAttributedString::from_nsstring(&NSString::from_str(&if crowned { format!("\u{fffc}\u{2009}{name}") } else { name.to_owned() }));
    let range = NSRange::new(0, text.length());
    // SAFETY: the attribute names are paired with the value types AppKit
    // documents for them, over the whole string.
    unsafe {
        text.addAttribute_value_range(objc2_app_kit::NSFontAttributeName, font, range);
        text.addAttribute_value_range(objc2_app_kit::NSForegroundColorAttributeName, color, range);
    }
    if crowned {
        // SAFETY: AppKit exports the weight for the process lifetime.
        let weight = unsafe { objc2_app_kit::NSFontWeightSemibold };
        if let Some(image) = theme::symbol("crown.fill", font.pointSize() * 0.82, weight, Some(&theme::accent())) {
            let attachment = objc2_app_kit::NSTextAttachment::new();
            attachment.setImage(Some(&image));
            let size = image.size();
            attachment.setBounds(NSRect::new(NSPoint::new(0.0, -size.height * 0.08), size));
            // SAFETY: an NSTextAttachment is the attachment attribute's value;
            // the object-replacement character is the first UTF-16 unit.
            unsafe { text.addAttribute_value_range(objc2_app_kit::NSAttachmentAttributeName, &attachment, NSRange::new(0, 1)) };
        }
    }
    Retained::into_super(text)
}

/// The height of a line of `font` as TextKit 1 sets it.
/// Where a hosted view goes whose text starts at `top` (document points):
/// its frame's top on the last whole device pixel at `scale`, and its text
/// the rest below (`Transcript::place_views`).
fn pixel_place(top: f64, scale: f64) -> (f64, f64) {
    let frame = (top * scale + 1.0e-6).floor() / scale;
    (frame, top - frame)
}

/// Puts `view`'s frame at `y` in its row, its text `offset` below; a new
/// offset draws its fragments again.
fn place_view(view: &MarkdownTextView, y: f64, offset: f64) {
    let origin = view.frame().origin;
    if origin.y != y {
        view.setFrameOrigin(NSPoint::new(origin.x, y));
    }
    view.set_hosted_content_offset(offset);
}

fn line_height(font: &NSFont) -> f64 {
    MEASURE.with(|measure| {
        let manager = measure.get_or_init(NSLayoutManager::new);
        manager.defaultLineHeightForFont(font)
    })
}

/// The space the first paragraph asks for above it, which TextKit drops
/// at the top of a view: a segment that continues a message restores it.
fn first_spacing_before(storage: &NSTextStorage) -> f64 {
    if storage.length() == 0 {
        return 0.0;
    }
    // SAFETY: the index lies in the storage; a null range pointer is allowed.
    let style = unsafe { storage.attribute_atIndex_effectiveRange(NSParagraphStyleAttributeName, 0, std::ptr::null_mut()) };
    style
        .and_then(|style| style.downcast::<NSParagraphStyle>().ok())
        .map_or(0.0, |style| style.paragraphSpacingBefore())
}

/// Images: relative paths against the session's folder, any local file,
/// and remote ones through the app's loader; none when they are turned off.
fn configure_images(view: &MarkdownTextView, settings: &Settings) {
    if !settings.show_inline_images {
        view.set_remote_image_resolver(Some(Rc::new(|_: &str| RemoteImage::Failed)));
        return;
    }
    let document = settings.image_base.join(".omperor-transcript.md");
    view.set_document_url(Some(document.to_string_lossy().into_owned()));
    view.set_local_asset_authorizer(Some(Rc::new(|_: &str| true)));
    view.set_remote_image_resolver(Some(Rc::new(native_markdown::remote_image)));
}

fn copy_text(text: &str) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: the type is an AppKit static.
    pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
}

/// A height from the text alone: lines at the measure, code lines, and a
/// placeholder's worth for diagrams, formulas and images. Close enough to
/// place a row until it is laid out.
pub(crate) fn estimate_markdown(text: &str, width: f64, body: f64) -> f64 {
    let line = body * 1.6;
    let per_line = (width / (body * 0.5)).max(20.0);
    let code = body * 1.35;
    let mut height = 0.0;
    let mut fence: Option<&str> = None;
    let mut fence_lines = 0usize;
    for raw in text.lines() {
        let trimmed = raw.trim_start();
        if let Some(kind) = fence {
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                height += match kind {
                    "mermaid" => (48.0 + fence_lines as f64 * 30.0).clamp(120.0, 480.0) + line,
                    "math" => 60.0 + line,
                    _ => fence_lines as f64 * code + line * 1.5,
                };
                fence = None;
                fence_lines = 0;
            } else {
                fence_lines += 1;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(match trimmed.trim_start_matches(['`', '~']).trim() {
                "mermaid" => "mermaid",
                "math" => "math",
                _ => "code",
            });
            continue;
        }
        if trimmed.is_empty() {
            height += body * 0.4;
        } else if trimmed.starts_with('#') {
            height += line * 1.8;
        } else if trimmed.starts_with("![") {
            height += 240.0;
        } else if trimmed.starts_with('|') {
            height += line * 1.3;
        } else if trimmed == "$$" {
            height += 30.0;
        } else {
            height += (trimmed.chars().count() as f64 / per_line).ceil().max(1.0) * line;
        }
    }
    if fence.is_some() {
        height += fence_lines as f64 * code + line;
    }
    height.max(line)
}

// MARK: - The renderer's delegate

struct RowDelegate {
    transcript: Weak<Transcript>,
}

impl MarkdownTextViewDelegate for RowDelegate {
    fn did_change_content_height(&self, view: &MarkdownTextView, _height: f64) {
        if let Some(transcript) = self.transcript.upgrade() {
            transcript.height_reported(view);
        }
    }

    fn did_activate_link(&self, _view: &MarkdownTextView, destination: &str, _range: upleft_core::NSRange, _modifiers: NSEventModifierFlags) {
        if let Some(transcript) = self.transcript.upgrade() {
            transcript.open_link(destination);
        }
    }

    fn did_change_selection(&self, view: &MarkdownTextView) {
        if let Some(transcript) = self.transcript.upgrade() {
            transcript.selection_changed(view);
        }
    }

    fn wants_context_menu_for(&self, view: &MarkdownTextView, target: &ContextTarget) -> Option<Retained<NSMenu>> {
        self.transcript.upgrade()?.context_menu(view, target)
    }

    /// Task boxes in a chat are a record, not a checklist: a click changes
    /// nothing.
    fn did_toggle_checkbox_at_mark_offset(&self, _view: &MarkdownTextView, _offset: isize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transcript_only_follows_when_the_view_is_near_the_end() {
        assert!(near_bottom(1_000.0, 1_000.0));
        assert!(near_bottom(1_000.0, 970.0));
        assert!(!near_bottom(1_000.0, 900.0));
        assert!(near_bottom(300.0, 500.0));
    }

    #[test]
    fn marks_are_named_and_read_back() {
        for mark in [
            Mark::Line { row: 7, offset: 1_204, element: 1_190, character: 57 },
            Mark::Piece { row: 12, offset: 0 },
            Mark::Activity { row: 3 },
        ] {
            assert_eq!(Mark::parse(&mark.name()), Some(mark));
            assert_eq!(mark.with_row(99).row(), 99);
        }
        assert_eq!(Mark::parse("Q1:2"), None);
    }

    #[test]
    fn a_row_keeps_its_fingerprint_when_history_renumbers_its_detail() {
        let message = |detail_id| TranscriptMessage {
            author: "read".into(),
            body: "Read src/main.rs".into(),
            kind: TranscriptMessageKind::Tool,
            status: None,
            detail_id,
        };
        assert_eq!(fingerprint(&message(Some(4))), fingerprint(&message(Some(40))));
        assert_ne!(fingerprint(&message(Some(4))), fingerprint(&message(None)));
    }

    #[test]
    fn an_estimate_grows_with_the_text_and_counts_diagrams() {
        let short = estimate_markdown("One line.", 700.0, 15.0);
        let long = estimate_markdown(&"A line of prose that wraps.\n\n".repeat(20), 700.0, 15.0);
        assert!(long > short * 10.0);
        let diagram = estimate_markdown("```mermaid\nflowchart LR\n  A --> B\n```\n", 700.0, 15.0);
        assert!(diagram >= 120.0);
    }
}
