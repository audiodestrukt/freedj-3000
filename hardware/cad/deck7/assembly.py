"""assembly.py — the full-size deck assembled, plus cutaway and exploded scenes.

    python3 hardware/cad/deck7/assembly.py   ->  build/deck7*.glb

Adapted from deck5's: four top and four base pieces, the KEYS list, the 7"
display with its adapter board, the screen image from deck5's assets.

Printed parts come from their modules (one source of truth in params.py);
the bought parts (Pi 5 + Active Cooler, the 7" display, the fader,
608 bearings, shaft, AS5600 board) are simple stand-ins at their datasheet
sizes, placed from the same parameters.  Exports, in metres, Y-up:

  deck7.glb           the deck, assembled
  deck7_cutaway.glb   cut on the jog's axis (x = JOG_CX), looking in from +x
  deck7_exploded.glb  layers lifted apart, the top split at its seams
  deck7_seam.glb      the four top pieces pulled apart
"""
import os, sys
import numpy as np
import trimesh
from PIL import Image
from trimesh.visual.material import PBRMaterial
from build123d import *
from params import *
import top_shell, base, jog, caps, keycap
from top_shell import box, cyl

BUILD = os.path.join(HERE, "build")

# name: (rgba, metallic, roughness)
MAT = {
    "shell":   ((0.055, 0.058, 0.064, 1), 0.0, 0.55),   # matte charcoal PLA
    "base":    ((0.10, 0.10, 0.11, 1), 0.0, 0.7),
    "platter": ((0.045, 0.047, 0.05, 1), 0.0, 0.35),
    "cap":     ((0.16, 0.165, 0.17, 1), 0.0, 0.5),
    "play":    ((0.10, 0.45, 0.20, 1), 0.0, 0.4),
    "cue":     ((0.85, 0.42, 0.05, 1), 0.0, 0.4),
    "knob":    ((0.80, 0.80, 0.78, 1), 0.0, 0.45),
    "pcb":     ((0.05, 0.30, 0.15, 1), 0.0, 0.5),
    "black":   ((0.02, 0.02, 0.022, 1), 0.0, 0.4),
    "alu":     ((0.75, 0.76, 0.78, 1), 1.0, 0.3),
    "steel":   ((0.62, 0.63, 0.65, 1), 1.0, 0.25),
    "blue":    ((0.05, 0.15, 0.55, 1), 0.0, 0.5),
    "clamp":   ((0.25, 0.25, 0.27, 1), 0.0, 0.6),
}


