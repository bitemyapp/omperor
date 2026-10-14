"""Grade what the page itself submitted after real pointer and key input, not answer claims."""
import json
import sys
from pathlib import Path


def grade(state):
    submitted = state.get('submitted')
    if not submitted:
        raise ValueError('the page never submitted: no real click and Enter reached it')
    if submitted.get('approved') is not True or submitted.get('trusted') is not True:
        raise ValueError(f'submission was not approved through real input: {submitted!r}')
    if submitted.get('code') != state['expected']:
        raise ValueError(f"submitted code {submitted.get('code')!r}, expected {state['expected']!r}")


if __name__ == '__main__':
    # Invalid or missing evidence must fail the command grader with a diagnostic.
    grade(json.loads(Path(sys.argv[1]).read_text()))
    print('PASS: the page accepted the approval and code through real input')
