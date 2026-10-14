# Independent app state

Omperor and Omperor Dev can run together, including in the same repository. Build both with the current helper: older helpers hold an exclusive repository lock that prevents another edition from opening it. Finish work and quit the older app before replacing it.

| Launch | Default state root (macOS; Linux) |
|---|---|
| Installed Omperor (`production` Cargo feature) | `~/Library/Application Support/Omperor`; `$XDG_DATA_HOME/omperor` |
| Local Omperor Dev, debug or release | `~/Library/Application Support/Omperor Dev`; `$XDG_DATA_HOME/omperor-dev` |
| `omperor eval`, from either executable | Omperor Dev’s root |
| Design preview | Temporary fixture state; no real accounts or inference |

On Linux, `$XDG_DATA_HOME` defaults to `~/.local/share`, and a root that
earlier Linux builds created under `~/Library/Application Support` keeps
being used where it is. `omperor --state-paths` prints the roots a build
resolves.

Each root owns `omperor.sqlite3` (preferences, account labels/defaults and the project/session registry), `omp/sessions` (journals and metrics), `omp/data`, `archives`, `worktrees`, and debug `logs`. OMP’s state, cache, configuration and worktree directories are pinned beneath that root’s `omp/data`, with the default OMP profile. Inherited TUI `OMP_*_DIR` settings cannot redirect these child stores. Global `~/.o2` configuration is not automatically imported.

The existing production root stays in place. Dev starts with independent settings and no accounts; sign in through its Settings → Accounts page. Prior test conversations already stored in production remain there. There is no automatic copying, moving or deletion of history or credentials.

`scripts/build-app.sh` creates `target/Omperor Dev.app`. `scripts/build-app.sh --production` creates `target/Omperor.app`; `just install` installs the latter and leaves the Dev bundle intact. The channel is compiled into the executable, so renaming or moving a bundle cannot switch its state. Both the window and app menu identify the edition.

## Isolated test campaigns

Set `OMPEROR_STATE_DIR` to an absolute directory to select a separate store for a GUI launch or a headless eval campaign. For example:

```sh
OMPEROR_STATE_DIR=/tmp/omperor-campaign/state \
  './target/Omperor Dev.app/Contents/MacOS/omperor' --state-paths
```

`--state-paths` reports the resolved channel, app paths and eval root as JSON without opening a window, creating directories, reading accounts or running inference. The explicit root overrides both editions; deliberately giving both the same root makes them share state. An empty override uses the channel default. A relative override is rejected.

For authenticated evals, first sign in using that explicit Dev root. Run evals with the same override. Evals prefer the Dev packaged helper even from a production executable: one data store must use one helper generation. If there is no Dev bundle, they fall back to the invoking app’s helper. `OMPEROR_OMP_EXECUTABLE` explicitly overrides that choice. A spec’s `data_dir` takes precedence for its OMP credential/model store; other run artifacts stay in the requested output directory. See [evals](EVALS.md).

This isolates app data, not arbitrary task side effects. Both editions can still edit a repository or control the same browser/desktop when asked. Use separate working copies for independent coding experiments. Native macOS permissions remain separate for the two bundle identities.

## Shared repositories

OMP namespaces document ownership by data store and profile. It serializes filesystem commits across those namespaces and checks content before replacing it, preserving conflicts rather than allowing a stale edit to overwrite another edition’s change. Session journal writers remain exclusive within each store. A compatibility lock prevents an old helper with the former global authority behavior from running alongside the new scheme.

Concurrent first launches into one empty store also coordinate credential initialization. Schema migration decisions are made under the SQLite write lock; local credential keys are fully written before atomic publication. Transient SQLite journal-mode contention has a bounded retry. Existing accounts and key files keep their format.

The regression harness exercises real helper processes, parallel requests, separate journals, writer exclusivity, shutdown and resume against a loopback inference fixture:

```sh
python3 scripts/test-concurrent-rpc.py --cold-start --isolated-stores \
  --helper '/Applications/Omperor.app/Contents/Helpers/omp' \
  --other-helper './target/Omperor Dev.app/Contents/Helpers/omp'
```

The bundle routing check is `python3 scripts/test-app-state.py --production /Applications/Omperor.app/Contents/MacOS/omperor --development "./target/Omperor Dev.app/Contents/MacOS/omperor"`. It verifies channel identities, independent paths, explicit roots and Dev account routing from both executables.

Run `python3 scripts/test-concurrent-rpc.py --cold-start` to check parallel eval helpers sharing a freshly initialized Dev-style store. The patched helper’s `just test-omperor-isolated-state` and `just test-omperor-credential-startup` recipes cover lock semantics and concurrent store/key creation at the library level.

The concurrency harness uses a temporary HOME and project, and retains its evidence directory. It does not use saved accounts.
