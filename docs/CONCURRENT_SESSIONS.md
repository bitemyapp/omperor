# Concurrent sessions implementation plan

Status: proposed implementation plan, based on source exploration on 2026-09-20. This document proposes changes; concurrent primary-session execution is not implemented by this work.

## Goal and scope

Let several primary sessions execute independently while the user reads or writes in any one of them. Switching sessions, opening a historical transcript, or creating a conversation must not stop another session. The initial acceptance workload is three concurrent primary sessions, including delegated agents, matching the everyday persona in [Product direction](PRODUCT_DIRECTION.md). Three is a verification workload, not a hard product limit.

Keep the existing single-window sidebar, conversation, and Agent Observatory. This is an execution-ownership change, not a tab redesign or a new orchestration interface. Remote execution, multiple windows, detached execution after quitting Omperor, cross-device restoration, and automatic worktree creation are outside this change. Existing external/TUI sessions remain read-only.

## Findings from the current code

| Area | Observed implementation | Consequence |
| --- | --- | --- |
| Execution ownership | `AppDelegateIvars` in `src/main.rs:1356–1764` owns one `OnceCell<OmpRuntime>`, one `Conversation`, one live session ID, one busy state, one pending prompt, and one subagent board. | Starting additional children alone would still mix their state in the GUI. |
| Navigation | `open_sidebar_session` in `src/main.rs:2344–2405` refuses navigation during a turn, captures the current desk, clears subagent caches, then resumes an owned session or views an external transcript. | The refusal protects single-runtime ownership; remove it only after routing and state isolation exist. |
| Commands | `send_message` and `stop_omp` in `src/main.rs:2133–2236` address the sole runtime; sending while busy steers that turn. | Prompt, steer, and stop need explicit session targets, including prompts prepared asynchronously. |
| Runtime lifecycle | `OmpRuntime` in `src/omp_runtime.rs:838–1047` owns command/event channels and a worker. `NewSession`, `NewSessionWithModel`, and `ForkSession` in `handle_command` shut down the child before restarting it. | A runtime currently represents the application's changing execution destination rather than a stable session owner. |
| Event identity | `ArrivedEvent` is `(Instant, UiEvent)` in `src/omp_runtime.rs:801–804`; `Ready` includes session identity, but ordinary changes and notices do not. | Every event needs an owner outside its payload, including failures before `Ready`. |
| Polling | `poll_omp_runtime` in `src/main.rs:2238–2326` drains one receiver within a four-millisecond budget and selects timer cadence from singleton state. | Preserve a total budget across sessions and schedule fairly; do not multiply the budget by the number of runtimes. |
| Retained workspace | `SessionDesk` in `src/workspace_model.rs` holds draft text, pins, positions, selection, and layout. `SavedViews` in `src/session_state.rs` persists path-keyed views with one provisional `None` entry. | Reuse this model, but one provisional identity cannot represent several new conversations. |
| Attachments | `BackgroundEvent::AttachmentPrepared` carries a job and attachment ID; `AttachmentPromptPrepared` adds a generation but no session identity. `PendingAttachments` also owns native controls. | Separate attachment data from visible controls and route preparation to the original composer even after navigation. |
| Other asynchronous work | `BackgroundEvent` in `src/main.rs:267–346` already uses session and generation guards for evidence reads, while file-picker and workspace results have different scopes. | Extend the existing guarded-result pattern instead of introducing a second convention. |
| Shutdown | `application_should_terminate` and `complete_termination_if_ready` coordinate one runtime and ordered preference saves. `OmpRuntime::Drop` calls shutdown and joins the worker. | Never drop an unreaped runtime on the AppKit thread; quitting must drain every runtime and pending save. |
| Session safety | `SessionSummary::is_omperor_owned` in `src/session.rs:213–218` permits resuming owned journal sessions. The external-session branch in `open_sidebar_session` explains that OMP's fork path can open a source journal mutably. | Preserve read-only external browsing and investigate safe snapshot semantics before allowing forks of actively written journals. |

