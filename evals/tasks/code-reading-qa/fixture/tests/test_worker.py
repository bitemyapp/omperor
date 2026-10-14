import unittest

from jobq.clock import FakeClock
from jobq.job import JobStatus
from jobq.queue import Queue
from jobq.registry import Registry
from jobq.settings import Settings
from jobq.store import MemoryStore
from jobq.worker import Worker


class WorkerTest(unittest.TestCase):
    def test_runs_a_job(self):
        registry = Registry()
        seen = []
        registry.register("hello", lambda name: seen.append(name))
        queue = Queue(MemoryStore(), Settings(), FakeClock())
        job = queue.submit("hello", {"name": "world"})
        self.assertTrue(Worker(queue, registry).run_once())
        self.assertEqual(seen, ["world"])
        self.assertIs(queue.store.get(job.id).status, JobStatus.SUCCEEDED)

    def test_unknown_handler_fails_the_job(self):
        queue = Queue(MemoryStore(), Settings(), FakeClock())
        job = queue.submit("missing")
        Worker(queue, Registry()).run_once()
        self.assertIs(queue.store.get(job.id).status, JobStatus.FAILED)
