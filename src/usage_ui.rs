//! Native usage dashboard. Journal reads and aggregation run on the job pool.
use super::*;

use objc2_app_kit::NSImageSymbolConfiguration;

impl UsageState {
    pub(super) fn owner_is(&self, key: live_sessions::SessionKey) -> bool {
        self.owner.get() == Some(key)
    }
    pub(super) fn is_loading(&self) -> bool {
        self.loading.get()
    }
    /// The last read's `/usage` figures.
    pub(super) fn card(&self) -> Option<usage::Card> {
        self.report.borrow().as_ref().map(|report| report.card.clone())
    }
}

#[derive(Debug, Default)]
pub(super) struct UsageState {
    generation: Cell<u64>,
    loading: Cell<bool>,
    owner: Cell<Option<live_sessions::SessionKey>>,
    path: RefCell<Option<std::path::PathBuf>>,
    model: RefCell<String>,
    revision: Cell<u64>,
    last_read: Cell<Option<std::time::Instant>>,
    report: RefCell<Option<usage::Report>>,
    badge_last_tick: Cell<Option<std::time::Instant>>,
}

/// One stat in the header strip: a glyph in a round badge, the figure in
/// tabular numerals, and its name under it, quieter.
#[derive(Debug)]
struct StatChip {
    chip: Retained<NSBox>,
    badge: Retained<NSBox>,
    glyph: Retained<NSImageView>,
    value: Retained<NSTextField>,
    label: Retained<NSTextField>,
}

/// The header strip under the title: the session's usage as stat chips, the
/// first in gold, then a chip of recent tokens a second as a sparkline; and
/// the model pill that sits on the status line above it.
#[derive(Debug)]
pub(super) struct UsageHeader {
    view: Retained<NSView>,
    stats: [StatChip; 4],
    spinner: Retained<NSProgressIndicator>,
    spark_chip: Retained<NSBox>,
    spark: Retained<header_ui::Sparkline>,
    spark_value: Retained<NSTextField>,
    spark_label: Retained<NSTextField>,
    pill: Retained<NSBox>,
    pill_glyph: Retained<NSImageView>,
    pill_text: Retained<NSTextField>,
}

/// The stat the strip leads with, drawn in gold.
const LEAD: usize = 0;
const STAT_GLYPHS: [&str; 4] = [
    "arrow.down",
    "arrow.up",
    "bolt.fill",
    "arrow.triangle.2.circlepath",
];
const STRIP_HEIGHT: f64 = 58.0;
const CHIP_HEIGHT: f64 = 52.0;
const SPARK_WIDTH: f64 = 176.0;

fn figure_font(size: f64) -> Retained<NSFont> {
    // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
    NSFont::monospacedDigitSystemFontOfSize_weight(size, unsafe { NSFontWeightSemibold })
}

fn glyph_view(name: &str, size: f64, tint: &NSColor, mtm: MainThreadMarker) -> Retained<NSImageView> {
    let view = NSImageView::initWithFrame(NSImageView::alloc(mtm), rect(0.0, 0.0, 16.0, 16.0));
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
        .and_then(|image| {
            image.imageWithSymbolConfiguration(&NSImageSymbolConfiguration::configurationWithPointSize_weight(
                size,
                // SAFETY: AppKit exports NSFontWeightBold for the process lifetime.
                unsafe { NSFontWeightBold },
            ))
        });
    if let Some(image) = image {
        image.setTemplate(true);
        view.setImage(Some(&image));
    }
    view.setContentTintColor(Some(tint));
    view.setImageScaling(NSImageScaling::ScaleProportionallyDown);
    // Ornament: the figure beside it carries the spoken label.
    view.setAccessibilityElement(false);
    view
}