def bought():
    """Stand-ins for the parts we buy, world frame."""
    s = []
    # Raspberry Pi 5: board, Active Cooler (heatsink + fan), USB / Ethernet.
    x0, y0, z0 = PI_X0, PI_Y0, PI_Z0
    s.append(("pi_board", box(x0, x0 + PI[0], y0, y0 + PI[1], z0, z0 + PI[2]), "pcb"))
    zc = z0 + PI[2]
    s.append(("pi_cooler", box(x0 + 4, x0 + PI[0] - 4, y0 + 12, y0 + 64, zc + 1.5, zc + PI_COOLER_H), "alu"))
    s.append(("pi_fan", cyl(x0 + PI[0] / 2 - 6, y0 + 40, zc + PI_COOLER_H, zc + PI_COOLER_H + 0.5, 30.0), "black"))
    ye = y0 + PI[1] + PI_PORT_OVER
    s.append(("pi_eth", box(x0 + 2, x0 + 18, ye - 21, ye, zc, zc + 13.5), "alu"))
    for xu in (x0 + 21, x0 + 39):
        s.append((f"pi_usb{int(xu)}", box(xu, xu + 14.5, ye - 17.5, ye, zc, zc + PI_PORT_H), "alu"))
    # Display: glass + board (the lit face is a textured quad, below).
    ow, oh = SCR_OUT
    s.append(("display", box(SCR_CX - ow / 2, SCR_CX + ow / 2, SCR_CY - oh / 2, SCR_CY + oh / 2,
                             SCR_Z_BOT, SCR_Z_TOP - 0.05), "black"))
    bx, by, bz = SCR_BACK     # the adapter board and its standoffs, under the metal frame
    s.append(("display_back", box(SCR_CX - bx / 2, SCR_CX + bx / 2, SCR_CY - by / 2, SCR_CY + by / 2,
                                  SCR_Z_BOT - bz, SCR_Z_BOT - 0.05), "pcb"))
    # Fader: body under the plate, lever up through the slot (centred).
    fx, fl, fh = FADER_BODY
    s.append(("fader", box(FADER_X - fx / 2, FADER_X + fx / 2, FADER_CY - fl / 2, FADER_CY + fl / 2,
                           FADER_Z_TOP - fh, FADER_Z_TOP), "alu"))
    lx, ly, lh = FADER_LEVER
    s.append(("fader_lever", box(FADER_X - lx / 2, FADER_X + lx / 2, FADER_CY - ly / 2, FADER_CY + ly / 2,
                                 FADER_Z_TOP, FADER_Z_TOP + lh), "steel"))
    # Jog bearings, shaft, AS5600 board.
    for z in (BRG_UP_Z, BRG_LO_Z):
        s.append((f"bearing{int(z)}", cyl(JOG_CX, JOG_CY, z, z + BRG[2], BRG[0]) -
                  cyl(JOG_CX, JOG_CY, z - 1, z + BRG[2] + 1, BRG[1]), "steel"))
    s.append(("shaft", cyl(JOG_CX, JOG_CY, SHAFT_Z0, SHAFT_Z1, SHAFT_D), "steel"))
    # A brushed aluminium label disc in the platter's centre recess (bought
    # or turned; a silk-PLA print stands in until then).
    s.append(("jog_label", cyl(JOG_CX, JOG_CY, JOG_Z0 + JOG_H - jog.REC_DEPTH, JOG_Z0 + JOG_H - 0.2,
                               jog.REC_D - 1.0), "alu"))
    ex, ey, et = ENC_PCB      # in its cage under the boss, chip up under the shaft's magnet
    s.append(("as5600", box(JOG_CX - ex / 2, JOG_CX + ex / 2, JOG_CY - ey / 2, JOG_CY + ey / 2, ENC_Z0, ENC_Z0 + et), "blue"))
    s.append(("as5600_chip", box(JOG_CX - 2.5, JOG_CX + 2.5, JOG_CY - 3.0, JOG_CY + 3.0, ENC_Z0 + et, ENC_Z0 + et + ENC_CHIP_H), "black"))
    # Shaft collar under the lower bearing; M8 washer between the platter's hub and the upper bearing.
    cz = BRG_LO_Z - 0.5 - COLLAR[1]
    s.append(("collar", cyl(JOG_CX, JOG_CY, cz, cz + COLLAR[1], COLLAR[0]) - cyl(JOG_CX, JOG_CY, cz - 1, cz + COLLAR[1] + 1, SHAFT_D), "steel"))
    wz = BRG_UP_Z + BRG[2]
    s.append(("washer", cyl(JOG_CX, JOG_CY, wz, wz + WASHER_T, 16.0) - cyl(JOG_CX, JOG_CY, wz - 1, wz + WASHER_T + 1, 8.4), "steel"))
    return s


def printed():
    s = [(f"top_{p}", t, "shell") for p, t in top_shell.parts().items()]
    s += [(f"base_{p}", b, "base") for p, b in base.parts().items()]
    s.append(("jog_platter", jog.place_mesh(jog.platter(), jog.MATES["platter_axis"], jog.MATES["jog_world_platter"]), "platter"))
    s.append(("cap_play", Pos(*PLAY, H) * caps.big_cap(), "play"))
    s.append(("cap_cue", Pos(*CUE, H) * caps.big_cap(), "cue"))
    for x, y, name, size in KEYS:
        s.append((f"cap_{name}", Pos(x, y, H) * keycap.key_cap(size), "cap"))
    s.append(("knob_fader", Pos(FADER_X, FADER_CY, H) * caps.fader_knob(), "knob"))
    s.append(("knob_encoder", Pos(*ENC, H) * caps.enc_knob(), "cap"))
    s.append(("screen_frame", Pos(SCR_CX, SCR_CY, SCR_Z_BOT - FRAME_T) * caps.screen_frame(), "clamp"))
    return s


def screen_face(dz=0.0):
    """The lit display: a textured quad over the active area (glass top)."""
    aw, ah = SCR_AA
    x0, x1, y0, y1, z = SCR_CX - aw / 2, SCR_CX + aw / 2, SCR_CY - ah / 2, SCR_CY + ah / 2, SCR_Z_TOP + dz
    v = np.array([[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]])
    m = trimesh.Trimesh(v, [[0, 1, 2], [0, 2, 3]], process=False)
    img = Image.open(os.path.join(DECK5, "assets", "screen.png")).convert("RGB")
    mat = PBRMaterial(name="screen", baseColorTexture=img, emissiveTexture=img, emissiveFactor=[1.0, 1.0, 1.0],
                      metallicFactor=0.0, roughnessFactor=0.15)
    m.visual = trimesh.visual.TextureVisuals(uv=np.array([[0, 0], [1, 0], [1, 1], [0, 1]], float), material=mat)
    return m


