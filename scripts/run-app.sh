#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${1:-}" == "--rebuild-omp" ]]; then
    "$PROJECT_DIR/scripts/build-omp-helper.sh" --force >/dev/null
elif [[ $# -ne 0 ]]; then
    echo "usage: $0 [--rebuild-omp]" >&2
    exit 2
fi

# Day-to-day launches use the optimized application. Debug remains available
# explicitly through scripts/build-app.sh --debug for debugger-only work.
APP_DIR="$("$PROJECT_DIR/scripts/build-app.sh")"

open "$APP_DIR"
