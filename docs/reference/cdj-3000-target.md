# Could OpenDeck run on a stock CDJ-3000?

Status: **assessment, nothing built** (2026-10-23). Prompted by the
[cdj3k-emu](https://github.com/nsaintot/cdj3k-emu) project, a QEMU harness
that runs Pioneer's CDJ-3000 firmware on a desktop. This page records what
that project's open documentation says about the hardware, what the harness
can and cannot tell us, what was checked on our side today, and the two ways
forward. Nothing here comes from Pioneer firmware or code; the hardware facts
are the emulator authors' published reverse engineering (Apache-2.0/MIT
docs, GPL-2.0 kernel modules), cited the way beat-link and dysentery are.

## The hardware, as the emulator documents it

| | CDJ-3000 |
|---|---|
| SoC | Rockchip RK3399: 2× Cortex-A72 + 4× Cortex-A53, Mali-T860 MP4. U-Boot `model=CDJ3K-RK3399`, six cores |
| Main LCD | 1280×720, touch delivered through the sub-CPU frame (not an input device) |
| Jog LCD | a second display, driven by the player app through DRM dumb buffers on `/dev/dri/card0` |
| Controls and LEDs | a separate sub-CPU ("subucom") on SPI `ff1d0000.spi`, mode 3, 3.2 MHz; 64-byte full-duplex frames at ~850 Hz, CRC-16/X-25 over bytes 0..62 |
| Stock software | Linux, systemd, Xorg with a Pioneer-customised fbdev driver ("Rockchip Graphics"), the player app `EP122` (a JUCE app) at `SCHED_FIFO` 98 pinned to a core; audio through libasound to `pcmC0D0p` |
| Storage | eMMC: U-Boot 4 MiB (env holds `model`, `serial_number`, `rev_kernel`), TF-A 4 MiB, Rockchip resource blob 4 MiB, recovery 128 MiB FAT32, firmware slots A and B 256 MiB raw, settings 64 MiB ext4, userdata ~28 GiB ext4 |
| Updates | `.UPD` files are LUKS1-encrypted; the deck derives per-model/per-serial keys itself (`genkey_pr` for the cabinet image) |
| Network | eth0 (Pro DJ Link); the 3000X and 1500X also have a wifi interface |
| USB | the PC-Link USB-B port is the SoC's gadget port |

The sub-CPU frame, from `docs/subucom.md` there:

- MISO (inputs, sub-CPU → app): buttons as bitmasks in bytes 5..12; direction
  rocker byte 4 (a 2-bit value); rotary encoder 16-bit LE at 14..16; LCD
  touch two 16-bit LE coordinates at 16..20; tempo slider 16-bit LE at 22..24
  with a dead band; vinyl-speed rotary byte 24; jog position (16-bit LE),
  velocity (16-bit LE, inverted) and touch byte at 26..31; capacitive sensors
  12 bytes at 32..44; device state byte 12 (power, SD cover).
- MOSI (LEDs, app → sub-CPU): single-bit LEDs (PLAY, CUE, MASTER, …), two-bit
  step LEDs (SOURCE/BROWSE/… and SLIP/QUANTIZE: off, dim, bright), RGB PWM
  triples for the eight hot-cue pads at 12..36 and SD / USB / ON-AIR at 36..45.
- Four valid frames written back to the SPI device are a reset request; the
  sub-CPU then reboots the SoC. The sub-CPU also owns power-off.

That frame is the whole control surface. An OpenDeck build for this board is
an input adapter that decodes MISO into the input bus (the seam the DJ2Go and
S2 adapters use) and encodes button/pad LED state into MOSI.

## What the emulator is, and is not

It is QEMU's generic `aarch64-virt` machine, a vanilla Linux 6.6 kernel with
virtio-gpu, virtio-blk and a virtio-snd module, an out-of-tree `subucom_virt`
module that replaces the SPI sub-CPU with a char device fed from a host panel
UI, and an LD_PRELOAD shim that fakes the Rockchip DRM interface the stock
app expects. Pioneer's rootfs and `EP122` run unmodified on top. The host side
is a Rust/egui app with a model-neutral panel, a control API (TCP 4400+slot)
for scripted presses, touches, lamp reads and LCD captures, and bridged
networking so the guest sits on the real Pro DJ Link segment.

So it does **not** model the RK3399, the Mali GPU, the DSI panel, the audio
codec, the boot ROM or U-Boot. It cannot answer "do our kernel and drivers
come up on the board". It **can** provide a CDJ-3000-shaped front panel and
LCD that speak the real sub-CPU protocol, and a 3000 on the LAN.

