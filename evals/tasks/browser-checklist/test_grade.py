import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location('browser_grader', Path(__file__).parent / 'grade.py')
grader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(grader)


class BrowserGraderTests(unittest.TestCase):
    def events(self, failed=False):
        # Native browser output can be wrapped as JSON lines in a shell text block.
        output = {'content': [{'type': 'text', 'text': '\n'.join([
            json.dumps({'action': 'run', 'artifacts': [{'kind': 'screenshot', 'uri': 'artifact://test', 'byte_len': 42}]}),
            json.dumps({'action': 'close', 'result': {'remaining_tabs': 0}}),
        ])}]}
        return [
            {'event': 'tool_updated', 'data': {'call_id': 'call1', 'content': json.dumps(output)}},
            {'event': 'tool_finished', 'data': {'call_id': 'call1', 'is_error': failed, 'result': {'storage': 'spilled'}}},
        ]

    def test_grades_page_state_and_successful_wrapped_artifacts(self):
        grader.grade(['Check keyboard navigation'], self.events())

    def test_failed_tool_cannot_supply_screenshot_or_close_evidence(self):
        with self.assertRaisesRegex(ValueError, 'no retained screenshot'):
            grader.grade(['Check keyboard navigation'], self.events(failed=True))

    def test_unknown_tool_outcome_cannot_supply_success_evidence(self):
        with self.assertRaisesRegex(ValueError, 'no retained screenshot'):
            grader.grade(['Check keyboard navigation'], self.events(failed=None))

    def test_follows_results_spilled_to_the_blob_store(self):
        with tempfile.TemporaryDirectory() as directory:
            blobs = Path(directory)
            references = []
            for value in (
                {'kind': 'ok', 'value': {'action': 'run', 'artifacts': [
                    {'kind': 'screenshot', 'uri': 'artifact://test', 'byte_len': 42}]}},
                {'kind': 'ok', 'value': {'action': 'close', 'result': {'remaining_tabs': 0}}},
            ):
                text = json.dumps(value)
                digest = hashlib.sha256(text.encode()).hexdigest()
                (blobs / digest[:2] / digest[2:4]).mkdir(parents=True, exist_ok=True)
                (blobs / digest[:2] / digest[2:4] / digest).write_text(text)
                references.append({'storage': 'spilled', 'blob': {'hash': digest}})
            events = [{'event': 'tool_finished',
                       'data': {'call_id': f'call{index}', 'is_error': False, 'result': reference}}
                      for index, reference in enumerate(references)]
            grader.grade(['Check keyboard navigation'], events, blobs)
            with self.assertRaisesRegex(ValueError, 'no retained screenshot'):
                grader.grade(['Check keyboard navigation'], events)

    def test_duplicate_clicks_fail_even_with_screenshot_and_close(self):
        with self.assertRaisesRegex(ValueError, 'incorrect persisted checklist'):
            grader.grade(['Check keyboard navigation'] * 2, self.events())


if __name__ == '__main__':
    unittest.main()
