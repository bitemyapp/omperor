"""The queue: submitting, leasing, and recording outcomes."""

import uuid

from .clock import SystemClock
from .errors import LeaseLost, TransientError
from .hooks import Hooks
from .job import Job, JobStatus
from .metrics import Metrics
from .settings import Settings


class Queue:
    def __init__(self, store, settings: Settings | None = None, clock=None, hooks: Hooks | None = None,
                 metrics: Metrics | None = None):
        self.store = store
        self.settings = settings or Settings()
        self.clock = clock or SystemClock()
        self.hooks = hooks or Hooks()
        self.metrics = metrics or Metrics()

    # -- producers ---------------------------------------------------------

    def submit(self, handler: str, args: dict | None = None, *, max_retries: int | None = None,
               priority: int = 0, delay: float = 0.0) -> Job:
        """Queue a job. Its retry budget is fixed now, from ``max_retries`` or
        the queue's settings."""
        policy = self.settings.retry_policy(max_retries)
        job = Job(
            id=uuid.uuid4().hex[:12],
            handler=handler,
            args=dict(args or {}),
            max_retries=policy.max_retries,
            priority=priority,
            run_at=self.clock.now() + delay,
            seq=self.store.next_seq(),
        )
        self.store.put(job)
        self.metrics.inc("submitted")
        self.hooks.fire("submitted", job)
        return job

    def cancel(self, job_id: str) -> bool:
        job = self.store.get(job_id)
        if job is None or job.finished or job.status is JobStatus.RUNNING:
            return False
        job.status = JobStatus.CANCELLED
        self.store.put(job)
        self.hooks.fire("cancelled", job)
        return True

    # -- workers -----------------------------------------------------------

    def lease(self, lease_seconds: float) -> Job | None:
        """Hand the most urgent ready job to a worker for ``lease_seconds``.
        Every lease counts as an attempt, including a re-lease after a worker
        died without reporting back."""
        job = self.store.next_ready(self.clock.now())
        if job is None:
            return None
        job.status = JobStatus.RUNNING
        job.attempts += 1
        job.lease_expires_at = self.clock.now() + lease_seconds
        self.store.put(job)
        self.metrics.inc("leased")
        return job

    def heartbeat(self, job: Job, lease_seconds: float) -> None:
        current = self.store.get(job.id)
        if current is None or current.status is not JobStatus.RUNNING or current.attempts != job.attempts:
            raise LeaseLost(job.id)
        if current.lease_expires_at is not None and current.lease_expires_at <= self.clock.now():
            raise LeaseLost(job.id)
        current.lease_expires_at = self.clock.now() + lease_seconds
        self.store.put(current)

    def complete(self, job: Job) -> None:
        job.status = JobStatus.SUCCEEDED
        job.lease_expires_at = None
        self.store.put(job)
        self.metrics.inc("succeeded")
        self.hooks.fire("succeeded", job)

    def fail(self, job: Job, error: BaseException) -> None:
        """Record a failed run. Transient errors are retried while the job's
        budget lasts; anything else fails the job for good."""
        job.last_error = f"{type(error).__name__}: {error}"
        job.lease_expires_at = None
        if not isinstance(error, TransientError):
            job.status = JobStatus.FAILED
            self.store.put(job)
            self.metrics.inc("failed")
            self.hooks.fire("failed", job)
            return
        policy = self.settings.retry_policy(job.max_retries)
        if policy.should_retry(job.attempts):
            job.status = JobStatus.QUEUED
            job.run_at = self.clock.now() + policy.delay_for(job.attempts)
            self.store.put(job)
            self.metrics.inc("retried")
            self.hooks.fire("retried", job)
        else:
            self.bury(job)

    def bury(self, job: Job) -> None:
        """Move a job whose retries are used up to the dead-letter state."""
        job.status = JobStatus.DEAD
        job.lease_expires_at = None
        self.store.put(job)
        self.metrics.inc("dead")
        self.hooks.fire("dead", job)
