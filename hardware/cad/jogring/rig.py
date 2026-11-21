"""rig.py — build, check and export the ring-jog test rig.

    python3 hardware/cad/jogring/rig.py        (or: make jogring)

Writes print/*.stl (each turned to its print orientation) and
build/jogring.glb + build/jogring_cutaway.glb for render.sh.  Every printed
part must be watertight, one body and fit the bed, and no two parts of the
assembly may overlap by more than 1 mm³.  The two places that touch on
purpose (the wheels' V in the ring's groove, the roller's tyre on the
skirt) are drawn 0.05 mm apart.  Exits non-zero on any failure.
"""
import itertools, math, os, sys
import numpy as np
import trimesh
from trimesh.visual.material import PBRMaterial
from build123d import *
from params import *

PRINT = os.path.join(HERE, "print")
BUILD = os.path.join(HERE, "build")


# ── helpers ──
def cyl(x, y, z0, z1, d):
    return Pos(x, y, z0) * Cylinder(d / 2, z1 - z0, align=(Align.CENTER, Align.CENTER, Align.MIN))


def box(x0, x1, y0, y1, z0, z1):
    return Pos(x0, y0, z0) * Box(x1 - x0, y1 - y0, z1 - z0, align=(Align.MIN, Align.MIN, Align.MIN))


def tube(r0, r1, z0, z1, x=0.0, y=0.0):
    return cyl(x, y, z0, z1, 2 * r1) - cyl(x, y, z0 - OVS, z1 + OVS, 2 * r0)


def revolve_rz(pts):
    """Revolve a closed (r, z) outline about the z axis."""
    return revolve(make_face(Plane.XZ * Polyline(*pts, close=True)), Axis.Z)


def slot(x, y0, y1, d, z0, z1):
    return cyl(x, y0, z0, z1, d) + cyl(x, y1, z0, z1, d) + box(x - d / 2, x + d / 2, y0, y1, z0, z1)


def slot_x(x0, x1, d, z0, z1):
    """A slot along x from x0 to x1 (y = 0)."""
    s = cyl(x0, 0, z0, z1, d)
    if x1 > x0:
        s += cyl(x1, 0, z0, z1, d) + box(x0, x1, -d / 2, d / 2, z0, z1)
    return s


def polar(r, deg):
    a = math.radians(deg)
    return r * math.cos(a), r * math.sin(a)


def hexagon(x, y, af, z0, z1):
    return Pos(x, y, z0) * extrude(RegularPolygon(af / math.sqrt(3), 6), z1 - z0)


# ── printed parts (rig frame) ──
def ring():
    r_out = JOG_D / 2
    s = tube(RING_BORE / 2, r_out, TOP_Z0, TOP_Z1)
    s += tube(r_out - RIM_W, r_out, TOP_Z0 - RIM_DROP, TOP_Z0 + 0.5)
    s += tube(SKIRT_RI, SKIRT_R, SKIRT_Z0, TOP_Z0 + 0.5)
    # The V groove: the wheel's V, offset V_FLANK_CLR on each flank, with
    # V_TIP_CLR past its tip.  Along a wheel's radius the V's half-width at
    # ring radius r is TIP_W/2 + R_TIP - (WHEEL_R - r).
    hw = lambda r: WHEEL_TIP_W / 2 + WHEEL_R_TIP - (WHEEL_R - r) + V_FLANK_CLR
    r_root = WHEEL_R - WHEEL_R_TIP - V_TIP_CLR
    r_o = SKIRT_R + OVS
    s -= revolve_rz([(r_root, WHEEL_ZC - hw(r_root)), (r_o, WHEEL_ZC - hw(r_o)),
                     (r_o, WHEEL_ZC + hw(r_o)), (r_root, WHEEL_ZC + hw(r_root))])
    return s


def ring_edge():
    """The edge variant's ring: a track under the top for the supports, a
    plain skirt for the guides, and a lip under the guides."""
    r_out = JOG_D / 2
    s = tube(RING_BORE / 2, r_out, TOP_Z0, TOP_Z1)
    s += tube(r_out - RIM_W, r_out, TOP_Z0 - RIM_DROP, TOP_Z0 + 0.5)
    s += tube(SKIRT_RI, SKIRT_R, EDGE_SKIRT_Z0, TOP_Z0 + 0.5)
    s += tube(SKIRT_R - 0.5, LIP_R1, EDGE_SKIRT_Z0, EDGE_SKIRT_Z0 + LIP_T2)
    s += tube(TRACK_R - TRACK_W / 2, TRACK_R + TRACK_W / 2, TOP_Z0 - TRACK_H, TOP_Z0 + 0.5)
    return s


