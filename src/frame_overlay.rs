//! The frame budget in view: a small readout above the composer, on while
//! Developer mode asks for it, that draws the latest passes of the main
//! thread against the 4 ms line and names the last one over it.
use super::*;
use crate::frame_budget::{self, BUDGET_US};

/// The readout's size and place: pinned above the composer at the right
/// edge of the conversation pane.
const WIDTH: f64 = 344.0;
const HEIGHT: f64 = 72.0;
const INSET: f64 = 12.0;
/// The sparkline's height; a pass four times the budget reaches the top.
const SPARK_HEIGHT: f64 = 22.0;
const BAR_WIDTH: f64 = 2.0;
const BAR_GAP: f64 = 1.0;
/// How often the readout is brought up to date, and how many of those
/// refreshes pass between the figures being reset: the sparkline moves at
/// ten a second, the text at two, since relaying out two labels costs more
/// than drawing the bars.
const REFRESH_SECONDS: f64 = 0.1;
const FIGURES_EVERY: u32 = 5;

#[derive(Debug, Default)]
pub(super) struct FrameOverlayIvars {
    /// The latest passes, oldest first, in microseconds.
    samples: RefCell<Vec<u32>>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond calling the designated
    //   initializer, which `new` does.
    // - MainThreadOnly matches AppKit's threading model.
    // - FrameOverlayView does not implement Drop.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = FrameOverlayIvars]
    #[derive(Debug)]
    pub(super) struct FrameOverlayView;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for FrameOverlayView {}

    // SAFETY: both methods keep AppKit's declared signatures; the view draws
    // only inside its own bounds and, being a readout, is never hit, so the
    // transcript under it keeps every click and scroll.
    impl FrameOverlayView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }
    }
);

impl FrameOverlayView {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(FrameOverlayIvars::default());
        // SAFETY: NSView's designated initializer takes a frame rectangle.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// How many passes the sparkline has room for.
    fn capacity(&self) -> usize {
        ((self.bounds().size.width - 2.0 * INSET) / (BAR_WIDTH + BAR_GAP)).floor() as usize
    }

    fn set_samples(&self, samples: &[u32]) {
        let keep = samples.len().saturating_sub(self.capacity());
        let mut current = self.ivars().samples.borrow_mut();
        if *current != samples[keep..] {
            current.clear();
            current.extend_from_slice(&samples[keep..]);
            drop(current);
            self.setNeedsDisplay(true);
        }
    }

    fn draw(&self) {
        let bounds = self.bounds();
        let plate = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 8.0, 8.0);
        theme::surface().colorWithAlphaComponent(0.94).setFill();
        plate.fill();
        plate.setLineWidth(1.0);
        theme::hairline().setStroke();
        plate.stroke();

        // The sparkline: one bar per pass, the budget as a line across it,
        // a pass over budget in plum.
        let base = INSET;
        let ceiling = BUDGET_US as f64 * 4.0;
        let budget_y = base + SPARK_HEIGHT * (BUDGET_US as f64 / ceiling);
        let line = NSBezierPath::bezierPath();
        line.setLineWidth(1.0);
        line.moveToPoint(NSPoint::new(INSET, budget_y.round() + 0.5));
        line.lineToPoint(NSPoint::new(
            bounds.size.width - INSET,
            budget_y.round() + 0.5,
        ));
        theme::accent().colorWithAlphaComponent(0.55).setStroke();
        line.stroke();
        // Two paths, one fill each: the readout must not itself cost a frame.
        let samples = self.ivars().samples.borrow();
        let mut x = bounds.size.width - INSET - samples.len() as f64 * (BAR_WIDTH + BAR_GAP);
        let quiet = NSBezierPath::bezierPath();
        let loud = NSBezierPath::bezierPath();
        for micros in samples.iter() {
            let height = (SPARK_HEIGHT * (*micros as f64 / ceiling)).clamp(1.0, SPARK_HEIGHT);
            let bar = rect(x, base, BAR_WIDTH, height);
            if u128::from(*micros) > BUDGET_US {
                loud.appendBezierPathWithRect(bar);
            } else {
                quiet.appendBezierPathWithRect(bar);
            }
            x += BAR_WIDTH + BAR_GAP;
        }
        theme::secondary_text()
            .colorWithAlphaComponent(0.7)
            .setFill();
        quiet.fill();
        theme::plum().setFill();
        loud.fill();
    }
}

/// The readout and its timer.
#[derive(Debug)]
pub(super) struct Overlay {
    pub(super) view: Retained<FrameOverlayView>,
    figures: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    shown: RefCell<(String, String)>,
    refreshes: Cell<u32>,
}

