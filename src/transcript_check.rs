//! `omperor --transcript-check <session.oms> <out-dir>`: opens a copy of a
//! session's journal in the real transcript, off screen in the scratch home
//! the headless benchmark sets up (`markdown_bench::prepare`), and scrolls it
//! top to bottom and back, in fixed steps and in a fling. At every step it
//! checks what the reader would see:
//!
//! - every made row's view sits at its row's place and height, an activity
//!   run's text fills its row, no two row views overlap, and no row view is
//!   left in the document that no row owns;
//! - once the history is in, a `/usage` card is put below it and checked
//!   with the rest;
//! - the viewport as the layers hold it — what the window would composite —
//!   equals the viewport after every view in it drew all of itself again:
//!   pixels kept from an earlier draw that no longer match the view's state
//!   are stale.
//!
//! A failure writes the two captures and their difference as PNGs; the run
//! ends with `report.json` and exits 1 on any failure, 0 when clean.
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use block2::RcBlock;
use objc2::{
    AnyThread, msg_send,
    encode::{Encode, Encoding},
    rc::Retained,
};
use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSView};
use objc2_core_foundation::{CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer, kCFRunLoopCommonModes};
use objc2_foundation::NSSize;
use serde_json::{Value, json};

use crate::markdown_bench::Host;

/// Points a fixed step scrolls: about a third of the viewport.
const STEP: f64 = 240.0;
/// Points a fling's frame scrolls: some 100 000 pt/s at 60 Hz.
const FLING: f64 = 1_800.0;
/// Frames a fixed step waits for the rows in sight to be made.
const MAX_WAITS: usize = 30;
const FRAME: f64 = 1.0 / 60.0;
const SCALE: f64 = 2.0;
/// Failures whose captures are written, at most.
const MAX_IMAGES: usize = 16;
/// The history is loaded and nothing has changed for this long.
const SETTLE: Duration = Duration::from_millis(1_500);
const TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    Down,
    Up,
    FlingDown,
    FlingUp,
}

const PASSES: [Pass; 4] = [Pass::Down, Pass::Up, Pass::FlingDown, Pass::FlingUp];
/// Scrolling while the history loads starts where a reader does: at the
/// end, going up into what is still arriving.
const EARLY_PASSES: [Pass; 4] = [Pass::Up, Pass::Down, Pass::FlingUp, Pass::FlingDown];

/// `OMPEROR_CHECK_OPEN_ONLY=1`: the open alone, measured, then a pause for
/// the app to save the heights; no scrolling passes.
fn open_only() -> bool {
    std::env::var_os("OMPEROR_CHECK_OPEN_ONLY").is_some_and(|value| value == "1")
}

fn early() -> bool {
    std::env::var_os("OMPEROR_CHECK_EARLY").is_some_and(|value| value == "1")
}

fn passes() -> [Pass; 4] {
    if early() { EARLY_PASSES } else { PASSES }
}

/// Checks made while the session opens, at the end where it opens, before
/// any scrolling: after the passes in the statistics.
const OPEN: usize = PASSES.len();

fn pass_name(pass: usize) -> &'static str {
    if pass == OPEN { "open" } else { passes()[pass].name() }
}

impl Pass {
    fn name(self) -> &'static str {
        match self {
            Pass::Down => "down",
            Pass::Up => "up",
            Pass::FlingDown => "fling-down",
            Pass::FlingUp => "fling-up",
        }
    }

    fn stride(self) -> f64 {
        match self {
            Pass::Down | Pass::Up => STEP,
            Pass::FlingDown | Pass::FlingUp => FLING,
        }
    }

    fn downward(self) -> bool {
        matches!(self, Pass::Down | Pass::FlingDown)
    }

    /// A fling does not wait for rows to be made: it checks what it passes.
    fn waits(self) -> bool {
        matches!(self, Pass::Down | Pass::Up)
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    Starting,
    Loading { step: usize },
    Scroll { pass: usize, step: usize },
    Look { pass: usize, step: usize, waits: usize, end: bool },
}

#[derive(Default)]
struct PassStats {
    steps: usize,
    geometry: usize,
    stale: usize,
    stale_pixels: usize,
    /// Steps whose kept capture had no ink: the pixel check saw nothing.
    blank: usize,
}

