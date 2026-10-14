# Agent evaluations

`omperor eval` runs Omperor's agent headlessly against real inference and measures two things: how often it succeeds at a task, and how many tokens that costs. It uses the same OMP helper, account pinning, tools, and receipt ledger as the desktop app, but never opens a window, so it works over SSH and in CI.

## Quick start

```sh
just release                               # builds target/Omperor Dev.app and its helper
./target/release/omperor eval doctor       # is this machine ready?
./target/release/omperor eval suite --output /tmp/evals/first --tag smoke
```

`doctor` prints the state root, the helper it will use, and every signed-in account with OMP's own view of its credential (`Active`, `RefreshRequired`, ...). It exits 0 when a suite can run. With no accounts, sign in headlessly:

```sh
./target/release/omperor eval login openai-codex --label Work --default
```

`login` runs OMP's interactive sign-in: it prints an authorization URL, you finish in any browser, and paste the callback URL back. Nothing needs a local browser or a GUI. The new account is recorded with its label so suites can pick it by default.

Evals use Omperor Dev's state (`~/Library/Application Support/Omperor Dev`) unless `OMPEROR_STATE_DIR` names another absolute root; see [state isolation](STATE_ISOLATION.md). To evaluate with accounts that live in another edition's store, point `OMPEROR_STATE_DIR` at that root and `OMPEROR_OMP_EXECUTABLE` at the helper that edition runs, so one data store always sees one helper generation.

## Suites

```sh
omperor eval suite [TASK_OR_DIRECTORY...] --output DIR [options]
```

| Option | Meaning |
|---|---|
| `--model M` | Model to evaluate; repeat it for a matrix. Defaults to each task's `model`. |
| `--account ID` | Account to pin for its provider; one per provider. Defaults to the provider's default account, else its only one. |
| `--thinking LEVEL` | Reasoning level for every attempt. Defaults to the task's, else the backend's. |
| `--repeat N` | Attempts per task and model (default 1). Use 3–5 for a rate worth reading. |
| `--jobs N` | Attempts run at once (default 1). |
| `--task PATTERN`, `--tag TAG`, `--exclude-tag TAG` | Select tasks; all repeatable. `--exclude-tag external` skips tasks that clone from the network. |
| `--resume` | Continue an interrupted output directory. Finished attempts are kept; partial ones start over. |
| `--prune-workspaces` | Delete each workspace after its diff is recorded. |
| `--dry-run` | Print the planned attempts, accounts and levels, then exit. |
| `--json` | Print the summary as JSON instead of Markdown. |

With no task arguments, `suite` runs `evals/tasks`. Each attempt is a child `omperor eval run` with its own freshly provisioned workspace, so attempts are isolated from each other and can run in parallel. Ctrl-C stops scheduling, lets running attempts save an interrupted result, and writes the summary; `--resume` picks up from there. If an account fails to authenticate, later attempts that need it are skipped instead of failing one by one.

Progress goes to stderr. The final table goes to stdout and to `DIR/summary.md`, with the machine-readable `DIR/summary.json` beside it:

| Task | Model | Passed | Pass rate (95% CI) | Median tokens | Tokens per pass | Cached | Requests | Tool calls (errors) | Median time |
|---|---|---|---|---:|---:|---:|---:|---:|---:|

- **Pass rate** counts graded attempts only. An attempt whose account was rejected, whose setup failed, or that the operator interrupted is *infrastructure*, shown separately and never counted as an agent failure. The interval is a Wilson 95% score interval; with a handful of attempts it is wide, and that is the honest answer.
- **Tokens** are input + output + cache reads + cache writes from provider receipts, sub-agents included. They measure repeated inference, not unique context.
- **Tokens per pass** divides every token spent, failures included, by the number of passes: the cost of one success.
- **Cached** is the share of prompt tokens read from the provider's prompt cache.

Compare two runs, for example before and after a change to the agent harness:

```sh
omperor eval report /tmp/evals/after --compare /tmp/evals/before
```

Exit status: 0 when every graded attempt passed, 1 on any failure or infrastructure problem, 2 for an invalid invocation, 3 when nothing was graded.

### Artifacts

```
DIR/
  suite.json                      the plan: tasks, models, accounts, helper, state root
  summary.json, summary.md
  attempts/TASK/MODEL/NNN/
    attempt.json                  the normalized record the summary is built from
    spec.json                     the schema-1 request this attempt ran
    workspace/                    the agent's project directory (a git repository)
    setup.log, service-NAME.log
    run/                          `eval run` output: result.json, events.jsonl,
                                  answer.txt, changes.patch, check-N.std{out,err},
                                  sessions/*.oms (the OMP journals)
```

`changes.patch` holds everything the agent changed relative to the workspace's starting commit, committed or not, captured before any grader ran.

## Writing a task

A task is a directory:

```
evals/tasks/rust-ledger-rounding-bug/
  task.toml           prompt, workspace, checks
  fixture/            copied into a fresh workspace for every attempt
  grade.py, hidden/   graders and hidden tests; the agent never sees them
  reference/          a solution overlaid on the fixture (or reference.patch,
                      or reference.sh run in the workspace)
  reference-answer.txt  the answer graders read, for answer-based tasks
```

