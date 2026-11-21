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


def hub(disp="210"):
    glass, window, thick = DISPLAYS[disp]
    big = DISPLAYS["210"][0] + 2 * HUB_FIT
    s = tube(big / 2, HUB_OD / 2, PLATE_TOP, TOP_Z1)
    s += tube(window / 2, big / 2 + 0.5, TOP_Z1 - LIP_T, TOP_Z1)
    s += tube(big / 2 - 0.5, HUB_FLANGE[0] / 2, PLATE_TOP, PLATE_TOP + HUB_FLANGE[1])
    if disp != "210":
        # A locating ring under the lip for the smaller module.
        z1 = TOP_Z1 - LIP_T
        s += tube(glass / 2 + HUB_FIT, big / 2 + 0.5, z1 - thick, z1 + 0.2)
    for a in (0.0, 120.0, 240.0):
        x, y = polar(HUB_SCREW_R, a)
        s -= cyl(x, y, PLATE_TOP - OVS, PLATE_TOP + HUB_FLANGE[1] + OVS, M3_CLR)
    return s


def plate():
    s = cyl(0, 0, 0, PLATE_T, PLATE_D)
    s -= cyl(0, 0, -OVS, PLATE_T + OVS, CABLE_HOLE_D)
    for a in (0.0, 120.0, 240.0):                 # hub screws: heat-set inserts
        x, y = polar(HUB_SCREW_R, a)
        s -= cyl(x, y, PLATE_TOP - INSERT[1], PLATE_TOP + OVS, INSERT[0])
    for a in WHEEL_ANGLES:
        x, y = polar(WHEEL_R, a)
        if a == ECC_WHEEL:                        # eccentric spacer: its shank in a hole, nut under a counterbore
            s += cyl(x, y, PLATE_TOP - 0.1, WHEEL_Z0 - ECC[1], 14.0)
            s -= cyl(x, y, -OVS, WHEEL_Z0, ECC[2])
            s -= cyl(x, y, -OVS, 2.0, 13.0)
        else:                                     # fixed: spacer on a boss, nut trapped from below
            s += cyl(x, y, PLATE_TOP - 0.1, WHEEL_Z0 - SPACER[1], 12.0)
            s -= cyl(x, y, -OVS, WHEEL_Z0, M5_CLR)
            s -= hexagon(x, y, M5_NUT_AF + 0.2, -OVS, M5_NUT_T)
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


def arm():
    """The roller's arm: the boss with the two bearings, the AS5600 under
    it, a flexure beam, and the slotted foot.  Drawn at the tyre's touch."""
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
        s -= slot(x, y - PRELOAD_MAX, y, M3_CLR, ARM_Z0 - OVS, FOOT_Z1 + OVS)
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


# name: (solid, print flipped?, how many)
def printed():
    return {
        "ring": (ring(), True, 1),
        "plate": (plate(), False, 1),
        "hub_210": (hub("210"), True, 1),
        "hub_128": (hub("128"), True, 1),
        "arm": (arm(), False, 1),
        "roller": (roller(), True, 1),
        "magnet_cup": (cup(), False, 1),
    }


def bought():
    s = []
    wheel = revolve_rz([(WHEEL_BORE / 2, 0), (WHEEL_R_CORE, 0),
                        (WHEEL_R_CORE, WHEEL_W / 2 - WHEEL_TIP_W / 2 - WHEEL_V_H),
                        (WHEEL_R_TIP, WHEEL_W / 2 - WHEEL_TIP_W / 2),
                        (WHEEL_R_TIP, WHEEL_W / 2 + WHEEL_TIP_W / 2),
                        (WHEEL_R_CORE, WHEEL_W / 2 + WHEEL_TIP_W / 2 + WHEEL_V_H),
                        (WHEEL_R_CORE, WHEEL_W), (WHEEL_BORE / 2, WHEEL_W)])
    for a in WHEEL_ANGLES:
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
    for i in range(2):
        z = BRG1_Z0 + i * BRG[2]
        s.append((f"mr105_{i}", tube(BRG[1] / 2, BRG[0] / 2, z, z + BRG[2], 0, ROLL_R), "steel"))
    s.append(("axle", cyl(0, ROLL_R, AXLE_Z0, AXLE_Z1, AXLE_D), "steel"))
    s.append(("magnet", cyl(0, ROLL_R, MAG_Z0, MAG_Z0 + MAGNET[1], MAGNET[0]), "magnet"))
    pw = ENC_PCB[0] / 2
    s.append(("as5600", box(-pw, pw, ROLL_R - pw, ROLL_R + pw, ARM_Z0, ARM_Z0 + ENC_PCB[2]), "blue"))
    s.append(("as5600_chip", box(-2.5, 2.5, ROLL_R - 3, ROLL_R + 3, ARM_Z0 + ENC_PCB[2], CHIP_TOP), "black"))
    oring = Pos(0, ROLL_R, TYRE_ZC) * Torus((ROLL_ROOT_D + ORING_CS) / 2, ORING_CS / 2)
    s.append(("oring", oring, "rubber"))
    glass, window, thick = DISPLAYS["210"]
    z1 = TOP_Z1 - LIP_T
    s.append(("display", cyl(0, 0, z1 - thick, z1 - 0.05, glass), "black"))
    return s


