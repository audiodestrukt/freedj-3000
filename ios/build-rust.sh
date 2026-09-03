#!/bin/sh
# Xcode "Build Rust" script phase: compile the opendeck-app staticlib for the
# arch/config Xcode is building and hand it to the linker.
#
# Assumes: rustup + the iOS targets installed (see ios/README.md), and this
# `ios/` dir lives at the repo root.
set -eu

REPO="$SRCROOT/.."
cd "$REPO"
# Xcode's build environment does NOT source the login shell, so cargo is almost
# never on PATH.  Cover both installs: rustup.rs (~/.cargo/bin) and Homebrew's
# rustup formula (shims in /opt/homebrew/opt/rustup/bin, nothing in ~/.cargo).
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/rustup/bin:/usr/local/opt/rustup/bin:$PATH"
command -v cargo >/dev/null || {
    echo "error: cargo not found on PATH — install rustup, or add its bin dir here" >&2
    exit 1
}

# Map Xcode's platform/arch to a Rust target triple.
if [ "$PLATFORM_NAME" = "iphonesimulator" ]; then
    case "$ARCHS" in
        arm64)  RUST_TARGET="aarch64-apple-ios-sim" ;;
        x86_64) RUST_TARGET="x86_64-apple-ios" ;;
        *) echo "error: unsupported simulator arch '$ARCHS'" >&2; exit 1 ;;
    esac
else
    RUST_TARGET="aarch64-apple-ios"   # device
fi

if [ "$CONFIGURATION" = "Release" ]; then
    PROFILE_FLAG="--release"; PROFILE_DIR="release"
else
    PROFILE_FLAG=""; PROFILE_DIR="debug"
fi

# Shown on the UTILITY screen (screen.rs APP_VERSION); build.rs makes cargo
# notice when it changes.
export OPENDECK_VERSION="${MARKETING_VERSION:-dev} (${CURRENT_PROJECT_VERSION:-0})"
echo "cargo build --lib --target $RUST_TARGET ($CONFIGURATION) — version $OPENDECK_VERSION"
# shellcheck disable=SC2086
cargo build -p opendeck-app --lib $PROFILE_FLAG --target "$RUST_TARGET"

# Copy the .a where the Xcode linker looks (LIBRARY_SEARCH_PATHS includes this).
mkdir -p "$BUILT_PRODUCTS_DIR"
cp "target/$RUST_TARGET/$PROFILE_DIR/libopendeck_app.a" "$BUILT_PRODUCTS_DIR/libopendeck_app.a"
echo "→ $BUILT_PRODUCTS_DIR/libopendeck_app.a"
