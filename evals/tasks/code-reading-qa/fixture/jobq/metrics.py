"""In-process counters, printed by ``jobq worker --stats``."""

from collections import Counter


class Metrics:
    def __init__(self):
        self.counts = Counter()

    def inc(self, name: str, amount: int = 1) -> None:
        self.counts[name] += amount

    def snapshot(self) -> dict:
        return dict(sorted(self.counts.items()))
