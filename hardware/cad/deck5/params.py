"""params.py — the 5" deck (ADK-1000 small): single source of truth.

Every dimension shared between parts or used by the assembly lives here.
World frame: x across the deck (left → right as you stand at it), y away
from you (front edge y = 0, back y = D), z up (floor z = 0).  Parts are
modelled in this frame directly (they are all panels of one box), so the
assembly places most of them at the identity; the moving / separate parts
(jog platter, tower, caps) carry MATES.

⚠ VERIFY marks a dimension taken from a datasheet or a guess, not a
caliper.  Measure the real part and change the number here; everything
else follows.
"""
from build123d import Location, Pos, Rot


def place(solid, frm: Location, onto: Location):
    """Snap a part so its mate `frm` (part-local) coincides with world `onto`."""
    return (onto * frm.inverse()) * solid


# ── Printer ──────────────────────────────────────────────────────────────────
BED = (220.0, 220.0)          # every printed part must fit this (x, y) footprint

# ── Enclosure ────────────────────────────────────────────────────────────────
W, D, H = 214.0, 280.0, 46.0  # outside: width, depth, height (top surface z = H)
R_PLAN = 10.0                 # plan-view corner radius
WALL = 3.2                    # side walls (4 perimeters at 0.4 + a skin)
TOP_T = 3.0                   # top plate
EDGE_R = 4.0                  # fillet round the top edge
BASE_T = 3.0                  # base plate (the shell's walls start on it)
Z_UNDER = H - TOP_T           # underside of the top plate

# The top shell prints in two pieces split across the depth, between the
# control panel (front) and the screen panel (back).  The base splits too,
# somewhere else, so the back base half bridges the top seam and its
# screws clamp both top halves together.
Y_SEAM = 184.0                # top shell split (y)
Y_BASE_SEAM = 120.0           # base split (y)
BASE_LAP = 10.0               # base half-lap length across its seam

# Seam joint: a thickened band each side of the seam, a tongue on the back
# piece and a groove in the front, and a shallow reveal line outside so the
# joint reads as a designed line, not a crack.
BAND_HALF = 8.0               # band reaches this far either side of the seam
BAND_T = 6.5                  # band thickness, from the outside surface in
TONGUE = (2.2, 4.2)           # tongue spans these insets from the outside surface
TONGUE_L = 4.0                # how far the tongue reaches into the front piece
FIT = 0.15                    # groove clearance per side (CA glue line)
TONGUE_Z0 = BASE_T + 4.0      # tongue legs stop above the base
REVEAL = (1.2, 0.8)           # reveal channel on the outside: width, depth

# Base screws: M3 heat-set inserts in shell bosses, countersunk M3 from below.
BOSS_D = 9.0
INSERT_D, INSERT_L = 4.6, 8.0
M3_CLR, M3_CSK = 3.4, 6.6     # clearance, countersink head diameter

def base_screws():
    """Boss / screw positions: corners, the front middle, both seams."""
    e = 7.5
    pts = [(e, e), (W - e, e), (e, D - e), (W - e, D - e), (W / 2, e)]
    for y in (Y_BASE_SEAM, Y_SEAM - 10.0, Y_SEAM + 10.0):
        pts += [(e, y), (W - e, y)]
    return pts

# ── Display: Hosyond 5" DSI, 800 x 480 IPS, capacitive ⚠ VERIFY all ──────────
# 800 x 480 is the panel the deck's fill path already runs at (the 7"
# official display is the same resolution), so the UI needs no new layout.
# No Hosyond drawing found: these are the numbers the common 5" DSI boards
# share (outline 120.7 x 75.8 x 4.9, M2.5 panel holes 113 x 68).  Caliper
# the outline, the thickness at the inserts, and the insert pitch.
#
# Mount: the display drops into an opening in the top plate from above,
# glass flush with the top surface, and sits on a printed frame under the
# plate; M2.5 screws come up through the frame into the threaded inserts on
# the display's back, and the frame screws to four bosses under the plate.
SCR_OUT = (120.7, 75.8)       # outline (glass and board)
SCR_AA = (108.0, 64.8)        # active area (assumed centred: VERIFY)
SCR_T = 4.9                   # glass top to the back face the inserts are in
SCR_HOLES = (113.0, 68.0)     # threaded inserts on the back: pitch (x, y), M2.5
SCR_CX, SCR_CY = W / 2, 236.0 # centre of the display
SCR_FIT = 0.3                 # opening clearance per side
SCR_SINK = 0.0                # glass this far below the top surface (0 = flush)
SCR_Z_TOP = H - SCR_SINK                  # glass top
SCR_Z_BOT = SCR_Z_TOP - SCR_T             # display back = frame top
SCR_BOSS = (66.0, 28.0)       # frame bosses at (cx ± dx, cy ± dy)
# The frame sits only SCR_T below the top surface, too shallow for an insert
# above it, so its bosses hang down through clearance holes in the frame's
# ears and an M3 + washer from below pulls the frame up: blind bores, no
# screw shows on top.
SKIN = 1.5                    # least plastic left over any blind bore, top surface
FRAME_T = 3.0                 # screen frame thickness
FRAME_BOSS_Z0 = SCR_Z_BOT - FRAME_T + 0.3  # boss bottom (0.3 inside the ear, so the washer clamps the ear)
INSERT_DEPTH = 6.0            # an M3 insert's bore (5.7 long)
FRAME_RING = 9.0              # frame ring width under the display's edges
M25_CLR = 2.8

