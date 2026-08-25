# Workstreams

Everything currently open, what each unlocks, and what blocks what. Written to
answer "where do I go first" rather than to be a complete backlog.

Current state as of `26737d5`: a single-deck player that decodes a file from
`argv`, renders a scrolling three-band waveform with a beat grid, takes MIDI
from a DJ2Go, and shows an external deck's tempo from received ProDJ Link beat
packets. Five of the twelve crates — `engine`, `db`, `ui`, `timecode`,
`protocol` — are not compiled into the binary at all.

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

### A2. Position truth — **broken, small**

`position` is advanced by the *processor* thread at `audio.rs:314`, immediately
after reading a source block — before Rubber Band and before ~93 ms of ring
buffer (`RING_BUFFER_SAMPLES = 8_192`). Everything visual runs that far ahead of
what you hear, and the offset varies with buffer fill.

Fix: a second atomic incremented in the cpal callback by frames actually
consumed, minus stretcher latency; drive the renderer from that.

**Blocks B2.** Broadcasting beat packets derived from a position that lies puts
every other deck on the network out of sync, variably. This stops being hygiene
and becomes a prerequisite the moment Link send exists.

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

### B1. Fix the wire format — **small**

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

## F. Library

`crates/db` has SQLite with FTS5 search, beat grids, cue points and playlists
designed — and is imported by nothing. F1 wire it up, F2 the BROWSE screen, F3
the rekordbox USB export parser. All wait on C3.

---

## G. Hardware and control

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
