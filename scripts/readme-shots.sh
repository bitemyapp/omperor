#!/usr/bin/env bash
# Regenerates the README's screenshots in docs/screenshots/.
#
# Builds the production app into its own target directory, copies it under a
# bundle id of its own, and runs `--readme-shots` against the mock backend's
# tour in a scratch home: no account, no inference, no window on screen, and
# the app is never activated. The demo project is recreated from
# tests/readme-demo at OMPEROR_SHOTS_PROJECT (default /Users/Shared/harbor),
# a path that reads well in the window's header.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEMO="${OMPEROR_SHOTS_PROJECT:-/Users/Shared/harbor}"
OUTPUT="$PROJECT_DIR/docs/screenshots"
MARKER=.git/omperor-readme-demo

# The demo project: the fixture's base committed, its work left uncommitted
# for the Changes panel. Only a directory this script made is replaced.
if [[ -e "$DEMO" && ! -e "$DEMO/$MARKER" ]]; then
    echo "$DEMO exists and is not a demo project this script made; set OMPEROR_SHOTS_PROJECT" >&2
    exit 1
fi
rm -rf "$DEMO"
mkdir -p "$DEMO"
/usr/bin/ditto "$PROJECT_DIR/tests/readme-demo/base" "$DEMO"
(
    cd "$DEMO"
    export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
    git init --quiet --initial-branch=main
    git add --all
    GIT_AUTHOR_DATE=2026-10-01T09:00:00Z GIT_COMMITTER_DATE=2026-10-01T09:00:00Z \
        git -c user.name="Harbor Dev" -c user.email=dev@example.com commit --quiet --message "Initial chat loop"
    git apply "$PROJECT_DIR/tests/readme-demo/work.patch"
    touch "$MARKER"
)

export CARGO_TARGET_DIR="${OMPEROR_SHOTS_TARGET:-$PROJECT_DIR/target-shots}"
APP="$("$PROJECT_DIR/scripts/build-app.sh" --production)"

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/omperor-shots.XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT
BUNDLE="$SCRATCH/Omperor.app"
/usr/bin/ditto "$APP" "$BUNDLE"
/usr/libexec/PlistBuddy -c 'Set :CFBundleIdentifier com.bitemyapp.omperor.shots' "$BUNDLE/Contents/Info.plist"
/usr/bin/codesign --force --deep --sign - "$BUNDLE" 2>/dev/null

HOME_DIR="$SCRATCH/home"
STATE="$HOME_DIR/state"
mkdir -p "$STATE"
touch "$STATE/.omperor-readme-shots-home"
sqlite3 "$STATE/omperor.sqlite3" "CREATE TABLE preferences (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
    INSERT INTO preferences VALUES ('developer_mode', 'true'), ('inference_backend', 'mock'), ('appearance', 'dark');"

RAW="$SCRATCH/raw"
(
    cd "$HOME_DIR"
    HOME="$HOME_DIR" CFFIXED_USER_HOME="$HOME_DIR" OMPEROR_STATE_DIR="$STATE" OMPEROR_PROJECT="$DEMO" \
        GIT_CONFIG_GLOBAL=/dev/null "$BUNDLE/Contents/MacOS/omperor" --readme-shots "$RAW" 2>&1 \
        | grep -E '^readme-shots' >&2
)

mkdir -p "$OUTPUT"
uv run --quiet --with pillow python "$PROJECT_DIR/scripts/frame-screenshots.py" "$RAW" "$OUTPUT"
rm -rf "$DEMO"
echo "$OUTPUT"
