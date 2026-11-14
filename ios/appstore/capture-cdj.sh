#!/usr/bin/env bash
# App Store iPad 13" screenshots (2064x2752 portrait) of the CDJ-3000 deck,
# rendered headlessly by the app itself, as capture.sh does for the XDJ set.
#
#   ios/appstore/capture-cdj.sh [outdir]
#
# The scenes need a track with rekordbox hot cue colours, a grid on the beat,
# a key, and real stems, all from our own music (techno.mp3 in the repo root):
#   1. split it into stems on this machine (HTDemucs through ONNX Runtime;
#      OPENDECK_SEPARATE_MODEL=…/htdemucs.onnx, cargo feature separate-onnx);
#   2. build a rekordbox test stick with coloured cues (fixture_stick) and put
#      the stems on it in OverCue's USB format (stick_from_pcm), the track
#      tagged with its key;
#   3. load it from the stick with the dev script and capture each scene.
# Needs xvfb-run, ffmpeg, ImageMagick and a debug build.
set -euo pipefail
OUT=${1:-ios/appstore/screenshots/ipad-13}
MODEL=${OPENDECK_SEPARATE_MODEL:?set OPENDECK_SEPARATE_MODEL to htdemucs.onnx}
SRC=${TRACK:-techno.mp3}
BPM=${BPM:-134.998}; FIRST_MS=${FIRST_MS:-52.45}; KEY=${KEY:-Fm}   # analyze_file's grid for techno.mp3
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
mkdir -p "$OUT" "$W/split" "$W/music"

# 1. Stems (48 kHz WAVs), then the 96 kHz raw roles OverCue's format stores.
SPLIT_IN=$PWD/$SRC SPLIT_OUT=$W/split OPENDECK_SEPARATE_MODEL=$MODEL \
  cargo test --release -q -p opendeck-app --features separate-onnx --lib split_a_real_track -- --ignored >/dev/null
python3 - "$W" <<'PY'
import sys, wave, array
w = sys.argv[1]
def rd(n):
    f = wave.open(f"{w}/split/{n}.wav"); return array.array('h', f.readframes(f.getnframes()))
d, v, h = rd("drums"), rd("vocal"), rd("harmonics")
full = array.array('h', (max(-32768, min(32767, a + b + c)) for a, b, c in zip(d, v, h)))
for n, a in [("drums", d), ("vocal", v), ("harmonics", h), ("full", full)]:
    open(f"{w}/{n}48.raw", "wb").write(a.tobytes())
PY
for n in drums vocal harmonics full; do
  ffmpeg -loglevel error -y -f s16le -ar 48000 -ac 2 -i "$W/${n}48.raw" -af aresample=96000:resampler=soxr -f s16le -ar 96000 -ac 2 "$W/$n.raw"
done

# 2. The stick: coloured cues + grid, the key-tagged track, the stems.
ffmpeg -loglevel error -y -i "$SRC" -c copy -map_metadata 0 -metadata TKEY="$KEY" -id3v2_version 3 "$W/track.mp3"
cargo run -q -p opendeck-rekordbox --example fixture_stick -- "$W/media/OPENDECK" "$SRC" amplificate.mp3 --bpm "$BPM" --first-ms "$FIRST_MS" >/dev/null
cargo run -q --release -p opendeck-stems --example stick_from_pcm -- "$W/media/OPENDECK" "/Contents/Loopmasters/UnknownAlbum/Demo Track 1.mp3" \
  "$W/track.mp3" "$W/drums.raw" "$W/vocal.raw" "$W/harmonics.raw" "$W/full.raw" >/dev/null
cp "$W/track.mp3" "$W/music/X0001000X (Original Mix).mp3"

# 3. Scenes: load from the stick, play, then each panel.
cap() {
  local name=$1 extra=$2; shift 2
  local X; X=$(mktemp -d)
  env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 OPENDECK_PORTRAIT=1 OPENDECK_WINDOW=2064x2752 WINIT_X11_SCALE_FACTOR=1 \
    XDG_DATA_HOME="$X" OPENDECK_SERVE=0 OPENDECK_MEDIA_DIRS="$W/media" RUST_LOG=off \
    OPENDECK_SCRIPT="2:screen=browse; 3:open=USB; 5:category=2; 6:open=Demo Track 1; 8:play; $extra" "$@" \
    timeout 120 xvfb-run -a -s "-screen 0 2200x2900x24" target/debug/opendeck "$W/music/X0001000X (Original Mix).mp3" >/dev/null 2>&1 || true
  rm -rf "$X"
  convert "$OUT/$name.png" -alpha off "$OUT/$name.png"   # App Store Connect rejects alpha
  echo "$name: $(identify -format '%wx%h' "$OUT/$name.png")"
}
cap 01-playback "20:shot=$OUT/01-playback.png; 22:quit"
cap 02-stems    "12:stems=1,0,0.55; 20:shot=$OUT/02-stems.png; 22:quit" OPENDECK_CDJ_PANEL=stems
cap 03-beatloop "14:press=BEATLOOP_HALF; 20:shot=$OUT/03-beatloop.png; 22:quit" OPENDECK_CDJ_PANEL=loop
cap 04-keyshift "12:keyshift=2; 20:shot=$OUT/04-keyshift.png; 22:quit" OPENDECK_CDJ_PANEL=key
cap 05-grid     "18:shot=$OUT/05-grid.png; 20:quit" OPENDECK_GRID_ADJUST=1
