# Operating jobq

_Last reviewed: March 2023._

## Retries

Jobs whose handler raises `TransientError` are retried up to 5 times
(`DEFAULT_MAX_RETRIES` in `jobq/config.py`) with exponential backoff that
starts at 1 second and doubles each time, capped at 60 seconds, with jitter.
After that they are marked failed.

## Leases

A worker leases a job for 30 seconds (`DEFAULT_LEASE_SECONDS`) and sends a
heartbeat every 10 seconds while the handler runs. If the worker dies, the job
becomes visible again when the lease runs out. Set `JOBQ_LEASE` to change the
lease, for example `JOBQ_LEASE=2m` for slow report jobs.

## Dead jobs

`jobq list --status failed` shows jobs that will not run again. Re-submit
them after fixing the cause.
