# OpenDeck features

Everything the deck does today, by area. Keep it current: a change that adds,
removes or changes a feature updates this file in the same commit as its
CHANGELOG entry. The CHANGELOG says *when* something changed; this file says
*what there is now*.

✅ works · 🧪 experimental / not yet tested on real hardware · 📋 planned

Platforms: **D** desktop (Linux x86_64/aarch64, Windows, macOS) · **Pi**
Raspberry Pi (5, and Zero W via the SDL backend) · **iOS** iPhone and iPad
(App Store: OpenDeck DJ) · **And** Android (.apk). "All" = every one.

## Tracks and library

| Feature | Status | Where |
|---|---|---|
| Formats: MP3, AAC/M4A, ALAC, FLAC, WAV, AIFF, OGG Vorbis; mono and multichannel play as stereo | ✅ | All |
| Instant load: playable once 4 s past the cue are decoded, the rest streams in behind playback (8.8-min MP3: 19 ms on a desktop) | ✅ desktop measured; iOS / emulator not yet timed | All |
| Any sample rate (44.1, 48, 96, 192 kHz…), converted to the output's rate on load; the same grid at every rate | ✅ | All |
| Bit depth: WAV / AIFF 16, 24, 32-bit integer and 32 / 64-bit float; FLAC 16, 24 and 32-bit (FLAC 1.4+, bit-exact against `flac`); ALAC 16 and 24-bit (32-bit untested) | ✅ | All |
| BROWSE a music folder; LOAD; LOAD PREVIOUS TRACK; TAG LIST; track INFO; cover art | ✅ | All |
| BROWSE remembers its place: per source (home folder, each stick), across launches, BACK lands on the folder it left; a new stick does not take over the USB key | ✅ | All |
| SEARCH with the on-screen keyboard (CDJ-3000 skin; `/` on the desktop) | ✅ | All |
| rekordbox USB sticks: export.pdb categories (ARTIST, ALBUM, TRACK, KEY, PLAYLIST, HISTORY, FOLDER), beat grids, memory and hot cues, preview waveforms, covers | ✅ | All |
| USB drives plugged into the device (iOS: "USB: ADD DRIVE…" picker, remembered; Android: "All files access") | ✅ | iOS, And, D, Pi |
| Files-app music folder with a bundled demo track | ✅ | iOS (Android: Music folder) |
| Stems prepared with OverCue Desktop: mute / solo / level per stem (MENU → STEMS, off by default) | ✅ | All |
| Sorting, TRACK FILTER, KEY SHIFT (CDJ-3000 screen) | 📋 | — |
| OneLibrary sticks (rekordbox's new encrypted USB library, read by the CDJ-3000X); today OpenDeck reads the Device Library (`export.pdb`) that rekordbox 7.2.11+ writes alongside it | 📋 | — |

## Analysis

| Feature | Status | Where |
|---|---|---|
| BPM and beat grid for tracks without rekordbox data: tempo and phase fitted together, rekordbox's beat timing; 222/226 tempos exact on a test stick, grids 1.3 ms median from rekordbox's | ✅ | All |
| rekordbox's own grid and cues used when present (stick or LINK) | ✅ | All |
| GRID ADJUST: SNAP GRID, SHIFT GRID (folds a synced jog nudge into the grid), reset | ✅ | All |
| Waveforms: detail (zoomable, beat and bar markers) and full-track overview with needle search | ✅ | All |
| Waveforms for other players: blue, RGB (PWV4/PWV5) and 3-band (PWV6/PWV7), fitted to rekordbox's | ✅ | All |
| Analysis cache (a track is analysed once) | ✅ | All |
| Key detection for untagged tracks (72 % exact vs rekordbox on a house stick) | ✅ | All |
| Phrase analysis served as rekordbox PSSI (phrase starts P 0.70 / R 0.73 vs rekordbox on 31 house tracks; mood needs more genres) | 🧪 | All |
| Downbeat (bar 1) detection; drum & bass at full tempo; 4:5 / 2:3 tempo mistakes | 📋 | — |

## Playing

| Feature | Status | Where |
|---|---|---|
| PLAY / CUE with hold-to-preview and back-cue, AUTO CUE (with level setting) | ✅ | All |
| Hot cues (A–D on the XDJ screen, A–H on the CDJ-3000 faceplate), memory cues | ✅ | All |
| Loops: manual IN / OUT, beat loops 1/2–16, BEAT JUMP | ✅ | All |
| QUANTIZE for cues and loops | ✅ | All |
| SLIP mode | ✅ | All |
| Jog wheel: VINYL (scratch) and CDJ (nudge) modes; the platter turns under the finger, one turn = one record turn | ✅ | All |
| Scrub audio: a paused jog (or a VINYL scratch) plays the audio under the playhead | ✅ | All |
| Drag the big waveform to seek | ✅ | All |
| Tempo fader ±6 / ±10 / ±16 / WIDE; MASTER TEMPO key lock | ✅ | All |
| Key-lock engines: Rubber Band R3 (default), Bungee, WSOLA (low-power boards); a tone holds its pitch within 0.2 cents from −16 % to +16 % in all three | ✅ | All (WSOLA for Pi Zero) |
| Vinyl brake, key lock off (varispeed), real-time sample-rate conversion | 📋 | — |

## Screens and controls

| Feature | Status | Where |
|---|---|---|
| XDJ-1000MK2 screen: waveforms, grid, INFO, BROWSE, PERFORM pads, MENU / UTILITY | ✅ | All |
| Waveform Color BLUE / RGB / 3BAND (MENU, saved; 3BAND default, as the CDJ-3000) | ✅ | All |
| CDJ-3000 screen and faceplate (the default screen; MENU → SCREEN switches; the full deck by default on the Mac, Windows and iPad, `F` / MENU → DECK VIEW toggles it on the desktop): playback, BEAT LOOP / BEAT JUMP panels, SOURCE, BROWSE, TAG LIST, UTILITY, SEARCH, phase meter and master badge, STEMS panel | ✅ | D, Pi, iPad, Android tablet |
| iPhone / Android phone: two landscape pages (SCREEN, CONTROLS) | ✅ | iOS, And |
| Keyboard control | ✅ | D |
| USB MIDI controllers mapped in `controllers.toml` with LED feedback (DJ2Go preset) | ✅ | D, Pi (not yet iOS / Android) |
| PCF8574 I2C button boards | 🧪 built and unit-tested, no board yet | Pi |
| A CDJ-3000 as OpenDeck's control surface and screen (HID over LINK, Control Mode) | 🧪 emulator only | D |
| The CDJ-3000's own panel over its sub-CPU SPI device (`OPENDECK_SUBUCOM`): keys, jog, fader, rotary, LCD touch, LEDs | 🧪 unit-tested, not yet run in the emulator | Linux |
| OpenDeck as cdj3k-emu firmware: emulator UPD, bare 1280×720 CDJ-3000 screen, framebuffer output (`OPENDECK_FBDEV`) | 🧪 boots in QEMU with the emulator's kernel; wizard install not yet tried | cdj3k-emu |
| ADK-1000 hardware panel (RP2350 hub) | 📋 PCB routed, not made | — |

## Pro DJ Link

| Feature | Status | Where |
|---|---|---|
| Appears as a player: announce, beats, status (the CDJ-3000's long status with the active loop, the shorter one to older players) | ✅ tested on an XDJ-1000MK2 | All |
| PLAYER No. AUTO: claims a free player number like a CDJ (tries 4, 5, 6, 3, 2, 1), defends it, moves on a collision; or set 1–6 (5–6 need a 6-player network) | 🧪 tested between OpenDecks, not yet real players | All |
| SYNC to the master's tempo and phase; a jog nudge while synced is kept as an offset | ✅ | All |
| MASTER, handoff both ways | ✅ tested on an XDJ-1000MK2 | All |
| LINK: browse and load from other players' USB / SD and from rekordbox (dbserver + NFS), with their grids and cues | ✅ | All |
| Players browsing OpenDeck: ARTIST, ALBUM, TRACK, KEY, PLAYLIST, HISTORY, MATCHING, FILE NAME | ✅ CDJ-3000 (emulator) | All |
| Serve our own music to other decks (metadata, grid, waveforms, art, hot and memory cues) | ✅ OpenDeck→OpenDeck; a CDJ-3000 lists, browses (ARTIST, ALBUM, TRACK, PLAYLIST, FILE NAME), loads and plays it with its waveforms (emulator); XDJ untested | All |
| The playing track's metadata, grid, waveforms and phrases answer a lookup by the ID our status names, from the moment it loads, wherever the file came from (for other players and Pro DJ Link lighting controllers) | ✅ unit-tested; lighting hardware untested | All |
| CDJ-3000 Absolute Position packets (playhead ms, length, pitch, BPM every 30 ms on 50001, to each device as a CDJ-3000 sends them) for lighting / video / timecode tools | ✅ measured on a test network; untested with the tester's tool | All |
| Works over Wi-Fi or ethernet; survives iOS backgrounding | ✅ | All |
| DJM mixer channel assignment of player numbers | 📋 | — |
| Read a CDJ-3000's USB over LINK | ✅ emulator: 226 tracks, audio, analysis | All |

## Platforms and releases

| Feature | Status |
|---|---|
| Linux x86_64 / aarch64, Windows x64, macOS universal app, Android APK, iOS IPA: one `v*` tag publishes all on one GitHub release | ✅ |
| App Store: OpenDeck DJ (iPhone + iPad) | ✅ |
| Raspberry Pi 5 (60 fps, ~0.6 of a core playing) | ✅ |
| Raspberry Pi Zero W (SDL renderer on KMSDRM, WSOLA, 49 fps) | ✅ |
| Running on a CDJ-3000 itself | 📋 designed, not built |

## Planned

- Sampler deck: auto one-shots, quantized triggers, cue sequencing ([#24](https://github.com/audiodestrukt/freedj-3000/issues/24)).
- The remaining CDJ-3000 screen work: [TODO-cdj-3000.md](TODO-cdj-3000.md).
- Analysis improvements (phrases, mood, downbeats, key): [TODO-analysis.md](TODO-analysis.md).
