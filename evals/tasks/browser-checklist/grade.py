"""Grade persisted page state and successful browser artifacts, not answer claims."""
import json
import os
import sys
from pathlib import Path


def spilled(value, blobs):
    """Contents of a tool result OMP spilled to the session's blob store, if any."""
    digest = value.get('blob', {}).get('hash', '') if value.get('storage') == 'spilled' else ''
    if blobs is None or len(digest) != 64 or not all(c in '0123456789abcdef' for c in digest):
        return None
    try:
        return json.loads((blobs / digest[:2] / digest[2:4] / digest).read_text())
    except (OSError, ValueError):
        return None


def objects(value, blobs=None):
    if isinstance(value, dict):
        yield value
        # The browser tool called directly returns large results as blob references.
        stored = spilled(value, blobs)
        if stored is not None:
            yield from objects(stored, blobs)
        for child in value.values():
            yield from objects(child, blobs)
    elif isinstance(value, list):
        for child in value:
            yield from objects(child, blobs)
    elif isinstance(value, str):
        for line in value.splitlines():
            try:
                parsed = json.loads(line)
            except ValueError:
                continue
            # Only descend into structured JSON, avoiding repeated string decoding.
            if isinstance(parsed, (dict, list)):
                yield from objects(parsed, blobs)


def grade(state, events, blobs=None):
    if state != ['Check keyboard navigation']:
        raise ValueError(f'incorrect persisted checklist: {state!r}')
    successful = {e['data']['call_id'] for e in events
                  if e['event'] == 'tool_finished' and e['data'].get('is_error') is False}
    outputs = [obj for e in events if e['event'] in ('tool_updated', 'tool_finished')
               and e['data']['call_id'] in successful
               for obj in objects(e['data'].get('content', e['data'].get('result')), blobs)]
    if not any(obj.get('kind') == 'screenshot' and obj.get('byte_len', 0) > 0
               and obj.get('uri') for obj in outputs):
        raise ValueError('no retained screenshot in successful tool output')
    if not any(obj.get('action') == 'close' and obj.get('result', {}).get('remaining_tabs') == 0
               for obj in outputs if isinstance(obj.get('result'), dict)):
        raise ValueError('no successful browser close with zero remaining tabs')


if __name__ == '__main__':
    run = Path(os.environ['OMPEROR_EVAL_RUN_DIR'])
    # Invalid or missing evidence must fail the command grader with a diagnostic.
    grade(json.loads(Path(sys.argv[1]).read_text()),
          [json.loads(line) for line in (run / 'events.jsonl').read_text().splitlines()],
          run / 'sessions' / 'blobs')
    print('PASS: persisted checklist, retained screenshot, and closed browser')
