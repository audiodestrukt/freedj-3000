# Design: OpenDeck as an engine, players and features as mods

Status: **proposal, not decided** (2026-11-14). Nothing here is built.
Builds on [player-crate.md](player-crate.md) (lifting the deck engine out of
the app) and [transport-rules.md](transport-rules.md) (CUE / PLAY as a pure,
spec-checked state machine).

## The idea

OpenDeck started as a reproduction of one player, the XDJ-1000MK2, and grew a
second, the CDJ-3000. Both are the same engine underneath: the same decoder,
key lock, transport, loops, cues, analysis, library and Pro DJ Link. What
differs is the screen, the controls, and a handful of behaviour rules.

So turn it around. **OpenDeck is an engine. A player is a mod on top of it.**
The CDJ-3000 screen and feature set is one mod; the XDJ-1000MK2 is another;
the sampler deck ([sampler-deck.md](sampler-deck.md)) would be a third. A DJ,
or a modder, can change a player or write a new one without touching, or
being able to break, the engine.

## Why now: the community is already modding CDJs

DJs are modding their CDJ firmware. OverCue, for example, adds stems
playback to a CDJ-3000 by injecting a library into Pioneer's player process.
It works, and people want it. But that kind of mod:

- patches a closed binary, so every firmware update can break it;
- runs inside the player's process, so a bug in the mod can stop the deck's
  audio or crash it mid-set;
- has no API: each modder re-discovers the same internals.

OpenDeck can offer what those modders are reaching for: a deck whose
"firmware" is open, with a stable, documented engine API. The player itself
is replaceable, and a mod can't take the audio down. And OpenDeck already
runs as firmware on CDJ-3000 hardware in the emulator
([../hardware/cdj-3000-install.md](../hardware/cdj-3000-install.md)). So the
same engine and mods could run on a CDJ-3000 itself.

## The constraint: absolute performance

The core is Rust because performance comes first: zero underruns, a
playhead that never judders, CUE landing on the beat, and a Pi Zero W playing
cleanly on one core. Mods are only worth having if the engine stays exactly
that fast. That gives one rule, which everything below follows:

**The engine's hot paths do not know mods exist.**

- **The audio path** (decode → key lock → ring → device callback) never calls
  mod code, never waits on a mod, and never takes a lock a mod can hold.
- **The UI frame** never waits on mod code. Mods get the state the frame
  already builds, and send requests into a bounded queue that the frame drains
  the way it drains controller input.

With no mods, the cost is an empty-queue check per frame. With mods, the
engine's extra work is a copy of a few hundred bytes of state per frame plus
applying events at controller rates. Everything a mod itself costs runs below
the priority of `audio-proc`, which runs SCHED_FIFO where permitted (see
[rt-audio-isolation.md](rt-audio-isolation.md)).

**A player mod is first-party Rust by default.** The CDJ-3000 and XDJ
personalities ship compiled in (option A below), so they are exactly as fast
as today. They just go through the engine API instead of reaching into
`DeckApp`. The isolation options only matter for third-party code.

## Three layers

```
┌──────────────────────────────────────────────────────────────┐
│ Feature mods     add to a player: a panel, a control mapping,│
│                  a lighting / visuals / logging tool, a voice │
├──────────────────────────────────────────────────────────────┤
│ Player mods      CDJ-3000 · XDJ-1000MK2 · sampler · your own  │
│                  screen, controls, behaviour rules (specs)    │
├══════════════════════════════════════════════════════════════┤
│ Engine           decks · transport primitives · key lock /    │
│  (Rust, owns     key shift · loops · cues · SLIP · analysis · │
│   the audio)     library · media server · Pro DJ Link · output│
└──────────────────────────────────────────────────────────────┘
          the engine API (the double line) is the only way in
```

### The engine

Everything with timing or audio in it stays here, in Rust, owned by the
engine:

- the audio path and the ring;
- transport *primitives*: play, stop with its 3 ms fade, seek, cue
  preview, loop wrap inside a block;
- SLIP's shadow playhead;
- key lock and key shift;
- quantize;
- the beat clock;
- analysis, the library and Link.

A player never re-implements these. It chooses between them and configures
them.

