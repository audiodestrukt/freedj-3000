"""mini.py — the "mini" version: the pioneer layout at 5" (127 mm), to print
and iterate fast.

Dan's brief (2026-11-21):
- 5" across, so a test print is quick;
- the little 10 mm bearings he has, snapped onto bosses that stand up from
  the ring below: no pins, no tools;
- that lower ring hollow in the middle, so it prints fast;
- it still mounts up under a deck shell from underneath, with the platter
  held down by the shell (the ridge under the deck top, as in pioneer).

Here:
- 10 bearings at r 55 on snap stubs: each stub stands out radially from a
  post printed with the carrier, split down its length by a vertical slot (it
  prints without support, and carries the press in the halves' stiff
  direction), with a 0.2 mm barb at its tip.  Push a bearing on: the halves
  close, the barb clicks past the inner race and holds it.  A shoulder on the
  stub touches the inner race only.
- Dan's bearings are 10 × 6 × 3 (MR106ZZ).
- The carrier is an open ring (r 46..60) with three spokes to a small ring
  under the hub, and a pad that holds the two opto interrupters.  The middle
  is open for the display's cable.
- The deck top (`mini/retainer`) has four posts hanging from its underside,
  with M3 heat-set inserts in their feet; the carrier screws up into them
  from below, and the hub screws up into the carrier's middle ring the same
  way.  Everything fastens from inside, as on the Pioneer.
- The encoder fin at r 40, 64 slots: 256 counts a turn in quadrature.  The
  sensors sit on brackets screwed up from below, so they can be swapped.
- A 1.28" display (the GC9A01 Dan has) under a Ø54 × 3 clear window,
  standing on its four M2 standoffs (25 × 19) on a floor in the hub.
"""
import math
from build123d import *
from params import OVS, M3_CLR, INSERT, DISPLAYS, HUB_FIT, HUB_WALL, LIP_T, TOP_T, RIM_W, RIM_DROP
import pioneer
from rig import cyl, box, tube, revolve_rz, polar

# ── bearings: 10 mm, either bore ──
BEARINGS = {"mr106": (10.0, 6.0, 3.0)}   # OD, ID, width: Dan's 10 x 6 x 3 (MR106ZZ) ⚠
BEARING = "mr106"
STUB_D = 5.8                      # the bore as Dan's calipers read it (inside jaws read a small bore low; nominal 6.0)
W_D, W_W = 10.0, 3.0
R_W, N_W = 55.0, 10
A_W = [360.0 / N_W * k for k in range(N_W)]
GROOVE = (W_W + 0.3, 0.6)         # the bearings' groove in the ring: width, depth

# ── snap stub ──
SHOULDER = 2.5                    # from the post's face to the bearing's inner face
SHOULDER_D = 0.8                  # over the bore: the shoulder touches the inner race only
PIN_FIT = 0.05                    # the stub under the bore
PLAY = 0.1                        # axial, between the shoulder and the barb
BARB, BARB_L, LEAD = 0.25, 0.5, 1.0  # the barb: radial catch, its land, the lead-in cone (Ø6.3: holds a true 6.0 bore too)
SLOT_W = 1.2                      # the split: the halves close 0.2 each to pass the bore (wide, so they bend easily)
POST_T, POST_W = 3.5, 8.0         # the post: radial thickness, width

