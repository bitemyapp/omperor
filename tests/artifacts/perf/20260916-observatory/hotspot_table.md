# Ranked findings

| Rank | Path | Evidence before optimization | Confidence | Change |
| --- | --- | --- | --- | --- |
| 1 | Density / scope changes | 20 actions, median 68.54 ms; two full refreshes per density action | High | Remove duplicate refresh |
| 2 | Agent scrolling | 20 alternating quarter-page actions; 20 scroll callbacks outside layout; p95 22.91 ms; 200 native rows created; row creation dominates native work | High | Retain overlapping visible rows and avoid unchanged range-label writes |
| 3 | Idle preview | 206 interrupt wakeups in 20.07 seconds; fixed 100 ms timer | High | Deliver job-completion notifications; no preview polling timer |
| 4 | Unchanged transcript sync | Static call chain: every empty result refreshes both evidence surfaces, up to 2.5 times/sec; worker returns an empty vector for unchanged files | High mechanism, live cadence unmeasured | Return before touching UI on empty updates; defer full view clones until cadence permits work |
| 5 | Preference persistence | Static repeating 100 ms timer remains active after first save | High | Reuse worker-completion notifications |
| 6 | Profiling logger | File serialization and write_all on caller thread; disabled in release unless opted in | High | Bounded nonblocking channel to a writer thread |

Sampled CPU traces include Accessibility inspection overhead from the UI driver, so they are attribution evidence rather than a standalone production benchmark. Raw profiles stay local because they are large and host-specific; summarized timings and counters are preserved here.
