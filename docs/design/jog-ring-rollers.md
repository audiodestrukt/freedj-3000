# Design: a ring jog on rollers, a fixed display in the middle

Status: **plan only, not built** (2026-11-20).  For deck7's 206 mm jog.

## Why the jog changes

deck7's jog is a solid platter on an 8 mm shaft in two 608s, with the
AS5600 under a magnet in the shaft's end (`hardware/cad/deck7/README.md`).
A CDJ-style jog has a **display in the middle that does not turn** and a
**touch-sensitive top**.  With a display there:

- the middle is taken, so the platter becomes a **ring**;
- a ring has no shaft, so it has to be **held at its rim**, by rollers;
- with no shaft end there is nowhere for an on-axis magnet, so the
  **angle has to be sensed some other way**;
- the display's cable has to get **down through the middle** to the Pi.

## The layout

```
              fixed display window (hub)        platter ring (turns)
             ┌──────────────────────┐   ┌──────────────────────────────┐
  ═══════════│      LCD + bezel     │═══│  top surface (touch)         │════
             │                      │ g │                              │
             │   hub column         │   └───┬──────────────────────────┘
             │   (fixed, hollow:    │       │ skirt  ◄ V-rib (outward)
             │    display cable)    │       │◄►(V-wheel)    (sense roller)
  top plate ─┴──────────────────────┴───────┴───────┴──────────┴─────────
              cable down to the Pi        wheel posts in the top plate
```

| | Value | Why |
|---|---|---|
| Platter OD | 206 mm | unchanged (layout.toml `jog.platter_mm`) |
| Hub window | ~80 mm ⚠ | about 40 % of the jog, like a CDJ; set by the display chosen (below) |
| Ring bore | hub + 2 × 1.5 mm gap | the only visible seam on the top |
| Skirt | Ø140 mm, under the platter | a larger support radius resists tilt better when a DJ leans on the rim |
| Rib | a 90° V around the skirt's outside | one rib carries radial load and up/down load |
| Wheels | 3 × V-groove wheels, 120° apart, vertical axes, on posts in the top plate | they sit at r ≈ 82, inside the platter's shadow (r < 103), so they can't be seen |
| Preload | one wheel on an eccentric spacer | removes play; the same trick as on a V-slot gantry |

### Why rollers on a V-rib, and not a big bearing

| Option | For | Against |
|---|---|---|
| **3 V-wheels on a V-rib** (recommended) | cheap (3D-printer V-wheels, 625 bearings, M5 posts and an eccentric spacer); self-centring; preload is adjustable; one rib takes every load | a printed rib wears against the wheels.  Fine for the prototype; later a turned or laser-cut ring |
| Thin-section bearing (e.g. a 61818, 90 × 115 mm) | smoothest; no adjustment | expensive; heavy; the bore sets the hub size |
| Lazy-susan bearing | very cheap | gritty and noisy, with play, which is exactly what a jog must not have |
| Printed ball race with loose balls | no bought parts | hard to make quiet and free of play |

Load check: a hard press on the rim is a few kg at r = 103 against wheels at
r ≈ 70–82.  625 bearings are rated well above that (about 1.7 kN dynamic).
The weak part is the printed rib's flank, not the wheels.

## Sensing the angle: a roller on the skirt

The recommendation: **a small roller with an O-ring tyre, sprung against the
skirt's smooth face, with the AS5600 under a magnet in the roller's axle.**

- It reuses the sensor, magnet and firmware the deck already has.
- It is geared up for free: at a Ø140 skirt and a Ø14 roller, the roller
  turns 10 × per platter turn.  At the AS5600's 4096 counts that is about
  41,000 counts per platter turn, against 4096 now.
- The jog only needs relative motion, so the exact ratio is calibrated once
  in software (spin the platter one turn against a mark).
- The spring keeps the tyre from slipping.  The only drag on the roller is
  the magnet.

**Risk: slip on a hard flick or a spin-back.**  That is the first thing the
test rig has to answer.  The fallbacks, in order:

1. **Gear teeth** moulded into the skirt, with a small gear on the sensor.
   It can't slip, but it has backlash and makes noise.
2. **An optical mouse sensor** (a PMW3360-class part) reading the ring's
   underside.  It doesn't touch the ring and has very high resolution, but it
   needs a textured underside at a fixed gap.
3. **A magnetic pole ring on the skirt** with a matching linear or ring
   sensor (the AS5304/AS5311 family).  It doesn't touch the ring either, but
   the parts cost more and are harder to get.

Sensing on one of the V-wheels themselves would save a part.  But the rib
touches the wheel on its flanks, so the ratio is uncertain, and the
eccentric preload changes it.  Rejected for now.

## Touch: capacitive, across the gap

The ring turns, so a wire can't reach its top.  Instead it couples across an
air gap, with no slip ring:

- The platter's top surface is conductive: aluminium tape or foil, or later
  a metal top ring, as on a CDJ.  It is joined to a conductive band inside
  the skirt.
- A fixed electrode, a copper ring on the top plate, faces that band across
  about 1 mm.
- Coupling: ε₀ · A / d with a 10 mm band at r = 70 is about 2π · 70 · 10 mm²
  at 1 mm, roughly **40 pF**.  That is plenty for a touch controller, which
  detects a few pF.
- It is read by the control board (#77, RP2350): PIO charge-time sensing, or
  an MPR121 on I²C.  It replaces layout.toml's `jog_touch` tact switch, so
  pressing the jog is no longer required.
- The hub's bezel stays plastic, so touching the display is not a jog touch.

## The display

It is fixed on the hub column, and its cable goes down the hollow column.
The candidates (all ⚠ measure the module before drawing the hub):

| Display | Drive | Note |
|---|---|---|
| A round MIPI-DSI panel of about 3.4" | **the Pi 5's second MIPI port** | full frame rate from the deck's own renderer; the recommendation if one fits about 80 mm |
| A round 2.1–2.8" panel, 480 × 480 (RGB/SPI) | an adapter, or the RP2350 | smaller; more glue |
| GC9A01, 1.28", 240 × 240, SPI | the RP2350 | cheap and tiny; the hub would shrink to about 40 mm |

Software follows the hardware: the deck draws a jog-display view (playhead
angle, cue marks, artwork) to a second output.  This is a separate piece of
work.

## How to get there

1. **A standalone test rig**, `hardware/cad/jogring/`: a base plate, the hub
   column, the skirted ring, three wheel posts (one eccentric) and the sense
   roller on its spring arm.  It fits the 256 mm bed, so the 206 ring is one
   print.  Print it and answer three questions:
   - Does it feel smooth and without play?
   - Does the roller slip on a hard flick?
   - Does touch work across the gap?
2. **Fold it into deck7**: `top_fr` takes the wheel posts, the hub column
   and the sense roller where the jog boss and the AS5600 cage are now.  The
   jog code (the AS5600 path) only gains a ratio setting.
3. **The display and its view.**

## Open questions (Dan)

- Which centre display?  It sets the hub, so it is the first thing to pin
  down.
- Should the ring be **heavy**, with a metal insert for flywheel feel, or
  as light as it prints?
- Should the top's touch surface be **metal** (CDJ feel) or foil under a
  printed skin?
