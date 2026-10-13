# Design: the Player crate — one deck engine, N decks, new instruments

Status: **plan, not started** (2026-10-13). Supersedes the sketch in
[`../WORKSTREAMS.md`](../WORKSTREAMS.md) §C (C1 transport as a value, C4
dual-layer, C5 routing) and is the groundwork for the sampler deck in
[`sampler-deck.md`](sampler-deck.md) / #24. Related: #17 (per-deck DSP
threads), #37 (player-number arbitration).

Provenance: this design derives from this repo's own code and documents and
from the public Pro DJ Link analysis (Deep Symmetry). Nothing in it is taken
from any other DJ product's software, and none is to be consulted while
building it; comparisons with other products are done the way the XDJ was
compared, as a black box on the network.

## What this is for

The XDJ-1000MK2 reproduction is done well enough to ship, and three things in
it are worth more than the reproduction itself:

- **The audio engine.** A decoded buffer behind an ArcSwap, a processor thread
  with a fractional cursor and Rubber Band R3, loop wraps assembled inside a
  block so the stretcher never sees a discontinuity, seek-while-paused, in-flight
  accounting so the UI shows what is heard, a 3 ms stop fade and no start fade.
  CUE lands on the beat; Dan has said so on the device.
- **The UI pacing.** Compositor-paced frames, a playhead that free-runs at the
  true rate and phase-locks to the audio clock, 10 fps idle. Glitch-free on a
  phone, an iPad, a Pi 5 and a desktop.
- **The Rust core.** One workspace that builds for Linux, the Pi and iOS with
  the same crates.

None of it is reusable today without taking the XDJ screen along. The engine
is in `crates/app/src/audio.rs`; the deck state (transport rules, cues, loops,
SLIP, grid, waveform, the loader pipeline) is fields of `DeckApp` in
`crates/app/src/lib.rs`, next to the screen, the faceplate, the browser, Link
and the window. This document is the plan for lifting the deck out so that a
process can hold N of them, and so that a second product (an "advanced"
one-deck instrument, and possibly later a multi-deck app) is built on the same
engine rather than beside it.

The design keeps a line the project drew in `PLAN.md`: this is a deck, not a
mixer. N players and a routing table are not a mixer; a crossfader UI would
be. The product decision at the end is still open, but phases 0–2 are the same
whichever way it goes.

## What exists, and what is dead

| Piece | Where | Status |
|---|---|---|
| `AudioHandle`, processor thread, cpal callback | `crates/app/src/audio.rs` | live, the engine |
| CUE/PLAY rules | `crates/transport` | live, pure, spec-tested; already the right shape |
| Hot cues, memory cues, loops, SLIP anchor, grid, waveform, tags | fields of `DeckApp` | live |
| Loader pipeline (`Prep` → `Prepared` → `apply_prepared`, loader thread) | `crates/app/src/lib.rs` | live |
| Other-deck phase (`PeerPhase`), master/sync, Link sender | `crates/app/src/prodj.rs` | live |
| `crates/engine` (transport, slip, loop_engine) | 550 lines | **imported by nothing**; superseded by the above. Delete it in phase 0, do not revive it. |

## Target shape

```
crates/player   Player — the deck engine.  Decoded buffer, cursor, stretcher,
                ring producer; transport rules; hot/memory cues; loops; SLIP;
                grid + waveform; loader pipeline.  No window, no screen, no
                Link, no cpal.  N per process, one processor thread each.
crates/output   Output — the one cpal stream.  Takes N ring consumers, sums
                them with a per-player gain, routes each player to an output
                pair.  The stop fade lives here, per player.
crates/sync     Clock — who is master, at what tempo and bar phase.  One peer
                trait, two kinds of peer: a Player in this process, or a
                Pro DJ Link player on the wire.
crates/link     Unchanged on the wire; becomes a sync peer plus media.
crates/app      The XDJ-1000MK2: one Player + screen + faceplate + Link.
crates/<new>    The advanced deck: N Players + its own UI (phase 3).
```

The XDJ app is the first consumer of every new crate and must behave exactly
as before at the end of each phase. That is what makes the refactor safe to do
first and what keeps the shipped app shipping.

### Player

- **Commands:** `ControlEvent` (`crates/protocol`) is already the command set
  for a deck; `Player::apply(ControlEvent)` takes over the deck half of
  `DeckApp::apply`. Visual-only events (page flips, zoom, screen modes) stay in
  the app.
- **State out:** `PlayerSnapshot`, the deck half of today's `DeckSnapshot`
  (position, heard position, speed, cue point, cued, loop, hot cues, grid,
  waveform handle, tags, loading). `DeckSnapshot` becomes
  `PlayerSnapshot` + screen flags.
- **Audio out:** a ring consumer handed to `Output` at construction. The
  processor is moved, not rewritten.
- **Loading:** the loader thread and `Prep`/`Prepared` move with it. Media
  sources (disk, NFS, dbserver) stay where they are; a `Player` is handed
  bytes plus an optional analysis, as `apply_prepared` is today.
- **Rules that travel with it, as tests:** the transport specs in
  `specs/xdj-1000mk2/`, the seek-while-paused behaviour, the ring flush
  handshake, no fade-in. These are the cue-latency contract and must not be
  reinterpreted during the move.

### Output

- One cpal stream per process. The callback pops from N rings, multiplies by a
  per-player gain, and writes each player to its routed output pair; players
  routed to the same pair sum. Default: every player to pair 0.
