"""pioneer.py — the "pioneer" version of the rig: the layout of a Pioneer jog.

Dan took a Pioneer jog apart (2026-11-20):
- the platter is one ring carried on two concentric sets of small plastic
  wheels on pins, about 10 mm across, axles horizontal;
- a little ridge round its foot sits under the deck's own top, which stops
  it lifting; the whole assembly fastens from inside the deck;
- the angle comes from a gear the ring turns, not a friction roller.

Here:
- 24 rollers, 10 × 4: either 623 bearings (10 × 3 × 4) or printed wheels
  (TPU or PETG), on 3 mm pins that drop into slots open at the top;
- 12 rollers at r 70 and 12 at r 94, each set in its own shallow groove in
  the ring's underside, so the rollers' sides centre it;
- the ridge carries the gear teeth (m 1, 214 teeth, 45° shroud above them),
  under a "deck top" ring that stands in for the deck's body;
- a 20-tooth gear on the AS5600 arm's axle, sprung into mesh by the flexure,
  so the mesh has no backlash: 10.7 turns per platter turn.

The rig's plate is square (250) so the arm's board, out at r 117, stays on
it; it sits at 45°.
"""
import math
from build123d import *
from params import *
import rig
from rig import cyl, box, tube, revolve_rz, polar

# ── rollers ──
W_D, W_W = 10.0, 4.0              # 623ZZ: 10 × 3 × 4, or a printed wheel the same size
W_BORE = 3.2                      # printed wheel on a 3 mm pin
PIN3 = (3.0, 10.0)                # 3 mm steel pin ⚠ (or an M3 screw's shank)
R_IN, R_OUT = 70.0, 94.0
N_IN, N_OUT = 12, 12
A_IN = [15.0 + 30.0 * k for k in range(N_IN)]
A_OUT = [30.0 * k for k in range(N_OUT)]
GROOVE = (W_W + 0.3, 0.6)         # the rollers' grooves in the ring: width, depth
STATION_T = 3.0                   # each side wall of a roller's block
SIDE_GAP = 0.3
BOSS5 = (5.0, SIDE_GAP)           # Ø5 bosses on the walls bear on a 623's inner race only

# ── gear: on the ridge round the ring's foot, and on the arm's axle ──
M = 1.0
Z_RING, Z_PIN = 214, 20
RP_RING, RP_PIN = M * Z_RING / 2, M * Z_PIN / 2        # 107, 10
BACKLASH = 0.1                    # removed from each tooth's thickness (the spring takes up the rest)
SENSE_R = RP_RING + RP_PIN        # 117: the arm's axle
SENSE_ANGLE = 45.0
DY = SENSE_R - ROLL_R             # the shared arm is drawn at ROLL_R on +y; moved out this far

# ── heights ──
FL_Z0 = ROLLER_Z0 + ROLLER_HUB[1]           # 26.1: the ring's foot = the gear band's bottom
FL_Z1 = FL_Z0 + ROLLER_TYRE_H               # 30.1: 4 mm of teeth, the same height as the arm's gear
RC = FL_Z0 + GROOVE[1] - 0.05 - W_D / 2     # roller axles: their tops 0.05 under the grooves' roofs
SHROUD = 5.0                      # 45° shroud over the teeth, from r 108 in to the wall
RET_Z0 = FL_Z1 + 0.3              # the "deck top": 0.3 over the ridge (the lift stop)
RET_T = 3.0
RET_Z1 = RET_Z0 + RET_T
RET_R1 = 124.0
POSTS = (15.0, 105.0, 165.0, 225.0, 285.0, 345.0)
POST_R, POST_D = 118.0, 10.0
P_TOP_Z1 = RET_Z1 + 11.6          # the jog stands 11.6 proud of the deck top
P_TOP_Z0 = P_TOP_Z1 - TOP_T
WALL = (99.0, 103.0)              # outer wall
IN_WALL = (62.0, 65.0)            # inner wall, over the inner rollers' inside
PLATE_SQ = 250.0

assert RC - W_D / 2 > PLATE_TOP + 1.0
assert RC + PIN3[0] / 2 + 1.5 < FL_Z0 - 1.0, "roller blocks touch the ring"
assert RP_RING + M < POST_R - POST_D / 2 - 2
assert P_TOP_Z0 > RET_Z1 + 1


def involute_pts(z, m, backlash=0.0, steps=8):
    """An external involute gear's outline (20°), as (x, y) points."""
    pa = math.radians(20)
    rp, rb, ra, rf = m * z / 2, m * z / 2 * math.cos(pa), m * z / 2 + m, m * z / 2 - 1.25 * m
    inv = lambda a: math.tan(a) - a
    half_p = math.pi / (2 * z) - backlash / 2 / rp + inv(pa)      # half the tooth's angle at the base circle
    r0 = max(rb, rf)
    rs = [r0 + (ra - r0) * i / steps for i in range(steps + 1)]
    pts = []
    for k in range(z):
        c = 2 * math.pi * k / z
        flank = [(r, half_p - inv(math.acos(min(1.0, rb / r)))) for r in rs]
        side = ([(rf, half_p)] if rf < rb else []) + flank
        for r, h in side:                       # leading flank, root to tip
            pts.append((r * math.cos(c - h), r * math.sin(c - h)))
        for r, h in reversed(side):             # trailing flank, tip to root
            pts.append((r * math.cos(c + h), r * math.sin(c + h)))
        mid = c + math.pi / z                   # root, between teeth
        pts.append((rf * math.cos(mid), rf * math.sin(mid)))
    return pts


