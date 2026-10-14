//! The main thread telling on itself. Every pass of the main run loop is
//! timed from the moment it wakes to the moment it waits again — sources,
//! timers, notifications, layout, and Core Animation's commit included — and
//! any pass longer than one 240 Hz frame is written down with the spans that
//! ran in it: always, not only when profiling is switched on. The standard is
//! every pass under budget; a pass over it is a defect with a name.
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_core_foundation::{
    CFIndex, CFRetained, CFRunLoop, CFRunLoopActivity, CFRunLoopObserver, kCFRunLoopCommonModes,
};
use serde_json::json;

/// One 240 Hz frame, the budget every pass is held to.
pub(crate) const BUDGET_US: u128 = 4_000;
/// The log beside `metrics.jsonl`: one line per pass over budget.
pub(crate) const FILE_NAME: &str = "frames.jsonl";
/// The pass is bounded by the first and last observers to run, so nothing
/// AppKit or Core Animation does on the thread escapes it. Core Animation
/// commits at order 2 000 000; a mark just before it tells the app's own
/// handling from the commit.
const FIRST_ORDER: CFIndex = CFIndex::MIN;
const LAST_ORDER: CFIndex = CFIndex::MAX;
const PRE_COMMIT_ORDER: CFIndex = 1_999_999;
/// A pass that woke for a timer runs the timer first, then its sources —
/// events, AppKit's display cycle, blocks — so the first `BeforeSources`
/// mark of a pass tells the timers' work from what followed it.
const TIMERS_DONE_ORDER: CFIndex = CFIndex::MIN;
/// How many of a pass's spans a report names, longest first.
const SPANS_NAMED: usize = 8;
/// A pass that opens more spans than this keeps the longest of the first
/// thousand; the ledger must never grow without bound inside one pass.
const SPANS_LEDGERED: usize = 1_024;
/// How many of the latest passes are kept for the live readout: two seconds
/// of frames at 240 Hz.
const RECENT_PASSES: usize = 480;
/// The upper edges, in microseconds, of the histogram's buckets; the last
/// bucket is everything above.
pub(crate) const BUCKET_EDGES_US: [u32; 7] = [1_000, 2_000, 4_000, 8_000, 16_000, 50_000, 250_000];

