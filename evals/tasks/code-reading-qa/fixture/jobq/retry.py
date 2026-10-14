"""When a job that raised TransientError runs again."""

import random
from dataclasses import dataclass


@dataclass(frozen=True)
class RetryPolicy:
    """Exponential backoff.

    ``max_retries`` counts re-runs after the first attempt, so a policy with
    ``max_retries=2`` runs a job at most three times.
    """

    max_retries: int = 5
    base_delay: float = 1.0
    factor: float = 2.0
    max_delay: float = 60.0
    jitter: bool = True

    @property
    def max_attempts(self) -> int:
        return self.max_retries + 1

    def should_retry(self, attempts: int) -> bool:
        """Whether to run again after ``attempts`` runs (counting the one that just failed)."""
        return attempts <= self.max_retries

    def delay_for(self, attempts: int, rng=random) -> float:
        """Seconds to wait before the next run, after ``attempts`` runs."""
        delay = min(self.max_delay, self.base_delay * self.factor ** (attempts - 1))
        if self.jitter:
            delay *= rng.uniform(0.5, 1.0)
        return delay
