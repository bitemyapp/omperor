//! `--scroll-smoke` (Linux): how the transcript keeps up with real scrolling;
//! `--switch-smoke`: how fast the Conversation / Observatory switch answers.
//!
//! Streams the mock backend's showcase answer into a fresh conversation,
//! then scrolls the transcript through Sidestep's own input path, as the
//! compositor would: touchpad gestures down and up, wheel detents both
//! ways, and a fling that coasts. Input arrives on a fixed schedule of real
//! time — when the main thread falls behind, the events it missed are all
//! waiting for it, as they would be in the Wayland socket — so a pass that
//! is too slow shows up as input piling up, not as a gentler test.
//!
//! It reports how late the main thread got to its input, how long its
//! turns took, and how far each part of the script scrolled, then quits.
//! Run it under a scratch `HOME`; it starts no inference child. With
//! `SIDESTEP_TRACE_FRAMES=1` Sidestep prints each display pass and present
//! (`=tiles`: each tile drawn, and how much text it holds), and with
//! `OMPEROR_SCROLL_SMOKE_HOLD=1` input pauses mid-gesture, announced on
//! stderr, for a screenshot of what a reader sees then.
//!
//! `--switch-smoke` streams the same answer, then clicks the switch's two
//! segments in turn, a press and a release 80 ms apart as a quick click
//! is, and reports for each how long after the release the switch chose
//! the segment (its action done) and the longest main-thread turn around
//! it; `OMPEROR_SWITCH_SMOKE_FRAMES` writes the frames of every switch
//! (see `dump_dir`).

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use objc2::{MainThreadMarker, Message};
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSScrollView, NSWindow};
use objc2_foundation::{NSRunLoop, NSRunLoopCommonModes, NSTimer};
use sidestep::appkit::testing;

/// The prompt streamed before scrolling; `OMPEROR_SCROLL_SMOKE_PROMPT`
/// replaces it (a paced stream relays out the answer as a reader sees it).
pub(crate) fn prompt() -> String {
    std::env::var("OMPEROR_SCROLL_SMOKE_PROMPT").unwrap_or_else(|_| "/mock scenario=showcase rate=0 ttft=0".into())
}

/// Events a second a touchpad sends, and how often the harness looks.
const TOUCHPAD_HZ: f64 = 120.0;
const WHEEL_HZ: f64 = 30.0;

pub(crate) fn requested() -> bool {
    std::env::args().any(|arg| arg == "--scroll-smoke" || arg == "--switch-smoke")
}

/// `OMPEROR_SWITCH_SMOKE_FRAMES=dir` with `SIDESTEP_BACKEND=null`: Sidestep's
/// null render thread draws at 2×. From a press until 700 ms after it, a
/// worker saves the switch's pixels as PPM files named by the requested
/// frame's milliseconds since clicking started. Only one capture/write is
/// in flight; busy ticks skip capture so diagnostics cannot queue up work.
fn dump_dir() -> Option<String> {
    std::env::var("OMPEROR_SWITCH_SMOKE_FRAMES").ok().filter(|_| std::env::var("SIDESTEP_BACKEND").is_ok_and(|b| b == "null"))
}

/// Before any window shows: capture pixels at 2× for the frame dump.
pub(crate) fn prepare_capture() {
    if dump_dir().is_some() {
        testing::use_null_backend_scale(2);
        testing::capture_pixels(true);
    }
}

/// `--switch-smoke` rather than `--scroll-smoke`.
pub(crate) fn switching() -> bool {
    std::env::args().any(|arg| arg == "--switch-smoke")
}

