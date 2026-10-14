"""Jobs and their states."""

import enum
from dataclasses import dataclass, field


class JobStatus(str, enum.Enum):
    QUEUED = "queued"          # waiting for run_at, or for a worker
    RUNNING = "running"        # leased by a worker
    SUCCEEDED = "succeeded"
    FAILED = "failed"          # a non-transient error; never retried
    DEAD = "dead"              # transient errors used up every retry
    CANCELLED = "cancelled"


TERMINAL = frozenset({JobStatus.SUCCEEDED, JobStatus.FAILED, JobStatus.DEAD, JobStatus.CANCELLED})


@dataclass
class Job:
    id: str
    handler: str
    args: dict = field(default_factory=dict)
    status: JobStatus = JobStatus.QUEUED
    #: How many times the job has been leased, including the current run.
    attempts: int = 0
    #: Re-runs allowed after the first attempt, fixed when the job is submitted.
    max_retries: int = 0
    priority: int = 0
    run_at: float = 0.0
    lease_expires_at: float | None = None
    last_error: str | None = None
    #: Submission order, used to break ties between equally urgent jobs.
    seq: int = 0

    @property
    def finished(self) -> bool:
        return self.status in TERMINAL
