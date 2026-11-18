#!/usr/bin/env bash
# render.sh — rebuild the assembly and every render in renders/ (JPEG).
#   hardware/cad/deck7/render.sh [samples]      (needs Blender; GPU if present)
# Uses deck5's render_blender.py (the camera distance is in bounding-box radii).
set -euo pipefail
cd "$(dirname "$0")"
S=${1:-128}
BLENDER=${BLENDER:-$(ls -d /opt/blender-5.0.1*/blender 2>/dev/null || command -v blender)}
RB=../deck5/render_blender.py
python3 assembly.py
mkdir -p renders build/r
r() { out=$1; inp=$2; shift 2
      LC_ALL=C LANG=C "$BLENDER" -b -P "$RB" -- "$inp" "build/r/$out.png" --samples "$S" --bg "$@" >/dev/null 2>&1 </dev/null
      python3 -c "from PIL import Image; Image.open('build/r/$out.png').convert('RGB').save('renders/$out.jpg', quality=88)"
      echo "renders/$out.jpg"; }
r hero          build/deck7.glb          --res 1600 --resy 1100 --az -24 --el 40 --dist 3.1
r top           build/deck7.glb          --res 1100 --resy 1500 --az 0   --el 89 --dist 2.6
r back          build/deck7.glb          --res 1600 --resy 1000 --az 160 --el 22 --dist 2.9
r cutaway       build/deck7_cutaway.glb  --res 1600 --resy 1000 --az 72  --el 20 --dist 2.7
r exploded      build/deck7_exploded.glb --res 1300 --resy 1600 --az 32  --el 24 --dist 2.9
r seams         build/deck7_seam.glb     --res 1500 --resy 1300 --az 20  --el 55 --dist 2.6
r screen_mount  build/deck7_cutaway.glb  --res 1400 --resy 900  --az 70  --el 22 --dist 0.9 --target 0.1506,0.3168,0.040 --lens 70