# ── assembly, checks, export ──
MAT = {
    "ring": ((0.045, 0.047, 0.05, 1), 0.0, 0.35), "plate": ((0.10, 0.10, 0.11, 1), 0.0, 0.7),
    "hub": ((0.16, 0.165, 0.17, 1), 0.0, 0.5), "arm": ((0.85, 0.42, 0.05, 1), 0.0, 0.5),
    "pom": ((0.9, 0.9, 0.88, 1), 0.0, 0.4), "alu": ((0.75, 0.76, 0.78, 1), 1.0, 0.3),
    "steel": ((0.62, 0.63, 0.65, 1), 1.0, 0.25), "blue": ((0.05, 0.15, 0.55, 1), 0.0, 0.5),
    "black": ((0.02, 0.02, 0.022, 1), 0.0, 0.2), "rubber": ((0.03, 0.03, 0.03, 1), 0.0, 0.8),
    "magnet": ((0.4, 0.4, 0.42, 1), 1.0, 0.4),
}
KEY = {"ring": "ring", "plate": "plate", "hub_210": "hub", "arm": "arm", "roller": "arm", "magnet_cup": "arm"}


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


def main():
    os.makedirs(PRINT, exist_ok=True)
    tmp = os.path.join(BUILD, "parts")
    os.makedirs(tmp, exist_ok=True)
    bad = []
    parts = printed()
    for name, (solid, flip, n) in parts.items():
        s = Rot(180, 0, 0) * solid if flip else solid
        bb = s.bounding_box()
        path = os.path.join(PRINT, f"{name}.stl")
        export_stl(Pos(-bb.min.X, -bb.min.Y, -bb.min.Z) * s, path)
        m = trimesh.load(path)
        ext = m.bounds[1] - m.bounds[0]
        bodies = len(m.split(only_watertight=False))
        ok = m.is_watertight and bodies == 1 and ext[0] <= BED[0] and ext[1] <= BED[1]
        print(f"{'ok ' if ok else 'BAD'} {name:11s} x{n}  {ext.round(1)}  watertight={m.is_watertight} bodies={bodies}")
        if not ok:
            bad.append(name)

    items = [(n, mesh(s, os.path.join(tmp, f"{n}.stl")), KEY[n]) for n, (s, _, _) in parts.items() if n != "hub_128"]
    items += [(n, mesh(s, os.path.join(tmp, f"{n}.stl")), k) for n, s, k in bought()]
    clashes = 0
    for (a, ma, _), (b, mb, _) in itertools.combinations(items, 2):
        lo = np.maximum(ma.bounds[0], mb.bounds[0]); hi = np.minimum(ma.bounds[1], mb.bounds[1])
        if np.any(lo >= hi):
            continue
        v = trimesh.boolean.intersection([ma, mb], engine="manifold").volume
        if v > 1.0:
            clashes += 1
            print(f"CLASH {a} x {b}: {v:.1f} mm3")
    # The small hub has to hold the 1.28" module too.
    m128 = mesh(hub("128"), os.path.join(tmp, "hub_128.stl"))
    g, _, t = DISPLAYS["128"]
    z1 = TOP_Z1 - LIP_T
    d128 = mesh(cyl(0, 0, z1 - t, z1 - 0.05, g), os.path.join(tmp, "display128.stl"))
    if trimesh.boolean.intersection([m128, d128], engine="manifold").volume > 1.0:
        clashes += 1
        print("CLASH hub_128 x the 1.28\" module")
    print(f"assembly: {len(items)} parts, {clashes} clashes")
    print(f"roller: {RATIO:.2f} turns per platter turn = {4096 * RATIO:.0f} counts/turn; "
          f"beam {BEAM_K:.2f} N/mm, so {PRELOAD_MAX} mm of preload is {BEAM_K * PRELOAD_MAX:.1f} N; axle {AXLE_L:.1f} mm")

    scene(items).export(os.path.join(BUILD, "jogring.glb"))
    cut = []
    for n, m, k in items:          # cut on the plane through the roller and the 270° wheel (x = 0), keep x < 0
        if m.bounds[1][0] <= 0:
            cut.append((n, m, k)); continue
        if m.bounds[0][0] >= 0:
            continue
        c = m.slice_plane([0, 0, 0], [-1, 0, 0], cap=True)
        if len(c.faces):
            cut.append((n, c, k))
    scene(cut).export(os.path.join(BUILD, "jogring_cutaway.glb"))
    if bad or clashes:
        sys.exit(1)


if __name__ == "__main__":
    main()
