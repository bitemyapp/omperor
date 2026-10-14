"""Grades the jobq code-reading answers (verified by running the code: 4 runs
ending dead, waits of 2, 6 and 15 s, a 90 s lease) and checks that the agent
changed no files."""
import json
import os
import re
import subprocess
import sys
from pathlib import Path

EXPECTED = {
    "handler_invocations": 4,
    "final_status": "dead",
    "retry_delays_seconds": [2, 6, 15],
    "effective_lease_seconds": 90,
}
problems = []


def load_answer(text):
    text = text.strip()
    fenced = re.search(r"```(?:json)?\s*\n(.*?)\n\s*```", text, re.S)
    if fenced:
        text = fenced.group(1)
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        start, end = text.find("{"), text.rfind("}")
        if start == -1 or end <= start:
            raise
        return json.loads(text[start:end + 1])


def number(value):
    if isinstance(value, bool):
        raise ValueError("boolean")
    if isinstance(value, (int, float)):
        return float(value)
    return float(str(value).strip())


def same_number(value, want):
    try:
        return abs(number(value) - want) < 1e-9
    except (TypeError, ValueError):
        return False


answer_text = Path(os.environ["OMPEROR_EVAL_ANSWER_PATH"]).read_text()
try:
    answer = load_answer(answer_text)
except (json.JSONDecodeError, ValueError) as error:
    print(f"FAIL: the answer is not a JSON object: {error}\n{answer_text[:500]!r}")
    sys.exit(1)
if not isinstance(answer, dict):
    print("FAIL: the answer is not a JSON object")
    sys.exit(1)

missing = sorted(set(EXPECTED) - set(answer))
if missing:
    problems.append(f"missing keys: {missing}")
if "handler_invocations" in answer and not same_number(answer["handler_invocations"], 4):
    problems.append(f"handler_invocations is {answer['handler_invocations']!r}")
if "final_status" in answer:
    status = str(answer["final_status"]).strip().lower().removeprefix("jobstatus.")
    if status != "dead":
        problems.append(f"final_status is {answer['final_status']!r}")
if "retry_delays_seconds" in answer:
    delays = answer["retry_delays_seconds"]
    if not (isinstance(delays, list) and len(delays) == 3
            and all(same_number(got, want) for got, want in zip(delays, [2, 6, 15]))):
        problems.append(f"retry_delays_seconds is {delays!r}")
if "effective_lease_seconds" in answer and not same_number(answer["effective_lease_seconds"], 90):
    problems.append(f"effective_lease_seconds is {answer['effective_lease_seconds']!r}")

status = subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True, check=True).stdout
if status.strip():
    problems.append(f"files were changed:\n{status}")

if problems:
    print("FAIL:\n- " + "\n- ".join(problems))
    sys.exit(1)
print("PASS: all four answers correct and the tree is unchanged")
