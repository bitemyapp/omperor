# Observatory: agents, preview tabs, and transcripts

Status: integrated with the production OMP runtime on `codex/interface-redesign`, revised 2026-09-17. The Observatory uses a tabbed view of the harness's actual transcripts. The isolated design fixture remains available for visual testing.

## Product model

The Observatory is a browser for agents in the current session. Clicking a roster row opens a preview tab. Opening another agent replaces that preview; double-clicking a row or tab makes it an ordinary tab that stays until closed. Preview titles are italic. The tab's context menu and View → Keep Agent Tab Open (Command Return) provide explicit and keyboard-accessible equivalents. Tabs, drafts, filters, expanded tool outputs, text selections, and reading positions are saved per session across selections, session switches, and normal application restarts. OMP journals remain the source of transcript content; the preferences database stores only view metadata and unsent drafts.

The current interface supports four ordinary tabs and one preview. Ordinary tabs are never silently evicted. Comparison displays the ordinary tabs' transcripts together, retaining their independent reading positions. Selecting a tab returns to a single reader beside the roster. The roster can also occupy the full area for swarm scanning. There is no per-agent fullscreen button or separate activity-stage navigator.

## What OMP actually supplies

The authoritative source for this iteration is the version of OMP vendored in this checkout, rather than assumptions about other harnesses or future protocol versions.

| Observable data | Source in this checkout | UI meaning |
| --- | --- | --- |
| Agent ID, display order, class, description, lifecycle state, start/duration, error, workspace warning, journal path | `vendor/oh-my-pi/crates/app/src/rpc_mode.rs`, `rpc_subagents`; `src/omp_runtime.rs`, `SubagentInfo` | Roster identity, status, elapsed time, and explicit notices |
| Snapshot/progress subscription | `vendor/oh-my-pi/crates/rpc/src/protocol.rs`, `SubagentSnapshot`; `src/omp_runtime.rs` | Authoritative lifecycle updates, separate from what the transcript happens to say |
| Chronological user messages, assistant text, tool calls/results, selected event notices | `src/journal.rs`, `fold`; OMP `.oms` journal records | Native transcript rows in journal order |
| Tool name, descriptive summary, available detailed payload, failure flag | `src/session.rs`, `SessionMessage`; `src/journal.rs`, `finalize_tool` | Inline disclosure of recorded tool detail; explicit failure when recorded |
| Incremental subagent transcript RPC | `vendor/oh-my-pi/crates/rpc/src/protocol.rs`, `SubagentMessages` | Available backend capability; this app currently uses worker-side reads of local journals |

The journal is not an assignment → tool → response state machine. It can contain repeated prompts, interleaved tools, interruptions, empty or partial assistant turns, and tool-only work. A first user message need not be a complete task specification, and the latest assistant message need not be a final result. The UI therefore says Prompt and Agent response, and keeps them in sequence rather than presenting them as universal stages or aligning them as equivalent work across agents.

Omperor's folded message representation is smaller than the underlying journal. In particular it does not preserve stable entry IDs, every journal event, all tool lifecycle states, or a dependency graph. A non-failed tool row alone does not establish that the call completed successfully; the UI does not add a success checkmark on that basis. Displayed message numbers address the current folded transcript, not durable event IDs. A remote execution target, cost, dependency edge, or acknowledgement must not be fabricated from this data.

## Reading and comparison

The transcript uses the same native message renderer as the main conversation. Prompt and assistant text retain their normal typesetting. Tools appear in order as compact disclosure rows, expanding their recorded payload inline without a modal. The identity, lifecycle state, and any task notice remain attached to the reader. Missing journals show an explicit unavailable state.

A bounded chronological page shows at most eight messages and 12 KB of source body text. Earlier and later controls traverse the entire transcript, including oversized messages at UTF-8 boundaries. Latest returns to the newest page. Reading older content anchors the view so newly appended messages do not move the text being read. Tool expansion also anchors the current page. Scroll positions, text selection, expanded tools, roster filters, tabs, and the active arrangement belong to the session.

Compare retains independent native transcript readers; it does not infer phases or synchronize unrelated records. Narrow windows show one selected reader at a time with named tabs. Discuss with lead creates an editable draft containing bounded, source-labelled transcript excerpts and task notices. Nothing is sent automatically. The next useful evolution is deliberate selection of exact messages/artifacts with durable source IDs, once the backend and journal projection preserve those IDs.