The product specification explicitly identifies background execution of multiple primary sessions as unfinished and calls for separating browsing state from execution ownership. The existing saved desk, background job pool, runtime worker, and bounded GUI polling are foundations to retain.

The vendored server owns one kernel/session pair and one turn-running flag (`vendor/oh-my-pi/crates/app/src/rpc_mode.rs:1593–1613`); its session transitions replace that session (`2710–2761`). The SDK's request-ID correlation is therefore not primary-session multiplexing. Its event broadcast buffer holds 1,024 events and reports lag (`vendor/oh-my-pi/crates/rpc/src/client.rs:393–443,778–781`); Omperor currently treats event-stream errors as runtime failure (`src/omp_runtime.rs:1480–1483`). Subagent concurrency remains scoped to its parent's kernel/job board rather than providing a top-level session registry.

## Recommended architecture

### One runtime per live primary session

Use a registry of session entries, each with at most one session-bound `OmpRuntime` and OMP child. Keep the existing worker/channel mechanism rather than introducing a second transport or multiplexing independent primary sessions through one child. Existing child replacement may remain an internal operation for restarting the same session, but New Chat and Fork must create a different registry entry instead of repurposing another entry's runtime.

A session may exist without a runtime: a new draft, a historical conversation being read, an external read-only transcript, or a previously active session whose idle child has been retired. Merely browsing history must not launch a child. Sending to an owned historical session starts or resumes its runtime asynchronously; selecting a live session reuses its existing runtime. This intentionally replaces selection-triggered resume and the associated `RESUME_SETTLE` behavior, rather than retaining a second lifecycle policy.

[INFERENCE] Process-per-session is the smallest backend change because the current Omperor runtime launches and controls one RPC client at a time. It also isolates child failure and permits reuse of its existing startup, event projection, and shutdown behavior. Shared credential stores, journal locking, and fork behavior still require the backend validation gate below; separate processes do not prove those resources are safe.

### Identity before startup

Introduce a small opaque `SessionKey`, allocated before any asynchronous work. Store the eventual OMP session ID and journal path as metadata on the entry; do not replace the key when `Ready` arrives. Maintain a journal-identity lookup so reopening the same owned journal selects its existing entry rather than creating a second writer. Normalize filesystem identity off-thread and include the source location rather than assuming bare IDs are globally unique.

Every runtime event carries `{ session_key, runtime_generation, arrived, event }`. Every session command carries its target key and expected runtime generation. Advance the generation whenever the child is replaced, including same-session account/model refresh restarts, and expose that transition explicitly to the registry. Accept `Ready` only if its reported identity matches the requested resume; report a mismatch on that entry without retargeting it.

Use separate request generations for replaceable work such as history loads, file-picker queries, attachment submission, and evidence reads. Changing the selected session does not invalidate legitimate work for another retained entry. Deleting or replacing its request does. No completion may determine its destination by asking which session happens to be selected when it arrives.

### State ownership

| Owner | State |
| --- | --- |
| Session entry | Conversation, lifecycle, runtime identity, pending prompt, selected model/account/thinking/fast settings, available model capabilities, subagents, turn/tool timings, live rate, session notices, transcript/history state, draft and attachment data, and session-local request generations. |
| Retained session view | Existing `agents::ViewState` and `SessionDesk`, including draft, Observatory filters, pins, reading positions, and selection; add Changes-pane selection and reading state where currently global. Keep one authoritative draft rather than duplicating it in two models. |
| Selected presentation | AppKit controls, text render caches, native attachment previews, active field editor, visible inspector and file-picker UI. Bind these to an entry; do not allocate a complete window's controls for every session. |
| Application services | Registry, selected key, project registry, discovery, preference writer, account catalog and login coordinator, aggregate metrics storage, global settings, and termination state. |
| Workspace resources | File index, watcher, and reusable workspace snapshot keyed by canonical working directory. Session-specific diff selection stays with its view. |

Extract the registry and non-AppKit session model from `main.rs` into a focused module. Keep conversation reduction reusable and free of AppKit calls. Split the existing event application path into owner-specific model updates and selected-view rendering; do not temporarily swap singleton ivars to impersonate a background session.

