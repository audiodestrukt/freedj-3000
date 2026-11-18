"""top_shell.py — the full-size deck's top: one shell, printed as four pieces.

    python3 hardware/cad/deck7/top_shell.py  ->  build/top_{fl,fr,bl,br}.stl

As deck5's top, but the shell is too big for one seam on a 256 mm bed, so
it splits on a 2 x 2 grid: Y_SEAM across the depth and X_SEAM along it,
both under the platter (the cross is hidden by it).  Each seam gets the
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
    return [(x, y) for x, y, _ in KEYS]


def whole():
    body = outer() - rrect(WALL, BASE_T - OVS, Z_UNDER)
    adds = []
    inner = rrect(BAND_T, BASE_T - OVS, H - BAND_T)
    adds.append((outer() & box(-OVS, W + OVS, Y_SEAM - BAND_HALF, Y_SEAM + BAND_HALF, 0, H + OVS)) - inner)
    adds.append((outer() & box(X_SEAM - BAND_HALF, X_SEAM + BAND_HALF, -OVS, D + OVS, 0, H + OVS)) - inner)
    for x, y in base_screws():
        adds.append(cyl(x, y, BASE_T, Z_UNDER + 1.0, BOSS_D))
    for sy in (-1, 1):
        adds.append(cyl(FADER_X, FADER_CY + sy * FADER_BOSS_Y, Z_UNDER - FADER_BOSS_H, Z_UNDER + 1.0, BOSS_D))
    adds.append(cyl(JOG_CX, JOG_CY, BRG_UP_Z - 1.0, Z_UNDER + 1.0, JOG_BOSS_D))
    for sx in (-1, 1):
        for sy in (-1, 1):
            adds.append(cyl(SCR_CX + sx * SCR_BOSS[0], SCR_CY + sy * SCR_BOSS[1], FRAME_BOSS_Z0, Z_UNDER + 1.0, 8.0))
    for x, y in (PLAY, CUE):
        adds.append(cyl(x, y, Z_UNDER - GUIDE_L, Z_UNDER + 1.0, BIG_CAP_D + CAP_CLR + 2 * GUIDE_WALL))
    kx, ky = KEY[0] + CAP_CLR + 2 * GUIDE_WALL, KEY[1] + CAP_CLR + 2 * GUIDE_WALL
    for x, y in keys():
        adds.append(box(x - kx / 2, x + kx / 2, y - ky / 2, y + ky / 2, Z_UNDER - GUIDE_L, Z_UNDER + 1.0))
    body = body.fuse(*adds)

    cuts = []
    ow, oh = SCR_OUT[0] + 2 * SCR_FIT, SCR_OUT[1] + 2 * SCR_FIT
    cuts.append(box(SCR_CX - ow / 2, SCR_CX + ow / 2, SCR_CY - oh / 2, SCR_CY + oh / 2, SCR_Z_BOT - OVS, H + OVS))
    cuts.append(cyl(JOG_CX, JOG_CY, BRG_UP_Z, H + OVS, SHAFT_D + 1.0))
    cuts.append(cyl(JOG_CX, JOG_CY, BRG_UP_Z - 1.0 - OVS, BRG_UP_Z + BRG[2], BRG[0] + BRG_FIT))
    slot = FADER_TRAVEL + FADER_LEVER[1] + 1.0
    cuts.append(box(FADER_X - FADER_SLOT_W / 2, FADER_X + FADER_SLOT_W / 2,
                    FADER_CY - slot / 2, FADER_CY + slot / 2, Z_UNDER - OVS, H + OVS))
    for sy in (-1, 1):
        cuts.append(cyl(FADER_X, FADER_CY + sy * FADER_BOSS_Y, Z_UNDER - FADER_BOSS_H - OVS, Z_UNDER + 0.3, INSERT_D))
    for x, y in (PLAY, CUE):
        cuts.append(cyl(x, y, Z_UNDER - GUIDE_L - OVS, H + OVS, BIG_CAP_D + CAP_CLR))
    hx, hy = KEY[0] + CAP_CLR, KEY[1] + CAP_CLR
    for x, y in keys():
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
    assert Z_UNDER + 0.3 <= H - SKIN, "fader insert would break through the top"
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
