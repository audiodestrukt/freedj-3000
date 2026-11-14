# OpenDeck (FreeDJ-3000)

An open-source DJ media player, in Rust. One deck with a CDJ-3000's screen,
keys and behaviour (or the XDJ-1000MK2's, switchable in MENU → SCREEN) that
speaks Pro DJ Link, so it joins a network of CDJ-3000s, XDJs and rekordbox as
one more player:

- it takes a free player number;
- it follows the master tempo, or takes master and the real decks follow it;
- it browses and loads from the other players' media;
- it serves its own music, which a CDJ-3000 lists, loads and plays with
  OpenDeck's beat grid, waveforms, key, hot cues and memory cues.

It ships as **OpenDeck DJ** for iPad and iPhone on the
[App Store](https://apps.apple.com/app/id6807472453), and as prebuilt
releases for Linux (x86_64 and aarch64, including the Raspberry Pi), Windows,
macOS and Android. Rendering is on the GPU (wgpu: Vulkan, Metal, or GL), with
an SDL backend for boards as small as a Pi Zero W. Frames are paced from the
display, and the audio engine runs on its own real-time thread.

How it got here:

- [I cloned an XDJ-1000 and it runs on a Raspberry Pi](https://audiodestrukt.com/blog/posts/cloning-an-xdj-1000.html)
- [My XDJ clone grew a CDJ-3000 screen](https://audiodestrukt.com/blog/posts/freedj-cdj-3000-skin.html)

> **"The CDJ-3000 costs $2,400. A Raspberry Pi 5 costs $80."**

---

## What is this?

The CDJ is the industry-standard DJ media player, found in every serious club
and festival worldwide. It is also a closed, proprietary device that costs
thousands of dollars, requires Pioneer's ecosystem to unlock basic features,
and charges recurring subscription fees for music analysis software.

OpenDeck reimplements the deck from the outside in. Nothing of Pioneer's is in
it: no firmware, no code, no imagery.

- **Screens:** the XDJ-1000MK2 screen was laid out from photographs of a real
  unit. The CDJ-3000 screen and faceplate were measured in pixels against an
  emulated CDJ-3000 showing the same state.
- **Controls** are drawn procedurally.
- **Protocols** come from the public reverse-engineering work of the open
  source community ([dysentery](https://github.com/Deep-Symmetry/dysentery),
  [beat-link](https://github.com/Deep-Symmetry/beat-link),
  [crate-digger](https://github.com/Deep-Symmetry/crate-digger)) and from
  packets captured off the units: a real XDJ-1000MK2 on the desk and an
  emulated CDJ-3000 on a private network.

This project takes the position that DJ equipment protocols are
infrastructure, not intellectual property. It is not a "compatible" or
"inspired by" product. It is a direct alternative, designed to be
interoperable with Pioneer hardware on the same network.

---

## Status

**On the App Store and in daily use next to a real XDJ.** The full feature
list, by area, with what has been tested where, is
**[docs/FEATURES.md](docs/FEATURES.md)**. What changed when is in
[CHANGELOG.md](CHANGELOG.md).

In short:

- **Deck:** PLAY / CUE, hot cues A–H, memory cues, loops, BEAT JUMP, SLIP,
  QUANTIZE, jog (VINYL / CDJ), tempo ±6 to WIDE, and MASTER TEMPO key lock
  (Rubber Band R3, Bungee or WSOLA).
- **Tracks:**
  - MP3, AAC, ALAC, FLAC, WAV, AIFF and OGG at any sample rate.
  - A track is playable as soon as the audio around the cue is decoded, and
    the rest streams in.
  - rekordbox USB sticks browse by rekordbox's categories, with their grids
    and cues.
  - Stems prepared with OverCue play with per-stem levels. Splitting on the
    device is experimental.
- **Analysis:** BPM and beat grid, key and phrases, for tracks rekordbox has
  not analysed (BPM matches rekordbox's on 222 of 226 test tracks).
- **Screens:** the CDJ-3000 screen and faceplate (the default; the iPhone
  keeps the XDJ-1000MK2 layout) and the XDJ-1000MK2 screen. Waveform colour
  is 3BAND, RGB or BLUE.
- **Pro DJ Link:**
  - **Sync:** SYNC, MASTER handoff both ways, and PLAYER No. AUTO.
  - **Media:** browse and load from other players and rekordbox, and serve
    OpenDeck's own music to them.
  - **Lighting and timecode tools:** the playing track's analysis is served
    from the moment it loads, and CDJ-3000 Absolute Position packets go out
    every 30 ms.

**What has been tested on what:**
- **Real XDJ-1000MK2:** every sync and master row.
- **Emulated CDJ-3000** (Pioneer's firmware 3.22, on a private network
  segment):
  - It browses OpenDeck by artist, album, track, key and playlist.
  - It loads a track and plays it with OpenDeck's waveforms, grid, key, hot
    cues and memory cue, and its hot cue pads jump to them.
  - Sync and SHIFT GRID work against it.
  - Our position packets match its byte for byte.
- **Physical CDJ-3000:** not on this network yet. **Own one?** Ten minutes and
  a Python script would settle it:
  [docs/testing/cdj-3000-interop.md](docs/testing/cdj-3000-interop.md).

**Next:**
- rekordbox's hot cue colours and comments, phrases and colour waveforms,
  read from sticks and passed through to CDJs.
- Cues set on OpenDeck saved across launches.
- The remaining CDJ-3000 screen work:
  [docs/TODO-cdj-3000.md](docs/TODO-cdj-3000.md).
- A hardware control surface and a sampler deck.

---

## Hardware targets

**Raspberry Pi 5.** The reference board. 60 fps with 0.12 ms frame jitter,
about 0.6 of one core while playing with key lock on. wgpu runs on DRM/KMS
with no X server. A Pi 4 runs it too, with the timestretcher taking most of a
core. A Pi Zero W (one ARMv6 core) plays cleanly on the SDL renderer with the
WSOLA key lock, at 49 fps. Bring-up, build and the gaps to expect are in
**`docs/reference/raspberry-pi.md`**. Measurements are in
**`docs/BENCHMARKS.md`**.

**iPad and iPhone.** The same crates build into the iOS app (`ios/`),
rendering through Metal. Pro DJ Link works over Wi-Fi or a USB ethernet
adapter with no special entitlement.

**Desktop and Android.**
- Any Linux, Windows or Mac machine runs it.
- Android runs it as an APK (not on Google Play).

**A control surface.** Today the controls are:
- any USB MIDI controller, mapped in `controllers.toml`;
- PCF8574 I2C button boards on the Pi.

The ADK-1000 panel board (RP2350 hub) is generated and routed but not made
([hardware/](hardware/README.md)).

**Display: any screen.**
- The CDJ-3000 screen is drawn at the unit's 1280×720 and scales to any
  window.
- The XDJ-1000MK2 layout is 1024×600.
- Both are laid out in fractions of the screen.

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
| `F` | DECK VIEW: the whole deck (jog, keys, fader) ↔ the screen alone (also MENU → DECK VIEW; saved) |
| `B` `L` `I` `U` | BROWSE · TAG LIST · INFO · MENU/UTILITY |
| `O` · `H` | SOURCE · LOAD PREVIOUS TRACK |
| `/` | SEARCH; then type, `Backspace` deletes, `Esc` leaves |
| in a list: `↑`/`↓` · `Enter` · `Backspace` · `G` | Move · open / **load track** · back · TAG TRACK |
| `Q` / `Esc` | Quit (`Esc` closes an open screen first) |

The deck's look is chosen in MENU → SCREEN, or at launch with
`OPENDECK_SKIN=cdj3000` (or `xdj1000mk2`). The CDJ-3000 screen is the
default everywhere except on phones, which keep the XDJ-1000MK2 layout. The CDJ-3000 screen comes with
its hardware buttons. On the Mac, Windows and the iPad it is the whole top
panel (the default; `--screen-only` drops it on the desktop), every key where
the unit has it: the screen keys above the screen, hot cues under it (hold CALL /
DELETE and press a pad to delete one), loops, BEAT JUMP, TRACK SEARCH and
SEARCH down the left, CALL / DELETE / MEMORY over the jog, SYNC / MASTER,
the tempo keys and the fader down the right. On Linux the default is the
screen with SOURCE / BROWSE / TAG LIST / SEARCH / MENU and BACK / TAG TRACK /
the rotary either side of it (`--faceplate` for the whole panel). `F` or
MENU → DECK VIEW switches between the two at any time, and the choice is
saved. The first window is scaled down to fit a smaller screen.

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

### Touch screen

On the iPad, iPhone and Android the screen is the panel; on the desktop the
mouse stands in for a finger. Press or drag on the overview to needle-search; drag the
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
which side of the controller drives this instance, so two OpenDeck instances
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

### Pro DJ Link

OpenDeck is a Link peer.

**Player number.** It announces itself on UDP 50000 and claims a free player
number the way a CDJ does. PLAYER No. AUTO tries 4, 5, 6, 3, 2, 1 in that
order, keeps the number it gets, and moves if another player takes it.
`--player N` (or `OPENDECK_PLAYER`) fixes the number, 1–6; 5 and 6 work only
on an all-CDJ-3000 network.

**What it sends:**
- beats on 50001, from the audible position;
- status on 50002: the CDJ-3000's long form, with the active loop, and the
  short form to older players;
- CDJ-3000 Absolute Position packets every 30 ms, carrying the playhead, the
  track length, the pitch and the BPM. They go to each device, as a CDJ-3000
  sends them.

**Sync.** It listens on 50001 and 50002. SYNC follows the master's tempo and
phase. MASTER hands over both ways with a real XDJ. While synced, a jog nudge
is kept as a phase offset, as a CDJ does, until SYNC is pressed again or
SHIFT GRID writes it into the grid.

`OPENDECK_LINK_IFACE=<iface>` pins Link to one network interface, for example
a private bridge to an emulated deck, or `lo` to share the machine with
[Deckbench](docs/testing/deckbench.md).

**Media from other players.** The LINK rows in SOURCE list every peer's media
slots the way a player does ("3 USB: OPENDECK", players and a rekordbox laptop
in LINK mode). OpenDeck browses their categories over Pioneer's dbserver
protocol and loads the audio over NFSv2.

**OpenDeck as a media source.** The app serves its music folder the same way.
It runs a dbserver on TCP 1051 (12523 tells players where to find it) and an
NFSv2 server. A background analysis fills in each track's tags, duration,
tempo, key, beat grid, waveforms (blue, RGB and 3-band), phrases and cover
art, and caches them after the first launch. A CDJ-3000 then lists the
folder in SOURCE, browses it by artist, album, track, key, playlist, history
and matching, and loads from it. It shows OpenDeck's hot and memory cues.

**The playing track.** Whatever OpenDeck is playing is served under the ID its
status names, from the moment it loads, even when the file came from a stick
or the Files app. Other players and lighting controllers can then look up its
grid, waveforms and phrases. A grid adjusted on the deck is the grid they get.

`OPENDECK_SERVE=0` turns the media source off. `opendeck-serve <dir>` is the
stand-alone server. Wire details, rekordbox quirks and test recipes are in
`docs/reference/prodj-link-media.md`.

Two instances on one machine link to each other:

```bash
make link-pair                     # two decks, same track
make link-pair TRACK2=other.mp3
```

For single-machine testing without real hardware:

```bash
python3 tools/send_beat.py 130.0 127.0.0.1 50002   # or: make two-deck BPM=130
```

With a real deck, connect it and this machine to the same switch, or use a
direct ethernet cable (Pioneer decks auto-negotiate crossover). Start
playback on the deck and run `make dev`. Every packet received on
50000–50002 is logged in full hex.

---

## Testing

```bash
cargo test --workspace      # ~355 tests, no hardware or network needed
make perf                   # key-lock engines' real-time factors on this machine
make test-shift-grid        # end-to-end: SYNC + SHIFT GRID against an emulated CDJ-3000
tools/cdj_parity.py --track T --out DIR   # the CDJ-3000 skin against the emulated unit, key by key
```

The tests are hermetic. They build their own fake USB sticks and rekordbox
exports and never scan the machine's real drives.

**Deck harness.** The deck runs headless in tests (`deck_harness`), with the
real audio processor and no window or sound card. A test presses keys as a DJ
would and checks what is heard: SLIP returning after a hot cue, a loop
wrapping, TEMPO RANGE.

**Protocol tests** check replies byte for byte against captures from an
emulated CDJ-3000: track rows, cue lists, position packets.

**Link checks** run against [Deckbench](docs/testing/deckbench.md), which
simulates players and a mixer on the loopback.

**End-to-end against the emulator.** These need a private Link bridge and the
emulator (setup in
[docs/design/cdj-grid-adjust.md](docs/design/cdj-grid-adjust.md)); without
them they report SKIP and exit 77.

**Scripts.** `OPENDECK_SCRIPT` drives the app from a timed script, such as
`"1:play; 3:sync; 12:press=HOT_A; 26:shiftsync; 36:quit"`; see
`crates/app/src/devscript.rs`. Its `press=` actions take the emulator's button
names, so `tools/cdj_parity.py` runs one scenario on OpenDeck and on the
emulated CDJ-3000 and compares their screens and lamps
([docs/testing/cdj-3000-parity.md](docs/testing/cdj-3000-parity.md)).

---

## Architecture

The project is a Cargo workspace. Each crate has a single responsibility.

```
crates/
  types/        — Core traits and types shared by every crate
  decode/       — Audio decoding via Symphonia (MP3, AAC, ALAC, FLAC, OGG, WAV, AIFF; FLAC vendored for 32-bit)
  analysis/     — Beat grid, key, phrases, waveforms (offline and streaming)
  minibpm-sys/  — Bindings to MiniBPM, the first tempo guess
  engine/       — Real-time audio transport
  timestretch/  — Key lock (Rubber Band R3, Bungee, WSOLA), speed change and resampling
  transport/    — CUE / PLAY rules as a pure state machine, checked against behaviour specs
  link/         — Pro DJ Link packets: announce, beats, status, position, master handoff
  dbserver/     — Pioneer dbserver protocol (menus, metadata, analysis over TCP), client and server
  nfs/          — NFSv2/UDP client and server, the media transport between players
  rekordbox/    — export.pdb and ANLZ readers (vendored, patched rekordcrate)
  mediaserver/  — Serve a music folder to other decks (dbserver + NFS + background analysis)
  stems/        — OverCue-prepared stems (the CDJMODS USB format)
  protocol/     — Control events; MCU serial protocol for the RP2350 control surface
  timecode/     — DVS vinyl timecode decoder (stub; not a current priority)
  db/           — SQLite track library (not yet used by the app)
  ui/           — Shared UI components
  app/          — Main binary: the deck, both screens and faceplates, browser, Link
                  glue, wgpu / SDL / framebuffer output, control surfaces, iOS and Android entry
```

### Graphics

- **wgpu**: Vulkan on Linux and the Pi, Metal on iOS and the Mac, GL where
  that is all there is.
- **SDL2**: for GLES 2 boards and bare KMSDRM.
- **winit**: windows and input.
- **egui**: the screens, keys and panels.
- **WGSL shaders**: the waveforms.

The UI is a **game HUD**, not an application.
- **Playing:** every frame redraws the whole screen, paced by the display's
  frame callback, so the playhead moves in whole display periods and never
  judders. The drawn playhead free-runs at the true playback rate and is
  phase-locked to the audio clock.
- **Paused and untouched:** it idles at 10 fps.

### Audio engine

The audio engine runs on its own real-time thread and talks to the UI through
lock-free atomics.

- **Key lock:** MASTER TEMPO uses the selected key-lock engine across the full
  tempo range. MASTER TEMPO off is plain varispeed.
- **Stops and starts:** a stop fades over 3 ms, so a released CUE never
  clicks. A start has no fade, so CUE lands on the beat.
- **SLIP** keeps a shadow playhead running under loops, held hot cues and
  scratches, and jumps to it when they end.
- **Loading:** a load decodes in the background. Play can start as soon as
  4 s past the cue are ready, and playback waits at the decode frontier if it
  ever catches up. Every track is resampled to the output's rate.

Details: `AUDIO_ENGINE.md` and `docs/design/track-loading.md`.

### Analysis

- **Beat grid:** MiniBPM makes a first tempo guess. Then the tempo and phase
  are fitted together against an onset curve over the whole track, placing
  beats where rekordbox does. Tempo matches rekordbox's exactly on 222 of
  226 test tracks, and grids land a median 1.3 ms from rekordbox's.
- **Key:** a whitened, tuning-corrected chroma, matched against key profiles
  learned from rekordbox-analysed tracks. Keys are named as rekordbox names
  them.
- **Phrases:** intro, verse, chorus and outro, written as rekordbox's PSSI
  section.
- **Waveforms:** built for both screens and for other players in rekordbox's
  formats (blue, RGB, 3-band).

### Protocol compatibility

**Pro DJ Link** is Pioneer's CDJ network protocol, documented by the
[dysentery](https://github.com/Deep-Symmetry/dysentery) project. OpenDeck
implements:
- the announce, beat, status and position packets;
- the SYNC / master handshake;
- the dbserver protocol, both directions;
- the NFS media transport.

So it syncs tempo and phase with real players, hands master back and forth
with them, and exchanges tracks with them.

---

## Philosophy

Pioneer makes good hardware. They also use their market position to maintain a closed ecosystem that extracts money from DJs and venues at every step: expensive hardware, rekordbox subscription, proprietary USB formats, licensing fees for third-party integration.

The open source DJ tooling community (Mixxx, xwax, beat-link, dysentery) has done remarkable work. This project's goal is to turn that knowledge into a complete, deployable hardware product that any DJ can build, any venue can install, and any developer can extend.

The name is deliberately provocative. The CDJ-3000 is not a trademark we are imitating; it is a benchmark we are matching.

---

## Contributing

The codebase is Rust throughout. Contributions are welcome in any area. The
most useful near-term work:

- **A physical CDJ-3000 on the network.** Run the
  [interop kit](docs/testing/cdj-3000-interop.md) and report back.
- **rekordbox data, both ways.** Hot cue colours and comments, phrases and
  colour waveforms from sticks, passed through to CDJs.
- **CDJ-3000 parity.** The open items in
  [docs/TODO-cdj-3000.md](docs/TODO-cdj-3000.md): sorting, TRACK FILTER and
  KEY SHIFT.
- **An XDJ loading from OpenDeck.** The servers are in; the acceptance test on
  a real XDJ is not (see `docs/reference/prodj-link-media.md`).
- **Hardware.** The panel board is generated and routed
  ([hardware/](hardware/README.md)). It needs its control positions checked
  and a first fab. RP2350 firmware for it is no_std Rust.

**Docs in the same commit.** A change that adds, removes or changes a feature
updates [CHANGELOG.md](CHANGELOG.md), [docs/FEATURES.md](docs/FEATURES.md) and
any sentence in this README it makes false.

---

## License

GPL-2.0-or-later. If you build a product with this, the product must be open source. That is intentional.

Protocol implementations (ProDJ Link, timecode formats) are based on publicly documented reverse-engineered specifications and are not subject to any Pioneer trademark or patent claims we are aware of. If you believe otherwise, open an issue.
