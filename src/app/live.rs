//! Live sessions: launching each one's OMP child, draining what its
//! runtime reports, and applying it; the transcript kept in step.

use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use aizome_kit::Cx;

use super::{App, DRAIN_BUDGET, Msg, Row};
use crate::chat::TranscriptMessageKind;
use crate::live_sessions::{Lifecycle, LiveSession, SessionHistoryLoad, SessionKey};
use crate::markdown::picture::{Picture, Source};
use crate::markdown::{self, Block, Doc};
use crate::omp_runtime::{self, AuthProvider, OmpRuntime, TurnOutcome, UiEvent};
use crate::preferences::{self, SessionRecord};
use crate::rules::{AccountResolution, resolve_provider_account};
use crate::session::{self, DiscoveryUpdate, HistoryUpdate};
use crate::{agents, mock_inference};

fn body_hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Whether a row's body is Markdown to parse.
fn parsed_kind(kind: TranscriptMessageKind) -> bool {
    matches!(kind, TranscriptMessageKind::User | TranscriptMessageKind::Assistant | TranscriptMessageKind::Thinking)
}

impl App {
    /// Start `session`'s OMP child, as `target` says.
    pub(crate) fn launch_runtime(
        &mut self,
        session: &LiveSession,
        account: Option<String>,
        target: omp_runtime::SessionStartup,
    ) {
        if session.omp_runtime.get().is_some() || self.quitting {
            return;
        }
        let (Some(data_dir), Some(session_dir)) = (self.data_dir.clone(), self.sessions_dir.clone()) else {
            return;
        };
        if let omp_runtime::SessionStartup::Resume(id) = &target {
            session.resuming_session_id.replace(Some(id.clone()));
        }
        session.runtime_starting.set(true);
        let backend = self.preferences.effective_backend();
        session.backend.set(backend);
        let model = {
            let model = session.current_model.borrow();
            if model.trim().is_empty() {
                omp_runtime::sanitized_model_preference(self.preferences.model.as_deref())
            } else {
                omp_runtime::sanitized_model_preference(Some(&model))
            }
        };
        let thinking = session.current_thinking_level.borrow().clone();
        let startup = omp_runtime::RuntimeStartup {
            helper: None,
            backend,
            project: session.current_project.borrow().clone(),
            model,
            account: account.or_else(|| session.current_account.borrow().clone()),
            thinking_level: (!thinking.is_empty()).then_some(thinking),
            fast_mode: session.fast_mode.get(),
            omperor_tools: session.omperor_tools.get(),
            computer_use: self.preferences.computer_use,
            data_dir,
            session_dir,
            environment: vec![],
            session: target,
        };
        let _ = session.omp_runtime.set(OmpRuntime::start_configured(startup));
    }

    /// Take in what the workers and every session's runtime have
    /// delivered, within a budget; what's left waits for the next frame.
    pub(crate) fn drain(&mut self, cx: &mut Cx<Msg>) {
        self.poll_discovery(cx);
        self.poll_history(cx);
        self.poll_watchers(cx);
        let started = Instant::now();
        let mut backlogged = false;
        let count = self.live.len();
        let mut idle = 0;
        while idle < count {
            let Some(session) = self.live.next_poll() else { break };
            let mut received = 0;
            if let Some(runtime) = session.omp_runtime.get() {
                while received < 16 {
                    let Some((arrived, event)) = runtime.try_event() else { break };
                    self.apply_event(&session, event, arrived, cx);
                    received += 1;
                    if started.elapsed() >= DRAIN_BUDGET {
                        break;
                    }
                }
            }
            if received == 0 {
                idle += 1;
            } else {
                idle = 0;
            }
            if started.elapsed() >= DRAIN_BUDGET {
                backlogged = true;
                break;
            }
        }
        if backlogged {
            // Come back for the rest once this frame is drawn.
            cx.send(Msg::Frame);
        }
        self.sync_subagents(cx);
        self.retire_runtimes();
    }

    /// Drop the runtimes whose children have exited for good.
    fn retire_runtimes(&mut self) {
        for session in self.live.entries() {
            if session.lifecycle.get() == Lifecycle::Retiring
                && session.omp_runtime.get().is_some_and(OmpRuntime::try_reap)
            {
                session.lifecycle.set(Lifecycle::History);
            }
        }
    }

