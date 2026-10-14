//! Nonmodal credential recovery on the existing runtime and Accounts page.
use super::*;

#[derive(Debug)]
pub(super) struct Banner {
    pub(super) view: Retained<NSBox>,
    icon: Retained<NSImageView>,
    message: Retained<NSTextField>,
    manage: Retained<NSButton>,
    later: Retained<NSButton>,
    dismiss: Retained<NSButton>,
    shown: RefCell<Option<account_alert::Alert>>,
}

impl Banner {
    pub(super) fn new(delegate: &AppDelegate) -> Self {
        let mtm = delegate.mtm();
        let view = card_box(NSRect::ZERO, 8.0, mtm);
        view.setContentViewMargins(NSSize::new(0.0, 0.0));
        view.setFillColor(&theme::surface_inset());
        view.setBorderColor(&theme::plum());
        view.setBorderWidth(1.0);
        view.setHidden(true);
        let icon = NSImageView::initWithFrame(NSImageView::alloc(mtm), NSRect::ZERO);
        icon.setImage(
            theme::symbol(
                "exclamationmark.triangle.fill",
                14.0,
                unsafe { NSFontWeightMedium },
                None,
            )
            .as_deref(),
        );
        icon.setContentTintColor(Some(&theme::plum()));
        let message = text_label("", theme::CAPTION_SIZE, false, mtm);
        message.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        let manage = settings_button(
            "Accounts",
            NSRect::ZERO,
            delegate,
            sel!(accountAlertManage:),
            mtm,
        );
        let later = settings_button(
            "Later",
            NSRect::ZERO,
            delegate,
            sel!(accountAlertSnooze:),
            mtm,
        );
        later.setToolTip(Some(ns_string!("Remind me in 30 minutes")));
        let dismiss = settings_button(
            "Dismiss",
            NSRect::ZERO,
            delegate,
            sel!(accountAlertDismiss:),
            mtm,
        );
        dismiss.setToolTip(Some(ns_string!(
            "Dismiss this warning until Omperor is restarted"
        )));
        view.addSubview(&icon);
        view.addSubview(&message);
        for button in [&manage, &later, &dismiss] {
            view.addSubview(button);
        }
        Self {
            view,
            icon,
            message,
            manage,
            later,
            dismiss,
            shown: RefCell::new(None),
        }
    }

    pub(super) fn layout(&self, pane: NSSize) -> f64 {
        if self.shown.borrow().is_none() {
            return 0.0;
        }
        let width = (pane.width - 48.0).max(280.0);
        let stacked = width < 860.0;
        let height = if stacked { 86.0 } else { 44.0 };
        self.view
            .setFrame(rect(24.0, pane.height - 162.0 - layout::USAGE_STRIP_HEIGHT - height, width, height));
        self.icon.setFrame(rect(12.0, height - 30.0, 16.0, 18.0));
        self.message
            .setMaximumNumberOfLines(if stacked { 2 } else { 1 });
        self.message.setFrame(if stacked {
            rect(36.0, 38.0, width - 48.0, 38.0)
        } else {
            rect(36.0, 11.0, width - 300.0, 22.0)
        });
        self.dismiss.setFrame(rect(width - 246.0, 8.0, 76.0, 28.0));
        self.later.setFrame(rect(width - 164.0, 8.0, 60.0, 28.0));
        self.manage.setFrame(rect(width - 98.0, 8.0, 88.0, 28.0));
        height + 8.0
    }
}

impl AppDelegate {
    pub(super) fn sync_account_banner(&self) {
        let owner = self.session();
        let Some(banner) = self.ivars().account_banner.get() else {
            return;
        };
        let alerts = owner.account_alerts.borrow();
        let visible = alerts.visible(std::time::Instant::now());
        if banner.shown.borrow().as_ref() == visible {
            return;
        }
        let alert = visible.cloned();
        drop(alerts);
        if let Some(alert) = &alert {
            let message = NSString::from_str(&alert.message());
            banner.message.setStringValue(&message);
            banner.message.setToolTip(Some(&message));
            banner.message.setAccessibilityLabel(Some(&message));
        }
        banner.view.setHidden(alert.is_none());
        banner.shown.replace(alert);
        // Reflow both native surfaces; the warning must never cover transcript
        // text or the Observatory's toolbar. This also preserves reader state.
        self.refit_composer_chips();
    }

