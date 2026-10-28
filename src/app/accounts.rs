//! Accounts: what OMP stores, joined with the labels and defaults Omperor
//! keeps; signing in (through the browser, a key, a token); each
//! account's plan and limits; and the credential trouble a session hits.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use aizome_kit::prelude::*;

use super::{App, Choice, ChoiceAction, Msg, Pane, Prompt, PromptAction, Sheet, SignIn};
use crate::account_alert;
use crate::launchers::LauncherKind;
use crate::live_sessions::LiveSession;
use crate::omp_runtime::{self, AccountSummary, AuthMethod, AuthProvider, UiEvent};
use crate::preferences::{self, StoredAccount};
use crate::rules::{AccountEntry, labels_are_valid};

impl App {
    /// The events about accounts and signing in, whichever session's child
    /// reported them.
    pub(crate) fn apply_global_event(&mut self, session: &Rc<LiveSession>, event: UiEvent, cx: &mut Cx<Msg>) {
        let owns = self.sign_in.as_ref().is_some_and(|s| s.owner == session.key && s.busy);
        let current = |app: &App, id: &str| owns && app.sign_in.as_ref().and_then(|s| s.session.as_deref()) == Some(id);
        match event {
            UiEvent::Ready { accounts: Some(accounts), .. } | UiEvent::AccountsChanged(accounts) => {
                self.replace_accounts(accounts, cx)
            }
            UiEvent::AccountChanged(Some(id)) => {
                if let Some(account) = self.account(&id).cloned() {
                    self.remember_account(session, account.provider(), &id, cx);
                }
            }
            UiEvent::AccountUsage(usages) => {
                self.usage_loading = false;
                if !usages.is_empty() {
                    self.usage_fetched_ms = Some(crate::rules::wall_clock_ms());
                }
                for usage in usages {
                    self.account_usage.insert(usage.account_id.clone(), usage);
                }
                self.refresh_usage_cards();
            }
            UiEvent::AuthenticationStarted { generation, .. } => {
                let _ = generation;
            }
            UiEvent::AuthenticationSession { generation, session_id } => {
                let stale = !owns
                    || self.sign_in.as_ref().is_none_or(|s| s.generation != generation || s.cancelling);
                if stale {
                    if let Some(runtime) = session.omp_runtime.get() {
                        runtime.cancel_authentication(session_id);
                    }
                } else if let Some(sign_in) = &mut self.sign_in {
                    sign_in.session = Some(session_id);
                }
            }
            UiEvent::OpenAuthenticationUrl { session_id, url, launch } => {
                if !current(self, &session_id) {
                    return;
                }
                if let Some(sign_in) = &mut self.sign_in {
                    sign_in.url = Some((url, launch));
                    sign_in.waiting = Some(format!("Finish signing in with {} in your browser…", sign_in.provider.product_name()));
                }
                self.open_sign_in_page(cx);
            }
            UiEvent::AuthenticationDeviceCode { session_id, code, verification_url } => {
                if current(self, &session_id)
                    && let Some(sign_in) = &mut self.sign_in
                {
                    sign_in.device = Some((code, verification_url));
                }
            }
            UiEvent::AuthenticationWaiting { session_id, message } => {
                if current(self, &session_id)
                    && let Some(sign_in) = &mut self.sign_in
                {
                    sign_in.waiting = Some(message);
                }
            }
            UiEvent::AuthenticationPrompt(prompt) => {
                if current(self, &prompt.session_id)
                    && let Some(sign_in) = &mut self.sign_in
                {
                    sign_in.submitting = false;
                    sign_in.secret.clear();
                    sign_in.prompt = Some(prompt);
                }
            }
            UiEvent::AuthenticationCompleted { session_id, provider, account_id, principal, account } => {
                // A completed sign-in is a credential now, whichever attempt
                // produced it.
                if current(self, &session_id) {
                    self.close_sign_in(cx);
                }
                session.authenticated_providers.borrow_mut().insert(provider);
                session.runtime_ready.set(false);
                session.runtime_starting.set(true);
                session.chat_busy.set(false);
                session.chat_status_text.replace(match account {
                    Some(account) => format!("{} connected as {account} • starting a fresh session…", provider.product_name()),
                    None => format!("{} connected • starting a fresh session…", provider.product_name()),
                });
                if let Some(account_id) = account_id {
                    self.recover_alert(session, &account_id, provider, true);
                    self.register_account(provider, account_id, principal, cx);
                }
                self.note(format!("{} is connected", provider.product_name()), cx);
            }
            UiEvent::AuthenticationFailed { session_id, generation, error } => {
                let current = match session_id.as_deref() {
                    Some(id) => current(self, id),
                    None => {
                        owns && self.sign_in.as_ref().is_some_and(|s| Some(s.generation) == generation && s.session.is_none())
                    }
                };
                if current {
                    self.sign_in_failed(&error);
                }
            }
            UiEvent::AuthenticationCancelled { session_id }
                if current(self, &session_id) => {
                    self.close_sign_in(cx);
                    self.note("Sign-in cancelled", cx);
                }
            _ => {}
        }
    }