struct Driver {
    host: Rc<dyn Host>,
    out: PathBuf,
    id: String,
    timer: CFRetained<CFRunLoopTimer>,
    phase: Phase,
    started: Instant,
    opened: Option<Instant>,
    /// The last time the history, the document's height or the rows changed.
    changed: Instant,
    last_extent: ((usize, usize), f64),
    loaded_ms: Option<u128>,
    /// Whether `/usage` has put its card below the history: the passes
    /// check the card's row as they do every other.
    card_sent: bool,
    /// The open: the first frame with rows in sight, the first with every
    /// row in sight made and nothing pending, and the scroll bar's thumb —
    /// its top and length on screen — frame to frame, with how often and
    /// how far it moved while nobody scrolled.
    first_rows_ms: Option<u128>,
    complete_ms: Option<u128>,
    thumb: Option<(f64, f64)>,
    thumb_jumps: usize,
    thumb_travel: f64,
    thumb_max: f64,
    /// Each move: when, and by how much the top and the length changed.
    thumb_moves: Vec<Value>,
    thumb_shown_ms: Option<u128>,
    stats: Vec<PassStats>,
    failures: Vec<Value>,
    images: usize,
    writers: Vec<std::thread::JoinHandle<()>>,
    /// The session had saved heights when the check opened it.
    saved_found: bool,
}

thread_local! {
    static DRIVER: RefCell<Option<Driver>> = const { RefCell::new(None) };
}

/// From `applicationDidFinishLaunching:` in a check run: starts driving.
pub(crate) fn start(host: Rc<dyn Host>, journal: PathBuf, out: PathBuf) {
    let id = journal.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let block = RcBlock::new(|_timer: *mut CFRunLoopTimer| {
        DRIVER.with(|driver| {
            if let Ok(mut driver) = driver.try_borrow_mut()
                && let Some(driver) = driver.as_mut()
            {
                driver.act();
            }
        });
    });
    // SAFETY: the handler is copied by Core Foundation and runs only on the
    // main run loop; the driver keeps the timer for the process.
    let timer = unsafe { CFRunLoopTimer::with_handler(None, CFAbsoluteTimeGetCurrent() + 0.1, 1.0e9, 0, 0, Some(&block)) }
        .expect("a run-loop timer");
    if let Some(run_loop) = CFRunLoop::main() {
        // SAFETY: the mode is a process-lifetime CF constant.
        run_loop.add_timer(Some(&timer), unsafe { kCFRunLoopCommonModes });
    }
    let _ = std::fs::create_dir_all(&out);
    let now = Instant::now();
    let driver = Driver {
        host,
        out,
        id,
        timer,
        phase: Phase::Starting,
        started: now,
        opened: None,
        changed: now,
        last_extent: ((0, 0), 0.0),
        loaded_ms: None,
        card_sent: false,
        first_rows_ms: None,
        complete_ms: None,
        thumb: None,
        thumb_jumps: 0,
        thumb_travel: 0.0,
        thumb_max: 0.0,
        thumb_moves: Vec::new(),
        thumb_shown_ms: None,
        stats: (0..=PASSES.len()).map(|_| PassStats::default()).collect(),
        failures: Vec::new(),
        images: 0,
        writers: Vec::new(),
        saved_found: false,
    };
    DRIVER.with(|slot| slot.replace(Some(driver)));
}

/// A `CGContextRef`, as `-[NSGraphicsContext CGContext]` returns it and
/// `-[CALayer renderInContext:]` takes it.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct CgContext(*mut std::ffi::c_void);

// SAFETY: a pointer to the opaque `struct CGContext`, as the headers type it.
unsafe impl Encode for CgContext {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGContext", &[]));
}

#[cfg_attr(target_os = "macos", link(name = "CoreGraphics", kind = "framework"))]
unsafe extern "C" {
    fn CGContextTranslateCTM(context: *mut std::ffi::c_void, tx: f64, ty: f64);
}

/// Premultiplied RGBA, top row first.
struct Capture {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    stride: usize,
}

