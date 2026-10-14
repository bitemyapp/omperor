# Behavioral checks

## Retained rows

The ordered row map retains an existing native view if and only if its absolute row index remains in the new visible range and this is a scroll-only update. Its task data, frame, accessible name and pin styling therefore stay valid. Any data, density, font, pin or layout refresh forces a rebuild as before. Entering rows use the same constructor and are inserted before the next retained native sibling, preserving Accessibility order when scrolling upward. Leaving rows are removed, keeping memory proportional to the visible range plus two-row overscan.

The baseline and after native Accessibility observations matched at every step of 20 density actions and 20 scroll actions. The expected initial agent ordering was also checked after scrolling back. The JSON files record those observations without transient AX indices; their checksums match. The pin bodies and record controls were present throughout. A full-range scroll reached agents 27–33, switching away and back restored that exact range, and both original pins remained.

## Redundant refresh and empty transcript batches

`edit_agent_view` already calls `refresh_agents_panel`, which calls `refresh_session_workspace` before checking sidebar visibility. Removing the caller's second refresh changes no final state. An empty transcript batch changes no cache entry; returning early preserves the existing cache, selection, native evidence text and reading state. Nonempty batches still take the original refresh path. Borrowing the agent board before the cadence gate avoids a deep clone without changing the journal plan or its worker execution.

## Worker notifications

Workers publish the result before requesting a coalesced main-loop callback. The queued flag is cleared before draining, so a concurrent completion can always schedule another delivery. A four-millisecond drain budget queues a continuation rather than discarding results. Common run-loop modes retain delivery during native tracking and deferred termination. The preference writer remains serial and signals each completed write. Unit coverage verifies publication-before-notification and order over 100 jobs. Native verification includes launch preference loading and preference changes followed by relaunch.

## Logging

Profiling is still opt-in in release. The caller uses a bounded nonblocking channel; directory creation, file opening, serialization and writes run on a dedicated thread. Saturation drops diagnostics rather than blocking the GUI, and the next accepted record reports the dropped count. A unit test fills a one-record queue and verifies sequence gaps, dropped count and schema. Like other asynchronous diagnostics, an abrupt process exit can lose a queued tail.
