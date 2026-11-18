"""make_print.py — build every printed part, check it, export print-ready STLs.

    python3 hardware/cad/deck7/make_print.py     (or: make deck7)

Adapted from deck5's: the same checks, for four top and four base pieces.

Each part is exported already turned to its print orientation (the face
that should be on the bed, down; sitting on z = 0) into print/, and must:
  - be watertight and a single body (or a slicer will mangle it),
  - fit the bed (params.BED),
and the assembled deck must have no two parts overlapping (> 1 mm³) —
the fit test: a hole moved without its boss, or a cap that hits a
sleeve, fails here.  Exits non-zero on any failure.
"""
import itertools, os, sys
import numpy as np
import trimesh
from build123d import *
from params import *
import top_shell, base, jog, caps, keycap

PRINT = os.path.join(HERE, "print")
FLIP = Rot(180, 0, 0)   # top face down

# name: (solid, flip for printing, how many to print)
def parts():
    out = {f"top_{p}": (t, True, 1) for p, t in top_shell.parts().items()}
    out.update({f"base_{p}": (b, False, 1) for p, b in base.parts().items()})
    return out | {
        "jog_platter": (jog.platter(), True, 1), "jog_tower": (jog.tower(), False, 1),
        "jog_shaft": (jog.shaft(), True, 1),
        "cap_play_cue": (caps.big_cap(), True, 2),
        "knob_fader": (caps.fader_knob(), True, 1), "knob_encoder": (caps.enc_knob(), True, 1),
        "screen_frame": (caps.screen_frame(), False, 1),
        "fader_bracket": (caps.fader_bracket(), True, 1),
    } | {keycap.stem(sz): (keycap.key_cap(sz), True, n) for sz, n in keycap.sizes(KEYS).items()}


def allowed_openings():
    """Every opening meant to go through the top surface, as (kind, ...)."""
    g = 0.5   # tolerance at the edges
    out = [("rect", SCR_CX, SCR_CY, SCR_OUT[0] + 2 * SCR_FIT + g, SCR_OUT[1] + 2 * SCR_FIT + g),
           ("rect", FADER_X, FADER_CY, FADER_SLOT_W + g, FADER_TRAVEL + FADER_LEVER[1] + 1.0 + g),
           ("circle", JOG_CX, JOG_CY, SHAFT_D + 1.0 + g),
           ("circle", *ENC, ENC_HOLE + g)]
    out += [("circle", x, y, BIG_CAP_D + CAP_CLR + g) for x, y in (PLAY, CUE)]
    out += [("rect", x, y, w + CAP_CLR + g, h + CAP_CLR + g) for x, y, _, (w, h) in KEYS]
    return out


def top_openings(ms, step=1.0):
    """Fire rays down at the top surface; any point that is not top surface
    (first hit lower than H - 0.3) must lie in an allowed opening.  Catches a
    screw hole or an insert bore that breaks through where it shouldn't."""
    xs = np.arange(EDGE_R + 2, W - EDGE_R - 2, step)
    ys = np.arange(EDGE_R + 2, D - EDGE_R - 2, step)
    X, Y = np.meshgrid(xs, ys)
    pts = np.c_[X.ravel(), Y.ravel()]
    pts = pts[np.abs(pts[:, 1] - Y_SEAM) > REVEAL[0] / 2 + 0.6]       # the reveals are meant
    pts = pts[np.abs(pts[:, 0] - X_SEAM) > REVEAL[0] / 2 + 0.6]
    ok = np.zeros(len(pts), bool)
    for kind, cx, cy, *dims in allowed_openings():
        if kind == "rect":
            ok |= (np.abs(pts[:, 0] - cx) <= dims[0] / 2) & (np.abs(pts[:, 1] - cy) <= dims[1] / 2)
        else:
            ok |= np.hypot(pts[:, 0] - cx, pts[:, 1] - cy) <= dims[0] / 2
    origins = np.c_[pts, np.full(len(pts), H + 5.0)]
    dirs = np.tile([0.0, 0.0, -1.0], (len(pts), 1))
    top = trimesh.util.concatenate([ms[f"top_{p}"] for p in top_shell.PIECES])
    hit = np.zeros(len(pts), bool)
    loc, idx, _ = top.ray.intersects_location(origins, dirs, multiple_hits=False)
    zfirst = np.full(len(pts), -np.inf)
    zfirst[idx] = loc[:, 2]
    open_ = zfirst < H - 0.3
    stray = open_ & ~ok
    print(f"top surface: {open_.sum()} open points, {stray.sum()} outside the allowed openings")
    if stray.any():
        for p in pts[stray][:: max(1, stray.sum() // 12)]:
            print(f"  STRAY OPENING at x={p[0]:.1f} y={p[1]:.1f}")
    return int(stray.sum())


def main():
    os.makedirs(PRINT, exist_ok=True)
    bad = []
    for name, (solid, flip, n) in parts().items():
        path = os.path.join(PRINT, f"{name}.stl")
        if isinstance(solid, trimesh.Trimesh):   # a mesh part (the platter)
            s = solid.copy()
            if flip:
                s.apply_transform(trimesh.transformations.rotation_matrix(np.pi, [1, 0, 0]))
            s.apply_translation(-s.bounds[0])
            s.export(path)
        else:
            s = FLIP * solid if flip else solid
            bb = s.bounding_box()
            s = Pos(-bb.min.X, -bb.min.Y, -bb.min.Z) * s
            export_stl(s, path)
        m = trimesh.load(path)
        ext = m.bounds[1] - m.bounds[0]
        bodies = len(m.split(only_watertight=False))
        fits = ext[0] <= BED[0] and ext[1] <= BED[1]
        ok = m.is_watertight and bodies == 1 and fits
        print(f"{'ok ' if ok else 'BAD'} {name:14s} x{n}  {ext.round(1)}  watertight={m.is_watertight} bodies={bodies} fits={fits}")
        if not ok:
            bad.append(name)

    # Fit: the assembled deck, every pair of parts.
    import assembly
    tmp = os.path.join(HERE, "build", "parts")
    os.makedirs(tmp, exist_ok=True)
    ms = {}
    for name, solid, _ in assembly.printed() + assembly.bought():
        ms[name] = assembly.mesh(solid, os.path.join(tmp, f"{name}.stl"))
    clashes = 0
    for (a, ma), (b, mb) in itertools.combinations(ms.items(), 2):
        lo = np.maximum(ma.bounds[0], mb.bounds[0]); hi = np.minimum(ma.bounds[1], mb.bounds[1])
        if np.any(lo >= hi):
            continue
        v = trimesh.boolean.intersection([ma, mb], engine="manifold").volume
        if v > 1.0:
            clashes += 1
            print(f"CLASH {a} x {b}: {v:.1f} mm3")
    print(f"assembly: {len(ms)} parts, {clashes} clashes")
    stray = top_openings(ms)
    if bad or clashes or stray:
        sys.exit(1)


if __name__ == "__main__":
    main()