/// `view`'s layer tree as it is — the pixels each layer keeps, not a new
/// draw — at `SCALE`.
fn layers(view: &NSView) -> Option<Capture> {
    let layer = view.layer()?;
    let size = view.bounds().size;
    let wide = (size.width * SCALE).ceil().max(1.0) as isize;
    let high = (size.height * SCALE).ceil().max(1.0) as isize;
    // SAFETY: null planes ask AppKit to allocate the pixel buffer; the color
    // space name is an AppKit constant.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            wide,
            high,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    rep.setSize(NSSize::new(size.width, size.height));
    let length = rep.bytesPerRow() as usize * rep.pixelsHigh() as usize;
    // SAFETY: the rep owns `length` bytes of pixel data.
    unsafe { std::ptr::write_bytes(rep.bitmapData(), 0, length) };
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    // SAFETY: -CGContext returns the context's own, valid while it lives.
    let cg: CgContext = unsafe { msg_send![&context, CGContext] };
    // The layer draws in its own space, whose origin is the clip view's
    // scroll point: moved back so the viewport lands in the bitmap.
    // SAFETY: -bounds is CALayer's geometry, a CGRect.
    let bounds: objc2_foundation::NSRect = unsafe { msg_send![&layer, bounds] };
    // SAFETY: the context is live; translating its CTM has no other effect.
    unsafe { CGContextTranslateCTM(cg.0, -bounds.origin.x, -bounds.origin.y) };
    // SAFETY: renderInContext: draws the layer tree into a live context.
    let _: () = unsafe { msg_send![&layer, renderInContext: cg] };
    context.flushGraphics();
    // SAFETY: as above; copied out before the rep is released.
    let pixels = unsafe { std::slice::from_raw_parts(rep.bitmapData(), length) }.to_vec();
    Some(Capture { pixels, width: rep.pixelsWide() as usize, height: rep.pixelsHigh() as usize, stride: rep.bytesPerRow() as usize })
}

/// Pixels that differ, and the rows (in points) the differences span.
fn compare(a: &Capture, b: &Capture) -> (usize, Option<(usize, usize)>) {
    if a.width != b.width || a.height != b.height {
        return (a.width * a.height, Some((0, a.height)));
    }
    let mut count = 0;
    let mut span: Option<(usize, usize)> = None;
    for y in 0..a.height {
        let (ra, rb) = (&a.pixels[y * a.stride..y * a.stride + a.width * 4], &b.pixels[y * b.stride..y * b.stride + b.width * 4]);
        if ra == rb {
            continue;
        }
        let differing = ra.chunks_exact(4).zip(rb.chunks_exact(4)).filter(|(p, q)| p != q).count();
        count += differing;
        span = Some(span.map_or((y, y), |(top, _)| (top, y)));
    }
    (count, span.map(|(top, bottom)| ((top as f64 / SCALE) as usize, (bottom as f64 / SCALE).ceil() as usize)))
}

/// Premultiplied RGBA over white, as RGB.
fn rgb(capture: &Capture) -> Vec<u8> {
    let mut out = Vec::with_capacity(capture.width * capture.height * 3);
    // The layer tree renders bottom up: the last row is the viewport's top.
    for row in capture.pixels.chunks(capture.stride).take(capture.height).rev() {
        for pixel in row[..capture.width * 4].chunks_exact(4) {
            let alpha = u32::from(pixel[3]);
            for channel in 0..3 {
                out.push((u32::from(pixel[channel]) + 255 * (255 - alpha) / 255).min(255) as u8);
            }
        }
    }
    out
}

impl Driver {
    fn arm(&self, after: f64) {
        self.timer.set_next_fire_date(CFAbsoluteTimeGetCurrent() + after);
    }

    fn clip(&self) -> Option<Retained<NSView>> {
        Some(Retained::into_super(self.host.transcript_scroll_view()?.contentView()))
    }

    fn document_height(&self) -> f64 {
        self.host
            .transcript_scroll_view()
            .and_then(|scroll| scroll.documentView())
            .map_or(0.0, |document| document.frame().size.height)
    }

    fn viewport(&self) -> (f64, f64) {
        self.clip().map_or((0.0, 0.0), |clip| (clip.bounds().origin.y, clip.bounds().size.height))
    }

    /// What AppKit does at the end of a pass: display what needs it, commit.
    fn display(&self) {
        if let Some(window) = self.host.window() {
            window.displayIfNeeded();
        }
        if let Some(transaction) = objc2::runtime::AnyClass::get(c"CATransaction") {
            // SAFETY: +[CATransaction flush] takes no arguments.
            let _: () = unsafe { msg_send![transaction, flush] };
        }
    }

