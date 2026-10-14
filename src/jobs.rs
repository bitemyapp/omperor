//! A small pool of worker threads for work that must never run on the AppKit
//! main thread: subprocesses, filesystem walks, anything that can take longer
//! than a frame. Each job produces one event; the main thread drains events
//! when notified and updates the interface.

use std::thread;

use crate::perf;

type Work<E> = Box<dyn FnOnce() -> E + Send + 'static>;

#[derive(Debug)]
pub(crate) struct JobPool<E> {
    queue: flume::Sender<Work<E>>,
    results: flume::Receiver<E>,
}

impl<E: Send + 'static> JobPool<E> {
    /// Starts `workers` threads that run submitted jobs in submission order
    /// across the pool. Workers exit once the pool is dropped and the queue
    /// drains.
    #[cfg(test)]
    pub(crate) fn start(workers: usize, name: &str) -> Self {
        Self::start_notifying(workers, name, || {})
    }

    /// The callback runs after publishing each result. It must only schedule a
    /// main-thread delivery, never touch UI or block a worker awaiting the UI.
    pub(crate) fn start_notifying(
        workers: usize,
        name: &str,
        ready: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let ready = std::sync::Arc::new(ready);
        let (queue, jobs) = flume::unbounded::<Work<E>>();
        let (events, results) = flume::unbounded::<E>();
        for index in 0..workers.max(1) {
            let jobs = jobs.clone();
            let events = events.clone();
            let ready = ready.clone();
            thread::Builder::new()
                .name(format!("{name}-{index}"))
                .spawn(move || {
                    while let Ok(work) = jobs.recv() {
                        let _span = perf::span("gui.job.run");
                        debug_job_delay();
                        let event = work();
                        if events.send(event).is_err() {
                            return;
                        }
                        ready();
                    }
                })
                .expect("background job worker must start");
        }
        Self { queue, results }
    }

    /// Queues `work`; returns `false` only when every worker has stopped.
    pub(crate) fn submit(&self, work: impl FnOnce() -> E + Send + 'static) -> bool {
        perf::event("gui.job.submitted");
        self.queue.send(Box::new(work)).is_ok()
    }

    /// Takes one finished job's event without waiting.
    pub(crate) fn try_next(&self) -> Option<E> {
        self.results.try_recv().ok()
    }
}

/// Debug builds honour `OMPEROR_JOB_DELAY_MS` so the interface's activity
/// state can be inspected for jobs that would otherwise finish within a
/// frame. Release builds never delay.
fn debug_job_delay() {
    #[cfg(debug_assertions)]
    if let Some(delay) = std::env::var("OMPEROR_JOB_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|delay| *delay > 0)
    {
        thread::sleep(std::time::Duration::from_millis(delay));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::JobPool;

    #[test]
    fn completion_notification_follows_publication_and_preserves_result_order() {
        let (ready, signals) = flume::unbounded();
        let pool = JobPool::<u32>::start_notifying(1, "omperor-notify-test", move || {
            ready.send(()).unwrap();
        });
        for value in 0..100 {
            assert!(pool.submit(move || value));
        }
        for expected in 0..100 {
            signals.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(pool.try_next(), Some(expected));
        }
        assert!(pool.try_next().is_none());
        assert!(signals.try_recv().is_err());
    }

    #[test]
    fn jobs_run_off_the_calling_thread_and_report_back_in_order_per_worker() {
        let pool = JobPool::<(u32, String)>::start(1, "omperor-test-jobs");
        let caller = std::thread::current().id();
        for index in 0..3u32 {
            assert!(
                pool.submit(move || { (index, format!("{:?}", std::thread::current().id()),) })
            );
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut received = Vec::new();
        while received.len() < 3 && Instant::now() < deadline {
            match pool.try_next() {
                Some(event) => received.push(event),
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        assert_eq!(
            received.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(
            received
                .iter()
                .all(|(_, thread)| thread != &format!("{caller:?}"))
        );
        assert!(pool.try_next().is_none());
    }
}