/// The harness makes and saves a conversation: it never runs in the
/// user's real home.
pub(crate) fn refuse_real_home() {
    if std::env::var_os("OMPEROR_STATE_DIR").is_some_and(|value| !value.is_empty()) {
        eprintln!("scroll-smoke: unset OMPEROR_STATE_DIR before running the scratch-home harness");
        std::process::exit(2);
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let canonical = |path: &std::path::Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let real = crate::markdown_bench::real_home();
    if home.is_none() || home.as_deref().map(canonical) == real.as_deref().map(canonical) {
        eprintln!("scroll-smoke: refusing to run in your real home; give it a scratch one:\n\n    HOME=\"$(mktemp -d)\" omperor --scroll-smoke\n");
        std::process::exit(2);
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Touchpad { phase: u8, velocity: f64 },
    Wheel,
    /// `OMPEROR_SCROLL_SMOKE_HOLD`: input stops a while mid-gesture.
    Hold,
    /// The script's end, once a fling has coasted out.
    End,
}

/// How long a hold lasts.
const HOLD: f64 = 2.0;

/// One input event, due `at` after the scrolling starts.
#[derive(Clone, Copy, Debug)]
struct Input {
    at: f64,
    dy: f64,
    kind: Kind,
    label: &'static str,
}

/// The input, in real time: gestures at a brisk reading pace, detents at
/// a quick wheel's, and a fling.
fn script() -> Vec<Input> {
    let mut inputs = Vec::new();
    let mut t = 0.3;
    let hold = std::env::var_os("OMPEROR_SCROLL_SMOKE_HOLD").is_some();
    let gesture = |t: &mut f64, seconds: f64, speed: f64, coast: f64, label: &'static str, inputs: &mut Vec<Input>| {
        let step = 1.0 / TOUCHPAD_HZ;
        let count = (seconds * TOUCHPAD_HZ).round() as usize;
        for i in 0..=count {
            // Holds halfway and at the end of a gesture that doesn't coast.
            if hold && (i == count / 2 || i == count) && coast == 0.0 {
                inputs.push(Input { at: *t, dy: 0.0, kind: Kind::Hold, label });
                *t += HOLD;
            }
            if i == count {
                break;
            }
            let phase = if i == 0 { 1 } else { 2 };
            inputs.push(Input { at: *t, dy: speed * step, kind: Kind::Touchpad { phase, velocity: 0.0 }, label });
            *t += step;
        }
        inputs.push(Input { at: *t, dy: 0.0, kind: Kind::Touchpad { phase: 3, velocity: coast }, label });
    };
    gesture(&mut t, 1.5, 2400.0, 0.0, "touchpad down", &mut inputs);
    t += 0.6;
    gesture(&mut t, 1.5, -2400.0, 0.0, "touchpad up", &mut inputs);
    t += 0.6;
    for (sign, label) in [(1.0, "wheel down"), (-1.0, "wheel up")] {
        for _ in 0..(1.5 * WHEEL_HZ) as usize {
            inputs.push(Input { at: t, dy: sign, kind: Kind::Wheel, label });
            t += 1.0 / WHEEL_HZ;
        }
        t += 0.6;
    }
    gesture(&mut t, 0.25, 3000.0, 3000.0, "fling", &mut inputs);
    inputs.push(Input { at: t + 2.5, dy: 0.0, kind: Kind::End, label: "coasting" });
    inputs
}

struct Driver {
    scroll: Retained<NSScrollView>,
    id: u32,
    point: (f64, f64),
    inputs: Vec<Input>,
    next: usize,
    started: Instant,
    last_tick: Instant,
    /// Per tick: how long since the previous one (main-thread turns).
    gaps: Vec<f64>,
    /// Per input: how late it was handed to the main thread.
    late: Vec<(&'static str, f64)>,
    /// Per tick: the input under way and how far (points) the scroll
    /// position moved. Input is handled after the tick that sends it.
    moved: Vec<(&'static str, f64)>,
    label: &'static str,
    last_y: f64,
    timer: Option<Retained<NSTimer>>,
}

thread_local! {
    static DRIVER: RefCell<Option<Driver>> = const { RefCell::new(None) };
}

/// Start scrolling `scroll`, the transcript, from its top.
pub(crate) fn start(window: &NSWindow, scroll: Retained<NSScrollView>) {
    let clip = scroll.contentView();
    clip.scrollToPoint(objc2_foundation::NSPoint::new(0.0, 0.0));
    scroll.reflectScrolledClipView(&clip);
    let frame = scroll.convertRect_toView(scroll.bounds(), None);
    let content = window.contentView().map_or(0.0, |view| view.frame().size.height);
    let x = frame.origin.x + frame.size.width / 2.0;
    let y = content - (frame.origin.y + frame.size.height / 2.0);
    let document = scroll.documentView().map_or(0.0, |doc| doc.frame().size.height);
    eprintln!(
        "scroll-smoke: transcript {:.0}×{:.0} pt, document {:.0} pt tall, input at ({x:.0}, {y:.0})",
        frame.size.width, frame.size.height, document
    );
    let now = Instant::now();
    let id = testing::showing_id(window);
    // The pointer rests over the transcript, as a reader's does: content
    // moving under it updates cursor rectangles and tracking areas.
    testing::inject_enter(id, x, y);
    testing::inject_motion(id, x, y, 0);
    let block = block2::RcBlock::new(|_timer: std::ptr::NonNull<NSTimer>| tick());
    // SAFETY: the block runs on the main run loop; common modes keep it
    // firing whatever mode a scroll puts the loop in.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(1.0 / TOUCHPAD_HZ, true, &block) };
    // SAFETY: the main run loop and the mode constant live for the process.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    DRIVER.with(|driver| {
        *driver.borrow_mut() = Some(Driver {
            scroll,
            id,
            point: (x, y),
            inputs: script(),
            next: 0,
            started: now,
            last_tick: now,
            gaps: Vec::new(),
            late: Vec::new(),
            moved: Vec::new(),
            label: "start",
            last_y: 0.0,
            timer: Some(timer),
        })
    });
}

fn tick() {
    let done = DRIVER.with(|driver| {
        let mut driver = driver.borrow_mut();
        let Some(d) = driver.as_mut() else { return false };
        let now = Instant::now();
        d.gaps.push(now.duration_since(d.last_tick).as_secs_f64() * 1000.0);
        d.last_tick = now;
        let elapsed = now.duration_since(d.started).as_secs_f64();
        // Everything due by now arrives together, as from a socket the main
        // thread did not read while it was busy.
        let y = d.scroll.contentView().bounds().origin.y;
        let label = d.label;
        d.moved.push((label, y - d.last_y));
        d.last_y = y;
        while d.next < d.inputs.len() && d.inputs[d.next].at <= elapsed {
            let input = d.inputs[d.next];
            d.late.push((input.label, (elapsed - input.at) * 1000.0));
            d.label = input.label;
            let (x, y) = d.point;
            match input.kind {
                Kind::Touchpad { phase, velocity } => {
                    testing::inject_scroll(d.id, x, y, (0.0, input.dy), phase, (0.0, velocity));
                }
                Kind::Wheel => testing::inject_wheel(d.id, x, y, 0.0, input.dy),
                Kind::End => {}
                Kind::Hold => {
                    let y = d.scroll.contentView().bounds().origin.y;
                    eprintln!("scroll-smoke: hold during {} at y = {y:.0}", input.label);
                }
            }
            d.next += 1;
        }
        d.next >= d.inputs.len()
    });
    if done {
        finish();
    }
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    values[((values.len() - 1) as f64 * p).round() as usize]
}

fn finish() {
    let Some(mut d) = DRIVER.with(|driver| driver.borrow_mut().take()) else { return };
    if let Some(timer) = d.timer.take() {
        timer.invalidate();
    }
    let mut gaps = d.gaps.clone();
    let p50 = percentile(&mut gaps, 0.5);
    let p95 = percentile(&mut gaps, 0.95);
    let worst = gaps.last().copied().unwrap_or(0.0);
    let over = d.gaps.iter().filter(|gap| **gap > 1000.0 / 60.0 + 2.0).count();
    eprintln!(
        "scroll-smoke: main-thread turns between input ticks: median {p50:.1} ms, 95th {p95:.1} ms, worst {worst:.1} ms; {over} of {} over a 60 Hz frame",
        d.gaps.len()
    );
    let mut labels: Vec<&'static str> = Vec::new();
    for (label, _) in &d.late {
        if !labels.contains(label) {
            labels.push(label);
        }
    }
    for label in labels {
        let mut late: Vec<f64> = d.late.iter().filter(|(l, _)| *l == label).map(|(_, ms)| *ms).collect();
        let moved: f64 = d.moved.iter().filter(|(l, _)| *l == label).map(|(_, dy)| dy.abs()).sum();
        let p50 = percentile(&mut late, 0.5);
        let worst = late.last().copied().unwrap_or(0.0);
        eprintln!("scroll-smoke: {label:>13}: input handled late by median {p50:.1} ms, worst {worst:.1} ms; moved {moved:.0} pt");
    }
    let y = d.scroll.contentView().bounds().origin.y;
    eprintln!("scroll-smoke: finished at y = {y:.0}");
    NSApplication::sharedApplication(MainThreadMarker::new().unwrap()).terminate(None);
}

/// How long the harness waits for the showcase to finish streaming.
pub(crate) const STREAM_TIMEOUT: Duration = Duration::from_secs(90);
/// Settle after the stream ends: late layout and images land first.
pub(crate) const SETTLE: Duration = Duration::from_millis(1500);

/// How long a click holds the button, and how far apart clicks are.
const CLICK_HOLD: f64 = 0.08;
const CLICK_EVERY: f64 = 1.2;
const CLICKS: usize = 8;

/// One click on the switch: the segment, when its press and release were
/// sent, and when the switch showed the segment chosen.
struct Click {
    segment: isize,
    down: Option<Instant>,
    up: Option<Instant>,
    chosen: Option<Instant>,
    /// The longest main-thread turn from the press until a second after.
    longest_turn: f64,
}

/// The GUI only queues requests and polls completions. The render thread
/// composites and crops; this worker waits for pixels, encodes and writes.
struct FrameDump {
    jobs: crate::jobs::JobPool<Result<usize, String>>,
    dir: PathBuf,
    pending: bool,
    written: usize,
    error: Option<String>,
}

impl FrameDump {
    fn new(dir: String) -> Self {
        let mut dump = Self {
            jobs: crate::jobs::JobPool::start_notifying(1, "switch-capture", || {}),
            dir: PathBuf::from(dir),
            pending: false,
            written: 0,
            error: None,
        };
        let dir = dump.dir.clone();
        dump.submit(move || {
            std::fs::create_dir_all(&dir)
                .map(|()| 0)
                .map_err(|error| format!("cannot create {}: {error}", dir.display()))
        });
        dump
    }

    fn submit(&mut self, work: impl FnOnce() -> Result<usize, String> + Send + 'static) {
        self.pending = self.jobs.submit(work);
        if !self.pending {
            self.error = Some("frame capture worker stopped".into());
        }
    }

    fn poll(&mut self) {
        if let Some(result) = self.jobs.try_next() {
            self.pending = false;
            match result {
                Ok(count) => self.written += count,
                Err(error) => self.error = Some(error),
            }
        }
    }

    fn capture(&mut self, window: &NSWindow, crop: [u32; 4], elapsed: f64) {
        if self.pending || self.error.is_some() {
            return;
        }
        let pixels = testing::request_window_pixels(window, testing::media_time(), crop);
        let path = self.dir.join(format!("t{:07.1}.ppm", elapsed * 1000.0));
        self.submit(move || {
            let (width, height, pixels) = pixels.recv_timeout(Duration::from_secs(10))
                .map_err(|error| format!("frame capture did not complete: {error}"))?
                .ok_or_else(|| "frame capture unavailable or crop outside window".to_string())?;
            let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
            out.reserve(pixels.len() * 3);
            for [r, g, b, _] in pixels {
                out.extend([r, g, b]);
            }
            std::fs::write(&path, out)
                .map(|()| 1)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))
        });
    }
}

