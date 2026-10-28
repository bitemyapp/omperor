//! The composer: typing, recalling earlier prompts, the `@` file and `/`
//! command pickers, attachments, sending (or steering a turn under way),
//! and the model, reasoning, account and Fast choices beside it.

use std::path::PathBuf;
use std::sync::Arc;

use aizome_kit::prelude::*;
use aizome_kit::{Selection, editor::EditorAction};

use super::{App, Msg, Picker, PickerItem, PickerKind};
use crate::attachments::{self, AttachmentSource};
use crate::cache_policy::{CacheAction, CacheWarning};
use crate::live_sessions::{LiveSession, SessionKey};
use crate::omp_runtime::AuthProvider;
use crate::rules::{AccountResolution, model_change_requires_new_session, resolve_provider_account};
use crate::{file_picker, slash_commands, usage, usage_card};

/// How many files the `@` picker lists.
const PICKER_ROWS: usize = 8;

impl App {
    fn composer_text(&self, key: SessionKey) -> String {
        self.views.get(&key).map(|v| v.editor.text()).unwrap_or_default()
    }

    pub(super) fn set_composer_text(&mut self, key: SessionKey, text: &str) {
        let view = self.view_mut(key);
        view.editor.set_text(text);
        let end = view.editor.len_chars();
        view.editor.set_selection(Selection::caret(end));
        view.picker = None;
    }

    pub(super) fn edit(&mut self, action: EditorAction, cx: &mut Cx<Msg>) {
        let session = self.session();
        let key = session.key;
        let edits = action.edits();
        self.view_mut(key).editor.perform(action);
        if edits {
            self.schedule_save_views(cx);
        }
        self.update_picker(&session);
    }

    /// Open, filter or close the picker for what is typed before the caret.
    fn update_picker(&mut self, session: &LiveSession) {
        let mock = session.backend.get() == crate::mock_inference::Backend::Mock || self.preferences.developer_mode;
        let files = session.project_files.borrow().clone();
        let view = self.view_mut(session.key);
        let caret = view.editor.selection().head;
        let text = view.editor.text();
        let selected = view.picker.as_ref().map_or(0, |p| p.selected);
        // `/word` alone, the caret in it: commands.
        if let Some(word) = slash_commands::active_query(&text, caret) {
            let items: Vec<PickerItem> = slash_commands::filter(&slash_commands::available(mock), word)
                .into_iter()
                .map(|c| PickerItem {
                    label: format!("/{}", c.name),
                    detail: if c.arguments.is_empty() { c.summary.to_owned() } else { format!("{} — {}", c.arguments, c.summary) },
                    insert: c.completion(),
                })
                .collect();
            view.picker = (!items.is_empty()).then(|| Picker {
                kind: PickerKind::Command,
                start: 0,
                selected: selected.min(items.len().saturating_sub(1)),
                items,
            });
            return;
        }
        // `@word` at the caret: files of the project.
        let prefix: String = text.chars().take(caret).collect();
        let picker = file_picker::active_at_query(&prefix).and_then(|(query, start)| {
            let items: Vec<PickerItem> = file_picker::filter_project_files(&files, &query, PICKER_ROWS)
                .into_iter()
                .map(|path| {
                    let (dir, name) = match path.rsplit_once('/') {
                        Some((dir, name)) => (dir.to_owned(), name.to_owned()),
                        None => (String::new(), path.clone()),
                    };
                    PickerItem { label: name, detail: dir, insert: format!("@{path} ") }
                })
                .collect();
            (!items.is_empty()).then(|| Picker {
                kind: PickerKind::File,
                start,
                selected: selected.min(items.len().saturating_sub(1)),
                items,
            })
        });
        view.picker = picker;
    }

    pub(super) fn picker_move(&mut self, delta: isize) {
        let key = self.session().key;
        if let Some(picker) = &mut self.view_mut(key).picker {
            let len = picker.items.len() as isize;
            if len > 0 {
                picker.selected = (picker.selected as isize + delta).rem_euclid(len) as usize;
            }
        }
    }

    pub(super) fn picker_choose(&mut self, index: Option<usize>) {
        let key = self.session().key;
        let view = self.view_mut(key);
        let Some(picker) = view.picker.take() else { return };
        let Some(item) = picker.items.get(index.unwrap_or(picker.selected)) else { return };
        let caret = view.editor.selection().head;
        view.editor.perform(EditorAction::Replace { range: picker.start..caret, text: item.insert.clone() });
        let end = picker.start + item.insert.chars().count();
        view.editor.set_selection(Selection::caret(end));
    }

