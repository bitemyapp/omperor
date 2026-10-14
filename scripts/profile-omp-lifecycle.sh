#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
    echo "usage: $0 <run-label> <app-executable> <omp-executable> <perf-log.jsonl>" >&2
    exit 2
fi

RUN_LABEL="$1"
APP_EXECUTABLE="$2"
OMP_EXECUTABLE="$3"
PERF_LOG="$4"
PROMPT="Reply with exactly: pong"

if [[ ! -x "$APP_EXECUTABLE" ]]; then
    echo "app executable is not runnable: $APP_EXECUTABLE" >&2
    exit 2
fi
if [[ ! -x "$OMP_EXECUTABLE" ]]; then
    echo "OMP executable is not runnable: $OMP_EXECUTABLE" >&2
    exit 2
fi

OMPEROR_PERF_RUN="$RUN_LABEL" \
OMPEROR_PERF_LOG="$PERF_LOG" \
OMPEROR_OMP_EXECUTABLE="$OMP_EXECUTABLE" \
    "$APP_EXECUTABLE" &
APP_PID=$!

cleanup() {
    if kill -0 "$APP_PID" 2>/dev/null; then
        osascript \
            -e 'tell application "System Events"' \
            -e "set appProcess to first application process whose unix id is $APP_PID" \
            -e 'set frontmost of appProcess to true' \
            -e 'keystroke "q" using command down' \
            -e 'end tell' >/dev/null 2>&1 || true
        for _ in {1..50}; do
            kill -0 "$APP_PID" 2>/dev/null || break
            sleep 0.1
        done
    fi
}
trap cleanup EXIT

ax_text() {
    osascript \
        -e 'tell application "System Events"' \
        -e "set appProcess to first application process whose unix id is $APP_PID" \
        -e 'get value of every static text of splitter group 1 of front window of appProcess' \
        -e 'end tell' 2>/dev/null || true
}

wait_for_text() {
    local expected="$1"
    local timeout="$2"
    local deadline=$((SECONDS + timeout))
    local current=""
    while (( SECONDS < deadline )); do
        if ! kill -0 "$APP_PID" 2>/dev/null; then
            echo "Omperor exited while waiting for: $expected" >&2
            return 1
        fi
        current="$(ax_text)"
        if [[ "$current" == *"$expected"* ]]; then
            echo "$RUN_LABEL: reached $expected"
            return 0
        fi
        sleep 0.1
    done
    echo "Timed out waiting for '$expected'; last UI text: $current" >&2
    return 1
}

wait_for_text "Ready •" 180

osascript \
    -e 'tell application "System Events"' \
    -e "set appProcess to first application process whose unix id is $APP_PID" \
    -e 'set frontmost of appProcess to true' \
    -e 'click button "New chat" of splitter group 1 of front window of appProcess' \
    -e 'end tell' >/dev/null

wait_for_text "New OMP session ready" 120

osascript \
    -e 'tell application "System Events"' \
    -e "set appProcess to first application process whose unix id is $APP_PID" \
    -e 'set frontmost of appProcess to true' \
    -e 'set inputField to first text field of splitter group 1 of front window of appProcess whose value of attribute "AXPlaceholderValue" is "Message OMP…"' \
    -e "set value of inputField to \"$PROMPT\"" \
    -e 'set focused of inputField to true' \
    -e 'key code 36' \
    -e 'end tell' >/dev/null

wait_for_text "Ready for another prompt" 240

osascript \
    -e 'tell application "System Events"' \
    -e "set appProcess to first application process whose unix id is $APP_PID" \
    -e 'set frontmost of appProcess to true' \
    -e 'keystroke "q" using command down' \
    -e 'end tell' >/dev/null

for _ in {1..300}; do
    kill -0 "$APP_PID" 2>/dev/null || break
    sleep 0.1
done
if kill -0 "$APP_PID" 2>/dev/null; then
    echo "Timed out waiting for Omperor shutdown" >&2
    exit 1
fi
wait "$APP_PID"
trap - EXIT
echo "$RUN_LABEL: complete"
