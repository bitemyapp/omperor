//! Retained session ownership and UI-independent runtime event reduction.
use crate::{
    account_alert, agents, attachments,
    chat::Conversation,
    composer,
    file_picker::ProjectFile,
    file_watcher, metrics,
    omp_runtime::{self, AuthProvider, OmpRuntime, ToolOutcome, TurnOutcome, UiEvent},
    session::{self, SessionMessage, Speaker},
    transcripts, workspace,
};

/// What the next fresh session the child reports should be recorded as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingSession {
    pub(crate) project_id: String,
    pub(crate) directory: PathBuf,
    pub(crate) parent_session_id: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) base_branch: Option<String>,
    pub(crate) worktree_path: Option<PathBuf>,
}

/// A session's recorded history, arriving a page at a time from its reader.
#[derive(Debug)]
pub(crate) struct SessionHistoryLoad {
    pub(crate) receiver: flume::Receiver<session::HistoryUpdate>,
    pub(crate) message_count: usize,
}
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Instant,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[derive(Default)]
pub(crate) struct SessionKey(pub u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    #[default]
    Draft,
    History,
    Starting,
    Ready,
    Running,
    Stopping,
    Failed,
    Retiring,
}

/// Submitted user prompts and the position currently shown in the composer.
/// The empty composer is the sentinel immediately after the newest prompt.
#[derive(Debug, Default)]
pub(crate) struct PromptHistory {
    prompts: Vec<String>,
    cursor: Option<usize>,
    /// The unsent draft recall replaced, restored past the newest prompt.
    stash: Option<String>,
}

impl PromptHistory {
    fn prompts_in(messages: &[SessionMessage]) -> Vec<String> {
        messages
            .iter()
            .filter(|message| message.speaker == Speaker::User)
            .map(|message| message.text.clone())
            .collect()
    }

    pub(crate) fn replace_from_messages(&mut self, messages: &[SessionMessage]) {
        self.prompts = Self::prompts_in(messages);
        self.cursor = None;
    }

    pub(crate) fn prepend_from_messages(&mut self, messages: &[SessionMessage]) {
        let older = Self::prompts_in(messages);
        if older.is_empty() {
            return;
        }
        if let Some(cursor) = &mut self.cursor {
            *cursor += older.len();
        }
        self.prompts.splice(0..0, older);
    }

    pub(crate) fn record(&mut self, prompt: &str) {
        let prompt = prompt.trim();
        if !prompt.is_empty() {
            self.prompts.push(prompt.to_owned());
        }
        self.cursor = None;
        self.stash = None;
    }

    pub(crate) fn clear(&mut self) {
        self.prompts.clear();
        self.cursor = None;
        self.stash = None;
    }

    /// Whether a recalled prompt is showing.
    pub(crate) fn navigating(&self) -> bool {
        self.cursor.is_some()
    }