    /// ↑ at the top of the composer, ↓ at its foot: the prompts sent before.
    pub(super) fn recall(&mut self, direction: isize) {
        let session = self.session();
        let current = self.composer_text(session.key);
        let recalled = session.prompt_history.borrow_mut().navigate(direction, &current);
        if let Some(text) = recalled {
            self.set_composer_text(session.key, &text);
        }
    }

    pub(super) fn send(&mut self, cx: &mut Cx<Msg>) {
        let session = self.session();
        let key = session.key;
        if self.quitting || self.fork_owns(key) {
            return;
        }
        let mut value = self.composer_text(key);
        if let Some(local) = slash_commands::parse_local(&value) {
            match self.run_local(&session, local, &value, cx) {
                Some(request) => value = request,
                None => return,
            }
        }
        if session.attachment_submission.get() {
            return;
        }
        let refuse = |app: &mut App, text: &str, cx: &mut Cx<Msg>| app.note(text.to_owned(), cx);
        if session.session_history.borrow().is_some() {
            return refuse(self, "Still loading this conversation • send again in a moment", cx);
        }
        if session.resuming_session_id.borrow().is_some() {
            return refuse(self, "Wait for this session to finish resuming", cx);
        }
        if session.read_only_session.get() {
            return refuse(self, "This session is read-only • start a new session to send a prompt", cx);
        }
        if session.signed_out() {
            return refuse(self, "Sign in to an account first: Settings ▸ Accounts", cx);
        }
        if session.pending_attachments.borrow().blocked() {
            return refuse(self, "Wait for attachments to finish, or remove failed ones", cx);
        }
        if session.chat_busy.get() && !session.pending_attachments.borrow().is_empty() {
            return refuse(self, "Attachments go with your next message; wait for this turn to finish", cx);
        }
        if session.pending_prompt.borrow().is_some() {
            return refuse(self, "Your first prompt is queued while the agent starts", cx);
        }
        if value.trim().is_empty() && session.pending_attachments.borrow().is_empty() {
            return;
        }
        self.view_mut(key).picker = None;
        if session.chat_busy.get() {
            // Steering: the turn under way takes it as guidance.
            if !session.conversation.borrow_mut().steer_user(&value) {
                return;
            }
            session.prompt_history.borrow_mut().record(&value);
            self.set_composer_text(key, "");
            let sent = session.omp_runtime.get().is_some_and(|runtime| runtime.steer(value.into()));
            if !sent {
                self.note("The agent stopped answering while steering", cx);
            }
            self.bump_scroll(key);
            return;
        }
        let adding_images = session.pending_attachments.borrow().items.iter().any(|item| {
            item.result.as_ref().and_then(|r| r.as_ref().ok()).is_some_and(|a| a.mime_type.starts_with("image/"))
        });
        if adding_images
            && AuthProvider::for_model(&session.current_model.borrow()) == Some(AuthProvider::Anthropic)
            && !session.image_cache_warning_seen.get()
        {
            let model = session.current_model.borrow().clone();
            if let Some(warning) =
                CacheWarning::for_action(&model, CacheAction::ImagePresenceChange { from: false, to: true })
            {
                session.cache_warning.replace(Some(warning));
            }
            session.image_cache_warning_seen.set(true);
        }
        let mut display = value.clone();
        for item in &session.pending_attachments.borrow().items {
            display.push_str(&format!("\n\nAttached: {}", item.name));
        }
        if !session.conversation.borrow_mut().submit_user(&display) {
            return;
        }
        session.prompt_history.borrow_mut().record(&value);
        self.set_composer_text(key, "");
        self.bump_scroll(key);
        if session.live_session_id.borrow().is_some() {
            session.live_state.replace(crate::agents::AgentState::Working);
        }
        let prepared = session.pending_attachments.borrow().prepared();
        session.pending_attachments.borrow_mut().clear();
        session.chat_busy.set(true);
        session.attachment_submission.set(true);
        let generation = session.attachment_generation.get();
        cx.spawn(move || Msg::PromptReady(key, generation, Arc::new(attachments::compose_prompt(value, &prepared))));
        self.schedule_save_views(cx);
    }

