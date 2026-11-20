"""params.py — the full-size deck (ADK-1000): single source of truth.

An XDJ-1000MK2-sized deck around a 206 mm jog and the official Raspberry Pi
7" Touch Display (800 x 480, the panel the test Pi runs).  Same world frame
as deck5: x across (left -> right as you stand at it), y away from you
(front edge y = 0), z up (floor z = 0).

The face comes from hardware/panel/layout.toml, the XDJ-1000MK2 face
measured off photos (good to a few mm, `status` per control) that the panel
PCB generator also reads, so plastic and copper agree.  That file is drawn
y-DOWN from the face's top-left (the back of the deck); here y = D - y_face.
A few controls move where the official display's wide glass or this deck's
own parts need it: every move is in OVERRIDES below, with its reason.

Shared code: deck5's jog.py (platter, tower, shaft) and caps.py (caps,
knobs, screen frame, fader bracket) are used unchanged.  They read their
dimensions with `from params import *`, and run from here that is THIS
file.  This module puts deck5 on the import path (after deck7) for them.

⚠ VERIFY marks a dimension taken from a datasheet or a guess, not a
caliper.  Measure the real part and change the number here.
"""
import os, sys, tomllib
from build123d import Location, Pos, Rot

HERE = os.path.dirname(os.path.abspath(__file__))
DECK5 = os.path.normpath(os.path.join(HERE, "..", "deck5"))
if DECK5 not in sys.path:
    sys.path.append(DECK5)      # after deck7: deck5's jog / caps, deck7's params and top_shell

NAME = "deck7"
LAYOUT_TOML = os.path.normpath(os.path.join(HERE, "..", "..", "panel", "layout.toml"))
with open(LAYOUT_TOML, "rb") as _f:
    LAYOUT = tomllib.load(_f)
_CTRL = {c["name"]: c for c in LAYOUT["control"]}


def place(solid, frm: Location, onto: Location):
    """Snap a part so its mate `frm` (part-local) coincides with world `onto`."""
    return (onto * frm.inverse()) * solid


# ── Printer ──────────────────────────────────────────────────────────────────
BED = (256.0, 256.0)          # Bambu X1 / P1 / A1: every printed part must fit

# ── Enclosure ────────────────────────────────────────────────────────────────
W, D = LAYOUT["face"]["width_mm"], LAYOUT["face"]["height_mm"]   # 305.7 x 387.5 (published depth 378.8: verify)
H = 50.0                      # flat top (the unit slopes)

def face(x, y_face):
    """layout.toml (y down from the back) -> deck (y up from the front)."""
    return (x, D - y_face)
R_PLAN = 10.0
WALL = 3.2
TOP_T = 3.0
EDGE_R = 4.0
BASE_T = 3.0
Z_UNDER = H - TOP_T

# The top prints as a 2 x 2 grid: one seam across the depth (Y_SEAM) and one
# along it (X_SEAM).  Both run under the platter, so the cross is hidden;
# they also miss every key, boss and the fader.  The base splits on its own
# grid (half-laps) so its pieces bridge the top seams and clamp them.
X_SEAM = 110.0                # top: between RELOOP and the jog's bearing boss
Y_SEAM = D - 158.0            # top: between the loop row and TRACK SEARCH / VINYL (face y 158)
X_BASE_SEAM = 90.0            # base: clear of the jog tower and the Pi
Y_BASE_SEAM = 200.0
BASE_LAP = 10.0

BAND_HALF = 8.0
BAND_T = 6.5
TONGUE = (2.2, 4.2)
TONGUE_L = 4.0
FIT = 0.15
TONGUE_Z0 = BASE_T + 4.0
REVEAL = (1.2, 0.8)

BOSS_D = 9.0
INSERT_D, INSERT_L = 4.6, 8.0
M3_CLR, M3_CSK = 3.4, 6.6

def base_screws():
    """Corners, mid front / back, both ends of each base lap, either side of
    each top seam on the walls, and one column at the base laps' cross
    (under the platter, where it ties the plate to the base)."""
    e = 7.5
    pts = [(e, e), (W - e, e), (e, D - e), (W - e, D - e), (W / 2, e), (W / 2, D - e)]
    pts += [(e, Y_BASE_SEAM), (W - e, Y_BASE_SEAM)]
    es = 5.5      # the top seam's wall bosses sit deeper in the wall: LOOP IN's flange is near
    for y in (Y_SEAM - 10.0, Y_SEAM + 10.0):
        pts += [(es, y), (W - es, y)]
    for x in (X_BASE_SEAM, X_SEAM - 10.0, X_SEAM + 10.0):
        pts += [(x, e), (x, D - e)]
    pts.append((X_BASE_SEAM, Y_BASE_SEAM))
    return pts

