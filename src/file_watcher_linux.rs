//! inotify registration, filtering and teardown stay off the GUI thread.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};

use super::ChangeFilter;

static EVENT_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn current_event_id() -> u64 {
    EVENT_ID.load(Ordering::Acquire)
}

#[derive(Debug)]
pub(crate) struct ProjectWatcher {
    root: PathBuf,
    stop: Arc<AtomicBool>,
    filters: flume::Sender<ChangeFilter>,
    covered: Arc<AtomicU64>,
}

impl ProjectWatcher {
    pub(crate) fn start(root: PathBuf, notices: flume::Sender<PathBuf>) -> Result<Self, String> {
        let (filters, updates) = flume::unbounded();
        let watcher = Self {
            filters,
            root: root.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            covered: Arc::new(AtomicU64::new(0)),
        };
        let stop = Arc::clone(&watcher.stop);
        let covered = Arc::clone(&watcher.covered);
        std::thread::Builder::new()
            .name("omperor-inotify".into())
            .spawn(move || {
                let run = || -> Result<(), String> {
                    let mut filter = ChangeFilter::build(&root, std::slice::from_ref(&root));
                    let (events, received) = flume::unbounded();
                    let mut native =
                        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                            // Access events from our own inspections must not trigger another.
                            if event
                                .as_ref()
                                .is_ok_and(|event| matches!(event.kind, EventKind::Access(_)))
                            {
                                return;
                            }
                            let id = EVENT_ID.fetch_add(1, Ordering::AcqRel) + 1;
                            let _ = events.send((id, event));
                        })
                        .map_err(|error| error.to_string())?;
                    native
                        .watch(&root, RecursiveMode::Recursive)
                        .map_err(|error| error.to_string())?;
                    while !stop.load(Ordering::Acquire) {
                        let Ok((id, event)) = received.recv_timeout(Duration::from_millis(250))
                        else {
                            continue;
                        };
                        if id <= covered.load(Ordering::Acquire) {
                            continue;
                        }
                        while let Ok(updated) = updates.try_recv() {
                            filter = updated;
                        }
                        let refresh = match event {
                            Ok(event) => {
                                event.need_rescan()
                                    || event
                                        .paths
                                        .iter()
                                        .any(|path| filter.is_relevant(path, path.is_dir()))
                            }
                            Err(_) => true,
                        };
                        if refresh {
                            let _ = notices.send(root.clone());
                            crate::gui_wake::notify();
                        }
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    crate::perf::event_with(
                        "gui.watcher.unavailable",
                        || serde_json::json!({ "error": error }),
                    );
                }
            })
            .map_err(|error| format!("could not start the watcher thread: {error}"))?;
        Ok(watcher)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn set_filter(&self, filter: ChangeFilter) {
        let _ = self.filters.send(filter);
    }

    pub(crate) fn mark_covered_through(&self, event_id: u64) {
        self.covered.fetch_max(event_id, Ordering::AcqRel);
    }
}

impl Drop for ProjectWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn watches_edits_and_filters_ignored_files_and_git_index() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("omperor-inotify-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("ignored-output")).unwrap();
        std::fs::write(root.join(".gitignore"), "ignored-output/\n").unwrap();
        std::fs::write(root.join(".git/index"), "before").unwrap();
        let (notices, received) = flume::unbounded();
        let watcher = ProjectWatcher::start(root.clone(), notices).unwrap();
        // Registration is asynchronous. Keep editing until inotify is ready.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            std::fs::write(root.join("source.rs"), "edit").unwrap();
            if received.recv_timeout(Duration::from_millis(30)).is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "watcher did not report an edit");
        }
        watcher.set_filter(ChangeFilter::build(&root, std::slice::from_ref(&root)));
        std::thread::sleep(Duration::from_millis(100));
        while received.try_recv().is_ok() {}
        watcher.mark_covered_through(current_event_id());
        std::fs::write(root.join("ignored-output/artifact"), "ignored").unwrap();
        std::fs::write(root.join(".git/index"), "inspection").unwrap();
        assert!(received.recv_timeout(Duration::from_millis(300)).is_err());
        std::fs::write(root.join("source.rs"), "another edit").unwrap();
        assert_eq!(received.recv_timeout(Duration::from_secs(3)).unwrap(), root);
        drop(watcher);
        std::fs::remove_dir_all(root).unwrap();
    }
}
