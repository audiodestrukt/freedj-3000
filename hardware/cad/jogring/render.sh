#!/usr/bin/env bash
# render.sh — rebuild the rig and its renders in renders/ (JPEG).
#   hardware/cad/jogring/render.sh [samples]     (needs Blender; uses deck5's render_blender.py)
set -euo pipefail
cd "$(dirname "$0")"
S=${1:-96}
BLENDER=${BLENDER:-$(ls -d /opt/blender-5.0.1*/blender 2>/dev/null || command -v blender)}
RB=../deck5/render_blender.py
python3 rig.py
mkdir -p renders build/r
r() { out=$1; inp=$2; shift 2
      LC_ALL=C LANG=C "$BLENDER" -b -P "$RB" -- "$inp" "build/r/$out.png" --samples "$S" --bg "$@" >/dev/null 2>&1 </dev/null
      python3 -c "from PIL import Image; Image.open('build/r/$out.png').convert('RGB').save('renders/$out.jpg', quality=88)"
      echo "renders/$out.jpg"; }
r hero     build/jogring_vwheel.glb          --res 1500 --resy 1100 --az -24 --el 38 --dist 2.6
r cutaway  build/jogring_vwheel_cutaway.glb  --res 1600 --resy 1000 --az 90  --el 12 --dist 2.0
r roller   build/jogring_vwheel_cutaway.glb  --res 1400 --resy 1000 --az 90  --el 10 --dist 0.9 --target 0.0,0.054,0.020 --lens 60
r wheel    build/jogring_vwheel_cutaway.glb  --res 1400 --resy 1000 --az 90  --el 10 --dist 0.9 --target 0.0,-0.080,0.020 --lens 60
r wheel608 build/jogring_608_cutaway.glb  --res 1400 --resy 1000 --az 90  --el 10 --dist 0.9 --target 0.0,-0.0835,0.020 --lens 60
r edge_support build/jogring_edge_cutaway.glb --res 1400 --resy 1000 --az 90 --el 10 --dist 0.9 --target 0.0,-0.088,0.020 --lens 60
r edge_guide   build/jogring_edge_cutaway.glb --res 1400 --resy 1000 --az 90 --el 10 --dist 0.9 --target 0.0,0.075,0.020 --lens 60
r edge_open    build/jogring_edge_noring.glb  --res 1500 --resy 1100 --az -20 --el 45 --dist 2.4
