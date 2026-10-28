# Omperor product direction

Status: living product specification. Owner: Christopher Allen. Updated: 2026-09-17. This document records the product goals supplied by the owner, the design interpretation guiding the current prototype, and backend requirements that remain future work. Paragraphs intentionally have no internal line breaks so the document is easy to edit.

_Note, October 2026: Omperor is now a native Aizome app. The Agent Observatory described below is realized as the Agents panel beside the conversation (agents by state with a search and an attention lens, one opened with its recorded work, and Discuss quoting it into the draft); side-by-side comparison of several agents is not carried over yet. Per-context chat font sizes gave way to the desktop's own Size setting and one text-size choice for conversations._

## Purpose

Omperor is ChatGPT for power users and oh-my-pi for the rest of us: a beautiful, native, readable environment for working with capable agents, from one conversation to large distributed workloads. Its advantage should be greater productivity, efficiency, control, and legibility without requiring users to become terminal multiplexer operators. The emperor pun is intentional; the interface should communicate capability through useful information and direct control, not military language or a command-and-conquer metaphor.

Conversation is the starting point and remains the center of gravity. Agent activity belongs beside it when useful. Users should always be able to understand what they asked for, what is happening, what needs their judgment, and what happened as a result. Make the app empowering—ompowering—through clarity and capability rather than decorative complexity.

## Personas and reference experiences

### Everyday conversation and occasional delegation

The common user runs one to three sessions concurrently. Most sessions are conversations with one primary agent. That agent occasionally delegates bounded subtasks to finish sooner, conserve tokens, or bring in an independent perspective. ChatGPT for Mac and Claude Desktop are the anchors for readability, composure, approachable defaults, and low ceremony. Opening a conversation must not require configuring an orchestration system.

### Large-scale agent orchestration

Jeffrey Emanuel, also known as doodlestein and Dicklesworthstone, represents the orchestration-intensive user. This user may operate many agents locally, on remote hosts over SSH, in Kubernetes, or across mixed execution targets within the same session, workload, task, or goal. Reference experiences include herdr, ntm, cmux, and Superlogical (Mitchell Hashimoto and team). Omperor should exceed multiplexer-oriented tools in productivity, efficiency, control, and legibility while retaining their practical information density and keyboard fluency.

Scale should increase the usefulness of grouping, search, attention queues, task relationships, and selective detail. It should not force every agent into an equally prominent terminal or large card. Preserve direct access to transcripts, artifacts, execution details, and raw evidence when a user needs them.

### Personal agent and guardian angel

A tertiary persona uses a persistent personal agent, with Hermes as an anchor. The agent can help across tasks and time, remember appropriate context, and bring useful developments to the user. The interface should make continuity, delegated responsibilities, outstanding decisions, and the scope of authorized ongoing work understandable. Avoid turning this into an incessant notification feed. Specific memory, scheduling, and permission behavior remains to be designed.

## Progressive revelation, within the session

There is no global beginner/expert switch. Users should move fluidly between conversation, observing delegated work, inspecting evidence, and directing a large workload in the same session. Each step should expose a useful next capability without making users learn everything at once.

1. Start a conversation with a readable transcript and a straightforward composer.
2. When delegation occurs, reveal a compact connected list of the primary agent and its tasks, with explicit states and elapsed time.
3. Open one task to inspect its transcript, result, or failure. Show the relevant evidence with a clear route back.
4. Filter a busy workload, narrow it to work needing attention, or use a denser list without changing the session or hiding capabilities globally.
5. Expand the activity area when orchestration becomes the task at hand; return space to the conversation just as easily.
6. Introduce execution targets, resource budgets, dependencies, and batch controls in the context where they answer a real question. Explain prerequisites at the point of use.

Settings may calibrate typography, density defaults, and other preferences. They must not divide the product into categorical modes with separate feature sets. Contextual controls should retain their state for the relevant session where practical.

## Information and interaction principles

### A compact, connected activity tree

Re-anchor the agents pane on the original compact tree. The primary agent and delegated tasks should read as related work, with restrained connection lines, concise hierarchy, informative text, and clear selection. Favor a practical, slightly hackery character: crisp alignment, useful counts, readable timing and identifiers, and easy access to evidence. Avoid oversized cards, redundant headings, heroic empty space, or a decorative empire metaphor.

