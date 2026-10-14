//! Opt-in visual fixture. Compiled only with `--features design-preview`.
//! Uses the actual AppKit view hierarchy and renderer, without starting OMP.
use super::*;

impl AppDelegate {
    pub(super) fn show_design_preview(&self) {
        let ivars = self.ivars();
        // The fixture has its own preference database. Load it on a worker,
        // allowing calibration to survive relaunch without touching real data.
        self.jobs().submit(|| {
            BackgroundEvent::PreviewPreferencesLoaded(preferences::load().unwrap_or_default())
        });
        // The fixture clock is static. Job completions wake the main run loop;
        // no timer runs just to poll an empty queue or repaint the same time.
        let directory = std::path::PathBuf::from("/Projects/omperor");
        ivars.current_project.replace(directory.clone());
        ivars
            .current_model
            .replace("openai-codex/gpt-6-astra".into());
        ivars.current_thinking_level.replace("high".into());
        ivars.active_project_id.replace(Some("omperor".into()));
        ivars.projects.replace(vec![
            Project {
                id: "omperor".into(),
                name: "Omperor".into(),
                directories: vec![directory.clone()],
                last_opened_at_ms: 2,
                collapsed: false,
            },
            Project {
                id: "atlas".into(),
                name: "Atlas".into(),
                directories: vec!["/Projects/atlas".into()],
                last_opened_at_ms: 1,
                collapsed: false,
            },
        ]);
        let titles = [
            "Build the next release",
            "Inspect a parallel workload",
            "Improve keyboard navigation",
            "Explore the rendering pipeline",
            "Plan the next release",
            "Document the architecture",
        ];
        let mut summaries = Vec::new();
        for (i, title) in titles.into_iter().enumerate() {
            let id = format!("preview-{i}");
            let folder = if i < 4 {
                "/Projects/omperor"
            } else {
                "/Projects/atlas"
            };
            let mut summary = session::SessionSummary::for_test(&id, title, Some(folder), true);
            summary.model = Some("gpt-6-astra".into());
            summary.outcome = Some(session::SessionOutcome::Completed);
            if i == 0 {
                ivars
                    .selected_session_path
                    .replace(Some(summary.path.clone()));
            }
            ivars.session_records.borrow_mut().insert(
                id.clone(),
                SessionRecord {
                    session_id: id,
                    project_id: if i < 4 { "omperor" } else { "atlas" }.into(),
                    directory: folder.into(),
                    parent_session_id: None,
                    branch: None,
                    base_branch: None,
                    worktree_path: None,
                    created_at_ms: i as i64,
                },
            );
            summaries.push(summary);
        }
        ivars.sessions.replace(summaries);
        self.rebuild_sidebar_sessions();
        self.refresh_project_location();
        ivars
            .center_title
            .get()
            .unwrap()
            .setStringValue(ns_string!("Build the next release"));
        self.load_preview_brief();
        self.load_preview_team();
        self.set_header_session(Some("preview-session"), None);
        self.set_chat_status("Design preview · Sample conversation · No inference running");
        self.set_model_label("GPT-6 Astra");
        self.set_thinking_label("high");
        self.configure_preview_composer();
        self.refit_composer_chips();
        ivars.input.get().unwrap().setEditable(true);
        ivars.send_button.get().unwrap().setEnabled(false);
        ivars
            .connect_button
            .get()
            .unwrap()
            .setTitle(ns_string!("Design preview"));
        self.load_preview_changes();
        self.set_inspector_rows(
            "codex/interface-redesign",
            "Up to date",
            "Refine the native reading experience",
        );
        self.refresh_chat_transcript();
        if let Some(scroll) = ivars
            .chat_transcript
            .get()
            .map(|transcript| transcript.scroll_view().clone())
        {
            let clip = scroll.contentView();
            clip.scrollToPoint(NSPoint::new(0.0, 0.0));
            scroll.reflectScrolledClipView(&clip);
        }
        ivars.new_chat_button.get().unwrap().setEnabled(true);
        ivars.new_chat_menu_item.get().unwrap().setEnabled(true);
        self.show_workbench_panel(1);
        self.layout_header();
        if std::env::args().any(|arg| arg == "--account-alert") {
            // Synthetic identities only. The ordinary binary never compiles
            // this scenario and no credential store or OMP child is involved.
            let accounts: Vec<_> = (0..6).map(|i| AccountEntry {
                summary: AccountSummary { id: format!("openai-codex:preview-{i}"), provider: AuthProvider::OpenAi,
                    principal: Some(format!("sample-{i}@example.test")), state: omp_runtime::AccountState::Active },
                label: Some(if i == 5 { "Work · release engineering".into() } else { format!("Sample account {}", i + 1) }),
                authenticated_at_ms: 0, is_default: i == 0,
            }).collect();
            ivars.accounts.replace(accounts);
            self.note_account_trouble("no credential is stored for openai-codex account openai-codex:preview-5; sign in again", false, None);
        }
    }
    fn configure_preview_composer(&self) {
        let ivars = self.ivars();
        let model = ivars.model_picker.get().unwrap();
        model.removeAllItems();
        for title in ["GPT-6 Astra", "GPT-5.6 Sol", "GPT-5.6 Luna"] {
            model.addItemWithTitle(&NSString::from_str(title));
        }
        model.setEnabled(true);
        let reasoning = ivars.thinking_picker.get().unwrap();
        reasoning.selectItemWithTitle(ns_string!("high"));
        reasoning.setEnabled(true);
        let account = ivars.account_picker.get().unwrap();
        account.removeAllItems();
        for title in ["Personal", "Work"] {
            account.addItemWithTitle(&NSString::from_str(title));
        }
        account.setHidden(false);
        account.setEnabled(true);
        ivars.account_label.get().unwrap().setHidden(false);
        ivars
            .account_label
            .get()
            .unwrap()
            .setStringValue(ns_string!("Personal  ▾"));
        ivars
            .fast_models
            .replace(vec![ivars.current_model.borrow().clone()]);
        ivars.fast_button.get().unwrap().setEnabled(true);
        self.refresh_fast_chip();
    }

