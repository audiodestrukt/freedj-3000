"""make_print.py — build every printed part, check it, export print-ready STLs.

    python3 hardware/cad/deck5/make_print.py     (or: make deck5)

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
import top_shell, base, jog, caps

HERE = os.path.dirname(os.path.abspath(__file__))
PRINT = os.path.join(HERE, "print")
FLIP = Rot(180, 0, 0)   # top face down

# name: (solid, flip for printing, how many to print)
def parts():
    tf, tb = top_shell.parts()
    bf, bb = base.parts()
    return {
        "top_front": (tf, True, 1), "top_back": (tb, True, 1),
        "base_front": (bf, False, 1), "base_back": (bb, False, 1),
        "jog_platter": (jog.platter(), True, 1), "jog_tower": (jog.tower(), False, 1),
        "cap_play_cue": (caps.big_cap(), True, 2), "cap_key": (caps.key_cap(), True, 6),
        "knob_fader": (caps.fader_knob(), True, 1), "knob_encoder": (caps.enc_knob(), True, 1),
        "screen_clamp": (caps.clamp_strap(), False, 2),
    }


def main():
    os.makedirs(PRINT, exist_ok=True)
    bad = []
    for name, (solid, flip, n) in parts().items():
        s = FLIP * solid if flip else solid
        bb = s.bounding_box()
        s = Pos(-bb.min.X, -bb.min.Y, -bb.min.Z) * s
        path = os.path.join(PRINT, f"{name}.stl")
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
    if bad or clashes:
        sys.exit(1)


if __name__ == "__main__":
    main()
