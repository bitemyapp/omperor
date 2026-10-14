# Interface redesign

The current implementation and production verification are documented in [Agent inspection design](AGENT_INSPECTION_DESIGN.md) and [Production readiness review](../CODE-CLEANUP.md). The dated iteration notes below preserve earlier design decisions; their prototype limitations describe those checkpoints.

This branch explores a conversation-first direction for Omperor. It starts from `6980810`, independently of the typography work in the original checkout.

## Design decisions

- **Conversation and activity together.** Navigation is 248 points wide and the activity pane starts at 352 points. Expand makes room for inspecting work while retaining at least 440 points for conversation, including at the minimum window size. Focus (⌘⇧I) hides activity; ⌘1 and ⌘2 open Changes and Agents.
- **A connected task tree.** The primary agent and delegated tasks form a compact tree with explicit states, distinct symbols, task roles, aligned elapsed times, and nearby warnings. Titles use native text measurement and wrap only when needed. No large task cards or Command framing.
- **Progressive controls in the session.** All, Active, and Attention are lenses over the same workload. Search appears for eight or more tasks and can be revealed at any size. Finished work can collapse; searching or choosing Attention reveals matching work even if it was collapsed. Search, density, selected task, filter, and pagination are retained per session while the app runs.
- **Useful density.** Comfortable rows show warning excerpts. Compact rows tighten spacing while retaining readable font sizes. When compact activity has enough width, it becomes a single-line tree with aligned State, Role, and Elapsed columns. Narrowing the window naturally returns to two-line rows. The sidebar is paginated into at most 40 native task controls, with paging controls above the list; the Observatory uses continuous scrolling with only visible rows materialized.
- **Evidence without interruption.** Selecting a task opens a native inspection drawer beside the conversation. It shows available metadata, warnings, and a bounded excerpt of the latest recorded output, with Pin in Observatory one action away. The Observatory reads recorded history in place; the old modal agent-transcript path has been removed. Missing journals have a clear explanation. Reads run on workers with an active indicator and stale-response protection.
- **Coherent themes.** Dark mode retains plum navigation and restrained gold selection. Both side panels now share the same adaptive plum surface, removing the subtle left/right color mismatch in light and dark appearances.
- **Preserved reading and input improvements.** The composer keeps aligned native model/reasoning/account controls, and all 17 chat typography contexts remain adjustable with live native examples. Quick Actions (⌘K) retains searchable keyboard navigation.

See [Product direction](PRODUCT_DIRECTION.md) for the personas, progressive revelation, metrics-tool plans, personal-agent direction, and distributed-execution requirements.

## Review the design

```sh
cd ~/work/omperor-redesign   # a checkout of the redesign branch
SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk scripts/preview-design.sh --release
```

The SDK override is needed for this machine's current toolchain, as documented in the existing handoff. Use the normal build without an override on a compatible SDK.

Use `--release` when judging responsiveness. It uses the production Cargo release profile, including full optimization and thin link-time optimization. Omitting it builds the faster-to-compile debug preview. Put `--release` before preview arguments such as `--light`.

The preview builds the real views with deterministic sample content. It has its own app identity, starts no OMP process, and isolates any settings changes under the temporary `omperor-design-preview` directory. The `design-preview` feature is opt-in; ordinary builds retain normal inference and session behavior. Quit the preview before rebuilding it. `--light` starts it in light appearance; appearance and text-size settings also work during review.

Try:

