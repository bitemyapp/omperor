//! Compact native task tree and per-session lenses. View changes use the
//! runtime's in-memory snapshot; transcripts keep their existing worker path.
use super::*;
use objc2_app_kit::NSAttributedStringNSStringDrawing;

pub(super) const SEARCH_TAG: isize = -8802;

impl AppDelegate {
    /// Visual fixtures hold a stable clock; production uses the actual event time.
    pub(super) fn agent_clock_ms(&self) -> u64 {
        #[cfg(feature = "design-preview")]
        {
            self.ivars().preview_clock_ms.get()
        }
        #[cfg(not(feature = "design-preview"))]
        {
            wall_clock_ms()
        }
    }

    pub(super) fn agent_view(&self) -> agents::ViewState {
        let owner = self.session();
        let view = self
            .ivars()
            .agent_view_states
            .borrow()
            .get(&*owner.selected_session_path.borrow())
            .cloned()
            .unwrap_or_default();
        view
    }

    pub(super) fn edit_agent_view(&self, edit: impl FnOnce(&mut agents::ViewState)) {
        let owner = self.session();
        self.schedule_session_view_save();
        self.capture_workspace_positions();
        let key = owner.selected_session_path.borrow().clone();
        let layout_changed = {
            let mut states = self.ivars().agent_view_states.borrow_mut();
            let view = states.entry(key).or_default();
            let before = (view.query.clone(), view.scope);
            edit(view);
            let layout_changed = before != (view.query.clone(), view.scope);
            if layout_changed {
                view.desk.agent_positions = Default::default();
            }
            layout_changed
        };
        if layout_changed {
            self.preserve_workspace_layout();
        }
        self.ivars().agents_reset_scroll.set(true);
        self.refresh_agents_panel();
    }

    pub(super) fn filter_agent_work(&self) {
        if let Some(search) = self.ivars().agents_search.get() {
            let query = search.stringValue().to_string();
            self.edit_agent_view(|view| {
                view.query = query;
                view.page = 0;
            });
        }
    }

    pub(super) fn reveal_agent_search(&self) {
        self.edit_agent_view(|view| view.search_open = true);
        if let (Some(window), Some(search)) =
            (self.ivars().window.get(), self.ivars().agents_search.get())
        {
            window.makeFirstResponder(Some(&**search));
        }
    }

    pub(super) fn clear_agent_search(&self) {
        self.edit_agent_view(|view| {
            view.query.clear();
            view.search_open = false;
            view.page = 0;
        });
        if let Some(window) = self.ivars().window.get() {
            window.makeFirstResponder(
                self.ivars()
                    .agents_search_button
                    .get()
                    .map(|button| &**button as &NSResponder),
            );
        }
    }

    pub(super) fn activity_width(&self, total: f64) -> f64 {
        layout::activity_width(
            total,
            SIDEBAR_WIDTH,
            INSPECTOR_WIDTH,
            self.ivars().workbench_expanded.get(),
        )
    }

    pub(super) fn expand_agent_workspace(&self) {
        let ivars = self.ivars();
        ivars
            .workbench_expanded
            .set(!ivars.workbench_expanded.get());
        if let Some(split) = ivars.split_view.get() {
            split.setPosition_ofDividerAtIndex(
                split.bounds().size.width - self.activity_width(split.bounds().size.width) - 1.0,
                1,
            );
        }
        if let Some(button) = ivars.workbench_expand.get() {
            let expanded = ivars.workbench_expanded.get();
            let label = if expanded {
                "Restore activity width"
            } else {
                "Expand agent panel"
            };
            button.setToolTip(Some(&NSString::from_str(label)));
            button.setAccessibilityLabel(Some(&NSString::from_str(label)));
            let tint = if expanded {
                theme::accent()
            } else {
                theme::secondary_text()
            };
            button.setContentTintColor(Some(&tint));
        }
        self.refresh_agents_panel();
        self.layout_header();
        self.sync_transcript_measure();
        self.refit_composer_chips();
    }

    pub(super) fn refresh_workbench_location(&self) {
        let owner = self.session();
        if let Some(location) = self.ivars().workbench_location.get() {
            let path = owner.current_project.borrow().display().to_string();
            location.setStringValue(&NSString::from_str(&path));
            location.setToolTip(Some(&NSString::from_str(&path)));
        }
    }

