#!/usr/bin/env bash
# Cross-compile the release binary for a 64-bit Pi inside a throwaway container.
# The repo is bind-mounted, and so is the host's cargo download cache
# (~/.cargo/registry + git), so nothing is fetched twice and the git
# dependencies already on this box are reused.  Extra args go to cargo build.
set -euo pipefail
cd "$(dirname "$0")/../../.."
IMG=opendeck-cross-aarch64
docker image inspect "$IMG" >/dev/null 2>&1 || docker build -t "$IMG" deploy/pi/cross
mkdir -p ~/.cargo/registry ~/.cargo/git
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp \
  -v "$PWD":/work \
  -v "$HOME/.cargo/registry":/cargo-home/registry -v "$HOME/.cargo/git":/cargo-home/git \
  "$IMG" cargo build --release -p opendeck-app --target aarch64-unknown-linux-gnu "$@"
ls -la target/aarch64-unknown-linux-gnu/release/opendeck
