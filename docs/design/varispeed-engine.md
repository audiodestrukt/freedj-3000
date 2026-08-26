# Design: the varispeed / master-tempo audio engine

Status: **plan only, not implemented** (2026-08-26). This is the design for a
rewrite of the playback DSP so freedj can do the things a CDJ/turntable does that
the current Rubber-Band-only path cannot.

## Why one document for four features

Four requested behaviours are, underneath, the *same* missing capability —
**reading the source at a time-varying rate** (i.e. resampling / varispeed):

1. **Master Tempo OFF (key-lock off).** Today `key_lock` is an indicator only;
   playback is *always* Rubber Band (constant pitch). With MT off, pitch should
   follow speed — that is varispeed (resample), not time-stretch.
2. **Vinyl brake / release** (JOG MODE = VINYL; the button is labelled VINYL).
   Hitting stop spins the platter down: speed **and pitch** ramp to 0 over a
   brake time (a knob, up to ~5 s); hitting play spins it up 0 → fader speed.
   The pitch-drop *is* varispeed — Rubber Band can't do it and can't reach 0
   speed (infinite stretch). JOG MODE = CDJ keeps today's instant start/stop.
3. **Real-time sample-rate conversion (WORKSTREAMS A1).** Device runs at a fixed
   clock (often 48 kHz); tracks are various rates. A fixed-ratio resample
   `native_sr → device_sr` in the DSP chain replaces the offline
   resample-on-load stopgap (see [A1](../WORKSTREAMS.md)).
4. **(Later) vinyl scratch** when a platter is moved — also a time-varying read.

So: build **one engine** with a varispeed path, and these fall out of it.

## What exists today

- `crates/timestretch/`: `TimestretechStage` (Rubber Band R3, constant pitch,
  the real engine) and a stubbed `ResampleStage` (varispeed — `process()` is a
  passthrough with `TODO: run rubato`).
- `crates/app/src/audio.rs`: the processor thread reads the source at `proc_pos`,
  feeds `BLOCK_FRAMES` to the stretcher at `speed`, pushes to the `rtrb` ring;
  the cpal callback drains the ring. `key_lock` never reaches the DSP.
- On the Pi 4, R3 already costs ~65 % of one A72 core (see BENCHMARKS.md) — so
  CPU headroom is the constraint that shapes this design.

## The model: a platter with a rate and a pitch policy

Think of each deck as a platter producing a stream at the **device rate**.
State that drives the DSP each block:

- `source` — the decoded buffer (native rate) + a **fractional** read cursor.
- `rate` — the instantaneous platter rate = `fader_speed × nudge × brake_env`.
  1.0 = nominal; 0 = stopped. `brake_env` is 1.0 except during a spin-up/down.
- `master_tempo` — pitch policy: **on** = constant pitch (time-stretch);
  **off** = pitch follows rate (varispeed).
- `native_sr`, `device_sr` — the fixed SRC ratio `native_sr / device_sr`.

The **effective source-read ratio** per output frame is
`rate × native_sr / device_sr`. That single number is what advances the
fractional cursor; everything below is how the samples are produced from it.

## Two DSP paths, and when each runs

| Situation | Path | Pitch |
|---|---|---|
| MT **on**, steady rate | Rubber Band (time-stretch) + fixed SRC | constant |
| MT **off**, steady rate | varispeed resample | follows rate |
| Brake / release ramp (VINYL) | **varispeed**, always | follows rate → drops to 0 |
| Scratch (later) | varispeed | follows rate |

Key rule: **a brake always uses the varispeed path even if MT is on**, because a
real turntable powering down drops pitch. MT only chooses the *steady-state*
path; transitions through 0 are varispeed by physics.

### Varispeed path

A fractional-cursor interpolator reading the source at the effective ratio:
cursor `c` advances by `ratio` each output frame; output = interpolate(source, c).

