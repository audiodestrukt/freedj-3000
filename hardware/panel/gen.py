#!/usr/bin/env python3
"""gen.py — layout.toml → index.circuit.tsx for the ADK-1000 single panel board.

The face is described in millimetres from its top-left corner (y down).  The
tscircuit board has its origin at the centre with y up, so every position is
converted here once.  Everything electrical is generated from the control
list: a tact switch (+ LED and resistor) per button, PCF8574 expanders in
groups of eight pins placed near the controls they serve, the RP2350-Zero hub
module, the MT6701 under the jog shaft, the browse encoder, the fader slot,
the display cutout and the Pi mounting pattern.

    python3 gen.py            # writes index.circuit.tsx
"""
import math
import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
L = tomllib.loads((HERE / "layout.toml").read_text())

FACE_W, FACE_H = L["face"]["width_mm"], L["face"]["height_mm"]
MARGIN = L["face"].get("board_margin_mm", 4.0)
BOARD_W, BOARD_H = FACE_W - 2 * MARGIN, FACE_H - 2 * MARGIN


def P(x, y):
    """face mm (top-left, y down) → tscircuit mm (centre, y up)"""
    return round(x - FACE_W / 2, 2), round(FACE_H / 2 - y, 2)


out = []
traces = []
nets_used = set()


def emit(s):
    out.append("    " + s)


def trace(a, b):
    traces.append(f'    <trace from="{a}" to="{b}" />')


def text(s, x, y, size=1.2):
    px, py = P(x, y)
    emit(f'<silkscreentext text="{s}" pcbX={{{px}}} pcbY={{{py}}} fontSize={{{size}}} />')


# ── PCF8574 pin pool ──────────────────────────────────────────────────────────
# Each entry: (kind, control name, net).  Eight per chip, chips placed at the
# centroid of their group so the traces stay short.
pool = []            # [(net, face_x, face_y)]

