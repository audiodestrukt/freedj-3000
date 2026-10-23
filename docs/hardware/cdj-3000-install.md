# Installing OpenDeck on a CDJ-3000

Status: **procedure designed, not yet runnable** (2026-10-23). This page is
the install guide as it will read when the pieces exist. Each step says
whether its piece is built. Nothing on this page has been done on a deck.
The reasoning behind the design is in
[docs/reference/cdj-3000-target.md](../reference/cdj-3000-target.md).

| Piece | State |
|---|---|
| OpenDeck binary for the deck (aarch64, glibc 2.17) | not built |
| Sub-CPU adapter (`opendeck-subucom`: buttons, jog, touch, LEDs) | not built; spec in hand |
| Launcher and install scripts (`payload.sh`, phases, removal) | not written |
| Jog LCD | not driven; stays blank in the first version |
| Package (`.UPD`) | needs the deck's packaging key, which this project does not have or distribute |
| Dry run in the emulator | not done |
| Tested on a deck | no |

## Read this first

- **Voids the warranty. Unstable by definition.** Not on a deck you need for
  a gig, not on a club's unit, not on the only player you own.
- **RK3399 decks only**, firmware 3.13 to 3.22. A CDJ-3000 on firmware 1.x
  or 2.x is the Renesas hardware generation and is not covered here.
- **Nothing is reflashed.** The method is the one the community mods use:
  the deck's own updater installs an overlay, the stock system stays on the
  eMMC untouched, and a front-panel key combination at power-on removes the
  overlay. If OpenDeck fails to start, the deck still boots to stock after
  that combination.
- **No Pioneer code is used, read or patched.** OpenDeck replaces the stock
  player as a process and talks only to the kernel's device nodes. It does
  not load into the stock application and does not inspect it.

## 0. Prerequisites and the one check that matters

You need:

1. A CDJ-3000 (RK3399, firmware 3.13 to 3.22). UTILITY shows the version.
2. A USB stick, **MBR partition map, one FAT32 partition** (a Mac's default
   GUID map makes the deck stop at "Connect USB storage device").
3. A computer on the deck's network for Link tests, and for the SSH checks
   below.
4. A `.UPD` built with the deck's packaging key. See step 3.

**Before building anything, find out whether the deck has a GPU driver in
userspace.** OpenDeck draws with OpenGL ES through wgpu. The stock player
runs on an X server with a framebuffer driver, which suggests no GPU
libraries at all. On a deck that already has the community mods with root
SSH enabled, run:

```sh
ls /usr/lib /usr/lib/aarch64-linux-gnu 2>/dev/null | grep -i -e mali -e EGL -e GLES -e gbm
ls /dev/mali* /dev/dri/ 2>/dev/null
cat /proc/version
ldd --version | head -1
```

- Mali or EGL libraries present: proceed; the screen will be GPU-drawn.
- Nothing present: OpenDeck still runs with Mesa's software renderer
  shipped in the payload, but expect the waveform well under 60 fps on two
  A72 cores. Decide whether that is worth your time before going on.

