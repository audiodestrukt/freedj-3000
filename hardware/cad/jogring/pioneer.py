"""pioneer.py — the "pioneer" version of the rig: the layout of a Pioneer jog,
with Dan's changes.

From the Pioneer teardown (2026-11-20): the platter is a ring carried on small
plastic wheels on pins (about 10 mm, axles horizontal); a little ridge round its
foot sits under the deck's own top, which stops it lifting; the assembly fastens
from inside.

Dan's changes (2026-11-20):
- one circle of rollers, not two;
- an optical quadrature encoder (two slotted opto interrupters) instead of a gear;
- a laser-cut clear window glued into the ring over the fixed display.

Here:
- 16 rollers, 10 × 4 (623 bearings, or printed wheels), on 3 mm pins dropped
  into slots open at the top, at r 90, in one shallow groove that centres the ring;
- a thin fin hangs from the ring at r 75 with CODE_N slots round its foot; it runs
  through two interrupters a quarter-slot apart, so the pair gives direction and
  4 counts per slot (CODE_N × 4 per turn);
- the window sits in a rebate round the bore, flush with the top; the hub stays
  under it, so the display shows through and the top is one surface.
"""
import math
from build123d import *
from params import *
import rig
from rig import cyl, box, tube, revolve_rz, polar

# ── rollers: one circle ──
W_D, W_W = 10.0, 4.0              # 623ZZ: 10 × 3 × 4, or a printed wheel the same size
W_BORE = 3.2                      # printed wheel on a 3 mm pin
PIN3 = (3.0, 10.0)                # 3 mm steel pin ⚠ (or an M3 screw's shank)
R_W, N_W = 90.0, 16
A_W = [360.0 / N_W * k for k in range(N_W)]
GROOVE = (W_W + 0.3, 0.6)         # the rollers' groove in the ring: width, depth
STATION_T = 3.0                   # each side wall of a roller's block
SIDE_GAP = 0.3
BOSS5 = (5.0, SIDE_GAP)           # Ø5 bosses on the walls bear on a 623's inner race only

# ── optical encoder: a slotted fin through two opto interrupters ──
CODE_R = 75.0                     # the fin's mid-radius
CODE_T = 1.5                      # fin thickness (passes a 3.1 gap with 0.8 a side)
CODE_N = 180                      # slots: 720 counts a turn; pitch 2.62 mm, slot = bar = 1.31
CODE_SLOT_H = 4.5                 # how far the slots run up from the fin's foot
OPTO = (12.2, 6.3, 10.8, 3.1, 7.0)  # slotted interrupter body (TCST1202/TCST1103 are 11.9 long: 0.15 slack an end): length (radial), width, height, gap, gap depth ⚠
OPTO_AXIS = 3.5                   # its beam, below its top ⚠ (datasheet)
OPTO_A = 90.0                     # sensor A's angle; B is a whole number of slots + a quarter on
OPTO_B = OPTO_A + (10 + 0.25) * 360.0 / CODE_N
OPTO_WALL = 1.5                   # the printed pocket round each
OPTO_POCKET = 4.0                 # its depth: the fin passes over its walls

# ── window: laser-cut clear disc glued into a rebate round the bore ──
WIN_D, WIN_T = 84.0, 3.0          # ⚠ cut it 0.2 under the rebate
WIN_FIT = 0.2

# ── heights ──
RC = PLATE_TOP + 7.5              # roller axles
FL_Z0 = RC + W_D / 2 + 0.05 - GROOVE[1]     # the ring's foot (the groove's roof is 0.05 over the rollers)
FL_Z1 = FL_Z0 + 3.0               # the ridge's top
SHROUD = 5.0                      # 45° over the ridge: prints without support
RIDGE_R = 106.0                   # the ridge reaches under the deck top
RET_Z0 = FL_Z1 + 0.3              # the "deck top": 0.3 over the ridge (the lift stop)
RET_T = 3.0
RET_Z1 = RET_Z0 + RET_T
RET_R1 = 124.0
POSTS = (15.0, 75.0, 135.0, 195.0, 255.0, 315.0)
POST_R, POST_D = 118.0, 10.0
P_TOP_Z1 = RET_Z1 + 11.6          # the jog stands 11.6 proud of the deck top
P_TOP_Z0 = P_TOP_Z1 - TOP_T
HUB_TOP = P_TOP_Z1 - WIN_T - 0.8  # the hub stays under the window
WALL = (99.0, 103.0)              # outer wall
OPTO_TOP = PLATE_TOP + OPTO[2]    # the interrupters stand on the plate
CODE_Z0 = OPTO_TOP - OPTO[4] + 1.5          # the fin's foot: 1.5 over the gap's floor
PLATE_D2 = 250.0

