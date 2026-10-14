//! macOS filesystem events, owned by a dedicated run-loop thread.
use std::ffi::{CStr, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use super::ChangeFilter;
use objc2_core_foundation::{CFArray, CFRunLoop, CFString, kCFRunLoopDefaultMode};
// Apple deprecated run-loop scheduling in favor of `FSEventStreamSetDispatchQueue`,
// which needs libdispatch bindings this app does not otherwise carry; the
// stream owns a whole thread here, so a run loop is the natural fit.
#[allow(deprecated)]
use objc2_core_services::FSEventStreamScheduleWithRunLoop;
use objc2_core_services::{
    ConstFSEventStreamRef, FSEventStreamContext, FSEventStreamCreate, FSEventStreamEventFlags,
    FSEventStreamEventId, FSEventStreamInvalidate, FSEventStreamRelease, FSEventStreamStart,
    FSEventStreamStop, FSEventsGetCurrentEventId, kFSEventStreamCreateFlagFileEvents,
    kFSEventStreamCreateFlagNoDefer, kFSEventStreamEventFlagItemIsDir,
    kFSEventStreamEventFlagMustScanSubDirs, kFSEventStreamEventFlagRootChanged,
};

/// How long FSEvents may coalesce changes before delivering them.
const LATENCY_SECONDS: f64 = 0.5;

/// How often the watcher thread checks whether it has been asked to stop.
const STOP_POLL_SECONDS: f64 = 0.5;

/// `FSEventStreamEventIdSinceNow`: report only changes after the stream starts.
const SINCE_NOW: FSEventStreamEventId = u64::MAX;

/// The id of the most recent filesystem event system-wide. An inspection that
/// starts now will observe every change with an id at or below it.
pub(crate) fn current_event_id() -> FSEventStreamEventId {
    // SAFETY: a plain query with no arguments or preconditions.
    unsafe { FSEventsGetCurrentEventId() }
}

/// An FSEvents watcher on one project root. Dropping it asks the thread to
/// stop; the thread notices within `STOP_POLL_SECONDS`, tears the stream
/// down, and exits on its own — nothing blocks on it.
#[derive(Debug)]
pub(crate) struct ProjectWatcher {
    root: PathBuf,
    stop: Arc<AtomicBool>,
    filter: Arc<RwLock<ChangeFilter>>,
    covered: Arc<AtomicU64>,
}

impl ProjectWatcher {
    /// Starts watching `root`, sending `root` on `notices` whenever something
    /// relevant under it changes (see `ChangeFilter`). Until `set_filter`
    /// installs the index walk's view, only the root `.gitignore` applies.
    pub(crate) fn start(root: PathBuf, notices: flume::Sender<PathBuf>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let filter = Arc::new(RwLock::new(ChangeFilter::empty(&root)));
        let covered = Arc::new(AtomicU64::new(0));
        let (ready, started) = flume::bounded(1);
        let thread = WatchThread {
            root: root.clone(),
            notices,
            stop: Arc::clone(&stop),
            filter: Arc::clone(&filter),
            covered: Arc::clone(&covered),
            ready,
        };
        std::thread::Builder::new()
            .name("omperor-fsevents".to_owned())
            .spawn(move || thread.run())
            .map_err(|error| format!("could not start the watcher thread: {error}"))?;
        started
            .recv()
            .map_err(|_| "the watcher thread exited before reporting".to_owned())??;
        Ok(Self {
            root,
            stop,
            filter,
            covered,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Replaces how events are judged — with the filter the index walk built
    /// from the ignore files it honored.
    pub(crate) fn set_filter(&self, filter: ChangeFilter) {
        *self.filter.write().unwrap_or_else(PoisonError::into_inner) = filter;
    }

    /// Records that an inspection starting now will see every change up to
    /// `event_id`, so their events, arriving later, need not prompt another.
    pub(crate) fn mark_covered_through(&self, event_id: FSEventStreamEventId) {
        self.covered.fetch_max(event_id, Ordering::AcqRel);
    }
}

impl Drop for ProjectWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// What the stream callback sees: where to report, how to judge events, and
/// the event id through which an inspection has already looked.
struct WatchContext {
    root: PathBuf,
    notices: flume::Sender<PathBuf>,
    filter: Arc<RwLock<ChangeFilter>>,
    covered: Arc<AtomicU64>,
}

struct WatchThread {
    root: PathBuf,
    notices: flume::Sender<PathBuf>,
    stop: Arc<AtomicBool>,
    filter: Arc<RwLock<ChangeFilter>>,
    covered: Arc<AtomicU64>,
    ready: flume::Sender<Result<(), String>>,
}

impl WatchThread {
    fn run(self) {
        let Self {
            root,
            notices,
            stop,
            filter,
            covered,
            ready,
        } = self;
        let Some(run_loop) = CFRunLoop::current() else {
            let _ = ready.send(Err("no run loop on the watcher thread".to_owned()));
            return;
        };
        let Some(mode) = (unsafe { kCFRunLoopDefaultMode }) else {
            let _ = ready.send(Err("no default run loop mode".to_owned()));
            return;
        };
        // Reading the root `.gitignore` here keeps that I/O off the main
        // thread; the walk's fuller view replaces it as soon as it lands.
        *filter.write().unwrap_or_else(PoisonError::into_inner) =
            ChangeFilter::build(&root, std::slice::from_ref(&root));
        let context = Box::into_raw(Box::new(WatchContext {
            root: root.clone(),
            notices,
            filter,
            covered,
        }));
        let mut stream_context = FSEventStreamContext {
            version: 0,
            info: context.cast::<c_void>(),
            retain: None,
            release: None,
            copyDescription: None,
        };
        let paths = CFArray::from_retained_objects(&[CFString::from_str(&root.to_string_lossy())]);
        // SAFETY: `stream_context` outlives the call (FSEvents copies it), the
        // callback matches `FSEventStreamCallback`, and `context` stays alive
        // until the stream is invalidated and released below.
        let stream = unsafe {
            FSEventStreamCreate(
                None,
                Some(stream_callback),
                &raw mut stream_context,
                paths.as_opaque(),
                SINCE_NOW,
                LATENCY_SECONDS,
                kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer,
            )
        };
        if stream.is_null() {
            // SAFETY: `context` came from `Box::into_raw` above and nothing
            // else holds it once stream creation fails.
            drop(unsafe { Box::from_raw(context) });
            let _ = ready.send(Err(format!(
                "FSEventStreamCreate failed for {}",
                root.display()
            )));
            return;
        }
        // SAFETY: the stream is valid and this thread owns the run loop it is
        // scheduled on.
        #[allow(deprecated)]
        unsafe {
            FSEventStreamScheduleWithRunLoop(stream, &run_loop, mode);
        }
        // SAFETY: the stream is scheduled and not yet started.
        if !unsafe { FSEventStreamStart(stream) } {
            // SAFETY: invalidating unschedules the stream; releasing frees it;
            // the context is then unreachable, so freeing it is sound.
            unsafe {
                FSEventStreamInvalidate(stream);
                FSEventStreamRelease(stream);
                drop(Box::from_raw(context));
            }
            let _ = ready.send(Err(format!(
                "FSEventStreamStart failed for {}",
                root.display()
            )));
            return;
        }
        let _ = ready.send(Ok(()));
        while !stop.load(Ordering::Acquire) {
            CFRunLoop::run_in_mode(Some(mode), STOP_POLL_SECONDS, false);
        }
        // SAFETY: stopping and invalidating on the scheduling thread is the
        // documented teardown; after release no callback can run, so the
        // context can be freed.
        unsafe {
            FSEventStreamStop(stream);
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
            drop(Box::from_raw(context));
        }
    }
}

unsafe extern "C-unwind" fn stream_callback(
    _stream: ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: NonNull<c_void>,
    flags: NonNull<FSEventStreamEventFlags>,
    ids: NonNull<FSEventStreamEventId>,
) {
    // SAFETY: `info` is the `WatchContext` installed at stream creation and
    // stays alive until after the stream is released; without
    // `kFSEventStreamCreateFlagUseCFTypes`, `paths` is an array of `count`
    // C strings, and `flags` and `ids` are arrays of `count` entries.
    let context = unsafe { &*info.cast::<WatchContext>() };
    let paths = paths.as_ptr().cast::<*const c_char>();
    let covered = context.covered.load(Ordering::Acquire);
    let filter = context
        .filter
        .read()
        .unwrap_or_else(PoisonError::into_inner);
    let events = (0..count).map(|index| {
        let id = unsafe { *ids.as_ptr().add(index) };
        let flag = unsafe { *flags.as_ptr().add(index) };
        let path = unsafe { CStr::from_ptr(*paths.add(index)) };
        (
            id,
            flag,
            Path::new(std::ffi::OsStr::from_bytes(path.to_bytes())),
        )
    });
    if events_need_refresh(covered, &filter, events) {
        let _ = context.notices.send(context.root.clone());
    }
}

/// Whether a batch of events — `(id, flags, absolute path)` — warrants a
/// refresh: an event an inspection has already covered (id at or below
/// `covered`) never does; a lost-track event (dropped events, or the root
/// itself moved) always does; otherwise the filter judges the path.
pub(super) fn events_need_refresh<'a>(
    covered: FSEventStreamEventId,
    filter: &ChangeFilter,
    events: impl IntoIterator<Item = (FSEventStreamEventId, FSEventStreamEventFlags, &'a Path)>,
) -> bool {
    events.into_iter().any(|(id, flags, path)| {
        id > covered
            && (flags
                & (kFSEventStreamEventFlagMustScanSubDirs | kFSEventStreamEventFlagRootChanged)
                != 0
                || filter.is_relevant(path, flags & kFSEventStreamEventFlagItemIsDir != 0))
    })
}
