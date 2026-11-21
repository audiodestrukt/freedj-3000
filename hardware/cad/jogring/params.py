"""params.py — the ring-jog test rig: every dimension, one place.

The rig is the 206 mm jog of docs/design/jog-ring-rollers.md on its own
base plate: a ring platter on three V-wheels, a fixed hub holding a round
smart-watch display (1.28" to 2.1"), and a sprung roller that turns the
AS5600.  It answers three questions before deck7 changes: is the ring
smooth and free of play, does the roller slip, and how does it feel.

Frame: the jog's axis is z, the base plate's underside is z = 0, mm.
Bought parts marked ⚠ are datasheet or catalogue numbers: caliper them
before printing.
"""
import math, os

HERE = os.path.dirname(os.path.abspath(__file__))
BED = (256.0, 256.0)
OVS = 1.0                     # boolean overshoot

# ── Base plate ──
PLATE_D = 210.0
PLATE_T = 6.0
PLATE_TOP = PLATE_T
CABLE_HOLE_D = 40.0           # the display's cable leaves through the middle

# ── V-wheels (OpenBuilds-style "Solid V wheel" on two 625s) ⚠ VERIFY ──
WHEEL_OD = 23.9               # over the V tip
WHEEL_W = 10.23
WHEEL_TIP_W = 1.0             # flat at the tip of the V
WHEEL_V_H = 2.5               # radial height of the V (45° flanks)
WHEEL_BORE = 5.0
WHEEL_R_TIP = WHEEL_OD / 2
WHEEL_R_CORE = WHEEL_R_TIP - WHEEL_V_H
SPACER = (10.0, 6.0)          # aluminium spacer under a fixed wheel: OD, length ⚠
ECC = (11.0, 6.35, 7.2, 0.79)  # eccentric spacer: head across flats (as a disc), head height, shank hole, offset ⚠
M5_CLR = 5.3
M5_NUT_AF, M5_NUT_T = 8.0, 4.0
M5_HEAD = (8.5, 5.0)          # socket head: diameter, height
M5_WASHER = (10.0, 1.0)

# ── Or: 608 "skateboard" bearings with a printed V tyre (the 608 variant) ──
# The tyre's V is the V-wheel's V (same tip, flanks, height and centre
# height), so one ring fits both variants: the groove depends only on the V,
# not on the wheel's diameter.
B608 = (22.0, 8.0, 7.0)       # OD, ID, width
TYRE_BORE = 21.95             # press fit on the 608 ⚠ print tyre_fit.stl first and change this
TYRE_W = 8.0
TYRE_WALL = 2.0               # under the V
TYRE_R_CORE = B608[0] / 2 + TYRE_WALL
TYRE_R_TIP = TYRE_R_CORE + WHEEL_V_H          # 15.5: Ø31 over the V
TYRE_LIP = (20.4, 0.5)        # a lip under the bearing's outer race: bore, thickness
RACE_D = 11.0                 # printed spacers touch the inner race only (608 inner race ~Ø12)
M8_CLR = 8.3
M8_HEAD = (14.6, 5.3, 15.5)   # hex head: across corners, height, counterbore diameter
M8_JAM = (14.6, 5.0)          # thin nut: across corners, height
CBORE = 2.5                   # head counterbore under the plate
SLIDE = 1.5                   # the 270° 608 slides ± this in a radial slot to set the preload

WHEEL_Z0 = 14.0               # bottom face of every wheel
WHEEL_ZC = WHEEL_Z0 + WHEEL_W / 2
WHEEL_ANGLES = (30.0, 150.0, 270.0)   # the 270° wheel rides on the eccentric spacer
ECC_WHEEL = 270.0