Preserve runtime arrival timestamps for metrics. Tool IDs, subagent IDs, notices, and counters are scoped to their owning session even if their strings match another child's values. Background completion can update a sidebar summary or attention marker, but cannot change the selected transcript, model chip, account banner, focus, draft, or scroll position.

### Event processing and responsiveness

Use round-robin polling of session receivers with a small per-session batch and the existing total four-millisecond drain budget. Advance the cursor between passes so the first busy session cannot starve others. Timer activity depends on any starting, running, stopping, or backlogged entry, not only the selected one. Preserve the existing wake and coalesced-refresh mechanisms.

Update hidden sessions' data without rendering native markdown or rebuilding their Observatory panes. Coalesce sidebar updates once per pass. Avoid full conversation copies during navigation: retain session models, switch the visible binding, and rebuild only the selected presentation as necessary. Keep history and evidence loading paged; apply cache limits to inactive presentation/history caches without discarding pending drafts, submitted work, or terminal outcomes.

Omperor's command and UI-event channels are unbounded, while the SDK event broadcast is bounded. Measure aggregate queue depth and payload bytes under concurrent output; add ordered batching/coalescing for compatible streaming updates and a bounded delivery policy before claiming larger-scale support. Never discard lifecycle transitions or silently drop transcript content, and do not block the RPC reader in a way that prevents abort or shutdown from completing.

The current runtime awaits `handle_command` inside its event-consumption loop (`src/omp_runtime.rs:1441–1484`), and account-usage requests can wait up to 45 seconds (`1704–1723`). Keep event consumption and urgent abort/shutdown responsive while slow requests are pending, using the existing RPC request correlation and explicit ordering for state-changing commands rather than allowing arbitrary concurrent mutation. Test an unselected session emitting more than 1,024 events during a slow request; no SDK lag failure or delayed shutdown may be hidden by the new registry.

### Runtime lifecycle and resource policy

Represent lifecycle explicitly: draft/history-only, starting, ready, running, stopping, failed, and retiring. Keep turn outcome separate from runtime availability so a ready child does not erase a failed or interrupted turn. A pending prompt belongs to exactly one entry; start it once after successful readiness. Do not replay a prompt after an ambiguous transport failure because the child may already have accepted it.

Keep active primary turns, running delegated work, pending submissions, and authentication operations alive regardless of selection. Initially retain idle runtimes for open live sessions and provide an explicit stop/release lifecycle; add measured idle eviction only when quiescence can be established for the primary agent and its children. Never use a global replacement policy to make space for another session. If a resource limit is introduced, show a specific admission failure or explicit queue state without blocking browsing or stopping existing work.

Stop in the composer targets only the selected session and keeps the existing distinction between aborting a turn and ending a runtime. A contextual Stop on a background sidebar row captures that row's key. Selecting history or closing a local reading surface is not a stop command. Quitting Omperor stops all owned execution; it does not promise detached continuation.

Retirement has two stages: request shutdown, then poll for completed worker reaping. Keep the handle in a retiring collection until `try_reap` succeeds; only then drop it. At quit, request shutdown for all entries before waiting for any one, reject new submissions, display aggregate stopping progress, flush all retained views through the ordered writer, and complete AppKit termination only when every worker and required save has settled. Bound child shutdown and surface failures without joining live workers on the GUI thread.

## User-visible behavior

- Switching A → B while A streams is immediate; A's sidebar row continues to show activity, and returning to A reveals its accumulated output without losing the prior reading anchor.
- New Chat creates a distinct draft in the selected project while existing sessions continue. Multiple new drafts have independent identities even before OMP assigns journals.
- Send starts a turn in the selected session; Send during that session's active turn preserves the current steering behavior. Stop never means “stop whichever runtime last emitted an event.”
- Sidebar rows show their own starting, running, stopping, failed, and attention states using text or accessible labels as well as color. Background completion remains discoverable without stealing focus or silently reordering manually arranged rows.
- Draft text, attachments, evidence pins, Observatory filters, transcript selection, and Changes-pane position survive navigation. Pending attachment preparation continues for its originating session.
- External/TUI sessions remain read-only while owned sessions run. Reading an external transcript neither stops execution nor creates another journal writer.
- Model/account changes affect the selected entry only. If a change requires a fresh conversation, create a separate entry; if it requires restarting the same session, defer until that session is quiescent or require an explicit stop rather than interrupting it invisibly.

