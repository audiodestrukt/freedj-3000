# Real-world benchmarks

Measured performance of freedj on target hardware. The Pi 5 is the reference
target for this project. Add a row when you test a new platform — the
reproduction recipe below makes the numbers directly comparable.

All runs: `techno.mp3` (5:13, 44.1 kHz stereo MP3), release build, 1024×600
window, playing at 1.0× unless noted.

## Results

| Metric | RTX 4070 SUPER desktop | **Raspberry Pi 5 (4 GB)** | Raspberry Pi 4 |
|---|---|---|---|
| SoC / GPU | x86-64 / RTX 4070 SUPER | Cortex-A76 ×4 / V3D 7.1 | *(TBD)* |
| OS / compositor | Arch / Wayland | Bookworm 64-bit / wayfire | *(TBD)* |
| GPU backend | Vulkan | **Vulkan (V3DV Mesa)** | *(TBD)* |
| Present mode | Mailbox | **Mailbox** | *(TBD)* |
| Frame rate | 60 fps | **60 fps** | |
| Frame dt jitter (sd) | 1.64 ms | **0.12 ms** | |
| Frames in vsync window | ~100 % (after fix) | **100 %** | |
| Stalled / backwards frames | 0 / 0 | **0 / 0** | |
| GPU acquire (Mailbox) | ~0.04 ms | ~0.08 ms | |
| Decode + waveform + BPM | ~0.1 s | **0.7 s** | |
| CPU @ 1.0× | low | **~58 % of one core** | |
| Cores / load avg | many / low | **4 / 0.82** | |
| RSS | — | **249 MB** | |
| Peak temp (stock cooling) | — | **77.9 °C ⚠️** | |
| First `make build` | ~2–3 min | **6 m 47 s** | |

Blank Pi 5 cells match the desktop. Pi 4 column is a placeholder.

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
below), dt sd well under a millisecond, 0 stalled/backwards, temp under ~75 °C.

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
