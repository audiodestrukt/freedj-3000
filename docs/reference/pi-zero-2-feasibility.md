# Could OpenDeck run on a Raspberry Pi Zero 2 W?

*2026-10-27. Prompted by a claim of a CDJ-3000 clone running on a Zero 2 W
with the Bungee stretcher. Short answer: the audio engine would, the rest of
the deck would not, and the blocker is the GPU, not the CPU.*

The Zero 2 W is a BCM2710: four Cortex-A53 at 1 GHz (the Pi 3's silicon,
clocked lower), 512 MB, VideoCore IV. Three things were checked, two of them
measured on the desktop, one estimated.

## 1. GPU: hard no

The VideoCore IV's open driver (`vc4`) tops out at **OpenGL ES 2.0**. wgpu's
GL backend needs ES 3.0 as a floor, and our waveform shaders are ES 3.1
(validated on the CDJ-3000 work, `cdj-3000-target.md`). Reproduced here by
pinning Mesa to ES 2.0 on the software rasteriser:

```bash
env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 \
  MESA_GLES_VERSION_OVERRIDE=2.0 MESA_GL_VERSION_OVERRIDE=2.1 MESA_GLSL_VERSION_OVERRIDE=120 \
  WGPU_BACKEND=gl OPENDECK_SCREENSHOT=/tmp/es2.png xvfb-run -a ./target/release/opendeck track.mp3
# → wgpu_hal::gles::egl "No config found!", then
#   panicked: failed to create renderer: no compatible GPU adapter found
```

No shader rewrite fixes this: the adapter is refused before any shader is
compiled. The only ways round it are a different renderer (egui's glow
backend does run on ES 2.0, but the deck screen is custom wgpu, not egui
painting) or a different board. Anything claiming a full CDJ screen on a
Zero 2 is drawing through SDL/framebuffer with its own renderer.

## 2. RAM: tight, not a wall

Measured resident set, deck playing with `OPENDECK_STRETCH=bungee` at
1024×600:

| Where | Track | RSS |
|---|---|---|
| Desktop (NVIDIA, Vulkan) | amplificate.mp3, 527 s | 505 MB |
| Desktop (NVIDIA, Vulkan) | techno.mp3, 313 s | 431 MB |
| Desktop, xvfb + llvmpipe | amplificate.mp3 | 497 MB |
| Pi 5 (V3DV, 2026-08-26, `raspberry-pi.md`) | techno.mp3, 313 s | 249 MB |

The difference between the two desktop tracks (74 MB for 214 s) is exactly
the decoded audio: tracks are held as **32-bit float stereo, 10.6 MB per
minute**. Everything else is the baseline, which is dominated by the GPU
driver's userspace mappings: ~320 MB on the desktop, **~165 MB on the Pi 5**.

On a Zero 2 W with Raspberry Pi OS Lite about 410 MB is free. Pi-class
baseline plus a 9-minute track is ~260 MB; a 20-minute track (~210 MB of
audio) would not fit. So RAM alone does not rule the board out, but there is
no headroom for the analysis cache, the media server or a second deck. The
cheap fix if a low-RAM target ever matters: **16-bit sample storage** halves
the per-track cost (5.3 MB/min) and helps the 2 GB Pi 4 boards too.

## 3. CPU: Bungee fits, Rubber Band does not (estimate)

Nothing here simulates an A53's speed (QEMU timing is meaningless for
real-time factors; a throttled Pi 4 has twice the per-clock throughput). The
estimate chains two measured ratios:

- Rubber Band R3 at 1.0×: RTF **0.045** on this desktop vs **0.72 of a core
  on a Pi 4** (`pi4-render-bound` memory) → the Pi 4 is ~16× slower here.
- A 1 GHz Cortex-A53 vs the Pi 4's 1.5 GHz A72: ~2.5–3× slower (clock ×
  roughly half the IPC).

| Engine | Desktop RTF | Pi 4 (measured / scaled) | Zero 2 W (estimate) |
|---|---|---|---|
| Rubber Band R3 | 0.045 | 0.72 core | **2.2 cores** — not real-time |
| Bungee | 0.005 | ~0.08 core | **~0.25 core** |

