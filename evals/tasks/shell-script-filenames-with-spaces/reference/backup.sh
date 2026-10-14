#!/bin/bash
#
# backup.sh - snapshot a directory and rotate old snapshots.
#
# Usage: backup.sh [-n KEEP] SOURCE DEST
#
# Copies every regular file under SOURCE (recursively, keeping relative paths
# and permissions) into a new snapshot directory DEST/backup-YYYYMMDD-HHMMSS
# named after the current UTC time, skipping editor and scratch leftovers
# (*.tmp, *.swp). DEST is created if needed. Then deletes the oldest
# snapshots so that at most KEEP (default 5) remain, counting the new one.
# Prints the new snapshot's path.
#
# Any file name works: spaces, newlines, glob characters, leading dashes.
# Only directories named exactly backup-YYYYMMDD-HHMMSS are counted or
# rotated; nothing else in DEST is touched. If SOURCE is missing or a copy
# fails, nothing is rotated and the partial snapshot is removed.
#
# Exit status: 0 on success, 2 for usage errors, 1 for any other failure.
# Works with bash 3.2 (macOS /bin/bash).

set -u

usage() {
    echo "usage: backup.sh [-n KEEP] SOURCE DEST" >&2
    exit 2
}

KEEP=5
if [ "${1-}" = "-n" ]; then
    [ $# -ge 2 ] || usage
    KEEP=$2
    shift 2
fi
[ $# -eq 2 ] || usage
case $KEEP in
    '' | *[!0-9]*) usage ;;
esac
[ "$KEEP" -ge 1 ] || usage
SRC=$1
DEST=$2

SNAP=
LIST=
cleanup() {
    [ -n "$LIST" ] && rm -f -- "$LIST"
}
trap cleanup EXIT

fail() {
    echo "backup.sh: $*" >&2
    [ -n "$SNAP" ] && rm -rf -- "$SNAP"
    exit 1
}

[ -d "$SRC" ] || { echo "backup.sh: source is not a directory: $SRC" >&2; exit 1; }
mkdir -p -- "$DEST" || { echo "backup.sh: cannot create $DEST" >&2; exit 1; }

STAMP=$(date -u +%Y%m%d-%H%M%S)
if [ -e "$DEST/backup-$STAMP" ]; then
    echo "backup.sh: $DEST/backup-$STAMP already exists" >&2
    exit 1
fi
mkdir -- "$DEST/backup-$STAMP" || { echo "backup.sh: cannot create the snapshot" >&2; exit 1; }
SNAP=$DEST/backup-$STAMP

# NUL-separated list of files, relative to SOURCE as ./path, so every name
# survives and none can be mistaken for an option.
LIST=$(mktemp "${TMPDIR:-/tmp}/backup.XXXXXX") || fail "cannot create a temporary file"
(cd -- "$SRC" && find . -type f ! -name '*.tmp' ! -name '*.swp' -print0) > "$LIST" \
    || fail "cannot list $SRC"

while IFS= read -r -d '' file; do
    case $file in
        */*) dir=${file%/*} ;;
        *) dir=. ;;
    esac
    mkdir -p -- "$SNAP/$dir" || fail "cannot create $SNAP/$dir"
    cp -p -- "$SRC/$file" "$SNAP/$file" || fail "cannot copy $file"
done < "$LIST"

# Rotate: only exact snapshot names, newest first by name.
pattern='backup-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9][0-9][0-9]'
for path in "$DEST"/backup-*; do
    name=${path##*/}
    case $name in
        $pattern) ;;
        *) continue ;;
    esac
    [ -d "$path" ] && [ ! -L "$path" ] && printf '%s\n' "$name"
done | sort -r | tail -n +$((KEEP + 1)) | while IFS= read -r name; do
    rm -rf -- "$DEST/$name" || exit 1
done || { echo "backup.sh: rotation failed" >&2; exit 1; }

printf '%s\n' "$SNAP"