# ── Raspberry Pi 5 + Active Cooler ───────────────────────────────────────────
PI = (56.0, 85.0, 1.6)        # board (x, y, t): long side runs front→back
PI_HOLES = (49.0, 58.0)       # hole pitch (x, y), 3.5 in from the board edges
PI_INSET = 3.5
PI_STANDOFF = 6.0
PI_COOLER_H = 14.0            # Active Cooler above the board ⚠ VERIFY
PI_PORT_H = 16.0              # USB stack above the board
PI_PORT_OVER = 2.5            # ports overhang the board edge
PI_X0 = W / 2 - PI[0] / 2
PI_Y0 = D - WALL - PI_PORT_OVER - 0.3 - PI[1]   # USB / Ethernet end at the back wall
PI_Z0 = BASE_T + PI_STANDOFF                    # board underside

def pi_holes():
    xs = (PI_X0 + PI_INSET, PI_X0 + PI_INSET + PI_HOLES[0])
    ys = (PI_Y0 + PI_INSET, PI_Y0 + PI_INSET + PI_HOLES[1])   # the non-port end first
    return [(x, y) for x in xs for y in ys]

# ── Jog ──────────────────────────────────────────────────────────────────────
JOG_D = 140.0
JOG_CX, JOG_CY = 110.0, 82.0
JOG_H = 12.40                 # the reference platter at JOG_D (jog.py checks it)
JOG_GAP = 1.0                 # platter skirt clears the top by this
JOG_Z0 = H + JOG_GAP          # platter skirt bottom
SHAFT_D = 8.0                 # 8 mm steel rod (or ground M8 bolt shank)
BRG = (22.0, 8.0, 7.0)        # 608 bearing: OD, ID, width
BRG_FIT = 0.1
JOG_BOSS_D = 30.0
BRG_UP_Z = Z_UNDER - 2.0 - BRG[2]         # upper bearing bottom (2 mm shoulder over it)
TOWER = (42.0, 42.0, 20.0)    # jog tower on the base: x, y, height
TOWER_SCREW = 16.0            # tower screws at (±, ±) from the jog centre
BRG_LO_Z = BASE_T + TOWER[2] - BRG[2]     # lower bearing bottom
SENSOR_SLOT = (25.0, 4.5)     # AS5600 board slot under the shaft: width, height
SHAFT_Z0 = BASE_T + SENSOR_SLOT[1] + 0.5  # shaft bottom (diametric magnet in its end)
SHAFT_Z1 = JOG_Z0 + JOG_H - 4.0 - 2.0     # top, 2 mm under the platter's top face
MAGNET = (6.0, 2.5)           # AS5600 diametric magnet: diameter, thickness ⚠ VERIFY (kits ship 6 x 2.5)
MAGNET_FIT = 0.2              # pocket clearance on the diameter (glued in)
SHAFT_PRINT_D = 7.9           # printed test shaft: a hair under 8 so it slides into the 608s
GRUB_FLAT = 0.5               # flat the platter's grub screw bears on (printed shaft only)

# ── Pitch fader: 100 mm travel slide pot ⚠ VERIFY (Alps RS60 / Bourns PTA) ──
FADER_TRAVEL = 100.0
FADER_X, FADER_CY = 194.0, JOG_CY
FADER_BODY = (9.0, 128.0, 8.0)
FADER_PITCH = 120.0           # M3 mounting holes in the end plates
FADER_SLOT_W = 3.0
FADER_LEVER = (1.6, 5.0, 12.0)    # lever cross-section x, y, height above body ⚠
# Hidden mount: the fader screws to a printed bracket (countersunk M3 down
# into its end plates), whose two ears slip over bosses under the top plate
# and are pulled up by M3 + washer from below.  No screw shows on top.
FADER_BRACKET_T = 2.5         # bracket plate between the fader and the top plate
FADER_EAR = (15.0, 11.5, 6.0) # ear blocks past each end: width (x), length (y), depth (z)
FADER_BOSS_Y = FADER_BODY[1] / 2 + 6.0    # boss centres at FADER_CY ± this
FADER_BOSS_H = 5.7            # boss height under the plate (one M3 insert deep)
FADER_Z_TOP = Z_UNDER - FADER_BRACKET_T   # top of the fader body

# ── Controls on the top ──────────────────────────────────────────────────────
PLAY = (21.0, 32.0)
CUE = (21.0, 70.0)
BIG_CAP_D = 26.0
CAP_CLR = 0.5                 # hole is this much bigger than its cap (diameter / side)
KEY = (14.0, 10.0)
KEY_Y = 168.0
KEY_XS = (24.0, 48.0, 72.0, 96.0, 120.0, 144.0)   # BROWSE TAG MENU BACK LOOP SYNC
ENC = (186.0, KEY_Y)          # browse encoder (EC11)
ENC_HOLE = 7.2
GUIDE_L = 8.0                 # cap guide sleeves under the top
GUIDE_WALL = 2.0
