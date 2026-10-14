//! The timeline window's event list, virtualized the way a table view is:
//! only the rows in or near view have a view, each a single small view
//! that draws its own row. A session of thousands of events costs what a
//! screenful does. Older pages land by growing the document upwards: the
//! scroller keeps its place, as the chat's does when history arrives, and
//! the rows already in view keep their views.
use super::*;
use objc2_app_kit::{NSClipView, NSStringDrawing};
use std::collections::HashMap;

thread_local! {
    /// How many times a row has been drawn, and how many times one has been
    /// set from its text rather than copied from its image, for the smoke's
    /// report.
    static ROW_DRAWS: Cell<u64> = const { Cell::new(0) };
    static ROW_RENDERS: Cell<u64> = const { Cell::new(0) };
}

/// Space above the first row and below the last.
pub(super) const LIST_PADDING: f64 = 6.0;
/// How far past each edge of the view rows are kept drawn, so a scroll
/// finds them ready.
const OVERSCAN: f64 = 160.0;

/// What a row's view draws, made from its event once when the view is made
/// so a redraw only draws.
#[derive(Debug, Clone)]
enum Paint {
    Event {
        kind: timeline::Kind,
        /// Who or what, with the sub-agent in front when there is one.
        label: Retained<NSString>,
        detail: Retained<NSString>,
        offset: Retained<NSString>,
        figure: Retained<NSString>,
        duration_ms: u64,
        waiting_ms: u64,
        failed: bool,
        /// A sub-agent's event, which sits one level in.
        nested: bool,
    },
    Hidden {
        text: Retained<NSString>,
    },
}

impl Paint {
    /// The row's paint and what a screen reader says for it.
    fn of(row: &timeline::Row, loaded: &timeline::Loaded) -> Option<(Self, String)> {
        Some(match row {
            timeline::Row::Hidden {
                count,
                total_ms,
                open,
                ..
            } => {
                let plural = if *count == 1 { "" } else { "s" };
                let text = if *open {
                    format!("▾ {count} event{plural} the filter took out, shown dimmed · click to fold")
                } else {
                    format!(
                        "▸ {count} event{plural} hidden by the filter · {} · click to show",
                        duration_text(*total_ms)
                    )
                };
                (
                    Self::Hidden {
                        text: NSString::from_str(&text),
                    },
                    format!("{count} event{plural} hidden by the filter"),
                )
            }
            timeline::Row::Event { index, .. } => {
                let event = loaded.get(*index)?;
                let label = match event.agent.as_deref() {
                    Some(agent) => format!("↳ {agent} · {}", event.label),
                    None => event.label.clone(),
                };
                let spoken = format!("{label}: {}", event.detail);
                (
                    Self::Event {
                        kind: event.kind,
                        label: NSString::from_str(&label),
                        detail: NSString::from_str(&event.detail),
                        offset: NSString::from_str(&format!(
                            "+{}",
                            offset_text(event.started_ms.saturating_sub(loaded.summary.origin_ms))
                        )),
                        figure: NSString::from_str(&duration_text(event.duration_ms())),
                        duration_ms: event.duration_ms(),
                        waiting_ms: event.waiting_ms,
                        failed: event.failed,
                        nested: event.agent.is_some(),
                    },
                    spoken,
                )
            }
        })
    }
}

/// How many times a row has been set from its text at one size.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drawn {
    size: NSSize,
    times: u8,
}

impl Default for Drawn {
    fn default() -> Self {
        Self {
            size: NSSize::new(0.0, 0.0),
            times: 0,
        }
    }
}

impl Drawn {
    /// The count after one more draw at `size` from the text, or `None`
    /// once `limit` such draws have been made there and it is time for an
    /// image.
    fn times_at(self, size: NSSize, limit: u8) -> Option<u8> {
        if self.size != size {
            Some(1)
        } else {
            (self.times < limit).then_some(self.times + 1)
        }
    }
}

