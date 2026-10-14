"""The storage interface the queue relies on."""

from typing import Iterable, Protocol

from ..job import Job, JobStatus


class Store(Protocol):
    def put(self, job: Job) -> None:
        """Insert or replace a job."""

    def get(self, job_id: str) -> Job | None:
        """The job, or None."""

    def next_seq(self) -> int:
        """A number larger than every earlier one."""

    def next_ready(self, now: float) -> Job | None:
        """The most urgent job a worker may lease now: a queued job whose
        run_at has passed, or a running job whose lease has expired. Higher
        priority first, then earlier run_at, then submission order."""

    def all(self, status: JobStatus | None = None) -> Iterable[Job]:
        """Every job, optionally only those in one status."""


def ready(job: Job, now: float) -> bool:
    if job.status is JobStatus.QUEUED:
        return job.run_at <= now
    if job.status is JobStatus.RUNNING:
        return job.lease_expires_at is not None and job.lease_expires_at <= now
    return False


def urgency(job: Job):
    return (-job.priority, job.run_at, job.seq)