    fn replace_accounts(&mut self, summaries: Vec<AccountSummary>, cx: &mut Cx<Msg>) {
        let mut stored = self.account_metadata.clone();
        let live: HashSet<_> = summaries.iter().map(|s| s.id.clone()).collect();
        stored.retain(|account| live.contains(&account.account_id));
        self.account_choices.retain(|_, account| live.contains(account));
        for summary in &summaries {
            if !stored.iter().any(|entry| entry.account_id == summary.id) {
                stored.push(StoredAccount {
                    account_id: summary.id.clone(),
                    provider: summary.provider.id().into(),
                    principal: summary.principal.clone(),
                    label: None,
                    authenticated_at_ms: preferences::now_ms(),
                    is_default: false,
                });
            }
        }
        self.account_metadata = stored.clone();
        let observed = summaries.clone();
        self.write(cx, move || {
            for summary in &observed {
                preferences::record_account(&summary.id, summary.provider.id(), summary.principal.as_deref(), None)?;
            }
            preferences::retain_accounts(&observed.into_iter().map(|summary| summary.id).collect::<Vec<_>>())
        });
        let entries: Vec<AccountEntry> = summaries.into_iter().map(|s| AccountEntry::joined(s, &stored)).collect();
        let authenticated: HashSet<AuthProvider> = entries
            .iter()
            .filter(|e| e.summary.state == omp_runtime::AccountState::Active)
            .map(AccountEntry::provider)
            .collect();
        for session in self.live.entries() {
            session.account_alerts.borrow_mut().retain_accounts(&entries);
            session.authenticated_providers.replace(authenticated.clone());
            session.accounts_known.set(true);
        }
        self.accounts = entries;
    }

    pub(super) fn apply_account_metadata(&mut self, stored: Vec<StoredAccount>) {
        self.account_metadata = stored.clone();
        self.accounts = self.accounts.iter().map(|e| AccountEntry::joined(e.summary.clone(), &stored)).collect();
    }

    /// Write account metadata on a worker, then read it all back.
    fn write_accounts(&mut self, cx: &mut Cx<Msg>, work: impl FnOnce() -> Result<(), preferences::PreferencesError> + Send + 'static) {
        cx.spawn(move || {
            let result = work().and_then(|()| preferences::load_accounts()).map_err(|e| e.to_string());
            Msg::AccountsStored(Arc::new(result))
        });
    }

    /// A sign-in OMP just completed: shown at once (the child restarts),
    /// and labelled right away when the provider now holds several.
    fn register_account(&mut self, provider: AuthProvider, account_id: String, principal: Option<String>, cx: &mut Cx<Msg>) {
        let saved = (account_id.clone(), principal.clone());
        self.write_accounts(cx, move || {
            preferences::record_account(&saved.0, provider.id(), saved.1.as_deref(), Some(preferences::now_ms()))
        });
        let summary = AccountSummary { id: account_id.clone(), provider, principal, state: omp_runtime::AccountState::Active };
        let entry = AccountEntry::joined(summary, &self.account_metadata);
        match self.accounts.iter_mut().find(|e| e.id() == account_id) {
            Some(existing) => *existing = entry,
            None => self.accounts.push(entry),
        }
        let unlabelled: Vec<String> = self
            .accounts
            .iter()
            .filter(|e| e.provider() == provider && e.label.is_none())
            .map(|e| e.id().to_owned())
            .collect();
        if self.accounts.iter().filter(|e| e.provider() == provider).count() > 1
            && let Some(id) = unlabelled.first()
        {
            self.ask_rename_account(&id.clone(), cx);
        }
    }

