# Design: the ADK-1000 control board

Status: **design in progress** (2026-10-24). Nothing has been fabricated.

**Decision 2026-10-24: one board for the whole face.** Dan chose a single
panel PCB over a hub plus cluster boards, so there are no board-to-board
cables. That board lives in `hardware/panel/` and is *generated*: every control
is a line in `layout.toml` (position in mm from the face's top-left corner,
cap size, LED colour, provenance), and `gen.py` emits the tscircuit source.
On it:

- the hub is a **Waveshare RP2350-Zero module** (castellated, USB-C on the
  module, ~$4) rather than the bare RP2350A below, because a one-shot
  298 × 380 mm board should not carry the only hard routing problem; the bare
  chip section is kept here as the v2 / modular-rig variant;
- the jog is sensed by an **MT6701** (C3003196, SOP-8, 14-bit magnetic angle,
  ABZ quadrature out) under the platter shaft with a diametric magnet in the
  shaft end, four M3 bearing bolt holes around it; the ABZ pins land on the
  same GPIO the hub below uses, so firmware and the MIDI contract are
  unchanged;
- buttons are 6 × 6 tact switches under printed caps, LEDs 0805 beside them,
  on **PCF8574T** expanders (C7605) in groups of eight, addresses 0x20 up,
  placed near the buttons they serve and kept out of the cutouts;
- browse is an **EC11E18244A5** (C255515) with push, the tempo fader a 100 mm
  slide pot over a slot, the 7" display a cutout, the Pi 5 bolts to the back
  under the jog.

**What is still provisional:** the 13 photo-measured positions are good to a
few mm; the other controls on the XDJ face (track/search, sync, master, tempo
range, time mode, auto cue, shortcut, USB stop) are placed by eye and say
VERIFY on the silkscreen. A caliper pass on the real unit, or a straight-down
photo with a ruler, turns them into `status = "measured"` and nothing else
changes. The RP2350-Zero pin order and the fader part are also marked VERIFY.

The rest of this document describes the hub electronics, which are the same
on both boards. Companion documents: [faceplate and hardware](faceplate-and-hardware.md)
(the panel this board sits behind), [control surfaces](../hardware/control-surfaces.md)
(how the app maps any controller, this one included).

## What it is

One small board, the **hub**, that owns everything on the panel that has to be
sampled fast or analog, and presents the whole panel to the computer as **one
class-compliant USB MIDI device**. Slow things (buttons, lit buttons) stay on
cheap PCF8574 I2C panel modules that plug into the hub.

```
 jog encoder ──┐                                   ┌── USB-C ──► Pi 5 / iPad / laptop
 top-plate sw ─┤                                   │             (one USB MIDI device)
 pitch fader ──┤   RP2350A hub board               │
 browse knob ──┤   PIO quadrature, ADC, I2C master ┤
               │                                   │
 PCF8574 panel modules (buttons + LEDs) ═══ I2C ═══┘
   module 1 ─ module 2 ─ module 3 … (daisy chain, addresses 0x20–0x27)
```

Why this split and not the alternatives is argued in the
[control surfaces](../hardware/control-surfaces.md) discussion and summarised here:

- **Fast things on the hub.** A 206 mm platter on a 600 PPR encoder produces
  ~7,000 quadrature counts/s in a scratch throw, with edges ~140 µs apart.
  That is PIO territory; neither Linux GPIO nor an I2C expander polled from
  Linux can see it. The pitch fader is analog and the Pi has no ADC.
- **Slow things on I2C.** A button changes state a few times a second. A
  PCF8574 module costs a couple of dollars, needs no firmware, and the app
  already speaks to it both directly from the Pi header and, with this board,
  through the hub. The collaborator's existing modules plug in unchanged.
- **USB MIDI, not I2C, to the computer.** Class-compliant MIDI needs no driver,
  works on the iPad build through the same USB-C hub as the ethernet dongle
  (midir is built unconditionally, CoreMIDI on iOS), and makes the board useful
  to Mixxx and Traktor users, which is why other people will build it. The
  device is our own: a pid.codes vendor ID and our own note map, never a clone
  of a Pioneer or Numark product ID.
- **Not** the RP2350 as an I2C slave on the Pi bus: it keeps Linux polling,
  loses the iPad and the Mixxx audience, and saves one cable.

## Block diagram and connectors

