use std::{
    env,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

const PERF_LOG_ENV: &str = "OMPEROR_PERF_LOG";

#[derive(Debug)]
struct Recorder {
    started: Instant,
    sequence: AtomicU64,
    span_sequence: AtomicU64,
    records: flume::Sender<Value>,
    dropped: AtomicU64,
}

static RECORDER: OnceLock<Option<Recorder>> = OnceLock::new();

/// Enables newline-delimited JSON profiling when `OMPEROR_PERF_LOG` names an
/// output file. Debug builds also retain the latest unconfigured run under the
/// channel's state directory so failures launched through `run-app.sh` remain
/// diagnosable. Production launches without the environment override pay only
/// the `OnceLock` branch and do no allocation, clock reads, or file I/O.
pub(crate) fn initialize() -> bool {
    let enabled = recorder().is_some();
    if enabled {
        event_with("perf.profile.run_start", || {
            json!({
                "schema": "omperor.perf.v1",
                "run_label": env::var("OMPEROR_PERF_RUN").ok(),
                "executable": env::current_exe().ok(),
                "omp_executable_override": env::var_os("OMPEROR_OMP_EXECUTABLE")
                    .map(PathBuf::from),
            })
        });
    }
    enabled
}

pub(crate) fn event(name: &'static str) {
    event_with(name, || Value::Null);
}

pub(crate) fn event_with(name: &'static str, fields: impl FnOnce() -> Value) {
    let Some(recorder) = recorder() else {
        return;
    };
    recorder.emit("event", name, None, None, &fields());
}

pub(crate) fn span(name: &'static str) -> Span {
    span_with(name, || Value::Null)
}

pub(crate) fn span_with(name: &'static str, fields: impl FnOnce() -> Value) -> Span {
    // Every span joins the main thread's frame-budget ledger, profiling or
    // not: a pass over budget must be able to say what ran in it.
    crate::frame_budget::span_started(name);
    let Some(recorder) = recorder() else {
        return Span { name, id: 0, started: Instant::now(), fields: Value::Null, recorded: false };
    };
    let id = recorder.span_sequence.fetch_add(1, Ordering::Relaxed);
    let fields = fields();
    recorder.emit("span_begin", name, Some(id), None, &fields);
    Span { name, id, started: Instant::now(), fields, recorded: true }
}

#[derive(Debug)]
pub(crate) struct Span {
    name: &'static str,
    id: u64,
    started: Instant,
    fields: Value,
    /// Whether the profiling recorder saw this span begin.
    recorded: bool,
}

impl Drop for Span {
    fn drop(&mut self) {
        let micros = self.started.elapsed().as_micros();
        if self.recorded
            && let Some(recorder) = recorder()
        {
            recorder.emit("span_end", self.name, Some(self.id), Some(micros), &self.fields);
        }
        crate::frame_budget::span_finished(self.name, micros);
    }
}

impl Recorder {
    fn emit(
        &self,
        record: &'static str,
        name: &'static str,
        span_id: Option<u64>,
        duration_us: Option<u128>,
        fields: &Value,
    ) {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let elapsed_us = self.started.elapsed().as_micros();
        let wall_unix_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_micros());
        let thread = std::thread::current();
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        let entry = json!({
            "schema": "omperor.perf.v1",
            "seq": sequence,
            "record": record,
            "name": name,
            "span_id": span_id,
            "elapsed_us": elapsed_us,
            "wall_unix_us": wall_unix_us,
            "duration_us": duration_us,
            "pid": std::process::id(),
            "thread": thread.name().unwrap_or("main"),
            "fields": fields,
            "dropped_records": dropped,
        });
        // A slow disk must never stall the GUI or grow an unbounded queue.
        // Sequence gaps and dropped_records make saturation explicit.
        if self.records.try_send(entry).is_err() {
            self.dropped.fetch_add(dropped + 1, Ordering::Relaxed);
        }
    }
}

fn recorder() -> Option<&'static Recorder> {
    RECORDER
        .get_or_init(|| {
            let path = env::var_os(PERF_LOG_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(default_debug_log_path)?;
            let (records, receiver) = flume::bounded(8192);
            std::thread::Builder::new()
                .name("omperor-perf-writer".into())
                .spawn(move || {
                    let _ = write_records(path, receiver);
                })
                .ok()?;
            Some(Recorder {
                started: Instant::now(),
                sequence: AtomicU64::new(0),
                span_sequence: AtomicU64::new(0),
                records,
                dropped: AtomicU64::new(0),
            })
        })
        .as_ref()
}

/// File opening, serialization and writing all belong to the recorder worker.
fn write_records(path: PathBuf, records: flume::Receiver<Value>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    while let Ok(entry) = records.recv() {
        serde_json::to_writer(&mut file, &entry)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

fn default_debug_log_path() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    Some(crate::app_state::root().ok()?.join("logs/debug-latest.jsonl"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn saturated_recorder_never_blocks_and_reports_dropped_records() {
        use std::sync::atomic::AtomicU64;
        let (records, receiver) = flume::bounded(1);
        let recorder = super::Recorder {
            started: std::time::Instant::now(),
            sequence: AtomicU64::new(0),
            span_sequence: AtomicU64::new(0),
            records,
            dropped: AtomicU64::new(0),
        };
        recorder.emit("event", "first", None, None, &serde_json::Value::Null);
        recorder.emit("event", "dropped", None, None, &serde_json::Value::Null);
        assert_eq!(receiver.recv().unwrap()["name"], "first");
        recorder.emit("event", "third", None, None, &serde_json::Value::Null);
        let third = receiver.recv().unwrap();
        assert_eq!(third["seq"], 2);
        assert_eq!(third["dropped_records"], 1);
        assert_eq!(third["schema"], "omperor.perf.v1");
    }

    #[test]
    fn spans_time_themselves_without_a_profile_path() {
        let span = super::span("gui.test");
        assert_eq!(span.name, "gui.test");
        // Whether the recorder saw it depends on the process, not the span.
        drop(span);
    }

    #[test]
    fn debug_log_path_stays_in_the_selected_state_root() {
        if cfg!(debug_assertions) {
            let path = super::default_debug_log_path().expect("HOME should select a debug log");
            assert_eq!(path, crate::app_state::root().unwrap().join("logs/debug-latest.jsonl"));
        }
    }
}