def ring_crossed():
    """The crossed variant's ring: a diamond ridge around the skirt, a 3 mm
    land on each flank for the tilted 608s."""
    r_out = JOG_D / 2
    s = tube(RING_BORE / 2, r_out, X_TOP_Z0, X_TOP_Z1)
    s += tube(r_out - RIM_W, r_out, X_TOP_Z0 - RIM_DROP, X_TOP_Z0 + 0.5)
    s += tube(SKIRT_RI, SKIRT_R, X_SKIRT_Z0, X_TOP_Z0 + 0.5)
    k = math.sqrt(0.5)
    pts = []
    for sgn in (-1, 1):            # lower flank, then upper (reversed below)
        base = (SKIRT_R, RIDGE_Z + sgn * RIDGE_H)
        mid = (SKIRT_R + RIDGE_H / 2, RIDGE_Z + sgn * RIDGE_H / 2)
        u = (k, -sgn * k)          # along the flank, base -> tip
        n = (k, sgn * k)           # out of the flank
        a = (mid[0] - LAND_W / 2 * u[0], mid[1] - LAND_W / 2 * u[1])
        b = (mid[0] + LAND_W / 2 * u[0], mid[1] + LAND_W / 2 * u[1])
        f = [(SKIRT_R - 0.5, base[1]), base, a, (a[0] + LAND_H * n[0], a[1] + LAND_H * n[1]),
             (b[0] + LAND_H * n[0], b[1] + LAND_H * n[1]), b]
        pts += f if sgn < 0 else f[::-1]
    pts.insert(6, (RIDGE_TIP_R, RIDGE_Z))
    s += revolve_rz(pts)
    return s


def x_frame(c, up):
    """Bearing-local frame of a crossed 608 (station at angle 0): its centre,
    its axis tilted 45° (outward-up for the lower row, inward-up for the upper)."""
    return Pos(c[0], 0, c[1]) * Rot(0, -45 if up else 45, 0)


def x_post(c, up, x0, x1, cap=None):
    """A post under a tilted 608: a block cut square to the axis, with the Ø11
    boss and the pin's hole."""
    f = x_frame(c, up)
    blk = box(x0, x1, -X_POST_W / 2, X_POST_W / 2, PLATE_TOP - 0.1, c[1] + 15)
    blk -= f * Pos(0, 0, -X_POST_FACE) * Box(200, 200, 100, align=(Align.CENTER, Align.CENTER, Align.MIN))
    blk += f * Pos(0, 0, -X_POST_FACE) * Cylinder(RACE_D / 2, 1.0, align=(Align.CENTER, Align.CENTER, Align.MIN))
    if cap is not None:
        blk -= box(x0 - OVS, x1 + OVS, -X_POST_W, X_POST_W, cap, cap + 100)
    blk -= f * Pos(0, 0, -X_POST_FACE - X_PIN_DEPTH) * Cylinder(PIN_FIT / 2, X_PIN_DEPTH + 1.0 + OVS, align=(Align.CENTER, Align.CENTER, Align.MIN))
    return blk


def holddown():
    """An upper (hold-down) post, at station angle 0: its flange's slots let it
    slide ± SLIDE radially; push it in until the play is gone, tighten."""
    r0, r1, hw, t = X_UP_FLANGE
    s = x_post(X_UP_C, True, *X_UP_BLOCK)
    s += box(r0, r1, -hw, hw, PLATE_TOP, PLATE_TOP + t)
    for x, y in X_UP_SCREWS:
        s -= Pos(0, y, 0) * slot_x(x - SLIDE, x + SLIDE, M3_CLR, PLATE_TOP - OVS, PLATE_TOP + t + OVS)
    return s - box(-200, 200, -200, 200, PLATE_TOP - 5, PLATE_TOP)     # it stands on the plate


def hub(disp="210", top=TOP_Z1):
    glass, window, thick = DISPLAYS[disp]
    TOP_Z1_ = top
    big = DISPLAYS["210"][0] + 2 * HUB_FIT
    s = tube(big / 2, HUB_OD / 2, PLATE_TOP, TOP_Z1_)
    s += tube(window / 2, big / 2 + 0.5, TOP_Z1_ - LIP_T, TOP_Z1_)
    s += tube(big / 2 - 0.5, HUB_FLANGE[0] / 2, PLATE_TOP, PLATE_TOP + HUB_FLANGE[1])
    if disp != "210":
        # A locating ring under the lip for the smaller module.
        z1 = TOP_Z1_ - LIP_T
        s += tube(glass / 2 + HUB_FIT, big / 2 + 0.5, z1 - thick, z1 + 0.2)
    for a in (0.0, 120.0, 240.0):
        x, y = polar(HUB_SCREW_R, a)
        s -= cyl(x, y, PLATE_TOP - OVS, PLATE_TOP + HUB_FLANGE[1] + OVS, M3_CLR)
    return s