Every visible agent row should answer what the agent is doing, its actual state, and the most useful available context. Running, thinking, tool execution, blocked, idle, complete, failed, interrupted, and detached are different states. Completion with a warning must remain discoverable as needing attention. Never relabel a failure as merely ready or hide it in a success count.

Use words and, where helpful, distinct symbols as well as color. Color alone must never carry success, failure, running state, selection, or urgency. Show brief warnings near their task and provide the complete explanation on inspection. Keep the list stable while clocks update and preserve the reader's position during data updates.

### Conversation and evidence

Preserve comfortable reading measure, strong text contrast, coherent spacing, selectable native text, and a composer with aligned, working controls. Keep output, tool activity, reasoning, code, and metadata visually distinguishable without creating competing boxes around everything. Changes and task transcripts are evidence adjacent to the conversation; users should reach them without losing their place.

### Inspect instantly, resume exactly

Looking at information should be immediate, reversible, and additive. A click should reveal the most useful available evidence in place, without waiting for a presentation animation, dismissing another view, or taking control of the window. Escalate from a quick peek to a retained reading surface and then to deeper evidence in the same workspace. Prefer nonmodal inspection, independently scrollable panes, and explicit pinning. Reserve modal interactions for short decisions that truly require an answer before the initiating action can continue.

A session is a recoverable working arrangement, not just a transcript. Switching sessions, tasks, or projects should leave drafts, open evidence, selections, filters, reading positions, and layout intact. Returning should feel like returning to a desk that nobody touched. Background work may advance, but it must not reorder the user's thinking, steal focus, jump a reading position, or discard an unfinished message. Make new information discoverable without forcing the reader to follow it.

Progressive revelation must provide more room and depth, not merely squeeze more controls into the same small panel. Let a peek become a full reading surface, let several pieces of evidence remain visible for comparison, and let the user move that context into a conversation intentionally. A retained workspace should reduce the need to remember information between clicks.

### Evidence as working material

The interface calls the session’s agent visibility surface the **Agent Observatory** (Observatory in navigation). A project holds durable context and files; the Observatory shows agents and their recorded work within one session. Selecting an agent previews its transcript; double-clicking its open tab keeps it across subsequent selections. Native transcript rows follow the harness's recorded sequence, with inline tool disclosure rather than an assumed sequence of work stages. Open tabs can be compared side by side and their source-labelled excerpts brought into an editable draft for the primary agent. Inspection and tab retention are local reading actions. A saved immutable result is a separate future object that must preserve its own provenance; a following agent journal must not masquerade as one. See [the agent inspection design](AGENT_INSPECTION_DESIGN.md) for the current interaction and harness data contract.

This is a product hypothesis about the next useful step beyond process visibility: make the agent's observations, claims, uncertainty, and supporting records available as composable working material. Future capabilities may include comparing independent investigations, inspecting disagreement, tracing a conclusion to tool results, allocating a budget to resolve uncertainty, and asking an agent to explain the critical path. These require real provenance and runtime contracts; the UI must distinguish observations from inferred explanations and proposed actions.

### Visual identity and themes

The dark theme is the stronger reference from the current iteration. Preserve its calm surfaces, restrained plum and gold accents, and improved readability. The light theme should be a coherent light interpretation with a light navigation surface; avoid a permanently dark left rail that clashes with the rest of the window. Apply equivalent relationships among navigation, conversation, activity, inset controls, and selected rows in both themes.

Typography must work on high-density desktop displays without crowding the interface. Keep per-context chat font-size controls in Settings, use the term Default for resetting a static/default value, and show live, sandboxed examples built from the same native scalable elements as the real interface. Avoid screenshots as typography previews.

### Visual quality of design prototypes

A native design mock must meet a high visual standard even when inference and remote execution are simulated. Widgets need deliberate composition, clear controls, accurate alignment, intentional truncation, readable hierarchy, and finished visual treatments. Review the actual app in both appearances, at ordinary and minimum window sizes, and with small and large workloads before presenting an iteration.

