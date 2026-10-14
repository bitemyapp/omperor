import unittest

from jobq.retry import RetryPolicy


class RetryPolicyTest(unittest.TestCase):
    def test_budget_counts_reruns(self):
        policy = RetryPolicy(max_retries=2, jitter=False)
        self.assertEqual(policy.max_attempts, 3)
        self.assertEqual([policy.should_retry(n) for n in (1, 2, 3)], [True, True, False])

    def test_backoff_is_capped(self):
        policy = RetryPolicy(base_delay=1, factor=2, max_delay=5, jitter=False)
        self.assertEqual([policy.delay_for(n) for n in range(1, 6)], [1, 2, 4, 5, 5])