So the stretcher is the part of the claim that holds up, provided it is
Bungee (or Rubber Band R2, not measured) and not R3. `make perf` prints both
engines' numbers on whatever machine it runs on; a **Pi 3** is the exact
proxy for the Zero 2 W's CPU (same BCM2710, 1.2 GHz instead of 1.0).

## Verdict

| | Zero 2 W | Pi 4 (2 GB) | Pi 5 |
|---|---|---|---|
| GPU for the deck screen | **no** (ES 2.0) | yes (V3D, ES 3.1 / Vulkan) | yes |
| RAM | tight (~260 MB of 410) | fine | fine |
| Stretcher | Bungee only | R3 ok, Bungee cheap | either |
| Audio out | USB DAC only | analog jack / HDMI / USB | HDMI / USB |
| Network for Link | Wi-Fi only (USB OTG ethernet) | wired | wired |

The Zero 2 W is the right size for the *panel* half of the project (the
thing that talks to the XDJ's panel CPU, `xdj-1000mk2-panel-bus.md`) and
overkill there compared with the RP2350. For the deck, the Pi 4 with
cooling stays the floor and the Pi 5 the comfortable choice. A Zero-footprint
board with a Mali GPU (Radxa Zero 3W: RK3566, Mali-G52, Panfrost does
Vulkan and ES 3.2, up to 8 GB) is the untested alternative if the form
factor is the point.

## Measured on the board below it: Pi Zero W (original), 2026-10-28

The board that turned up on the bench was a **Pi Zero W Rev 1.1**, not a
Zero 2 W: one ARM1176 (ARMv6, VFPv2, no NEON) at 1 GHz, 426 MB, VideoCore IV,
32-bit Raspberry Pi OS (Trixie) with the labwc desktop. It is a tier below
the Zero 2 W, so these are a floor, not the Zero 2 W's numbers. Built with
`deploy/pi/cross/build.sh armv6` (sysroot lifted from the Pi; 15.8 MB
stripped; needs `-latomic` for Rubber Band's 64-bit atomics), run under the
desktop with `LIBGL_ALWAYS_SOFTWARE=1 WGPU_BACKEND=gl OPENDECK_STRETCH=bungee`
on a 60 s excerpt of amplificate.mp3.

| What | Zero W | Desktop, for scale |
|---|---|---|
| Decode 60 s MP3 | 3.8 s | ~0.1 s |
| Waveform + MiniBPM, 60 s | 25.9 s (9.5 s with the grid cached) | ~0.2 s |
| RSS, 60 s track loaded | 124 MB | — |
| Audio thread (Bungee, 1.0×) | ~40 % of the one core | 0.40 normalised |
| GPU | wgpu refuses VideoCore IV (ES 2.0); llvmpipe (LLVM 19) via GL comes up | Vulkan |
| First frame | 103–128 s (LLVM JIT compiling the shaders) | — |
| Steady state | **45 s per frame**, render thread 95 % of the core | 16.7 ms |
| Output | wrong: panels filled white, cyan diagonal streaks, text as outlines | — |
| Audio underruns | 17 s of silence during analysis; 57 s of the ~100 s once rendering started | none |

So on this board the app starts, decodes, analyses and plays, and the
rendering path is dead three ways at once: wgpu cannot use the GPU, software
GL is three orders of magnitude too slow, and llvmpipe on ARMv6 without NEON
produces corrupt frames (diagonal streaks are the fingerprint of a row-pitch
mismatch; not investigated, it would not matter at this speed). The audio
failures are a consequence of the single core: the analysis thread and the
render thread starve the audio thread, which has no RT priority (#13).

What this says about the Zero 2 W: the GPU verdict in §1 stands, so the
[SDL render backend (#47)](https://github.com/audiodestrukt/freedj-3000/issues/47)
is the only route for either board. Beyond it the Zero W would also need the
audio thread at RT priority and analysis throttled to a background core it
does not have; the Zero 2 W has three spare cores for exactly that, and each is
roughly three times faster than this one.