    /// Ask OMP for every account's plan and limits, at most every two
    /// minutes unless `force`d.
    pub(crate) fn request_account_usage(&mut self, force: bool, cx: &mut Cx<Msg>) {
        let _ = cx;
        let session = self.session();
        if !session.runtime_ready.get() || self.usage_loading || self.accounts.is_empty() {
            return;
        }
        if !force && self.usage_fetched_ms.is_some_and(|at| crate::rules::wall_clock_ms().saturating_sub(at) < 120_000) {
            return;
        }
        if session.omp_runtime.get().is_some_and(|runtime| runtime.refresh_account_usage()) {
            self.usage_loading = true;
        }
    }

    pub(crate) fn note_account_trouble(
        &mut self,
        session: &LiveSession,
        error: &str,
        primary: bool,
        hint: Option<AuthProvider>,
        cx: &mut Cx<Msg>,
    ) {
        let current = session.current_account.borrow().clone();
        let account = primary.then(|| current.as_deref().and_then(|id| self.account(id))).flatten();
        let hint = hint.or_else(|| primary.then(|| AuthProvider::for_model(&session.current_model.borrow())).flatten());
        if let Some(alert) = account_alert::resolve(error, &self.accounts, account, hint)
            && session.account_alerts.borrow_mut().record(alert.clone())
        {
            let _ = cx;
        }
    }

    fn recover_alert(&mut self, session: &LiveSession, id: &str, provider: AuthProvider, signed_in: bool) {
        session.account_alerts.borrow_mut().recovered(id, provider, signed_in);
        if signed_in {
            // A cached usage rejection belongs to the previous credential.
            self.account_usage.remove(id);
            self.usage_fetched_ms = None;
        }
    }

    // Signing in.

    /// Open the sign-in sheet for `provider` over Settings → Accounts, and
    /// start its usual method.
    pub(super) fn begin_sign_in(&mut self, provider: AuthProvider, cx: &mut Cx<Msg>) {
        if self.preferences.effective_backend() == crate::mock_inference::Backend::Mock {
            self.note("Turn off the mock backend in Settings → Developer to sign in", cx);
            return;
        }
        if let Some(sign_in) = &self.sign_in {
            if sign_in.provider == provider {
                return;
            }
            self.cancel_sign_in(cx);
        }
        self.open_settings(Pane::Accounts, cx);
        let host = self.settings.unwrap_or(self.main);
        let sheet = cx.show_sheet(host);
        self.sheets.insert(sheet, Sheet::SignIn);
        let session = self.session();
        self.sign_in = Some(SignIn {
            provider,
            method: provider.methods()[0],
            owner: session.key,
            generation: 0,
            session: None,
            busy: false,
            cancelling: false,
            url: None,
            device: None,
            waiting: None,
            prompt: None,
            secret: String::new(),
            submitting: false,
            failure: None,
            sheet,
        });
        self.start_login(cx);
    }

    /// Ask OMP to start the sign-in by its method.
    fn start_login(&mut self, cx: &mut Cx<Msg>) {
        let session = self.session();
        self.sign_in_generation += 1;
        let generation = self.sign_in_generation;
        let Some(sign_in) = &mut self.sign_in else { return };
        sign_in.owner = session.key;
        sign_in.generation = generation;
        sign_in.busy = true;
        sign_in.cancelling = false;
        sign_in.session = None;
        sign_in.url = None;
        sign_in.device = None;
        sign_in.prompt = None;
        sign_in.failure = None;
        sign_in.waiting = Some(format!("Starting {} sign-in…", sign_in.provider.product_name()));
        let (provider, method) = (sign_in.provider, sign_in.method);
        if !session.omp_runtime.get().is_some_and(|runtime| runtime.login(provider, method, generation)) {
            self.sign_in_failed(&format!(
                "The agent isn't running yet; try again in a moment ({} sign-in)",
                provider.product_name()
            ));
        }
        let _ = cx;
    }

