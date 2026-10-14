//! The header strip's own controls: the tokens-a-second sparkline beside the
//! usage chips, and the Conversation / Observatory switch drawn in the
//! crown's gold. Both draw only when what they show changes; every motion is
//! a Core Animation animation from `motion`.
use super::*;

use std::collections::VecDeque;

use objc2_foundation::NSCopying;

/// How many readings the sparkline keeps. At one reading every
/// `SPARK_EVERY` while a response streams, about half a minute of it.
const SPARK_CAPACITY: usize = 60;
const SPARK_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug, Default)]
pub(super) struct SparklineIvars {
    samples: RefCell<VecDeque<f64>>,
    last_sample: Cell<Option<std::time::Instant>>,
    last_settled: Cell<Option<f64>>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond calling the designated
    //   initializer, which `new` does.
    // - MainThreadOnly matches AppKit's threading model.
    // - Sparkline does not implement Drop.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = SparklineIvars]
    #[derive(Debug)]
    pub(super) struct Sparkline;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for Sparkline {}

    // SAFETY: drawRect: keeps AppKit's declared signature; the view draws
    // only inside its bounds and a plain NSView's own drawing is nothing.
    impl Sparkline {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl Sparkline {
    pub(super) fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SparklineIvars::default());
        // SAFETY: NSView's designated initializer takes a frame rectangle.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Takes a reading: while streaming (`settled` false) at most one every
    /// `SPARK_EVERY`; a receipt's exact figure once per figure. Returns
    /// whether the line changed, so the caller can refresh its figures.
    pub(super) fn record(&self, rate: f64, settled: bool) -> bool {
        let ivars = self.ivars();
        let now = std::time::Instant::now();
        if settled {
            if ivars.last_settled.get() == Some(rate) {
                return false;
            }
            ivars.last_settled.set(Some(rate));
        } else if ivars
            .last_sample
            .get()
            .is_some_and(|last| now.duration_since(last) < SPARK_EVERY)
        {
            return false;
        }
        ivars.last_sample.set(Some(now));
        let mut samples = ivars.samples.borrow_mut();
        if samples.len() == SPARK_CAPACITY {
            samples.pop_front();
        }
        samples.push_back(rate.max(0.0));
        drop(samples);
        self.setNeedsDisplay(true);
        true
    }

    /// Defers the next streaming reading by `SPARK_EVERY` from now.
    pub(super) fn hold(&self) {
        self.ivars().last_sample.set(Some(std::time::Instant::now()));
    }

    pub(super) fn clear(&self) {
        let ivars = self.ivars();
        ivars.samples.borrow_mut().clear();
        ivars.last_sample.set(None);
        ivars.last_settled.set(None);
        self.setNeedsDisplay(true);
    }

    /// The readings, oldest first.
    pub(super) fn samples(&self) -> Vec<f64> {
        self.ivars().samples.borrow().iter().copied().collect()
    }

    /// Shows these readings (a `/usage` card's snapshot).
    pub(super) fn set_samples(&self, samples: &[f64]) {
        let mut kept = self.ivars().samples.borrow_mut();
        kept.clear();
        kept.extend(samples.iter().rev().take(SPARK_CAPACITY).rev().map(|v| v.max(0.0)));
        drop(kept);
        self.setNeedsDisplay(true);
    }

    pub(super) fn latest(&self) -> Option<f64> {
        self.ivars().samples.borrow().back().copied()
    }

    pub(super) fn peak(&self) -> Option<f64> {
        self.ivars()
            .samples
            .borrow()
            .iter()
            .copied()
            .fold(None, |peak, v| Some(peak.map_or(v, |p: f64| p.max(v))))
    }

