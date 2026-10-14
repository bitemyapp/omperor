//! A nonmodal evidence drawer. Selecting a task never takes keyboard focus
//! away from the conversation; full history remains one action away.
use super::*;

#[derive(Debug)]
pub(super) struct AgentInspection {
    panel: Retained<NSView>,
    title: Retained<NSTextField>,
    metadata: Retained<NSTextField>,
    open: Retained<NSButton>,
    scroll: Retained<NSScrollView>,
    text: Retained<NSTextView>,
    spinner: Retained<NSProgressIndicator>,
    rendered: RefCell<String>,
    rendered_task: RefCell<Option<String>>,
    rendered_session: RefCell<Option<live_sessions::SessionKey>>,
}

impl AgentInspection {
    pub(super) fn new(parent: &NSView, delegate: &AppDelegate, mtm: MainThreadMarker) -> Self {
        let panel = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(12.0, 64.0, INSPECTOR_WIDTH - 24.0, 280.0),
        );
        let background = fill_box(
            rect(0.0, 0.0, INSPECTOR_WIDTH - 24.0, 280.0),
            &theme::canvas(),
            8.0,
            mtm,
        );
        background.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        panel.addSubview(&background);
        let title = text_label("", 13.0, true, mtm);
        title.setFrame(rect(12.0, 246.0, INSPECTOR_WIDTH - 74.0, 20.0));
        title.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        panel.addSubview(&title);
        let metadata = text_label("", 12.0, false, mtm);
        metadata.setTextColor(Some(&theme::secondary_text()));
        metadata.setFrame(rect(12.0, 222.0, INSPECTOR_WIDTH - 78.0, 20.0));
        metadata.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        panel.addSubview(&metadata);
        let spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            rect(INSPECTOR_WIDTH - 52.0, 224.0, 14.0, 14.0),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        spinner.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        panel.addSubview(&spinner);
        let close = icon_button(
            "xmark",
            "Close task inspection",
            rect(INSPECTOR_WIDTH - 54.0, 244.0, 26.0, 26.0),
            delegate,
            sel!(closeAgentInspection:),
            mtm,
        );
        close.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        panel.addSubview(&close);
        let open = navigation_button(
            "Inspect in Observatory",
            "arrow.up.right",
            rect(8.0, 8.0, INSPECTOR_WIDTH - 40.0, 26.0),
            delegate,
            sel!(pinInspectedTask:),
            mtm,
        );
        open.setFont(Some(&theme::ui_font(12.0, true)));
        open.setContentTintColor(Some(&theme::accent()));
        open.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        panel.addSubview(&open);
        let (scroll, text) =
            transcript_control(rect(4.0, 40.0, INSPECTOR_WIDTH - 32.0, 174.0), mtm);
        text.setRichText(true);
        text.setTextContainerInset(NSSize::new(10.0, 6.0));
        text.setAccessibilityLabel(Some(ns_string!("Selected task’s latest recorded output")));
        panel.addSubview(&scroll);
        panel.setHidden(true);
        parent.addSubview(&panel);
        Self {
            panel,
            title,
            metadata,
            open,
            scroll,
            text,
            spinner,
            rendered: RefCell::new(String::new()),
            rendered_task: RefCell::new(None),
            rendered_session: RefCell::new(None),
        }
    }
}

