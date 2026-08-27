# Real-world benchmarks

Measured performance of freedj on target hardware. The Pi 5 is the reference
target for this project. Add a row when you test a new platform — the
reproduction recipe below makes the numbers directly comparable.

All runs: `techno.mp3` (5:13, 44.1 kHz stereo MP3), release build, 1024×600
window, playing at 1.0× unless noted.

## Results

| Metric | RTX 4070 desktop | **Raspberry Pi 5 (4 GB)** | **Raspberry Pi 4 (4 GB)** |
|---|---|---|---|
| SoC / GPU | x86-64 / RTX 4070 SUPER | Cortex-A76 ×4 / V3D 7.1 | Cortex-A72 ×4 / V3D 4.2 |
| OS / compositor | Arch / Wayland | Bookworm 64-bit / wayfire | Bookworm 64-bit / labwc |
| GPU backend | Vulkan | Vulkan (V3DV) | **Vulkan (V3DV)** |
| Present mode | Mailbox | Mailbox | **Mailbox** |
| Display / frame rate | 60 fps | 60 fps | **30 fps** (4K@30 HDMI) |
| Frame dt jitter (sd) | 1.64 ms | 0.12 ms | ~2.4 ms (idle) |
| Stalled / backwards frames | 0 / 0 | 0 / 0 | **0 / 0** |
| GPU acquire (Mailbox) | ~0.04 ms | ~0.08 ms | ~0.2 ms |
| Decode + waveform + BPM | ~0.1 s | 0.7 s | **~2.1 s** |
| Render thread CPU | low | ~30 % of one core | ~17 % of one core |
| **Timestretch thread CPU @ 1.0×** | low | *(folded above)* | **~72 % of one core** ⚠️ |
| Total active CPU @ 1.0× | low | ~58 % (0.6 core) | ~95 % (~1.0 core) |
| Load avg (4 cores) | — | 0.82 | 1.90 |
| RSS | — | 249 MB | 252 MB |
| Peak temp | — | **77.9 °C ⚠️** | 62.8 °C (no throttle) |
| First `make build` | ~2–3 min | 6 m 47 s | **18 m 22 s** |

Both Pis: hardware Vulkan via V3DV, Mailbox available, clean pacing (0 stalls),
audio at 44.1 kHz. The Pi 4's frame rate is the *display's* 30 Hz mode, not a
freedj limit.

## iPad (on-device baseline — 2026-08-27)

First on-device numbers from the iOS build (`ios/`), read off Xcode's Debug
Navigator gauges while running the `--faceplate` deck synced to a real XDJ. These
are gauge-level (not `/proc` per-thread like the Pi rows), so it's a baseline to
watch as we go, not a directly comparable column.

| Metric | 13-inch iPad |
|---|---|
| CPU | ~26 % of one core |
| Memory | ~200 MB |
| Frame rate | ~53 fps |
| Workload | `--faceplate`, R3 key-lock timestretch @ 1.0×, Link + track loaded from the XDJ |

Caveats to resolve before treating these as a reference:
- **Build config unconfirmed — likely Debug.** The Rust lib is only built
  `--release` under Xcode's Release scheme; a Debug run inflates CPU and drops
  frames. Re-read under Release.
- **~53 fps, not a clean 60/120.** ProMotion is 120 Hz and freedj advances the
  playhead in display-refresh units, so the iOS pacing target needs confirming
  (partly the Debug build, partly render pacing).
- As on the Pi, the CPU is dominated by the R3 timestretch; Master-Tempo-off via
  the [varispeed engine](design/varispeed-engine.md) would cut it.

## What the numbers say

- **The graphics path holds on both Pis.** V3DV exposes Mailbox on the Pi 5
  *and* the Pi 4, so the desktop frame-pacing design runs unchanged on both.
  The Pi 5 is actually *tighter* than the NVIDIA box (dt sd 0.12 ms vs 1.64);
  the Pi 4 is looser (3.08 ms) but still stall-free. The project's biggest
  hardware unknown — pacing on VideoCore — is resolved for both.

- **The Pi 4's hot thread is the timestretch, not the renderer.** Measured with
  `/proc` utime/stime per thread (the authoritative source — `top -H`'s %CPU is
  unreliable here and first reported a spurious 86 % on the render thread): the
  **audio-proc thread running Rubber Band R3 sits at ~72 % of one A72 core even
  at 1.0×**, while the render thread is a comfortable ~17 % (gdb confirms it
  sleeps in `epoll_wait` between frames — it is not busy-waiting). So DSP, not
  the UI, is the single-core cost on the Pi 4.