| Block | Parts | Connector (2.54 mm, friction-lock) |
|---|---|---|
| Power | USB-C receptacle (TYPE-C-31-M-12, C165948), 5.1 k CC pull-downs, USBLC6-2SC6 ESD on D+/D− (C7519), AMS1117-3.3 LDO (C6186), 10 µF / 22 µF | — |
| MCU | RP2350A (C42411118), W25Q16JVSNIQ QSPI flash (C2456211), 12 MHz crystal (X322512MSB4SI, C9002) + 2 × 15 pF, 3.3 µH core-buck inductor, IOVDD decoupling at every pin, BOOT and RESET tact switches | SWD/debug: 3V3, GND, SWCLK, SWDIO, TX, RX |
| Jog | 74LVC2G17 dual Schmitt buffer (5 V-tolerant inputs → clean 3.3 V edges), 4.7 k pull-ups on A/B so open-collector *and* push-pull encoders both work | **J_JOG** 6 p: 5V, GND, A, B, Z, TOUCH |
| Fader | 1 k + 100 nF RC into ADC0; AUX pot on ADC1 for a second analog control | **J_FADER** 3 p: 3V3, WIPER, GND; **J_AUX** 3 p |
| Browse | EC11 encoder + push, 10 k pull-ups, 100 nF debounce | **J_BROWSE** 5 p: GND, A, B, SW, 3V3 (or the EC11 soldered on the hub) |
| Panel bus | I2C0 at 100 kHz with 4.7 k pull-ups, PCF8574 INT line | **J_PANEL** 5 p: 3V3, GND, SDA, SCL, INT (two in parallel for chaining) |
| Spare | 8 GPIOs straight out, for a button matrix or a second encoder later | **J_GPIO** 10 p: 3V3, GND, GPIO11–18 |
| Status | one LED on GPIO10 | — |

Everything runs from USB VBUS. Budget: RP2350 ~50 mA, encoder 40–80 mA (fed
5 V because industrial encoders want it), PCF8574 modules with LEDs limited by
their own resistors to ~3 mA per LED. Stay under 500 mA so a plain USB 2 port
is enough; the LED budget per module is the module designer's problem and is
written into the panel-module notes below.

## GPIO assignment

| GPIO | Function | Notes |
|---|---|---|
| 0, 1 | I2C0 SDA, SCL | panel modules |
| 2 | PANEL_INT | PCF8574 INT, open-drain, pull-up on the hub |
| 3, 4 | JOG_A, JOG_B | through the Schmitt buffer; PIO quadrature decoder |
| 5 | JOG_Z | index pulse, optional, for an absolute platter angle |
| 6 | JOG_TOUCH | the top-plate switch (mechanical on the XDJ) |
| 7, 8, 9 | BROWSE_A, BROWSE_B, BROWSE_SW | |
| 10 | STATUS_LED | |
| 11–18 | spare, on J_GPIO | |
| 20, 21 | UART0 TX, RX | debug header |
| 26 (ADC0) | FADER | |
| 27 (ADC1) | AUX | second pot or a tempo-range switch ladder |
| 28, 29 | free | |

## What the firmware sends

A single USB MIDI port named `ADK-1000`. Channel 0. The note and CC numbers
below are the preset `crates/app/surfaces/adk1000.toml` once the board exists;
until then they are the contract the firmware is written to.

| Control | Message | Detail |
|---|---|---|
| Jog | CC 0x10 relative, `twos_complement` | counts since the last report, one report per 1 ms USB frame while moving; the host times arrivals for velocity. A second CC with MCU-measured velocity is added only if nudge feels steppy. |
| Jog touch | note 0x20 on/off | the top-plate switch |
| Pitch fader | CC 0x0D / 0x2D 14-bit | 12-bit ADC oversampled ×4, centre detent snapped by the host |
| AUX | CC 0x0E / 0x2E 14-bit | |
| Browse | CC 0x1A relative, note 0x21 push | |
| Panel buttons | notes 0x30 + (module × 8 + pin) | module order = I2C address order |
| Panel LEDs | notes 0x30 + (module × 8 + pin) *in* | the host's `[[surface.led]]` entries; velocity ≥ 64 lights the pin |

Encoder velocity at nudge speeds: the encoder ticks every 100–150 ms, so
velocity cannot be read from per-millisecond counts. The host measures arrival
times (USB gives 1 ms resolution, ~1 % at those speeds) and the feel model in
freedj does the rest, which keeps the model tunable without hardware.

## Panel-module notes (for the PCF8574 board builder)