    fn draw(&self) {
        let bounds = self.bounds();
        let (width, height) = (bounds.size.width, bounds.size.height);
        let samples = self.ivars().samples.borrow();
        // The floor: a quiet rule the line rises from.
        let floor = NSBezierPath::bezierPath();
        floor.setLineWidth(1.0);
        floor.moveToPoint(NSPoint::new(2.0, 1.5));
        floor.lineToPoint(NSPoint::new(width - 2.0, 1.5));
        theme::hairline().setStroke();
        floor.stroke();
        if samples.is_empty() {
            return;
        }
        let peak = samples.iter().copied().fold(1.0_f64, f64::max);
        // Newest at the right edge; older readings walk left a fixed step,
        // so the line scrolls as it grows rather than stretching.
        let step = (width - 8.0) / (SPARK_CAPACITY - 1) as f64;
        let last = samples.len() - 1;
        let point = |i: usize, v: f64| {
            NSPoint::new(
                width - 4.0 - (last - i) as f64 * step,
                2.0 + (height - 7.0) * (v / peak),
            )
        };
        let line = NSBezierPath::bezierPath();
        line.setLineWidth(1.6);
        line.setLineCapStyle(NSLineCapStyle::Round);
        line.setLineJoinStyle(objc2_app_kit::NSLineJoinStyle::Round);
        for (i, &v) in samples.iter().enumerate() {
            if i == 0 {
                line.moveToPoint(point(i, v));
            } else {
                line.lineToPoint(point(i, v));
            }
        }
        if samples.len() > 1 {
            // A faint wash under the line, down to the floor.
            let fill = line.copy();
            fill.lineToPoint(NSPoint::new(point(last, 0.0).x, 1.5));
            fill.lineToPoint(NSPoint::new(point(0, 0.0).x, 1.5));
            fill.closePath();
            theme::accent_fill_strong().setFill();
            fill.fill();
            theme::accent().setStroke();
            line.stroke();
        }
        // The newest reading, as a bead on the end of the line.
        let tip = point(last, samples[last]);
        let bead = NSBezierPath::bezierPathWithOvalInRect(rect(tip.x - 2.5, tip.y - 2.5, 5.0, 5.0));
        theme::accent().setFill();
        bead.fill();
    }
}

/// The two surfaces of a session, as a capsule track with a pill of crown
/// gold behind the chosen one. The native segmented control stays on top,
/// transparent, so clicks, the action, ⌘3/⌘4, tooltips, and VoiceOver's
/// "tab, selected" all keep working exactly as before; the views under it
/// are what is seen.
#[derive(Debug, Default)]
pub(super) struct SurfaceSwitchIvars {
    track: OnceCell<Retained<NSBox>>,
    wash: OnceCell<Retained<NSBox>>,
    pill: OnceCell<Retained<NSBox>>,
    labels: OnceCell<[Retained<NSTextField>; 2]>,
    /// Which segment the pill sits behind.
    shown: Cell<isize>,
}

define_class!(
    // SAFETY:
    // - NSSegmentedControl is subclassed through NSView's designated
    //   initializer, which `install` calls.
    // - MainThreadOnly matches AppKit's threading model.
    // - SurfaceSwitch does not implement Drop.
    #[unsafe(super = NSSegmentedControl)]
    #[thread_kind = MainThreadOnly]
    #[ivars = SurfaceSwitchIvars]
    #[derive(Debug)]
    pub(super) struct SurfaceSwitch;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for SurfaceSwitch {}

    // SAFETY: mouseDown: keeps AppKit's signature and calls super, which
    // runs the control's own tracking and sends its action on release.
    impl SurfaceSwitch {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let pressed = if point.x < self.bounds().size.width / 2.0 { 0 } else { 1 };
            let before = self.selectedSegment();
            self.show_press(pressed);
            // SAFETY: the superclass implements mouseDown: with this signature.
            let _: () = unsafe { msg_send![super(self), mouseDown: event] };
            let after = self.selectedSegment();
            if after == pressed && after != before {
                self.fulfil(after);
            } else {
                self.abandon();
            }
        }
    }
);

impl SurfaceSwitch {
    const HEIGHT: f64 = 30.0;
    const INSET: f64 = 3.0;