    pub(super) fn account_banner_space(&self, pane: NSSize) -> f64 {
        self.ivars()
            .account_banner
            .get()
            .map_or(0.0, |banner| banner.layout(pane))
    }

    pub(super) fn note_session_account_trouble(&self, owner: &live_sessions::LiveSession, error: &str, primary: bool, hint: Option<AuthProvider>) {
        let accounts = self.ivars().accounts.borrow();
        let current = owner.current_account.borrow();
        let account = primary
            .then(|| {
                current
                    .as_deref()
                    .and_then(|id| accounts.iter().find(|a| a.id() == id))
            })
            .flatten();
        let hint = hint.or_else(|| {
            primary
                .then(|| AuthProvider::for_model(&owner.current_model.borrow()))
                .flatten()
        });
        let alert = account_alert::resolve(error, &accounts, account, hint);
        drop(current);
        drop(accounts);
        if let Some(alert) = alert {
            let changed = owner.account_alerts.borrow_mut().record(alert);
            if changed && self.session().key == owner.key {
                self.sync_account_banner();
                self.mark_accounts_ui_dirty();
            }
        }
    }

    pub(super) fn recover_account_alert(&self, id: &str, provider: AuthProvider, signed_in: bool) {
        let owner = self.session();
        let changed = owner.account_alerts
            .borrow_mut()
            .recovered(id, provider, signed_in);
        if !changed && !signed_in {
            return;
        }
        if signed_in {
            // A cached usage rejection belongs to the previous credential.
            self.ivars().account_usage.borrow_mut().remove(id);
            self.ivars().account_usage_fetched_at.set(None);
            self.ivars().account_usage_answered_at.set(None);
        }
        self.sync_account_banner();
        self.mark_accounts_ui_dirty();
    }

    pub(super) fn manage_account_alert(&self) {
        let owner = self.session();
        let alert = self
            .ivars()
            .account_banner
            .get()
            .and_then(|b| b.shown.borrow().clone());
        if let Some(alert) = alert {
            owner.account_alerts.borrow_mut().suppress(
                &alert,
                false,
                std::time::Instant::now(),
            );
            self.sync_account_banner();
            self.reveal_account(alert.account_id.as_deref(), alert.provider);
        }
    }

    pub(super) fn suppress_account_alert(&self, later: bool) {
        let owner = self.session();
        let alert = self
            .ivars()
            .account_banner
            .get()
            .and_then(|b| b.shown.borrow().clone());
        if let Some(alert) = alert {
            owner.account_alerts.borrow_mut().suppress(
                &alert,
                later,
                std::time::Instant::now(),
            );
            self.sync_account_banner();
        }
    }

    fn reveal_account(&self, account_id: Option<&str>, provider: Option<AuthProvider>) {
        if self.ivars().settings_ui.get().is_none() {
            let settings = self.build_settings_window(self.mtm());
            let _ = self.ivars().settings_ui.set(settings);
            self.sync_settings_ui();
        }
        if let Some(index) = AuthProvider::ALL.iter().position(|p| Some(*p) == provider) {
            self.ivars().accounts_tab.set(index);
        }
        self.ivars()
            .account_highlight
            .replace(account_id.map(str::to_owned));
        self.ivars()
            .account_highlight_scroll
            .set(account_id.is_some());
        self.show_settings_section(SETTINGS_ACCOUNTS_PAGE);
        if let Some(settings) = self.ivars().settings_ui.get() {
            settings.window.makeKeyAndOrderFront(None);
        }
        self.sync_settings_account_ui();
    }
}
