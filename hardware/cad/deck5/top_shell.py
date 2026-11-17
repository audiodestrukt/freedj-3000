"""top_shell.py — the deck's top: one shell, printed as two pieces.

    python3 hardware/cad/deck5/top_shell.py  ->  build/top_front.stl, build/top_back.stl

The whole shell is built as one solid (top plate, walls, bosses, cutouts),
then split across the depth at Y_SEAM between the control panel (front)
and the screen panel (back).  Each side of the seam is thickened into a
band; the back piece carries a U-shaped tongue that runs across the top
and down both walls, the front piece the matching groove.  Glue it with
thin CA, or dry-fit it: the base's back half spans the seam and its
screws pull both pieces onto it.  A shallow reveal channel outside turns
the joint into a deliberate line.

Print both pieces TOP FACE DOWN on a smooth plate: the visible surface is
the bed side, and walls, bosses and sleeves rise with no supports.  The
only bridge is the top of the Pi port window (54 mm).
"""
import os
from build123d import *
from params import *

OVS = 1.0   # boolean overshoot


def rrect(inset, z0, z1):
    """The deck outline inset by `inset`, as a prism from z0 to z1."""
    w, d, r = W - 2 * inset, D - 2 * inset, max(R_PLAN - inset, 0.5)
    return Pos(W / 2, D / 2, z0) * extrude(RectangleRounded(w, d, r), z1 - z0)