impl AppDelegate {
    pub(super) fn inspect_agent_row(&self, sender: &AnyObject) {
        let owner = self.session();
        self.capture_agent_inspection_position();
        let Some(index) = Self::sender_row(sender) else {
            return;
        };
        let id = match self.ivars().agents_rows.borrow().get(index) {
            Some(agents::AgentRow::Subagent { id, .. }) => id.clone(),
            _ => return,
        };
        self.schedule_session_view_save();
        owner
            .agent_inspection_generation
            .set(owner.agent_inspection_generation.get().wrapping_add(1));
        owner.agent_inspection_loading.replace(None);
        owner.agent_inspection_failure.replace(None);
        let cached = owner.subagent_transcripts.borrow().contains_key(&id);
        let path = owner
            .subagents
            .borrow()
            .iter()
            .find(|info| info.id == id)
            .and_then(|info| info.session_file.clone());
        if !cached && let Some(path) = path {
            let generation = owner.agent_inspection_generation.get();
            let session = owner.selected_session_path.borrow().clone();
            let key = owner.key;
            let task = id.clone();
            let submitted = self
                .jobs()
                .submit(move || BackgroundEvent::AgentInspectionRead {
                    key,
                    generation,
                    session,
                    id: task,
                    result: transcripts::read(&path),
                });
            if submitted {
                owner.agent_inspection_loading.replace(Some(id.clone()));
            } else {
                owner.agent_inspection_failure.replace(Some((
                    id.clone(),
                    "Could not start reading the task journal.".into(),
                )));
            }
        }
        // Preserve the tree position when inspecting a task.
        let key = owner.selected_session_path.borrow().clone();
        self.ivars()
            .agent_view_states
            .borrow_mut()
            .entry(key)
            .or_default()
            .selected = Some(id);
        self.refresh_agents_panel();
    }

    pub(super) fn close_agent_inspection(&self) {
        let owner = self.session();
        self.schedule_session_view_save();
        let key = owner.selected_session_path.borrow().clone();
        self.ivars()
            .agent_view_states
            .borrow_mut()
            .entry(key)
            .or_default()
            .selected = None;
        owner
            .agent_inspection_generation
            .set(owner.agent_inspection_generation.get().wrapping_add(1));
        owner.agent_inspection_loading.replace(None);
        self.refresh_agents_panel();
    }

    pub(super) fn capture_agent_inspection_position(&self) {
        let owner = self.session();
        let Some(ui) = self.ivars().agent_inspection.get() else {
            return;
        };
        if *ui.rendered_session.borrow() != Some(owner.key) {
            return;
        }
        if let Some(id) = ui.rendered_task.borrow().clone() {
            let y = ui.scroll.contentView().bounds().origin.y;
            self.edit_desk(|desk| {
                desk.peek_y.insert(id, y);
            });
        }
    }