#[derive(Debug)]
pub(super) struct TimelineRowIvars {
    row: timeline::Row,
    paint: Paint,
    /// How often the row has been set from its text at its size, and the
    /// row as an image once it keeps being asked to draw at that size.
    drawn: Cell<Drawn>,
    image: RefCell<Option<Retained<NSImage>>>,
    /// Every bar is drawn against the session's longest event.
    longest_ms: u64,
    /// Who hears about clicks: the delegate, kept as a plain object.
    target: Retained<AnyObject>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond calling the designated
    //   initializer, which `new` does.
    // - MainThreadOnly matches AppKit's threading model.
    // - TimelineRowView does not implement Drop.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TimelineRowIvars]
    #[derive(Debug)]
    pub(super) struct TimelineRowView;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for TimelineRowView {}

    // SAFETY: every override keeps AppKit's declared signature; drawRect:
    // draws only inside the view's bounds, and mouseDown: consumes the click.
    impl TimelineRowView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// The window moves by its background; a press on a row is a click
        /// on it, never the start of a window drag.
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        /// A row is set from its text the first times it is drawn at a
        /// size. Scrolling asks every row in view to draw again on every
        /// step, so a row that keeps being asked is set once more into an
        /// image, and from then on a draw copies pixels, which is far cheaper
        /// than setting the text. A row passed over in a jump is drawn only a
        /// time or two and never pays for the image.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let _span = perf::span("gui.timeline.draw_row");
            ROW_DRAWS.with(|draws| draws.set(draws.get() + 1));
            let ivars = self.ivars();
            let bounds = self.bounds();
            let cached = ivars
                .image
                .borrow()
                .clone()
                .filter(|image| image.size() == bounds.size);
            if let Some(image) = cached {
                image.drawInRect(bounds);
            } else if let Some(times) = ivars.drawn.get().times_at(bounds.size, Self::DRAWS_BEFORE_IMAGE) {
                ivars.drawn.set(Drawn { size: bounds.size, times });
                ROW_RENDERS.with(|renders| renders.set(renders.get() + 1));
                Styles::with(|styles| {
                    Pen::new(styles, bounds.size.width, ivars.longest_ms)
                        .row(&ivars.paint, bounds.size.height);
                });
            } else {
                let image = self.render(bounds.size);
                ivars.image.replace(Some(image.clone()));
                image.drawInRect(bounds);
            }
        }

        /// Dark and light set the row in different inks: draw it afresh.
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn view_did_change_effective_appearance(&self) {
            // SAFETY: NSView implements the method; the call keeps its
            // behaviour.
            let _: () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            self.ivars().image.replace(None);
            self.ivars().drawn.set(Drawn::default());
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let target = self.ivars().target.clone();
            // SAFETY: the target is the delegate, which implements
            // timelineRowClicked: and outlives the window.
            let _: () = unsafe { msg_send![&*target, timelineRowClicked: self] };
        }
    }
);

impl TimelineRowView {
    /// Draws from the text at one size before the row keeps an image: a
    /// jump draws a row about twice before it goes by, a scroll draws it on
    /// every step.
    const DRAWS_BEFORE_IMAGE: u8 = 2;

    fn new(
        frame: NSRect,
        row: timeline::Row,
        (paint, spoken): (Paint, String),
        longest_ms: u64,
        target: &AnyObject,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let dimmed = matches!(row, timeline::Row::Event { dimmed: true, .. });
        let this = Self::alloc(mtm).set_ivars(TimelineRowIvars {
            row,
            paint,
            drawn: Cell::new(Drawn::default()),
            image: RefCell::new(None),
            longest_ms,
            target: target.retain(),
        });
        // SAFETY: NSView's designated initializer takes a frame rectangle.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setAccessibilityLabel(Some(&NSString::from_str(&spoken)));
        // A run the filter took out, opened: its events at half strength.
        if dimmed {
            view.setAlphaValue(0.5);
        }
        view
    }

    /// An image of the row at `size`, set when it is first drawn and kept
    /// by the image for later draws at the same scale.
    fn render(&self, size: NSSize) -> Retained<NSImage> {
        let paint = self.ivars().paint.clone();
        let longest_ms = self.ivars().longest_ms;
        let handler = block2::RcBlock::new(move |_rect: NSRect| -> objc2::runtime::Bool {
            let _span = perf::span("gui.timeline.render_row");
            ROW_RENDERS.with(|renders| renders.set(renders.get() + 1));
            Styles::with(|styles| Pen::new(styles, size.width, longest_ms).row(&paint, size.height));
            objc2::runtime::Bool::YES
        });
        NSImage::imageWithSize_flipped_drawingHandler(size, true, &handler)
    }

