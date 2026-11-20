"""top_shell.py — the full-size deck's top: one shell, printed as four pieces.

    python3 hardware/cad/deck7/top_shell.py  ->  build/top_{fl,fr,bl,br}.stl

As deck5's top, but the shell is too big for one seam on a 256 mm bed, so
it splits on a 2 x 2 grid: Y_SEAM across the depth and X_SEAM along it,
missing every key and boss (the cross is under the platter).  Each seam gets the
deck5 joint: a thickened band, a tongue on one side (the BACK pieces for
Y_SEAM, the RIGHT pieces for X_SEAM) in a groove on the other, and a
reveal line outside.  The tongues stop short of the cross so they never
meet.  The base's laps bridge both seams and its screws clamp them.

Print every piece TOP FACE DOWN, no supports.
"""
import os
from build123d import *
from params import *

OVS = 1.0
PIECES = ("fl", "fr", "bl", "br")          # front-left, front-right, back-left, back-right


def rrect(inset, z0, z1):
    w, d, r = W - 2 * inset, D - 2 * inset, max(R_PLAN - inset, 0.5)
    return Pos(W / 2, D / 2, z0) * extrude(RectangleRounded(w, d, r), z1 - z0)


def box(x0, x1, y0, y1, z0, z1):
    return Pos((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2) * Box(x1 - x0, y1 - y0, z1 - z0)


def cyl(x, y, z0, z1, d):
    return Pos(x, y, (z0 + z1) / 2) * Cylinder(d / 2, z1 - z0)


def outer():
    o = rrect(0, BASE_T, H)
    return fillet(o.edges().group_by(Axis.Z)[-1], EDGE_R)


def u_across_y(a, b, y0, y1, z0):
    """A U across a Y seam (top plate + left / right walls), insets a..b."""
    return box(a, W - a, y0, y1, z0, H - a) - box(b, W - b, y0 - OVS, y1 + OVS, z0 - OVS, H - b)


def u_across_x(a, b, x0, x1, z0):
    """A U across an X seam (top plate + front / back walls), insets a..b."""
    return box(x0, x1, a, D - a, z0, H - a) - box(x0 - OVS, x1 + OVS, b, D - b, z0 - OVS, H - b)


def keys():
    """(x, y, w, h) per key: each key has its own cap size."""
    return [(x, y, w, h) for x, y, _, (w, h) in KEYS]


def fader_pads():
    """(y0, y1) of the two pads the fader's end plates sit on: from just past
    the lever's travel to the body's end, around each screw."""
    clear = FADER_TRAVEL / 2 + FADER_LEVER[1] / 2 + 1.0
    end = FADER_BODY[1] / 2
    assert clear + 1.5 < FADER_PITCH / 2 - M3_CLR / 2, "pad too short for the screw"
    return [tuple(sorted((FADER_CY + s * clear, FADER_CY + s * end))) for s in (-1, 1)]


def enc_cage():
    """The AS5600's cage under the jog boss (see params): four corner posts,
    each an L round a board corner with a stop over it, up into the top
    plate; and two spring fingers on the y sides, hanging from the boss, each
    with a lip under the board's edge and a lead-in to snap it in from below."""
    cx, cy = JOG_CX, JOG_CY
    a = ENC_PCB[0] / 2 + CAGE_CLR             # pocket half-width
    b = a + CAGE_WALL
    z_lo = ENC_Z0 - 0.8                       # bottom of the lips and posts (0.2 over the base)
    z_top = ENC_Z0 + ENC_PCB[2] + 0.1         # underside of the stops over the board
    out = []
    for sx in (-1, 1):
        for sy in (-1, 1):
            x0, x1 = sorted((cx + sx * a, cx + sx * b))
            y0, y1 = sorted((cy + sy * a, cy + sy * b))
            xl0, xl1 = sorted((cx + sx * (a - CAGE_LEG), cx + sx * b))
            yl0, yl1 = sorted((cy + sy * (a - CAGE_LEG), cy + sy * b))
            out.append(box(x0, x1, yl0, yl1, z_lo, Z_UNDER + 1.0))        # leg along y
            out.append(box(xl0, xl1, y0, y1, z_lo, Z_UNDER + 1.0))        # leg along x
            # stop over the board's corner, outside the lower bearing's path
            sx0, sx1 = sorted((cx + sx * (a - 3.0), cx + sx * b))
            sy0, sy1 = sorted((cy + sy * (a - 3.0), cy + sy * b))
            out.append(box(sx0, sx1, sy0, sy1, z_top, z_top + 2.0))
    w, t, lip = FINGER
    for sy in (-1, 1):
        y0, y1 = sorted((cy + sy * a, cy + sy * (a + t)))
        out.append(box(cx - w / 2, cx + w / 2, y0, y1, z_lo, BRG_LO_Z + 1.0))
        # lip: flat on top (the board rests on it), 45 degree lead-in below
        d = CAGE_CLR + lip                    # reaches `lip` under the board's edge
        prof = Polygon((sy * (a + 0.5), z_lo), (sy * a, z_lo), (sy * (a - d), ENC_Z0), (sy * (a + 0.5), ENC_Z0), align=None)   # overlaps the finger
        out.append(Pos(cx - w / 2, cy, 0) * extrude(Plane.YZ * prof, w))
    return out


def whole():
    body = outer() - rrect(WALL, BASE_T - OVS, Z_UNDER)
    adds = []
    inner = rrect(BAND_T, BASE_T - OVS, H - BAND_T)
    adds.append((outer() & box(-OVS, W + OVS, Y_SEAM - BAND_HALF, Y_SEAM + BAND_HALF, 0, H + OVS)) - inner)
    adds.append((outer() & box(X_SEAM - BAND_HALF, X_SEAM + BAND_HALF, -OVS, D + OVS, 0, H + OVS)) - inner)
    for x, y in base_screws():
        adds.append(cyl(x, y, BASE_T, Z_UNDER + 1.0, BOSS_D))
    for (y0, y1) in fader_pads():
        adds.append(box(FADER_X - FADER_BODY[0] / 2, FADER_X + FADER_BODY[0] / 2, y0, y1, Z_UNDER - FADER_PAD_T, Z_UNDER + 1.0))
    adds.append(cyl(JOG_CX, JOG_CY, BRG_LO_Z, Z_UNDER + 1.0, JOG_BOSS_D))
    adds += enc_cage()
    for sx in (-1, 1):
        for sy in (-1, 1):
            adds.append(cyl(SCR_CX + sx * SCR_BOSS[0], SCR_CY + sy * SCR_BOSS[1], FRAME_BOSS_Z0, Z_UNDER + 1.0, 8.0))
    for x, y in (PLAY, CUE):
        adds.append(cyl(x, y, Z_UNDER - GUIDE_L, Z_UNDER + 1.0, BIG_CAP_D + CAP_CLR + 2 * GUIDE_WALL))
    for x, y, w, h in keys():
        kx, ky = w + CAP_CLR + 2 * GUIDE_WALL, h + CAP_CLR + 2 * GUIDE_WALL
        adds.append(box(x - kx / 2, x + kx / 2, y - ky / 2, y + ky / 2, Z_UNDER - GUIDE_L, Z_UNDER + 1.0))
    body = body.fuse(*adds)

    cuts = []
    ow, oh = SCR_OUT[0] + 2 * SCR_FIT, SCR_OUT[1] + 2 * SCR_FIT
    cuts.append(box(SCR_CX - ow / 2, SCR_CX + ow / 2, SCR_CY - oh / 2, SCR_CY + oh / 2, SCR_Z_BOT - OVS, H + OVS))
    bd = BRG[0] + BRG_FIT
    cuts.append(cyl(JOG_CX, JOG_CY, BRG_UP_Z, H + OVS, bd))                          # upper seat, from above
    cuts.append(cyl(JOG_CX, JOG_CY, BRG_LO_Z - OVS, BRG_UP_Z + OVS, BRG_MID_D))       # between the ledges
    cuts.append(cyl(JOG_CX, JOG_CY, BRG_LO_Z - OVS, BRG_LO_Z + BRG[2], bd))           # lower seat, from below
    slot = FADER_TRAVEL + FADER_LEVER[1] + 1.0
    cuts.append(box(FADER_X - FADER_SLOT_W / 2, FADER_X + FADER_SLOT_W / 2,
                    FADER_CY - slot / 2, FADER_CY + slot / 2, Z_UNDER - OVS, H + OVS))
    h = (M3_CSK - M3_CLR) / 2     # 90 degree countersink, head flush with the top
    for sy in (-1, 1):
        y = FADER_CY + sy * FADER_PITCH / 2
        cuts.append(cyl(FADER_X, y, Z_UNDER - FADER_PAD_T - OVS, H + OVS, M3_CLR))
        cuts.append(Pos(FADER_X, y, H - h) * Cone(M3_CLR / 2, M3_CSK / 2 + OVS, h + OVS, align=(Align.CENTER, Align.CENTER, Align.MIN)))
    for x, y in (PLAY, CUE):
        cuts.append(cyl(x, y, Z_UNDER - GUIDE_L - OVS, H + OVS, BIG_CAP_D + CAP_CLR))
    for x, y, w, h in keys():
        hx, hy = w + CAP_CLR, h + CAP_CLR
        cuts.append(box(x - hx / 2, x + hx / 2, y - hy / 2, y + hy / 2, Z_UNDER - GUIDE_L - OVS, H + OVS))
    cuts.append(cyl(*ENC, Z_UNDER - OVS, H + OVS, ENC_HOLE))
    # Back wall: the Pi's USB / Ethernet window, the panel USB-C, vents.
    yb0, yb1 = D - WALL - OVS, D + OVS
    cuts.append(box(PI_X0 + 1.0, PI_X0 + PI[0] - 1.0, yb0, yb1, PI_Z0 + PI[2] - 1.0, PI_Z0 + PI[2] + PI_PORT_H + 2.0))
    cuts.append(box(USBC_X - 5.0, USBC_X + 5.0, yb0, yb1, USBC_Z - 2.2, USBC_Z + 2.2))
    for x in (USBC_X - 9.0, USBC_X + 9.0):
        cuts.append(Pos(x, D - WALL / 2, USBC_Z) * Rot(90, 0, 0) * Cylinder(1.1, WALL + 2 * OVS))
    for x in VENT_XS:
        cuts.append(box(x - 1.25, x + 1.25, yb0, yb1, 12.0, H - 14.0))
    for x, y in base_screws():
        cuts.append(cyl(x, y, BASE_T - OVS, BASE_T + INSERT_L, INSERT_D))
    assert FRAME_BOSS_Z0 + INSERT_DEPTH <= H - SKIN, "screen frame insert would break through the top"
    for sx in (-1, 1):
        for sy in (-1, 1):
            cuts.append(cyl(SCR_CX + sx * SCR_BOSS[0], SCR_CY + sy * SCR_BOSS[1],
                            FRAME_BOSS_Z0 - OVS, FRAME_BOSS_Z0 + INSERT_DEPTH, INSERT_D))
    # Reveal lines on both seams: across the top and down the walls they cross.
    rw, rd = REVEAL
    y0, y1 = Y_SEAM - rw / 2, Y_SEAM + rw / 2
    cuts += [box(-OVS, W + OVS, y0, y1, H - rd, H + OVS),
             box(-OVS, rd, y0, y1, BASE_T - OVS, H + OVS), box(W - rd, W + OVS, y0, y1, BASE_T - OVS, H + OVS)]
    x0, x1 = X_SEAM - rw / 2, X_SEAM + rw / 2
    cuts += [box(x0, x1, -OVS, D + OVS, H - rd, H + OVS),
             box(x0, x1, -OVS, rd, BASE_T - OVS, H + OVS), box(x0, x1, D - rd, D + OVS, BASE_T - OVS, H + OVS)]
    return body.cut(*cuts)


def cell(p):
    """The piece's share of the deck, as a box (x and y ranges)."""
    xs = (-OVS, X_SEAM) if p[1] == "l" else (X_SEAM, W + OVS)
    ys = (-OVS, Y_SEAM) if p[0] == "f" else (Y_SEAM, D + OVS)
    return xs, ys


# Tongues stop this far short of the other seam, so the two never meet at the cross.
CROSS_GAP = TONGUE_L + FIT + 1.0


def split(solid):
    a, b = TONGUE
    ty = u_across_y(a, b, Y_SEAM - TONGUE_L, Y_SEAM + 1.0, TONGUE_Z0)         # on the back pieces
    gy = u_across_y(a - FIT, b + FIT, Y_SEAM - TONGUE_L - FIT, Y_SEAM + OVS, TONGUE_Z0 - FIT)
    tx = u_across_x(a, b, X_SEAM - TONGUE_L, X_SEAM + 1.0, TONGUE_Z0)         # on the right pieces
    gx = u_across_x(a - FIT, b + FIT, X_SEAM - TONGUE_L - FIT, X_SEAM + OVS, TONGUE_Z0 - FIT)
    # Only where the shell has material: a tongue must not fill an opening
    # it runs across (X_SEAM crosses the screen opening).
    ty, tx = ty & solid, tx & solid
    out = {}
    for p in PIECES:
        (x0, x1), (y0, y1) = cell(p)
        s = solid & box(x0, x1, y0, y1, -OVS, H + OVS)
        if p[0] == "b":   # its share of the Y tongue (its x range, short of the cross)
            xa, xb = (x0, X_SEAM - CROSS_GAP) if p[1] == "l" else (X_SEAM + CROSS_GAP, x1)
            s = s.fuse(ty & box(xa, xb, -OVS, D + OVS, -OVS, H + OVS))
        else:
            s = s.cut(gy)
        if p[1] == "r":   # its share of the X tongue (its y range, short of the cross)
            ya, yb = (y0, Y_SEAM - CROSS_GAP) if p[0] == "f" else (Y_SEAM + CROSS_GAP, y1)
            s = s.fuse(tx & box(-OVS, W + OVS, ya, yb, -OVS, H + OVS))
        else:
            s = s.cut(gx)
        out[p] = s
    return out


_cache = {}
def parts():
    """{"fl": solid, "fr": …, "bl": …, "br": …}"""
    if not _cache:
        _cache.update(split(whole()))
    return dict(_cache)


if __name__ == "__main__":
    import trimesh
    os.makedirs(os.path.join(HERE, "build"), exist_ok=True)
    for p, s in parts().items():
        out = os.path.join(HERE, "build", f"top_{p}.stl")
        export_stl(s, out)
        m = trimesh.load(out)
        ext = (m.bounds[1] - m.bounds[0]).round(1)
        print(f"top_{p}: {ext} bodies: {len(m.split(only_watertight=False))} watertight: {m.is_watertight}"
              f" fits bed: {ext[0] <= BED[0] and ext[1] <= BED[1]}")
