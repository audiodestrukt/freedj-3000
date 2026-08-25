# CDJ-3000 playback screen — parity reference

The target for FreeDJ's "classic" display mode. Element numbering follows the
CDJ-3000X instruction manual, *Part names → Playback screen (waveform screen)*,
callouts 1–29.

Run `make reference` to pull the manual pages locally. The annotated screenshot
is `reference/pioneer/manual-p-023.png`; the callout text runs across pages
23–26 and the Shortcut screen settings are on 111–112. Those pages are
AlphaTheta's copyright and are gitignored — this file is the part we own and
ship.

The 3000X is the 2025 refresh. Its waveform screen is materially identical to
the CDJ-3000 apart from callout 10 (internet / Wi-Fi status), which is 3000X
only.

## Actual layout

Taken from the manual screenshot, not inferred. Two details are easy to get
wrong from the callout list alone:

- **The phase meter is a horizontal bar** sitting between the track-info row and
  the enlarged waveform, sharing that row with the beat countdown. It is not an
  edge strip.
- **The overall waveform is not a standalone bottom strip.** It lives inside the
  bottom info block, with hot-cue letter markers above it and a time scale
  (`-4:00 … -1:00`) below.

```
┌─────────────────────────────────────────────────────────────────────┐
│ [4] │ [5] artwork · title · time · BPM · key    │ [6][7][8] [9][10] │  top bar
├─────┼───────────────────────────────────────────────────────────────┤
│ [3] │ [11] phase meter — bar/beat deviation from sync master        │
├─────┴───────────────────────────────────────────────────────────────┤
│                                                                     │
│ [2] enlarged waveform — 3-band, beat grid, cue/loop/hot-cue marks   │
│                                                        [1]  [12]    │
├─────────────────────────────────────────────────────────────────────┤
│ [15][16][17] [18] [19][20] │ [21] 03:44.802 │[22]│[23][24]│[25][26] │  info
│ [14][13]  [29] overall waveform + hot cues + time scale    │[27][28]│  block
└─────────────────────────────────────────────────────────────────────┘
```

## Status

`Built` renders and is correct for what it shows. `Partial` means the data
exists but the presentation differs. Size is implementation effort, not
importance.

### Waveform area

| # | Element | Status | Size | Notes |
|---|---------|--------|------|-------|
| 2 | Enlarged waveform | **Built** | — | 3-band colour, beat grid, centred playhead. Cue/loop/hot-cue overlays pending — blocked on the transport. |
| 29 | Overall waveform | Missing | S | Highest payoff per hour. `WaveformCache` already holds every column in the GPU storage buffer; this is a second pass at a different zoom, not new analysis. |
| 11 | Phase meter | Partial | S | The B2 strip is this element in embryo. On the CDJ it *replaces* the enlarged waveform via a touch toggle (`Waveform/Phase Meter` on the Shortcut screen), so it wants to be a view mode, not a permanent strip. |
| 3 | Beat countdown | Missing | M | Bars + beats to the nearest **saved cue** — needs cue points to be real. |
| 1 | Loop beat count | Missing | S | Blocked on `engine/loop_engine.rs` being wired in. |
| 12 | Zoom / Grid Adjust | Missing | S | `cols_visible` is hardcoded to `600.0` at `renderer.rs:140` and `:311`. Grid Adjust doubles as the beat-grid editor. |

### Track and transport readouts

| # | Element | Status | Size | Notes |
|---|---------|--------|------|-------|
| 25 | BPM | **Built** | — | Scales with the pitch fader, matching CDJ behaviour. |
| 21 | Time display | Partial | S | Elapsed-only `mm:ss.ss`. CDJ defaults to REMAIN, shows msec, and makes it headline-sized. |
| 23 | Playback speed | Partial | S | Shown as `1.02×`; every DJ reads `+2.40%`. Formatting only. |
| 24 | Speed adjustment range | Missing | S | CDJ exposes ±6/10/16/WIDE. FreeDJ clamps to ±16% in `midi.rs` with no selection or display. |
| 5 | Track information | Partial | S | Filename from `argv`. Symphonia already parses tags. |
| 28 | Key | Missing | **L** | No key detection exists — `crates/analysis/` is `beat.rs` and `waveform.rs` only. Chromagram + Krumhansl profile match. |
| 27 | MT (Master Tempo) | Partial | S | Key lock works but is always on, with no indicator or toggle. |
| 4 | Device icon / eject | Missing | M | Needs load/eject. `AudioHandle::open` has one call site, `main.rs:377`. |
| 17 | Track number | Missing | S | Needs a library. |
| 22 | CONTINUE / SINGLE | Missing | S | Needs a playlist concept. |