    /// The row this view draws.
    pub(super) fn row(&self) -> &timeline::Row {
        &self.ivars().row
    }
}

/// The list: the document the scroller shows, the events that have
/// arrived, the rows as laid out, and a view for each row in or near view.
#[derive(Debug)]
pub(super) struct TimelineList {
    document: Retained<NSView>,
    target: Retained<AnyObject>,
    loaded: RefCell<timeline::Loaded>,
    rows: RefCell<Vec<timeline::Row>>,
    /// Each row's top below the first row's, then the last row's bottom.
    tops: RefCell<Vec<f64>>,
    live: RefCell<HashMap<timeline::Row, Retained<TimelineRowView>>>,
}

impl TimelineList {
    /// A list drawing into `document`, which must not be flipped: growing
    /// it for older rows then keeps what is in view where it is.
    pub(super) fn new(document: Retained<NSView>, target: &AnyObject) -> Self {
        // One tip for the list: a tip on every row makes AppKit rework the
        // window's tip areas each time a row comes into view.
        document.setToolTip(Some(ns_string!(
            "Click an event for its details, or a hidden run to show or fold it."
        )));
        Self {
            document,
            target: target.retain(),
            loaded: RefCell::default(),
            rows: RefCell::default(),
            tops: RefCell::new(vec![0.0]),
            live: RefCell::default(),
        }
    }

    /// What has arrived of the session's events.
    pub(super) fn loaded(&self) -> std::cell::Ref<'_, timeline::Loaded> {
        self.loaded.borrow()
    }

    pub(super) fn set_loaded(&self, loaded: timeline::Loaded) {
        self.loaded.replace(loaded);
    }

    /// Puts events from before everything that has arrived in front.
    pub(super) fn prepend(&self, older: Vec<timeline::Event>) {
        self.loaded.borrow_mut().prepend(older);
    }

    /// Takes the rows as laid out; `layout` puts them on screen.
    pub(super) fn set_rows(&self, rows: Vec<timeline::Row>) {
        self.tops.replace(timeline::row_tops(&rows));
        self.rows.replace(rows);
    }

    /// How many rows are laid out.
    pub(super) fn row_count(&self) -> usize {
        self.rows.borrow().len()
    }

    /// How many rows have a view now.
    pub(super) fn live_rows(&self) -> usize {
        self.live.borrow().len()
    }

    /// The first and last event with a view now.
    pub(super) fn events_in_view(&self) -> Option<(usize, usize)> {
        let live = self.live.borrow();
        let indices = live.keys().filter_map(|row| match row {
            timeline::Row::Event { index, .. } => Some(*index),
            timeline::Row::Hidden { .. } => None,
        });
        indices.fold(None, |range, index| match range {
            None => Some((index, index)),
            Some((first, last)) => Some((first.min(index), last.max(index))),
        })
    }

    /// Where row `index` starts below the document's top, and its height.
    pub(super) fn row_span(&self, index: usize) -> Option<(f64, f64)> {
        let tops = self.tops.borrow();
        let top = *tops.get(index)?;
        let bottom = *tops.get(index + 1)?;
        Some((LIST_PADDING + top, bottom - top))
    }

    /// Sizes the document to the rows and the scroller's width, then puts
    /// views on the rows in view.
    pub(super) fn layout(&self, clip: &NSClipView) {
        let bounds = clip.bounds();
        let width = bounds.size.width;
        let content = self.tops.borrow().last().copied().unwrap_or(0.0) + 2.0 * LIST_PADDING;
        let height = content.max(bounds.size.height);
        let frame = self.document.frame();
        if frame.size.width != width || frame.size.height != height {
            // No borrow is held here: resizing the document can scroll the
            // clip, whose notification lands in `sync`.
            self.document.setFrame(rect(0.0, 0.0, width, height));
        }
        self.sync(clip);
    }

    /// Puts a view on each row in or near view, keeping those that already
    /// have one, and takes the rest away.
    pub(super) fn sync(&self, clip: &NSClipView) {
        let _span = perf::span("gui.timeline.sync_rows");
        let visible = clip.bounds();
        let document = self.document.frame().size;
        // The document is not flipped: measure down from its top.
        let from = document.height - (visible.origin.y + visible.size.height);
        let to = document.height - visible.origin.y;
        let tops = self.tops.borrow();
        let rows = self.rows.borrow();
        let range = timeline::rows_between(
            &tops,
            from - LIST_PADDING - OVERSCAN,
            to - LIST_PADDING + OVERSCAN,
        );
        let frames: HashMap<&timeline::Row, NSRect> = range
            .map(|index| {
                let row = &rows[index];
                let height = row.height();
                let top = LIST_PADDING + tops[index];
                (row, rect(0.0, document.height - top - height, document.width, height))
            })
            .collect();
        let mut live = self.live.borrow_mut();
        live.retain(|row, view| {
            let keep = frames.contains_key(row);
            if !keep {
                view.removeFromSuperview();
            }
            keep
        });
        let loaded = self.loaded.borrow();
        let mtm = self.document.mtm();
        for (row, frame) in frames {
            if let Some(view) = live.get(row) {
                if view.frame() != frame {
                    let resized = view.frame().size.width != frame.size.width;
                    view.setFrame(frame);
                    if resized {
                        view.setNeedsDisplay(true);
                    }
                }
                continue;
            }
            let Some(paint) = Paint::of(row, &loaded) else {
                continue;
            };
            let view = TimelineRowView::new(
                frame,
                row.clone(),
                paint,
                loaded.summary.longest_ms,
                &self.target,
                mtm,
            );
            self.document.addSubview(&view);
            live.insert(row.clone(), view);
        }
    }
}

