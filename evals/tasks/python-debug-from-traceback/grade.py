"""Grades the invoicer crash fix: hidden tests on a scratch copy, the project's
own tests, and at least one new regression test."""
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


def count_tests(texts):
    return sum(len(re.findall(r"^\s*def test", text, re.M)) for text in texts)


listing = subprocess.run(["git", "ls-tree", "-r", "--name-only", "HEAD", "tests/"],
                         capture_output=True, text=True, check=True).stdout.split()
baseline = count_tests(subprocess.run(["git", "show", f"HEAD:{name}"], capture_output=True, text=True).stdout
                       for name in listing if name.endswith(".py"))
current = count_tests(path.read_text(errors="replace") for path in (WORKSPACE / "tests").rglob("*.py"))
if current <= baseline:
    fail(f"no regression test was added under tests/ ({current} test functions, baseline {baseline})")

original_samples = {name: subprocess.run(["git", "show", f"HEAD:samples/{name}"], capture_output=True,
                                         text=True, check=True).stdout
                    for name in ("october.csv", "september.csv", "rates.csv")}

with tempfile.TemporaryDirectory(prefix="invoicer-grade-") as scratch:
    copy = Path(scratch) / "workspace"
    shutil.copytree(WORKSPACE, copy, ignore=shutil.ignore_patterns(".git", "__pycache__"))
    # Grade against the samples as the user sent them, even if they were edited.
    for name, text in original_samples.items():
        (copy / "samples" / name).write_text(text)
    environment = dict(os.environ, PYTHONPATH=str(copy), PYTHONDONTWRITEBYTECODE="1", INVOICER_WORKSPACE=str(copy))
    hidden_dir = Path(scratch) / "hidden"
    hidden_dir.mkdir()
    shutil.copy(TASK / "hidden" / "test_hidden_invoicer.py", hidden_dir)
    hidden = subprocess.run([sys.executable, "-m", "unittest", "-v", "test_hidden_invoicer"], cwd=hidden_dir,
                            env=environment, capture_output=True, text=True, timeout=120)
    print(hidden.stderr[-8000:])
    if hidden.returncode != 0:
        fail("hidden tests failed")
    own = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-t", "."], cwd=copy,
                         env=environment, capture_output=True, text=True, timeout=120)
    print(own.stderr[-3000:])
    if own.returncode != 0:
        fail("the project's own tests do not pass")

print("PASS: H:MM durations parsed at the source, unreadable values rejected with the line, totals correct")
