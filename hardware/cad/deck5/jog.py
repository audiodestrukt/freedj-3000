"""jog.py — the jog platter and the tower that carries its lower bearing.

    python3 hardware/cad/deck5/jog.py  ->  build/jog_platter.stl, build/jog_tower.stl, build/jog_shaft.stl

The platter rides on an 8 mm shaft held by two 608 bearings: one in the
boss under the top plate, one in the tower screwed to the base.  A
diametric magnet glued into the shaft's bottom end turns over an AS5600
board that slides into the slot under the tower (contactless, 4096
counts a turn).  Shaft collars under the upper bearing and over the lower
one set its height; a grub screw clamps the platter to it.

Platter: Dan's Pioneer reference shape (see platter()); print TOP FACE
DOWN (the flat top on the bed; skirt, shoulder and hub rise, no supports).
Tower: print standing, bearing seat up (the sensor slot bridges 25 mm).
"""
import os
from build123d import *
from params import *
from top_shell import box, cyl, OVS

# Platter (local frame: skirt bottom at z = 0, axis on z)
#
# The platter's shape is Dan's Pioneer reference: a Meshy mesh made from a
# photo of a real platter, its dome cut flat in Blender (hardware/cad/jog.stl,
# jog.blend).  REF is that mesh normalised once by `python3 jog.py --ref`:
# its two bodies (platter and base band) welded, simplified to 0.03 mm at
# 140 mm, top up, 1 unit across, base at z = 0.  platter() scales it to
# JOG_D (the 200 mm deck will only change JOG_D) and cuts the working parts
# into it with mesh booleans: the hollow underside, the hub with its shaft
# bore and grub screw, and the label recess.  It is a trimesh, not a
# build123d solid: the dimples on the shoulder are not a revolve.
REF = os.path.join(os.path.dirname(os.path.abspath(__file__)), "assets", "platter_ref.stl")
REF_SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "jog.stl")
TOP_T_J = 4.0            # top plate over the hollow
CAVITY_D = 0.68 * JOG_D  # hollow under the flat top; the dimpled shoulder stays solid (inertia)
REC_D, REC_DEPTH = 60.0, 1.2
HUB_OD = 22.0
SET_TAP = 2.5            # M3 thread-forming grub screw


def make_ref(src=REF_SRC, out=REF, tol_mm=0.03):
    """Normalise the reference mesh (see REF).  Run once; the result is committed."""
    import numpy as np, trimesh, manifold3d as mf
    m = trimesh.load(src)
    bodies = [b for b in m.split(only_watertight=False) if len(b.faces) > 100]   # drops a 2-face speck
    v = np.vstack([b.vertices for b in bodies])
    c = (v.min(0) + v.max(0)) / 2
    rot = np.array([[1, 0, 0], [0, 0, 1], [0, -1, 0]], float)   # the mesh is y-up, top at -y
    parts = []
    for b in bodies:
        w = (b.vertices - c) @ rot.T
        parts.append(trimesh.Trimesh(w, b.faces, process=False))
    allv = np.vstack([p.vertices for p in parts])
    k = 1.0 / (2 * np.hypot(allv[:, 0], allv[:, 1]).max())
    zmin = allv[:, 2].min()
    man = None
    for p in parts:
        q = mf.Manifold(mf.Mesh(vert_properties=((p.vertices - [0, 0, zmin]) * k).astype(np.float32),
                                tri_verts=p.faces.astype(np.uint32)))
        man = q if man is None else man + q
    # The base band is a separate shell 0.3 mm (at 140) outside the platter:
    # weld it with a ring hidden inside both.
    h = man.bounding_box()[5]
    # (the gap is at r 0.4735-0.478 of 1 across, band z 0-0.0147)
    ring = (mf.Manifold.cylinder(0.010, 0.488, 0.488, 512) - mf.Manifold.cylinder(0.012, 0.462, 0.462, 512).translate([0, 0, -0.001]))
    man = (man + ring.translate([0, 0, 0.002])).simplify(tol_mm / 140.0)
    man = max(man.decompose(), key=lambda p: p.volume())   # drop the simplifier's slivers
    o = man.to_mesh()
    t = trimesh.Trimesh(o.vert_properties[:, :3], o.tri_verts, process=False)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    t.export(out)
    print(f"ref: {len(t.faces)} faces, h/d {h:.4f}, bodies {len(t.split(only_watertight=False))}, watertight {t.is_watertight}")
    return t


def _man(solid):
    """A build123d solid as a manifold3d Manifold (via a fine STL)."""
    import tempfile, numpy as np, trimesh, manifold3d as mf
    with tempfile.NamedTemporaryFile(suffix=".stl") as f:
        export_stl(solid, f.name, tolerance=0.01, angular_tolerance=0.05)
        t = trimesh.load(f.name)
    return mf.Manifold(mf.Mesh(vert_properties=t.vertices.astype(np.float32), tri_verts=t.faces.astype(np.uint32)))


