//! Native controls for calibrating chat typography against live examples.
use super::*;
use typography_examples::TextExample;

const TEXT_SIZE_NOTE: &str = "Transcript defaults follow the style and text scale in Appearance. Examples use actual sizes. Use the arrows, or type a size and press Return.";

#[derive(Debug)]
struct TextSizeRow {
    role: TextRole,
    view: Retained<NSView>,
    label: Retained<NSTextField>,
    field: Retained<NSTextField>,
    unit: Retained<NSTextField>,
    edited: Cell<bool>,
    stepper: Retained<NSStepper>,
    detail: Retained<NSTextField>,
    reset: Retained<NSButton>,
    example: TextExample,
    example_surface: Retained<NSBox>,
}

#[derive(Debug)]
pub(super) struct TextSizeSettingsUi {
    rows: Vec<TextSizeRow>,
    note: Retained<NSTextField>,
    reset: Retained<NSButton>,
    scroll: Retained<NSScrollView>,
    content: Retained<NSView>,
    groups: Vec<(usize, Retained<NSTextField>)>,
    laying_out: Cell<bool>,
    last_typography: Cell<Option<native_markdown::Typography>>,
    last_width: Cell<f64>,
}

impl AppDelegate {
    pub(super) fn build_chat_typography_page(
        &self,
        page: &NSView,
        mtm: MainThreadMarker,
    ) -> TextSizeSettingsUi {
        settings_page_header(
            page,
            "Chat text",
            "Tune each context with a live example from the conversation.",
            mtm,
        );
        let note = wrapping_label(TEXT_SIZE_NOTE, 12.0, mtm);
        note.setTextColor(Some(&theme::secondary_text()));
        note.setFrame(rect(40.0, SETTINGS_HEIGHT - 148.0, 600.0, 48.0));
        note.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        page.addSubview(&note);

        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(40.0, 76.0, 600.0, SETTINGS_HEIGHT - 234.0),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(false);
        scroll.setHasHorizontalScroller(false);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 600.0, 1.0));
        let mut groups = Vec::new();
        let mut previous_group = "";
        let mut rows = Vec::new();
        for (index, role) in TextRole::ALL.into_iter().enumerate() {
            if previous_group != role.group() {
                let group = text_label(role.group(), 12.0, true, mtm);
                group.setTextColor(Some(&theme::secondary_text()));
                content.addSubview(&group);
                groups.push((index, group));
                previous_group = role.group();
            }
            let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 600.0, 160.0));
            let label = wrapping_label(role.label(), 14.0, mtm);
            let detail = text_label("", 12.0, false, mtm);
            detail.setTextColor(Some(&theme::secondary_text()));
            let field =
                NSTextField::initWithFrame(NSTextField::alloc(mtm), rect(0.0, 0.0, 62.0, 26.0));
            field.setFont(Some(&NSFont::systemFontOfSize(14.0)));
            field.setAlignment(NSTextAlignment::Right);
            field.setTag(TEXT_SIZE_FIELD_TAG + role as isize);
            field.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                "{} size in points",
                role.label()
            ))));
            let (min, max) = role.bounds();
            field.setToolTip(Some(&NSString::from_str(&format!(
                "{min}–{max} pt, in half-point increments. Return applies the size."
            ))));
            let unit = text_label("pt", 12.0, false, mtm);
            let stepper =
                NSStepper::initWithFrame(NSStepper::alloc(mtm), rect(0.0, 0.0, 20.0, 28.0));
            stepper.setMinValue(min);
            stepper.setMaxValue(max);
            stepper.setIncrement(0.5);
            stepper.setValueWraps(false);
            stepper.setAutorepeat(true);
            stepper.setTag(role as isize);
            stepper.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                "Adjust {} size",
                role.label()
            ))));
            let reset = settings_button(
                "Default",
                rect(0.0, 0.0, 82.0, 28.0),
                self,
                sel!(resetChatTextSize:),
                mtm,
            );
            reset.setTag(role as isize);
            reset.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                "Restore default size for {}",
                role.label()
            ))));
            // SAFETY: AppDelegate implements these selectors and outlives Settings.
            unsafe {
                field.setTarget(Some::<&AnyObject>(self));
                field.setAction(Some(sel!(editChatTextSize:)));
                field.setDelegate(Some(ProtocolObject::from_ref(self)));
                stepper.setTarget(Some::<&AnyObject>(self));
                stepper.setAction(Some(sel!(stepChatTextSize:)));
            }
            for child in [&*label, &*detail, &*field, &*unit] {
                view.addSubview(child);
            }
            view.addSubview(&stepper);
            view.addSubview(&reset);
            let example_surface =
                fill_box(rect(228.0, 12.0, 360.0, 136.0), &theme::canvas(), 12.0, mtm);
            example_surface.setBorderWidth(1.0);
            example_surface.setBorderColor(&theme::hairline());
            view.addSubview(&example_surface);
            let example = TextExample::new(role, mtm);
            example_surface.setHidden(matches!(&example, TextExample::Composer(_)));
            view.addSubview(example.view());
            content.addSubview(&view);
            rows.push(TextSizeRow {
                role,
                view,
                label,
                field,
                unit,
                edited: Cell::new(false),
                stepper,
                detail,
                reset,
                example,
                example_surface,
            });
        }
        scroll.setDocumentView(Some(&content));
        let clip = scroll.contentView();
        clip.setPostsFrameChangedNotifications(true);
        // SAFETY: the selector is implemented by AppDelegate, which owns Settings.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(textSizeExamplesFrameDidChange:),
                Some(NSViewFrameDidChangeNotification),
                Some(&clip),
            );
        }
        page.addSubview(&scroll);
        let reset = settings_button(
            "Reset all to defaults",
            rect(40.0, 28.0, 184.0, 32.0),
            self,
            sel!(resetAllChatTextSizes:),
            mtm,
        );
        page.addSubview(&reset);
        TextSizeSettingsUi {
            rows,
            note,
            reset,
            scroll,
            content,
            groups,
            laying_out: Cell::new(false),
            last_typography: Cell::new(None),
            last_width: Cell::new(0.0),
        }
    }

    pub(super) fn invalidate_text_size_examples(&self) {
        if let Some(settings) = self.ivars().settings_ui.get() {
            settings.text_sizes.last_typography.set(None);
            self.layout_text_size_examples();
        }
    }

    pub(super) fn layout_text_size_examples(&self) {
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let ui = &settings.text_sizes;
        if ui.laying_out.replace(true) {
            return;
        }
        let typography = {
            let prefs = self.ivars().preferences.borrow();
            native_markdown::Typography::new(prefs.chat_style, prefs.chat_text_size.scale())
                .with_sizes(prefs.chat_text_sizes)
        };
        let width = ui.scroll.contentSize().width;
        let rerender = ui.last_typography.get() != Some(typography);
        if width < 260.0 || (!rerender && ui.last_width.get() == width) {
            ui.laying_out.set(false);
            return;
        }
        let clip = ui.scroll.contentView();
        let visible_top = clip.bounds().origin.y + clip.bounds().size.height;
        // Keep the row under the user's eye stationary when other examples
        // grow. This also prevents a held stepper from moving under the pointer.
        let anchor = (ui.last_width.get() > 0.0)
            .then(|| {
                ui.rows
                    .iter()
                    .enumerate()
                    .find(|(_, row)| row.view.frame().origin.y < visible_top)
                    .map(|(index, row)| {
                        (
                            index,
                            row.view.frame().origin.y + row.view.frame().size.height - visible_top,
                        )
                    })
            })
            .flatten();
        let example_width = width - 240.0;
        let heights: Vec<f64> = ui
            .rows
            .iter()
            .map(|row| (row.example.fit(typography, example_width, rerender) + 24.0).max(136.0))
            .collect();
        let height = (heights.iter().sum::<f64>() + ui.groups.len() as f64 * 36.0 + 12.0)
            .max(ui.scroll.contentSize().height);
        ui.content.setFrameSize(NSSize::new(width, height));
        let mut top = height;
        for (index, (row, height)) in ui.rows.iter().zip(heights).enumerate() {
            if let Some((_, group)) = ui.groups.iter().find(|(start, _)| *start == index) {
                top -= 36.0;
                group.setFrame(rect(12.0, top + 7.0, width - 24.0, 20.0));
            }
            top -= height;
            row.view.setFrame(rect(0.0, top, width, height));
            row.label.setFrame(rect(12.0, height - 52.0, 204.0, 40.0));
            row.detail.setFrame(rect(12.0, height - 73.0, 204.0, 18.0));
            row.field.setFrame(rect(12.0, height - 109.0, 62.0, 26.0));
            row.unit.setFrame(rect(80.0, height - 105.0, 22.0, 18.0));
            row.stepper
                .setFrame(rect(106.0, height - 111.0, 20.0, 28.0));
            row.reset.setFrame(rect(134.0, height - 111.0, 82.0, 28.0));
            let example_height = row.example.view().frame().size.height;
            row.example
                .view()
                .setFrameOrigin(NSPoint::new(228.0, height - 12.0 - example_height));
            row.example_surface.setFrame(row.example.view().frame());
        }
        let target_top = anchor.map_or(height, |(index, offset)| {
            let frame = ui.rows[index].view.frame();
            frame.origin.y + frame.size.height - offset
        });
        let viewport_height = clip.bounds().size.height;
        clip.scrollToPoint(NSPoint::new(
            0.0,
            (target_top - viewport_height).clamp(0.0, (height - viewport_height).max(0.0)),
        ));
        ui.scroll.reflectScrolledClipView(&clip);
        ui.last_typography.set(Some(typography));
        ui.last_width.set(width);
        ui.laying_out.set(false);
    }

    pub(super) fn sync_text_size_settings(&self) {
        let Some(settings) = self.ivars().settings_ui.get() else {
            return;
        };
        let prefs = self.ivars().preferences.borrow();
        for row in &settings.text_sizes.rows {
            if row.edited.get() {
                continue;
            }
            let default = row
                .role
                .default_size(prefs.chat_style, prefs.chat_text_size.scale());
            let custom = prefs.chat_text_sizes.get(row.role);
            let size = custom.unwrap_or(default);
            row.field
                .setStringValue(&NSString::from_str(&format!("{size:.1}")));
            row.stepper.setDoubleValue(size);
            row.detail
                .setStringValue(&NSString::from_str(&if custom.is_some() {
                    format!("Custom · default {default:.1} pt")
                } else {
                    format!("Default · {default:.1} pt")
                }));
            row.reset.setEnabled(custom.is_some());
        }
        settings.text_sizes.reset.setEnabled(
            prefs.chat_text_sizes != TextSizes::DEFAULTS
                || settings.text_sizes.rows.iter().any(|row| row.edited.get()),
        );
        drop(prefs);
        self.layout_text_size_examples();
    }
    pub(super) fn mark_text_size_edited(&self, field: &NSTextField) {
        if let Some(settings) = self.ivars().settings_ui.get() {
            if let Some(row) = settings
                .text_sizes
                .rows
                .iter()
                .find(|row| std::ptr::eq(&*row.field, field))
            {
                row.edited.set(true);
                row.reset.setEnabled(true);
                settings.text_sizes.reset.setEnabled(true);
                // Keep the paired stepper based on a valid draft, so clicking
                // an arrow after typing 20 yields 20.5, not the old size + 0.5.
                if let Ok(size) = row.field.stringValue().to_string().trim().parse::<f64>() {
                    let mut draft = TextSizes::DEFAULTS;
                    if draft.set(row.role, Some(size)) {
                        row.stepper.setDoubleValue(draft.get(row.role).unwrap());
                    }
                }
            }
        }
    }

    pub(super) fn cancel_text_size_edit(&self, role: Option<TextRole>) {
        if let Some(settings) = self.ivars().settings_ui.get() {
            for row in &settings.text_sizes.rows {
                if role.is_none_or(|role| role == row.role) {
                    row.edited.set(false);
                    row.field.abortEditing();
                }
            }
        }
    }

    pub(super) fn commit_text_size(&self, field: &NSTextField) {
        let edited = self
            .ivars()
            .settings_ui
            .get()
            .and_then(|settings| {
                settings
                    .text_sizes
                    .rows
                    .iter()
                    .find(|row| std::ptr::eq(&*row.field, field))
            })
            .is_some_and(|row| row.edited.replace(false));
        // AppKit can deliver both an action and end-editing for Return.
        // A focus change alone must not turn a default size into a custom one.
        if !edited {
            return;
        }

        let Some(role) = field
            .tag()
            .checked_sub(TEXT_SIZE_FIELD_TAG)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| TextRole::ALL.get(index))
            .copied()
        else {
            return;
        };
        let value = field.stringValue().to_string();
        let valid = value
            .trim()
            .parse::<f64>()
            .ok()
            .is_some_and(|size| self.change_text_size(role, Some(size)));
        if !valid {
            if let Some(settings) = self.ivars().settings_ui.get() {
                let (min, max) = role.bounds();
                settings.text_sizes.note.setStringValue(&NSString::from_str(&format!("Enter a number from {min} to {max} pt for {}. The previous size is unchanged.", role.label().to_lowercase())));
            }
        }
        self.sync_text_size_settings();
    }

    pub(super) fn change_text_size(&self, role: TextRole, value: Option<f64>) -> bool {
        {
            let mut prefs = self.ivars().preferences.borrow_mut();
            let previous = prefs.chat_text_sizes;
            if !prefs.chat_text_sizes.set(role, value) {
                return false;
            }
            if previous == prefs.chat_text_sizes {
                return true;
            }
        }
        self.apply_chat_typography();
        self.store_preferences();
        true
    }

    pub(super) fn apply_chat_typography(&self) {
        self.refit_composer_chips();
        self.layout_header();
        let status = self.session().chat_status_text.borrow().clone();
        self.set_chat_status(&status);
        self.refresh_chat_transcript();
        self.refresh_agents_panel();
        self.sync_text_size_settings();
        if let Some(settings) = self.ivars().settings_ui.get() {
            settings
                .text_sizes
                .note
                .setStringValue(&NSString::from_str(TEXT_SIZE_NOTE));
        }
    }
}
