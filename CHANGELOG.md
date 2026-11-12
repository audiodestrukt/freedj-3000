# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- **The CDJ-3000's panel as a control surface.** With
  `OPENDECK_SUBUCOM=/dev/subucom_spi1.0`, OpenDeck reads the CDJ-3000's
  sub-CPU directly, as the deck's own player does: every key, the jog (turn
  and touch), the tempo fader and the browse rotary drive the deck, and the
  key and pad LEDs follow it. It is the same device on a real deck and in
  cdj3k-emu, so OpenDeck running as the emulator's guest gets the drawn
  panel. Keys behave as on the on-screen CDJ-3000 faceplate. Unit-tested on
  synthetic frames; not yet run on the device. LCD touch is next.
- **Key detection.** Tracks without a key tag get a detected key, named as
  rekordbox names it, shown in KEY and served to players. On a 226-track
  rekordbox stick it agrees exactly with rekordbox on 72 % (cross-validated);
  most misses are the right tonic in the other mode. About 60 ms a track.
- **Phrase analysis (first cut).** OpenDeck detects phrases (intro, verse /
  up / down, chorus, outro; mood high / mid / low) and serves them as
  rekordbox's PSSI section, so a CDJ-3000 can colour the waveform by phrase.
  The encoder matches rekordbox's (masked) format byte for byte; the
  detector itself is not yet measured against rekordbox's phrases
  (`examples/phrase_vs_rekordbox` does that, given a source with them).
- **Colour waveforms for other players.** OpenDeck's analysis now also
  makes the waveforms newer players draw: the CDJ-3000's 3-band preview and
  detail (PWV6 / PWV7) and the nexus 2 RGB preview and detail (PWV4 /
  PWV5), served over LINK for every track, whether rekordbox analysed it
  or not. They are fitted to rekordbox's own on 30 tracks
  (`examples/waveform_vs_rekordbox`): the 3-band detail follows rekordbox's
  at a correlation of 0.93 (lows), 0.83 (mids), 0.83 (highs). The analysis
  cache is rebuilt once.
- **PLAYER No. AUTO.** OpenDeck now picks its own Pro DJ Link player
  number, as a CDJ set to AUTO does: it listens for the players already on
  the network, takes a free number (the device's usual one when it is free:
  3 on an iPad, 4 on an iPhone) and claims it with the players' own claim
  sequence, moving on if a player says the number is taken. Once it has a
  number it defends it, and it moves if another device turns up with the
  same one. AUTO is the new default; a number you set yourself is kept.
  Players 5 and 6 can be chosen too (they need a 6-player network of
  CDJ-3000s); AUTO uses them only when 1–4 are taken. Tested with five
  OpenDecks on one network; not yet against real players. Mixer channel
  assignment (DJM) is not supported yet.
- **Apple Lossless (ALAC) tracks load.** They were refused.

### Fixed
- **A CDJ-3000 now lists OpenDeck's music in SOURCE** ("USB LINK2:
  OPENDECK"). Five things stopped it, found against an emulated CDJ-3000:
  our NFS export named no network (a player reads that as "not for you"),
  the root file handle was all zeros, we could not read a CDJ's media query
  (its layout differs from Beat Link's), our media answer put our IP where
  the owning player's number goes, and our status claimed an SD card. Our
  own media queries now use the player layout too, so OpenDeck learns what
  is in a CDJ-3000's slots.
- **A CDJ-3000 loads and plays OpenDeck's tracks**, with OpenDeck's beat
  grid, BPM, art and overview waveform. Two more fixes, found by recording
  one emulated CDJ-3000 loading from another: when a track has no data of a
  kind (cues, a waveform, an analysis section), OpenDeck now answers the way
  a player does, an empty reply with status 0x32, instead of "unavailable",
  which made the CDJ give up and reconnect; and NFS reads are served 32 KiB
  at a time as players ask (capped at 8 KiB, the CDJ stopped with "E-8302:
  CANNOT PLAY TRACK").
- **Players draw OpenDeck's scrolling waveform.** The detail waveform's
  20-byte header was sent as zeros, so a player read zero entries and drew
  nothing; it now carries the entry count and rate as a CDJ sends them.
- **A CDJ-3000 browses OpenDeck by the right categories** (ARTIST, ALBUM,
  TRACK, PLAYLIST, FILE NAME). Our category item types were one off from
  the documented ones (Deep Symmetry's Menu Item Types), so a CDJ showed
  BPM, ALBUM, TRACK, PLAYLIST. Labels are now marked as a player marks them,
  a player's sort-options request is answered, and request 0x1014 (KEY) is
  no longer taken for the track list. OpenDeck browsing a CDJ over LINK
  names its categories correctly too.
- **KEY, HISTORY and MATCHING on players browsing OpenDeck.** KEY lists
  the keys in Camelot order with the CDJ's related-key levels (same key,
  plus relative, plus a fifth either way); HISTORY lists what this deck
  played from its own library this session; MATCHING suggests tracks in a
  related key within 6 % of the chosen player's tempo. Request formats as
  captured from a CDJ-3000. Keys come from tags, or key detection for untagged tracks.
- **Other devices could not show OpenDeck's track title or position.** Our
  status packets carried a track ID left over from the captured packet they
  were built from, so Beat Link, ShowKontrol and the players asked our
  database for a track that does not exist (grids, sent in beat packets,
  were fine). Status now names the loaded track: one of ours by the ID our
  database serves it under, one loaded from a player or rekordbox over LINK
  by that player, slot and ID, as a CDJ reports it. With no track loaded,
  status says so.
- **rekordbox shows OpenDeck as a player.** Tested with rekordbox 7 (free
  plan) on a Mac: OpenDeck browses rekordbox's collection (its categories
  now read correctly), loads a track with rekordbox's grid and waveforms
  over LINK, and rekordbox's LINK panel then shows that track for OpenDeck's
  player, as it does for a CDJ-3000 on the same network. A track from
  OpenDeck's own library shows as "other device", as a CDJ's own stick does.
  A player's request 0x3007 after connecting is now answered as rekordbox
  answers it.
- **Big folders on a linked player could be cut short:** folder listings
  over LINK kept only the player's first reply (8 KB, roughly 100–250
  names) and never asked for the rest. They now read to the end.
