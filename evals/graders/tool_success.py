"""Require a successful invocation of the named native tool in an eval trace."""
import json
import os
import sys
from pathlib import Path

events = Path(os.environ['OMPEROR_EVAL_RUN_DIR']) / 'events.jsonl'
for line in events.read_text().splitlines():
    event = json.loads(line)
    if event['event'] == 'tool_finished':
        data = event['data']
        if data.get('name') == sys.argv[1] and data.get('is_error') is False:
            print(f'PASS: native {sys.argv[1]} completed successfully')
            sys.exit(0)
raise SystemExit(f'No successful native {sys.argv[1]} invocation in trace')