/// The attributes each kind of text on a row is set with, and the inks its
/// marks are drawn in, made once per thread: building fonts, colors and
/// paragraph styles for every row made a screenful of new rows cost more
/// than a frame. The colors are dynamic and still follow the appearance.
struct Styles {
    offset: Retained<NSDictionary<NSString, AnyObject>>,
    label: Retained<NSDictionary<NSString, AnyObject>>,
    failed_label: Retained<NSDictionary<NSString, AnyObject>>,
    detail: Retained<NSDictionary<NSString, AnyObject>>,
    figure: Retained<NSDictionary<NSString, AnyObject>>,
    hidden: Retained<NSDictionary<NSString, AnyObject>>,
    quiet: Retained<NSColor>,
    red: Retained<NSColor>,
    track: Retained<NSColor>,
    waiting: Retained<NSColor>,
    receiving: Retained<NSColor>,
    tool: Retained<NSColor>,
}

impl Styles {
    fn new() -> Self {
        let attributes = |font: &NSFont, color: &NSColor, alignment| {
            let style = NSMutableParagraphStyle::new();
            style.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            style.setAlignment(alignment);
            // SAFETY: AppKit exports the attribute names for the process
            // lifetime; each is paired with the value type it documents.
            let keys = unsafe {
                [
                    NSFontAttributeName,
                    NSForegroundColorAttributeName,
                    NSParagraphStyleAttributeName,
                ]
            };
            let values: [&AnyObject; 3] = [font.as_ref(), color.as_ref(), style.as_ref()];
            NSDictionary::from_slices(&keys, &values)
        };
        let body = theme::ui_font(11.0, false);
        let (quiet, ink, red) = (
            theme::secondary_text(),
            NSColor::labelColor(),
            NSColor::systemRedColor(),
        );
        // SAFETY: AppKit exports NSFontWeightRegular for the process lifetime.
        let digits = NSFont::monospacedDigitSystemFontOfSize_weight(10.5, unsafe {
            NSFontWeightRegular
        });
        let bold = theme::ui_font(12.0, true);
        Self {
            offset: attributes(&digits, &quiet, NSTextAlignment::Right),
            label: attributes(&bold, &ink, NSTextAlignment::Left),
            failed_label: attributes(&bold, &red, NSTextAlignment::Left),
            detail: attributes(&body, &quiet, NSTextAlignment::Left),
            figure: attributes(&body, &ink, NSTextAlignment::Right),
            hidden: attributes(&body, &quiet, NSTextAlignment::Center),
            quiet,
            red,
            track: theme::surface_inset(),
            waiting: theme::accent_fill_strong(),
            receiving: theme::accent(),
            tool: theme::plum(),
        }
    }