def plate(variant="vwheel"):
    s = cyl(0, 0, 0, PLATE_T, PLATE_D)
    s -= cyl(0, 0, -OVS, PLATE_T + OVS, CABLE_HOLE_D)
    for a in (0.0, 120.0, 240.0):                 # hub screws: heat-set inserts
        x, y = polar(HUB_SCREW_R, a)
        s -= cyl(x, y, PLATE_TOP - INSERT[1], PLATE_TOP + OVS, INSERT[0])
    for a in WHEEL_ANGLES if variant == "vwheel" else ():
        x, y = polar(WHEEL_R, a)
        if a == ECC_WHEEL:                        # eccentric spacer: its shank in a hole, nut under a counterbore
            s += cyl(x, y, PLATE_TOP - 0.1, WHEEL_Z0 - ECC[1], 14.0)
            s -= cyl(x, y, -OVS, WHEEL_Z0, ECC[2])
            s -= cyl(x, y, -OVS, 2.0, 13.0)
        else:                                     # fixed: spacer on a boss, nut trapped from below
            s += cyl(x, y, PLATE_TOP - 0.1, WHEEL_Z0 - SPACER[1], 12.0)
            s -= cyl(x, y, -OVS, WHEEL_Z0, M5_CLR)
            s -= hexagon(x, y, M5_NUT_AF + 0.2, -OVS, M5_NUT_T)
    vert = {"608": [(a, TYRE_R, a == ECC_WHEEL) for a in WHEEL_ANGLES],
            "edge": [(a, GUIDE_R, a == EDGE_GUIDE_SLIDE) for a in EDGE_GUIDE_ANGLES]}.get(variant, [])
    for a, R, sliding in vert:
        # M8 up from below, its head in a counterbore; one in a radial slot,
        # so pushing its bearing in and tightening sets the preload.
        r0, r1 = (R - SLIDE, R + SLIDE) if sliding else (R, R)
        for d, z0, z1 in ((M8_CLR, -OVS, PLATE_T + OVS), (M8_HEAD[2], -OVS, CBORE)):
            s -= Rot(0, 0, a) * slot_x(r0, r1, d, z0, z1)
    for a in EDGE_SUP_ANGLES if variant == "edge" else ():
        # A support 608 on edge between two cheeks, on a pin pushed in from
        # outside; Ø11 bosses bear on its inner race only.
        c = box(SUP_R0, SUP_R0 + CHEEK_T, -CHEEK_W / 2, CHEEK_W / 2, PLATE_TOP - 0.1, CHEEK_Z1)
        c += box(SUP_R1 - CHEEK_T, SUP_R1, -CHEEK_W / 2, CHEEK_W / 2, PLATE_TOP - 0.1, CHEEK_Z1)
        for x0, x1 in ((SUP_R0 + CHEEK_T, SUP_R0 + CHEEK_T + SUP_BOSS), (SUP_R1 - CHEEK_T - SUP_BOSS, SUP_R1 - CHEEK_T)):
            c += Pos(x0, 0, SUP_ZC) * Rot(0, 90, 0) * Cylinder(RACE_D / 2, x1 - x0, align=(Align.CENTER, Align.CENTER, Align.MIN))
        c -= Pos(SUP_R0 - OVS, 0, SUP_ZC) * Rot(0, 90, 0) * Cylinder(PIN_FIT / 2, PIN[1] + 2 * OVS, align=(Align.CENTER, Align.CENTER, Align.MIN))
        s += Rot(0, 0, a) * c
    for a in X_LOW_ANGLES if variant == "crossed" else ():
        s += Rot(0, 0, a) * x_post(X_LOW_C, False, *X_LOW_BLOCK, cap=X_LOW_CAP)
    for a in X_UP_ANGLES if variant == "crossed" else ():
        for x, y in X_UP_SCREWS:
            px, py = x * math.cos(math.radians(a)) - y * math.sin(math.radians(a)), x * math.sin(math.radians(a)) + y * math.cos(math.radians(a))
            s -= cyl(px, py, PLATE_TOP - INSERT[1], PLATE_TOP + OVS, INSERT[0])
    # The roller arm: a pad under its foot (the arm floats 1 mm up), two
    # inserts, a recess under the AS5600 board and a hole for its wires.
    x0, x1, y0, y1 = FOOT
    s += box(x0, x1, y0, y1 + PRELOAD_MAX, PLATE_TOP - 0.1, ARM_Z0)
    for x, y in FOOT_SCREWS:
        s -= cyl(x, y, ARM_Z0 - INSERT[1], ARM_Z0 + OVS, INSERT[0])
    e = ENC_PCB[0] / 2 + 3.0
    s -= box(-e, e, ROLL_R - e, ROLL_R + e, PLATE_TOP - 2.0, PLATE_TOP + OVS)
    s -= cyl(0, ROLL_R, -OVS, PLATE_TOP, 8.0)
    return s