def mesh(solid, path):
    if isinstance(solid, trimesh.Trimesh):      # a mesh part (the platter)
        solid.export(path)
        return solid
    export_stl(solid, path, tolerance=0.02, angular_tolerance=0.08)
    return trimesh.load(path)


def material(key):
    rgba, metal, rough = MAT[key]
    return PBRMaterial(name=key, baseColorFactor=rgba, metallicFactor=metal, roughnessFactor=rough, doubleSided=True)


def to_scene(items, extra=()):
    sc = trimesh.Scene()
    for name, m, key in items:
        m = m.copy()
        m.visual = trimesh.visual.TextureVisuals(material=material(key))
        sc.add_geometry(m, geom_name=name)
    for name, m in extra:
        sc.add_geometry(m, geom_name=name)
    sc.apply_transform(np.diag([1e-3, 1e-3, 1e-3, 1.0]))                         # mm -> m
    sc.apply_transform(trimesh.transformations.rotation_matrix(-np.pi / 2, [1, 0, 0]))   # Z-up -> Y-up
    return sc


EXPLODE = {   # z lift per part (mm); the top halves also part along y
    "base_": 0, "jog_tower": 25, "as5600": 12, "pi_": 25,
    "display_back": 80, "display": 95, "screen_frame": 70, "fader": 120, "fader_bracket": 105, "bearing": 60,
    "shaft": 45, "top_": 120, "cap_": 165, "knob_": 175, "jog_platter": 210, "jog_label": 225,
}
SPLIT = {"top_fl": (-14.0, -18.0), "top_fr": (14.0, -18.0), "top_bl": (-14.0, 18.0), "top_br": (14.0, 18.0)}


def lift(name):
    for k, v in EXPLODE.items():
        if name.startswith(k):
            return v
    return 0


def main():
    os.makedirs(BUILD, exist_ok=True)
    tmp = os.path.join(BUILD, "parts")
    os.makedirs(tmp, exist_ok=True)
    items = []
    print("manifest:")
    for name, solid, key in printed() + bought():
        m = mesh(solid, os.path.join(tmp, f"{name}.stl"))
        items.append((name, m, key))
        ext = (m.bounds[1] - m.bounds[0]).round(1)
        print(f"  {name:16s} {key:8s} {ext}")
    lo = np.min([m.bounds[0] for _, m, _ in items], axis=0)
    hi = np.max([m.bounds[1] for _, m, _ in items], axis=0)
    print("assembly bbox:", (hi - lo).round(1), "from", lo.round(1))
    assert hi[0] - lo[0] < W + 1 and hi[1] - lo[1] < D + 1, "a part is outside the deck"

    to_scene(items, [("screen_face", screen_face())]).export(os.path.join(BUILD, f"{NAME}.glb"))

    # Cutaway on the jog's axis: keep x < JOG_CX, capped so the section reads solid.
    cut = []
    for name, m, key in items:
        if m.bounds[1][0] <= JOG_CX:
            cut.append((name, m, key)); continue
        if m.bounds[0][0] >= JOG_CX:
            continue
        c = m.slice_plane([JOG_CX, 0, 0], [-1, 0, 0], cap=True)
        if len(c.faces):
            cut.append((name, c, key))
    face = screen_face().slice_plane([JOG_CX, 0, 0], [-1, 0, 0])
    to_scene(cut, [("screen_face", face)]).export(os.path.join(BUILD, f"{NAME}_cutaway.glb"))

    # Exploded.
    ex = []
    for name, m, key in items:
        m = m.copy()
        dx, dy = SPLIT.get(name, (0.0, 0.0))
        m.apply_translation([dx, dy, lift(name)])
        ex.append((name, m, key))
    to_scene(ex, [("screen_face", screen_face(EXPLODE["display"]))]).export(os.path.join(BUILD, f"{NAME}_exploded.glb"))

    # The seams: just the four top pieces, pulled apart to show the tongues and grooves.
    seam = []
    for name, m, key in items:
        if name in SPLIT:
            dx, dy = SPLIT[name]
            m = m.copy(); m.apply_translation([dx * 2.5, dy * 2.0, 0])
            seam.append((name, m, key))
    to_scene(seam).export(os.path.join(BUILD, f"{NAME}_seam.glb"))

    # Quick geometry preview (OpenSCAD) of the assembled deck.
    scad = os.path.join(BUILD, f"{NAME}_preview.scad")
    with open(scad, "w") as f:
        for name, m, key in items:
            r, g, b, a = MAT[key][0]
            f.write(f'color([{r},{g},{b}]) import("parts/{name}.stl");\n')
    print(f"wrote {NAME}.glb, {NAME}_cutaway.glb, {NAME}_exploded.glb, {NAME}_seam.glb")


if __name__ == "__main__":
    main()
