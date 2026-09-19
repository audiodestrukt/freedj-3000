#!/usr/bin/env bash
# App Store screenshots for the iPad 13" listing (2064x2752 portrait), rendered
# headlessly by the app itself — the same draw code the iPad runs, at the
# panel's exact pixel size, so they are the app as shipped.  Apple only needs
# the pixel size to match a supported device; a device capture is not required.
#
#   ios/appstore/capture.sh /path/to/music [outdir]
#   DEVICE=iphone ios/appstore/capture.sh /path/to/music      # iPhone 6.9" set
#
# DEVICE=iphone renders the phone's two landscape pages at 2868x1320: the app
# runs at the phone's logical size (956x440 pt) with a 3x scale factor, so
# touch targets and safe-area insets come out the size the device draws them.
#
# The music folder is the BROWSE listing; TRACK (default: its first .mp3) is
# the loaded track.  CUES are memory/hot-cue seconds for the loaded track.
# Needs xvfb-run + ImageMagick and a debug build (make build).
set -euo pipefail
MUSIC=${1:?usage: capture.sh /path/to/music [outdir]}
DEVICE=${DEVICE:-ipad}
if [ "$DEVICE" = iphone ]; then
    OUT=${2:-ios/appstore/screenshots/iphone-6.9}
    DEV_ENV=(OPENDECK_PHONE=1 OPENDECK_WINDOW=956x440 WINIT_X11_SCALE_FACTOR=3)
    XVFB="-screen 0 3000x1400x24"
else
    OUT=${2:-ios/appstore/screenshots/ipad-13}
    DEV_ENV=(OPENDECK_PORTRAIT=1 OPENDECK_WINDOW=2064x2752 WINIT_X11_SCALE_FACTOR=1)
    XVFB="-screen 0 2200x2900x24"
fi
BIN=${BIN:-target/debug/opendeck}
TRACK=${TRACK:-$(ls "$MUSIC"/*.mp3 | head -1)}
CUES=${CUES:-62.5,124.9,187.3,249.7}
LOOP_AT=${LOOP_AT:-62.5}
mkdir -p "$OUT"
XDG=$(mktemp -d)                      # isolate persisted tag list / grids / settings
trap 'rm -rf "$XDG"' EXIT

cap() {
    local name=$1; shift
    env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 "${DEV_ENV[@]}" \
        XDG_DATA_HOME="$XDG" OPENDECK_SERVE=0 \
        OPENDECK_SCREENSHOT="$OUT/$name.png" \
        RUST_LOG=opendeck=info,wgpu=off,naga=off,egui=off "$@" \
        timeout 90 xvfb-run -a -s "$XVFB" "$BIN" "$TRACK" 2>&1 \
        | grep -E "captured|panic" || true
    # App Store Connect rejects screenshots with an alpha channel.
    convert "$OUT/$name.png" -alpha off "$OUT/$name.png"
    echo "$name: $(identify -format '%wx%h %[channels]' "$OUT/$name.png")"
}

MEM=$(echo "$CUES" | cut -d, -f1,2,4)
if [ "$DEVICE" = iphone ]; then
    # The two pages first (they are what the phone listing has to explain),
    # then the LCD screens on the SCREEN page.
    cap 01-playback OPENDECK_PLAY=1 OPENDECK_MEMORY_CUES="$MEM"
    cap 02-controls OPENDECK_PLAY=1 OPENDECK_PHONE_PAGE=controls OPENDECK_MEMORY_CUES="$MEM"
    cap 03-loop     OPENDECK_PLAY=1 OPENDECK_LOOP="$LOOP_AT,4" OPENDECK_MEMORY_CUES="$MEM"
    cap 04-perform  OPENDECK_PLAY=1 OPENDECK_SCREEN=perform OPENDECK_HOT_CUES="$CUES"
    cap 05-browse   OPENDECK_SCREEN=browse
    cap 06-grid     OPENDECK_GRID_ADJUST=1 OPENDECK_CUE="$LOOP_AT"
else
    cap 01-playback OPENDECK_PLAY=1 OPENDECK_MEMORY_CUES="$MEM"
    cap 02-loop     OPENDECK_PLAY=1 OPENDECK_LOOP="$LOOP_AT,4" OPENDECK_MEMORY_CUES="$MEM"
    cap 03-perform  OPENDECK_PLAY=1 OPENDECK_SCREEN=perform OPENDECK_HOT_CUES="$CUES"
    cap 04-browse   OPENDECK_SCREEN=browse
    cap 05-grid     OPENDECK_GRID_ADJUST=1 OPENDECK_CUE="$LOOP_AT"
fi
