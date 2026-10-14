# Results

## Exact `run-app.sh` reproduction

| Phase | Duration |
|---|---:|
| Debug Omperor + release OMP no-op builds, bundle recreation, `open` return | 2,112.986 ms |
| Omperor process to native window visible | 240.025 ms |
| OMP RPC spawn/ready handshake | 4,242.350 ms |
| Omperor process to OMP `Ready` | 4,565.439 ms |
| Shell invocation to OMP `Ready` | 7,048.297 ms |

This reproduces a multi-second release-backend delay. The exact trace is `exact-runapp.jsonl`.

## Repeated bundle launches

Twenty immediately repeated launches of the same bundle and helper, without rebuilding OMP between samples:

| Metric | Ready latency |
|---|---:|
| Minimum | 901.440 ms |
| Median | 952.894 ms |
| p95 | 1,044.041 ms |
| Maximum | 4,583.169 ms |

The maximum is the first launch. The following nineteen samples range from 901.440 to 1,044.041 ms. This is a cold-versus-warm discontinuity, not ordinary run-to-run noise.

## First-execution split

OMP's persistent log supplies the first Rust log timestamp; Omperor supplies the child-spawn and ready timestamps.

| Phase | First launch after helper build | Immediate warm launch |
|---|---:|---:|
| Child spawn to first OMP Rust log | 3,002.129 ms | 40.417 ms |
| First OMP log to RPC spawn completion | 1,238.174 ms | 586.142 ms |
| Complete OMP RPC spawn | 4,240.274 ms | 626.540 ms |

Approximately 71% of the slow first spawn occurs before OMP reaches its first Rust-level startup log. That time belongs to executable loading, code-signature validation, page faults, and pre-main initialization rather than RPC or Omperor logic.

## No-op build mutation

Running `scripts/build-omp-helper.sh` when Cargo reports the release target as already finished changes the OMP binary's filesystem change time while preserving both modification time and contents:

| Property | Before | After | Changed? |
|---|---:|---:|---|
| `ctime` | 1788309657 | 1788309697 | Yes |
| `mtime` | 1788300185 | 1788300185 | No |
| SHA-256 | `971d6873…ff1187` | `971d6873…ff1187` | No |

The no-op build itself took 0.88 seconds. The unchanged hash proves there was no new OMP code. The changed `ctime` is consistent with Cargo/macOS reprocessing the executable's ad-hoc signature and invalidating the kernel's cached executable validation state.

## Binary shape

The release OMP Mach-O is dominated by executable/read-only content:

| Segment/section | Bytes |
|---|---:|
| Whole file | 277,753,760 |
| `__TEXT` | 267,075,584 |
| `__text` | 123,885,832 |
| `__const` | 123,781,685 |
| Code-directory page hashes | 67,286 entries |

The helper is ad-hoc linker-signed. A cold validation/page-in of this unusually large `__TEXT` segment can therefore dominate startup and vary substantially with cache and system pressure.
