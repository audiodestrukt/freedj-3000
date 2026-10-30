#!/usr/bin/env bash
# Cross-compile the release binary for a Pi inside a throwaway container.
#
#   build.sh [aarch64|armv6] [extra cargo args]
# Both builds enable the SDL render backend (OPENDECK_RENDERER=sdl), the only
# one the VideoCore IV boards can use; wgpu stays in for the Pi 4/5.
#     aarch64  64-bit Pi OS (Pi 3/4/5, Zero 2 W)        — default
#     armv6    32-bit Pi OS on a Pi Zero W / Pi 1; needs target/pi-sysroot
#              from sysroot-from-pi.sh first
#
# The repo is bind-mounted, and so is the host's cargo download cache
# (~/.cargo/registry + git), so nothing is fetched twice and the git
# dependencies already on this box are reused.
set -euo pipefail
cd "$(dirname "$0")/../../.."
arch=${1:-aarch64}; [ $# -gt 0 ] && shift
case "$arch" in
  aarch64) IMG=opendeck-cross-aarch64; DF=Dockerfile;       TARGET=aarch64-unknown-linux-gnu; STRIP=aarch64-linux-gnu-strip ;;
  armv6)   IMG=opendeck-cross-armv6;   DF=Dockerfile.armv6; TARGET=arm-unknown-linux-gnueabihf; STRIP=arm-linux-gnueabihf-strip
           [ -d target/pi-sysroot/usr/lib ] || { echo "no target/pi-sysroot — run deploy/pi/cross/sysroot-from-pi.sh <pi-host> first" >&2; exit 1; } ;;
  *) echo "unknown arch $arch (aarch64|armv6)" >&2; exit 2 ;;
esac
docker image inspect "$IMG" >/dev/null 2>&1 || docker build -t "$IMG" -f "deploy/pi/cross/$DF" deploy/pi/cross
mkdir -p ~/.cargo/registry ~/.cargo/git
run() { docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp \
          ${OPENDECK_VERSION:+-e OPENDECK_VERSION="$OPENDECK_VERSION"} \
          -v "$PWD":/work \
          -v "$HOME/.cargo/registry":/cargo-home/registry -v "$HOME/.cargo/git":/cargo-home/git \
          "$IMG" "$@"; }
run cargo build --release -p opendeck-app --features renderer-sdl --target "$TARGET" "$@"
run "$STRIP" -o "target/$TARGET/release/opendeck-stripped" "target/$TARGET/release/opendeck"
ls -la "target/$TARGET/release/opendeck" "target/$TARGET/release/opendeck-stripped"