    pub(super) fn sign_in_method(&mut self, method: AuthMethod, cx: &mut Cx<Msg>) {
        let Some(sign_in) = &self.sign_in else { return };
        if sign_in.method == method {
            return;
        }
        self.abandon_login();
        if let Some(sign_in) = &mut self.sign_in {
            sign_in.method = method;
            sign_in.secret.clear();
        }
        self.start_login(cx);
    }

    /// End OMP's login session, if one is running, keeping the sheet.
    fn abandon_login(&mut self) {
        let Some(sign_in) = &mut self.sign_in else { return };
        if !sign_in.busy {
            return;
        }
        match sign_in.session.clone() {
            Some(id) => {
                if let Some(owner) = self.live.get(sign_in.owner)
                    && let Some(runtime) = owner.omp_runtime.get()
                {
                    runtime.cancel_authentication(id);
                }
            }
            // Not acknowledged yet: it's cancelled when it is.
            None => sign_in.cancelling = true,
        }
        if let Some(sign_in) = &mut self.sign_in {
            sign_in.busy = false;
        }
    }

    pub(super) fn sign_in_submit(&mut self, cx: &mut Cx<Msg>) {
        let Some(sign_in) = &mut self.sign_in else { return };
        let secret = sign_in.secret.trim().to_owned();
        if secret.is_empty() {
            return;
        }
        let Some(prompt) = sign_in.prompt.clone() else {
            self.note("OMP hasn't asked for it yet; wait a moment", cx);
            return;
        };
        sign_in.submitting = true;
        let owner = sign_in.owner;
        let sent = self.live.get(owner).is_some_and(|s| s.omp_runtime.get().is_some_and(|r| r.answer_authentication(prompt, secret)));
        if !sent {
            self.sign_in_failed("Could not send the answer to OMP");
        }
    }

    pub(super) fn open_sign_in_page(&mut self, cx: &mut Cx<Msg>) {
        let Some((url, launch)) = self.sign_in.as_ref().and_then(|s| s.url.clone()) else { return };
        // OMP's loopback launcher redirects to the page and is what the
        // callback expects from a browser on this machine.
        let target = launch.unwrap_or(url);
        match self.launchers.choose(LauncherKind::Browser, self.preferences.browser.as_deref()) {
            Some(browser) if self.preferences.browser.is_some() => {
                cx.spawn(move || Msg::Launched(browser.launch_url(&target)));
            }
            _ => cx.open_url(&target),
        }
    }

    pub(super) fn cancel_sign_in(&mut self, cx: &mut Cx<Msg>) {
        self.abandon_login();
        self.close_sign_in(cx);
    }

    fn close_sign_in(&mut self, cx: &mut Cx<Msg>) {
        if let Some(sign_in) = self.sign_in.take() {
            self.sheets.remove(&sign_in.sheet);
            cx.close_window(sign_in.sheet);
        }
    }

    /// The sign-in ended badly: the sheet keeps the reason, with a way to
    /// try again.
    pub(crate) fn sign_in_failed(&mut self, error: &str) {
        if let Some(sign_in) = &mut self.sign_in {
            sign_in.busy = false;
            sign_in.submitting = false;
            sign_in.waiting = None;
            sign_in.failure = Some(error.to_owned());
        }
    }

    // Managing accounts.

    pub(super) fn ask_rename_account(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(entry) = self.account(id).cloned() else { return };
        let prompt = Prompt {
            title: format!("Name this {}", entry.provider().account_noun()),
            message: format!(
                "{} — a name like Work or Personal tells it apart wherever accounts are chosen.",
                entry.identity()
            ),
            value: entry.label.clone().unwrap_or_default(),
            confirm: "Save".into(),
            action: PromptAction::LabelAccount(id.to_owned()),
            error: None,
        };
        let sheet = cx.show_sheet(self.main_or_settings());
        self.sheets.insert(sheet, Sheet::Prompt(prompt));
    }

