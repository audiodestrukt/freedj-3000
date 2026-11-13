# Stems: reading OverCue-prepared media

Status, 2026-11-10: phases 1–3 are built: the reader crate
(`crates/stems`), the engine (`crates/app/src/stems.rs`) and the on-screen
controls. Stems play from local sticks. They're behind MENU → STEMS, off by
default. Phases 0 and 4–6 are not started.

## What OverCue is, and what we take from it

[OverCue](https://overcue.gg/) is an unofficial firmware mod for the CDJ-3000
by three DJs. It is free and noncommercial. **OverCue Desktop** (Mac/Win) splits
each track on a rekordbox-exported USB into three stems (**Drums**, **Vocal**,
**Harmonics** = everything else) and writes them to a `CDJMODS/` folder beside
the export. The modded player then mutes or levels each stem live.

We don't need the firmware. The USB format is published as an open spec,
MIT-licensed:
[OverCue-gg/overcue-stems-format](https://github.com/OverCue-gg/overcue-stems-format)
(`FORMAT.md`, plus a Python validator `verify_usb.py`). The spec says outright
that "a project for another player can read the same files and use its own
controls and audio path." So the plan is:

- **freedj reads `CDJMODS/`** from a local stick, and later from a linked
  player's stick over NFS.
- **freedj plays stems through its own engine**, with its own STEMS panel.
- **Nothing is written to the stick.** freedj never runs or links the
  Pioneer firmware, the OverCue firmware or their installer. The clean-room
  rule is untouched: we implement a published MIT spec.
- **Reference only:** to see how the unit behaves, Dan's OverCue 0.14.6
  ran on cdj3k-emu (his call, 2026-11-13). freedj's code comes from what the
  screen showed and from OverCue's public manual, not from OverCue's files.
  None of them are in this repo.

## The format in brief (read FORMAT.md for the details)

```text
USB root/
  Contents/...                 exported music, unchanged
  PIONEER/rekordbox/export.pdb
  CDJMODS/
    index.json                 schema "overcue-index/1"; tracks keyed by export.pdb ID (+ file_path)
    index-v2/                  bounded binary lookup by track ID (bucketed, hashed)
    stems/<16-hex bundle>/
      stems-sidecar-{vocal,drums,harmonics,full-mix}.s16le.pgz
      overcue-manifest.json    advisory (latency pad, source codec)
```

- **Audio:** each role is stereo, signed 16-bit, **96 kHz** (the CDJ-3000
  profile), and every role is the same length. Each is cut into 128 KiB
  **pages** (32,768 frames, about 0.34 s each).
- **Pages:** each page is an independent **FLAC** stream (`OVPGZ003`) or zlib
  stream (legacy `OVPGZ001`). A big-endian header and page table give each
  page's offset, length and SHA-256.
- **Banks:** new banks (`three_part: 2`) store only the four isolated roles.
  Pairs such as "instrumental" are formed by adding two stems. `full-mix`
  must never be mixed with the stems.
- **Identity:** an index entry gives the export.pdb ID, `file_path`,
  `frames` and SHA-256s of the source file and of each role's PCM.
- **Timing:** the writer pads leading silence so the stems line up with the
  CDJ-3000's own decode of the original (`source_timing_policy`). The spec
  warns that other players must check alignment against their own decode.
- **CDJ-2000 profiles (experimental):** 44.1 kHz, as `OVPGZ004` FLAC pages
  or plain WAV.

## Fit with freedj

What exists today:

- A load decodes the whole track to `Arc<Vec<f32>>` at the deck rate. It runs
  on the loader thread: `Prep::finish` → `Prepared` → `apply_prepared`.
- The audio processor pulls source blocks through `read_block()`
  (loop-aware), then the stretcher, then the ring. Paused scrub reads the
  same buffer.
- Symphonia is already a dependency with FLAC enabled, so it can decode a
  page directly.
- LINK loads already know the export.pdb track ID (dbserver) and the file
  path (NFS). Local rekordbox loads know both too.

### Key engine decision: mix the stems before the stretcher

The processor builds each source block as `g_d·drums + g_v·vocal +
g_h·harmonics`, ramping the gains per sample. Everything downstream is
unchanged: the stretcher, loops, slip, scrub, cue latency and the
seek/drain handshake.

- **CPU:** stays at one stretcher, not three. The mix costs three
  multiply-adds per sample, which is negligible next to R3, Bungee or WSOLA.
- **The catch:** a mute is heard only after the stretcher's latency. That's
  small with WSOLA, but Bungee is about 186 ms. We measure R3 and Bungee
  before deciding. If it's too slow under Bungee, the fallback is to stretch
  the stems separately only when Master Tempo is on and a stem is changed,
  at 3× the cost. That's a later decision; the measurement comes first.

### Play the original until a stem is touched

When every gain is 1.0, play the deck's own decode of the original, exactly
as today. Switch to the stem sum only while a stem is muted or below full,
with a short crossfade at each switch.

- Nothing changes for anyone who doesn't use stems.
- FULL MIX brings back the real master, not a re-summed approximation that
  carries separation artefacts.
- It also gives a clean fallback while stems load or when pages are missing.
  OverCue does the same: "it never plays silence".

### Alignment: measure it, don't trust the pad

Our decode (Symphonia, same as OverCue Desktop), the rekordbox grid timeline
and the bank's padded timeline can differ by one MP3 frame (about 26 ms).
That's the bug OverCue's `native-mp3-header/2` fixed.

- On load, cross-correlate a few seconds of our decoded full mix against the
  bank's `full-mix` role at a few points in the track. That gives one offset,
  in samples, applied to every stem.
- Refuse the stems (S badge with a warning, original keeps playing) if:
  - the correlation is weak,
  - the offset drifts along the track, or
  - `source_sha256` doesn't match the file we loaded.
- This removes any dependence on timing-policy versions. It also catches a
  stale bank after a re-export (OverCue shows that as E-8302).

### Memory

A 6-minute track at 48 kHz stereo:

| Buffer | Size |
|---|---|
| Full mix, f32 (today) | 138 MB |
| 3 stems, i16 at the deck rate | 207 MB |
| 3 stems, f32 | 415 MB (no) |

The plan stores stems as **i16 at the deck rate**, about 2.5× today's
footprint. That's fine on desktop, iPad, Pi 4/5 and recent iPhones. Stems are
off on the Pi Zero W, and probably on the Zero 2. The 96 → 48 kHz resample is
an exact 2:1 decimation (a half-band FIR, cheap, works on any range). 96 →
44.1 kHz goes through the existing offline resampler.

### Load in pages, starting at the playhead

Decode and resample page by page, from the cue point outward, into a
preallocated stem buffer. A per-page "ready" bitmap is published atomically.

- The processor uses the stem mix only where every page in the block is
  ready. Anywhere else it plays the original.
- The deck plays as soon as the full mix is ready, as it does today. The
  STEMS tab stays grey until the pages around the playhead are ready.
- The same mechanism makes LINK work: pages arrive over NFS ahead of the
  playhead.

## Phases

| # | Phase | What | Size |
|---|---|---|---|
| 0 | Test media | Dan runs OverCue Desktop on his Mac on a spare stick with a few tracks: a LAME MP3, an ffmpeg-tagged (Beatport) MP3, a FLAC, a WAV. We also build synthetic banks in tests (our own writer, as the spec's tests do), so CI needs no music | S |
| 1 ✓ | `opendeck-stems` crate | Pure, no audio thread. Parses `index.json` and `index-v2`. Reads `.pgz` header and page table. Validates every field and hash the spec lists, and keeps index paths inside the USB root. Decodes FLAC and zlib pages. Decimates 96 → 48 kHz. Reads through a `ReadAt` trait so local files and NFS share the code | M |
| 2 ✓ | Engine | A `StemBank` behind an `ArcSwap` in `AudioHandle`, with three atomic gains, the stem mix in `read_block`, original/stems crossfade, the page bitmap, and the same source for scrub. A loader stage after `Prepared` fills pages and runs the alignment check. Tests: sum of stems at gain 1 ≈ original; a mute ramp has no click; loop wrap; seek; scrub | M |
| 3 ✓ | UI | **CDJ skin:** STEMS tab and panel as on the unit (TOGGLE/VOLUME, FULL MIX, DRUMS/VOCAL/HARMONICS, grey while loading, blue underline when changed), stem-tinted waveform, S badge in BROWSE. **XDJ skin:** no screen element, per the screen-fidelity rule; stems go on the faceplate/CONTROLS side. **Controllers:** `stem_mute`/`stem_level` actions in `controllers.toml` (DJ2Go preset, I2C). Gains reset to full mix on every load | M |
| 4 | LINK | On a dbserver/NFS load, look the track ID up in the peer's `CDJMODS/index-v2` (or `index.json` by path), then fetch pages over NFS ahead of the playhead in priority order. A lost page means the original plays there. Works with any player whose stick was prepared, stock or modded, because we only read files | M |
| 5 | Serve | freedj's media server exports `CDJMODS/` when the stick it serves has one, so OverCue players can load stems from freedj. Open question: how an OverCue player decides "the media owner runs OverCue". Testing it needs an OverCue player (see the questions below) | S–?|
| 6 | Make stems (optional) | Separation in the desktop media server (HTDemucs via ONNX, offline), writing the same format so the output stays interchangeable. Covers non-rekordbox libraries (lookup by `file_path`). Ties into analysis scope (#46) and the sampler-deck vision | L |

Phases 1–3 make a usable feature for local sticks. Phase 4 is the one that
matters in a booth: a freedj next to CDJs, loading from their sticks.

## Phase 1 as built

`crates/stems` (`opendeck-stems`):

- `index` parses `index.json` and refuses duplicate members.
- `v2` reads the bounded index and fails closed when it's damaged.
- `pgz` checks the header and page table, then decodes one page at a time
  (FLAC through Symphonia, or zlib), verifying each page's hash, or the
  whole role.
- `decimate` converts 96 → 48 kHz with a zero-phase half-band filter.
- `media` is the `Media` trait, plus `DirMedia` with path and symlink
  checks.
- `synth` writes synthetic prepared sticks for tests.

What the tests cover:

- Every codec round-trips, read through both `index.json` and `index-v2`.
- Tampering is caught in the header, a page, trailing bytes, an index-v2
  bucket and the index-v2 marker.
- Pages compressed by the reference `flac` encoder decode correctly.
- OverCue's own reference validator (`verify_usb.py`) accepts sticks from
  `examples/synth_stick.rs` in every variant. That confirms our test writer
  matches the spec, and with it what the reader is tested against.
- Still to do: a run against a stick from OverCue Desktop itself (phase 0;
  the Wine route is blocked by a Wine `ole32` bug).

## Phase 2 as built

- **Bank:** `StemBank` stores three i16 stems at the deck rate, shifted onto
  the deck's own decode. It keeps a `Weak` to the sample buffer it was built
  for, and the processor uses it only with that buffer. A LOAD resets
  `StemControl` (no bank, full mix).
- **Mixing:** `Mixer::mix` runs on the same index ranges as `read_block`
  (`block_ranges`), so loops wrap identically. Gains and the
  original↔stems blend ramp linearly across each block (about 12 ms). With
  every stem at full, the block is the original buffer, untouched. The
  paused scrub reads through `Mixer::at`.
- **Loading:** `start_stems` runs after each local LOAD on a `stems-load`
  thread.
  1. `find_stick` finds the nearest `CDJMODS/index.json` above the file.
  2. The track is looked up by path; a mismatched `source_sha256` is
     refused.
  3. Each role is read with every hash checked, then converted to the deck
     rate: the decimator at 96 → 48 kHz, rubato otherwise.
  4. `align` cross-correlates the full-mix copy with the deck's decode at
     25 / 50 / 75 % of the track. All three must give correlation ≥ 0.9 and
     agree within one frame, or the bank is refused.

  `OPENDECK_STEMS=0` turns stems off.

End-to-end test, headless on a null sink:

- **Stick:** 90 s of `amplificate.mp3`, split by FFT into three bands that
  sum exactly (stand-ins for drums, vocal and harmonics), padded the way
  OverCue pads an MP3 and written as FLAC pages. OverCue's validator accepts
  it.
- **Load:** stems were ready 1.9 s after LOAD (release build). On the
  44.1 kHz deck, alignment found **1152 frames**, exactly the MP3 metadata
  frame from the spec's `native-mp3-header` notes, with correlation
  0.97–0.98 at all three points.
- **Playback:**
  - Muting drums took the low band to zero.
  - FULL MIX restored the same spectrum.
  - Drums only left only the low band.
- **Mute delay:** heard about 70–120 ms after the command with R3. That's
  the frame, the ~93 ms output buffer and the stretcher latency. Bungee
  still needs measuring.

## Phase 3 as built

**The setting.** Dan's call: stems are a MENU setting, **STEMS**, off by
default, so the XDJ and CDJ skins stay what the stock units are.
- Turning it on looks up the loaded track's stems.
- Turning it off drops the stems, back to the plain track.
- The engine loads no stems while it's off.

**CDJ-3000 skin** (`cdj3000.rs`):
- A **STEMS** key at (774,0)–(886,90) on the unit grid, left of BEAT LOOP.
  The title is clipped short of it. Grey while loading or when the track
  has none; a blue underline while the mix is changed and the panel is
  closed.
- `Panel::Stems` uses the panel row like BEAT LOOP / BEAT JUMP: FULL MIX,
  the TOGGLE / VOLUME selector, then DRUMS, VOCAL and HARMONICS, two slots
  wide each.
  - TOGGLE: a tap mutes or unmutes a stem (dark = muted).
  - VOLUME: each stem is a horizontal fader with a blue fill.
  - The selector goes back to TOGGLE only when every stem is fully on or
    off, as OverCue's does.
- `OPENDECK_CDJ_PANEL=stems` opens the panel at startup.

**XDJ skin** (`screen.rs`):
- A **STEMS** key under BANK switches the PERFORM pads to stem mode.
- Pads 1–3 are the stems: tap to toggle, drag up or down for a level, with
  the fill rising from the bottom. Pad 4 is FULL MIX.
- DELETE and BANK rest in this mode.
- Dev script: `screen=perform-stems`.

**Shared widget:** `screen::stem_touch`. A drag moves the level from where
it is (with 6 pt of slop), so a touch never jumps it; a tap toggles.

**Stem waveform** (built 2026-11-13). The enlarged waveform shows the
stems playing at their levels, as OverCue's does; the overview stays the
full track.
- The stems job also runs the deck's waveform analysis on each stem
  (`stems::stem_waves`, one thread per stem).
- `stems::mix_columns` mixes them by level. A column band is an energy, and
  separate stems' energies add, so each band is √Σ(gᵢ·eᵢ)². One stem at
  full is that stem's own columns.
- At full mix the enlarged waveform is the track's own columns.
- Where the mix is silent, a 2-px grey line (128,128,128) runs through the
  middle, as on the unit. Where some stem still plays, OverCue draws that
  line in the waveform's own colour; ours stays grey.
- Both renderers keep the track's columns for the overview. wgpu stores the
  mix in the second half of its column buffer (`detail_off`); the SDL
  backend keeps a second column list.
- The app recomputes the mix only when the bank, a level or the waveform
  changes (`sync_stem_wave`), so a VOLUME drag costs one pass over the
  columns per frame.

**Seen on OverCue 0.14.6** (cdj3k-emu, Dan's CDJ2 stick):
- The panel: FULL MIX, the TOGGLE/VOLUME selector, DRUMS / VOCAL /
  HARMONICS, and an EQ key on the right (grey without a compatible mixer).
- Stem keys use the unit's checkered key face: blue when playing, the same
  dark grey as an unlit key when muted, white text on both.
- The enlarged waveform shrinks to a strip above the panel. OverCue draws
  it from a precomputed waveform per stem combination on the stick
  (`stems-<combo>-waveform.EXT/.2EX`, rekordbox ANLZ). freedj builds its
  own from the stems instead, which also covers partial levels.
- The STEMS tab gets a blue underline when the mix is changed and another
  tab is open; at full mix it is grey again.
- BROWSE lists mark tracks that have stems with a small "S" under the "H"
  of the track icon.
- VOLUME's blue fill could not be checked: the emulator's touch control
  cannot drag.

**"S" in BROWSE** (built 2026-11-13). With STEMS on, the CDJ skin's lists
and track tables mark a local track with prepared stems with a small "S"
low on the left of its ♪. `stems::StemMarks` reads each stick's
`index.json` once and caches the answer per file; a stick coming or going
clears it. The unit also draws a stock "H" above the S; freedj doesn't
draw it yet.

**Panel faces** (built 2026-11-13). The panel takes OverCue's own key
spacing (`stem_keys`, measured on the unit: FULL MIX 18–97, the selector
116–238 split at 423–424, the stems 257–502 / 517–762 / 777–1022, EQ
1064–1207, rows 383–466). Keys use the unit's 4-px checker from each key's
corner, dark square first: blue (0,100,165)/(0,125,225), grey
(35,35,35)/(50,50,50). The faces match the unit pixel for pixel. VOLUME
fills the blue from the left to the level, with no percentage (manual
p. 12). EQ is grey and inert. The unit sets its labels in bold; freedj has
no bold face yet.

**Not yet:**
- Bold panel labels, the stock "H" mark in lists.
- Controller actions (`controllers.toml`) for the hardware deck.

## Open questions for Dan

1. **Test stick:** answered. Dan's CDJ2 stick has six tracks prepared with
   OverCue Desktop.
2. **OverCue on the emulator:** answered. Dan approved it; it runs in
   cdj3k-emu as a launch-only mod (see "Seen on OverCue 0.14.6" above).
3. **Stretcher latency:** if mutes under Bungee feel late, do we accept it or
   pay for per-stem stretching while a stem is changed?
4. **Store text:** decided 2026-11-10: name OverCue. Use it in the
   description only, as a factual compatibility line ("plays stems prepared
   with OverCue Desktop"), never in the app name, subtitle or keywords
   (guideline 2.3.7). Add OverCue to the trademark disclaimer with "not
   affiliated". Once phase 3 works on a real stick, send OverCue a
   compatibility report (their README asks for them) and ask to be listed
   as a reader.

## Out of scope

- Stems EQ: mixer EQ driving stems. It needs OverCue's DJM firmware mod.
- OverCue's other features (Gate Cue, Smart Cue, Fader Start, phrase
  indicators). They're on their own TODO lines in
  `docs/TODO-cdj-3000.md`, independent of stems.