def box(x0, x1, y0, y1, z0, z1):
    return Pos((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2) * Box(x1 - x0, y1 - y0, z1 - z0)


def cyl(x, y, z0, z1, d):
    return Pos(x, y, (z0 + z1) / 2) * Cylinder(d / 2, z1 - z0)


def outer():
    o = rrect(0, BASE_T, H)
    return fillet(o.edges().group_by(Axis.Z)[-1], EDGE_R)


def u_profile(a, b, y0, y1, z0):
    """A U across the seam: the cross-section between insets a and b from
    the outside (top + both walls), open at the bottom, from y0 to y1."""
    outer_ = box(a, W - a, y0, y1, z0, H - a)
    inner_ = box(b, W - b, y0 - OVS, y1 + OVS, z0 - OVS, H - b)
    return outer_ - inner_


def whole():
    body = outer() - rrect(WALL, BASE_T - OVS, Z_UNDER)
    adds = []
    # Seam band: the shell thickened to BAND_T for BAND_HALF each side.
    band = (outer() & box(-OVS, W + OVS, Y_SEAM - BAND_HALF, Y_SEAM + BAND_HALF, 0, H + OVS)) \
        - rrect(BAND_T, BASE_T - OVS, H - BAND_T)
    adds.append(band)
    # Base-screw bosses, wall to top plate (welded 1 mm into the plate).
    for x, y in base_screws():
        adds.append(cyl(x, y, BASE_T, Z_UNDER + 1.0, BOSS_D))
    # Fader bracket bosses (blind: nothing shows on top).
    for sy in (-1, 1):
        adds.append(cyl(FADER_X, FADER_CY + sy * FADER_BOSS_Y, Z_UNDER - FADER_BOSS_H, Z_UNDER + 1.0, BOSS_D))
    # Upper jog bearing boss.
    adds.append(cyl(JOG_CX, JOG_CY, BRG_UP_Z - 1.0, Z_UNDER + 1.0, JOG_BOSS_D))
    # Screen frame bosses, down to the display's back (the frame's top).
    for sx in (-1, 1):
        for sy in (-1, 1):
            adds.append(cyl(SCR_CX + sx * SCR_BOSS[0], SCR_CY + sy * SCR_BOSS[1],
                            FRAME_BOSS_Z0, Z_UNDER + 1.0, 8.0))
    # Cap guide sleeves.
    for x, y in (PLAY, CUE):
        adds.append(cyl(x, y, Z_UNDER - GUIDE_L, Z_UNDER + 1.0, BIG_CAP_D + CAP_CLR + 2 * GUIDE_WALL))
    for x in KEY_XS:
        kx, ky = KEY[0] + CAP_CLR + 2 * GUIDE_WALL, KEY[1] + CAP_CLR + 2 * GUIDE_WALL
        adds.append(box(x - kx / 2, x + kx / 2, KEY_Y - ky / 2, KEY_Y + ky / 2, Z_UNDER - GUIDE_L, Z_UNDER + 1.0))
    body = body.fuse(*adds)

    cuts = []
    # Screen: an opening the display drops into from above (it rests on the
    # screen frame under the plate, glass flush with the top).
    ow, oh = SCR_OUT[0] + 2 * SCR_FIT, SCR_OUT[1] + 2 * SCR_FIT
    cuts.append(box(SCR_CX - ow / 2, SCR_CX + ow / 2, SCR_CY - oh / 2, SCR_CY + oh / 2, SCR_Z_BOT - OVS, H + OVS))
    # Jog: shaft through the plate, the bearing seat in the boss.
    cuts.append(cyl(JOG_CX, JOG_CY, BRG_UP_Z, H + OVS, SHAFT_D + 1.0))
    cuts.append(cyl(JOG_CX, JOG_CY, BRG_UP_Z - 1.0 - OVS, BRG_UP_Z + BRG[2], BRG[0] + BRG_FIT))
    # Fader: only the lever slot shows on top; the bracket's bosses are blind.
    slot = FADER_TRAVEL + FADER_LEVER[1] + 1.0
    cuts.append(box(FADER_X - FADER_SLOT_W / 2, FADER_X + FADER_SLOT_W / 2,
                    FADER_CY - slot / 2, FADER_CY + slot / 2, Z_UNDER - OVS, H + OVS))
    for sy in (-1, 1):
        cuts.append(cyl(FADER_X, FADER_CY + sy * FADER_BOSS_Y, Z_UNDER - FADER_BOSS_H - OVS, Z_UNDER + 0.3, INSERT_D))
    # Cap holes, through plate and sleeve.
    for x, y in (PLAY, CUE):
        cuts.append(cyl(x, y, Z_UNDER - GUIDE_L - OVS, H + OVS, BIG_CAP_D + CAP_CLR))
    for x in KEY_XS:
        kx, ky = KEY[0] + CAP_CLR, KEY[1] + CAP_CLR
        cuts.append(box(x - kx / 2, x + kx / 2, KEY_Y - ky / 2, KEY_Y + ky / 2, Z_UNDER - GUIDE_L - OVS, H + OVS))
    cuts.append(cyl(*ENC, Z_UNDER - OVS, H + OVS, ENC_HOLE))
    # Back wall: the Pi's USB / Ethernet window, a panel USB-C, vent slots.
    yb0, yb1 = D - WALL - OVS, D + OVS
    cuts.append(box(PI_X0 + 1.0, PI_X0 + PI[0] - 1.0, yb0, yb1, PI_Z0 + PI[2] - 1.0, PI_Z0 + PI[2] + PI_PORT_H + 2.0))
    cuts.append(box(48.0 - 5.0, 48.0 + 5.0, yb0, yb1, 18.0 - 2.2, 18.0 + 2.2))
    for x in (48.0 - 9.0, 48.0 + 9.0):
        cuts.append(Pos(x, D - WALL / 2, 18.0) * Rot(90, 0, 0) * Cylinder(1.1, WALL + 2 * OVS))
    for i in range(8):
        x = 148.0 + i * 6.0
        cuts.append(box(x - 1.25, x + 1.25, yb0, yb1, 12.0, 34.0))
    # Heat-set insert bores in the base-screw bosses.
    for x, y in base_screws():
        cuts.append(cyl(x, y, BASE_T - OVS, BASE_T + INSERT_L, INSERT_D))
    # Screen frame screws (M3 inserts, blind).
    assert FRAME_BOSS_Z0 + INSERT_DEPTH <= H - SKIN, "screen frame insert would break through the top"
    assert Z_UNDER + 0.3 <= H - SKIN, "fader insert would break through the top"
    for sx in (-1, 1):
        for sy in (-1, 1):
            cuts.append(cyl(SCR_CX + sx * SCR_BOSS[0], SCR_CY + sy * SCR_BOSS[1],
                            FRAME_BOSS_Z0 - OVS, FRAME_BOSS_Z0 + INSERT_DEPTH, INSERT_D))
    # Reveal line round the outside at the seam.
    rw, rd = REVEAL
    y0, y1 = Y_SEAM - rw / 2, Y_SEAM + rw / 2
    cuts.append(box(-OVS, W + OVS, y0, y1, H - rd, H + OVS))
    cuts.append(box(-OVS, rd, y0, y1, BASE_T - OVS, H + OVS))
    cuts.append(box(W - rd, W + OVS, y0, y1, BASE_T - OVS, H + OVS))
    return body.cut(*cuts)


def split(solid):
    """(front, back): split at the seam, tongue on the back, groove in the front."""
    big = 1000.0
    front = solid & box(-OVS, W + OVS, -OVS, Y_SEAM, -OVS, H + OVS)
    back = solid & box(-OVS, W + OVS, Y_SEAM, D + OVS, -OVS, H + OVS)
    a, b = TONGUE
    tongue = u_profile(a, b, Y_SEAM - TONGUE_L, Y_SEAM + 1.0, TONGUE_Z0)
    groove = u_profile(a - FIT, b + FIT, Y_SEAM - TONGUE_L - FIT, Y_SEAM + OVS, TONGUE_Z0 - FIT)
    back = back.fuse(tongue)
    front = front.cut(groove)
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
    front, back = parts()
    for name, s in (("top_front", front), ("top_back", back)):
        out = os.path.join(here, "build", f"{name}.stl")
        export_stl(s, out)
        m = trimesh.load(out)
        ext = (m.bounds[1] - m.bounds[0]).round(1)
        print(f"{name}: {ext} bodies: {len(m.split(only_watertight=False))} watertight: {m.is_watertight}"
              f" fits bed: {ext[0] <= BED[0] and ext[1] <= BED[1]}")
