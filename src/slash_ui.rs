//! The composer's `/` commands: the popover that lists them as the user
//! types, and `/usage`, answered here with a card in the transcript. The
//! popover is an overlay in the window like the `@` picker, built with the
//! composer so the first `/` shows it at once; filtering a handful of
//! commands is a few comparisons, so it runs where the key lands.
use super::*;

use slash_commands::Command;

const ROW_HEIGHT: f64 = 34.0;
const PADDING: f64 = 6.0;

#[derive(Debug, Default)]
pub(super) struct SlashPicker {
    popover: OnceCell<Retained<NSBox>>,
    selection: OnceCell<Retained<NSBox>>,
    /// The press's seed: a bar of full gold at the selection's left edge
    /// that sweeps across it when the command is chosen.
    seed: OnceCell<Retained<NSBox>>,
    /// Bumped whenever the list shows or hides, so a fade that finishes
    /// after the list came back leaves it be.
    shown: Cell<u64>,
    /// One row per command, made once; hidden rows are filtered out.
    rows: RefCell<Vec<(Command, Retained<NSButton>)>>,
    matches: RefCell<Vec<Command>>,
    selected: Cell<usize>,
    /// Cards waiting for the usage read: session, row index, snapshot.
    pending: RefCell<Vec<(live_sessions::SessionKey, usize, usage_card::Body)>>,
    /// Cards waiting for their account's limit windows: session, row.
    awaiting_limits: RefCell<Vec<(live_sessions::SessionKey, usize)>>,
}

fn row_title(command: &Command) -> Retained<NSMutableAttributedString> {
    // SAFETY: AppKit exports the weight constants for the process lifetime.
    let name_font = NSFont::monospacedSystemFontOfSize_weight(12.5, unsafe { NSFontWeightSemibold });
    let title = attributed(&format!("/{}", command.name), &name_font, &theme::accent(), 0.0);
    if !command.arguments.is_empty() {
        title.appendAttributedString(&attributed(
            &format!(" {}", command.arguments),
            // SAFETY: as above.
            &NSFont::monospacedSystemFontOfSize_weight(11.0, unsafe { NSFontWeightRegular }),
            &theme::plum(),
            0.0,
        ));
    }
    title.appendAttributedString(&attributed(
        &format!("   {}", command.summary),
        &NSFont::systemFontOfSize(12.0),
        &theme::secondary_text(),
        0.0,
    ));
    title
}

/// What became of a submitted line Omperor may handle itself.
pub(super) enum LocalCommand {
    /// Not one of Omperor's commands: send it as a prompt.
    None,
    /// Answered in the transcript; nothing is sent.
    Answered,
    /// Handled, and this text is to be sent as the prompt.
    Send(String),
}

