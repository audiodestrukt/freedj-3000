"""caps.py — the small printed parts: PLAY / CUE caps, key caps, fader
knob, encoder knob, and the screen frame.

    python3 hardware/cad/deck5/caps.py  ->  build/<part>.stl

Caps drop into their holes from below before the switch board goes on: a
flange under the top plate keeps them in, a stem presses a 12 mm tact
switch.  Print caps top face down; the knobs and the frame flat.
"""
import os
from build123d import *
from params import *
from top_shell import box, cyl, OVS

FLANGE = 2.0          # cap flange: overhang past the hole, and thickness 1.2
CAP_PROUD = 3.0       # how far a cap stands above the top surface
STEM = (6.0, 5.0)     # stem: diameter, length (to the switch plunger)
FL_GAP = 0.3          # flange rests this far under its guide sleeve (switch spring holds it up)
Z_FL_TOP = -TOP_T - GUIDE_L - FL_GAP      # flange top, from the deck's top surface
Z_FL_BOT = Z_FL_TOP - 1.2


def big_cap():
    """PLAY / CUE: local frame, z = 0 at the top surface of the deck."""
    d = BIG_CAP_D
    top = cyl(0, 0, Z_FL_TOP - 0.5, CAP_PROUD, d)
    top = fillet(top.edges().group_by(Axis.Z)[-1], 1.5)
    fl = cyl(0, 0, Z_FL_BOT, Z_FL_TOP, d + CAP_CLR + 2 * FLANGE)
    st = cyl(0, 0, Z_FL_BOT - STEM[1], Z_FL_BOT + 0.5, STEM[0])
    return top.fuse(fl, st)


def key_cap():
    kx, ky = KEY
    top = box(-kx / 2, kx / 2, -ky / 2, ky / 2, Z_FL_TOP - 0.5, CAP_PROUD - 1.0)
    top = fillet(top.edges().group_by(Axis.Z)[-1], 1.0)
    fx, fy = kx + CAP_CLR + 2 * FLANGE, ky + CAP_CLR + 2 * FLANGE
    fl = box(-fx / 2, fx / 2, -fy / 2, fy / 2, Z_FL_BOT, Z_FL_TOP)
    st = cyl(0, 0, Z_FL_BOT - STEM[1], Z_FL_BOT + 0.5, 4.0)
    return top.fuse(fl, st)


def fader_knob():
    """Local frame: z = 0 at the top surface; the lever pocket opens down."""
    w, l, h = 14.0, 22.0, 11.0
    z0 = 2.0
    k = box(-w / 2, w / 2, -l / 2, l / 2, z0, z0 + h)
    k = fillet(k.edges().group_by(Axis.Z)[-1], 3.0)
    k -= box(-w / 2 - OVS, w / 2 + OVS, -1.0, 1.0, z0 + h - 0.8, z0 + h + OVS)     # centre line
    lx, ly = FADER_LEVER[0] + 0.2, FADER_LEVER[1] + 0.2
    k -= box(-lx / 2, lx / 2, -ly / 2, ly / 2, z0 - OVS, z0 + 8.0)
    return k


def enc_knob():
    d, h = 20.0, 14.0
    k = cyl(0, 0, 1.5, 1.5 + h, d)
    k = fillet(k.edges().group_by(Axis.Z)[-1], 2.0)
    k -= cyl(0, 0, 1.5 - OVS, 1.5 + 10.0, 6.2)            # 6 mm D-shaft (press fit)
    k = k.fuse(box(1.5, 3.2, -3.0, 3.0, 1.5, 1.5 + 10.0))   # the D's flat
    for i in range(24):
        a = i * 15.0
        k -= Rot(0, 0, a) * Pos(d / 2 + 0.3, 0, 1.5 + h / 2 + 2.0) * Cylinder(0.8, h)
    return k


def screen_frame():
    """The frame the display sits on (local frame: centred on the display,
    z = 0 underneath, top face = the display's back).  M2.5 screws come up
    through it into the display's inserts; its four ears screw to bosses
    under the top plate.  Print flat."""
    ow, oh = SCR_OUT
    f = box(-ow / 2, ow / 2, -oh / 2, oh / 2, 0, FRAME_T)
    for sx in (-1, 1):
        x0, x1 = sorted((sx * (ow / 2 - 1.0), sx * (SCR_BOSS[0] + 6.0)))
        f = f.fuse(box(x0, x1, -SCR_BOSS[1] - 6.0, SCR_BOSS[1] + 6.0, 0, FRAME_T))
    r = FRAME_RING
    cuts = [box(-ow / 2 + r, ow / 2 - r, -oh / 2 + r, oh / 2 - r, -OVS, FRAME_T + OVS)]
    for sx in (-1, 1):
        for sy in (-1, 1):
            cuts.append(cyl(sx * SCR_HOLES[0] / 2, sy * SCR_HOLES[1] / 2, -OVS, FRAME_T + OVS, M25_CLR))
            cuts.append(cyl(sx * SCR_BOSS[0], sy * SCR_BOSS[1], -OVS, FRAME_T + OVS, M3_CLR))
    return f.cut(*cuts)


PARTS = {"cap_play_cue": big_cap, "cap_key": key_cap, "knob_fader": fader_knob,
         "knob_encoder": enc_knob, "screen_frame": screen_frame}

if __name__ == "__main__":
    import trimesh
    here = os.path.dirname(os.path.abspath(__file__))
    os.makedirs(os.path.join(here, "build"), exist_ok=True)
    for name, fn in PARTS.items():
        out = os.path.join(here, "build", f"{name}.stl")
        export_stl(fn(), out)
        m = trimesh.load(out)
        print(f"{name}: {(m.bounds[1] - m.bounds[0]).round(1)} bodies: {len(m.split(only_watertight=False))}"
              f" watertight: {m.is_watertight}")