# ── Ring (the platter) ──
JOG_D = 206.0
SKIRT_R = 70.0                # the skirt's outer face, where the V groove is
SKIRT_WALL = 7.0
SKIRT_RI = SKIRT_R - SKIRT_WALL   # 63: the sense roller runs on this face
SKIRT_Z0 = 13.0
V_PEN = 2.0                   # how far the wheel's V sits into the groove
V_TIP_CLR = 0.5               # past the V's tip, at the groove's root
V_FLANK_CLR = 0.05            # on each flank (the eccentric takes up the rest)
WHEEL_R = SKIRT_R + WHEEL_R_TIP - V_PEN        # wheel axes: r = 79.95
TYRE_R = SKIRT_R + TYRE_R_TIP - V_PEN          # 608 axes: r = 83.5
TYRE_Z0 = WHEEL_ZC - TYRE_W / 2
B608_Z0 = TYRE_Z0 + TYRE_LIP[1]
SPACER608_Z1 = B608_Z0                          # the printed spacer's step meets the inner race here
NUT_Z0 = B608_Z0 + B608[2] + 1.0                # 1 mm race spacer, then the jam nut
TOP_Z0 = 31.5                 # underside of the ring's top plate
TOP_T = 4.0
TOP_Z1 = TOP_Z0 + TOP_T       # the jog's top surface, z = 35.5
RIM_W, RIM_DROP = 3.0, 4.0    # a lip down the outside edge, for looks
# Under the top, r 74..100 stays flat and clear (no ribs, no screws): room
# for an aluminium or copper sheet and fixed magnets, if eddy-current drag
# is tried for the feel.
EDDY_BAND = (74.0, 100.0)

# ── Hub (fixed): the display window ──
# Sized for the largest display (2.1"); a second top takes the 1.28" module.
DISPLAYS = {
    # name: glass/module diameter, active-area window, module thickness  ⚠ VERIFY every one
    "210": (61.0, 54.0, 4.0),     # 2.1" round, 480 x 480 (ST7701-class)
    "128": (39.0, 32.6, 4.0),     # 1.28" round, 240 x 240 GC9A01 module (round PCB)
}
HUB_WALL = 2.5
HUB_FIT = 0.2
HUB_OD = DISPLAYS["210"][0] + 2 * HUB_FIT + 2 * HUB_WALL   # 66.4
HUB_GAP = 1.5                 # ring bore to hub: the only seam on the top
RING_BORE = HUB_OD + 2 * HUB_GAP
LIP_T = 1.5                   # the bezel lip over the glass
HUB_FLANGE = (78.0, 3.0)
HUB_SCREW_R = 36.2            # 3 x M3 down through the flange into the plate
M3_CLR, M3_HEAD = 3.4, (5.5, 3.0)
INSERT = (4.0, 5.7)           # M3 heat-set insert: bore, depth

# ── Sense roller: an O-ring tyre on the skirt's inner face ──
ROLL_ANGLE = 90.0             # where it sits (deg); the wheels are at 30/150/270
ROLL_TYRE_D = 18.0            # O-ring 14 x 2 on a 14 mm root ⚠
ROLL_ROOT_D = 14.0
ORING_CS = 2.0
ROLL_R = SKIRT_RI - ROLL_TYRE_D / 2 - 0.05       # axle radius: tyre just touching, 53.95
RATIO = SKIRT_RI / (ROLL_TYRE_D / 2)            # roller turns per platter turn, about 7
AXLE_D = 5.0                  # steel rod ⚠ length below
BRG = (10.0, 5.0, 4.0)        # MR105ZZ: OD, ID, width ⚠
BRG_FIT = 0.1
MAGNET = (6.0, 2.5)           # diametric, for the AS5600 ⚠
ENC_PCB = (23.8, 23.8, 1.6)   # the AS5600 board ⚠ (as deck7)
ENC_PARTS_H = 2.0             # its components, chip side
CHIP_H = 1.5