    /// `-1` moves toward older prompts and `1` toward newer prompts, like a
    /// shell: the draft being written when recall starts is kept, and moving
    /// past the newest prompt brings it back. Moving down again is left to
    /// the editor's caret.
    pub(crate) fn navigate(&mut self, direction: isize, current: &str) -> Option<String> {
        match direction.cmp(&0) {
            std::cmp::Ordering::Less => {
                let next = match self.cursor {
                    Some(cursor) => cursor.saturating_sub(1),
                    None => {
                        let last = self.prompts.len().checked_sub(1)?;
                        self.stash = Some(current.to_owned());
                        last
                    }
                };
                self.cursor = Some(next);
                self.prompts.get(next).cloned()
            }
            std::cmp::Ordering::Greater => {
                let cursor = self.cursor?;
                if cursor + 1 < self.prompts.len() {
                    let next = cursor + 1;
                    self.cursor = Some(next);
                    self.prompts.get(next).cloned()
                } else {
                    self.cursor = None;
                    Some(self.stash.take().unwrap_or_default())
                }
            }
            std::cmp::Ordering::Equal => None,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct LiveSession {
    pub(crate) key: SessionKey,
    pub(crate) generation: Cell<u64>,
    pub(crate) lifecycle: Cell<Lifecycle>,
    pub(crate) prompt_history: RefCell<PromptHistory>,
    pub(crate) backend: Cell<crate::mock_inference::Backend>,
    pub(crate) attachments_restored: Cell<bool>,
    pub(crate) pending_attachments: RefCell<composer::PendingAttachments>,
    pub(crate) attachment_generation: Cell<u64>,
    pub(crate) attachment_submission: Cell<bool>,
    pub(crate) usage_revision: Cell<u64>,
    /// This session's receipts as they arrived, for a `/usage` card when
    /// there is no journal to read them from (the mock's sessions).
    pub(crate) live_receipts: RefCell<Vec<crate::usage::LiveReceipt>>,
    pub(crate) image_cache_warning_seen: Cell<bool>,
    pub(crate) cache_warning: RefCell<Option<crate::cache_policy::CacheWarning>>,
    pub(crate) cache_clock: RefCell<crate::cache_policy::CacheClock>,
    pub(crate) live_rate: RefCell<metrics::LiveRate>,
    pub(crate) turn_started_ms: Cell<Option<u64>>,
    pub(crate) tool_starts: RefCell<HashMap<String, Instant>>,
    pub(crate) header_session: RefCell<Option<(String, PathBuf)>>,
    pub(crate) chat_status_text: RefCell<String>,
    pub(crate) selected_session_path: RefCell<Option<PathBuf>>,
    pub(crate) resuming_session_id: RefCell<Option<String>>,
    pub(crate) session_history: RefCell<Option<SessionHistoryLoad>>,
    pub(crate) active_project_id: RefCell<Option<String>>,
    pub(crate) pending_session: RefCell<Option<PendingSession>>,
    pub(crate) fast_models: RefCell<Vec<String>>,
    pub(crate) fast_mode: Cell<bool>,
    /// Whether the agent is offered the `omperor_` tools in this session.
    pub(crate) omperor_tools: Cell<bool>,
    pub(crate) workspace_snapshot: RefCell<Option<workspace::WorkspaceSnapshot>>,
    pub(crate) workspace_refresh_pending: Cell<bool>,
    pub(crate) selected_change: RefCell<Option<PathBuf>>,
    pub(crate) current_project: RefCell<PathBuf>,
    pub(crate) chat_image_base: RefCell<Option<PathBuf>>,
    pub(crate) current_model: RefCell<String>,
    pub(crate) current_thinking_level: RefCell<String>,
    pub(crate) available_models: RefCell<Vec<String>>,
    pub(crate) conversation: RefCell<Conversation>,
    pub(crate) omp_runtime: OnceCell<OmpRuntime>,
    pub(crate) backend_switch_pending: Cell<bool>,
    pub(crate) chat_busy: Cell<bool>,
    pub(crate) chat_dirty: Cell<bool>,
    pub(crate) history_prepend_pending: Cell<bool>,
    pub(crate) read_only_session: Cell<bool>,
    pub(crate) runtime_ready: Cell<bool>,
    pub(crate) runtime_starting: Cell<bool>,
    pub(crate) authenticated_providers: RefCell<HashSet<AuthProvider>>,
    pub(crate) accounts_known: Cell<bool>,
    pub(crate) current_account: RefCell<Option<String>>,
    pub(crate) account_change_pending: Cell<bool>,
    pub(crate) account_alerts: RefCell<account_alert::Alerts>,
    pub(crate) account_alert_live_since: Cell<u64>,
    pub(crate) new_session_pending: Cell<bool>,
    pub(crate) pending_prompt: RefCell<Option<attachments::PreparedPrompt>>,
    pub(crate) project_files: RefCell<Arc<Vec<ProjectFile>>>,
    pub(crate) project_files_dir: RefCell<Option<PathBuf>>,
    pub(crate) project_files_indexing: Cell<bool>,
    pub(crate) project_watcher: RefCell<Option<file_watcher::ProjectWatcher>>,
    pub(crate) workspace_change_seen_at: Cell<Option<Instant>>,
    pub(crate) change_tree_collapsed: RefCell<HashSet<PathBuf>>,
    pub(crate) workspace_read_failures: RefCell<HashMap<String, String>>,
    pub(crate) subagents: RefCell<Vec<omp_runtime::SubagentInfo>>,
    pub(crate) subagent_transcripts: RefCell<HashMap<String, transcripts::Entry>>,
    pub(crate) subagent_sync_inflight: Cell<bool>,
    pub(crate) subagent_sync_at: Cell<Option<Instant>>,
    pub(crate) live_session_id: RefCell<Option<String>>,
    pub(crate) live_state: RefCell<agents::AgentState>,
    pub(crate) turn_started_at: Cell<Option<Instant>>,
}

#[derive(Debug)]
pub(crate) struct LiveSessions {
    entries: Vec<Rc<LiveSession>>,
    selected: SessionKey,
    journals: HashMap<PathBuf, SessionKey>,
    next_key: u64,
    cursor: usize,
}
impl Default for LiveSessions {
    fn default() -> Self {
        let seed = crate::rules::wall_clock_ms().saturating_mul(1_000_000);
        let mut registry = Self {
            entries: Vec::new(),
            selected: SessionKey(0),
            journals: HashMap::new(),
            next_key: seed,
            cursor: 0,
        };
        registry.selected = registry.create().key;
        registry
    }
}
impl LiveSessions {
    pub(crate) fn create(&mut self) -> Rc<LiveSession> {
        self.next_key = self
            .next_key
            .checked_add(1)
            .expect("session identity exhausted");
        let entry = Rc::new(LiveSession {
            key: SessionKey(self.next_key),
            selected_session_path: RefCell::new(Some(crate::session_state::draft_path(
                self.next_key,
            ))),
            ..LiveSession::default()
        });
        self.entries.push(entry.clone());
        entry
    }
    pub(crate) fn restore_draft(&mut self, key: SessionKey) -> Rc<LiveSession> {
        if let Some(entry) = self.get(key) {
            return entry;
        }
        self.next_key = self.next_key.max(key.0);
        let entry = Rc::new(LiveSession {
            key,
            selected_session_path: RefCell::new(Some(crate::session_state::draft_path(key.0))),
            ..LiveSession::default()
        });
        self.entries.push(entry.clone());
        entry
    }
    pub(crate) fn get(&self, key: SessionKey) -> Option<Rc<LiveSession>> {
        self.entries.iter().find(|entry| entry.key == key).cloned()
    }
    pub(crate) fn selected(&self) -> Rc<LiveSession> {
        self.get(self.selected)
            .expect("selected session remains retained")
    }
    pub(crate) fn select(&mut self, key: SessionKey) -> bool {
        if self.get(key).is_none() {
            return false;
        }
        self.selected = key;
        true
    }
    pub(crate) fn entries(&self) -> Vec<Rc<LiveSession>> {
        self.entries.clone()
    }
    /// Forgets a session for good, returning it so its runtime can be shut
    /// down. The selected session is never removed: select another first.
    pub(crate) fn remove(&mut self, key: SessionKey) -> Option<Rc<LiveSession>> {
        if key == self.selected {
            return None;
        }
        let index = self.entries.iter().position(|entry| entry.key == key)?;
        let entry = self.entries.remove(index);
        self.journals.retain(|_, owner| *owner != key);
        if self.cursor > index {
            self.cursor -= 1;
        }
        Some(entry)
    }
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
    /// Paths must be normalized by the discovery worker, never by this main-thread model.
    pub(crate) fn find_path(&self, path: &Path) -> Option<Rc<LiveSession>> {
        self.journals
            .get(path)
            .and_then(|key| self.get(*key))
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|entry| entry.selected_session_path.borrow().as_deref() == Some(path))
                    .cloned()
            })
    }
    pub(crate) fn bind_journal(
        &mut self,
        key: SessionKey,
        path: PathBuf,
    ) -> Result<(), SessionKey> {
        if let Some(existing) = self.find_path(&path)
            && existing.key != key {
                return Err(existing.key);
            }
        let Some(entry) = self.get(key) else {
            return Err(key);
        };
        self.journals.retain(|_, owner| *owner != key);
        entry.selected_session_path.replace(Some(path.clone()));
        self.journals.insert(path, key);
        Ok(())
    }
    pub(crate) fn next_poll(&mut self) -> Option<Rc<LiveSession>> {
        if self.entries.is_empty() {
            return None;
        }
        self.cursor %= self.entries.len();
        let entry = self.entries[self.cursor].clone();
        self.cursor = (self.cursor + 1) % self.entries.len();
        Some(entry)
    }
}