### Touch controls

| # | Element | Status | Size | Notes |
|---|---------|--------|------|-------|
| 8 | BEAT JUMP | Missing | S | `cmd_beat_jump` already written in `engine/transport.rs`, unreachable. |
| 6 | BEAT LOOP | Missing | M | `cmd_beat_loop` likewise. Shares the control with Slip. |
| 7 | KEY SHIFT | Missing | **L** | Rubber Band can pitch-shift independently of speed, but the control is meaningless without key detection (28). |
| 9 | Info panel | Missing | S | Cheap once tags are read. |
| 10 | Connection icons | Missing | S | 3000X only. A PRO DJ LINK indicator matters far more than an internet one. |

### Status flags and sync

| # | Element | Status | Size | Notes |
|---|---------|--------|------|-------|
| 26 | MASTER / SYNC | Missing | M | Requires ProDJ Link **send**, not just receive. Highest-value item on the screen for the deck-appliance architecture. |
| 15 | Player number | Missing | S | ProDJ Link assigns 1–4; free once announce packets exist. |
| 14 | Quantize beat value | Missing | S | Transport carries a `quantize` flag; the value isn't configurable. |
| 13 | Beat Jump beat value | Missing | S | Pairs with 8. |
| 20 | AUTO CUE | Missing | S | On by default on every CDJ in every club. Its absence is felt immediately. |
| 18 | GATE CUE | Missing | S | Play-while-held from the cue point. |
| 19 | SMART CUE | Missing | M | Beat-aligned cueing against the sync master. Depends on 26. |
| 16 | File cache | Missing | S | FreeDJ decodes the whole file to RAM, so this would always be lit. Meaningful only if a streaming loader lands. |

**Tally:** 2 built, 5 partial, 22 missing. 17 of the missing are small; the two
large ones are both key detection.

## Waveform colour modes

The CDJ offers three under `Waveform Color` on the Shortcut screen. FreeDJ
already implements one of them correctly:

| Mode | Mapping | FreeDJ |
|------|---------|--------|
| `RGB` | low → red, mid → green, high → blue | **Built** — this is what `analysis/waveform.rs` computes and the shader draws |
| `3 BAND` | low → blue, mid → amber, high → white | Missing — shader constant |
| `BLUE` | monochrome | Missing — shader constant |

Neither missing mode needs new analysis or a new buffer. The manual screenshot
shows 3 BAND, which is why CDJ waveforms read as blue/orange rather than
red/green.

Two related settings worth copying:

- **Waveform Current Position** — `CENTER` or `LEFT`. FreeDJ hardcodes centre at
  `renderer.rs:445`.
- **Waveform Divisions** — `TIME SCALE` (marks every 30 s) or `PHRASE`. Time
  scale comes free with the overall waveform. Phrase needs rekordbox phrase
  data, so it waits on a USB library parser.

## Other screens

| Screen | Purpose | FreeDJ |
|--------|---------|--------|
| BROWSE | Category column, hierarchical track list, sortable columns, preview | Missing. `crates/db/` has SQLite + FTS5 designed for this and is imported by nothing. |
| SOURCE | Device picker — USB, SD, network libraries | Missing. Prerequisite for load/eject meaning anything. |
| SHORTCUT | Live settings: waveform colour/position/divisions, time mode, auto cue, quantize value, beat jump value, vinyl speed adjust, brightness | Missing. Most map onto flags FreeDJ already hardcodes. |
| Jog display | SLIP, artwork, SYNC, playback point indicator, VINYL, cue/loop position, MASTER | Missing. Waits on the RP2350 surface, but the rotating playback-point indicator is the most recognisable thing on a CDJ. |

## Sources

- CDJ-3000X Instruction Manual, AlphaTheta DRI1956B —
  <https://downloads.support.alphatheta.com/manuals/dj-players/CDJ-3000X/CDJ-3000X_DRI1956B_manual.pdf>
- Waveform colour options —
  <https://support.alphatheta.com/en-US/articles/8113178546201?product=9366984218137>
- Code references are to this repo at `fb5750b`.
