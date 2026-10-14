# OMP debug/release lifecycle profile

## Question

Where does Omperor spend time across native application startup, a new-chat single-turn inference, and graceful shutdown, and how does the result change when the same GUI executable launches a debug versus release `omp` RPC helper?

## Scenario

The reusable `scripts/profile-omp-lifecycle.sh` harness launches Omperor, waits for the native UI to report `Ready`, starts a new OMP session, submits `Reply with exactly: pong`, waits for `Ready for another prompt`, sends Command-Q, and waits for the process to exit. The two runs use the same debug Omperor application executable, workspace, OpenAI account, model selection, prompt, and UI automation. Only `OMPEROR_OMP_EXECUTABLE` changes:

- debug: `vendor/oh-my-pi/target/debug/omp`
- release: `vendor/oh-my-pi/target/release/omp`

The runs were sequential on the same host, debug first and release second. Both binaries were local, warm build outputs. No cache flushing or operating-system tuning was applied. Consequently, this is a controlled directional comparison, not a cold-start study.

The release Python archive contains LLVM-22 bitcode. Homebrew's unversioned `lld` was 23.1.0 on this host, so the release helper was linked with the installed `lld@22` 22.1.8 keg. The ignored staging checkout's linker-shim path was pointed at `/opt/homebrew/opt/lld@22/bin/ld64.lld`; no vendored source revision was changed for the measurement.

## Invocation

The commands below use the current local development bundle path; the original measurement predates the Omperor Dev identity.

```sh
scripts/profile-omp-lifecycle.sh debug \
  "target/Omperor Dev.app/Contents/MacOS/omperor" \
  vendor/oh-my-pi/target/debug/omp \
  tests/artifacts/perf/20260901T213148Z-omp-debug-release/debug.jsonl

scripts/profile-omp-lifecycle.sh release \
  "target/Omperor Dev.app/Contents/MacOS/omperor" \
  vendor/oh-my-pi/target/release/omp \
  tests/artifacts/perf/20260901T213148Z-omp-debug-release/release.jsonl
```

## Measurement semantics

- `window visible`: process trace origin to the point after the native window is made key and visible.
- `startup ready`: trace origin to delivery of OMP's projected `Ready` event on the GUI thread.
- `OMP spawn`: the complete `OmpClient::spawn` call inside the worker thread.
- `new-chat action`: native action entry through completion of the GUI-side session reset request.
- `new-session RPC`: `new_session` command handling through RPC completion.
- `prompt RPC`: `prompt` command handling through RPC acknowledgement. This is not inference completion.
- `prompt to first content`: GUI command enqueue to first raw `message_update` event.
- `agent turn`: raw `agent_start` to raw `agent_end`.
- `prompt to turn UI`: GUI command enqueue to delivery of `turn_complete` on the GUI thread.
- `RPC shutdown`: OMP client's graceful `shutdown()` call.
- `GUI shutdown`: native shutdown request through worker-stopped delivery.
- `total scenario`: trace origin through `applicationWillTerminate`.

All durations use a monotonic clock. Wall-clock timestamps exist only for cross-process correlation. The trace records event kinds, sizes, executable paths, and timing metadata; it does not record prompt text, model output, credentials, or RPC payload contents.

## Validity checks

- Both application runs emitted `perf.profile.run_complete`.
- Both fresh OMP transcripts contain the submitted prompt and assistant response `pong`.
- Both application runs exited after Command-Q.
- No Omperor or `omp rpc` process remained after the runs.
- Neither trace contains a failed command/event enqueue marker.

## Limitations

There is one observation per build. Startup and local RPC comparisons are strong enough to identify order-of-magnitude effects, but percentiles are not claimed. The inference portion calls an external service, so its debug/release difference is not attributable to compiler profile from one sequential sample. Session discovery overlaps startup on a background thread and must not be added to the critical-path durations.
