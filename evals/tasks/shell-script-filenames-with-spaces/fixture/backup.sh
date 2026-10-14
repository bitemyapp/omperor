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
# Exit status: 0 on success, 2 for usage errors, 1 for any other failure.

KEEP=5
if [ "$1" = "-n" ]; then
    KEEP=$2
    shift 2
fi
if [ $# -ne 2 ]; then
    echo "usage: backup.sh [-n KEEP] SOURCE DEST" >&2
    exit 2
fi
SRC=$1
DEST=$2

STAMP=$(date -u +%Y%m%d-%H%M%S)
SNAP=$DEST/backup-$STAMP
mkdir -p $SNAP

cd $SRC
for f in $(find . -type f); do
    case $f in
        *.tmp|*.swp) continue ;;
    esac
    mkdir -p $SNAP/$(dirname $f)
    cp -p $f $SNAP/$f
done
cd - > /dev/null

# Keep the newest $KEEP snapshots.
for old in $(ls -d $DEST/backup-* | sort -r | tail -n +$((KEEP + 1))); do
    rm -rf $old
done

echo $SNAP