    pub(super) fn refresh_agents_panel(&self) {
        let owner = self.session();
        let _span = crate::perf::span("gui.refresh_agents_panel");
        self.refresh_session_workspace();
        let mtm = self.mtm();
        let ivars = self.ivars();
        let (Some(scroll), Some(container), Some(panel)) = (
            ivars.agents_scroll.get(),
            ivars.agents_view.get(),
            ivars.agents_panel.get(),
        ) else {
            return;
        };
        if panel.isHidden() {
            return;
        }
        self.refresh_workbench_location();
        let key = owner.selected_session_path.borrow().clone();
        let changed_session = ivars.agents_rendered_session.borrow().as_ref() != Some(&key);
        if changed_session {
            ivars.agents_rendered_session.replace(Some(key));
            ivars.agents_reset_scroll.set(true);
        }
        let previous_top = scroll.contentView().bounds().origin.y + scroll.contentSize().height;
        let reset_scroll = ivars.agents_reset_scroll.replace(false);
        let restore_layout = ivars.agents_layout_restore.replace(false);
        let distance_from_top =
            if changed_session || ivars.desk_restore_pending.get() || restore_layout {
                self.agent_view().desk.tree_top
            } else if reset_scroll {
                0.0
            } else {
                (container.frame().size.height - previous_top).max(0.0)
            };
        for view in ivars.agents_row_views.borrow_mut().drain(..) {
            view.removeFromSuperview();
        }
        ivars.agents_ticking.borrow_mut().clear();
        let root = self.root_agent();
        let subagents = owner.subagents.borrow();
        let state = self.agent_view();
        let work = agents::visible_work(&root, &subagents, self.agent_clock_ms(), &state);
        if state.page != work.page {
            let key = owner.selected_session_path.borrow().clone();
            ivars
                .agent_view_states
                .borrow_mut()
                .entry(key)
                .or_default()
                .page = work.page;
        }
        let show_search = state.search_open || !state.query.is_empty() || subagents.len() >= 8;
        if let Some(search) = ivars.agents_search.get() {
            search.setHidden(!show_search);
            if search.stringValue().to_string() != state.query {
                search.setStringValue(&NSString::from_str(&state.query));
            }
        }
        if let Some(button) = ivars.agents_density.get() {
            let label = if state.compact {
                "Use comfortable rows"
            } else {
                "Use compact rows"
            };
            button.setToolTip(Some(&NSString::from_str(label)));
            button.setAccessibilityLabel(Some(&NSString::from_str(label)));
            let tint = if state.compact {
                theme::accent()
            } else {
                theme::secondary_text()
            };
            button.setContentTintColor(Some(&tint));
        }
        if let Some(scope) = ivars.agents_scope.get() {
            scope.setSelectedSegment(match state.scope {
                agents::WorkScope::All => 0,
                agents::WorkScope::Active => 1,
                agents::WorkScope::Attention => 2,
            });
        }
        if let Some(summary) = ivars.agents_summary.get() {
            let line = if subagents.is_empty() {
                "Primary agent · No delegated tasks".into()
            } else if subagents.len() > 6 {
                format!(
                    "{} tasks · {} active · {} attention",
                    subagents.len(),
                    subagents
                        .iter()
                        .filter(|info| agents::AgentState::from_status(&info.status).is_busy())
                        .count(),
                    subagents
                        .iter()
                        .filter(|info| agents::needs_attention(info))
                        .count()
                )
            } else {
                agents::summary_line(&subagents)
            };
            summary.setStringValue(&NSString::from_str(&line));
            summary.setToolTip(Some(&NSString::from_str(&agents::summary_line(&subagents))));
        }
        let inspection_height = self.refresh_agent_inspection();
        let pane = panel.bounds().size;
        let pager = work.pages > 1;
        let controls_bottom = pane.height - if show_search { 226.0 } else { 190.0 };
        let list_top = controls_bottom - if pager { 32.0 } else { 0.0 };
        scroll.setFrame(rect(
            12.0,
            64.0 + inspection_height,
            pane.width - 24.0,
            (list_top - 64.0 - inspection_height).max(60.0),
        ));
        let width = (scroll.contentSize().width - 12.0).max(240.0);
        let columns = state.compact && width >= 480.0;
        let rows = work.rows;
        let title_font = theme::ui_font(13.0, true);
        let title_heights: Vec<f64> = rows
            .iter()
            .map(|row| {
                let available = width
                    - if matches!(row, agents::AgentRow::Root { .. }) {
                        37.0
                    } else {
                        51.0
                    };
                let measured = attributed(row.title(), &title_font, &NSColor::labelColor(), 0.0);
                if !state.compact
                    && (measured.size().width > available || row.title().contains('\n'))
                {
                    35.0
                } else {
                    19.0
                }
            })
            .collect();
        let row_heights: Vec<f64> = rows
            .iter()
            .zip(&title_heights)
            .map(|(row, title_height)| {
                if matches!(row, agents::AgentRow::Section { .. }) {
                    return 32.0;
                }
                if columns && matches!(row, agents::AgentRow::Subagent { .. }) {
                    return 32.0;
                }
                if state.compact {
                    return 44.0;
                }
                let note = matches!(
                    row,
                    agents::AgentRow::Subagent { error: Some(_), .. }
                        | agents::AgentRow::Subagent {
                            warning: Some(_),
                            ..
                        }
                        | agents::AgentRow::Root {
                            state: agents::AgentState::Blocked(_),
                            ..
                        }
                );
                title_height + 33.0 + if note { 22.0 } else { 0.0 }
            })
            .collect();
        let empty = work.matched == 0;
        let height = (row_heights.iter().sum::<f64>() + if empty { 88.0 } else { 12.0 })
            .max(scroll.contentSize().height);
        container.setFrameSize(NSSize::new(width, height));
        let mut views: Vec<Retained<NSView>> = Vec::new();
        let mut buttons = Vec::new();
        let mut top = height;
        let selected = state.selected.clone();
        let last_child_center = rows
            .iter()
            .zip(&row_heights)
            .scan(height, |top, (row, row_height)| {
                let old = *top;
                *top -= row_height;
                Some((row, old - 19.0))
            })
            .filter_map(|(row, y)| matches!(row, agents::AgentRow::Subagent { .. }).then_some(y))
            .last();
        if let Some(bottom) = last_child_center {
            let trunk = fill_box(
                rect(12.0, bottom, 1.0, height - 28.0 - bottom),
                &theme::hairline(),
                0.0,
                mtm,
            );
            container.addSubview(&trunk);
            views.push(trunk.into_super());
        }
        for (index, row) in rows.iter().enumerate() {
            let row_height = row_heights[index];
            let y = top - row_height;
            if let agents::AgentRow::Section { section, count, .. } = row {
                let title = format!("{} · {count}", section.title());
                if *section == agents::Section::Finished
                    && state.query.trim().is_empty()
                    && state.scope == agents::WorkScope::All
                {
                    let collapse = navigation_button(
                        &title,
                        if state.finished_collapsed {
                            "chevron.right"
                        } else {
                            "chevron.down"
                        },
                        rect(
                            26.0,
                            y + 2.0,
                            width - if columns { 264.0 } else { 32.0 },
                            26.0,
                        ),
                        self,
                        sel!(toggleFinishedAgents:),
                        mtm,
                    );
                    collapse.setFont(Some(&theme::ui_font(12.0, true)));
                    collapse.setContentTintColor(Some(&theme::secondary_text()));
                    collapse.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                        "{} {title}",
                        if state.finished_collapsed {
                            "Expand"
                        } else {
                            "Collapse"
                        }
                    ))));
                    container.addSubview(&collapse);
                    views.push(collapse.into_super().into_super());
                } else {
                    let label = text_label(&title, 12.0, true, mtm);
                    label.setTextColor(Some(&theme::secondary_text()));
                    label.setFrame(rect(
                        44.0,
                        y + 5.0,
                        width - if columns { 282.0 } else { 50.0 },
                        20.0,
                    ));
                    container.addSubview(&label);
                    views.push(label.into_super().into_super());
                }
                if columns {
                    for (caption, x, column_width, alignment) in [
                        ("State", width - 226.0, 78.0, NSTextAlignment::Left),
                        ("Role", width - 144.0, 68.0, NSTextAlignment::Left),
                        ("Elapsed", width - 73.0, 66.0, NSTextAlignment::Right),
                    ] {
                        let label = text_label(caption, 12.0, false, mtm);
                        label.setTextColor(Some(&theme::secondary_text()));
                        label.setAlignment(alignment);
                        label.setFrame(rect(x, y + 5.0, column_width, 20.0));
                        container.addSubview(&label);
                        views.push(label.into_super().into_super());
                    }
                }
                top = y;
                continue;
            }
            let is_root = matches!(row, agents::AgentRow::Root { .. });
            let row_columns = columns && !is_root;
            let note = match row {
                agents::AgentRow::Subagent { error, warning, .. } => {
                    error.as_deref().or(warning.as_deref())
                }
                agents::AgentRow::Root {
                    state: agents::AgentState::Blocked(why),
                    ..
                } => Some(why.as_str()),
                _ => None,
            };
            let ink = if note.is_some()
                || matches!(
                    row.state(),
                    agents::AgentState::Errored | agents::AgentState::Interrupted
                ) {
                theme::plum()
            } else if row.state().is_busy() {
                theme::accent()
            } else {
                theme::secondary_text()
            };
            let text_x = if is_root { 30.0 } else { 44.0 };
            let icon_x = if is_root { 5.0 } else { 25.0 };
            if !is_root {
                let branch = fill_box(
                    rect(12.0, top - 19.0, 13.0, 1.0),
                    &theme::hairline(),
                    0.0,
                    mtm,
                );
                container.addSubview(&branch);
                views.push(branch.into_super());
            }
            let frame = rect(
                text_x - 7.0,
                y + 2.0,
                width - text_x + 5.0,
                row_height - 3.0,
            );
            let pill = fill_box(frame, &theme::accent_fill_strong(), 6.0, mtm);
            pill.setWantsLayer(true);
            pill.setAlphaValue(0.0);
            container.addSubview(&pill);
            views.push(pill.clone().into_super());
            let seed = fill_box(seed_frame(frame), &theme::accent(), 2.0, mtm);
            seed.setWantsLayer(true);
            seed.setAlphaValue(0.0);
            container.addSubview(&seed);
            views.push(seed.clone().into_super());
            let symbol = if note.is_some() {
                "exclamationmark.circle"
            } else {
                match row.state() {
                    agents::AgentState::Working
                    | agents::AgentState::Thinking
                    | agents::AgentState::RunningTool(_) => {
                        "arrow.trianglehead.2.clockwise.rotate.90"
                    }
                    agents::AgentState::Done => "checkmark.circle",
                    agents::AgentState::Errored => "xmark.circle",
                    agents::AgentState::Interrupted => "stop.circle",
                    agents::AgentState::Blocked(_) => "exclamationmark.circle",
                    agents::AgentState::Detached => "clock",
                    agents::AgentState::Idle => "circle",
                }
            };
            let icon = NSImageView::initWithFrame(
                NSImageView::alloc(mtm),
                rect(icon_x, top - 26.0, 15.0, 15.0),
            );
            icon.setImage(
                theme::symbol(symbol, 12.0, unsafe { NSFontWeightMedium }, None).as_deref(),
            );
            icon.setContentTintColor(Some(&ink));
            container.addSubview(&icon);
            views.push(icon.into_super().into_super());
            let title = wrapping_label(row.title(), 13.0, mtm);
            title.setFont(Some(&theme::ui_font(13.0, true)));
            title.setMaximumNumberOfLines(if state.compact { 1 } else { 2 });
            title.setLineBreakMode(if state.compact {
                objc2_app_kit::NSLineBreakMode::ByTruncatingTail
            } else {
                objc2_app_kit::NSLineBreakMode::ByWordWrapping
            });
            let title_height = title_heights[index];
            title.setFrame(rect(
                text_x,
                top - title_height - 7.0,
                width - text_x - if row_columns { 238.0 } else { 7.0 },
                title_height,
            ));
            container.addSubview(&title);
            views.push(title.into_super().into_super());
            let info = match row {
                agents::AgentRow::Subagent { id, .. } => {
                    subagents.iter().find(|info| &info.id == id)
                }
                _ => None,
            };
            let subtitle = if row_columns {
                if note.is_some() && row.state() == &agents::AgentState::Done {
                    "review".into()
                } else {
                    row.state().label()
                }
            } else if let Some(info) = info {
                if note.is_some() && row.state() == &agents::AgentState::Done {
                    format!("done · review · {}", info.agent)
                } else {
                    format!("{} · {}", row.state().label(), info.agent)
                }
            } else {
                format!("Primary · {}", row.subtitle())
            };
            let meta_y = if row_columns {
                top - 25.0
            } else {
                top - title_height - 22.0
            };
            let time = row.elapsed().or_else(|| {
                info.and_then(|info| info.duration_ms.map(std::time::Duration::from_millis))
            });
            let time_width = if time.is_some() { 70.0 } else { 0.0 };
            let meta = text_label(&subtitle, 12.0, false, mtm);
            meta.setTextColor(Some(&ink));
            meta.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
            meta.setFrame(rect(
                if row_columns { width - 226.0 } else { text_x },
                meta_y,
                if row_columns {
                    78.0
                } else {
                    width - text_x - time_width - 9.0
                },
                18.0,
            ));
            container.addSubview(&meta);
            views.push(meta.into_super().into_super());
            if row_columns && let Some(info) = info {
                let role = text_label(&info.agent, 12.0, false, mtm);
                role.setTextColor(Some(&theme::secondary_text()));
                role.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
                role.setFrame(rect(width - 144.0, meta_y, 68.0, 18.0));
                container.addSubview(&role);
                views.push(role.into_super().into_super());
            }
            if let Some(time) = time {
                let clock_label = text_label(&agents::elapsed_text(time), 12.0, false, mtm);
                clock_label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                    12.0,
                    unsafe { NSFontWeightRegular },
                )));
                clock_label.setTextColor(Some(&theme::secondary_text()));
                clock_label.setAlignment(NSTextAlignment::Right);
                clock_label.setFrame(rect(width - 73.0, meta_y, 66.0, 18.0));
                container.addSubview(&clock_label);
                if row.state().is_busy() {
                    let clock = if is_root {
                        Some(AgentClock::Turn)
                    } else {
                        info.and_then(|info| info.started_ms)
                            .map(AgentClock::Started)
                    };
                    if let Some(clock) = clock {
                        ivars.agents_ticking.borrow_mut().push((
                            clock_label.clone(),
                            String::new(),
                            clock,
                        ));
                    }
                }
                views.push(clock_label.into_super().into_super());
            }
            if let Some(note) = note.filter(|_| !state.compact) {
                let label = text_label(note, 12.0, false, mtm);
                label.setTextColor(Some(&theme::plum()));
                label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
                label.setFrame(rect(text_x, meta_y - 22.0, width - text_x - 7.0, 18.0));
                container.addSubview(&label);
                views.push(label.into_super().into_super());
            }
            if let agents::AgentRow::Subagent { id, .. } = row {
                let button =
                    AgentRowButton::new(id, frame, &pill, &seed, self, sel!(inspectAgent:), mtm);
                button.setTag(index as isize);
                let detail = format!(
                    "{}. {}. {}",
                    row.title(),
                    row.subtitle_line(),
                    note.unwrap_or("Inspect task")
                );
                button.setAccessibilityLabel(Some(&NSString::from_str(&detail)));
                button.setToolTip(Some(&NSString::from_str(&detail)));
                button.set_selected(selected.as_deref() == Some(id.as_str()));
                container.addSubview(&button);
                views.push(button.clone().into_super().into_super().into_super());
                buttons.push(button);
            }
            top = y;
        }
        if empty {
            let text = if subagents.is_empty() {
                "Delegated tasks appear here as your agent starts them."
            } else if state.scope == agents::WorkScope::Attention && state.query.is_empty() {
                "Nothing needs attention. All tasks remain available under All."
            } else {
                "No matching tasks. Try another search or choose All."
            };
            let note = wrapping_label(text, 13.0, mtm);
            note.setTextColor(Some(&theme::secondary_text()));
            note.setFrame(rect(30.0, top - 75.0, width - 42.0, 60.0));
            container.addSubview(&note);
            views.push(note.into_super().into_super());
        }
        if pager {
            let label = text_label(
                &format!(
                    "Page {} of {} · {} tasks",
                    work.page + 1,
                    work.pages,
                    work.matched
                ),
                12.0,
                false,
                mtm,
            );
            label.setTextColor(Some(&theme::secondary_text()));
            label.setFrame(rect(18.0, controls_bottom - 24.0, width - 80.0, 20.0));
            panel.addSubview(&label);
            views.push(label.into_super().into_super());
            for (tag, symbol, title, enabled) in [
                (0, "chevron.left", "Previous agents", work.page > 0),
                (
                    1,
                    "chevron.right",
                    "Next agents",
                    work.page + 1 < work.pages,
                ),
            ] {
                let button = icon_button(
                    symbol,
                    title,
                    rect(
                        pane.width - 78.0 + tag as f64 * 30.0,
                        controls_bottom - 28.0,
                        28.0,
                        28.0,
                    ),
                    self,
                    sel!(pageAgentWork:),
                    mtm,
                );
                button.setTag(tag);
                button.setEnabled(enabled);
                panel.addSubview(&button);
                views.push(button.into_super().into_super());
            }
        }
        *ivars.agents_row_views.borrow_mut() = views;
        *ivars.agents_row_buttons.borrow_mut() = buttons;
        *ivars.agents_rows.borrow_mut() = rows;
        let clip = scroll.contentView();
        clip.scrollToPoint(NSPoint::new(
            0.0,
            (height - distance_from_top - scroll.contentSize().height).max(0.0),
        ));
        scroll.reflectScrolledClipView(&clip);
    }
}
