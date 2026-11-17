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
    through it into the display's inserts; its four ears slip over bosses
    under the top plate and an M3 + washer from below pulls each up (the
    bosses' bores are blind: nothing shows on top).  Print flat."""
    ow, oh = SCR_OUT
    f = box(-ow / 2, ow / 2, -oh / 2, oh / 2, 0, FRAME_T)
    for sx in (-1, 1):
        x0, x1 = sorted((sx * (ow / 2 - 1.0), sx * (SCR_BOSS[0] + 7.5)))
        f = f.fuse(box(x0, x1, -SCR_BOSS[1] - 7.5, SCR_BOSS[1] + 7.5, 0, FRAME_T))
    r = FRAME_RING
    cuts = [box(-ow / 2 + r, ow / 2 - r, -oh / 2 + r, oh / 2 - r, -OVS, FRAME_T + OVS)]
    for sx in (-1, 1):
        for sy in (-1, 1):
            cuts.append(cyl(sx * SCR_HOLES[0] / 2, sy * SCR_HOLES[1] / 2, -OVS, FRAME_T + OVS, M25_CLR))
            cuts.append(cyl(sx * SCR_BOSS[0], sy * SCR_BOSS[1], -OVS, FRAME_T + OVS, 8.0 + 0.6))   # over the boss
    return f.cut(*cuts)


def fader_bracket():
    """The fader's hidden mount (local frame: centred on the fader, z = 0 on
    the face that sits against the top plate's underside, body below).
    The fader hangs under it on two countersunk M3 (heads flush in this
    face); the ears slip over the plate's bosses and an M3 + washer from
    below pulls each one up.  Print with the z = 0 face on the bed."""
    ex, ey, ez = FADER_EAR
    half = FADER_BODY[1] / 2 + 0.5
    b = box(-ex / 2, ex / 2, -half, half, -FADER_BRACKET_T, 0)
    for sy in (-1, 1):
        y0, y1 = sorted((sy * (half - 0.5), sy * (half + ey)))
        b = b.fuse(box(-ex / 2, ex / 2, y0, y1, -ez, 0))
    slot = FADER_TRAVEL + FADER_LEVER[1] + 1.0
    cuts = [box(-FADER_SLOT_W / 2 - 0.5, FADER_SLOT_W / 2 + 0.5, -slot / 2, slot / 2, -ez - OVS, OVS)]
    h = (M3_CSK - M3_CLR) / 2
    for sy in (-1, 1):
        y = sy * FADER_PITCH / 2
        cuts.append(cyl(0, y, -FADER_BRACKET_T - OVS, OVS, M3_CLR))
        cuts.append(Pos(0, y, -h) * Cone(M3_CLR / 2, M3_CSK / 2 + 0.5, h + 0.5, align=(Align.CENTER, Align.CENTER, Align.MIN)))
        cuts.append(cyl(0, sy * FADER_BOSS_Y, -ez - OVS, OVS, BOSS_D + 0.6))     # over the boss
    return b.cut(*cuts)


PARTS = {"cap_play_cue": big_cap, "cap_key": key_cap, "knob_fader": fader_knob,
         "knob_encoder": enc_knob, "screen_frame": screen_frame, "fader_bracket": fader_bracket}

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
