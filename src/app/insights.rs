//! What looks back over sessions: a session's Timeline (every prompt,
//! response and tool call on one clock) and its Usage (tokens, cache and
//! requests from the receipts), each in a window of its own, and
//! Settings' Metrics page over the metrics log. Each is read and folded on
//! a worker; the windows show what has arrived and that more is coming.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aizome_kit::prelude::*;

use super::{App, Msg};
use crate::live_sessions::SessionKey;
use crate::{metrics, timeline, usage};

/// How many of the slowest events "Next Slowest" cycles through.
const SLOWEST: usize = 5;

/// The Timeline window and what it shows. One at a time: asking for
/// another session's timeline shows it in the same window.
pub(crate) struct TimelineWindow {
    pub(crate) window: WindowKey,
    pub(crate) title: String,
    generation: u64,
    /// Cleared when the window closes or shows another session, which
    /// stops the reader at its next page.
    listening: Arc<AtomicBool>,
    pub(crate) loaded: Option<timeline::Loaded>,
    pub(crate) complete: bool,
    pub(crate) error: Option<String>,
    pub(crate) filter: timeline::Filter,
    /// The minimum duration as typed; the filter holds it once it reads.
    pub(crate) min_text: String,
    pub(crate) open_runs: HashSet<usize>,
    /// The selected event, by its index in the whole session.
    pub(crate) selected: Option<usize>,
    /// The rows as laid out for the filter, made again when it or the
    /// events change.
    pub(crate) rows: Rc<Vec<timeline::Row>>,
}

impl TimelineWindow {
    fn relayout(&mut self) {
        self.rows = Rc::new(match &self.loaded {
            Some(loaded) => timeline::rows(loaded, &self.filter, &self.open_runs),
            None => Vec::new(),
        });
    }

    pub(crate) fn min_invalid(&self) -> bool {
        timeline::parse_duration_ms(&self.min_text).is_none()
    }

    /// The row the selected event sits in, if it shows.
    pub(crate) fn selected_row(&self) -> Option<usize> {
        let selected = self.selected?;
        self.rows.iter().position(|row| matches!(row, timeline::Row::Event { index, .. } if *index == selected))
    }
}

/// The Usage window: one session's receipts, read again on request.
pub(crate) struct UsageWindow {
    pub(crate) window: WindowKey,
    pub(crate) key: SessionKey,
    pub(crate) title: String,
    pub(crate) page: usize,
    pub(crate) report: Option<Arc<usage::Report>>,
    pub(crate) loading: bool,
    generation: u64,
}

/// What Settings' Metrics page has read from the log.
#[derive(Default)]
pub(crate) struct MetricsPage {
    pub(crate) records: Option<Arc<Vec<metrics::Record>>>,
    /// The sessions the log knows, newest activity first.
    pub(crate) sessions: Vec<metrics::SessionActivity>,
    /// The session the figures are for; `None` for all of them.
    pub(crate) scope: Option<String>,
    pub(crate) summary: Option<Arc<metrics::Summary>>,
    pub(crate) loading: bool,
    generation: u64,
}

/// A read of the metrics log: its records, its sessions, the scope chosen
/// and that scope's figures.
#[derive(Debug)]
pub(crate) struct MetricsRead {
    records: Arc<Vec<metrics::Record>>,
    sessions: Vec<metrics::SessionActivity>,
    scope: Option<String>,
    summary: metrics::Summary,
}

impl App {
    // The Timeline.

    /// Show the timeline of session `id`, or of the session in view.
    pub(super) fn show_timeline(&mut self, id: Option<String>, cx: &mut Cx<Msg>) {
        let found = match &id {
            Some(id) => self.sessions.iter().find(|s| &s.id == id).map(|s| (s.title.clone(), s.path.clone())),
            None => self.journal_in_view().map(|path| {
                let title = self.sessions.iter().find(|s| s.path == path).map(|s| s.title.clone());
                (title.unwrap_or_else(|| "This session".to_owned()), path)
            }),
        };
        let Some((title, path)) = found.filter(|(_, path)| crate::journal::names_journal(path)) else {
            self.note("This session has no journal yet: its timeline starts with its first prompt", cx);
            return;
        };
        let window_title = format!("Timeline — {title}");
        let window = match self.timeline.take() {
            Some(previous) => {
                previous.listening.store(false, Ordering::Relaxed);
                cx.set_title(previous.window, window_title);
                cx.activate(previous.window);
                previous.window
            }
            None => cx.open_window(WindowSpec::new(window_title).size(1000.0, 680.0).min_size(680.0, 420.0)),
        };
        self.next_generation += 1;
        let generation = self.next_generation;
        let listening = Arc::new(AtomicBool::new(true));
        self.timeline = Some(TimelineWindow {
            window,
            title,
            generation,
            listening: listening.clone(),
            loaded: None,
            complete: false,
            error: None,
            filter: timeline::Filter::default(),
            min_text: String::new(),
            open_runs: HashSet::new(),
            selected: None,
            rows: Rc::new(Vec::new()),
        });
        let sender = cx.stream();
        let started = std::thread::Builder::new().name("omperor-timeline".into()).spawn(move || {
            timeline::stream(&path, |update| {
                listening.load(Ordering::Relaxed) && sender.send(Msg::TimelineUpdate(generation, Arc::new(update)))
            });
        });
        if started.is_err()
            && let Some(timeline) = &mut self.timeline
        {
            timeline.error = Some("could not start the timeline reader".into());
            timeline.complete = true;
        }
    }

