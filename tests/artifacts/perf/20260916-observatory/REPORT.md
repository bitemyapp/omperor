# Observatory efficiency and main-thread review

## Result

The sidebar crown is aligned to the visible cap height of “Omperor.” The optimized release preview is running. The Observatory now retains rows that remain visible during scrolling, skips duplicate filter/density refreshes, and leaves evidence views alone when a journal check returns no changes. Preview jobs and preference saves deliver completion notifications instead of maintaining repeating 100 ms timers. Profiling file I/O runs on a bounded asynchronous writer. Runtime and background-result drains yield between batches to protect input responsiveness.

## Measurements

These are native callback durations from the same symbolized release-optimized build settings on an Apple M5 Max. Both interaction runs had the same opt-in span instrumentation. They measure application work, not end-to-end input-to-display latency. Twenty density changes and twenty alternating quarter-page scroll actions used the 84-task, two-pin fixture at 1440 × 900 points. Asynchronous logging was added after these comparisons, so logger changes do not explain their timing difference.

| Measurement | Before | After |
| --- | ---: | ---: |
| Scroll callback median | 10.06 ms | 1.21 ms |
| Scroll callback p95 | 22.91 ms | 2.20 ms |
| Total scroll callback time, 20 actions | 234.46 ms | 25.44 ms |
| Native rows constructed while scrolling | 200 | 20 |
| Density change median | 68.54 ms | 48.66 ms |
| Density change p95 | 72.69 ms | 50.81 ms |

The scroll callback meets the 8 ms budget. Density changes still rebuild the visible controls and can exceed a frame; this is an explicit remaining interaction cost, not a claim that every GUI callback is now below a frame.

Idle measurements use macOS per-process resource counters with the Mach timebase conversion; profiling was disabled. Each row below is twenty one-second samples. Both initial idle layouts used compact rows and two pins. No system power settings were changed.

| Idle run | CPU time across ~20 seconds | Interrupt wakeups | Package-idle wakeups |
| --- | ---: | ---: | ---: |
| Before | 12.09 ms | 206 | 29 |
| After, first run | 74.10 ms | 104 | 4 |
| After, quiet repeat | 0.27 ms | 7 | 0 |
| After saving preferences and relaunching | 1.12 ms | 8 | 2 |

The first after-run contains a short burst in seconds 11–13, preserved in the raw counters rather than excluded. It was not attributed conclusively. Most other one-second intervals show zero CPU time and zero wakeups. The removal of the periodic 10 Hz preview timer is independently established by the implementation. These results support reducing unnecessary idle work; they do not establish battery-runtime savings or whole-machine energy consumption. Other applications and agents were running. Memory snapshots are included in the JSON, but differing settings-window lifecycles make a memory-reduction claim inappropriate.

## Main-thread boundaries

| Area | Finding |
| --- | --- |
| Observatory journal reads, file metadata, transcript folding | Submitted to workers; no filesystem reads in the native row renderer |
| Project indexing, git inspection and diffs | Existing background paths retained |
| Preference snapshots from the redesigned settings | Ordered single-worker writes; completion delivered on the common-mode main run loop |
| Metrics append log | Existing dedicated writer thread retained |
| Profiling diagnostics | File opening, directory creation, serialization and writes moved to a worker; saturation drops diagnostics and reports a count instead of blocking the GUI |
| Rendering | AppKit objects remain on the main thread; work scales with visible rows plus overscan, not the entire swarm |
| Event bursts | A 4 ms budget between events yields before draining indefinitely; one expensive event or transcript render can still exceed this budget |
| Unchanged journal batches | Return before any evidence/Observatory rebuild; full desk snapshots are cloned only after the journal cadence gate |

## Open findings in the older core app

This is not a clean bill of health for every production path. `start_background_services` still loads preferences and the project registry synchronously. Account/project management still calls SQLite helpers on the GUI thread, including `replace_accounts`, `register_authenticated_account`, `load_project_registry` and several editing actions. Directory canonicalization and creation also remain synchronous in some project actions. These predate the Observatory iteration and can stall on slow or locked storage. They need typed worker results and cached metadata, preserving account and project transaction ordering, before those paths satisfy the repository's main-thread rule.

The production OMP timer still polls at 30 Hz, including when idle; this preview's no-polling idle result does not apply to that timer. A subsequent production-runtime change should deliver stream, watcher and discovery notifications, preserve the streaming redraw cadence, and retain termination handling in common run-loop modes. Live inference, blocked SQLite, remote filesystems and extremely large transcript rendering were not exercised in this fixture. The trace also contains Accessibility work from the test driver, so sampled CPU attribution is not a standalone user-interaction benchmark.

## Verification

All 177 tests passed, including new worker-publication ordering and saturated-recorder checks. The production-feature check and strict Clippy check passed with only the repository's existing lint exceptions. Native Accessibility observations matched at every step of both twenty-action comparisons. The list also scrolled to agents 27–33, switched sessions, and returned to the exact saved range with both pins intact. A preference change was saved, the app quit and relaunched, and the saved value loaded; the original System appearance was then restored. The final release executable and launched bundle have matching Mach-O UUIDs in `fingerprint.json`.

See `PROOF.md`, `span_summary.json`, `idle_summary.json`, the per-second counters and matching native UI golden checksums for evidence. Raw symbolized sample traces and per-event profiling logs are kept locally and ignored by Git; summarized evidence is committed. Optimization commits are separated so each change can be reverted independently.

## Reproduce

Build with `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk cargo build --locked --offline --profile profiling --features design-preview`. Launch that executable in the separate design-preview bundle with `OMPEROR_PERF_LOG` set to a writable JSONL path. Open “Inspect a parallel workload,” leave the initial Split arrangement and comfortable density, alternate the density control twenty times, then alternate one-quarter-page down/up scrolls in Running twenty times. Use `gui.observatory.scope`, `gui.observatory.scroll` and `gui.observatory.row.create` spans; omit layout-time scroll notifications by taking only events after the final density span. Keep logging configuration identical between runs.

For idle, run `scripts/preview-design.sh --release`, choose the fixture's compact Split view, stop UI automation, then run `python3 scripts/profile-gui-idle.py PID`. Save all samples, including bursts. The script reads counters only and does not attach a debugger or change system configuration. Use `sample PID 25 1 -file OUTPUT` for native stack attribution during the interaction workload. Run `shasum -a 256 -c golden_checksums.txt` from this artifact directory to verify the saved UI observations.