- **This is the real headroom question.** R3 gets *heavier* off 1.0× (pitch-
  bending), so a hard nudge could push that thread toward saturating its core on
  the A72. The documented fallback is signalsmith-stretch (MIT, lighter) — see
  raspberry-pi.md. The egui redraw, by contrast, is a non-issue on the Pi 4: the
  render thread has ample room, so the "skip egui when unchanged" idea buys
  almost nothing here and is not worth the complexity for this board.

- **Total compute is fine on both.** ~0.6 cores (Pi 5) / ~1.0 core (Pi 4) of
  active work at 1.0×, on 4-core parts — but it is concentrated in one thread
  (the stretcher), which is why per-deck DSP maps so naturally onto the spare
  cores (see below).

- **Audio was in tune by luck (both).** HDMI/analog negotiated 44.1 kHz to match
  the track, so no pitch error. A 48 kHz output would still play 44.1 kHz sharp
  until sample-rate conversion lands (WORKSTREAMS A1). "Audio fine on the Pi" is
  not "SRC not needed." (And on the Pi it went to a device we did not choose —
  device selection, B3, is unfinished.)

- **Thermals: the Pi 5 is the hot one.** 77.9 °C under light load (needs an
  active cooler; throttles ~80–85 °C). The Pi 4 ran 62.8 °C, no throttle — the
  A72 draws less. Counter-intuitively the *faster* board is the one that needs
  the cooling attention.

## What the Pi 5 numbers say

- **The graphics path holds.** V3DV exposes Mailbox, so the desktop frame-pacing
  design (`pre_present_notify` + Mailbox + redraw-from-callback) runs unchanged
  and is actually *tighter* than on the NVIDIA box (dt sd 0.12 ms vs 1.64 ms) —
  the Pi's display stack is less jittery. This was the project's biggest
  hardware unknown and it is resolved.
- **Compute has headroom.** 58 % of one core with three idle at 1.0×. The
  Rubber Band R3 time-stretch (the heaviest component) only works harder off
  1.0×, and there is clearly room. The render thread — egui redrawing every
  frame — is the largest single consumer and the first optimization if needed.
- **Audio was in tune by luck.** HDMI negotiated 44.1 kHz to match the track,
  so no pitch error. A 48 kHz output (most USB DACs) would still play 44.1 kHz
  material sharp until sample-rate conversion lands (WORKSTREAMS A1). Do not
  read "audio fine on the Pi" as "SRC not needed."
- **Thermals are the hardware gate.** 77.9 °C under *light* load, stock setup.
  The Pi 5 throttles ~80–85 °C, and throttling is what turns into audio
  glitches. An active cooler is not optional for sustained use.

## Threads and cores

freedj is already multi-threaded — the ~1.5 cores of load on the Pi 5 spread
across, with the render thread the single biggest share:

| Thread | Work | Load on Pi 5 |
|---|---|---|
| main | winit events + egui tessellation + wgpu submit | ~30 % of a core |
| `audio-proc` | Rubber Band R3 time-stretch → ring buffer | light at 1.0×, heavier off it |
| cpal callback | RT: drain ring → device | tiny, must stay un-preempted |
| `prodj-rx-*` ×3 | Link listeners (announce/beat/status) | ~idle |
| `prodj-tx` | Link sender | ~idle |

So the OS already lands these on different cores; we are not single-core bound
(load 0.82 on 4 cores). Where **more** parallelism actually pays off, in order:

1. **Per-deck, via the dual-deck feature (WORKSTREAMS C4).** Two decks means
   two independent `audio-proc` time-stretch threads — the natural way to use
   more cores, and it maps cleanly onto the Pi 5's four: render, deck-A DSP,
   deck-B DSP, RT-out. This is the real multi-core story, and it comes for free
   with the feature rather than as speculative parallelism.
2. **RT isolation for the audio callback (WORKSTREAMS A3)** — pin it to a
   dedicated core (`isolcpus`) with `SCHED_FIFO` so it is never preempted. This
   is about *determinism* (glitch-free audio), not throughput, and it matters
   more than raw parallelism.
3. **Load-time analysis** (decode + FFT + BPM, 0.7 s on the Pi) could fan the
   waveform FFT across cores with `rayon` if instant-load becomes a goal — a
   one-time cost, low priority.

What *not* to do: parallelize for its own sake. The render thread at 30 % is
not a bottleneck, and extra threads in the RT path add jitter risk — the
single-producer/single-consumer lock-free ring buffer is deliberately simple.
The right rule here is "one heavy job per thread, one thread per core it
needs," and the heaviest job that scales is per-deck DSP.

## Regression guard (in the test suite)