# Arm z stack (the arm floats 1 mm over the plate on a pad under its foot)
ARM_Z0 = PLATE_TOP + 1.0      # 7: the AS5600 board's underside
CHIP_TOP = ARM_Z0 + ENC_PCB[2] + CHIP_H         # 10.1
MAG_Z0 = CHIP_TOP + 1.0                         # 1 mm air to the magnet
CUP = (8.0, 4.0)                                # magnet cup on the axle's end: OD, height
BRG1_Z0 = MAG_Z0 + CUP[1] + 0.5                 # 15.6
BOSS_Z1 = BRG1_Z0 + 2 * BRG[2]                  # 23.6: two bearings stacked
BOSS_D = BRG[0] + 4.0
ROLLER_Z0 = BOSS_Z1 + 0.5
ROLLER_HUB = (10.0, 2.0)                        # grub-screw hub under the tyre: OD, height
ROLLER_TYRE_H = 4.0
ROLLER_Z1 = ROLLER_Z0 + ROLLER_HUB[1] + ROLLER_TYRE_H   # 30.1
TYRE_ZC = ROLLER_Z1 - ROLLER_TYRE_H / 2
AXLE_Z0 = MAG_Z0 + MAGNET[1] + 0.5              # the rod stops on a ledge over the magnet
AXLE_Z1 = ROLLER_Z1 - 0.5
AXLE_L = AXLE_Z1 - AXLE_Z0

# Flexure: one beam, tangential, bending radially.  The foot's slots let it
# slide outward 0..PRELOAD_MAX; the beam's spring then presses the tyre on.
BEAM_T, BEAM_Z = 1.6, (12.0, 17.0)
BEAM_Y = ROLL_R - 6.15        # along the boss's inner side
FOOT = (-44.0, -24.0, 34.0, 50.0)      # x0, x1, y0, y1 (rig frame at ROLL_ANGLE = 90)
FOOT_Z1 = 11.5
RISER = (-30.0, -24.0)        # where the beam leaves the foot, full height
FOOT_SCREWS = ((-40.0, 39.0), (-35.0, 44.5))   # in the plate; the slots run inward from them
PRELOAD_MAX = 3.0
E_PETG = 2000.0               # MPa
_L = abs(RISER[1]) - math.sqrt((BOSS_D / 2) ** 2 - (ROLL_R - BEAM_Y) ** 2)
BEAM_K = 3 * E_PETG * ((BEAM_Z[1] - BEAM_Z[0]) * BEAM_T ** 3 / 12) / _L ** 3   # N/mm

# ── Checks on the numbers ──
assert WHEEL_R + WHEEL_R_TIP < JOG_D / 2 - RIM_W - 4, "wheels show past the platter"
assert TYRE_R + TYRE_R_TIP < JOG_D / 2 - RIM_W - 0.5, "608 tyres hit the rim"
assert NUT_Z0 + M8_JAM[1] < TOP_Z0 - 1.0, "M8 nuts hit the top"
assert WHEEL_Z0 + WHEEL_W + M5_WASHER[1] + M5_HEAD[1] < TOP_Z0 - 1.0, "bolt heads hit the top"
assert ROLL_R + BOSS_D / 2 < SKIRT_RI - 1.0, "the roller's boss rubs the skirt"
assert SKIRT_Z0 < TYRE_ZC - ROLLER_TYRE_H / 2 and ROLLER_Z1 < TOP_Z0 - 1.0, "tyre off the skirt"
assert 0.5 <= MAG_Z0 - CHIP_TOP <= 3.0, "AS5600 air gap"
assert HUB_OD / 2 < SKIRT_RI - ROLL_TYRE_D - 2, "no room for the roller"
for _x, _y in FOOT_SCREWS:
    assert math.hypot(_x, _y + PRELOAD_MAX) + M3_HEAD[0] / 2 < SKIRT_RI - 0.5, "a foot screw's head under the skirt"
assert math.hypot(RISER[0], FOOT[3] + PRELOAD_MAX) < SKIRT_RI - 0.5, "the riser hits the skirt at full preload"
assert BED[0] >= JOG_D and BED[0] >= PLATE_D
