#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="release"
CARGO_ARGS=(--release)
APP_NAME="Omperor Dev"

for argument in "$@"; do
    case "$argument" in
        --debug) PROFILE="debug" ;;
        --production) APP_NAME="Omperor" ;;
        *) echo "usage: $0 [--debug] [--production]" >&2; exit 2 ;;
    esac
done
if [[ "$PROFILE" == debug ]]; then CARGO_ARGS=(--profile dev); fi
if [[ "$APP_NAME" == Omperor ]]; then CARGO_ARGS+=(--features production); fi

cd "$PROJECT_DIR"
cargo build --manifest-path "$PROJECT_DIR/Cargo.toml" "${CARGO_ARGS[@]}"
OMP_HELPER="$("$PROJECT_DIR/scripts/build-omp-helper.sh")"

TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_DIR/target}"
APP_DIR="$TARGET_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"

rm -rf "$APP_DIR"
mkdir -p "$CONTENTS_DIR/Helpers" "$CONTENTS_DIR/MacOS" "$CONTENTS_DIR/Resources"
install -m 755 "$TARGET_DIR/$PROFILE/omperor" "$CONTENTS_DIR/MacOS/omperor"
if [[ "$PROFILE" == "debug" ]]; then
    # Keep local debug launches responsive without coupling the helper profile
    # to Omperor's: the launcher points at the release OMP build in target and
    # avoids copying/validating that large binary on every debug app rebuild.
    # Its signed path file avoids the out-of-bundle symlink macOS rejects during
    # strict bundle validation.
    install -m 755 "$PROJECT_DIR/scripts/omp-debug-launcher.sh" "$CONTENTS_DIR/Helpers/omp"
    printf '%s\n' "$OMP_HELPER" > "$CONTENTS_DIR/Helpers/omp-path"
else
    install -m 755 "$OMP_HELPER" "$CONTENTS_DIR/Helpers/omp"
fi
install -m 644 "$PROJECT_DIR/macos/Info.plist" "$CONTENTS_DIR/Info.plist"
if [[ "$APP_NAME" == Omperor ]]; then
    /usr/libexec/PlistBuddy -c 'Set :CFBundleName Omperor' "$CONTENTS_DIR/Info.plist"
    /usr/libexec/PlistBuddy -c 'Set :CFBundleDisplayName Omperor' "$CONTENTS_DIR/Info.plist"
    /usr/libexec/PlistBuddy -c 'Set :CFBundleIdentifier com.bitemyapp.omperor' "$CONTENTS_DIR/Info.plist"
fi
# The Markdown renderer's math fonts (vendor/upleft's docs/EMBEDDING.md): the
# app looks for them in its resources at launch.
MATH_FONTS="$PROJECT_DIR/vendor/upleft/vendor/downright/Vendor/SwiftMath/Sources/SwiftMath/mathFonts.bundle"
if [[ ! -d "$MATH_FONTS" ]]; then
    echo "missing $MATH_FONTS: run git submodule update --init vendor/upleft, then vendor/downright inside it" >&2
    exit 1
fi
/usr/bin/ditto "$MATH_FONTS" "$CONTENTS_DIR/Resources/mathFonts.bundle"
# The app icon is generated from assets/01-crowned-gate.svg by
# scripts/build-app-icon.sh and committed as macos/AppIcon.icns.
if [[ -f "$PROJECT_DIR/macos/AppIcon.icns" ]]; then
    install -m 644 "$PROJECT_DIR/macos/AppIcon.icns" "$CONTENTS_DIR/Resources/AppIcon.icns"
fi

/usr/bin/codesign --force --deep --sign - "$APP_DIR"
echo "$APP_DIR"