struct Switcher {
    window: Retained<NSWindow>,
    /// The switch's pixels at 2×: x, y, width, height.
    crop: [u32; 4],
    dump: Option<FrameDump>,
    switch: Retained<objc2_app_kit::NSSegmentedControl>,
    id: u32,
    /// The segments' centres, from the window's top left.
    points: [(f64, f64); 2],
    clicks: Vec<Click>,
    started: Instant,
    last_tick: Instant,
    timer: Option<Retained<NSTimer>>,
}

thread_local! {
    static SWITCHER: RefCell<Option<Switcher>> = const { RefCell::new(None) };
}

/// Start clicking `switch` back and forth.
pub(crate) fn start_switching(window: &NSWindow, switch: Retained<objc2_app_kit::NSSegmentedControl>) {
    let frame = switch.convertRect_toView(switch.bounds(), None);
    let content = window.contentView().map_or(0.0, |view| view.frame().size.height);
    let y = content - (frame.origin.y + frame.size.height / 2.0);
    let points = [0.25, 0.75].map(|share| (frame.origin.x + frame.size.width * share, y));
    eprintln!("scroll-smoke: switch at ({:.0}, {:.0}) {:.0}×{:.0} pt", frame.origin.x, y, frame.size.width, frame.size.height);
    let id = testing::showing_id(window);
    testing::inject_enter(id, points[0].0, points[0].1);
    let first = 1 - switch.selectedSegment().clamp(0, 1);
    let clicks = (0..CLICKS)
        .map(|i| Click { segment: (first + i as isize) % 2, down: None, up: None, chosen: None, longest_turn: 0.0 })
        .collect();
    let block = block2::RcBlock::new(|_timer: std::ptr::NonNull<NSTimer>| switch_tick());
    // SAFETY: as in `start`: common modes keep the timer firing while the
    // switch tracks the press in its own loop.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(0.004, true, &block) };
    // SAFETY: the main run loop and the mode constant live for the process.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    let now = Instant::now();
    let dump = dump_dir().map(FrameDump::new);
    if dump.is_some() {
        window.setTitle(objc2_foundation::ns_string!("Omperor — Capturing switch frames…"));
    }
    SWITCHER.with(|s| {
        *s.borrow_mut() = Some(Switcher {
            window: window.retain(),
            crop: [((frame.origin.x - 12.0) * 2.0) as u32, ((y - frame.size.height / 2.0 - 12.0) * 2.0) as u32, ((frame.size.width + 24.0) * 2.0) as u32, ((frame.size.height + 24.0) * 2.0) as u32],
            dump,
            switch,
            id,
            points,
            clicks,
            started: now,
            last_tick: now,
            timer: Some(timer),
        })
    });
}

