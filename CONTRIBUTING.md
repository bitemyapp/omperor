# Contributing

Omperor is experimental, and its internals change quickly. Before a large
change, open an issue to discuss it. Contributions are dual-licensed under
MIT OR Apache-2.0, as described in [LICENSE](LICENSE).

## Build and run

Install the prerequisites listed in the [README](README.md#requirements),
then:

```sh
git submodule update --init --recursive
just run        # build target/Omperor Dev.app (optimized) and open it
just debug      # debug bundle, when you need a debugger
```

`just` alone lists every recipe. Local builds are **Omperor Dev**, with their
own state root, so they never touch an installed Omperor's accounts or
sessions. Quit Omperor Dev before rebuilding it.

To review interface changes without accounts or inference,
`scripts/preview-design.sh` builds a preview with deterministic sample
content; add `--release` when judging responsiveness.

## Tests

| Command | What it checks |
|---|---|
| `just test` | The app's unit and integration tests (`cargo test`), after generating Upleft's Markdown corpus if the checkout lacks it. |
| `cargo test --bin omperor eval` | The eval runner only. |
| `./target/release/omperor eval validate [TASK...]` | That each eval task's graders reject the untouched fixture and accept the reference solution. No inference. |
| `just test-linux` | The Linux build and its tests, through Sidestep. See [docs/LINUX_BUILD.md](docs/LINUX_BUILD.md). |

Run `eval validate` for any task you add or change under `evals/tasks`, and
see [docs/EVALS.md](docs/EVALS.md) for how to write one. Tests and smoke
checks must not use your real accounts or state. Set `OMPEROR_STATE_DIR`, or a
temporary `HOME`, for anything that launches the app or the helper.

The tree is not uniformly rustfmt-formatted. Keep formatting changes out of
unrelated files.

## Never block the main thread

From [AGENTS.md](AGENTS.md), and binding for every change:

> Do not perform non-trivial compute, I/O (including OMP RPC interactions), or
> shell commands on the AppKit main thread.

Work that can take longer than a frame (git and other subprocesses,
filesystem walks, network requests, waiting on the OMP child) runs on a
background worker: the job pool, the runtime thread, or a dedicated thread.
It reports back through a channel that the main-thread timer polls. While it
runs, the interface shows that work is in progress and stays responsive.

## Changing OMP

`vendor/oh-my-pi` is pinned to one commit and stays clean. Omperor's fixes to
OMP are carried as patches in `patches/oh-my-pi/`, numbered in the order they
apply:

1. `scripts/build-omp-helper.sh` clones the pinned commit into a staging
   directory (`vendor/oh-my-pi/target/omperor-source` by default), applies
   every patch listed in its `PATCH_FILES` array in order, and builds the
   helper there.
2. Make the fix in a scratch worktree of `vendor/oh-my-pi` at the pinned
   commit, with the listed patches applied (`git apply --unidiff-zero`) and
   committed, so that `git diff` shows only your change. Save that diff as
   the next numbered file, `patches/oh-my-pi/NNNN-short-name.patch`. Do not
   work in the staging directory itself: the build deletes and recreates it
   whenever the patches change.
3. Add the new file to `PATCH_FILES`. A patch that is not listed there is
   never applied.
4. Run `just release`. The helper's build key hashes the patches, so changing
   any of them restages the source and rebuilds the helper.

Add focused tests to the patch itself. Several patches add
`just test-omperor-*` recipes to OMP's `justfile`. Run them inside the staging
checkout, with the same `SDKROOT` the build uses.

The Linux-only patches in `patches/upleft/` and `patches/sidestep/` are
applied by `scripts/linux-cargo`, which picks up every file in those
directories.

## Commits

Follow the existing history (`git log`):

- An imperative subject line without a trailing period, such as "Isolate
  production and development state and concurrent startup".
- A body wrapped at about 72 columns that explains what changed and why.
  Group a large change by area (runtime, data, composer), and name OMP
  patches by number.
- One coherent change per commit. Keep generated files and local artifacts
  out.

## Documentation

[docs/AGENT_HANDOFF.md](docs/AGENT_HANDOFF.md) is a long working history of
the code, written for agents and maintainers. The other files in `docs/`
cover specific subsystems. Update the one your change affects.
