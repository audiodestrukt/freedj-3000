# Stems split on the device

Status, 2026-11-14: a test build. Linux desktop runs it end to end; the
iPad Air 13" splits on 0.3.13 (with the increased memory limit). Off unless a model is
supplied. Code: `crates/app/src/separate.rs`, shim in `ios/freedj/main.m`.

## What it does

A track with no OverCue stems on its stick is split by a separation model on
the deck itself, starting when its STEMS panel (or the XDJ screen's STEMS
pad mode) is opened; loading a track never starts a split. STEMS is on by
default since 2026-11-14. The stems play through the
same engine as OverCue's (`stems.rs`): the model's output becomes an
ordinary `StemBank`.

**Playing while it splits.** Once the model is loaded the split publishes
an empty bank (`StemBank::filling`, through `Job::bank`) and writes each
finished stretch into it, in order from the top of the track; `ready`
marks how far it has got. The stem keys work from then on and read
"SPLITTING n%" (n is the bank's written share). The processor plays the
stems below `ready` and the original above it. It decides per block, one
block ahead: a block whose next block would reach past `ready` ramps the
mix back to the original, so the fade runs over written stems and never
steps at the edge, and the stems ramp back in once the split is a block
ahead again. The samples are `AtomicI16` (relaxed loads cost the same as
plain ones) with a release/acquire `ready`, so the audio thread reads while
the split writes, with no lock and no copy. The stems' waveforms are built
when the split finishes; until then the enlarged waveform is the track's.
A failed split drops its partial bank.

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

The rest of the peak is ONNX Runtime's constant folding, not the audio:
it keeps about 3 GB of pre-computed tensors resident from the model's load.
One 7.8 s chunk on 4 threads (warmed up, the same output to 1e-6):

| Optimisation | Resident after load | Peak | Chunk |
|---|---|---|---|
| all (folding on) | 4,294 MB | 4,431 MB | 2.27 s |
| all but ConstantFolding | 542 MB | 1,446 MB | 2.89 s |
| none | 522 MB | 1,226 MB | 3.00 s |

So folding is on only with at least 7 GB available
(`separate::fold_constants`; `OPENDECK_SEPARATE_FOLD=1`/`0` overrides).
A whole 5:13 track on 4 cores of the Linux machine, folded: 2.4× real
time, peak 5.1 GB with the test's track buffers.

Against OverCue's stems for the same tracks (OverCue as the reference; it
is a different separator, so this measures agreement, not quality):

| Track | Drums | Vocal | Harmonics |
|---|---|---|---|
| Brand New Funk | 10.2 dB | 4.9 dB | 10.8 dB |
| KICK BACK | 4.7 dB | 0.2 dB | 6.9 dB |

Quality is a listening call.

### Against Meta's own HTDemucs (2026-11-15)

The same 44.1 kHz WAV of techno.mp3 (5:13) through Demucs 4.0.1
(`-n htdemucs --shifts 0 --overlap 0.25 --float32`) and through ours
(`SPLIT_SR=44100`, ONNX, folding off), sample-aligned:

| Stem | Agreement | Note |
|---|---|---|
| drums | 55.3 dB (60.3 dB unclipped frames) | Demucs's CLI rescales a stem that would pass 0.99 (`--clip-mode rescale`; here ×1/1.2236); undone before comparing |
| harmonics | 52.3 dB | |
| vocal | 33.8 dB | a flat −72 dBFS floor; this track has almost no vocal, so the ratio is small |

One 7.8 s chunk straight through both models: ONNX vs PyTorch 80–86 dB
(drums, bass, other), 56 dB (vocals). So the ONNX export and our chunking,
crossfade and normalisation reproduce the reference model; what's left is
about −70 dBFS of arithmetic difference.

Two things to know when comparing:

- **Same input or nothing.** HTDemucs is not shift-invariant: decoding the
  MP3 with and without the encoder delay (1,105 samples) moves every chunk
  boundary and alone drops agreement to 11–25 dB.
- **Stems can be louder than their mix** (drums peak 1.21 here, the mix
  1.0). The bank stores 16-bit, so a split is stored at half level
  (`stems::SPLIT_SCALE`, one bit for 6 dB of headroom) and played back at
  its own; before, it clamped 532 drum samples on this track. Drums now
  agree with the reference to 60.2 dB over the whole track.

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
  or slower (3–6 min for a 3.5-min track). Unfolded (above) the model
  peaks at 1.45 GB, so the 4 GB Pi holds it; it needs a 64-bit OS
  (ONNX Runtime ships aarch64, not armhf, and a 32-bit process cannot
  address it anyway). Not measured on the Pi yet.
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

0.3.12's log on the iPad Air 13" (iPad16,10), Amplificate (8:47):

```text
start        | mem 701 MB, 4418 MB left
model ready  | mem 893 MB, 4226 MB left
```

