# Contributing

Omperor is experimental, and its internals change quickly. Before a large
change, open an issue to discuss it. Contributions are dual-licensed under
MIT OR Apache-2.0, as described in [LICENSE](LICENSE).

## Build and run

Set up as the [README](README.md#build-and-run) says, then, in `nix develop`:

```sh
just run          # Omperor Dev, optimized
just run-debug    # a debug build, when you need a debugger
```

`just` alone lists every recipe. Local builds are **Omperor Dev**, with their
own state root, so they never touch an installed Omperor's accounts or
sessions. To try the interface without accounts or inference, turn on
Settings ▸ Developer ▸ Developer mode and choose the Mock inference backend:
new sessions are then answered by a scripted local agent (`/mock` lists its
scenarios).

## How the app is put together

Omperor is an `aizome-kit` program (`vendor/sekka/desktop/aizome/crates/aizome-kit`,
documented in `desktop/aizome/docs/kit.md` there): its state lives in
`App` (`src/app/`), messages (`Msg`) change it in `update`, and `src/ui/`
draws every window from it, in full, each time it changes. The kit keeps the
widgets' own state (scroll positions, selections, the composer's text) by
their place in the tree, so views give lasting parts a `key`.

- `src/app/mod.rs`: the state, the messages, and their dispatch.
- `src/app/live.rs`: each session's OMP runtime, its events, its history.
- `src/app/{sessions,compose,accounts,changes,subagents,projects,insights,tools,sheets}.rs`:
  the rest of the behaviour, by area.
- `src/ui/`: the main window (`mod.rs`, `sidebar.rs`, `transcript.rs`,
  `composer.rs`, `changes.rs`, `agents.rs`), Settings, the Timeline and Usage
  windows, and the sheets.
- `src/markdown/`: answers parsed into a document on workers, highlighted
  into theme roles, and drawn.

The kit is a submodule. When Omperor needs something it lacks, extend the kit
there (in Sekka's style, with its tests), rather than working around it here.

## Never block the interface thread

From [AGENTS.md](AGENTS.md), and binding for every change:

> Do not perform non-trivial compute, I/O (including OMP RPC interactions), or
> shell commands on the interface thread.

`update` and the views run on the interface thread. Anything that can take
longer than a frame (git and other subprocesses, reading or even checking
files, network requests, waiting on the OMP child) runs on a worker:
`cx.spawn` for one piece of work, the runtime thread for OMP, or a thread of
its own for a stream (the timeline reader, session discovery). It answers
with a message. While it runs, the window shows that work is under way (a job
in the status line, a spinner where the result will go) and stays responsive.
Results that may arrive out of order carry a generation, and a stale one is
dropped.

## Tests

| Command | What it checks |
|---|---|
| `just test` | The unit tests, and the app driven as a person would through the kit's `Harness` (`src/app/tests.rs`), headless, on the mock backend. |
| `just shots` | Redraws `docs/screenshots` from those tests. Look at them after changing what a window shows. |
| `cargo test --bin omperor eval` | The eval runner only. |
| `./target/release/omperor eval validate [TASK...]` | That each eval task's graders reject the untouched fixture and accept the reference solution. No inference. |
| `just clippy` | Lints. |

Run `eval validate` for any task you add or change under `evals/tasks`, and
see [docs/EVALS.md](docs/EVALS.md) for how to write one. Tests must not use
your real accounts or state: the app tests set `OMPEROR_STATE_DIR` to a
temporary root, and anything else that launches the app or the helper must
too.

The tree is not uniformly rustfmt-formatted. Keep formatting changes out of
unrelated files.

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
4. Run `just helper`. The helper's build key hashes the patches, so changing
   any of them restages the source and rebuilds the helper.

Add focused tests to the patch itself. Several patches add
`just test-omperor-*` recipes to OMP's `justfile`; run them inside the
staging checkout.

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

The files in `docs/` cover specific subsystems. Update the one your change
affects.