    /// Runs `draw` with the styles, made on first use.
    fn with<T>(draw: impl FnOnce(&Self) -> T) -> T {
        thread_local! {
            static STYLES: std::cell::OnceCell<Styles> = const { std::cell::OnceCell::new() };
        }
        STYLES.with(|styles| draw(styles.get_or_init(Self::new)))
    }
}

/// What drawing a row needs: the columns for its width, and the styles.
struct Pen<'a> {
    styles: &'a Styles,
    width: f64,
    longest: f64,
    bar_x: f64,
    bar_width: f64,
    figure_width: f64,
}

impl<'a> Pen<'a> {
    /// Where the label and detail start; a sub-agent's sit one level in.
    const TEXT_X: f64 = 86.0;
    const NESTED_INDENT: f64 = 22.0;
    /// A text field's own inset, which drawn text keeps so the columns line
    /// up with the window's labels.
    const INSET: f64 = 2.0;

    fn new(styles: &'a Styles, width: f64, longest_ms: u64) -> Self {
        let figure_width = 64.0;
        let bar_width = ((width - 320.0) * 0.4).clamp(90.0, 240.0);
        Self {
            styles,
            width,
            longest: longest_ms.max(1) as f64,
            bar_x: width - 16.0 - figure_width - 8.0 - bar_width,
            bar_width,
            figure_width,
        }
    }

    fn row(&self, paint: &Paint, height: f64) {
        match paint {
            Paint::Hidden { text } => self.hidden_run(text, height),
            Paint::Event { .. } => self.event(paint, height),
        }
    }

    fn fill(&self, frame: NSRect, color: &NSColor, radius: f64) {
        color.setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(frame, radius, radius).fill();
    }

    /// One line of `text`, cut short with an ellipsis when it does not fit.
    fn text(&self, text: &NSString, attributes: &NSDictionary<NSString, AnyObject>, frame: NSRect) {
        if text.length() == 0 {
            return;
        }
        // SAFETY: the dictionary maps AppKit's attribute names to the value
        // types they document.
        unsafe {
            text.drawInRect_withAttributes(
                rect(
                    frame.origin.x + Self::INSET,
                    frame.origin.y,
                    (frame.size.width - 2.0 * Self::INSET).max(0.0),
                    frame.size.height,
                ),
                Some(attributes),
            );
        }
    }

    /// A run of events the filter took out: one pill saying how many.
    fn hidden_run(&self, text: &NSString, height: f64) {
        self.fill(
            rect(12.0, 3.0, self.width - 24.0, height - 6.0),
            &self.styles.track,
            (height - 6.0) / 2.0,
        );
        self.text(
            text,
            &self.styles.hidden,
            rect(24.0, 5.0, self.width - 48.0, height - 8.0),
        );
    }

    /// An event: its offset from the session's start, who or what, the
    /// detail, a bar for how long it took against the longest event, and
    /// the figure.
    fn event(&self, paint: &Paint, height: f64) {
        let Paint::Event {
            kind,
            label,
            detail,
            offset,
            figure,
            duration_ms,
            waiting_ms,
            failed,
            nested,
        } = paint
        else {
            return;
        };
        let (kind, duration, failed) = (*kind, *duration_ms, *failed);
        let styles = self.styles;
        let text_x = Self::TEXT_X + if *nested { Self::NESTED_INDENT } else { 0.0 };
        let text_width = (self.bar_x - 12.0 - text_x).max(60.0);
        self.text(offset, &styles.offset, rect(12.0, 11.0, 64.0, 17.0));
        self.text(
            label,
            if failed { &styles.failed_label } else { &styles.label },
            rect(text_x, 3.0, text_width, 18.0),
        );
        self.text(detail, &styles.detail, rect(text_x, 20.0, text_width, height - 20.0));
        match kind {
            timeline::Kind::Prompt | timeline::Kind::Notice => {
                // A moment: a dot where the bar would start.
                let color = if kind == timeline::Kind::Notice {
                    &styles.red
                } else {
                    &styles.quiet
                };
                self.fill(rect(self.bar_x, 17.0, 6.0, 6.0), color, 3.0);
            }
            timeline::Kind::Response | timeline::Kind::Tool => {
                // Square-root scaled, so a short event still has a bar
                // beside a long one.
                let length = (self.bar_width * (duration as f64 / self.longest).sqrt())
                    .clamp(3.0, self.bar_width);
                self.fill(rect(self.bar_x, 16.0, self.bar_width, 8.0), &styles.track, 4.0);
                if kind == timeline::Kind::Response {
                    // The wait for the first token, then the tokens.
                    let waiting = if duration == 0 {
                        0.0
                    } else {
                        length * ((*waiting_ms).min(duration) as f64 / duration as f64)
                    };
                    if waiting >= 1.0 {
                        self.fill(rect(self.bar_x, 16.0, waiting, 8.0), &styles.waiting, 4.0);
                    }
                    if length - waiting >= 1.0 {
                        self.fill(
                            rect(self.bar_x + waiting, 16.0, length - waiting, 8.0),
                            &styles.receiving,
                            4.0,
                        );
                    }
                } else {
                    let color = if failed { &styles.red } else { &styles.tool };
                    self.fill(rect(self.bar_x, 16.0, length, 8.0), color, 4.0);
                }
                self.text(
                    figure,
                    &styles.figure,
                    rect(self.width - 16.0 - self.figure_width, 12.0, self.figure_width, 18.0),
                );
            }
        }
    }
}