def gear(z, m, z0, z1, backlash=BACKLASH):
    return Pos(0, 0, z0) * extrude(make_face(Polyline(*involute_pts(z, m, backlash), close=True)), z1 - z0)


def ring():
    r_out = JOG_D / 2
    s = tube(RING_BORE / 2, r_out, P_TOP_Z0, P_TOP_Z1)
    s += tube(r_out - RIM_W, r_out, P_TOP_Z0 - RIM_DROP, P_TOP_Z0 + 0.5)
    s += tube(*WALL, FL_Z1 - 0.5, P_TOP_Z0 + 0.5)
    s += tube(*IN_WALL, FL_Z0, P_TOP_Z0 + 0.5)
    # The foot: a band over each roller set (open between them, so the ring
    # has no sealed void), with the gear round its edge.
    s += tube(IN_WALL[0], R_IN + 7.0, FL_Z0, FL_Z1)
    s += tube(R_OUT - 7.0, WALL[0] + 1, FL_Z0, FL_Z1)
    # A gap of the teeth faces the arm's gear (SENSE_ANGLE), so the two mesh.
    s += Rot(0, 0, SENSE_ANGLE - 180.0 / Z_RING) * gear(Z_RING, M, FL_Z0, FL_Z1) - cyl(0, 0, FL_Z0 - OVS, FL_Z1 + OVS, 2 * (WALL[0] + 0.5))
    # 45° shroud over the teeth: prints without support, and keeps the gear in line.
    ra = RP_RING + M
    s += revolve_rz([(WALL[0], FL_Z1 - 0.01), (ra, FL_Z1 - 0.01), (ra - SHROUD, FL_Z1 + SHROUD), (WALL[0], FL_Z1 + SHROUD)])
    for r in (R_IN, R_OUT):
        s -= tube(r - GROOVE[0] / 2, r + GROOVE[0] / 2, FL_Z0 - OVS, FL_Z0 + GROOVE[1])
    return s


def retainer():
    """The deck top's stand-in: an annulus over the ridge (0.3 clear, its
    inner edge following the shroud), on six posts."""
    s = tube(WALL[1] + 0.6, RET_R1, RET_Z0, RET_Z1)
    ra = RP_RING + M + 0.3                        # 0.3 over the 45° shroud (0.21 normal to it)
    s -= revolve_rz([(90, FL_Z1 - 2), (ra + 2, FL_Z1 - 2), (ra - (RET_Z1 + 2 - FL_Z1), RET_Z1 + 2), (90, RET_Z1 + 2)])
    for a in POSTS:
        x, y = polar(POST_R, a)
        s -= cyl(x, y, RET_Z0 - OVS, RET_Z1 + OVS, M3_CLR)
    return s


def station(r, a):
    """A roller's block, at radius r and angle a: two walls with Ø5 bosses,
    a pocket for the roller, a slot open at the top for its pin."""
    h = W_W / 2 + SIDE_GAP
    x0, x1 = r - h - STATION_T, r + h + STATION_T
    blk = box(x0, x1, -5.0, 5.0, PLATE_TOP - 0.1, RC + PIN3[0] / 2 + 1.5)
    blk -= box(r - h, r + h, -W_D / 2 - 1.0, W_D / 2 + 1.0, RC - W_D / 2 - 1.0, RC + 20)
    for xa, xb in ((r - h, r - h + BOSS5[1]), (r + h - BOSS5[1], r + h)):
        blk += Pos(xa, 0, RC) * Rot(0, 90, 0) * Cylinder(BOSS5[0] / 2, xb - xa, align=(Align.CENTER, Align.CENTER, Align.MIN))
    blk -= box(x0 - OVS, x1 + OVS, -(PIN3[0] + 0.1) / 2, (PIN3[0] + 0.1) / 2, RC, RC + 20)
    blk -= Pos(x0 - OVS, 0, RC) * Rot(0, 90, 0) * Cylinder((PIN3[0] + 0.1) / 2, x1 - x0 + 2 * OVS, align=(Align.CENTER, Align.CENTER, Align.MIN))
    return Rot(0, 0, a) * blk


def place_arm(s):
    """The shared arm, drawn at ROLL_R on +y, moved out to the gear and round to SENSE_ANGLE."""
    return Rot(0, 0, SENSE_ANGLE - 90.0) * Pos(0, DY, 0) * s


