# Startup optimization results

## Change: launch a stable OMP release helper

- Ordering preserved: yes; Omperor launches the same release executable bytes from a stable installed path.
- Tie-breaking unchanged: N/A.
- Floating-point: N/A.
- RNG seeds: unchanged.
- Behavior proof: the Cargo output and installed helper both had SHA-256 `971d687307b70202f47fdad810c1561d907e222ef73c9270525b86cf8cff1187`.
- Cache proof: two unchanged helper builds left the installed executable at inode `180474662`, ctime `1788315925`, and mtime `1788315924`.
- Rollback: revert the stable-copy section of `scripts/build-omp-helper.sh`.

## Change: skip unchanged OMP release builds

The build key includes the vendored OMP revision, local patch hashes, build-script hash, Rust toolchain, architecture, selected LLD version, embedded-Python configuration and stamps, plus relevant Rust/macOS build flags. `--force` bypasses the key.

For the normal development entry point, `scripts/run-app.sh --rebuild-omp` forces that rebuild before launching.

- Before: an unchanged helper invocation ran Cargo and took approximately 0.9-1.8 seconds in observed runs.
- After: an unchanged helper lookup took 0.29 seconds and did not invoke Cargo.
- Debug app bundle build after cache hit: 0.83 seconds, down from 1.90 seconds immediately before the cache hit was enabled.
- Exact `scripts/run-app.sh` invocation after the changes: 1.36 seconds, down from the 2.113-second baseline.

## Change: queue startup New Chat and first prompt

This is an intentional UX behavior change rather than an isomorphic optimization. New Chat received during the RPC handshake is queued on the existing client startup rather than terminating and respawning OMP. One first prompt may be entered before readiness and is dispatched after both RPC and the requested session are ready.

Joined-system trace (`/tmp/omperor-startup-queue-20260901-d.jsonl`):

- GUI window visible: 216 ms.
- OMP spawn began: 259 ms.
- New Chat accepted: 297 ms.
- First prompt accepted: 394 ms.
- OMP spawn completed: 1,340 ms.
- RPC Ready emitted: 1,347 ms.
- New session created: 1,347 ms.
- Queued prompt dispatched: 1,361 ms.
- Turn finished in the GUI: 3,326 ms.
- OMP spawn count: one.

## Verification

- `bash -n scripts/build-omp-helper.sh scripts/build-app.sh scripts/run-app.sh`
- `cargo fmt --all -- --check`
- `cargo test`: 53 passed, 0 failed.
- Real signed debug app bundle launched through `scripts/run-app.sh`, reached Ready, and shut down cleanly.
- Real app accepted New Chat and a prompt during startup, completed the inference turn, and shut down cleanly.
