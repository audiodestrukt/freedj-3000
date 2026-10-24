# hardware — ADK-1000 physical parts

Parametric CAD for the physical control surface (see
[`docs/design/faceplate-and-hardware.md`](../docs/design/faceplate-and-hardware.md)).
Parts are authored in [build123d](https://build123d.readthedocs.io) — named
dimensions → operations → a watertight, single-body STL — using the
`build123d-part` skill.

## Parts

| File | Part | Status |
|---|---|---|
| `cad/jog_wheel.py` | Jog platter: dished top + center recess, ribbed grip skirt, encoder hub with shaft bore + radial grub-screw | v1, prints |
| `control-board/` | Control hub PCB: RP2350A, USB-C (class-compliant USB MIDI), jog encoder in, fader ADC, browse encoder, I2C panel bus for PCF8574 modules ([design](../docs/design/control-board.md)) | schematic complete, placement + autoroute iterating, not fabricated |

## Build

```sh
python3 hardware/cad/jog_wheel.py      # -> hardware/cad/build/jog_wheel.stl
make cad                               # same, from the repo root
```

Each part prints a sanity line (bbox / body count / watertight) and every build
is verified watertight + single-body before it's considered done.

## jog_wheel — before you print

- **Verify `OUTER_D` first.** It defaults to **206 mm** — the classic Pioneer
  platter, and what the jog's proportion in our faceplate photo works out to
  (~0.34× the face width). Caliper the real wheel and set the constant; every
  other dimension scales sensibly around it.
- **It's ~206 mm across** — needs a ~220 mm bed. If yours is smaller, say so and
  it can be split into a hub-core + snap/bolt rim.
- **Print top-face-down** (recess on the bed): the skirt walls and hub then rise
  with no internal supports. The radial grub-screw hole bridges cleanly.
- **`SHAFT_D` defaults to 6 mm** (common encoder shaft). Set it to your encoder;
  `BORE_FIT` is the slip clearance. The grub screw (`SET_TAP`, M3 thread-forming)
  clamps the wheel to the shaft.
- The top-plate switch / center display from the design doc are **not** modeled
  yet — the center recess is where a label or future center display goes.

## control-board — PCB

Authored in [tscircuit](https://tscircuit.com) (`index.circuit.tsx`: real JLCPCB
footprints under `imports/`, placement in code, routing disabled), exported to
KiCad, routed by [TraceMaker](https://github.com/DingoOz/TraceMaker) and judged
by `kicad-cli pcb drc`.  The design and the toolchain decision are in
[`docs/design/control-board.md`](../docs/design/control-board.md).

```sh
cd hardware/control-board && npm ci          # tscircuit (bun on PATH)
make check                                   # schematic connectivity only
make all                                     # export -> prep (JLC rules, GND pour) -> TraceMaker -> DRC
```

`make all` needs `tracemaker` on PATH (release tarball, Linux x86_64, glibc ≥ 2.38),
KiCad 9/10's `kicad-cli`, and the system `python3` with the `pcbnew` module for
`prep_board.py` (from the `pcb-layout` skill).  Outputs land in `build/`:
`control-board.routed.kicad_pcb` is what you open in KiCad to finish the tail.
