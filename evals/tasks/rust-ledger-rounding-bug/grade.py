"""Grades the ledger rounding fix.

Copies the agent's crate (without build output) to a scratch directory, adds the
hidden tests, and runs the whole test suite offline. Also requires that the
agent added at least one regression test of its own and kept the crate free of
dependencies.
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

TASK = Path(__file__).resolve().parent
WORKSPACE = Path.cwd()


def fail(message):
    print(f"FAIL: {message}")
    sys.exit(1)


def count_tests(text):
    return len(re.findall(r"#\[test\]", text))


def tracked_rust_sources(revision):
    names = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", revision], capture_output=True, text=True, check=True
    ).stdout.split()
    return {name: subprocess.run(["git", "show", f"{revision}:{name}"], capture_output=True, text=True, check=True).stdout
            for name in names if name.endswith(".rs")}


def current_rust_sources():
    sources = {}
    for path in WORKSPACE.rglob("*.rs"):
        relative = path.relative_to(WORKSPACE)
        if relative.parts[0] in ("target", ".git"):
            continue
        sources[str(relative)] = path.read_text(errors="replace")
    return sources


manifest = tomllib.loads((WORKSPACE / "Cargo.toml").read_text())
if any(manifest.get(table) for table in ("dependencies", "dev-dependencies", "build-dependencies")):
    fail("the crate must stay dependency-free")

baseline = sum(count_tests(text) for text in tracked_rust_sources("HEAD").values())
now = sum(count_tests(text) for text in current_rust_sources().values())
if now <= baseline:
    fail(f"no regression test was added ({now} #[test] functions, baseline {baseline})")

with tempfile.TemporaryDirectory(prefix="ledger-grade-") as scratch:
    crate = Path(scratch) / "crate"
    shutil.copytree(WORKSPACE, crate, ignore=shutil.ignore_patterns("target", ".git"))
    (crate / "tests").mkdir(exist_ok=True)
    shutil.copy(TASK / "hidden" / "hidden_rounding.rs", crate / "tests" / "hidden_rounding.rs")
    environment = dict(os.environ, CARGO_TARGET_DIR=str(Path(scratch) / "target"), CARGO_TERM_COLOR="never")
    result = subprocess.run(
        ["cargo", "test", "--offline", "--quiet"], cwd=crate, env=environment, capture_output=True, text=True
    )
    print(result.stdout[-6000:])
    print(result.stderr[-6000:], file=sys.stderr)
    if result.returncode != 0:
        fail("cargo test failed (hidden or existing tests)")
    # Run the hidden target by name too, so a manifest that disables test
    # discovery cannot skip it.
    hidden = subprocess.run(
        ["cargo", "test", "--offline", "--quiet", "--test", "hidden_rounding"],
        cwd=crate, env=environment, capture_output=True, text=True,
    )
    if hidden.returncode != 0 or "6 passed" not in hidden.stdout:
        print(hidden.stdout[-3000:], hidden.stderr[-3000:])
        fail("the hidden tests did not all run and pass")

print("PASS: credits round half away from zero, splits mirror and sum, full refunds zero the balance")