    pub(super) fn prompt_ready(&mut self, key: SessionKey, generation: u64, prompt: attachments::PreparedPrompt, cx: &mut Cx<Msg>) {
        let Some(session) = self.live.get(key) else { return };
        if generation != session.attachment_generation.get() {
            return;
        }
        session.attachment_submission.set(false);
        session.chat_busy.set(false);
        session.pending_prompt.replace(Some(prompt));
        self.dispatch_prompt(&session, cx);
    }

    pub(crate) fn bump_scroll(&mut self, key: SessionKey) {
        let view = self.view_mut(key);
        view.scroll_token = view.scroll_token.wrapping_add(1);
    }

    /// A line Omperor answers itself: `/usage`, `/omperor`. What to send
    /// after, if anything.
    fn run_local(
        &mut self,
        session: &LiveSession,
        local: slash_commands::Local,
        value: &str,
        cx: &mut Cx<Msg>,
    ) -> Option<String> {
        let key = session.key;
        let request = match &local {
            slash_commands::Local::Omperor(request) => Some(request.clone()),
            _ => None,
        };
        if request.as_deref().is_none_or(str::is_empty) {
            session.prompt_history.borrow_mut().record(value);
        }
        self.set_composer_text(key, "");
        let (all, note) = match local {
            slash_commands::Local::Usage { all } => (all, None),
            slash_commands::Local::UsageArguments(arguments) => {
                (false, Some(format!("/usage takes only “all”; “{arguments}” was ignored.")))
            }
            slash_commands::Local::Omperor(request) => {
                self.set_omperor_tools(session, true, cx);
                return (!request.is_empty()).then_some(request);
            }
            slash_commands::Local::OmperorOff => {
                self.set_omperor_tools(session, false, cx);
                return None;
            }
        };
        self.request_account_usage(true, cx);
        let mut body = self.usage_body(session, all);
        body.note = note;
        let mock = session.backend.get() == crate::mock_inference::Backend::Mock;
        if mock {
            body.card = Some(usage::live_card(&session.live_receipts.borrow()));
            let source = "Temporary mock session: figures from this run's live receipts.".to_owned();
            body.note = Some(match body.note.take() {
                Some(note) => format!("{note} {source}"),
                None => source,
            });
        }
        let index = session.conversation.borrow_mut().push_usage(body.encode());
        self.bump_scroll(key);
        if !mock {
            let path = session.header_session.borrow().as_ref().map(|(_, path)| path.clone());
            let children: Vec<PathBuf> =
                session.subagents.borrow().iter().filter_map(|a| a.session_file.clone()).collect();
            let model = session.current_model.borrow().clone();
            let job = self.begin_job("Reading this session's requests…");
            cx.spawn(move || {
                let report = usage::read(path, children, model);
                let mut body = body;
                body.card = Some(report.card);
                Msg::UsageRead(job, key, index, Arc::new(body))
            });
        }
        None
    }

    /// What a `/usage` card shows besides the receipts.
    pub(crate) fn usage_body(&self, session: &LiveSession, all: bool) -> usage_card::Body {
        let labels = session
            .subagents
            .borrow()
            .iter()
            .filter_map(|agent| {
                let stem = agent.session_file.as_ref()?.file_stem()?.to_string_lossy().into_owned();
                let label = if agent.label.is_empty() || agent.label == agent.id {
                    agent.agent.clone()
                } else {
                    format!("{} · {}", agent.agent, agent.label)
                };
                Some((stem, label))
            })
            .collect();
        let rates = self.views.get(&session.key).map(|v| v.rates.iter().copied().collect()).unwrap_or_default();
        let account = session.current_account.borrow().clone();
        let mut body = usage_card::Body { labels, rates, account, all, ..usage_card::Body::default() };
        self.fill_account_blocks(session, &mut body);
        body
    }