    /// The journal of the session in view, if it has one yet.
    pub(crate) fn journal_in_view(&self) -> Option<PathBuf> {
        let session = self.session();
        let header = session.header_session.borrow().as_ref().map(|(_, path)| path.clone());
        header.or_else(|| session.selected_session_path.borrow().clone())
    }

    pub(super) fn timeline_update(&mut self, generation: u64, update: Arc<timeline::Update>) {
        let Some(window) = self.timeline.as_mut().filter(|t| t.generation == generation) else { return };
        match Arc::try_unwrap(update).unwrap_or_else(|shared| (*shared).clone()) {
            timeline::Update::Newest { summary, events } => window.loaded = Some(timeline::Loaded::new(summary, events)),
            timeline::Update::Older(events) => {
                if let Some(loaded) = &mut window.loaded {
                    loaded.prepend(events);
                }
            }
            timeline::Update::Complete => window.complete = true,
            timeline::Update::Failed(error) => {
                window.error = Some(error);
                window.complete = true;
            }
        }
        window.relayout();
    }

    pub(super) fn timeline_filter(&mut self, change: TimelineChange) {
        let Some(window) = &mut self.timeline else { return };
        let models = window.loaded.as_ref().map(|l| l.summary.models.clone()).unwrap_or_default();
        match change {
            TimelineChange::Query(query) => window.filter.query = query,
            TimelineChange::Min(text) => {
                if let Some(ms) = timeline::parse_duration_ms(&text) {
                    window.filter.min_ms = ms;
                }
                window.min_text = text;
            }
            TimelineChange::Kind(index) => {
                window.filter.kind = match index {
                    1 => Some(timeline::Kind::Prompt),
                    2 => Some(timeline::Kind::Response),
                    3 => Some(timeline::Kind::Tool),
                    4 => Some(timeline::Kind::Notice),
                    _ => None,
                }
            }
            TimelineChange::Model(index) => window.filter.model = index.checked_sub(1).and_then(|i| models.get(i).cloned()),
            TimelineChange::Clear => {
                window.filter = timeline::Filter::default();
                window.min_text.clear();
            }
        }
        window.relayout();
    }

    /// A row was picked: an event opens its detail; a run of hidden events
    /// opens or folds.
    pub(super) fn timeline_pick(&mut self, row: Option<usize>) {
        let Some(window) = &mut self.timeline else { return };
        match row.and_then(|row| window.rows.get(row).cloned()) {
            Some(timeline::Row::Event { index, .. }) => window.selected = Some(index),
            Some(timeline::Row::Hidden { first, count, .. }) => {
                let key = timeline::Row::run_key(first, count);
                if !window.open_runs.remove(&key) {
                    window.open_runs.insert(key);
                }
                window.relayout();
            }
            None => window.selected = None,
        }
    }

    /// Select the next of the slowest events shown, longest first.
    pub(super) fn timeline_slowest(&mut self) {
        let Some(window) = &mut self.timeline else { return };
        let Some(loaded) = &window.loaded else { return };
        let mut shown: Vec<(usize, u64)> = window
            .rows
            .iter()
            .filter_map(|row| match row {
                timeline::Row::Event { index, dimmed: false } => loaded.get(*index).map(|e| (*index, e.duration_ms())),
                _ => None,
            })
            .filter(|(_, duration)| *duration > 0)
            .collect();
        shown.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        shown.truncate(SLOWEST);
        let next = window
            .selected
            .and_then(|selected| shown.iter().position(|(index, _)| *index == selected))
            .map_or(0, |at| (at + 1) % shown.len().max(1));
        window.selected = shown.get(next).map(|(index, _)| *index);
    }

    pub(super) fn close_timeline(&mut self) {
        if let Some(window) = self.timeline.take() {
            window.listening.store(false, Ordering::Relaxed);
        }
    }