    pub(crate) fn start_discovery(&mut self) {
        let (Some(sessions), Some(data)) = (self.sessions_dir.clone(), self.data_dir.clone()) else { return };
        self.discovering = true;
        self.discovery_error = None;
        let (sender, receiver) = flume::bounded(2);
        self.discovery = Some(receiver);
        let spawned = std::thread::Builder::new().name("omperor-session-discovery".into()).spawn(move || {
            session::stream_discovery(&sessions, &data, sender);
            crate::wake::notify();
        });
        if spawned.is_err() {
            self.discovering = false;
            self.discovery_error = Some("could not start the session reader".into());
        }
    }

    fn poll_discovery(&mut self, cx: &mut Cx<Msg>) {
        loop {
            let Some(receiver) = &self.discovery else { return };
            let update = match receiver.try_recv() {
                Ok(update) => update,
                Err(flume::TryRecvError::Empty) => return,
                Err(flume::TryRecvError::Disconnected) => {
                    self.discovery = None;
                    self.discovering = false;
                    return;
                }
            };
            let (owned, result) = match update {
                DiscoveryUpdate::Omperor(result) => (true, result),
                DiscoveryUpdate::Omp(result) => {
                    self.discovery = None;
                    self.discovering = false;
                    (false, result)
                }
            };
            match result {
                Ok(mut found) => {
                    found.retain(|row| !self.removed_session_ids.contains(&row.id));
                    // Live sessions' rows (their titles and models) win over
                    // a reading that started before they changed.
                    let mut others: Vec<_> =
                        std::mem::take(&mut self.sessions).into_iter().filter(|s| s.is_omperor_owned() != owned).collect();
                    if owned {
                        found.append(&mut others);
                        self.sessions = found;
                    } else {
                        others.append(&mut found);
                        self.sessions = others;
                    }
                    if owned && self.bootstrap_pending {
                        self.bootstrap_pending = false;
                        let sessions = self.sessions.clone();
                        let job = self.begin_job("Filing sessions under their projects…");
                        cx.spawn(move || {
                            let result = crate::app::boot::bootstrap_sessions(&sessions)
                                .and_then(|()| preferences::load_registry())
                                .map_err(|e| e.to_string());
                            Msg::RegistryReloaded(Some(job), Arc::new(result))
                        });
                    }
                }
                Err(error) => self.discovery_error = Some(error.to_string()),
            }
        }
    }

    fn poll_history(&mut self, cx: &mut Cx<Msg>) {
        let _ = cx;
        for session in self.live.entries() {
            loop {
                let update = {
                    let history = session.session_history.borrow();
                    let Some(history) = history.as_ref() else { break };
                    match history.receiver.try_recv() {
                        Ok(update) => update,
                        Err(_) => break,
                    }
                };
                match update {
                    HistoryUpdate::Page(messages) => {
                        let first = {
                            let mut history = session.session_history.borrow_mut();
                            let Some(history) = history.as_mut() else { break };
                            let first = history.message_count == 0;
                            history.message_count += messages.len();
                            first
                        };
                        if first {
                            session.prompt_history.borrow_mut().replace_from_messages(&messages);
                            session.conversation.borrow_mut().load_history(&messages);
                        } else {
                            session.prompt_history.borrow_mut().prepend_from_messages(&messages);
                            session.conversation.borrow_mut().prepend_history(&messages);
                        }
                    }
                    HistoryUpdate::Complete => {
                        session.session_history.borrow_mut().take();
                    }
                    HistoryUpdate::Failed(error) => {
                        session.session_history.borrow_mut().take();
                        session.conversation.borrow_mut().notice(format!("History unavailable: {error}"), true);
                    }
                }
            }
        }
    }

