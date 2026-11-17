"""jog.py — the jog platter and the tower that carries its lower bearing.

    python3 hardware/cad/deck5/jog.py  ->  build/jog_platter.stl, build/jog_tower.stl

The platter rides on an 8 mm shaft held by two 608 bearings: one in the
boss under the top plate, one in the tower screwed to the base.  A
diametric magnet glued into the shaft's bottom end turns over an AS5600
board that slides into the slot under the tower (contactless, 4096
counts a turn).  Shaft collars under the upper bearing and over the lower
one set its height; a grub screw clamps the platter to it.

Platter: print TOP FACE DOWN (the grip skirt and hub rise, no supports).
Tower: print standing, bearing seat up (the sensor slot bridges 25 mm).
"""
import os
from build123d import *
from params import *
from top_shell import box, cyl, OVS

# Platter (local frame: skirt bottom at z = 0, axis on z)
TOP_T_J = 4.0
WALL_J = 3.0
REC_D, REC_DEPTH = 60.0, 1.2
HUB_OD = 22.0
SET_TAP = 2.5            # M3 thread-forming grub screw
GROOVES, GROOVE_R = 3, 1.0


def platter():
    r = JOG_D / 2
    z_under = JOG_H - TOP_T_J
    pts = [(0, z_under), (r - WALL_J, z_under), (r - WALL_J, 0), (r, 0), (r, JOG_H),
           (REC_D / 2, JOG_H), (REC_D / 2, JOG_H - REC_DEPTH), (0, JOG_H - REC_DEPTH)]
    with BuildPart() as bp:
        with BuildSketch(Plane.XZ):
            with BuildLine():
                Polyline(pts, close=True)
            make_face()
        revolve(axis=Axis.Z)
    body = bp.part
    hub_bot = 1.0
    body = body.fuse(cyl(0, 0, hub_bot, JOG_H - REC_DEPTH - 0.5, HUB_OD))
    bore_top = SHAFT_Z1 - JOG_Z0
    body -= cyl(0, 0, hub_bot - OVS, bore_top, SHAFT_D + 0.25)
    z = hub_bot + 5.0
    body -= Pos(HUB_OD / 4 + 1, 0, z) * Rot(0, 90, 0) * Cylinder(SET_TAP / 2, HUB_OD / 2 + OVS)
    for i in range(GROOVES):
        body -= Pos(0, 0, 2.5 + i * 3.5) * Torus(r, GROOVE_R)
    return body


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
    for name, s in (("jog_platter", platter()), ("jog_tower", tower())):
        out = os.path.join(here, "build", f"{name}.stl")
        export_stl(s, out)
        m = trimesh.load(out)
        print(f"{name}: {(m.bounds[1] - m.bounds[0]).round(1)} bodies: {len(m.split(only_watertight=False))}"
              f" watertight: {m.is_watertight}")