Agents must be recognizable by task name without memorizing numbered glyphs or hovering for basic information. Expose state in words and a distinct symbol, retain a visible route to deeper evidence, and make pin selection obvious. Let the user allocate space between swarm browsing and retained evidence without leaving the session. A swarm surface must show materially more agents than the sidebar; a few rows behind repeated pagination is insufficient. Lists should grow with the window, scroll continuously and independently, and offer local density and expansion controls. Keep pinned evidence reachable when browsing takes priority. Avoid nesting the agent lists in a scrolling page: a gesture over an agent list should move that list, while filters, the other group, and retained evidence stay stationary. Use fixed sibling areas and local space-allocation controls to support browsing and reading without competing scroll targets. Nested scrolling inside retained evidence cards is less disruptive and can remain where needed. Show one agent browser at a time: when the full side panel is open, give the Observatory’s center to pinned evidence; when it closes, restore the compact overview. Running and finished work need independent access so a large active batch cannot crowd completed results out of view. Counts, empty states, visible ranges, timestamps, and selected controls must describe the data being shown accurately. Sample clocks should remain deterministic across long review sessions.

### Native behavior and responsiveness

Use the desktop's own text, menus, keyboard navigation, focus behavior, accessibility labels, and scalable drawing, through Aizome's toolkit. Important controls should be discoverable with the mouse and efficient from the keyboard. Keep the interface thread free of non-trivial computation, filesystem work, subprocesses, network calls, and OMP interactions. Background operations must show an active state and deliver results asynchronously.

## The bitter lesson: tools that let agents reason

Prefer general capabilities, accessible data, and composable tools over a growing pile of fixed product heuristics. Omperor should help the agent investigate, explain, and act using evidence, while the user retains clear scope and control. Built-in summaries are useful for orientation; they should not become the only interpretation of the underlying data.

A planned example is a magic slash command that dynamically makes metrics tools available to the agent. The tools should query, filter, and aggregate metrics for the current session or an explicitly selected set of sessions. The agent can then investigate token consumption, repeated context, tool latency, queueing, and where wall-clock time is being lost, and report conclusions with supporting data. The exact command name and tool schema are open design decisions; this is a product requirement, not a claim that the tools already exist.

The interaction should expose which data scope is being granted, what analysis is running, and how to inspect the supporting records. Favor agent-authored explanations and recommendations over a dashboard full of unexplained numbers. Preserve the option to inspect raw data and export reproducible queries/results. Distinguish measured values, estimates, unavailable values, and inferences.

## Distributed execution and backend implications

The UI iteration is deliberately allowed to anticipate substantial OMP changes. Planned capabilities include phone access through a gRPC proxy, secure ephemeral remote execution proxies, SSH hosts, Kubernetes workers, and mixed local/remote task execution. Prototype these concepts cheaply, but keep live functionality, illustrative sample data, and future plans clearly distinguishable.

| Concept | Meaning |
| --- | --- |
| Project | Durable context and materials for related work. |
| Session | A conversation, its goals and decisions, and the work launched within it. |
| Goal/task | Intended outcome or bounded work, with identity independent of a specific process. |
| Agent | A worker performing work and producing a trace/transcript. |
| Execution target | The machine or environment where work actually runs. |
| Artifact | A result, change, report, or other inspectable output. |
| Attention request | A specific decision, failure, or dependency requiring intervention. |

Future event contracts need stable agent/task identities, parent-child and dependency relationships, execution target identity, authoritative lifecycle state, timestamps, activity summaries, metrics provenance, artifacts, and actionable attention requests. The current flat subagent records do not supply all of these fields; the UI must not manufacture them.

Remote commands need acknowledged pending/running/finished states, idempotency, cancellation semantics, reconnect/resume behavior, and an audit trail. Loss of a client connection is not evidence that an agent stopped. Host credentials and delegated capabilities need explicit scope and expiry; secrets must not appear in transcripts. Backend details belong in setup and diagnostics unless they help the user make an immediate decision.

On a phone, preserve the same session and task identities with an adaptive conversation/activity presentation. Prioritize decisions and progress over fitting desktop columns onto a narrow screen. Reconnecting should recover work and outstanding decisions without duplicating commands.

## Acceptance scenarios

