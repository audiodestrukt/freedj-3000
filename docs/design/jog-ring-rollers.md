# Design: a ring jog on rollers, a fixed display in the middle

Status: **test rig designed, not printed** (2026-11-20): [`hardware/cad/jogring`](../../hardware/cad/jogring/README.md).  For deck7's 206 mm jog.

Dan's decisions so far:

- **Display:** a round smart-watch LCD, 2.1" at most, with the 1.28" GC9A01 he has for the prototype.
- **Weight:** none.  A Pioneer doesn't weight the ring.  Eddy-current drag may be tried for the feel.
- **Bearings:** skateboard bearings (608s) are what he has.  The rig has a 608 version, a 608 in a printed V tyre, beside the V-wheel one.  Both use the same ring.
- **Touch:** undecided.  A Pioneer uses a physical switch, not capacitive.

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
             │   (fixed, hollow:    │       │ skirt  ◄ V groove (outside)
             │    display cable)    │       │◄►(V-wheel)    (sense roller)
  top plate ─┴──────────────────────┴───────┴───────┴──────────┴─────────
              cable down to the Pi        wheel posts in the top plate
```

| | Value | Why |
|---|---|---|
| Platter OD | 206 mm | unchanged (layout.toml `jog.platter_mm`) |
| Hub | Ø66.4 | a 2.1" watch panel (glass ~61 ⚠) under a 1.5 mm lip; a second top for the 1.28" |
| Ring bore | hub + 2 × 1.5 mm gap | the only visible seam on the top |
| Skirt | Ø140 mm, under the platter | a larger support radius resists tilt better when a DJ leans on the rim |
| Groove | a 90° V around the skirt's outside | V-wheels have the V on the wheel, so the ring gets the groove; one groove carries radial load and up/down load |
| Wheels | 3 × V-groove wheels, 120° apart, vertical axes, on posts in the top plate | they sit at r ≈ 80, inside the platter's shadow (r < 103), so they can't be seen |
| Preload | one wheel on an eccentric spacer | removes play; the same trick as on a V-slot gantry |

### Why rollers in a V groove, and not a big bearing

| Option | For | Against |
|---|---|---|
| **3 V-wheels in a V groove** (built) | cheap (3D-printer V-wheels, 625 bearings, M5 posts and an eccentric spacer); self-centring; preload is adjustable; one groove takes every load | a printed groove wears against the wheels.  Fine for the prototype; later a turned or laser-cut ring |
| Thin-section bearing (e.g. a 61818, 90 × 115 mm) | smoothest; no adjustment | expensive; heavy; the bore sets the hub size |
| Lazy-susan bearing | very cheap | gritty and noisy, with play, which is exactly what a jog must not have |
| Printed ball race with loose balls | no bought parts | hard to make quiet and free of play |

Load check: a hard press on the rim is a few kg at r = 103 against wheels at
r ≈ 70–82.  625 bearings are rated well above that (about 1.7 kN dynamic).
The weak part is the printed groove's flank, not the wheels.

## Sensing the angle: a roller on the skirt

The recommendation: **a small roller with an O-ring tyre, sprung against the
skirt's smooth face, with the AS5600 under a magnet in the roller's axle.**

- It reuses the sensor, magnet and firmware the deck already has.
- It is geared up for free: the roller (an Ø18 tyre) runs on the skirt's
  inside face (Ø126), so it turns 7 × per platter turn.  At the AS5600's
  4096 counts that is about 28,700 counts per platter turn, against 4096
  now.  (A smaller roller would gear it up more, but its bearing boss has
  to clear the skirt.)
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

Sensing on one of the V-wheels themselves would save a part.  But the groove
touches the wheel on its flanks, so the ratio is uncertain, and the
eccentric preload changes it.  Rejected for now.

## Touch: undecided

A Pioneer jog uses a **physical switch**, not capacitive sensing.  The ring
turns, so a wire can't reach its top.  There are two routes:

- **A switch.**  The top plate floats a little on the ring (springs), and
  pressing it pushes a continuous lip under the ring down onto a fixed
  switch or optical interrupter.  Because the lip runs all the way round,
  it works at any angle.  This is the CDJ feel.
- **Capacitive.**  A conductive top joins a band on the ring, which
  couples across about 1 mm to a fixed electrode.  A 10 mm band at r = 70
  gives roughly 40 pF, plenty for a touch controller.  No moving parts, but
  not what a CDJ does.

The rig's ring is one piece for now, so the rollers and the sensing can be
judged first.

## Feel: no weight, maybe eddy-current drag

The ring is as light as it prints, as on a Pioneer.  For drag, an aluminium
or copper sheet under the top and fixed magnets below it would give a
resistance that grows with speed and is set by the gap, with no contact.
The rig keeps r 74–100 under the top flat and clear for that
(`EDDY_BAND`).

## The display

A round smart-watch LCD, fixed on the hub column, with its cable down the
hollow column:

| Display | Drive | Note |
|---|---|---|
| 1.28" GC9A01, 240 × 240, SPI | the RP2350, or the Pi's SPI | the prototype part; `hub_128` |
| 2.1" round, 480 × 480 (ST7701-class, RGB/SPI init) | an RGB adapter, or the Pi's DPI | the largest that fits; `hub_210` sets the hub's size |

Software follows: the deck draws a jog-display view (playhead angle, cue
marks, artwork) to it.  This is a separate piece of work.

## How to get there

1. **A standalone test rig**, [`hardware/cad/jogring/`](../../hardware/cad/jogring/README.md) (designed;
   `make jogring`): a base plate, the hub column, the skirted ring, three
   wheel posts (one eccentric) and the sense roller on its flexure arm.  It fits the 256 mm bed, so the 206 ring is one
   print.  Print it and answer three questions:
   - Does it feel smooth and without play?
   - Does the roller slip on a hard flick?
   - Does touch work across the gap?
2. **Fold it into deck7**: `top_fr` takes the wheel posts, the hub column
   and the sense roller where the jog boss and the AS5600 cage are now.  The
   jog code (the AS5600 path) only gains a ratio setting.
3. **The display and its view.**

## Open questions (Dan)

- **Touch**: a floating top with a switch, or capacitive.
- **Eddy-current drag**: try it on the rig once the rollers are judged.