def arm(inward=False):
    """The roller's arm: the boss with the two bearings, the AS5600 under
    it, a flexure beam, and the slotted foot.  Drawn at the tyre's touch.
    The foot slides outward to press the tyre on (or inward, for the pioneer
    version's gear, which meshes from outside the ring)."""
    cx, cy = 0.0, ROLL_R
    pw = ENC_PCB[0] / 2
    blk_z1 = ARM_Z0 + ENC_PCB[2] + ENC_PARTS_H + 1.0
    s = box(cx - pw - 1.5, cx + pw + 1.5, cy - pw - 1.5, cy + pw + 1.5, ARM_Z0, blk_z1)
    s += cyl(cx, cy, ARM_Z0, BOSS_Z1, BOSS_D)
    s += box(RISER[1] - 0.5, cx - 2.0, BEAM_Y - BEAM_T / 2, BEAM_Y + BEAM_T / 2, *BEAM_Z)
    x0, x1, y0, y1 = FOOT
    s += box(x0, x1, y0, y1, ARM_Z0, FOOT_Z1)
    s += box(RISER[0], RISER[1], y0, y1, ARM_Z0, BEAM_Z[1])
    for x, y in FOOT_SCREWS:
        y0s, y1s = (y, y + PRELOAD_MAX) if inward else (y - PRELOAD_MAX, y)
        s -= slot(x, y0s, y1s, M3_CLR, ARM_Z0 - OVS, FOOT_Z1 + OVS)
    # The board slides in from below, chip up; its parts sit in a cavity.
    s -= box(cx - pw - 0.1, cx + pw + 0.1, cy - pw - 0.1, cy + pw + 0.1, ARM_Z0 - OVS, ARM_Z0 + ENC_PCB[2])
    q = pw - 1.5
    s -= box(cx - q, cx + q, cy - q, cy + q, ARM_Z0, ARM_Z0 + ENC_PCB[2] + ENC_PARTS_H)
    s -= cyl(cx, cy, ARM_Z0 + ENC_PCB[2], BRG1_Z0 + 0.1, CUP[0] + 1.0)      # the magnet cup turns in here
    s -= cyl(cx, cy, BRG1_Z0, BOSS_Z1 + OVS, BRG[0] + BRG_FIT)
    return s


def roller():
    z0, (hd, hh) = ROLLER_Z0, ROLLER_HUB
    s = cyl(0, ROLL_R, z0, z0 + hh, hd)
    zt = z0 + hh
    s += cyl(0, ROLL_R, zt, ROLLER_Z1, ROLL_ROOT_D + 1.5)
    s -= tube(ROLL_ROOT_D / 2, ROLL_ROOT_D, TYRE_ZC - ORING_CS / 2 - 0.1, TYRE_ZC + ORING_CS / 2 + 0.1, 0, ROLL_R)
    s -= cyl(0, ROLL_R, z0 - OVS, ROLLER_Z1 + OVS, AXLE_D)
    s -= Pos(0, ROLL_R, z0 + hh / 2) * Rot(0, 90, 0) * Cylinder(1.25, hd)   # M3 grub, thread-forming
    return s


def cup():
    z0 = MAG_Z0
    s = cyl(0, ROLL_R, z0, z0 + CUP[1], CUP[0])
    s -= cyl(0, ROLL_R, z0 - OVS, z0 + MAGNET[1], MAGNET[0] + 0.2)
    s -= cyl(0, ROLL_R, AXLE_Z0, z0 + CUP[1] + OVS, AXLE_D)
    return s


def tyre(bore=TYRE_BORE):
    """The 608's V tyre, at the origin: the V-wheel's V on a 608."""
    hw0 = WHEEL_TIP_W / 2 + WHEEL_V_H
    s = revolve_rz([(bore / 2, 0), (TYRE_R_CORE, 0), (TYRE_R_CORE, TYRE_W / 2 - hw0),
                    (TYRE_R_TIP, TYRE_W / 2 - WHEEL_TIP_W / 2), (TYRE_R_TIP, TYRE_W / 2 + WHEEL_TIP_W / 2),
                    (TYRE_R_CORE, TYRE_W / 2 + hw0), (TYRE_R_CORE, TYRE_W), (bore / 2, TYRE_W)])
    s += tube(TYRE_LIP[0] / 2, bore / 2 + 0.5, 0, TYRE_LIP[1])
    return s


def tyre_fit():
    """Three short rings, bores TYRE_BORE - 0.1 / +0 / +0.1, marked with 1 / 2 / 3
    notches: press a 608 into each and set TYRE_BORE to the snug one."""
    s = None
    for i, d in enumerate((TYRE_BORE - 0.1, TYRE_BORE, TYRE_BORE + 0.1)):
        x = i * 30.0
        r = tube(d / 2, d / 2 + 2.5, 0, 4.0, x, 0)
        for k in range(i + 1):
            r -= Pos(x + (k - i / 2) * 3.0, d / 2 + 2.5, 2.0) * Box(1.2, 2.0, 5.0)
        s = r if s is None else s + r
    s += box(0, 60, -1.5, 1.5, 0, 1.2)       # a tie bar, so it prints as one part
    return s


