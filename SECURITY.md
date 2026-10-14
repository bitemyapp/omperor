# Security

Omperor is experimental software that lets a language model run commands on
your computer. This page describes what it can do, where it keeps your
credentials, what it sends over the network, and how to report a
vulnerability.

## Trust model

**Agents act with your permissions, without a sandbox.** Each session's agent
works in the session's project folder through OMP's tools: `bash`, `read`,
`write`, `edit`, `ast_edit`, `glob`, `grep`, `browser`, `task` (sub-agents),
and a few others. They run as your user account. Nothing confines them to the
project folder, and Omperor does not ask before a command runs. A command can
read, change or delete anything your account can, and reach any network host.

Anything the agent reads can steer it: repository files, web pages, command
output, attachments. Treat a session as a terminal you have handed to the
model. Keep the project under version control, review changes in the Changes
panel, and fork risky work into a worktree.

On first launch, before you add a project, the app works in the checkout it
was built from (or `OMPEROR_PROJECT`, or your home folder). Add the folder you
mean with **New project** before sending a prompt.

Further points:

- **Browser.** The `browser` tool is on by default. It loads whatever pages the
  agent chooses. Its parameters also let the agent start a browser binary or
  attach to an existing Chrome DevTools Protocol endpoint, which would expose
  that browser's signed-in sessions.
- **Computer use** is off by default. Settings → Agent → **Allow computer use
  in new sessions** turns it on for sessions started afterwards. Agents can
  then capture the screen and control native apps, within the permissions
  macOS grants the app.
- **Omperor's own tools** (`omperor_*`: list, delete and archive sessions and
  projects) are off unless you enable them for one session with `/omperor`.
  Deleting sessions or archives and archiving a project ask for confirmation.
- **Evals** run with the same permissions. They are not a sandbox, and a
  grader is not tamper-proof. Tasks tagged `external` clone repositories from
  the network. See [docs/EVALS.md](docs/EVALS.md).

## Credentials and local data

OMP performs sign-in and keeps the credentials: OAuth tokens, API keys and
pasted access tokens. They live in OMP's store inside the state root (see
[docs/STATE_ISOLATION.md](docs/STATE_ISOLATION.md)):

- `omp/data/credentials.db` holds them encrypted.
- `omp/data/credentials.key`, an owner-only file beside it, holds the key.
  Omperor starts OMP with `OMP_LLM_KEY_SOURCE=local-file` unless that variable
  is already set. The protection is your account's file permissions: any
  process running as you can read both files.

Omperor's own database, `omperor.sqlite3`, never holds a secret. It records
account IDs, labels, defaults, and the principal OMP reports for each account,
which for some providers is the account email.

Conversations are stored unencrypted. OMP's session journals, including tool
output, are under `omp/sessions`. Pasted clipboard data is saved under
`omp/data/attachments`; dropped files are referenced where they are.

## Network

Omperor has no telemetry, analytics or update checks. Network traffic comes
from these sources:

- **The OMP helper** talks to the providers you sign in to: OpenAI and
  Anthropic. That covers sign-in, inference, model discovery, and the plan and
  usage figures shown on account cards. OMP can export OpenTelemetry data, but
  only when `OTEL_EXPORTER_OTLP_ENDPOINT` (or a per-signal variant) is set in
  its environment. Omperor does not set one, but the helper inherits
  Omperor's environment.
- **Agent tools** reach whatever hosts the agent's commands and browser
  contact.
- **Images in the transcript.** With Settings → Appearance → **Show images**
  on (the default), Omperor itself fetches any `http` or `https` image that
  appears in a message's Markdown, including the agent's replies, with a
  plain GET request. The image's host sees your IP address, and anything the
  agent placed in the URL. With the setting on, Markdown images that name an
  absolute local path or a `file://` URL are also read from disk and
  displayed. Turn the setting off to stop both.
- **Links and sign-in pages** open in your default browser only when you click
  them or start a sign-in.

## Reporting a vulnerability

Please do not open a public issue. Open a private GitHub security advisory on
this repository instead: the **Security** tab, then **Report a
vulnerability**. Include the affected commit, the steps to reproduce, and what
an attacker gains.

Only the current `main` branch is supported. There are no releases yet.

Vulnerabilities in OMP itself, in Upleft or in Sidestep may belong upstream as
well. Report them here if Omperor's patches or its use of those components
are involved.