assert RC - W_D / 2 > PLATE_TOP + 1.0
assert RC + PIN3[0] / 2 + 1.5 < FL_Z0 - 1.0, "roller blocks touch the ring"
assert CODE_Z0 + CODE_SLOT_H > OPTO_TOP - OPTO_AXIS + 1.0 and CODE_Z0 < OPTO_TOP - OPTO_AXIS - 1.0, "the slots miss the beam"
assert CODE_Z0 + 0.3 < OPTO_TOP and OPTO[3] - CODE_T >= 1.2, "the fin in the gap"
assert math.pi * 2 * CODE_R / CODE_N / 2 > 1.2, "slots too fine to print"
assert CODE_R + OPTO[0] / 2 + OPTO_WALL < R_W - W_W / 2 - SIDE_GAP - STATION_T - 1, "an interrupter hits the roller blocks"
assert PLATE_TOP + OPTO_POCKET < CODE_Z0 - 0.5, "the fin hits the interrupters' pockets"
assert P_TOP_Z0 > RET_Z1 + 1


def ring():
    r_out = JOG_D / 2
    s = tube(RING_BORE / 2, r_out, P_TOP_Z0, P_TOP_Z1)
    s += tube(r_out - RIM_W, r_out, P_TOP_Z0 - RIM_DROP, P_TOP_Z0 + 0.5)
    s += tube(*WALL, FL_Z1 - 0.5, P_TOP_Z0 + 0.5)
    # The foot: a band over the rollers, out to the ridge under the deck top.
    s += tube(R_W - 7.0, RIDGE_R, FL_Z0, FL_Z1)
    s += revolve_rz([(WALL[0], FL_Z1 - 0.01), (RIDGE_R, FL_Z1 - 0.01), (RIDGE_R - SHROUD, FL_Z1 + SHROUD), (WALL[0], FL_Z1 + SHROUD)])
    s -= tube(R_W - GROOVE[0] / 2, R_W + GROOVE[0] / 2, FL_Z0 - OVS, FL_Z0 + GROOVE[1])
    # The encoder fin, its foot slotted.
    s += tube(CODE_R - CODE_T / 2, CODE_R + CODE_T / 2, CODE_Z0, P_TOP_Z0 + 0.5)
    w = math.pi * 2 * (CODE_R + CODE_T) / CODE_N / 2
    tooth = box(CODE_R - CODE_T, CODE_R + CODE_T, -w / 2, w / 2, CODE_Z0 - OVS, CODE_Z0 + CODE_SLOT_H)
    slots = None
    for k in range(CODE_N):
        t = Rot(0, 0, 360.0 * k / CODE_N) * tooth
        slots = t if slots is None else slots + t
    s -= slots
    # A stiffening lip at the bore, and the window's rebate in the top.
    s += tube(RING_BORE / 2, RING_BORE / 2 + 3.0, P_TOP_Z0 - 4.0, P_TOP_Z0 + 0.5)
    s -= cyl(0, 0, P_TOP_Z1 - WIN_T, P_TOP_Z1 + OVS, WIN_D + WIN_FIT)
    return s


def window():
    """The laser-cut clear disc (3 mm acrylic), for the assembly."""
    return cyl(0, 0, P_TOP_Z1 - WIN_T, P_TOP_Z1, WIN_D)


def retainer():
    """The deck top's stand-in: an annulus over the ridge (0.3 clear, its
    inner edge following the shroud), on six posts."""
    s = tube(WALL[1] + 0.6, RET_R1, RET_Z0, RET_Z1)
    ra = RIDGE_R + 0.3                            # 0.3 over the 45° shroud (0.21 normal to it)
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


def opto_frame(a):
    """An interrupter at angle a: long axis radial, its gap across the fin."""
    return Rot(0, 0, a) * Pos(CODE_R, 0, PLATE_TOP)


def opto():
    l, w, h, g, gd = OPTO
    b = box(-l / 2, l / 2, -w / 2, w / 2, 0, h) - box(-g / 2, g / 2, -w, w, h - gd, h + OVS)
    return b


def plate():
    s = cyl(0, 0, 0, PLATE_T, PLATE_D2)
    s -= cyl(0, 0, -OVS, PLATE_T + OVS, CABLE_HOLE_D)
    for a in (0.0, 120.0, 240.0):
        x, y = polar(HUB_SCREW_R, a)
        s -= cyl(x, y, PLATE_TOP - INSERT[1], PLATE_TOP + OVS, INSERT[0])
    for a in A_W:
        s += station(R_W, a)
    for a in POSTS:
        x, y = polar(POST_R, a)
        s += cyl(x, y, PLATE_TOP - 0.1, RET_Z0, POST_D) - cyl(x, y, RET_Z0 - INSERT[1], RET_Z0 + OVS, INSERT[0])
    # A pocket for each interrupter (OPTO_POCKET deep: under the fin's foot), and a slot under it for its leads.
    l, w, h, g, gd = OPTO
    for a in (OPTO_A, OPTO_B):
        f = opto_frame(a)
        t = OPTO_WALL
        s += f * (box(-l / 2 - t, l / 2 + t, -w / 2 - t, w / 2 + t, -0.1, OPTO_POCKET) - box(-l / 2 - 0.1, l / 2 + 0.1, -w / 2 - 0.1, w / 2 + 0.1, 0, OPTO_POCKET + OVS))
        s -= f * box(-l / 2 + 1, l / 2 - 1, -w / 2 + 1, w / 2 - 1, -PLATE_T - OVS, OVS)
    return s


