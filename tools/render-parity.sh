#!/usr/bin/env bash
# Render the same paused frame through the wgpu and SDL backends (headless,
# under xvfb) and diff the PNGs.  Passes when the waveform rects match to
# one level (the CPU rasteriser's LUT rounding) and at least 90 % of all
# pixels are identical — what differs is text and stroke anti-aliasing, which
# wgpu blends in linear light and SDL in gamma space.
#
#   tools/render-parity.sh [track] [outdir]      needs: cargo feature renderer-sdl, xvfb-run, python3-PIL/numpy
set -euo pipefail
cd "$(dirname "$0")/.."
TRACK=${1:-amplificate.mp3}; OUT=${2:-target/parity}; mkdir -p "$OUT"
BIN=${BIN:-target/release/opendeck}
[ -x "$BIN" ] || cargo build --release -p opendeck-app --features renderer-sdl
cap() {   # cap <name> [env...]
    local name=$1; shift
    local xdg; xdg=$(mktemp -d)
    env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 WINIT_X11_SCALE_FACTOR=1 SDL_VIDEODRIVER=x11 \
        XDG_DATA_HOME="$xdg" OPENDECK_SERVE=0 OPENDECK_LINK_SEND=0 OPENDECK_WINDOW=1024x600 OPENDECK_AUTOPLAY=0 \
        OPENDECK_SCREENSHOT="$OUT/$name.png" RUST_LOG=opendeck=info,wgpu=off,naga=off "$@" \
        timeout 120 xvfb-run -a -s "-screen 0 1280x800x24" "$BIN" "$TRACK" 2>&1 | grep -E "captured|panic|error" || true
    rm -rf "$xdg"
}
cap wgpu OPENDECK_RENDERER=wgpu
cap sdl  OPENDECK_RENDERER=sdl OPENDECK_WAVE_SCROLL=smooth   # the pixel-scroll cache quantises the playhead; parity measures the raster
python3 - "$OUT" <<'PY'
import sys; from PIL import Image; import numpy as np
out=sys.argv[1]
a=np.asarray(Image.open(f"{out}/wgpu.png").convert("RGB")).astype(int)
b=np.asarray(Image.open(f"{out}/sdl.png").convert("RGB")).astype(int)
assert a.shape==b.shape, f"size mismatch {a.shape} vs {b.shape}"
d=np.abs(a-b).max(axis=2)
Image.fromarray(np.clip(d*4,0,255).astype(np.uint8)).save(f"{out}/diff.png")
ident=100*(d==0).mean()
# The two waveform rects of the 1024x600 screen-only layout (screen::layout).
wave=d[210:395,200:800]; over=d[518:548,135:790]
print(f"identical {ident:.1f}%  within 2 levels {100*(d<=2).mean():.1f}%  mean {d.mean():.2f}  max {d.max()}")
print(f"enlarged waveform max {wave.max()}   overview max {over.max()}   (diff image: {out}/diff.png)")
ok = ident>=90 and wave.max()<=1 and over.max()<=1
print("PARITY", "OK" if ok else "FAIL"); sys.exit(0 if ok else 1)
PY