1. Start with **Build the next release**, a three-task sample. Select the review warning, inspect it beside the conversation, then choose Inspect in Observatory and read earlier records without a modal.
2. Open **Inspect a parallel workload**, the 84-task fixture. Its first visit opens Observatory with two illustrative findings pinned. Read an earlier tool record in one pane and scroll the other independently. The main sidebar remains available throughout.
3. Search the named agent lists. Scroll Running and Finished independently. Try Compact rows and the local Overview / Inspect / Compare selector to move between full-height browsing, quick inspection, and comparison, or use the header’s Agents button to open the full task tree. Collapse finished work in the tree, then choose Attention to reveal affected tasks. **Agent events · Review** demonstrates a running task that has not published a journal.
4. Leave a draft, switch sessions, and return. The surface, draft, pinned records, independent scroll positions, text selections, filter, search, list pages, and task selection are retained across session switches and normal restarts. The current runtime still blocks switching while a primary turn is active; the fixture does not run inference.
5. Use ⌘3 for Conversation and ⌘4 for Observatory. Observatory gives the reading area to evidence and retains the composer draft. Resize down to 1080 × 720 and open the named tree: evidence panes stack vertically as the reading area narrows. Resizing preserves the reading position. Session Timeline is available at ⌘5.
6. Compare Light and Dark in Settings, then review Chat text and its native examples.
7. Toggle Focus with ⌘⇧I; use ⌘1 and ⌘2 to navigate between Changes and Agents. Select a sample file to view the native diff.
8. Choose Discuss with lead. The app appends bounded, quoted excerpts with task IDs and record numbers to the existing draft. Review or edit the prompt; nothing is sent automatically. Open ⌘K and search “observatory” or “conversation” to move between surfaces.

## Calibrate chat typography

Open **Settings → Chat text** (⌘,). Seventeen contexts have independent sizes: assistant replies, your messages, four heading levels/groups, inline code, code blocks, code labels, speaker names, activity, thinking, notices, message input, composer controls, the conversation title, and status/hints.

- Type a point size and press Return or Tab, or use the half-point steppers. The open conversation updates immediately; the composer grows vertically for larger input text.
- **Default** restores a context's default. **Reset all to defaults** clears every override. Default transcript sizes follow the style and text scale under Appearance; composer/header defaults use the interface's native sizing.
- Custom sizes are absolute points and stay fixed when style or scale changes. Labels show both the current value and the default value for comparison.
- Each setting has an adjacent live example at actual point size. Transcript examples use the same native Markdown renderer; composer and header examples share the real AppKit controls, font selection, and layout helpers. They contain fixed sample text with no delegates, actions, links, image loading, or RPC. TextKit measures wrapped content so examples grow with the text, while the current row stays in place. Appearance and style changes update the examples.
- The ordinary app and isolated preview each save their settings across restarts. Writes run on one background worker in order; Settings shows when a save is pending or failed, and normal quit lets pending saves finish without blocking the UI thread.

Modern body text defaults to 15 pt, with 23/20/17/15 pt headings; Editorial uses 15.5 pt body text and 24/21/18/15.5 pt headings. Chat, agent transcripts, and the Settings examples share these defaults. Terminal retains its compact 14 pt body size, and explicit per-context overrides remain unchanged.

Defaults and validated overrides live in `src/chat_typography.rs`. The native controls live in `src/typography_settings.rs`; sizes persist in the existing preferences table, so older databases require no schema migration. `src/typography_examples.rs` owns the isolated native examples.

## Implementation and verification

`src/layout.rs` holds the pure reading-column and composer geometry. Regression tests cover extreme label widths, narrow windows, separation of controls, and the reserved action area. AppKit autoresizing is disabled on controls whose frames this layout owns; applying both had caused the composer to shrink twice during live resizing.

The file reference picker follows the composer's actual frame. The redesign adds no production filesystem walks, subprocesses, network requests, or RPC calls on the main thread. Quick Actions filters a fixed in-memory command list, and agent transcript loading stays on the existing worker path. Existing discovery and runtime workers continue independently of workbench visibility.

Validation:

- All 175 unit/integration tests pass, including persisted overrides, validation, and actual native font attributes for each transcript context.
- Native preview checked in light and dark appearances, with activity shown/hidden and at the minimum window size.
- Chat text controls checked for typed input, half-point adjustments, invalid values, reset, persistence after relaunch, and large text with a narrow workbench layout.
- In-situ examples checked at maximum body size, after window resizing, and across Modern/Editorial and light/dark appearances.
- Settings and native Markdown rendering inspected visually; the existing snapshot example exercises headings, lists, quotes, inline code, fenced code, and tables.
- Clippy passes with seven existing lint classes allowed on the command line. A completely strict run remains blocked by pre-existing digit grouping, type complexity, redundant conversion, constant assertion, nested-if, modulo, and boolean-assert warnings. No lint suppressions were added to source.