- **Interpolator:** cubic Hermite (4-tap) is cheap (a few mults/frame) and good
  enough; upgrade to a short windowed-sinc only if aliasing on large downward
  pitch shifts is audible. Cubic handles a *continuously varying* ratio (the
  brake) trivially — rubato's fixed-ratio resamplers do not, so the varispeed
  path is hand-rolled, not rubato.
- **Cost:** cheaper than Rubber Band. Nice consequence: **MT-off is lighter than
  MT-on**, so key-lock-off buys Pi 4 headroom rather than costing it.
- **Latency:** ~0 (no analysis window), which simplifies `in_flight` during a
  brake (see accounting below).

### Time-stretch path (MT on)

Rubber Band at the tempo ratio for `rate`, plus a **fixed** `native_sr →
device_sr` resample (rubato `SincFixedOut`, before or after RB). RB's own cost is
unchanged by SRC — the resampler is *added* CPU (this is the nuance from A1: RB
is fixed-rate and does not fold SRC in). Measure on the Pi 4 before shipping.

### Switching paths without a click

Toggling MT, or entering/leaving a brake ramp, swaps engines mid-stream. Both
paths share the **fractional source cursor**, so position is continuous; the
click risk is the amplitude/phase discontinuity of RB's internal buffer. Mitigate
with a short (~5–10 ms) equal-power crossfade between the two paths' outputs, or
by resetting RB at a zero-crossing. The crossfade is the safe default.

## Position & latency accounting (the dangerous part)

The phase-locked playhead depends on `position` (source cursor) and `in_flight`
(decoded-but-unheard distance). This is the code that already carries the open
**~2.9 s position-jump bug** (the playhead lurches ~0.5 s on the Pi audio path;
not reproduced on desktop), so changes here are high-risk:

- `proc_pos` becomes **fractional** source samples; the UI reads a rounded value.
- `in_flight` must reflect the *current path's* latency: RB latency (frames) for
  MT-on, ~0 for varispeed. During a brake the ratio is changing every block, so
  `in_flight` in *source samples* is `ring_frames × ratio(t)` — time-varying.
  Getting this right is what keeps the waveform from lurching during a brake.
- Keep the hard-RT consumer allocation-free; pre-size the varispeed scratch
  buffers once (see [rt-audio-isolation](rt-audio-isolation.md)).

## Controls & state to add

- `master_tempo: bool` — the MT button (today indicator-only) → selects the path.
- `jog_mode: { Cdj, Vinyl }` — the VINYL button; VINYL enables brake/release.
- `brake_secs: f32` — the brake-time knob (0 = instant … ~5 s). 0 in CDJ mode.
- Transport state machine: `Stopped → SpinUp → Playing → SpinDown → Stopped`
  (add `Scratch` later). Play/Stop drive transitions; in CDJ mode SpinUp/SpinDown
  are instantaneous.

## Suggested increments (each shippable, measured on the Pi 4)

1. **Varispeed `ResampleStage` (cubic fractional read).** Wire it as the MT-off
   path. Delivers real key-lock-off, self-contained, and *reduces* Pi 4 load.
   No transport/brake changes yet.
2. **Path selection by `master_tempo`** with the crossfade switch. The MT button
   becomes real.
3. **Fixed SRC (`native_sr → device_sr`)** folded into both paths → completes
   real-time A1; retire the offline resample-on-load stopgap.
4. **Vinyl brake / release.** Add `jog_mode`, `brake_secs`, and the transport
   ramp riding on the varispeed path. Prove the `in_flight` accounting under a
   time-varying ratio here.
5. **(Later) scratch** — needs a scrub/touch input a plain DJ2Go jog can't give;
   the paused-jog vinyl-scrub already added is the stepping stone.

## Risks / watch-items

- **The RT position path** (open jitter bug) — do increment 1 without touching
  `in_flight` semantics first, then extend carefully.
- **Click on path switch** — crossfade; test toggling MT while playing.
- **Pi 4 CPU** — measure each increment; SRC adds cost, varispeed removes it.
- **Brake feel** — `brake_env` shape (linear vs exponential): a real turntable
  decays roughly exponentially. Start linear, make the curve a constant if it
  sounds wrong.