    /// The accounts' last known usage, into a card.
    pub(crate) fn fill_account_blocks(&self, session: &LiveSession, body: &mut usage_card::Body) {
        body.at_ms = crate::rules::wall_clock_ms();
        let block = |entry: &crate::rules::AccountEntry| {
            usage_card::account_block(entry.id(), &entry.display_name(), entry.provider(), self.account_usage.get(entry.id()))
        };
        body.accounts = if body.all {
            AuthProvider::ALL
                .iter()
                .flat_map(|provider| self.accounts.iter().filter(move |e| e.provider() == *provider && e.is_usable()))
                .map(block)
                .collect()
        } else {
            body.account.as_deref().and_then(|id| self.account(id)).map(block).into_iter().collect()
        };
        body.limits.clear();
        body.refreshing = self.usage_loading;
        body.fetched_ms = self.usage_fetched_ms.filter(|_| !self.account_usage.is_empty());
        let cached = body.accounts.iter().any(|b| b.reported);
        body.accounts_note = (!session.runtime_ready.get()).then(|| {
            if cached {
                "Start the session to read current usage; these are the last values read.".to_owned()
            } else {
                "Start the session to read current usage.".to_owned()
            }
        });
    }

    pub(super) fn usage_read(&mut self, job: u64, key: SessionKey, index: usize, body: &usage_card::Body) {
        self.finish_job(job);
        let Some(session) = self.live.get(key) else { return };
        let mut body = body.clone();
        self.fill_account_blocks(&session, &mut body);
        session.conversation.borrow_mut().set_usage(index, body.encode());
    }

    /// OMP answered the accounts' usage: the cards on screen take it in.
    pub(crate) fn refresh_usage_cards(&mut self) {
        for session in self.live.entries() {
            let count = session.conversation.borrow().len();
            for index in 0..count {
                let Some(message) = session.conversation.borrow().message(index) else { continue };
                if message.kind != crate::chat::TranscriptMessageKind::Usage {
                    continue;
                }
                let mut body = usage_card::Body::decode(&message.body);
                if !body.refreshing && body.card.is_some() {
                    continue;
                }
                self.fill_account_blocks(&session, &mut body);
                session.conversation.borrow_mut().set_usage(index, body.encode());
            }
        }
    }

    fn set_omperor_tools(&mut self, session: &LiveSession, on: bool, cx: &mut Cx<Msg>) {
        session.omperor_tools.set(on);
        if let Some(runtime) = session.omp_runtime.get() {
            runtime.set_omperor_tools(on);
        }
        self.note(
            if on {
                "Omperor's tools are on for this session: the agent can list, archive and delete sessions"
            } else {
                "Omperor's tools are off for this session"
            },
            cx,
        );
    }

    // Attachments.

    pub(super) fn attachment_dir(&self) -> PathBuf {
        crate::app_state::root().map_or_else(|_| std::env::temp_dir().join("omperor-attachments"), |r| r.join("attachments"))
    }

