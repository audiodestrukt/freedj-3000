# Design: the XDJ-1000MK2 faceplate — touch skin → hardware

Status: **plan only, not implemented** (2026-08-27). This captures the direction
for making freedj *look and feel* like an XDJ-1000MK2 — first as a full on-screen
skin, then as a physical control surface — without committing to a build yet.

## The one decision, and why it doesn't block us

The faceplate has two possible targets:

1. **Touch skin** — the whole face rendered on a touchscreen (tablet, or the Pi
   panel): jog, pitch fader, transport, cue/loop, rotary, all touch-operated.
2. **Hardware faceplate** — a physical unit: the screen stays screen-accurate,
   and the controls become real (jog wheel, fader, buttons) wired in.

They are not exclusive, and the order is settled: **build the touch skin first,
regardless of the hardware plan.** It is the cheapest way to nail layout,
proportions, and control placement, and it doubles as the dimensioned mockup for
the panel. When hardware arrives, the physical controls *replace the touch
adapters emitting the same events* — nothing downstream changes.

That last sentence is only true because of an existing decision:

## What makes this tractable: the input bus

Everything already drives the engine through one `ControlEvent` stream, with
adapters behind it (see [`INPUT_PLAN.md`](../INPUT_PLAN.md)). A touch jog and a
physical jog emit the *same* event; the engine never knows which moved. So:

- the skin is **new adapters + paint**, not an engine change;
- hardware is **another adapter** (RP2350 over USB/serial) feeding the same bus;
- both can be developed and tested against the bus independently, and a recorded
  hardware session replays as a script.

And the [screen-fidelity split](../reference/xdj-1000mk2-screens.md) stands: on
the **hardware** target the 7" screen stays a faithful XDJ screen with **no
on-screen buttons** — physical controls own that surface. On the **touch** target
the skin adds control zones around (and, for the jog, over) the screen region.

## The jog wheel is the whole ballgame

Everything else on the face is a button, a fader, or a knob — solved patterns.
The jog is the hard part, and it splits into a part we're *not* worried about and
a part we are.

### Not the risk: the encoder

The sensing hardware is a solved problem we can do *better* than Pioneer — a
high-resolution encoder (optical or magnetic, hundreds of counts/rev) gives us
finer deltas than the stock platter. Resolution is not the constraint.

Two ground-truth notes about the real XDJ-1000MK2 jog, to build to:

- **The top plate is a physical switch, not capacitive.** Pressing the platter
  top closes a mechanical contact → "touch"/scratch engaged; release → bend/free.
  (Slightly odd versus the capacitive tops on many controllers, but it's what the
  unit does, so the hardware replicates a momentary switch under the plate — and
  the feel model keys scratch-vs-bend off that switch, exactly as the XDJ does.)
- Optional center display in the platter — a later nicety, not core.

### The actual risk: the feel model (curves, acceleration, smoothing)

Making the platter *feel* like an XDJ is a signal-processing / tuning problem,
not a mechanical one: how raw encoder deltas become platter velocity, and how
that velocity drives the audio read rate. The pieces to get right:

- **Delta → velocity mapping.** Encoder counts/tick → an angular velocity. Needs
  a response curve (likely nonlinear): small slow moves = fine bend; fast spins =
  scratch throw. Getting the curve's knee and gain to match the XDJ is the feel.
- **Acceleration / throw.** A flick should carry momentum; the platter shouldn't
  feel 1:1-rigid. This is the "acceleration" the XDJ tuning has.
- **Smoothing / inertia.** Encoder deltas are quantized and bursty at tick rate;
  a smoothing time-constant (and, in vinyl mode, a small inertial model) turns
  them into continuous, natural motion — too little = steppy, too much = laggy.
- **Two modes, off the physical switch:**
  - **VINYL (switch pressed):** the platter *is* the transport — a time-varying
    read (scratch). This is the same varispeed read the
    [varispeed engine](varispeed-engine.md) is being built for; jog scratch is
    item 4 there. Release = spin back to fader speed (brake/release curve).
  - **CDJ / bend (switch released):** the platter *nudges* pitch temporarily
    (tempo bend) without grabbing the transport — the current nudge behavior.

