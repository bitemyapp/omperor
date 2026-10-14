"""Grades the GetItem -> FetchItem and ItemNotFoundErr -> ErrItemNotFound rename.

On a scratch copy of the module: go build, go vet, gofmt; an AST check that no
old identifiers or doc-comment mentions remain while the wire method string
stays; the hidden behaviour test plus the module's own tests; README updates.
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
problems = []


def run(argv, cwd, environment):
    result = subprocess.run(argv, cwd=cwd, env=environment, capture_output=True, text=True, timeout=120)
    output = (result.stdout + result.stderr).strip()
    print(f"$ {' '.join(argv)} -> {result.returncode}\n{output[-4000:]}")
    return result.returncode, output


with tempfile.TemporaryDirectory(prefix="stockroom-grade-") as scratch:
    module = Path(scratch) / "module"
    shutil.copytree(WORKSPACE, module, ignore=shutil.ignore_patterns(".git"))
    environment = dict(os.environ, GOTOOLCHAIN="local", GOPROXY="off", GOWORK="off", GOFLAGS="-mod=mod")
    for argv in (["go", "build", "./..."], ["go", "vet", "./..."]):
        status, _ = run(argv, module, environment)
        if status != 0:
            problems.append(f"{' '.join(argv)} failed")
    status, unformatted = run(["gofmt", "-l", "."], module, environment)
    if status != 0 or unformatted:
        problems.append(f"gofmt -l reports: {unformatted}")
    status, _ = run(["go", "run", str(TASK / "checker" / "checkrename.go"), str(module)], scratch, environment)
    if status != 0:
        problems.append("old identifiers or comment mentions remain, or the wire name changed (see above)")
    hidden = module / "zz_hidden"
    hidden.mkdir()
    shutil.copy(TASK / "hidden" / "hidden_test.go", hidden / "hidden_test.go")
    status, output = run(["go", "test", "-count=1", "./..."], module, environment)
    if status != 0:
        problems.append("go test ./... failed (hidden behaviour test or the module's own tests)")
    elif "example.com/stockroom/zz_hidden" not in output:
        problems.append("the hidden test package did not run")

readme = (WORKSPACE / "README.md").read_text()
leftover = re.sub(r"Inventory\.GetItem", "", readme)
if re.search(r"\bGetItem\b|\bItemNotFoundErr\b", leftover):
    problems.append("README.md still documents GetItem or ItemNotFoundErr")
if "FetchItem" not in readme or "ErrItemNotFound" not in readme:
    problems.append("README.md does not document FetchItem and ErrItemNotFound")
if readme.count("Inventory.GetItem") < 1 or "Inventory.FetchItem" in readme:
    problems.append("README.md changed the wire method name")

if problems:
    print("FAIL:\n- " + "\n- ".join(problems))
    sys.exit(1)
print("PASS: renamed everywhere, wire name kept, GetItems/GetItemCount untouched, build/vet/fmt/tests clean")
