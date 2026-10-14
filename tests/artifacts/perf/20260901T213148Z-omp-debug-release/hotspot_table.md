# Ranked latency targets

| Rank | Target | Evidence | Class | Confidence | Next measurement |
|---:|---|---|---|---|---|
| 1 | OMP RPC process initialization | `omp.rpc.spawn` is 12.977 s debug and 4.266 s release; it explains 99.93% of the readiness delta. | CPU / loader / initialization | High | Add phase marks inside OMP's `OmpClient::spawn` to separate executable load, embedded Python initialization, RPC handshake, provider discovery, and initial state hydration. |
| 2 | Remote inference wait | Agent turn is 2.812 s debug and 1.561 s release in the single samples. Local prompt RPC overhead is below 1 ms. | Network / upstream service | High that it dominates the turn; low that build profile caused the difference | Run at least 10 alternating debug/release trials with the same model and report median/IQR. |
| 3 | Session discovery | About 2.2 s in both builds. It runs in the background and overlaps OMP startup, so it does not delay window visibility or readiness in this scenario. | Filesystem / parsing | High | Instrument root enumeration, SQLite title lookup, and JSONL probing separately before optimizing. |
| 4 | OMP-to-GUI polling | Event queue delivery p95 is about 34 ms and max about 45 ms in both builds, consistent with the 30 Hz poll timer. | Scheduling | High | Compare an event-driven main-thread wakeup against the current timer while measuring CPU idle cost and stream smoothness. |
| 5 | First native Markdown render | Cold render is about 31 ms; later transcript renders are below 0.8 ms. | One-time initialization | Medium | Separate syntax-set/theme initialization from layout only if cold first-paint polish becomes important. |

Graceful shutdown is not a hotspot: the user-visible path is 22–32 ms and leaves no worker process behind.
