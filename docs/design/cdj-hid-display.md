# CDJ HID-over-LINK — native track display (0x22–0x30) + transport completion

Status: **design / not built.** Prerequisites done: HID control (0x20 input)
and the session handshake are working and validated on the emulator; the
`lanhid` surface drives freedj's deck from the CDJ, and 0x21 status is sent as
the keepalive. This doc plans the remaining half: pushing freedj's **title,
waveform, beat grid and cues onto the CDJ's own screen**, plus the transport
work (UDP tunnel + fragmentation) needed to run it on real hardware.

Source of truth for wire formats: the iPhone-CDJ HID Developer Brief (pages 3,
4, 6) + what we observed live. See also [docs/cdj-hid.md](../cdj-hid.md).

---

## 1. Goal

In HID control mode the CDJ is a surface and **freedj is the deck** (the
rekordbox role): audio plays from freedj, and the CDJ's screen must mirror
freedj's deck so you can cue/scratch on the hardware. Today the screen shows
nothing of freedj's track. This closes that: the CDJ paints freedj's RGB
waveform, scrolling detail wave, beat grid, title/artist and hot-cue markers,
updated on track load and cue edits, with 0x21 status at ~60 Hz throughout.

freedj already owns all the data (it is the analysis/playback engine), so this
is overwhelmingly **formatting existing data into the CDJ wire layout and
scheduling the transfers** — not new DSP.

---

## 2. Two transport regimes (the key structural fact)

The handshake/input code we have speaks the **emulator's USB-B socket**: whole
64-byte reports, `type(1)|len_BE(2)|payload`, no tunnel, no fragmentation. The
**real LAN path** is different and the display work forces us to confront it:

| | Emulator (USB-B Unix socket) | Real LAN (UDP tunnel) |
|---|---|---|
| Transport | Unix stream, bare 64-byte report | UDP 50000 (discovery) / 50001 (tunnel) |
| Host→CDJ frame | `type 1` + 64-byte report | 100 B: `M +04+ N("DJM-A9") +01 00 21 00 40+` **routed payload64** |
| CDJ→Host frame | `type 1` + 64-byte report | 68 B: `M +04+ N("CDJ-3000") +01 00 p 00 20+` **routed payload32** |
| Payload offset | byte 3 | UDP byte 36 |
| Route byte | — (byte 0 already 0) | byte 0 = `(player<<4)|kind`; replaces canonical byte 0 |
| **Fragmentation** | host display frames only | **both ways** — CDJ payload is **32 bytes**, so inbound frames > ~26 B span multiple reports |

Consequence worth calling out: on the real UDP path the CDJ→host payload is
**32 bytes**, so even the **handshake 0x13** (LEN 0x2a = 42 B) arrives across
**two** 32-byte fragments and must be reassembled. On the emulator it came as
one 64-byte report, so our current `Handshake` never had to reassemble. **The
UDP path needs inbound reassembly that the emulator path doesn't.**

Route `kind` nibble (brief p.3): `0x01` host report, `0x05` availability,
`0x09` CDJ input. Host sends `availability` + announcement every 2 s and an
empty `keepalive` (0x01, 63 zero bytes) every 400 ms.

### Decision D1 — build order across regimes
Build and validate the **display encoders + fragmentation on the emulator's
64-byte socket first** (its screen renders over `main.shm`, so we can *see* the
waveform appear — see §8). Keep the fragmentation + command encoders
**transport-agnostic** (they emit canonical 64-byte reports; the transport
wraps them). Then add the UDP tunnel (`lanhid_net`) as a second transport under
the same encoders. This gets a visible result early and defers the
hardware-only UDP testing.

---

## 3. Fragmentation layer (shared by every display command)

Brief p.6: large payloads are split into canonical 64-byte reports:

```
byte 0 : 00            (route byte applied later by the transport)
byte 1 : CMD           (0x22…0x30)
byte 2-3: seq   LE16   (1-based; fragment number)
byte 4-5: count LE16   (total fragments)
byte 6-63: up to 58 body bytes         ← 58 = 64 − 6
```
Final report zero-padded. So capacity = `58 * count` bytes; `count =
ceil(total_body / 58)`.

