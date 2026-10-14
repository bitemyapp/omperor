//! The Developer page: the scripted test backend, and the main thread's
//! frame budget shown live in the window.
use super::*;
use crate::mock_inference::Backend;

/// The frame-timing card's height; the Developer mode card's depends on
/// whether the mode is on.
const TIMING_CARD_HEIGHT: f64 = 130.0;

#[derive(Debug)]
pub(super) struct DeveloperSettingsUi {
    scroll: Retained<NSScrollView>,
    content: Retained<NSView>,
    card: Retained<NSBox>,
    toggle: Retained<NSButton>,
    explanation: Retained<NSTextField>,
    controls: Retained<NSView>,
    backend: Retained<NSPopUpButton>,
    example: Retained<NSButton>,
    timing_card: Retained<NSBox>,
    timing_toggle: Retained<NSButton>,
    state: Cell<Option<(bool, f64, f64)>>,
    laying_out: Cell<bool>,
}

impl AppDelegate {
    pub(super) fn build_developer_settings(
        &self,
        page: &NSView,
        mtm: MainThreadMarker,
    ) -> DeveloperSettingsUi {
        settings_page_header(
            page,
            "Developer",
            "A scripted backend for repeatable tests, and the main thread's frame budget in view",
            mtm,
        );
        let width = SETTINGS_WIDTH - SETTINGS_SIDEBAR_WIDTH;
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(
                0.0,
                16.0,
                width,
                SETTINGS_HEIGHT - SETTINGS_PAGE_HEADER_HEIGHT - 16.0,
            ),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, width, 568.0));
        content.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let card = settings_card(rect(40.0, 16.0, 600.0, 104.0), mtm);
        card.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        // SAFETY: selectors are implemented by AppDelegate for the window lifetime.
        let toggle = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Developer mode"),
                Some(self),
                Some(sel!(toggleDeveloperMode:)),
                mtm,
            )
        };
        toggle.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        card.addSubview(&toggle);
        let explanation = text_label(
            "Enable tools for repeatable UI and performance tests. OMP remains the backend until you choose Mock.",
            12.0,
            false,
            mtm,
        );
        explanation.setTextColor(Some(&theme::secondary_text()));
        explanation.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
        explanation.setMaximumNumberOfLines(2);
        explanation.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        card.addSubview(&explanation);
        let controls = NSView::initWithFrame(NSView::alloc(mtm), rect(20.0, 16.0, 556.0, 206.0));
        controls.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let label = text_label("Inference backend", 13.0, true, mtm);
        label.setFrame(rect(0.0, 170.0, 210.0, 24.0));
        controls.addSubview(&label);
        let backend = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(304.0, 166.0, 252.0, 30.0),
            false,
        );
        backend.addItemWithTitle(ns_string!("OMP — real agents"));
        backend.addItemWithTitle(ns_string!("Mock — scripted agent, no credentials"));
        backend.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        unsafe {
            backend.setTarget(Some::<&AnyObject>(self));
            backend.setAction(Some(sel!(selectInferenceBackend:)));
        }
        controls.addSubview(&backend);
        let detail = text_label(
            "Changing backend starts a new conversation. Mock plays a scripted agent turn locally — reasoning, tool calls, sub-agents, a streamed answer, a receipt — through the same events OMP sends, without credentials or tools. Its conversations are temporary and excluded from metrics.",
            12.0,
            false,
            mtm,
        );
        detail.setTextColor(Some(&theme::secondary_text()));
        detail.setMaximumNumberOfLines(3);
        detail.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
        detail.setFrame(rect(0.0, 112.0, 556.0, 50.0));
        detail.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        controls.addSubview(&detail);
        let syntax = text_label(mock_inference::EXAMPLE, 12.0, false, mtm);
        syntax.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
            12.0,
            unsafe { objc2_app_kit::NSFontWeightRegular },
        )));
        syntax.setSelectable(true);
        syntax.setFrame(rect(0.0, 80.0, 556.0, 24.0));
        syntax.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        controls.addSubview(&syntax);
        let help = text_label(
            "Type anything, or /mock scenario=agent|answer|subagents|error|auth|slow|stress|markdown.\nKnobs: ttft=ms, rate=chars/sec (0 unpaced), size=chars, file=path in the project,\ntemplate=text|markdown|code|table|unicode|stress.",
            11.0,
            false,
            mtm,
        );
        help.setTextColor(Some(&theme::secondary_text()));
        help.setMaximumNumberOfLines(3);
        help.setFrame(rect(0.0, 34.0, 556.0, 46.0));
        help.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        controls.addSubview(&help);
        let example = settings_button(
            "Insert example in chat",
            rect(0.0, 0.0, 180.0, 30.0),
            self,
            sel!(insertMockExample:),
            mtm,
        );
        example.setToolTip(Some(ns_string!(
            "Inserts a scenario line without sending it. Select Mock first."
        )));
        controls.addSubview(&example);
        card.addSubview(&controls);
        content.addSubview(&card);

        // The frame budget: always measured and logged; shown in the window
        // on request.
        let timing_card = settings_card(rect(40.0, 16.0, 600.0, TIMING_CARD_HEIGHT), mtm);
        timing_card.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let timing_title = text_label("Main thread", 13.0, true, mtm);
        timing_title.setFrame(rect(20.0, TIMING_CARD_HEIGHT - 42.0, 330.0, 21.0));
        timing_card.addSubview(&timing_title);
        // SAFETY: selectors are implemented by AppDelegate for the window lifetime.
        let timing_toggle = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Show frame timing in the window"),
                Some(self),
                Some(sel!(toggleFrameTiming:)),
                mtm,
            )
        };
        timing_toggle.setFrame(rect(20.0, TIMING_CARD_HEIGHT - 72.0, 340.0, 24.0));
        timing_card.addSubview(&timing_toggle);
        let timing_explanation = text_label(
            "The main thread is timed pass by pass against one 240 Hz frame (4 ms). The readout above the composer draws the latest passes against that line and names what ran longest in the last pass over budget; passes over budget are always logged to frames.jsonl beside the sessions.",
            11.0,
            false,
            mtm,
        );
        timing_explanation.setTextColor(Some(&theme::secondary_text()));
        timing_explanation.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
        timing_explanation.setMaximumNumberOfLines(3);
        if let Some(cell) = timing_explanation.cell() {
            cell.setWraps(true);
            cell.setUsesSingleLineMode(false);
        }
        timing_explanation.setFrame(rect(20.0, 12.0, 556.0, 44.0));
        timing_explanation.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        timing_card.addSubview(&timing_explanation);
        content.addSubview(&timing_card);

        scroll.setDocumentView(Some(&content));
        let clip = scroll.contentView();
        clip.setPostsFrameChangedNotifications(true);
        // SAFETY: the delegate implements this selector and outlives the clip.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(settingsDeveloperFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&clip),
            );
        }
        page.addSubview(&scroll);
        DeveloperSettingsUi {
            scroll,
            content,
            card,
            toggle,
            explanation,
            controls,
            backend,
            example,
            timing_card,
            timing_toggle,
            state: Cell::new(None),
            laying_out: Cell::new(false),
        }
    }

    pub(super) fn mock_backend_active(&self) -> bool {
        self.session().backend.get() == Backend::Mock
    }

    fn backend_can_change(&self) -> bool {
        !self.ivars().termination_pending.get()
    }

    /// Shows the frame-timing readout when Developer mode asks for it.
    pub(super) fn sync_frame_overlay(&self) {
        let shown = {
            let preferences = self.ivars().preferences.borrow();
            preferences.developer_mode && preferences.frame_timing_overlay
        };
        if let Some(overlay) = self.ivars().frame_overlay.get() {
            overlay.set_shown(shown, self);
        }
    }

    pub(super) fn change_frame_timing_overlay(&self) {
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let shown = settings.developer.timing_toggle.state() == NSControlStateValueOn;
        self.ivars().preferences.borrow_mut().frame_timing_overlay = shown;
        self.store_preferences();
        self.sync_developer_settings();
    }

    pub(super) fn sync_developer_settings(&self) {
        let owner = self.session();
        self.sync_frame_overlay();
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let ui = &settings.developer;
        let (enabled, timing) = {
            let preferences = self.ivars().preferences.borrow();
            (preferences.developer_mode, preferences.frame_timing_overlay)
        };
        ui.timing_toggle.setState(if timing {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        ui.timing_toggle.setEnabled(enabled);
        ui.timing_toggle.setToolTip(Some(ns_string!(
            "Turn on Developer mode to show the readout."
        )));
        ui.toggle.setState(if enabled {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        ui.toggle.setEnabled(self.backend_can_change());
        ui.toggle.setToolTip(Some(ns_string!(
            "Finish or stop the active turn before changing Developer mode."
        )));
        ui.backend
            .selectItemAtIndex(if self.mock_backend_active() { 1 } else { 0 });
        ui.backend.setEnabled(self.backend_can_change());
        ui.example.setEnabled(
            self.mock_backend_active()
                && owner.runtime_ready.get()
                && !owner.read_only_session.get()
                && !owner.chat_busy.get(),
        );
        ui.controls.setHidden(!enabled);
        if ui.laying_out.replace(true) {
            return;
        }
        let viewport = ui.scroll.contentSize();
        let layout = (enabled, viewport.width, viewport.height);
        let prior = ui.state.replace(Some(layout));
        if prior != Some(layout) {
            let height = if enabled { 324.0 } else { 104.0 };
            let width = viewport.width;
            let document_height =
                (16.0 + height + 16.0 + TIMING_CARD_HEIGHT + 16.0_f64).max(viewport.height);
            let old_height = ui.content.frame().size.height;
            let old_origin = ui.scroll.contentView().bounds().origin.y;
            ui.content.setFrameSize(NSSize::new(width, document_height));
            ui.card.setFrame(rect(
                40.0,
                document_height - 16.0 - height,
                width - 80.0,
                height,
            ));
            ui.timing_card.setFrame(rect(
                40.0,
                document_height - 32.0 - height - TIMING_CARD_HEIGHT,
                width - 80.0,
                TIMING_CARD_HEIGHT,
            ));
            ui.toggle.setFrame(rect(20.0, height - 42.0, 280.0, 26.0));
            ui.explanation
                .setFrame(rect(20.0, height - 82.0, width - 124.0, 38.0));
            // Anchor to the top even when the window is taller than its content.
            // Preserve the visible position as the developer section expands.
            let limit = (document_height - viewport.height).max(0.0);
            let origin = if prior.is_none() {
                limit
            } else {
                (old_origin + document_height - old_height).clamp(0.0, limit)
            };
            ui.scroll
                .contentView()
                .scrollToPoint(NSPoint::new(0.0, origin));
            ui.scroll.reflectScrolledClipView(&ui.scroll.contentView());
        }
        ui.laying_out.set(false);
    }

    pub(super) fn change_developer_mode(&self) {
        if !self.backend_can_change() {
            self.sync_developer_settings();
            return;
        }
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let was_mock = self.mock_backend_active();
        let enabled = settings.developer.toggle.state() == NSControlStateValueOn;
        {
            let mut preferences = self.ivars().preferences.borrow_mut();
            preferences.developer_mode = enabled;
            if !enabled {
                preferences.inference_backend = Backend::Omp;
            }
        }
        self.store_preferences();
        if was_mock && !enabled {
            self.switch_inference_backend(Backend::Omp);
        }
        self.sync_developer_settings();
    }

    pub(super) fn change_inference_backend(&self) {
        if !self.backend_can_change() {
            self.sync_developer_settings();
            return;
        }
        if !self.ivars().preferences.borrow().developer_mode {
            return;
        }
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let backend = if settings.developer.backend.indexOfSelectedItem() == 1 {
            Backend::Mock
        } else {
            Backend::Omp
        };
        if self.ivars().preferences.borrow().effective_backend() == backend {
            return;
        }
        self.ivars().preferences.borrow_mut().inference_backend = backend;
        self.store_preferences();
        self.switch_inference_backend(backend);
    }

    fn switch_inference_backend(&self, _backend: Backend) {
        self.set_chat_status("Backend preference saved • applies to new conversations; existing sessions are unchanged");
        self.sync_developer_settings();
    }

    pub(super) fn insert_mock_example(&self) {
        let owner = self.session();
        if !self.mock_backend_active()
            || !owner.runtime_ready.get()
            || owner.chat_busy.get()
            || owner.read_only_session.get()
        {
            return;
        }
        if let Some(input) = self.ivars().input.get() {
            // Keep an existing draft. The example is also selectable in Settings.
            if !input.stringValue().is_empty() {
                self.set_chat_status("Clear the draft before inserting the mock example");
                return;
            }
            input.setStringValue(&NSString::from_str(mock_inference::EXAMPLE));
            self.schedule_session_view_save();
            if let Some(settings) = self.ivars().settings_ui.get() {
                settings.window.orderOut(None);
            }
            if let Some(window) = self.ivars().window.get() {
                window.makeKeyAndOrderFront(None);
                window.makeFirstResponder(Some(input));
            }
        }
    }
}
