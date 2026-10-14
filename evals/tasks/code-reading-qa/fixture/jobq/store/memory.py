"""A store in a dict, for tests and one-process setups."""

import copy
import itertools

from .base import ready, urgency


class MemoryStore:
    def __init__(self):
        self._jobs = {}
        self._seq = itertools.count(1)

    def put(self, job):
        self._jobs[job.id] = copy.deepcopy(job)

    def get(self, job_id):
        job = self._jobs.get(job_id)
        return copy.deepcopy(job) if job else None

    def next_seq(self):
        return next(self._seq)

    def next_ready(self, now):
        candidates = [job for job in self._jobs.values() if ready(job, now)]
        return copy.deepcopy(min(candidates, key=urgency)) if candidates else None

    def all(self, status=None):
        jobs = sorted(self._jobs.values(), key=lambda job: job.seq)
        return [copy.deepcopy(job) for job in jobs if status is None or job.status is status]
