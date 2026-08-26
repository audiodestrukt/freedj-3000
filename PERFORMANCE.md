# Performance Analysis

## Profiling setup

```
cargo build --release -p opendeck-app   # release + debug = 1 (line-level symbols)
sudo sysctl kernel.perf_event_paranoid=1
cargo flamegraph --bin opendeck -- <track.mp3>
```

Generate flamegraph from an existing `perf.data`:

```
perf script | inferno-collapse-perf | inferno-flamegraph > flamegraph.svg
```

## Thread model

| Thread | Name in perf | Role |
|--------|-------------|------|
| Main | `opendeck` | winit event loop, egui layout, wgpu render |
| Audio processor | `audio-proc` | RubberBand R3 timestretch → ring buffer fill |
| Audio device I/O | `data-loop.0` | cpal/PipeWire callback, ring buffer drain → device |
| Clipboard daemon | `smithay-clipboa` | Separate subprocess spawned by egui-winit for Wayland clipboard; not our code |

## March 2026 baseline (post-fix)

Profiled on Linux/Wayland with a 5-minute 44.1 kHz stereo MP3 at 1× speed.
64 497 samples, `cpu_core/cycles` event.

### Top CPU consumers (opendeck process)

| Overhead | Symbol | Notes |
|----------|--------|-------|
| 1.86% | `pthread_mutex_lock` | Mutex contention inside wgpu Vulkan layer |
| 1.73% | `egui::context::Context::create_widget` | egui layout pass, runs every frame |
| 1.65% | `__memmove_avx_unaligned_erms` | egui tessellation memory copies |
| 1.37% | `wgpu_core::queue_submit` | GPU command submission, once per frame |

Nothing above 2%. No single hotspot. RubberBand, `processor_loop`, and
`nanosleep`/`clock_nanosleep` are **absent** from the profile — the audio
thread spends its time in OS sleep, not on-CPU.

### Root cause of earlier high CPU (revised August 2026)

The render loop was unbounded: `request_redraw()` in `about_to_wait` caused the
GPU to render as fast as possible (measured at 12 500 fps with a non-blocking
swapchain), pegging one CPU core.

The March fix — a `ControlFlow::WaitUntil(last_render + 16.67 ms)` timer — did
cap the frame rate, but it treated the symptom. The actual cause was that
**winit on Wayland only requests the compositor's frame callback when the app
calls `window.pre_present_notify()` before presenting**, and the app never did.
Without that callback, `request_redraw()` is never gated on the display and the
loop free-runs.

The timer also introduced its own problem: it was not phase-locked to vsync,
so only 59% of frames landed in their refresh slot and the compositor showed
the rest twice or dropped them — visible as waveform judder that survived every
attempt to smooth the position feeding the shader.

**Current design** (see `renderer.rs` and the `RedrawRequested` handler):
`pre_present_notify()` before every present, the next redraw requested from the
`RedrawRequested` handler, `about_to_wait` does nothing, and a Mailbox swapchain
so acquire never adds a second throttle. The compositor is the only clock. The
playhead advances by whole display periods (from `refresh_rate_millihertz`)
rather than measured wall-clock, since each frame is shown for an integer
number of refreshes regardless of CPU-side jitter.

Measured over 400 frames: acquire never blocks, zero double frames, one
skipped frame, motion exactly one period per frame. Verified on
NVIDIA/Vulkan/Wayland; X11 and GLES paths not yet measured.

## Frame instrument

```
RUST_LOG=opendeck=debug,wgpu=off,naga=off,egui=off ./target/release/opendeck track.mp3
```

Logs one line per frame:

```
frame: dt 16.72ms  audio  16.68ms  ratio  1.00  lag  92.6ms
gpu: acquire  0.04ms  present  0.08ms
```

- `dt` — wall-clock since the previous frame started
- `audio` — how far the playhead advanced this frame; should equal `dt`
- `ratio` — `audio / dt`; sustained departure from 1.00 is judder
- `lag` — decode-ahead distance being compensated
- `acquire` — time blocked in `get_current_texture()`; nonzero means the
  swapchain is throttling, which should not happen under Mailbox