    pub(super) fn preview_select_model(&self) {
        if let Some(title) = self
            .ivars()
            .model_picker
            .get()
            .and_then(|picker| picker.titleOfSelectedItem())
        {
            self.set_model_label(&title.to_string());
            self.set_chat_status("Design preview · Model selection updated · No inference running");
        }
    }

    pub(super) fn preview_select_thinking(&self) {
        if let Some(title) = self
            .ivars()
            .thinking_picker
            .get()
            .and_then(|picker| picker.titleOfSelectedItem())
        {
            self.set_thinking_label(&title.to_string());
            self.set_chat_status(
                "Design preview · Reasoning selection updated · No inference running",
            );
        }
    }

    pub(super) fn preview_select_account(&self) {
        if let Some(title) = self
            .ivars()
            .account_picker
            .get()
            .and_then(|picker| picker.titleOfSelectedItem())
        {
            self.ivars()
                .account_label
                .get()
                .unwrap()
                .setStringValue(&NSString::from_str(&format!("{title}  ▾")));
            self.refit_composer_chips();
            self.set_chat_status(
                "Design preview · Account selection updated · No inference running",
            );
        }
    }

    pub(super) fn preview_toggle_fast(&self) {
        self.ivars().fast_mode.set(!self.ivars().fast_mode.get());
        self.refresh_fast_chip();
        self.set_chat_status("Design preview · Fast mode updated · No inference running");
    }
    fn load_preview_brief(&self) {
        self.ivars().conversation.borrow_mut().load_history(&[
            session::SessionMessage::spoken(session::Speaker::User, "Help me ship the next release. Review the interface, check the implementation, and flag anything that needs my attention."),
            session::SessionMessage::tool("task", "Delegated interface, implementation, and release reviews", Some("Design preview: these are isolated sample tasks. Open their task rows to inspect sample transcripts.".into()), false),
            session::SessionMessage::spoken(session::Speaker::Assistant, concat!(
                "## Release review\n\n",
                "I’ve divided the review into focused tasks. You can keep the conversation here and follow each agent alongside it.\n\n",
                "### Ready for your review\n\n",
                "The interface review is complete. The implementation check is in progress. The release review found a change that needs your attention.\n\n",
                "- **Interface:** clearer hierarchy and readable native controls.\n",
                "- **Implementation:** checking the composer and keyboard flow.\n",
                "- **Release:** changes are ready, but have not been merged.\n\n",
                "Open an agent task to read its work. Use **Focus** when you want more room for the conversation.\n\n",
                "This is an interactive design preview. No agents or inference are running."
            )),
        ]);
    }