    /// Read `summary`'s recorded history into `session`, a page at a time.
    pub(crate) fn load_history(&mut self, session: &LiveSession, summary: session::SessionSummary, read_only: bool) {
        session.session_history.borrow_mut().take();
        session.prompt_history.borrow_mut().clear();
        session.subagents.borrow_mut().clear();
        session.read_only_session.set(read_only);
        session.history_prepend_pending.set(false);
        session.chat_image_base.replace(summary.cwd.clone().or_else(|| Some(session.current_project.borrow().clone())));
        session.header_session.replace(Some((summary.id.clone(), summary.path.clone())));
        session.conversation.borrow_mut().reset("Loading the most recent messages…");
        session.chat_status_text.replace(if read_only {
            "Read-only OMP session • loading recent messages…".into()
        } else {
            "Loading recent messages…".into()
        });
        let (sender, receiver) = flume::bounded(1);
        session.session_history.replace(Some(SessionHistoryLoad { receiver, message_count: 0 }));
        let _ = std::thread::Builder::new().name("omperor-session-history".into()).spawn(move || {
            // Each page wakes the interface; the reader waits while it's full.
            let (tx, rx) = flume::bounded::<HistoryUpdate>(1);
            let pump = std::thread::spawn(move || session::stream_history(summary, tx));
            for update in rx {
                if sender.send(update).is_err() {
                    break;
                }
                crate::wake::notify();
            }
            let _ = pump.join();
        });
    }

    /// Apply one event `session`'s runtime received at `arrived`.
    pub(crate) fn apply_event(&mut self, session: &Rc<LiveSession>, event: UiEvent, arrived: Instant, cx: &mut Cx<Msg>) {
        let event = match event {
            // The agent's calls to Omperor's own tools are answered by the
            // app, never shown as transcript events.
            UiEvent::HostToolCall { call, tool, arguments } => {
                self.host_tool_call(session, call, &tool, &arguments, cx);
                return;
            }
            UiEvent::HostToolCancelled { call } => {
                self.host_tool_cancelled(session.key, call);
                return;
            }
            UiEvent::TurnFinished(TurnOutcome::Errored(reason)) => {
                UiEvent::TurnFinished(TurnOutcome::Errored(crate::rules::name_accounts_in(&reason, &self.accounts)))
            }
            event => event,
        };
        let previous_path = session.selected_session_path.borrow().clone();
        // Validate identity before Ready can move the session's saved view.
        let event = if let UiEvent::Ready { model, session_id, session_file: Some(path), resumed, .. } = &event {
            let backend_matches = !session.backend_switch_pending.get()
                || (model == mock_inference::MODEL) == (session.backend.get() == mock_inference::Backend::Mock);
            let resume_matches = session.resuming_session_id.borrow().as_deref().is_none_or(|expected| {
                resumed.as_deref() == Some(expected) && session_id.as_deref() == Some(expected)
            });
            if backend_matches && resume_matches {
                if let Err(existing) = self.live.bind_journal(session.key, path.clone()) {
                    if let Some(runtime) = session.omp_runtime.get() {
                        runtime.begin_shutdown();
                    }
                    session.read_only_session.set(true);
                    UiEvent::Failed(format!(
                        "OMP journal already belongs to session {} • duplicate runtime stopped",
                        existing.0
                    ))
                } else {
                    event
                }
            } else {
                event
            }
        } else {
            event
        };
        let released = matches!(&event, UiEvent::SessionReleased);
        let failed = matches!(&event, UiEvent::Failed(_));
        let effects = session.reduce(event, arrived);
        if effects.ready {
            // The draft's saved arrangement follows it to its journal.
            let next_path = session.selected_session_path.borrow().clone();
            if previous_path != next_path
                && let Some(view) = self.saved_views.remove(&previous_path)
            {
                self.saved_views.insert(next_path, view);
            }
        }
        if effects.rejected {
            return;
        }
        if released
            && let Some((target, parent)) = self.pending_forks.get(&session.key).cloned()
            && let Some(target) = self.live.get(target)
        {
            self.launch_runtime(&target, None, omp_runtime::SessionStartup::Fork(parent));
        }
        if effects.ready || failed {
            let source = self.pending_forks.iter().find_map(|(source, (target, _))| {
                (*target == session.key || (failed && *source == session.key)).then_some(*source)
            });
            if let Some(source) = source
                && let Some((target, _)) = self.pending_forks.remove(&source)
                && failed
                && target != session.key
                && let Some(target) = self.live.get(target)
            {
                target.new_session_pending.set(false);
                target.lifecycle.set(Lifecycle::Failed);
                target.chat_status_text.replace("Source release failed; choose Fork again to retry".into());
            }
        }
        for record in effects.metrics {
            if let Some(log) = self.metrics_log() {
                log.record(record);
            }
        }
        for event in effects.global_events {
            self.apply_global_event(session, event, cx);
        }
        if effects.refresh_subagents
            && let Some(runtime) = session.omp_runtime.get()
        {
            runtime.refresh_subagents();
        }
        if effects.discover_sessions {
            self.start_discovery();
        }
        if effects.refresh_workspace {
            self.inspect_workspace(session, cx);
        }
        if effects.ready {
            self.sessions.retain(|row| row.id != format!("draft-{}", session.key.0));
            if let Some((id, _)) = session.header_session.borrow().clone()
                && let Some(pending) = session.pending_session.borrow_mut().take()
            {
                let record = SessionRecord {
                    session_id: id,
                    project_id: pending.project_id,
                    directory: pending.directory,
                    created_at_ms: preferences::now_ms(),
                    parent_session_id: pending.parent_session_id,
                    branch: pending.branch,
                    base_branch: pending.base_branch,
                    worktree_path: pending.worktree_path,
                };
                self.records.insert(record.session_id.clone(), record.clone());
                self.write(cx, move || preferences::record_session(&record));
            }
            self.resolve_account(session);
            self.request_account_usage(false, cx);
        }
        if effects.dispatch_prompt || effects.ready {
            self.dispatch_prompt(session, cx);
        }
        for (message, primary, provider) in effects.account_trouble {
            self.note_account_trouble(session, &message, primary, provider, cx);
        }
        if let Some(error) = effects.authentication_failure
            && self.sign_in.as_ref().is_some_and(|s| s.owner == session.key && s.busy)
        {
            self.sign_in_failed(&error);
        }
        if let Some(provider_id) = effects.account_recovery
            && let Some(account) = session.current_account.borrow().as_deref().and_then(|id| self.account(id))
            && account.provider().id() == provider_id
        {
            session.account_alerts.borrow_mut().recovered(account.id(), account.provider(), false);
        }
        if effects.sync_subagent_transcripts {
            self.request_transcripts(session, true, cx);
        }
    }

