# FreeDJ-3000

A clean-room implementation of the Pioneer XDJ-1000MK2, in Rust. It is a
single-deck DJ media player with the XDJ's screen, controls and behaviour, and
it speaks Pro DJ Link on the wire, so it joins a network of CDJ-3000 and XDJ
players as one more deck: it takes a player number, follows the master tempo
or takes master and the real decks follow it, browses the other players'
media and serves its own.

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
photographs of a real XDJ-1000MK2, the controls are rendered procedurally, and
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
protocol and the media sharing work; the physical control surface and the
sampler features are the roadmap.

| Feature | Status |
|---|---|
| MP3/FLAC/AAC/OGG/WAV/AIFF decode | ✅ Working |
| XDJ-1000MK2 screen: waveforms, beat grid, INFO, BROWSE, PERFORM, MENU/UTILITY | ✅ Working |
| Play / CUE (hold-to-preview, back-cue) / hot cues A–H / loops / beat jump | ✅ Working |
| Jog wheel: VINYL and CDJ modes, nudge, scratch | ✅ Working |
| SLIP mode | 📋 Planned ([#28](https://github.com/audiodestrukt/freedj-3000/issues/28)) |
| Tempo fader ±16 %, MASTER TEMPO key lock (Rubber Band R3) | ✅ Working |
| Beat detection + grid (MiniBPM + onset detector), GRID ADJUST | ✅ Working |
| rekordbox analysis import (beat grid, cues) from USB export / LINK | ✅ Working |
| Pro DJ Link: announce, beats, status, SYNC, master handoff both ways | ✅ Working on a real XDJ-1000MK2 |
| Pro DJ Link media: browse and load from linked players / rekordbox | ✅ Working (dbserver + NFS) |
| Pro DJ Link media: serve tracks to other decks (metadata, grid, waveforms, art) | ✅ OpenDeck→OpenDeck proven; XDJ acceptance test pending |
| iPad / iPhone app (OpenDeck DJ) | ✅ On the App Store |
| Raspberry Pi 5 | ✅ 60 fps, ~0.6 of one core playing |
| Control surfaces: any USB MIDI controller, PCF8574 I2C boards on the Pi, mapped in `controllers.toml` with LED feedback | ✅ MIDI on a DJ2Go; I2C path built and unit-tested, first board pending ([docs/hardware/control-surfaces.md](docs/hardware/control-surfaces.md)) |
| Sampler-deck: auto one-shots, quantized triggers, cue sequencing | 📋 Roadmap ([#24](https://github.com/audiodestrukt/freedj-3000/issues/24)) |
| Hardware control surface (RP2350) | 📋 Roadmap |
| CDJ-3000-generation media transfer | 📋 Not started (the 3000 moves files differently) |
| Running OpenDeck *on* a CDJ-3000 (RK3399, as a payload through the deck's updater) | 📋 Designed, not built ([docs/hardware/cdj-3000-install.md](docs/hardware/cdj-3000-install.md)) |

Everything in the Link rows has been exercised against an XDJ-1000MK2.
CDJ-3000s speak the same Pro DJ Link sync protocol; one has not been on this
network yet, so treat that as compatible-by-protocol rather than tested.
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

### Windows

Prebuilt: the [Releases page](https://github.com/audiodestrukt/freedj-3000/releases)
has `OpenDeck-<version>-windows-x64.zip` (unzip, run `opendeck.exe track.mp3`;
details in the bundled `README-WINDOWS.txt`). It is built by
`.github/workflows/windows.yml` on every `win-v*` tag:

```bash
git tag win-v0.3.0 && git push origin win-v0.3.0
```

Building locally needs only rustup and the Visual Studio Build Tools
("Desktop development with C++" — MSVC compiles the vendored Rubber Band and
MiniBPM); no other system libraries. Then `cargo build --release -p opendeck-app`.

### Controls (MVP)

| Key | Action |
|---|---|
| `Space` | Play / pause |
| `Enter` | CUE — set / return-to-cue / play-from-cue (CDJ) |
| `,` / `.` | Jog: nudge (playing) / vinyl-scrub (paused) — the jog wheel does this too |
| `←` / `→` | Seek ±10 seconds |
| `+` / `−` / `0` | Tempo ±1% / reset (±16% range, as the TEMPO readout) |
| `Z` / `X` | Zoom the enlarged waveform in / out |
| `K` `S` `Y` `M` | Master tempo · SLIP · SYNC · MASTER (indicators; SYNC/MASTER wait on Link send) |
| `C` | Cycle waveform colour: RGB → 3 BAND → BLUE (the *Waveform Color* setting) |
| `T` | Toggle time display: TIME (elapsed) ↔ REMAIN |
| `P` | Phase meter: 4-box beat display ↔ master alignment ticks (also tap it) |
| `B` | Open / close the file **browser** (also tap BROWSE) |
| in browser: `↑`/`↓` · `Enter` · `Backspace` | Move selection · open folder / **load track** · up a level |
| `Q` / `Esc` | Quit (`Esc` closes the browser first) |

**USB sticks.** Mounted removable media appear as `USB: <label>` rows at the
top of the browser (re-scanned once a second, so hot-plug works). A rekordbox
stick is detected by its `PIONEER/rekordbox/export.pdb` and browses as
PLAYLIST / TRACK with rekordbox's grids and cues; anything else browses as
folders. Pulling the stick is safe: the loaded track is already in memory.
On a headless Pi something still has to mount the stick (udiskie), see
`docs/reference/raspberry-pi.md`.

### Touch screen (mouse for now)

egui treats mouse and touch as the same pointer, so the mouse stands in for
the panel. Press or drag on the overview to needle-search; scroll the wheel
over the enlarged waveform to zoom; tap ZOOM / GRID, SLIP, SYNC, MASTER, the
MT pill, the time readout (TIME ↔ REMAIN) and the LINK / FILE source keys.
Keys with no function yet (BROWSE, CUE/LOOP, CALL…) press visually and log.
Every input — keys and touch alike — is an `Event` on one bus applied in
`DeckApp::apply`; see `docs/INPUT_PLAN.md`.

### Controller input (development only)

Until the custom hardware and RP2350 firmware exist, a **Numark DJ2Go** (or any class-compliant USB MIDI controller) can be used as a temporary control surface. It is detected automatically over USB MIDI if plugged in before launch.

| Control | Action |
|---|---|
| Jog wheel | Nudge playback speed (snaps back on release) |
| Pitch fader | Set playback speed (±16%) |
| Pitch +/− buttons | Step speed ±1% |
| Play button | Play / pause |
| Cue button | Return to zero |

Deck B controls drive the second beat grid (for ProDJ Link sync comparison):

| Control | Action |
|---|---|
| Jog wheel (ch 1) | Nudge B2 BPM |
| Pitch fader (ch 1) | Set B2 BPM |
| Cue button (ch 1) | Reset B2 beat phase |

The full DJ2Go mapping is `docs/reference/dj2go-midi-map.md` (from Mixxx, hardware-confirmed): Play, Cue, Sync (drives real Link SYNC), Loop In/Out, pitch fader, jog, plus the shared browse knob and Load. `--deck A` (default) or `--deck B` selects which side of the controller drives this instance — two freedj instances split one DJ2Go. To map a different controller, run with `RUST_LOG=opendeck::midi=debug`; every message is logged.

### ProDJ Link

FreeDJ-3000 is a Link peer: it announces itself on UDP 50000 every 1.5 s and
sends a beat packet on 50001 at every beat of the audible position, as player
`--player N` (default 1; `OPENDECK_PLAYER` also works). It listens on 50001
(where real CDJ/XDJ hardware sends beats) and 50002, ignoring its own
broadcasts. Status packets go out on 50002; SYNC follows the master and the
master handoff works both ways with a real XDJ.

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

## Architecture

The project is a Cargo workspace. Each crate has a single responsibility.

```
crates/
  types/        — Core traits: Decoder, BeatAnalyzer, TimecodeDecoder
  decode/       — Audio decoding via Symphonia (MP3, FLAC, AAC, OGG, WAV, AIFF)
  analysis/     — Waveform FFT, beat grid computation
  minibpm-sys/  — Bindings to MiniBPM for tempo estimation
  engine/       — Real-time audio transport
  timestretch/  — Key-lock (Rubber Band R3) and speed change
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
  app/          — Main binary: deck, screen, faceplate, wgpu renderer, winit event loop, iOS entry
```

### Graphics stack

- **wgpu** — GPU abstraction: Vulkan on Linux and the Pi, Metal on iOS
- **winit** — Window and input handling, DRM/KMS capable (no display server required)
- **egui** — Immediate-mode UI overlay (transport info, time display)
- **Custom WGSL shader** — Waveform renderer: frequency-colored bar chart, scrolls in real time

The UI philosophy is a **game HUD**, not an application. While playing, every frame redraws the full screen, paced by the compositor's frame callback so the playhead advances by whole display periods and never judders; paused and untouched, it idles at 10 fps. The drawn playhead free-runs at the true playback rate and is phase-locked to the audio clock. Audio runs on a separate real-time thread.

### Audio engine

The audio engine runs on an isolated real-time thread and communicates with the UI via lock-free atomics. The callback-based cpal stream advances an `AtomicU64` position counter that the renderer reads each frame.

Speed changes use the Rubber Band R3 time-stretcher (`crates/timestretch/`) for key-locked pitch-preserving playback across the full ±16% pitch range; MASTER TEMPO off switches to plain varispeed. Stops fade over 3 ms so a released CUE never clicks; starts have no fade, so CUE lands on the beat. Planned: slip mode.

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

- **Beat grid editor** — UI for manually correcting auto-detected beat grids
- **RP2350 firmware** — no_std Rust for the control surface MCU
- **XDJ media-source acceptance** — an XDJ browsing and loading from OpenDeck over LINK (dbserver + NFS servers are in; see `docs/reference/prodj-link-media.md`)
- **Hardware BOM and PCB** — the physical build hasn't started yet

See `AUDIO_ENGINE.md` for detailed design documentation on the audio engine,
and `docs/design/track-loading.md` for how a track load runs off the UI thread.

---

## License

GPL-2.0-or-later. If you build a product with this, the product must be open source. That is intentional.

Protocol implementations (ProDJ Link, timecode formats) are based on publicly documented reverse-engineered specifications and are not subject to any Pioneer trademark or patent claims we are aware of. If you believe otherwise, open an issue.
