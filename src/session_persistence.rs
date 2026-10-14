//! Native scheduling for durable session reading state. Serialization and SQLite
//! run on the same ordered worker as preferences; no per-frame disk activity.
use super::*;

impl AppDelegate {
    pub(super) fn waiting_for_session_views(&self) -> bool {
        !self.ivars().session_views_resolved.get() && self.ivars().session_views_edited.get()
    }

    pub(super) fn complete_termination_if_ready(&self) {
        let ivars = self.ivars();
        if ivars.termination_pending.get()
            && !self.waiting_for_session_views()
            && ivars.preference_saves_pending.get() == 0
            && ivars
                .live_sessions
                .borrow()
                .entries()
                .iter()
                .all(|session| {
                    session
                        .omp_runtime
                        .get()
                        .is_none_or(|runtime| runtime.try_reap())
                })
        {
            if let Some(timer) = ivars.runtime_timer.get() {
                timer.invalidate();
            }
            NSApplication::sharedApplication(self.mtm()).replyToApplicationShouldTerminate(true);
        }
    }

    pub(super) fn schedule_session_view_save(&self) {
        let ivars = self.ivars();
        ivars.session_views_edited.set(true);
        if !ivars.session_views_loaded.get()
            || ivars.termination_pending.get()
            || ivars.session_save_scheduled.replace(true)
        {
            return;
        }
        // Coalesce interaction bursts. The run loop owns this one-shot timer.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                1.0,
                self,
                sel!(saveSessionViews:),
                None,
                false,
            )
        };
        unsafe {
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
    }

    pub(super) fn save_session_views(&self) {
        let ivars = self.ivars();
        if !ivars.session_views_loaded.get() {
            return;
        }
        self.capture_session_desk();
        let entries = ivars.live_sessions.borrow().entries();
        let mut views = ivars.agent_view_states.borrow_mut();
        for session in entries {
            let path = session.selected_session_path.borrow().clone();
            let desk = &mut views.entry(path.clone()).or_default().desk;
            // Background attachment completion must be saved even while another desk is visible.
            if session.attachments_restored.get()
                || !session.pending_attachments.borrow().items.is_empty()
            {
                desk.attachments = session.pending_attachments.borrow().saved();
            }
            if path.as_deref().and_then(session_state::draft_key).is_some() {
                desk.draft_context = Some(workspace_model::DraftContext {
                    project: session.current_project.borrow().clone(),
                    project_id: session.active_project_id.borrow().clone(),
                    model: session.current_model.borrow().clone(),
                    account: session.current_account.borrow().clone(),
                    thinking_level: session.current_thinking_level.borrow().clone(),
                    fast_mode: session.fast_mode.get(),
                });
            } else {
                desk.draft_context = None;
            }
        }
        let snapshot = session_state::SavedViews::capture(&views);
        drop(views);
        self.write_preferences(move || preferences::store_session_views(&snapshot));
    }

    pub(super) fn apply_saved_session_views(
        &self,
        result: Result<session_state::SavedViews, preferences::PreferencesError>,
    ) {
        let ivars = self.ivars();
        ivars.session_views_resolved.set(true);
        match result {
            Ok(saved) => {
                let edited = ivars.session_views_edited.get();
                let mut states = ivars.agent_view_states.borrow_mut();
                for (mut path, view) in saved.into_states() {
                    // Version 1 had a single anonymous draft; give it one durable identity.
                    if path.is_none() {
                        let entry = ivars.live_sessions.borrow_mut().create();
                        path = Some(session_state::draft_path(entry.key.0));
                        entry.selected_session_path.replace(path.clone());
                    }
                    if edited && states.contains_key(&path) {
                        continue;
                    }
                    if let Some(key) = path.as_deref().and_then(session_state::draft_key) {
                        let entry = ivars
                            .live_sessions
                            .borrow_mut()
                            .restore_draft(live_sessions::SessionKey(key));
                        if let Some(context) = &view.desk.draft_context {
                            entry.current_project.replace(context.project.clone());
                            entry.active_project_id.replace(context.project_id.clone());
                            entry.current_model.replace(context.model.clone());
                            entry.current_account.replace(context.account.clone());
                            entry
                                .current_thinking_level
                                .replace(context.thinking_level.clone());
                            entry.fast_mode.set(context.fast_mode);
                        }
                        entry.draft.replace(view.desk.draft.clone());
                    }
                    if edited {
                        states.entry(path).or_insert(view);
                    } else {
                        states.insert(path, view);
                    }
                }
                drop(states);
                ivars.session_views_loaded.set(true);
                // Drafts saved before empty ones were let go of.
                if self.discard_empty_drafts() {
                    self.schedule_session_view_save();
                }
                self.rebuild_sidebar_sessions();
                if !edited {
                    ivars.desk_restore_pending.set(true);
                    self.restore_session_desk();
                    self.refresh_chat_transcript();
                }
                if edited {
                    self.schedule_session_view_save();
                }
            }
            Err(error) => {
                // Preserve unsupported or damaged state instead of overwriting
                // the user's drafts with defaults on the next interaction.
                self.set_chat_status(&format!("Could not restore session views: {error}"));
                ivars.preference_save_error.replace(Some(error.to_string()));
                self.sync_preference_save_status();
            }
        }
        if ivars.termination_pending.get() {
            self.save_session_views();
            self.complete_termination_if_ready();
        }
    }
}