Please paste that output into a
[CDJ-3000 report](https://github.com/audiodestrukt/freedj-3000/issues/new?template=cdj-3000-report.md)
either way. It is the single most useful thing a deck owner can send.

## 1. Build the payload

*Not built yet. This is the build as designed.*

The deck's glibc is 2.17, the first aarch64 release. A binary built on a
current distro will not load. Build against 2.17 with zig as the linker:

```sh
cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu
cargo zigbuild --release --target aarch64-unknown-linux-gnu.2.17 -p opendeck-app --features deck-cdj3000
```

`--features deck-cdj3000` (to be added) turns on the sub-CPU adapter and the
1280×720 screen profile, and turns off the desktop chrome. Check the result
before it goes anywhere near a deck:

```sh
objdump -T target/aarch64-unknown-linux-gnu/release/opendeck | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1   # must print GLIBC_2.17
```

The payload directory, as the installer lays it out on the deck's overlay:

```
/home/root/opendeck/
  opendeck              the binary
  run.sh                launcher (below)
  lib/                  only if the deck has no GL: Mesa llvmpipe + EGL, built for glibc 2.17
  tracks/               optional: a demo track
```

`run.sh`, as designed:

```sh
#!/bin/sh
# Started by the overlay in place of the stock player.  The stock X server is
# already up on :0 (the launcher keeps that part of the stock boot).
export DISPLAY=:0
export WGPU_BACKEND=gl
export OPENDECK_WINDOW=1280x720
export OPENDECK_SUBUCOM=/dev/subucom_spi1.0     # the sub-CPU: buttons, jog, touch, LEDs
export OPENDECK_AUDIO_DEVICE=hw:0,0            # the deck's codec, as the stock player opens it
export OPENDECK_PLAYER=1                       # or the number set in the stock UTILITY before switching
export OPENDECK_LINK_SEND=1
export RUST_LOG=opendeck=info
cd /home/root/opendeck
exec ./opendeck 2>>/home/root/settings/opendeck.log
```

The sub-CPU adapter reads 64-byte input frames from the SPI device at the
deck's poll rate and writes LED frames back, using the frame layout
published by the cdj3k-emu project (`docs/subucom.md` there): buttons,
direction rocker, rotary encoder, LCD touch, tempo slider, vinyl-speed
rotary, jog position/velocity/touch, capacitive sensors, power state; and
single-bit, step and RGB-pad LEDs the other way. CRC-16/X-25 on both.

## 2. Dry run in the emulator

*Not done yet.* Before any deck, boot the payload in
[cdj3k-emu](https://github.com/nsaintot/cdj3k-emu)'s machine with a rootfs
of our own (their kernel and `subucom_virt` module, a Debian arm64 userland,
no Pioneer firmware). Their panel UI then drives the same sub-CPU device the
deck has, and their control API scripts it. The pass criteria: every panel
control reaches OpenDeck, every LED we set lights on the panel, audio plays,
and a second emulator slot syncs to us over Link. The recipe for that guest
is part of this work and will live next to this page.

## 3. Pack the `.UPD`

The deck installs a `.UPD` that is a LUKS container holding the payload
and the installer scripts, named `CDJ3Kv<three digits>.UPD`. The container
has to be encrypted with the deck's packaging key. **This project does not
have that key, does not distribute it, and will not tell you where to get
it.** The community mods' `package/updater/` (MIT/Apache-2.0) is the packing
pipeline we would reuse: `pack.sh --key <key>` with our payload and scripts
in place of theirs.

The installer scripts, as designed (not written):

| Script | Runs | Does |
|---|---|---|
| `payload.sh` | at update time, as root, from the stick | copies `/home/root/opendeck/` into the overlay; writes the drop-in that starts `run.sh` instead of the stock player's command line; enables root SSH if an `authorized_keys` sits next to the `.UPD`; logs to the stick as `install.log` |
| `phase1.sh`, `phase2.sh` | the two reboots that follow | finish the overlay and verify the binary loads (`ldd` clean); log to the stick |
| `uninstall.sh` | the removal `.UPD`, or the front-panel combination | clears the overlay; the deck is stock again |

The all-or-nothing rule applies: if the firmware is outside 3.13 to 3.22, or
the binary does not load on this deck's libc, nothing installs and the deck
boots stock.

## 4. Install on the deck

1. Copy exactly one `.UPD` to the root of the stick. Two makes the deck stop
   and ask you to fix it. Optional: an `authorized_keys` file next to it for
   root SSH.
2. Stick in the **top USB port**. The SD slot is not read for updates.
3. Run the deck's normal firmware-update procedure (power on with the
   update stick; the deck finds the file and asks to update).
4. **Do not cut power or pull the stick.** The deck restarts twice.
5. Leave the stick in until the deck is up, then read `install.log`,
   `phase1.log` and `phase2.log` from it on a computer.

## 5. First boot

What you should see, in order:

1. The stock boot screen, then OpenDeck's screen at 1280×720 in place of
   the stock player. If the screen stays on the stock UI, the drop-in did
   not take; read `phase2.log`.
2. PLAY, CUE and the pads light as OpenDeck sets them. If every LED is
   dark, the sub-CPU device did not open; `opendeck.log` says why.
3. Touch the screen, turn the jog, move the tempo slider: the INFO page
   shows each input. If touch is off by a constant, the touch-space scale
   in the profile is wrong; report the coordinates.
4. Load the demo track and press PLAY: sound from the line out. Silence
   with the meter moving means the ALSA device name differs; try
   `aplay -l` over SSH and set `OPENDECK_AUDIO_DEVICE`.
5. Put another player on the LINK port: it lists OpenDeck, and SYNC and
   MASTER work both ways. The checklist is in
   [docs/testing/cdj-3000-interop.md](../testing/cdj-3000-interop.md).

Logs: `/home/root/settings/opendeck.log` on the deck's one writable
partition, readable over SSH or copied to the stick by the removal script.

## 6. Going back to stock

Either:

- **Front panel.** Power off. Hold **CUE/LOOP CALL ◀** and **TEMPO
  ±6/±10/±16/WIDE** together and power on while holding. The deck starts in
  service mode, clears the overlay, and boots stock on the next restart.
  (This is the community mods' removal path, and the overlay convention is
  shared on purpose so one method clears either.)
- **Removal `.UPD`.** Flash it the same way as the install; it must be the
  only `.UPD` on the stick.

Nothing needs restoring, because nothing was replaced.

## What is still unknown

- Whether the stock rootfs has any GPU userspace (step 0). Decides the
  screen.
- Whether the sub-CPU expects to be polled continuously (a watchdog or
  power-handling rule). The emulator documents reset and power-off, not a
  timeout. The adapter will poll at the stock rate regardless.
- Which ALSA device and sample rate the codec wants, and whether the stock
  launcher's real-time setup (core pinning, `SCHED_FIFO`) has to be
  reproduced for glitch-free audio. Plan: reproduce it.
- The jog LCD. A second display behind `/dev/dri/card0`, 2D only; the
  first version leaves it blank.
- The Renesas-generation deck: no plan.

## Help wanted

In order of value: the GPU check output from step 0 on any modded deck; a
Link capture from a stock 3000 (`tools/link_capture.py`, ten minutes, no
build); and, once the adapter exists, a volunteer with a modded deck and
root SSH who will run the payload by hand before anyone packs a `.UPD`.