def spacer608(foot=16.0):
    """Under each 608: a foot on the plate, a Ø11 step on the inner race only."""
    s = cyl(0, 0, PLATE_TOP, TYRE_Z0 - 0.5, foot) + cyl(0, 0, TYRE_Z0 - 0.5, SPACER608_Z1, RACE_D)
    return s - cyl(0, 0, PLATE_TOP - OVS, SPACER608_Z1 + OVS, M8_CLR)


def race_spacer():
    z0 = B608_Z0 + B608[2]
    return tube(M8_CLR / 2, RACE_D / 2, z0, z0 + 1.0)


# name: (solid, print flipped?, how many); "vwheel/..." and "608/..." go in print/<variant>/
def printed():
    return {
        "ring": (ring(), True, 1),
        "vwheel/plate": (plate("vwheel"), False, 1),
        "608/plate": (plate("608"), False, 1),
        "608/tyre": (tyre(), False, 3),
        "608/tyre_fit": (tyre_fit(), False, 1),
        "608/spacer": (spacer608(), False, 3),
        "608/race_spacer": (race_spacer(), False, 3),
        "edge/ring": (ring_edge(), True, 1),
        "edge/plate": (plate("edge"), False, 1),
        "edge/spacer": (spacer608(GUIDE_FOOT_D), False, 3),
        "edge/race_spacer": (race_spacer(), False, 3),
        "crossed/ring": (ring_crossed(), True, 1),
        "crossed/plate": (plate("crossed"), False, 1),
        "crossed/holddown": (holddown(), False, 3),
        "crossed/hub_210": (hub("210", X_TOP_Z1), True, 1),
        "crossed/hub_128": (hub("128", X_TOP_Z1), True, 1),
        "hub_210": (hub("210"), True, 1),
        "hub_128": (hub("128"), True, 1),
        "arm": (arm(), False, 1),
        "roller": (roller(), True, 1),
        "magnet_cup": (cup(), False, 1),
    }


