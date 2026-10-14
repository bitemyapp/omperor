#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="${CARGO_TARGET_DIR:-$PROJECT_DIR/target}/Omperor Dev.app"
APP_PID=""
OSA_PID=""
TEST_HOME="$(mktemp -d /tmp/omperor-lifecycle.XXXXXX)"
HELPER_PIDS=""

cleanup() {
    [[ -z "$OSA_PID" ]] || /bin/kill -TERM "$OSA_PID" 2>/dev/null || true
    [[ -z "$APP_PID" ]] || /bin/kill -TERM "$APP_PID" 2>/dev/null || true
    # Only descendants observed under this test's process are owned here.
    for pid in $HELPER_PIDS; do /bin/kill -TERM "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT

if [[ "${1:-}" == "--skip-build" ]]; then
    shift
elif [[ -n "${1:-}" ]]; then
    echo "usage: $0 [--skip-build]" >&2
    exit 2
else
    "$PROJECT_DIR/scripts/build-app.sh" >/dev/null
fi
[[ $# -eq 0 ]] || { echo "usage: $0 [--skip-build]" >&2; exit 2; }

for key in CFBundleName CFBundleDisplayName CFBundleIdentifier; do
    expected="Omperor Dev"
    [[ "$key" != "CFBundleIdentifier" ]] || expected="com.bitemyapp.omperor.dev"
    actual="$(/usr/libexec/PlistBuddy -c "Print :$key" "$APP_DIR/Contents/Info.plist")"
    [[ "$actual" == "$expected" ]] || { echo "unexpected $key: $actual" >&2; exit 1; }
done

collect_children() {
    local parent="$1" child
    for child in $(/usr/bin/pgrep -P "$parent" || true); do
        case " $HELPER_PIDS " in
            *" $child "*) ;;
            *) HELPER_PIDS="$HELPER_PIDS $child" ;;
        esac
        collect_children "$child"
    done
}

for delay in 0.01 0.02 0.05 0.10 0.25 1.00 3.00 ready; do
    log="$TEST_HOME/quit-$delay.log"
    HELPER_PIDS=""
    HOME="$TEST_HOME" CFFIXED_USER_HOME="$TEST_HOME" OMPEROR_STATE_DIR="$TEST_HOME/state" OMPEROR_PROJECT="$TEST_HOME" "$APP_DIR/Contents/MacOS/omperor" --lifecycle-children >"$log" 2>&1 &
    APP_PID=$!
    if [[ "$delay" != ready ]]; then
        sleep "$delay"
        collect_children "$APP_PID"
        # Address only our PID, never another dev or installed application.
        /usr/bin/osascript -l JavaScript -e "ObjC.import('AppKit'); \$.NSRunningApplication.runningApplicationWithProcessIdentifier($APP_PID).terminate" >"$TEST_HOME/quit-$delay-automation.log" 2>&1 &
        OSA_PID=$!
        limit=400
    else
        limit=4000
    fi
    exited=false
    for ((attempt=0; attempt<limit; attempt++)); do
        collect_children "$APP_PID"
        if ! /bin/kill -0 "$APP_PID" 2>/dev/null; then exited=true; break; fi
        sleep 0.025
    done
    if [[ "$exited" != true ]]; then
        sample "$APP_PID" 1 1 -file "$TEST_HOME/hang-$APP_PID.txt" >/dev/null 2>&1 || true
        echo "quit after ${delay}: timed out; evidence $TEST_HOME" >&2
        exit 1
    fi
    wait "$APP_PID"
    APP_PID=""
    if [[ -n "$OSA_PID" ]]; then wait "$OSA_PID" || true; OSA_PID=""; fi
    for pid in $HELPER_PIDS; do
        for _ in {1..200}; do
            /bin/kill -0 "$pid" 2>/dev/null || break
            sleep 0.01
        done
        if /bin/kill -0 "$pid" 2>/dev/null; then
            /bin/ps -p "$pid" -o pid,ppid,state,command >&2
            echo "owned helper $pid survived shutdown; $log" >&2
            exit 1
        fi
    done
    if [[ "$delay" == ready ]]; then
        python3 - "$log" <<'PY'
import pathlib, sys
text = pathlib.Path(sys.argv[1]).read_text()
assert 'lifecycle-children ready=3 independent_owners=3' in text, text
assert 'shutdown=all-workers-reaped' in text, text
PY
    fi
    HELPER_PIDS=""
    echo "quit after ${delay}: ok"
done
echo "lifecycle evidence: $TEST_HOME"
