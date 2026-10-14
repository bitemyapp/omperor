"""Callbacks on job events: submitted, succeeded, retried, failed, dead, cancelled."""

import logging

log = logging.getLogger(__name__)

EVENTS = frozenset({"submitted", "succeeded", "retried", "failed", "dead", "cancelled"})


class Hooks:
    def __init__(self):
        self._callbacks = {event: [] for event in EVENTS}

    def on(self, event: str, callback) -> None:
        if event not in EVENTS:
            raise ValueError(f"unknown event {event!r}")
        self._callbacks[event].append(callback)

    def fire(self, event: str, job) -> None:
        for callback in self._callbacks[event]:
            try:
                callback(job)
            except Exception:  # a broken hook must not lose the job's state
                log.exception("hook for %s failed on job %s", event, job.id)