## Checked on our side today

The stock design runs its screen through X11 on fbdev, but a kernel of our
own on this SoC would use Panfrost, which gives the Mali-T860 OpenGL ES 3.1
and no Vulkan. OpenDeck has only ever been validated on Vulkan (desktop, Pi)
and Metal (iOS), so the GL path was the open question:

- The renderer now honours `WGPU_BACKEND` (commit 632b47f) and only asks the
  surface for `COPY_SRC` where the backend offers it; GL surfaces do not.
- `WGPU_BACKEND=gl` on Mesa llvmpipe over Wayland: the deck runs, plays,
  60 fps (compositor-paced), no shader or validation errors.
- Both waveform shader entry points translate with naga to GLSL ES 3.10
  (`--profile es310`) without error, storage buffers included. The egui
  shaders are upstream's and already run on GL.
- X11 + EGL on this box fails to match an EGL config (wgpu/winit visual
  quirk, also with the NVIDIA driver); not chased, and not what the board
  would use if we bring our own compositor-less stack.
- CPU: the closest measured proxy is the Pi 4 (Cortex-A72 at 1.5 GHz), where
  the deck took ~1.0 core total with the R3 timestretch at ~72 % of one core
  (BENCHMARKS.md). The RK3399's A72s clock higher, so it is at least a Pi 4.

Verdict on the software: nothing in OpenDeck is tied to Vulkan or to the Pi.
An aarch64 Linux build with a GLES 3.1 driver is enough.

## Two ways to use the harness

**A. As designed: Pioneer firmware in the emulator, OpenDeck beside it on
the LAN.** The fastest way to close the README's "compatible by protocol,
not tested" gap: master handoff, SYNC, phase, and media serving against a
3000, with the emulator's control API scripting the other side. It needs a
`.UPD` and its LUKS key, which the project does not ship. Running the
firmware as a black box is no different from having the unit on the desk,
but the decryption key is a circumvention question and the firmware is
Pioneer's. Dan's call; if taken, the firmware, the key, and any capture that
contains their assets stay outside this repo. Note the emulator's own caveat
that LINK MODE (loading from other players) does not work there.

**B. Our own guest on their machine model.** Boot their QEMU, kernel,
`subucom_virt` module and panel UI with a rootfs of ours (Debian arm64, a
minimal X or cage/Wayland session on virtio-gpu, the aarch64 `opendeck`
binary) and no Pioneer bits at all. OpenDeck then has a virtual CDJ-3000
front panel to develop the sub-CPU adapter against, with the control API to
script it. Work, in order:

1. `opendeck-subucom` crate: open the SPI device, read 64-byte MISO frames
   at the poll rate, verify the CRC, decode per the map above into input-bus
   events; encode LED state into MOSI. Unit-test the codec against the
   emulator's published idle frames and CRC. (~2–3 days)
2. A 1280×720 screen profile; the chrome surface sized for the panel; jog
   LCD later (DRM dumb buffer on `/dev/dri/card0`, 2D). (~2 days)
3. The guest image: kernel + modules from their build, a rootfs with Xorg
   modesetting on virtio-gpu and llvmpipe (or virgl), opendeck at boot.
   (~2–3 days, mostly packaging)
4. Boot it in the emulator with `--headless --dev`, drive it over the
   control API, and reuse this as the front-panel model for the ADK-1000
   controls work.

Licensing: their panel codecs and docs are MIT/Apache-2.0; the kernel
module and QEMU patches are GPL-2.0 and we would run, not link, them. Our
decoder is written from their documentation, the same stance as writing the
Link sender from the public protocol analysis.

## The modders' path: a userspace payload through the deck's own updater