impl Overlay {
    /// Builds the readout for a conversation pane `width` wide, hidden.
    pub(super) fn new(width: f64, mtm: MainThreadMarker) -> Self {
        let view = FrameOverlayView::new(
            rect(
                width - 24.0 - WIDTH,
                layout::COMPOSER_HEIGHT + 40.0 + 12.0,
                WIDTH,
                HEIGHT,
            ),
            mtm,
        );
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        view.setHidden(true);
        let figures = text_label("", 11.0, true, mtm);
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        figures.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            11.0,
            unsafe { NSFontWeightSemibold },
        )));
        figures.setFrame(rect(
            INSET,
            HEIGHT - INSET - 15.0,
            WIDTH - 2.0 * INSET,
            15.0,
        ));
        figures.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        view.addSubview(&figures);
        let detail = text_label("", 10.0, false, mtm);
        detail.setTextColor(Some(&theme::secondary_text()));
        detail.setFrame(rect(
            INSET,
            HEIGHT - INSET - 30.0,
            WIDTH - 2.0 * INSET,
            14.0,
        ));
        detail.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        view.addSubview(&detail);
        Self {
            view,
            figures,
            detail,
            timer: RefCell::new(None),
            shown: RefCell::new((String::new(), String::new())),
            refreshes: Cell::new(0),
        }
    }

    /// Shows the readout and keeps it current, or hides it and stops. The
    /// transcript gives up a band above the composer while the readout is
    /// shown: a readout drawn over the text would have the text redrawn
    /// under it ten times a second, and a layer of its own would make the
    /// whole window layer-backed, which cost every pass more than it saved.
    pub(super) fn set_shown(&self, shown: bool, delegate: &AppDelegate) {
        if self.view.isHidden() != shown {
            return;
        }
        self.view.setHidden(!shown);
        delegate.refit_composer_chips();
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.invalidate();
        }
        if !shown {
            return;
        }
        self.refreshes.set(0);
        self.refresh();
        // SAFETY: AppDelegate implements frameOverlayTick: with the standard
        // timer action signature and outlives the timer, which is
        // invalidated when the readout is hidden.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                REFRESH_SECONDS,
                delegate,
                sel!(frameOverlayTick:),
                None,
                true,
            )
        };
        // SAFETY: the timer and mode are valid Foundation objects; common
        // modes keep the readout moving while a menu or a scroll is live.
        unsafe {
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
        self.timer.borrow_mut().replace(timer);
    }

    /// The height the transcript gives up for the readout while it is shown.
    pub(super) fn band(&self) -> f64 {
        if self.view.isHidden() {
            0.0
        } else {
            HEIGHT + 8.0
        }
    }

    /// Places the readout at the right edge of a pane `width` wide, just
    /// above a composer whose top is at `composer_top`.
    pub(super) fn place(&self, width: f64, composer_top: f64) {
        let frame = rect(width - 24.0 - WIDTH, composer_top + 12.0, WIDTH, HEIGHT);
        if self.view.frame() != frame {
            self.view.setFrame(frame);
        }
    }

    /// Brings the readout up to date with the budget as it stands.
    pub(super) fn refresh(&self) {
        let _span = perf::span("gui.frame_overlay.refresh");
        let snapshot = frame_budget::snapshot();
        let refreshes = self.refreshes.get();
        self.refreshes.set(refreshes.wrapping_add(1));
        if refreshes.is_multiple_of(FIGURES_EVERY) {
            let (figures, detail) = readout(&snapshot);
            let mut shown = self.shown.borrow_mut();
            if shown.0 != figures {
                self.figures.setStringValue(&NSString::from_str(&figures));
                shown.0 = figures;
            }
            if shown.1 != detail {
                self.detail.setStringValue(&NSString::from_str(&detail));
                shown.1 = detail;
            }
        }
        self.view.set_samples(&snapshot.recent);
    }
}

/// The two lines of the readout: the figures, and the share of passes in
/// budget with the last one over it.
fn readout(snapshot: &frame_budget::Snapshot) -> (String, String) {
    let [median, tail] = snapshot.percentiles_us([0.5, 0.99]);
    let figures = format!(
        "p50 {} · p99 {} · worst {} ms",
        median.map_or("—".to_owned(), ms),
        tail.map_or("—".to_owned(), ms),
        ms(u32::try_from(snapshot.worst_us).unwrap_or(u32::MAX)),
    );
    let mut detail = format!(
        "{:.1} % of {} passes ≤ {} ms",
        snapshot.within_budget() * 100.0,
        group_digits(snapshot.passes),
        BUDGET_US / 1_000,
    );
    if let Some(culprit) = &snapshot.last_over {
        detail.push_str(" · over: ");
        match culprit.span {
            Some((name, span_us)) => {
                detail.push_str(name);
                detail.push(' ');
                detail.push_str(&ms(span_us));
            }
            None => {
                detail.push_str("unattributed ");
                detail.push_str(&ms(culprit.pass_us));
            }
        }
    }
    (figures, detail)
}

/// Microseconds as milliseconds: one decimal, two below a millisecond so a
/// quiet thread's figures do not all read as zero.
fn ms(micros: u32) -> String {
    let millis = f64::from(micros) / 1_000.0;
    if millis < 1.0 {
        format!("{millis:.2}")
    } else {
        format!("{millis:.1}")
    }
}

/// A count with thin spaces between groups of three.
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('\u{202f}');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readout_reads_as_two_lines() {
        let snapshot = frame_budget::Snapshot {
            passes: 16_289,
            over: 53,
            worst_us: 93_010,
            recent: vec![1_200, 900, 3_400],
            histogram: [0; frame_budget::BUCKET_EDGES_US.len() + 1],
            last_over: Some(frame_budget::Culprit {
                pass_us: 8_400,
                span: Some(("gui.rebuild_sidebar", 8_030)),
            }),
        };
        let (figures, detail) = readout(&snapshot);
        assert_eq!(figures, "p50 1.2 · p99 3.4 · worst 93.0 ms");
        assert_eq!(ms(40), "0.04");
        assert_eq!(
            detail,
            "99.7 % of 16\u{202f}289 passes ≤ 4 ms · over: gui.rebuild_sidebar 8.0"
        );
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1\u{202f}000");
    }
}