- **Mono tracks would not load** ("track has 1 channels but the deck runs
  2"). They now play on both sides; tracks with more than two channels play
  their front left and right. Mono ALAC files also read at half length,
  because their header names no channel count; the layout now comes from the
  decoded audio.
- **A track whose header gave no sample rate** was taken for 44.1 kHz, so a
  48 kHz one would play 8 % slow. The rate of the decoded audio now wins.
  No track we could find did this; checked over 684 tracks and every
  supported format (`examples/probe_rate` checks a folder).
- **MP3s were 24 ms off from rekordbox and the players.** The decoder trimmed
  the MP3 encoder delay; rekordbox and Pioneer players keep it, so every
  rekordbox beat grid and cue on an MP3 sat ~24 ms late against our audio (a
  cue on a kick started after the attack), a synced deck next to a CDJ ran
  ~24 ms early, and grids we served over LINK were ~24 ms early on the
  players. MP3s now decode with the delay kept, like rekordbox. Measured over
  a 226-track rekordbox stick (`examples/analysis_vs_rekordbox`): the deck's
  own grids moved from a median 22.7 ms off rekordbox's to 0.7 ms, and
  OverCue stems now line up within 1 ms without correction. The Link
  analysis cache is rebuilt once. Grids hand-adjusted with GRID ADJUST on an
  MP3 before this sit ~24 ms early: SNAP GRID them again.
- **Beat grids on the off-beat.** The deck's own analysis (tracks without a
  rekordbox grid) put a fifth of house / tech-house grids half a beat out,
  on the open hats instead of the kick: the phase vote summed energy per
  frequency bin, and the highs have ~60× the bins of the bass. Each band is
  now normalised on its own and the mids (the kick's click) weigh most.
  Against rekordbox's grids on a 226-track stick: off-beat grids 50 → 27,
  grids within 10 ms of rekordbox's at 60 s in 164 → 186 of 222. The
  analysis has tests now (it had none), including the off-beat case.
- **BPM a hair off, grids drifting.** The deck's own analysis now fits tempo
  and phase together over the whole analysed span, to a thousandth of a BPM,
  and places beats where rekordbox does (6 ms before the attack). Against
  rekordbox on the same stick: tempo exact on 222 of 226 tracks (was 198;
  the rest read 0.1–0.3 low, so long tracks drifted), grid median 1.3 ms
  from rekordbox's (p90 6.3 ms), off-beat grids 27 → 12. Load time is
  unchanged. Still wrong: four tracks read at 4/5 or 2/3 tempo, and drum &
  bass at half tempo.

### Changed
- **CDJ-3000 skin: UTILITY shows the version** and support address at the
  bottom, as the XDJ skin's MENU always has.
- **A player on LINK that serves its export without a database service**
  opens on the rekordbox categories, like a USB stick (it went straight to
  PLAYLIST).
- **Releases:** only a `v*` tag publishes a GitHub release, so every release
  carries every platform; per-platform tags build without publishing.

## [0.3.8] — 2026-11-10

Includes 0.3.3–0.3.7, which went to TestFlight and Android test builds only.
### Fixed
- **CUE lost the transient it was set on** (Rubber Band, the default
  key-lock engine). After a seek, R3 swallowed the first ~30–50 ms of what
  followed, so a CUE on a kick played the kick's tail. The stretcher now
  gets Rubber Band's start pad and drops its start delay after every reset,
  so playback from a cue starts on the attack, on time. Measured through
  the deck: the first kick after CUE went from silent for 29 ms to present
  from the first millisecond.
- **Stems from OverCue Desktop were refused** ("unsupported sample_rate
  96000"): OverCue writes its default 96 kHz rate into the index, and only
  a missing rate was read as 96 kHz. Checked on a real prepared stick: all
  six tracks load (MP3 and M4A) and drums / vocal mute and solo as they
  should.
- **Tempo fader dead spots at max and min.** The handle overhangs the slot
  at either end and that overhang took no touches, and the fader was read
  absolutely, so grabbing the handle at an end did nothing until the finger
  reached the slot. The touch area now covers the handle's whole travel
  (trimmed at MASTER TEMPO), and a drag moves the fader by how far the
  finger moves from where it took hold.

### Changed
- **A rekordbox stick opens on its categories** (ARTIST, ALBUM, TRACK,
  KEY, PLAYLIST, HISTORY, FOLDER) from the USB row and the USB key too, not
  only from the CDJ-3000 SOURCE screen, as on the units. Folder lists no
  longer show a stick's system folders (PIONEER, CDJMODS, System Volume
  Information, $RECYCLE.BIN, LOST.DIR).
- **CDJ-3000 skin: MENU opens UTILITY on a tap** (the settings, STEMS
  included). LOAD PREVIOUS TRACK moved to the hold. The unit has it the
  other way round, but on a touchscreen the settings must be one tap away.
  The floating "— UTILITY" caption above the key is gone.

### Added
- **USB drives on iOS and Android.** A rekordbox stick (or any drive)
  plugged into an iPhone, iPad or Android device now browses like it does on
  the desktop: USB row, rekordbox categories, cues and grids, and served to
  other decks over Link.
  - **iOS:** BROWSE → "USB: ADD DRIVE…" (also on the CDJ-3000 SOURCE
    screen) opens the system folder picker; pick the drive once. It is
    remembered and comes back whenever it is plugged in.
  - **Android:** the drive shows as "USB: XXXX-XXXX ALLOW ACCESS"; opening it
    goes to the "All files access" setting (needed on Android 11+ to read
    export.pdb). Flip it on and come back.
- **Android.** An APK for phones and tablets (Android 8.0+), from
  `deploy/android/build.sh` and `.github/workflows/android.yml`. Same deck as
  iOS: two landscape pages on a phone, the portrait deck on a tablet; tracks
  from the shared Music folder; Pro DJ Link over Wi-Fi or ethernet. USB MIDI
  controllers are not supported there yet.
- **Releases for every platform.** One `v*` tag publishes Linux, Windows,
  macOS (new: a universal `OpenDeck.app`), Android and iOS (`.ipa` for
  side-loading) on one GitHub release.

- **Stems.** MENU → STEMS turns on playback of stems prepared with OverCue
  Desktop (its open USB format: drums, vocal, harmonics). It's off by
  default, so both skins look and behave like the stock units.
  - **Loading:** a track loaded from a prepared stick finds its stems in
    the background. Each load is checked against the stick's hashes and
    lined up with the deck's own decode. Stems that don't match are
    refused, and the original keeps playing.
  - **CDJ-3000 skin:** a STEMS key beside BEAT LOOP opens a panel with
    FULL MIX, TOGGLE / VOLUME, and the three stems. TOGGLE mutes a stem
    with a tap; VOLUME turns each stem into a fader you drag.
  - **XDJ skin:** PERFORM gets a STEMS pad mode. Tap a pad to mute or
    unmute a stem, drag up or down to set its level; the fourth pad is
    FULL MIX.
  - **Playback:** stems are mixed before the time-stretcher, so they cost
    almost no CPU. With every stem at full, the original track plays
    untouched.

- **CDJ-3000 phase meter.** The slot beside the Bars rows shows the
  master's player number in an orange badge and the phase meter: the
  master's beats on an orange line and this deck's on a blue one, red ticks
  on bar starts, a tap hides it (the unit's WAVEFORM / PHASE METER switch).
  Measured off the emulated unit; it draws the master's beats from the Link
  beat packets, where the emulator (following freedj) drew a bare line.

- **Scrub audio.** Turning the jog while paused (or scratching in VINYL
  mode) now plays the audio under the playhead, its pitch following the
  wheel as a record's does, and stops 150 ms after the wheel does; PLAY
  starts from where it left off. Read straight from the track (no
  stretcher), at most 8× either way, about 50 ms ahead of the device.

- **The jog turns.** The platter is drawn turning under the finger, and it
  follows a circular motion: the finger's angle about the centre moves it
  (clockwise = forward), wherever on the platter the finger is, as on a
  real platter. A sideways swipe across it still works, as a turn. The
  shadow under it stays put (the platter and its shadow are two layers
  now), and one turn of the platter moves the track 1.8 s, a record's turn
  at 33⅓ rpm, so the centre display's marker turns with the finger (it ran
  about ten times faster on the iPad).

## [0.3.2] — 2026-11-09

### Fixed
- **Loading a track from a linked player** broke BROWSE (only ALL TRACKS
  left) and could fail the download. The LOAD opened a second dbserver
  session from the same player number, and a CDJ drops the first one when
  that happens (checked on the emulated CDJ-3000:
  `examples/dbserver_two_sessions`). A LOAD now asks on BROWSE's own
  session, and BROWSE reconnects once if its session has gone. freedj's own
  server takes two sessions, which is why LINK loads between OpenDeck decks
  never showed it.

## [0.3.1] — 2026-11-09

### Fixed
- iPhone UTILITY: with the new SCREEN row the last setting ran into the
  version footer; the rows now fit above it.

## [0.3.0] — 2026-11-09

iOS release; carries everything since 0.2.9.

### Added
- **A CDJ-3000 screen beside the XDJ-1000MK2 one.** MENU → SCREEN switches
  the deck's screens: playback with the CDJ's waveform look, BEAT LOOP / BEAT
  JUMP panels, INFO, SOURCE, BROWSE with rekordbox categories and the track
  table, TAG LIST, UTILITY, SEARCH with the on-screen keyboard, LOAD PREVIOUS
  TRACK, and GRID ADJUST with SNAP / SHIFT GRID. Both screens drive the same
  deck. The phone keeps the XDJ layout for now ("CDJ-3000: iPad only").
- **The CDJ-3000 faceplate on the iPad** (and `--faceplate` on a computer):
  every key the unit has, where the unit has it, measured off cdj3k-emu's
  panel drawing. Hot cues under the screen (hold CALL / DELETE and press a
  pad to delete one), 4 / 8 BEAT LOOP that halve and double a running loop,
  BEAT JUMP, TRACK SEARCH, SEARCH that speeds up as you hold it, TIME MODE
  (hold: AUTO CUE), TEMPO RANGE and TEMPO RESET, and MENU (hold: UTILITY).
  Lamps follow the unit's: screen keys glow dimly and light on their screen,
  IN / OUT blink in a loop, RELOOP lights once a loop is stored. On a
  computer the screen keys and BACK sit either side of the screen.
- **Checked against the emulated unit.** `tools/cdj_parity.py` drives
  freedj and a cdj3k-emu slot through the same button presses
  (`tools/cdj_parity/features.txt`) and compares screens and lamps; all 31
  checks match (`docs/testing/cdj-3000-parity.md`). The dev script gained
  `press=`, `hold=`, `release=`, `rotary=`, `lamps=` and `shot=` for it.

### Changed
- CDJ-3000 screen details from that comparison: QUANTIZE and BEAT JUMP
  boxes, A.HOT CUE and AUTO CUE badges, no caption on elapsed time and a
  count-up ruler, tempo-range badge colours (±6 green, ±10 orange, ±16 white,
  WIDE red text with one decimal), hot-cue / memory / cue markers on both
  waveforms, the played part of the address bar in white, and the Bars
  countdown to the next memory point (red 00.0 on one).
- CDJ-3000 skin: SYNC and MASTER with no other player show "No LINKED
  Players found." and stay off, as the unit does; the SEARCH key closes
  SEARCH and BACK leaves it open; TRACK SEARCH |◀◀ stops at the track's
  start cue and wraps round the folder.
- The CDJ session handshake (HID over LINK) is not compiled into the iOS
  build.

### Added (desktop, Pi and tools)
- Linux release builds: `.github/workflows/linux.yml` publishes `OpenDeck-<version>-linux-{x86_64,aarch64}.tar.gz` on a `linux-v*` tag (vendored Rubber Band + Bungee, SDL backend included, glibc 2.36); `deploy/linux/package.sh` builds the tarball locally.
- SDL backend on KMSDRM (no compositor): `OPENDECK_FULLSCREEN=1` picks the closest display mode and letterboxes 1:1. Pi Zero W: 49 fps playing with WSOLA, no underruns.
- SDL backend: the enlarged waveform scrolls in whole pixels through two GPU render targets (shift by copy, upload only the new columns); `OPENDECK_WAVE_SCROLL=smooth` keeps the sub-pixel scroll. Pi Zero W: 17.9 fps playing / 26 fps idle.
- `OPENDECK_STRETCH=wsola`: a time-domain WSOLA key-lock engine (pure Rust, `crates/timestretch/src/wsola.rs`), about a sixth of Bungee's cost, for one-core boards; Master Tempo off becomes a Hermite varispeed read.
- SDL2 render backend (`--features renderer-sdl`, `OPENDECK_RENDERER=sdl`): egui over `SDL_RenderGeometry`, waveforms rasterised on the CPU; for boards wgpu cannot drive (VideoCore IV, KMSDRM). Parity with wgpu checked by `tools/render-parity.sh`. Measured on a Pi Zero W: 5 fps playing, 15 fps idle (#47).
- Audio: the ALSA stream is rebuilt when cpal reports it dead (an xrun on a loaded board left the deck silent); audio threads ask for `SCHED_FIFO` (`deploy/pi/rt-audio`); `OPENDECK_RING_MS` lengthens the decode-ahead ring.
- Pi cross-builds from the x86 box: `deploy/pi/cross/build.sh aarch64|armv6` (the ARMv6 one links against a sysroot copied off the Pi).
- **Bungee as a second key-lock engine.** `OPENDECK_STRETCH=bungee` runs
  master tempo through Bungee (vendored, MPL-2.0) instead of Rubber Band R3:
  an adaptive phase vocoder at about a sixth of R3's CPU cost, with native
  reverse and zero-speed support for the coming varispeed engine. Rubber
  Band stays the default. `cargo run --example stretch_ab` renders a track
  through both for a blind listening test; `make perf` prints both engines'
  real-time factors.
- **Control surfaces from a file.** `controllers.toml` describes every
  controller attached to the deck — USB MIDI controllers and PCF8574 I2C
  button boards on a Raspberry Pi, any number of each — with buttons,
  encoders, faders and LEDs bound to deck actions, a SHIFT layer, per-deck
  tables, and LED feedback (PLAY/CUE lamps that behave as the XDJ's do).
  The DJ2Go adapter became the first built-in preset in that format. Guide
  with wiring and the field reference: `docs/hardware/control-surfaces.md`;
  examples in `docs/hardware/surfaces/`. `--controllers PATH` /
  `OPENDECK_CONTROLLERS` point at a file elsewhere.
- **CDJ-3000 tester kit** (`docs/testing/cdj-3000-interop.md`): a
  dependency-free `tools/link_capture.py` that records every Pro DJ Link
  packet on the wire with its bytes, a `replay_capture` example in the link
  crate that runs a capture through the parser offline and fails on a
  rejected packet, an issue template for reports, and the PERF LOG file on
  iOS now records Link packets too (first of each kind per player as hex,
  then every status change) so an App Store install can send one back.
- The renderer honours `WGPU_BACKEND`, and runs on the GL backend (Mesa
  llvmpipe, and a Mali-class GLES 3.1 driver) by asking the surface for
  `COPY_SRC` only where it exists. Assessment of running on a stock CDJ-3000
  in `docs/reference/cdj-3000-target.md`.

### Fixed
- **No click when playback stops.** Releasing CUE, pausing, or a platter or
  waveform hold used to cut to silence on the next sample, a step you hear
  as a click. The output now fades over 3 ms on stop, on the audio already
  buffered, with a soft landing. Starting is untouched: no fade-in, so cue
  latency is exactly as before.

## [0.2.9] — 2026-09-22

Submitted for App Store review 2026-09-23 (build 1790138933, manual release); carries 0.2.4–0.2.8.

### Fixed
- **Turning the BROWSE knob down on the iPhone no longer opens Control
  Center.** The knob sits at the top of the side column, so a downward turn
  that began near the edge was taken by iOS. The app now defers the top and
  bottom system gestures, so Control Center and the home gesture need a
  second swipe, as they do in games; this also helps the page-flip pill at
  the bottom (#45).

### Added
- **Touch-to-jog on the enlarged waveform.** Drag the big waveform sideways
  to move through the track: the transport is held while the finger is
  down, every point of movement seeks by the audio it spans at the current
  zoom, and lifting resumes, like a VINYL platter drag but pixel-exact. The
  grab only starts once the finger has clearly moved sideways, so a tap or
  the first finger of the phone's page swipe never holds the transport, and
  a second finger releases it. An OpenDeck extension: the XDJ's enlarged
  waveform is not draggable. No cost when not dragging.

## [0.2.8] — 2026-09-22

TestFlight build.

### Added
- **LINK and PLAYERS rows on INFO.** LINK shows this deck's player number
  and the address, interface and broadcast it speaks Link from; PLAYERS
  lists every player heard with its name and address and marks the master.
  A deck on the wrong interface or an isolated network now shows it on the
  device itself.

### Fixed
- **A deck that sat suspended no longer comes back hot and deaf.** iOS
  reclaims an app's sockets while it is suspended; afterwards every receive
  failed at once and the three Link listeners retried without waiting, a
  core each (the iPad "running hot again", and Link dead until a restart).
  Listeners now back off and bind afresh, the sender reopens its socket
  after three failed announces, and it re-checks its interface on every
  announce, so an app carried from one Wi-Fi to another without a restart
  moves to the new network's broadcast address on its own.
- **The other deck's phase row runs smoothly over Wi-Fi.** An access point
  holds broadcast packets for a power-saving client until its beacon
  interval, so beats arrive late and bunched; 0.2.7's "hold after one beat"
  rule made the row stop and restart on every late packet. The row is now a
  phase-locked free-run, like our own playhead: it runs at the peer's tempo
  and each packet pulls it a quarter of the way to the beat it marks. It
  holds only when the peer's status says it is paused. Beats also go by
  unicast to every OpenDeck peer, which is not held that way, and the
  duplicate is dropped on receipt; CDJs still see broadcast only.
- **MASTER can be taken again after both decks went quiet.** A deck
  remembered its last master forever, so after a long idle it kept asking a
  deck that was no longer answering for a handoff, and MASTER lit on
  neither. A master that has sent no status for five seconds is forgotten,
  and the next MASTER press takes the role. A deck in SYNC whose master
  vanishes promotes itself, as the protocol analysis says a CDJ does (to be
  confirmed against the XDJ-1000MK2).
- **Hold CUE, hold PLAY, release CUE, release PLAY: playback stays locked.**
  The second finger's PLAY latched on its press, but whether it should act on
  the press or on its lift was re-evaluated when it lifted, by which time CUE
  had already been released, so the lift counted as a second tap and paused
  the deck. The choice is now made when the finger lands and that finger is
  then spent.

## [0.2.7] — 2026-09-22

TestFlight build.

### Changed
- **iOS boots paused.** The app no longer starts playing the first track in
  Documents at launch; the deck sits cued at the track's first sound, as a
  CDJ does after a load, and PLAY starts it. The desktop keeps autoplay for
  the dev loop (`OPENDECK_AUTOPLAY=0` boots it paused too).

### Fixed
- **A tap that lands and lifts inside one frame no longer vanishes.** egui
  never counts a same-frame press and release as a drag, so a drag-only
  momentary control (CUE, the pads) would drop such a tap. They now read the
  raw pointer events and treat it as a press followed by a release.

## [0.2.7] — 2026-09-22

TestFlight build.

### Fixed
- **No more phantom deck on the phase meter.** With nobody on the network
  the top row still ran, because the "other deck" tempo was seeded with our
  own BPM at launch and its phase was a free-running clock anchored to
  nothing, so it drifted against our beat. The row now appears only once a
  peer has sent a beat, its phase extrapolates from that deck's last beat
  packet for at most one beat and then holds (a paused deck sends no beats),
  and the row is dropped after three seconds without a beat or a status
  packet from that deck (it left, slept, or was unplugged).
- **PLAY acts on the press, not the lift.** The button sensed clicks, which
  egui reports on release, so playback started when the finger came off. It
  now senses the downstroke, on both the phone pages and the iPad faceplate.
  This is also why "hold CUE, hit PLAY" sometimes restarted from the cue: the
  late PLAY could land after the CUE release had already returned and paused,
  so the lock-in became a fresh start. With PLAY on the press the lock-in
  lands while CUE is still held, and playback simply continues. A second
  finger's PLAY acts on its press while CUE or the platter is held, and on a
  still tap's lift otherwise, so a page swipe's second finger landing on PLAY
  still cannot toggle the transport.

## [0.2.6] — 2026-09-22

TestFlight build.

### Fixed
- **Pause, then CUE, sets the cue where you paused.** The play/pause toggle
  never cleared "sitting on the cue", so after PLAY → PLAY (pause) a CUE press
  previewed the old point instead of setting one at the paused position, the
  standard CDJ way to place a cue. The CUE/PLAY rules now live in
  `crates/transport` as a pure state machine, verified against behaviour specs
  in `specs/xdj-1000mk2/` (`cargo test -p opendeck-transport`); see
  `docs/design/transport-rules.md`.
- **A released CUE no longer sticks on screen where the preview stopped.**
  The audio thread only honoured seeks while playing, so the return-to-cue
  seek sat pending until the next press, and its own progress store could
  overwrite the screen's position hint; after a hold longer than about half a
  second the playhead then snapped forward to where the preview stopped. Seeks
  are now applied while paused too (the thread is woken for them) and the
  progress store yields to a pending seek. Audio was always right; the display
  was not.

## [0.2.5] — 2026-09-20

TestFlight build.

### Fixed
- **Hold CUE, hit PLAY: playback locks in.** The transport already latched
  on PLAY during a CUE preview, but on the phone and iPad the second finger
  never reached PLAY: egui-winit turns only the first finger into the
  pointer, and every later finger arrives as a bare touch event that no
  widget sees. PLAY and CUE now also answer a second finger (tracked the way
  egui-winit tracks the first), so with one finger holding CUE, or resting
  on the platter, the other can hit PLAY or CUE. PLAY takes the second
  finger's tap on lift, so the second finger of a page swipe landing on it
  does not toggle the deck.

## [0.2.4] — 2026-09-20

TestFlight build.

### Fixed
- **A held CUE now plays while held and snaps back on release, from the first
  millisecond.** CUE, the platter and the performance pads sensed
  click-and-drag, and egui only calls a still press a "drag" after 0.8 s; a
  shorter still tap became a click on lift. So a quick CUE tap on the phone
  started the preview as the finger came off and never stopped it, and a
  longer hold started 0.8 s late. They now sense drag only, so press and
  release fire the moment the finger lands and lifts. Transport behaviour is
  the XDJ's: paused → hold plays from the cue, release returns and pauses;
  playing → press returns to the cue and pauses.

## [0.2.3] — 2026-09-20

Submitted for App Store review 2026-09-20 (build 1789864853, manual release), replacing the withdrawn 0.2.2 submission; carries 0.2.1 and 0.2.2.

### Changed
- **The perf log is opt-in.** The CPU meter's file (`Documents/opendeck-perf.log`)
  is now written only while MENU → PERF LOG is ON, and removed when it is
  OFF (the default), so a production install never carries a log in the
  user's Documents. The `cpu:` log line and the CPU row on INFO stay.

## [0.2.2] — 2026-09-19

TestFlight build 1789863307; its review submission was withdrawn in favour of 0.2.3. 0.2.0 was approved 2026-09-19.

### Fixed
- **Swiping back from CONTROLS could leave the transport stopped.** A
  two-finger swipe begins as one finger, and on the controls page that finger
  lands on the platter (VINYL mode holds the transport while it is down) or on
  CUE. Their releases come from the widgets' own drag-stop, and once the page
  had flipped the widgets were no longer drawn, so the release never came.
  A page flip now releases whatever a finger was holding. (#43)

## [0.2.1] — 2026-09-19

Point release after the first day of two-device testing: iPhone touch
targets, iPhone ↔ iPad browsing, and power.

### Fixed
- **iPhone touch targets were off by the safe-area insets.** winit reports
  the *safe area* as the window's `inner_size` on iOS while the view (and the
  Metal layer) covers the whole screen. The surface, the layout and egui's
  screen rect were all sized from it, so the frame was stretched over the
  display and every tap landed up to 120 pt from where it was drawn (CUE, PLAY,
  the MENU rows). The drawable and the layout now use the full view, and the
  safe-area insets come from the device (`inner_position` / `inner_size`)
  instead of a fixed table — the Dynamic Island side is right whichever way
  the phone is turned. The iPad's 3.5 % vertical stretch is gone too. (#43)
- **Two OpenDecks on one network could not browse each other.** Both iOS
  devices seeded PLAYER No. 3 and same-numbered players ignore each other's
  packets; an iPhone now seeds 4 (an iPad stays 3), and a 0.2.0 install still
  on the blanket seed is re-seeded once. Separately, the media server never
  started on iOS because the NFS portmapper needs UDP 111, a privileged port:
  it now falls back to 50111 like rekordbox, and the client tries 111 then
  50111 — so the desktop build serves too. (#44)

### Changed — power
- **Idle frame pacing on iOS.** A paused deck with no finger on it renders at
  10 fps (`OPENDECK_IDLE_FPS`, 0 = off) instead of the display rate; anything
  moving — audio, a load, CUE, a touch in the last second, an egui animation —
  brings the display rate straight back. egui-winit's "repaint" answer to
  `RedrawRequested` no longer requests the next frame, which had made the loop
  self-driving.
- **No idle wakeups.** The audio thread parks while paused (PLAY unparks it)
  instead of polling every 2 ms; the Link sender sleeps to its next announce /
  status / media-query deadline when not playing instead of ticking every
  millisecond. Measured on the desktop: both drop to 0 % paused.
- **The app measures its own CPU.** Every 10 s a `cpu:` line names the
  process' share of one core and the threads it went to (Linux: /proc;
  Apple: Mach `thread_info`). iOS also appends it to
  `Documents/opendeck-perf.log` for the Files app, and INFO shows the process
  figure — the numbers to read before and after each power change.

## [0.2.0] — 2026-09-19

First Universal build (iPhone + iPad): TestFlight build 1789780380.

### Added — 2026-09-17
- **OpenDeck is a Pro DJ Link media source.** The app now serves its music
  folder to the other decks the way a CDJ serves its USB stick: a dbserver
  (remote-database) service for browsing, metadata, file paths and beat grids,
  and an NFSv2 server for the audio, plus the media-query answer and the
  "USB loaded" status flags that make a player list us under LINK. Player
  browsing of *any* peer now goes through dbserver (with the old export.pdb
  fallback), so two OpenDecks can load from each other, and an XDJ should be
  able to load from an iPad. `OPENDECK_SERVE=0` turns it off;
  `opendeck-serve <dir>` is the stand-alone server for testing. (#44)
- **rekordbox as a LINK source.** A rekordbox laptop in LINK mode appears
  under LINK; its collection and playlists browse over dbserver and tracks load
  over its NFS export (portmapper on 50111), with the beat grid. (#30)
- `OPENDECK_LINK_UNICAST=ip,…` sends announces straight to peers across a
  routed link (Tailscale), and peers are addressed where their announce came
  from, not the address inside the packet.
- **Fast network loads.** NFS reads keep 32 requests in flight instead of one
  (NFSv2 caps a read at 8 KB, so a serial client paid one round trip per
  8 KB: about two minutes for a 12 MB track over a phone hotspot, now a few
  seconds). (#31)
- **Loads no longer stall the deck.** Every load, local or LINK, runs on a
  loader thread: fetch, decode, resample, waveform, beat grid and auto cue.
  The UI thread only swaps the prepared track in (about 5 ms including the
  waveform upload), so the deck keeps playing and responding through a load
  instead of freezing for a second (several on a Pi). (#19)
- `docs/reference/prodj-link-media.md`: how media browsing and loading works
  between decks (dbserver + NFS), rekordbox 7 findings, test recipes.

### Added — 2026-09-18
- **iPhone layout (#43).** On a phone the deck is two landscape pages
  flipped with a two-finger swipe: SCREEN (the XDJ LCD at full height with
  BROWSE, TAG TRACK, BACK, CUE and PLAY beside it) and CONTROLS (jog, tempo
  fader, transport, loops, MASTER TEMPO, JOG MODE, with the phase meter and
  the TEMPO / BPM readouts across the top). Swipe up for the controls, down
  for the screen. The iOS target now builds for iPhone and iPad; iPhones are
  landscape-only. Desktop preview: `OPENDECK_PHONE=1` (Tab flips pages;
  `OPENDECK_PHONE_PAGE=controls` picks the page for captures).
- **LOADING readout.** While a track is fetched and prepared on the loader
  thread, the title bar (playback and BROWSE screens) shows "LOADING name"
  with a sweeping bar; it clears the frame the track lands.
- **LINK list like a player's.** Peers are listed per media slot with the
  player number, slot and volume name ("3 USB: OPENDECK", "3 SD: …") from
  the Link media query the deck now sends, and entering a source opens its
  category menu (PLAYLIST / ARTIST / ALBUM / TRACK / FILENAME) instead of
  jumping to ALL TRACKS. SD slots load from the player's "/B/" export. (#32)
- **Served library with real metadata.** The media source now fills in tags,
  duration, tempo, beat grid, waveform preview + detail and cover art per
  track from a background analysis (one track at a time, cached in the app
  data dir), and answers artist / album / filename menus and the artwork and
  waveform requests. Other decks see file names at once and the full rows as
  each track is analysed. (#44)
- **One analysis per track.** Deck loads and the served library share the
  link cache: a load takes its grid from the server's analysis when there is
  one and writes its own analysis when there is not, so MiniBPM no longer
  runs twice per track (once for the deck, once for the library).
- Media responses also go to the address a query came from, so a peer across
  a routed link (Tailscale) sees our media name.
- Dev: `OPENDECK_SCREENSHOT_FRAME=n` picks the captured frame;
  `PROBE_DUMP=file` makes `dbserver_probe` write a reply's blob out.

## [0.1.13] — 2026-09-16

First public release of **OpenDeck DJ** on the App Store (iPad, build
1788938809): https://apps.apple.com/app/id6807472453. Approved 2026-09-16
after one metadata rejection (the subtitle used the word "iPad"; now
"Beat-synced single-deck player").

### Changed — 2026-09-09
- **Faceplate chrome is rendered, not photographed.** The jog wheel (dimpled
  grip rim, silver bezel, glossy platter, centre recess), CUE / PLAY, LOOP IN /
  OUT, RELOOP, MASTER TEMPO, the BROWSE knob and the tempo-fader knob are now
  shaded from height fields under one shared light rig (`chrome.rs`) and baked
  to textures per pixel size at first draw. One light for every control is
  what makes the panel read as a single object; lit states (PLAY green, CUE
  orange, loops, MASTER TEMPO) are baked variants rather than tints painted
  over a photo. The deck photo, its bundling step and the JPEG decoder are
  gone — the app ships no Pioneer imagery. `cargo run --example chrome_dump`
  writes the sprites as PNGs for tuning.

### Added — 2026-09-03
- **JOG MODE (VINYL / CDJ).** The faceplate's JOG MODE button is live and lit
  in vinyl mode; the hub badge only reads "Vinyl" then. While PLAYING, a drag
  in VINYL mode holds the transport and moves the playhead directly (the glass
  platter counts as always pressed, since there is no push sensor), resuming on
  release; in CDJ mode a drag nudges. Paused, the platter scrubs in either
  mode. Remembered across launches; keyboard `V`. No scrub audio or brake ramp
  yet (needs the real-time resampler). Closes the dead-control half of #40.
- **UTILITY screen shows the app version and support address** (as the unit
  shows its firmware version): `OpenDeck DJ 0.1.x (build)` from Xcode's
  MARKETING_VERSION / CURRENT_PROJECT_VERSION via `OPENDECK_VERSION`, plus
  support@audiodestrukt.com and the support URL.

### Fixed — 2026-09-02
- **Decoder dropped the tail of every packet larger than its output buffer.**
  FLAC (4608-frame blocks) lost ~11 % of its samples, silently shortening
  tracks; the decoder now carries the remainder across calls. **Ogg Vorbis
  crashed** on its zero-frame first packet (skipped now). **M4A/AAC in MP4**
  never opened — the MP4 demuxer was not enabled. Opus is hidden in the browser
  (no decoder exists). All six advertised formats verified end-to-end.

### Added — 2026-08-26
- **Jog wheel: vinyl / nudge modes.** The DJ2Go jog is not touch-sensitive, so
  play state selects the mode: **playing → nudge** (a temporary pitch bend that
  snaps back), **paused → vinyl** (the wheel scrubs the playhead through the
  track; position + waveform only, no scrub-audio yet). Keyboard `,` / `.` drive
  the same path for desktop testing.
- **Start cue point (CDJ CUE).** CUE now behaves like a CDJ: **playing →** return
  to the cue and pause; **paused at the cue →** play from it; **paused
  elsewhere →** set the cue there — so you place the start cue by pausing,
  jogging to the drop, and pressing CUE. The cue shows as an **orange marker** on
  both the enlarged waveform and the overview. Keyboard `Enter` = CUE. (The TEMPO
  readout tracks the pitch fader, not the nudge — matching the real XDJ.)

- **Load tracks at any sample rate (offline SRC).** The browser LOAD path now
  resamples a track whose rate differs from the deck's pipeline (e.g. a 48 kHz
  track into a 44.1 kHz deck) with rubato, once at load — so a mixed-rate library
  loads and plays at the right pitch. Real-time streaming SRC (the way CDJs do
  it, no load pass) stays on the roadmap as WORKSTREAMS A1.

### Fixed — 2026-08-26
- **One malformed MP3 frame no longer kills the whole load.** The decoder skipped
  to `?` on any codec error (e.g. an MP3 bit-reservoir desync, *"invalid main_data
  offset"*), failing the entire track. It now logs and skips the bad packet and
  keeps decoding — a few lost frames beat a track that won't load.

- **File browser (BROWSE screen).** Browse the filesystem like a CDJ reading a
  USB stick without a rekordbox export — folders are categories, audio files are
  rows. The select encoder / `↑``↓` move the highlight, LOAD / `Enter` opens a
  folder or **loads and plays the highlighted track**, Back / `Backspace` goes
  up a level, `B` (or the BROWSE key) toggles the screen. The source column,
  info row and overview keep running while you browse — the loaded track plays
  on. Loading swaps the decoded audio live (lock-free `ArcSwap`, no audio-thread
  teardown) and re-uploads the waveform to the GPU; the deck lands paused at the
  start, as a CDJ does. A new track must match the running device sample
  rate / channel count (resampling is A1); a mismatch is refused, not corrupted.

### Fixed — 2026-08-26
- **Deck got stuck at end of track.** Nothing set `playing` false when a track
  finished, so the phase-locked playhead free-ran past the end into blank
  forever (it is clamped monotonic and cannot return) — the deck looked stuck,
  still "playing", no audio. Now it stops and pins at the end, as a CDJ does in
  SINGLE mode; Cue + Play restarts. Also: the processor stopped publishing
  `in_flight` once the source was exhausted, so it froze ~93 ms short and the
  audible-position estimate never reached the end — it now keeps reporting the
  ring-buffer drain, so end-of-track is actually detected.

### Fixed — 2026-08-25
- **ProDJ Link parser rejected every real packet.** It checked the type byte
  at offset 5 — which in the real 10-byte-magic format (`Qspt1WmJOL`) is the
  `W` — and read BPM from 0x24, the next-beat countdown. It only ever
  understood `send_beat.py`'s private layout. Rewritten to the documented
  96-byte layout (player 0x21, six countdown u32s from 0x24, pitch 0x54, BPM
  u16 at 0x5a, beat-in-bar 0x5c) and verified against `prolink_virtual_cdj`:
  0/16 → 16/16 beats decoded. The captured packet is a unit test. Announce
  packets follow the 0x36-byte layout. `send_beat.py` now emits the real
  format and defaults to 50001.
- **Beat listener only bound 50002.** Real hardware sends beats on 50001.
  Both are bound now.
- **App would not start**: two stacked breakages since March. The binary was
  linked against `librubberband.so.2` while the system had moved to `.so.3`
  (cargo does not notice a C soname change — `make relink`), and
  `Limits::downlevel_defaults()` capped surface size at 2048 px so any display
  wider than that panicked in `Surface::configure`. Limits now come from the
  adapter; surface size is clamped on resize.
- **Waveform judder, part 1 — position**: the playhead was the decoder's cursor,
  which advances one 512-frame block at a time from a sleeping thread. 37% of
  frames showed zero movement and the rest lurched 2–4 blocks. `render_frame`
  now free-runs a phase-locked playhead against the audio clock, with the
  reference low-passed and the playhead clamped monotonic. Stalled frames
  37.2% → 0.0%.
- **Playhead ran ~93 ms ahead of the audio**: `AudioHandle::in_flight` now
  publishes ring-buffer contents plus stretcher latency, and the renderer
  subtracts it. Measured 92.6 ms. Anything derived from position — the beat
  grid, and later ProDJ Link send — was that far in the future.
- **Waveform judder, part 2 — frame pacing**: three clocks (a CPU `WaitUntil`
  timer, the Fifo acquire block, the compositor callback) competed and none was
  locked to the display; only 59% of frames hit their vsync slot. Root cause:
  winit on Wayland only requests the compositor frame callback if the app calls
  `window.pre_present_notify()`, which it never did. Now compositor-paced with a
  Mailbox swapchain, and the playhead advances by whole display periods read
  from the monitor. Zero double frames, one skip in 400. Verified on
  NVIDIA/Vulkan/Wayland only.

### Added — 2026-08-26
- **ProDJ Link send** (`--player N`): announce (50000), beat (50001), and
  status (0x0a, unicast to every peer) are all sent; own broadcasts filtered.
  Status is built from the XDJ's captured packet as a template.
- **Tempo-master handoff**: `M` takes master — 0x26 request, then assert with
  a higher Syncn counter (0x84) and hold. Verified live: the real XDJ-1000MK2
  yields to us, and we yield back to a peer with a higher counter.
- **SYNC follow**: with SYNC on, match the master's effective BPM via the
  pitch fader and phase-lock to its beat. Verified live against the
  XDJ-1000MK2: tempo snapped to −6.48 %, phase converged to ±0.01 beat.
- **Handled from other decks**: incoming 0x26 (master request), 0x27 (yield),
  and 0x2a (sync-control: sync on/off, become master).
- **Beat timing at Pioneer's level**: the sender free-runs a phase-locked
  audible clock (τ ≈ 200 ms) with a sleep-to-deadline for the last 1.5 ms.
  Measured at the same receiver in the same run: XDJ sd 1.33 ms, freedj
  sd 1.23 ms. Method and history in `PERFORMANCE.md`.
- **Status packet parser** (`ProDjLink::parse_status`): play state, PLAY /
  MASTER / SYNC / ON-AIR flags, pitch, BPM, beat, beat-in-bar, master
  handoff, firmware. Tested against a packet captured from the XDJ, as is
  its beat packet (25/25 decoded at 126.00 BPM).
- **Linked-player screen states** from photos of two linked units: two
  phase-meter views (`P` / tap), Bars readouts as cue countdowns (dashes),
  gold MASTER key and BPM box, blue PLAYER box when linked. Photos tracked
  in `reference/photos/`; captures in `reference/link-captures/`.
- **Touch via mouse** on a single input bus (`Event` → `DeckApp::apply`):
  needle search, zoom, SLIP / SYNC / MASTER / MT, time mode, source keys.

### Added — 2026-08-25
- **XDJ-1000MK2 playback screen** (`crates/app/src/screen.rs`): laid out first
  from the manual's *Normal playback screen* diagram, then re-measured against
  a photograph of the unit (`reference/pioneer/xdj-1000mk2-photo.png`, local
  only). From the photo: red full-height playhead; beat grid as edge ticks only
  (red at bars, white at beats); phase meter as two rows of four outlined boxes
  with the current beat solid; off-state pills hidden rather than dimmed;
  light proportional face for the big readouts; green source bar on the
  selected key's edge only; `NEEDLE SEARCH` bar; BLUE waveform and TIME mode
  as defaults to match the unit. `T` toggles REMAIN/TIME. Originally — touch-key row,
  title bar, LINK/source column, MASTER PLAYER + phase meter + beat countdown,
  enlarged waveform with CUE/LOOP · CALL · ZOOM column, info row (PLAYER, TRACK,
  cue pills, REMAIN time, TEMPO, SYNC/MASTER), and the bottom row with SLIP,
  the whole-track overview and BPM. Window defaults to the panel's 1024×600.
  Elements whose data does not exist yet (cues, key, loops, MASTER) are drawn
  in their real positions, dim.
- **Overview waveform** in the shader, peak-per-pixel so transients survive
  the downsample; played portion dims as on the unit.
- **Waveform colour modes**: RGB, 3 BAND (default; blue/amber/white stacked
  by band, dominant band at full height), BLUE. `C` cycles. Colours are
  authored in sRGB and converted once — the surface is sRGB, and writing
  linear values directly had made the ground grey and the bands pastel.
- **Display gain**: bar height normalised to the track's peak column so a
  quiet master still fills the display.
- **`DeckSnapshot`** (`snapshot.rs`): everything the deck knows this frame, in
  one struct; renderer and screen chrome are consumers of it.
- **Frame capture**: `OPENDECK_SCREENSHOT=path` writes frame 90 to a PNG and
  exits; `make shot` wraps it. This is how the layout was checked against the
  manual page without a working desktop screenshot tool.
- **`Makefile`**: `make` lists targets; `run`, `dev`, `two-deck`, `relink`,
  `reference` and the usual `check`/`fmt`/`clippy`/`test`. Thin wrappers over
  cargo.
- **`docs/WORKSTREAMS.md`**: every open workstream with dependencies and three
  defensible starting points.
- **`docs/reference/cdj-3000-playback-screen.md`**: all 29 CDJ-3000 playback
  screen callouts mapped to current status. `make reference` pulls the manual
  pages locally (gitignored — AlphaTheta's copyright).
- **Frame instrument**: `RUST_LOG=opendeck=debug` logs per-frame wall dt, audio
  advance, decode-ahead lag, and acquire/present times. This is how both judder
  causes were found.

### Previously working (documented here for completeness)
- **Key lock / timestretching**: pitch-preserving speed change via Rubber Band R3
  (`crates/timestretch/`), active across the full ±16% pitch range.

### Added
- **ProDJ Link listener** (`crates/app/src/prodj.rs`): UDP listener on port 50002
  receives Pioneer CDJ/XDJ beat packets and drives the second beat grid in real
  time. Uses `socket2` with `SO_REUSEADDR`/`SO_REUSEPORT` so the port can be
  shared with other ProDJ Link tools. Falls back gracefully if the port is
  unavailable.
- **`tools/send_beat.py`**: test utility that sends fake ProDJ Link beat packets
  at a configurable BPM to a configurable host:port, for single-machine testing
  without real Pioneer hardware.
- **`fader_speed` atomic** (`Arc<AtomicU32>`): stable pitch-fader speed, separate
  from the instantaneous playback speed that includes jog nudges. Written by the
  MIDI handler when the pitch fader or pitch-increment buttons are used; read by
  the renderer for beat grid scaling.

### Fixed
- **Second beat grid (B2) scroll velocity**: the B2 strip was always animating at
  1× wall-clock rate while the audio beat markers scroll at `fader_speed ×`
  wall-clock rate, causing continuous phase drift whenever the pitch fader was
  not at centre. Fixed by scaling `beat2_period_cols` by `fader_speed` so both
  grids scroll at the same visual velocity when beatmatched.
- **Jog-nudge interference with B2 strip**: after the velocity fix was first
  implemented using instantaneous `speed`, jogging the local deck temporarily
  changed the B2 strip density and caused it to snap back when the nudge
  released. Fixed by using `fader_speed` (stable, no jog component) instead of
  `speed` for the B2 period scaling.
- **Beat grid density mismatch at non-unity speed**: the audio beat grid period
  was computed from the raw MiniBPM-detected BPM while the B2 strip used the
  incoming CDJ BPM; at matching effective tempos (e.g. local deck slowed from
  135 → 130 BPM to match an incoming 130 BPM CDJ) the grids had different pixel
  densities. Fixed by scaling `beat2_period_cols` by `fader_speed`.
- **`send_beat.py` timing drift**: the original `time.sleep(interval)` loop
  accumulated jitter because each sleep fires slightly late. Switched to
  sleeping until an absolute `monotonic` deadline so errors are corrected on the
  next iteration rather than accumulating.
- **B2 strip visibility**: the 20 px strip was barely distinguishable from the
  background (fill colour `0x03, 0x03, 0x07` vs background `0x04, 0x04, 0x04`),
  and 1 px markers were easy to miss. Increased strip height to 40 px, widened
  markers to 3 px, and changed fill to a distinct dark-blue `(0.0, 0.05, 0.15)`.
- **BPM change logging in renderer**: added a one-time log line when `beat2_bpm`
  changes inside `render_frame`, confirming ProDJ data reaches the renderer.
