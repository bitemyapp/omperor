//! A small native action launcher. It searches a bounded, in-memory command
//! list and dispatches existing actions; it never discovers files or runs I/O.
use super::*;

pub(super) const FIELD_TAG: isize = -8801;
const ACTIONS: [(&str, &str, &str); 10] = [
    ("New conversation", "Start a task in this project", "plus"),
    (
        "Show agents",
        "Follow delegated work alongside conversation",
        "person.3",
    ),
    (
        "Show changes",
        "Review the workspace and file diffs",
        "square.stack.3d.up",
    ),
    (
        "Toggle focus",
        "Give the current view more room",
        "sidebar.right",
    ),
    (
        "Chat typography",
        "Calibrate text sizes with native examples",
        "textformat.size",
    ),
    ("Projects", "Manage project folders and defaults", "folder"),
    (
        "Accounts",
        "Manage providers and sign-in",
        "person.crop.circle",
    ),
    (
        "Search conversations",
        "Find a session in the sidebar",
        "magnifyingglass",
    ),
    (
        "Agent Observatory",
        "Inspect agents, follow their activity, and compare their work",
        "square.split.2x2",
    ),
    (
        "Conversation",
        "Return to the conversation and your draft",
        "text.bubble",
    ),
];

#[derive(Debug)]
pub(super) struct QuickActions {
    window: Retained<NSWindow>,
    query: Retained<NSSearchField>,
    results: Retained<NSView>,
    matches: Vec<usize>,
    selected: usize,
}

