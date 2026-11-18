"""params.py — the full-size deck (ADK-1000): single source of truth.

A CDJ-3000-sized deck (329 x 453 mm) around a 200 mm jog and the official
Raspberry Pi 7" Touch Display (800 x 480, the panel the test Pi runs).
Same world frame as deck5: x across (left -> right as you stand at it), y
away from you (front edge y = 0), z up (floor z = 0).

Shared code: deck5's jog.py (platter, tower, shaft) and caps.py (caps,
knobs, screen frame, fader bracket) are used unchanged.  They read their
dimensions with `from params import *`, and run from here that is THIS
file, so the same code makes the 200 mm platter and the 7" frame.  This
module puts deck5 on the import path (after deck7) for them.

⚠ VERIFY marks a dimension taken from a datasheet or a guess, not a
caliper.  Measure the real part and change the number here.
"""
import os, sys
from build123d import Location, Pos, Rot

HERE = os.path.dirname(os.path.abspath(__file__))
DECK5 = os.path.normpath(os.path.join(HERE, "..", "deck5"))
if DECK5 not in sys.path:
    sys.path.append(DECK5)      # after deck7: deck5's jog / caps, deck7's params and top_shell

NAME = "deck7"


def place(solid, frm: Location, onto: Location):
    """Snap a part so its mate `frm` (part-local) coincides with world `onto`."""
    return (onto * frm.inverse()) * solid


# ── Printer ──────────────────────────────────────────────────────────────────
BED = (256.0, 256.0)          # Bambu X1 / P1 / A1: every printed part must fit

# ── Enclosure ────────────────────────────────────────────────────────────────
W, D, H = 329.0, 453.0, 50.0  # CDJ-3000 footprint; flat top (the unit slopes)
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
X_SEAM = 104.5                # top: between hot cues A-B and C (x 89.5 / 119.5)
Y_SEAM = 252.0                # top: past the fader's bracket, under the platter
X_BASE_SEAM = 220.0           # base: clear of the jog tower and the Pi
Y_BASE_SEAM = 205.0
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
    for y in (Y_BASE_SEAM, Y_SEAM - 10.0, Y_SEAM + 10.0):
        pts += [(e, y), (W - e, y)]
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
SCR_CX, SCR_CY = W / 2, 362.0
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
PI_X0 = 240.0                 # right of the base's x lap
PI_Y0 = D - WALL - PI_PORT_OVER - 0.3 - PI[1]
PI_Z0 = BASE_T + PI_STANDOFF

def pi_holes():
    xs = (PI_X0 + PI_INSET, PI_X0 + PI_INSET + PI_HOLES[0])
    ys = (PI_Y0 + PI_INSET, PI_Y0 + PI_INSET + PI_HOLES[1])
    return [(x, y) for x in xs for y in ys]

# Back wall: panel USB-C (power, to the Pi) and exhaust vents.
USBC_X, USBC_Z = 60.0, 18.0
VENT_XS = [130.0 + i * 6.0 for i in range(12)]

# ── Jog: 200 mm, same reference shape as deck5's, same bearings and sensor ──
JOG_D = 200.0
JOG_CX, JOG_CY = W / 2, 165.0
JOG_H = 17.71                 # the reference platter at JOG_D (jog.py checks it)
JOG_GAP = 1.0
JOG_Z0 = H + JOG_GAP
SHAFT_D = 8.0
BRG = (22.0, 8.0, 7.0)
BRG_FIT = 0.1
JOG_BOSS_D = 30.0
BRG_UP_Z = Z_UNDER - 2.0 - BRG[2]
TOWER = (42.0, 42.0, 20.0)
TOWER_SCREW = 16.0
BRG_LO_Z = BASE_T + TOWER[2] - BRG[2]
SENSOR_SLOT = (25.0, 4.5)
SHAFT_Z0 = BASE_T + SENSOR_SLOT[1] + 0.5
SHAFT_Z1 = JOG_Z0 + JOG_H - 4.0 - 2.0
MAGNET = (6.0, 2.5)           # ⚠ VERIFY
MAGNET_FIT = 0.2
SHAFT_PRINT_D = 7.9
GRUB_FLAT = 0.5

# ── Pitch fader, right of the jog ⚠ VERIFY (Alps RS60 / Bourns PTA) ─────────
FADER_TRAVEL = 100.0
FADER_X, FADER_CY = 300.0, JOG_CY
FADER_BODY = (9.0, 128.0, 8.0)
FADER_PITCH = 120.0
FADER_SLOT_W = 3.0
FADER_LEVER = (1.6, 5.0, 12.0)
FADER_BRACKET_T = 2.5
FADER_EAR = (15.0, 11.5, 6.0)
FADER_BOSS_Y = FADER_BODY[1] / 2 + 6.0
FADER_BOSS_H = 5.7
FADER_Z_TOP = Z_UNDER - FADER_BRACKET_T

# ── Controls (CDJ-3000 layout) ───────────────────────────────────────────────
PLAY = (36.0, 40.0)
CUE = (36.0, 82.0)
BIG_CAP_D = 26.0
CAP_CLR = 0.5
KEY = (20.0, 12.0)            # one key size for every key (caps.key_cap)
GUIDE_L = 8.0
GUIDE_WALL = 2.0

_ROW = [JOG_CX + (i - 3.5) * 30.0 for i in range(8)]   # 59.5 … 269.5
KEYS = (
    # Above the screen, as on the unit and the app's top layout.
    [(x, 432.0, n) for x, n in zip(_ROW, ("SOURCE", "BROWSE", "TAG LIST", "PLAYLIST",
                                          "SEARCH", "MENU", "BACK", "TAG TRACK"))]
    # Hot cues A-H between the jog and the screen.
    + [(x, 290.0, f"HOT CUE {c}") for x, c in zip(_ROW, "ABCDEFGH")]
    # Left of the jog, over CUE: loops and track search.
    + [(36.0, y, n) for y, n in ((130.0, "LOOP IN"), (152.0, "LOOP OUT"), (174.0, "RELOOP / EXIT"),
                                 (205.0, "TRACK |<<"), (227.0, "TRACK >>|"))]
    # Right of the jog: tempo keys under the fader, SYNC / MASTER over it.
    + [(300.0, y, n) for y, n in ((50.0, "MASTER TEMPO"), (72.0, "TEMPO RANGE"),
                                  (272.0, "MASTER"), (296.0, "BEAT SYNC"))]
)
ENC = (300.0, 362.0)          # browse encoder (EC11, push), right of the screen
ENC_HOLE = 7.2
