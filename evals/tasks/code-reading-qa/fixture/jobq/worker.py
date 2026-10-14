"""Workers lease jobs, run their handlers, and report the outcome."""

import logging
import socket
import threading

from .errors import LeaseLost
from .registry import default_registry

log = logging.getLogger(__name__)

#: Longest lease a worker will take. A long lease keeps a job invisible for
#: that long after its worker crashes.
MAX_LEASE_SECONDS = 90.0


class Worker:
    def __init__(self, queue, registry=None, settings=None, name: str | None = None):
        self.queue = queue
        self.registry = registry or default_registry
        settings = settings or queue.settings
        self.settings = settings
        self.name = name or socket.gethostname()
        # The lease must outlive a few missed heartbeats, but stay short enough
        # that a crashed worker's job is picked up again soon.
        self.lease_seconds = max(3 * settings.heartbeat_seconds, min(settings.lease_seconds, MAX_LEASE_SECONDS))

    def run_once(self) -> bool:
        """Run one ready job, if there is one. Returns whether a job ran."""
        job = self.queue.lease(self.lease_seconds)
        if job is None:
            return False
        stop = threading.Event()
        beat = threading.Thread(target=self._heartbeat, args=(job, stop), daemon=True)
        beat.start()
        try:
            handler = self.registry.get(job.handler)
            handler(**job.args)
        except Exception as error:
            log.info("job %s (%s) attempt %d failed: %s", job.id, job.handler, job.attempts, error)
            stop.set()
            beat.join()
            self.queue.fail(job, error)
        else:
            stop.set()
            beat.join()
            self.queue.complete(job)
        return True

    def run(self, max_jobs: int | None = None) -> int:
        """Work until ``max_jobs`` jobs have run (forever if None)."""
        done = 0
        while max_jobs is None or done < max_jobs:
            if self.run_once():
                done += 1
            else:
                self.queue.clock.sleep(self.settings.poll_interval)
        return done

    def _heartbeat(self, job, stop: threading.Event) -> None:
        while not stop.wait(self.settings.heartbeat_seconds):
            try:
                self.queue.heartbeat(job, self.lease_seconds)
            except LeaseLost:
                log.warning("lost the lease on job %s", job.id)
                return
