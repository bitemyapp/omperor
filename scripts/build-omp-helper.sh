#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VENDOR_DIR="$PROJECT_DIR/vendor/oh-my-pi"
PATCH_FILES=(
    "$PROJECT_DIR/patches/oh-my-pi/0001-force-codex-http.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0003-codex-discovery-tool-capability.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0004-session-tool-protocol-fields.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0005-token-efficiency.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0006-background-completion-guidance.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0007-codex-client-metadata-strings.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0008-background-settlement-delivery.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0009-kernel-owned-explicit-async.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0010-cache-usage-telemetry.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0011-dynamic-anthropic-models.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0012-dynamic-device-lifecycle.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0013-rpc-terminal-tool-fault.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0014-computer-use-launch-override.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0015-browser-api-guidance.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0016-session-native-web-search.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0017-tool-screenshot-media-projection.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0018-atomic-discovery-cache-startup.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0019-partition-backend-disconnect.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0020-response-cache-timer.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0021-isolated-environment-state.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0022-concurrent-credential-startup.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0023-repository-ignore-boundaries.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0024-idle-abort-is-a-no-op.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0025-repairable-session-tool-arguments.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0026-debug-request-capture.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0027-isolated-daemon-idle-exit.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0028-codex-conversation-routing-headers.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0029-browser-and-computer-use-vocabulary.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0030-browser-lifetime.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0031-linux-embedded-python.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0032-linux-mv-xattrs.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0033-linux-desktop-and-sandbox-builds.patch"
    "$PROJECT_DIR/patches/oh-my-pi/0034-lockable-directory-handles.patch"
)

FORCE_BUILD=false
if [[ "${1:-}" == "--force" ]]; then
    FORCE_BUILD=true
elif [[ $# -ne 0 ]]; then
    echo "usage: $0 [--force]" >&2
    exit 2
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$VENDOR_DIR/target}"
if [[ "$TARGET_DIR" != /* ]]; then
    TARGET_DIR="$PROJECT_DIR/$TARGET_DIR"
fi
STAGING_DIR="$TARGET_DIR/omperor-source"
SOURCE_REV="$(git -C "$VENDOR_DIR" rev-parse HEAD)"
PATCH_SHA="$(sha256sum "${PATCH_FILES[@]}" | sha256sum | awk '{print $1}')"
STAGING_KEY="$SOURCE_REV:$PATCH_SHA"
HELPER_DIR="${OMPEROR_HELPER_DIR:-$PROJECT_DIR/target/omperor-helpers}"
INSTALLED_HELPER="$HELPER_DIR/omp"
BUILD_KEY_FILE="$HELPER_DIR/omp.build-key"

if [[ ! -f "$STAGING_DIR/.omperor-source" ]] || \
    ! grep -Fqx "$STAGING_KEY" "$STAGING_DIR/.omperor-source"; then
    rm -rf "$STAGING_DIR"
    git clone --quiet --shared --no-checkout "$VENDOR_DIR" "$STAGING_DIR"
    git -c advice.detachedHead=false -C "$STAGING_DIR" checkout --quiet --detach "$SOURCE_REV"
    for patch_file in "${PATCH_FILES[@]}"; do
        git -C "$STAGING_DIR" apply --unidiff-zero "$patch_file"
    done
    mkdir -p "$STAGING_DIR/vendor"
    printf '%s\n' "$STAGING_KEY" > "$STAGING_DIR/.omperor-source"
fi

# The embedded Python lives in the vendor checkout's ignored vendor/, so it
# survives restaging. The staged fetch script carries the patches (0031 runs
# the archive's interpreter under a Nix toolchain's loader).
PYTHON_DIR="$VENDOR_DIR/vendor/python"
if [[ ! -f "$PYTHON_DIR/pyo3-config.txt" ]]; then
    "$STAGING_DIR/crates/py/scripts/fetch-python.sh" "$VENDOR_DIR/vendor"
fi
ln -sfn "$PYTHON_DIR" "$STAGING_DIR/vendor/python"

BUILD_KEY="$({
    printf '%s\n' "$STAGING_KEY"
    sha256sum "$0" | awk '{print $1}'
    rustc -Vv
    cargo -V
    uname -m
    sha256sum \
        "$PYTHON_DIR/pyo3-config.txt" \
        "$PYTHON_DIR/.archive.stamp" \
        "$PYTHON_DIR/bundled/.requirements.stamp" | awk '{print $1}'
    printf 'RUSTFLAGS=%s\n' "${RUSTFLAGS-}"
    printf 'CARGO_ENCODED_RUSTFLAGS=%s\n' "${CARGO_ENCODED_RUSTFLAGS-}"
} | sha256sum | awk '{print $1}')"

if [[ "$FORCE_BUILD" == false ]] && [[ -x "$INSTALLED_HELPER" ]] && \
    [[ -f "$BUILD_KEY_FILE" ]] && grep -Fqx "$BUILD_KEY" "$BUILD_KEY_FILE"; then
    echo "$INSTALLED_HELPER"
    exit 0
fi

CARGO_TARGET_DIR="$TARGET_DIR" \
    just --justfile "$STAGING_DIR/justfile" --working-directory "$STAGING_DIR" build-release

BUILT_HELPER="$TARGET_DIR/release/omp"
mkdir -p "$HELPER_DIR"

# Cargo and the linker may refresh metadata on their output even when the
# executable bytes are unchanged. Omperor launches this stable copy so a
# no-op build does not replace the helper a running Omperor has mapped.
if [[ ! -f "$INSTALLED_HELPER" ]] || ! cmp -s "$BUILT_HELPER" "$INSTALLED_HELPER"; then
    TEMP_HELPER="$(mktemp "$HELPER_DIR/.omp.XXXXXX")"
    trap 'rm -f "$TEMP_HELPER"' EXIT
    install -m 755 "$BUILT_HELPER" "$TEMP_HELPER"
    mv -f "$TEMP_HELPER" "$INSTALLED_HELPER"
    trap - EXIT
fi

TEMP_KEY="$(mktemp "$HELPER_DIR/.omp.build-key.XXXXXX")"
trap 'rm -f "$TEMP_KEY"' EXIT
printf '%s\n' "$BUILD_KEY" > "$TEMP_KEY"
mv -f "$TEMP_KEY" "$BUILD_KEY_FILE"
trap - EXIT

echo "$INSTALLED_HELPER"