    fn scroll_to(&self, y: f64) -> f64 {
        let Some(scroll) = self.host.transcript_scroll_view() else { return 0.0 };
        let clip = scroll.contentView();
        let bottom = (self.document_height() - clip.bounds().size.height).max(0.0);
        let target = y.clamp(0.0, bottom);
        clip.scrollToPoint(objc2_foundation::NSPoint::new(0.0, target));
        scroll.reflectScrolledClipView(&clip);
        target
    }

    fn act(&mut self) {
        if self.started.elapsed() > TIMEOUT {
            eprintln!("transcript-check: stopping: the run took longer than {} s ({:?})", TIMEOUT.as_secs(), self.phase);
            std::process::exit(3);
        }
        let host = Rc::clone(&self.host);
        match self.phase {
            Phase::Starting => {
                if host.ready() && self.started.elapsed() >= Duration::from_secs(1) {
                    // `OMPEROR_CHECK_LOOK=editorial:dark`: the chat style and appearance.
                    if let Ok(look) = std::env::var("OMPEROR_CHECK_LOOK") {
                        let (style, appearance) = look.split_once(':').unwrap_or((look.as_str(), "light"));
                        host.set_look(style, appearance == "dark");
                    }
                    // `OMPEROR_CHECK_WINDOW=1000x800`: the window's size, off screen as before.
                    if let Some((width, height)) = std::env::var("OMPEROR_CHECK_WINDOW")
                        .ok()
                        .and_then(|size| size.split_once('x').and_then(|(w, h)| Some((w.parse::<f64>().ok()?, h.parse::<f64>().ok()?))))
                        && let Some(window) = host.window()
                    {
                        let mut frame = window.frame();
                        frame.size = NSSize::new(width, height);
                        window.setFrame_display(frame, false);
                    }
                    self.saved_found = self
                        .host
                        .window()
                        .is_some()
                        .then(|| std::env::var_os("OMPEROR_PROJECT").map(PathBuf::from))
                        .flatten()
                        .is_some_and(|project| crate::saved_heights::path_for(&project.join(format!("{}.oms", self.id))).is_file());
                    host.open_journal(&self.id);
                    self.opened = Some(Instant::now());
                    self.changed = Instant::now();
                    self.phase = Phase::Loading { step: 0 };
                }
                self.arm(0.1);
            }
            Phase::Loading { step } => {
                // Every frame of the open is checked, the reader at the end.
                if host.content_extent().0 > 0 {
                    self.look(OPEN, step);
                    self.watch_open();
                }
                self.phase = Phase::Loading { step: step + 1 };
                let extent = (host.content_extent(), self.document_height());
                if extent != self.last_extent {
                    self.last_extent = extent;
                    self.changed = Instant::now();
                }
                if host.history_loading() || upleft_render::fragments::async_objects::pending_count() > 0 {
                    self.changed = Instant::now();
                } else if self.loaded_ms.is_none() {
                    self.loaded_ms = self.opened.map(|opened| opened.elapsed().as_millis());
                }
                // `OMPEROR_CHECK_EARLY=1`: scroll as soon as there is anything to
                // scroll, while the history still loads, as a reader would.
                let early = early() && extent.1 > 3.0 * self.viewport().1;
                if open_only() && self.changed.elapsed() >= SETTLE {
                    // Long enough for the app to save the heights.
                    if self.changed.elapsed() >= SETTLE + Duration::from_secs(3) {
                        self.finish();
                    }
                    self.arm(0.1);
                } else if !self.card_sent && self.changed.elapsed() >= SETTLE {
                    // The card is read on the job pool and fills in; the
                    // open settles again once it has.
                    self.card_sent = true;
                    host.send_prompt("/usage");
                    self.changed = Instant::now();
                    self.arm(FRAME);
                } else if early || self.changed.elapsed() >= SETTLE {
                    eprintln!(
                        "transcript-check: {} rows, document {:.0} pt, loaded in {} ms",
                        extent.0.0,
                        extent.1,
                        self.loaded_ms.unwrap_or(0)
                    );
                    self.phase = Phase::Scroll { pass: 0, step: 0 };
                    self.arm(0.0);
                } else {
                    self.arm(FRAME);
                }
            }
            Phase::Scroll { pass, step } => {
                let kind = passes()[pass];
                let (_, height) = self.viewport();
                let bottom = (self.document_height() - height).max(0.0);
                let wanted = if kind.downward() { step as f64 * kind.stride() } else { bottom - step as f64 * kind.stride() };
                let at = self.scroll_to(wanted);
                // Going up while older history still arrives above: not done.
                let end = if kind.downward() { at >= bottom } else { at <= 0.0 && !host.history_loading() };
                self.phase = Phase::Look { pass, step, waits: 0, end };
                self.arm(FRAME);
            }
            Phase::Look { pass, step, waits, end } => {
                let kind = passes()[pass];
                if kind.waits() && waits < MAX_WAITS && host.viewport_gaps().0 > 0 {
                    self.phase = Phase::Look { pass, step, waits: waits + 1, end };
                    self.arm(FRAME);
                    return;
                }
                self.look(pass, step);
                self.phase = if !end {
                    Phase::Scroll { pass, step: step + 1 }
                } else if pass + 1 < PASSES.len() {
                    Phase::Scroll { pass: pass + 1, step: 0 }
                } else {
                    self.finish();
                };
                self.arm(FRAME);
            }
        }
    }