## Why this is simpler

Selecting an agent, preserving a tab, reading its transcript, and comparing open transcripts are familiar and distinct operations. The removed design asked people to understand both a novel pin object and an invented taxonomy of agent work. The revised design spends the available space on the recorded conversation, exposes tool detail progressively, and lets the harness supply the structure.

The inspiration remains durable context in [Herdr](https://herdr.dev/docs/agents/), low-cost peeking in [Amp](https://ampcode.com/news/agents-panel), and source-associated attention in [cmux](https://github.com/manaflow-ai/cmux/blob/main/docs/notifications.md). The user's preview-tab analogy adds a better retention model. These are design influences rather than a claim that the prototype exceeds those products in implemented capability.

## Implementation boundaries

Roster rows remain virtualized. Journal reads and folding stay on background workers. Rendering is bounded to the visible readers and cached by content, width, typography, and disclosure state. Tabs reuse existing native surfaces; browsing does not launch an agent. Inspection adds no animated activity loop, network call, or main-thread journal read. Durable view saves use one-shot coalescing timers. The design fixture runs no inference and cannot prove behavior under production streaming load.

The most recent 24 replaced previews retain reading state without duplicating transcript bodies. The four ordinary tabs are outside that recent-preview limit. Durable journal-entry anchors, richer remote execution data, and controls for live workers remain backend work. Reading anchors currently refer to the folded journal’s message index and byte offset; an OMP rewrite that changes earlier message ordering can invalidate that coordinate. Ordinary appends preserve it.

## Validation of this iteration

The optimized native preview was checked at 1440 × 900 and 1080 × 720 in dark and light appearances. Checks covered replacing a preview without closing ordinary tabs, double-clicking both roster rows and tabs, Command Return, inline tool expansion, restoring expanded output after a session switch, independent comparison readers, narrow comparison tab switching, and the missing-transcript state. The 181-test suite covers transcript ordering, arbitrary tool names, interleaved prompts and interruptions, bounded Unicode paging without lost text, tab retention, reading anchors, and roster/layout behavior. Production compilation and the existing lint configuration pass. These checks validate the fixture and model behavior; they do not constitute a live-inference or streaming-load test.

## Production integration

The approved interface uses the existing single-primary OMP runtime. Agent identities and lifecycle states come from its subagent snapshot/progress events. Journals are read on background workers, including explicit reads beyond the preload threshold. Errors retain the last successfully loaded transcript and expose a retry through Latest. Read completions carry a monotonic start time so an older request cannot replace a newer cache entry. Loading placeholders never capture or overwrite transcript reading positions.

Version 1 session-view metadata lives in the existing preferences database. Writes are coalesced during interaction, serialized by the preferences worker, and flushed before normal termination. Unsupported or damaged state is reported and left intact. A fresh session’s provisional view is saved under the actual journal path reported by OMP once it is known. Existing OMP journals and account credentials are not rewritten by view persistence.

Startup filesystem and registry reads, account synchronization, and new-project folder registration run on workers. Visible Observatory clocks update once per second without rebuilding their rows. Runtime polling slows to two checks per second while idle; lifecycle events wake it immediately, while active streaming remains frame-batched with a four-millisecond event-drain budget. Finished transcript tabs use the slow journal-check cadence. These scheduling changes concern Omperor’s GUI process, not the independent power consumption of an inference provider or OMP workers.

Switching the primary session during an active turn remains unavailable on this runtime. Stop the turn before switching; concurrent primary sessions, remote workers, and phone access are separate backend work agreed outside this pass.

## Production verification, September 17

The signed, optimized app launched with its bundled OMP helper and completed a bounded real conversation. It resumed an existing session and read its actual successful and failed subagent journals. Native checks covered preview and kept tabs, independent comparison, earlier message pages, inline tool output, attention filtering, and source-labelled transfer into an editable unsent draft. A normal quit and relaunch preserved a Unicode draft, both kept tabs, comparison layout, the selected transcript page, and expanded tool output. No new delegated workload was launched during this check.

All 187 tests pass. New file-backed tests cover saved views alongside unrelated preference edits, malformed state, journals larger than the preload limit, continued updates to explicitly inspected journals, failed reads, and stale completion ordering. The all-target/all-feature Clippy check passes with the repository’s existing lint exceptions. See [production readiness review](../CODE-CLEANUP.md) for coverage, resource measurements, and remaining inherited paths.
