"""Grades backup.sh against hostile file names, rotation rules and failure
handling. Everything runs inside a scratch directory; the script runs under
/bin/bash (3.2 on macOS) with a scratch working directory full of decoys."""
import datetime
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

WORKSPACE = Path.cwd()
SCRIPT = WORKSPACE / "backup.sh"
SNAPSHOT = re.compile(r"backup-\d{8}-\d{6}")
problems = []

# Relative paths (bytes allowed via surrogateescape-free str) -> content.
SOURCE_FILES = {
    "plain.txt": "plain\n",
    "with space.txt": "space\n",
    "  leading spaces.txt": "leading\n",
    "trailing space ": "trailing\n",
    "-rf": "dash rf\n",
    "--help.txt": "double dash\n",
    "-n": "dash n\n",
    "star*.txt": "star\n",
    "starX.txt": "star x\n",
    "q?.txt": "question\n",
    "[abc].txt": "brackets\n",
    "b.txt": "b\n",
    "brace{a,b}.txt": "brace\n",
    "dollar$HOME.txt": "dollar\n",
    "back\\slash.txt": "backslash\n",
    "new\nline.txt": "newline\n",
    "tab\tname.txt": "tab\n",
    "quote'single.txt": "single\n",
    'quote"double.txt': "double\n",
    "ümlaut café.txt": "unicode\n",
    "empty file": "",
    "keep.tmp.txt": "not scratch\n",
    "run me.sh": "#!/bin/sh\necho hi\n",
    "sub dir/nested -x/deep*.txt": "deep\n",
    "sub dir/.hidden file": "hidden\n",
    "sub dir/-inner": "inner\n",
    "-dir/--x y.txt": "dash dir\n",
    "line\nbreak dir/file.txt": "newline dir\n",
    "*/q.txt": "star dir\n",
}
SKIPPED = ["ignore.tmp", "sub dir/x.swp", "-dir/.swap.swp"]


def write_tree(root, files):
    for relative, content in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)


def snapshot_of(root):
    """{relative path: (bytes, executable)} for every file under root."""
    result = {}
    for directory, _, names in os.walk(root):
        for name in names:
            path = Path(directory) / name
            result[str(path.relative_to(root))] = (path.read_bytes(), bool(path.stat().st_mode & stat.S_IXUSR))
    return result


def entries(directory):
    return sorted(os.listdir(directory))


def run(args, cwd):
    return subprocess.run(["/bin/bash", str(SCRIPT), *args], cwd=cwd, capture_output=True, text=True, timeout=60,
                          env=dict(os.environ, LC_ALL="en_US.UTF-8"))


syntax = subprocess.run(["/bin/bash", "-n", str(SCRIPT)], capture_output=True, text=True)
if syntax.returncode != 0:
    print(f"FAIL: bash -n: {syntax.stderr}")
    sys.exit(1)