def wheel():
    """A printed roller, the size of a 623 (TPU for quiet, PETG for firm)."""
    return tube(W_BORE / 2, W_D / 2, 0, W_W)


def printed():
    return {
        "pioneer/ring": (ring(), True, 1),
        "pioneer/plate": (plate(), False, 1),
        "pioneer/retainer": (retainer(), True, 1),
        "pioneer/wheel": (wheel(), False, N_W),
        "pioneer/hub_210": (rig.hub("210", HUB_TOP), True, 1),
        "pioneer/hub_128": (rig.hub("128", HUB_TOP), True, 1),
    }


def assembly():
    """Every part, world frame: (name, solid, material)."""
    s = [("pioneer/ring", ring(), "ring"), ("pioneer/plate", plate(), "plate"),
         ("pioneer/retainer", retainer(), "hub"), ("pioneer/hub_210", rig.hub("210", HUB_TOP), "hub"),
         ("window", window(), "glass")]
    w = Pos(-W_W / 2, 0, 0) * Rot(0, 90, 0) * (Cylinder(W_D / 2, W_W, align=(Align.CENTER, Align.CENTER, Align.MIN))
                                               - Cylinder(1.5, W_W, align=(Align.CENTER, Align.CENTER, Align.MIN)))
    p = Pos(-PIN3[1] / 2, 0, 0) * Rot(0, 90, 0) * Cylinder(PIN3[0] / 2, PIN3[1], align=(Align.CENTER, Align.CENTER, Align.MIN))
    for a in A_W:
        f = Rot(0, 0, a) * Pos(R_W, 0, RC)
        s.append((f"w{int(a * 10)}", f * w, "pom"))
        s.append((f"pin{int(a * 10)}", f * p, "steel"))
    for tag, a in (("a", OPTO_A), ("b", OPTO_B)):
        s.append((f"opto_{tag}", opto_frame(a) * opto(), "black"))
    glass, window_d, thick = DISPLAYS["210"]
    z1 = HUB_TOP - LIP_T
    s.append(("display", cyl(0, 0, z1 - thick, z1 - 0.05, glass), "black"))
    return s


def encoder_check(ring_mesh, name="pioneer", code_r=CODE_R, code_n=CODE_N, opto_a=OPTO_A, opto_b=OPTO_B, zb=OPTO_TOP - OPTO_AXIS):
    """Cast each interrupter's beam across its gap while turning the ring mesh
    through two slot pitches; the A/B states must step through the quadrature
    cycle one state at a time, in one direction, 4 per slot.  Returns the
    number of failures."""
    import numpy as np, trimesh
    CODE_R, CODE_N, OPTO_A, OPTO_B = code_r, code_n, opto_a, opto_b

    def blocked(m, a):
        c, s = math.cos(math.radians(a)), math.sin(math.radians(a))
        loc, _, _ = m.ray.intersects_location([[(CODE_R - 3) * c, (CODE_R - 3) * s, zb]], [[c, s, 0]])
        return any(np.hypot(*l[:2]) < CODE_R + 3 for l in loc)

    sub = 16
    seq = []
    for i in range(2 * sub + 1):
        m = ring_mesh.copy()
        m.apply_transform(trimesh.transformations.rotation_matrix(math.radians(i * 360.0 / CODE_N / sub), [0, 0, 1]))
        seq.append((int(blocked(m, OPTO_A)), int(blocked(m, OPTO_B))))
    states = [s for i, s in enumerate(seq) if i == 0 or s != seq[i - 1]]
    gray = {(0, 0): 0, (0, 1): 1, (1, 1): 2, (1, 0): 3}
    steps = [(gray[b] - gray[a]) % 4 for a, b in zip(states, states[1:])]
    ok = len(steps) == 8 and len(set(steps)) == 1 and steps[0] in (1, 3)
    print(f"encoder {name}: {' '.join(f'{a}{b}' for a, b in states)} -> "
          f"{'quadrature, ' + str(4 * CODE_N) + ' counts a turn' if ok else 'NOT QUADRATURE'}")
    return 0 if ok else 1