thread_local! {
    static PASS_STARTED: Cell<Option<Instant>> = const { Cell::new(None) };
    static TIMERS_US: Cell<Option<u128>> = const { Cell::new(None) };
    static HANDLED_US: Cell<Option<u128>> = const { Cell::new(None) };
    static ACTIVE: RefCell<Vec<(&'static str, Instant)>> = const { RefCell::new(Vec::new()) };
    static FINISHED: RefCell<Vec<(&'static str, u128)>> = const { RefCell::new(Vec::new()) };
    /// Spans past the ledger's thousand, summed by name: a benchmark's
    /// totals stay whole while the ledger stays bounded.
    static OVERFLOW: RefCell<Vec<(&'static str, u128, u32)>> = const { RefCell::new(Vec::new()) };
    static OBSERVERS: RefCell<Vec<CFRetained<CFRunLoopObserver>>> = const { RefCell::new(Vec::new()) };
    static RECENT: RefCell<VecDeque<u32>> = const { RefCell::new(VecDeque::new()) };
    static HISTOGRAM: RefCell<[u64; BUCKET_EDGES_US.len() + 1]> = const { RefCell::new([0; BUCKET_EDGES_US.len() + 1]) };
    static LAST_OVER: RefCell<Option<Culprit>> = const { RefCell::new(None) };
    /// Every pass since a benchmark asked, while it is collecting.
    static COLLECTED: RefCell<Option<Vec<Pass>>> = const { RefCell::new(None) };
    /// A benchmark's look at the end of every pass, after it is timed.
    static PASS_HOOK: Cell<Option<fn(Instant, u32)>> = const { Cell::new(None) };
}

/// Calls `hook` at the end of every main-thread pass, with when the pass
/// began and how long it took, outside its timing: a headless benchmark
/// looks at what the pass left on screen.
pub(crate) fn set_pass_hook(hook: Option<fn(Instant, u32)>) {
    PASS_HOOK.set(hook);
}

/// One main run-loop pass as a benchmark keeps it: when it began, how long
/// it took, and every span that closed in it (the ledger's first thousand).
#[derive(Debug, Clone)]
pub(crate) struct Pass {
    pub(crate) started: Instant,
    pub(crate) us: u32,
    pub(crate) spans: Vec<(&'static str, u32)>,
    /// Spans past the ledger: name, total microseconds, count.
    pub(crate) overflow: Vec<(&'static str, u64, u32)>,
}

/// Starts keeping every pass that ends from now on, dropping any kept
/// before. For a headless benchmark: the passes are its measurements.
pub(crate) fn collect_passes() {
    COLLECTED.with(|collected| collected.replace(Some(Vec::new())));
}

/// The passes kept since `collect_passes`, oldest first; collecting stops.
pub(crate) fn take_passes() -> Vec<Pass> {
    COLLECTED.with(|collected| collected.take().unwrap_or_default())
}

/// A benchmark measures thousands of passes over budget on purpose; its
/// own report replaces the per-pass line on stderr. The log still gets them.
static QUIET: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

/// The last pass over budget, and the span that ran longest in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Culprit {
    pub(crate) pass_us: u32,
    pub(crate) span: Option<(&'static str, u32)>,
}

/// The frame budget as it stands, for a readout: the counters, the latest
/// passes oldest first, the histogram, and the last pass over budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) passes: u64,
    pub(crate) over: u64,
    pub(crate) worst_us: u64,
    pub(crate) recent: Vec<u32>,
    pub(crate) histogram: [u64; BUCKET_EDGES_US.len() + 1],
    pub(crate) last_over: Option<Culprit>,
}

impl Snapshot {
    /// Several percentiles of the latest passes from one sort.
    pub(crate) fn percentiles_us<const N: usize>(&self, fractions: [f64; N]) -> [Option<u32>; N] {
        if self.recent.is_empty() {
            return [None; N];
        }
        let mut sorted = self.recent.clone();
        sorted.sort_unstable();
        fractions.map(|fraction| {
            let rank = ((sorted.len() as f64 - 1.0) * fraction.clamp(0.0, 1.0)).round() as usize;
            sorted.get(rank).copied()
        })
    }

    /// The share of all passes inside the budget, 0 to 1.
    pub(crate) fn within_budget(&self) -> f64 {
        if self.passes == 0 {
            1.0
        } else {
            1.0 - self.over as f64 / self.passes as f64
        }
    }

    /// The histogram as a line: how many passes fell under each edge.
    pub(crate) fn histogram_line(&self) -> String {
        let total = self.histogram.iter().sum::<u64>().max(1) as f64;
        let mut parts = Vec::with_capacity(self.histogram.len());
        for (index, count) in self.histogram.iter().enumerate() {
            let label = match BUCKET_EDGES_US.get(index) {
                Some(edge) => format!("≤{} ms", edge / 1_000),
                None => format!(">{} ms", BUCKET_EDGES_US[BUCKET_EDGES_US.len() - 1] / 1_000),
            };
            parts.push(format!("{label} {:.1} %", *count as f64 * 100.0 / total));
        }
        parts.join(", ")
    }
}

/// The budget as it stands on this thread; meaningful on the main thread.
pub(crate) fn snapshot() -> Snapshot {
    Snapshot {
        passes: PASSES.load(Ordering::Relaxed),
        over: OVER_BUDGET.load(Ordering::Relaxed),
        worst_us: WORST_US.load(Ordering::Relaxed),
        recent: RECENT.with(|recent| recent.borrow().iter().copied().collect()),
        histogram: HISTOGRAM.with(|histogram| *histogram.borrow()),
        last_over: LAST_OVER.with(|last| last.borrow().clone()),
    }
}

static PASSES: AtomicU64 = AtomicU64::new(0);
static OVER_BUDGET: AtomicU64 = AtomicU64::new(0);
static WORST_US: AtomicU64 = AtomicU64::new(0);
static SINK: OnceLock<Option<flume::Sender<String>>> = OnceLock::new();

/// Installs the observer on the main run loop. Once, on the main thread, at
/// launch; `directory` is where the log lives, or `None` for counters and
/// stderr alone.
pub(crate) fn install(directory: Option<PathBuf>) {
    let _ = SINK.get_or_init(|| directory.and_then(open_sink));
    if MainThreadMarker::new().is_none() {
        return;
    }
    let Some(run_loop) = CFRunLoop::main() else {
        return;
    };
    let ends = CFRunLoopActivity::BeforeWaiting | CFRunLoopActivity::Exit;
    let starts = CFRunLoopActivity::AfterWaiting | CFRunLoopActivity::Entry;
    let begin = RcBlock::new(
        |_observer: *mut CFRunLoopObserver, _activity: CFRunLoopActivity| {
            begin_pass();
        },
    );
    let end = RcBlock::new(
        |_observer: *mut CFRunLoopObserver, _activity: CFRunLoopActivity| {
            end_pass();
        },
    );
    let handled = RcBlock::new(
        |_observer: *mut CFRunLoopObserver, _activity: CFRunLoopActivity| {
            mark_handled();
        },
    );
    let timers = RcBlock::new(
        |_observer: *mut CFRunLoopObserver, _activity: CFRunLoopActivity| {
            mark_timers_done();
        },
    );
    // SAFETY: the handlers are copied by Core Foundation and only ever run on
    // the main thread's run loop; the observers are retained below for the
    // process lifetime, and the common-modes constant is a process-lifetime
    // static.
    let observers = unsafe {
        [
            CFRunLoopObserver::with_handler(None, starts.0, true, FIRST_ORDER, Some(&begin)),
            CFRunLoopObserver::with_handler(
                None,
                CFRunLoopActivity::BeforeSources.0,
                true,
                TIMERS_DONE_ORDER,
                Some(&timers),
            ),
            CFRunLoopObserver::with_handler(None, ends.0, true, PRE_COMMIT_ORDER, Some(&handled)),
            CFRunLoopObserver::with_handler(None, ends.0, true, LAST_ORDER, Some(&end)),
        ]
    };
    for observer in observers.into_iter().flatten() {
        // SAFETY: the constant is a process-lifetime static exported by CF.
        run_loop.add_observer(Some(&observer), unsafe { kCFRunLoopCommonModes });
        OBSERVERS.with(|slot| slot.borrow_mut().push(observer));
    }
}

/// Passes seen, passes over budget, and the worst pass in microseconds.
pub(crate) fn summary() -> (u64, u64, u64) {
    (
        PASSES.load(Ordering::Relaxed),
        OVER_BUDGET.load(Ordering::Relaxed),
        WORST_US.load(Ordering::Relaxed),
    )
}

/// A `perf` span opened on the main thread joins the pass's ledger.
pub(crate) fn span_started(name: &'static str) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    ACTIVE.with(|active| active.borrow_mut().push((name, Instant::now())));
}

/// The span closed; what it cost is remembered until the pass ends.
pub(crate) fn span_finished(name: &'static str, micros: u128) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        if let Some(index) = active.iter().rposition(|(open, _)| *open == name) {
            active.remove(index);
        }
    });
    ledger(name, micros);
}

/// Keeps a closed span for the pass: in the ledger while it has room, then
/// summed by name.
fn ledger(name: &'static str, micros: u128) {
    FINISHED.with(|finished| {
        let mut finished = finished.borrow_mut();
        if finished.len() < SPANS_LEDGERED {
            finished.push((name, micros));
        } else {
            OVERFLOW.with(|overflow| {
                let mut overflow = overflow.borrow_mut();
                match overflow.iter_mut().find(|entry| entry.0 == name) {
                    Some(entry) => {
                        entry.1 += micros;
                        entry.2 += 1;
                    }
                    None => overflow.push((name, micros, 1)),
                }
            });
        }
    });
}

fn begin_pass() {
    PASS_STARTED.with(|started| {
        if started.get().is_none() {
            started.set(Some(Instant::now()));
            TIMERS_US.with(Cell::take);
            HANDLED_US.with(Cell::take);
            FINISHED.with(|finished| finished.borrow_mut().clear());
            OVERFLOW.with(|overflow| overflow.borrow_mut().clear());
        }
    });
}

/// The pass's timers have run; the sources — events, AppKit's display
/// cycle, queued blocks — follow. Only the first mark of a pass counts.
fn mark_timers_done() {
    if let Some(started) = PASS_STARTED.with(Cell::get) {
        TIMERS_US.with(|timers| {
            if timers.get().is_none() {
                timers.set(Some(started.elapsed().as_micros()));
            }
        });
    }
}

/// The app's own handling is done; what follows is Core Animation's commit.
fn mark_handled() {
    if let Some(started) = PASS_STARTED.with(Cell::get) {
        HANDLED_US.with(|handled| handled.set(Some(started.elapsed().as_micros())));
    }
}

fn end_pass() {
    let Some(started) = PASS_STARTED.with(Cell::take) else {
        return;
    };
    let micros = started.elapsed().as_micros();
    let handled = HANDLED_US.with(Cell::take).unwrap_or(micros).min(micros);
    let timers = TIMERS_US.with(Cell::take).unwrap_or(handled).min(handled);
    let mut spans = FINISHED.with(|finished| std::mem::take(&mut *finished.borrow_mut()));
    // A span still open when the pass ends — there should be none — counts
    // for what it has cost so far.
    ACTIVE.with(|active| {
        for (name, opened) in active.borrow().iter() {
            spans.push((name, opened.elapsed().as_micros()));
        }
    });
    COLLECTED.with(|collected| {
        if let Some(passes) = collected.borrow_mut().as_mut() {
            let clip = |micros: u128| u32::try_from(micros).unwrap_or(u32::MAX);
            passes.push(Pass {
                started,
                us: clip(micros),
                spans: spans.iter().map(|(name, micros)| (*name, clip(*micros))).collect(),
                overflow: OVERFLOW.with(|overflow| {
                    overflow
                        .borrow_mut()
                        .drain(..)
                        .map(|(name, total, count)| (name, u64::try_from(total).unwrap_or(u64::MAX), count))
                        .collect()
                }),
            });
        }
    });
    record_pass(micros, Split { timers, handled }, spans);
    if let Some(hook) = PASS_HOOK.get() {
        hook(started, u32::try_from(micros).unwrap_or(u32::MAX));
    }
}

/// Where a pass's time went, as elapsed marks from its start.
#[derive(Clone, Copy)]
struct Split {
    /// The timers that woke the pass have run.
    timers: u128,
    /// The sources — events, AppKit's display cycle, blocks — have run;
    /// Core Animation's commit follows.
    handled: u128,
}

/// Counts the pass and, when it overran the budget, reports it with its
/// longest spans.
fn record_pass(micros: u128, split: Split, spans: Vec<(&'static str, u128)>) {
    PASSES.fetch_add(1, Ordering::Relaxed);
    let clipped = u32::try_from(micros).unwrap_or(u32::MAX);
    RECENT.with(|recent| {
        let mut recent = recent.borrow_mut();
        if recent.len() == RECENT_PASSES {
            recent.pop_front();
        }
        recent.push_back(clipped);
    });
    HISTOGRAM.with(|histogram| {
        let bucket = BUCKET_EDGES_US
            .iter()
            .position(|edge| clipped <= *edge)
            .unwrap_or(BUCKET_EDGES_US.len());
        histogram.borrow_mut()[bucket] += 1;
    });
    if micros <= BUDGET_US {
        return;
    }
    OVER_BUDGET.fetch_add(1, Ordering::Relaxed);
    WORST_US.fetch_max(u64::try_from(micros).unwrap_or(u64::MAX), Ordering::Relaxed);
    let longest = spans
        .iter()
        .max_by_key(|(_, span_us)| *span_us)
        .map(|(name, span_us)| (*name, u32::try_from(*span_us).unwrap_or(u32::MAX)));
    LAST_OVER.with(|last| {
        last.borrow_mut().replace(Culprit {
            pass_us: clipped,
            span: longest,
        })
    });
    let line = report(micros, split, spans);
    if let Some(Some(sink)) = SINK.get() {
        // The writer is its own thread; a full channel drops the line rather
        // than making the pass longer.
        let _ = sink.try_send(line);
    }
}

/// The report for a pass over budget: a line on stderr and the JSON line
/// for the log, longest spans first.
fn report(micros: u128, split: Split, mut spans: Vec<(&'static str, u128)>) -> String {
    spans.sort_by(|a, b| b.1.cmp(&a.1));
    spans.truncate(SPANS_NAMED);
    let named = spans
        .iter()
        .map(|(name, micros)| format!("{name} {micros} µs"))
        .collect::<Vec<_>>()
        .join(", ");
    let timers = split.timers;
    let sources = split.handled - split.timers;
    let commit = micros - split.handled;
    if !QUIET.load(Ordering::Relaxed) {
        eprintln!(
        "frame budget: a main-thread pass took {micros} µs ({:.1}× the {BUDGET_US} µs frame; timers {timers} µs, sources {sources} µs, commit {commit} µs): {}",
        micros as f64 / BUDGET_US as f64,
        if named.is_empty() { "no spans" } else { &named }
        );
    }
    json!({
        "k": "frame",
        "at": wall_ms(),
        "us": u64::try_from(micros).unwrap_or(u64::MAX),
        "timers_us": u64::try_from(timers).unwrap_or(u64::MAX),
        "sources_us": u64::try_from(sources).unwrap_or(u64::MAX),
        "commit_us": u64::try_from(commit).unwrap_or(u64::MAX),
        "budget_us": u64::try_from(BUDGET_US).unwrap_or(u64::MAX),
        "spans": spans
            .iter()
            .map(|(name, micros)| json!({"name": name, "us": u64::try_from(*micros).unwrap_or(u64::MAX)}))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn open_sink(directory: PathBuf) -> Option<flume::Sender<String>> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(FILE_NAME))
        .ok()?;
    let (sender, receiver) = flume::bounded::<String>(256);
    std::thread::Builder::new()
        .name("frame-budget-log".into())
        .spawn(move || {
            for line in receiver.iter() {
                if writeln!(file, "{line}").is_err() {
                    break;
                }
                let _ = file.flush();
            }
        })
        .ok()?;
    Some(sender)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_under_budget_is_counted_and_never_reported() {
        let (passes, over, _) = summary();
        record_pass(
            3_999,
            Split {
                timers: 3_000,
                handled: 3_500,
            },
            vec![("gui.tick", 3_000)],
        );
        let (passes_after, over_after, _) = summary();
        // Other tests share the counters; only the direction is certain.
        assert!(passes_after > passes);
        assert!(over_after >= over);
    }

    #[test]
    fn a_pass_over_budget_names_its_longest_spans_first() {
        let line = report(
            12_500,
            Split {
                timers: 11_000,
                handled: 12_000,
            },
            vec![
                ("gui.tick", 12_000),
                ("gui.layout_header", 300),
                ("gui.render_chat_transcript", 11_000),
            ],
        );
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["k"], "frame");
        assert_eq!(value["us"], 12_500);
        assert_eq!(value["timers_us"], 11_000);
        assert_eq!(value["sources_us"], 1_000);
        assert_eq!(value["commit_us"], 500);
        assert_eq!(value["budget_us"], 4_000);
        let names = value["spans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|span| span["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "gui.tick",
                "gui.render_chat_transcript",
                "gui.layout_header"
            ]
        );
    }

    #[test]
    fn the_snapshot_keeps_the_latest_passes_and_names_the_last_culprit() {
        for micros in [1_000, 2_000, 3_000, 3_500] {
            record_pass(
                micros,
                Split {
                    timers: micros,
                    handled: micros,
                },
                Vec::new(),
            );
        }
        record_pass(
            12_000,
            Split {
                timers: 12_000,
                handled: 12_000,
            },
            vec![("gui.tick", 11_000), ("gui.rebuild_sidebar", 9_000)],
        );
        let snapshot = snapshot();
        let tail = &snapshot.recent[snapshot.recent.len() - 5..];
        assert_eq!(tail, [1_000, 2_000, 3_000, 3_500, 12_000]);
        assert!(snapshot.percentiles_us([0.99])[0].unwrap() >= 3_500);
        assert_eq!(
            snapshot.last_over,
            Some(Culprit {
                pass_us: 12_000,
                span: Some(("gui.tick", 11_000))
            })
        );
        assert!(snapshot.histogram_line().starts_with("≤1 ms "));
        assert!(snapshot.within_budget() <= 1.0);
    }

    #[test]
    fn a_pass_over_budget_raises_the_counters_and_the_worst_time() {
        record_pass(
            9_000,
            Split {
                timers: 9_000,
                handled: 9_000,
            },
            Vec::new(),
        );
        let (_, over, worst) = summary();
        assert!(over >= 1);
        assert!(worst >= 9_000);
    }

    #[test]
    fn a_collecting_benchmark_keeps_every_pass_with_its_spans_until_it_takes_them() {
        // Nothing is kept before a benchmark asks.
        begin_pass();
        end_pass();
        assert!(take_passes().is_empty());
        collect_passes();
        for span_us in [300_u128, 9_000] {
            begin_pass();
            FINISHED.with(|finished| finished.borrow_mut().push(("gui.bench.draw", span_us)));
            end_pass();
        }
        let passes = take_passes();
        assert_eq!(passes.len(), 2, "short and long passes alike");
        assert_eq!(passes[1].spans, [("gui.bench.draw", 9_000)]);
        assert!(passes[0].started <= passes[1].started);
        // Taking stops the collection.
        begin_pass();
        end_pass();
        assert!(take_passes().is_empty());
    }

    #[test]
    fn spans_past_the_ledger_are_summed_by_name_for_a_benchmark() {
        collect_passes();
        begin_pass();
        for _ in 0..SPANS_LEDGERED + 3 {
            ledger("gui.markdown.syntax", 2);
        }
        ledger("gui.bench.draw", 7);
        end_pass();
        let passes = take_passes();
        assert_eq!(passes[0].spans.len(), SPANS_LEDGERED);
        assert_eq!(passes[0].overflow, [("gui.markdown.syntax", 6, 3), ("gui.bench.draw", 7, 1)]);
        // The next pass starts with nothing over.
        collect_passes();
        begin_pass();
        end_pass();
        assert!(take_passes()[0].overflow.is_empty());
    }
}