impl UsageHeader {
    pub(super) fn new(parent: &NSView, mtm: MainThreadMarker) -> Self {
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 100.0, STRIP_HEIGHT));
        view.setToolTip(Some(ns_string!(
            "Recorded session + worker totals. Updates as requests finish. Open Usage for details."
        )));
        let titles = ["Input tokens", "Output tokens", "Cache reads", "Requests"];
        let stats = std::array::from_fn(|i| {
            let lead = i == LEAD;
            let chip = fill_box(
                rect(0.0, 0.0, 100.0, CHIP_HEIGHT),
                &*if lead { theme::accent_fill_strong() } else { theme::chip_fill() },
                12.0,
                mtm,
            );
            chip.setBorderWidth(1.0);
            chip.setBorderColor(&*if lead { theme::accent_rule() } else { theme::hairline() });
            let badge = fill_box(
                rect(10.0, (CHIP_HEIGHT - 28.0) / 2.0, 28.0, 28.0),
                &*if lead { theme::crown_gold() } else { theme::plum_fill() },
                14.0,
                mtm,
            );
            chip.addSubview(&badge);
            let glyph = glyph_view(
                STAT_GLYPHS[i],
                12.0,
                &*if lead { theme::on_gold() } else { theme::plum() },
                mtm,
            );
            glyph.setFrame(rect(16.0, (CHIP_HEIGHT - 16.0) / 2.0, 16.0, 16.0));
            chip.addSubview(&glyph);
            let value = text_label("—", 19.0, true, mtm);
            value.setFont(Some(&figure_font(19.0)));
            value.setTextColor(Some(&*if lead { theme::accent() } else { NSColor::labelColor() }));
            value.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            // Layer-backed so a changed figure can roll in on the layer,
            // clipped so the roll stays inside the figure's own line.
            value.setWantsLayer(true);
            if let Some(layer) = value.layer() {
                layer.setMasksToBounds(true);
            }
            chip.addSubview(&value);
            let label = text_label(titles[i], 10.5, false, mtm);
            label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
            label.setTextColor(Some(&theme::secondary_text()));
            label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            chip.addSubview(&label);
            view.addSubview(&chip);
            StatChip { chip, badge, glyph, value, label }
        });
        let spinner = spinner(rect(0.0, 0.0, 12.0, 12.0), mtm);
        spinner.setControlSize(NSControlSize::Mini);
        let spark_chip = fill_box(rect(0.0, 0.0, SPARK_WIDTH, CHIP_HEIGHT), &theme::chip_fill(), 12.0, mtm);
        spark_chip.setBorderWidth(1.0);
        spark_chip.setBorderColor(&theme::hairline());
        spark_chip.setToolTip(Some(ns_string!(
            "Output tokens a second over the last half minute of streaming, with each response's exact figure once its receipt arrives."
        )));
        let spark = header_ui::Sparkline::new(rect(10.0, 22.0, SPARK_WIDTH - 20.0, 24.0), mtm);
        spark.setAccessibilityElement(false);
        spark_chip.addSubview(&spark);
        let spark_value = text_label("—", 11.0, true, mtm);
        spark_value.setFont(Some(&figure_font(11.0)));
        spark_value.setTextColor(Some(&theme::accent()));
        spark_value.setAlignment(NSTextAlignment::Right);
        spark_value.setWantsLayer(true);
        if let Some(layer) = spark_value.layer() {
            layer.setMasksToBounds(true);
        }
        spark_chip.addSubview(&spark_value);
        let spark_label = text_label("tok/s", 10.5, false, mtm);
        spark_label.setFont(Some(&NSFont::systemFontOfSize(10.5)));
        spark_label.setTextColor(Some(&theme::secondary_text()));
        spark_chip.addSubview(&spark_label);
        view.addSubview(&spark_chip);
        view.addSubview(&spinner);
        parent.addSubview(&view);
        // The model pill rides the status line; `place_pill` puts it there.
        let pill = fill_box(rect(0.0, 0.0, 10.0, 19.0), &theme::plum_fill(), 9.5, mtm);
        pill.setHidden(true);
        let pill_glyph = glyph_view("cpu", 10.0, &theme::plum(), mtm);
        pill_glyph.setFrame(rect(8.0, 3.0, 14.0, 14.0));
        pill.addSubview(&pill_glyph);
        let pill_text = text_label("", 11.0, true, mtm);
        // SAFETY: AppKit exports NSFontWeightSemibold for the process lifetime.
        pill_text.setFont(Some(&NSFont::systemFontOfSize_weight(11.0, unsafe { NSFontWeightSemibold })));
        pill_text.setTextColor(Some(&theme::plum()));
        pill_text.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        pill.addSubview(&pill_text);
        parent.addSubview(&pill);
        let this = Self {
            view,
            stats,
            spinner,
            spark_chip,
            spark,
            spark_value,
            spark_label,
            pill,
            pill_glyph,
            pill_text,
        };
        this.update(None);
        this
    }
    /// The sparkline's readings, oldest first, for a `/usage` card.
    pub(super) fn rates(&self) -> Vec<f64> {
        self.spark.samples()
    }

    pub(super) fn layout(&self, size: NSSize) {
        // The strip's figures moved to the `/usage` card; it keeps
        // collecting (the sparkline's readings, the read's spinner) unseen.
        self.view.setHidden(true);
        let width = (size.width - 48.0).max(280.0);
        self.view
            .setFrame(rect(24.0, size.height - 180.0, width, STRIP_HEIGHT));
        let gap = 8.0;
        // The sparkline gives way first on a narrow pane.
        let spark = width >= 660.0;
        self.spark_chip.setHidden(!spark);
        let stats_width = if spark { width - SPARK_WIDTH - gap } else { width };
        let column = ((stats_width - 3.0 * gap) / 4.0).floor();
        let y = (STRIP_HEIGHT - CHIP_HEIGHT) / 2.0;
        // A narrow chip keeps its figure and drops the badge.
        let badged = column >= 118.0;
        for (i, stat) in self.stats.iter().enumerate() {
            let x = i as f64 * (column + gap);
            stat.chip.setFrame(rect(x, y, column, CHIP_HEIGHT));
            stat.badge.setHidden(!badged);
            stat.glyph.setHidden(!badged);
            let text_x = if badged { 46.0 } else { 12.0 };
            stat.value.setFrame(rect(text_x, 21.0, column - text_x - 8.0, 24.0));
            stat.label.setFrame(rect(text_x, 7.0, column - text_x - 8.0, 14.0));
        }
        let last = 3.0 * (column + gap);
        self.spinner.setFrame(rect(last + column - 18.0, y + CHIP_HEIGHT - 18.0, 12.0, 12.0));
        if spark {
            self.spark_chip
                .setFrame(rect(width - SPARK_WIDTH, y, SPARK_WIDTH, CHIP_HEIGHT));
            self.spark.setFrame(rect(10.0, 22.0, SPARK_WIDTH - 20.0, 24.0));
            self.spark_label.setFrame(rect(10.0, 6.0, 90.0, 14.0));
            self.spark_value.setFrame(rect(SPARK_WIDTH - 100.0, 6.0, 90.0, 14.0));
        }
    }
    pub(super) fn update(&self, report: Option<&usage::Report>) {
        for (i, stat) in self.stats.iter().enumerate() {
            let metric = report.map(|r| &r.metrics[i]);
            let figure = metric.map_or("—", |m| &m.compact);
            if stat.value.stringValue().to_string() != figure {
                // A figure that changes rolls in; the first one just appears.
                if report.is_some() && stat.value.stringValue().to_string() != "—" {
                    motion::tick(&stat.value);
                }
                stat.value.setStringValue(&NSString::from_str(figure));
            }
            let tip = NSString::from_str(
                metric.map_or("Reading recorded session usage…", |m| &m.tooltip),
            );
            stat.value.setToolTip(Some(&tip));
            stat.label.setToolTip(Some(&tip));
            stat.chip.setToolTip(Some(&tip));
            stat.value.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                "{}: {}",
                stat.label.stringValue(),
                metric.map_or("Loading", |m| &m.value)
            ))));
        }
    }

    /// Takes a tokens-a-second reading from the speedometer into the
    /// sparkline: `settled` for a receipt's exact figure.
    pub(super) fn record_rate(&self, rate: f64, settled: bool) {
        if !self.spark.record(rate, settled) {
            return;
        }
        let latest = self.spark.latest().map_or(0, |v| v.round() as u64);
        let figure = format!("{latest}");
        if settled && self.spark_value.stringValue().to_string() != figure {
            motion::tick(&self.spark_value);
        }
        self.spark_value.setStringValue(&NSString::from_str(&figure));
        let peak = self.spark.peak().map_or(0, |v| v.round() as u64);
        self.spark_label
            .setStringValue(&NSString::from_str(&format!("tok/s · peak {peak}")));
        self.spark_chip.setAccessibilityLabel(Some(&NSString::from_str(&format!(
            "Output tokens a second: latest {latest}, peak {peak}"
        ))));
    }

    /// A response is waiting for its first token: the next streaming
    /// reading is taken a beat after it arrives.
    pub(super) fn hold_rates(&self) {
        self.spark.hold();
    }

    /// A new session starts its own line.
    pub(super) fn clear_rates(&self) {
        self.spark.clear();
        self.spark_value.setStringValue(ns_string!("—"));
        self.spark_label.setStringValue(ns_string!("tok/s"));
    }

    /// Names the model and account on the pill; empty hides it. Returns
    /// whether anything changed, so the header is refit only then.
    pub(super) fn set_identity(&self, text: &str, tooltip: &str) -> bool {
        let hidden = text.is_empty();
        if self.pill_text.stringValue().to_string() == text && self.pill.isHidden() == hidden {
            return false;
        }
        self.pill_text.setStringValue(&NSString::from_str(text));
        self.pill.setToolTip(Some(&NSString::from_str(tooltip)));
        self.pill.setAccessibilityLabel(Some(&NSString::from_str(tooltip)));
        self.pill.setHidden(hidden);
        true
    }

    /// Puts the pill at `x`, centred on `centre_y`, at most `max_width`
    /// wide; returns the room it took (none when hidden).
    pub(super) fn place_pill(&self, x: f64, centre_y: f64, max_width: f64) -> f64 {
        if self.pill.isHidden() {
            return 0.0;
        }
        self.pill_text.sizeToFit();
        let text = self.pill_text.frame().size.width.ceil();
        let width = (text + 34.0).min(max_width.max(60.0));
        self.pill
            .setFrame(rect(x, (centre_y - 9.5).round(), width, 19.0));
        self.pill_glyph.setFrame(rect(8.0, 2.5, 14.0, 14.0));
        self.pill_text.setFrame(rect(24.0, 1.5, width - 32.0, 16.0));
        width
    }
}

