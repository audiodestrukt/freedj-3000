"""mini_lm393.py — the mini with two premade LM393 slot-sensor boards in
place of the TCST1202 brackets, to demo the optical quadrature quickly.

Dan's boards (2026-11-23): 14.25 × 31.25, the slotted interrupter across one
short end, its beam across the board's width.  So each board lies flat and
TANGENTIAL: its centre line touches the fin's circle at the sensor, the fin
runs down the interrupter's gap, and the board runs off straight from there,
away from the other board.  A straight board leaves the circle: its far end
passes under the fin (5 mm over the board, clear of its chips) and out under
the bearing circle, so the bearings it would hit are left off (8 of 10).

Only the carrier is new (Dan: print nothing else): each board drops into a
pocket moulded on it, raised so the board's beam crosses the mini ring's fin
slots (z 12.3..16.8) at z 14.3, their middle: ±2 mm for an error in the
guessed beam height.  The pocket is open underneath, through the carrier, for
the interrupter's leads and the wires.  The ring, retainer and hub are the
mini's.

⚠ Not calipered: the interrupter's gap, its depth, the beam's height and
where along the board the sensor sits are an ITR9608's typical numbers.
Check them on a board: the beam must fall inside the fin's slots.
"""
import math
from build123d import *
from params import OVS
import mini
from mini import CT, CODE_R, CODE_SLOT_H, OPTO_A, OPTO_B, A_W, PAD, R_W, POST_T, SHOULDER, W_W
from rig import cyl, box

# ── the board (Dan's calipers) ──
BOARD_L, BOARD_W, BOARD_T = 31.25, 14.25, 1.5   # FR4 1.5 (Dan)
# ── its interrupter: 10 mm tall over the board (Dan); the rest ⚠ typical, not calipered ──
SENSE_AT = 4.5                     # the beam's centre from the board's sensor end
BODY = (6.5, 12.5, 10.0)           # the interrupter: along the board, across it, height over the board
GAP, GAP_DEPTH = 5.0, 6.5          # the slot's width (radial here) and depth from the body's top
BEAM_H = BODY[2] - 3.5             # the beam over the board's top face: 3.5 under the body's top, as a TCST
HEADER = (2.54 * 3, 2.54, 8.5, 2.5)  # a straight 3-pin header: across, along, height, its centre from the far end ⚠
# ── the pocket on the carrier ──
TRAY_WALL, TRAY_FIT, TRAY_LIP = 1.2, 0.3, 0.8   # walls, slack round the board, walls over its top
LEDGE = 1.0                        # the ledge under the board's edges (Dan: 1 mm, clear of the through-hole pins underneath); open inside, through the carrier
HEADER_SLOT = 10.0                 # the notch in the far end wall for the header's wires (Dan)

CODE_Z0 = mini.CODE_Z0             # the mini ring's fin, as printed
BEAM_Z = CODE_Z0 + CODE_SLOT_H / 2 - 0.25   # 14.3: the middle of its slots
BOARD_Z = BEAM_Z - BEAM_H - BOARD_T        # the board's underside: 5.7, 1.7 over the carrier
DIRS = {OPTO_A: -1, OPTO_B: +1}    # each board runs off away from the other

assert CODE_Z0 > BOARD_Z + BOARD_T + BODY[2] - GAP_DEPTH + 0.5, "the fin touches the gap's floor"
assert CODE_Z0 + CODE_SLOT_H > BEAM_Z + 1.0 and CODE_Z0 < BEAM_Z - 1.0, "the slots miss the beam"
assert GAP - mini.CODE_T >= 2.0, "the fin in the gap"
assert BOARD_Z >= CT + 0.8, "no ledge under the board"


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
    """A board's pocket, moulded on the carrier (sensor frame)."""
    y0, y1 = ys()
    bw, f, t = BOARD_W / 2, TRAY_FIT, TRAY_WALL
    top = BOARD_Z + BOARD_T + TRAY_LIP
    s = box(-bw - f - t, bw + f + t, y0 - f - t, y1 + f + t, 0, top)
    s -= box(-bw - f, bw + f, y0 - f, y1 + f, BOARD_Z, top + OVS)                      # the board's pocket
    s -= box(-bw + LEDGE, bw - LEDGE, y0 + LEDGE, y1 - LEDGE, -OVS, BOARD_Z + OVS)     # open under it, through the carrier: leads
    s -= box(-HEADER_SLOT / 2, HEADER_SLOT / 2, y1 - 1, y1 + f + t + OVS, BOARD_Z, top + OVS)   # the wires out the far end
    s -= box(-GAP / 2 - 0.5, GAP / 2 + 0.5, y0 - f - t - OVS, y0 + 1, BOARD_Z, top + OVS)  # the fin's way past the sensor end wall
    # The pocket's inner wall reaches r 30.9, over the hub's flange (r 31.2):
    # cut back round it (the board's own edge is at r 32.4).
    s -= Pos(-CODE_R, 0, 0) * cyl(0, 0, CT, top + OVS, 2 * (mini.HUB_FL[0] + 0.4))
    return s


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


# The boards and their pockets take 44..102° and 126..182° of the bearing circle
# (found by clear_angles()), so the mini's ten even bearings become nine
# spread over what is left: the widest gap 62°, at 72 and at 154.
STATIONS = [6.0, 42.0, 104.0, 124.0, 184.0, 220.0, 256.0, 292.0, 328.0]


def clear_angles(angles):
    """Of `angles`, those where a bearing and its post miss both boards, pockets and headers."""
    hits = [place(a, tray()) for a in DIRS] + [place(a, p) for a in DIRS for p in board()]
    out = []
    for w in angles:
        st = mini.station(mini.BEARING, w) + Rot(0, 0, w) * Pos(R_W, 0, mini.RC) * mini.bearing(mini.BEARING)
        if all((st & h).volume < 0.01 for h in hits):
            out.append(w)
    return out
# The pad under both pockets: from the hub's ring out to the carrier's ring,
# from past board A's far end to past board B's.
_reach = math.degrees(math.atan2(ys()[1] + 3.0, CODE_R)) + 3.0
PAD_B = (PAD[0], mini.CAR[1], OPTO_A - _reach, OPTO_B + _reach)


def carrier():
    """The mini's carrier with the two pockets moulded on, nine bearings."""
    s = mini.carrier(stations=STATIONS, pad=PAD_B, tcst=False)
    y0, y1 = ys()
    for a in DIRS:
        s += place(a, tray())
        s -= place(a, box(-BOARD_W / 2 + LEDGE, BOARD_W / 2 - LEDGE, y0 + LEDGE, y1 - LEDGE, -OVS, BOARD_Z + OVS))   # keep the leads' opening clear of the pad
    return s


def printed():
    return {"mini_lm393/carrier": (carrier(), False, 1)}


def assembly():
    s = [(n, sol, k) for n, sol, k in mini.assembly()
         if not n.startswith(("mini/opto_bracket", "opto_", "b", "mini/carrier"))]
    s.append(("mini_lm393/carrier", carrier(), "plate"))
    b = mini.bearing(mini.BEARING)
    for w in STATIONS:
        s.append((f"b{mini.BEARING}_{int(w)}", Rot(0, 0, w) * Pos(R_W, 0, mini.RC) * b, "steel"))
    for tag, a in (("a", OPTO_A), ("b", OPTO_B)):
        pb, body, hdr = board()
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