/// `--timeline-smoke <journal>`: the journal whose timeline the smoke opens.
pub(super) fn smoke_journal() -> Option<std::path::PathBuf> {
    let mut arguments = std::env::args();
    arguments.position(|argument| argument == "--timeline-smoke")?;
    arguments.next().map(std::path::PathBuf::from)
}

/// With `OMPEROR_TIMELINE_SMOKE_SHOTS=<directory>`, the smoke stops for a
/// moment at a few points and has `screencapture` take a picture of its own
/// window (by window number, so nothing else on screen is captured).
fn shoot(smoke: &mut TimelineSmoke, window: &NSWindow, name: &str) {
    let Some(directory) = std::env::var_os("OMPEROR_TIMELINE_SMOKE_SHOTS") else {
        return;
    };
    // Long enough for the display to catch up and the picture to be taken.
    smoke.hold = 45;
    let _ = std::process::Command::new("/usr/sbin/screencapture")
        .args(["-o", "-x", "-l", &window.windowNumber().to_string()])
        .arg(std::path::Path::new(&directory).join(format!("{name}.png")))
        .spawn();
}

/// The phases of the timeline smoke, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmokePhase {
    Loading,
    Scrolling,
    Gliding,
    Filtering,
    Resizing,
    Done,
}

/// A run of `--timeline-smoke`: opens the timeline for one journal without
/// loading the user's sessions or starting inference, waits for the newest
/// rows and then the whole session, scrolls a screen at a time from the
/// newest event to the oldest, filters and clears, resizes, and reports how
/// long the rows took and the longest main-thread pass in each phase.
#[derive(Debug)]
pub(super) struct TimelineSmoke {
    started: std::time::Instant,
    phase: SmokePhase,
    step: usize,
    first_rows_ms: Option<u64>,
    complete_ms: Option<u64>,
    /// The pass counter at the last tick, to find the passes since.
    seen_passes: u64,
    /// Per phase before `Done`: the longest pass (µs) and how many passes
    /// went over budget.
    passes: [(u32, u64); 5],
    max_row_views: usize,
    /// The phase the log was last marked with.
    logged: Option<SmokePhase>,
    /// Ticks left to stay put while a picture of the window is taken.
    hold: usize,
    /// Per phase before `Done`: rows drawn, rows set from their text, and
    /// ticks spent.
    draws: [(u64, u64, u64); 5],
    seen_draws: (u64, u64),
    failures: Vec<String>,
}

