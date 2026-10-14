# Production readiness review

## Scope

- Base: `main`, merge-base `69808107fa358b25989c5bb227e8566ca014ea37`.
- Starting head: `eae9f24` on `codex/interface-redesign`.
- Mode: audit and fix. Review the approved native interface against real OMP sessions, preserve the isolated visual fixture, and verify the production build separately.

## Findings and work plan

### High: Make transcript loading explicit and recoverable

- Evidence: `src/session_workspace.rs::ensure_workspace_evidence`, `src/main.rs::apply_transcript_updates`, `src/transcripts.rs::sync`.
- Concern: Open journals larger than the preload limit stop refreshing, background read errors disappear, direct reads omit their actual file stamp, and placeholders can survive successful empty reads. A cache that only prunes departed agents can also grow beyond its advertised bound.
- Cleanup: Share stamped worker reads, report failures without discarding the last successful transcript, prioritize inspected agents, bound the retained cache, and keep retry independent of stale content.
- Verification: File-backed journal tests covering updates, errors, large-file planning, cache limits, and stale completions; native loading and retry checks.

### High: Persist session reading state and drafts

- Evidence: `src/workspace_model.rs::SessionDesk`, `src/agent_workspace.rs::agent_view`, `src/session_workspace.rs::capture_session_desk`.
- Concern: Filters, tabs, drafts, and reading positions disappear at process exit, despite appearing durable in the interface. State currently uses a provisional path key before a session acquires a journal.
- Cleanup: Versioned, typed state saved by an ordered worker, with bounded metadata and no duplicate transcript bodies. Retain session identity across the first live journal announcement. Save on session transitions and normal quit, and coalesce updates during interaction.
- Verification: Round-trip and malformed-state tests, Unicode selection handling, session identity transition tests, and an actual quit/relaunch check.

### High: Verify production lifecycle and session switching

- Evidence: `src/main.rs::open_sidebar_session`, `src/omp_runtime.rs::OmpRuntime`, `docs/PRODUCT_DIRECTION.md`.
- Concern: The visual fixture bypasses authentication, resume, streaming, real subagent events, and shutdown. The existing runtime owns a single primary session and rejects switching during a turn.
- Cleanup: Exercise the real runtime and its worker events; make ownership and limitations explicit. The owner chose to finish the approved UI on the current runtime first; concurrent primary sessions are deferred.
- Verification: Real production launch, bounded primary inference, existing live journal inspection, session return, failure handling, and clean shutdown.

### High: Keep storage and runtime work off the main thread

- Evidence: `src/main.rs::start_background_services`, project/account actions, `tests/artifacts/perf/20260916-observatory/REPORT.md`.
- Concern: The production path still does synchronous preference/registry reads and some project filesystem/SQLite operations. The preview's idle measurements do not establish production efficiency.
- Cleanup: Use worker results for initialization and touched storage paths, coalesce updates, and measure the actual optimized production app.
- Verification: Main-thread call-path review, worker tests, native responsiveness, and production idle/resource measurements.

## Review coverage

Reviewed the branch delta by subsystem: native layout and theme; typography model, renderer, examples, and settings; sidebar tree and quick inspection; Observatory tab/roster/reader model and native controls; transcript workers and runtime events; startup, preferences, and view-state persistence; worker wakeups and profiling; the isolated fixture, preview/build instructions, Markdown snapshot example, and committed performance artifacts. Existing tests were retained. The cross-cutting review checked feature gating, worker ownership, stale results, cache bounds, SQLite write ordering, native UTF-16 coordinates, normal termination, and the boundary between recorded data and inferred activity.

The production work adds no remote-execution or multi-primary controls. Remote execution, phone transport, dynamically injected metrics tools, and personal-agent scheduling remain roadmap items. Older design notes and performance reports describe historical checkpoints; the current harness contract and verification are in `docs/AGENT_INSPECTION_DESIGN.md`.

## Implementation notes

- Fixed late-read ordering using monotonic read start times, independent of filesystem timestamp granularity. Direct inspection and warming now share the same stamped read result.
- Added versioned view persistence, ordered writes, normal-quit draining, malformed-state preservation, and provisional-session identity migration. A loading placeholder cannot replace a saved transcript reading position.
- Moved production startup reads and account synchronization off AppKit. New project resolves/registers its directory on a worker and enters through the same state-preserving transition as New conversation. A late folder result cannot hijack another session or interrupt a turn.
- Removed the permanent 30 Hz idle runtime poll. Lifecycle events wake the timer; active streaming remains batched. Only visible running Observatory clocks tick, without rematerializing rows.
- Made the four-kept-tab limit explicit for tab double-clicks as well as menu actions. Inspection remains possible in the separate preview slot.

## Remaining inherited paths

The main application still contains older synchronous project-management operations outside the Observatory, including first-time registry seeding and some Settings folder edits. The new inspection, persistence, startup, account synchronization, and New project paths do not perform journal or database I/O on AppKit. This pass does not establish a blanket main-thread guarantee for every inherited feature.

## Verification results

- All 187 tests pass with `cargo test --locked --offline --all-features --quiet`. File-backed tests cover persistence and unrelated preference edits, malformed/versioned data, large inspected journals, append refresh, read failures, and monotonic stale-result rejection.
- All-target/all-feature Clippy passes with the repository's seven existing lint exceptions. No source-level lint suppressions were added. `git diff --check` passes.
- The ordinary optimized application was packaged and signed with its bundled OMP helper. A disposable repository hosted one bounded, tool-free primary inference, which returned the requested text. No source files were changed by that inference.
- A real existing two-agent session exercised successful and failed journal output, ephemeral/kept tabs, comparison, inline tool expansion, earlier pages, attention/empty filters, and editable source-labelled excerpts. The verification excerpt was cleared without sending it.
- Normal quit/relaunch preserved a Unicode draft, two kept tabs, comparison layout, an earlier transcript page, and expanded tool output. The saved-view path contains no duplicated transcript bodies.
- A 20-second production idle sample measured 5.13 ms of GUI CPU, 85 interrupt wakeups, and zero package-idle wakeups. See `tests/artifacts/perf/20260917-production/REPORT.md` and its raw samples. This does not establish battery-runtime savings.
- Fixed two issues found during final integration: a session no longer inherits another session's speed receipt, and text-size edits immediately refresh open agent readers.

## Practical limits

The current OMP runtime owns one primary session. Session switching is still unavailable during an active primary turn, as agreed for this pass. No new multi-agent workload was launched for verification; running-subagent scale is covered by the deterministic native fixture and model tests, not a production swarm benchmark. Full VoiceOver traversal, blocked-storage fault injection, remote filesystems, and sustained streaming-load profiling remain unverified. The older synchronous project-management paths below the redesigned surfaces remain identified above; this review does not claim the entire application satisfies the main-thread rule.
