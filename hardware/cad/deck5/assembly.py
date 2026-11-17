"""assembly.py — the 5" deck assembled, plus cutaway and exploded scenes.

    python3 hardware/cad/deck5/assembly.py   ->  build/deck5*.glb, build/deck5_preview.png

Printed parts come from their modules (one source of truth in params.py);
the bought parts (Pi 5 + Active Cooler, the 5" DSI display, the fader,
608 bearings, shaft, AS5600 board) are simple stand-ins at their datasheet
sizes, placed from the same parameters.  Exports, in metres, Y-up:

  deck5.glb           the deck, assembled
  deck5_cutaway.glb   cut on the jog's axis (x = JOG_CX), looking in from +x
  deck5_exploded.glb  layers lifted apart, the top split at its seam
"""
import os, sys
import numpy as np
import trimesh
from PIL import Image
from trimesh.visual.material import PBRMaterial
from build123d import *
from params import *
import top_shell, base, jog, caps
from top_shell import box, cyl

HERE = os.path.dirname(os.path.abspath(__file__))
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
    # Display module: back (PCB + frame) and the lit face (textured, below).
    ow, oh = SCR_OUT
    s.append(("display", box(SCR_CX - ow / 2, SCR_CX + ow / 2, SCR_CY - oh / 2, SCR_CY + oh / 2,
                             SCR_Z_BOT, SCR_Z_TOP - 0.05), "black"))
    # Fader: body under the plate, lever up through the slot (centred).
    fx, fl, fh = FADER_BODY
    s.append(("fader", box(FADER_X - fx / 2, FADER_X + fx / 2, FADER_CY - fl / 2, FADER_CY + fl / 2,
                           Z_UNDER - fh, Z_UNDER), "alu"))
    lx, ly, lh = FADER_LEVER
    s.append(("fader_lever", box(FADER_X - lx / 2, FADER_X + lx / 2, FADER_CY - ly / 2, FADER_CY + ly / 2,
                                 Z_UNDER, Z_UNDER + lh), "steel"))
    # Jog bearings, shaft, AS5600 board.
    for z in (BRG_UP_Z, BRG_LO_Z):
        s.append((f"bearing{int(z)}", cyl(JOG_CX, JOG_CY, z, z + BRG[2], BRG[0]) -
                  cyl(JOG_CX, JOG_CY, z - 1, z + BRG[2] + 1, BRG[1]), "steel"))
    s.append(("shaft", cyl(JOG_CX, JOG_CY, SHAFT_Z0, SHAFT_Z1, SHAFT_D), "steel"))
    # A brushed aluminium label disc in the platter's centre recess (bought
    # or turned; a silk-PLA print stands in until then).
    s.append(("jog_label", cyl(JOG_CX, JOG_CY, JOG_Z0 + JOG_H - jog.REC_DEPTH, JOG_Z0 + JOG_H - 0.2,
                               jog.REC_D - 1.0), "alu"))
    s.append(("as5600", box(JOG_CX - 11.5, JOG_CX + 11.5, JOG_CY - 11.5, JOG_CY + 11.5, BASE_T, BASE_T + 1.6), "blue"))
    return s


def printed():
    tf, tb = top_shell.parts()
    bf, bb = base.parts()
    s = [("top_front", tf, "shell"), ("top_back", tb, "shell"),
         ("base_front", bf, "base"), ("base_back", bb, "base")]
    s.append(("jog_platter", place(jog.platter(), jog.MATES["platter_axis"], jog.MATES["jog_world_platter"]), "platter"))
    s.append(("jog_tower", place(jog.tower(), jog.MATES["tower_foot"], jog.MATES["jog_world_tower"]), "base"))
    s.append(("cap_play", Pos(*PLAY, H) * caps.big_cap(), "play"))
    s.append(("cap_cue", Pos(*CUE, H) * caps.big_cap(), "cue"))
    for i, x in enumerate(KEY_XS):
        s.append((f"cap_key{i}", Pos(x, KEY_Y, H) * caps.key_cap(), "cap"))
    s.append(("knob_fader", Pos(FADER_X, FADER_CY, H) * caps.fader_knob(), "knob"))
    s.append(("knob_encoder", Pos(*ENC, H) * caps.enc_knob(), "cap"))
    for sy in (-1, 1):
        s.append((f"screen_clamp{sy}", Pos(SCR_CX, SCR_CY + sy * SCR_BOSS[1], SCR_Z_BOT - CLAMP_T) * caps.clamp_strap(), "clamp"))
    return s


def screen_face(dz=0.0):
    """The lit display: a textured quad over the active area (glass top)."""
    aw, ah = SCR_AA
    x0, x1, y0, y1, z = SCR_CX - aw / 2, SCR_CX + aw / 2, SCR_CY - ah / 2, SCR_CY + ah / 2, SCR_Z_TOP + dz
    v = np.array([[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]])
    m = trimesh.Trimesh(v, [[0, 1, 2], [0, 2, 3]], process=False)
    img = Image.open(os.path.join(HERE, "assets", "screen.png")).convert("RGB")
    mat = PBRMaterial(name="screen", baseColorTexture=img, emissiveTexture=img, emissiveFactor=[1.0, 1.0, 1.0],
                      metallicFactor=0.0, roughnessFactor=0.15)
    m.visual = trimesh.visual.TextureVisuals(uv=np.array([[0, 0], [1, 0], [1, 1], [0, 1]], float), material=mat)
    return m


def mesh(solid, path):
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
    "base_front": 0, "base_back": 0, "jog_tower": 25, "as5600": 12, "pi_": 25,
    "display": 95, "screen_clamp": 70, "fader": 120, "bearing": 60, "shaft": 45,
    "top_front": 120, "top_back": 120, "cap_": 165, "knob_": 175, "jog_platter": 210, "jog_label": 225,
}
SPLIT_Y = {"top_front": -18.0, "top_back": 18.0}


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

    to_scene(items, [("screen_face", screen_face())]).export(os.path.join(BUILD, "deck5.glb"))

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
    to_scene(cut, [("screen_face", face)]).export(os.path.join(BUILD, "deck5_cutaway.glb"))

    # Exploded.
    ex = []
    for name, m, key in items:
        m = m.copy()
        m.apply_translation([0, SPLIT_Y.get(name, 0.0), lift(name)])
        ex.append((name, m, key))
    to_scene(ex, [("screen_face", screen_face(EXPLODE["display"]))]).export(os.path.join(BUILD, "deck5_exploded.glb"))

    # The seam: just the two top pieces, pulled apart to show the tongue and groove.
    seam = []
    for name, m, key in items:
        if name in ("top_front", "top_back"):
            m = m.copy(); m.apply_translation([0, -30.0 if name == "top_front" else 30.0, 0])
            seam.append((name, m, key))
    to_scene(seam).export(os.path.join(BUILD, "deck5_seam.glb"))

    # Quick geometry preview (OpenSCAD) of the assembled deck.
    scad = os.path.join(BUILD, "deck5_preview.scad")
    with open(scad, "w") as f:
        for name, m, key in items:
            r, g, b, a = MAT[key][0]
            f.write(f'color([{r},{g},{b}]) import("parts/{name}.stl");\n')
    print("wrote deck5.glb, deck5_cutaway.glb, deck5_exploded.glb")


if __name__ == "__main__":
    main()
