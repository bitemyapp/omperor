#!/usr/bin/env bash
# Regenerates the macOS app icon (macos/AppIcon.icns) from the source SVG.
# Run this after editing assets/01-crowned-gate.svg; commit the refreshed
# AppIcon.icns alongside the SVG. build-app.sh only copies the committed icns,
# so the app build stays deterministic and does not depend on this tooling.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SVG="${1:-$PROJECT_DIR/assets/01-crowned-gate.svg}"
OUT="${2:-$PROJECT_DIR/macos/AppIcon.icns}"

if [[ ! -f "$SVG" ]]; then
    echo "source icon not found: $SVG" >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
ICONSET="$WORK/AppIcon.iconset"
mkdir -p "$ICONSET"

# Rasterize with AppKit's own SVG support (scripts/render-app-icon.swift):
# gradients and the clip path render as drawn, the margin stays transparent,
# and a soft shadow is laid under the tile at every size.
swift "$PROJECT_DIR/scripts/render-app-icon.swift" "$SVG" "$ICONSET"

iconutil -c icns "$ICONSET" -o "$OUT"
echo "$OUT"