## Shared resources and safety gates

### Accounts and settings

Keep one application-level login coordinator with explicit originating runtime/session and login generation. A sign-in sheet remains attached to that operation across navigation; late success cannot change another session's account pin. Reuse a suitable retained runtime for account management initially, pinning its lifetime while the operation is pending, rather than starting one account helper per session. Account usage refreshes should be coalesced globally, while model capabilities and account selection remain session-local.

Validate concurrent credential reads, token refresh, login, logout, and account metadata writes in the vendored OMP implementation. Serialize app-initiated mutations and propagate invalidation to affected sessions. Do not assume this prevents races with automatic refreshes in multiple children: fix the shared-store contract if required. Removing a credential must identify affected sessions and must not silently switch their accounts. Developer backend changes should apply to new sessions by default; any operation affecting existing sessions must name its scope and preserve their state.

### Journals, forks, and discovery

Enforce one writer per owned journal in the registry and verify backend locking across Omperor processes. Treat a journal already owned elsewhere as unavailable for execution, not as permission to start a second writer. Discovery refreshes merge disk summaries with live entries; they must not replace an in-memory conversation, clear active status, or erase an entry whose journal has not appeared yet.

Do not fork a running source through a path that opens its journal mutably. The first implementation may require the source and its delegated work to be quiescent and release its writer before forking, with an explicit explanation. Supporting uninterrupted live forks requires a backend immutable snapshot/committed-boundary contract and is not something to infer from concurrent reads. Fork/worktree results retain the source and destination keys through every background step and must not switch focus if the user has navigated elsewhere.

### Working directories

Independent sessions do not imply isolated files. Allow conversations in the same working directory, but clearly indicate shared-workspace execution and make the existing worktree path discoverable. Do not automatically create branches or claim conflicting edits are isolated. Key watcher notices, file-picker indexes, workspace snapshots, diff reads, and launcher actions to the correct directory and session request; selecting a different project must not retarget a pending operation. Share watchers by directory where practical, with dirty invalidation from any relevant session's tool completion.

### Persistence

Evolve `SavedViews` with a versioned migration from path-keyed version 1. Preserve existing paths and map the legacy `None` draft to one stable draft identity; support multiple draft records thereafter. Persist the durable session/draft identity and existing view data, not runtime handles or a misleading “still running” state. Bind a draft record to its real journal once known without losing its view or allowing out-of-order saves to resurrect the provisional entry.

Persist attachment descriptors referencing the existing prepared-attachment storage, not native images or encoded payloads in preferences. On restart, validate referenced files off-thread and show unavailable attachments explicitly. Restore drafts and reading arrangements without automatically resubmitting prompts or restarting inference. Unsupported or damaged saved state must retain the current protection against destructive overwrite.

## Implementation sequence

1. **Validate backend contracts.** Trace the vendored RPC server's session ownership, journal exclusion, fork source access, credential-store locking, and descendant shutdown. Exercise two real helper processes with separate journals and a shared data directory using a temporary workspace and controlled inference endpoint. Record which contracts are safe and fix any required backend support before enabling concurrent writing. Verify that the same journal cannot be opened twice for execution.
2. **Extract explicit session state.** Add the registry and stable keys; move singleton execution/composer state into entries while retaining one-session behavior. Reuse `SessionDesk` and `ViewState`, split attachment data from controls, and make event reduction owner-specific. Migrate all command callers, including menus, quick actions, account UI, developer settings, project actions, and preview/mock paths. Keep no fallback that resolves a missing target to the selected session.
3. **Bind runtime lifetimes to entries.** Accept a complete fresh/resume/fork launch configuration at runtime creation, avoiding an unnecessary fresh child before resume. Add event and command generations, same-session restart transitions, registry deduplication, and asynchronous retirement. Replace New/Resume/Fork/OpenProject commands that repurpose a runtime with registry-level creation or selection; retain only genuinely session-local runtime operations.
4. **Enable concurrent execution and navigation.** Add fair polling and aggregate activity scheduling, then remove the navigation busy guard. Make history browsing runtime-free and attach/resume on submission. Derive controls and sidebar states from entries, preserve foreground focus and reading anchors, and route all background completions explicitly. Keep primary and delegated activity visible while their session is hidden.
5. **Complete shared-service and durability integration.** Migrate saved views and attachment descriptors, merge discovery safely, route workspace services by directory, coordinate account operations, and update shutdown to include every live and retiring entry. Implement the safe fork policy established in step 1.
6. **Verify and clean up.** Run the behavioral and native scenarios below, then remove obsolete singleton fields, selected-session execution aliases, stale resume-settle logic, and single-runtime tests/comments. Update the product specification and applicable handoff/release notes only after the behavior is proven. Each implementation stage must leave the one-session path operational; the feature is complete only after the full concurrency matrix passes.

