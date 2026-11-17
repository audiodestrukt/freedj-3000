"""base.py — the base plate, printed as two halves with a half-lap joint.

    python3 hardware/cad/deck5/base.py  ->  build/base_front.stl, build/base_back.stl

The base splits at Y_BASE_SEAM, away from the top shell's seam, so the
back half spans the top seam and its screws clamp the two top pieces
together.  The halves overlap in a half-lap (each half thick across the
lap) with two screws through it into shell bosses.  The back half carries
the Pi standoffs and the intake vents under the Pi; the front half the
jog tower's screws.  Countersunk M3 from below everywhere; shallow
pockets take 10 mm rubber feet.

Print flat, bottom face down (the countersinks print as cones on the bed).
"""
import os
from build123d import *
from params import *
from top_shell import rrect, box, cyl, OVS

FOOT = (10.5, 1.0)       # rubber-foot pocket: diameter, depth
LAP_FIT = 0.2


def csk(x, y):
    """A countersunk M3 from below: clearance bore + cone at the bottom face."""
    h = (M3_CSK - M3_CLR) / 2
    return [cyl(x, y, -OVS, BASE_T + OVS, M3_CLR),
            Pos(x, y, -OVS) * Cone(M3_CSK / 2 + 0.5, M3_CLR / 2, h + 0.5 + OVS,
                                   align=(Align.CENTER, Align.CENTER, Align.MIN))]


def whole():
    plate = rrect(0, 0, BASE_T)
    adds = []
    for x, y in pi_holes():
        adds.append(cyl(x, y, BASE_T - 0.5, BASE_T + PI_STANDOFF, 6.0))
    plate = plate.fuse(*adds)
    cuts = []
    for x, y in base_screws():
        cuts += csk(x, y)
    for sx in (-1, 1):
        for sy in (-1, 1):
            cuts += csk(JOG_CX + sx * TOWER_SCREW, JOG_CY + sy * TOWER_SCREW)
    for x, y in pi_holes():
        cuts.append(cyl(x, y, 0.6, BASE_T + PI_STANDOFF + OVS, 2.2))   # M2.5 self-tapping
    # Intake vents under the Pi's cooler.
    for i in range(7):
        x = PI_X0 + 8.0 + i * 6.7
        cuts.append(box(x - 1.25, x + 1.25, PI_Y0 + 14.0, PI_Y0 + 52.0, -OVS, BASE_T + OVS))
    # Rubber-foot pockets.
    for x, y in ((20, 20), (W - 20, 20), (20, D - 20), (W - 20, D - 20)):
        cuts.append(cyl(x, y, -OVS, FOOT[1], FOOT[0]))
    return plate.cut(*cuts)


def split(solid):
    s0, s1 = Y_BASE_SEAM - BASE_LAP / 2, Y_BASE_SEAM + BASE_LAP / 2
    zl = BASE_T / 2
    front = solid & box(-OVS, W + OVS, -OVS, s0, -OVS, H)
    front = front.fuse(solid & box(-OVS, W + OVS, s0 - OVS, s1 - LAP_FIT, -OVS, zl))
    back = solid & box(-OVS, W + OVS, s1, D + OVS, -OVS, H)
    back = back.fuse(solid & box(-OVS, W + OVS, s0 + LAP_FIT, s1 + OVS, zl, H))
    return front, back


_cache = {}
def parts():
    if not _cache:
        _cache["front"], _cache["back"] = split(whole())
    return _cache["front"], _cache["back"]


if __name__ == "__main__":
    import trimesh
    here = os.path.dirname(os.path.abspath(__file__))
    os.makedirs(os.path.join(here, "build"), exist_ok=True)
    for name, s in zip(("base_front", "base_back"), parts()):
        out = os.path.join(here, "build", f"{name}.stl")
        export_stl(s, out)
        m = trimesh.load(out)
        ext = (m.bounds[1] - m.bounds[0]).round(1)
        print(f"{name}: {ext} bodies: {len(m.split(only_watertight=False))} watertight: {m.is_watertight}"
              f" fits bed: {ext[0] <= BED[0] and ext[1] <= BED[1]}")