Live authenticated inference was not exercised during this visual review. The preview is sample content, not a recorded model run. The full VoiceOver navigation experience has not been audited; accessibility-tree labels and native control behavior were checked.

### Composer checkpoint

The preview model, reasoning, account, and Fast controls now operate on isolated sample state through native menus. Sending remains disabled because the fixture does not run inference. Physical clicking of the model chip and Fast toggle was checked, alongside collision tests for long labels, narrow panes, and missing account controls.

### Activity workspace iteration

`src/agents.rs` owns the pure filtering, grouping, and bounded pagination logic. `src/agent_workspace.rs` owns the native task tree and session lenses; `src/agent_inspection.rs` owns the nonmodal evidence drawer. `src/quick_actions.rs` owns the keyboard launcher. The design fixture includes three-task and 84-task workloads, cached transcripts, a missing-journal case, and native diffs. These interactions never start inference or remote work.

Regression coverage includes attention for completed warnings and failures, multiword search across task/role/error fields, search through collapsed groups, stable nonduplicating pages, changing workload sizes, and minimum conversation width during expansion. The UI still consumes the current flat subagent board: nested delegation, execution hosts, agent token/cost metrics, batch control, and dependency scheduling need backend contracts described in the product specification. The sample workload is a design exercise rather than a scale benchmark.

### Agent Observatory iteration

`src/session_workspace.rs` owns the native workload overview, search, evidence panes, and surface transitions. `src/workspace_model.rs` holds per-session reading state, bounded record navigation, and explicit source transfer into a draft. The Running and Finished lists share search and scope semantics with the task tree, with independent continuous scrolling and no page-size cap. Their height grows with the window. The local Overview / Inspect / Compare selector allocates space between browsing and comparison. The agent lists live in a stationary native area, separate from the evidence scroller. Scrolling one list does not scroll the surrounding Observatory. Compact rows is a local, per-session density control. Comfortable rows show task name, explicit state, role, elapsed time, and pin action; compact rows retain the name, state, and pin label, plus elapsed time where width permits. Full metadata stays in native tooltips and accessibility labels. The arrangement selector stays visible in the footer, keeping both browsing and evidence within reach. Only visible rows and a small overscan region have native controls, keeping view construction bounded by the viewport rather than the workload size. Each list retains its first visible task and within-row offset across session switches, resizing, new arrivals, and density changes. Search or scope changes reset both lists to the first matching task. A selected pin has both a tinted background and a Pinned label. Long task names truncate deliberately and remain available in the native tooltip and accessibility label. The compact overview yields to the full agent browser while the Agents side panel is open, leaving the center for evidence instead of repeating filters and task lists. It returns when the panel closes. Evidence panes stack when the reading area narrows and use the available height. The overview no longer uses numbered glyphs or a repeated status legend. Up to four evidence panes are retained; adding a fifth never silently replaces an existing pin. Each pane can follow the latest recorded output or retain an earlier record. Long records remain readable in bounded UTF-8-safe segments, including full tool details. Both panels use the same adaptive surface; cyan identifies retained evidence, while task state remains explicit in text.

Reading is independent of inference. Journal reads run on workers. Background replies are checked against session identity, and pinned tasks remain in the journal warming plan even when they are outside the newest 32 tasks. Each retained snapshot is bounded; native rendering works on one small segment per open pane. Agent transcript modal code was removed. Existing tool-detail and Settings dialogs are outside this iteration.

The Observatory layout and drafts are retained in memory for the current launch, not written to disk. Live authenticated inference, concurrent primary runtimes, reconnect behavior, and remote execution were not exercised. These are deliberate boundaries of the prototype, not claims of completed orchestration infrastructure. The product specification records the stronger intended guarantee.