## Verification and acceptance

### Automated behavior

Extend existing Rust tests near the owning modules and run `cargo test`. Test interleaved events for two and three sessions, identical tool/subagent IDs across sessions, failures before readiness, stale generations after restart, wrong-session `Ready`, repeated selection of one journal, and simultaneous fresh drafts. Assert exact prompt/steer/abort targets and that an accepted prompt is never automatically replayed after a disconnect.

Test history/live-event reconciliation, discovery during startup and streaming, late attachment/evidence/file-picker/worktree results after navigation, attachment removal during preparation, and persistence migration with a delayed ordered writer. Include draft-to-journal binding, unsupported saved versions, missing attachment files, and quit before initial view loading finishes.

Test fair polling with one noisy producer and two quiet ones, checking bounded total drain time and eventual delivery for each. Verify that background events never mutate selected presentation state and that hidden output does not trigger native transcript rendering. Extend deterministic mock-runtime coverage to create independent sessions with configurable delayed startup, output, completion, and failure; keep mocks behind the existing backend rather than adding a production simulation path.

### Real runtime smoke

Add a concurrency smoke scenario using the actual built Omperor Dev app and bundled OMP helpers in isolated test data, not only reducer tests. Run two overlapping turns, start a third session, and record session IDs, helper PIDs, command targets, arrival times, and journal outputs. Prove overlap from runtime timestamps rather than from two spinners. Confirm killing one test helper fails only its session and leaves the others progressing; explicitly resume the failed session without duplicate submission.

Extend `scripts/test-app-lifecycle.sh` to cover several children in starting, running, ready, and retiring states, including delegated children. Verify app exit and absence of surviving owned descendants after quit, including rapid startup/quit races. Preserve the script's development-bundle checks and do not terminate unrelated Omperor or OMP processes. Reuse `scripts/profile-omp-lifecycle.sh` and existing performance instrumentation where appropriate, adding session key and runtime generation to diagnostic events.

### Native acceptance matrix

| Scenario | Required result |
| --- | --- |
| A streams; select and send in B; create and send in C | All three turns overlap; selecting a session does not replace another helper. |
| Browse owned history and external/TUI history while A runs | Browsing remains responsive, creates no unnecessary execution child, and leaves A untouched. |
| Prepare attachments in A, switch to B, then return | A retains its preparation/results and draft; B receives none of A's payload. |
| Stop B while A and C run | Only B is interrupted; its outcome is recorded correctly and the others continue. |
| A finishes or fails while B has focus | A gets the correct attention state; B's text, focus, model/account controls, and reading position remain unchanged. |
| Switch between different projects and shared-directory sessions | Diff, file picker, editor/terminal actions, and worktree results stay bound to their original scope. |
| A delegates work, then its primary turn settles | Its children remain tracked; the runtime is not evicted merely because the primary turn ended. |
| Account change/login completes after navigation | Only the intended session pin changes; global account state refreshes without retargeting execution. |
| Quit with concurrent work and unsaved drafts, then reopen | Owned processes are reaped, drafts and reading state restore, and inference does not restart automatically. |
| Several sessions emit large output while the user types and scrolls | Input and scrolling remain responsive; aggregate drain stays bounded and no session starves. |