def bought(variant="vwheel"):
    s = []
    wheel = revolve_rz([(WHEEL_BORE / 2, 0), (WHEEL_R_CORE, 0),
                        (WHEEL_R_CORE, WHEEL_W / 2 - WHEEL_TIP_W / 2 - WHEEL_V_H),
                        (WHEEL_R_TIP, WHEEL_W / 2 - WHEEL_TIP_W / 2),
                        (WHEEL_R_TIP, WHEEL_W / 2 + WHEEL_TIP_W / 2),
                        (WHEEL_R_CORE, WHEEL_W / 2 + WHEEL_TIP_W / 2 + WHEEL_V_H),
                        (WHEEL_R_CORE, WHEEL_W), (WHEEL_BORE / 2, WHEEL_W)])
    for a in WHEEL_ANGLES if variant == "vwheel" else ():
        x, y = polar(WHEEL_R, a)
        n = int(a)
        s.append((f"wheel{n}", Pos(x, y, WHEEL_Z0) * wheel, "pom"))
        if a == ECC_WHEEL:
            s.append((f"ecc{n}", tube(WHEEL_BORE / 2, ECC[0] / 2, WHEEL_Z0 - ECC[1], WHEEL_Z0, x, y)
                      + tube(WHEEL_BORE / 2, ECC[2] / 2, 0.5, WHEEL_Z0 - ECC[1] + 0.1, x, y), "alu"))
        else:
            s.append((f"spacer{n}", tube(WHEEL_BORE / 2, SPACER[0] / 2, WHEEL_Z0 - SPACER[1], WHEEL_Z0, x, y), "alu"))
        zw = WHEEL_Z0 + WHEEL_W
        s.append((f"washer{n}", tube(WHEEL_BORE / 2, M5_WASHER[0] / 2, zw, zw + M5_WASHER[1], x, y), "steel"))
        zh = zw + M5_WASHER[1]
        s.append((f"bolt{n}", cyl(x, y, 0.5, zh, WHEEL_BORE) + cyl(x, y, zh, zh + M5_HEAD[1], M5_HEAD[0]), "steel"))
    for a in WHEEL_ANGLES if variant == "608" else ():
        x, y = polar(TYRE_R, a)
        n = int(a)
        s.append((f"tyre{n}", Pos(x, y, TYRE_Z0) * tyre(), "tyre"))
        s.append((f"b608_{n}", tube(B608[1] / 2, B608[0] / 2, B608_Z0, B608_Z0 + B608[2], x, y), "steel"))
        s.append((f"spacer{n}", Pos(x, y, 0) * spacer608(), "tyre"))
        s.append((f"race{n}", Pos(x, y, 0) * race_spacer(), "tyre"))
        s.append((f"nut{n}", tube(M8_CLR / 2, M8_JAM[0] / 2, NUT_Z0, NUT_Z0 + M8_JAM[1], x, y), "steel"))
        s.append((f"bolt{n}", cyl(x, y, CBORE - M8_HEAD[1], CBORE, M8_HEAD[0]) + cyl(x, y, CBORE, CBORE + 25.0, 8.0), "steel"))
    for a in EDGE_GUIDE_ANGLES if variant == "edge" else ():
        x, y = polar(GUIDE_R, a)
        n = int(a)
        s.append((f"guide{n}", tube(B608[1] / 2, B608[0] / 2, GUIDE_Z0, GUIDE_Z0 + B608[2], x, y), "steel"))
        s.append((f"spacer{n}", Pos(x, y, 0) * spacer608(GUIDE_FOOT_D), "tyre"))
        s.append((f"race{n}", Pos(x, y, 0) * race_spacer(), "tyre"))
        s.append((f"nut{n}", tube(M8_CLR / 2, M8_JAM[0] / 2, NUT_Z0, NUT_Z0 + M8_JAM[1], x, y), "steel"))
        s.append((f"bolt{n}", cyl(x, y, CBORE - M8_HEAD[1], CBORE, M8_HEAD[0]) + cyl(x, y, CBORE, CBORE + 25.0, 8.0), "steel"))
    for a in EDGE_SUP_ANGLES if variant == "edge" else ():
        n = int(a)
        rot = Rot(0, 0, a)
        b = Pos(TRACK_R - B608[2] / 2, 0, SUP_ZC) * Rot(0, 90, 0) * (
            Cylinder(B608[0] / 2, B608[2], align=(Align.CENTER, Align.CENTER, Align.MIN))
            - Cylinder(B608[1] / 2, B608[2], align=(Align.CENTER, Align.CENTER, Align.MIN)))
        s.append((f"support{n}", rot * b, "steel"))
        pin = Pos(SUP_R0, 0, SUP_ZC) * Rot(0, 90, 0) * Cylinder(PIN_FIT / 2, PIN[1], align=(Align.CENTER, Align.CENTER, Align.MIN))
        s.append((f"pin{n}", rot * pin, "steel"))
    if variant == "crossed":
        b = Cylinder(B608[0] / 2, B608[2]) - Cylinder(B608[1] / 2, B608[2])
        pin = Pos(0, 0, B608[2] / 2 - X_ROD[1]) * Cylinder(PIN_FIT / 2, X_ROD[1], align=(Align.CENTER, Align.CENTER, Align.MIN))
        for angles, c, up in ((X_LOW_ANGLES, X_LOW_C, False), (X_UP_ANGLES, X_UP_C, True)):
            for a in angles:
                n = int(a)
                f = Rot(0, 0, a) * x_frame(c, up)
                s.append((f"x608_{n}", f * b, "steel"))
                s.append((f"xpin{n}", f * pin, "steel"))
                if up:
                    s.append((f"holddown{n}", Rot(0, 0, a) * holddown(), "arm"))
    s += sense_stack()
    glass, window, thick = DISPLAYS["210"]
    z1 = (X_TOP_Z1 if variant == "crossed" else TOP_Z1) - LIP_T
    s.append(("display", cyl(0, 0, z1 - thick, z1 - 0.05, glass), "black"))
    return s


def sense_stack(oring=True):
    """The bought parts of the sense arm, at ROLL_R on +y."""
    s = []
    for i in range(2):
        z = BRG1_Z0 + i * BRG[2]
        s.append((f"mr105_{i}", tube(BRG[1] / 2, BRG[0] / 2, z, z + BRG[2], 0, ROLL_R), "steel"))
    s.append(("axle", cyl(0, ROLL_R, AXLE_Z0, AXLE_Z1, AXLE_D), "steel"))
    s.append(("magnet", cyl(0, ROLL_R, MAG_Z0, MAG_Z0 + MAGNET[1], MAGNET[0]), "magnet"))
    pw = ENC_PCB[0] / 2
    s.append(("as5600", box(-pw, pw, ROLL_R - pw, ROLL_R + pw, ARM_Z0, ARM_Z0 + ENC_PCB[2]), "blue"))
    s.append(("as5600_chip", box(-2.5, 2.5, ROLL_R - 3, ROLL_R + 3, ARM_Z0 + ENC_PCB[2], CHIP_TOP), "black"))
    if oring:
        s.append(("oring", Pos(0, ROLL_R, TYRE_ZC) * Torus((ROLL_ROOT_D + ORING_CS) / 2, ORING_CS / 2), "rubber"))
    return s


