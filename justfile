# Omperor's day-to-day tasks. `just` alone lists them.

# Honor an explicit SDKROOT; otherwise use the newest Command Line Tools SDK
# compatible with the helper's LLD 22 linker. The macOS 27 CLT SDK also ships
# arm64e.x1 stubs, so selecting the newest SDK unconditionally breaks install.
sdk := env("SDKROOT", `ls -d /Library/Developer/CommandLineTools/SDKs/MacOSX*.sdk 2>/dev/null | sort -Vr | while IFS= read -r candidate; do stub="$candidate/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics.tbd"; if [ -f "$stub" ] && ! grep -Fq 'arm64e.x1-' "$stub"; then printf '%s' "$candidate"; break; fi; done`)
export SDKROOT := sdk

# List the recipes.
default:
    @just --list --unsorted

# Rebuild the local release app at target/Omperor Dev.app, rebuilding the OMP helper only if its inputs changed.
release:
    scripts/build-app.sh


# Build release and replace /Applications/Omperor.app, preserving the local Dev bundle.
install:
    #!/usr/bin/env bash
    set -euo pipefail
    app="$(scripts/build-app.sh --production)"
    staging="$(mktemp -d "${TMPDIR:-/tmp}/omperor-install.XXXXXX")"
    trap 'rm -rf "$staging"' EXIT
    bundle="$staging/Omperor.app"
    /usr/bin/ditto "$app" "$bundle"
    /usr/bin/codesign --force --deep --sign - "$bundle"
    /usr/bin/codesign --verify --deep --strict "$bundle"
    destination="/Applications/Omperor.app"
    elevate=(/usr/bin/env)
    if [[ ! -w /Applications || ( -e "$destination" && ! -w "$destination" ) ]]; then
        elevate=(sudo)
    fi
    "${elevate[@]}" /bin/rm -rf "$destination"
    "${elevate[@]}" /usr/bin/ditto "$bundle" "$destination"
    /usr/bin/codesign --verify --deep --strict "$destination"
    # The staging bundle has the production identity. Retire
    # that launch registration without deleting or stopping a running session.
    lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    legacy="$PWD/target/Omperor.app"
    if [[ -d "$legacy" ]]; then
        # Unregistering an already retired bundle can return -10814.
        "$lsregister" -u "$legacy" || echo "Legacy local bundle is not registered; continuing installation." >&2
    fi
    "$lsregister" -f "$destination"
    /usr/bin/swift scripts/update-installed-dock.swift "$legacy" "$destination"
    echo "Installation does not replace a running Omperor process." >&2
    echo "Finish active work, quit Omperor, then open /Applications/Omperor.app explicitly." >&2
    echo "Installed $destination"
# Rebuild the release bundle and open it.
run-release:
    open "$(scripts/build-app.sh)"

# The normal run path is release-optimized too.
run:
    scripts/run-app.sh

# Build and open a debug bundle only when a debugger requires it.
run-debug:
    open "$(scripts/build-app.sh --debug)"

# Build the debug bundle without opening it.
debug:
    scripts/build-app.sh --debug

# Run the app's unit tests, generating Upleft's Markdown corpus first if a fresh checkout lacks it.
test:
    test -f vendor/upleft/corpus/generated/docs/Docs__STATUS.md || (cd vendor/upleft && python3 scripts/build-corpus.py)
    cargo test

# Build the native Linux app using Sidestep.
linux:
    scripts/linux-cargo build --release --locked

# Verify the Linux app and its platform-independent unit tests.
test-linux:
    scripts/linux-cargo test --locked

# Exercise real pasteboard, native Paste, and Command-V in the installed release.
# Replaces clipboard contents; exits without starting inference or loading sessions.
test-installed-paste:
    /Applications/Omperor.app/Contents/MacOS/omperor --attachment-paste-smoke

# Regenerate the README's screenshots in docs/screenshots, headless, against the mock backend.
readme-shots:
    scripts/readme-shots.sh

# Regenerate macos/AppIcon.icns from assets/01-crowned-gate.svg.
icon:
    scripts/build-app-icon.sh