controls = L["control"]
buttons = [c for c in controls if c["kind"] == "button" and c["name"] != "jog_touch"]
buttons.sort(key=lambda c: (c["y"] // 60, c["x"]))   # band by rows of ~60 mm, then left→right

# ── buttons ───────────────────────────────────────────────────────────────────
for c in buttons:
    n = c["name"]
    px, py = P(c["x"], c["y"])
    emit(f'<pushbutton name="SW_{n}" footprint="pushbutton" pcbX={{{px}}} pcbY={{{py}}} />')
    # tscircuit's 6x6 "pushbutton" footprint pairs pins 1-2 (left side) and
    # 3-4 (right side) internally; the contact is between the sides.  Wiring
    # 1 and 2 to different nets would short them.  Tie each side explicitly.
    trace(f"SW_{n}.pin1", f"net.BTN_{n}"); trace(f"SW_{n}.pin2", f"net.BTN_{n}")
    trace(f"SW_{n}.pin3", "net.GND");      trace(f"SW_{n}.pin4", "net.GND")
    pool.append((f"BTN_{n}", c["x"], c["y"]))
    label = n.upper().replace("_", " ")
    if c.get("status") == "guess":
        label += " VERIFY"
    text(label, c["x"], c["y"] - 5.5, 1.0)
    # cap outline on the silkscreen so the printed cap can be checked against the board
    cap = c.get("cap_mm", 0)
    if cap:
        w, h = (cap, cap) if isinstance(cap, (int, float)) else cap
        emit(f'<silkscreenrect pcbX={{{px}}} pcbY={{{py}}} width="{w}mm" height="{h}mm" isFilled={{false}} />')
    if c.get("led"):
        dx, dy = c.get("led_offset_mm", [6.0, 0.0])
        lx, ly = P(c["x"] + dx, c["y"] + dy)
        rx, ry = P(c["x"] + dx + 4.5, c["y"] + dy)
        emit(f'<led name="LED_{n}" footprint="0805" color="{c["led"]}" pcbX={{{lx}}} pcbY={{{ly}}} />')
        emit(f'<resistor name="RL_{n}" resistance="1k" footprint="0603" pcbX={{{rx}}} pcbY={{{ry}}} />')
        trace(f"LED_{n}.cathode", f"net.LED_{n}")
        trace(f"LED_{n}.anode", f"RL_{n}.pin1")
        trace(f"RL_{n}.pin2", "net.V3V3")
        pool.append((f"LED_{n}", c["x"] + dx, c["y"] + dy))

# ── expanders: eight pins each, hard-wired addresses 0x20 + i ─────────────────
n_chips = math.ceil(len(pool) / 8)
switch_xy = [(c["x"], c["y"]) for c in buttons]
for i in range(n_chips):
    group = pool[i * 8:(i + 1) * 8]
    cx = sum(g[1] for g in group) / len(group)
    cy = sum(g[2] for g in group) / len(group) + 14.0   # below the row of buttons it serves
    # keep the chip body clear of any switch footprint
    for _ in range(6):
        if all(math.hypot(cx - sx, cy - sy) > 13 for sx, sy in switch_xy):
            break
        cy += 8
    px, py = P(cx, cy)
    u = f"U_EXP{i}"
    emit(f'<PCF8574T_3_518 name="{u}" pcbX={{{px}}} pcbY={{{py}}} />')
    dpx, dpy = P(cx + 8.5, cy)
    emit(f'<capacitor name="C_{u}" capacitance="100nF" footprint="0603" pcbX={{{dpx}}} pcbY={{{dpy}}} />')
    trace(f"{u}.VDD", "net.V3V3"); trace(f"{u}.VSS", "net.GND")
    trace(f"C_{u}.pin1", "net.V3V3"); trace(f"C_{u}.pin2", "net.GND")
    trace(f"{u}.SDA", "net.SDA"); trace(f"{u}.SCL", "net.SCL"); trace(f"{u}.INT", "net.PANEL_INT")
    for bit, a in enumerate(("A0", "A1", "A2")):
        trace(f"{u}.{a}", "net.V3V3" if (i >> bit) & 1 else "net.GND")
    for k, (net, _, _) in enumerate(group):
        trace(f"{u}.P{k}", f"net.{net}")
    text(f"EXP{i} 0x{0x20 + i:02X}", cx, cy - 7, 0.9)

# ── hub: RP2350-Zero module (castellated; modelled as two 9-pin rows) ─────────
hub = L["hub"]
hx, hy = P(hub["x"], hub["y"])
rot = hub.get("rotation", 0)
# Pin order per Waveshare RP2040/RP2350-Zero, from the USB-C end.  VERIFY against
# the wiki before fabrication.
left = ["V5", "GND", "V3V3", "GP29", "GP28", "GP27", "GP26", "GP15", "GP14"]
right = ["GP0", "GP1", "GP2", "GP3", "GP4", "GP5", "GP6", "GP7", "GP8"]
holes = []
labels = {}
for k in range(9):
    y = (4 - k) * 2.54
    for side, names, x in (("L", left, -7.62), ("R", right, 7.62)):
        pin = len(holes) + 1
        holes.append(f'<platedhole portHints={{["pin{pin}"]}} pcbX="{x}mm" pcbY="{y}mm" shape="circle" holeDiameter="1.0mm" outerDiameter="1.7mm" />')
        labels[f"pin{pin}"] = names[k]
lab = ", ".join(f'{k}: ["{v}"]' for k, v in labels.items())
emit(f'<chip name="U_HUB" pcbX={{{hx}}} pcbY={{{hy}}} pcbRotation={{{rot}}} pinLabels={{{{ {lab} }}}} footprint={{<footprint>{"".join(holes)}</footprint>}} />')
emit(f'<silkscreenrect pcbX={{{hx}}} pcbY={{{hy}}} width="18mm" height="23.5mm" isFilled={{false}} />')
text("RP2350-ZERO  USB-C ↑  VERIFY PINOUT", hub["x"], hub["y"] + 15, 1.0)
trace("U_HUB.V3V3", "net.V3V3"); trace("U_HUB.GND", "net.GND"); trace("U_HUB.V5", "net.VBUS")
trace("U_HUB.GP0", "net.SDA"); trace("U_HUB.GP1", "net.SCL"); trace("U_HUB.GP2", "net.PANEL_INT")
trace("U_HUB.GP3", "net.JOG_A"); trace("U_HUB.GP4", "net.JOG_B"); trace("U_HUB.GP5", "net.JOG_Z"); trace("U_HUB.GP6", "net.JOG_TOUCH")
trace("U_HUB.GP7", "net.BR_A"); trace("U_HUB.GP8", "net.BR_B"); trace("U_HUB.GP14", "net.BR_SW")
trace("U_HUB.GP26", "net.FADER")
# bus pull-ups beside the hub
for k, (name, net, val) in enumerate((("R_SDA", "SDA", "4.7k"), ("R_SCL", "SCL", "4.7k"), ("R_INT", "PANEL_INT", "10k"), ("R_JT", "JOG_TOUCH", "10k"))):
    rx, ry = P(hub["x"] + 14, hub["y"] - 8 + 3 * k)
    emit(f'<resistor name="{name}" resistance="{val}" footprint="0603" pcbX={{{rx}}} pcbY={{{ry}}} />')
    trace(f"{name}.pin1", f"net.{net}"); trace(f"{name}.pin2", "net.V3V3")

# ── jog: MT6701 under the shaft, bearing bolt holes, touch-switch header ──────
jog = next(c for c in controls if c["kind"] == "jog")
jx, jy = P(jog["x"], jog["y"])
emit(f'<MT6701CT_STD_R name="U_JOG" pcbX={{{jx}}} pcbY={{{jy}}} />')
cx, cy = P(jog["x"] + 7, jog["y"])
emit(f'<capacitor name="C_JOG" capacitance="100nF" footprint="0603" pcbX={{{cx}}} pcbY={{{cy}}} />')
trace("U_JOG.VDD", "net.V3V3"); trace("U_JOG.GND", "net.GND"); trace("U_JOG.MODE", "net.GND")
trace("C_JOG.pin1", "net.V3V3"); trace("C_JOG.pin2", "net.GND")
trace("U_JOG.A", "net.JOG_A"); trace("U_JOG.B", "net.JOG_B"); trace("U_JOG.Z", "net.JOG_Z")
r = jog["bearing_bolt_circle_mm"] / 2
for k in range(4):
    a = math.radians(45 + 90 * k)
    bx, by = P(jog["x"] + r * math.cos(a), jog["y"] + r * math.sin(a))
    emit(f'<hole diameter="{jog["bearing_bolt_mm"]}mm" pcbX={{{bx}}} pcbY={{{by}}} />')
emit(f'<silkscreencircle pcbX={{{jx}}} pcbY={{{jy}}} radius="{jog["platter_mm"] / 2}mm" />')
text("JOG  MT6701 UNDER SHAFT  MAGNET GAP 1-2mm", jog["x"], jog["y"] + 22, 1.0)
jt = next(c for c in controls if c["name"] == "jog_touch")
tx, ty = P(jt["x"], jt["y"])
emit(f'<pinheader name="J_TOUCH" pinCount={{2}} footprint="pinrow2" pcbX={{{tx}}} pcbY={{{ty}}} pinLabels={{{{ pin1: "SW", pin2: "GND" }}}} />')
trace("J_TOUCH.SW", "net.JOG_TOUCH"); trace("J_TOUCH.GND", "net.GND")

# ── browse encoder ────────────────────────────────────────────────────────────
br = next(c for c in controls if c["kind"] == "encoder")
bx, by = P(br["x"], br["y"])
emit(f'<EC11E18244A5 name="ENC_BROWSE" pcbX={{{bx}}} pcbY={{{by}}} />')
emit(f'<silkscreencircle pcbX={{{bx}}} pcbY={{{by}}} radius="{br["knob_mm"] / 2}mm" />')
trace("ENC_BROWSE.A", "net.BR_A"); trace("ENC_BROWSE.B", "net.BR_B"); trace("ENC_BROWSE.C", "net.GND")
trace("ENC_BROWSE.pin6", "net.BR_SW"); trace("ENC_BROWSE.pin7", "net.GND")
for k, net in enumerate(("BR_A", "BR_B", "BR_SW")):
    rx, ry = P(br["x"] - 14, br["y"] - 4 + 4 * k)
    cx, cy = P(br["x"] - 18, br["y"] - 4 + 4 * k)
    emit(f'<resistor name="R_{net}" resistance="10k" footprint="0603" pcbX={{{rx}}} pcbY={{{ry}}} />')
    emit(f'<capacitor name="C_{net}" capacitance="100nF" footprint="0603" pcbX={{{cx}}} pcbY={{{cy}}} />')
    trace(f"R_{net}.pin1", f"net.{net}"); trace(f"R_{net}.pin2", "net.V3V3")
    trace(f"C_{net}.pin1", f"net.{net}"); trace(f"C_{net}.pin2", "net.GND")

# ── fader: slot + 3-pin pot + RC at the hub side ──────────────────────────────
fd = next(c for c in controls if c["kind"] == "fader")
sw, sh = fd["slot_mm"]
fx, fy = P(fd["x"], fd["y"])
emit(f'<cutout shape="rect" width="{sw}mm" height="{sh}mm" pcbX={{{fx}}} pcbY={{{fy}}} />')
px, py = P(fd["x"] + 6, fd["y"] + sh / 2 + 6)
emit(f'<potentiometer name="P_TEMPO" maxResistance="10k" footprint="pinrow3" pcbRotation={{90}} pcbX={{{px}}} pcbY={{{py}}} />')
for dy in (-sh / 2 - 6, sh / 2 + 6):
    hx2, hy2 = P(fd["x"], fd["y"] + dy)
    emit(f'<hole diameter="3.2mm" pcbX={{{hx2}}} pcbY={{{hy2}}} />')
trace("P_TEMPO.pin1", "net.V3V3"); trace("P_TEMPO.pin3", "net.GND"); trace("P_TEMPO.pin2", "R_FADER.pin1")
rx, ry = P(fd["x"] + 6, fd["y"] + sh / 2 + 14)
cx, cy = P(fd["x"] + 6, fd["y"] + sh / 2 + 18)
emit(f'<resistor name="R_FADER" resistance="1k" footprint="0603" pcbX={{{rx}}} pcbY={{{ry}}} />')
emit(f'<capacitor name="C_FADER" capacitance="100nF" footprint="0603" pcbX={{{cx}}} pcbY={{{cy}}} />')
trace("R_FADER.pin2", "net.FADER"); trace("C_FADER.pin1", "net.FADER"); trace("C_FADER.pin2", "net.GND")
text("TEMPO  100mm SLIDE POT  VERIFY PART", fd["x"] - 12, fd["y"], 1.0)

# ── display cutout, Pi mount ──────────────────────────────────────────────────
d = L["display"]
m = d.get("cutout_margin_mm", 1.0)
dx, dy = P(d["x"] + d["width"] / 2, d["y"] + d["height"] / 2)
emit(f'<cutout shape="rect" width="{d["width"] + 2 * m}mm" height="{d["height"] + 2 * m}mm" pcbX={{{dx}}} pcbY={{{dy}}} />')
pi = L["pi"]
for ox, oy in ((0, 0), (58, 0), (0, 49), (58, 49)):
    hx3, hy3 = P(pi["x"] + ox, pi["y"] + oy)
    emit(f'<hole diameter="2.7mm" pcbX={{{hx3}}} pcbY={{{hy3}}} />')
text("PI 5 MOUNT VERIFY", pi["x"] + 29, pi["y"] - 4, 1.0)

# ── write ─────────────────────────────────────────────────────────────────────
src = f'''/**
 * ADK-1000 panel board — GENERATED by gen.py from layout.toml; edit those, not
 * this.  One flat board, routing disabled (TraceMaker routes the export).
 */
import React from "react"
import {{ PCF8574T_3_518 }} from "./imports/PCF8574T_3_518"
import {{ MT6701CT_STD_R }} from "./imports/MT6701CT_STD_R"
import {{ EC11E18244A5 }} from "./imports/EC11E18244A5"

export default () => (
  <board width="{BOARD_W:.1f}mm" height="{BOARD_H:.1f}mm" routingDisabled>
{chr(10).join(out)}

{chr(10).join(traces)}
    <copperpour connectsTo="net.GND" layer="bottom" />
  </board>
)
'''
(HERE / "index.circuit.tsx").write_text(src)
print(f"index.circuit.tsx: board {BOARD_W:.1f} x {BOARD_H:.1f} mm, {len(buttons)} buttons, {len(pool)} expander pins on {n_chips} PCF8574s, {len(traces)} traces")
