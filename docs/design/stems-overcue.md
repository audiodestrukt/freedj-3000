# Stems: reading OverCue-prepared media

Status: plan, 2026-11-10. Nothing built yet.

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
- **Nothing is written to the stick.** The Pioneer firmware, the OverCue
  firmware and their installer are never involved. The clean-room rule is
  untouched: we implement a published MIT spec.

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
| 1 | `opendeck-stems` crate | Pure, no audio thread. Parses `index.json` and `index-v2`. Reads `.pgz` header and page table. Validates every field and hash the spec lists, and keeps index paths inside the USB root. Decodes FLAC and zlib pages. Decimates 96 → 48 kHz. Reads through a `ReadAt` trait so local files and NFS share the code | M |
| 2 | Engine | A `StemBank` behind an `ArcSwap` in `AudioHandle`, with three atomic gains, the stem mix in `read_block`, original/stems crossfade, the page bitmap, and the same source for scrub. A loader stage after `Prepared` fills pages and runs the alignment check. Tests: sum of stems at gain 1 ≈ original; a mute ramp has no click; loop wrap; seek; scrub | M |
| 3 | UI | **CDJ skin:** STEMS tab and panel as on the unit (TOGGLE/VOLUME, FULL MIX, DRUMS/VOCAL/HARMONICS, grey while loading, blue underline when changed), stem-tinted waveform, S badge in BROWSE. **XDJ skin:** no screen element, per the screen-fidelity rule; stems go on the faceplate/CONTROLS side. **Controllers:** `stem_mute`/`stem_level` actions in `controllers.toml` (DJ2Go preset, I2C). Gains reset to full mix on every load | M |
| 4 | LINK | On a dbserver/NFS load, look the track ID up in the peer's `CDJMODS/index-v2` (or `index.json` by path), then fetch pages over NFS ahead of the playhead in priority order. A lost page means the original plays there. Works with any player whose stick was prepared, stock or modded, because we only read files | M |
| 5 | Serve | freedj's media server exports `CDJMODS/` when the stick it serves has one, so OverCue players can load stems from freedj. Open question: how an OverCue player decides "the media owner runs OverCue". Testing it needs an OverCue player (see the questions below) | S–?|
| 6 | Make stems (optional) | Separation in the desktop media server (HTDemucs via ONNX, offline), writing the same format so the output stays interchangeable. Covers non-rekordbox libraries (lookup by `file_path`). Ties into analysis scope (#46) and the sampler-deck vision | L |

Phases 1–3 make a usable feature for local sticks. Phase 4 is the one that
matters in a booth: a freedj next to CDJs, loading from their sticks.

## Open questions for Dan

1. **Test stick:** can you prepare one with OverCue Desktop on the Mac (phase
   0)? It's the only way to test real banks and real timing.
2. **OverCue on the emulator:** phase 5 needs a player running OverCue.
   Installing it on cdj3k-emu means putting a firmware mod on Pioneer's
   firmware in the emulator. That's your call, and we can skip phase 5
   entirely.
3. **Stretcher latency:** if mutes under Bungee feel late, do we accept it or
   pay for per-stem stretching while a stem is changed?
4. **Store text:** "reads stems prepared by OverCue Desktop" is a factual
   compatibility statement. Mention it, or keep the listing generic
   ("stems")? The listing must not name the CDJ-3000 either way.

## Out of scope

- Stems EQ: mixer EQ driving stems. It needs OverCue's DJM firmware mod.
- OverCue's other features (Gate Cue, Smart Cue, Fader Start, phrase
  indicators). They're on their own TODO lines in
  `docs/TODO-cdj-3000.md`, independent of stems.
