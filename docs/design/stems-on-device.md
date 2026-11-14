# Stems split on the device

Status, 2026-11-14: a test build. Linux desktop runs it end to end; the
iPad build is written but has not run on an iPad yet. Off unless a model is
supplied. Code: `crates/app/src/separate.rs`, shim in `ios/freedj/main.m`.

## What it does

A track loaded with MENU → STEMS on, and no OverCue stems on its stick, is
split by a separation model on the deck itself. The stem keys read
"SPLITTING n%", then the stems play through the same engine as OverCue's
(`stems.rs`): the model's output becomes an ordinary `StemBank`.

## Model

**HTDemucs** (Meta's Demucs v4), MIT. Four stems: drums, bass, other,
vocals. The deck's three are drums, vocal, and harmonics = bass + other
(OverCue's split).

| Build | Where | File | Size |
|---|---|---|---|
| Core ML | iOS 18+ | `htdemucs.mlpackage` ([slaughters85j/htdemucs-coreml](https://huggingface.co/slaughters85j/htdemucs-coreml)) | 425 MB (287 MB zipped) |
| ONNX | desktop | `htdemucs.onnx` ([StemSplitio/htdemucs-onnx](https://huggingface.co/StemSplitio/htdemucs-onnx)) | 316 MB |

Both take `mix` [1, 2, 343 980] (7.8 s of stereo at 44.1 kHz) and return
the four stems, planar. Neither is in the repo or the app: the user puts the
file on the device. Both builds must stay 32-bit float; the Core ML
converter reports that 16-bit breaks the spectrogram reconstruction.

Better-scoring models exist (BS-RoFormer, Mel-RoFormer), but some of their
published weights are non-commercial, so they are out for the App Store.

## Pipeline (`separate.rs`)

As Demucs's own `apply_model`:

1. Normalise by the mono sum's mean and standard deviation.
2. Convert to 44.1 kHz.
3. Run 7.8 s chunks overlapping by a quarter; crossfade with Demucs's
   triangular weights.
4. Fold to three stems, undo the normalisation (the DC goes on harmonics so
   the three add back to the mix), convert back to the deck rate.

Output is streamed: a stretch is finished once no later chunk touches it,
so only one chunk of 44.1 kHz stems is held. A test with a fake model checks
that the stems come back sample-aligned with the deck's decode.

## Measured (Linux, 28 threads, ONNX Runtime 1.23 on the CPU)

| Track | Length | Split | Speed | Peak memory |
|---|---|---|---|---|
| Brand New Funk | 3:32 | 48 s | 4.4× real time | 4.9 GB |
| KICK BACK | 3:11 | 41 s | 4.7× real time | — |

ONNX Runtime's defaults (a growing arena and a whole-graph buffer plan)
took the peak to 9 GB; both are off now, at the same speed.

Against OverCue's stems for the same tracks (OverCue as the reference; it
is a different separator, so this measures agreement, not quality):

| Track | Drums | Vocal | Harmonics |
|---|---|---|---|
| Brand New Funk | 10.2 dB | 4.9 dB | 10.8 dB |
| KICK BACK | 4.7 dB | 0.2 dB | 6.9 dB |

Quality is a listening call.

## Platforms

- **iPad / iPhone:** Core ML on the GPU (the converter reports the Neural
  Engine rejects the model's inverse transform). The open question is
  memory: an 8 GB iPad lets an app use roughly half its RAM. The test log
  records the footprint and what iOS says is left.
- **Mac:** the same Core ML path would run, and faster than the iPad;
  not built (the desktop Mac build would need the shim).
- **Windows / Linux:** ONNX Runtime, as on Linux today. Windows needs
  `onnxruntime.dll` shipped beside the app; with DirectML or CUDA it would
  use the GPU.
- **Raspberry Pi 5:** possible but slow. Four Cortex-A76 cores are roughly
  a sixth to a tenth of the Linux test machine, so expect about real time
  or slower (3–6 min for a 3.5-min track), and the ~5 GB peak needs the
  8 GB Pi. Better as a "prepare this folder" job than on load. Not measured.
- **Pi Zero, Android phones:** no.

## First iPad run (0.3.11, iPad Air 13")

The app quit during the first split of a local track. iOS kills an app that
passes its memory limit without a crash report, so 0.3.12 writes each step
(start, model ready, chunks) with the memory in use to the test file, line
by line, runs the model on the GPU only (the Neural Engine attempt costs
compile time and memory for nothing). 0.3.12 meant to declare
`com.apple.developer.kernel.increased-memory-limit` too, but the project
signs with no entitlements file by default (`FREEDJ_ENTITLEMENTS=""`, to
keep the ungranted multicast entitlement out), so the key never reached the
app. 0.3.13 signs with `freedj-memory.entitlements`, which holds only it.

## Open

- Run on the iPad: speed, memory, and whether iOS kills the app.
- Cache splits, so a track is split once.
- Fill the bank as chunks finish, from the cue point, so stems are usable
  seconds after a load instead of after the whole split.
- Getting the model onto the device: today by hand (Files app); a release
  would download it on first use, with consent.
- Memory: the iPad may need the increased-memory-limit entitlement, or a
  smaller chunk.