### Player mods

What makes a CDJ-3000 a CDJ-3000 rather than an XDJ:

- **Screen:** layout, panels, waveform style, colours.
- **Control map:** physical and touch controls → engine events. The CDJ-3000
  has eight hot cues, the XDJ four per bank; the CDJ-3000 has KEY SHIFT; and
  so on.
- **Behaviour rules:** when CUE previews versus sets, what AUTO CUE does,
  what SNAP lights after. These are already moving into data: the transport
  rules crate checks behaviour against `specs/xdj-1000mk2/`. A player would
  ship its own specs, and the engine runs whichever rule set the player
  selects. Rules are decisions at button-press time, not per sample, so this
  costs nothing on the hot path.
- **Settings:** its MENU / UTILITY, saved under the player's name.

Today all of this is mixed into the app crate (`cdj3000.rs`, `cdjface.rs`,
`screen.rs`, branches in `lib.rs`). Moving each skin behind the engine API is
the refactor, and it's the same work as the player-crate plan's phases.

### Feature mods

Smaller additions on top of a player:

- a panel;
- a remapped controller;
- a lighting or visuals bridge;
- set logging, a practice tool, a cue sequencer;
- later, a sound voice.

## The engine API

Two seams that already exist, made formal:

- **Commands in.** Every key, pad, controller and touch is already an `Event`
  (`ControlEvent` / `UiEvent`) applied in `DeckApp::apply`. That becomes the
  command set, versioned, with permissions.
- **State out.** Each frame builds a `DeckSnapshot`: playhead, grid, cues,
  loop, tempo, key, master. That becomes the state stream. Track analysis
  (grid, waveforms, phrases, cues) and library queries are on-demand reads.
  Pro DJ Link already publishes much of this to the network.

A third seam is new and only for players:

- **Drawing.** A player describes its screen as layers the engine renders:
  the waveform, overview, grid and cue markers stay engine-drawn GPU passes,
  because they're performance-critical. The player draws text, keys and
  panels around them through egui, as today. Third-party players get a
  declarative version of the same thing (option C).

## Where mod code runs

The API is the same for every mod; how its code is hosted depends on whose
code it is.

### A. Compiled-in Rust (first-party players and features)

Built into the binary behind cargo features, calling the engine API directly.

- **Performance:** identical to today.
- **Safety:** code review only; it runs in-process.
- **For:** the CDJ-3000 and XDJ players, the sampler, anything we ship.

### B. Out of process, over a local socket (tools and controllers)

The mod is its own program, in any language. It connects on a local or LAN
socket, gets the state stream, and sends commands.

- **Performance:** the engine gains one listener thread. A slow mod gets the
  latest state and drops the rest; the engine never waits. The OS keeps its
  CPU off the audio core.
- **Safety:** the strongest available. A crash or loop stays in its own
  process.
- **Platforms:** desktop and Pi, and LAN-only for an iPad (iOS can't run
  other programs).
- **For:** lighting and visuals, set logging, remote control from a phone or
  laptop, automated testing. Deckbench already drives OpenDeck this way, and
  Link is a read-only version of it.

### C. In-process WebAssembly (third-party players and features)

The mod is a `.wasm` module run on a dedicated thread by an embedded runtime,
with a CPU budget (fuel or epochs) and a memory cap.

- **Performance:** near-native on desktop (wasmtime). Interpreted on iOS
  (wasmi; iOS forbids JIT), around 10× slower, which is fine for UI and
  rules, not for audio DSP. The runtime adds megabytes to the binary. On a
  one-core Pi Zero W the mod thread shares the audio's only core.
- **Safety:** good. A trap or a timeout disables the mod; the engine keeps
  playing.
- **Platforms:** all, including the iPad.
- **For:** community players and features that should work on every device
  without a rebuild. This is the closest thing to "drop a mod on the stick".

### Not supported: native libraries

Loading a `.so` / `.dylib` into the process, the way CDJ mods inject into
Pioneer's player, would be fast but would let a mod do anything the engine
can: block the audio, corrupt memory, crash the deck. It's ruled out for
third-party code. A modder who wants native speed can contribute a Rust player
or feature to the repo (A).

