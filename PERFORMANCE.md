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
