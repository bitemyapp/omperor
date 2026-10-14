# Token efficiency in Omperor

Omperor enables the helper's `OMP_TOKEN_EFFICIENT=1` profile on every launch. The implementation is maintained in `patches/oh-my-pi/0005-token-efficiency.patch`, `0006-background-completion-guidance.patch`, `0007-codex-client-metadata-strings.patch`, `0008-background-settlement-delivery.patch`, and `0009-kernel-owned-explicit-async.patch`; the helper build applies them after the existing protocol fixes.

| Control | Behavior |
|---|---|
| Context | Automatic compaction at 48,000 tokens, retaining approximately 8,000 recent tokens and at least two complete assistant/tool rounds. A long single user turn can now compact. Its user instructions remain verbatim. |
| Task allowance | One shared allowance per explicit user turn: 160 logical inference calls or 1,000,000 receipted uncached input tokens. Parent, task workers, compaction, and advisor calls share it. |
| Workers | At most two active task workers; workers cannot spawn further workers. Each gets 40 calls plus one wrap-up call, subject to the shared allowance. |
| Cancellation | A completed, cancelled, failed, or abruptly aborted parent stops its remaining workers. A provider rate/authentication failure stops the tree and preserves the original error. |
| Accounts | Task workers inherit the parent's explicit account selection. |
| Cache | Default prompt-cache and provider-session identities are stable for a journal, including resume, and change when switching journals. Explicit overrides are preserved. |
| Coordination | Owned jobs and workers automatically deliver results; the model is instructed to do other work or end its reply to be woken. Explicit finite coordination waits last at least 30 seconds while idle and wake early on messages/completion. Inbox drains batch up to 32 messages. Automatic auxiliary model replies to peer messages are disabled. |
| Tool results | Native results without rendered parts become readable content and pass through the existing output limits. This avoids repeated commands and file redirection just to recover invisible output. |

The allowance is checked before each logical inference operation. Provider receipts arrive afterward, so in-flight responses can exceed the token threshold. Provider-internal retries can add physical attempts. Cached input and output are separate from the uncached-input threshold; this is not an account-wide quota meter. An explicit new user turn receives a new allowance.

The session journal retains the original history. Compaction changes the context sent to the model, including when the journal is reopened.

## Upstream background-job guidance