    fn load_preview_team(&self) {
        let ivars = self.ivars();
        ivars.live_session_id.replace(Some("preview-0".into()));
        ivars.live_state.replace(agents::AgentState::Idle);
        let examples = [
            (
                "implementation",
                "task",
                "Check implementation and keyboard flow",
                "running",
                None,
                "I’m checking the composer controls, tab order, and resizing behavior.\n\nThe model and reasoning menus respond to mouse and keyboard input. I’m checking the remaining narrow-window cases.",
            ),
            (
                "release",
                "reviewer",
                "Review the release changes",
                "completed",
                Some("Changes are ready; merge still needs review."),
                "## Ready for review\n\nThe changes pass the available checks. The branch still needs a human review before merging.\n\nThis warning stays visible in the task tree so it is easy to find.",
            ),
            (
                "interface",
                "scout",
                "Audit the interface and typography",
                "completed",
                None,
                "## Review complete\n\n- Native text remains sharp at every configured size.\n- The conversation and agent workspace are visible together.\n- Status labels describe state without relying on color.\n\nThe next useful check is a full keyboard and VoiceOver walkthrough.",
            ),
        ];
        let now = wall_clock_ms();
        self.ivars().preview_clock_ms.set(now);
        let mut team = Vec::new();
        for (index, (id, class, title, status, warning, reply)) in examples.into_iter().enumerate()
        {
            team.push(omp_runtime::SubagentInfo {
                id: id.into(),
                index: index as u64,
                agent: class.into(),
                label: title.into(),
                status: status.into(),
                started_ms: Some(now.saturating_sub(134_000)),
                duration_ms: (status == "completed").then_some(78_000),
                error: None,
                warning: warning.map(str::to_owned),
                session_file: Some(format!("/design-preview/{id}.oms").into()),
            });
            ivars.subagent_transcripts.borrow_mut().insert(
                id.into(),
                transcripts::Entry {
                    read_started: std::time::Instant::now(),
                    stamp: transcripts::Stamp::default(),
                    messages: vec![
                        session::SessionMessage::spoken(session::Speaker::User, title),
                        session::SessionMessage::tool("verify", "Native interaction check", Some("Sample verification record\n\n1. Open the conversation.\n2. Inspect a delegated task.\n3. Return to the conversation.\n\nObserved: the selected task remains visible and keyboard navigation continues to work.\n\nThis is illustrative evidence; no command was run.".into()), false),
                        session::SessionMessage::spoken(session::Speaker::Assistant, reply),
                    ],
                },
            );
        }
        ivars.subagents.replace(team);
    }