# ── assembly, checks, export ──
MAT = {
    "ring": ((0.045, 0.047, 0.05, 1), 0.0, 0.35), "plate": ((0.10, 0.10, 0.11, 1), 0.0, 0.7),
    "hub": ((0.16, 0.165, 0.17, 1), 0.0, 0.5), "arm": ((0.85, 0.42, 0.05, 1), 0.0, 0.5),
    "pom": ((0.9, 0.9, 0.88, 1), 0.0, 0.4), "alu": ((0.75, 0.76, 0.78, 1), 1.0, 0.3),
    "steel": ((0.62, 0.63, 0.65, 1), 1.0, 0.25), "blue": ((0.05, 0.15, 0.55, 1), 0.0, 0.5),
    "black": ((0.02, 0.02, 0.022, 1), 0.0, 0.2), "rubber": ((0.03, 0.03, 0.03, 1), 0.0, 0.8),
    "magnet": ((0.4, 0.4, 0.42, 1), 1.0, 0.4), "glass": ((0.85, 0.9, 0.95, 0.25), 0.0, 0.05), "tyre": ((0.10, 0.45, 0.20, 1), 0.0, 0.5),
}
KEY = {"ring": "ring", "edge/ring": "ring", "crossed/ring": "ring", "crossed/plate": "plate", "crossed/hub_210": "hub", "vwheel/plate": "plate", "608/plate": "plate", "edge/plate": "plate", "hub_210": "hub", "arm": "arm", "roller": "arm", "magnet_cup": "arm"}


def mesh(solid, path):
    export_stl(solid, path, tolerance=0.02, angular_tolerance=0.08)
    return trimesh.load(path)


def scene(items):
    sc = trimesh.Scene()
    for name, m, key in items:
        m = m.copy()
        rgba, metal, rough = MAT[key]
        m.visual = trimesh.visual.TextureVisuals(material=PBRMaterial(
            name=key, baseColorFactor=rgba, metallicFactor=metal, roughnessFactor=rough, doubleSided=True))
        sc.add_geometry(m, geom_name=name)
    sc.apply_transform(np.diag([1e-3, 1e-3, 1e-3, 1.0]))
    sc.apply_transform(trimesh.transformations.rotation_matrix(-np.pi / 2, [1, 0, 0]))
    return sc


SPRUNG = {"pinion", "oring", "roller", "arm", "magnet_cup", "axle", "magnet", "as5600", "as5600_chip", "mr105_0", "mr105_1"}


def held(variant, items):
    """Nudge the ring 0.4 mm along each axis and tilt it 0.5° each way: every
    nudge must run it into a bearing (or the lip into one).  The sprung roller
    doesn't count.  Returns the number of free directions."""
    ring = next(m for n, m, _ in items if n.endswith("ring"))
    fixed = [(n, m) for n, m, _ in items if not n.endswith("ring") and n.split("/")[-1] not in SPRUNG and "hub" not in n and n not in ("display", "window")]   # the window is glued to the ring
    T = trimesh.transformations
    moves = {f"{s}{ax}": T.translation_matrix(np.eye(3)[i] * (0.4 if s == "+" else -0.4))
             for i, ax in enumerate("xyz") for s in "+-"}
    moves |= {f"tilt{s}{ax}": T.rotation_matrix(math.radians(0.5 if s == "+" else -0.5), np.eye(3)[i])
              for i, ax in enumerate("xy") for s in "+-"}
    free, stops = [], {}
    for k, t in moves.items():
        r = ring.copy(); r.apply_transform(t)
        hit = None
        for n, m in fixed:
            lo = np.maximum(r.bounds[0], m.bounds[0]); hi = np.minimum(r.bounds[1], m.bounds[1])
            if np.any(lo >= hi):
                continue
            if trimesh.boolean.intersection([r, m], engine="manifold").volume > 0.01:
                hit = n; break
        if hit is None:
            free.append(k)
        stops[k] = hit
    print(f"held {variant}: " + ", ".join(f"{k}->{v}" for k, v in stops.items()))
    if free:
        print(f"FREE [{variant}]: the ring can move {free}")
    return len(free)


# The tyre's press fit on the 608, as a volume.
PRESS_FIT = math.pi * ((B608[0] / 2) ** 2 - (TYRE_BORE / 2) ** 2) * (TYRE_W - TYRE_LIP[1])