# ── Display: Raspberry Pi Touch Display (v1), 7", 800 x 480 ⚠ VERIFY ─────────
# Numbers from the published mechanical drawing (glass 192.96 x 110.76,
# active area 154.08 x 85.92, four M3 on the metal frame at 126.2 x 65.65).
# Caliper the glass, the glass-top to metal-back depth (SCR_T), the hole
# pitch, and where the active area sits on the glass (assumed centred).
# Mounted as deck5's: dropped in from the top, glass flush, onto a printed
# frame under the plate; screws come up through the frame into the
# display's own holes.  The adapter board and its standoffs hang below.
SCR_OUT = (192.96, 110.76)
SCR_AA = (154.08, 85.92)
SCR_T = 7.0                   # glass top to the metal frame's back ⚠
SCR_HOLES = (126.2, 65.65)    # M3 in the display's metal frame ⚠
SCR_BACK = (100.0, 62.0, 13.0)    # adapter board + standoffs under it: x, y, depth ⚠
_d = LAYOUT["display"]         # the XDJ panel's active area; the official 7" is centred on it
SCR_CX, SCR_CY = face(_d["x"] + _d["width"] / 2, _d["y"] + _d["height"] / 2)
SCR_FIT = 0.3
SCR_SINK = 0.0
SCR_Z_TOP = H - SCR_SINK
SCR_Z_BOT = SCR_Z_TOP - SCR_T
SCR_BOSS = (104.5, 40.0)      # frame bosses, past the glass's long edges
SKIN = 1.5
FRAME_T = 3.0
FRAME_BOSS_Z0 = SCR_Z_BOT - FRAME_T + 0.3
INSERT_DEPTH = 6.0
FRAME_RING = 9.0
M25_CLR = 3.4                 # caps.screen_frame's display-screw hole: M3 on the 7" (name from deck5)

# ── Raspberry Pi 5 + Active Cooler, under the screen at the back wall ───────
PI = (56.0, 85.0, 1.6)
PI_HOLES = (49.0, 58.0)
PI_INSET = 3.5
PI_STANDOFF = 6.0
PI_COOLER_H = 14.0            # ⚠ VERIFY
PI_PORT_H = 16.0
PI_PORT_OVER = 2.5
PI_X0 = 180.0                 # under the screen, ports out the back; clear of the base laps
# (layout.toml's [pi] bolts it to the panel PCB under the jog; this printed deck puts it on the base)
PI_Y0 = D - WALL - PI_PORT_OVER - 0.3 - PI[1]
PI_Z0 = BASE_T + PI_STANDOFF

def pi_holes():
    xs = (PI_X0 + PI_INSET, PI_X0 + PI_INSET + PI_HOLES[0])
    ys = (PI_Y0 + PI_INSET, PI_Y0 + PI_INSET + PI_HOLES[1])
    return [(x, y) for x in xs for y in ys]

# Back wall: panel USB-C (power, to the Pi) and exhaust vents.
USBC_X, USBC_Z = 60.0, 18.0
VENT_XS = [242.0 + i * 6.0 for i in range(8)]

# ── Jog: 200 mm, same reference shape as deck5's, same bearings and sensor ──
JOG_D = float(_CTRL["jog"]["platter_mm"])            # 206 ⚠ VERIFY
JOG_CX, JOG_CY = face(_CTRL["jog"]["x"], _CTRL["jog"]["y"])
JOG_H = 18.25                 # the reference platter at JOG_D (jog.py checks it)
JOG_GAP = 1.0
JOG_Z0 = H + JOG_GAP
SHAFT_D = 8.0
BRG = (22.0, 8.0, 7.0)
BRG_FIT = 0.1
TOWER = (42.0, 42.0, 20.0)    # deck5's tower: no longer used here (both bearings are in the top)
TOWER_SCREW = 16.0
SENSOR_SLOT = (25.0, 4.5)
SHAFT_Z0 = BASE_T + SENSOR_SLOT[1] + 0.5     # unchanged: the shaft and platter are printed
SHAFT_Z1 = JOG_Z0 + JOG_H - 4.0 - 2.0
MAGNET = (6.0, 2.5)           # ⚠ VERIFY
MAGNET_FIT = 0.2
SHAFT_PRINT_D = 7.9
GRUB_FLAT = 0.5

# Both 608s in one boss under the top plate, as on the freedj-esp mini deck.
# The upper one drops in from above (the platter hides the opening) onto a
# ledge; the lower one presses in from below up to a ledge.  The platter
# rests on the upper inner race through an M8 washer; a shaft collar under
# the lower inner race stops it lifting.  The tower on the base is not used.
WASHER_T = 1.6                # DIN 125 M8 washer (8.4 x 16 x 1.6) under the platter's hub
BRG_PROUD = 0.4               # the upper bearing stands this far above the top surface
BRG_UP_Z = H + BRG_PROUD - BRG[2]
assert abs(BRG_UP_Z + BRG[2] + WASHER_T - (JOG_Z0 + 1.0)) < 1e-6, "hub (JOG_Z0 + 1) must rest on the washer"
COLLAR = (18.0, 9.0)          # 8 mm shaft collar: OD, width ⚠ VERIFY
BRG_LO_Z = SHAFT_Z0 + 0.5 + COLLAR[1] + 1.0  # the collar fits between the shaft's end and the lower bearing
BRG_MID_D = 19.0              # between the seats: the ledges bear on the outer races only
JOG_BOSS_D = 32.0
assert BRG_UP_Z - (BRG_LO_Z + BRG[2]) > 10.0, "bearings too close"

