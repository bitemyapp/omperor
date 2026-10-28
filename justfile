# Omperor's day-to-day tasks. `just` alone lists them. Run them in the dev
# shell (`nix develop`), which has the toolchain, the linker setup and
# Aizome's fonts.

# Where `just install` puts Omperor.
prefix := env("PREFIX", env("HOME") / ".local")

# List the recipes.
default:
    @just --list --unsorted

# Build the OMP helper Omperor runs, at target/omperor-helpers/omp (only when its inputs changed).
helper:
    scripts/build-omp-helper.sh

# Run Omperor Dev (org.aizome.OmperorDev, state in ~/.local/share/omperor-dev), release-optimized.
run *args: helper
    cargo run --release --locked -- {{args}}

# Run Omperor Dev as a debug build, for a debugger.
run-debug *args: helper
    cargo run --locked -- {{args}}

# Run the tests: the app's own, and the window driven headless with the mock backend.
test *args:
    cargo test --locked {{args}}

# Check formatting-independent lints.
clippy:
    cargo clippy --locked --all-targets

# Build Omperor (org.aizome.Omperor, state in ~/.local/share/omperor) and install it, its helper,
# its desktop entry and its icon under {{prefix}}. A running Omperor keeps going until it's quit.
install: helper
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --release --locked --features production
    prefix="{{prefix}}"
    install -Dm755 target/release/omperor "$prefix/bin/omperor.new"
    mv -f "$prefix/bin/omperor.new" "$prefix/bin/omperor"
    install -Dm755 target/omperor-helpers/omp "$prefix/libexec/omperor/omp.new"
    mv -f "$prefix/libexec/omperor/omp.new" "$prefix/libexec/omperor/omp"
    install -Dm644 data/org.aizome.Omperor.svg "$prefix/share/icons/hicolor/scalable/apps/org.aizome.Omperor.svg"
    mkdir -p "$prefix/share/applications"
    # The session's PATH may not hold $prefix/bin: name the binary in full.
    sed "s|^Exec=omperor |Exec=$prefix/bin/omperor |" data/org.aizome.Omperor.desktop \
        > "$prefix/share/applications/org.aizome.Omperor.desktop.new"
    mv -f "$prefix/share/applications/org.aizome.Omperor.desktop.new" "$prefix/share/applications/org.aizome.Omperor.desktop"
    echo "Installed Omperor in $prefix"

# Remove what `just install` installed. Omperor's state stays in ~/.local/share/omperor.
uninstall:
    rm -f "{{prefix}}/bin/omperor" "{{prefix}}/share/applications/org.aizome.Omperor.desktop" \
        "{{prefix}}/share/icons/hicolor/scalable/apps/org.aizome.Omperor.svg"
    rm -rf "{{prefix}}/libexec/omperor"

# Redraw the README's screenshots in docs/screenshots: the app tests, headless, on the mock backend.
shots:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="$(mktemp -d)"
    OMPEROR_SHOTS="$dir" cargo test --locked --bin omperor app::tests
    for shot in main-dark main-light agents timeline math settings-metrics; do
        cp "$dir/$shot.png" docs/screenshots/
    done
    echo "The tests' other pictures are in $dir"