A healthy run has `dt` and `audio` both near the display period with no zeros
and no negatives, and `acquire` near zero.

The audio processor thread was also sleeping only 1 ms between back-pressure
checks. **Fix:** sleep scales proportionally with ring buffer fill level (up to
8 ms when the buffer is nearly full), so the processor thread wakes only as
often as needed to stay ahead of the device callback.

## Known remaining overhead

- **egui runs every frame** even when playback position hasn't changed and
  there is no user input. Could be optimised by skipping the egui pass on
  frames with no state change, but the gain is marginal at 60 fps.
- **wgpu submit per frame** is unavoidable for a continuously scrolling
  waveform.
- **Wayland compositor** and **Vulkan driver** overhead are outside our
  control and account for some of the mutex contention visible in the profile.

## ProDJ Link beat timing

A CDJ sends one beat packet per beat; every other deck on the network aligns
its phase meter — and, with SYNC, its playback — to those packets. Their
timing is therefore a direct measure of how good a peer we are.

### The benchmark

XDJ-1000MK2, firmware 1.44, playing at 126.00 BPM, measured at a freedj
instance on the same switch from microsecond log timestamps at packet arrival:

```
interval 476.14 ms   sd 1.33 ms   min 473.7   max 477.3      (60000 / 126 = 476.19)
```

That is the target: **beat-to-beat jitter ≈ 1.3 ms as received by a peer.**

### Where our jitter came from, and what it is now

The sender derives beat crossings from the *audible* position — the decoder
cursor minus what is still queued in the ring buffer and the stretcher. Three
successive designs, each measured with two freedj instances (`make
link-pair`) and, for the last, alongside the XDJ in the same run:

| Sender | sent (self-timed) | received by peer | cause of the remainder |
|---|---|---|---|
| raw `position − in_flight`, 1 ms poll | double-fired (46 beats where 18 were due) | — | `in_flight` swings ±35 ms between decode blocks; the beat index crossed the same boundary repeatedly |
| + monotonic guard, low-passed `in_flight` (τ 50 ms) | 446.6, **sd 14.7** | sd 14.4 | crossings land on 512-frame block edges; intervals alternate 430 / 463 ms |
| + phase-locked estimate at true rate, pull 2 %/tick | 445.2, **sd 3.1** | sd 2.8 | 1 ms poll rounds each crossing to a tick edge (±0.5 ms); the pull still admits block-rate noise |
| + τ 200 ms on both filters, sleep-to-deadline for the last 1.5 ms | 446.0 | **sd 1.23** | at the benchmark |

The self-timed column overstates the last row (sd 3.8 with one 465 ms
outlier the peer never saw) because the timestamp was taken at loop top,
before the spin-wait; fixed to stamp at the send. The received column is
the comparable number: identical method to the XDJ measurement, same
receiver, same run.

### Why the estimate, not the reference

The audio thread only ever gives the sender a *quantised* fact: which
512-frame block it is on and how much of the ring buffer is unplayed. Both
step, and the second is noisy. No amount of clever polling of that reference
yields sub-millisecond crossings. What works is the same idea as the
renderer's playhead: run a local clock at the true audible rate
(`sample_rate × fader_speed`), pull it toward the reference slowly enough
that block-rate noise averages out (τ ≈ 200 ms), snap only on a real seek,
and take beat crossings from the clock. Then the only jitter left is the
poll granularity, and a sleep-to-deadline removes that too.

### Method

```
make link-pair                          # or: run --player 2 next to the XDJ
RUST_LOG=opendeck=debug ... | grep 'ProDJ beat: player N'   # at the receiver
```

Intervals are differences of the log timestamps (`env_logger` prints
microseconds). Drop the first two: the first send lands mid-beat, so the
first interval is partial. `sd` over ≥ 20 intervals. The receiver's own
scheduling adds to both the XDJ's and our numbers equally, which is why the
same-run comparison is the fair one.

### Still to do

- Give `prodj-tx` real-time priority. It currently shares the default
  scheduler with everything else; under load the spin-wait can be
  preempted.
- Measure against the XDJ *as the receiver*: put freedj as MASTER, the XDJ
  on SYNC, and see whether its phase meter holds still. That needs status
  packets from us first.