`make perf` (and `cargo test --workspace`) runs `timestretch_realtime_factor`, a
hermetic guard on the R3 timestretch cost at 1.0×, 0.5× (heaviest), and 2.0×.
It uses a synthetic signal (no audio device, GPU, or track file) and is
profile-independent (Rubber Band is optimised C), so it runs the same under
`cargo test` and `--release`. Two guards per speed:

- **Normalised ratio** `rb_time / fft_calibration` — the primary regression
  catch. The R3 time is divided by an independent FFT calibration measured on
  the same machine in the same run, so CPU speed cancels out: the ratio is
  stable to ~1% run-to-run and roughly machine-independent, letting the band be
  tight (≈1.7× the desktop value — catches a ~70% slowdown while tolerating
  cross-CPU drift). A real R3 regression trips it on any hardware.
- **Absolute RTF** `rb_time / audio_duration` — the viability floor (< 1.0 = the
  DSP keeps up), sized for the Pi 5 target; also the backstop if the FFT
  calibration itself regressed.

Both are printed every run so a sub-threshold creep is visible as a trend.
`OPENDECK_RTF_CEIL=<scale>` relaxes both (hardware slower than the Pi 5) or
tightens them (a stricter local check).

Reference (RTX desktop): norm ≈ 2.44 / 4.45 / 1.90, RTF ≈ 0.034 / 0.062 / 0.027
at 1.0× / 0.5× / 2.0×. The Pi 4's audio-proc sits near 0.7 of a core at 1.0×
(see the underrun discussion below), which is why it is scheduling-jitter
sensitive there.

## Reproduction recipe

On the target, from the repo root:

```bash
# Frame pacing + GPU backend + audio rate (8 s):
export XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1   # if over ssh
RUST_LOG=opendeck=debug,wgpu=off,naga=off,egui=off \
  timeout 9 ./target/release/opendeck techno.mp3 > run.log 2>&1

grep -E "GPU:|present mode|display refresh|device sample rate" run.log   # backend, pacing clock, SRC
```

Frame stats from `run.log` (the `frame:` instrument, see PERFORMANCE.md):
```bash
grep "frame: dt" run.log | tail -300 \
  | sed 's/.*frame: dt //; s/ms  audio /,/; s/ms  ratio.*//' \
  | awk -F, '{n++; s+=$1; a[n]=$1; if($2<0.01)z++} END{
      m=s/n; for(i=1;i<=n;i++)v+=(a[i]-m)^2;
      printf "dt mean %.2fms sd %.2fms  stalled %d  ~%.0f fps\n", m, sqrt(v/n), z+0, 1000/m}'
```

CPU / mem / temp while playing:
```bash
./target/release/opendeck techno.mp3 >/dev/null 2>&1 & P=$!
sleep 3
ps -o %cpu,%mem,rss -p $P | tail -1     # %cpu is out of 100 per core
vcgencmd measure_temp                    # Pi only
kill $P
```

A "good" result: `via Vulkan`, `present mode: Mailbox` (or a clean Fifo — see
below), `dt` mean matching the display period (16.7 ms @ 60 Hz, 33.3 ms @ 30 Hz),
`dt` sd small relative to that, 0 stalled/backwards, and the **render thread
well under a full core** (`top -H`) — the Pi 4 showed that thread is the first
thing to saturate. Temp under ~75 °C.

## Testing a Pi 4 (or any second aarch64 board)

- **Fast path:** the Pi 5 and Pi 4 are both `aarch64` on the same OS, and we do
  **not** build with `target-cpu=native`, so the Pi 5's binary should run on a
  Pi 4 as-is. `scp target/release/opendeck` across and try it before spending
  15+ minutes on a native Pi 4 build. (If it faults with an illegal
  instruction, fall back to a native build.)
- **What to watch on a Pi 4:**
  - **GPU:** VideoCore VI / V3D 4.2. Recent Mesa V3DV gives Vulkan, but confirm
    `via Vulkan` and not `Gl` — an older Mesa may only offer GLES, which takes
    the Fifo pacing path.
  - **Present mode:** if it prints `Fifo` (Mailbox not offered), the "one clock"
    logic is less validated there — check the frame stats carefully.
  - **CPU:** the A72 is ~2–3× slower than the Pi 5's A76. R3 time-stretch off
    1.0× is the thing that may not keep up; watch for xruns (currently silent —
    WORKSTREAMS A3 adds a counter). If it can't hold, signalsmith-stretch (MIT,
    lighter) is the fallback stretcher.
  - **Micro-HDMI ×2, 2/4/8 GB variants**; audio and thermals as on the Pi 5.