- A multichannel interface gives the dual-layer and two-deck cases separate
  outputs to an external mixer, which is how a Denon dual-layer deck does it;
  a stereo interface gives an internal sum. Both are the one routing table.
  This settles C4's "two output pairs or an internal mix" with "both, by
  configuration".
- The stop fade becomes per-player gain ramps inside the callback; `playing`
  and `drain_flag` become per-ring.
- Per-player processor threads are the multi-core story (#17): on a Pi 5,
  render, player A, player B and the RT output each get a core.

### Sync

- Today SYNC means "follow the Link master", the master role is negotiated in
  `prodj.rs`, and the other deck's bar phase is a `PeerPhase` free-run nudged
  by beat packets.
- Lift this into a `Clock` with peers. A peer reports tempo, beat, bar phase
  and whether it is playing; a `Player` with SYNC on follows the clock's
  master through the same `PeerPhase` code whether the master is a local
  `Player` or a CDJ on the wire. A local `Player` can also be master, and then
  the Link sender broadcasts its beats as it does now.
- **Player numbers.** Every deck that announces on Pro DJ Link is a player
  and needs a number (1–4; #37 arbitration is not built). Default for a
  multi-player process: one `Player` is visible on Link; the others follow it
  through the local clock. Making each one a Link player is a later option
  that waits on #37.
- The same rule as today for test instances on the LAN: receive-only, never
  master, while real decks are in use.

## Phases

### Phase 0 — extract `Player`, no behaviour change

Mechanical. Move `AudioHandle` + processor out of `audio.rs`, and out of
`DeckApp` the transport, cues, loops, SLIP, grid, waveform, tags and the
loader. `DeckApp` keeps the screen, faceplate, browser, Link and window and
holds one `Player`. Delete `crates/engine`.

Gate, run before and after every step:

- `scratch cue_hold.sh`: press → "proc: seek" within a few ms, release →
  "(paused)" seek within a few ms.
- Idle CPU baselines: ~11 % phone layout, ~27 % iPad layout at 10 fps.
- `make perf` (R3 realtime factor guard) and `cargo test -p opendeck-transport`.
- A TestFlight build on the phone and iPad, CUE feel checked by hand.

### Phase 1 — `Output` with N rings

The processor keeps its ring; it hands the consumer to `Output` instead of
opening a stream. Routing table, per-player gain, per-player fade. XDJ app:
N = 1, output identical. Verify with the null-sink recording recipe (every
stop lands at 0.0, release → paused seek ≤ 4 ms).

### Phase 2 — `Clock`

Extract master/sync/PeerPhase from `prodj.rs` into `crates/sync` with the two
peer kinds. XDJ app behaviour on Link unchanged (the `PeerPhase` unit tests
and the two-device home test cover it). New: two `Player`s in one process
sync to each other with no network.

### Phase 3 — the new instrument

A new app crate with its own UI. The screen-fidelity rule does not apply to
it; that rule exists for the XDJ reproduction only. What it is, below.

### Phase 4 — more decks, if wanted

`players.push(Player::new())` and a layout.

## The product decision (open)

Three instruments the `Player` primitive can become. Phases 0–2 serve all
three, so the decision can wait until phase 2 is done; it should not wait
longer than that, because phase 3's UI is the real work.

1. **The sampler deck** ([`sampler-deck.md`](sampler-deck.md), #24). One deck,
   one jog, one screen, plus a pool of 4–8 voices reading the same decoded
   buffer: auto-extracted one-shots, quantized triggers, rolls, arm-to-grid,
   a step sequencer over regions. The voice is the phase-0 cursor; the pool
   is N of them under one `Player`. Nothing else on the market is this, and it
   is what the 2024 "custom firmware" wish was asking for.
2. **The dual-layer deck** (WORKSTREAMS C4). Two `Player`s under one control
   surface, a layer switch, separate or summed outputs. Denon ships this on
   the SC6000; Pioneer does not. Needs phases 0–2 and little else.
3. **A two-deck app.** Two `Player`s, two screens, a crossfader. Useful on an
   iPad, but it is the mixer the non-goals excluded, and it competes with
   everything.

Recommendation: aim phase 3 at the sampler deck, with the dual-layer deck as
the second product on the same crates. Both are one-deck instruments, which is
where this project's identity is; neither needs a four-deck layout. The
two-deck app stays possible without being the goal.

## Budgets

- Rubber Band R3 per key-locked `Player`: ~10 % of a phone core; ~0.6 of a
  core for the whole app on a Pi 5; ~72 % of a Pi 4 core. The Pi 5 carries
  two key-locked players; the Pi 4 one. Sampler voices default to the cheap
  varispeed cursor ([`varispeed-engine.md`](varispeed-engine.md)); key-locked
  voices are opt-in and counted against this budget.
- The cue-latency chain (`Sense::drag` press, `set_playing` unpark,
  seek-while-paused, ring flush wait) and the no-fade-in rule move into
  `Player` unchanged and are gated in phase 0.
- Idle pacing stays the app's business: a `Player` that is paused parks its
  thread, as now; the app decides the frame rate.

## Open questions

- Does `Player` own its loader thread, or does the app own one loader and
  hand `Prepared` tracks to players? (Lean: one loader per process; a
  `Player` only needs `apply_prepared`.)
- Per-voice key lock in the sampler: how many R3 instances before the Pi 5
  runs out? Measure with `make perf` once voices exist.
- Should the dual-layer deck's second layer be a Link player of its own?
  Waits on #37.
- Where the segment map (regions with roles) is stored and whether it rides
  along in the served library (#44) so other decks see the same cues.
