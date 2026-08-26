# Design: the sampler-deck (auto one-shots, cue scheduling, sequencing)

Status: **vision / feature backlog** (2026-08-26). Not planned for implementation
yet — this is the running list and the first-principles model for an "advanced
deck" that blurs the line between DJ deck and sampler. Seeded from DJing on the
XDJs: a song often has a vocal sample in the intro that you could play in time
with the CUE button; extend that idea all the way.

## Thesis

A DJ deck and a sampler are the same instrument seen from two angles:

- A **deck** is *one playhead moving along one fixed timeline*, with a few named
  points (cues) you can jump to.
- A **sampler** is *a set of named buffers you trigger*, each with its own tiny
  playhead, quantized to a clock.

Unify them: a deck is **a timeline plus a set of named, triggerable regions**,
where the regions can be *found automatically*, *triggered in time*, and
*scheduled/sequenced against the beat grid* — and where a region can play as its
own voice while the main track keeps going. Everything the sampler side needs
(tempo, phase, structure) we are already computing for the deck side.

## First-principles model

Three primitives:

1. **Region** — a span of the loaded track `[start, end)` with a role
   (vocal-phrase, hit, riser, downbeat, loop) and metadata (its bar/beat phase,
   pitch, loudness). A cue point is a zero-length region; a loop is a region you
   repeat; a one-shot is a region you trigger as a voice.
2. **Voice** — an independent playback cursor over the source buffer, with its
   own rate/pitch (reuse the varispeed engine). The main deck is voice 0; sample
   triggers spawn voices 1..N that mix into the same output. **This is the one
   real engine change**: today there is a single playhead; the sampler side needs
   a small fixed pool of voices (see below).
3. **Grid clock** — the beat grid we already detect (BPM + anchor + downbeat)
   *is* the sampler's transport clock. Triggers, quantization, and sequencing are
   all expressed as bar/beat phase against it. No new clock needed.

Given these, the feature list is "ways to make regions, ways to trigger voices,
ways to schedule them against the grid."

## Feature list

### A. Automatic region extraction (offline, at load — extends the analysis crate)

- **Intro vocal / a-cappella one-shot.** Detect a vocal-present, low-percussion
  span near the start and offer it as a triggerable one-shot (the seed idea).
- **Vocal-phrase regions** across the track (vocal activity detection; or full
  source separation, offline/optional — Demucs-class).
- **Hits / one-shots** from strong isolated transients (we already compute an
  onset-strength envelope — `crates/analysis/src/beat.rs`).
- **Structure / section map** — intro, build, drop, breakdown, outro via novelty
  detection, so cues can be placed at *musical* boundaries.
- **Auto-cue set** — semantic cue points (first downbeat, each phrase boundary at
  8/16/32 bars, the drop, the vocal start) instead of one "auto cue to first
  sound". Placed on the grid.
- **Cheap bootstrap first:** the waveform already carries per-column low/mid/high
  band energy — "mid/high present, low absent" is a serviceable first pass at
  "vocal/melodic, no kick" without source separation. Ship the heuristic, add
  separation later.

### B. Triggering & performance

- **One-shots on the hot-cue pads** (builds on hot cues, #8): a pad triggers its
  region as a voice while the track keeps playing.
- **CUE-button one-shot** — the seed: hold/tap CUE to fire the intro vocal in
  time (quantized), main track underneath.
- **Quantized trigger** — a trigger fires on the next grid division (QUANTIZE,
  #21), so it lands in time even if you press early/late.
- **Tempo/pitch match** — a triggered region time-stretches to the deck tempo
  (reuse Rubber Band / the varispeed engine) so a vocal plays in time and, if
  wanted, in key. Optional key-lock per voice.
- **Trigger modes** — momentary (gate), latched (toggle), one-shot (play to end),
  retrigger/stutter, and **choke groups** (a new trigger cuts a previous voice).

### C. Scheduling & sequencing

- **Arm-to-grid** — arm a cue/region to auto-fire at the next downbeat / next
  phrase, hands-free (this is "cue point scheduling").
- **Rolls / beat-repeat** — loop a region at 1/1…1/16 for the duration of a hold.
- **Step sequencer** — place regions on a grid-synced pattern (e.g. 16 steps =
  1 bar) and let it run, turning the deck into a phrase sequencer over the track.
- **Scenes / chains** — a saved arrangement of which regions fire when, replayable
  as a performance macro.

## Architectural implications

- **Voice pool (the real new thing).** A small fixed pool of `Voice`s
  (e.g. 4–8) mixed into the output, each a fractional-cursor reader over the
  `Arc<Vec<f32>>` we already hold — no re-decode, regions are just index ranges.
  Reuse the varispeed cursor from `docs/design/varispeed-engine.md` (increment 1).
  Keep the RT path allocation-free: pre-allocate the pool; a trigger just claims
  a free voice and sets its range/rate. Per-voice Rubber Band is the CPU question
  — measure with the RTF guard (`make perf`); on the Pi, ungated key-lock per
  voice may be too heavy, so varispeed (pitch-follows-rate) voices are the cheap
  default and time-stretched voices the opt-in.
- **Segment map** is a new analysis output alongside BeatGrid/WaveformCache: a
  list of `Region`s with roles + grid phase, produced at load (heavier bits
  async, like the beat grid landing after audio — see #19).
- **Grid scheduler** — a small sequencer that, each block, fires any triggers due
  at the current grid phase. Deterministic, driven by the same position the
  playhead uses.
- **UI** — the trigger pads / sequencer live in the *separate button-controls
  surface*, never on the faithful deck screen (see the screen-fidelity rule).

## Connections to the existing roadmap

- Beat grid + onset envelope (built) → extraction + grid scheduling.
- Waveform band energy (built) → cheap vocal/percussive heuristic.
- Hot cues (#8) → the trigger surface.
- Varispeed engine (`docs/design/varispeed-engine.md`) → per-voice rate/pitch.
- Quantize (#21) → grid-snapped triggers.
- Button-controls surface (#23) → where pads/sequencer render.
- Non-blocking load (#19) → where the segment-map analysis runs.

## Prior art (worth studying, not copying)

Pioneer/rekordbox active-loop & memory cues; Serato Sampler / Pitch 'n Time;
Ableton (Simpler/Slicing, Follow Actions, clip launch quantize); Maschine
(one-shots, choke groups, scenes); Sononym / stem-separation tools. The
first-principles angle: those bolt a sampler *next to* a deck; here the deck *is*
the sampler because both ride the same detected grid and the same decoded buffer.

## Open questions

- How many voices before the Pi 5 runs out of DSP headroom? (Measure per-voice
  varispeed vs time-stretch cost with the RTF guard.)
- Extraction quality vs cost: how far does the band-energy heuristic get before
  source separation is worth its (offline) cost and RAM?
- Trigger latency: sample voices must feel instant — reuse the instant-cue
  pre-prime work (#20) so a triggered voice starts with no re-prime gap.
- Save format for the segment map + user edits (ties into the library, F1/F2).
