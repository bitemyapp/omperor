# `scripts/run-app.sh` startup investigation

## Question

Why can a release OMP backend still take several seconds—or, under the user's observed conditions, more than ten seconds—to become ready when Omperor is started through `scripts/run-app.sh`?

## Scenario

Measure the exact development path: a no-op debug Omperor build, a no-op release OMP build, bundle recreation and ad-hoc signing, LaunchServices launch, OMP RPC ready handshake, then graceful quit. Compare the first launch after the helper build with immediately repeated launches that do not rebuild the helper.

## Golden output

Every sample must make the native window visible, deliver the OMP `ready` event, emit `perf.profile.run_complete`, and leave no Omperor or `omp rpc` process behind.

## Scope

This round diagnoses and ranks causes. It does not modify OMP or attempt an optimization.

## Host and binaries

Same fingerprint as `tests/artifacts/perf/20260901T213148Z-omp-debug-release/fingerprint.json`. The OMP helper is the release binary at `vendor/oh-my-pi/target/release/omp`, SHA-256 `971d687307b70202f47fdad810c1561d907e222ef73c9270525b86cf8cff1187`, size 277,753,760 bytes.
