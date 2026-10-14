# Hypothesis ledger

1. Keeping overlapping rows will reduce new AppKit controls to entering rows while preserving task IDs, order, frames, accessible labels and reading anchors. Test with the same 20 scroll actions and native AX output.
2. Removing the second density refresh will reduce transition cost without changing the final native hierarchy. Compare 20 alternating density actions, including visible ranges and evidence text.
3. Worker completion notifications will remove periodic idle preview and preference-save wakeups without adding job latency. Check ordered completion, UI preference persistence and idle counters with profiling disabled.
4. An empty transcript batch has no information to apply. Skipping it must preserve cache and UI; nonempty batches retain the existing path. The live backend is reviewed statically, not exercised with real inference in this design fixture.
5. Buffered asynchronous profiling must preserve record schema and run/span ordering as sent; bounded saturation must be explicitly reported and never block the GUI.
