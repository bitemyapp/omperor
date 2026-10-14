# Selective typography-branch integration

## Scope

On 2026-09-17, the owner approved carrying correctness fixes from `typography` into `codex/interface-redesign` while retaining the approved redesign's type scale, fonts, spacing, and native surfaces. This is a selective integration, not a merge of the visual design. The source branch ends at `16644d3`; its seven commits are based on the same `69808107` main revision as this redesign.

## Incorporated changes

- Settings account and metrics layouts use a scoped reentrancy guard from `347d3a3`. AppKit scrollbar frame notifications cannot append duplicate controls during a partially completed layout.
- Journal replay and the session timeline recognize the content kind in OMP's `assistant.content` patches, from `acefaa7`. Recorded reasoning summaries occupy their own rows/sections, subsequent reply segments keep their chronological position, empty segments are dropped, and thinking text uses parsed Markdown. Typography and heading changes from that commit were omitted.
- The OMP submodule advances from `b8349029` to `0547bd2a`, as in `2b125a8`. That single upstream change skips unchanged manifest entries before acquiring document leases during restore planning. It avoids opening watchers for every unchanged file during a delegated worktree merge. Existing Omperor helper patches remain applied through the normal staged build.
- Nonmodal credential warnings, Accounts navigation, reauthorization actions, and legible account errors carry the intent of `ca904b8` and `ab50cb0`, adapted to the production redesign.

## Account recovery adaptations

Failure state is independent of banner state. Managing, snoozing for 30 minutes, or dismissing a warning leaves the failure explanation and Sign in again action in Accounts. Multiple affected accounts are retained; handling one warning exposes the next. The Accounts action selects the relevant provider and scrolls to the identified account once, without repeatedly moving the reader during subsequent updates.

Exact account identity takes precedence. A primary's account is a fallback only when consistent with the provider. Subagent errors never inherit the primary's account: OMP's subagent board does not supply that identity. Ambiguous failures show a provider-level or generic warning instead of accusing an arbitrary account. OpenAI subscription and API credentials are not conflated by the human label “OpenAI.”

An initial subagent snapshot is historical data. Warnings require a task observed running in this runtime or one whose recorded start is after the runtime became ready. All changed tasks are considered, not just the first failed task. Duplicate failures do not reset snoozing or dismissal. A listed “active” account is not proof of recovery. Recovery requires a successful sign-in or a matching-provider inference receipt with output tokens; a different working account cannot clear an unidentified provider failure. Explicit successful sign-in also invalidates stale usage errors for that account.

The banner reserves space below the Conversation/Observatory selector, uses a stacked layout in narrow panes, and keeps the full message available to accessibility and hover text. Attention colors use the existing appearance-aware palette. Account actions retain the runtime's asynchronous RPC path and ordered background persistence. Banner synchronization does not rebuild unchanged views; Settings updates are coalesced and subagent snapshot matching uses an ID index.

## Verification

The app suite passes 194 tests, including file-backed journal/timeline separation and account-error classification, exact and ambiguous identity, historical/live task transitions, duplicate suppression, multiple-account snoozing/dismissal, and recovery. All-target/all-feature Clippy passes with the branch's seven existing lint exceptions; no new source lint suppressions were added. Native optimized preview checks cover both appearances, narrow and wide center panes, Conversation/Observatory reflow, Accounts reveal and scrolling, single-copy account labels, and retaining the reauthorization action after leaving and returning to Accounts.

The vendored environment daemon's `just test-pkg omp-envd` passed 841 tests, including the restore-manifest regression, with two skipped tests and no doctest failures. The dependency revision is also present on its configured remote branch. This machine requires the documented `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk` override for the native helper link.

The final ordinary release bundle was built with the SDK override, verified with strict deep code-signature validation, and launched successfully to Ready. Its frontend UUID is `73E1C3F4-DD83-3442-B684-4031DC08653D`; its rebuilt helper UUID is `4C4C44D9-5555-3144-A1D2-BF3D4AAC32BA`, matching the staged helper at `0547bd2a`. Existing real session history confirmed that reasoning summaries display separately from answers. Reopening a session with an old revoked-token subagent failure loaded the two-agent board without raising a new credential banner. No new inference prompt was sent.

For a repeatable native account-warning scenario, run `scripts/preview-design.sh --release --account-alert` (optionally `--light`). It uses synthetic accounts with reserved example addresses, shares the real warning and Settings controls, starts no OMP child, and never touches the real credential store. The reauthorization button is disabled in this fixture because there is no runtime. The ordinary binary does not compile the scenario.

Authentication failure attribution still depends on textual errors when OMP does not supply a structured account ID. Unknown identity remains explicit. Live credential revocation and browser reauthentication were not forced during this integration; existing credentials were left untouched. No new large production swarm or battery-runtime benchmark was run.