#[derive(Debug)]
pub(super) struct UsageUi {
    window: Retained<NSWindow>,
    status: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    document: Retained<NSView>,
    tabs: Retained<NSSegmentedControl>,
    spinner: Retained<NSProgressIndicator>,
    rendered_width: Cell<f64>,
    rendering: Cell<bool>,
    dirty: Cell<bool>,
    reset_scroll: Cell<bool>,
}

fn spinner(frame: NSRect, mtm: MainThreadMarker) -> Retained<NSProgressIndicator> {
    let spinner = NSProgressIndicator::initWithFrame(NSProgressIndicator::alloc(mtm), frame);
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setDisplayedWhenStopped(false);
    spinner
}
fn loading(spinner: &NSProgressIndicator, active: bool) {
    // SAFETY: AppKit animations are invoked on the main thread.
    unsafe {
        if active {
            spinner.startAnimation(None);
        } else {
            spinner.stopAnimation(None);
        }
    }
}

impl AppDelegate {
    pub(super) fn show_usage(&self) {
        let mtm = self.mtm();
        let ui = self.ivars().usage_ui.get_or_init(|| {
            // SAFETY: retained by the delegate for its lifetime, including after close.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    rect(0.0, 0.0, 900.0, 750.0),
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Resizable,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(ns_string!("Token usage & cache"));
            window.setMinSize(NSSize::new(680.0, 480.0));
            let content = window.contentView().expect("usage content");
            let title = text_label("Usage & cache", 24.0, true, mtm);
            title.setFrame(rect(24.0, 690.0, 660.0, 36.0));
            title.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            content.addSubview(&title);
            let status = text_label("Reading usage…", 12.0, false, mtm);
            status.setTextColor(Some(&theme::secondary_text()));
            status.setFrame(rect(24.0, 660.0, 816.0, 22.0));
            status.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
            status.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            content.addSubview(&status);
            let refresh = navigation_button(
                "Refresh",
                "arrow.clockwise",
                rect(784.0, 696.0, 92.0, 28.0),
                self,
                sel!(refreshUsage:),
                mtm,
            );
            refresh.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinXMargin
                    | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            content.addSubview(&refresh);
            let spinner = spinner(rect(856.0, 664.0, 16.0, 16.0), mtm);
            spinner.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinXMargin
                    | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            content.addSubview(&spinner);
            let tabs = NSSegmentedControl::initWithFrame(
                NSSegmentedControl::alloc(mtm),
                rect(24.0, 618.0, 370.0, 28.0),
            );
            tabs.setSegmentCount(3);
            for (i, title) in ["Overview", "Requests", "Cache policy"].iter().enumerate() {
                tabs.setLabel_forSegment(&NSString::from_str(title), i as isize);
            }
            tabs.setSegmentStyle(NSSegmentStyle::Separated);
            tabs.setSegmentDistribution(NSSegmentDistribution::FillEqually);
            tabs.setSelectedSegment(0);
            tabs.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
            // SAFETY: the delegate implements the selector and outlives this window.
            unsafe {
                tabs.setTarget(Some::<&AnyObject>(self));
                tabs.setAction(Some(sel!(selectUsagePage:)));
            }
            content.addSubview(&tabs);
            let scroll = NSScrollView::initWithFrame(
                NSScrollView::alloc(mtm),
                rect(8.0, 12.0, 884.0, 592.0),
            );
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDrawsBackground(false);
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            let document = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 884.0, 592.0));
            scroll.setDocumentView(Some(&document));
            let clip = scroll.contentView();
            clip.setPostsFrameChangedNotifications(true);
            // SAFETY: the delegate implements this selector and outlives the clip.
            unsafe {
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    self,
                    sel!(usageFrameDidChange:),
                    Some(NSViewFrameDidChangeNotification),
                    Some(&clip),
                );
            }
            content.addSubview(&scroll);
            window.center();
            UsageUi {
                window,
                status,
                scroll,
                document,
                tabs,
                spinner,
                rendered_width: Cell::new(0.0),
                rendering: Cell::new(false),
                dirty: Cell::new(true),
                reset_scroll: Cell::new(true),
            }
        });
        ui.window.makeKeyAndOrderFront(None);
        ui.dirty.set(true);
        self.request_usage_read(true);
        self.layout_usage_detail();
    }

    pub(super) fn select_usage_page(&self) {
        if let Some(ui) = self.ivars().usage_ui.get() {
            ui.dirty.set(true);
            ui.reset_scroll.set(true);
        }
        self.layout_usage_detail();
    }

    pub(super) fn request_usage_read(&self, force: bool) {
        if self.ivars().usage_header.get().is_none() {
            return;
        }
        self.layout_usage_detail();
        let state = &self.ivars().usage_state;
        let owner = self.session();
        let path = owner.selected_session_path.borrow().clone().or_else(|| {
            owner
                .header_session
                .borrow()
                .as_ref()
                .map(|(_, p)| p.clone())
        });
        let model = owner.current_model.borrow().clone();
        let changed = state.owner.get() != Some(owner.key)
            || *state.path.borrow() != path
            || *state.model.borrow() != model;
        if !changed
            && (state.loading.get()
                || (!force
                    && (state.revision.get() == owner.usage_revision.get()
                        || state
                            .last_read
                            .get()
                            .is_some_and(|t| t.elapsed().as_secs() < 5))))
        {
            return;
        }
        let generation = state.generation.get().wrapping_add(1);
        state.generation.set(generation);
        state.owner.set(Some(owner.key));
        state.path.replace(path.clone());
        state.model.replace(model.clone());
        state.revision.set(owner.usage_revision.get());
        state.last_read.set(Some(std::time::Instant::now()));
        state.loading.set(true);
        if changed {
            state.report.replace(None);
        }
        if let Some(header) = self.ivars().usage_header.get() {
            if changed {
                header.update(None);
            }
            loading(&header.spinner, true);
        }
        if let Some(ui) = self.ivars().usage_ui.get() {
            ui.status
                .setStringValue(ns_string!("Reading recorded requests…"));
            loading(&ui.spinner, true);
            if changed {
                ui.dirty.set(true);
                ui.reset_scroll.set(true);
            }
        }
        let children = owner
            .subagents
            .borrow()
            .iter()
            .filter_map(|s| s.session_file.clone())
            .collect();
        let key = owner.key;
        if !self.jobs().submit(move || BackgroundEvent::UsageRead {
            key,
            generation,
            report: usage::read(path, children, model),
        }) {
            state.loading.set(false);
            if let Some(header) = self.ivars().usage_header.get() {
                loading(&header.spinner, false);
            }
            if let Some(ui) = self.ivars().usage_ui.get() {
                loading(&ui.spinner, false);
                ui.status.setStringValue(ns_string!(
                    "Usage reader unavailable. Use Refresh to retry."
                ));
            }
        }
    }

    pub(super) fn finish_usage_read(
        &self,
        key: live_sessions::SessionKey,
        generation: u64,
        report: usage::Report,
    ) {
        let state = &self.ivars().usage_state;
        if state.generation.get() != generation
            || state.owner.get() != Some(key)
            || self.session().key != key
        {
            return;
        }
        state.loading.set(false);
        let owner = self.session();
        if *state.model.borrow() == *owner.current_model.borrow() {
            owner.cache_clock.borrow_mut().observe(report.cache_observation.clone(), wall_clock_ms());
        }
        if let Some(header) = self.ivars().usage_header.get() {
            header.update(Some(&report));
            loading(&header.spinner, false);
        }
        if let Some(ui) = self.ivars().usage_ui.get() {
            loading(&ui.spinner, false);
            ui.status.setStringValue(&NSString::from_str(&report.scope));
            let session = state
                .path
                .borrow()
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "New session".into());
            ui.status.setToolTip(Some(&NSString::from_str(&session)));
            ui.dirty.set(true);
        }
        self.fill_usage_cards(key, &report.card);
        state.report.replace(Some(report));
        self.layout_usage_detail();
    }

    /// Only native view layout runs here. Page data has already been aggregated
    /// off the main thread. Resizing or changing pages never rereads a journal.
    pub(super) fn layout_usage_detail(&self) {
        let Some(ui) = self.ivars().usage_ui.get() else {
            return;
        };
        if !ui.window.isVisible() {
            return;
        }
        let width = ui.scroll.contentSize().width;
        if !ui.dirty.get() && (ui.rendered_width.get() - width).abs() < 1.0 {
            return;
        }
        // A legacy scroller may change the clip width while we place the
        // document. Finish this pass before laying out that new width.
        if ui.rendering.replace(true) {
            ui.dirty.set(true);
            return;
        }
        ui.dirty.set(false);
        ui.rendered_width.set(width);
        let clip = ui.scroll.contentView();
        let old_top =
            (ui.document.frame().size.height - clip.bounds().origin.y - clip.bounds().size.height)
                .max(0.0);
        let children = ui.document.subviews();
        for i in 0..children.count() {
            children.objectAtIndex(i).removeFromSuperview();
        }
        let report = self.ivars().usage_state.report.borrow();
        let mut cards: Vec<Retained<NSBox>> = vec![];
        let card_width = (width - 32.0).max(300.0);
        let page = ui.tabs.selectedSegment().clamp(0, 2) as usize;
        if let Some(report) = report.as_ref() {
            if page == 0 {
                cards.push(metric_cards(&report.metrics, card_width, self.mtm()));
                cards.push(composition_card(report.composition, card_width, self.mtm()));
            }
            cards.extend(
                report.pages[page]
                    .iter()
                    .map(|s| section_card(s, card_width, self.mtm())),
            );
        } else {
            let label = usage::Section {
                title: "Reading usage…".into(),
                note: "Gathering recorded session and worker requests.".into(),
                columns: vec![],
                weights: vec![],
                rows: vec![],
                tooltips: vec![],
            };
            cards.push(section_card(&label, card_width, self.mtm()));
        }
        let height = cards
            .iter()
            .map(|c| c.frame().size.height + 12.0)
            .sum::<f64>()
            .max(clip.bounds().size.height);
        ui.document.setFrame(rect(0.0, 0.0, width, height));
        let mut top = 0.0;
        for card in cards {
            let h = card.frame().size.height;
            card.setFrame(rect(16.0, height - top - h, card_width, h));
            ui.document.addSubview(&card);
            top += h + 12.0;
        }
        let offset = if ui.reset_scroll.replace(false) {
            0.0
        } else {
            old_top
        };
        clip.scrollToPoint(NSPoint::new(
            0.0,
            (height - clip.bounds().size.height - offset).max(0.0),
        ));
        ui.scroll.reflectScrolledClipView(&clip);
        ui.rendering.set(false);
        if ui.dirty.get() {
            self.layout_usage_detail();
        }
    }

    /// Pure policy lookup; cache changes never block the requested action.
    pub(super) fn cache_warning_for_action(&self, action: cache_policy::CacheAction<'_>) -> Option<cache_policy::CacheWarning> {
        let owner = self.session();
        if self.mock_backend_active() || (owner.conversation.borrow().len() <= 1
            && owner.selected_session_path.borrow().is_none()) {
            return None;
        }
        cache_policy::CacheWarning::for_action(&owner.current_model.borrow(), action)
    }

    pub(super) fn note_cache_action(&self, action: cache_policy::CacheAction<'_>) {
        if matches!(action, cache_policy::CacheAction::AccountSwitch { from, to } if from != to) {
            self.session().cache_clock.borrow_mut().reset(wall_clock_ms());
        }
        if let Some(warning) = self.cache_warning_for_action(action) {
            self.session().cache_warning.replace(Some(warning));
            self.refit_composer_chips();
        }
    }

    fn current_cache_warning(&self) -> cache_policy::CacheWarning {
        let owner = self.session();
        let window = owner.cache_clock.borrow().warning(wall_clock_ms());
        let action = owner.cache_warning.borrow().clone();
        match action {
            Some(action) => action.with_window(window),
            None => window.unwrap_or_else(cache_policy::CacheWarning::waiting),
        }
    }

    /// Only a constant-size in-memory clock check; no journal reads or RPC.
    pub(super) fn tick_cache_warning(&self) {
        let now = std::time::Instant::now();
        let last = &self.ivars().usage_state.badge_last_tick;
        if last.get().is_some_and(|last| now.duration_since(last).as_secs() < 1) { return; }
        last.set(Some(now));
        let Some(badge) = self.ivars().cache_warning_badge.get() else { return; };
        let was_visible = !badge.isHidden();
        if self.sync_cache_warning_badge() != was_visible { self.refit_composer_chips(); }
    }

    /// Opt-in native fixture: no OMP child, account loading, or session discovery.
    pub(super) fn run_cache_warning_smoke(&self) {
        let owner = self.session();
        owner.current_model.replace("anthropic/claude-sonnet-4-6".into());
        owner.conversation.borrow_mut().notice("Cache timer preview. The countdown is always visible, turns amber in five seconds, and reaches zero one minute later. No inference is running.", false);
        self.refresh_chat_transcript();
        self.set_model_label("Claude Sonnet 4.6");
        self.set_chat_status("Cache warning preview · No inference running");
        if let Some(input) = self.ivars().input.get() { input.setEditable(true); }
        let now = wall_clock_ms();
        owner.cache_clock.borrow_mut().observe(Some(cache_policy::CacheObservation {
            completed_at_ms: now.saturating_sub(235_000),
            ttl_seconds: std::env::var("OMPEROR_CACHE_PREVIEW_TTL_SECONDS").ok().and_then(|s| s.parse().ok()),
            retention: None,
        }), now);
        self.refit_composer_chips();
        // SAFETY: the delegate implements this selector and outlives the timer.
        unsafe {
            let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                0.5, self, sel!(tickCacheWarning:), None, true,
            );
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
    }

    /// Returns whether the composer needs to reserve the badge's hit area.
    pub(super) fn sync_cache_warning_badge(&self) -> bool {
        let Some(badge) = self.ivars().cache_warning_badge.get() else { return false; };
        let owner = self.session();
        let warning = self.current_cache_warning();
        let visible = !owner.read_only_session.get()
            && !self.mock_backend_active() && !self.agent_view().desk.active;
        badge.setHidden(!visible);
        if visible {
            badge.setTitle(&NSString::from_str(&warning.badge_label));
            badge.setAccessibilityLabel(Some(&NSString::from_str(&format!("{} · {}", warning.headline, warning.badge_label))));
            let color = if warning.urgent { theme::accent() } else { theme::secondary_text() };
            badge.setContentTintColor(Some(&color));
            let tooltip = NSString::from_str(&warning.tooltip);
            if badge.toolTip().as_deref() != Some(&*tooltip) {
                badge.setToolTip(Some(&tooltip));
                badge.setAccessibilityHelp(Some(&tooltip));
            }
        }
        visible
    }

    pub(super) fn show_cache_warning(&self) {
        let warning = self.current_cache_warning();
        let alert = NSAlert::new(self.mtm());
        alert.setAlertStyle(NSAlertStyle::Informational);
        alert.setMessageText(&NSString::from_str(warning.headline));
        alert.setInformativeText(&NSString::from_str(&warning.explanation));
        alert.addButtonWithTitle(ns_string!("OK"));
        alert.runModal();
    }

}