# ── sizes ──
JOG_D = 127.0                     # 5"
R_OUT = JOG_D / 2
WALL = (60.0, R_OUT)
RIDGE_R = 66.5                    # the ridge reaches under the deck top
SHROUD = 4.0
GLASS, ACTIVE, THICK = DISPLAYS["128"]
BIG = GLASS + 2 * HUB_FIT
BOARD_D, GLASS_T = 36.0, 1.5      # the module's round PCB under the glass (Dan); the glass layer ⚠ assumed
HUB_OD = BIG + 2 * 2.0            # 46.4: a 2 mm wall, so it fits under the ring's window seat
RING_BORE = 47.4                  # the window seat's inner edge
WIN_D, WIN_T, WIN_FIT = 54.0, 3.0, 0.2
HUB_FL = (31.2, 6.5)              # the hub's flange: outer radius, thickness (takes the inserts)
HUB_SCREWS = (27.0, (30.0, 150.0, 270.0))   # 3 x M3 on a 54 mm circle (BCD), 120° apart
HUB_INSERT = 4.4                  # the inserts' holes: 4.0 printed too tight (FDM holes print small); 1.9 mm wall outside
HUB_CLR = 3.8                     # the carrier's holes for them: 0.4 of slack over M3_CLR to take a print's error

# ── encoder ──
CODE_R, CODE_T, CODE_N, CODE_SLOT_H = 39.5, 1.5, 64, 4.5   # 64 slots (1.96 mm): coarse enough for a premade board's wide beam
OPTO = pioneer.OPTO
OPTO_A = 90.0
OPTO_B = OPTO_A + (8 + 0.25) * 360.0 / CODE_N
OPTO_WALL, OPTO_POCKET = 1.5, 3.0
# Each sensor sits on a bracket screwed up from under the carrier (two M3
# thread-forming screws), so the sensor can change without reprinting the
# ring or the carrier: `mini/opto_bracket` takes a flangeless TCST1202 /
# TCST1103; a bracket for a premade LM393 board uses the same two holes.
BR_T, BR_BOSS, EAR = 3.0, 5.5, 8.0      # bracket base, its screw bosses' height, the screws ± this tangentially

# ── carrier (the ring below) ──
CT = 4.0                          # its thickness; its top is z = CT
CAR = (46.0, 60.0)                # the open ring
SPOKES, SPOKE_W = (210.0, 330.0), 7.0
PAD = (31.0, 48.0, OPTO_A - 18.0, OPTO_B + 18.0)   # the opto pad: r0, r1, a0, a1
POSTS, POST_R, POST_D = (45.0, 135.0, 225.0, 315.0), 73.0, 9.0

# ── heights ──
RC = CT + 1.5 + W_D / 2           # bearing axles: 1.5 over the carrier
FL_Z0 = RC + W_D / 2 + 0.05 - GROOVE[1]
FL_Z1 = FL_Z0 + 3.0
FOOT_R0 = R_W - 3.2               # the foot's inner edge: over the groove's inner wall, clear of the posts
RET_Z0 = FL_Z1 + 0.3              # the deck top, 0.3 over the ridge
RET_T, RET_R1 = 3.0, 82.0
RET_Z1 = RET_Z0 + RET_T
P_TOP_Z1 = RET_Z1 + 8.0           # 8 mm proud of the deck top
P_TOP_Z0 = P_TOP_Z1 - TOP_T
GLASS_GAP = 0.5                   # the glass's top under the window's underside: just under the jog's centre
GLASS_Z1 = P_TOP_Z1 - WIN_T - GLASS_GAP
HUB_TOP = P_TOP_Z0 - 1.0          # the hub's wall stops 1 under the ring's window seat, level with the glass's underside (the standoffs hold the module)
OPTO_TOP = CT + BR_T + OPTO[2]
CODE_Z0 = OPTO_TOP - OPTO[4] + 1.5
# The module stands on four M2 standoffs; a floor in the hub takes them, M2
# screws up through it from below (through the carrier's open middle).
DISP_HOLES = (25.0, 19.0)         # the module's M2 holes, centres: across (x), and up the screen (y) ⚠ Dan's calipers
DISP_STACK = 11.5                 # the module, glass top to the standoffs' feet (Dan, calipered 2026-11-22)
M2_CLR, FLOOR_T, FLOOR_HOLE = 2.4, 2.0, 22.0   # the floor: screw clearance, thickness, its middle opening for the cable
FLOOR_Z1 = GLASS_Z1 - DISP_STACK   # the floor's top, where the standoffs stand
# A bump on the module (its connector tab) makes it 44 across in one place;
# it points out from a 25 mm side of the hole pattern (-y here).  The hub's
# wall is notched for it, and the ring's bore lip is moved out over it.
BUMP_A = 270.0                    # its direction
BUMP = (23.0, 16.0, GLASS_T, 6.0)  # reach from the centre (Dan: 2 past the Ø42 bezel, 44 across), width (~16, tapered), its top and bottom under the glass (it is the board)
RELIEF_R = math.hypot(BUMP[0], BUMP[1] / 2) + 0.8   # the ring's bore under the window seat, past the bump's corners (was 23.7)