    fn load_preview_workload(&self) {
        let ivars = self.ivars();
        let now = wall_clock_ms();
        self.ivars().preview_clock_ms.set(now);
        let areas = [
            "Authentication",
            "Session storage",
            "Agent events",
            "Workspace isolation",
            "Metrics pipeline",
            "Keyboard navigation",
            "Native rendering",
            "Reconnect behavior",
            "Tool permissions",
            "Cancellation",
            "Transcript search",
            "Artifact indexing",
        ];
        let kinds = [
            "Review",
            "Test",
            "Investigate",
            "Implement",
            "Verify",
            "Document",
            "Profile",
        ];
        let mut work = Vec::new();
        for index in 0..84 {
            let id = format!("sample-task-{index:03}");
            let title = format!(
                "{} · {}",
                areas[index % areas.len()],
                kinds[index / areas.len()]
            );
            let status = match index % 8 {
                0..=2 => "running",
                3 => "failed",
                4 => "cancelled",
                _ => "completed",
            };
            let note = match index % 8 {
                3 => Some(
                    "The validation command exited with an error. Inspect the transcript before retrying.",
                ),
                5 => Some("Work completed; proposed changes need review."),
                _ => None,
            };
            let role = if index % 3 == 0 {
                "scout"
            } else if index % 3 == 1 {
                "reviewer"
            } else {
                "task"
            };
            work.push(omp_runtime::SubagentInfo {
                id: id.clone(),
                index: index as u64,
                agent: role.into(),
                label: title.clone(),
                status: status.into(),
                started_ms: Some(now.saturating_sub(90_000 + index as u64 * 13_000)),
                duration_ms: (status != "running").then_some(42_000 + index as u64 * 3_000),
                error: (status == "failed").then(|| note.unwrap().to_owned()),
                warning: (index % 8 == 5).then(|| note.unwrap().to_owned()),
                session_file: (index != 2).then(|| format!("/design-preview/{id}.oms").into()),
            });
            if index == 2 {
                continue;
            }
            let finding = match index {
                3 => {
                    "## A session should outlive its client\n\n**Finding:** a lost connection is not evidence that work stopped. Keep task identity independent of the UI process.\n\n### Proposed boundary\n\n- Preserve the task's last acknowledged state.\n- Reconnect to the same execution identity.\n- Show an explicit unknown state until the worker answers.\n\n### Evidence still needed\n\nA disconnect/reconnect test while a delegated task is writing output. Expand the validation tool above to inspect its recorded output.\n\n*Illustrative review for this design fixture.*"
                }
                5 => {
                    "## Preserve the reader's place\n\n**Finding:** new output should be discoverable without moving the text someone is reading.\n\n### Proposed interaction\n\n1. Open the agent in a tab while you inspect another task.\n2. Switch sessions with an unfinished draft.\n3. Return to the same record, selection, and reading position.\n\n### Open question\n\nHow should an unread update be exposed when the reader is comparing two older records?\n\n*Illustrative review for this design fixture.*"
                }
                _ => "",
            };
            ivars.subagent_transcripts.borrow_mut().insert(id, transcripts::Entry {
                    read_started: std::time::Instant::now(),
                stamp: transcripts::Stamp::default(),
                messages: vec![
                    session::SessionMessage::spoken(session::Speaker::User, if index == 3 {
                        "Review what happens when a client disconnects while its worker is still running. Inspect ownership of session state, exercise the reconnect path, and report any lost or duplicated events. Bring back the validation output; do not change the runtime yet.".into()
                    } else if index == 5 {
                        "Audit keyboard focus when switching between two sessions with an unfinished draft. Check text selection, the current activity record, and scroll restoration. Report concrete failures and the expected behavior before proposing changes.".into()
                    } else {
                        format!("Investigate {title}. Trace the relevant execution path, test one concrete failure case, and return the supporting tool output along with your findings. Keep changes scoped to this assignment.")
                    }),
                    session::SessionMessage::spoken(session::Speaker::Assistant, "I’ll separate the lifecycle state from what the client currently displays, then exercise the return path. The test output will determine whether the behavior is ready for review."),
                    session::SessionMessage::tool("read", "Inspected the lifecycle boundary", Some("## Source inspection\n\nThe client retains task identity while the worker owns execution state. The remaining question is whether reconnecting restores the last acknowledged event without duplicating it.\n\n*Illustrative source observation for this design fixture.*".into()), false),
                    session::SessionMessage::tool("validation", "Captured validation output", Some(format!("# Validation record\n\nArea: {title}\nWorker: {role}\n\n```text\nload fixture       ok\nexercise lifecycle ok\ncheck return path  {}\n```\n\nThis record is illustrative. No process or remote host was started.", if status == "failed" { "FAILED" } else { "ok" })), status == "failed"),
                    session::SessionMessage::spoken(session::Speaker::Assistant, if !finding.is_empty() { finding.to_owned() } else { format!("## Sample task evidence\n\nThis is an illustrative workload for testing native navigation at scale. No process was launched.\n\n- **Task:** {title}\n- **State:** {status}\n- **Role:** {role}\n\n{}", note.unwrap_or("Open another task, or use search and the attention filter to narrow the workload.")) }),
                ],
            });
        }
        ivars.subagents.replace(work);
        ivars.live_state.replace(agents::AgentState::Idle);
        ivars.conversation.borrow_mut().load_history(&[
            session::SessionMessage::spoken(session::Speaker::User, "Show me how a larger parallel workload would feel in Omperor."),
            session::SessionMessage::spoken(session::Speaker::Assistant, "## Keep the whole workload legible\n\nThis sample contains **84 delegated tasks**, with running work, completed work, failures, interruptions, and review warnings. They all belong to this conversation.\n\n### Follow what matters\n\n- Choose **Attention** to find tasks that need intervention.\n- Search for an area, role, or error—for example, `authentication` or `reviewer`.\n- Use **compact rows** to scan more work at once; expand the workspace to reveal aligned state, role, and elapsed-time columns.\n- **Expand** the workspace when inspection becomes the task at hand.\n- Open any task to read its sample evidence.\n\nSearch and spacing belong to this session. Return to **Build the next release** to see the smaller workload exactly as you left it.\n\nThis is sample data. Remote execution and task scheduling are future backend work; no agents are running."),
        ]);
    }