with tempfile.TemporaryDirectory(prefix="backup-grade-") as scratch:
    root = Path(scratch)
    cwd = root / "cwd"
    cwd.mkdir()
    decoys = ["copy", "with", "space.txt", "plain.txt", "-rf", "target"]
    for name in decoys:
        (cwd / name).write_text("decoy\n")

    # --- 1. hostile names, existing snapshots and foreign folders in DEST ---
    source = root / "My Files"
    write_tree(source, SOURCE_FILES)
    write_tree(source, {name: "scratch\n" for name in SKIPPED})
    (source / "run me.sh").chmod(0o755)
    before_source = snapshot_of(source)
    dest = root / "backup target"
    old = ["backup-20200101-000000", "backup-20210101-000000", "backup-20220101-000000", "backup-20230101-000000"]
    foreign = ["backup-notes", "backup-20200101-000000 copy", "backup-2019", "README.txt"]
    for name in old:
        write_tree(dest / name, {"old.txt": name})
    for name in foreign[:-1]:
        write_tree(dest / name, {"keep.txt": "keep\n"})
    (dest / "README.txt").write_text("do not delete\n")

    started = datetime.datetime.now(datetime.timezone.utc)
    result = run(["-n", "3", str(source), str(dest)], cwd)
    print(f"run 1: exit {result.returncode}\nstdout: {result.stdout!r}\nstderr: {result.stderr[-2000:]!r}")
    new = [name for name in entries(dest) if SNAPSHOT.fullmatch(name) and name not in old]
    if result.returncode != 0:
        problems.append(f"exit status {result.returncode} on a valid run")
    if len(new) != 1:
        problems.append(f"expected exactly one new snapshot in DEST, found {new}")
    else:
        snap = dest / new[0]
        stamp = datetime.datetime.strptime(new[0], "backup-%Y%m%d-%H%M%S").replace(tzinfo=datetime.timezone.utc)
        if abs((stamp - started).total_seconds()) > 120:
            problems.append(f"snapshot name {new[0]} is not the current UTC time")
        if result.stdout.rstrip("\n").split("\n")[-1] != str(snap):
            problems.append(f"last line of output is not the snapshot path {str(snap)!r}")
        copied = snapshot_of(snap)
        expected = {path: value for path, value in before_source.items()
                    if not (path.endswith(".tmp") or path.endswith(".swp"))}
        missing = sorted(set(expected) - set(copied))
        extra = sorted(set(copied) - set(expected))
        wrong = sorted(path for path in set(expected) & set(copied) if copied[path][0] != expected[path][0])
        if missing:
            problems.append(f"files missing from the snapshot: {missing}")
        if extra:
            problems.append(f"unexpected files in the snapshot: {extra}")
        if wrong:
            problems.append(f"files with wrong content in the snapshot: {wrong}")
        if "run me.sh" in copied and not copied["run me.sh"][1]:
            problems.append("permissions were not preserved (run me.sh lost its executable bit)")
    expected_dest = sorted(new + old[2:] + foreign)
    if entries(dest) != expected_dest:
        problems.append(f"DEST after rotation is {entries(dest)}, expected {expected_dest}")
    for name in foreign[:-1]:
        if (dest / name).is_dir() and not (dest / name / "keep.txt").exists():
            problems.append(f"the contents of {name!r} were touched")
    if snapshot_of(source) != before_source:
        problems.append("SOURCE was modified")
    if entries(cwd) != sorted(decoys):
        problems.append(f"files in the caller's working directory were changed: {entries(cwd)}")

    # --- 2. DEST that does not exist yet, nested, with spaces ---
    simple = root / "simple src"
    write_tree(simple, {"a b.txt": "x\n", "-c": "y\n"})
    fresh = root / "new dest" / "inner dir"
    result = run([str(simple), str(fresh)], cwd)
    made = [name for name in entries(fresh)] if fresh.is_dir() else []
    if result.returncode != 0 or len(made) != 1 or not SNAPSHOT.fullmatch(made[0]) \
            or snapshot_of(fresh / made[0]) != snapshot_of(simple):
        problems.append(f"a run into a new DEST did not produce one complete snapshot (exit {result.returncode}, "
                        f"found {made}, stderr {result.stderr[-300:]!r})")

    # --- 3. missing SOURCE: fail, rotate nothing, leave no snapshot ---
    guarded = root / "guarded dest"
    for name in old:
        write_tree(guarded / name, {"old.txt": name})
    result = run(["-n", "1", str(root / "no such source"), str(guarded)], cwd)
    if result.returncode == 0:
        problems.append("a missing SOURCE exited 0")
    if entries(guarded) != old:
        problems.append(f"a missing SOURCE changed DEST: {entries(guarded)}")

    # --- 4. a copy that fails: fail, rotate nothing, leave no snapshot ---
    if os.geteuid() != 0:
        broken = root / "broken src"
        write_tree(broken, {"ok.txt": "ok\n", "secret.txt": "no\n", "z last.txt": "z\n"})
        (broken / "secret.txt").chmod(0)
        guarded2 = root / "guarded dest 2"
        for name in old:
            write_tree(guarded2 / name, {"old.txt": name})
        result = run(["-n", "1", str(broken), str(guarded2)], cwd)
        (broken / "secret.txt").chmod(0o644)
        if result.returncode == 0:
            problems.append("a failed copy exited 0")
        if entries(guarded2) != old:
            problems.append(f"a failed copy rotated or left a partial snapshot: {entries(guarded2)}")

    # --- 5. usage errors ---
    result = run([], cwd)
    if result.returncode != 2:
        problems.append(f"no arguments should exit 2, got {result.returncode}")
    if entries(cwd) != sorted(decoys):
        problems.append(f"files in the caller's working directory were changed: {entries(cwd)}")

if problems:
    print("FAIL:\n- " + "\n- ".join(problems))
    sys.exit(1)
print("PASS: hostile names copied exactly, strict rotation, clean failure, usage, bash 3.2")