    pub(super) fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<Msg>) {
        let session = self.session();
        if session.read_only_session.get() {
            self.note("This session is read-only", cx);
            return;
        }
        let dir = self.attachment_dir();
        for path in paths {
            let name = crate::rules::path_leaf(&path);
            let id = session.pending_attachments.borrow_mut().insert(name);
            let key = session.key;
            let dir = dir.clone();
            cx.spawn(move || {
                let result = attachments::prepare(AttachmentSource::File(path), &dir);
                Msg::AttachmentReady(key, id, Arc::new(result))
            });
        }
        self.schedule_save_views(cx);
    }

    /// A paste the editor left to us: a picture, or files.
    pub(super) fn paste_data(&mut self, types: &[String], cx: &mut Cx<Msg>) {
        let image = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/tiff"]
            .into_iter()
            .find(|mime| types.iter().any(|t| t == mime));
        // Another program's picture or file list can take a while to come:
        // it is read, and prepared, on a worker, the chip showing meanwhile.
        if let Some(mime) = image {
            let session = self.session();
            let extension = mime.trim_start_matches("image/");
            let name = format!("Pasted image.{extension}");
            let id = session.pending_attachments.borrow_mut().insert(name.clone());
            let key = session.key;
            let dir = self.attachment_dir();
            cx.read_clipboard(mime, move |bytes| {
                let result = match bytes {
                    Some(bytes) => attachments::prepare(AttachmentSource::Bytes { name, bytes }, &dir),
                    None => Err("The clipboard's picture could not be read".to_owned()),
                };
                Msg::AttachmentReady(key, id, Arc::new(result))
            });
            return;
        }
        if types.iter().any(|t| t == "text/uri-list") {
            cx.read_clipboard("text/uri-list", |bytes| {
                Msg::AttachPaths(bytes.map(|b| files_in_uri_list(&String::from_utf8_lossy(&b))).unwrap_or_default())
            });
        }
    }

    pub(super) fn dropped(&mut self, urls: Vec<String>, text: Option<String>, cx: &mut Cx<Msg>) {
        let paths = files_in_uri_list(&urls.join("\n"));
        if !paths.is_empty() {
            self.attach_paths(paths, cx);
        } else if let Some(text) = text {
            let key = self.session().key;
            self.view_mut(key).editor.perform(EditorAction::Paste(text));
        }
    }

    pub(super) fn attachment_ready(
        &mut self,
        key: SessionKey,
        id: u64,
        result: &Result<attachments::PreparedAttachment, String>,
        cx: &mut Cx<Msg>,
    ) {
        let Some(session) = self.live.get(key) else { return };
        if session.pending_attachments.borrow_mut().complete(id, result.clone())
            && let Err(error) = result
        {
            self.note(format!("Could not attach: {error}"), cx);
        }
    }

    // The model, reasoning, account and Fast.

    pub(super) fn request_model(&mut self, model: String, cx: &mut Cx<Msg>) {
        let session = self.session();
        if session.backend.get() == crate::mock_inference::Backend::Mock {
            return;
        }
        let current = session.current_model.borrow().clone();
        if current == model {
            return;
        }
        if model_change_requires_new_session(&current, &model) {
            // Another provider: a fresh session of its own, with an account
            // of that provider settled first.
            let project = self.project_key(&session);
            let provider = AuthProvider::for_model(&model);
            let account = match provider {
                Some(provider) if session.accounts_known.get() => {
                    let choice = self.account_choices.get(&(project, provider.id().to_owned())).cloned();
                    match resolve_provider_account(&self.accounts, provider, choice.as_deref(), None) {
                        AccountResolution::Chosen(id) => Some(id),
                        AccountResolution::Unavailable => None,
                        AccountResolution::NeedsChoice => {
                            self.ask_account(provider, Some(model), cx);
                            return;
                        }
                    }
                }
                _ => None,
            };
            self.start_with_model(model, account, cx);
            return;
        }
        session.chat_status_text.replace(format!("Switching to {model}…"));
        if session.omp_runtime.get().is_some_and(|runtime| runtime.set_model(model.clone())) {
            if let Some(warning) = CacheWarning::for_action(&model, CacheAction::ModelSwitch { from: &current, to: &model }) {
                session.cache_warning.replace(Some(warning));
            }
            self.preferences.model = Some(model);
            self.store_preferences(cx);
        } else {
            // No child yet: the model goes with the launch.
            session.current_model.replace(model.clone());
            self.preferences.model = Some(model);
            self.store_preferences(cx);
        }
    }

    /// A fresh session on `model`, in the project in view.
    pub(crate) fn start_with_model(&mut self, model: String, account: Option<String>, cx: &mut Cx<Msg>) {
        let previous = self.session();
        let warning = {
            let from = previous.current_model.borrow().clone();
            CacheWarning::for_action(&model, CacheAction::ModelSwitch { from: &from, to: &model })
        };
        let directory = previous.current_project.borrow().clone();
        let project = previous.active_project_id.borrow().clone();
        self.preferences.model = Some(model.clone());
        self.store_preferences(cx);
        match project {
            Some(project) => {
                self.create_session(&project, directory, None, cx);
                let session = self.session();
                session.current_model.replace(model);
                session.cache_warning.replace(warning);
                if let Some(runtime) = session.omp_runtime.get() {
                    // The child was launched with the old model a moment
                    // ago: switch it now, before anything is sent.
                    let model = session.current_model.borrow().clone();
                    runtime.set_model(model);
                    if let Some(account) = account {
                        runtime.set_account(account);
                    }
                }
            }
            None => self.note("Choose a project before changing provider", cx),
        }
    }

    pub(super) fn request_thinking(&mut self, level: String, cx: &mut Cx<Msg>) {
        let session = self.session();
        let current = session.current_thinking_level.borrow().clone();
        if current == level {
            return;
        }
        self.preferences.thinking_level = Some(level.clone());
        self.store_preferences(cx);
        match session.omp_runtime.get() {
            Some(runtime) => {
                session.chat_status_text.replace(format!("Setting reasoning to {level}…"));
                if runtime.set_thinking_level(level.clone()) {
                    let model = session.current_model.borrow().clone();
                    if let Some(warning) =
                        CacheWarning::for_action(&model, CacheAction::ReasoningChange { from: &current, to: &level })
                    {
                        session.cache_warning.replace(Some(warning));
                    }
                }
            }
            None => {
                session.current_thinking_level.replace(level);
            }
        }
    }

    pub(super) fn request_fast(&mut self, on: bool, cx: &mut Cx<Msg>) {
        let session = self.session();
        let previous = session.fast_mode.replace(on);
        if let Some(runtime) = session.omp_runtime.get() {
            if runtime.set_fast_mode(on) {
                let model = session.current_model.borrow().clone();
                if let Some(warning) =
                    CacheWarning::for_action(&model, CacheAction::FastModeChange { from: previous, to: on })
                {
                    session.cache_warning.replace(Some(warning));
                }
            } else {
                session.fast_mode.set(previous);
                self.note("The agent stopped answering", cx);
            }
        }
    }

    pub(super) fn request_account(&mut self, account: String, cx: &mut Cx<Msg>) {
        let session = self.session();
        if session.current_account.borrow().as_deref() == Some(account.as_str()) && !session.account_change_pending.get() {
            return;
        }
        let current = session.current_account.borrow().clone().unwrap_or_default();
        if session.account_change_pending.replace(true) {
            return;
        }
        if let Some(entry) = self.account(&account).cloned() {
            self.remember_account(&session, entry.provider(), &account, cx);
            session.chat_status_text.replace(format!("Switching to {}…", entry.display_name()));
        }
        let model = session.current_model.borrow().clone();
        let warning = CacheWarning::for_action(&model, CacheAction::AccountSwitch { from: &current, to: &account });
        match session.omp_runtime.get() {
            Some(runtime) => {
                if runtime.set_account(account) {
                    if let Some(warning) = warning {
                        session.cache_warning.replace(Some(warning));
                    }
                } else {
                    session.account_change_pending.set(false);
                    self.note("The agent stopped answering", cx);
                }
            }
            None => {
                session.current_account.replace(Some(account));
                session.account_change_pending.set(false);
                self.dispatch_prompt(&session, cx);
            }
        }
    }

    /// Remember `account` as the project's choice for its provider.
    pub(crate) fn remember_account(&mut self, session: &LiveSession, provider: AuthProvider, account: &str, cx: &mut Cx<Msg>) {
        let project = self.project_key(session);
        let account = account.to_owned();
        self.account_choices.insert((project.clone(), provider.id().into()), account.clone());
        self.write(cx, move || crate::preferences::set_project_account(&project, provider.id(), &account));
    }

    // The `@` picker's files.

    pub(crate) fn index_project_files(&mut self, session: &LiveSession, cx: &mut Cx<Msg>) {
        let dir = session.current_project.borrow().clone();
        if session.project_files_indexing.get() || session.project_files_dir.borrow().as_ref() == Some(&dir) {
            return;
        }
        session.project_files_indexing.set(true);
        let key = session.key;
        cx.spawn(move || {
            let index = file_picker::index_project_files(&dir);
            Msg::ProjectFiles(key, dir, Arc::new(index))
        });
    }

    pub(super) fn project_files(
        &mut self,
        key: SessionKey,
        dir: PathBuf,
        index: Arc<file_picker::ProjectIndex>,
        cx: &mut Cx<Msg>,
    ) {
        let Some(session) = self.live.get(key) else { return };
        session.project_files_indexing.set(false);
        session.project_files.replace(Arc::new(index.files.clone()));
        session.project_files_dir.replace(Some(dir.clone()));
        self.watch_project(&session, dir, &index.ignore_dirs, cx);
    }
}

/// The local files a `text/uri-list` names. Whether each is there is the
/// attachment worker's to find out.
fn files_in_uri_list(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let path = l.strip_prefix("file://")?;
            let path = path.strip_prefix("localhost").unwrap_or(path);
            Some(PathBuf::from(percent_decode(path)))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::files_in_uri_list;

    #[test]
    fn a_uri_list_names_local_files_decoded() {
        let list = "# comment\nfile:///tmp/a%20b.png\nfile://localhost/tmp/c.txt\nhttps://example.test/a\n";
        assert_eq!(
            files_in_uri_list(list),
            vec![std::path::PathBuf::from("/tmp/a b.png"), std::path::PathBuf::from("/tmp/c.txt")]
        );
    }
}