    /// Builds the switch inside a container added to `parent`; place it with
    /// `place`.
    pub(super) fn install(
        parent: &NSView,
        titles: [&str; 2],
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let container = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 260.0, Self::HEIGHT));
        container.setWantsLayer(true);
        // Everything stays inside the capsule: the pill's spring never
        // draws past the track.
        if let Some(layer) = container.layer() {
            layer.setCornerRadius(Self::HEIGHT / 2.0);
            layer.setMasksToBounds(true);
        }
        let track = fill_box(rect(0.0, 0.0, 260.0, Self::HEIGHT), &theme::surface_inset(), Self::HEIGHT / 2.0, mtm);
        track.setBorderWidth(1.0);
        track.setBorderColor(&theme::hairline());
        container.addSubview(&track);
        let inner = Self::HEIGHT - 2.0 * Self::INSET;
        let wash = fill_box(rect(0.0, 0.0, 10.0, inner), &theme::accent_fill_strong(), inner / 2.0, mtm);
        wash.setWantsLayer(true);
        wash.setAlphaValue(0.0);
        container.addSubview(&wash);
        let pill = fill_box(rect(0.0, 0.0, 10.0, inner), &theme::crown_gold(), inner / 2.0, mtm);
        pill.setWantsLayer(true);
        container.addSubview(&pill);
        let labels = titles.map(|title| {
            let label = text_label(title, 12.0, true, mtm);
            label.setAlignment(NSTextAlignment::Center);
            label.setTextColor(Some(&theme::secondary_text()));
            container.addSubview(&label);
            label
        });
        let this = Self::alloc(mtm).set_ivars(SurfaceSwitchIvars::default());
        // SAFETY: NSSegmentedControl inherits NSView's designated initializer.
        let this: Retained<Self> =
            unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, 260.0, Self::HEIGHT)] };
        // Seen through: the views beneath draw it; this keeps the behaviour.
        this.setAlphaValue(0.0);
        container.addSubview(&this);
        parent.addSubview(&container);
        let ivars = this.ivars();
        let _ = ivars.track.set(track);
        let _ = ivars.wash.set(wash);
        let _ = ivars.pill.set(pill);
        let _ = ivars.labels.set(labels);
        ivars.shown.set(-1);
        this
    }

    fn segment_frame(&self, index: isize) -> NSRect {
        let width = self.bounds().size.width;
        let inner = Self::HEIGHT - 2.0 * Self::INSET;
        let half = (width - 2.0 * Self::INSET) / 2.0;
        rect(Self::INSET + half * index.clamp(0, 1) as f64, Self::INSET, half, inner)
    }

    /// Sets the container's frame and lays the parts out in it, the pill at
    /// the chosen segment without motion.
    pub(super) fn place(&self, frame: NSRect) {
        // SAFETY: the switch lives in its container for the process lifetime.
        let Some(container) = (unsafe { self.superview() }) else { return };
        let frame = rect(frame.origin.x, frame.origin.y, frame.size.width, Self::HEIGHT);
        container.setFrame(frame);
        let bounds = rect(0.0, 0.0, frame.size.width, Self::HEIGHT);
        self.setFrame(bounds);
        let ivars = self.ivars();
        if let Some(track) = ivars.track.get() {
            track.setFrame(bounds);
        }
        if let Some(labels) = ivars.labels.get() {
            for (i, label) in labels.iter().enumerate() {
                let segment = self.segment_frame(i as isize);
                label.setFrame(rect(segment.origin.x, segment.origin.y + (segment.size.height - 16.0) / 2.0, segment.size.width, 16.0));
            }
        }
        // The pill stays behind the segment it shows; `select` moves it.
        let shown = match ivars.shown.get() {
            -1 => self.selectedSegment(),
            shown => shown,
        };
        if let Some(pill) = ivars.pill.get() {
            pill.setFrame(self.segment_frame(shown));
        }
        self.paint_labels(shown);
        ivars.shown.set(shown);
    }

    /// Chooses `index` as the program's choice (⌘3/⌘4, a restored desk): the
    /// pill springs across if it moves.
    pub(super) fn select(&self, index: isize) {
        if self.selectedSegment() != index {
            self.setSelectedSegment(index);
        }
        if self.ivars().shown.get() != index {
            self.move_pill(index);
        }
    }

    fn paint_labels(&self, selected: isize) {
        self.paint_labels_over(selected, 0.0);
    }

    /// Colors the labels for `selected`, cross-fading over `duration` (the
    /// pill's slide) rather than at once, so neither label is dark on the
    /// track before the pill reaches it.
    fn paint_labels_over(&self, selected: isize, duration: f64) {
        if let Some(labels) = self.ivars().labels.get() {
            for (i, label) in labels.iter().enumerate() {
                if duration > 0.0 {
                    motion::crossfade(label, duration);
                }
                label.setTextColor(Some(&*if i as isize == selected {
                    theme::on_gold()
                } else {
                    theme::secondary_text()
                }));
            }
        }
    }

    fn move_pill(&self, index: isize) {
        let ivars = self.ivars();
        let Some(pill) = ivars.pill.get() else { return };
        let from = pill.frame();
        let to = self.segment_frame(index);
        pill.setFrame(to);
        motion::slide_x(pill, from.origin.x - to.origin.x);
        self.paint_labels_over(index, motion::BLOOM);
        ivars.shown.set(index);
    }

    /// The press: the segment tints with a share of the gold. Unlike a row,
    /// the switch plants no seed: the pill is already the gold that moves,
    /// and a second shape sweeping beside it reads as a glitch.
    fn show_press(&self, index: isize) {
        let ivars = self.ivars();
        let Some(wash) = ivars.wash.get() else { return };
        if index == ivars.shown.get() {
            return;
        }
        wash.setFrame(self.segment_frame(index));
        motion::animate(motion::PRESS_IN, || {
            wash.animator().setAlphaValue(motion::PRESS_TINT);
        });
    }

    /// Fulfilment: the pill springs over the tint, which dissolves into it.
    fn fulfil(&self, index: isize) {
        self.move_pill(index);
        let Some(wash) = self.ivars().wash.get() else { return };
        motion::animate(motion::BLOOM, || {
            wash.animator().setAlphaValue(0.0);
        });
    }

    fn abandon(&self) {
        let Some(wash) = self.ivars().wash.get() else { return };
        motion::animate(motion::FADE_OUT, || {
            wash.animator().setAlphaValue(0.0);
        });
    }
}
