"""Grades the merge of feature/max-length into main.

The merge must be concluded (no MERGE_HEAD, no unmerged paths, nothing left
uncommitted), HEAD must be main with exactly the two original tips as parents,
no conflict markers may remain, and the committed tree must keep both
behaviours, both sides' tests, and both changelog entries.
"""
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

TASK = Path(__file__).resolve().parent
problems = []


def git(*args, check=True):
    result = subprocess.run(["git", *args], capture_output=True, text=True)
    if check and result.returncode != 0:
        raise SystemExit(f"FAIL: git {' '.join(args)}: {result.stderr.strip()}")
    return result.stdout.strip()


git_dir = Path(git("rev-parse", "--absolute-git-dir"))
main_tip = (git_dir / "eval-main-tip").read_text().strip()
feature_tip = (git_dir / "eval-feature-tip").read_text().strip()

if (git_dir / "MERGE_HEAD").exists():
    problems.append("the merge is still in progress (MERGE_HEAD exists)")
if git("ls-files", "-u"):
    problems.append("there are unmerged paths")
if git("status", "--porcelain", "--untracked-files=no"):
    problems.append("tracked files have uncommitted changes:\n" + git("status", "--short", "--untracked-files=no"))
if git("symbolic-ref", "--quiet", "--short", "HEAD", check=False) != "main":
    problems.append("HEAD is not the main branch")
parents = git("rev-list", "--parents", "-n", "1", "HEAD").split()[1:]
if parents != [main_tip, feature_tip]:
    problems.append(f"HEAD's parents are {parents}, expected [main tip {main_tip[:9]}, feature tip {feature_tip[:9]}]")
if git("rev-parse", "feature/max-length", check=False) != feature_tip:
    problems.append("feature/max-length was moved or deleted")

markers = git("grep", "-n", "-E", r"^(<<<<<<<|>>>>>>>|\|\|\|\|\|\|\||=======)( |$)", "HEAD", "--", ".", check=False)
if markers:
    problems.append("conflict markers are committed:\n" + markers)

changelog = git("show", "HEAD:CHANGELOG.md", check=False)
unreleased = changelog.split("## 1.2.0")[0]
if "max_length" not in unreleased or "transliterated" not in unreleased:
    problems.append("CHANGELOG.md's Unreleased section must keep both entries")
if "max_length" not in git("show", "HEAD:README.md", check=False):
    problems.append("the feature branch's README change is missing")

tests = git("show", "HEAD:tests/test_slug.py", check=False)
for name in ("test_basic", "test_collapses_separators", "test_transliterates_accents",
             "test_max_length_cuts_at_word_boundary", "test_max_length_truncates_a_long_first_word"):
    if not re.search(rf"def {name}\b", tests):
        problems.append(f"tests/test_slug.py at HEAD lost {name}")

with tempfile.TemporaryDirectory(prefix="slugkit-grade-") as scratch:
    tree = Path(scratch) / "tree"
    tree.mkdir()
    archive = subprocess.run(["git", "archive", "--format=tar", "HEAD"], capture_output=True, check=True).stdout
    subprocess.run(["tar", "-x", "-C", str(tree)], input=archive, check=True)
    environment = dict(os.environ, PYTHONPATH=str(tree), PYTHONDONTWRITEBYTECODE="1")
    own = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], cwd=tree,
                         env=environment, capture_output=True, text=True, timeout=60)
    print(own.stderr[-3000:])
    if own.returncode != 0 or "Ran 0 tests" in own.stderr:
        problems.append("the committed test suite does not pass")
    hidden = subprocess.run([sys.executable, str(TASK / "hidden" / "test_hidden_merge.py"), "-v"], cwd=scratch,
                            env=environment, capture_output=True, text=True, timeout=60)
    print(hidden.stderr[-4000:])
    if hidden.returncode != 0:
        problems.append("hidden behaviour tests failed on the committed tree")

if problems:
    print("FAIL:\n- " + "\n- ".join(problems))
    sys.exit(1)
print("PASS: merge concluded on main with both parents, both behaviours, tests and changelog entries")
