import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('trusted_grader', Path(__file__).parent / 'grade.py')
grader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(grader)


class TrustedInputGraderTests(unittest.TestCase):
    def state(self, **submitted):
        return {'expected': 'K7Q2XZ', 'session': 'token',
                'submitted': {'approved': True, 'trusted': True, 'code': 'K7Q2XZ', **submitted}}

    def test_accepts_the_page_submission(self):
        grader.grade(self.state())

    def test_rejects_a_page_that_never_submitted(self):
        with self.assertRaisesRegex(ValueError, 'never submitted'):
            grader.grade({'expected': 'K7Q2XZ', 'session': 'token', 'submitted': None})

    def test_rejects_a_wrong_code(self):
        with self.assertRaisesRegex(ValueError, 'expected'):
            grader.grade(self.state(code='AAAAAA'))

    def test_rejects_an_unapproved_submission(self):
        with self.assertRaisesRegex(ValueError, 'not approved'):
            grader.grade(self.state(approved=False))


if __name__ == '__main__':
    unittest.main()
