# Hypothesis ledger

| Hypothesis | Verdict | Evidence |
|---|---|---|
| The app is accidentally launching debug OMP. | Rejected | The bundle path resolves to `vendor/oh-my-pi/target/release/omp`; trace and binary fingerprint match the release helper. |
| Native Omperor construction causes the wait. | Rejected | Window visibility is about 240 ms. |
| OMP's normal warm RPC path inherently takes more than ten seconds. | Rejected | Nineteen consecutive warm launches reach ready in 0.90–1.04 s. |
| The first launch after `run-app.sh` is made cold by the no-op helper build. | Supported | Helper `ctime` changes with identical contents; exact `run-app.sh` reproduction then spends 3.0 s before OMP's first Rust log and 4.24 s in spawn. |
| OMP itself still has meaningful eager startup work. | Supported | Even after the loader reaches Rust, the cold launch spends another 1.24 s before ready; source shows full session/environment construction before the ready frame. |
| A private OMP fork is required for the first fix. | Rejected | Omperor can first stop executing Cargo's mutable build artifact directly and avoid reprocessing it on every `run-app.sh`. OMP-internal lazy-ready work is a separate second-stage opportunity. |