Regression tests also cover complete group membership with 10,000 synthetic agents, bounded visible-row windows with no gaps, shared search/attention semantics, and reading anchors across insertions, density changes, removals, and shrinking workloads. These are model tests, not a live 10,000-agent performance benchmark. The preview clock is fixed to its sample event time so a fixture left open overnight retains meaningful sample durations. Earlier regression tests cover idempotent pinning without eviction, long Unicode evidence with no gaps, stable historical records while newer records arrive, complete tool payloads with explicit failure labels, and bounded quoted prompts with provenance. Native interaction checks cover the draft/reading-position return trip, independent record and scroll state, keyboard surface switching, and editable evidence transfer without sending.

### Visual review for Observatory

The numbered glyph map was replaced with named Running and Finished lists. Native review exercised both appearances, 1440 × 900 and 1080 × 720 layouts, the three-task and 84-task fixtures, independent scrolling, compact and expanded arrangements, combined search/attention filtering, empty results, pin/unpin actions, and the layout with the full agent panel open. The preview uses actual AppKit controls and native text rendering; these checks assess composition and interaction, not live inference or distributed execution.

The preserved checkpoint `399a531` showed about eight agents at 1440 × 900 in each group in the balanced arrangement and twenty in expanded compact view. At 1080 × 720, expanded compact view shows thirteen per group. These counts are consequences of available height, not page limits. Native checks reached both list ends independently, preserved positions and density across session switches, reset filters to the first matching result, pinned an agent from the end of the finished list, and navigated between browsing and retained evidence using fixed shortcuts. Both themes and wrapped controls beside the Changes panel were inspected.

### Fixed-area scrolling experiment

The next iteration removes the outer scrolling page from the agent browser. Running and Finished are independently scrollable siblings of the evidence area, with fixed filters and headers. Agents uses the full browser height; Split keeps a compact evidence area below it; Pins uses the full height for comparison. Each arrangement retains the session’s reading state and draft. Selecting an agent from Agents reveals its evidence immediately. A narrow center temporarily uses a focused arrangement instead of squeezing all three areas into unusable strips. The evidence grid may still scroll when several card rows need space.

[Scrolling exploration](OBSERVATORY_SCROLL_EXPLORATION.md) compares the implemented approach with a single table, category tabs, and a side-by-side evidence column, and records remaining review questions. The approved checkpoint remains available unchanged as `399a531`.


### Agent-first inspection and comparison

The latest iteration replaces pin-on-click with immediate inspection. A transient browsing slot can move among agents while up to four deliberately kept agents remain available. Each inspector shows agent identity, task title, lifecycle state, attention notice, semantic record shortcuts, and nearby activity. The native record menu and previous/next controls retain access to the complete journal. Compare aligns actual assignments, last tool results, or responses, and explicitly reports an absent record. Discuss with lead copies the selected sources into an editable draft without sending it. The native sidebar drawer now opens the same inspection surface.

[Agent inspection design](AGENT_INSPECTION_DESIGN.md) records the source research, object model, visual hierarchy, prototype boundaries, and future opportunities. It supersedes the earlier pin interaction while preserving the fixed sibling scroll regions, native rendering, worker reads, and session-local context. The recent unkept-inspection history is bounded to 24 reading positions; kept agents remain retained. No new polling loop is introduced.

## Transcript tabs, September 16

The Observatory now follows OMP's chronological journal instead of imposing Assignment / Last tool / Response stages. Single-clicking an agent opens an italic preview tab; double-clicking its row or tab keeps that tab open. Tool output expands inline using the shared native renderer, comparisons keep each transcript independent, and the per-agent expand/fullscreen control is removed. The [inspection design](AGENT_INSPECTION_DESIGN.md) records the current harness contract and projection limits.

## Production integration, September 17

The normal release build now runs the approved interface against the bundled OMP runtime, with durable session views and drafts, recoverable journal read failures, stale-read protection, and worker-based startup/account storage. Real inference and existing subagent journals were exercised separately from the design fixture. Runtime polling slows while idle and wakes for lifecycle events. Per-context font changes refresh open agent readers immediately, and switching sessions clears the preceding session’s speed readout. The current single-primary-session constraint is unchanged by agreement.