def post_face():
    return R_W - W_W / 2 - SHOULDER


assert R_W - W_D / 2 > 0 and RC - W_D / 2 >= CT + 1.0
assert post_face() < FOOT_R0 - 0.5, "a post under the ring's foot"
assert FOOT_R0 < R_W - GROOVE[0] / 2 - 0.8, "the groove's inner wall too thin to print"
for _b, (_od, _id, _w) in BEARINGS.items():
    assert RC + _id / 2 + SHOULDER_D / 2 < FL_Z0 - 1.0, "a shoulder under the ring"
    assert SLOT_W >= 2 * BARB + 0.15, "the halves can't close past the bore"
assert R_W + W_W / 2 + PLAY + BARB_L + LEAD < WALL[0] - 1.0, "a stub's tip hits the ring's wall"
assert CODE_Z0 + CODE_SLOT_H > OPTO_TOP - pioneer.OPTO_AXIS + 1.0 and CODE_Z0 < OPTO_TOP - pioneer.OPTO_AXIS - 1.0, "the slots miss the beam"
assert CT + BR_T + OPTO_POCKET < CODE_Z0 - 0.5 and CT + BR_BOSS < CODE_Z0 - 1.0, "the fin hits a bracket"
assert math.pi * 2 * CODE_R / CODE_N / 2 > 1.2, "slots too fine to print"
assert CODE_R + OPTO[0] / 2 + OPTO_WALL < R_W - W_W / 2 - SHOULDER - POST_T + 0.2, "an interrupter hits the posts"
assert HUB_FL[0] - HUB_SCREWS[0] - HUB_INSERT / 2 >= 1.9 and CODE_R - OPTO[0] / 2 - OPTO_WALL > HUB_FL[0] + 0.1, "an interrupter hits the hub's flange"
assert CODE_R - CODE_T / 2 > WIN_D / 2 + 1 and CODE_R + CODE_T / 2 < FOOT_R0 - 1
assert POST_R - POST_D / 2 > RIDGE_R + 1.0, "a deck-top post hits the ridge"
assert P_TOP_Z0 > RET_Z1 + 1 and GLASS_Z1 - THICK > CT + HUB_FL[1] and HUB_OD / 2 < RING_BORE / 2 - 0.4
assert GLASS_Z1 - BUMP[2] < P_TOP_Z0 - 0.4 and RELIEF_R + 3 < CODE_R - CODE_T / 2 - 1, "the bump hits the window seat"
assert FLOOR_Z1 - FLOOR_T >= CT, "standoffs too long for the hub: lower the floor below the carrier"
assert math.hypot(*DISP_HOLES) / 2 + M2_CLR / 2 + 1.0 < BIG / 2 and math.hypot(*DISP_HOLES) / 2 - 2.5 > FLOOR_HOLE / 2
assert math.hypot(*DISP_HOLES) / 2 + 2.0 < 18.5, "the screw heads can't be reached through the carrier's middle"


def xcyl(x0, x1, d):
    """A cylinder along x, from x0 to x1, on the axis y = z = 0."""
    return Pos(x0, 0, 0) * Rot(0, 90, 0) * Cylinder(d / 2, x1 - x0, align=(Align.CENTER, Align.CENTER, Align.MIN))