```toml
description = "Rust: fix two sign-dependent cent-rounding bugs behind a refund report."
tags = ["coding", "rust", "debugging"]
timeout_secs = 900          # 1–3600; covers startup and the agent's turn
thinking_level = "medium"   # optional
prompt = """A customer refunded order 1042 in full and still owes us a cent..."""

[workspace]
# Default: copy fixture/, then `git init` and commit it as the baseline.
# git = { repo = "https://github.com/astral-sh/ruff", rev = "fde6172..." }  # instead of a fixture
# git_init = false          # e.g. when setup builds the repository itself
setup = [["cargo", "fetch", "--offline"]]   # run in the workspace; output joins the baseline

[env]                       # for setup, services, the agent's tools, and graders
CARGO_NET_OFFLINE = "true"

[[services]]                # background processes on a loopback port
name = "web"
argv = ["python3", "{task}/server.py", "--port", "{port:web}"]

[[checks]]
type = "command"            # exit 0 passes; runs in the workspace after the agent stops
argv = ["python3", "{task}/grade.py"]
timeout_secs = 120

[[checks]]
type = "answer_contains"    # exact, case-sensitive substring of the final answer
text = "..."
```

Placeholders `{task}`, `{workspace}`, `{attempt}` and `{port:NAME}` expand in the prompt, setup, services and check arguments; other braces are left alone. Graders receive `OMPEROR_EVAL_ANSWER_PATH` (the final answer) and `OMPEROR_EVAL_RUN_DIR` (events, journals and the patch). Git repositories named by URL are cloned once into a shared cache (`OMPEROR_EVAL_CACHE`, default `eval-cache` under the state root) and every attempt gets a cheap shared clone at the pinned revision.

Good graders check behavior, not claims. Keep hidden tests out of `fixture/`, copy the workspace to a temporary directory before running anything that writes files, and reject the shortcuts an agent might take: deleted tests, hard-coded outputs, compatibility aliases instead of a rename. Then prove the grader can tell the difference, without spending any inference:

```sh
omperor eval validate [TASK_OR_DIRECTORY...]
```

`validate` provisions each task twice. The untouched fixture must fail at least one check, and the reference solution must pass all of them. When something is wrong it prints the grader's last lines and keeps the workspaces for inspection. `omperor eval tasks` lists what a directory holds.

## The bundled tasks

| Task | What it measures |
|---|---|
| `ping` | One request, no tools: the cheapest end-to-end check. |
| `csv-day-plan` | Reading a file and following scheduling rules exactly. |
| `browser-checklist` | Driving the native browser tool against a local page, with a screenshot and tab cleanup. |
| `browser-trusted-input` | A page that ignores scripted events: clicking a canvas button by position and pressing a real Enter (`tab.cua`/`tab.ax`). |
| `code-reading-qa` | Answering precise questions about a 30-file codebase past stale docs, without editing it. |
| `rust-ledger-rounding-bug` | Finding and fixing sign-dependent rounding bugs from a customer report, with a regression test. |
| `python-duration-parser` | Implementing a written spec with exact decimals and strict errors, plus tests. |
| `python-debug-from-traceback` | Fixing the root cause one level below where a traceback surfaces. |
| `go-rename-across-packages` | A rename across packages, interfaces, fakes and docs, leaving look-alikes alone. |
| `git-resolve-merge-conflict` | Concluding a conflicted merge so both branches' behavior survives. |
| `shell-script-filenames-with-spaces` | Hardening a bash 3.2 script for hostile file names. |
| `sqlite-weekly-report` | A SQL report with time zones across DST, graded on a hidden export too. |
| `ruff-python3-fences` (`external`) | A small change with tests in a large real repository; clones Ruff once. |

These are a starting set chosen to exercise the integrations a working day touches. Their pass rate is not a benchmark of general coding ability; report the task list, the number of attempts and the interval with any figure.

## Single runs

`suite` is built on `omperor eval run SPEC.json OUTPUT_DIRECTORY`, which runs one schema-1 request ([evals/schema.json](../evals/schema.json)) and writes one result ([evals/result.schema.json](../evals/result.schema.json)). Use it directly to script something a task directory cannot express. The workspace must already exist; the output directory must not.

| Field | Meaning |
|---|---|
| `schema_version`, `id`, `workspace`, `model`, `prompt` | Required. `workspace` is an existing absolute directory. |
| `account`, `thinking_level` | Pin them for comparable runs; the runtime must confirm both before the prompt is sent. |
| `timeout_secs` | Default 300, range 1–3600. |
| `checks` | `command` and `answer_contains` checks, as in `task.toml`. |
| `env` | Variables for the agent's tools and the graders. |
| `diff_baseline` | A commit to record the agent's changes against, before graders run. |
| `computer_use` | Default false. |
| `data_dir` | Another OMP credential and model store; defaults to the state root's. |

`omperor eval accounts` prints the desktop's saved account labels and defaults as JSON without contacting OMP. SIGINT and SIGTERM stop a run cleanly and save an interrupted result; graceful shutdown gets 20 seconds.

## Limits

- Runs are trusted local processes with the agent's normal file and network permissions. They are not a sandbox, and a grader is not tamper-proof. Use disposable workspaces, which `suite` always does.
- Receipts can miss tokens from failed or aborted requests and auxiliary calls. Attempts with unreported usage are flagged in the summary; their token figures are lower bounds.
- Journals in `run/sessions` can refer to blobs in the shared OMP data store, so a run directory is not a self-contained export.
- Computer-use tasks need a desktop session and OS permissions that a headless machine does not have.

Run the runner's own tests with `cargo test --bin omperor eval`, and a bundled grader's with `python3 -m unittest discover -s evals/tasks/browser-checklist -p 'test_*.py'` (likewise `browser-trusted-input`).
