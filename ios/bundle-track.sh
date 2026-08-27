#!/bin/sh
# Xcode "Bundle Track" script phase: copy a track into the .app.
#
# Tracks are gitignored (*.mp3), so they can't be a file reference in the
# .pbxproj — but the app needs one inside the bundle, and it has to land there
# BEFORE code signing seals the bundle.  A script phase runs ahead of the
# implicit CodeSign step, so this is the place.
#
# Which track:
#   FREEDJ_TRACK=/path/to/file.mp3   an explicit build setting, or
#   the first audio file in the repo root (what the desktop build plays).
#
# No track is not an error — the app logs an actionable message on launch and
# a signed, installable bundle is still useful (e.g. for the Link probe).
set -eu

REPO="$SRCROOT/.."
APP="$BUILT_PRODUCTS_DIR/$CONTENTS_FOLDER_PATH"

track="${FREEDJ_TRACK:-}"
if [ -z "$track" ]; then
    for f in "$REPO"/*.mp3 "$REPO"/*.m4a "$REPO"/*.aac "$REPO"/*.wav "$REPO"/*.aiff "$REPO"/*.flac; do
        [ -f "$f" ] && { track="$f"; break; }
    done
fi

if [ -z "$track" ]; then
    echo "warning: no track to bundle — set FREEDJ_TRACK=/path/to/file.mp3 or drop one in the repo root"
    exit 0
fi
if [ ! -f "$track" ]; then
    echo "error: FREEDJ_TRACK does not exist: $track" >&2
    exit 1
fi

# One track only: the app plays the first audio file it finds in the bundle, so
# leftovers from a previous build with a different track would win on sort order.
find "$APP" -maxdepth 1 -type f \( -name '*.mp3' -o -name '*.m4a' -o -name '*.aac' \
     -o -name '*.wav' -o -name '*.aiff' -o -name '*.flac' \) -delete

cp "$track" "$APP/$(basename "$track")"
echo "bundled track: $(basename "$track")"
