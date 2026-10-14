# Ranked causes

| Rank | Cause | Evidence | Confidence |
|---:|---|---|---|
| 1 | `run-app.sh` rebuilds/reprocesses the release helper before every launch, invalidating warm executable state | No-op helper build changes `ctime` but not `mtime` or SHA-256; first post-build spawn spends 3.002 s before OMP's first log, versus 40 ms warm | High |
| 2 | OMP is a 278 MB Mach-O with 267 MB of `__TEXT` and 67,286 signed pages | First-launch loader/pre-main time is 74× warm and accounts for 71% of the slow spawn | High |
| 3 | OMP eagerly constructs the complete headless session before emitting RPC `ready` | `rpc_mode::run_inner` performs settings/catalog/auth setup, `HeadlessSession::open_with_policy`, project environment attachment, discovery, and environment-daemon handshake before writing `ReadyFrame` | High |
| 4 | First OMP session also boots/connects the shared environment daemon | OMP logs show daemon initialization and handshake during the first session; repeated launches reuse the warm daemon and helper pages | Medium |
| 5 | Native Omperor startup | Window visible in 240 ms and does not overlap the multi-second pre-main OMP gap | Rejected as primary cause |