    /// A frame of the open: when it first shows rows, when it is first
    /// complete, and how the thumb moves.
    fn watch_open(&mut self) {
        let Some(opened) = self.opened else { return };
        let elapsed = opened.elapsed().as_millis();
        // Past the one row that says the history is loading.
        let history = self.host.content_extent().0 > 1;
        if history {
            self.first_rows_ms.get_or_insert(elapsed);
        }
        if self.complete_ms.is_none()
            && history
            && self.host.viewport_gaps() == (0, 0)
            && upleft_render::fragments::async_objects::pending_count() == 0
        {
            self.complete_ms = Some(elapsed);
        }
        let Some(scroller) = self.host.transcript_scroll_view().and_then(|scroll| scroll.verticalScroller()) else { return };
        let track = scroller.frame().size.height;
        let proportion = scroller.knobProportion();
        let length = proportion * track;
        let top = scroller.doubleValue() * (track - length);
        // A thumb only moves once shown: a transcript too short to scroll has none.
        let shown = scroller.isEnabled() && proportion > 0.0 && proportion < 1.0;
        if !shown {
            self.thumb = None;
            return;
        }
        self.thumb_shown_ms.get_or_insert(elapsed);
        if let Some((last_top, last_length)) = self.thumb.replace((top, length)) {
            let moved = (top - last_top).abs().max((length - last_length).abs());
            if moved > 0.5 {
                self.thumb_jumps += 1;
                self.thumb_travel += moved;
                self.thumb_max = self.thumb_max.max(moved);
                let (rows, _) = self.host.content_extent();
                self.thumb_moves.push(json!({
                    "ms": elapsed,
                    "top": (top - last_top * 1.0),
                    "length": length - last_length,
                    "rows": rows,
                    "loading": self.host.history_loading(),
                    "document": self.document_height(),
                }));
            }
        }
    }

    /// One step's checks.
    fn look(&mut self, pass: usize, step: usize) {
        self.display();
        let (top, _) = self.viewport();
        let stats = &mut self.stats[pass];
        stats.steps += 1;
        // `OMPEROR_CHECK_PIXELS=0`: geometry only, for timing the open finely.
        if std::env::var_os("OMPEROR_CHECK_PIXELS").is_none_or(|value| value != "0") {
            self.pixels(pass, step, top);
        }
        let geometry = self.host.layout_violations();
        if !geometry.is_empty() {
            self.stats[pass].geometry += 1;
            eprintln!("transcript-check: {} step {step} at {top:.0}: {}", pass_name(pass), geometry[0]);
            self.failures.push(json!({
                "kind": "geometry",
                "pass": pass_name(pass),
                "step": step,
                "scroll_y": top,
                "violations": geometry.iter().take(8).collect::<Vec<_>>(),
                "count": geometry.len(),
            }));
        }
    }

