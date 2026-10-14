# Baseline results

All values are milliseconds except ratios. `Delta` is debug minus release; positive values favor release.

| Phase | Debug | Release | Delta | Debug / release | Interpretation |
|---|---:|---:|---:|---:|---|
| Native window visible | 215.916 | 203.466 | 12.450 | 1.061× | GUI startup is effectively unchanged. |
| OMP RPC spawn | 12,976.921 | 4,266.424 | 8,710.497 | 3.042× | Dominant debug/release startup difference. |
| Startup ready | 13,313.291 | 4,597.034 | 8,716.257 | 2.896× | Release reaches ready 65.5% sooner. |
| Background session discovery | 2,231.872 | 2,166.752 | 65.120 | 1.030× | Large but concurrent; not a readiness blocker. |
| Background workspace inspection | 93.772 | 63.565 | 30.207 | 1.475× | Concurrent and small relative to OMP spawn. |
| New-chat GUI action | 40.777 | 38.506 | 2.271 | 1.059× | Native action cost is profile-independent. |
| `new_session` RPC | 0.958 | 0.694 | 0.264 | 1.380× | Sub-millisecond in both builds. |
| `prompt` RPC acknowledgement | 0.829 | 0.407 | 0.422 | 2.037× | Sub-millisecond in both builds. |
| Prompt to agent start | 1.623 | 0.971 | 0.652 | 1.672× | Local scheduling/RPC overhead is negligible. |
| Prompt to first content | 1,587.482 | 1,234.379 | 353.103 | 1.286× | External-service sample; build effect is inconclusive. |
| Agent turn | 2,811.716 | 1,560.616 | 1,251.100 | 1.802× | External-service sample; build effect is inconclusive. |
| Prompt to turn-complete UI | 2,831.471 | 1,587.658 | 1,243.813 | 1.783× | Mostly inference wait; do not attribute to release compilation. |
| OMP graceful RPC shutdown | 15.304 | 8.985 | 6.319 | 1.703× | Very small absolute cost. |
| GUI shutdown | 32.039 | 22.434 | 9.605 | 1.428× | Current shutdown path is not a perceptible bottleneck. |
| Whole scenario | 17,160.655 | 7,100.460 | 10,060.195 | 2.417× | Release is 58.6% faster in this one controlled scenario. |

The `omp` spawn delta is 99.93% of the startup-ready delta. Native window presentation takes about 0.2 seconds in both runs; the remaining wait to readiness is overwhelmingly inside `OmpClient::spawn`.

The full-scenario difference is less clean because inference is remote. Startup alone explains 86.6% of the total debug/release difference. The remaining difference is almost entirely the single external inference sample and should not be treated as a release-build win without repeated randomized trials.

## GUI delivery and rendering

OMP-to-GUI event delivery is currently polled at 30 Hz:

| Build | Samples | p50 | p95 | Max |
|---|---:|---:|---:|---:|
| Debug | 9 | 18.112 | 33.725 | 45.293 |
| Release | 9 | 26.103 | 34.364 | 45.006 |

This profile-independent 0–33 ms scheduling delay is the clearest internal GUI latency opportunity. It is small compared with OMP startup and inference, but it affects every streamed event.

Native transcript rendering was also stable across runs. The first cold render cost 31.764 ms in debug and 30.572 ms in release; subsequent renders had medians near 0.6 ms and maxima below 0.8 ms. Both tests used the same GUI binary, so close agreement is expected.
