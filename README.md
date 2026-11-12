# FreeDJ-3000

A clean-room implementation of the Pioneer XDJ-1000MK2, in Rust. It is a
single-deck DJ media player with the XDJ's screen, controls and behaviour, and
it speaks Pro DJ Link on the wire, so it joins a network of CDJ-3000 and XDJ
players as one more deck: it takes a player number, follows the master tempo
or takes master and the real decks follow it, browses the other players'
media and serves its own.

It has two screens, switchable in MENU → SCREEN: the **XDJ-1000MK2** it
started as, and a **CDJ-3000** screen (BEAT LOOP / BEAT JUMP panels, SOURCE,
LOAD PREVIOUS TRACK, rekordbox category browsing, SEARCH with the on-screen
keyboard, GRID ADJUST). Both draw the same deck, so switching mid-set loses
nothing. How it was built: [My XDJ clone grew a CDJ-3000
screen](https://audiodestrukt.com/blog/posts/freedj-cdj-3000-skin.html).

It ships as **OpenDeck DJ** for iPad and iPhone on the
[App Store](https://apps.apple.com/app/id6807472453), and the same code runs
on a Raspberry Pi 5 and on any Linux desktop. Rendering is GPU-accelerated
(wgpu: Vulkan on Linux and the Pi, Metal on iOS) and paced from the display,
so the waveform scrolls at the panel's refresh rate on everything from a
phone to a Pi, while the audio engine runs on its own real-time thread.
The story of how it got here: [I cloned an XDJ-1000 and it runs on a
Raspberry Pi](https://audiodestrukt.com/blog/posts/cloning-an-xdj-1000.html).

> **"The CDJ-3000 costs $2,400. A Raspberry Pi 5 costs $80."**

---

## What is this?

The CDJ is the industry-standard DJ media player, found in every serious club
and festival worldwide. It is also a closed, proprietary device that costs
thousands of dollars, requires Pioneer's ecosystem to unlock basic features,
and charges recurring subscription fees for music analysis software.

FreeDJ-3000 reimplements the deck from the outside in. Nothing of Pioneer's is
in it: no firmware, no code, no imagery. The screen was laid out from
photographs of a real XDJ-1000MK2 (the CDJ-3000 screen was measured in
pixels against an emulated CDJ-3000 showing the same state), the controls are rendered procedurally, and
the protocols come from the public reverse-engineering work of the open source
community ([dysentery](https://github.com/Deep-Symmetry/dysentery),
[beat-link](https://github.com/Deep-Symmetry/beat-link),
[crate-digger](https://github.com/Deep-Symmetry/crate-digger)) and from
packets captured off the unit on the desk. Every Link feature is verified
against that real XDJ, not a simulator.

This project takes the position that DJ equipment protocols are
infrastructure, not intellectual property. It is not a "compatible" or
"inspired by" product. It is a direct alternative, designed to be
interoperable with Pioneer hardware on the same network.

---

## Status

**On the App Store and in daily use next to a real XDJ.** The deck, the Link
protocol, media sharing and both screens work; the physical control surface
and the sampler features are the roadmap. What is left on the CDJ-3000 side
is listed in [docs/TODO-cdj-3000.md](docs/TODO-cdj-3000.md).
The full, current feature list by area and platform is
[docs/FEATURES.md](docs/FEATURES.md).

| Feature | Status |
|---|---|
| MP3/FLAC/AAC/OGG/WAV/AIFF decode | ✅ Working |
| XDJ-1000MK2 screen: waveforms, beat grid, INFO, BROWSE, PERFORM, MENU/UTILITY | ✅ Working |
| CDJ-3000 screen: playback, BEAT LOOP/JUMP, INFO, SOURCE, LOAD PREVIOUS TRACK, BROWSE/TAG LIST/UTILITY, GRID ADJUST (SNAP/SHIFT GRID), SEARCH | ✅ Working; sorting, TRACK FILTER, KEY SHIFT still to do ([TODO](docs/TODO-cdj-3000.md)) |
| Play / CUE (hold-to-preview, back-cue) / hot cues A–H / memory cues / loops / beat jump | ✅ Working |
| Jog wheel: VINYL and CDJ modes, nudge, scratch | ✅ Working |
| SLIP mode | ✅ Working |
| Tempo fader ±16 %, MASTER TEMPO key lock (Rubber Band R3, Bungee or WSOLA) | ✅ Working |
| Beat detection + grid (MiniBPM + onset detector), GRID ADJUST | ✅ Working |
| rekordbox USB sticks: export.pdb browsing (ARTIST/ALBUM/TRACK/KEY/PLAYLIST/HISTORY), grids, cues, preview waveforms, covers | ✅ Working |
| rekordbox analysis import (beat grid, cues) over LINK | ✅ Working |
| Pro DJ Link: announce, beats, status, SYNC, master handoff both ways | ✅ Working on a real XDJ-1000MK2 |
| SYNC keeps a jog nudge as a phase offset; SHIFT GRID writes it into the grid | ✅ Working, end-to-end test against an emulated CDJ-3000 (`make test-shift-grid`) |
| Pro DJ Link media: browse and load from linked players / rekordbox | ✅ Working (dbserver + NFS) |
| Pro DJ Link media: serve tracks to other decks (metadata, grid, waveforms, art) | ✅ OpenDeck→OpenDeck proven; XDJ acceptance test pending |
| iPad / iPhone app (OpenDeck DJ) | ✅ On the App Store |
| Raspberry Pi 5 | ✅ 60 fps, ~0.6 of one core playing |
| Raspberry Pi Zero W / older GPUs (SDL backend, WSOLA key lock) | ✅ Plays cleanly; 49 fps on KMSDRM |
| Linux (x86_64, aarch64), Windows, macOS, Android (.apk) and iOS (.ipa) prebuilt releases | ✅ [Releases](https://github.com/audiodestrukt/freedj-3000/releases) |
| Control surfaces: any USB MIDI controller, PCF8574 I2C boards on the Pi, mapped in `controllers.toml` with LED feedback | ✅ MIDI on a DJ2Go; I2C path built and unit-tested, first board pending ([docs/hardware/control-surfaces.md](docs/hardware/control-surfaces.md)) |
| Sampler-deck: auto one-shots, quantized triggers, cue sequencing | 📋 Roadmap ([#24](https://github.com/audiodestrukt/freedj-3000/issues/24)) |
| A CDJ-3000 as freedj's control surface and screen (HID over LINK, Control Mode) | 🧪 Experimental, emulator only ([docs/design/cdj-hid-display.md](docs/design/cdj-hid-display.md)) |
| Hardware control surface (ADK-1000 panel board, RP2350 hub) | 📋 PCB generated and routed, not fabricated ([hardware/](hardware/README.md)) |
| CDJ-3000-generation media transfer | 📋 Not started (the 3000 moves files differently) |
| Running OpenDeck *on* a CDJ-3000 (RK3399, as a payload through the deck's updater) | 📋 Designed, not built ([docs/hardware/cdj-3000-install.md](docs/hardware/cdj-3000-install.md); the emulator and deck `.UPD`s: [docs/hardware/cdj-3000-upd.md](docs/hardware/cdj-3000-upd.md)) |

Everything in the Link rows has been exercised against an XDJ-1000MK2.
CDJ-3000s speak the same Pro DJ Link sync protocol. Sync and master handoff
have been tested against an emulated CDJ-3000 on a private network segment,
but a physical CDJ-3000 has not been on this network yet, so treat it as
compatible-by-protocol rather than tested.
**Own a CDJ-3000?** Ten minutes and a Python script would settle it:
[docs/testing/cdj-3000-interop.md](docs/testing/cdj-3000-interop.md).

---

## Hardware targets

**Primary: Raspberry Pi 5 (8GB) + RP2350 MCU**

The Pi 5 runs the audio engine and UI. An RP2350 microcontroller (Raspberry Pi's own chip) handles the physical control surface — jog wheel, encoders, buttons — over SPI/GPIO. The RP2350 firmware will be open source and part of this repo.

The Pi 5's GPU (VideoCore VII / Mesa V3DV) supports Vulkan 1.2, which is what the renderer requires. No X server needed — wgpu runs directly on DRM/KMS. Measured on the Pi 5: 60 fps with 0.12 ms frame jitter, about 0.6 of one core while playing with key lock on. A Pi 4 runs it as well, with the timestretcher taking most of one core.

**iPad and iPhone**

The same crates build into the iOS app (`ios/`), rendering through Metal. Pro DJ Link works over Wi-Fi or a USB ethernet adapter with no special entitlement.

**Fallback: Intel NUC or any x86 Linux box**

Any Linux machine with Vulkan support works for development and testing.

Bring-up on the Pi (hardware checklist, build, and the graphics/audio gaps to
expect) is in **`docs/reference/raspberry-pi.md`**.
Measured performance on real hardware — the Pi 5 is the reference target — is
in **`docs/BENCHMARKS.md`**.

**Display: any HDMI screen.** The reference layout is the XDJ-1000MK2's 7" 1024×600 panel (the window opens at that size); regions are fractions of the screen so the UI adapts to any resolution.

---

## Building

### Dependencies

```bash
# Debian/Ubuntu
sudo apt install libasound2-dev libvulkan-dev libwayland-dev \
                 libxkbcommon-dev librubberband-dev \
                 pkg-config build-essential

# Optional: poppler-utils, for `make reference`
sudo apt install poppler-utils

# Raspberry Pi (additionally)
sudo apt install mesa-vulkan-drivers
```

Rubber Band is linked dynamically. If you upgrade `librubberband` the existing
binary keeps its old soname and fails at startup with
`error while loading shared libraries: librubberband.so.N`. Cargo does not
notice, because no Rust source changed — force a relink with `make relink`.

### Rust toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Compile and run

```bash
git clone https://github.com/audiodestrukt/freedj-3000
cd freedj-3000
make run TRACK=/path/to/track.mp3
```

`make` on its own lists every target. The underlying commands are plain cargo —
`make build` is `cargo build --release -p opendeck-app` and nothing more.

### Releases (all platforms)

One tag builds every platform and publishes them together on one release,
"OpenDeck X.Y.Z" (marked latest), via `.github/workflows/release.yml`:

```bash
git tag v0.4.0 && git push origin v0.4.0
```

That release has the Linux tarballs, the Windows zip, the macOS zip, the
Android `.apk` and the iOS `.ipa`, and the same run uploads the iOS build to TestFlight (the version
must be higher than the last TestFlight upload). It is the only way a release
is published, so every release carries every platform. The per-platform tags
below (`linux-v*`, `win-v*`, `mac-v*`, `android-v*`, `ios-v*`) only build (and
`ios-v*` uploads to TestFlight); they publish nothing. For a tag that already
exists: `gh workflow run release.yml --ref v0.3.8`.

### Linux (prebuilt)

The [Releases page](https://github.com/audiodestrukt/freedj-3000/releases)
has `OpenDeck-<version>-linux-x86_64.tar.gz` and `…-linux-aarch64.tar.gz`
(Pi 3 / 4 / 5 / Zero 2 W on a 64-bit OS). Unpack, `./opendeck track.mp3`;
details in the bundled `README-LINUX.txt`. The binaries link only ALSA and
SDL2 from the system (Rubber Band and Bungee are built in) against glibc
2.36, so they run on Debian 12, Ubuntu 22.04, Raspberry Pi OS Bookworm or
newer:

```bash
sudo apt install libasound2 libsdl2-2.0-0 mesa-vulkan-drivers
```

Built by `.github/workflows/linux.yml` for every `v*` release; a `linux-v*`
tag or a manual run builds the tarballs as run artifacts only:

```bash
git tag linux-v0.3.0 && git push origin linux-v0.3.0
```

The 32-bit Pi Zero W / Pi 1 build needs a sysroot copied off the Pi, so it
stays local: `deploy/pi/cross/build.sh armv6` (see
`docs/reference/raspberry-pi.md`).

### Windows

Prebuilt: the [Releases page](https://github.com/audiodestrukt/freedj-3000/releases)
has `OpenDeck-<version>-windows-x64.zip` (unzip, run `opendeck.exe track.mp3`;
details in the bundled `README-WINDOWS.txt`). It is built by
`.github/workflows/windows.yml` for every `v*` release (a `win-v*` tag builds
it as a run artifact only):

```bash
git tag win-v0.3.0 && git push origin win-v0.3.0
```

Building locally needs only rustup and the Visual Studio Build Tools
("Desktop development with C++" — MSVC compiles the vendored Rubber Band and
MiniBPM); no other system libraries. Then `cargo build --release -p opendeck-app`.

### macOS

Prebuilt: the [Releases page](https://github.com/audiodestrukt/freedj-3000/releases)
has `OpenDeck-<version>-macos-universal.zip`: one `OpenDeck.app` for Apple
Silicon and Intel, macOS 11 or newer. It is ad-hoc signed, not notarized, so
the first launch needs right-click → Open (or
`xattr -dr com.apple.quarantine OpenDeck.app`). Double-clicking opens an empty
deck; to open a track straight away run
`OpenDeck.app/Contents/MacOS/opendeck track.mp3` from Terminal. Details in the
bundled `README-MAC.txt`. It is built by `.github/workflows/macos.yml` for
every `v*` release (a `mac-v*` tag builds it as a run artifact only):

```bash
git tag mac-v0.4.0 && git push origin mac-v0.4.0
```

Building locally needs only rustup and the Xcode command-line tools; then
`cargo build --release -p opendeck-app`.

### Android

Prebuilt: the [Releases page](https://github.com/audiodestrukt/freedj-3000/releases)
has `OpenDeck-DJ-<version>-android.apk` (arm64 phones and tablets, plus x86_64
for the emulator; Android 8.0 or newer, Vulkan or GLES 3.1). Install it from
the device; it is not on Google Play. Tracks go in the shared **Music** folder
(the app asks for music access on first launch); the demo track is seeded into
`Music/OpenDeck`. A phone gets the two landscape pages, a tablet the portrait
deck, as on iOS. Built by `.github/workflows/android.yml` for every `v*`
release (an `android-v*` tag builds it as a run artifact only):

```bash
git tag android-v0.4.0 && git push origin android-v0.4.0
```

There is no Gradle project: it is a NativeActivity (`crates/app/src/android.rs`)
and `deploy/android/build.sh [VERSION] [ABI...]` builds the APK with cargo-ndk
and the SDK's aapt2 / zipalign / apksigner. Locally it needs the Android SDK +
NDK, `cargo install cargo-ndk` and `rustup target add aarch64-linux-android
x86_64-linux-android`. Release builds are signed with
`~/private_keys/android-release.jks` (in CI, the `ANDROID_KEYSTORE_BASE64` /
`ANDROID_KEYSTORE_PASSWORD` secrets); never replace that key, or installed
copies stop accepting updates. Dev switches that are env vars on the desktop
go in a system property: `adb shell setprop debug.opendeck.env "OPENDECK_IDLE_FPS=0"`.
USB MIDI controllers are not supported on Android yet. A USB drive (rekordbox
stick) shows as `USB: XXXX-XXXX ALLOW ACCESS` until the app has "All files
access" (Android 11+); opening the row goes to that setting.

### Keyboard

The keyboard stands in for the deck's buttons on a desktop.

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `Enter` | CUE (momentary): set / return-to-cue / hold to preview |
| `,` / `.` | Jog: nudge (playing) / scrub (paused) |
| `V` | Jog mode: VINYL ↔ CDJ |
| `←` / `→` | Seek ±10 seconds |
| `+` / `−` / `0` | Tempo fader up / down / centre |
| `K` `S` `Y` `M` | MASTER TEMPO · SLIP · SYNC · MASTER |
| `N` · `[` / `]` · `Delete` | Memory cue: set · CALL previous / next · delete |
| `A` | GRID ADJUST (the arrows then move the grid) |
| `Z` / `X` | Zoom the enlarged waveform in / out |
| `C` | Waveform colour: RGB → 3 BAND → BLUE |
| `T` | TIME ↔ REMAIN |
| `P` | Phase meter: beat boxes ↔ master alignment |
| `B` `L` `I` `U` | BROWSE · TAG LIST · INFO · MENU/UTILITY |
| `O` · `H` | SOURCE · LOAD PREVIOUS TRACK |
| `/` | SEARCH; then type, `Backspace` deletes, `Esc` leaves |
| in a list: `↑`/`↓` · `Enter` · `Backspace` · `G` | Move · open / **load track** · back · TAG TRACK |
| `Q` / `Esc` | Quit (`Esc` closes an open screen first) |

The deck's look is chosen in MENU → SCREEN, or at launch with
`OPENDECK_SKIN=cdj3000` (or `xdj1000mk2`). The CDJ-3000 screen is the
default everywhere except on phones, which keep the XDJ-1000MK2 layout. The CDJ-3000 screen comes with
its hardware buttons: on the desktop, SOURCE / BROWSE / TAG LIST / SEARCH /
MENU and BACK / TAG TRACK / the rotary sit either side of the screen; on the
iPad (and with `--faceplate`) it is the whole top panel, every key where the
unit has it: the screen keys above the screen, hot cues under it (hold CALL /
DELETE and press a pad to delete one), loops, BEAT JUMP, TRACK SEARCH and
SEARCH down the left, CALL / DELETE / MEMORY over the jog, SYNC / MASTER,
the tempo keys and the fader down the right. The phone keeps the XDJ
layout for now.

**USB sticks.** Mounted removable media appear as `USB: <label>` rows at the
top of the browser (re-scanned once a second, so hot-plug works). A rekordbox
stick is detected by its `PIONEER/rekordbox/export.pdb` and browses by
rekordbox's categories (on the CDJ-3000 screen: ARTIST, ALBUM, TRACK, KEY,
PLAYLIST, HISTORY, FOLDER, with BPM/KEY columns and each track's preview
waveform), with rekordbox's grids and cues; anything else browses as
folders. Pulling the stick is safe: the loaded track is already in memory.
On a headless Pi install the appliance automount once
(`sudo deploy/pi/usb-automount/install.sh`, udev + systemd, read-only mounts
under `/media/usb`), see `docs/reference/raspberry-pi.md`.

### Touch screen (mouse for now)

egui treats mouse and touch as the same pointer, so the mouse stands in for
the panel. Press or drag on the overview to needle-search; drag the
enlarged waveform to jog; scroll over it to zoom; the on-screen keys and
panels (BROWSE, BEAT LOOP, hot-cue pads, the SEARCH keyboard…) respond to a
tap; the few still inert, like KEY SHIFT, are in the TODO list. Every input — keys and touch alike — is an `Event` on one bus applied in
`DeckApp::apply`; see `docs/INPUT_PLAN.md`.

### Controllers

Any class-compliant USB MIDI controller, and PCF8574 I2C button boards on
the Pi, are mapped to deck actions in `controllers.toml`, with LED feedback
([docs/hardware/control-surfaces.md](docs/hardware/control-surfaces.md)). A
**Numark DJ2Go** is the built-in preset (full map in
`docs/reference/dj2go-midi-map.md`): play, cue, sync, loop in/out, pitch
fader, jog, browse knob and load. `--deck A` (default) or `--deck B` picks
which side of the controller drives this instance, so two freedj instances
can share one DJ2Go. To map a different controller, run with
`RUST_LOG=opendeck::midi=debug`; every message is logged.

### Key-lock engine

Master tempo runs through Rubber Band R3 by default: the best-sounding
open-source stretcher, and the single biggest CPU cost on a Pi 4 (about 0.7
of a core). `OPENDECK_STRETCH=bungee` switches to [Bungee](https://github.com/kupix/bungee)
(vendored, MPL-2.0), an adaptive phase vocoder at roughly a sixth of the
cost. `OPENDECK_STRETCH=wsola` is the engine for boards that cannot afford
either: a time-domain WSOLA (the SoundTouch family old Mixxx used), pure
Rust, a sixth of Bungee again, with that family's sound at large ratios.
With master tempo off it is a plain varispeed read. Compare them on your
own tracks, master tempo on, blind:

```bash
cargo run --release -p opendeck-app --example stretch_ab -- track.mp3 /tmp/ab 0.92,1.08
make perf        # all three engines' real-time factors on this machine
```

### Render backend

wgpu (Vulkan / Metal / GL) draws the deck on the desktop, the Pi 4/5 and
iOS.  Boards whose GPU stops at OpenGL ES 2.0 (Pi Zero / Zero 2 W / Pi 3) or
a bare KMSDRM appliance use the SDL2 backend instead: build with
`--features renderer-sdl` and run with `OPENDECK_RENDERER=sdl`.  Same screen,
pixel-for-pixel on the waveforms (`tools/render-parity.sh`); see
`docs/reference/raspberry-pi.md`.

### ProDJ Link

FreeDJ-3000 is a Link peer: it announces itself on UDP 50000 every 1.5 s and
sends a beat packet on 50001 at every beat of the audible position, as player
`--player N` (default 1; `OPENDECK_PLAYER` also works). It listens on 50001
(where real CDJ/XDJ hardware sends beats) and 50002, ignoring its own
broadcasts. Status packets go out on 50002; SYNC follows the master and the
master handoff works both ways with a real XDJ. While synced, a jog nudge is
kept as a phase offset (as a CDJ does) until SYNC is pressed again or SHIFT
GRID writes it into the grid. `OPENDECK_LINK_IFACE=<iface>` pins Link to one
network interface, for example a private bridge to an emulated deck.

**Media over the link.** The LINK folder in the browser lists every peer's
media slots the way a player does ("3 USB: OPENDECK"; players and a
rekordbox laptop in LINK mode), opens the source's category menu (PLAYLIST /
ARTIST / ALBUM / TRACK / FILENAME) over Pioneer's dbserver protocol and loads
over NFSv2. The app also *serves* its music folder the same way, with tags,
duration, tempo, beat grid, waveforms and cover art filled in by a background
analysis that is cached after the first launch, so other decks list it under
LINK and load from it (`OPENDECK_SERVE=0` disables; `opendeck-serve <dir>` is
the stand-alone server). A track in flight shows a LOADING readout in the
title bar. Wire details, rekordbox quirks and test recipes:
`docs/reference/prodj-link-media.md`.

Two instances on one machine link to each other:

```bash
make link-pair                     # players 1 and 2, same track
make link-pair TRACK2=other.mp3
```

When a CDJ on the same network sends beats, the phase meter shows that deck's tempo and bar phase, and SYNC follows it.

For single-machine testing without real hardware:

```bash
python3 tools/send_beat.py 130.0 127.0.0.1 50002   # or: make two-deck BPM=130
```

With a real deck: connect it and this machine to the same switch (or a direct
ethernet cable — Pioneer decks auto-negotiate crossover), start playback on the
deck, and run `make dev`. Every packet received on 50000–50002 is logged in
full hex, which is the reference for the packet-format work in
`docs/WORKSTREAMS.md` §B1.

---

## Testing

```bash
cargo test --workspace      # ~215 unit tests, no hardware or network needed
make perf                   # key-lock engines' real-time factors on this machine
make test-shift-grid        # end-to-end: SYNC + SHIFT GRID against an emulated CDJ-3000
tools/cdj_parity.py --track T --out DIR   # the CDJ-3000 skin against the emulated unit, key by key
```

The unit tests are hermetic: they build their own fake USB sticks and
rekordbox exports and never scan the machine's real drives. The end-to-end
test needs a private Link bridge and the emulator (setup in
[docs/design/cdj-grid-adjust.md](docs/design/cdj-grid-adjust.md)); without
them it reports SKIP and exits 77. `OPENDECK_SCRIPT` drives the app from a
timed script (`"1:play; 3:sync; 14:jog=3x20; 26:shiftsync; 36:quit"`) for
tests like that one; see `crates/app/src/devscript.rs`. Its `press=` actions
take the emulator's button names, so `tools/cdj_parity.py` runs one scenario
on freedj and the emulated CDJ-3000 and compares their screens and lamps
([docs/testing/cdj-3000-parity.md](docs/testing/cdj-3000-parity.md)).

---

## Architecture

The project is a Cargo workspace. Each crate has a single responsibility.

```
crates/
  types/        — Core traits: Decoder, BeatAnalyzer, TimecodeDecoder
  decode/       — Audio decoding via Symphonia (MP3, FLAC, AAC, OGG, WAV, AIFF)
  analysis/     — Waveform FFT, beat grid computation
  minibpm-sys/  — Bindings to MiniBPM for tempo estimation
  engine/       — Real-time audio transport
  timestretch/  — Key lock (Rubber Band R3, Bungee, WSOLA) and speed change
  transport/    — CUE / PLAY rules as a pure state machine, checked against behaviour specs
  timecode/     — DVS vinyl timecode decoder (stub; not a current priority)
  link/         — Pro DJ Link packets: announce, beat, status, master handoff
  dbserver/     — Pioneer dbserver protocol (menus and metadata over TCP), client and server
  nfs/          — NFSv2/UDP client and server, the media transport between players
  rekordbox/    — export.pdb and ANLZ readers (vendored, patched rekordcrate)
  mediaserver/  — Serve a music folder to other decks (dbserver + NFS + background analysis)
  protocol/     — Control events; MCU serial protocol for the RP2350 control surface
  db/           — SQLite track library (beat grids, cue points, playlists, FTS5 search)
  ui/           — Shared UI components
  app/          — Main binary: deck, both screens (XDJ, CDJ-3000), browser, faceplate,
                  wgpu and SDL renderers, Link glue, control surfaces, iOS entry
```

### Graphics stack

- **wgpu** — GPU abstraction: Vulkan on Linux and the Pi, Metal on iOS
- **winit** — Window and input handling, DRM/KMS capable (no display server required)
- **egui** — Immediate-mode UI overlay (transport info, time display)
- **Custom WGSL shader** — Waveform renderer: frequency-colored bar chart, scrolls in real time

The UI philosophy is a **game HUD**, not an application. While playing, every frame redraws the full screen, paced by the compositor's frame callback so the playhead advances by whole display periods and never judders; paused and untouched, it idles at 10 fps. The drawn playhead free-runs at the true playback rate and is phase-locked to the audio clock. Audio runs on a separate real-time thread.

### Audio engine

The audio engine runs on an isolated real-time thread and communicates with the UI via lock-free atomics. The callback-based cpal stream advances an `AtomicU64` position counter that the renderer reads each frame.

Speed changes use the Rubber Band R3 time-stretcher (`crates/timestretch/`, or Bungee / WSOLA, see above) for key-locked pitch-preserving playback across the full ±16% pitch range; MASTER TEMPO off switches to plain varispeed. Stops fade over 3 ms so a released CUE never clicks; starts have no fade, so CUE lands on the beat. SLIP keeps a shadow playhead running under loops and held hot cues, and jumps to it when they end. Every track, including the one opened at start-up, is resampled to the audio device's rate on load, so a 48 kHz file plays at the right speed on a 44.1 kHz device.

### Waveform analysis

Each waveform column is a 2048-sample FFT window (Hann-windowed, HOP_SIZE=512) mapped to three frequency bands:

- **R** — Bass (20–200 Hz)
- **G** — Mid (200 Hz–2 kHz)
- **B** — High (2 kHz–20 kHz)
- **A** — Overall RMS amplitude (controls bar height)

The entire waveform for a 5-minute track is computed in ~0.1 seconds and stored as a GPU storage buffer, one `u32` per column.

### Beat detection

Beat analysis runs offline before playback starts. A 6-second analysis window is fed to [MiniBPM](https://github.com/breakfastquay/minibpm) for tempo estimation, followed by a half-wave-rectified spectral flux onset detector that finds the beat anchor by maximising onset energy at grid positions. The result is a `BeatGrid` with a confidence score, BPM, and anchor sample position.

The beat grid is displayed as white tick marks on the waveform (red on downbeats). The displayed BPM scales with the current pitch-fader position so it always shows the effective playback tempo.

The phase meter shows the master deck's beat and bar position from Pro DJ Link beat packets. Over Wi-Fi those arrive in bursts (access points hold broadcasts for sleeping clients), so the other deck's phase is a phase-locked free-run nudged by each packet rather than a direct readout.

### Protocol compatibility

**Pro DJ Link** — Pioneer's CDJ network protocol, documented by the [dysentery](https://github.com/Deep-Symmetry/dysentery) project. FreeDJ-3000 implements the announce, beat and status packets, the SYNC/master handshake, the dbserver menu protocol and the NFS media transport, so it syncs tempo and phase with real players, hands master back and forth with them, and exchanges tracks with them.

---

## Philosophy

Pioneer makes good hardware. They also use their market position to maintain a closed ecosystem that extracts money from DJs and venues at every step: expensive hardware, rekordbox subscription, proprietary USB formats, licensing fees for third-party integration.

The open source DJ tooling community (Mixxx, xwax, beat-link, dysentery) has done remarkable work. This project's goal is to turn that knowledge into a complete, deployable hardware product that any DJ can build, any venue can install, and any developer can extend.

The name is deliberately provocative. The CDJ-3000 is not a trademark we are imitating — it is a benchmark we are matching. The deck it reproduces control for control is the XDJ-1000MK2, because that is the one on the desk.

---

## Contributing

The codebase is Rust throughout. Contributions welcome in any area. The most useful near-term work:

- **CDJ-3000 parity** — the open items in [docs/TODO-cdj-3000.md](docs/TODO-cdj-3000.md): sorting, TRACK FILTER, KEY SHIFT, key detection, hot-cue colours
- **A physical CDJ-3000 on the network** — run the [interop kit](docs/testing/cdj-3000-interop.md) and report back
- **RP2350 firmware** — no_std Rust for the control surface MCU
- **XDJ media-source acceptance** — an XDJ browsing and loading from OpenDeck over LINK (dbserver + NFS servers are in; see `docs/reference/prodj-link-media.md`)
- **Hardware** — the panel board is generated and routed ([hardware/](hardware/README.md)); it needs its control positions checked and a first fab

A change that adds, removes or changes a feature updates
[CHANGELOG.md](CHANGELOG.md) and [docs/FEATURES.md](docs/FEATURES.md) in the
same commit.

See `AUDIO_ENGINE.md` for detailed design documentation on the audio engine,
and `docs/design/track-loading.md` for how a track load runs off the UI thread.

---

## License

GPL-2.0-or-later. If you build a product with this, the product must be open source. That is intentional.

Protocol implementations (ProDJ Link, timecode formats) are based on publicly documented reverse-engineered specifications and are not subject to any Pioneer trademark or patent claims we are aware of. If you believe otherwise, open an issue.
