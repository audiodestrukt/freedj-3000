#!/usr/bin/env bash
# render.sh — rebuild the assembly and every render in renders/ (JPEG).
#   hardware/cad/deck5/render.sh [samples]      (needs Blender; GPU if present)
set -euo pipefail
cd "$(dirname "$0")"
S=${1:-128}
# The skill-validated build first: a PATH blender may differ (one here segfaults headless).
BLENDER=${BLENDER:-$(ls -d /opt/blender-5.0.1*/blender 2>/dev/null || command -v blender)}
python3 assembly.py
mkdir -p renders build/r
r() { out=$1; inp=$2; shift 2
      LC_ALL=C LANG=C "$BLENDER" -b -P render_blender.py -- "$inp" "build/r/$out.png" --samples "$S" --bg "$@" >/dev/null 2>&1 </dev/null
      python3 -c "from PIL import Image; Image.open('build/r/$out.png').convert('RGB').save('renders/$out.jpg', quality=88)"
      echo "renders/$out.jpg"; }
r hero          build/deck5.glb          --res 1600 --resy 1100 --az -24 --el 38 --dist 3.3
r back          build/deck5.glb          --res 1600 --resy 1000 --az 160 --el 22 --dist 3.0
r cutaway       build/deck5_cutaway.glb  --res 1600 --resy 1000 --az 72  --el 20 --dist 2.8
r exploded      build/deck5_exploded.glb --res 1300 --resy 1600 --az 32  --el 24 --dist 3.0
r exploded_back build/deck5_exploded.glb --res 1300 --resy 1600 --az 148 --el 26 --dist 3.0
r seam_tongue   build/deck5_seam.glb     --res 1400 --resy 1000 --az 35  --el 38 --dist 1.9
r seam_groove   build/deck5_seam.glb     --res 1400 --resy 1000 --az 150 --el 38 --dist 1.9
r screen_mount  build/deck5_cutaway.glb  --res 1400 --resy 900  --az 70  --el 22 --dist 0.9 --target 0.110,0.236,0.038 --lens 70