def stub(bore):
    """The snap stub, axle frame (x radial out, origin at the bearing's centre)."""
    _, _, w = BEARINGS[bore]
    d = STUB_D
    xf = -w / 2 - SHOULDER
    x_barb = w / 2 + PLAY
    s = xcyl(xf - 0.5, -w / 2 - 0.05, d + SHOULDER_D)
    s += xcyl(-w / 2 - 0.1, x_barb, d - PIN_FIT)
    s += xcyl(x_barb, x_barb + BARB_L, d + 2 * BARB)
    s += Pos(x_barb + BARB_L, 0, 0) * Rot(0, 90, 0) * Cone(d / 2 + BARB, d / 2 - 0.3, LEAD, align=(Align.CENTER, Align.CENTER, Align.MIN))
    s -= box(xf, x_barb + BARB_L + LEAD + OVS, -SLOT_W / 2, SLOT_W / 2, -d, d)
    return s


def station(bore, a):
    _, d, _ = BEARINGS[bore]
    xf = -W_W / 2 - SHOULDER
    post = box(xf - POST_T, xf, -POST_W / 2, POST_W / 2, CT - 0.1 - RC, d / 2 + 2.0)
    return Rot(0, 0, a) * Pos(R_W, 0, RC) * (post + stub(bore))


def opto_frame(a):
    return Rot(0, 0, a) * Pos(CODE_R, 0, CT)


def sector(r0, r1, a0, a1, z0, z1):
    pts = [polar(r1, a0 + (a1 - a0) * i / 16) for i in range(17)] + [polar(r0, a1 - (a1 - a0) * i / 16) for i in range(17)]
    return Pos(0, 0, z0) * extrude(Polygon(*pts, align=None), z1 - z0)


def carrier(bore=BEARING):
    """The ring below: open in the middle, three spokes and the opto pad to
    the hub's ring, the bearings' posts, lugs up to the deck top's posts."""
    s = tube(*CAR, 0, CT) + tube(18.5, HUB_FL[0], 0, CT)
    for a in SPOKES:
        s += Rot(0, 0, a) * box(HUB_FL[0] - 1, CAR[0] + 1, -SPOKE_W / 2, SPOKE_W / 2, 0, CT)
    s += sector(PAD[0], PAD[1], PAD[2], PAD[3], 0, CT)
    for a in POSTS:
        x, y = polar(POST_R, a)
        s += Rot(0, 0, a) * box(CAR[1] - 1, POST_R, -POST_D / 2, POST_D / 2, 0, CT) + cyl(x, y, 0, CT, POST_D + 2)
        s -= cyl(x, y, -OVS, CT + OVS, M3_CLR)
    r, angles = HUB_SCREWS
    for a in angles:
        x, y = polar(r, a)
        s -= cyl(x, y, -OVS, CT + OVS, HUB_CLR)
    for a in A_W:
        s += station(bore, a)
    l, w, h, g, gd = OPTO
    for a in (OPTO_A, OPTO_B):
        f = opto_frame(a)
        s -= f * box(-l / 2 + 1, l / 2 - 1, -w / 2 + 1, w / 2 - 1, -CT - OVS, OVS)    # the leads
        for y in (-EAR, EAR):
            s -= f * cyl(0, y, -CT - OVS, OVS, M3_CLR)
    return s


