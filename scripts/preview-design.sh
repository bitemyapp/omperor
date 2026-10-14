#!/usr/bin/env bash
# The local development app with an in-memory fixture for reviewing the design.
set -euo pipefail
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_DIR"
PROFILE="debug"
CARGO_ARGS=(--locked --features design-preview)
if [[ "${1:-}" == "--release" ]]; then
    PROFILE="release"
    CARGO_ARGS+=(--release)
    shift
fi
cargo build "${CARGO_ARGS[@]}"
TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_DIR/target}"
APP_DIR="$TARGET_DIR/Omperor Dev.app"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
install -m 755 "$TARGET_DIR/$PROFILE/omperor" "$APP_DIR/Contents/MacOS/omperor"
install -m 644 "$PROJECT_DIR/macos/Info.plist" "$APP_DIR/Contents/Info.plist"
install -m 644 "$PROJECT_DIR/macos/AppIcon.icns" "$APP_DIR/Contents/Resources/AppIcon.icns"
/usr/bin/codesign --force --deep --sign - "$APP_DIR"
open "$APP_DIR" --args "$@"