    /// Whether `label` would do for account `id`.
    pub(crate) fn label_error(&self, id: &str, label: &str) -> Option<String> {
        let provider = self.account(id)?.provider();
        let existing: Vec<String> = self
            .accounts
            .iter()
            .filter(|e| e.provider() == provider && e.id() != id)
            .filter_map(|e| e.label.clone())
            .collect();
        (!labels_are_valid(&[label.to_owned()], &existing))
            .then(|| "Give it a name no other account of this provider has".to_owned())
    }

    pub(crate) fn label_account(&mut self, id: String, label: String, cx: &mut Cx<Msg>) {
        let label = label.trim().to_owned();
        self.write_accounts(cx, move || preferences::set_account_label(&id, &label));
    }

    pub(super) fn make_default(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(entry) = self.account(id).cloned() else { return };
        let id = id.to_owned();
        let provider = entry.provider();
        self.write_accounts(cx, move || preferences::set_default_account(provider.id(), Some(&id)));
    }

    pub(super) fn ask_remove_account(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let Some(entry) = self.account(id).cloned() else { return };
        cx.alert(
            self.main_or_settings(),
            Alert::new(
                format!("Remove “{}”?", entry.display_name()),
                "OMP forgets this account's credential. Sessions that used it ask for another account.",
            )
            .destructive_button("Remove", Msg::ConfirmRemoveAccount(id.to_owned()))
            .cancel_button("Cancel"),
        );
    }

    pub(super) fn remove_account(&mut self, id: &str, cx: &mut Cx<Msg>) {
        let session = self.session();
        if !session.omp_runtime.get().is_some_and(|runtime| runtime.logout_account(id.to_owned())) {
            self.note("The agent isn't running; try again once it is", cx);
        }
    }

    /// Several accounts serve a provider and nothing decides: ask which.
    pub(crate) fn ask_account(&mut self, provider: AuthProvider, model: Option<String>, cx: &mut Cx<Msg>) {
        let accounts: Vec<&AccountEntry> = self.accounts.iter().filter(|e| e.provider() == provider && e.is_usable()).collect();
        let choice = Choice {
            title: format!("Which {}?", provider.account_noun()),
            message: format!("Several {} accounts are signed in. This project uses the one you choose.", provider.product_name()),
            options: accounts.iter().map(|e| e.display_name()).collect(),
            selected: 0,
            confirm: "Use This Account".into(),
            action: ChoiceAction::Account { provider, model, accounts: accounts.iter().map(|e| e.id().to_owned()).collect() },
        };
        let sheet = cx.show_sheet(self.main);
        self.sheets.insert(sheet, Sheet::Choice(choice));
    }

    pub(super) fn set_launcher(&mut self, kind: LauncherKind, index: usize, cx: &mut Cx<Msg>) {
        let installed = self.launchers.of(kind);
        // The first option is the automatic choice.
        let name = index.checked_sub(1).and_then(|i| installed.get(i)).map(|l| l.name.to_owned());
        match kind {
            LauncherKind::Editor => self.preferences.editor = name,
            LauncherKind::Terminal => self.preferences.terminal = name,
            LauncherKind::Browser => self.preferences.browser = name,
        }
        self.store_preferences(cx);
    }

    pub(crate) fn launch(&mut self, kind: LauncherKind, target: std::path::PathBuf, cx: &mut Cx<Msg>) {
        let preference = match kind {
            LauncherKind::Editor => self.preferences.editor.clone(),
            LauncherKind::Terminal => self.preferences.terminal.clone(),
            LauncherKind::Browser => self.preferences.browser.clone(),
        };
        let chosen = self.launchers.choose(kind, preference.as_deref());
        cx.spawn(move || {
            let result = match chosen {
                Some(launcher) => launcher.launch(&target),
                None => Err(format!("No {} is installed that Omperor knows how to open", kind.noun())),
            };
            Msg::Launched(result)
        });
    }
}
