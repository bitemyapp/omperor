# jobq

A small persistent job queue: submit named jobs, run them in workers with
leases and heartbeats, retry transient failures with backoff, and fire jobs on
cron schedules. Standard library only; jobs live in SQLite (`jobq.db` by
default).

```sh
jobq submit send_invoice --arg invoice_id=42
JOBQ_LEASE=1m jobq worker --handlers examples.tasks
jobq status 3f9c2a7d1b20
jobq list --status dead
```

Handlers raise `jobq.errors.TransientError` for failures worth retrying and
`PermanentError` (or anything else) for failures that are not.

Configuration comes from `jobq.settings.Settings`, overridable with `JOBQ_*`
environment variables (see `Settings.from_env`). Operational notes are in
`docs/OPERATIONS.md`.

Tests: `python3 -m unittest discover -s tests -t .`