impl AppDelegate {
    pub(super) fn present_quick_actions(&self) {
        if self.ivars().quick_actions.borrow().is_some() {
            return;
        }
        let Some(host) = self.ivars().window.get() else {
            return;
        };
        if host.attachedSheet().is_some() {
            return;
        }
        let mtm = self.mtm();
        // SAFETY: the ivar owns this sheet until dismissal. Disable AppKit's
        // legacy release-on-close behavior for retained Rust ownership.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, 600.0, 544.0),
                NSWindowStyleMask::Titled | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setBackgroundColor(Some(&theme::canvas()));
        let content = window.contentView().unwrap();
        let title = text_label("Quick actions", 20.0, true, mtm);
        title.setFrame(rect(24.0, 498.0, 430.0, 28.0));
        content.addSubview(&title);
        let close = settings_button(
            "Close",
            rect(492.0, 497.0, 84.0, 28.0),
            self,
            sel!(closeQuickActions:),
            mtm,
        );
        close.setKeyEquivalent(ns_string!("\u{1b}"));
        content.addSubview(&close);
        let query =
            NSSearchField::initWithFrame(NSSearchField::alloc(mtm), rect(24.0, 451.0, 552.0, 34.0));
        query.setFont(Some(&theme::ui_font(16.0, false)));
        query.setPlaceholderString(Some(ns_string!("Find an action…")));
        query.setAccessibilityLabel(Some(ns_string!("Search quick actions")));
        query.setTag(FIELD_TAG);
        // SAFETY: AppDelegate owns the sheet and implements the edit delegate.
        unsafe {
            query.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        content.addSubview(&query);
        let results = NSView::initWithFrame(NSView::alloc(mtm), rect(16.0, 40.0, 568.0, 400.0));
        content.addSubview(&results);
        let help = section_label("↑ ↓ to choose   ·   Return to open   ·   Esc to close", mtm);
        help.setFrame(rect(24.0, 12.0, 552.0, 20.0));
        content.addSubview(&help);
        self.ivars().quick_actions.replace(Some(QuickActions {
            window: window.clone(),
            query: query.clone(),
            results,
            matches: Vec::new(),
            selected: 0,
        }));
        self.filter_quick_actions();
        host.beginSheet_completionHandler(&window, None);
        window.makeFirstResponder(Some(&*query));
    }

    pub(super) fn filter_quick_actions(&self) {
        {
            let mut state = self.ivars().quick_actions.borrow_mut();
            let Some(ui) = state.as_mut() else {
                return;
            };
            let query = ui.query.stringValue().to_string().to_lowercase();
            ui.matches = ACTIONS
                .iter()
                .enumerate()
                .filter(|(_, (title, detail, _))| {
                    let text = format!("{title} {detail}").to_lowercase();
                    query.split_whitespace().all(|word| text.contains(word))
                })
                .map(|(index, _)| index)
                .collect();
            ui.selected = 0;
        }
        self.render_quick_actions();
    }

    fn render_quick_actions(&self) {
        let state = self.ivars().quick_actions.borrow();
        let Some(ui) = state.as_ref() else {
            return;
        };
        let previous = ui.results.subviews();
        for index in 0..previous.count() {
            previous.objectAtIndex(index).removeFromSuperview();
        }
        if ui.matches.is_empty() {
            let note = text_label(
                "No matching actions. Try agents, changes, or text.",
                14.0,
                false,
                self.mtm(),
            );
            note.setFrame(rect(8.0, 340.0, 540.0, 28.0));
            ui.results.addSubview(&note);
        }
        let first_visible = ui.selected.saturating_sub(7);
        for (row, index) in ui
            .matches
            .iter()
            .copied()
            .skip(first_visible)
            .take(8)
            .enumerate()
        {
            let (title, detail, symbol) = ACTIONS[index];
            let y = 400.0 - (row as f64 + 1.0) * 49.0;
            if row + first_visible == ui.selected {
                let fill = fill_box(
                    rect(0.0, y, 568.0, 46.0),
                    &theme::accent_fill_strong(),
                    9.0,
                    self.mtm(),
                );
                ui.results.addSubview(&fill);
            }
            let icon = NSImageView::initWithFrame(
                NSImageView::alloc(self.mtm()),
                rect(12.0, y + 23.0, 18.0, 18.0),
            );
            icon.setImage(theme::row_symbol(symbol, 14.0, 0.0).as_deref());
            icon.setContentTintColor(Some(&theme::secondary_text()));
            ui.results.addSubview(&icon);
            let label = text_label(title, 14.0, true, self.mtm());
            label.setFrame(rect(38.0, y + 23.0, 510.0, 20.0));
            ui.results.addSubview(&label);
            let note = section_label(detail, self.mtm());
            note.setFrame(rect(38.0, y + 3.0, 507.0, 18.0));
            ui.results.addSubview(&note);
            // A single full-row target avoids duplicate keyboard stops.
            let hit = settings_button(
                "",
                rect(0.0, y, 568.0, 46.0),
                self,
                sel!(chooseQuickAction:),
                self.mtm(),
            );
            hit.setTag(index as isize);
            hit.setTransparent(true);
            hit.setAccessibilityLabel(Some(&NSString::from_str(&format!("{title}. {detail}"))));
            ui.results.addSubview(&hit);
        }
    }

    pub(super) fn quick_action_command(&self, command: objc2::runtime::Sel) -> bool {
        if command == sel!(cancelOperation:) {
            self.dismiss_quick_actions();
            return true;
        }
        if command == sel!(insertNewline:) {
            let selected = self
                .ivars()
                .quick_actions
                .borrow()
                .as_ref()
                .and_then(|ui| ui.matches.get(ui.selected).copied());
            if let Some(index) = selected {
                self.run_quick_action(index);
            }
            return true;
        }
        let delta = if command == sel!(moveUp:) {
            -1
        } else if command == sel!(moveDown:) {
            1
        } else {
            return false;
        };
        {
            let mut state = self.ivars().quick_actions.borrow_mut();
            if let Some(ui) = state.as_mut() {
                ui.selected = (ui.selected as isize + delta)
                    .clamp(0, ui.matches.len().saturating_sub(1) as isize)
                    as usize;
            }
        }
        self.render_quick_actions();
        true
    }

    pub(super) fn dismiss_quick_actions(&self) {
        let state = self.ivars().quick_actions.borrow_mut().take();
        if let Some(ui) = state {
            if let Some(host) = self.ivars().window.get() {
                host.endSheet(&ui.window);
            }
            ui.window.orderOut(None);
        }
    }

    pub(super) fn run_quick_action(&self, index: usize) {
        self.dismiss_quick_actions();
        if (4..=6).contains(&index) {
            // SAFETY: this existing action creates and presents Settings.
            let _: () = unsafe { msg_send![self, showSettings: self] };
        }
        match index {
            0 => {
                // SAFETY: AppDelegate implements this existing native action.
                let _: () = unsafe { msg_send![self, newChat: self] };
            }
            1 => self.show_workbench_panel(1),
            2 => self.show_workbench_panel(0),
            3 => self.set_workbench_visible(!self.ivars().inspector_visible.get()),
            4 => self.show_settings_section(SETTINGS_TEXT_PAGE),
            5 => self.show_settings_section(SETTINGS_PROJECTS_PAGE),
            6 => self.show_settings_section(SETTINGS_ACCOUNTS_PAGE),
            7 => {
                if let (Some(window), Some(search)) =
                    (self.ivars().window.get(), self.ivars().session_search.get())
                {
                    window.makeFirstResponder(Some(&**search));
                }
            }
            8 => self.set_session_surface(true),
            9 => self.set_session_surface(false),
            _ => {}
        }
    }
}