def main():
    os.makedirs(PRINT, exist_ok=True)
    tmp = os.path.join(BUILD, "parts")
    import shutil
    shutil.rmtree(tmp, ignore_errors=True)       # no stale parts from an older layout
    os.makedirs(tmp, exist_ok=True)
    bad = []
    import pioneer, mini
    parts = printed() | pioneer.printed() | mini.printed()
    for name, (solid, flip, n) in parts.items():
        s = Rot(180, 0, 0) * solid if flip else solid
        bb = s.bounding_box()
        path = os.path.join(PRINT, f"{name}.stl")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        export_stl(Pos(-bb.min.X, -bb.min.Y, -bb.min.Z) * s, path)
        m = trimesh.load(path)
        ext = m.bounds[1] - m.bounds[0]
        bodies = len(m.split(only_watertight=False))
        ok = m.is_watertight and bodies == 1 and ext[0] <= BED[0] and ext[1] <= BED[1]
        print(f"{'ok ' if ok else 'BAD'} {name:17s} x{n}  {ext.round(1)}  watertight={m.is_watertight} bodies={bodies}")
        if not ok:
            bad.append(name)

    shared = {n: mesh(s, os.path.join(tmp, f"{n.replace('/', '_')}.stl")) for n, (s, _, _) in parts.items()
              if n in KEY and not n.endswith("plate") and not n.endswith("ring") and "hub" not in n}
    clashes = 0
    for variant in ("vwheel", "608", "edge", "crossed", "pioneer", "mini", "mini_mr105"):
        if variant == "pioneer":
            items = [(n, mesh(s, os.path.join(tmp, f"{variant}_{n.replace('/', '_')}.stl")), k) for n, s, k in pioneer.assembly()]
        elif variant.startswith("mini"):
            bore = "mr105" if variant == "mini_mr105" else "623"
            items = [(n, mesh(s, os.path.join(tmp, f"{variant}_{n.replace('/', '_')}.stl")), k) for n, s, k in mini.assembly(bore)]
        else:
            items = [(n, m, KEY[n]) for n, m in shared.items()]
            for part in ("plate", "ring", "hub_210"):
                pn = f"{variant}/{part}" if f"{variant}/{part}" in parts else part
                items.append((pn, mesh(parts[pn][0], os.path.join(tmp, f"{variant}_{part}.stl")), KEY[pn]))
            items += [(n, mesh(s, os.path.join(tmp, f"{variant}_{n}.stl")), k) for n, s, k in bought(variant)]
        c = 0
        for (a, ma, _), (b, mb, _) in itertools.combinations(items, 2):
            lo = np.maximum(ma.bounds[0], mb.bounds[0]); hi = np.minimum(ma.bounds[1], mb.bounds[1])
            if np.any(lo >= hi):
                continue
            v = trimesh.boolean.intersection([ma, mb], engine="manifold").volume
            if a.startswith("tyre") and b == "b608_" + a[4:]:
                v -= PRESS_FIT     # the tyre is pressed on: exactly that overlap is meant
            if v > 1.0:
                c += 1
                print(f"CLASH [{variant}] {a} x {b}: {v:.1f} mm3")
        print(f"assembly {variant}: {len(items)} parts, {c} clashes")
        c += held(variant, items)
        if variant == "pioneer":
            c += pioneer.encoder_check(next(m for n, m, _ in items if n == "pioneer/ring"))
        if variant == "mini":
            c += mini.encoder_check(next(m for n, m, _ in items if n == "mini/ring"))
            print(mini.snap_report())
        clashes += c
        scene(items).export(os.path.join(BUILD, f"jogring_{variant}.glb"))
        cut = []
        for n, m, k in items:      # cut on the plane through the roller and the 270° wheel (x = 0), keep x < 0
            if m.bounds[1][0] <= 0:
                cut.append((n, m, k)); continue
            if m.bounds[0][0] >= 0:
                continue
            cm = m.slice_plane([0, 0, 0], [-1, 0, 0], cap=True)
            if len(cm.faces):
                cut.append((n, cm, k))
        scene(cut).export(os.path.join(BUILD, f"jogring_{variant}_cutaway.glb"))
        scene([i for i in items if not i[0].endswith(("ring", "retainer", "window"))]).export(os.path.join(BUILD, f"jogring_{variant}_noring.glb"))
    # The small hub has to hold the 1.28" module too.
    m128 = mesh(hub("128"), os.path.join(tmp, "hub_128.stl"))
    g, _, t = DISPLAYS["128"]
    z1 = TOP_Z1 - LIP_T
    d128 = mesh(cyl(0, 0, z1 - t, z1 - 0.05, g), os.path.join(tmp, "display128.stl"))
    if trimesh.boolean.intersection([m128, d128], engine="manifold").volume > 1.0:
        clashes += 1
        print("CLASH hub_128 x the 1.28\" module")
    print(f"roller: {RATIO:.2f} turns per platter turn = {4096 * RATIO:.0f} counts/turn; "
          f"beam {BEAM_K:.2f} N/mm, so {PRELOAD_MAX} mm of preload is {BEAM_K * PRELOAD_MAX:.1f} N; axle {AXLE_L:.1f} mm")
    if bad or clashes:
        sys.exit(1)


if __name__ == "__main__":
    main()