    /// Return the height reserved below the tree. Only the latest short output
    /// is rendered here; reading an entire journal stays on a worker.
    pub(super) fn refresh_agent_inspection(&self) -> f64 {
        let owner = self.session();
        let ivars = self.ivars();
        let Some(ui) = ivars.agent_inspection.get() else {
            return 0.0;
        };
        let id = self.agent_view().selected;
        let board = owner.subagents.borrow();
        let Some(info) = board.iter().find(|info| Some(&info.id) == id.as_ref()) else {
            ui.panel.setHidden(true);
            return 0.0;
        };
        let bounds = ivars.agents_panel.get().unwrap().bounds().size;
        let height = (bounds.height * 0.34).clamp(224.0, 320.0);
        ui.panel
            .setFrame(rect(12.0, 64.0, bounds.width - 24.0, height));
        ui.panel.setHidden(false);
        ui.title.setStringValue(&NSString::from_str(&info.label));
        ui.title.setToolTip(Some(&NSString::from_str(&info.label)));
        let loading = owner.agent_inspection_loading.borrow().as_deref() == Some(info.id.as_str());
        // SAFETY: a nil sender is valid and this indicator is main-thread owned.
        unsafe {
            if loading {
                ui.spinner.startAnimation(None);
            } else {
                ui.spinner.stopAnimation(None);
            }
        }
        let metadata = if loading {
            "Reading journal…".into()
        } else {
            format!(
                "{} · {} · {}",
                agents::AgentState::from_status(&info.status).label(),
                info.agent,
                info.id
            )
        };
        ui.metadata.setStringValue(&NSString::from_str(&metadata));
        ui.metadata
            .setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        ui.metadata.setToolTip(Some(&NSString::from_str(&metadata)));
        ui.open.setEnabled(true);
        let cache = owner.subagent_transcripts.borrow();
        let latest = cache.get(&info.id).and_then(|entry| {
            entry.messages.iter().rev().find(|message| {
                message.speaker != session::Speaker::User && !message.text.trim().is_empty()
            })
        });
        let failure = owner.agent_inspection_failure.borrow();
        let mut body = String::new();
        if let Some(note) = info.error.as_ref().or(info.warning.as_ref()) {
            body.extend(note.chars().take(800));
            body.push_str("\n\n---\n\n");
        }
        if let Some(message) = latest {
            body.extend(message.text.chars().take(2000));
            if message.text.chars().nth(2000).is_some() {
                body.push_str("\n\n… Inspect in Observatory to read further.");
            }
        } else if let Some((task, error)) = &*failure
            && task == &info.id
        {
            body.push_str(error);
        } else if loading {
            body.push_str("Loading the latest recorded output…");
        } else if info.session_file.is_none() {
            body.push_str("This task has not published a journal yet. Its state will continue to update above.");
        } else {
            body.push_str("No recorded output yet. The latest output will appear here as the task writes its journal.");
        }
        let preferences = ivars.preferences.borrow();
        let typography = native_markdown::Typography::new(
            preferences.chat_style,
            preferences.chat_text_size.scale(),
        )
        .with_sizes(preferences.chat_text_sizes);
        let width = ui.scroll.contentSize().width;
        let fingerprint = format!(
            "{}:{width}:{:?}:{:?}:{:?}:{body}",
            info.id,
            preferences.chat_style,
            preferences.chat_text_size,
            preferences.chat_text_sizes
        );
        if *ui.rendered.borrow() != fingerprint || *ui.rendered_session.borrow() != Some(owner.key)
        {
            let same_task = ui.rendered_task.borrow().as_deref() == Some(info.id.as_str())
                && *ui.rendered_session.borrow() == Some(owner.key);
            let previous_y = ui.scroll.contentView().bounds().origin.y;
            let message = chat::TranscriptMessage {
                author: "Latest recorded output".into(),
                body,
                kind: chat::TranscriptMessageKind::Assistant,
                status: None,
                detail_id: None,
            };
            let rendered = native_markdown::render_message(
                &message,
                width - 40.0,
                &owner.current_project.borrow(),
                typography,
                false,
            );
            // SAFETY: NSTextView owns its storage, text container, and layout manager.
            unsafe {
                if let (Some(storage), Some(container), Some(layout)) = (
                    ui.text.textStorage(),
                    ui.text.textContainer(),
                    ui.text.layoutManager(),
                ) {
                    container.setWidthTracksTextView(false);
                    container.setContainerSize(NSSize::new(width - 36.0, 1.0e7));
                    storage.setAttributedString(&rendered);
                    layout.ensureLayoutForTextContainer(&container);
                    ui.text.setFrameSize(NSSize::new(
                        width,
                        (layout.usedRectForTextContainer(&container).size.height + 12.0)
                            .max(ui.scroll.contentSize().height),
                    ));
                }
            }
            let clip = ui.scroll.contentView();
            let y = if same_task {
                previous_y
                    .min((ui.text.frame().size.height - ui.scroll.contentSize().height).max(0.0))
            } else {
                self.agent_view()
                    .desk
                    .peek_y
                    .get(&info.id)
                    .copied()
                    .unwrap_or(0.0)
                    .min((ui.text.frame().size.height - ui.scroll.contentSize().height).max(0.0))
            };
            clip.scrollToPoint(NSPoint::new(0.0, y));
            ui.scroll.reflectScrolledClipView(&clip);
            ui.rendered.replace(fingerprint);
            ui.rendered_task.replace(latest.map(|_| info.id.clone()));
            ui.rendered_session.replace(Some(owner.key));
        }
        height + 12.0
    }
}
