#!/bin/bash
# Quick check: one snapshot of a small tree, with rotation down to two.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src/docs" "$work/dest/backup-20200101-000000" "$work/dest/backup-20210101-000000"
echo one > "$work/src/a.txt"
echo two > "$work/src/docs/b.txt"
echo scratch > "$work/src/c.tmp"
snapshot=$("$here/backup.sh" -n 2 "$work/src" "$work/dest")
test -f "$snapshot/a.txt"
test -f "$snapshot/docs/b.txt"
test ! -e "$snapshot/c.tmp"
test -d "$work/dest/backup-20210101-000000"
test ! -e "$work/dest/backup-20200101-000000"
echo "smoke test passed"