Exercise the actual optimized native app in light and dark appearances at ordinary and minimum window sizes. Compare one-session idle behavior and three-session activity with the existing frame-budget diagnostics; record measured regressions and queue growth rather than claiming a performance improvement from unit tests. If native automation is unavailable, report the unverified visual scenarios explicitly rather than treating mock or reducer tests as visual proof.

## Completion criteria and investigation record

The change is complete when independent sessions execute concurrently, every command and asynchronous result has an explicit owner, navigation preserves retained work, all owned children shut down without blocking AppKit, and the native acceptance matrix and real-helper smoke pass. Do not ship a partial implementation that merely removes the busy guard or adds a runtime map while leaving singleton event application intact.

This planning task inspected the source and existing lifecycle scripts; it did not modify application behavior or run the proposed concurrency scenarios. Runtime overlap, shared-store safety, cross-process journal exclusion, and native performance are implementation verification gates, not established results of this document.

The runtime investigation attempted `cargo test omp_runtime::tests -- --nocapture`. `cargo` was absent from the shell PATH; invoking `~/.cargo/bin/cargo` reached dependency resolution but failed with “no matching package named `omp-journal` found” at the vendored journal path. No test pass is claimed. Resolve the vendored dependency/build prerequisite before executing the implementation verification plan.


### Native controller verification, 2026-09-21

The opt-in `--concurrency-smoke` scenario runs the optimized native AppKit application with the existing deterministic mock backend. It starts three independent runtime workers, records actual turn-start timestamps and distinct session IDs, applies a background delta while checking the selected NSTextView is unchanged, preserves drafts across navigation, checks a dormant draft has no runtime, invokes the background sidebar Stop action while another session is selected, and verifies the other two turns remain running. AppKit termination asserts every owned runtime worker has been reaped.

Reproduction: `cargo build --release`, then launch `target/release/omperor --concurrency-smoke` with an isolated `HOME`. The 2026-09-21 run passed (251 Rust tests also passed). `/tmp/native-concurrency-smoke6.txt` records starts at 1790046193562/1790046193563/1790046193563 ms and `shutdown=all-workers-reaped`. This is native controller/mock-runtime evidence, not proof of concurrent real-helper inference, child-process journal isolation, or the complete light/dark/minimum-size visual matrix. The run measured 10 of 25 main-thread passes above 4 ms, worst 108332 µs; it establishes no performance improvement.

The signed optimized development bundle was also built with `scripts/build-app.sh` and launched directly from `target/Omperor Dev.app/Contents/MacOS/omperor --concurrency-smoke` with a fresh isolated `HOME`. `/tmp/native-concurrency-bundle-smoke.txt` records all scenarios passing, including two native sidebar selections of the same dormant draft without creating another owner, contextual Stop targeting an unselected session, and all workers reaped. That bundle run measured 8/40 passes over 4 ms, worst 122648 µs.

## Activity indicators and sidebar folding (2026-09-22)

The selected session's running spinner and status label now sit below the composer, replacing the send-key hint while work is active. Startup, input preparation, primary work, delegated work, and stopping are derived from that session's retained state. Generic local background jobs use the same footer when the session is idle. The header keeps its static status dot. Switching to an idle session hides its running indicator without affecting other sessions' sidebar spinners.

Each project initially shows five sessions in its existing order, plus any selected or running sessions beyond that fold. **Show more (N)** expands the remaining sessions; **Show less** restores the abbreviated list. Search ignores the fold. Expansion is local view state for this app run and does not rewrite session ordering. Dragging a visible row preserves hidden siblings' relative ordering; order persistence runs on the background preference writer. Sidebar refreshes preserve the reading position, while a changed search or source resets to the top.

Validation: 276 tests passed (2 ignored). The native `--sidebar-activity-smoke` fixture verifies three simultaneous sidebar indicators, composer-relative spinner geometry, and idle-selection behavior without loading user sessions or starting inference. Manual AppKit checks covered expansion, collapse, search below the fold, and continued background indicators after selecting an idle session.
