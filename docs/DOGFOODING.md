# Readiness review — 7 October 2026

Branch `claude/dogfood-release-readiness`, on top of `codex/dogfooding-evals`. This is the second readiness pass. It covers code review of the app and its OMP patches, GUI testing of an isolated build with real inference, a headless eval suite against real accounts, and preparation for releasing the source.

## Verdict

Ready for supervised daily use on real projects with an OpenAI (Codex) subscription account, **once the patched helper is installed** (`just install`). Before this pass, a task that started a sub-agent could delete tracked files from the user's project, and did so in testing; that and several lost-prompt and lost-work paths are fixed below.

Not yet validated: Anthropic accounts (the saved sign-in had lapsed and only the owner can renew it), long multi-hour sessions, and Linux. Not yet comparable to ChatGPT or Claude Desktop in everyday polish: there is no tool-approval mode, no notifications, no find in the transcript, and the composer is three lines tall (see [open issues](#open-issues)).

## What this pass fixed

| Severity | Problem | Fix | Evidence |
|---|---|---|---|
| Data loss | Merging back an untouched sub-agent deleted tracked files. OMP's snapshot walker applied ignore rules from directories *above* the project, so with a home directory kept in git, `~/.gitignore` (`bin/`, `build/`, `workspace/`, `.cargo`, …) hid files from the sub-agent's copy and the merge read them as deleted. | OMP patch 0023: a repository root, or an isolated copy, is an ignore boundary. | GUI test: 142 tracked files deleted. Eval `subagent-merge-keeps-files`: installed helper deletes 5/5 files; patched helper keeps all, 3/3 runs. |
| Data loss | Git run by a sub-agent in its copy climbed to the nearest enclosing repository — the home directory — so `git status` scanned all of `~`, and `git add -A` or `git clean` would act on the user's files. | Helper runs with `GIT_CEILING_DIRECTORIES` at OMP's worktree directory. | Sub-agent `git status` ran 30 s+ until killed before; returns "not a git repository" now. |
| Data loss | Deleting a session whose sub-agent is named like shared storage (`metrics`, `blobs`, `tmp`, `frames`) deleted that storage for every session. | Shared names are never part of a session's files. | Unit test. |
| Data loss | Reopening a session after relaunch discarded its unsent attachments. | Clear before restoring, not after. | Code trace. |
| Data loss | If preferences failed to load at startup, the next Settings change overwrote every saved preference with defaults. | Never store before a successful load. | Code trace. |
| Lost message | A prompt sent while the agent restarted (Refresh Models, sign-in, account change) vanished. | Session-level commands survive a restart; stops, steers and sign-in answers for the old child do not. Prompts that cannot be sent are reported. | Unit test. |
| Lost work | Finishing a sign-in killed the running turn without telling the UI. | The restart waits until the turn ends. | Unit test. |
| Wrong result | A second Stop press, or Stop on an idle session, queued an interrupt that cancelled the user's *next* prompt. | Omperor sends no abort while stopping or idle; OMP patch 0024 ignores an abort with no turn running. | OMP RPC test fails before, passes after. |
| Stuck turn | A model mistake in a sub-agent call (`{"i": …}` inside a task item) ended the whole turn with "session tool JSON is invalid". | OMP patch 0025: the call fails as a tool error the model can repair. | Eval: failed turn before; the model corrected the call and passed after. |
| Leak | Every sub-agent left its ~45 MB environment daemon running forever. | OMP patch 0027: a daemon for an isolated copy exits once idle. | 6 orphans before; 0 after. |
| Leak | A browser the agent opened outlived its environment daemon: 8 orphaned Chromes, ~7.8 GB, 4 of them visible windows. | OMP patch 0030: the daemon closes its tabs before it exits; eval attempts are reaped as a process group. | Real-browser test fails before, passes after. |
| Cost | Codex requests carried no conversation identity, so a conversation's turns landed on machines without its prompt cache; only the static ~7k-token prefix was ever cached. | A per-conversation `prompt_cache_key`, and OMP patch 0028 adds the `session_id`/`conversation_id` headers the Codex client sends. | Same 11 tasks: cached share 24% → 75%; fresh input 1.90M → 0.67M tokens (−65%). Request bodies verified append-only with patch 0026's capture. |
| Privacy | Images on the web linked from a reply were fetched as soon as they rendered — the standard exfiltration path for a prompt-injected agent. | Settings › Appearance › Load web images, off by default. Local images unaffected. | Visual check. |
| UX | Up arrow replaced the draft with history from any line; Down past the newest prompt gave an empty field. | Recall only from the first line; Down past the newest restores the draft. | GUI test. |
| UX | No keyboard way to stop; no Close, Hide, Zoom, About or Bring All to Front menu items; quitting killed running agents silently; smart quotes and dashes rewrote prompts (`--flag` → em dash). | Escape and File › Stop Response (⌘.) stop a turn; standard menus; quitting while agents work asks first; substitutions off in the composer. | GUI test. |
| UX | The sidebar re-sorted a session to the top whenever it finished a turn; the composer was read-only while a session loaded; the window forgot its frame; status lines said "OMP RPC". | Unarranged sessions sort by creation; typing never waits; the frame is saved; plain wording. | Tests and GUI. |

Also: the `markdown_snapshot` example compiled again; test-only process spawns no longer race other tests' file locks.

## Evals

`omperor eval suite` now runs task directories against real inference, in parallel, with repeats and a model matrix, and reports pass rates (Wilson intervals), tokens, tokens per pass, cache share and time. `validate` proves graders tell a solved task from an unsolved one without inference; `doctor` and `login` make a headless machine ready. See [EVALS.md](EVALS.md). Fourteen tasks are bundled, each checked against a reference solution and against shortcut "solutions" that must fail.

Final pass, patched helper, `openai-codex/gpt-5.6-luna` at medium reasoning, one attempt per task:

| Task | Passed | Tokens | Cached | Requests | Time |
|---|---|---:|---:|---:|---:|
| browser-checklist | 1/1 | 82,005 | 79% | 9 | 47 s |
| code-reading-qa | 1/1 | 535,573 | 76% | 48 | 79 s |
| csv-day-plan | 1/1 | 14,382 | 0% | 2 | 8 s |
| git-resolve-merge-conflict | 1/1 | 147,401 | 74% | 13 | 60 s |
| go-rename-across-packages | 1/1 | 189,261 | 58% | 12 | 58 s |
| ping | 1/1 | 7,104 | 69% | 1 | 3 s |
| python-debug-from-traceback | 1/1 | 512,314 | 75% | 43 | 87 s |
| python-duration-parser | 1/1 | 159,691 | 77% | 14 | 92 s |
| ruff-python3-fences (Ruff repository) | 1/1 | 408,513 | 86% | 21 | 97 s |
| rust-ledger-rounding-bug | 1/1 | 469,451 | 79% | 36 | 150 s |
| shell-script-filenames-with-spaces | 1/1 | 257,301 | 88% | 22 | 102 s |
| sqlite-weekly-report | 1/1 | 361,427 | 72% | 21 | 88 s |
| subagent-merge-keeps-files | 1/1 | 69,953 | 54% | 9 | 25 s |
| **All** | **13/13 (95% CI 77–100%)** | median 189,261 | 76% | 19.3 mean | 79 s median |

The first pass, on the installed helper, passed 8 of 11. Two failures were the agent's: it skipped tests it was asked to add while reporting "Ran 5 tests, OK", and it shipped a backup script that produced no snapshot. The third was OMP ending a turn over a malformed sub-agent call (now patch 0025). Single attempts vary a great deal — one task's tokens rose 180% between runs while another's fell 64% — so read pass rates with their intervals and compare token figures over repeats.

Across nine attempts of the two longest tasks on the patched helper, one stalled: a finished sub-agent's job never settled, and the parent waited until the 15-minute deadline. Three reruns did not reproduce it. It is listed below rather than attributed.

## GUI testing

An isolated build (its own bundle id and state root, sharing only the credential store) was driven through computer use on a disposable clone of a real Rust repository: a read-explain-add-a-test task with sub-agents, the Changes and Agents panels, Usage, Settings, history recall, Escape to stop, the menus, quit with daemons checked afterwards. The sub-agent data loss above was found this way. Other observations:

- The first prompt's `\(...\)` rendered as blank space: user messages are typeset as Markdown with math, and an unrenderable formula shows nothing.
- The Agents panel rebuilt every row view on each agent state change during a turn, costing 2.5–10 ms of main thread against the app's 4 ms frame budget.
- "Read code, add one test" spawned two sub-agents and duplicated their work in the parent; the eval corpus shows the same over-delegation on a two-request CSV task (6 requests, 3× the tokens).
- The Send button's accessibility press reports failure although it works.

## Compared with ChatGPT Desktop and Claude Desktop

From product knowledge as of mid-2026 rather than side-by-side testing; the competitors' apps were not driven in this pass, and their features change often:

| | Omperor | ChatGPT Desktop | Claude Desktop |
|---|---|---|---|
| Interface | Native AppKit, no web view; dense, developer-oriented | Native Mac app with broad consumer polish | Electron app; Chat, Cowork and Code surfaces |
| Accounts and models | Several subscriptions and API keys per provider, OpenAI and Anthropic side by side, explicit per-project choice | OpenAI models only | Anthropic models only |
| Coding agent | OMP runtime with sub-agents in isolated copies, worktree forks, Changes panel | Codex, local and cloud | Claude Code: worktrees, diff review, terminal, permission modes |
| Safety controls | No tool approvals; computer use off by default | Confirmations for consequential actions | Permission modes per session |
| Observability | Per-request token ledger, cache timer, agent tree, Observatory, headless evals with the same runtime | Plan usage limits; no per-request ledger | Plan usage limits; no per-request ledger in the app |
| Everyday features missing here | Find in transcript, content search, edit and regenerate, notifications, a growing composer, export, restore last conversation | — | — |

Omperor's advantage is the combination no competitor offers: several providers and accounts in one native app, full visibility into what an agent spent and did, and evals over the same runtime. Its gaps are the features users notice every hour.

### Browser and computer use, aligned with ChatGPT Desktop

ChatGPT Desktop's tool interfaces were read from its installed bundle (the app was not run). Its browser is a JavaScript `tab` with `tab.cua` (computer-use actions in screenshot pixels) and `tab.ax` (numbered accessibility elements); its computer tool takes app-scoped calls such as `get_app_state({app})`, `click({app, x, y})` and `press_key({app, key: "super+c"})`. OMP patch 0029 gives Omperor's agents the same vocabulary, so a model trained on either desktop's tools needs no relearning:

- **First-class tools.** The browser, and computer use once enabled in Settings, are tools the model calls directly. Before, both were reached through `bash` with `dyn browser …`, and every use began with discovery round trips. A session that does not opt in keeps upstream OMP's six-tool roster.
- **Browser.** `tab.cua.*` and `tab.ax.*` send real (trusted) mouse, wheel and keyboard events, in a headless tab or a spawned window, at screenshot pixels for any device pixel ratio; `tab.back`, `forward` and `reload` were added. Chromium key events now carry key codes, so Enter submits a form, arrows and Backspace act, and ⌘ shortcuts run their editing commands on macOS.
- **Keys.** One parser serves both tools and accepts Playwright (`ArrowDown`, `ControlOrMeta`), xdotool (`Return`, `Page_Down`, `KP_Enter`, `super+c`) and upper-case action names (`CTRL`, `ENTER`). In the browser `ctrl` means ⌘ on macOS, as in ChatGPT.
- **Computer.** ChatGPT's app-scoped calls lift onto OMP's desktop operations: `list_apps`, `get_app_state` (screenshot plus accessibility tree), `click` by point or `element_index`, `drag`, `scroll` by pages, `type_text`, `press_key`, `set_value` and `perform_secondary_action`. `app` now targets that app's frontmost window for every call; screenshots and input used to ignore it. Arguments may be JavaScript object literals, where the "JavaScript-like" language had accepted only strict JSON. The tool description, which named no methods at all, now lists them.
- **Safety.** OMP's computer-use policy already matched ChatGPT's tiers (confirm consequential actions; screen content is untrusted data). It now also keeps agents within the apps, sites and accounts the task involves. ChatGPT's per-app and per-site approval prompts have no Omperor counterpart yet.

A new eval, `browser-trusted-input`, serves a page that ignores scripted events: a canvas button that must be clicked by position, and a field submitted by a real Enter. `openai-codex/gpt-5.6-luna` at medium reasoning, three attempts each:

| Task | Before (patch 0028) | After (0030) |
|---|---|---|
| browser-checklist | 3/3; 82,226 tokens, 9 requests, 33 s (medians) | 3/3; 65,324 tokens (−21%), 7 requests, 20 s |
| browser-trusted-input | 0/3; 339,310 tokens, 24 requests, 151 s | 3/3; 55,135 tokens (−84%), 6 requests, 20 s |

An earlier run with patch 0029 alone also passed 6/6 (medians 65,597 and 102,164 tokens). After the final run no browser outlived its attempt.

Before, the agent reported the failure honestly each time ("the page ignored the browser tool's scripted click"). In the first run after, it opened a visible Chrome with `app.path` to get "real input". Synthetic input then reached only headless tabs, so the run failed until spawned windows got it too and the description said a headless tab suffices.

Two defects surfaced along the way:

- **Leaked browsers.** A browser the tool launched, headless or windowed, was closed only if the browser actor's thread ran before its process exited. When the environment daemon went idle and exited, it was cut off, and the browser was orphaned. Eight such Chromes were running here, about 7.8 GB with their helper processes, four of them visible windows left by eval attempts. OMP patch 0030 makes the daemon wait for its actor to close every tab. A test fails without it, and `omperor eval suite` now runs each attempt in its own process group and reaps it.
- **Grader coupling.** The checklist grader read results only as JSON inside `bash` output. A directly called tool returns large results as blob references, so the grader now follows them into the session's blob store.

## Open issues

In rough order of what a daily user would hit first:

1. **No tool-approval mode.** Agents run shell commands and edits with the user's permissions without asking. OMP has an approval mode; Omperor does not surface it.
2. **Anthropic unvalidated.** The saved Claude sign-in's refresh was rejected; sign in again, then run `omperor eval suite --model anthropic/claude-sonnet-5`. OMP also emits no Anthropic cache markers by default, so Claude sessions are likely uncached ([PROMPT_CACHE.md](PROMPT_CACHE.md)).
3. **Composer** is a three-line field that does not scroll; long prompts are clipped.
4. **No notifications or Dock badge** when a turn finishes or a confirmation waits in the background.
5. **Missing basics:** find in the transcript (⌘F), search by content, edit and resend, regenerate, export, restoring the last conversation at launch, generated titles (the title is the truncated first prompt).
6. **User messages typed as math** (`\(`, `$…$`). Upleft's `ParseOptions::detect_math` can be turned off for user messages; the transcript's parse worker needs to know the speaker.
7. **Intermittent sub-agent settlement stall** (1 in 9 long runs): see Evals.
8. **Agent over-delegation** costs tokens: sub-agents for trivial reads, and the parent repeating their work.
9. **Main-thread costs:** the Agents panel rebuilds all rows per state change; diff highlighting runs on the main thread; the sidebar is rebuilt per search keystroke.
10. **Stale sub-agent copies** are never collected: 13 from September hold about 800 MB in the installed app's store. `omp worktree` can clear them; housekeeping should.
11. **Account usage reads** (`/usage`, Settings › Accounts) hold the command queue for up to 45 s; a prompt sent meanwhile waits.
12. Window minimum width is 1080 pt and the sidebar cannot be hidden; "Load web images" and similar settings have no keyboard path; VoiceOver does not announce speakers or session states.
13. OMP's `central_truncation_spills_and_notrunc_explicitly_opts_out` dispatch test fails on the current patch stack, independent of this pass; so do `edit::replace::tests::ladder_handles_unicode_bom_crlf_and_indentation` and `edit::apply_patch::tests::repeated_sloppy_sections_merge_in_authored_order`.
14. **No per-app or per-site approval** for computer and browser use, as ChatGPT asks before an agent first touches an app or origin. Computer use is off by default and the model's policy asks before consequential actions, but nothing in the UI enforces scope.
15. **Computer use was not exercised against a live desktop** in this pass. The ChatGPT-style calls are covered by parser tests, and the browser by real-Chrome tests and evals.
16. Three headless Chromes, orphaned 6 to 7 hours before this pass by an earlier helper, are still running (about 2.5 GB). `pkill -f omp-webview` clears them when no session is using the browser.

## Releasing the source

Done in the tree: README, SECURITY and CONTRIBUTING; personal account labels, ids, emails and paths removed from docs and test data; investigation dumps containing personal data deleted. The OMP fork's pinned branch is published on the public fork `github.com/bitemyapp/oh-my-pi` (its 52 commits over upstream `omp2` were checked for credentials, account ids, emails and paths first), and `.gitmodules` points there instead of the private GitLab alias.

The public repository is published from the current tree, without its git history, which still holds the removed personal data.

Remaining:

- `vendor/upleft` is pinned to a commit on the public feature branch `omperor/math-dialect`; merge it into Upleft's `main` in time.
- The helper build assumes Apple silicon Homebrew paths for LLD.

## Reproducing

```sh
just release && ./target/release/omperor eval doctor
./target/release/omperor eval validate --exclude-tag external
./target/release/omperor eval suite --model openai-codex/gpt-5.6-luna --thinking medium --jobs 3 --output /tmp/evals/run
./target/release/omperor eval report /tmp/evals/run --compare /tmp/evals/previous
```

Set `OMP_DEBUG_REQUEST_DIR` to keep each Codex request body for cache analysis (patch 0026).