/// Returns a wrapping native label with a measured height at the supplied width.
fn wrap_label(
    text: &str,
    width: f64,
    size: f64,
    bold: bool,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let label = text_label(text, size, bold, mtm);
    label.setMaximumNumberOfLines(0);
    label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
    if let Some(cell) = label.cell() {
        cell.setWraps(true);
        cell.setUsesSingleLineMode(false);
    }
    let height = label
        .cell()
        .map(|c| c.cellSizeForBounds(rect(0.0, 0.0, width, 10000.0)).height)
        .unwrap_or(size * 1.5)
        .ceil()
        .max(size * 1.5);
    label.setFrame(rect(0.0, 0.0, width, height));
    label
}
fn dashboard_card(width: f64, height: f64, mtm: MainThreadMarker) -> Retained<NSBox> {
    let card = card_box(rect(0.0, 0.0, width, height), 10.0, mtm);
    card.setContentViewMargins(NSSize::new(0.0, 0.0));
    card
}
fn metric_cards(
    metrics: &[usage::Metric; 4],
    width: f64,
    mtm: MainThreadMarker,
) -> Retained<NSBox> {
    let card = dashboard_card(width, 106.0, mtm);
    let column = (width - 32.0) / 4.0;
    for (i, metric) in metrics.iter().enumerate() {
        let x = 16.0 + i as f64 * column;
        let title = text_label(metric.title, 11.0, true, mtm);
        title.setTextColor(Some(&theme::secondary_text()));
        title.setFrame(rect(x, 78.0, column - 12.0, 16.0));
        let value = text_label(
            &metric.value,
            if column < 170.0 { 21.0 } else { 25.0 },
            true,
            mtm,
        );
        value.setFrame(rect(x, 42.0, column - 12.0, 32.0));
        value.setToolTip(Some(&NSString::from_str(&metric.tooltip)));
        value.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        let caption = wrap_label(&metric.caption, column - 12.0, 10.0, false, mtm);
        caption.setTextColor(Some(&theme::secondary_text()));
        caption.setFrameOrigin(NSPoint::new(x, 8.0));
        card.addSubview(&title);
        card.addSubview(&value);
        card.addSubview(&caption);
    }
    card
}
fn composition_card(amounts: [u64; 3], width: f64, mtm: MainThreadMarker) -> Retained<NSBox> {
    let card = dashboard_card(width, 66.0, mtm);
    let colors = [
        theme::accent(),
        NSColor::systemTealColor(),
        NSColor::systemBlueColor(),
    ];
    let total = amounts.iter().map(|n| *n as f64).sum::<f64>();
    let track = fill_box(
        rect(16.0, 38.0, width - 32.0, 10.0),
        &theme::surface_inset(),
        4.0,
        mtm,
    );
    card.addSubview(&track);
    let mut x = 16.0;
    for i in 0..3 {
        let share = if total > 0.0 {
            amounts[i] as f64 / total
        } else {
            0.0
        };
        let w = (width - 32.0) * share;
        if w > 0.0 {
            card.addSubview(&fill_box(rect(x, 38.0, w, 10.0), &colors[i], 0.0, mtm));
        }
        x += w;
        let legend_x = 16.0 + i as f64 * (width - 32.0) / 3.0;
        card.addSubview(&fill_box(
            rect(legend_x, 16.0, 8.0, 8.0),
            &colors[i],
            4.0,
            mtm,
        ));
        let label = text_label(
            ["Ordinary input", "Cache reads", "Cache writes"][i],
            11.0,
            false,
            mtm,
        );
        label.setFrame(rect(
            legend_x + 14.0,
            11.0,
            (width - 32.0) / 3.0 - 16.0,
            18.0,
        ));
        card.addSubview(&label);
    }
    card.setToolTip(Some(ns_string!(
        "Composition of recorded input tokens. Missing telemetry is not displayed as a cache miss."
    )));
    card
}
fn section_card(section: &usage::Section, width: f64, mtm: MainThreadMarker) -> Retained<NSBox> {
    // Collect measured labels in top-down coordinates, then place in AppKit's
    // bottom-up coordinate system after the card height is known.
    let mut labels: Vec<(Retained<NSTextField>, f64, f64)> = vec![];
    let mut lines = vec![];
    let mut top = 16.0;
    let title = wrap_label(&section.title, width - 32.0, 15.0, true, mtm);
    top += title.frame().size.height;
    labels.push((title, 16.0, 16.0));
    if !section.note.is_empty() {
        top += 8.0;
        let note = wrap_label(&section.note, width - 32.0, 12.0, false, mtm);
        note.setTextColor(Some(&theme::secondary_text()));
        let h = note.frame().size.height;
        labels.push((note, 16.0, top));
        top += h;
    }
    if !section.columns.is_empty() {
        top += 16.0;
        let sum = section.weights.iter().sum::<f64>().max(1.0);
        for (row_index, row) in std::iter::once(&section.columns)
            .chain(section.rows.iter())
            .enumerate()
        {
            let mut x = 16.0;
            let mut height: f64 = 18.0;
            for (i, text) in row.iter().enumerate() {
                let cell_width =
                    (width - 32.0) * section.weights.get(i).copied().unwrap_or(1.0) / sum;
                let label = wrap_label(
                    text,
                    (cell_width - 12.0).max(30.0),
                    11.0,
                    row_index == 0,
                    mtm,
                );
                if row_index == 0 {
                    label.setTextColor(Some(&theme::secondary_text()));
                }
                if section.columns.get(i).is_some_and(|column| {
                    matches!(
                        column.as_str(),
                        "Recorded tokens"
                            | "Requests"
                            | "Input"
                            | "Output"
                            | "Cache reads"
                            | "Read"
                            | "Write"
                    )
                }) {
                    label.setAlignment(NSTextAlignment::Right);
                }
                if let Some(tip) = row_index
                    .checked_sub(1)
                    .and_then(|i| section.tooltips.get(i))
                {
                    label.setToolTip(Some(&NSString::from_str(tip)));
                }
                height = height.max(label.frame().size.height);
                labels.push((label, x, top));
                x += cell_width;
            }
            top += height + 9.0;
            lines.push(top);
            top += 9.0;
        }
    }
    let height = top + 8.0;
    let card = dashboard_card(width, height, mtm);
    for (label, x, y) in labels {
        label.setFrameOrigin(NSPoint::new(x, height - y - label.frame().size.height));
        card.addSubview(&label);
    }
    for y in lines {
        card.addSubview(&fill_box(
            rect(16.0, height - y, width - 32.0, 1.0),
            &theme::hairline(),
            0.0,
            mtm,
        ));
    }
    card
}