# AS5600 board (WWZMDiB, 23.8 square, chip in the middle), hung from the
# boss in a cage: four corner posts locate it and stop it above, two spring
# fingers snap under its edges.  Everything sits outside the lower bearing's
# 22 mm path, so the bearing still goes in from below past the cage.
ENC_PCB = (23.8, 23.8, 1.6)
ENC_CHIP_H = 1.5
ENC_Z0 = BASE_T + 1.0         # board underside: 1 mm over the base
ENC_AIR = SHAFT_Z0 + 0.2 - (ENC_Z0 + ENC_PCB[2] + ENC_CHIP_H)   # magnet (0.2 recessed) to chip top
assert 0.5 <= ENC_AIR <= 3.0, ENC_AIR             # AS5600 working range
CAGE_CLR = 0.3                # board edge to the posts
CAGE_WALL = 2.0
CAGE_LEG = 4.5                # each corner post's two legs, along the board's edges
FINGER = (6.0, 1.6, 0.5)      # spring finger: width, thickness, lip under the board
assert ENC_PCB[0] / 2 - FINGER[2] > (BRG[0] + BRG_FIT) / 2 + 0.3, "lips in the lower bearing's path"

# Fader from the front: two countersunk M3 down through the top into the
# fader's own end-plate holes (120 mm centres), on pads as thick as the old
# bracket, so the fader and its knob sit where they did.
FADER_PAD_T = 2.5
FADER_SCREW = "M3 x 8 countersunk"

# ── Pitch fader, right of the jog ⚠ VERIFY (Alps RS60 / Bourns PTA) ─────────
FADER_TRAVEL = 100.0
FADER_X, FADER_CY = face(_CTRL["fader"]["x"], _CTRL["fader"]["y"])
FADER_BODY = (9.0, 128.0, 8.0)
FADER_PITCH = 120.0
FADER_SLOT_W = 3.0
FADER_LEVER = (1.6, 5.0, 12.0)
FADER_BRACKET_T = 2.5         # deck5's bracket (caps.fader_bracket): not used here any more
FADER_EAR = (15.0, 11.5, 6.0)
FADER_BOSS_Y = FADER_BODY[1] / 2 + 6.0
FADER_BOSS_H = 5.7
FADER_Z_TOP = Z_UNDER - FADER_PAD_T   # the fader's top, on its pads

# ── Controls: layout.toml, with the moves below ──────────────────────────────
# Moved, and why (face coordinates, y down, as in layout.toml):
OVERRIDES = {
    # The official 7" glass is 193 mm wide (the XDJ's own panel is narrower):
    "time_mode": (28.0, 50.0), "auto_cue": (28.0, 65.0), "shortcut": (28.0, 95.0),   # were x 60, under the glass
    "back": (272.0, 22.0), "tag_track": (290.0, 22.0),    # were (250.4 / 264.6, 42.6): the glass and the frame's ear
    "usb_stop": (282.0, 112.0),                           # was (235, 95), under the glass
    # SYNC / MASTER were guesses at (110 / 135, 143): under the 206 mm platter's rim.
    "sync": (258.0, 143.0), "master": (282.0, 143.0),
    # MASTER TEMPO touched the fader bracket's ear; TEMPO RANGE (a guess) moves with it.
    "master_tempo": (282.8, 214.0), "tempo_range": (282.8, 198.0),
    # The search pairs (guesses) were 18 mm apart; their caps' flanges need 18.5.
    "track_search_fwd": (41.0, 190.0), "search_fwd": (41.0, 205.0),
}
ENC_FACE = (280.0, 70.0)      # browse encoder, was (258.3, 79.4): its body cleared the frame's ear
NOT_BUILT = {"jog_touch"}     # the top-plate switch lives in the jog (not designed yet)


def _cap(c):
    v = c["cap_mm"]
    return (float(v), float(v)) if not isinstance(v, list) else (float(v[0]), float(v[1]))


PLAY = face(_CTRL["play"]["x"], _CTRL["play"]["y"])
CUE = face(_CTRL["cue"]["x"], _CTRL["cue"]["y"])
BIG_CAP_D = 30.0              # layout.toml says 35: a 35 mm PLAY would cut into the edge fillet
CAP_CLR = 0.5
KEY = (14.0, 10.0)            # caps.key_cap's default; each key below has its own size
GUIDE_L = 8.0
GUIDE_WALL = 2.0

KEYS = []                     # (x, y, name, (w, h)) in deck coordinates
for _c in LAYOUT["control"]:
    if _c["kind"] != "button" or _c["name"] in ("play", "cue") or _c["name"] in NOT_BUILT:
        continue
    _x, _y = OVERRIDES.get(_c["name"], (_c["x"], _c["y"]))
    KEYS.append((*face(_x, _y), _c["name"], _cap(_c)))

ENC = face(*ENC_FACE)
ENC_HOLE = 7.2