- **Outbound (host→CDJ):** `fragment(cmd, body) -> Vec<[u8;64]>`. Pure, trivially
  unit-testable by fragment count and byte layout.
- **Inbound (CDJ→host, UDP only):** a reassembler keyed by `(player, cmd?)` that
  concatenates `payload[1..32]` continuations until the canonical `LEN`
  (byte 7 of the first fragment) is satisfied, per brief p.4 ("Retain padding
  until the declared length is reached"). Needed for the handshake and any
  multi-fragment input on real hardware; **not** exercised by the emulator.

---

## 4. Command catalogue — wire format ← freedj data

All bodies below are the *content* that the fragmentation layer splits. Unless
noted, positions convert from freedj **interleaved source samples** to
milliseconds: `ms = sample / channels / sample_rate * 1000`.

### 4.1 Text — 0x22 title, 0x23 artist, 0x26 auxiliary
Body: NUL-terminated **UTF-16LE**.
- Source: `DeckSnapshot.title` (0x22); artist from `DeckSnapshot.tags`
  (`TrackTags`) or the rekordbox `RbTrack.artist` (0x23); 0x26 optional (key,
  comment) — defer.
- Trivial. One or two fragments. Send on track change.

### 4.2 Deck rows / assignment — 0x2A
Body: **11 rows**, each an 8-byte prefix `{row index, selected/icon (0x80=sel),
color, BPM fraction, BPM integer LE16, key, reserved}` + NUL-terminated
UTF-16LE text. (The brief warns the *older* 4-byte prefix corrupts CDJ-3000
text — use the 8-byte prefix.)
- Purpose: Browse on the CDJ picks a row to **bind a software deck to that
  player's session** ("authentication alone does not assign a deck").
- freedj is single-deck today, so most rows are empty; row 0/1 = our deck.
- **Open question Q1:** the emulator streamed 0x20 input *without* us sending
  0x2A. Is 0x2A required for the **screen** to light, or only for multi-deck
  binding? Test early (send nothing vs send a 1-row list, watch the screen).

### 4.3 RGB overview (the zoomed-out whole-track bar) — 0x2C
Body: `0xC0` + **600 records × 7 bytes** = **4201 B = 73 fragments**. Each
record: `bg RGB565 LE16 | fg RGB565 LE16 | bg height | fg height | reserved 0`.
Heights **1..40**. Clear = `0x00` + 4200 zero bytes.
- Source: `WaveformCache.columns: Vec<[u8;4]>` (R,G,B,amplitude). Resample N
  columns → **exactly 600** (area/max-pool, not point-sample, so transients
  survive). Per output column:
  - `RGB565 = ((R>>3)<<11)|((G>>2)<<5)|(B>>3)`.
  - `fg height = 1 + round(amplitude/255 * 39)`.
  - **D2:** freedj has one amplitude; the CDJ overview has *two* bars (bg/fg).
    Options: (a) bg = fg (flat) — simplest; (b) bg = a slower envelope / peak
    hold, fg = RMS — richer. Start with (a).

### 4.4 Scrolling detail waveform — 0x2E
Body: `point_count LE32` + one **LE16 per point**:
`(R3<<13)|(G3<<10)|(B3<<7)|(height5<<2)` — RGB 0..7, height 0..31. Timebase
**150 points/sec**.
- Size: 5-min track ≈ 45 000 pts → 90 004 B → **~1552 fragments**. The big one.
- Source: `WaveformCache` at ~86 col/s → **resample to 150/s** (interpolate).
  Per point: `R3 = R>>5` (etc.), `height5 = amplitude>>3`.
- This is the transfer that needs pacing and cancel-on-reassign (§6).

### 4.5 Beat grid — 0x2F
Body: `beat_count LE16` + records `{ beat_in_bar (1..4), total_ms LE24 }` (4 B
each). "Use actual beat timestamps, including variable tempo."
- Source: `BeatGrid`.
  - Variable tempo: iterate `beats: Vec<u64>` (per-beat sample offsets).
  - Constant: generate from `anchor_sample` + period (`period = 60/bpm *
    sample_rate` frames) across the track.
  - `beat_in_bar = ((index + downbeat_offset) % 4) + 1`.
  - **D3 / Q2:** confirm whether `BeatGrid.anchor_sample`/`beats` are **frames**
    or interleaved samples before the ms conversion (snapshot positions are
    interleaved; grids may be frames). One factor-of-channels bug waiting here.

### 4.6 Hot cues — 0x30
Body (52 B, one fragment): `duration {min, sec, ms LE16}` then **8 slots × 6 B**
`{type, color, min, sec, ms LE16}`. Stored `type=1`, empty slot = six zeros.
- Source: `CueMap.hot_cues: [Option<CuePoint>;8]` (NOT the bare
  `DeckSnapshot.hot_cues: [Option<u64>;8]`, which lacks color).
  - `CuePoint.color: Rgb` → nearest CDJ palette ID from the working set
    `A..H = 04 12 3A 08 0C 3C 0E 01` (nearest-RGB match; freedj's 8 named `Rgb`
    consts map cleanly).
  - position samples → min/sec/ms.
- Send on track change **and after every cue add/move/delete**. "A nonzero
  stored-cue color supplies colored illumination and markers; zero switches the
  indicator off" — so clearing a slot = six zeros re-sent.

### 4.7 Not a separate command: loop shading
Brief p.6: native loop shading comes from **0x21 state + endpoints**, not a
bitmap. Already covered by `lanhid_status`; just ensure loop fields populated.

---

## 5. Scheduling, pacing, cancellation

The deck must keep feeling live while a 1500-fragment wave transfers.

- **Continuous:** 0x21 status at ~60 Hz (already the keepalive cadence). Never
  blocked by a transfer.
- **On track load/change:** (re)send 0x22/0x23 (text) → 0x30 (cues) → 0x2F
  (grid) → 0x2C (overview) → 0x2E (scroll, last/largest). Cheap-first so the
  screen populates progressively.
- **On cue edit:** resend 0x30 only.
- **Pacing (D4):** interleave display fragments with the status cadence —
  e.g. a bounded send queue drained at N fragments per status tick, so the
  ~1552-fragment scroll spreads over a second or two without starving 0x21 or
  the inbound 0x20. Tune N against the observed ~60 Hz packet budget.
- **Cancel-on-reassign:** a new track (or deck reassignment) **cancels the
  in-flight transfer** and starts fresh (brief: "cancel stale transfers on
  reassignment"). The send queue is cleared and a transfer generation counter
  bumped so late fragments from the old track are dropped.

---

## 6. Architecture

Keep wire knowledge in `opendeck-link`; keep data-sourcing in the app surface.

**`crates/link` (new, pure + unit-tested):**
- `lanhid_frag.rs` — `fragment(cmd, body)` + the inbound reassembler.
- `lanhid_display.rs` — encoders: `title()`, `artist()`, `overview(&[Rgb+amp])`,
  `scroll(...)`, `beat_grid(&BeatGrid-like)`, `hot_cues(&[Option<Cue>;8])`,
  `deck_rows(...)`. Input is **neutral** structs (not app types) so the crate
  stays dependency-light (it already avoids `opendeck-protocol`).
- `lanhid_net.rs` (phase 3) — the UDP tunnel: discovery/announce/availability,
  per-player session state, tunnel wrap/unwrap, route bytes, keepalive, and the
  inbound de-fragment. The handshake + input decode plug in unchanged above it.

**`crates/app/src/surface/lanhid.rs` (extend):**
- A `DisplaySource` adapter that converts the live deck into the neutral inputs:
  `DeckSnapshot` + `WaveformCache` + `BeatGrid` + `CueMap` → the encoder inputs,
  with the samples→ms and resampling logic. This is where app types meet the
  wire.
- A `DisplayScheduler` holding the send queue, transfer generation, and the
  track-change / cue-edit triggers; it emits fragments the session loop
  interleaves with the 0x21 keepalive.

**Transport seam:** the session loop today writes 64-byte reports to the Unix
socket. Abstract "write one canonical report (player, kind, 64 bytes)" so the
same scheduler feeds either the emulator socket or `lanhid_net`.

---

## 7. Validation

- **Unit (known-answer):** fragment counts (0x2C ⇒ 73, 0x30 ⇒ 1, 5-min 0x2E ⇒
  ~1552); exact byte layouts; RGB565 and 3-bit packing; palette nearest-match;
  samples→ms; constant+variable grid generation; reassembler round-trips a
  split handshake frame.
- **Emulator screen capture (the payoff):** the emulator renders to
  `main.shm`/virtio-gpu and the ctl API has `Lcd`/`Shot`. After sending
  0x22/0x2C/0x2E/0x2F/0x30 over the USB-B socket, **screenshot the emulator**
  and confirm the title/waveform/grid/cues actually paint. This is the same
  "does the emulator firmware implement it?" question the handshake had — must
  be tested, not assumed (the emulator source shows a `jog_screen`; whether it
  renders the main 0x2C/0x2E waveform screen is **Q3**).
- **Bandwidth sanity:** 0x2E ≈ 90 KB/track; at 64 B/report that's ~1552
  reports. At, say, 300 reports/s interleaved it's ~5 s — confirm acceptable or
  raise the rate. Measure on the emulator before worrying about Wi-Fi/DTIM on
  real hardware.
- **UDP path:** framing unit-testable; end-to-end needs real hardware (emulator
  is socket-only). Flag as hardware-gated.

---

## 8. Risks & open decisions

- **Q1** Is 0x2A deck assignment required for the screen to light, or only for
  multi-deck binding? (test cheaply, early.)
- **Q2/D3** `BeatGrid` sample units (frames vs interleaved) — confirm before ms
  conversion.
- **Q3** Does the emulator firmware render the main waveform screen from
  0x2C/0x2E at all? (screenshot test; if not, the encoders are still correct
  for real hardware but can't be seen locally — same situation as the handshake
  responder, which *did* turn out to work.)
- **D2** overview bg/fg bars from one amplitude (flat vs enveloped).
- **D4** fragment pacing rate vs the 60 Hz status budget.
- **Color fidelity** RGB565 (overview) and 3-bit (scroll) quantization of
  freedj's palette — acceptable by design (CDJ's own formats), just verify it
  reads well.
- **Scope** do we want 0x26 aux text and full multi-deck 0x2A now, or defer?

## 9. Phased build order

1. **Frag + text + cues, emulator socket.** `lanhid_frag`, `lanhid_display`
   title/artist/hotcues; wire into the surface on track load; **screenshot** the
   emulator to confirm text + cue markers paint. Answers Q1/Q3 cheaply.
2. **Grid + overview.** 0x2F, 0x2C; resampling to 600 cols; confirm the
   zoomed-out wave + beat grid render. Resolve D2/D3.
3. **Scrolling waveform + scheduler.** 0x2E, the send queue, pacing (D4),
   cancel-on-reassign. The full moving waveform under the playhead.
4. **UDP tunnel (`lanhid_net`).** Discovery, sessions, tunnel wrap/unwrap,
   inbound reassembly (so the handshake works on 32-byte payloads), keepalive.
   Moves everything from the emulator socket to real wire. Hardware-gated test.

Phases 1–3 are fully testable on the emulator today; phase 4 is what a real
CDJ (or an iPhone-as-host build) needs.

---

## 10. Live findings on cdj3k-emu (2026-11-08)

Validated with `examples/cdj_hid_host.rs` and the emulator control API's
`lcd main` capture (port 4401). These settle Q1 and Q3.

- **The deck must be in Control Mode first.** On the CDJ: SOURCE → DJ APP →
  **CONTROL MODE (CDJ)** → push the rotary twice (it warns that it will drop the
  playing local track). In normal player mode an authenticated session still
  streams 0x20 input, but the screen ignores everything the host sends. Even a
  correct 0x21 with `track_loaded` changed nothing. BROWSE opens the CDJ's own
  source browser ("Please select a source."), not the host deck list.
- **In Control Mode the native skin renders host content at once.** With the
  0x21 stream running, plus 0x2A rows and the 0x22/0x23 text each sent as one
  fragment, the screen showed the title, the duration and a 0–3:00 ruler,
  elapsed time advancing (00:19 → 00:33), BPM 128.0, ±10 and PLAYER 1, with
  CONTROL MODE in the waveform area. The fragmentation header
  `00 CMD seqLE16 countLE16` works as the brief says (0x22 rendered correctly).
- **Q3 answered:** the emulator does render host-driven display, so phases 1–3
  can all be checked against `lcd main` screenshots. Real hardware is needed only
  for the phase-4 UDP tunnel.
- **Q1, partly answered:** the single emulated player bound to the host session
  with **no** Browse row selection; the deck sent no Browse events. Keep sending
  0x2A: with two CDJs, the Browse pick is still how each player chooses its
  deck (brief p.4).
- The empty waveform field and the empty overview strip are now the target. 0x2C
  should fill the strip under the ruler, and 0x2E the enlarged waveform.
- The emulator also sends a stream of all-zero type-1 reports (`payload[1] =
  0x00`) after the handshake. They are not 0x20 input and are ignored for now.
- **0x2C overview verified (amplificate.mp3, 8:47).** The strip under the ruler
  draws the real track. The CDJ mirrors each record around a centre line and
  draws both bars: the background (here the span's peak, dimmed to half) is the
  outer edge and the foreground (the span's mean) the inner bar. That settles
  D2: peak/mean, not flat. Raw band energies drew as near-black maroon, so
  each record is now scaled so its strongest band is 255 (hue kept, loudness
  in the height). A loudly mastered track fills most of the height; that is
  accurate.
- **0x2E scrolling waveform verified.** amplificate.mp3 → 79 099 points →
  2 728 fragments (2 801 with the overview), sent through a queue that sends at
  most 64 fragments per loop pass. The 0x21 clock never stalled during the
  transfer. The enlarged waveform draws around the playhead and scrolls with
  the status time (about 36 px/s at the default zoom). Time check: the first
  silent gap sits about 25.8 s into the scroll and about 25.6 s on the 0x2C
  strip, so the 86 col/s → 150 pt/s resample is time-correct. Each point takes
  the loudest column it overlaps, with colour handled as in 0x2C.
- **0x2F beat grid verified.** The analyser's grid (141.8 BPM for
  amplificate.mp3) is expanded from the anchor in both directions:
  2 207 beats → 0x2F. The CDJ draws ticks above and below the scroll, red on
  each downbeat, and every tick lands on a kick's attack at the CDJ's zoom.
  Status BPM now comes from the grid. **Q2/D3, partly settled:**
  `BeatAnalyzerImpl` takes interleaved stereo and downmixes it, so its
  `anchor_sample` is in **frames**. Still to check: whether `DeckSnapshot`
  positions are interleaved before the app calls `grid_beats`.
- **0x30 hot cues verified.** Four cues (palette IDs 04/12/3A/08) drew the "A"
  marker at the predicted x in the scroll and lit pads A–D blue/green/violet/
  blue (ctl `lamps`). E–H stayed at the idle level. **Open:** no cue letters
  appear above the 0x2C overview strip, though a CDJ normally shows them. This
  may be emulator behaviour or need another field; check on hardware.
- **In the app (2026-11-08).** `surface::lanhid::Feed` carries `Live` →
  0x21 every 16 ms and `Assets` → light/heavy fragment sets. `DeckApp::feed_cdj`
  converts interleaved samples to ms and rebuilds assets only when a key (file,
  title, length, columns, cues, grid) changes. Running `opendeck
  amplificate.mp3` against the emulator in Control Mode, the CDJ showed the deck
  0.4 s after the handshake (2 894 fragments). Two turns of the emulated jog
  while playing moved both freedj and the CDJ clock 3.6 s (+1.4 s of play),
  which matches 1.8 s/rev.