    /// Settle which account the session's provider uses and pin it, or
    /// say a choice is needed.
    pub(crate) fn resolve_account(&mut self, session: &LiveSession) {
        let Some(provider) = AuthProvider::for_model(&session.current_model.borrow()) else { return };
        if !session.accounts_known.get() {
            return;
        }
        let project = self.project_key(session);
        let choice = self.account_choices.get(&(project, provider.id().to_owned())).cloned();
        let current = session.current_account.borrow().clone();
        match resolve_provider_account(&self.accounts, provider, choice.as_deref(), current.as_deref()) {
            AccountResolution::Unavailable => {
                session.current_account.replace(None);
            }
            AccountResolution::Chosen(account) => {
                if current.as_deref() != Some(account.as_str()) {
                    if let Some(runtime) = session.omp_runtime.get() {
                        session.account_change_pending.set(true);
                        if !runtime.set_account(account) {
                            session.account_change_pending.set(false);
                        }
                    } else {
                        // Pin the launch's intent before the child exists.
                        session.current_account.replace(Some(account));
                    }
                }
            }
            AccountResolution::NeedsChoice => {
                session.current_account.replace(None);
                if !session.read_only_session.get() {
                    session
                        .chat_status_text
                        .replace(format!("Choose a {} account to continue", provider.product_name()));
                }
            }
        }
    }

    /// Whether the session's provider has its account settled.
    pub(crate) fn account_resolved(&self, session: &LiveSession) -> bool {
        let Some(provider) = AuthProvider::for_model(&session.current_model.borrow()) else { return true };
        if !session.accounts_known.get() {
            return true;
        }
        if session.account_change_pending.get() {
            return false;
        }
        let usable: Vec<_> = self.accounts.iter().filter(|a| a.provider() == provider && a.is_usable()).collect();
        if usable.is_empty() {
            return true;
        }
        session.current_account.borrow().as_deref().is_some_and(|id| usable.iter().any(|a| a.id() == id))
    }

