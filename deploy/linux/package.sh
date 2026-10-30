#!/usr/bin/env bash
# Bundle a built Linux binary into the release tarball the GitHub workflow
# publishes (.github/workflows/linux.yml); usable locally too:
#
#   deploy/linux/package.sh <arch> <version> <path/to/opendeck>
#     → dist/OpenDeck-<version>-linux-<arch>.tar.gz
#
# arch is the tarball label (x86_64, aarch64). The layout mirrors the Windows
# zip: binary, README/CHANGELOG, controller mappings, the demo track (an
# original by the author) and the Pi appliance bits (rt-audio limits, USB
# automount) under pi/.
set -euo pipefail
cd "$(dirname "$0")/../.."
arch=${1:?arch}; version=${2:?version}; bin=${3:?binary}
NAME="OpenDeck-$version-linux-$arch"
D="dist/$NAME"
rm -rf "$D"; mkdir -p "$D/surfaces" "$D/pi"
install -m 755 "$bin" "$D/opendeck"
cp README.md CHANGELOG.md "$D/"
cp crates/app/surfaces/dj2go.toml docs/hardware/surfaces/generic-midi.toml "$D/surfaces/"
cp amplificate.mp3 "$D/"
cp -r deploy/pi/rt-audio deploy/pi/usb-automount "$D/pi/"
cat > "$D/README-LINUX.txt" <<TXT
OpenDeck for Linux ($arch)

Run:      ./opendeck amplificate.mp3
          ./opendeck /path/to/track.mp3
          ./opendeck                     (empty deck; B opens the browser)
Version:  ./opendeck --version
Controls: see README.md, "Controls (MVP)".

Needs:    ALSA and SDL2 shared libraries plus a Vulkan driver for the
          default renderer. Debian / Ubuntu / Raspberry Pi OS:
            sudo apt install libasound2 libsdl2-2.0-0 mesa-vulkan-drivers
          (on Ubuntu 24.04 the ALSA package is libasound2t64).
          Built against glibc 2.36: Debian 12, Ubuntu 22.04, Raspberry Pi OS
          Bookworm, or anything newer. Rubber Band and Bungee are built in.

Renderers:
  default             wgpu over Vulkan (Pi 4 / Pi 5, any desktop GPU)
  OPENDECK_RENDERER=sdl
                      SDL2 renderer for boards wgpu cannot drive (VideoCore IV:
                      Pi 3, Zero 2 W). Without a desktop session add
                      SDL_VIDEODRIVER=kmsdrm OPENDECK_FULLSCREEN=1.
  OPENDECK_STRETCH=wsola
                      cheap key-lock engine for low-power boards.
The 32-bit Pi Zero W / Pi 1 (ARMv6) need a separate build:
docs/reference/raspberry-pi.md, deploy/pi/cross/build.sh armv6.

Settings, tag list and grids live in \$XDG_DATA_HOME/opendeck
(~/.local/share/opendeck). Pro DJ Link needs the machine on the same wired
network as the decks (UDP 50000-50002). Real-time audio priority on a Pi:
pi/rt-audio/README.md. USB-stick automount on a headless Pi: pi/usb-automount.
TXT
mkdir -p dist
tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
ls -l "dist/$NAME.tar.gz"
[ -n "${GITHUB_OUTPUT:-}" ] && echo "tarball=dist/$NAME.tar.gz" >> "$GITHUB_OUTPUT" || true
