"""Grades report.py: exact CSV on the task's database and on a hidden export
with different data (autumn DST change and a new year), computed independently;
the database must be left untouched."""
import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

TASK = Path(__file__).resolve().parent
sys.path.insert(0, str(TASK))
sys.dont_write_bytecode = True  # keep the task directory free of __pycache__
import datagen  # noqa: E402

WORKSPACE = Path.cwd()
problems = []


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def parse(text):
    import csv
    lines = [line.rstrip("\r") for line in text.split("\n")]
    while lines and not lines[-1].strip():
        lines.pop()
    return [row for row in csv.reader(lines)]


def compare(label, expected, output):
    actual = parse(output)
    if actual == expected:
        print(f"{label}: {len(expected) - 1} weeks match")
        return
    detail = []
    for index in range(max(len(expected), len(actual))):
        want = expected[index] if index < len(expected) else None
        got = actual[index] if index < len(actual) else None
        if want != got:
            detail.append(f"  row {index}: expected {want}, got {got}")
    problems.append(f"{label}: output differs from the expected summary\n" + "\n".join(detail[:12]))


script = WORKSPACE / "report.py"
if not script.is_file():
    print("FAIL: report.py is missing")
    sys.exit(1)
changed = subprocess.run(["git", "status", "--porcelain", "--", "shop.db"], capture_output=True, text=True).stdout
if changed.strip():
    problems.append(f"shop.db was modified: {changed.strip()}")

with tempfile.TemporaryDirectory(prefix="report-grade-") as scratch:
    scratch = Path(scratch)
    code = scratch / "code"
    shutil.copytree(WORKSPACE, code, ignore=shutil.ignore_patterns(".git", "__pycache__"))
    data = scratch / "exports"
    data.mkdir()
    task_db = data / "current.db"
    shutil.copy(WORKSPACE / "shop.db", task_db)
    hidden_db = data / "autumn.db"
    datagen.build_database(hidden_db, (TASK / "fixture" / "db" / "schema.sql").read_text(), datagen.HIDDEN)
    for label, database in (("task database", task_db), ("hidden export", hidden_db)):
        expected = datagen.expected_report(database)
        before = digest(database)
        result = subprocess.run([sys.executable, str(code / "report.py"), str(database)], cwd=code,
                                capture_output=True, text=True, timeout=60)
        if result.returncode != 0:
            problems.append(f"{label}: report.py exited {result.returncode}: {result.stderr[-1500:]}")
            continue
        compare(label, expected, result.stdout)
        if digest(database) != before:
            problems.append(f"{label}: report.py modified the database")

if problems:
    print("FAIL:\n- " + "\n- ".join(problems))
    sys.exit(1)
print("PASS: New York weeks across DST and the new year, paid real orders, refund rules, empty weeks, format")