Reviewed public [`can1357/oh-my-pi` main](https://github.com/can1357/oh-my-pi/tree/06f834ef7734f7df41c4aa1245d0bdf2ce3bdcff) on 2026-09-22, at `06f834ef7734f7df41c4aa1245d0bdf2ce3bdcff`. The screenshot matches [commit `dc696d25b5`](https://github.com/can1357/oh-my-pi/commit/dc696d25b53621ca3a17173473e8107370532e5b): it strengthens the Bash description and background-job result notice to discourage sleep/process/log polling and explain automatic completion delivery. That commit changes guidance and tests, not the inference transport or scheduler.

Patch 0006 ports that behavior into the Rust fork. It updates both Bash description paths, the tool-layer notice, both native dispatcher detachment paths, and Omperor's efficiency guidance. Updating only the similarly named tool-layer formatter would miss native dispatch: the kernel owns the invocation's lifetime and generates its own result notice.

The existing kernel waits for owned jobs at a candidate end of reply, then injects their results and resumes inference. There is no inference while it waits. The existing scripted regression checks exactly two requests—one before delivery and one after—and verifies that delivery is journaled once. The dispatcher regression checks that the actual model-visible result contains the wake-up guidance.

Upstream also has an [adaptive hub wait ladder](https://github.com/can1357/oh-my-pi/commit/23a5b9ae38864d3f785dc6cbc96eb6d674a1d32d) (5, 10, 30, 60, 300 seconds). This port retains our existing 30-second minimum for explicit coordination and prefers automatic delivery for owned jobs. It does not add the upstream ladder or merge unrelated TypeScript runtime changes.

The author's approximately 30% saving is not an Omperor measurement, and the reviewed commit includes no cost benchmark. Fewer polling turns should reduce inference traffic; actual savings depend on the workload and model behavior. This is guidance, not a hard ban on shell commands needed for real diagnostics.

All 20 focused regressions passed; the three affected crates also passed doctest builds. The updated release is installed at `/Applications/Omperor.app`; both installed programs match the build after excluding code signatures and their size metadata, the signature verifies, and the helper contains the new guidance. The change applies on the next app launch. The build and test evidence was recorded locally and is not kept in the repository. Reproduce the focused offline checks with `just test-omperor-background` in the patched helper source, using the SDK and Cargo settings below.

## Live-model follow-up

The requested Luna and Opus 5 checks exposed gaps in the initial offline coverage. Patch 0009 makes explicitly asynchronous external shell calls use the kernel-owned job board, retaining their completion stream instead of returning an unsupported daemon-side detached envelope. Patch 0007 serializes the generated Codex `client_metadata.responses_lite` flag as a string, matching the server's validated wire contract. Patch 0008 drains background progress without resuming inference until a terminal follow-up exists, and recovers inline terminal payloads instead of retaining the original detached-job payload.

The new controlled regression failed before the wake-up fix: stdout progress triggered a third inference while the tool was still running. It now checks that inference stays idle during progress, the final request ends with a user completion message, and that message contains the actual terminal output. A restart regression also verifies recovery of inline output. The 14 Codex codec tests and 27 turn/job tests pass. Nextest reported one leak warning in the existing orphan/revive test; the known baseline receipt test remains excluded.

The app also persists acknowledged account changes against the event's owning workspace, so restarting the helper does not overwrite a freshly selected account with the old workspace preference. All 256 app tests pass (two existing ignored tests). In the installed GUI, both final 75-second jobs completed with exact output: Opus used three model calls and one Bash call; Luna used four model calls, one Bash call, and two bounded hub waits.

The detailed live-model evidence (initial failures, offline regressions, final live results, and total usage including unsuccessful tests) was recorded locally and is not kept in the repository.

## Verification

The original verification below used scripted inference, local fixtures, and ordinary unit tests, with no live model requests. The subsequent user-requested Luna/Opus 5 smoke tests use live inference and are recorded separately.

- App suite: 256 passed, two existing ignored tests.
- Driver suite: 189 passed.
- Focused helper suite: 221 passed (12 compaction/budget, 189 driver, 19 projection, and one duplex-stream regression); doctest builds passed. The projection test expectation was updated to retain the user instructions when a turn is split.
- A synthetic long-turn regression checks a context reduction greater than 75%, preservation of the user goal, complete call/result pairs, repeated compaction, and journal reopen.
- Budget tests cover concurrent workers sharing one allowance, aggregate usage, owner-future abortion, cancellation, and preservation of the provider's original error.
- Cache identity and native output regressions cover the remaining inference-path changes.

The latest driver run passed with one nextest leak warning in an unchanged secrets-config test; two earlier driver runs passed without that warning.

The full agent suite has two existing failures: `central_truncation_spills_and_notrunc_explicitly_opts_out` and `tool_call_round_settles_in_the_dom_then_runs_second_inference`. Both were reproduced in a separate baseline checkout with only patches 0001–0004. An unrelated session `replay_law` test also fails to compile because its `TurnReceipt` initializer lacks existing fields; the focused suite runs the projection tests and the compaction replay regression directly.

After building the helper, its staged source provides `just test-omperor-efficiency`. On this machine, use `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk` for the bundled LLD linker and `CARGO_NET_OFFLINE=true` for Cargo. The staged source is `vendor/oh-my-pi/target/omperor-source`.

The synthetic reduction is a regression assertion, not a measured percentage reduction in provider quota use. The observed token burn that motivated these changes came from a recorded session investigation that is not kept in the repository.

The optimized release is installed at `/Applications/Omperor.app`. Its signature was verified. Both installed executables match the final build when excluding their signatures and the associated signature-size metadata. Build hashes and compact test evidence were recorded with that investigation and are not kept in the repository. The changes take effect on the next application launch.