### Comparison

| | A. Compiled-in Rust | B. Out of process | C. WASM | Native libs |
|---|---|---|---|---|
| Engine cost, no mods | 0 | ~0 | ~0 (+ binary size) | 0 |
| A mod crashes | deck dies | mod dies | mod disabled | deck dies |
| A mod loops forever | deck hangs | its own core busy | stopped by budget | deck hangs |
| On the iPad itself | yes | no (LAN only) | yes (interpreted) | no |
| Who writes them | us, contributors | anyone, any language | anyone | — |
| Can be a whole player | yes | no | yes | — |

## Mods that make sound

The hard part, and separate from the rest. Stems are the community's own
example.

- **Voices** (samplers, one-shots, extra stems): the mod renders ahead into
  its own ring at lower priority. The output mixes that ring in if samples
  are there and plays nothing if not. A late or dead mod is silence on its bus;
  the deck plays on.
- **Processing the deck's own audio** (effects, stem separation): the mod
  must hear the audio before it plays, which puts it in the chain the rule
  keeps it out of. The safe form runs it in parallel and falls back to the
  dry signal on any late block. That's twice the work for that chain, and the
  effect audibly drops out on a late block. Stems in OpenDeck today are
  engine features (the OverCue reader, on-device HTDemucs), not mods, for
  exactly this reason.

**Proposal:** voices as mods; processing stays in the engine until a concrete
mod needs it.

## Show safety

- **Watchdog:** each mod has a CPU and memory budget and an event-rate limit,
  well above any controller's. Over budget means it's disabled for the
  session, with its name shown on screen.
- **Performance lock:** no installing, updating or re-permissioning mods
  while a track plays.
- **Safe mode:** a start-up option, and a key held at boot, that starts the
  built-in player with every mod off.
- **Fallback player:** if a third-party player mod fails, the deck drops to
  the built-in CDJ-3000 player with the same track, position and cues. The
  engine still holds all of that, so nothing is lost but the look.
- **Permissions, declared per mod:**
  - read-only (state, analysis, Link);
  - cues and loops;
  - transport;
  - tempo and key;
  - load tracks;
  - own panel;
  - own audio bus.

## How to prove it: hostile-mod tests

Each guarantee becomes a test: play a track on the headless deck harness
(`deck_harness`, the real audio processor) while a deliberately broken mod
runs:

- a mod that spins forever;
- one that allocates until it's killed;
- one that crashes on its first state update;
- one that floods 10,000 events a second;
- one that connects and never reads;
- a voice that stops filling its ring mid-bar;
- a player mod that traps while drawing.

A test passes only with **zero underrun events**, the playhead where it
should be, and PLAY / CUE still answering. They run in CI. On the Pi Zero W the
same suite runs by hand.

## A path, if we go ahead

1. **Engine API inside the app.** Do player-crate phases 0–2. Then move the
   XDJ and CDJ-3000 players behind the API as compiled-in mods (A). No
   behaviour change; measured with `make perf`, the underrun counters and
   [`../BENCHMARKS.md`](../BENCHMARKS.md) before and after.
2. **State stream out (B, read-only).** Lighting, visuals, logging, remote
   display. Zero risk to the deck.
3. **Commands in, with permissions, the watchdog and the hostile-mod
   tests.**
4. **WASM players and features (C).** Community mods on every device.
5. **Voices as mods.** Last, and only for a real mod that needs it.

Steps 1–3 are worth doing even if no third-party mod ever ships: step 1 is
the player-crate plan, and steps 2–3 are the remote-control and test API.

## Open questions

- Is the first non-built-in player a community one, or ours (the sampler)?
- How much of the screen should a third-party player control? Engine-drawn
  waveforms with a mod-drawn frame is fast; fully mod-drawn screens would
  need a GPU budget per frame.
- Wire format for B: something schema'd (FlatBuffers, Cap'n Proto) or
  JSON lines for approachability? Either is small at 60 Hz.
- Distribution: files the DJ copies onto a stick (like CDJ mods today), a
  catalogue, or both? And signing?
- When OpenDeck runs on a CDJ-3000 itself, do mods install the same way?