/// Effects retain their originating entry at the controller boundary. Only the
/// selected entry's dirty presentation is rendered; global events are coordinated
/// application-wide and must not be fed back through this reducer.
#[derive(Debug, Default)]
pub(crate) struct Effects {
    pub(crate) metrics: Vec<metrics::Record>,
    pub(crate) global_events: Vec<UiEvent>,
    pub(crate) account_trouble: Vec<(String, bool, Option<AuthProvider>)>,
    pub(crate) account_recovery: Option<String>,
    pub(crate) authentication_failure: Option<String>,
    pub(crate) refresh_workspace: bool,
    pub(crate) refresh_subagents: bool,
    pub(crate) discover_sessions: bool,
    pub(crate) sync_subagent_transcripts: bool,
    pub(crate) dispatch_prompt: bool,
    pub(crate) ready: bool,
    pub(crate) rejected: bool,
}
impl LiveSession {
    /// Whether OMP has said no account is signed in (the mock needs none).
    pub(crate) fn signed_out(&self) -> bool {
        self.accounts_known.get()
            && self.authenticated_providers.borrow().is_empty()
            && self.backend.get() != crate::mock_inference::Backend::Mock
    }

    fn status(&self, text: impl Into<String>) {
        self.chat_status_text.replace(text.into());
    }
    fn state(&self, state: agents::AgentState) {
        self.live_state.replace(state);
    }
    /// Session-local activity shared by the composer and sidebar. A completed
    /// primary turn can still own running workers; an idle selected session
    /// must never borrow another session's activity indicator.
    pub(crate) fn activity_label(&self) -> Option<&'static str> {
        if matches!(self.lifecycle.get(), Lifecycle::Stopping | Lifecycle::Retiring) {
            Some("Stopping…")
        } else if self.attachment_submission.get() {
            Some("Preparing input…")
        } else if self.runtime_starting.get() || self.pending_prompt.borrow().is_some() {
            Some("Starting…")
        } else if self.chat_busy.get() && !matches!(*self.live_state.borrow(), agents::AgentState::Blocked(_)) {
            Some("Working…")
        } else if self.subagents.borrow().iter().any(|agent| agents::AgentState::from_status(&agent.status).is_busy()) {
            Some("Workers running…")
        } else {
            None
        }
    }

    /// The state the sidebar shows for this session: working while anything
    /// is under way, else what the session is doing or how it stopped.
    pub(crate) fn sidebar_state(&self) -> agents::AgentState {
        if self.activity_label().is_some() {
            agents::AgentState::Working
        } else {
            self.live_state.borrow().clone()
        }
    }

    pub(crate) fn reduce(&self, event: UiEvent, arrived: Instant) -> Effects {
        let mut effects = Effects::default();
        if self.backend_switch_pending.get() {
            match &event {
                UiEvent::Ready { model, .. }
                    if (model == crate::mock_inference::MODEL)
                        == (self.backend.get() == crate::mock_inference::Backend::Mock) =>
                {
                    self.backend_switch_pending.set(false)
                }
                UiEvent::Failed(_) | UiEvent::Stopped => self.backend_switch_pending.set(false),
                UiEvent::RuntimeGeneration(_) => {}
                _ => {
                    effects.rejected = true;
                    return effects;
                }
            }
        }
        match event {
            UiEvent::RuntimeGeneration(generation) => {
                self.generation.set(generation);
            }
            UiEvent::Connecting { project, .. } => {
                self.runtime_ready.set(false);
                self.runtime_starting.set(true);
                self.lifecycle.set(Lifecycle::Starting);
                self.current_project.replace(project);
                if !self.read_only_session.get() {
                    self.status("Starting the agent…");
                }
            }
            ready @ UiEvent::Ready { .. } => {
                let UiEvent::Ready {
                    model,
                    models,
                    fast_models,
                    fast_mode,
                    thinking_level,
                    session_id,
                    session_file,
                    project,
                    authenticated_providers,
                    accounts,
                    account,
                    session_is_fresh,
                    resumed,
                } = &ready
                else {
                    unreachable!()
                };
                let expected = self.resuming_session_id.borrow();
                if expected.as_deref().is_some_and(|expected| {
                    resumed.as_deref() != Some(expected) || session_id.as_deref() != Some(expected)
                }) {
                    drop(expected);
                    self.resuming_session_id.borrow_mut().take();
                    self.fail_runtime();
                    self.read_only_session.set(true);
                    self.status(
                        "The agent opened a different session than requested • showing history read-only",
                    );
                    effects.rejected = true;
                    return effects;
                }
                drop(expected);
                if self.resuming_session_id.borrow_mut().take().is_some() {
                    self.read_only_session.set(false);
                }
                self.account_alert_live_since
                    .set(crate::rules::wall_clock_ms_at(arrived));
                self.current_project.replace(project.clone());
                if *self.current_model.borrow() != *model
                    || (self.current_account.borrow().is_some() && *self.current_account.borrow() != *account)
                {
                    self.cache_clock.borrow_mut().reset(crate::rules::wall_clock_ms_at(arrived));
                }
                self.current_model.replace(model.clone());
                self.current_account.replace(account.clone());
                self.account_change_pending.set(false);
                self.live_session_id.replace(session_id.clone());
                self.subagents.borrow_mut().clear();
                if !matches!(
                    *self.live_state.borrow(),
                    agents::AgentState::Errored
                        | agents::AgentState::Interrupted
                        | agents::AgentState::Done
                ) {
                    self.state(agents::AgentState::Idle);
                }
                self.accounts_known.set(accounts.is_some());
                self.authenticated_providers
                    .replace(authenticated_providers.iter().copied().collect());
                self.available_models.replace(models.clone());
                self.fast_models.replace(fast_models.clone());
                self.fast_mode.set(*fast_mode);
                self.current_thinking_level.replace(thinking_level.clone());
                self.runtime_ready.set(true);
                self.runtime_starting.set(false);
                self.lifecycle.set(Lifecycle::Ready);
                if let (Some(id), Some(path)) = (session_id, session_file) {
                    self.header_session
                        .replace(Some((id.clone(), path.clone())));

                }
                if *session_is_fresh {
                    self.new_session_pending.set(false);
                }
                self.chat_busy.set(false);
                if !self.read_only_session.get() {
                    self.status(format!("Ready • {model} • reasoning {thinking_level}"));
                }
                effects.ready = true;
                effects.dispatch_prompt = true;
                effects.global_events.push(ready);
            }
            UiEvent::Running => {
                self.conversation.borrow_mut().begin_turn();
                self.chat_busy.set(true);
                self.lifecycle.set(Lifecycle::Running);
                self.status("Working…");
                self.turn_started_at.set(Some(arrived));
                self.turn_started_ms
                    .set(Some(crate::rules::wall_clock_ms_at(arrived)));
                self.tool_starts.borrow_mut().clear();
                self.state(agents::AgentState::Working);
            }
            UiEvent::AssistantStarted => {
                self.conversation.borrow_mut().start_assistant();
                self.chat_dirty.set(true);
                self.state(agents::AgentState::Working);
                self.live_rate.borrow_mut().response_started();
            }
            UiEvent::AssistantDelta(delta) => {
                self.live_rate
                    .borrow_mut()
                    .delta(delta.chars().count(), arrived);
                self.conversation.borrow_mut().append_assistant(&delta);
                self.chat_dirty.set(true);
            }
            UiEvent::ThinkingDelta(delta) => {
                self.live_rate
                    .borrow_mut()
                    .thinking_delta(delta.chars().count(), arrived);
                self.conversation.borrow_mut().append_thinking(&delta);
                self.chat_dirty.set(true);
                self.status("Thinking…");
                self.state(agents::AgentState::Thinking);
            }
            UiEvent::AssistantFinished(text) => {
                self.conversation
                    .borrow_mut()
                    .finish_assistant(text.as_deref());
                self.chat_dirty.set(true);
            }
            UiEvent::InferenceReceipt(receipt) => {
                self.cache_clock.borrow_mut().observe(
                    crate::usage::live_cache_observation(&receipt, &self.current_model.borrow()),
                    crate::rules::wall_clock_ms_at(arrived),
                );
                self.usage_revision.set(self.usage_revision.get().wrapping_add(1));
                let usage = crate::usage::Evidence::from(&receipt);
                self.live_receipts
                    .borrow_mut()
                    .push(crate::usage::live_receipt(&receipt, crate::rules::wall_clock_ms_at(arrived)));
                if receipt.tokens_out > 0 {
                    effects.account_recovery = receipt.provider.clone();
                }
                let inference = metrics::Inference {
                    usage,
                    at_ms: crate::rules::wall_clock_ms_at(arrived),
                    session: self
                        .live_session_id
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| "unknown".into()),
                    turn_ms: self.turn_started_ms.get().unwrap_or_default(),
                    provider: receipt.provider,
                    model: receipt.model,
                    tokens_in: receipt.tokens_in,
                    tokens_out: receipt.tokens_out,
                    tokens_reasoning: receipt.tokens_reasoning,
                    cache_read: receipt.cache_read,
                    cache_write: receipt.cache_write,
                    ttft_ms: receipt.ttft_ms,
                    last_token_ms: receipt.last_token_ms,
                    duration_ms: receipt.duration_ms.unwrap_or(0),
                    fast: self.fast_mode.get()
                        && self
                            .fast_models
                            .borrow()
                            .contains(&self.current_model.borrow()),
                    reasoning_hidden: receipt.tokens_reasoning > 0
                        && !self.live_rate.borrow().streamed_reasoning(),
                };
                self.live_rate.borrow_mut().settle(&inference);
                if self.backend.get() != crate::mock_inference::Backend::Mock {
                    effects.metrics.push(metrics::Record::Inference(inference));
                }
            }
            UiEvent::TurnFinished(outcome) => {
                if let TurnOutcome::Errored(error) = &outcome {
                    effects.account_trouble.push((error.clone(), true, None));
                }
                let mut conversation = self.conversation.borrow_mut();
                conversation.finish_turn();
                let mock = self.backend.get() == crate::mock_inference::Backend::Mock;
                match &outcome {
                    TurnOutcome::Completed => {
                        self.cache_warning.borrow_mut().take();
                    }
                    TurnOutcome::Interrupted if mock => {
                        conversation.notice("The mock turn was stopped.", false)
                    }
                    TurnOutcome::Interrupted => conversation.note_turn_end(false, None),
                    TurnOutcome::Errored(reason) if mock => conversation
                        .notice(format!("The mock turn ended with an error: {reason}"), true),
                    TurnOutcome::Errored(reason) => conversation.note_turn_end(true, Some(reason)),
                }
                drop(conversation);
                self.chat_busy.set(false);
                if let Some(start) = self.turn_started_at.get()
                    && !mock {
                        effects.metrics.push(metrics::Record::Turn(metrics::Turn {
                            at_ms: crate::rules::wall_clock_ms_at(arrived),
                            session: self
                                .live_session_id
                                .borrow()
                                .clone()
                                .unwrap_or_else(|| "unknown".into()),
                            turn_ms: self.turn_started_ms.get().unwrap_or_default(),
                            duration_ms: u64::try_from(
                                arrived.saturating_duration_since(start).as_millis(),
                            )
                            .unwrap_or(u64::MAX),
                            outcome: match outcome {
                                TurnOutcome::Completed => "completed",
                                TurnOutcome::Interrupted => "interrupted",
                                TurnOutcome::Errored(_) => "errored",
                            }
                            .into(),
                        }));
                    }
                self.turn_started_at.set(None);
                self.turn_started_ms.set(None);
                self.tool_starts.borrow_mut().clear();
                self.live_rate.borrow_mut().response_abandoned();
                self.lifecycle.set(if self.runtime_ready.get() {
                    Lifecycle::Ready
                } else {
                    Lifecycle::Failed
                });
                self.state(match outcome {
                    TurnOutcome::Completed => agents::AgentState::Done,
                    TurnOutcome::Interrupted => agents::AgentState::Interrupted,
                    TurnOutcome::Errored(_) => agents::AgentState::Errored,
                });
                self.status(match outcome {
                    TurnOutcome::Completed => "Ready for another prompt",
                    TurnOutcome::Interrupted => "Turn interrupted",
                    TurnOutcome::Errored(_) if mock => {
                        "The mock turn ended with an error · see the explanation in chat"
                    }
                    TurnOutcome::Errored(_) => "The turn ended with an error",
                });
                self.chat_dirty.set(true);
                effects.discover_sessions = true;
                effects.refresh_workspace = true;
                effects.refresh_subagents = true;
            }
            UiEvent::ToolStarted {
                call_id,
                name,
                args,
                intent,
            } => {
                let mut conversation = self.conversation.borrow_mut();
                conversation.start_tool(&call_id, &name);
                conversation.set_tool_context(&call_id, args.as_deref(), intent.as_deref());
                drop(conversation);
                self.tool_starts.borrow_mut().insert(call_id, arrived);
                self.status(format!("Running {name}…"));
                self.chat_dirty.set(true);
                self.state(agents::AgentState::RunningTool(name));
            }
            UiEvent::ToolArguments { call_id, delta } => {
                self.conversation
                    .borrow_mut()
                    .append_tool_arguments(&call_id, &delta);
                self.chat_dirty.set(true);
            }
            UiEvent::ToolUpdated {
                call_id,
                content,
                args,
            } => {
                let mut conversation = self.conversation.borrow_mut();
                conversation.append_tool_update(&call_id, &content);
                conversation.set_tool_context(&call_id, args.as_deref(), None);
                self.chat_dirty.set(true);
            }
            UiEvent::ToolFinished(outcome) => {
                self.finish_tool(&outcome, arrived, &mut effects);
                if outcome.is_error == Some(true) {
                    self.status(format!(
                        "Tool {} failed…",
                        outcome.name.as_deref().unwrap_or("call")
                    ));
                } else {
                    self.status("Working…");
                }
                self.state(agents::AgentState::Working);
                effects.refresh_workspace = crate::rules::tool_may_change_files(outcome.name.as_deref());
            }
            UiEvent::TurnResults(outcomes) => {
                for outcome in outcomes {
                    self.finish_tool(&outcome, arrived, &mut effects);
                }
            }
            UiEvent::SubagentsSnapshot(subagents) => {
                if *self.subagents.borrow() == subagents {
                    return effects;
                }
                let previous = self.subagents.borrow();
                let by_id: HashMap<_, _> =
                    previous.iter().map(|old| (old.id.as_str(), old)).collect();
                for next in &subagents {
                    if let Some(error) = account_alert::fresh_subagent_failure(
                        by_id.get(next.id.as_str()).copied(),
                        next,
                        self.account_alert_live_since.get(),
                    ) {
                        effects
                            .account_trouble
                            .push((error.to_owned(), false, None));
                    }
                }
                drop(previous);
                self.usage_revision.set(self.usage_revision.get().wrapping_add(1));
                self.subagents.replace(subagents);
                effects.sync_subagent_transcripts = true;
            }
            UiEvent::SubagentChanged(info) => {
                self.usage_revision.set(self.usage_revision.get().wrapping_add(1));
                let mut subagents = self.subagents.borrow_mut();
                let old = subagents.iter_mut().find(|old| old.id == info.id);
                if let Some(error) = account_alert::fresh_subagent_failure(
                    old.as_deref(),
                    &info,
                    self.account_alert_live_since.get(),
                ) {
                    effects
                        .account_trouble
                        .push((error.to_owned(), false, None));
                }
                match old {
                    Some(old) => *old = info,
                    None => subagents.push(info),
                }
                effects.sync_subagent_transcripts = true;
            }
            UiEvent::SubagentsChanged => {
                effects.refresh_subagents = true;
            }
            UiEvent::Failed(error) => {
                effects.account_trouble.push((error.clone(), true, None));
                self.fail_runtime();
                self.state(agents::AgentState::Errored);
                if self.resuming_session_id.borrow_mut().take().is_some() {
                    self.read_only_session.set(true);
                    self.status(format!(
                        "Could not resume this Omperor session • showing history read-only: {error}"
                    ));
                    self.conversation.borrow_mut().notice(
                        format!("Could not resume this session, so it is shown read-only. {error}"),
                        true,
                    );
                    self.chat_dirty.set(true);
                } else if !self.read_only_session.get() {
                    self.conversation.borrow_mut().fail_assistant(&error);
                    self.status("The agent stopped unexpectedly");
                    self.chat_dirty.set(true);
                }
                effects.authentication_failure =
                    Some(format!("Sign-in failed: {error}"));
            }
            UiEvent::SessionReleased => {
                self.runtime_ready.set(false);
                self.runtime_starting.set(false);
                self.lifecycle.set(Lifecycle::History);
                self.pending_prompt.borrow_mut().take();
            }
            UiEvent::Stopped => {
                self.fail_runtime();
                self.live_session_id.borrow_mut().take();
                self.subagents.borrow_mut().clear();
                self.state(agents::AgentState::Idle);
                self.lifecycle.set(Lifecycle::History);
                if self.resuming_session_id.borrow_mut().take().is_some() {
                    self.read_only_session.set(true);
                    self.status(
                        "The agent stopped while resuming this session • showing history read-only",
                    );
                } else if !self.read_only_session.get() {
                    self.status("The agent stopped");
                }
                effects.authentication_failure =
                    Some("The agent stopped before the sign-in finished".into());
            }
            UiEvent::AuthenticationRequired { provider, error } => {
                effects
                    .account_trouble
                    .push((error.clone(), true, Some(provider)));
                self.state(agents::AgentState::Blocked("sign in again".into()));
                self.live_rate.borrow_mut().response_abandoned();
                self.runtime_starting.set(false);
                self.new_session_pending.set(false);
                self.pending_prompt.borrow_mut().take();
                self.authenticated_providers.borrow_mut().remove(&provider);
                if !self.read_only_session.get() {
                    self.conversation.borrow_mut().fail_assistant(&error);
                    self.chat_dirty.set(true);
                }
                self.chat_busy.set(false);
                self.status(format!(
                    "{} authorization expired • connect again to continue",
                    provider.product_name()
                ));
                effects
                    .global_events
                    .push(UiEvent::AuthenticationRequired { provider, error });
            }
            UiEvent::ModelChanged(model) => {
                if *self.current_model.borrow() != model {
                    self.cache_clock.borrow_mut().reset(crate::rules::wall_clock_ms_at(arrived));
                }
                self.current_model.replace(model.clone());
                if !self.read_only_session.get() {
                    self.status(format!("Ready • {model}"));
                }
            }
            UiEvent::ThinkingLevelChanged(level) => {
                self.current_thinking_level.replace(level.clone());
                if !self.read_only_session.get() {
                    self.status(format!(
                        "Ready • {} • reasoning {level}",
                        self.current_model.borrow()
                    ));
                }
            }
            UiEvent::OmperorToolsChanged(enabled) => {
                self.omperor_tools.set(enabled);
            }
            // The app answers these itself, before the reducer sees them.
            UiEvent::HostToolCall { .. } | UiEvent::HostToolCancelled { .. } => {}
            UiEvent::FastModeChanged(enabled) => {
                self.fast_mode.set(enabled);
                if !self.read_only_session.get() {
                    self.status(format!(
                        "Ready • {} • fast {}",
                        self.current_model.borrow(),
                        if enabled { "on" } else { "off" }
                    ));
                }
            }
            UiEvent::AccountChanged(account) => {
                if *self.current_account.borrow() != account {
                    self.cache_clock.borrow_mut().reset(crate::rules::wall_clock_ms_at(arrived));
                }
                self.account_change_pending.set(false);
                self.current_account.replace(account.clone());
                effects.dispatch_prompt = true;
                effects.global_events.push(UiEvent::AccountChanged(account));
            }
            UiEvent::Notice(message) => {
                if !self.read_only_session.get() {
                    self.status(message);
                }
            }
            event @ (UiEvent::AccountsChanged(_)
            | UiEvent::AccountUsage(_)
            | UiEvent::AuthenticationStarted { .. }
            | UiEvent::AuthenticationSession { .. }
            | UiEvent::OpenAuthenticationUrl { .. }
            | UiEvent::AuthenticationDeviceCode { .. }
            | UiEvent::AuthenticationWaiting { .. }
            | UiEvent::AuthenticationPrompt(_)
            | UiEvent::AuthenticationCancelled { .. }
            | UiEvent::AuthenticationCompleted { .. }
            | UiEvent::AuthenticationFailed { .. }) => {
                effects.global_events.push(event);
            }
        }
        effects
    }
    fn fail_runtime(&self) {
        self.turn_started_at.set(None);
        self.turn_started_ms.set(None);
        self.tool_starts.borrow_mut().clear();
        self.live_rate.borrow_mut().response_abandoned();
        self.runtime_ready.set(false);
        self.runtime_starting.set(false);
        self.new_session_pending.set(false);
        self.pending_prompt.borrow_mut().take();
        self.chat_busy.set(false);
        self.lifecycle.set(Lifecycle::Failed);
    }
    fn finish_tool(&self, outcome: &ToolOutcome, arrived: Instant, effects: &mut Effects) {
        if let Some(start) = self.tool_starts.borrow_mut().remove(&outcome.call_id)
            && self.backend.get() != crate::mock_inference::Backend::Mock {
                effects.metrics.push(metrics::Record::Tool(metrics::Tool {
                    at_ms: crate::rules::wall_clock_ms_at(arrived),
                    session: self
                        .live_session_id
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| "unknown".into()),
                    turn_ms: self.turn_started_ms.get().unwrap_or_default(),
                    name: outcome.name.clone().unwrap_or_else(|| "tool".into()),
                    duration_ms: u64::try_from(
                        arrived.saturating_duration_since(start).as_millis(),
                    )
                    .unwrap_or(u64::MAX),
                    error: outcome.is_error == Some(true),
                }));
            }
        self.conversation.borrow_mut().finish_tool(
            &outcome.call_id,
            outcome.name.as_deref(),
            outcome.is_error,
            outcome.result.as_ref(),
        );
        self.chat_dirty.set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cache_warning_survives_failed_turns_and_clears_only_for_completed_owner() {
        let a = LiveSession::default();
        let b = LiveSession::default();
        let warning = crate::cache_policy::CacheWarning::for_action(
            "openai-codex/gpt-5.6-luna",
            crate::cache_policy::CacheAction::NewSession,
        )
        .expect("Codex new sessions may miss the prior session's cache");
        a.cache_warning.replace(Some(warning.clone()));
        b.cache_warning.replace(Some(warning.clone()));
        let now = Instant::now();
        for outcome in [TurnOutcome::Errored("offline".into()), TurnOutcome::Interrupted] {
            a.reduce(UiEvent::TurnFinished(outcome), now);
            assert_eq!(*a.cache_warning.borrow(), Some(warning.clone()));
        }
        a.reduce(UiEvent::TurnFinished(TurnOutcome::Completed), now);
        assert!(a.cache_warning.borrow().is_none());
        assert_eq!(*b.cache_warning.borrow(), Some(warning));
    }

    #[test]
    fn cache_window_is_owned_refreshed_by_receipts_and_reset_by_namespace_changes() {
        let a = LiveSession::default();
        let b = LiveSession::default();
        let model = "anthropic/claude-sonnet-4-6";
        a.current_model.replace(model.into());
        let arrived = Instant::now();
        let now = crate::rules::wall_clock_ms_at(arrived);
        a.reduce(UiEvent::InferenceReceipt(omp_runtime::InferenceReceipt {
            model: Some(model.into()), recorded_at_ms: Some(now), duration_ms: Some(240_000),
            cache_write: 100, ..Default::default()
        }), arrived);
        assert!(a.cache_clock.borrow().warning(now).is_some());
        assert!(b.cache_clock.borrow().warning(now).is_none());
        a.reduce(UiEvent::TurnFinished(TurnOutcome::Completed), arrived);
        assert!(a.cache_clock.borrow().warning(now).is_some());
        a.reduce(UiEvent::AccountChanged(Some("another-account".into())), arrived);
        a.cache_clock.borrow_mut().observe(Some(crate::cache_policy::CacheObservation { completed_at_ms: now - 240_000, ttl_seconds: None, retention: None }), now); // stale async report
        assert!(a.cache_clock.borrow().warning(now).is_none());
        a.cache_clock.borrow_mut().observe(Some(crate::cache_policy::CacheObservation { completed_at_ms: now + 1000, ttl_seconds: None, retention: None }), now + 1000);
        assert!(a.cache_clock.borrow().warning(now + 241_000).is_some());
        a.reduce(UiEvent::ModelChanged("anthropic/claude-opus-4-6".into()), arrived);
        assert!(a.cache_clock.borrow().warning(now + 240_000).is_none());
    }

    #[test]
    fn streamed_text_leaves_the_sidebar_state_alone() {
        // The sidebar redraws a session's indicator only when this changes,
        // so a stream of text must not change it; a turn starting must.
        let session = LiveSession::default();
        let at = Instant::now();
        let _ = session.reduce(UiEvent::Running, at);
        let _ = session.reduce(UiEvent::AssistantStarted, at);
        let working = session.sidebar_state();
        for delta in ["Stream", "ed ", "text"] {
            let effects = session.reduce(UiEvent::AssistantDelta(delta.into()), at);
            assert!(!effects.ready && !effects.discover_sessions && effects.global_events.is_empty());
            assert_eq!(session.sidebar_state(), working);
        }
        session.chat_busy.set(true);
        assert_eq!(session.sidebar_state(), agents::AgentState::Working);
    }

    #[test]
    fn prompt_history_moves_older_newer_and_back_to_empty() {
        let mut history = PromptHistory::default();
        history.record("first prompt");
        history.record("second prompt");

        assert_eq!(history.navigate(-1, "half-written").as_deref(), Some("second prompt"));
        assert!(history.navigating());
        assert_eq!(history.navigate(-1, "second prompt").as_deref(), Some("first prompt"));
        assert_eq!(history.navigate(-1, "first prompt").as_deref(), Some("first prompt"));
        assert_eq!(history.navigate(1, "first prompt").as_deref(), Some("second prompt"));
        // Past the newest prompt the draft comes back instead of an empty field.
        assert_eq!(history.navigate(1, "second prompt").as_deref(), Some("half-written"));
        assert_eq!(history.navigate(1, "half-written"), None);
    }

    #[test]
    fn prompt_history_follows_loaded_and_prepended_session_pages() {
        let mut history = PromptHistory::default();
        history.replace_from_messages(&[
            SessionMessage::spoken(Speaker::User, "recent question"),
            SessionMessage::spoken(Speaker::Assistant, "recent answer"),
        ]);
        history.prepend_from_messages(&[
            SessionMessage::spoken(Speaker::User, "oldest question"),
            SessionMessage::spoken(Speaker::Assistant, "oldest answer"),
        ]);

        assert_eq!(history.navigate(-1, "").as_deref(), Some("recent question"));
        assert_eq!(history.navigate(-1, "recent question").as_deref(), Some("oldest question"));
    }

    #[test]
    fn drafts_keep_distinct_identity_and_restore_advances_allocator() {
        let mut sessions = LiveSessions::default();
        let a = sessions.selected();
        let b = sessions.create();
        assert_ne!(a.key, b.key);
        assert_ne!(
            *a.selected_session_path.borrow(),
            *b.selected_session_path.borrow()
        );
        let restored_key = SessionKey(b.key.0 + 100);
        let restored = sessions.restore_draft(restored_key);
        assert!(Rc::ptr_eq(&restored, &sessions.restore_draft(restored_key)));
        assert!(sessions.create().key.0 > restored_key.0);
        assert_eq!(sessions.selected().key, a.key);
    }

    #[test]
    fn journal_binding_rejects_second_writer_without_retargeting() {
        let mut sessions = LiveSessions::default();
        let a = sessions.selected();
        let b = sessions.create();
        let path = PathBuf::from("/sessions/one.oms");
        assert_eq!(sessions.bind_journal(a.key, path.clone()), Ok(()));
        assert_eq!(sessions.bind_journal(b.key, path.clone()), Err(a.key));
        assert_eq!(sessions.find_path(&path).unwrap().key, a.key);
        assert_ne!(b.selected_session_path.borrow().as_ref(), Some(&path));
    }

    #[test]
    fn interleaved_background_output_and_failure_do_not_touch_selected_session() {
        let mut sessions = LiveSessions::default();
        let a = sessions.selected();
        let b = sessions.create();
        let now = Instant::now();
        for entry in [&a, &b] {
            entry.runtime_ready.set(true);
            entry.reduce(UiEvent::Running, now);
            entry.reduce(UiEvent::AssistantStarted, now);
        }
        a.reduce(UiEvent::AssistantDelta("alpha".into()), now);
        b.reduce(UiEvent::AssistantDelta("bravo".into()), now);
        sessions.select(b.key);
        a.reduce(
            UiEvent::TurnFinished(TurnOutcome::Errored("A failed".into())),
            now,
        );
        assert_eq!(sessions.selected().key, b.key);
        assert!(b.chat_busy.get());
        assert!(!a.chat_busy.get());
        let a_text: String = a.conversation.borrow().messages().map(|m| m.body).collect();
        let b_text: String = b.conversation.borrow().messages().map(|m| m.body).collect();
        assert!(a_text.contains("alpha"));
        assert!(!a_text.contains("bravo"));
        assert!(b_text.contains("bravo"));
        assert!(!b_text.contains("alpha"));
    }

    #[test]
    fn release_preserves_session_identity_and_transcript_for_resume() {
        let entry = LiveSession::default();
        let now = Instant::now();
        let path = PathBuf::from("/sessions/released.oms");
        entry
            .live_session_id
            .replace(Some("released-session".into()));
        entry.selected_session_path.replace(Some(path.clone()));
        entry.runtime_ready.set(true);
        entry.reduce(UiEvent::AssistantStarted, now);
        entry.reduce(UiEvent::AssistantDelta("retained answer".into()), now);
        entry.reduce(UiEvent::TurnFinished(TurnOutcome::Completed), now);
        entry.reduce(UiEvent::SessionReleased, now);
        assert!(!entry.runtime_ready.get());
        assert!(!entry.runtime_starting.get());
        assert_eq!(entry.lifecycle.get(), Lifecycle::History);
        assert_eq!(
            entry.live_session_id.borrow().as_deref(),
            Some("released-session")
        );
        assert_eq!(entry.selected_session_path.borrow().as_ref(), Some(&path));
        let text: String = entry
            .conversation
            .borrow()
            .messages()
            .map(|m| m.body)
            .collect();
        assert!(text.contains("retained answer"));
    }

    #[test]
    fn activity_follows_each_session_through_start_run_stop_and_navigation() {
        let mut sessions = LiveSessions::default();
        let a = sessions.selected();
        let b = sessions.create();
        a.runtime_starting.set(true);
        assert_eq!(a.activity_label(), Some("Starting…"));
        assert_eq!(b.activity_label(), None);
        a.runtime_starting.set(false);
        a.reduce(UiEvent::Running, Instant::now());
        b.reduce(UiEvent::Running, Instant::now());
        assert_eq!(a.activity_label(), Some("Working…"));
        assert_eq!(b.activity_label(), Some("Working…"));
        sessions.select(b.key);
        a.reduce(UiEvent::TurnFinished(TurnOutcome::Completed), Instant::now());
        assert_eq!(a.activity_label(), None);
        assert_eq!(sessions.selected().activity_label(), Some("Working…"));
        b.lifecycle.set(Lifecycle::Stopping);
        assert_eq!(b.activity_label(), Some("Stopping…"));
        b.reduce(UiEvent::TurnFinished(TurnOutcome::Interrupted), Instant::now());
        assert_eq!(b.activity_label(), None);
    }

    #[test]
    fn runtime_generation_boundary_updates_owning_entry_only() {
        let a = LiveSession::default();
        let b = LiveSession::default();
        a.reduce(UiEvent::RuntimeGeneration(2), Instant::now());
        assert_eq!(a.generation.get(), 2);
        assert_eq!(b.generation.get(), 0);
    }

    #[test]
    fn repeated_tool_ids_and_metric_receipts_are_session_local() {
        let sessions = LiveSessions::default();
        let a = sessions.selected();
        let b = LiveSession::default();
        let now = Instant::now();
        for (entry, id) in [(&*a, "a"), (&b, "b")] {
            entry.live_session_id.replace(Some(id.into()));
            entry.reduce(UiEvent::Running, now);
            entry.reduce(
                UiEvent::ToolStarted {
                    call_id: "same-id".into(),
                    name: "read".into(),
                    args: None,
                    intent: None,
                },
                now,
            );
        }
        let outcome = ToolOutcome {
            call_id: "same-id".into(),
            name: Some("read".into()),
            is_error: Some(false),
            result: None,
        };
        let effects = a.reduce(UiEvent::ToolFinished(outcome), now);
        assert!(a.tool_starts.borrow().is_empty());
        assert_eq!(b.tool_starts.borrow().len(), 1);
        assert!(matches!(&effects.metrics[0], metrics::Record::Tool(tool) if tool.session == "a"));
    }

    #[test]
    fn round_robin_cursor_survives_partial_drain_passes() {
        let mut sessions = LiveSessions::default();
        let a = sessions.selected();
        let b = sessions.create();
        let c = sessions.create();
        assert_eq!(sessions.next_poll().unwrap().key, a.key);
        // A GUI budget expiry does not reset the next pass to the noisy first producer.
        assert_eq!(sessions.next_poll().unwrap().key, b.key);
        assert_eq!(sessions.next_poll().unwrap().key, c.key);
        assert_eq!(sessions.next_poll().unwrap().key, a.key);
    }

    #[test]
    fn model_events_are_not_replayed_as_global_events() {
        let entry = LiveSession::default();
        let now = Instant::now();
        for event in [
            UiEvent::ModelChanged("model".into()),
            UiEvent::ThinkingLevelChanged("high".into()),
            UiEvent::Failed("failure".into()),
            UiEvent::Stopped,
        ] {
            assert!(entry.reduce(event, now).global_events.is_empty());
        }
        assert_eq!(&*entry.current_model.borrow(), "model");
        assert_eq!(&*entry.current_thinking_level.borrow(), "high");
    }

    #[test]
    fn unknown_session_records_turn_once() {
        let entry = LiveSession::default();
        let now = Instant::now();
        entry.reduce(UiEvent::Running, now);
        let effects = entry.reduce(
            UiEvent::TurnFinished(TurnOutcome::Completed),
            now + Duration::from_millis(15),
        );
        assert!(
            matches!(&effects.metrics[0], metrics::Record::Turn(turn) if turn.session == "unknown" && turn.duration_ms == 15)
        );
        assert!(
            entry
                .reduce(UiEvent::TurnFinished(TurnOutcome::Completed), now)
                .metrics
                .is_empty()
        );
    }

    #[test]
    fn backend_switch_rejects_output_and_stop_clears_identity() {
        let entry = LiveSession::default();
        let now = Instant::now();
        entry.backend_switch_pending.set(true);
        assert!(
            entry
                .reduce(UiEvent::AssistantDelta("old".into()), now)
                .rejected
        );
        assert!(!entry.chat_dirty.get());
        entry.live_session_id.replace(Some("old".into()));
        let effects = entry.reduce(UiEvent::Stopped, now);
        assert!(!entry.backend_switch_pending.get());
        assert!(entry.live_session_id.borrow().is_none());
        assert!(effects.authentication_failure.is_some());
    }

    #[test]
    fn inference_receipt_emits_account_recovery_not_global_model_event() {
        let entry = LiveSession::default();
        let now = Instant::now();
        entry.current_model.replace("fast".into());
        entry.fast_models.replace(vec!["fast".into()]);
        entry.fast_mode.set(true);
        let receipt = || omp_runtime::InferenceReceipt {
            provider: Some("openai".into()),
            model: Some("fast".into()),
            tokens_in: 10,
            tokens_out: 20,
            tokens_reasoning: 4,
            cache_read: 2,
            cache_write: 3,
            ttft_ms: Some(5),
            last_token_ms: Some(100),
            duration_ms: Some(110),
            ..Default::default()
        };
        let effects = entry.reduce(UiEvent::InferenceReceipt(receipt()), now);
        assert!(effects.global_events.is_empty());
        assert_eq!(effects.account_recovery.as_deref(), Some("openai"));
        assert!(
            matches!(&effects.metrics[0], metrics::Record::Inference(metric)
            if metric.session == "unknown" && metric.fast && metric.reasoning_hidden && metric.tokens_out == 20)
        );
        entry.backend.set(crate::mock_inference::Backend::Mock);
        assert!(
            entry
                .reduce(UiEvent::InferenceReceipt(receipt()), now)
                .metrics
                .is_empty()
        );
    }

    #[test]
    fn wrong_resume_identity_never_dispatches_pending_prompt() {
        let entry = LiveSession::default();
        entry.resuming_session_id.replace(Some("expected".into()));
        entry.pending_prompt.replace(Some("do not replay".into()));
        let effects = entry.reduce(
            UiEvent::Ready {
                model: "test/model".into(),
                models: Vec::new(),
                fast_models: Vec::new(),
                fast_mode: false,
                thinking_level: "high".into(),
                session_id: Some("different".into()),
                session_file: Some("/different.oms".into()),
                project: "/tmp".into(),
                authenticated_providers: Vec::new(),
                accounts: None,
                account: None,
                session_is_fresh: false,
                resumed: Some("expected".into()),
            },
            Instant::now(),
        );
        assert!(effects.rejected);
        assert!(!effects.dispatch_prompt);
        assert!(entry.pending_prompt.borrow().is_none());
        assert!(entry.read_only_session.get());
        assert!(!entry.runtime_ready.get());
    }
}