def opto_bracket():
    """A TCST1202 / TCST1103 in a pocket on a base, two bosses for M3
    thread-forming screws up through the carrier (opto frame)."""
    l, w, h, g, gd = OPTO
    t = OPTO_WALL
    s = box(-l / 2 - t, l / 2 + t, -w / 2 - t, w / 2 + t, 0, BR_T) + box(-4.5, 4.5, -EAR - 3.5, EAR + 3.5, 0, BR_T)   # a cross: clear of the bearing posts
    s += box(-l / 2 - t, l / 2 + t, -w / 2 - t, w / 2 + t, BR_T - 0.1, BR_T + OPTO_POCKET)
    s -= box(-l / 2 - 0.1, l / 2 + 0.1, -w / 2 - 0.1, w / 2 + 0.1, BR_T, BR_T + OPTO_POCKET + OVS)
    s -= box(-l / 2 + 1, l / 2 - 1, -w / 2 + 1, w / 2 - 1, -OVS, BR_T + OVS)    # the leads
    for y in (-EAR, EAR):
        s += cyl(0, y, BR_T - 0.1, BR_BOSS, 7.0)
        s -= cyl(0, y, -OVS, BR_BOSS + OVS, 2.5)
    return s


def sensor_frame(a):
    return Rot(0, 0, a) * Pos(CODE_R, 0, CT + BR_T + 0.1)


def ring():
    s = tube(RING_BORE / 2, R_OUT, P_TOP_Z0, P_TOP_Z1)
    s += tube(R_OUT - RIM_W, R_OUT, P_TOP_Z0 - RIM_DROP, P_TOP_Z0 + 0.5)
    s += tube(*WALL, FL_Z1 - 0.5, P_TOP_Z0 + 0.5)
    s += tube(FOOT_R0, RIDGE_R, FL_Z0, FL_Z1)
    s += revolve_rz([(WALL[0], FL_Z1 - 0.01), (RIDGE_R, FL_Z1 - 0.01), (RIDGE_R - SHROUD, FL_Z1 + SHROUD), (WALL[0], FL_Z1 + SHROUD)])
    s -= tube(R_W - GROOVE[0] / 2, R_W + GROOVE[0] / 2, FL_Z0 - OVS, FL_Z0 + GROOVE[1])
    s += tube(CODE_R - CODE_T / 2, CODE_R + CODE_T / 2, CODE_Z0, P_TOP_Z0 + 0.5)
    w = math.pi * 2 * (CODE_R + CODE_T) / CODE_N / 2
    tooth = box(CODE_R - CODE_T, CODE_R + CODE_T, -w / 2, w / 2, CODE_Z0 - OVS, CODE_Z0 + CODE_SLOT_H)
    slots = None
    for k in range(CODE_N):
        t = Rot(0, 0, 360.0 * k / CODE_N) * tooth
        slots = t if slots is None else slots + t
    s -= slots
    s += tube(RELIEF_R, RELIEF_R + 3.0, P_TOP_Z0 - 3.0, P_TOP_Z0 + 0.5)   # the bore lip, out over the module's bump
    s -= cyl(0, 0, P_TOP_Z1 - WIN_T, P_TOP_Z1 + OVS, WIN_D + WIN_FIT)
    return s


def window():
    return cyl(0, 0, P_TOP_Z1 - WIN_T, P_TOP_Z1, WIN_D)


def retainer():
    """The deck top's stand-in: an annulus over the ridge, four posts hanging
    from its underside with inserts in their feet for the carrier's screws."""
    s = tube(WALL[1] + 0.6, RET_R1, RET_Z0, RET_Z1)
    ra = RIDGE_R + 0.3
    s -= revolve_rz([(WALL[0] - 5, FL_Z1 - 2), (ra + 2, FL_Z1 - 2), (ra - (RET_Z1 + 2 - FL_Z1), RET_Z1 + 2), (WALL[0] - 5, RET_Z1 + 2)])
    for a in POSTS:
        x, y = polar(POST_R, a)
        s += cyl(x, y, CT, RET_Z0 + 0.1, POST_D) - cyl(x, y, CT - OVS, CT + INSERT[1], INSERT[0])
    return s


def disp_holes():
    hx, hy = DISP_HOLES[0] / 2, DISP_HOLES[1] / 2
    return [(sx * hx, sy * hy) for sx in (-1, 1) for sy in (-1, 1)]