    /// Send the session's queued prompt once its child is ready, starting
    /// or resuming the child first if it has none.
    pub(crate) fn dispatch_prompt(&mut self, session: &LiveSession, cx: &mut Cx<Msg>) {
        let _ = cx;
        if self.quitting || session.read_only_session.get() || self.fork_owns(session.key) {
            return;
        }
        if session.omp_runtime.get().is_none() && session.pending_prompt.borrow().is_some() {
            self.resolve_account(session);
            if !self.account_resolved(session) {
                return;
            }
            let target = session
                .live_session_id
                .borrow()
                .clone()
                .map_or(omp_runtime::SessionStartup::Fresh, omp_runtime::SessionStartup::Resume);
            let account = session.current_account.borrow().clone();
            self.launch_runtime(session, account, target);
        }
        if !session.runtime_ready.get() && !session.runtime_starting.get() && session.pending_prompt.borrow().is_some()
            && let Some(runtime) = session.omp_runtime.get()
        {
            session.runtime_starting.set(true);
            let started = if let Some(id) = session.live_session_id.borrow().clone() {
                session.resuming_session_id.replace(Some(id.clone()));
                runtime.resume_session(id, session.current_project.borrow().clone())
            } else {
                runtime.retry_startup()
            };
            if !started {
                session.runtime_starting.set(false);
                session.resuming_session_id.borrow_mut().take();
                session.chat_busy.set(false);
                session.pending_prompt.borrow_mut().take();
                session
                    .conversation
                    .borrow_mut()
                    .notice("This runtime has retired; start a new session to continue.", true);
            }
        }
        if !session.runtime_ready.get()
            || session.resuming_session_id.borrow().is_some()
            || session.account_change_pending.get()
        {
            return;
        }
        let Some(prompt) = session.pending_prompt.borrow_mut().take() else { return };
        session.chat_busy.set(true);
        if !session.omp_runtime.get().is_some_and(|runtime| runtime.prompt(prompt)) {
            session.chat_busy.set(false);
            session.conversation.borrow_mut().fail_assistant(
                "OMP stopped before accepting the prompt. Retry explicitly; it will not be replayed automatically.",
            );
        }
    }

    pub(crate) fn fork_owns(&self, key: SessionKey) -> bool {
        self.pending_forks.iter().any(|(source, (target, _))| *source == key || *target == key)
    }

    pub(crate) fn metrics_log(&mut self) -> Option<&crate::metrics::Log> {
        if self.metrics.is_none() {
            let dir = self.sessions_dir.clone()?;
            self.metrics = Some(crate::metrics::Log::open(dir.join(crate::metrics::FILE_NAME)));
        }
        self.metrics.as_ref()
    }

    /// Bring every session's transcript rows up to date with its
    /// conversation, and parse the bodies that changed.
    pub(crate) fn sync_transcripts(&mut self, cx: &mut Cx<Msg>) {
        let keys: Vec<SessionKey> = self.views.keys().copied().collect();
        for key in keys {
            let Some(session) = self.live.get(key) else {
                self.views.remove(&key);
                continue;
            };
            let changes = session.conversation.borrow_mut().take_changes();
            let conversation = session.conversation.borrow();
            let len = conversation.len();
            let view = self.views.get_mut(&key).expect("the key came from the map");
            let changed: Vec<usize> = match changes {
                crate::chat::Changes::All => {
                    view.rows.clear();
                    view.expanded.clear();
                    (0..len).collect()
                }
                crate::chat::Changes::Prepended(count) => {
                    let fresh: Vec<Row> = (0..count).filter_map(|i| conversation.message(i)).map(row).collect();
                    view.rows.splice(0..0, fresh);
                    view.expanded = view.expanded.iter().map(|i| i + count).collect();
                    (0..count).collect()
                }
                crate::chat::Changes::Messages(indices) => indices,
            };
            view.rows.truncate(len);
            for index in changed {
                let Some(message) = conversation.message(index) else { continue };
                let hash = body_hash(&message.body);
                if index < view.rows.len() {
                    let row = &mut view.rows[index];
                    if row.hash != hash || *row.message != message {
                        row.message = Rc::new(message);
                        row.hash = hash;
                    }
                } else {
                    while view.rows.len() < index {
                        let i = view.rows.len();
                        match conversation.message(i) {
                            Some(m) => view.rows.push(row(m)),
                            None => break,
                        }
                    }
                    view.rows.push(row(message));
                }
            }
            drop(conversation);
            // Parse what is new, one parse in flight a row: a row being
            // streamed is parsed again when its last parse comes back.
            let mut wanted = Vec::new();
            for (index, row) in view.rows.iter_mut().enumerate() {
                if parsed_kind(row.message.kind)
                    && !row.parsing
                    && row.doc.as_ref().is_none_or(|(hash, _)| *hash != row.hash)
                {
                    row.parsing = true;
                    wanted.push((index, row.hash, row.message.body.clone()));
                }
            }
            for (index, hash, body) in wanted {
                cx.spawn(move || Msg::Parsed(key, None, index, hash, Arc::new(markdown::parse(&body))));
            }
        }
    }