    pub(super) fn preview_open_conversation(&self, summary: &session::SessionSummary) {
        self.ivars()
            .center_title
            .get()
            .unwrap()
            .setStringValue(&NSString::from_str(&summary.title));
        if summary.id == "preview-0" {
            self.load_preview_brief();
            self.load_preview_team();
        } else if summary.id == "preview-1" {
            self.load_preview_workload();
            if !self.ivars().preview_workload_prepared.replace(true) {
                self.edit_desk(|desk| {
                    desk.active = true;
                    desk.inspect("sample-task-003");
                });
            }
        } else {
            self.ivars().subagents.borrow_mut().clear();
            self.ivars().conversation.borrow_mut().load_history(&[
                session::SessionMessage::spoken(session::Speaker::User, &summary.title),
                session::SessionMessage::spoken(session::Speaker::Assistant, "This sample conversation is ready for review. Use the sidebar to return to **Build the next release** and explore its delegated tasks."),
            ]);
        }
        self.ivars()
            .live_session_id
            .replace(Some(summary.id.clone()));
        self.restore_session_desk();
        self.set_header_session(Some(&summary.id), None);
        self.set_chat_status("Design preview · Sample conversation · No inference running");
        self.refresh_chat_transcript();
        self.refresh_agents_panel();
    }

    pub(super) fn preview_new_conversation(&self) {
        self.prepare_new_session_desk(false);
        self.ivars()
            .center_title
            .get()
            .unwrap()
            .setStringValue(ns_string!("New conversation"));
        self.ivars().selected_session_path.replace(None);
        self.ivars().desk_restore_pending.set(true);
        self.restore_session_desk();
        self.ivars().subagents.borrow_mut().clear();
        self.ivars().conversation.borrow_mut().load_history(&[
            session::SessionMessage::spoken(session::Speaker::Assistant, "## What would you like to work on?\n\nStart with a question, a rough idea, or a task. Omperor keeps the conversation and any delegated work together.\n\nThis design preview lets you explore the interface. Sending is available in the connected app."),
        ]);
        self.set_header_session(None, None);
        self.set_chat_status("Design preview · New conversation · No inference running");
        self.rebuild_sidebar_sessions();
        self.refresh_chat_transcript();
        self.refresh_agents_panel();
        NSApplication::sharedApplication(self.mtm())
            .mainWindow()
            .map(|window| {
                window.makeFirstResponder(
                    self.ivars()
                        .input
                        .get()
                        .map(|input| &**input as &NSResponder),
                )
            });
    }
    fn load_preview_changes(&self) {
        let changes = vec![
            workspace::WorkspaceChange {
                status: " M".into(),
                path: "src/layout.rs".into(),
                insertions: Some(3),
                deletions: Some(1),
            },
            workspace::WorkspaceChange {
                status: " M".into(),
                path: "src/theme.rs".into(),
                insertions: Some(2),
                deletions: Some(1),
            },
        ];
        self.ivars()
            .change_status
            .get()
            .unwrap()
            .setStringValue(ns_string!("Design preview · Sample changes"));
        self.set_change_section(changes.len(), Some((5, 2)));
        self.populate_change_list(&changes, None);
        self.ivars()
            .workspace_snapshot
            .replace(Some(workspace::WorkspaceSnapshot {
                project: "/Projects/omperor".into(),
                repository_root: Some("/Projects/omperor".into()),
                branch: Some("codex/interface-redesign".into()),
                changes,
                upstream: Some(workspace::Upstream {
                    ahead: 0,
                    behind: 0,
                }),
                head_subject: Some("Refine the workspace".into()),
            }));
    }

    pub(super) fn preview_change_diff(&self, change: &workspace::WorkspaceChange) {
        self.set_diff_title(Some(change));
        let value = if change.path.ends_with("layout.rs") {
            "@@ -1,2 +1,4 @@\n // Keep the conversation and agent work together.\n-let panel_visible = false;\n+let panel_visible = true;\n+let navigation_width = 248.0;\n+let command_width = 352.0;\n"
        } else {
            "@@ -1,2 +1,3 @@\n // Shared visual tokens.\n-let surface = white();\n+let surface = warm_canvas();\n+let accent = imperial_gold();\n"
        };
        let text = self.ivars().change_transcript.get().unwrap();
        // SAFETY: this standard NSTextView owns its storage for its lifetime.
        if let Some(storage) = unsafe { text.textStorage() } {
            storage.setAttributedString(&native_markdown::render_diff(value, &change.path));
        }
        text.scrollRangeToVisible(NSRange::new(0, 0));
    }
}
