"""Grades issue #41 (parse_duration and the `parse` command).

Runs the hidden unittest cases against a scratch copy of the workspace, runs
the project's own tests, and requires that the agent added parse tests.
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

TASK = Path(__file__).resolve().parent
WORKSPACE = Path.cwd()


def fail(message):
    print(f"FAIL: {message}")
    sys.exit(1)


own_tests = [path for path in (WORKSPACE / "tests").glob("test*.py")
             if re.search(r"\bparse_duration\b|[\"']parse[\"']", path.read_text(errors="replace"))]
if not own_tests:
    fail("no tests for parse_duration or the parse command were added under tests/")

with tempfile.TemporaryDirectory(prefix="timespan-grade-") as scratch:
    copy = Path(scratch) / "workspace"
    shutil.copytree(WORKSPACE, copy, ignore=shutil.ignore_patterns(".git", "__pycache__"))
    environment = dict(os.environ, PYTHONPATH=str(copy), PYTHONDONTWRITEBYTECODE="1", TIMESPAN_WORKSPACE=str(copy))
    environment.pop("PYTHONHOME", None)
    hidden_dir = Path(scratch) / "hidden"
    hidden_dir.mkdir()
    shutil.copy(TASK / "hidden" / "test_hidden_parse.py", hidden_dir)
    hidden = subprocess.run([sys.executable, "-m", "unittest", "-v", "test_hidden_parse"], cwd=hidden_dir,
                            env=environment, capture_output=True, text=True, timeout=120)
    print(hidden.stderr[-8000:])
    if hidden.returncode != 0:
        fail("hidden tests failed")
    own = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-t", "."], cwd=copy,
                         env=environment, capture_output=True, text=True, timeout=120)
    print(own.stderr[-3000:])
    if own.returncode != 0 or "Ran 0 tests" in own.stderr:
        fail("the project's own tests do not pass")

print("PASS: units, aliases, ordering, exact decimals, ASCII-only numbers, errors, CLI, round trip")