impl AppDelegate {
    pub(super) fn run_timeline_smoke(&self, path: std::path::PathBuf) {
        let title = path
            .file_stem()
            .map_or_else(|| "journal".to_owned(), |stem| stem.to_string_lossy().into_owned());
        self.ivars().timeline_smoke.replace(Some(TimelineSmoke {
            started: std::time::Instant::now(),
            phase: SmokePhase::Loading,
            step: 0,
            first_rows_ms: None,
            complete_ms: None,
            seen_passes: frame_budget::snapshot().passes,
            passes: [(0, 0); 5],
            max_row_views: 0,
            logged: None,
            hold: 0,
            draws: [(0, 0, 0); 5],
            seen_draws: (ROW_DRAWS.with(Cell::get), ROW_RENDERS.with(Cell::get)),
            failures: Vec::new(),
        }));
        self.open_timeline(title, path);
        // SAFETY: AppDelegate implements timelineSmokeTick: with the timer
        // action signature and outlives the timer, which it invalidates.
        unsafe {
            let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                1.0 / 60.0,
                self,
                sel!(timelineSmokeTick:),
                None,
                true,
            );
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
    }

    pub(super) fn tick_timeline_smoke(&self, timer: &NSTimer) {
        let _span = perf::span("gui.timeline.smoke_tick");
        let Some(mut smoke) = self.ivars().timeline_smoke.borrow_mut().take() else {
            timer.invalidate();
            return;
        };
        // The passes since the last tick ran what that tick set going.
        let snapshot = frame_budget::snapshot();
        let new = usize::try_from(snapshot.passes - smoke.seen_passes).unwrap_or(usize::MAX);
        smoke.seen_passes = snapshot.passes;
        let draws = (ROW_DRAWS.with(Cell::get), ROW_RENDERS.with(Cell::get));
        if let Some((drawn, rendered, ticks)) = smoke.draws.get_mut(smoke.phase as usize) {
            *drawn += draws.0 - smoke.seen_draws.0;
            *rendered += draws.1 - smoke.seen_draws.1;
            *ticks += 1;
        }
        smoke.seen_draws = draws;
        if let Some((worst, over)) = smoke.passes.get_mut(smoke.phase as usize) {
            for pass in snapshot.recent.iter().rev().take(new) {
                *worst = (*worst).max(*pass);
                if u128::from(*pass) > frame_budget::BUDGET_US {
                    *over += 1;
                }
            }
        }
        let (Some(window), Some(scroll), Some(list)) = (
            self.ivars().timeline_window.borrow().clone(),
            self.ivars().timeline_scroll.borrow().clone(),
            self.ivars().timeline_list.borrow().clone(),
        ) else {
            smoke.failures.push("the timeline window went away".to_owned());
            self.finish_timeline_smoke(smoke, timer);
            return;
        };
        smoke.max_row_views = smoke.max_row_views.max(list.live_rows());
        if smoke.logged != Some(smoke.phase) {
            // Marks the log, so the frame-budget lines after it read as this
            // phase's.
            eprintln!("timeline-smoke phase {:?}", smoke.phase);
            smoke.logged = Some(smoke.phase);
        }
        if smoke.hold > 0 {
            smoke.hold -= 1;
            self.ivars().timeline_smoke.replace(Some(smoke));
            return;
        }
        let elapsed = u64::try_from(smoke.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match smoke.phase {
            SmokePhase::Loading => {
                if smoke.first_rows_ms.is_none() && list.loaded().len() > 0 {
                    smoke.first_rows_ms = Some(elapsed);
                }
                if self.ivars().timeline_stream.borrow().is_none() {
                    smoke.complete_ms = Some(elapsed);
                    let loaded = list.loaded();
                    if loaded.len() != loaded.summary.events {
                        smoke.failures.push(format!(
                            "{} of {} events arrived",
                            loaded.len(),
                            loaded.summary.events
                        ));
                    }
                    smoke.phase = SmokePhase::Scrolling;
                    shoot(&mut smoke, &window, "newest");
                } else if elapsed > 60_000 {
                    smoke.failures.push("the journal took over a minute".to_owned());
                    self.finish_timeline_smoke(smoke, timer);
                    return;
                }
            }
            SmokePhase::Scrolling => {
                // A screen at a time from the newest event to the oldest.
                let clip = scroll.contentView();
                let bounds = clip.bounds();
                let height = scroll.documentView().map_or(0.0, |document| document.frame().size.height);
                let top = (height - bounds.size.height).max(0.0);
                if bounds.origin.y >= top - 0.5 {
                    let in_view = list.events_in_view();
                    if list.loaded().len() > 0 && in_view.map(|(first, _)| first) != Some(0) {
                        smoke.failures.push(format!(
                            "at the top (clip y {} of {height}), the rows in view cover events {in_view:?}; {} rows laid out for {} events, {} views, filter {:?}",
                            bounds.origin.y,
                            list.row_count(),
                            list.loaded().len(),
                            list.live_rows(),
                            self.ivars().timeline_filter.borrow(),
                        ));
                    }
                    smoke.phase = SmokePhase::Gliding;
                    shoot(&mut smoke, &window, "oldest");
                } else {
                    clip.scrollToPoint(NSPoint::new(0.0, (bounds.origin.y + bounds.size.height).min(top)));
                    scroll.reflectScrolledClipView(&clip);
                }
            }
            SmokePhase::Gliding => {
                // Back down the way a trackpad scrolls: a few rows a frame.
                const GLIDE_STEP: f64 = 24.0;
                const GLIDE_STEPS: usize = 300;
                let clip = scroll.contentView();
                let y = (clip.bounds().origin.y - GLIDE_STEP).max(0.0);
                clip.scrollToPoint(NSPoint::new(0.0, y));
                scroll.reflectScrolledClipView(&clip);
                smoke.step += 1;
                if smoke.step == GLIDE_STEPS || y == 0.0 {
                    smoke.step = 0;
                    smoke.phase = SmokePhase::Filtering;
                }
            }
            SmokePhase::Filtering => {
                let search = self.ivars().timeline_search.borrow().clone();
                if let Some(search) = search {
                    search.setStringValue(&NSString::from_str(if smoke.step == 0 { "hub" } else { "" }));
                }
                self.apply_timeline_filter();
                if smoke.step == 0 {
                    shoot(&mut smoke, &window, "filtered");
                }
                smoke.step += 1;
                if smoke.step == 2 {
                    smoke.step = 0;
                    smoke.phase = SmokePhase::Resizing;
                }
            }
            SmokePhase::Resizing => {
                let mut frame = window.frame();
                frame.size.width += if smoke.step == 0 { 160.0 } else { -160.0 };
                window.setFrame_display(frame, true);
                smoke.step += 1;
                if smoke.step == 2 {
                    smoke.phase = SmokePhase::Done;
                }
            }
            SmokePhase::Done => {
                self.finish_timeline_smoke(smoke, timer);
                return;
            }
        }
        self.ivars().timeline_smoke.replace(Some(smoke));
    }

    fn finish_timeline_smoke(&self, mut smoke: TimelineSmoke, timer: &NSTimer) {
        timer.invalidate();
        // A virtualized list holds a few screens of rows, whatever the
        // session's length.
        if smoke.max_row_views > 64 {
            smoke.failures.push(format!("{} row views were alive at once", smoke.max_row_views));
        }
        let events = self
            .ivars()
            .timeline_list
            .borrow()
            .as_ref()
            .map_or(0, |list| list.loaded().len());
        let phase = |index: usize| {
            let (worst, over) = smoke.passes[index];
            let (drawn, rendered, ticks) = smoke.draws[index];
            let ticks = ticks.max(1) as f64;
            format!(
                "{:.1}ms/{over}/{:.1}/{:.1}",
                f64::from(worst) / 1_000.0,
                drawn as f64 / ticks,
                rendered as f64 / ticks
            )
        };
        eprintln!(
            "timeline-smoke {} events={events} first_rows_ms={} complete_ms={} worst_pass/over_budget/rows_drawn/rows_set_per_tick load={} page_scroll={} glide={} filter={} resize={} max_row_views={}{}",
            if smoke.failures.is_empty() { "PASS" } else { "FAIL" },
            smoke.first_rows_ms.map_or_else(|| "-".to_owned(), |ms| ms.to_string()),
            smoke.complete_ms.map_or_else(|| "-".to_owned(), |ms| ms.to_string()),
            phase(0),
            phase(1),
            phase(2),
            phase(3),
            phase(4),
            smoke.max_row_views,
            if smoke.failures.is_empty() {
                String::new()
            } else {
                format!(" failures={:?}", smoke.failures)
            },
        );
        if !smoke.failures.is_empty() {
            std::process::exit(1);
        }
        NSApplication::sharedApplication(self.mtm()).terminate(None);
    }
}
