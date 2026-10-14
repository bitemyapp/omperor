//! Native evidence workspace. Retained text surfaces, session-local reading
//! anchors, and explicit context transfer; no inference or I/O on the UI thread.
use super::*;
use objc2_app_kit::NSAttributedStringNSStringDrawing;
use workspace_model::{EvidenceSnapshot, MAX_PINS, ObservatoryFocus, SessionDesk};

pub(super) const SEARCH_TAG: isize = -8803;

#[derive(Debug)]
struct EvidencePane {
    view: Retained<NSView>,
    title: Retained<NSTextField>,
    metadata: Retained<NSTextField>,
    source: Retained<NSTextField>,
    older: Retained<NSButton>,
    newer: Retained<NSButton>,
    latest: Retained<NSButton>,
    notice_toggle: Retained<NSButton>,
    notice: Retained<NSTextField>,
    rule: Retained<NSBox>,
    identity: Retained<NSImageView>,
    scroll: Retained<NSScrollView>,
    text: Retained<NSTextView>,
    fingerprint: RefCell<String>,
    anchor: RefCell<Option<(String, Option<usize>, usize)>>,
    page_start: Cell<(usize, usize)>,
}

#[derive(Debug)]
pub(super) struct SessionWorkspace {
    tabs: Retained<header_ui::SurfaceSwitch>,
    surface: Retained<NSView>,
    overview: Retained<NSView>,
    evidence_header: Retained<NSView>,
    scroll: Retained<NSScrollView>,
    document: Retained<NSView>,
    ornaments: RefCell<Vec<Retained<NSView>>>,
    panes: Vec<EvidencePane>,
    lanes: [ObservatoryLane; 2],
    updating_lanes: Cell<bool>,
    session: RefCell<Option<Option<std::path::PathBuf>>>,
    composer: RefCell<Option<Vec<(Retained<NSView>, bool)>>>,
    footer: Retained<NSTextField>,
    arrangement: Retained<NSSegmentedControl>,
    search: Retained<NSSearchField>,
    restoring_layout: Cell<bool>,
}

/// Persistent native scroll surfaces; only visible rows own AppKit controls.
#[derive(Debug)]
struct ObservatoryLane {
    view: Retained<NSView>,
    background: Retained<NSBox>,
    title: Retained<NSTextField>,
    count: Retained<NSTextField>,
    range: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    document: Retained<NSView>,
    tasks: RefCell<Vec<omp_runtime::SubagentInfo>>,
    ids: RefCell<Vec<String>>,
    rows: RefCell<std::collections::BTreeMap<usize, Retained<NSView>>>,
    clocks: RefCell<std::collections::BTreeMap<usize, (Retained<NSTextField>, u64)>>,
    clock_second: Cell<u64>,
    range_text: RefCell<String>,
    rendered: RefCell<std::ops::Range<usize>>,
    row_height: Cell<f64>,
    pins: RefCell<Vec<String>>,
    selected: RefCell<Option<String>>,
}

impl ObservatoryLane {
    fn new(parent: &NSView, heading: &str, delegate: &AppDelegate) -> Self {
        let mtm = delegate.mtm();
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 500.0, 400.0));
        let background = card_box(view.bounds(), 10.0, mtm);
        background.setFillColor(&theme::surface_inset());
        view.addSubview(&background);
        let title = text_label(heading, 15.0, true, mtm);
        let count = text_label("", 13.0, false, mtm);
        let range = text_label("", 11.0, false, mtm);
        count.setTextColor(Some(&theme::secondary_text()));
        range.setTextColor(Some(&theme::secondary_text()));
        range.setAlignment(NSTextAlignment::Right);
        for label in [&title, &count, &range] {
            view.addSubview(label);
        }
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 0.0, 500.0, 360.0));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setAccessibilityLabel(Some(&NSString::from_str(&format!("{heading} agents"))));
        let document = NSView::initWithFrame(NSView::alloc(mtm), scroll.bounds());
        scroll.setDocumentView(Some(&document));
        view.addSubview(&scroll);
        parent.addSubview(&view);
        let clip = scroll.contentView();
        clip.setPostsBoundsChangedNotifications(true);
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                delegate,
                sel!(observatoryDidScroll:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
            );
        }
        Self {
            view,
            background,
            title,
            count,
            range,
            scroll,
            document,
            tasks: RefCell::new(Vec::new()),
            ids: RefCell::new(Vec::new()),
            rows: RefCell::new(std::collections::BTreeMap::new()),
            clocks: RefCell::new(Default::default()),
            clock_second: Cell::new(0),
            range_text: RefCell::new(String::new()),
            rendered: RefCell::new(0..0),
            row_height: Cell::new(44.0),
            pins: RefCell::new(Vec::new()),
            selected: RefCell::new(None),
        }
    }

    fn top(&self) -> f64 {
        (self.document.frame().size.height
            - self.scroll.contentView().bounds().origin.y
            - self.scroll.contentSize().height)
            .max(0.0)
    }

    fn position(&self) -> workspace_model::AgentListPosition {
        workspace_model::AgentListPosition::capture(
            &self.ids.borrow(),
            self.top(),
            self.row_height.get(),
        )
    }
}

fn mono(label: &NSTextField, size: f64) {
    label.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        size,
        unsafe { NSFontWeightRegular },
    )));
}

fn action(
    title: &str,
    frame: NSRect,
    tag: isize,
    selector: objc2::runtime::Sel,
    delegate: &AppDelegate,
) -> Retained<NSButton> {
    let button = settings_button(title, frame, delegate, selector, delegate.mtm());
    button.setTag(tag);
    button.setFont(Some(&theme::ui_font(12.0, false)));
    button
}

impl EvidencePane {
    fn new(parent: &NSView, slot: usize, delegate: &AppDelegate) -> Self {
        let mtm = delegate.mtm();
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 500.0, 336.0));
        let frame = card_box(rect(0.0, 0.0, 500.0, 336.0), 6.0, mtm);
        frame.setFillColor(&theme::canvas());
        frame.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.addSubview(&frame);
        let line = fill_box(rect(1.0, 333.0, 498.0, 2.0), &theme::hairline(), 0.0, mtm);
        line.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.addSubview(&line);
        let title = text_label("", 19.0, true, mtm);
        title.setFrame(rect(14.0, 296.0, 448.0, 22.0));
        title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        title.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.addSubview(&title);
        let metadata = text_label("", 12.0, false, mtm);
        mono(&metadata, 11.0);
        metadata.setFrame(rect(14.0, 273.0, 472.0, 20.0));
        metadata.setTextColor(Some(&theme::signal()));
        metadata.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        metadata.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        view.addSubview(&metadata);
        let identity = NSImageView::initWithFrame(NSImageView::alloc(mtm), NSRect::ZERO);
        identity
            .setImage(theme::symbol("cpu", 14.0, unsafe { NSFontWeightMedium }, None).as_deref());
        identity.setContentTintColor(Some(&theme::accent()));
        view.addSubview(&identity);
        let notice_toggle = icon_button(
            "chevron.right",
            "Expand task notice",
            NSRect::ZERO,
            delegate,
            sel!(workspaceEvidenceAction:),
            mtm,
        );
        notice_toggle.setTag(slot as isize * 10 + 5);
        let notice = text_label("", 12.0, false, mtm);
        notice.setTextColor(Some(&theme::secondary_text()));
        view.addSubview(&notice_toggle);
        view.addSubview(&notice);
        let (scroll, text) = transcript_control(rect(4.0, 64.0, 492.0, 201.0), mtm);
        text.setRichText(true);
        text.setDelegate(Some(ProtocolObject::from_ref(delegate)));
        text.setTextContainerInset(NSSize::new(10.0, 8.0));
        view.addSubview(&scroll);
        let rule = fill_box(rect(12.0, 62.0, 476.0, 1.0), &theme::hairline(), 0.0, mtm);
        rule.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        view.addSubview(&rule);
        let source = text_label("", 12.0, false, mtm);
        mono(&source, 11.0);
        source.setTextColor(Some(&theme::secondary_text()));
        source.setFrame(rect(14.0, 39.0, 472.0, 18.0));
        source.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        source.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        view.addSubview(&source);
        let older = action(
            "‹",
            rect(10.0, 8.0, 28.0, 24.0),
            slot as isize * 10,
            sel!(workspaceEvidenceAction:),
            delegate,
        );
        let newer = action(
            "›",
            rect(40.0, 8.0, 28.0, 24.0),
            slot as isize * 10 + 1,
            sel!(workspaceEvidenceAction:),
            delegate,
        );
        let latest = action(
            "Latest",
            rect(410.0, 8.0, 78.0, 25.0),
            slot as isize * 10 + 3,
            sel!(workspaceEvidenceAction:),
            delegate,
        );
        latest.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        latest.setToolTip(Some(ns_string!(
            "Follow the latest recorded output, or retry loading the journal"
        )));
        older.setAccessibilityLabel(Some(ns_string!("Earlier transcript messages")));
        older.setToolTip(Some(ns_string!("Read the preceding transcript page")));
        newer.setAccessibilityLabel(Some(ns_string!("Later transcript messages")));
        newer.setToolTip(Some(ns_string!("Read the next transcript page")));
        for button in [&older, &newer, &latest] {
            view.addSubview(button);
        }
        parent.addSubview(&view);
        Self {
            view,
            title,
            metadata,
            source,
            older,
            newer,
            latest,
            notice_toggle,
            notice,
            rule,
            identity,
            scroll,
            text,
            fingerprint: RefCell::new(String::new()),
            anchor: RefCell::new(None),
            page_start: Cell::new((0, 0)),
        }
    }
    fn layout(&self, width: f64, height: f64, notice: &str, expanded: bool) {
        self.title
            .setFrame(rect(18.0, height - 64.0, width - 36.0, 30.0));
        self.metadata
            .setFrame(rect(42.0, height - 29.0, width - 82.0, 18.0));
        self.identity
            .setFrame(rect(18.0, height - 29.0, 16.0, 18.0));
        let has_notice = !notice.is_empty();
        self.notice.setHidden(!has_notice);
        self.notice.setStringValue(&NSString::from_str(notice));
        self.notice.setToolTip(Some(&NSString::from_str(notice)));
        self.notice
            .setMaximumNumberOfLines(if expanded { 0 } else { 1 });
        self.notice.setLineBreakMode(if expanded {
            objc2_app_kit::NSLineBreakMode::ByWordWrapping
        } else {
            objc2_app_kit::NSLineBreakMode::ByTruncatingTail
        });
        let can_expand = self.notice.attributedStringValue().size().width > width - 28.0
            || notice.contains('\n');
        self.notice_toggle.setHidden(!has_notice || !can_expand);
        let note_height = if !has_notice {
            0.0
        } else if expanded && can_expand {
            self.notice
                .cell()
                .map(|cell| {
                    cell.cellSizeForBounds(rect(0.0, 0.0, width - 54.0, 1000.0))
                        .height
                })
                .unwrap_or(36.0)
                .ceil()
                .clamp(18.0, (height * 0.3).max(18.0))
        } else {
            18.0
        };
        let notice_end = 70.0 + if has_notice { note_height + 6.0 } else { 0.0 };
        let header = notice_end + 8.0;
        let note_x = if can_expand { 34.0 } else { 14.0 };
        self.notice.setFrame(rect(
            note_x,
            height - notice_end + 3.0,
            width - note_x - 14.0,
            note_height,
        ));
        self.notice_toggle
            .setFrame(rect(11.0, height - 90.0, 20.0, 20.0));
        self.notice_toggle.setImage(
            theme::symbol(
                if expanded {
                    "chevron.down"
                } else {
                    "chevron.right"
                },
                10.0,
                unsafe { NSFontWeightRegular },
                None,
            )
            .as_deref(),
        );
        let notice_label = format!(
            "{} task notice: {notice}",
            if expanded { "Collapse" } else { "Expand" }
        );
        self.notice_toggle
            .setAccessibilityLabel(Some(&NSString::from_str(&notice_label)));
        self.notice_toggle
            .setToolTip(Some(&NSString::from_str(&notice_label)));
        self.source.setFrame(rect(14.0, 8.0, width - 182.0, 18.0));
        self.older.setFrame(rect(width - 153.0, 5.0, 28.0, 24.0));
        self.newer.setFrame(rect(width - 123.0, 5.0, 28.0, 24.0));
        self.latest.setFrame(rect(width - 90.0, 5.0, 80.0, 24.0));
        self.rule.setFrame(rect(12.0, 33.0, width - 24.0, 1.0));
        self.scroll.setFrame(rect(
            4.0,
            35.0,
            width - 8.0,
            (height - header - 37.0).max(0.0),
        ));
    }
}

