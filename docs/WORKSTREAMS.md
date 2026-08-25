# Workstreams

Everything currently open, what each unlocks, and what blocks what. Written to
answer "where do I go first" rather than to be a complete backlog.

Current state as of `26737d5`: a single-deck player that decodes a file from
`argv`, renders a scrolling three-band waveform with a beat grid, takes MIDI
from a DJ2Go, and shows an external deck's tempo from received ProDJ Link beat
packets. Five of the twelve crates — `engine`, `db`, `ui`, `timecode`,
`protocol` — are not compiled into the binary at all.

---

## Milestone 1 — a deck you can actually DJ with

Set 2026-08-25. **A running deck on commodity hardware that responds to an
existing USB DJ controller (Dan's NI Kontrol S2 MK2) and syncs over ProDJ
Link.** The analysis we have (MiniBPM grid, waveform) is enough for this;
no new DSP.

What "syncs over Link" means here, concretely:

1. **Seen** — freedj announces on 50000 and the XDJ-1000MK2 lists it as a
   player.
2. **Heard** — freedj sends beat (50001) and status (50002) packets derived
   from its own grid and *audible* position, so the XDJ's phase meter shows
   us.
3. **Follows** — with SYNC on, freedj sets its tempo to the master's
   effective BPM. Phase alignment (nudging to land on the master's beat) is
   the stretch goal; tempo follow alone already makes the phase meter and
   the B2 strip sit still.

Not in M1: key detection, cues/loops, library, touch, any custom hardware.

### Dependency chain

```
INPUT_PLAN step 1  ControlEvent bus + Deck::apply      ─┐
G1a  DJ2Go MIDI adapter first; S2 MK2 HID after (bc4a6dd^) ├─▶ controller drives the deck
INPUT_PLAN step 2  event log / --script / --record      ─┘   (and is regression-tested)

A1   sample-rate conversion (48 kHz devices play sharp) ─┐
B3   --device output selection                          ├─▶ sound is right on any box
A3a  xrun counter (know when it glitches)               ─┘

B2   Link send: announce + beat + status, --player N    ─┐
B4   status packet parsing (master flag, master's BPM)  ├─▶ seen, heard, follows
B5   SYNC: tempo-follow the master (new)                ─┘
```

Verification is the XDJ on the desk plus `make virtual-cdj`: the harness for
day-to-day, the unit for the final check — the harness cannot tell us whether
Pioneer firmware accepts *our* packets, only whether we parse *theirs*.

### S2 MK2 note

The Kontrol S2 MK2 is **HID, not MIDI**. Its mapping was reverse-engineered
in March and lives at `git show bc4a6dd^:crates/app/src/midi.rs`: VID 0x17CC
PID 0x1320; jog wheel is a 24-bit absolute counter in report bytes 1–3
(deltas by wrapping subtraction); platter touch byte 10 bit 0; play byte 11
bit 0, cue bit 1; pitch fader byte 7, centre calibrated from the first
report. `hidapi` is still in the workspace. It returns as an adapter
emitting `ControlEvent`s on the bus alongside the DJ2Go MIDI adapter — the
first proof that the bus design holds for two different device classes.

---

## A. Foundations

Not visible, but everything downstream inherits these.

### A1. Sample-rate conversion — **broken, small**

`audio.rs:138` logs *"device sample rate != file sample rate — pitch will be
wrong"* and plays anyway. PipeWire and most USB interfaces run at 48 kHz; most
tracks are 44.1 kHz. That is +8.8%, about a semitone and a half sharp, on a
large fraction of machines. `ResampleStage` exists in
`crates/timestretch/src/lib.rs` but its `process()` is a passthrough with a
`TODO: run rubato SincFixedOut`.

Blocks nothing technically. Makes the deck unusable on other people's hardware.

### A2. Position truth — **done** (`ea6dd13`)

`position` is the decoder's cursor and ran ~93 ms ahead of what is audible.
`AudioHandle::in_flight` now publishes that distance (ring buffer contents plus
stretcher latency, in source samples) and the renderer subtracts it.

This also fixed the waveform flicker, which turned out to be the same bug wearing
a different hat: `position` advances one `BLOCK_FRAMES` (11.61 ms of audio) at a
time from a thread sleeping 0–8 ms between blocks, so 37% of frames showed zero
movement and the rest lurched two to four blocks. `render_frame` now free-runs a
phase-locked playhead against the audio clock. Stalled frames 37.2% → 0.0%.

Frame pacing was the second half of the same problem and is also done
(`see git log`). The renderer had three competing clocks — a CPU `WaitUntil`
timer, the Fifo swapchain's acquire block, and the compositor — and none was
phase-locked to the display; only 59% of frames landed in their vsync slot.
Root cause: winit on Wayland only requests the compositor frame callback if the
app calls `window.pre_present_notify()` before presenting, and it never did, so
`request_redraw()` was never gated (the loop free-ran at 12,500 fps once the
swapchain stopped blocking). Now: `pre_present_notify` + Mailbox + redraw from
the `RedrawRequested` handler, and the playhead advances by whole display
periods (from `refresh_rate_millihertz`) rather than measured wall-clock.
Acquire never blocks, zero double frames, one skip in 400, motion exactly one
period per frame. Not yet verified on X11, GLES, or the Pi.

### A3. Real-time thread hardening — **medium**

`audio-proc` is a plain thread doing `thread::sleep` polling — no `SCHED_FIFO`,
no mlock, no priority anywhere in the tree. Underruns are silent:
`consumer.pop().unwrap_or(0.0)` at `audio.rs:200`, with no xrun counter. On a
loaded Pi with a compositor it will get preempted and you will not be able to
tell.

Cheapest useful slice: an xrun counter first, so the problem becomes visible
before it becomes a rewrite.

### A4. Streaming loader — **large**

`samples: Arc<Vec<f32>>` holds the whole decoded track. A 6-minute 44.1 kHz
stereo track is ~127 MB, and playback cannot start until the full decode
finishes. Two decks plus preview plus a browser that loads on hover gets tight
on a Pi 5 8 GB. Instant load is a headline CDJ feature for a reason.

Defer until C1 lands — the transport refactor changes who owns the buffer.

---

## B. ProDJ Link and two-deck testing

### B1. Fix the wire format — **done** (2026-08-25)

Parser and builder now use the real 96-byte layout, verified against
`prolink_virtual_cdj` traffic (16/16 beats decoded; the captured packet is
pinned as a unit test). `send_beat.py` emits the real format on 50001.
Harness: `make virtual-cdj` — see `docs/reference/link-test-harness.md`.
Original finding kept below for the record.


`build_beat()` and `parse_beat_packet()` in `crates/link/src/prodj.rs` disagree
with each other. `build_beat` writes the player number at byte 26 and BPM at 44;
the parser reads 0x10 and 0x24. Only `tools/send_beat.py` works today, because
it was written against the parser. Neither matches the real protocol:

```
0x00–0x09  magic  51 73 70 74 57 6d 4a 4f 4c 28
0x0a       type   0x28
0x0b–0x1f  device name
0x21       player number
0x24–0x3b  next beat / 2nd / next bar / 4th / 2nd bar / 8th  (u32 BE, ms)
0x54–0x57  pitch  (0x00100000 = 0%)
0x5a–0x5b  BPM    (u16 BE, x100)
0x5c       beat within bar, 1–4
           96 bytes total
```

Source: <https://djl-analysis.deepsymmetry.org/djl-analysis/beats.html>

Also: real hardware sends beat packets on **UDP 50001**; the app listened only
on 50002 until 2026-08-25, so an XDJ on the LAN would have been silent. Both
ports are bound now, and the beat-port log prints full packet hex. **Ground
truth for this item is the XDJ-1000MK2 on the desk** — plug it in over
ethernet, `make dev`, and capture what it sends. Fix the format to the
captured packets, not to the simulator.

Doing this first means the two-deck test *is* the compatibility work, rather
than validating a private format that proves nothing about real gear.

### B2. Link send — **medium**, needs A2 and B1

Announce packets (0x06) every 1.5 s, beat packets (0x28) at each beat onset
derived from our own grid and position, a `--player N` flag. `build_announce`
and `build_beat` are already drafted; they need correcting and calling.

Unlocks screen callouts 15 (player number) and 26 (MASTER/SYNC), and makes
`make two-deck` launch two real freedj instances that see each other.

### B3. Audio device selection — **small**

`AudioHandle::open` takes the default output device. Two instances on one box
need `--device` so they do not fight, and a real deck needs to choose its
interface anyway.

### B4. Status packet parsing — **medium**

`parse_status_packet` returns `None` with a TODO. Receiving track metadata and
playback state from real CDJs is what makes the second grid show more than a
tempo.

---

## C. Transport and multi-deck

The key structural observation: **"two layers in one box" and "two decks on a
network" are the same engineering.** Both need the transport to be a thing you
can have two of. Only the routing differs.

Denon's SC6000 Prime already ships the dual-layer version — two tracks on one
unit with two independent outputs. Pioneer's CDJ line does not. The gap is real
and the ergonomics are proven.

### C1. Transport as a value — **large**

Today `AudioHandle` owns one `Arc<Vec<f32>>` and `main.rs` builds the entire app
around a single deck. Refactor so a transport is constructible N times, with its
own position, speed, key-lock and grid.

**Unlocks C4, and is shared with the appliance model.** Everything in section C
waits on this.

### C2. Wire `crates/engine` into the binary — **medium**

550 lines of transport, slip, loops and hot cues exist and are imported by
nothing. `Transport::read_frame_at` at `transport.rs:373` returns `[0.0, 0.0]`
with a TODO — wired up as-is it would output silence. It needs a positional
buffer read, which C1 is already touching.

Converts the largest single block of missing screen elements at once: callouts
1, 3, 6, 8, 18, 19, 20, plus cue and loop overlays on both waveforms.

### C3. Load / eject — **medium**, needs C1

One call site today (`main.rs:377`). Prerequisite for the device icon (4), track
number (17), and any library work.

### C4. Dual-layer playback — **medium**, needs C1

Two tracks in one process with independent transports. The novel feature, and
the answer to "DJ with a single deck".

Open question worth settling early: **two output pairs, or an internal mix?**
Two pairs matches Denon and keeps the "this is a deck, not a mixer" line from
`PLAN.md`. An internal mix is more useful with one soundcard but starts building
the mixer the non-goals rule out.

### C5. Output routing — **medium**, needs C4

Multichannel device handling, per-deck gain, headphone cue bus. Currently output
is "whatever the default device is, mixed to `device_ch`."

---

## D. CDJ visual parity

Detail in `docs/reference/cdj-3000-playback-screen.md`. Tally: 2 built, 5
partial, 22 missing, of which 17 are small.

### D0. Panel geometry — **decide, free now, expensive later**

The README targets 1280×480 (8:3). The CDJ playback screen is a vertical stack —
track info, phase meter, enlarged waveform, info block with the overall waveform
inside it — and that stack does not fit an 8:3 letterbox. Which is roughly why
what exists today *is* just the waveform and a thin strip.

Recommendation: make 16:9 the reference layout, treat 1280×480 as a compact
variant. Everything in D is downstream of this.

### D1. `DeckSnapshot` — **small**, do before the rest of D

`Renderer::render()` takes eight positional arguments and has grown one nearly
every session — `fader_speed`, `beat2_bpm` and `beat2_phase_beats` were the last
three. Twenty-nine screen elements will not survive that signature.

Collapse to one struct carrying everything the deck knows. Half a day. It is
also the insurance for the "then innovate" half of the strategy: with a
snapshot, the classic CDJ layout is *a* consumer of deck state rather than *the*
architecture.

### D2. Info block — **small**, needs D1

The bottom band: time as REMAIN with msec, speed as signed percent, tempo range
badge, player number, MT indicator, quantize and beat-jump values. Nine
elements, almost all fed by state that already exists.

### D3. Overall waveform — **small**

Highest payoff per hour on the screen. `WaveformCache` already holds every
column in the GPU storage buffer; this is a second pass at a different zoom.
Note from the manual screenshot: it is not a standalone strip — it sits inside
the info block with hot-cue letter markers above and a time scale below.

### D4. Waveform modes — **small**

Zoom steps (`cols_visible` is hardcoded to `600.0` at `renderer.rs:140` and
`:311`), colour modes (`BLUE` and `3 BAND` alongside the `RGB` already
implemented), and `CENTER` / `LEFT` playhead position. All shader constants or
uniform fields over existing data.

### D5. Phase meter as a view mode — **small**

The B2 strip is this element in embryo. On the CDJ it *replaces* the enlarged
waveform via a touch toggle and shows bar/beat deviation from the sync master as
a horizontal bar. Reshape rather than reinvent. Becomes fully meaningful with
B2.

---

## E. Analysis

### E1. Key detection — **large**

The only genuine DSP hole. `crates/analysis/` is `beat.rs` and `waveform.rs`
only. Chromagram plus a Krumhansl-style profile match. Unlocks callouts 28 (key)
and 7 (KEY SHIFT), and Key Sync later.

Worth starting early precisely because it cannot be rushed at the end.

### E2. Full-track beat analysis — **medium**

`beat.rs:57` analyses `samples[samples.len() - ac_len..]` — the **last six
seconds of the track**, which for most tracks is an outro or a fade. One window
also cannot handle tempo drift or a live drummer. Full-track analysis plus the
manual grid editor already listed as a top contribution ask.

This is a correctness bug hiding as a design choice, and it is cheap to test:
any track with an ambient outro should mis-detect today.

---

## F. Library — rekordbox is the interop target

Decision 2026-08-25: interoperate with the CDJ ecosystem, i.e. rekordbox's
formats, and do not build a Mixxx-specific path. Mixxx reads rekordbox USB
exports but cannot write them (open since 2018, mixxxdj/mixxx#9463), and its
own analysis lives in a private sqlite/protobuf format nothing else consumes.
Reading rekordbox exports gives Mixxx interop for free; writing them would
give it in the other direction.

### F1. Read rekordbox USB exports — **medium**, the real "drop-in" feature

`PIONEER/rekordbox/export.pdb` (DeviceSQL: tracks, playlists, key, colour)
plus `PIONEER/USBANLZ/**/ANLZ0000.{DAT,EXT,2EX}` (beat grid, hot and memory
cues, waveforms including 3-band, phrases). A DJ's stick from a CDJ then
loads with *their* grids and cues instead of our MiniBPM guess. Use
`rekordcrate` (Holzhaus — a Mixxx maintainer; built on Deep Symmetry's
crate-digger). Read-only, actively developed, same lineage as the Link docs.
Unlocks screen callouts 4, 17, and the BROWSE screen; the cue data unlocks
the cue overlays once the transport is wired (C2).

### F2. BROWSE screen — **medium**, needs F1

`crates/db` (SQLite + FTS5) becomes the cache/index over imported exports and
our own analysed files.

### F3. Network library (dbserver, port 1051 via 12523 lookup) — **large**

Browse a linked CDJ's USB from freedj, and eventually serve ours. Same
ecosystem as Link; later.

### F4. Write rekordbox exports — **large, low priority**

Producing PDB/ANLZ from our analysis. No open-source implementation does it
well; DeviceSQL writing is under-documented and rekordcrate does not write.
Noted so it is not planned around.

Borrow from Mixxx: algorithms, not formats. libKeyFinder for E1 (key
detection), as MiniBPM was for tempo.

## G. Hardware and control

See `docs/INPUT_PLAN.md` for the input architecture: one `ControlEvent` bus
fed by keyboard, mouse/touch, MIDI, a scripted simulator (file / stdin / UDP,
with realistic jog profiles and session recording), and later the RP2350.
G1 below becomes an adapter on that bus; G2 is unchanged.


### G1. MIDI mapping system — **medium**

`midi.rs` has hardcoded constants for one DJ2Go. A mapping file format plus MIDI
learn turns that into a platform and makes the project run on hardware
thousands of people already own. Mixxx's mapping ecosystem is the proof.

### G2. RP2350 control surface — **large**

The expensive half of "drop-in replacement". Until a jog wheel exists, the claim
is "a deck that looks like a CDJ", which is worth making accurately.

---

## Suggested order

Dependencies, drawn out:

```
A2 position truth ──┐
B1 wire format   ───┴──> B2 Link send ──> D5 phase meter
                                     └──> callouts 15, 26

D0 panel geometry ──> D1 DeckSnapshot ──> D2 info block
                                      └──> D3 overall waveform
                                      └──> D4 waveform modes

C1 transport ──> C2 wire engine ──> cues, loops, slip, callouts 1/3/6/8/18-20
            └──> C3 load/eject ──> F library
            └──> C4 dual layer ──> C5 routing

E1 key detection ──> callouts 28, 7        (independent, start early)
```

Three defensible first moves, depending on what you want to be true soonest:

1. **"It sounds right and two boxes sync."** A1 + A2 + B1 + B2. Smallest total
   scope, fixes two correctness bugs, and ends with `make two-deck` running two
   real instances. Serves the stated Link-compatibility goal directly.

2. **"It looks like a CDJ."** D0 + D1 + D2 + D3 + D4. Almost entirely over data
   that already exists, and the familiarity wedge is the stated strategy. Does
   not fix A1/A2, so the thing that looks right still plays sharp on a 48 kHz
   device.

3. **"It does something no CDJ does."** C1 + C4. Biggest scope, and the transport
   refactor is shared with everything in C — but it is the differentiator, and
   doing it before D means the layout is designed for two layers from the start
   rather than retrofitted.

The one thing worth doing regardless of which you pick is **D1** — the render
signature gets worse every session it is left alone, and all three paths add
state to it.
