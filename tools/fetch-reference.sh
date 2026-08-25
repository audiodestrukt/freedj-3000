#!/usr/bin/env bash
# Download the CDJ-3000X instruction manual and extract the screen-layout pages
# as PNGs for local visual reference.
#
# Output goes to reference/pioneer/, which is gitignored: these pages are
# AlphaTheta's copyrighted material. Read them, build against them, don't
# redistribute them. The parity spec derived from them lives in
# docs/reference/cdj-3000-playback-screen.md and is ours to ship.
#
#   ./tools/fetch-reference.sh          # or: make reference

set -euo pipefail

URL="https://downloads.support.alphatheta.com/manuals/dj-players/CDJ-3000X/CDJ-3000X_DRI1956B_manual.pdf"
DIR="reference/pioneer"
PDF="$DIR/CDJ-3000X_manual.pdf"

# Printed page numbers, which match PDF page numbers in this document.
#   19     SOURCE screen
#   21-22  Browse screen + callouts
#   23-26  Playback (waveform) screen + callouts 1-29
#   27     Jog display + callouts 1-7
#   111-112 Shortcut screen setting values
FIRST=19
LAST=27

command -v pdftoppm >/dev/null || { echo "need poppler-utils (pdftoppm)"; exit 1; }

mkdir -p "$DIR"

if [ ! -f "$PDF" ]; then
  echo "downloading CDJ-3000X manual..."
  curl -fsSL "$URL" -o "$PDF"
fi

echo "extracting pages $FIRST-$LAST..."
pdftoppm -png -r 150 -f "$FIRST" -l "$LAST" "$PDF" "$DIR/manual-p"

# Also extract the Shortcut screen settings pages.
pdftoppm -png -r 150 -f 111 -l 112 "$PDF" "$DIR/manual-p"

echo
echo "reference pages in $DIR:"
ls -1 "$DIR"/*.png | sed 's/^/  /'
echo
echo "The playback screen with all 29 callouts is manual-p-023.png."