impl SessionWorkspace {
    /// The Conversation / Observatory switch, which the Linux scroll smoke
    /// drives.
    #[cfg(target_os = "linux")]
    pub(super) fn surface_switch(&self) -> Retained<NSSegmentedControl> {
        Retained::into_super(self.tabs.clone())
    }
}

impl AppDelegate {
    pub(super) fn install_session_workspace(&self, parent: &NSView) {
        let mtm = self.mtm();
        // Drawn in the crown's gold over a capsule; the native control
        // stays on top, transparent, for its behaviour and accessibility.
        let tabs = header_ui::SurfaceSwitch::install(parent, ["Conversation", "Observatory"], mtm);
        tabs.setSegmentCount(2);
        tabs.setLabel_forSegment(ns_string!("Conversation"), 0);
        tabs.setLabel_forSegment(ns_string!("Observatory"), 1);
        tabs.setSegmentStyle(NSSegmentStyle::Separated);
        tabs.setSegmentDistribution(NSSegmentDistribution::FillEqually);
        tabs.setSelectedSegment(0);
        tabs.setToolTip_forSegment(
            Some(ns_string!(
                "Inspect agents and compare their work without leaving this session"
            )),
            1,
        );
        unsafe {
            tabs.setTarget(Some::<&AnyObject>(self));
            tabs.setAction(Some(sel!(selectSessionSurface:)));
        }
        let footer = text_label(
            "Draft preserved · Conversation ⌘3 · Observatory ⌘4",
            12.0,
            false,
            mtm,
        );
        footer.setTextColor(Some(&theme::secondary_text()));
        mono(&footer, 11.0);
        footer.setHidden(true);
        parent.addSubview(&footer);
        let arrangement =
            NSSegmentedControl::initWithFrame(NSSegmentedControl::alloc(mtm), NSRect::ZERO);
        arrangement.setSegmentCount(3);
        arrangement.setSegmentStyle(NSSegmentStyle::Separated);
        arrangement.setSegmentDistribution(NSSegmentDistribution::FillEqually);
        arrangement.setFont(Some(&theme::ui_font(12.0, false)));
        for (i, label) in ["Overview", "Inspect", "Compare"].iter().enumerate() {
            arrangement.setLabel_forSegment(&NSString::from_str(label), i as isize);
        }
        arrangement.setAccessibilityLabel(Some(ns_string!("Observatory arrangement")));
        arrangement.setToolTip_forSegment(
            Some(ns_string!(
                "Give the entire area to the running and finished lists"
            )),
            0,
        );
        arrangement.setToolTip_forSegment(
            Some(ns_string!(
                "Inspect an agent beside the running and finished lists"
            )),
            1,
        );
        arrangement.setToolTip_forSegment(
            Some(ns_string!("Compare the transcripts of open agent tabs")),
            2,
        );
        unsafe {
            arrangement.setTarget(Some::<&AnyObject>(self));
            arrangement.setAction(Some(sel!(observatoryArrangement:)));
        }
        arrangement.setHidden(true);
        parent.addSubview(&arrangement);
        let surface = NSView::initWithFrame(NSView::alloc(mtm), rect(20.0, 218.0, 500.0, 400.0));
        let overview = NSView::initWithFrame(NSView::alloc(mtm), surface.bounds());
        let evidence_header = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        surface.addSubview(&overview);
        surface.addSubview(&evidence_header);
        surface.setHidden(true);
        parent.addSubview(&surface);
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(20.0, 218.0, 500.0, 400.0));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setBorderType(NSBorderType::NoBorder);
        let document = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 500.0, 400.0));
        scroll.setDocumentView(Some(&document));
        let search =
            NSSearchField::initWithFrame(NSSearchField::alloc(mtm), rect(0.0, 0.0, 240.0, 26.0));
        search.setTag(SEARCH_TAG);
        search.setFont(Some(&theme::ui_font(13.0, false)));
        search.setPlaceholderString(Some(ns_string!("Find a task, role, or error…")));
        search.setAccessibilityLabel(Some(ns_string!("Search Observatory agents")));
        unsafe {
            search.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        overview.addSubview(&search);
        let lanes =
            ["Running", "Finished"].map(|heading| ObservatoryLane::new(&overview, heading, self));
        let panes = (0..=MAX_PINS)
            .map(|slot| EvidencePane::new(&document, slot, self))
            .collect();
        scroll.setHidden(true);
        surface.addSubview(&scroll);
        self.ivars()
            .session_workspace
            .set(SessionWorkspace {
                tabs,
                surface,
                overview,
                evidence_header,
                scroll,
                document,
                panes,
                ornaments: RefCell::new(Vec::new()),
                lanes,
                updating_lanes: Cell::new(false),
                session: RefCell::new(None),
                composer: RefCell::new(None),
                footer,
                arrangement,
                search,
                restoring_layout: Cell::new(false),
            })
            .expect("workspace once");
        self.layout_session_workspace();
    }

    pub(super) fn edit_desk(&self, edit: impl FnOnce(&mut SessionDesk)) {
        let owner = self.session();
        let key = owner.selected_session_path.borrow().clone();
        edit(
            &mut self
                .ivars()
                .agent_view_states
                .borrow_mut()
                .entry(key)
                .or_default()
                .desk,
        );
    }

    pub(super) fn capture_workspace_positions(&self) {
        let owner = self.session();
        let Some(ui) = self.ivars().session_workspace.get() else {
            return;
        };
        let key = owner.selected_session_path.borrow().clone();
        if ui.session.borrow().as_ref() != Some(&key) {
            return;
        }
        self.edit_desk(|desk| {
            if !ui.updating_lanes.get() && !ui.overview.isHidden() && !ui.surface.isHidden() {
                for (index, lane) in
                    ui.lanes.iter().enumerate().filter(|(_, lane)| {
                        !lane.view.isHidden() && !lane.tasks.borrow().is_empty()
                    })
                {
                    desk.agent_positions[index] = lane.position();
                }
            }
            for (pane, pin) in ui
                .panes
                .iter()
                .zip(&mut desk.pins)
                .filter(|_| !ui.scroll.isHidden() && !ui.surface.isHidden())
            {
                if !pane.view.isHidden()
                    && pane.anchor.borrow().as_ref()
                        == Some(&(pin.id.clone(), pin.message, pin.offset))
                {
                    pin.scroll_y = pane.scroll.contentView().bounds().origin.y;
                    if pin.message.is_none() {
                        let bottom = (pane.text.frame().size.height
                            - pane.scroll.contentSize().height)
                            .max(0.0);
                        if bottom - pin.scroll_y > 24.0 {
                            let (index, offset) = pane.page_start.get();
                            pin.message = Some(index);
                            pin.offset = offset;
                        } else {
                            pin.scroll_y = f64::MAX;
                        }
                    }
                    let selection = pane.text.selectedRange();
                    pin.selection = (selection.location, selection.length);
                }
            }
            if !ui.scroll.isHidden() && !ui.surface.isHidden() {
                desk.evidence_top = (ui.document.frame().size.height
                    - ui.scroll.contentView().bounds().origin.y
                    - ui.scroll.contentSize().height)
                    .max(0.0);
            }
        });
    }

    pub(super) fn capture_session_desk(&self) {
        self.capture_workspace_positions();
        self.capture_agent_inspection_position();
        let ivars = self.ivars();
        let draft = ivars
            .input
            .get()
            .map(|input| input.stringValue().to_string())
            .unwrap_or_default();
        self.edit_desk(|desk| {
            desk.draft = draft;
            if !desk.active
                && let Some(transcript) = ivars.chat_transcript.get()
            {
                desk.conversation_y = transcript.scroll_view().contentView().bounds().origin.y;
                desk.conversation_place = Some(transcript.place());
                desk.conversation_selected = transcript.selection();
            }
            if let (Some(scroll), Some(view)) = (ivars.agents_scroll.get(), ivars.agents_view.get())
            {
                desk.tree_top = (view.frame().size.height
                    - scroll.contentView().bounds().origin.y
                    - scroll.contentSize().height)
                    .max(0.0);
            }
            if desk.active {
                desk.workspace_inspector = ivars.inspector_visible.get();
            } else {
                desk.conversation_inspector = ivars.inspector_visible.get();
            }
            desk.expanded = ivars.workbench_expanded.get();
            if let Some(picker) = ivars.workbench_switcher.get() {
                desk.panel = picker.selectedSegment();
            }
        });
    }

    pub(super) fn restore_session_desk(&self) {
        let desk = self.agent_view().desk;
        self.restore_pending_attachments(&desk.attachments);
        if let Some(input) = self.ivars().input.get() {
            input.setStringValue(&NSString::from_str(&desk.draft));
        }
        self.ivars().workbench_expanded.set(desk.expanded);
        self.show_workbench_panel(desk.panel);
        self.set_workbench_visible(if desk.active {
            desk.workspace_inspector
        } else {
            desk.conversation_inspector
        });
        self.layout_session_workspace();
        for pin in &desk.pins {
            self.ensure_workspace_evidence(&pin.id);
        }
    }

    pub(super) fn restore_conversation_position_if_ready(&self) {
        let owner = self.session();
        if owner.session_history.borrow().is_some()
            || !self.ivars().desk_restore_pending.replace(false)
        {
            return;
        }
        let desk = self.agent_view().desk;
        if let Some(transcript) = self.ivars().chat_transcript.get() {
            transcript.restore_place(desk.conversation_place.as_ref(), desk.conversation_y);
            transcript.restore_selection(desk.conversation_selected);
        }
    }

    pub(super) fn set_session_surface(&self, workspace: bool) {
        self.schedule_session_view_save();
        self.capture_session_desk();
        if workspace {
            self.hide_file_picker();
        }
        self.edit_desk(|desk| desk.active = workspace);
        let desk = self.agent_view().desk;
        self.set_workbench_visible(if workspace {
            desk.workspace_inspector
        } else {
            desk.conversation_inspector
        });
        self.layout_session_workspace();
        // Keep the conversation's reading anchor across width changes.
        self.ivars().desk_restore_pending.set(true);
        self.refresh_chat_transcript();
    }

    /// Capture before AppKit changes clip sizes; afterwards the old reading
    /// position can no longer be derived from the resized visible rectangle.
    pub(super) fn prepare_workspace_layout(&self) {
        let owner = self.session();
        let ivars = self.ivars();
        if let Some(ui) = ivars.session_workspace.get()
            && !ui.restoring_layout.replace(true)
        {
            self.capture_workspace_positions();
        }
        let key = owner.selected_session_path.borrow().clone();
        if ivars.agents_rendered_session.borrow().as_ref() == Some(&key)
            && !ivars.agents_layout_restore.replace(true)
            && let (Some(scroll), Some(view)) = (ivars.agents_scroll.get(), ivars.agents_view.get())
        {
            let top = (view.frame().size.height
                - scroll.contentView().bounds().origin.y
                - scroll.contentSize().height)
                .max(0.0);
            self.edit_desk(|desk| desk.tree_top = top);
        }
    }

    pub(super) fn layout_session_workspace(&self) {
        let owner = self.session();
        let ivars = self.ivars();
        let (Some(ui), Some(transcript)) = (ivars.session_workspace.get(), ivars.chat_transcript.get())
        else {
            return;
        };
        let scroll = transcript.scroll_view().clone();
        let Some(parent) = (unsafe { scroll.superview() }) else {
            return;
        };
        let frame = scroll.frame();
        ui.tabs
            .place(rect(24.0, parent.bounds().size.height - 152.0 - layout::USAGE_STRIP_HEIGHT, 260.0, 30.0));
        let active = self.agent_view().desk.active;
        let workspace_frame = rect(
            frame.origin.x,
            44.0,
            frame.size.width,
            (parent.bounds().size.height - 206.0 - layout::USAGE_STRIP_HEIGHT - self.account_banner_space(parent.bounds().size)).max(180.0),
        );
        if ui.surface.frame() != workspace_frame {
            self.prepare_workspace_layout();
            ui.surface.setFrame(workspace_frame);
        }
        ui.footer.setFrame(rect(
            frame.origin.x,
            12.0,
            (frame.size.width - 250.0).max(100.0),
            20.0,
        ));
        ui.footer
            .setStringValue(&NSString::from_str(if frame.size.width < 620.0 {
                "Draft preserved"
            } else if frame.size.width < 800.0 {
                "Draft preserved · ⌘3 / ⌘4"
            } else {
                "Draft preserved · Conversation ⌘3 · Observatory ⌘4"
            }));
        ui.footer
            .setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        ui.footer.setHidden(!active);
        ui.arrangement.setFrame(rect(
            frame.origin.x + frame.size.width - 248.0,
            8.0,
            248.0,
            28.0,
        ));
        ui.arrangement.setHidden(!active);
        if active {
            if ui.composer.borrow().is_none() {
                let mut controls = Vec::new();
                macro_rules! keep {
                    ($field:ident) => {
                        if let Some(control) = ivars.$field.get() {
                            let view: &NSView = control;
                            controls.push((view.retain(), view.isHidden()));
                        }
                    };
                }
                keep!(composer_surface);
                keep!(composer_hint);
                keep!(busy_spinner);
                keep!(busy_label);
                keep!(input);
                keep!(model_chip);
                keep!(thinking_chip);
                keep!(fast_chip);
                keep!(account_chip);
                keep!(model_label);
                keep!(thinking_label);
                keep!(fast_label);
                keep!(account_label);
                keep!(model_picker);
                keep!(thinking_picker);
                keep!(account_picker);
                keep!(fast_button);
                keep!(fast_icon);
                keep!(stop_button);
                keep!(attach_button);
                keep!(cache_warning_badge);
                keep!(send_button);
                ui.composer.replace(Some(controls));
            }
            if let Some(controls) = ui.composer.borrow().as_ref() {
                for (view, _) in controls {
                    view.setHidden(true);
                }
            }
        } else {
            let controls = ui.composer.borrow_mut().take();
            if let Some(controls) = controls {
                for (view, hidden) in controls {
                    view.setHidden(hidden);
                }
                if let Some(stop) = ivars.stop_button.get() {
                    stop.setHidden(!owner.chat_busy.get());
                }
                // Recompute chip widths after restoring their visibility.
                self.refit_composer_chips();
            }
        }
        // Attachment chips are rebuilt as preparation finishes, so update the
        // current buttons rather than retaining a stale visibility snapshot.
        self.set_pending_attachments_hidden(active);
        ui.tabs.select(if active { 1 } else { 0 });
        scroll.setHidden(active);
        ui.surface.setHidden(!active);
        if active {
            self.request_observatory_refresh();
        }
    }

    pub(super) fn pin_workspace_task(&self, id: &str) {
        self.open_workspace_agent(id, false);
    }

    pub(super) fn open_workspace_agent(&self, id: &str, persistent: bool) {
        self.capture_workspace_positions();
        let split_available = self.ivars().session_workspace.get().is_none_or(|ui| {
            let size = ui.surface.bounds().size;
            workspace_model::observatory_split_available(size.width - 12.0)
        });
        let mut capacity_reached = false;
        self.edit_desk(|desk| {
            desk.inspect(id);
            if persistent && let Some(slot) = desk.pins.iter().position(|pin| pin.id == id) {
                capacity_reached = !desk.keep(slot);
            }
            if !split_available {
                if let Some(slot) = desk.pins.iter().position(|pin| pin.id == id) {
                    desk.focus_pin(slot);
                }
            }
        });
        self.ensure_workspace_evidence(id);
        self.set_session_surface(true);
        if capacity_reached {
            self.flash_chat_status("Four agent tabs are open. Close one to open another; this agent remains available in preview.");
        }
        if let Some(ui) = self.ivars().session_workspace.get()
            && let Some(slot) = self
                .agent_view()
                .desk
                .pins
                .iter()
                .position(|pin| pin.id == id)
        {
            ui.document.scrollRectToVisible(ui.panes[slot].view.frame());
        }
    }

    pub(super) fn ensure_workspace_evidence(&self, id: &str) {
        let owner = self.session();
        if owner.subagent_transcripts.borrow().contains_key(id)
            && !owner.workspace_read_failures.borrow().contains_key(id)
        {
            return;
        }
        let session = owner.selected_session_path.borrow().clone();
        let generation = owner.evidence_generation.get();
        let key = (generation, id.to_owned());
        if owner.workspace_reads.borrow().contains(&key) {
            return;
        }
        let path = owner.subagents
            .borrow()
            .iter()
            .find(|task| task.id == id)
            .and_then(|task| task.session_file.clone());
        let Some(path) = path else {
            return;
        };
        let task = id.to_owned();
        let session_key = owner.key;
        if self
            .jobs()
            .submit(move || BackgroundEvent::WorkspaceEvidenceRead {
                key: session_key,
                generation,
                session,
                id: task,
                result: transcripts::read(&path),
            })
        {
            owner.workspace_read_failures.borrow_mut().remove(id);
            owner.workspace_reads.borrow_mut().insert(key);
        } else {
            owner.workspace_read_failures.borrow_mut().insert(
                id.to_owned(),
                "The journal reader is unavailable. Choose Latest to retry.".into(),
            );
        }
    }

    pub(super) fn evidence_action(&self, tag: isize) {
        let owner = self.session();
        self.schedule_session_view_save();
        if tag < 0 {
            return;
        }
        self.capture_workspace_positions();
        if tag >= 2000 {
            let mut kept = false;
            self.edit_desk(|desk| kept = desk.keep((tag - 2000) as usize));
            if !kept {
                self.flash_chat_status(
                    "Four agent tabs are open. Close one before opening another; previewing remains available.",
                );
            }
            self.preserve_workspace_layout();
            self.request_observatory_refresh();
            return;
        }
        if tag == 1000 {
            self.edit_desk(SessionDesk::compare_pins);
            self.preserve_workspace_layout();
            self.request_observatory_refresh();
            return;
        }
        let slot = tag as usize / 10;
        let action = tag as usize % 10;
        let mut retry = None;
        let mut capacity_reached = false;
        self.edit_desk(|desk| {
            if slot >= desk.pins.len() {
                return;
            }
            if action == 2 {
                let removed = desk.pins.remove(slot);
                if desk.focused_pin.as_deref() == Some(&removed.id) {
                    desk.compare_pins();
                }
                if desk.selected_pin.as_deref() == Some(&removed.id) {
                    desk.selected_pin = desk.pins.last().map(|pin| pin.id.clone());
                }
                if desk.observatory_focus == ObservatoryFocus::Pins
                    && desk.focused_pin.is_none()
                    && desk.pins.iter().filter(|pin| pin.kept).count() < 2
                {
                    desk.observatory_focus = ObservatoryFocus::Split;
                }
                return;
            }
            if action == 6 {
                desk.selected_pin = Some(desk.pins[slot].id.clone());
                if desk.focused_pin.is_some() {
                    desk.focused_pin = desk.selected_pin.clone();
                } else {
                    let tabbed_comparison = desk.observatory_focus == ObservatoryFocus::Pins
                        && self.ivars().session_workspace.get().is_some_and(|ui| {
                            let size = ui.scroll.contentSize();
                            workspace_model::evidence_uses_tabs(
                                size.width,
                                size.height,
                                desk.pins.iter().filter(|pin| pin.kept).count(),
                            )
                        });
                    if !tabbed_comparison {
                        desk.observatory_focus = ObservatoryFocus::Split;
                    }
                }
                if NSApplication::sharedApplication(self.mtm())
                    .currentEvent()
                    .is_some_and(|event| event.clickCount() == 2)
                {
                    capacity_reached = !desk.keep(slot);
                }
                desk.evidence_top = 0.0;
                return;
            }
            let pin = &mut desk.pins[slot];
            if action == 5 {
                pin.notice_expanded = !pin.notice_expanded;
                return;
            }
            if action == 3 {
                pin.message = None;
                pin.offset = 0;
                pin.scroll_y = f64::MAX;
                pin.selection = (0, 0);
                pin.comparison_scroll_y = None;
                retry = Some(pin.id.clone());
            } else if let Some(entry) = owner.subagent_transcripts.borrow().get(&pin.id) {
                workspace_model::navigate(pin, &entry.messages, action == 1);
            }
        });
        if capacity_reached {
            self.flash_chat_status("Four agent tabs are open. Close one before keeping another; previewing remains available.");
        }
        if let Some(id) = retry {
            self.ensure_workspace_evidence(&id);
        }
        self.preserve_workspace_layout();
        self.request_observatory_refresh();
    }

    pub(super) fn persist_selected_agent_tab(&self) {
        let desk = self.agent_view().desk;
        if !desk.active {
            return;
        }
        if let Some(slot) = desk
            .pins
            .iter()
            .position(|pin| Some(&pin.id) == desk.selected_pin.as_ref())
        {
            self.evidence_action(2000 + slot as isize);
        }
    }

    /// Tool details expand inside their originating transcript, never a modal.
    pub(super) fn toggle_workspace_tool(&self, text: &NSTextView, index: usize) -> bool {
        let owner = self.session();
        let Some(ui) = self.ivars().session_workspace.get() else {
            return false;
        };
        let Some(slot) = ui
            .panes
            .iter()
            .position(|pane| std::ptr::eq(&*pane.text, text))
        else {
            return false;
        };
        self.capture_workspace_positions();
        self.schedule_session_view_save();
        self.edit_desk(|desk| {
            if let Some(pin) = desk.pins.get_mut(slot) {
                if !owner.subagent_transcripts
                    .borrow()
                    .get(&pin.id)
                    .and_then(|entry| entry.messages.get(index))
                    .is_some_and(|message| message.speaker == session::Speaker::Tool)
                {
                    return;
                }
                let pane = &ui.panes[slot];
                let (start, offset) = pane.page_start.get();
                pin.message = Some(start);
                pin.offset = offset;
                pin.scroll_y = pane.scroll.contentView().bounds().origin.y;
                if !pin.expanded_tools.remove(&index) {
                    pin.expanded_tools.insert(index);
                }
                pin.selection = (0, 0);
            }
        });
        self.preserve_workspace_layout();
        self.request_observatory_refresh();
        true
    }

    pub(super) fn compose_from_evidence(&self) {
        self.capture_workspace_positions();
        let desk = self.agent_view().desk;
        let pins: Vec<_> = desk
            .pins
            .iter()
            .filter(|pin| {
                if desk.observatory_focus == ObservatoryFocus::Pins && desk.focused_pin.is_none() {
                    pin.kept
                } else {
                    Some(&pin.id) == desk.focused_pin.as_ref().or(desk.selected_pin.as_ref())
                }
            })
            .cloned()
            .collect();
        if pins.is_empty() {
            return;
        }
        let prompt = workspace_model::evidence_prompt(&pins);
        if let Some(input) = self.ivars().input.get() {
            let draft = input.stringValue().to_string();
            let combined = if draft.trim().is_empty() {
                prompt
            } else {
                format!("{draft}\n\n{prompt}")
            };
            input.setStringValue(&NSString::from_str(&combined));
            self.edit_desk(|desk| desk.draft = combined);
        }
        self.set_session_surface(false);
        if let (Some(window), Some(input)) = (self.ivars().window.get(), self.ivars().input.get()) {
            window.makeFirstResponder(Some(&**input));
            if let Some(editor) = input.currentEditor() {
                editor.setSelectedRange(NSRange::new(input.stringValue().length(), 0));
            }
        }
        self.flash_chat_status("Evidence added to your draft · Review and send when ready");
    }

    pub(super) fn filter_workspace_work(&self, clear: bool) {
        if let Some(ui) = self.ivars().session_workspace.get() {
            let query = if clear {
                String::new()
            } else {
                ui.search.stringValue().to_string()
            };
            self.edit_agent_view(|view| {
                view.query = query;
                view.page = 0;
                view.desk.agent_positions = Default::default();
            });
        }
    }

    pub(super) fn set_observatory_focus(&self, focus: ObservatoryFocus) {
        self.schedule_session_view_save();
        self.capture_workspace_positions();
        self.edit_desk(|desk| {
            desk.compare_pins();
            desk.observatory_focus = focus;
        });
        self.preserve_workspace_layout();
        if focus != ObservatoryFocus::Pins
            && self.ivars().inspector_visible.get()
            && self
                .ivars()
                .workbench_switcher
                .get()
                .is_some_and(|picker| picker.selectedSegment() == 1)
        {
            self.set_workbench_visible(false);
        }
        self.request_observatory_refresh();
    }

    pub(super) fn workspace_scope(&self, tag: isize) {
        let _span = perf::span("gui.observatory.scope");
        self.capture_workspace_positions();
        self.edit_agent_view(|view| match tag {
            21 => view.compact = !view.compact,
            _ => {
                view.scope = match tag {
                    1 => agents::WorkScope::Active,
                    2 => agents::WorkScope::Attention,
                    _ => agents::WorkScope::All,
                };
                view.page = 0;
                view.desk.agent_positions = Default::default();
            }
        });
    }

    /// Skip recapturing old frames after a deliberate reading-state edit.
    pub(super) fn preserve_workspace_layout(&self) {
        if let Some(ui) = self.ivars().session_workspace.get() {
            ui.restoring_layout.set(true);
        }
    }

    pub(super) fn scroll_observatory(&self) {
        let _span = perf::span("gui.observatory.scroll");
        let Some(ui) = self.ivars().session_workspace.get() else {
            return;
        };
        if ui.updating_lanes.get() || ui.restoring_layout.get() || ui.surface.isHidden() {
            return;
        }
        ui.updating_lanes.set(true);
        for lane in &ui.lanes {
            if !ui.overview.isHidden() && !lane.view.isHidden() {
                self.render_observatory_rows(lane, false);
            }
        }
        ui.updating_lanes.set(false);
        self.capture_workspace_positions();
        self.schedule_session_view_save();
    }

    fn layout_observatory_lane(
        &self,
        lane: &ObservatoryLane,
        data: &workspace_model::AgentLane<'_>,
        frame: NSRect,
        state: &agents::ViewState,
        index: usize,
    ) {
        lane.view.setFrame(frame);
        lane.background.setFrame(lane.view.bounds());
        let width = frame.size.width;
        let height = frame.size.height;
        lane.title.setFrame(rect(16.0, height - 30.0, 90.0, 22.0));
        lane.count.setFrame(rect(104.0, height - 30.0, 70.0, 22.0));
        lane.count
            .setStringValue(&NSString::from_str(&data.total.to_string()));
        lane.range
            .setFrame(rect(175.0, height - 29.0, (width - 193.0).max(0.0), 20.0));
        lane.scroll.setFrame(rect(0.0, 8.0, width, height - 44.0));
        let row_height = if state.compact { 30.0 } else { 44.0 };
        lane.row_height.set(row_height);
        lane.scroll.setVerticalLineScroll(row_height);
        lane.scroll.setHasVerticalScroller(
            data.total as f64 * row_height > lane.scroll.contentSize().height + 0.5,
        );
        lane.tasks
            .replace(data.tasks.iter().map(|task| (*task).clone()).collect());
        lane.ids
            .replace(data.tasks.iter().map(|task| task.id.clone()).collect());
        lane.pins.replace(
            state
                .desk
                .pins
                .iter()
                .filter(|pin| pin.kept)
                .map(|pin| pin.id.clone())
                .collect(),
        );
        lane.selected.replace(state.desk.selected_pin.clone());
        let viewport = lane.scroll.contentSize();
        let doc_height = (data.total as f64 * row_height).max(viewport.height);
        lane.document
            .setFrameSize(NSSize::new(viewport.width, doc_height));
        let top =
            state.desk.agent_positions[index].top(&lane.ids.borrow(), row_height, viewport.height);
        lane.scroll.contentView().scrollToPoint(NSPoint::new(
            0.0,
            (doc_height - viewport.height - top).max(0.0),
        ));
        lane.scroll
            .reflectScrolledClipView(&lane.scroll.contentView());
        self.render_observatory_rows(lane, true);
    }

    fn render_observatory_rows(&self, lane: &ObservatoryLane, force: bool) {
        let owner = self.session();
        let mtm = self.mtm();
        let tasks = lane.tasks.borrow();
        let width = lane.document.frame().size.width;
        let row_height = lane.row_height.get();
        let top = lane.top();
        let viewport = lane.scroll.contentSize().height;
        let range = workspace_model::visible_agent_rows(tasks.len(), top, viewport, row_height);
        let range_text = if tasks.is_empty() {
            "0 agents".into()
        } else {
            format!(
                "{}–{} of {}",
                (top / row_height).floor() as usize + 1,
                (((top + viewport) / row_height).ceil() as usize).min(tasks.len()),
                tasks.len()
            )
        };
        if *lane.range_text.borrow() != range_text {
            lane.range.setStringValue(&NSString::from_str(&range_text));
            lane.range_text.replace(range_text);
        }
        if !force && *lane.rendered.borrow() == range {
            return;
        }
        // Row identity survives scrolling. Layout/data changes still invalidate
        // the window so a changed task, pin, width or text size cannot go stale.
        let mut rows = lane.rows.borrow_mut();
        rows.retain(|index, view| {
            let keep = !force && range.contains(index);
            if !keep {
                view.removeFromSuperview();
            }
            keep
        });
        lane.clocks
            .borrow_mut()
            .retain(|row, _| rows.contains_key(row));
        lane.rendered.replace(range.clone());
        for row in range {
            if rows.contains_key(&row) {
                continue;
            }
            let _span = perf::span("gui.observatory.row.create");
            let task = &tasks[row];
            let task_state = agents::AgentState::from_status(&task.status);
            let attention = agents::needs_attention(task);
            let (symbol, status) = match task_state {
                agents::AgentState::Working => {
                    ("arrow.trianglehead.2.clockwise.rotate.90", "Running")
                }
                agents::AgentState::Done if attention => ("exclamationmark.circle", "Needs review"),
                agents::AgentState::Done => ("checkmark.circle", "Done"),
                agents::AgentState::Errored => ("xmark.circle", "Failed"),
                agents::AgentState::Interrupted => ("stop.circle", "Interrupted"),
                _ => ("circle", "Idle"),
            };
            let ink = if attention {
                theme::plum()
            } else if task_state.is_busy() {
                theme::accent()
            } else {
                theme::secondary_text()
            };
            let pinned = lane.pins.borrow().iter().any(|id| id == &task.id);
            let selected = lane.selected.borrow().as_ref() == Some(&task.id);
            let compact = row_height < 40.0;
            let view = NSView::initWithFrame(
                NSView::alloc(mtm),
                rect(
                    0.0,
                    lane.document.frame().size.height - (row + 1) as f64 * row_height,
                    width,
                    row_height,
                ),
            );
            let row_frame = rect(8.0, 1.0, width - 16.0, row_height - 2.0);
            let y = 0.0;
            let pill = fill_box(row_frame, &theme::signal_fill(), 6.0, mtm);
            pill.setAlphaValue(0.0);
            view.addSubview(&pill);
            let seed = fill_box(
                rect(8.0, 6.0, 2.0, row_height - 12.0),
                &theme::signal(),
                1.0,
                mtm,
            );
            seed.setAlphaValue(0.0);
            view.addSubview(&seed);
            let icon = NSImageView::initWithFrame(
                NSImageView::alloc(mtm),
                rect(17.0, if compact { 7.0 } else { 24.0 }, 16.0, 16.0),
            );
            icon.setImage(
                theme::symbol(symbol, 13.0, unsafe { NSFontWeightMedium }, None).as_deref(),
            );
            icon.setContentTintColor(Some(&ink));
            view.addSubview(&icon);
            let label = text_label(
                &task.label,
                if compact { 13.0 } else { 14.0 },
                !compact,
                mtm,
            );
            label.setFrame(if compact {
                rect(
                    40.0,
                    5.0,
                    width - if width >= 470.0 { 270.0 } else { 218.0 },
                    20.0,
                )
            } else {
                rect(40.0, 22.0, width - 124.0, 21.0)
            });
            label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            view.addSubview(&label);
            let status_label = text_label(status, 12.0, false, mtm);
            status_label.setFrame(rect(
                width - if compact { 174.0 } else { 116.0 },
                if compact { 5.0 } else { 2.0 },
                98.0,
                20.0,
            ));
            status_label.setAlignment(NSTextAlignment::Right);
            status_label.setTextColor(Some(&ink));
            view.addSubview(&status_label);
            let elapsed = if task_state.is_busy() {
                task.started_ms
                    .map(|started| self.agent_clock_ms().saturating_sub(started))
            } else {
                task.duration_ms
            };
            let meta = format!(
                "{}{}",
                task.agent,
                elapsed
                    .map(|time| format!(
                        " · {}",
                        agents::elapsed_text(std::time::Duration::from_millis(time))
                    ))
                    .unwrap_or_default()
            );
            let last_activity = owner.subagent_transcripts
                .borrow()
                .get(&task.id)
                .and_then(|entry| entry.messages.last())
                .map(workspace_model::record_label)
                .unwrap_or_else(|| "No journal yet".into());
            let caption = text_label(
                &format!("{} · {last_activity}", task.agent),
                12.0,
                false,
                mtm,
            );
            caption.setTextColor(Some(&theme::secondary_text()));
            caption.setFrame(rect(40.0, 2.0, width - 164.0, 20.0));
            caption.setHidden(compact);
            caption.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            view.addSubview(&caption);
            if compact && width >= 470.0 && (selected || pinned) {
                let time = text_label(
                    &elapsed
                        .map(|ms| agents::elapsed_text(std::time::Duration::from_millis(ms)))
                        .unwrap_or_else(|| "—".into()),
                    11.0,
                    false,
                    mtm,
                );
                time.setTextColor(Some(&theme::secondary_text()));
                time.setAlignment(NSTextAlignment::Right);
                time.setFrame(rect(width - 228.0, 5.0, 52.0, 20.0));
                view.addSubview(&time);
                if task_state.is_busy()
                    && let Some(started) = task.started_ms
                {
                    lane.clocks.borrow_mut().insert(row, (time, started));
                }
            }
            let elapsed_label = elapsed
                .map(|ms| agents::elapsed_text(std::time::Duration::from_millis(ms)))
                .unwrap_or_default();
            let pin = text_label(
                if selected {
                    "Viewing"
                } else if pinned {
                    "Kept open"
                } else {
                    &elapsed_label
                },
                11.0,
                selected,
                mtm,
            );
            pin.setFrame(if compact {
                rect(width - 73.0, 5.0, 55.0, 20.0)
            } else {
                rect(width - 78.0, 22.0, 60.0, 20.0)
            });
            pin.setAlignment(NSTextAlignment::Right);
            let pin_ink = if pinned {
                theme::signal()
            } else {
                theme::secondary_text()
            };
            pin.setTextColor(Some(&pin_ink));
            view.addSubview(&pin);
            if !selected
                && !pinned
                && task_state.is_busy()
                && let Some(started) = task.started_ms
            {
                lane.clocks.borrow_mut().insert(row, (pin, started));
            }
            if row + 1 < tasks.len() {
                let rule = fill_box(
                    rect(40.0, y, width - 58.0, 0.5),
                    &theme::hairline(),
                    0.0,
                    mtm,
                );
                view.addSubview(&rule);
            }
            let button = AgentRowButton::new(
                &task.id,
                row_frame,
                &pill,
                &seed,
                self,
                sel!(selectWorkspaceTask:),
                mtm,
            );
            button.set_selected(selected);
            let description = format!(
                "{} · {status} · {meta} · {}",
                task.label, "Preview agent; double-click to keep tab open"
            );
            button.setAccessibilityLabel(Some(&NSString::from_str(&description)));
            let tooltip = format!(
                "{description}\n{}{}",
                task.id,
                task.error
                    .as_ref()
                    .or(task.warning.as_ref())
                    .map(|note| format!("\n{note}"))
                    .unwrap_or_default()
            );
            button.setToolTip(Some(&NSString::from_str(&tooltip)));
            view.addSubview(&button);
            // Insert before the next retained row, preserving both visual and
            // accessibility order when scrolling back toward earlier agents.
            if let Some((_, next)) = rows.range(row..).next() {
                lane.document.addSubview_positioned_relativeTo(
                    &view,
                    NSWindowOrderingMode::Below,
                    Some(next),
                );
            } else {
                lane.document.addSubview(&view);
            }
            rows.insert(row, view);
        }
        if tasks.is_empty() {
            let empty = text_label("No agents in this view.", 13.0, false, mtm);
            empty.setTextColor(Some(&theme::secondary_text()));
            empty.setFrame(rect(
                17.0,
                (lane.document.frame().size.height - 34.0).max(4.0),
                width - 34.0,
                26.0,
            ));
            lane.document.addSubview(&empty);
            rows.insert(usize::MAX, empty.into_super().into_super());
        }
    }

    pub(super) fn tick_observatory_clocks(&self) {
        let Some(ui) = self.ivars().session_workspace.get() else {
            return;
        };
        if ui.surface.isHidden() || ui.overview.isHidden() {
            return;
        }
        let now = self.agent_clock_ms();
        for lane in &ui.lanes {
            if lane.view.isHidden() || lane.clock_second.replace(now / 1000) == now / 1000 {
                continue;
            }
            for (label, started) in lane.clocks.borrow().values() {
                label.setStringValue(&NSString::from_str(&agents::elapsed_text(
                    std::time::Duration::from_millis(now.saturating_sub(*started)),
                )));
            }
        }
    }

    pub(super) fn refresh_session_workspace(&self) {
        let owner = self.session();
        let _span = perf::span("gui.observatory.refresh");
        let ivars = self.ivars();
        let mtm = self.mtm();
        let Some(ui) = ivars.session_workspace.get() else {
            return;
        };
        if ui.surface.isHidden() {
            return;
        }
        if !ui.restoring_layout.replace(false) {
            self.capture_workspace_positions();
        }
        let mut state = self.agent_view();
        let key = owner.selected_session_path.borrow().clone();
        let board = owner.subagents.borrow();
        let lanes = workspace_model::observatory_lanes(&board, &state);
        ui.updating_lanes.set(true);
        let width = (ui.surface.bounds().size.width - 12.0).max(340.0);
        let height = ui.surface.bounds().size.height;
        let agent_sidebar = ivars.inspector_visible.get()
            && ivars
                .workbench_switcher
                .get()
                .is_some_and(|picker| picker.selectedSegment() == 1);
        let composition = workspace_model::observatory_composition(
            width,
            height,
            state.desk.observatory_focus,
            agent_sidebar,
            [lanes[0].total, lanes[1].total],
            state.desk.pins.len(),
        );
        let overview_visible = composition.overview_height > 0.0;
        let pins_visible = composition.evidence.height > 0.0;
        ui.arrangement.setSelectedSegment(composition.focus.index());
        ui.arrangement
            .setEnabled_forSegment(composition.split_available, 1);
        ui.overview.setHidden(!overview_visible);
        ui.evidence_header.setHidden(!pins_visible);
        ui.scroll.setHidden(!pins_visible);
        ui.overview.setFrame(rect(
            0.0,
            height - composition.overview_height,
            width,
            composition.overview_height,
        ));
        let evidence = composition.evidence;
        let focused = state
            .desk
            .focused_pin
            .as_ref()
            .and_then(|id| state.desk.pins.iter().position(|pin| &pin.id == id));
        let comparing = focused.is_none() && state.desk.observatory_focus == ObservatoryFocus::Pins;
        let kept_slots: Vec<_> = state
            .desk
            .pins
            .iter()
            .enumerate()
            .filter(|(_, pin)| pin.kept)
            .map(|(slot, _)| slot)
            .collect();
        ui.arrangement.setLabel_forSegment(
            &NSString::from_str(&format!("Compare · {}", kept_slots.len())),
            2,
        );
        ui.arrangement
            .setEnabled_forSegment(kept_slots.len() > 1, 2);
        if focused.is_some() {
            ui.arrangement.setSelectedSegment(1);
        }
        let tabbed = focused.is_none()
            && state.desk.pins.len() > 1
            && (!comparing
                || workspace_model::evidence_uses_tabs(
                    evidence.width,
                    evidence.height - 40.0,
                    kept_slots.len(),
                ));
        let header_height = 78.0;
        ui.evidence_header.setFrame(rect(
            evidence.x,
            evidence.y + (evidence.height - header_height).max(0.0),
            evidence.width,
            header_height,
        ));
        ui.scroll.setFrame(rect(
            evidence.x,
            evidence.y,
            evidence.width,
            (evidence.height - header_height - 4.0).max(0.0),
        ));
        for view in ui.ornaments.borrow_mut().drain(..) {
            view.removeFromSuperview();
        }
        let mut ornaments = ui.ornaments.borrow_mut();
        if overview_visible {
            let narrow = width < 850.0;
            let tiny = width < 510.0;
            let top = composition.overview_height;
            let active = board
                .iter()
                .filter(|task| agents::AgentState::from_status(&task.status).is_busy())
                .count();
            let attention = board
                .iter()
                .filter(|task| agents::needs_attention(task))
                .count();
            let scope = NSSegmentedControl::initWithFrame(
                NSSegmentedControl::alloc(mtm),
                rect(0.0, top - 30.0, 334.0, 30.0),
            );
            scope.setSegmentCount(3);
            scope.setSegmentStyle(NSSegmentStyle::Separated);
            scope.setSegmentDistribution(NSSegmentDistribution::FillEqually);
            scope.setFont(Some(&theme::ui_font(13.0, false)));
            for (i, title) in [
                format!("All {}", board.len()),
                format!("Running {active}"),
                format!("Attention {attention}"),
            ]
            .iter()
            .enumerate()
            {
                scope.setLabel_forSegment(&NSString::from_str(title), i as isize);
            }
            scope.setSelectedSegment(match state.scope {
                agents::WorkScope::All => 0,
                agents::WorkScope::Active => 1,
                agents::WorkScope::Attention => 2,
            });
            scope.setAccessibilityLabel(Some(ns_string!("Filter Observatory agents")));
            unsafe {
                scope.setTarget(Some::<&AnyObject>(self));
                scope.setAction(Some(sel!(observatoryScope:)));
            }
            ui.overview.addSubview(&scope);
            ornaments.push(scope.into_super().into_super());
            ui.search.setFrame(if narrow {
                rect(0.0, top - if tiny { 100.0 } else { 66.0 }, width, 30.0)
            } else {
                rect(350.0, top - 30.0, width - 510.0, 30.0)
            });
            let density = action(
                if state.compact {
                    "Comfortable rows"
                } else {
                    "Compact rows"
                },
                rect(
                    if tiny { 0.0 } else { width - 148.0 },
                    top - if tiny { 64.0 } else { 30.0 },
                    148.0,
                    28.0,
                ),
                21,
                sel!(workspaceScope:),
                self,
            );
            ui.overview.addSubview(&density);
            ornaments.push(density.into_super().into_super());
            if ui.search.stringValue().to_string() != state.query {
                ui.search.setStringValue(&NSString::from_str(&state.query));
            }
            for (i, lane) in lanes.iter().enumerate() {
                let frame = composition.lanes[i];
                ui.lanes[i].view.setHidden(frame.is_none());
                if let Some(frame) = frame {
                    let title = if lanes.iter().all(|lane| lane.total == 0) {
                        "Agents"
                    } else if i == 0 {
                        "Running"
                    } else {
                        "Finished"
                    };
                    ui.lanes[i].title.setStringValue(&NSString::from_str(title));
                    ui.lanes[i]
                        .scroll
                        .setAccessibilityLabel(Some(&NSString::from_str(&if title == "Agents" {
                            "Agents".into()
                        } else {
                            format!("{title} agents")
                        })));
                    self.layout_observatory_lane(
                        &ui.lanes[i],
                        lane,
                        rect(frame.x, frame.y, frame.width, frame.height),
                        &state,
                        i,
                    );
                }
            }
        }
        if !pins_visible {
            ui.session.replace(Some(key));
            ui.updating_lanes.set(false);
            return;
        }
        let selected = state
            .desk
            .selected_pin
            .as_ref()
            .and_then(|id| state.desk.pins.iter().position(|pin| &pin.id == id))
            .unwrap_or(0);
        let selected = if comparing && !kept_slots.contains(&selected) {
            kept_slots.first().copied().unwrap_or(0)
        } else {
            selected
        };
        let visible_pins: Vec<_> = match focused {
            Some(slot) => vec![slot],
            None if !comparing || tabbed => {
                if state.desk.pins.is_empty() {
                    vec![]
                } else {
                    vec![selected]
                }
            }
            None => kept_slots.clone(),
        };
        let viewport = ui.scroll.contentSize();
        let (total, _) =
            workspace_model::evidence_frames(viewport.width, viewport.height, visible_pins.len());
        ui.scroll
            .setHasVerticalScroller(total > viewport.height + 0.5);
        let viewport = ui.scroll.contentSize();
        let (total, frames) =
            workspace_model::evidence_frames(viewport.width, viewport.height, visible_pins.len());
        ui.document.setFrameSize(NSSize::new(viewport.width, total));
        let width = evidence.width;
        let label = text_label(
            &if comparing {
                format!("Compare {} agents", kept_slots.len())
            } else {
                if state.desk.pins.get(selected).is_some_and(|pin| !pin.kept) {
                    "Preview · double-click tab to keep open".into()
                } else {
                    "Agent transcript".into()
                }
            },
            13.0,
            true,
            mtm,
        );
        label.setFrame(rect(
            0.0,
            header_height - 29.0,
            (width - 140.0).max(100.0),
            22.0,
        ));
        ui.evidence_header.addSubview(&label);
        ornaments.push(label.into_super().into_super());
        let ask = action(
            "Discuss with lead",
            rect(width - 130.0, header_height - 32.0, 130.0, 28.0),
            0,
            sel!(composeFromEvidence:),
            self,
        );
        ask.setEnabled(visible_pins.iter().any(|slot| {
            owner.subagent_transcripts
                .borrow()
                .get(&state.desk.pins[*slot].id)
                .is_some_and(|entry| !entry.messages.is_empty())
        }));
        ask.setToolTip(Some(ns_string!(
            "Add source-labeled excerpts to an editable prompt. Nothing is sent."
        )));
        ui.evidence_header.addSubview(&ask);
        ornaments.push(ask.into_super().into_super());
        {
            let tab_slots: Vec<_> = if comparing {
                kept_slots.clone()
            } else {
                (0..state.desk.pins.len()).collect()
            };
            // A real tab strip: one italic preview, ordinary open tabs, and close buttons.
            // Overflow scrolls horizontally instead of shrinking names into unreadable slivers.
            let strip =
                NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 1.0, width, 40.0));
            strip.setHasHorizontalScroller(true);
            strip.setAutohidesScrollers(true);
            strip.setDrawsBackground(false);
            strip.setBorderType(NSBorderType::NoBorder);
            strip.setAccessibilityLabel(Some(ns_string!("Agent tabs")));
            let tabs_view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, width, 36.0));
            let mut x = 0.0;
            let mut selected_frame = NSRect::ZERO;
            for slot in tab_slots {
                let pin = &state.desk.pins[slot];
                let title = board
                    .iter()
                    .find(|task| task.id == pin.id)
                    .map(|task| task.label.as_str())
                    .unwrap_or(&pin.snapshot.title);
                let tab_width = (width / state.desk.pins.len().max(1) as f64).clamp(145.0, 240.0);
                let active = selected == slot;
                let background = fill_box(
                    rect(x, 0.0, tab_width - 4.0, 35.0),
                    (if active {
                        theme::surface_inset()
                    } else {
                        theme::canvas()
                    })
                    .as_ref(),
                    5.0,
                    mtm,
                );
                tabs_view.addSubview(&background);
                let tab = action(
                    title,
                    rect(x + 6.0, 3.0, tab_width - 38.0, 28.0),
                    (slot * 10 + 6) as isize,
                    sel!(workspaceEvidenceAction:),
                    self,
                );
                tab.setBordered(false);
                tab.setAlignment(NSTextAlignment::Left);
                let font = theme::ui_font(12.0, active);
                let font = if pin.kept {
                    font
                } else {
                    let descriptor = font.fontDescriptor().fontDescriptorWithSymbolicTraits(
                        font.fontDescriptor().symbolicTraits()
                            | objc2_app_kit::NSFontDescriptorSymbolicTraits::TraitItalic,
                    );
                    NSFont::fontWithDescriptor_size(&descriptor, 12.0).unwrap_or(font)
                };
                tab.setFont(Some(&font));
                if let Some(cell) = tab.cell() {
                    cell.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
                }
                let tip = format!(
                    "{title}\n{}",
                    if pin.kept {
                        "Kept open · closes only when you close it"
                    } else {
                        "Preview tab · double-click to keep open, or select and press ⌘Return"
                    }
                );
                tab.setToolTip(Some(&NSString::from_str(&tip)));
                tab.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                    "{}: {title}",
                    if pin.kept { "Agent tab" } else { "Preview tab" }
                ))));
                let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Agent tab"));
                let open = unsafe {
                    menu.addItemWithTitle_action_keyEquivalent(
                        ns_string!("Keep Tab Open"),
                        Some(sel!(openAgentTab:)),
                        ns_string!(""),
                    )
                };
                unsafe {
                    open.setTarget(Some::<&AnyObject>(self));
                }
                open.setTag(slot as isize);
                open.setEnabled(!pin.kept);
                // The menu targets the application delegate, which outlives all tabs.
                unsafe {
                    tab.setMenu(Some(&menu));
                }
                tabs_view.addSubview(&tab);
                let close = icon_button(
                    "xmark",
                    &format!("Close tab: {title}"),
                    rect(x + tab_width - 29.0, 9.0, 18.0, 18.0),
                    self,
                    sel!(workspaceEvidenceAction:),
                    mtm,
                );
                close.setTag((slot * 10 + 2) as isize);
                tabs_view.addSubview(&close);
                if active {
                    selected_frame = rect(x, 0.0, tab_width, 36.0);
                    let rule = fill_box(
                        rect(x + 7.0, 0.0, tab_width - 18.0, 2.0),
                        &theme::signal(),
                        0.0,
                        mtm,
                    );
                    tabs_view.addSubview(&rule);
                }
                x += tab_width;
            }
            tabs_view.setFrameSize(NSSize::new(x.max(width), 36.0));
            strip.setDocumentView(Some(&tabs_view));
            ui.evidence_header.addSubview(&strip);
            tabs_view.scrollRectToVisible(selected_frame);
            ornaments.push(strip.into_super());
        }
        if visible_pins.is_empty() {
            let empty = text_label(
                "Click an agent to preview its transcript.\nDouble-click a tab to keep it open for later reading or comparison.",
                15.0,
                false,
                mtm,
            );
            empty.setMaximumNumberOfLines(0);
            empty.setFrame(rect(16.0, total - 100.0, viewport.width - 32.0, 76.0));
            empty.setTextColor(Some(&theme::secondary_text()));
            ui.document.addSubview(&empty);
            ornaments.push(empty.into_super().into_super());
        }
        drop(ornaments);
        let cache = owner.subagent_transcripts.borrow();
        let preferences = ivars.preferences.borrow();
        let typography = native_markdown::Typography::new(
            preferences.chat_style,
            preferences.chat_text_size.scale(),
        )
        .with_sizes(preferences.chat_text_sizes);
        let same_session = ui.session.borrow().as_ref() == Some(&key);
        for (slot, pane) in ui.panes.iter().enumerate() {
            let Some(pin) = state.desk.pins.get_mut(slot) else {
                pane.view.setHidden(true);
                continue;
            };
            let info = board.iter().find(|task| task.id == pin.id);
            let page = cache
                .get(&pin.id)
                .and_then(|entry| workspace_model::evidence_page(&entry.messages, pin));
            if let Some(info) = info {
                pin.snapshot.title = info.label.clone();
                pin.snapshot.notice = info
                    .error
                    .as_ref()
                    .or(info.warning.as_ref())
                    .map(|note| note.chars().take(800).collect())
                    .unwrap_or_default();
                pin.snapshot.metadata = format!(
                    "AGENT {:02} · {} · {}",
                    info.index + 1,
                    info.agent,
                    agents::AgentState::from_status(&info.status).label()
                );
            }
            let loading = owner.workspace_reads
                .borrow()
                .contains(&(owner.evidence_generation.get(), pin.id.clone()));
            if let Some(page) = &page {
                pin.snapshot.source = page.source.clone();
                pin.snapshot.body = page.text.clone();
            } else {
                pin.snapshot = EvidenceSnapshot {
                    title: info
                        .map(|task| task.label.clone())
                        .unwrap_or_else(|| pin.id.clone()),
                    metadata: pin.snapshot.metadata.clone(),
                    notice: pin.snapshot.notice.clone(),
                    source: "No transcript yet".into(),
                    body: if loading {
                        "Reading task journal…".into()
                    } else if let Some(error) = owner.workspace_read_failures.borrow().get(&pin.id)
                    {
                        format!("Could not read this journal: {error}\n\nChoose Latest to retry.")
                    } else {
                        "No transcript is available for this agent yet. Its lifecycle state is shown above.".into()
                    },
                };
            }
            let Some(position) = visible_pins.iter().position(|visible| *visible == slot) else {
                pane.view.setHidden(true);
                continue;
            };
            let frame = frames[position];
            pane.view.setHidden(false);
            pane.view
                .setFrame(rect(frame.x, frame.y, frame.width, frame.height));
            pane.title
                .setStringValue(&NSString::from_str(&pin.snapshot.title));
            pane.title.setToolTip(Some(&pane.title.stringValue()));
            pane.metadata
                .setStringValue(&NSString::from_str(&if loading {
                    "Reading journal…".into()
                } else {
                    pin.snapshot.metadata.clone()
                }));
            pane.metadata.setToolTip(Some(&NSString::from_str(&format!(
                "{} · {}",
                pin.snapshot.metadata, pin.id
            ))));
            pane.metadata.setTextColor(Some(
                (if info.is_some_and(agents::needs_attention) {
                    theme::plum()
                } else {
                    theme::signal()
                })
                .as_ref(),
            ));
            pane.source
                .setStringValue(&NSString::from_str(&pin.snapshot.source));
            pane.source.setToolTip(Some(&pane.source.stringValue()));
            pane.text
                .setAccessibilityLabel(Some(&NSString::from_str(&format!(
                    "Agent transcript: {}",
                    pin.snapshot.title
                ))));
            pane.older
                .setEnabled(page.as_ref().is_some_and(|page| page.older));
            pane.newer
                .setEnabled(page.as_ref().is_some_and(|page| page.newer));
            pane.latest.setContentTintColor(Some(
                (if pin.message.is_none() {
                    theme::signal()
                } else {
                    theme::secondary_text()
                })
                .as_ref(),
            ));
            let read_error = owner.workspace_read_failures.borrow().get(&pin.id).cloned();
            let notice = match read_error {
                Some(error) if page.is_some() => format!(
                    "Could not refresh this journal: {error}. Showing the last loaded transcript. Choose Latest to retry.{}",
                    if pin.snapshot.notice.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", pin.snapshot.notice)
                    },
                ),
                _ => pin.snapshot.notice.clone(),
            };
            pane.layout(frame.width, frame.height, &notice, pin.notice_expanded);
            let text_width = pane.scroll.contentSize().width;
            let fingerprint = format!(
                "{:?}:{}:{}:{text_width}:{:?}:{:?}:{:?}:{:?}",
                key,
                pin.id,
                pin.snapshot.body,
                preferences.chat_style,
                preferences.chat_text_size,
                preferences.chat_text_sizes,
                pin.expanded_tools
            );
            if *pane.fingerprint.borrow() != fingerprint {
                let rendered = NSMutableAttributedString::initWithString(
                    NSMutableAttributedString::alloc(),
                    ns_string!(""),
                );
                let render = |author: String, body: String, kind, status, detail_id| {
                    native_markdown::render_message(
                        &chat::TranscriptMessage {
                            author,
                            body,
                            kind,
                            status,
                            detail_id,
                        },
                        text_width - 38.0,
                        &owner.current_project.borrow(),
                        typography,
                        false,
                    )
                };
                if let Some(page) = &page {
                    for record in &page.records {
                        let message = record.message;
                        let author = format!(
                            "{} · {}{}",
                            record.index + 1,
                            workspace_model::record_label(message),
                            if record.continued {
                                " · continued"
                            } else {
                                ""
                            }
                        );
                        if message.speaker == session::Speaker::Tool {
                            let expanded = pin.expanded_tools.contains(&record.index);
                            let summary = format!(
                                "{} · {}{}",
                                message.tool.as_deref().unwrap_or("Tool"),
                                message.text.chars().take(180).collect::<String>(),
                                if expanded { " · Hide output" } else { "" }
                            );
                            rendered.appendAttributedString(&render(
                                String::new(),
                                summary,
                                chat::TranscriptMessageKind::Tool,
                                message.failed.then_some(chat::ToolStatus::Failed),
                                Some(record.index),
                            ));
                            if expanded {
                                rendered.appendAttributedString(&render(
                                    author,
                                    record.text.into(),
                                    chat::TranscriptMessageKind::Assistant,
                                    None,
                                    None,
                                ));
                            }
                        } else {
                            let kind = match message.speaker {
                                session::Speaker::User => chat::TranscriptMessageKind::User,
                                session::Speaker::Thinking => chat::TranscriptMessageKind::Thinking,
                                session::Speaker::Event => chat::TranscriptMessageKind::Event,
                                _ => chat::TranscriptMessageKind::Assistant,
                            };
                            rendered.appendAttributedString(&render(
                                author,
                                record.text.into(),
                                kind,
                                None,
                                None,
                            ));
                        }
                    }
                } else {
                    rendered.appendAttributedString(&render(
                        String::new(),
                        pin.snapshot.body.clone(),
                        chat::TranscriptMessageKind::Assistant,
                        None,
                        None,
                    ));
                }
                unsafe {
                    if let (Some(storage), Some(container), Some(layout)) = (
                        pane.text.textStorage(),
                        pane.text.textContainer(),
                        pane.text.layoutManager(),
                    ) {
                        container.setWidthTracksTextView(false);
                        container.setContainerSize(NSSize::new(text_width - 36.0, 1.0e7));
                        storage.setAttributedString(&rendered);
                        layout.ensureLayoutForTextContainer(&container);
                        pane.text.setFrameSize(NSSize::new(
                            text_width,
                            (layout.usedRectForTextContainer(&container).size.height + 16.0)
                                .max(pane.scroll.contentSize().height),
                        ));
                    }
                }
                pane.fingerprint.replace(fingerprint);
            }
            unsafe {
                if let (Some(container), Some(layout)) =
                    (pane.text.textContainer(), pane.text.layoutManager())
                {
                    pane.text.setFrameSize(NSSize::new(
                        text_width,
                        (layout.usedRectForTextContainer(&container).size.height + 16.0)
                            .max(pane.scroll.contentSize().height),
                    ));
                }
            }
            if let Some(page) = &page {
                pane.page_start.set((page.index, page.start));
            }
            let anchor = (pin.id.clone(), pin.message, pin.offset);
            // Keep each surface's independent reading position, including when slots move.
            if !same_session
                || pane.anchor.borrow().as_ref() != Some(&anchor)
                || pane.scroll.contentView().bounds().origin.y != pin.scroll_y
            {
                let y = pin.scroll_y.min(
                    (pane.text.frame().size.height - pane.scroll.contentSize().height).max(0.0),
                );
                pane.scroll
                    .contentView()
                    .scrollToPoint(NSPoint::new(0.0, y));
                pane.scroll
                    .reflectScrolledClipView(&pane.scroll.contentView());
                let len = pane.text.string().length();
                if pin
                    .selection
                    .0
                    .checked_add(pin.selection.1)
                    .is_some_and(|end| end <= len)
                {
                    pane.text
                        .setSelectedRange(NSRange::new(pin.selection.0, pin.selection.1));
                }
            }
            // A loading/error placeholder has no transcript coordinate system.
            // Never let its zero scroll offset replace a saved reading anchor.
            pane.anchor.replace(page.as_ref().map(|_| anchor));
        }
        drop(preferences);
        drop(cache);
        drop(board);
        let y = (total - ui.scroll.contentSize().height - state.desk.evidence_top).max(0.0);
        ui.scroll.contentView().scrollToPoint(NSPoint::new(0.0, y));
        ui.scroll.reflectScrolledClipView(&ui.scroll.contentView());
        self.edit_desk(|desk| {
            desk.pins = state.desk.pins;
        });
        ui.session.replace(Some(key));
        ui.updating_lanes.set(false);
    }
}