def hub():
    """The fixed hub for the 1.28" module; it screws up into its flange's
    inserts.  The module stands on a floor inside it, screwed from below."""
    s = tube(BIG / 2, HUB_OD / 2, CT, HUB_TOP)
    s += tube(FLOOR_HOLE / 2, BIG / 2 + 0.5, FLOOR_Z1 - FLOOR_T, FLOOR_Z1)
    for x, y in disp_holes():
        s -= cyl(x, y, FLOOR_Z1 - FLOOR_T - OVS, FLOOR_Z1 + OVS, M2_CLR)
    w = BUMP[1] + 2.0
    s -= Rot(0, 0, BUMP_A) * box(15.0, HUB_OD / 2 + 1, -w / 2, w / 2, GLASS_Z1 - BUMP[3] - 0.5, HUB_TOP + OVS)
    s += tube(BIG / 2 - 0.5, HUB_FL[0], CT, CT + HUB_FL[1])
    r, angles = HUB_SCREWS
    for a in angles:
        x, y = polar(r, a)
        s -= cyl(x, y, CT - OVS, CT + INSERT[1], HUB_INSERT)
    return s


def bearing(bore):
    od, d, w = BEARINGS[bore]
    return xcyl(-w / 2, w / 2, od) - xcyl(-w / 2 - OVS, w / 2 + OVS, d)


def printed():
    return {
        "mini/ring": (ring(), True, 1),
        "mini/carrier": (carrier(), False, 1),
        "mini/retainer": (retainer(), True, 1),
        "mini/hub_128": (hub(), True, 1),
        "mini/opto_bracket": (opto_bracket(), False, 2),
    }


def assembly(bore=BEARING):
    s = [("mini/ring", ring(), "ring"), ("mini/carrier", carrier(bore), "plate"),
         ("mini/retainer", retainer(), "hub"), ("mini/hub_128", hub(), "hub"), ("window", window(), "glass")]
    b = bearing(bore)
    for a in A_W:
        s.append((f"b{bore}_{int(a)}", Rot(0, 0, a) * Pos(R_W, 0, RC) * b, "steel"))
    for tag, a in (("a", OPTO_A), ("b", OPTO_B)):
        s.append((f"opto_{tag}", sensor_frame(a) * pioneer.opto(), "black"))
        s.append((f"mini/opto_bracket_{tag}", opto_frame(a) * Pos(0, 0, 0.05) * opto_bracket(), "hub"))   # drawn 0.05 over the carrier
    z1 = GLASS_Z1
    tab = Rot(0, 0, BUMP_A) * box(BOARD_D / 2 - 0.5, BUMP[0], -BUMP[1] / 2, BUMP[1] / 2, z1 - BUMP[3], z1 - BUMP[2] + 0.01)
    s.append(("display", cyl(0, 0, z1 - GLASS_T, z1, GLASS) + cyl(0, 0, z1 - THICK, z1 - GLASS_T + 0.01, BOARD_D) + tab, "black"))
    for k, (x, y) in enumerate(disp_holes()):
        s.append((f"standoff_{k}", cyl(x, y, FLOOR_Z1 + 0.05, z1 - THICK - 0.05, 3.5), "steel"))
    return s


def encoder_check(ring_mesh):
    return pioneer.encoder_check(ring_mesh, "mini", CODE_R, CODE_N, OPTO_A, OPTO_B, OPTO_TOP - pioneer.OPTO_AXIS)


def snap_report():
    """The snap's strain, a cantilevered half-stub closing BARB: 3 t δ / (2 L²)."""
    out = []
    for b, (_, _, w) in BEARINGS.items():
        d = STUB_D
        t = (d - PIN_FIT - SLOT_W) / 2
        L = SHOULDER + w + PLAY + BARB_L
        out.append(f"{b}: halves {t:.2f} thick, {L:.1f} long, {100 * 3 * t * BARB / (2 * L * L):.1f} % strain to snap")
    return "snap " + "; ".join(out) + " (PETG yields near 4 %, PLA near 2.5 %)"
