"""The plan must be exactly A1 then A4: A2 does not fit after A1, done tasks are skipped."""
import json
import os
from pathlib import Path

answer = Path(os.environ['OMPEROR_EVAL_ANSWER_PATH']).read_text().strip()
if answer.startswith('```'):
    answer = '\n'.join(answer.splitlines()[1:-1])
result = json.loads(answer)
if result != {'scheduled': ['A1', 'A4'], 'hours_used': 7, 'hours_remaining': 1}:
    raise SystemExit(f'incorrect plan: {result!r}')
if Path('issues.csv').read_text().count('\n') != 7:
    raise SystemExit('issues.csv was modified')
print('PASS: priorities, capacity, dependency, closed-task exclusion, arithmetic')
