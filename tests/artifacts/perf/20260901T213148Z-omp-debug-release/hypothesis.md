# Hypothesis ledger

| Hypothesis | Result | Evidence |
|---|---|---|
| OMP dominates perceived application startup. | Supported | Native window visibility is about 0.2 s, while readiness is 13.3 s debug / 4.6 s release. `OmpClient::spawn` consumes 13.0 s / 4.3 s. |
| Compiling OMP in release mode materially improves startup. | Supported | The release helper's spawn is 8.71 s faster and 3.04× shorter than debug in the controlled pair. |
| Native GUI construction is responsible for the debug/release startup difference. | Rejected | Both runs use the same GUI executable and differ by only 12.45 ms to window visibility. |
| Session discovery blocks OMP readiness. | Rejected for this scenario | Discovery takes about 2.2 s but runs concurrently; OMP spawn continues for another 2.1–10.7 s afterward. |
| Release OMP makes remote inference faster. | Inconclusive | The one release turn was faster, but network, provider load, cache state, and model-service variation are uncontrolled. Local prompt RPC work is already sub-millisecond. |
| OMP teardown explains slow application closing. | Rejected for the current path | Graceful RPC shutdown is 9–15 ms and the whole GUI shutdown path is 22–32 ms, with no remaining process. |
| Timer polling adds internal GUI latency to streamed events. | Supported | The event queue shows p95 delivery around 34 ms in both builds, matching a 30 Hz timer's expected scheduling envelope. |