    /// The viewport as the layers keep it against the viewport drawn again.
    fn pixels(&mut self, pass: usize, step: usize, top: f64) {
        let Some(clip) = self.clip() else { return };
        let Some(kept) = layers(&clip) else { return };
        // The capture must show the text, or the comparison proves nothing.
        let inked = kept.pixels.chunks_exact(4).filter(|pixel| pixel[3] != 0 && pixel[..3] != [255; 3]).count();
        if inked == 0 && pass != OPEN && passes()[pass].waits() {
            self.stats[pass].blank += 1;
        }
        if pass == 0 && step == 1 {
            self.write_images("sample-down-0001".into(), Capture { pixels: kept.pixels.clone(), ..kept }, Capture { pixels: kept.pixels.clone(), ..kept });
        }
        self.host.invalidate_visible();
        self.display();
        let Some(redrawn) = layers(&clip) else { return };
        let (differing, span) = compare(&kept, &redrawn);
        if differing == 0 {
            return;
        }
        self.stats[pass].stale += 1;
        self.stats[pass].stale_pixels += differing;
        let stem = format!("stale-{:02}-{}-{step:04}", self.images, pass_name(pass));
        eprintln!(
            "transcript-check: {} step {step} at {top:.0}: {differing} stale pixels in viewport rows {:?}",
            pass_name(pass),
            span
        );
        let images = self.images < MAX_IMAGES;
        self.failures.push(json!({
            "kind": "stale",
            "pass": pass_name(pass),
            "step": step,
            "scroll_y": top,
            "pixels": differing,
            "viewport_rows_pt": span,
            "images": images.then(|| stem.clone()),
        }));
        if images {
            self.images += 1;
            self.write_images(stem, kept, redrawn);
        }
    }

    /// The kept and redrawn viewports, and the kept one with every differing
    /// pixel marked red, written on a worker.
    fn write_images(&mut self, stem: String, kept: Capture, redrawn: Capture) {
        let out = self.out.clone();
        self.writers.push(std::thread::spawn(move || {
            let (width, height) = (kept.width as u32, kept.height as u32);
            let (a, b) = (rgb(&kept), rgb(&redrawn));
            let mut diff = a.clone();
            for (index, (p, q)) in a.chunks_exact(3).zip(b.chunks_exact(3)).enumerate() {
                let o = &mut diff[index * 3..index * 3 + 3];
                if p == q {
                    for channel in o.iter_mut() {
                        *channel = 128 + *channel / 2;
                    }
                } else {
                    o.copy_from_slice(&[230, 0, 0]);
                }
            }
            for (suffix, pixels) in [("kept", &a), ("redrawn", &b), ("diff", &diff)] {
                let path = out.join(format!("{stem}.{suffix}.png"));
                if let Err(error) = image::save_buffer_with_format(&path, pixels, width, height, image::ExtendedColorType::Rgb8, image::ImageFormat::Png) {
                    eprintln!("transcript-check: cannot write {}: {error}", path.display());
                }
            }
        }));
    }

    fn finish(&mut self) -> ! {
        let passes: Vec<Value> = (0..=PASSES.len())
            .zip(&self.stats)
            .map(|(pass, stats)| {
                json!({
                    "pass": pass_name(pass),
                    "steps": stats.steps,
                    "geometry_failures": stats.geometry,
                    "stale_steps": stats.stale,
                    "stale_pixels": stats.stale_pixels,
                    "blank_captures": stats.blank,
                })
            })
            .collect();
        let failed = !self.failures.is_empty() || self.stats.iter().enumerate().any(|(pass, stats)| stats.blank > 0 || (stats.steps == 0 && !(open_only() && pass != OPEN)));
        let (rows, _) = self.host.content_extent();
        let report = json!({
            "tool": "omperor --transcript-check",
            "session": self.id,
            "rows": rows,
            "document_pt": self.document_height(),
            "loaded_ms": self.loaded_ms,
            "open": {
                "first_rows_ms": self.first_rows_ms,
                "complete_ms": self.complete_ms,
                "thumb_shown_ms": self.thumb_shown_ms,
                "thumb_jumps": self.thumb_jumps,
                "thumb_travel_pt": self.thumb_travel,
                "thumb_max_pt": self.thumb_max,
                "saved_heights_found": self.saved_found,
                "thumb_moves": self.thumb_moves,
            },
            "passes": passes,
            "clean": !failed,
            "failures": self.failures,
        });
        for pass in &passes {
            eprintln!("transcript-check: {pass}");
        }
        eprintln!("transcript-check: open {}", report["open"]);
        for writer in self.writers.drain(..) {
            let _ = writer.join();
        }
        let path = self.out.join("report.json");
        if let Err(error) = std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap_or_default()) {
            eprintln!("transcript-check: cannot write {}: {error}", path.display());
        }
        eprintln!("transcript-check: {} — wrote {}", if failed { "FAILED" } else { "clean" }, path.display());
        std::process::exit(i32::from(failed));
    }
}
