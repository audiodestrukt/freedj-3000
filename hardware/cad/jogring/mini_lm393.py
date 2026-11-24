"""mini_lm393.py — the mini with two premade LM393 slot-sensor boards in
place of the TCST1202 brackets, to demo the optical quadrature quickly.

Dan's boards (2026-11-23): 14.25 × 31.25, the slotted interrupter across one
short end, its beam across the board's width.  So each board lies flat and
TANGENTIAL: its centre line touches the fin's circle at the sensor, the fin
runs down the interrupter's gap, and the board runs off straight from there,
away from the other board.  A straight board leaves the circle: its far end
passes under the fin (5 mm over the board, clear of its chips) and out under
the bearing circle, so the bearings it would hit are left off (8 of 10).

Each board drops into a tray (walls round it, open underneath for the
interrupter's leads); the tray screws up through the carrier with two M3
thread-forming screws in ears past the board's ends.  The carrier's opto pad
is widened under both trays.  The ring is the mini's, its fin lowered or
raised so its slots cross this board's beam.

⚠ Not calipered: the interrupter's gap, its depth, the beam's height and
where along the board the sensor sits are an ITR9608's typical numbers.
Check them on a board before printing the ring (ring() is the only part
they change besides the tray).
"""
import math
from build123d import *
from params import OVS, M3_CLR
import mini
from mini import CT, CODE_R, CODE_SLOT_H, OPTO_A, OPTO_B, A_W, PAD, R_W, POST_T, SHOULDER, W_W
from rig import cyl, box

# ── the board (Dan's calipers) ──
BOARD_L, BOARD_W, BOARD_T = 31.25, 14.25, 1.6
# ── its interrupter (ITR9608-class, ⚠ typical, not calipered) ──
SENSE_AT = 4.5                     # the beam's centre from the board's sensor end
BODY = (6.5, 12.5, 11.0)           # the interrupter: along the board, across it, height over the board
GAP, GAP_DEPTH = 5.0, 7.5          # the slot's width (radial here) and depth from the body's top
BEAM_H = 7.0                       # the beam over the board's top face
HEADER = (2.54 * 3, 2.54, 8.5, 2.5)  # a straight 3-pin header: across, along, height, its centre from the far end ⚠
# ── the tray ──
TRAY_T, TRAY_WALL, TRAY_FIT, TRAY_LIP = 2.0, 1.2, 0.3, 0.8   # base, walls, slack round the board, walls over its top
LEDGE = 2.0                        # the base under the board's edges; open inside for the leads
EAR, EAR_T, EAR_D, SCREW_D = 4.5, 5.0, 7.5, 2.5   # an ear past each end: reach, thickness, boss, thread-forming pilot

BOARD_Z = CT + TRAY_T              # the board's underside
BEAM_Z = BOARD_Z + BOARD_T + BEAM_H
CODE_Z0 = BEAM_Z - 2.0             # the fin's foot: 2 under the beam, the slots reach 2.5 over it
DIRS = {OPTO_A: -1, OPTO_B: +1}    # each board runs off away from the other

assert CODE_Z0 > BOARD_Z + BOARD_T + BODY[2] - GAP_DEPTH + 0.5, "the fin touches the gap's floor"
assert CODE_Z0 + CODE_SLOT_H > BEAM_Z + 1.0 and CODE_Z0 < BEAM_Z - 1.0, "the slots miss the beam"
assert GAP - mini.CODE_T >= 2.0, "the fin in the gap"


def place(a, solid):
    """A part drawn in the sensor's frame (origin on the fin's circle, x
    radial out, y along the board towards where it runs off, z up from the
    carrier's underside), put at sensor a; mirrored for a board running off
    the other way."""
    s = solid if DIRS[a] > 0 else solid.mirror(Plane.XZ)
    return Rot(0, 0, a) * Pos(CODE_R, 0, 0) * s


def ys():
    """The board's ends along y: the sensor end, then the far end."""
    return -SENSE_AT, BOARD_L - SENSE_AT


def tray():
    y0, y1 = ys()
    bw, f, t = BOARD_W / 2, TRAY_FIT, TRAY_WALL
    top = BOARD_Z + BOARD_T + TRAY_LIP
    s = box(-bw - f - t, bw + f + t, y0 - f - t, y1 + f + t, CT, top)
    s -= box(-bw - f, bw + f, y0 - f, y1 + f, BOARD_Z, top + OVS)                      # the board's pocket
    s -= box(-bw + LEDGE, bw - LEDGE, y0 + LEDGE, y1 - LEDGE, CT - OVS, BOARD_Z + OVS)  # open under it: leads
    s -= box(-HEADER[0] / 2 - 1, HEADER[0] / 2 + 1, y1 - 1, y1 + f + t + OVS, BOARD_Z, top + OVS)   # the wires out the far end
    s -= box(-GAP / 2 - 0.5, GAP / 2 + 0.5, y0 - f - t - OVS, y0 + 1, BOARD_Z, top + OVS)  # the fin's way past the sensor end wall
    # The tray's inner wall reaches r 30.9, over the hub's flange (r 31.2):
    # cut back round it (the board's own edge is at r 32.4).
    s -= Pos(-CODE_R, 0, 0) * cyl(0, 0, CT - OVS, top + OVS, 2 * (mini.HUB_FL[0] + 0.4))
    for y, end in zip(ear_ys(), (y0, y1)):
        s += cyl(0, y, CT, CT + EAR_T, EAR_D)                                          # a boss for the screw
        s += box(-EAR_D / 2, EAR_D / 2, min(y, end), max(y, end), CT, BOARD_Z)          # joined to the tray's base
        s -= cyl(0, y, CT - OVS, CT + EAR_T + OVS, SCREW_D)
    return s