freedj already has jog **vinyl/nudge modes and a start cue** (shipped), so there
is a feel model to iterate on rather than a blank slate. The work is tuning its
curves against the real unit, ideally by capturing XDJ jog response and matching.

**Open feel questions to answer empirically:**
- What is the XDJ's delta→velocity curve, measured? (Capture platter motion vs
  audible pitch on the real unit.)
- Smoothing time-constant that reads as "tight but not steppy" at our encoder's
  count rate?
- Vinyl-mode inertia: pure 1:1 to the platter, or a light flywheel model?

## The rest of the face (straightforward, bus-mapped)

Ground truth: **every button on the XDJ is a cheap tact switch** — plain
momentary tactile switches, nothing analog or sensed. Even the big **CUE and
PLAY** transport buttons, which look substantial, are just a large keycap/plunger
over a standard tact switch underneath. So the entire non-jog control set is
trivial hardware (a button matrix into the MCU), and it's an easy place to *beat*
the unit if we want — better switches or silicone domes under the transport caps
for a nicer press, without changing anything downstream.

Each control is an adapter emitting existing `ControlEvent`s:

| Control | Type | Bus event | Notes |
|---|---|---|---|
| Pitch fader | long-throw linear, center detent | tempo/fader speed | detent → snap-to-0 already exists |
| Transport (PLAY/CUE) | momentary | play / cue (hold-preview) | CUE is momentary today |
| Cue/Loop (IN/OUT/RELOOP, hot cues) | momentary | cue/loop events | hot cues roadmapped |
| Rotary selector (browse) | encoder + push | browse delta / load | maps to today's browse encoder |
| SYNC / MASTER / etc. | momentary | sync / master | master handoff now works |
| Tempo range, key lock, quantize | momentary | mode toggles | button-state surface, off-screen |

## Hardware division of labor

Matches the bus-adapter plan in [`INPUT_PLAN.md`](../INPUT_PLAN.md):

- **RP2350 (or similar MCU)** owns the control cluster: reads the jog encoder +
  top-plate switch, the pitch fader (ADC), the button matrix, and the rotary;
  does the *time-critical, high-rate* jog sampling locally; and speaks **HID or
  MIDI (or raw serial)** to the Pi. Keeping the jog's fast loop on the MCU avoids
  USB/scheduling jitter in the feel path.
- **Pi 5** runs freedj (verified target): audio, screen, ProDJ Link. It receives
  control events over the bus and never has to poll hardware in the RT path.
- **Panel** = the 7" 1024×600 screen + the physical cluster; the touch skin's
  layout becomes the panel's dimensioned reference.

Design intent: the *jog feel model* lives in freedj (so it is tunable in
software and testable without hardware), while the MCU just delivers clean,
high-rate encoder/switch data. If jitter forces some smoothing onto the MCU,
that's a later optimization — start with the model in freedj.

## Suggested sequence

1. **Touch skin** on the current stack (egui/wgpu) — full face, touch adapters on
   the bus. Immediately useful, and the layout/mockup for hardware.
   *First pass landed:* run with `--faceplate` (or `OPENDECK_FACEPLATE=1`) to
   render the deck body, jog, tempo fader, transport, loop/sync/master keys and
   browse rotary around the screen; default stays screen-only. Proportions are
   approximate and want tuning against the real unit (`screen::faceplate_layout`),
   and the jog is a placeholder feel — see step 2.
2. **Jog feel model** — tune the existing vinyl/nudge curves against captured XDJ
   response; land it as part of (or feeding) the
   [varispeed engine](varispeed-engine.md).
3. **MCU control adapter** — RP2350 reading a real encoder + switch + fader +
   buttons, over the bus; validate against the skin's event contract.
4. **Panel / enclosure** — once the skin has settled the layout.

Nothing here is committed; this is the shared reference to iterate on as the
direction firms up.