- A person with one conversation can read and respond comfortably without learning agent management.
- A person with three sessions can find active work and decisions without checking each transcript manually.
- A small delegation stays compact, shows its relationship to the primary agent, and makes success, failure, and warnings distinguishable without color.
- A larger workload can be searched, filtered, and scanned with a denser presentation in the same session. Clearing a filter restores the full context.
- A user can expand activity to concentrate on orchestration and return to the conversation without losing state.
- A user can pin several pieces of task evidence, read them independently, leave a draft, switch sessions, and return to the same arrangement and reading positions.
- Inspection never blocks unrelated navigation; deeper reading has a nonmodal home.
- Adding selected evidence to a conversation produces an editable, source-labeled draft and never sends it automatically.
- An agent that completed with a warning remains visible in attention-oriented views and opens evidence explaining the warning.
- A metrics investigation can eventually be requested conversationally through scoped tools, with a report grounded in accessible records.
- Local and remote work can eventually coexist in one task tree without implying unimplemented connections in today's app.
- Light and dark appearances are equally coherent, and text settings show real native examples.

## The Agent Observatory experiment

The current experiment treats agent output as working material. Two named, independently scrollable lists show running and finished work at a locally adjustable density; selecting a task keeps its evidence in an independently scrollable native pane. Users can compare up to four pinned tasks, traverse recorded prompts/tool results/agent output without a modal, and stage a source-labeled comparison request in the conversation. Conversation and Observatory are two arrangements of the same session, reachable from the mouse or keyboard, with the draft kept intact.

This direction extends the process/workspace visibility represented by [cmux](https://www.cmux.dev/) and the durable-session ambition described by [Superlogical](https://www.superlogical.com/). It is a design hypothesis, not a claim that every competitor lacks evidence inspection. The opportunity is to reduce the human effort of collecting context, comparing claims, deciding what deserves attention, and conveying that context to the next agent action.

Promising next experiments include an attention queue organized around decisions with consequences, provenance links from a conclusion to tool records, an explicit comparison of independent investigations, a preview of the exact context and scope being sent to an agent, and agent-generated explanations of the critical path and wasted work. These should be built over general query/action tools so the model can investigate and explain, rather than encoding a fixed set of dashboard interpretations. A personal agent could reuse the same evidence and decision surfaces for ongoing responsibilities, with a clear scope and interruption policy.

The production interface retains surface selection, agent tabs, independent reading positions, text selections, conversation drafts, task filters, and pane arrangement across session switches and normal app restarts. Cross-device restoration, changes-pane evidence retention, and background execution of multiple primary sessions remain work to do. The current single-runtime implementation still refuses session switching during an active turn. The product principle requires eliminating that restriction by separating browsing state from execution ownership; hiding the restriction in this UI prototype would not solve it.

## Priorities and boundaries for this iteration

Preserve this specification and use the isolated native preview to test the Agent Observatory alongside the compact connected activity tree. Prioritize immediate inspection, retained context, readable comparison, deliberate evidence transfer into a conversation, and fluid movement between conversation and orchestration. Preserve typography calibration, native accessibility, and GUI responsiveness while exploring different arrangements.

Remote transports, full dependency scheduling, dynamically injected metrics tools, and persistent personal-agent behavior require later backend work. This iteration should make their product requirements concrete without showing inert controls as functioning features.

## Research anchors and open questions

- [herdr agent documentation](https://herdr.dev/docs/agents/): lifecycle visibility and attention across multiple agents. Use authoritative agent state rather than treating a disconnected display as a stopped task.
- [ntm](https://github.com/Dicklesworthstone/ntm): orchestration throughput and keyboard-driven control at scale.
- [cmux](https://github.com/manaflow-ai/cmux): native workspace navigation and inspectable execution context.
- [Hermes Agent](https://github.com/NousResearch/hermes-agent): the owner's anchor for a personal agent with continuity across tasks.
- [Superlogical](https://www.superlogical.com/): a durable session spanning interactive, automatic, local, and remote work, with structured data/actions and continuity across devices. This reinforces the requirement that a session belongs to the work, rather than to an open client window.
- Decide the default threshold and persistence rules for workload search/density controls after exercising real sessions.
- Design task dependencies, cross-host control, budget limits, metrics tool scopes, and ongoing personal-agent authorization with the backend contracts rather than guessing them in the view layer.

## Approved production scope, September 17

Finish the approved interface on the current OMP runtime first. Production work includes real agent snapshots and journals, durable session reading state and drafts, explicit loading and failure behavior, ordered background persistence, and verification in the optimized app. Concurrent primary sessions are a separate runtime change. The larger product goals above remain intact.
