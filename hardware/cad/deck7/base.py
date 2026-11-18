"""base.py — the full-size deck's base: a plate printed as four pieces.

    python3 hardware/cad/deck7/base.py  ->  build/base_{fl,fr,bl,br}.stl

Split on its own grid (X_BASE_SEAM, Y_BASE_SEAM), away from the top's
seams, so the pieces bridge them and their screws clamp the top together.
Neighbours overlap in half-laps (each half the plate thick across
BASE_LAP); where the two laps cross, front-left keeps the lower half and
back-right the upper, so the corner is full thickness with no step.
Countersunk M3 from below, rubber-foot pockets, Pi standoffs and intake
vents, the jog tower's screws.  Print flat, bottom face down.
"""
import os
from build123d import *
from params import *
from top_shell import rrect, box, cyl, OVS

FOOT = (10.5, 1.0)
LAP_FIT = 0.2


def csk(x, y):
    h = (M3_CSK - M3_CLR) / 2
    return [cyl(x, y, -OVS, BASE_T + OVS, M3_CLR),
            Pos(x, y, -OVS) * Cone(M3_CSK / 2 + 0.5, M3_CLR / 2, h + 0.5 + OVS,
                                   align=(Align.CENTER, Align.CENTER, Align.MIN))]


def whole():
    plate = rrect(0, 0, BASE_T)
    plate = plate.fuse(*[cyl(x, y, BASE_T - 0.5, BASE_T + PI_STANDOFF, 6.0) for x, y in pi_holes()])
    cuts = []
    for x, y in base_screws():
        cuts += csk(x, y)
    for sx in (-1, 1):
        for sy in (-1, 1):
            cuts += csk(JOG_CX + sx * TOWER_SCREW, JOG_CY + sy * TOWER_SCREW)
    for x, y in pi_holes():
        cuts.append(cyl(x, y, 0.6, BASE_T + PI_STANDOFF + OVS, 2.2))
    for i in range(7):
        x = PI_X0 + 8.0 + i * 6.7
        cuts.append(box(x - 1.25, x + 1.25, PI_Y0 + 14.0, PI_Y0 + 52.0, -OVS, BASE_T + OVS))
    for x, y in ((20, 20), (W - 20, 20), (20, D - 20), (W - 20, D - 20)):
        cuts.append(cyl(x, y, -OVS, FOOT[1], FOOT[0]))
    return plate.cut(*cuts)


def _bands(seam, end):
    """The three spans along one axis: core 0, the lap, core 1."""
    s0, s1 = seam - BASE_LAP / 2, seam + BASE_LAP / 2
    return (-OVS, s0), (s0, s1), (s1, end + OVS)


def regions():
    """{piece: [boxes]} — each piece's share of the plate, laps included."""
    zl = BASE_T / 2
    lo, hi = (-OVS, zl), (zl, H)
    X, Y = _bands(X_BASE_SEAM, W), _bands(Y_BASE_SEAM, D)
    out = {p: [] for p in ("fl", "fr", "bl", "br")}

    def span(rng, lower):       # a lap span, shortened by the fit at the far end
        a, b = rng
        return (a, b - LAP_FIT) if lower else (a + LAP_FIT, b)

    for i, xr in enumerate(X):
        for j, yr in enumerate(Y):
            col = {0: "l", 2: "r"}.get(i)
            row = {0: "f", 2: "b"}.get(j)
            if col and row:                                   # both cores: full height
                out[row + col].append((xr, yr, (-OVS, H)))
            elif row:                                         # the x lap: left low, right high
                out[row + "l"].append((span(xr, True), yr, lo))
                out[row + "r"].append((span(xr, False), yr, hi))
            elif col:                                         # the y lap: front low, back high
                out["f" + col].append((xr, span(yr, True), lo))
                out["b" + col].append((xr, span(yr, False), hi))
            else:                                             # the laps' cross
                out["fl"].append((span(xr, True), span(yr, True), lo))
                out["br"].append((span(xr, False), span(yr, False), hi))
    return out


def split(solid):
    res = {}
    for p, bx in regions().items():
        parts = [solid & box(x0, x1, y0, y1, z0, z1) for (x0, x1), (y0, y1), (z0, z1) in bx]
        s = parts[0]
        for q in parts[1:]:
            s = s.fuse(q)
        res[p] = s.clean()
    return res


_cache = {}
def parts():
    if not _cache:
        _cache.update(split(whole()))
    return dict(_cache)


if __name__ == "__main__":
    import trimesh
    os.makedirs(os.path.join(HERE, "build"), exist_ok=True)
    for p, s in parts().items():
        out = os.path.join(HERE, "build", f"base_{p}.stl")
        export_stl(s, out)
        m = trimesh.load(out)
        ext = (m.bounds[1] - m.bounds[0]).round(1)
        print(f"base_{p}: {ext} bodies: {len(m.split(only_watertight=False))} watertight: {m.is_watertight}"
              f" fits bed: {ext[0] <= BED[0] and ext[1] <= BED[1]}")