def ear_ys():
    """The two screws, past the board's ends."""
    y0, y1 = ys()
    f, t = TRAY_FIT, TRAY_WALL
    return y0 - f - t - EAR / 2 - 0.5, y1 + f + t + EAR / 2 + 0.5


def board():
    """The board, its interrupter (gap open along y, over x = 0) and header."""
    y0, y1 = ys()
    b = box(-BOARD_W / 2, BOARD_W / 2, y0, y1, BOARD_Z, BOARD_Z + BOARD_T)
    z = BOARD_Z + BOARD_T
    body = box(-BODY[1] / 2, BODY[1] / 2, -BODY[0] / 2, BODY[0] / 2, z, z + BODY[2])
    body -= box(-GAP / 2, GAP / 2, -BODY[0], BODY[0], z + BODY[2] - GAP_DEPTH, z + BODY[2] + OVS)
    hy = y1 - HEADER[3]
    hdr = box(-HEADER[0] / 2, HEADER[0] / 2, hy - HEADER[1] / 2, hy + HEADER[1] / 2, z, z + HEADER[2])
    return b, body, hdr


# The boards and trays take 44..102° and 126..182° of the bearing circle
# (found by clear_angles()), so the mini's ten even bearings become nine
# spread over what is left: the widest gap 62°, at 72 and at 154.
STATIONS = [6.0, 42.0, 104.0, 124.0, 184.0, 220.0, 256.0, 292.0, 328.0]


def clear_angles(angles):
    """Of `angles`, those where a bearing and its post miss both boards, trays and headers."""
    hits = [place(a, tray()) for a in DIRS] + [place(a, p) for a in DIRS for p in board()]
    out = []
    for w in angles:
        st = mini.station(mini.BEARING, w) + Rot(0, 0, w) * Pos(R_W, 0, mini.RC) * mini.bearing(mini.BEARING)
        if all((st & h).volume < 0.01 for h in hits):
            out.append(w)
    return out
# The pad under both trays: from the hub's ring out to the carrier's ring,
# from past board A's far end to past board B's.
_reach = math.degrees(math.atan2(ear_ys()[1] + EAR_D / 2, CODE_R)) + 3.0
PAD_B = (PAD[0], mini.CAR[1], OPTO_A - _reach, OPTO_B + _reach)


def carrier():
    s = mini.carrier(stations=STATIONS, pad=PAD_B, tcst=False)
    for a in DIRS:
        for y in ear_ys():
            s -= place(a, cyl(0, y, -OVS, CT + OVS, M3_CLR))
    return s


def ring():
    return mini.ring(code_z0=CODE_Z0)


def printed():
    return {
        "mini_lm393/ring": (ring(), True, 1),
        "mini_lm393/carrier": (carrier(), False, 1),
        "mini_lm393/tray_a": (tray().mirror(Plane.XZ), False, 1),   # board A runs off the other way
        "mini_lm393/tray_b": (tray(), False, 1),
    }


def assembly():
    s = [(n if n != "mini/ring" else "mini_lm393/ring", sol, k) for n, sol, k in mini.assembly()
         if not n.startswith(("mini/opto_bracket", "opto_", "b", "mini/carrier"))]
    s = [i for i in s if i[0] != "mini_lm393/ring"] + [("mini_lm393/ring", ring(), "ring"), ("mini_lm393/carrier", carrier(), "plate")]
    b = mini.bearing(mini.BEARING)
    for w in STATIONS:
        s.append((f"b{mini.BEARING}_{int(w)}", Rot(0, 0, w) * Pos(R_W, 0, mini.RC) * b, "steel"))
    for tag, a in (("a", OPTO_A), ("b", OPTO_B)):
        pb, body, hdr = board()
        s.append((f"mini_lm393/tray_{tag}", place(a, Pos(0, 0, 0.05) * tray()), "hub"))
        s.append((f"lm393_board_{tag}", place(a, Pos(0, 0, 0.06) * pb), "plate"))
        s.append((f"lm393_opto_{tag}", place(a, Pos(0, 0, 0.07) * body), "black"))
        s.append((f"lm393_header_{tag}", place(a, Pos(0, 0, 0.07) * hdr), "black"))
    return s


def encoder_check(ring_mesh):
    import pioneer
    return pioneer.encoder_check(ring_mesh, "mini_lm393", CODE_R, mini.CODE_N, OPTO_A, OPTO_B, BEAM_Z)


if __name__ == "__main__":
    assert clear_angles(STATIONS) == STATIONS, "a bearing hits a board"
    print("bearings at", STATIONS)
    print(f"board underside z {BOARD_Z}, beam z {BEAM_Z}, fin foot z {CODE_Z0} (mini's: {mini.CODE_Z0:.1f})")