impl AppDelegate {
    /// Makes the popover and its rows ahead of the first `/`.
    pub(super) fn prepare_slash_picker(&self) {
        let mtm = self.mtm();
        let picker = &self.ivars().slash_picker;
        let Some(input) = self.ivars().input.get() else { return };
        // SAFETY: the composer field lives in the chat pane for the window's lifetime.
        let Some(superview) = (unsafe { input.superview() }) else { return };
        let popover = picker.popover.get_or_init(|| {
            let container = card_box(rect(0.0, 0.0, 10.0, 10.0), 12.0, mtm);
            container.setFillColor(&theme::popover());
            container.setContentViewMargins(NSSize::new(0.0, 0.0));
            container.setAutoresizingMask(NSAutoresizingMaskOptions::empty());
            // It floats over the transcript, lifted like the app's cards.
            float_card(&container);
            container.setHidden(true);
            // SAFETY: NSAccessibilityListRole is an AppKit static.
            container.setAccessibilityRole(Some(unsafe { objc2_app_kit::NSAccessibilityListRole }));
            container.setAccessibilityLabel(Some(ns_string!("Commands")));
            superview.addSubview(&container);
            container
        });
        let content = popover.contentView().expect("box content view");
        picker.selection.get_or_init(|| {
            let bar = fill_box(rect(0.0, 0.0, 10.0, ROW_HEIGHT), &theme::accent_fill(), 8.0, mtm);
            bar.setBorderWidth(1.0);
            bar.setBorderColor(&theme::accent_rule());
            bar.setWantsLayer(true);
            content.addSubview(&bar);
            bar
        });
        picker.seed.get_or_init(|| {
            let seed = fill_box(seed_frame(rect(0.0, 0.0, 10.0, ROW_HEIGHT)), &theme::accent(), 1.5, mtm);
            seed.setWantsLayer(true);
            content.addSubview(&seed);
            seed
        });
        if picker.rows.borrow().is_empty() {
            let rows = slash_commands::COMMANDS
                .iter()
                .enumerate()
                .map(|(index, command)| {
                    // SAFETY: the delegate implements chooseSlashCommand: and outlives the row.
                    let button = unsafe {
                        NSButton::buttonWithTitle_target_action(
                            &NSString::from_str(command.name),
                            Some::<&AnyObject>(self),
                            Some(sel!(chooseSlashCommand:)),
                            mtm,
                        )
                    };
                    button.setBordered(false);
                    button.setAlignment(NSTextAlignment::Left);
                    button.setAttributedTitle(&row_title(command));
                    button.setTag(index as isize);
                    button.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
                    button.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                        "/{} {}. {}",
                        command.name, command.arguments, command.summary
                    ))));
                    button.setHidden(true);
                    content.addSubview(&button);
                    (*command, button)
                })
                .collect();
            picker.rows.replace(rows);
        }
    }

    pub(super) fn slash_picker_visible(&self) -> bool {
        self.ivars()
            .slash_picker
            .popover
            .get()
            .is_some_and(|popover| !popover.isHidden())
    }

    /// Shows, filters or hides the list from the composer's text and caret.
    pub(super) fn update_slash_picker(&self) {
        let Some(input) = self.ivars().input.get() else { return };
        let caret = input.currentEditor().map(|editor| editor.selectedRange().location);
        let text = input.stringValue().to_string();
        let query = caret.and_then(|caret| slash_commands::active_query(&text, caret));
        let Some(query) = query else {
            self.hide_slash_picker();
            return;
        };
        let mock = self.mock_backend_active() || self.ivars().preferences.borrow().developer_mode;
        let matches = slash_commands::filter(&slash_commands::available(mock), query);
        if matches.is_empty() {
            self.hide_slash_picker();
            return;
        }
        self.prepare_slash_picker();
        let picker = &self.ivars().slash_picker;
        let (Some(popover), Some(selection), Some(composer)) =
            (picker.popover.get(), picker.selection.get(), self.ivars().composer_surface.get())
        else {
            return;
        };
        let was_hidden = popover.isHidden();
        let height = matches.len() as f64 * ROW_HEIGHT + PADDING * 2.0;
        let frame = composer.frame();
        popover.setFrame(rect(frame.origin.x, frame.origin.y + frame.size.height + 8.0, frame.size.width, height));
        let width = frame.size.width - PADDING * 2.0;
        for (command, button) in picker.rows.borrow().iter() {
            match matches.iter().position(|m| m == command) {
                Some(i) => {
                    // The content view is not flipped: the first row is highest.
                    let y = height - PADDING - ROW_HEIGHT * (i as f64 + 1.0);
                    button.setFrame(rect(PADDING + 10.0, y, width - 20.0, ROW_HEIGHT));
                    button.setHidden(false);
                }
                None => button.setHidden(true),
            }
        }
        let keep = picker
            .matches
            .borrow()
            .get(picker.selected.get())
            .and_then(|current| matches.iter().position(|m| m == current))
            .unwrap_or(0);
        picker.matches.replace(matches);
        picker.selected.set(keep);
        selection.setHidden(false);
        popover.setHidden(false);
        popover.setAlphaValue(1.0);
        if was_hidden {
            picker.shown.set(picker.shown.get().wrapping_add(1));
            motion::settle_in(popover);
        }
        self.apply_slash_selection(!was_hidden);
    }

    fn apply_slash_selection(&self, animate: bool) {
        let picker = &self.ivars().slash_picker;
        let (Some(popover), Some(selection)) = (picker.popover.get(), picker.selection.get()) else { return };
        let selected = picker.selected.get();
        let Some(command) = picker.matches.borrow().get(selected).copied() else { return };
        let height = popover.frame().size.height;
        let y = height - PADDING - ROW_HEIGHT * (selected as f64 + 1.0);
        let frame = rect(PADDING, y, popover.frame().size.width - PADDING * 2.0, ROW_HEIGHT);
        let seed = picker.seed.get().cloned();
        if let Some(seed) = &seed {
            seed.setAlphaValue(1.0);
        }
        if animate && !motion::reduce_motion() {
            let selection = selection.clone();
            motion::animate(motion::PRESS_IN * 2.0, move || {
                selection.animator().setFrame(frame);
                if let Some(seed) = &seed {
                    seed.animator().setFrame(seed_frame(frame));
                }
            });
        } else {
            selection.setFrame(frame);
            if let Some(seed) = &seed {
                seed.setFrame(seed_frame(frame));
            }
        }
        // VoiceOver hears the row the selection moved to.
        if let Some((_, button)) = picker.rows.borrow().iter().find(|(c, _)| *c == command) {
            let announcement = NSString::from_str(&format!("/{}, {}", command.name, command.summary));
            // SAFETY: posting an accessibility announcement on the main thread.
            unsafe {
                let info = NSDictionary::from_slices(
                    &[objc2_app_kit::NSAccessibilityAnnouncementKey],
                    &[&*announcement as &AnyObject],
                );
                objc2_app_kit::NSAccessibilityPostNotificationWithUserInfo(
                    button,
                    objc2_app_kit::NSAccessibilityAnnouncementRequestedNotification,
                    Some(&*(Retained::as_ptr(&info) as *const NSDictionary<objc2_app_kit::NSAccessibilityNotificationUserInfoKey, AnyObject>)),
                );
            }
        }
    }

    pub(super) fn move_slash_selection(&self, delta: isize) {
        let picker = &self.ivars().slash_picker;
        let count = picker.matches.borrow().len();
        if count == 0 {
            return;
        }
        let next = (picker.selected.get() as isize + delta).rem_euclid(count as isize) as usize;
        picker.selected.set(next);
        self.apply_slash_selection(true);
    }

    pub(super) fn hide_slash_picker(&self) {
        let picker = &self.ivars().slash_picker;
        if let Some(popover) = picker.popover.get()
            && !popover.isHidden()
        {
            picker.shown.set(picker.shown.get().wrapping_add(1));
            popover.setHidden(true);
            popover.setAlphaValue(1.0);
        }
    }

    /// A chosen command's fulfilment: the seed sweeps across the selection
    /// and the list fades as the composer takes the command.
    fn fulfil_slash_picker(&self) {
        let picker = &self.ivars().slash_picker;
        let (Some(popover), Some(selection), Some(seed)) =
            (picker.popover.get().cloned(), picker.selection.get().cloned(), picker.seed.get().cloned())
        else {
            return self.hide_slash_picker();
        };
        if motion::reduce_motion() || popover.isHidden() {
            return self.hide_slash_picker();
        }
        let shown = picker.shown.get();
        let this = self.retain();
        let (fading, frame) = (popover.clone(), selection.frame());
        motion::animate_then(
            motion::BLOOM,
            move || {
                seed.animator().setFrame(frame);
                seed.animator().setAlphaValue(0.0);
                fading.animator().setAlphaValue(0.0);
            },
            move || {
                if this.ivars().slash_picker.shown.get() == shown {
                    this.hide_slash_picker();
                }
            },
        );
    }

    /// Completes the selected command; with `run`, one that takes no
    /// arguments also runs.
    pub(super) fn commit_slash_picker(&self, run: bool) {
        let picker = &self.ivars().slash_picker;
        let Some(command) = picker.matches.borrow().get(picker.selected.get()).copied() else {
            self.hide_slash_picker();
            return;
        };
        self.fulfil_slash_picker();
        let Some(input) = self.ivars().input.get() else { return };
        let completion = NSString::from_str(&command.completion());
        input.setStringValue(&completion);
        if let Some(editor) = input.currentEditor() {
            editor.setSelectedRange(NSRange::new(completion.length(), 0));
        }
        self.session().draft.replace(command.completion());
        if run && !command.takes_arguments() {
            // SAFETY: the delegate's sendMessage: action exists.
            unsafe {
                let _: () = msg_send![self, sendMessage: &**input];
            }
        }
    }

    pub(super) fn choose_slash_row(&self, tag: isize) {
        let picker = &self.ivars().slash_picker;
        let Some(command) = slash_commands::COMMANDS.get(tag.max(0) as usize) else { return };
        if let Some(index) = picker.matches.borrow().iter().position(|m| m == command) {
            picker.selected.set(index);
        }
        self.commit_slash_picker(true);
    }

    /// Answers a line Omperor handles itself.
    pub(super) fn run_local_command(&self, value: &str) -> LocalCommand {
        let Some(local) = slash_commands::parse_local(value) else { return LocalCommand::None };
        let owner = self.session();
        self.hide_slash_picker();
        let request = match &local {
            slash_commands::Local::Omperor(request) => Some(request.clone()),
            _ => None,
        };
        // A request after `/omperor` is the prompt, and is recorded when it
        // is sent.
        if request.as_deref().is_none_or(str::is_empty) {
            owner.prompt_history.borrow_mut().record(value);
        }
        if let Some(input) = self.ivars().input.get() {
            input.setStringValue(ns_string!(""));
        }
        owner.draft.replace(String::new());
        let (all, note) = match local {
            slash_commands::Local::Usage { all } => (all, None),
            slash_commands::Local::UsageArguments(arguments) => {
                (false, Some(format!("/usage takes only “all”; “{arguments}” was ignored.")))
            }
            slash_commands::Local::Omperor(request) => {
                self.set_omperor_tools(true);
                return if request.is_empty() { LocalCommand::Answered } else { LocalCommand::Send(request) };
            }
            slash_commands::Local::OmperorOff => {
                self.set_omperor_tools(false);
                return LocalCommand::Answered;
            }
        };
        // Last known account usage shows at once; a fresh reading is asked
        // for (off the main thread) and fills the card in when it lands.
        self.request_account_usage(true);
        let mut body = self.usage_snapshot(all);
        body.note = note;
        // A current reading shows at once; otherwise the card says it is
        // reading and fills in when the job pool answers.
        let state = &self.ivars().usage_state;
        // The mock's sessions keep no journal: their receipts are the ones
        // that arrived on the stream, and the card says so.
        let live = self.mock_backend_active();
        let fresh = live || (state.owner_is(owner.key) && !state.is_loading());
        if live {
            body.card = Some(usage::live_card(&owner.live_receipts.borrow()));
            let source = "Temporary mock session: figures from this run's live receipts.".to_owned();
            body.note = Some(match body.note.take() {
                Some(note) => format!("{note} {source}"),
                None => source,
            });
        } else if fresh {
            body.card = state.card();
        }
        let index = owner.conversation.borrow_mut().push_usage(body.encode());
        // The accounts' usage comes from OMP, asked for above; the card
        // takes the answer in place when it arrives.
        if body.refreshing {
            self.ivars().slash_picker.awaiting_limits.borrow_mut().push((owner.key, index));
        }
        if !fresh || body.card.is_none() {
            self.ivars().slash_picker.pending.borrow_mut().push((owner.key, index, body));
            self.request_usage_read(true);
        }
        self.refresh_chat_transcript();
        self.schedule_session_view_save();
        LocalCommand::Answered
    }

    /// What the card shows beside the receipts: the Agents panel's names,
    /// the speedometer's readings, and the accounts' plans and limit
    /// windows as OMP last reported them.
    fn usage_snapshot(&self, all: bool) -> usage_card::Body {
        let owner = self.session();
        let labels = owner
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
        let rates = self.ivars().usage_header.get().map(|header| header.rates()).unwrap_or_default();
        let account = owner.current_account.borrow().clone();
        let mut body = usage_card::Body { labels, rates, account, all, ..usage_card::Body::default() };
        self.fill_account_blocks(&mut body);
        body
    }

    /// Puts the accounts' last known usage into a card: the session's
    /// account, or with `all` every signed-in account grouped by provider;
    /// whether a reading is on its way, and how old the last one is.
    fn fill_account_blocks(&self, body: &mut usage_card::Body) {
        let now_ms = wall_clock_ms();
        body.at_ms = now_ms;
        let accounts = self.ivars().accounts.borrow();
        let usage = self.ivars().account_usage.borrow();
        let block = |entry: &AccountEntry| {
            usage_card::account_block(entry.id(), &entry.display_name(), entry.provider(), usage.get(entry.id()))
        };
        body.accounts = if body.all {
            AuthProvider::ALL
                .iter()
                .flat_map(|provider| accounts.iter().filter(move |entry| entry.provider() == *provider && entry.is_usable()))
                .map(block)
                .collect()
        } else {
            body.account
                .as_deref()
                .and_then(|id| accounts.iter().find(|entry| entry.id() == id))
                .map(block)
                .into_iter()
                .collect()
        };
        body.limits.clear();
        body.refreshing = self.ivars().account_usage_loading.get();
        body.fetched_ms = self.ivars().account_usage_answered_at.get().filter(|_| !usage.is_empty()).map(wall_clock_ms_at);
        // A history-only session has no runtime to ask with.
        let cached = body.accounts.iter().any(|block| block.reported);
        body.accounts_note = (!self.session().runtime_ready.get()).then(|| {
            if cached {
                "Start the session to read current usage; these are the last values read.".to_owned()
            } else {
                "Start the session to read current usage.".to_owned()
            }
        });
    }

    /// OMP answered account usage: cards waiting for it take the answer in
    /// place; the row's height changes go through the normal restack.
    pub(super) fn fill_usage_limits(&self) {
        let waiting = std::mem::take(&mut *self.ivars().slash_picker.awaiting_limits.borrow_mut());
        let mut refresh = false;
        for (key, index) in waiting {
            let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { continue };
            let Some(message) = owner.conversation.borrow().message(index) else { continue };
            let mut body = usage_card::Body::decode(&message.body);
            self.fill_account_blocks(&mut body);
            // Still reading the receipts: the read's answer keeps these.
            for pending in self.ivars().slash_picker.pending.borrow_mut().iter_mut() {
                if pending.0 == key && pending.1 == index {
                    pending.2.accounts = body.accounts.clone();
                    pending.2.refreshing = body.refreshing;
                    pending.2.fetched_ms = body.fetched_ms;
                    pending.2.accounts_note = body.accounts_note.clone();
                    pending.2.at_ms = body.at_ms;
                    body.card = pending.2.card.clone();
                }
            }
            owner.conversation.borrow_mut().set_usage(index, body.encode());
            refresh |= self.session().key == key;
        }
        if refresh {
            self.refresh_chat_transcript();
        }
    }

    /// The card's Retry: asks OMP again, and the current session's usage
    /// cards show that a reading is on its way until it lands.
    pub(super) fn retry_account_usage(&self) {
        self.request_account_usage(true);
        if !self.ivars().account_usage_loading.get() {
            return;
        }
        let owner = self.session();
        let count = owner.conversation.borrow().len();
        let mut awaiting = self.ivars().slash_picker.awaiting_limits.borrow_mut();
        for index in 0..count {
            let Some(message) = owner.conversation.borrow().message(index) else { continue };
            if message.kind != chat::TranscriptMessageKind::Usage {
                continue;
            }
            let mut body = usage_card::Body::decode(&message.body);
            if body.accounts.is_empty() || body.refreshing {
                continue;
            }
            body.refreshing = true;
            owner.conversation.borrow_mut().set_usage(index, body.encode());
            if !awaiting.contains(&(owner.key, index)) {
                awaiting.push((owner.key, index));
            }
        }
        drop(awaiting);
        self.refresh_chat_transcript();
    }

    /// The usage read landed: cards waiting on it for this session fill in.
    pub(super) fn fill_usage_cards(&self, key: live_sessions::SessionKey, card: &usage::Card) {
        let waiting: Vec<_> = {
            let mut pending = self.ivars().slash_picker.pending.borrow_mut();
            let (mine, rest) = pending.drain(..).partition(|(k, ..)| *k == key);
            *pending = rest;
            mine
        };
        if waiting.is_empty() {
            return;
        }
        let Some(owner) = self.ivars().live_sessions.borrow().get(key) else { return };
        for (_, index, mut body) in waiting {
            body.card = Some(card.clone());
            owner.conversation.borrow_mut().set_usage(index, body.encode());
        }
        if self.session().key == key {
            self.refresh_chat_transcript();
        }
    }

    /// Developer-only, for `--screenshot` with
    /// `OMPEROR_SHOT_SYNTHETIC_ACCOUNTS=1`: made-up accounts and usage, so
    /// the usage card's account sections can be reviewed from a scratch
    /// home. Every name and figure here is invented.
    pub(super) fn inject_synthetic_account_usage(&self, in_flight: bool) {
        if !markdown_bench::synthetic_accounts() {
            return;
        }
        let now = wall_clock_ms();
        let minutes = |m: u64| now + m * 60_000;
        let entry = |id: &str, provider: AuthProvider, label: &str| AccountEntry {
            summary: AccountSummary { id: id.into(), provider, principal: None, state: omp_runtime::AccountState::Active },
            label: Some(label.into()),
            authenticated_at_ms: 0,
            is_default: false,
        };
        let window = |label: &str, scope: &str, used: Option<u8>, resets: Option<u64>, spend: Option<omp_runtime::Spend>| {
            omp_runtime::UsageWindow { label: label.into(), scope: Some(scope.into()), used_percent: used, resets_at_ms: resets, spend }
        };
        let usages = vec![
            omp_runtime::AccountUsage {
                account_id: "shot:claude-work".into(),
                plan: Some("max_20x".into()),
                windows: vec![
                    window("Claude 5 Hour", "shared", Some(34), Some(minutes(72)), None),
                    window("Claude 7 Day", "shared", Some(61), Some(minutes(3 * 1440 + 4 * 60 + 20)), None),
                    window("Claude 7 Day (Fable)", "fable", Some(88), Some(minutes(3 * 1440 + 4 * 60 + 20)), None),
                    window("Claude Extra Usage", "extra", Some(25), Some(minutes(17 * 1440)), Some(omp_runtime::Spend { used_cents: 1_240, limit_cents: Some(5_000) })),
                ],
                error: None,
            },
            omp_runtime::AccountUsage {
                account_id: "shot:claude-home".into(),
                plan: Some("pro".into()),
                windows: Vec::new(),
                error: Some("the usage service answered 429 Too Many Requests".into()),
            },
            omp_runtime::AccountUsage {
                account_id: "shot:chatgpt-side".into(),
                plan: Some("plus".into()),
                windows: vec![
                    window("5 hours", "shared", Some(12), Some(minutes(215)), None),
                    window("7 days", "shared", Some(40), Some(minutes(5 * 1440 + 60)), None),
                    window("7 days (Spark)", "spark", Some(5), Some(minutes(5 * 1440 + 60)), None),
                ],
                error: None,
            },
            omp_runtime::AccountUsage {
                account_id: "shot:openai-api".into(),
                plan: None,
                windows: vec![window("Monthly spend", "shared", None, None, Some(omp_runtime::Spend { used_cents: 310, limit_cents: None }))],
                error: None,
            },
        ];
        self.ivars().accounts.replace(vec![
            entry("shot:claude-work", AuthProvider::Anthropic, "Work"),
            entry("shot:claude-home", AuthProvider::Anthropic, "Home"),
            entry("shot:chatgpt-side", AuthProvider::OpenAi, "Side project"),
            entry("shot:openai-api", AuthProvider::OpenAiApi, "Batch jobs"),
        ]);
        let mut stored = self.ivars().account_usage.borrow_mut();
        for usage in usages {
            stored.insert(usage.account_id.clone(), usage);
        }
        drop(stored);
        self.ivars().account_usage_answered_at.set(std::time::Instant::now().checked_sub(std::time::Duration::from_secs(150)));
        self.session().current_account.replace(Some("shot:claude-work".into()));
        // A reading that never lands, so the card keeps its spinner.
        self.ivars().account_usage_loading.set(in_flight);
    }
}