- Address jumpers A0–A2 so up to eight modules share the bus; the hub's I2C
  is 100 kHz because the PCF8574 is a 100 kHz part.
- Inputs: switch between pin and GND, nothing else; the chip's weak pull-up
  does the rest. LEDs: pin sinks the LED, resistor to 3V3, pick the resistor
  for ~3 mA with a high-efficiency LED.
- Bring INT out; the hub reads a module only when INT says something changed,
  and writes LED bytes only when they change.
- The same module works plugged straight into a Pi header (the app's
  `[surface.i2c]` path) or into the hub (`[surface.midi]`, notes as above).

## Toolchain decision (2026-10-24)

The board is authored in **tscircuit** (parts imported from JLCPCB with real
footprints, placement in code, modules per block) and exported to KiCad for
routing and fabrication outputs, as in the `pcb-layout` skill. What changed
is the router stage:

- **TraceMaker 0.11.0** (DingoOz, GPL-3, C++20, CUDA optional) replaces
  Freerouting as the first thing the exported board goes through. It reads
  and writes `.kicad_pcb` directly, is judged by `kicad-cli pcb drc`, and
  leaves anything it did not draw byte-for-byte alone. Tried here on the
  sbs-synth RP2350 board with all copper stripped: in 120 s it routed to
  28 unconnected with 106 clearance errors and 20 vias, against the
  Freerouting-plus-hand original's 25 unconnected, 502 clearance errors and
  98 vias. Same conclusion as its own README: dense QFN escapes are where it
  stops, so placement still decides. Runs on this workstation (Ubuntu 26.04,
  glibc 2.43, RTX 4070 SUPER).
- **KiCadRoutingTools** (drandyhaas, 614 stars, MIT) is the other serious
  file-based router; its `place_simple.py` placement optimiser is worth a
  trial once the board is composed.
- **WholeBoard** is a three-day-old end-to-end "describe a board" workbench
  from the TraceMaker author. We already have the design, so only its router
  matters to us.
- **Konnect** (mixelpixx, ~1.4k stars, AGPL) is a Rust MCP server with
  200-odd tools that drives **KiCad 10** over its IPC API: schematic
  placement and wiring, footprint moves, DRC/ERC, JLC part search, fab
  exports. KiCad has to be running with the board open; routing is
  Freerouting through a DSN/SES round trip. **Backplane** (i2cjak) is a
  KiCad fork with the same aim. Neither is needed for a board this size, and
  the workstation has KiCad 9.0.8; KiCad 10 is worth installing anyway for
  TraceMaker's plugin (the CLI works on KiCad 9 files today).

The pipeline as run (`hardware/control-board/Makefile`): `tsci build` →
`tsci export -f kicad_pcb` → `prep_board.py` (strip any tscircuit routing,
bottom GND pour, `.kicad_pro` with JLCPCB's real minimums) → `tracemaker route
--time 180` → `kicad-cli pcb drc`. The board is one flat tscircuit board with
`routingDisabled`, so there is one net scope and no false shorts on export.

**Where the layout stands (2026-10-24):** schematic complete and connectivity
clean (only the four-pin tact switches' spare pins warn). Each placement pass
takes ~5 min end to end. The first pass had the jog and browse headers on the
wrong side of the chip (GPIO0–18 leave the RP2350A on its left edge); with
the signal headers on the left and USB-C on the right TraceMaker closes about
140 of 162 connections in 180 s, the remainder being the QFN-60 escape
(core-buck V1V1 pins, QSPI, USB) and crowding around the I2C pull-ups. Next
passes: more room on the chip's top edge for the buck and QSPI, then
KiCadRoutingTools' `place_simple.py` as a second opinion, then hand-finish the
tail in KiCad. The alternative worth deciding before more passes: a **v1
carrier for an RP2350 module** (Waveshare RP2350-Zero, USB-C, castellated,
~$4) with the same connectors, which removes the QFN escape, the flash, the
crystal, the buck and the USB-C, and is hand-solderable; the bare-chip board
becomes v2 once firmware and panel are proven.

## Open questions for Dan

1. **Browse encoder on the hub or on the panel?** The hub carries the
   footprint either way; soldering it on means the hub must sit where the
   knob is.
2. **Jog index pulse.** Wire Z for an absolute angle (centre display later)
   or leave the pin spare?
3. **Second deck.** One hub per deck is the plan; a two-deck hub is a
   firmware-and-connector change, not a new board, if we ever want it.
