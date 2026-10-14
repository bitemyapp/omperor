import unittest

from jobq.clock import FakeClock
from jobq.errors import PermanentError, TransientError
from jobq.job import JobStatus
from jobq.queue import Queue
from jobq.settings import Settings
from jobq.store import MemoryStore


class QueueTest(unittest.TestCase):
    def setUp(self):
        self.clock = FakeClock()
        self.queue = Queue(MemoryStore(), Settings(max_retries=1), self.clock)

    def test_priority_then_order(self):
        low = self.queue.submit("a")
        high = self.queue.submit("b", priority=5)
        later = self.queue.submit("c", priority=5)
        self.assertEqual([self.queue.lease(30).id for _ in range(3)], [high.id, later.id, low.id])

    def test_permanent_error_fails_without_retry(self):
        self.queue.submit("a")
        job = self.queue.lease(30)
        self.queue.fail(job, PermanentError("nope"))
        self.assertIs(self.queue.store.get(job.id).status, JobStatus.FAILED)

    def test_transient_error_is_retried_later(self):
        self.queue.submit("a")
        job = self.queue.lease(30)
        self.queue.fail(job, TransientError("busy"))
        self.assertIsNone(self.queue.lease(30))
        self.clock.advance(60)
        self.assertEqual(self.queue.lease(30).attempts, 2)

    def test_expired_lease_is_leased_again(self):
        self.queue.submit("a")
        first = self.queue.lease(30)
        self.clock.advance(31)
        again = self.queue.lease(30)
        self.assertEqual((again.id, again.attempts), (first.id, 2))
