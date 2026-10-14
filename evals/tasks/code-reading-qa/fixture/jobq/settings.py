"""Runtime settings and their environment overrides."""

from dataclasses import dataclass, replace

from .durations import parse_duration
from .retry import RetryPolicy


@dataclass(frozen=True)
class Settings:
    db_path: str = "jobq.db"
    max_retries: int = 3
    retry_base_delay: float = 2.0
    retry_factor: float = 3.0
    retry_max_delay: float = 15.0
    retry_jitter: bool = False
    lease_seconds: float = 45.0
    heartbeat_seconds: float = 10.0
    poll_interval: float = 1.0

    def retry_policy(self, max_retries: int | None = None) -> RetryPolicy:
        """The backoff these settings describe, optionally with another retry budget."""
        return RetryPolicy(
            max_retries=self.max_retries if max_retries is None else max_retries,
            base_delay=self.retry_base_delay,
            factor=self.retry_factor,
            max_delay=self.retry_max_delay,
            jitter=self.retry_jitter,
        )

    @classmethod
    def from_env(cls, environ, base: "Settings | None" = None) -> "Settings":
        """``base`` (default: the defaults above) with ``JOBQ_*`` overrides applied."""
        settings = base or cls()
        changes = {}
        if "JOBQ_DB" in environ:
            changes["db_path"] = environ["JOBQ_DB"]
        if "JOBQ_MAX_RETRIES" in environ:
            changes["max_retries"] = int(environ["JOBQ_MAX_RETRIES"])
        if "JOBQ_LEASE" in environ:
            changes["lease_seconds"] = parse_duration(environ["JOBQ_LEASE"])
        if "JOBQ_HEARTBEAT" in environ:
            changes["heartbeat_seconds"] = parse_duration(environ["JOBQ_HEARTBEAT"])
        if "JOBQ_POLL" in environ:
            changes["poll_interval"] = parse_duration(environ["JOBQ_POLL"])
        if "JOBQ_RETRY_JITTER" in environ:
            changes["retry_jitter"] = environ["JOBQ_RETRY_JITTER"].strip().lower() in {"1", "true", "yes", "on"}
        return replace(settings, **changes)