    pub(crate) fn parsed(
        &mut self,
        key: SessionKey,
        agent: Option<String>,
        index: usize,
        hash: u64,
        doc: Arc<Doc>,
        cx: &mut Cx<Msg>,
    ) {
        let base = self.picture_base(key);
        self.request_pictures(&doc, base.as_deref(), cx);
        let Some(view) = self.views.get_mut(&key) else { return };
        let rows = match &agent {
            Some(id) => match view.agent_rows.get_mut(id) {
                Some(rows) => rows,
                None => return,
            },
            None => &mut view.rows,
        };
        if let Some(row) = rows.get_mut(index) {
            row.parsing = false;
            // A parse of older text still shows until the newer one lands.
            row.doc = Some((hash, doc));
        }
    }

    /// Read the pictures a document shows that aren't read yet, as the
    /// preferences allow: each on a worker, shown as loading meanwhile.
    fn request_pictures(&mut self, doc: &Doc, base: Option<&std::path::Path>, cx: &mut Cx<Msg>) {
        if !self.preferences.show_inline_images {
            return;
        }
        let mut places = Vec::new();
        collect_images(&doc.blocks, &mut places);
        for place in places {
            let source = Source::of(&place, base);
            match self.pictures.get(&source.key()) {
                Some(Picture::Blocked) if self.preferences.load_remote_images => {}
                Some(_) => continue,
                None => {}
            }
            if source.is_web() && !self.preferences.load_remote_images {
                Rc::make_mut(&mut self.pictures).insert(source.key(), Picture::Blocked);
            } else {
                self.load_picture(source, cx);
            }
        }
    }

    /// Read `source` on a worker: from the web too, when the reader asks.
    pub(crate) fn load_picture(&mut self, source: Source, cx: &mut Cx<Msg>) {
        let key = source.key();
        Rc::make_mut(&mut self.pictures).insert(key, Picture::Loading);
        cx.spawn(move || Msg::PictureLoaded(key, source.load()));
    }

    /// Every open transcript's pictures, once the preferences let more show.
    pub(crate) fn request_all_pictures(&mut self, cx: &mut Cx<Msg>) {
        let mut docs = Vec::new();
        for (key, view) in &self.views {
            let base = self.picture_base(*key);
            for row in view.rows.iter().chain(view.agent_rows.values().flatten()) {
                if let Some((_, doc)) = &row.doc {
                    docs.push((doc.clone(), base.clone()));
                }
            }
        }
        for (doc, base) in docs {
            self.request_pictures(&doc, base.as_deref(), cx);
        }
    }

    /// Where a session's answers' relative paths start: the folder its
    /// agent works in.
    pub(crate) fn picture_base(&self, key: SessionKey) -> Option<std::path::PathBuf> {
        let session = self.live.get(key)?;
        let base = session.chat_image_base.borrow().clone();
        base.or_else(|| Some(session.current_project.borrow().clone()))
    }
}

fn row(message: crate::chat::TranscriptMessage) -> Row {
    Row { hash: body_hash(&message.body), message: Rc::new(message), doc: None, parsing: false }
}

/// A transcript row for a sub-agent's recorded message.
pub(crate) fn agent_row(message: crate::chat::TranscriptMessage) -> Row {
    row(message)
}

fn collect_images(blocks: &[Block], urls: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Image { url, .. } => urls.push(url.clone()),
            Block::Quote(inner) | Block::Callout { blocks: inner, .. } => collect_images(inner, urls),
            Block::List { items, .. } => {
                for item in items {
                    collect_images(&item.blocks, urls);
                }
            }
            _ => {}
        }
    }
}

impl App {
    /// The account with id `id`.
    pub(crate) fn account(&self, id: &str) -> Option<&crate::rules::AccountEntry> {
        self.accounts.iter().find(|a| a.id() == id)
    }

    /// A session's state for the sidebar.
    pub(crate) fn sidebar_state(&self, id: &str) -> Option<agents::AgentState> {
        self.live
            .entries()
            .into_iter()
            .find(|s| s.live_session_id.borrow().as_deref() == Some(id))
            .map(|s| s.sidebar_state())
    }
}
