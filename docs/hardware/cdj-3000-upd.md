# OpenDeck as a CDJ-3000 `.UPD`: the emulator image and the deck overlay

Status: **emulator UPD built and booted in QEMU** (2026-11-12); deck UPD designed, not built. This page is the reference
for how OpenDeck is packaged as `.UPD` files: what kinds there are, what is
inside each, how each is consumed, and how they are built. The install
steps for a real deck are in [cdj-3000-install.md](cdj-3000-install.md).

| Piece | State |
|---|---|
| Sub-CPU panel surface (`OPENDECK_SUBUCOM`, keys / jog / fader / rotary / LEDs) | built, unit-tested, not yet run on the device ([control surface](#the-panel-is-the-same-device-in-both)) |
| Emulator UPD (`deploy/cdj3000/emu/`, `tools/upd/updpack.py emu`) | built; passes cdj3k-emu's install code (`tools/upd/emucheck`) and boots in QEMU with the emulator's kernel; not yet installed through the wizard |
| `make upd-deck` (deck overlay) | not built |
| Release job (`.github/workflows/cdj3000.yml`, called by `release.yml`) | built: every `v*` release attaches the plaintext `.iso`, plus the encrypted `.UPD` when the `CDJ3000_UPD_KEY` secret is set |

## The one thing to understand first

**There are two different UPDs, and neither works where the other does.**

| | Emulator UPD (`upd-emu`) | Deck UPD (`upd-deck`) |
|---|---|---|
| Runs on | [cdj3k-emu](https://github.com/nsaintot/cdj3k-emu), installed through its firmware wizard into a slot | A real CDJ-3000 (RK3399, firmware 3.13 to 3.22), through the deck's own updater |
| What it is | A **whole firmware image**: OpenDeck's own Linux userland in the layout of a Pioneer firmware release | An **overlay payload**: OpenDeck plus installer scripts, laid over the deck's stock system |
| Replaces | The slot's entire guest system (no Pioneer software runs in that slot) | Only the stock player *process*; Pioneer's kernel, drivers and boot stay |
| Pioneer code inside | None | None |
| Removal | Install a stock firmware UPD into the slot again, or empty the slot | Front-panel combination at power-on, or a removal UPD ([install §6](cdj-3000-install.md#6-going-back-to-stock)) |

Why they cannot be one file:

- **The emulator UPD must never go near a deck.** A deck's updater treats a
  firmware-shaped UPD as a firmware release and writes it to eMMC as the
  deck's operating system. Ours carries no bootloader, no Pioneer kernel
  and no drivers for the real board: the deck would not boot. Its file
  name says `EMU` and its `RELEASE.TXT` says so too; the deck refuses it
  only if its updater checks something we leave out, which is not a safety
  margin to rely on. **Never copy an emulator UPD to a deck's stick.**
- **The deck UPD does nothing in the emulator.** The wizard looks for a
  kernel and a firmware layout; an overlay payload has neither and is
  rejected.

## What a `.UPD` is

Both kinds share the outer shape of Pioneer's firmware files:

```
CDJ3Kv###.UPD        LUKS1 container (aes-xts-plain64 or aes-cbc-essiv:sha256)
└── ISO 9660 image   the plaintext once the container is opened
```

The container is encrypted with the CDJ-3000's packaging key. Whoever can
open a stock CDJ-3000 firmware UPD (the deck itself; cdj3k-emu with the key
its user supplies) can open ours. **This project does not publish the key
and does not say where to get one.** See [Keys](#keys).

The CDJ-3000X uses a different key and different frames; it is not covered.

## The emulator UPD

### What the emulator does with a UPD

From cdj3k-emu's `crates/cdj3k-emu-firmware` (read 2026-11-11):

1. **Decrypt** the LUKS1 container with the user's key, to an ISO.
2. **Read the firmware info** from the inner ISO `IMAGES/CDJ3K-RK3399.ISO`:
   `IMAGES/RELEASE.TXT`, `IMAGES/APP.REV`, `IMAGES/SYSTEM.REV`, and an MD5
   of `IMAGES/MINILOADER.IMG`. These end up in the slot's U-Boot
   environment and its "installed firmware" label.
3. **Pick the kernel** from the inner ISO's `IMAGES/IMAGES.TAR.GZ`: a file
   named `Image` whose path or contents contain `rk3399` (anything else is
   rejected as the Renesas generation).
4. **Take the root filesystem out of that kernel.** On a CDJ-3000 the whole
   rootfs is the gzip cpio embedded in the kernel image
   (`CONFIG_INITRAMFS_SOURCE`). The emulator keeps the largest embedded
   cpio it finds and **boots its own vanilla kernel** with it; the kernel
   from the UPD is never run.
5. **Patch the rootfs** with its numbered scripts
   (`initramfs-patch/patch-rootfs.d/`): SSH, root password, serial login,
   `subucom_virt` (the sub-CPU), Xorg on the virtual GPU, `virtio_snd`,
   USB, the PC-link bridge, the mods runner, and others.
6. **Stage the eMMC** for the slot with a partition table and that U-Boot
   environment, plus `IMAGES/CABINET.IMG` from the ISO in the partition one
   of the patches reads.

### What ours contains

```
CDJ3Kv###-OPENDECK-EMU.UPD           LUKS1, encrypted with the packaging key
└── ISO 9660
    └── IMAGES/
        └── CDJ3K-RK3399.ISO         inner ISO, as on a 3.13+ release
            └── IMAGES/
                ├── RELEASE.TXT      "OpenDeck <version> (cdj3k-emu image)"
                ├── APP.REV          OpenDeck's version
                ├── SYSTEM.REV       the rootfs build id
                ├── MINILOADER.IMG   placeholder (the emulator only hashes it; open question below)
                ├── CABINET.IMG      placeholder or empty (open question below)
                └── IMAGES.TAR.GZ
                    └── rk3399/Image a carrier "kernel": a stub whose only job is
                                     to hold our rootfs as its embedded gzip cpio
```

The rootfs inside the carrier, as designed:

| Part | What | Why |
|---|---|---|
| Userland | Debian 12 arm64 with systemd as `/init`, plus `busybox` (`deploy/cdj3000/emu/Containerfile`) | The emulator's patches write systemd units; its provisioning guest runs them with the image's own `bash`, `tar` and `busybox`. Patch 20 needs `/home/root/scripts/`; a no-op `wlan-monitor` unit is there for the older patch 23 that required one |
| Display | No X server: SDL's software renderer off-screen, each frame copied to `/dev/fb0` (`OPENDECK_FBDEV`) | SDL's KMSDRM page flips never reach the emulator's scanout; the kernel's fbdev emulation does. No GL means no llvmpipe threads competing with audio under TCG (one core instead of four) |
| Audio | ALSA | The emulator's `virtio_snd` (patch 17) is a normal ALSA card |
| OpenDeck | `/opt/opendeck/opendeck` and its unit, started once X is up | |
| Panel | Nothing extra: `OPENDECK_SUBUCOM=/dev/subucom_spi1.0` in the unit | `subucom_virt` (patch 08) creates the device; the emulator's drawn panel feeds it |
| LINK | systemd-networkd on the virtio NIC, link-local addressing | Pro DJ Link to other slots and to the host's `djlink0` bridge |

The OpenDeck in the image is the same binary as the deck overlay's
(aarch64, glibc 2.17), so whatever works in the emulator is what ships to
the deck.

### The panel is the same device in both

On a real deck and in the emulator, every button, the jog, the tempo fader,
the browse rotary and every LED are reached through one char device,
`/dev/subucom_spi1.0`: `read()` returns a 64-byte input frame at about
850 Hz, and the `MOSI_TRANSFER` ioctl takes a 64-byte LED frame. OpenDeck
reads it with `OPENDECK_SUBUCOM` (`crates/app/src/surface/subucom.rs`). That
is why the emulator is a faithful rehearsal of the deck for the controls:
same device, same frames. The frame layout is cdj3k-emu's published one
(`docs/subucom.md` there).

### What the checks answered

- A failing patch script stops the install (`patch-rootfs.sh` runs with
  `set -e`). With the two shims above, all of them pass on our rootfs.
- `CABINET.IMG` is optional: without it the wizard stages nothing.
  `MINILOADER.IMG` is only hashed.
- The carrier `Image` is never booted, only searched for the largest
  embedded gzip cpio.
- ISO names: `-relaxed-filenames` keeps `CDJ3K-RK3399.ISO`, and
  `-allow-multidot` keeps `IMAGES.TAR.GZ` (plain ISO 9660 turns it into
  `IMAGES_TAR.GZ`, which the emulator does not find).

### Building and checking it

```sh
deploy/pi/cross/build.sh aarch64                       # OpenDeck, aarch64
deploy/cdj3000/emu/build-rootfs.sh target/upd/rootfs.tar
python3 tools/upd/updpack.py emu --rootfs target/upd/rootfs.tar \
    --add target/aarch64-unknown-linux-gnu/release/opendeck-stripped:opt/opendeck/opendeck \
    --add amplificate.mp3:opt/opendeck/amplificate.mp3 \
    --version 0.0.0-dev --out target/upd [--key /path/to/packaging.key]
# cdj3k-emu's own install pipeline over the result (RES = its patch/ + tools/):
(cd tools/upd/emucheck && cargo run --release -- ../../../target/upd/CDJ3Kv999-OPENDECK-EMU.iso /tmp/chk --patch RES)
```

## The deck UPD

The design is in [cdj-3000-install.md §3](cdj-3000-install.md#3-pack-the-upd);
in short:

```
CDJ3Kv###.UPD                        LUKS1, encrypted with the packaging key
└── ISO 9660 (the community mods' updater layout)
    ├── payload.sh                   at update time, as root: copy the payload into the
    │                                overlay, add the drop-in that starts run.sh in place
    │                                of the stock player, optional root SSH
    ├── phase1.sh, phase2.sh         the two reboots after: finish, check `ldd` is clean
    └── opendeck/                    → /home/root/opendeck/ on the deck
        ├── opendeck                 aarch64, glibc 2.17
        ├── run.sh                   environment (OPENDECK_SUBUCOM, WGPU_BACKEND=gl, …) + exec
        └── lib/                     anything the deck's userland lacks (Mesa, if no GPU driver)
```

The deck's kernel, drivers, X server and boot stay stock; nothing is
reflashed. The all-or-nothing rule: outside firmware 3.13 to 3.22, or if
the binary does not load, nothing is installed and the deck boots stock.

## How they are built

Both from source, with the same make targets for everyone:

```sh
make upd-emu  KEY=/path/to/packaging.key    # → target/upd/CDJ3Kv###-OPENDECK-EMU.UPD
make upd-deck KEY=/path/to/packaging.key    # → target/upd/CDJ3Kv###.UPD
```

Steps, in order:

1. Build OpenDeck for aarch64 against glibc 2.17 (`cargo zigbuild`) and
   check the result needs nothing newer.
2. **Emulator:** build the rootfs, pack it as a gzip cpio into the carrier
   `Image`, write the inner ISO, then the outer ISO.
   **Deck:** lay out the payload and scripts, write the ISO.
3. Encrypt the ISO into a LUKS1 container with `KEY`.

Without `KEY`, the build stops after step 2 and says so. The unencrypted
ISO and rootfs stay in `target/upd/`, so anyone can build and inspect every
byte that would go into a UPD without having a key.

### Keys

- The key is read **only from the path given** in `KEY`. It is never copied
  into the build tree, written to a log, or kept in any output other than
  the encrypted UPD's own LUKS header (which holds key slots, not the key).
- No key, key file, or key-derived value is ever committed. The repository
  `.gitignore` ignores every `*.key` file as a second line of defence.
- The docs say where `KEY` goes, not where a key comes from.
- **Pre-built files.** Every release attaches the emulator image as a
  plaintext `.iso`: cdj3k-emu installs it with no key (the wizard's
  "decrypted .iso", or `cdj3k-emu --instance N --model cdj3k --provision
  <iso>`). The encrypted `.UPD` is attached too when the maintainer's key
  is stored as the `CDJ3000_UPD_KEY` repository secret (base64); CI
  decodes it to a temp file for the one encryption step and deletes it.
  Anyone with a key of their own can build the same files from the same
  source instead.

### Naming

- Emulator: `CDJ3Kv<NNN>-OPENDECK-EMU.UPD`, where `<NNN>` is the firmware
  number the emulator shows for the slot. The `-OPENDECK-EMU` tail keeps
  it from being mistaken for a deck file.
- Deck: `CDJ3Kv<NNN>.UPD`, the name the deck's updater looks for. Exactly
  one `.UPD` on the stick, or the deck stops and asks.

## Using the emulator UPD

1. In cdj3k-emu, open the firmware wizard for a spare slot (one you do not
   mind losing; installing replaces whatever is in it).
2. Pick `CDJ3Kv###-OPENDECK-EMU.UPD`; give the wizard the key it already
   asks for with stock firmware.
3. Start the slot. OpenDeck comes up on the screen; the drawn panel drives
   it; Pro DJ Link sees it as a player.

To go back, install a stock firmware UPD into the slot again.