def plate():
    h = PLATE_SQ / 2
    s = box(-h, h, -h, h, 0, PLATE_T)
    s -= cyl(0, 0, -OVS, PLATE_T + OVS, CABLE_HOLE_D)
    for a in (0.0, 120.0, 240.0):
        x, y = polar(HUB_SCREW_R, a)
        s -= cyl(x, y, PLATE_TOP - INSERT[1], PLATE_TOP + OVS, INSERT[0])
    for r, angles in ((R_IN, A_IN), (R_OUT, A_OUT)):
        for a in angles:
            s += station(r, a)
    for a in POSTS:
        x, y = polar(POST_R, a)
        s += cyl(x, y, PLATE_TOP - 0.1, RET_Z0, POST_D) - cyl(x, y, RET_Z0 - INSERT[1], RET_Z0 + OVS, INSERT[0])
    # The arm: the pad under its foot (the foot slides INWARD here, to push the
    # gear into mesh), its inserts, the recess under the board, the wire hole.
    x0, x1, y0, y1 = FOOT
    arm_feats = box(x0, x1, y0 - PRELOAD_MAX, y1, PLATE_TOP - 0.1, ARM_Z0)
    cuts = None
    for x, y in FOOT_SCREWS:
        c = cyl(x, y, ARM_Z0 - INSERT[1], ARM_Z0 + OVS, INSERT[0])
        cuts = c if cuts is None else cuts + c
    e = ENC_PCB[0] / 2 + 3.0
    cuts += box(-e, e, ROLL_R - e, ROLL_R + e, PLATE_TOP - 2.0, PLATE_TOP + OVS)
    cuts += cyl(0, ROLL_R, -OVS, PLATE_TOP, 8.0)
    s += place_arm(arm_feats)
    s -= place_arm(cuts)
    return s


def pinion():
    """The arm's gear: the O-ring roller's hub, with 20 teeth where its tyre was."""
    z0, (hd, hh) = ROLLER_Z0, ROLLER_HUB
    s = cyl(0, ROLL_R, z0, z0 + hh, hd)
    s += Pos(0, ROLL_R, 0) * Rot(0, 0, 270.0) * gear(Z_PIN, M, z0 + hh, ROLLER_Z1)    # a tooth points at the ring
    s -= cyl(0, ROLL_R, z0 - OVS, ROLLER_Z1 + OVS, AXLE_D)
    s -= Pos(0, ROLL_R, z0 + hh / 2) * Rot(0, 90, 0) * Cylinder(1.25, hd)
    return s


def wheel():
    """A printed roller, the size of a 623 (TPU for quiet, PETG for firm)."""
    return tube(W_BORE / 2, W_D / 2, 0, W_W)


def printed():
    return {
        "pioneer/ring": (ring(), True, 1),
        "pioneer/plate": (plate(), False, 1),
        "pioneer/retainer": (retainer(), True, 1),
        "pioneer/arm": (rig.arm(inward=True), False, 1),
        "pioneer/pinion": (pinion(), True, 1),
        "pioneer/wheel": (wheel(), False, 24),
        "pioneer/hub_210": (rig.hub("210", P_TOP_Z1), True, 1),
        "pioneer/hub_128": (rig.hub("128", P_TOP_Z1), True, 1),
    }


def assembly():
    """Every part, world frame: (name, solid, material)."""
    s = [("pioneer/ring", ring(), "ring"), ("pioneer/plate", plate(), "plate"),
         ("pioneer/retainer", retainer(), "hub"), ("pioneer/hub_210", rig.hub("210", P_TOP_Z1), "hub"),
         ("arm", place_arm(rig.arm(inward=True)), "arm"), ("pinion", place_arm(pinion()), "arm"),
         ("magnet_cup", place_arm(rig.cup()), "arm")]
    w = Pos(-W_W / 2, 0, 0) * Rot(0, 90, 0) * (Cylinder(W_D / 2, W_W, align=(Align.CENTER, Align.CENTER, Align.MIN))
                                               - Cylinder(1.5, W_W, align=(Align.CENTER, Align.CENTER, Align.MIN)))
    p = Pos(-PIN3[1] / 2, 0, 0) * Rot(0, 90, 0) * Cylinder(PIN3[0] / 2, PIN3[1], align=(Align.CENTER, Align.CENTER, Align.MIN))
    for tag, r, angles in (("i", R_IN, A_IN), ("o", R_OUT, A_OUT)):
        for a in angles:
            f = Rot(0, 0, a) * Pos(r, 0, RC)
            s.append((f"w{tag}{int(a)}", f * w, "pom"))
            s.append((f"pin{tag}{int(a)}", f * p, "steel"))
    for n, sol, k in rig.sense_stack(oring=False):
        s.append((n, place_arm(sol), k))
    glass, window, thick = DISPLAYS["210"]
    z1 = P_TOP_Z1 - LIP_T
    s.append(("display", cyl(0, 0, z1 - thick, z1 - 0.05, glass), "black"))
    return s