then nothing: the app died in the first chunk, before it was logged. So the
model loads cheaply (≈200 MB; the weights are mapped, not copied), and the
first GPU prediction is where memory runs out. 0.3.13 logs the moment before
the first chunk and reads `opendeck-stems.conf` in the OpenDeck folder
("cpu" or "all") to try other Core ML compute units without a rebuild.

## Open

- Run on the iPad: speed, memory, and whether iOS kills the app.
- Cache splits, so a track is split once.
- Split from the cue point (or the playhead) first instead of the top, so
  the stems are under the playhead sooner on a long intro.
- ~~Getting the model onto the device~~ done 2026-11-14 (`modelfetch.rs`):
  downloaded the first time STEMS is on, from a pinned Hugging Face
  revision, size and SHA-256 checked, unzipped as a stream (9.5 MB peak for
  the 423 MB weights), installed atomically in Application Support, excluded
  from backup. The model is picked from a catalogue by the device's memory
  limit; one entry today, room for a lighter one for smaller devices. The
  download restarts from scratch if iOS suspends the app partway (a
  background `NSURLSession` would continue it; not done).
- Order: since 2026-11-15 a split starts just ahead of the playhead and
  follows its jumps, wrapping to the top last
  ([stems-from-playhead.md](stems-from-playhead.md)).
- Memory: since 2026-11-15 the model stays loaded while the STEMS panel (or
  the XDJ screen's STEMS pad mode) is open and is released 10 s after it
  closes with no split running (2026-11-14 to -15 it was released after
  each split; before that it stayed loaded for the session). A split needs `SPLIT_NEEDS_MB`
  (4.6 GB) free or is refused with "NEEDS MORE MEMORY" rather than killed.
  That figure comes from the one iPad failure (0.3.12, standard limit:
  killed in the first chunk with 4.2 GB left after the model loaded); the
  peak of a successful iPad split (0.3.13+, CPU + GPU) is not recorded yet
  and should replace it. (The 548 → 5219 MB run sometimes quoted is Linux,
  ONNX Runtime on the CPU, not the iPad.)

## A Raspberry Pi 5: our own runtime (`opendeck-demucs`)

Measured 2026-11-15 on a 4 GB Pi 5 (64-bit Pi OS, no cooler, so throttled to
1.5 GHz at 85-89 °C for most runs). Target: split at 2x real time, i.e. one
chunk (5.85 s of new audio at 25 % overlap) in under 2.9 s.

| Route | Per chunk | Notes |
|---|---|---|
| ONNX Runtime 1.23, fp32 | 13.5 s | whole track 0.4x real time, peak 1.6 GB |
| ONNX Runtime, int8 MatMul | 12.7 s | only 6 % faster; agreement with fp32 falls to 26-38 dB |
| ONNX Runtime, int8 MatMul + Conv | — | slower on the desktop and broken (1 dB on vocals) |
| PyTorch 2.15 | 14.5 s | 18 GFLOPS; 20 % of its time is `copy_` |
| V3D GPU (Vulkan, ncnn) | — | 20-50x *slower* than the CPU on real CNNs (squeezenet 313 vs 7 ms) |

HTDemucs is **335 GFLOP per chunk** (transformer linears 127, convolutions
123, attention 85). The Pi's four A76 cores do fp32 matrix multiplies at
66-99 GFLOPS (PyTorch's sgemm) and ncnn reaches about that on whole CNNs, so
the runtimes, not the chip, are the limit: ONNX Runtime gets ~30 GFLOPS. At
90 GFLOPS a chunk takes ~3.7 s, about 1.6x real time (1.9x at 10 % overlap);
fp16 arithmetic in the transformer is the next lever after that.

`crates/demucs` is HTDemucs written out by hand in Rust from Meta's weights
(`tools/demucs/export_htdemucs.py` writes `htdemucs.safetensors`). The
frequency branch is kept as (freq, channel, time), which makes every
convolution, along time or along frequency, a strided matrix multiply over
the tensor in place: no im2col, no layout copies. `tests/reference.rs`
compares each stage with a PyTorch run on a real chunk: the stems agree at
102-123 dB (ONNX Runtime's export: 80-86, 56 on vocals).

| `opendeck-demucs`, per chunk | Desktop, 4 cores | Pi 5, 2.4 GHz |
|---|---|---|
| ONNX Runtime 1.23 (for comparison) | 3.0 s | 15.3 s |
| first version, 1 thread | 11.2 s | — |
| softmax subnormals flushed | 4.9 s (1 thread) | — |
| threads (rayon, disjoint output blocks) | 2.5 s | 11.4 s |
| vectorisable exp/erf, threaded norms and GLU | 2.3 s | 10.2 s |

On the Pi, `perf` puts 77 % of the time in `matrixmultiply`: 52 % in its
NEON kernel, 24 % packing operands (weights are re-packed every call, and
the strided time-branch convolutions re-pack each tap). Its kernel does
32 GFLOPS on one core but ~16 per core with all four running. Next: our own
matrix multiply with weights packed once at load, blocked for the A76's
512 KB L2; then fp16 arithmetic (the cores have `asimdhp`, twice the fp32
rate) where the reference test says it holds.