Added 2026-10-23 after looking at [cdj3k-mods](https://github.com/nsaintot/cdj3k-mods)
(MIT/Apache-2.0), which ships working mods for stock CDJ-3000s. What it
establishes, from its `package/` docs and scripts:

- **The deck's updater runs third-party code as root.** A `.UPD` is a LUKS
  container; the deck accepts any one encrypted with its packaging key and
  named `CDJ3Kv<3 digits>.UPD` on a FAT32/exFAT stick in the top USB port.
  There is no code signature beyond that symmetric key. The installer scripts
  inside run with full access: they write an overlay, add an `LD_PRELOAD`
  drop-in to the player's launcher, optionally enable root SSH with a key
  from the stick, and log to the stick. Two reboots later the deck runs the
  payload. Removal is a front-panel key combo or a removal `.UPD`; the rootfs
  is never rewritten. Logs: `install.log`, `phase1.log`, `phase2.log`.
- **The packaging key is a secret the modders hold.** It is not in the repo,
  and the build pipeline says so. It is the same gate as the emulator's
  firmware key. Nothing in this repo can supply it; a person would have to.
- **The stock userspace is old.** glibc 2.17 (aarch64 glibc's first release);
  the mods are built on 2.27 with `.symver` pins and an ABI check. An OpenDeck
  build for the deck has to target that: `cargo zigbuild` against
  `aarch64-unknown-linux-gnu.2.17`, or a static musl build with ALSA linked
  in. The deck's writable mount is `/home/root/settings` (`mmcblk1p7`).
- **Two hardware generations.** "Renesas" (firmware 1.x/2.x, the G2M) and
  "Rockchip (rk3399)" (3.x). The emulator only models the latter; the mods
  support both. Everything above is the RK3399 one.

What this changes: OpenDeck on a stock CDJ-3000 does not need a kernel, a
device tree, a display driver or maskrom at all. It is a userspace payload
on Pioneer's own kernel: stop `EP122.service`, start `opendeck` on the same
X display (or, if the stock stack exposes it, on DRM directly), talk to
`/dev/subucom_spi1.0` for controls and LEDs, ALSA for audio, eth0 for Link.
That is the custom firmware the 2024 post wanted, without replacing the
firmware. The sub-CPU adapter from option B is the one piece of new code
either way, and it is testable in the emulator first.

What it does not settle, in order of how much it matters:

1. **Is there any GPU in the stock rootfs?** Xorg runs on a customised fbdev
   driver, which suggests no Mali userspace at all. If there is a `libmali`
   or an EGL, our GL backend runs; if not, the choice is to ship Mesa
   llvmpipe with the payload (software GL at 1280×720 on two A72s, likely
   not 60 fps for the waveform shader) or add a CPU rasteriser path, which
   we do not have. One `ls /usr/lib | grep -i -e mali -e EGL -e GL` over the
   mods' root SSH answers this. It is the first thing to ask anyone with a
   modded deck.
2. **The key.** Without it nothing installs. Asking the modders is the only
   route, and whether to go that way is Dan's call, not a code decision.
3. **Sub-CPU expectations.** Whether the sub-CPU needs polling at its rate
   to stay happy (a watchdog, power handling) is not written down anywhere
   public; the emulator's `subucom_virt` documents reset and power-off, not
   a timeout.
4. **Audio ownership.** The stock app holds the ALSA device at `SCHED_FIFO`
   on a pinned core; our payload inherits that setup only if we reproduce
   the launcher's arrangements (`apl_start.sh` sets taskset and priorities).

Clean-room note: the mods resolve symbols inside Pioneer's stripped player
binary by signature and patch its vtables, which is binary reverse
engineering of `EP122`. OpenDeck would not do any of that. It would not load
into `EP122` or read it; it would replace it as a process and use only the
kernel's device nodes, with the protocol from the emulator's published
documentation. The one borrowed thing is the install mechanism, which is the
modders' MIT-licensed scripts plus their key.

The install guide built on this path, with the state of each piece, is
[docs/hardware/cdj-3000-install.md](../hardware/cdj-3000-install.md).

## Beyond the emulator: the real board, from scratch

(The route if the updater path is closed, or for a deck with no key.)

"Unmodified hardware" means no soldering, but it is a firmware replacement.
What it would take, none of it testable here:

- A unit on the desk, and a full eMMC dump before anything else, so the
  deck can be restored.
- A boot path. The firmware updater accepts a `.UPD` encrypted with the
  packaging key (see above), which gets a payload in but leaves the stock
  kernel in place. For our own kernel the route on an RK3399 is the SoC's maskrom USB mode
  over the OTG port (the USB-B PC-Link jack) with `rkdeveloptool`, which
  works unless the part is secure-boot fused; or a U-Boot console if one is
  exposed. Unknown until someone tries.
- Our own kernel: mainline RK3399 plus Panfrost is mature, but the board's
  device tree (DSI panel timings, the SPI sub-CPU, the audio codec, PMIC,
  jog LCD) has to be reconstructed from the vendor resource blob on the
  eMMC and from the board itself.
- The sub-CPU adapter from option B, unchanged.
- Audio: the codec and its wiring are unknown; the emulator bypasses it.

The honest ranking: option B is a clean, reusable week or two and is the
part every other path needs. With the modders' key, the payload path turns
"port a kernel" into "ship a binary", gated on whether the stock rootfs has
a GPU driver at all. Option A answers the Link
question fastest but is not a code decision. The real-board port is gated on
owning a CDJ-3000 and on the maskrom question, not on anything in this repo.
