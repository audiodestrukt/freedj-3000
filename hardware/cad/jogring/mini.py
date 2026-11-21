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
- The encoder fin at r 40, 96 slots: 384 counts a turn in quadrature.
- A 1.28" display (the GC9A01 Dan has) under a Ø54 × 3 clear window.
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
HUB_OD = BIG + 2 * HUB_WALL       # 44.4
RING_BORE = HUB_OD + 3.0
WIN_D, WIN_T, WIN_FIT = 54.0, 3.0, 0.2
HUB_FL = (30.5, 6.5)              # the hub's flange: outer radius, thickness (takes the inserts)
HUB_SCREWS = (27.0, (30.0, 150.0, 270.0))

# ── encoder ──
CODE_R, CODE_T, CODE_N, CODE_SLOT_H = 40.0, 1.5, 96, 4.5
OPTO = pioneer.OPTO
OPTO_A = 90.0
OPTO_B = OPTO_A + (4 + 0.25) * 360.0 / CODE_N
OPTO_WALL, OPTO_POCKET = 1.5, 4.0

# ── carrier (the ring below) ──
CT = 4.0                          # its thickness; its top is z = CT
CAR = (46.0, 60.0)                # the open ring
SPOKES, SPOKE_W = (210.0, 330.0), 7.0
PAD = (31.0, 48.0, OPTO_A - 10.0, OPTO_B + 10.0)   # the opto pad: r0, r1, a0, a1
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
HUB_TOP = P_TOP_Z1 - WIN_T - 0.8
OPTO_TOP = CT + OPTO[2]
CODE_Z0 = OPTO_TOP - OPTO[4] + 1.5


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
assert CT + OPTO_POCKET < CODE_Z0 - 0.5, "the fin hits the interrupters' pockets"
assert math.pi * 2 * CODE_R / CODE_N / 2 > 1.2, "slots too fine to print"
assert CODE_R + OPTO[0] / 2 + OPTO_WALL < R_W - W_W / 2 - SHOULDER - POST_T + 0.2, "an interrupter hits the posts"
assert CODE_R - OPTO[0] / 2 - OPTO_WALL > HUB_FL[0] + 0.5, "an interrupter hits the hub's flange"
assert CODE_R - CODE_T / 2 > WIN_D / 2 + 1 and CODE_R + CODE_T / 2 < FOOT_R0 - 1
assert POST_R - POST_D / 2 > RIDGE_R + 1.0, "a deck-top post hits the ridge"
assert P_TOP_Z0 > RET_Z1 + 1 and HUB_TOP - LIP_T - THICK > CT + HUB_FL[1]


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
        s -= cyl(x, y, -OVS, CT + OVS, M3_CLR)
    for a in A_W:
        s += station(bore, a)
    l, w, h, g, gd = OPTO
    for a in (OPTO_A, OPTO_B):
        f, t = opto_frame(a), OPTO_WALL
        s += f * (box(-l / 2 - t, l / 2 + t, -w / 2 - t, w / 2 + t, -0.1, OPTO_POCKET) - box(-l / 2 - 0.1, l / 2 + 0.1, -w / 2 - 0.1, w / 2 + 0.1, 0, OPTO_POCKET + OVS))
        s -= f * box(-l / 2 + 1, l / 2 - 1, -w / 2 + 1, w / 2 - 1, -CT - OVS, OVS)
    return s


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
    s += tube(RING_BORE / 2, RING_BORE / 2 + 3.0, P_TOP_Z0 - 3.0, P_TOP_Z0 + 0.5)
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


def hub():
    """The fixed hub for the 1.28" module; it screws up into its flange's inserts."""
    s = tube(BIG / 2, HUB_OD / 2, CT, HUB_TOP)
    s += tube(ACTIVE / 2, BIG / 2 + 0.5, HUB_TOP - LIP_T, HUB_TOP)
    s += tube(BIG / 2 - 0.5, HUB_FL[0], CT, CT + HUB_FL[1])
    r, angles = HUB_SCREWS
    for a in angles:
        x, y = polar(r, a)
        s -= cyl(x, y, CT - OVS, CT + INSERT[1], INSERT[0])
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
    }


def assembly(bore=BEARING):
    s = [("mini/ring", ring(), "ring"), ("mini/carrier", carrier(bore), "plate"),
         ("mini/retainer", retainer(), "hub"), ("mini/hub_128", hub(), "hub"), ("window", window(), "glass")]
    b = bearing(bore)
    for a in A_W:
        s.append((f"b{bore}_{int(a)}", Rot(0, 0, a) * Pos(R_W, 0, RC) * b, "steel"))
    for tag, a in (("a", OPTO_A), ("b", OPTO_B)):
        s.append((f"opto_{tag}", opto_frame(a) * pioneer.opto(), "black"))
    z1 = HUB_TOP - LIP_T
    s.append(("display", cyl(0, 0, z1 - THICK, z1 - 0.05, GLASS), "black"))
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
