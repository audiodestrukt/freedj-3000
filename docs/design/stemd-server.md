# OpenDeck as a stemd server

Status: **built 2026-11-21** (`crates/stemd`, `crates/app/src/stemdsrv.rs`),
tested over HTTP the way the CDJ-3000's client talks; not yet tried against a
deck running cdj3k-mods.

## Why

Dan: "Other people are streaming to OverCue", then "maybe we support stemd
first".  OverCue players load stems only from players running OverCue, and
OverCue's own Live Stems is a private protocol (tested twice on the emulator,
`stems-overcue.md`).  The open route to a CDJ-3000 is
[cdj3k-mods](https://github.com/loyahdev/cdj3k-mods) (Apache-2.0): its
**Server Stems** sends the loaded track to a [stemd](https://github.com/nsaintot/stemd)
server on the network (MIT OR Apache-2.0) and plays the stems that come back.
stemd's HTTP contract is published (`docs/api.md`).  So OpenDeck answers it,
and a modded CDJ-3000 gets stems split by OpenDeck: the iPad's Core ML, a
desktop's HTDemucs.

## What the deck's client uses

From cdj3k-mods `package/deck/stemd_client/` (read 2026-11-21):

- **Discovery:** `avahi-browse -rtp _stemd._tcp`, or a typed `host[:port]`
  (port 8420 by default).
- **Health:** `GET /v1/health`; compatible when `sample_rate` is 44100,
  `channels` is 2, and the strings `"harmonics"` and `"vocals"` appear.  Its
  cache is keyed on `separation_id`, else `backend-model-preset`.
- **Job:** `POST /v1/jobs?sample_rate=44100&channels=2&format=f32le&output_format=flac`,
  plus `&output_sample_rate=96000&dsp_mode=1` when it knows its pool runs at
  96 kHz.  Body: the deck's own decode, f32le.  Reply `200`/`202` with `id`.
- **Poll:** `GET /v1/jobs/{id}` every interval: `stage` (`queued`,
  `analysing`, `separating`, `reconstructing`, `writing`, `done`, `failed`),
  `fraction`, `completed`; when done `sample_rate`, `frames`, and
  `stems[].gain`.
- **Fetch:** `GET /v1/jobs/{id}/stems/harmonics` and `/vocals`, written to
  tmpfs.  Drums are rebuilt on the deck as mix − harmonics − vocals.
- **Cancel:** `DELETE /v1/jobs/{id}`.
- HTTP/1.1, `Connection: close`, exact `Content-Length` both ways.

## What we serve

All of the above, plus: `pcm16`/`s16le`, `pcm32`/`f32le`, `wav`, `flac`;
output at 24, 44.1, 48 or 96 kHz; `parts` drawn from harmonics, vocals and
drums (leaving one out, which is the derived part), `include_derived`; a
cache (the same audio and parameters again is a `200` with the finished job);
the last six finished jobs keep their stems (older ones answer `410`, as stemd
does).  Not served: sessions (the deck doesn't use them), `bass` / `other`
as parts (our split folds them into harmonics), `mp3`.

**dsp mode 1** is stemd's copy of the CDJ-3000's 44.1 → 96 kHz converter: a
polyphase FIR (L = 320, M = 147, 18,880 taps) that stemd's author measured
by driving the converter with impulses.  The deck subtracts the stems from a
mix *it* resampled, so the rebuilt drums only cancel if the stems went
through the same filter.  Ported from stemd under MIT with its coefficient
table and reference vectors (`crates/stemd/data/`, licence alongside); our
port nulls against stemd's captured reference below −120 dB.  Nothing was
taken from Pioneer's software.

**FLAC** stems are verbatim (valid FLAC, no compression, fixed 4096-frame
blocks): 69 MB for a 3-minute stem at 96 kHz where stemd's run 20–30 MB.
The deck spools them to RAM and decodes them once; a real encoder (fixed
predictor + Rice) would cut that about 2.5x and is the obvious next step.

**The model** is the device's own (`livestems::model_split`: the native Rust
HTDemucs, ONNX, or Core ML), at 44.1 kHz.  The server's splitter waits while
the device's deck is splitting and cancels a job under way when the deck
starts one, as live stems do.  On whenever live stems are
(`livestems::enabled`: a model, not a Pi by default); `OPENDECK_STEMD=0`
turns it off.

## Measured

`real_model_stemd_job_over_http` (desktop, `separate-native`, a 3:00 MP3,
f32le up, FLAC at 96 kHz via mode 1 back): split and encoded in 65 s (2.8x
real time), model residual −31 dB (lands on the rebuilt drums, as with
stemd's Fast preset), stem gains 0.90 (peaks past full scale scaled to fit).

## Tests

`crates/stemd/src/tests.rs` drives the server exactly as the deck's
`http.c` does: health passes the deck's compatibility check; a whole job
(f32le up, pcm16 back) rebuilds drums from the mix to 1e-4; the same audio
twice is a cache hit with no second run; DELETE stops a running job;
unknown jobs are 404 and unoffered formats / parts / dsp modes 400; mode 1
stems cancel against a mix converted with the same filter.  `mode1.rs`
holds stemd's reference test.  `stemdsrv.rs`: the app's splitter returns
the bank's levels and gives way to the deck.

## Next

- Try it against a CDJ-3000 running cdj3k-mods (the emulator can boot it).
- A real FLAC encoder.
- Sessions (stems near the needle first) if a client wants them.
- OpenDeck as a stemd **client**: split on a GPU machine running stemd.
