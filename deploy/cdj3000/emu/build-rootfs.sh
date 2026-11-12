#!/usr/bin/env bash
# Build the cdj3k-emu guest rootfs (Containerfile) for arm64 and export it as
# a tar: build-rootfs.sh OUT.tar
# Uses podman, or docker with buildx; on an x86 host both need qemu-user
# binfmt (apt install qemu-user-static), or run on an arm64 machine.
set -euo pipefail
out=${1:?usage: build-rootfs.sh OUT.tar}
here=$(cd "$(dirname "$0")" && pwd)
eng=${CONTAINER_ENGINE:-$(command -v podman || command -v docker)}
img=opendeck-emu-rootfs
mkdir -p "$(dirname "$out")"
"$eng" build --platform linux/arm64 -t "$img" -f "$here/Containerfile" "$here"
cid=$("$eng" create --platform linux/arm64 "$img" /init)
trap '"$eng" rm -f "$cid" >/dev/null' EXIT
"$eng" export -o "$out" "$cid"
ls -la "$out"