fn switch_tick() {
    let done = SWITCHER.with(|s| {
        let mut s = s.borrow_mut();
        let Some(d) = s.as_mut() else { return false };
        if let Some(dump) = &mut d.dump {
            dump.poll();
            if dump.error.is_some() {
                return true;
            }
        }
        let now = Instant::now();
        let turn = now.duration_since(d.last_tick).as_secs_f64() * 1000.0;
        d.last_tick = now;
        let elapsed = now.duration_since(d.started).as_secs_f64();
        let selected = d.switch.selectedSegment();
        for (i, click) in d.clicks.iter_mut().enumerate() {
            let at = 0.5 + i as f64 * CLICK_EVERY;
            if click.down.is_some_and(|down| now.duration_since(down).as_secs_f64() < 1.0) {
                click.longest_turn = click.longest_turn.max(turn);
            }
            let (x, y) = d.points[click.segment as usize];
            if click.down.is_none() && elapsed >= at {
                testing::inject_motion(d.id, x, y, 0);
                testing::inject_button(d.id, x, y, 0, true, 1, 0);
                click.down = Some(now);
                eprintln!("scroll-smoke: press on segment {}", click.segment);
            } else if click.down.is_some() && click.up.is_none() && elapsed >= at + CLICK_HOLD {
                testing::inject_button(d.id, x, y, 0, false, 1, 0);
                click.up = Some(now);
                eprintln!("scroll-smoke: release on segment {}", click.segment);
            } else if click.up.is_some() && click.chosen.is_none() && selected == click.segment {
                click.chosen = Some(now);
                eprintln!("scroll-smoke: chose segment {}", click.segment);
            }
        }
        if let Some(dump) = &mut d.dump
            && d.clicks.iter().any(|c| c.down.is_some_and(|down| now.duration_since(down).as_secs_f64() < 0.7))
        {
            dump.capture(&d.window, d.crop, elapsed);
        }
        elapsed > 0.5 + CLICKS as f64 * CLICK_EVERY && d.dump.as_ref().is_none_or(|dump| !dump.pending)
    });
    if done {
        finish_switching();
    }
}

