#!/usr/bin/env bash
# App Store screenshots for the iPad 13" listing (2064x2752 portrait), rendered
# headlessly by the app itself — the same draw code the iPad runs, at the
# panel's exact pixel size, so they are the app as shipped.  Apple only needs
# the pixel size to match a supported device; a device capture is not required.
#
#   ios/appstore/capture.sh /path/to/music [outdir]
#
# The music folder is the BROWSE listing; TRACK (default: its first .mp3) is
# the loaded track.  CUES are memory/hot-cue seconds for the loaded track.
# Needs xvfb-run + ImageMagick and a debug build (make build).
set -euo pipefail
MUSIC=${1:?usage: capture.sh /path/to/music [outdir]}
OUT=${2:-ios/appstore/screenshots/ipad-13}
BIN=${BIN:-target/debug/opendeck}
TRACK=${TRACK:-$(ls "$MUSIC"/*.mp3 | head -1)}
CUES=${CUES:-62.5,124.9,187.3,249.7}
LOOP_AT=${LOOP_AT:-62.5}
mkdir -p "$OUT"
XDG=$(mktemp -d)                      # isolate persisted tag list / grids / settings
trap 'rm -rf "$XDG"' EXIT

cap() {
    local name=$1; shift
    env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 WINIT_X11_SCALE_FACTOR=1 \
        XDG_DATA_HOME="$XDG" OPENDECK_PORTRAIT=1 OPENDECK_WINDOW=2064x2752 \
        OPENDECK_SCREENSHOT="$OUT/$name.png" \
        RUST_LOG=opendeck=info,wgpu=off,naga=off,egui=off "$@" \
        timeout 90 xvfb-run -a -s "-screen 0 2200x2900x24" "$BIN" "$TRACK" 2>&1 \
        | grep -E "captured|panic" || true
    # App Store Connect rejects screenshots with an alpha channel.
    convert "$OUT/$name.png" -alpha off "$OUT/$name.png"
    echo "$name: $(identify -format '%wx%h %[channels]' "$OUT/$name.png")"
}

MEM=$(echo "$CUES" | cut -d, -f1,2,4)
cap 01-playback OPENDECK_PLAY=1 OPENDECK_MEMORY_CUES="$MEM"
cap 02-loop     OPENDECK_PLAY=1 OPENDECK_LOOP="$LOOP_AT,4" OPENDECK_MEMORY_CUES="$MEM"
cap 03-perform  OPENDECK_PLAY=1 OPENDECK_SCREEN=perform OPENDECK_HOT_CUES="$CUES"
cap 04-browse   OPENDECK_SCREEN=browse
cap 05-grid     OPENDECK_GRID_ADJUST=1 OPENDECK_CUE="$LOOP_AT"