def platter():
    """The platter as a watertight trimesh, skirt bottom at z = 0."""
    import numpy as np, trimesh, manifold3d as mf
    ref = trimesh.load(REF)
    k = JOG_D / (2 * np.hypot(ref.vertices[:, 0], ref.vertices[:, 1]).max())
    body = mf.Manifold(mf.Mesh(vert_properties=(ref.vertices * k).astype(np.float32),
                               tri_verts=ref.faces.astype(np.uint32)))
    h = body.bounding_box()[5]
    assert abs(h - JOG_H) < 0.05, f"params.JOG_H should be {h:.2f} (the reference's height at JOG_D)"
    z_under = JOG_H - TOP_T_J
    hub_bot = 1.0
    bore_top = SHAFT_Z1 - JOG_Z0
    assert bore_top < JOG_H - REC_DEPTH - 1.5, "shaft bore breaks into the label recess"
    body = body - _man(cyl(0, 0, -OVS, z_under, CAVITY_D))
    body = body + _man(cyl(0, 0, hub_bot, z_under + 0.5, HUB_OD))
    body = body - _man(cyl(0, 0, hub_bot - OVS, bore_top, SHAFT_D + 0.25))
    body = body - _man(Pos(HUB_OD / 4 + 1, 0, (hub_bot + bore_top) / 2) * Rot(0, 90, 0) * Cylinder(SET_TAP / 2, HUB_OD / 2 + OVS))
    body = body - _man(cyl(0, 0, JOG_H - REC_DEPTH, JOG_H + OVS, REC_D))
    o = body.to_mesh()
    return trimesh.Trimesh(o.vert_properties[:, :3], o.tri_verts, process=False)


# Printed test shaft (local frame: magnet end at z = 0, axis on z)
#
# A stand-in for the 8 mm steel rod so the platter can be tried before the
# rod is cut: same length (SHAFT_Z0..SHAFT_Z1), the magnet pocket in its
# bottom end, and a flat where the platter's grub screw bears (a screw
# only dents a round printed shaft).  Print standing, magnet pocket UP: a
# round section for the bearings, and the pocket needs no bridge.
def shaft():
    L = SHAFT_Z1 - SHAFT_Z0
    s = cyl(0, 0, 0, L, SHAFT_PRINT_D)
    s = chamfer(s.edges().filter_by(GeomType.CIRCLE).group_by(Axis.Z)[-1], 0.5)
    pocket = cyl(0, 0, -OVS, MAGNET[1] + 0.2, MAGNET[0] + MAGNET_FIT)
    r = SHAFT_PRINT_D / 2
    flat = box(r - GRUB_FLAT, r + 1, -r - 1, r + 1, JOG_Z0 + 1.0 - SHAFT_Z0, L + OVS)
    return s.cut(pocket, flat)


def place_mesh(m, frm: Location, onto: Location):
    """params.place() for a trimesh part."""
    import numpy as np
    loc = onto * frm.inverse()
    t = loc.wrapped.Transformation()
    mat = np.eye(4)
    for i in range(3):
        for j in range(4):
            mat[i, j] = t.Value(i + 1, j + 1)
    out = m.copy()
    out.apply_transform(mat)
    return out


# Tower (local frame: base at z = 0, axis on z)
def tower():
    tx, ty, th = TOWER
    t = box(-tx / 2, tx / 2, -ty / 2, ty / 2, 0, th)
    t = fillet(t.edges().filter_by(Axis.Z), 4.0)
    cuts = [cyl(0, 0, th - BRG[2], th + OVS, BRG[0] + BRG_FIT),          # bearing seat
            cyl(0, 0, SENSOR_SLOT[1] - OVS, th - BRG[2] + OVS, SHAFT_D + 4.0),   # shaft + collar
            box(-SENSOR_SLOT[0] / 2, SENSOR_SLOT[0] / 2, -ty / 2 - OVS, ty / 2 + OVS, -OVS, SENSOR_SLOT[1])]
    for sx in (-1, 1):
        for sy in (-1, 1):
            cuts.append(cyl(sx * TOWER_SCREW, sy * TOWER_SCREW, -OVS, INSERT_L, INSERT_D))
    return t.cut(*cuts)


MATES = {
    "platter_axis": Pos(0, 0, 0),                       # platter: skirt bottom, on its axis
    "tower_foot": Pos(0, 0, 0),                         # tower: underside, on the axis
    "jog_world_platter": Pos(JOG_CX, JOG_CY, JOG_Z0),   # where they go
    "jog_world_tower": Pos(JOG_CX, JOG_CY, BASE_T),
}


if __name__ == "__main__":
    import trimesh
    here = os.path.dirname(os.path.abspath(__file__))
    os.makedirs(os.path.join(here, "build"), exist_ok=True)
    import sys
    if "--ref" in sys.argv:
        make_ref()
    for name, s in (("jog_platter", platter()), ("jog_tower", tower()), ("jog_shaft", shaft())):
        out = os.path.join(here, "build", f"{name}.stl")
        if isinstance(s, trimesh.Trimesh):
            s.export(out)
        else:
            export_stl(s, out)
        m = trimesh.load(out)
        print(f"{name}: {(m.bounds[1] - m.bounds[0]).round(1)} bodies: {len(m.split(only_watertight=False))}"
              f" watertight: {m.is_watertight}")
