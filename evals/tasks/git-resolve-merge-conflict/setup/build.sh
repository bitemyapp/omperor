#!/usr/bin/env bash
# Eval setup: builds a repository whose merge of feature/max-length into main
# stops on conflicts. Runs in the fresh, empty workspace; $1 is this directory
# (the task's setup/), which the agent never sees.
set -euo pipefail
source_dir=$(mktemp -d)
cp -R "$1"/. "$source_dir/"

g() {
    git -c core.hooksPath=/dev/null -c commit.gpgsign=false -c tag.gpgsign=false \
        -c merge.conflictStyle=diff3 -c rerere.enabled=false "$@"
}
commit() { # date author email message
    GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" GIT_AUTHOR_NAME="$2" GIT_AUTHOR_EMAIL="$3" \
        GIT_COMMITTER_NAME="$2" GIT_COMMITTER_EMAIL="$3" g commit --quiet --no-verify -m "$4"
}

g init --quiet -b main
# Local identity and settings so the agent's own commits work without the
# operator's hooks or signing keys.
git config user.name "Dana Whitfield"
git config user.email "dana@example.com"
git config commit.gpgsign false
git config tag.gpgsign false
git config core.hooksPath /dev/null

cp -R "$source_dir/base/." .
g add -A
commit "2024-04-18T09:12:00+02:00" "Dana Whitfield" "dana@example.com" "Release slugkit 1.2.0"

g switch --quiet -c feature/max-length
cp "$source_dir/feature/slugkit/slug.py" slugkit/slug.py
cp "$source_dir/feature/tests/test_slug.py" tests/test_slug.py
g add -A
commit "2024-04-22T14:03:00+02:00" "Marco Rossi" "marco@example.com" "Add max_length option to slugify"
cp "$source_dir/feature/CHANGELOG.md" CHANGELOG.md
cp "$source_dir/feature/README.md" README.md
g add -A
commit "2024-04-22T14:20:00+02:00" "Marco Rossi" "marco@example.com" "Document max_length"

g switch --quiet main
cp "$source_dir/main/slugkit/slug.py" slugkit/slug.py
cp "$source_dir/main/tests/test_slug.py" tests/test_slug.py
cp "$source_dir/main/CHANGELOG.md" CHANGELOG.md
g add -A
commit "2024-04-24T11:45:00+02:00" "Dana Whitfield" "dana@example.com" "Transliterate accented characters in slugs"

# Remember both tips for the grader (outside the working tree).
g rev-parse main > .git/eval-main-tip
g rev-parse feature/max-length > .git/eval-feature-tip
rm -rf "$source_dir"

export GIT_AUTHOR_DATE="2024-04-25T09:30:00+02:00" GIT_COMMITTER_DATE="2024-04-25T09:30:00+02:00"
if g merge --no-ff --no-edit feature/max-length > /dev/null 2>&1; then
    echo "setup error: the merge was expected to conflict" >&2
    exit 1
fi
test -f .git/MERGE_HEAD
test -n "$(git ls-files -u)"
echo "merge stopped with conflicts in: $(git diff --name-only --diff-filter=U | tr '\n' ' ')"