fn finish_switching() {
    let Some(mut d) = SWITCHER.with(|s| s.borrow_mut().take()) else { return };
    if let Some(timer) = d.timer.take() {
        timer.invalidate();
    }
    if let Some(dump) = d.dump {
        if let Some(error) = dump.error {
            eprintln!("scroll-smoke: frame dump failed: {error}");
            std::process::exit(1);
        }
        if dump.written == 0 {
            eprintln!("scroll-smoke: frame dump failed: no frames captured");
            std::process::exit(1);
        }
        eprintln!("scroll-smoke: wrote {} frames to {}", dump.written, dump.dir.display());
    }
    let names = ["Conversation", "Observatory"];
    let mut latencies = Vec::new();
    for click in &d.clicks {
        let chosen = match (click.up, click.chosen) {
            (Some(up), Some(chosen)) => {
                let ms = chosen.saturating_duration_since(up).as_secs_f64() * 1000.0;
                latencies.push(ms);
                format!("chosen {ms:.1} ms after the release")
            }
            _ => "never chosen".into(),
        };
        eprintln!(
            "scroll-smoke: switch to {:>12}: {chosen}; longest main-thread turn {:.1} ms",
            names[click.segment as usize], click.longest_turn
        );
    }
    let p50 = percentile(&mut latencies, 0.5);
    eprintln!("scroll-smoke: switching: median {p50:.1} ms from release to chosen, worst {:.1} ms", latencies.last().copied().unwrap_or(f64::NAN));
    NSApplication::sharedApplication(MainThreadMarker::new().unwrap()).terminate(None);
}
