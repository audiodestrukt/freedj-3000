# Running freedj on a Raspberry Pi

First-bring-up guide. The app runs on x86/NVIDIA/Wayland today; the Pi is a
different GPU (VideoCore, Mesa V3DV / GLES) and often no compositor, so expect
to shake out the graphics and audio paths. Nothing here is verified on a Pi
yet — this is the plan and the known risks.

## Hardware to gather

- **Raspberry Pi 5, 8 GB.** The Pi 4 can work (README lists it as a floor) but
  the Rubber Band R3 time-stretch is the heaviest thing in the app and the Pi 5
  has the headroom; start on the 5.
- **Official 27 W USB-C PSU.** The Pi 5 browns out on underpowered supplies,
  and audio glitches are the first symptom — don't debug xruns on a weak PSU.
- **Active cooler / heatsink.** Sustained time-stretch keeps a core busy.
- **NVMe HAT + a small NVMe, or a good A2 microSD.** Tracks decode fully into
  RAM at load (no streaming yet), so load time is disk-bound; NVMe is nicer.
- **HDMI cable** (micro-HDMI → HDMI on the Pi 5) for the display. Any screen;
  the layout targets 1024×600 but scales.
- **Audio out:** HDMI audio works out of the box, but for real use a **USB
  audio interface / DAC**. See the sample-rate caveat below.
- **Ethernet cable** if you want ProDJ Link (the Pi 5 has gigabit).
- The **DJ2Go** (or any class-compliant USB-MIDI controller) plugs straight in.

## OS

**Raspberry Pi OS (64-bit, Bookworm)** — matches the `aarch64-unknown-linux-gnu`
target already in `rust-toolchain.toml`. Bookworm defaults to a Wayland
compositor (labwc/wayfire), which is the closest to the dev environment and the
easiest first target. Running directly on DRM/KMS with no compositor (the
eventual appliance mode) is a second step — see "Graphics" below.

## Build

Native on the Pi is simplest to start (slower compile, no cross toolchain):

```bash
sudo apt update
sudo apt install libasound2-dev libvulkan-dev libwayland-dev \
                 libxkbcommon-dev librubberband-dev \
                 mesa-vulkan-drivers pkg-config build-essential git curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone git@github.com:audiodestrukt/freedj-3000.git
cd freedj-3000
make build          # first build is slow on a Pi; subsequent ones are fine
make run TRACK=techno.mp3
```

Cross-compiling from the x86 box is much faster once set up (the aarch64 target
is already added); worth doing if you iterate a lot, but not needed for a first
run. `cross` or a sysroot both work; the C deps (`librubberband`, ALSA) make a
sysroot the reliable path.

## Graphics — the main unknown

`renderer.rs` was tuned for NVIDIA/Vulkan/Wayland. On the Pi:

- **Vulkan via Mesa V3DV** should work (`wgpu::Backends::all()` picks it up).
  Confirm at startup: the log prints `GPU: ... via Vulkan`. If it says `Gl`,
  it fell back to GLES — still fine, but a different pacing path.
- **Frame pacing may need rework.** The current design is `pre_present_notify()`
  + a **Mailbox** swapchain + redraw-from-callback, which gave a perfect
  vsync-locked result on NVIDIA. **V3DV may not offer Mailbox** — the code
  already falls back to Fifo, but the "one clock" logic was validated with
  Mailbox. On the Pi, run `RUST_LOG=opendeck=debug` and check the frame
  instrument (PERFORMANCE.md): `acquire` near zero, `dt`/`audio` near the
  display period, no doubles/skips. If frames burst or judder, the Fifo path
  is the thing to fix (WORKSTREAMS/PERFORMANCE both flag this as unverified).
- **Texture limit:** the `using_resolution(adapter.limits())` fix means the
  V3DV max (4096/8192) is used, not the 2048 downlevel default — no repeat of
  the desktop 2560-wide panic.
- **DRM/KMS, no compositor:** the appliance target. winit can do this, but
  the compositor-frame-callback pacing won't exist there — Fifo becomes the
  only clock. Get it working under the Bookworm compositor first, then try
  bare KMS.

## Audio — two real gaps that bite harder on a Pi

- **Sample-rate conversion is not implemented (A1).** If the output device
  runs at 48 kHz and the track is 44.1 kHz, playback is a semitone-plus sharp.
  Desktop PipeWire sometimes hides this; a Pi with a USB DAC will not. Until
  A1 lands, use 44.1 kHz material and check the startup log for
  `device sample rate ... != file sample rate`. This is the single most likely
  "it sounds wrong on the Pi" cause.
- **The audio thread is not RT-hardened (A3).** No `SCHED_FIFO`, no mlock, and
  underruns are silent (`consumer.pop().unwrap_or(0.0)`). On a loaded Pi with a
  compositor this will glitch. First useful step is an xrun counter so glitches
  are visible; then RT priority on `audio-proc`. Run the audio group at higher
  priority (`ulimit -r`, a `@audio` limits.conf entry) as a stopgap.

## First-run checklist

1. `make run` — does a window open and the waveform scroll smoothly?
2. `RUST_LOG=opendeck=info` — confirm `GPU: ... via Vulkan` and the decoded
   sample rate vs the device rate (the SRC caveat).
3. `RUST_LOG=opendeck=debug` — check the frame instrument for clean pacing.
4. Plug in the DJ2Go — `MIDI: found DJ2Go` and controls drive the deck.
5. Ethernet to the XDJ (or `make virtual-cdj` on the Pi) — receive-only Link
   still works; freedj follows the master's tempo. (Leave Link **send** off —
   it is the default and the sender is still being hardened.)

## What not to expect yet

- No RP2350 control surface — the DJ2Go / keyboard / mouse are the inputs.
- No streaming loader — a track fully decodes to RAM at load, so a long track
  is a multi-second load and a couple hundred MB of RAM.
- No cues/loops/browser — see WORKSTREAMS.