    // Usage.

    /// Show the Usage window for the session in view, and read its
    /// receipts.
    pub(super) fn show_usage(&mut self, cx: &mut Cx<Msg>) {
        let session = self.session();
        let title = self
            .journal_in_view()
            .and_then(|path| self.sessions.iter().find(|s| s.path == path).map(|s| s.title.clone()))
            .unwrap_or_else(|| "This session".to_owned());
        let window_title = format!("Usage — {title}");
        let window = match self.usage_window.take() {
            Some(previous) => {
                cx.set_title(previous.window, window_title);
                cx.activate(previous.window);
                previous.window
            }
            None => cx.open_window(WindowSpec::new(window_title).size(900.0, 680.0).min_size(620.0, 420.0)),
        };
        self.usage_window = Some(UsageWindow { window, key: session.key, title, page: 0, report: None, loading: false, generation: 0 });
        self.read_usage(cx);
    }

    pub(super) fn read_usage(&mut self, cx: &mut Cx<Msg>) {
        let Some(key) = self.usage_window.as_ref().map(|w| w.key) else { return };
        let Some(session) = self.live.get(key) else { return };
        // A session opened from the sidebar has its journal before its agent.
        let path = session
            .header_session
            .borrow()
            .as_ref()
            .map(|(_, path)| path.clone())
            .or_else(|| session.selected_session_path.borrow().clone())
            .filter(|path| crate::journal::names_journal(path));
        let children: Vec<PathBuf> = session.subagents.borrow().iter().filter_map(|a| a.session_file.clone()).collect();
        let model = session.current_model.borrow().clone();
        self.next_generation += 1;
        let generation = self.next_generation;
        if let Some(window) = &mut self.usage_window {
            window.loading = true;
            window.generation = generation;
        }
        cx.spawn(move || Msg::UsageReport(generation, Arc::new(usage::read(path, children, model))));
    }

    pub(super) fn usage_report(&mut self, generation: u64, report: Arc<usage::Report>) {
        if let Some(window) = self.usage_window.as_mut().filter(|w| w.generation == generation) {
            window.report = Some(report);
            window.loading = false;
        }
    }

    // Metrics.

    /// Read the metrics log again, keeping the scope chosen (or choosing
    /// the session in view when the log knows it).
    pub(super) fn load_metrics(&mut self, cx: &mut Cx<Msg>) {
        let Some(dir) = self.sessions_dir.clone() else { return };
        let chosen = self.metrics_page.records.is_some().then(|| self.metrics_page.scope.clone());
        let in_view = self.session().live_session_id.borrow().clone();
        self.next_generation += 1;
        let generation = self.next_generation;
        self.metrics_page.generation = generation;
        self.metrics_page.loading = true;
        cx.spawn(move || {
            let records = metrics::read(&dir.join(metrics::FILE_NAME));
            let sessions = metrics::sessions(&records);
            let scope = chosen.unwrap_or_else(|| in_view.filter(|id| sessions.iter().any(|s| &s.id == id)));
            let summary = metrics::summarize(&records, scope.as_deref());
            Msg::MetricsRead(generation, Arc::new(MetricsRead { records: Arc::new(records), sessions, scope, summary }))
        });
    }

    pub(super) fn metrics_read(&mut self, generation: u64, read: Arc<MetricsRead>) {
        let page = &mut self.metrics_page;
        if page.generation != generation {
            return;
        }
        page.loading = false;
        page.records = Some(read.records.clone());
        page.sessions = read.sessions.clone();
        page.scope = read.scope.clone();
        page.summary = Some(Arc::new(read.summary.clone()));
    }

    /// Choose the page's scope: 0 for every session, else the log's
    /// sessions in order.
    pub(super) fn metrics_scope(&mut self, index: usize, cx: &mut Cx<Msg>) {
        let Some(records) = self.metrics_page.records.clone() else { return };
        let scope = index.checked_sub(1).and_then(|i| self.metrics_page.sessions.get(i)).map(|s| s.id.clone());
        self.next_generation += 1;
        let generation = self.next_generation;
        let page = &mut self.metrics_page;
        page.generation = generation;
        page.loading = true;
        page.scope = scope.clone();
        let sessions = page.sessions.clone();
        cx.spawn(move || {
            let summary = metrics::summarize(&records, scope.as_deref());
            Msg::MetricsRead(generation, Arc::new(MetricsRead { records, sessions, scope, summary }))
        });
    }
}

/// A change to the timeline's filter.
#[derive(Debug, Clone)]
pub(crate) enum TimelineChange {
    Query(String),
    Min(String),
    Kind(usize),
    Model(usize),
    Clear,
}
