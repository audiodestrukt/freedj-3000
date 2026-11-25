# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- DVS (MENU → DVS: OFF / ABS / REL / INT): a Serato Control Vinyl (or
  Control CD, `OPENDECK_DVS_FORMAT=cd`) on a turntable, through an audio
  interface's input, plays the deck. ABS follows the needle (track 0:00 at
  the first groove, and a needle drop jumps); REL takes the record's speed
  and direction and keeps cues and jumps on the deck; INT hands the deck
  back at the record's last speed. The record takes over only after 0.3 s of
  a steady 33 or 45 carrier, so music on the input never stops the deck.
  While it drives, the pitch follows the platter (key lock off), the record
  is the tempo on screen and on Link, PLAY follows the platter, SYNC goes
  off, and a still record is silent. The decoder and these rules are
  freedj-esp's, from its P4 deck, ported to Rust (`crates/timecode`; the
  same positions as the C++ on the Control CD recording). The menu row shows
  NO INPUT / NO SIGNAL / STOPPED / SPEED / LOCKED. Off by default: the
  input opens only when DVS is on (on iOS that is when the microphone
  permission is asked). Not yet on Android, nor in plugin decks. Stems and
  SLIP are not applied while the record drives. Bench: `OPENDECK_DVS`,
  `OPENDECK_DVS_DEVICE`, `OPENDECK_DVS_INPUTS=3/4`, and the `tc_analyze`
  example, which reads a recording the deck's way.
- FOLLOW: `--follow N` (`OPENDECK_FOLLOW`, or a plugin deck's saved
  `follow`) makes the deck a mirror of Link player N. It loads whatever
  that player loads over LINK, never plays, and draws that player's
  playhead, PLAY, pitch, MASTER and SYNC. A CDJ-3000's playhead comes from
  its position packets; an older player's from its beat number and beat
  packets: on the grid at every beat, run on at the pitch between. It only
  listens (no beats, no status, never master), and its own controls do
  nothing. Replaying a recorded XDJ-1000MK2 puts it on every beat within
  2.5 ms. Against an emulated CDJ-3000, placed from its beats alone, it is
  a median of 2.5 ms off the CDJ's own position, p95 22 ms, with no beat
  slips over six pauses. Each beat is named by its place in the bar, since
  a CDJ-3000's status beat number does not always move with its beat
  packet. A beat packet counts as PLAY (it comes about 300 ms before the
  status says so). `OPENDECK_FOLLOW_BEATS=1` ignores a CDJ-3000's
  position packets, for testing the beat clock against them
  (docs/testing/cdj-3000-interop.md).
- LINK loads (and so FOLLOW) from a player that names its track and shares
  its stick over NFS but has no dbserver: the track's path comes from the
  stick's `export.pdb`, by the rekordbox ID in the player's status. This is
  how a FOLLOW deck (or Overdeck's `follow = N`) mirrors freedj-esp's
  firmware on a CDJ-2000. The NFS check is brief (300 ms a try), so a
  player that serves neither still fails at once.

### Fixed
- FOLLOW: a player cued before its track's first beat says beat 0, and
  the mirror's beat clock overflowed on it. Beat 0 now means the lead-in:
  the playhead runs on through it but waits at beat 1 for that beat's
  packet. Found by replaying an XDJ-1000MK2 (firmware 1.45) emulated in
  cdj-nxs2-qemu, its packets stamped in the deck's own time: through two
  pauses and pitch changes to +2.5 % and -2.5 %, the clock had run on to
  each of 274 beats within a median of 0.9 ms (p95 2.6 ms, max 12.8 ms).
- An original (pre-nexus) CDJ-2000 is heard as playing: it leaves the
  status flags byte at 0 and says PLAY only in its play state. It names no
  beat number either, so a FOLLOW deck can load and show its track but not
  place its playhead.
- A plugin deck can draw its screen into its host's instead of opening a
  window (Overdeck's display contract, the `overdeck-display` crate). When
  the host names a screen for it in `OVERDECK_DISPLAY`, the deck renders
  off-screen at that screen's size, copies each frame in, and takes the
  touches the host sends back as its pointer. It shows the screen alone,
  without the faceplate. Overdeck's `--aio` uses it to put every deck in
  one window.
- `OPENDECK_TILE=N` for plugin decks: each deck's window takes its Nth of
  the screen's width, side by side, in the order the decks started (the
  layout scales to fit).  Works where a client may place its windows (X11
  or XWayland, not GNOME's Wayland).  Overdeck's `dock = true` sets it.

### Fixed
- The plugin no longer crashes its host on exit with two or more decks:
  every deck in a process now shares one GPU instance (closing a deck used
  to destroy its own Vulkan instance, and the next deck's teardown crashed
  in the NVIDIA driver), and destroying a plugin instance waits until its
  deck is really closed, so a host can exit straight after.  Found hosting
  two decks in Overdeck; likely the crash REAPER showed at quit.

## [0.3.35] — 2026-11-22

### Added
- Plugin parameters for **juggling and scratching** from host automation:
  Play, Cue (back-cue), Platter touch, and Platter (the record's position,
  so drawing it scratches).  `make reaper-routine TRACK=… BPM=…` plays a
  16-bar beat-juggle and scratch routine on two decks, all automation.
  `make reaper-mixer-check` runs the two-deck mixer and checks it from the
  decks' log and the meters.
- The plugin runs **any number of decks** in one host: two OpenDeck
  instances in REAPER are two decks, each with its own window, output and
  Pitch, sharing one event loop.  Each instance saves its track in its
  state, so a project reopens with its decks loaded.  `make reaper-mixer
  TRACK_A=… TRACK_B=…` sets up two decks into an automated crossfader
  (a test-harness JSFX).
- A ring-jog test rig to print (`hardware/cad/jogring/`, `make jogring`):
  the 206 mm jog as a ring on three V-wheels, around a fixed hub for a round
  smart-watch display (1.28" to 2.1").  An O-ring roller sprung against the
  skirt turns the AS5600 7 × per platter turn.  There are six bearing
  versions, and the build checks that each one holds the ring against every
  nudge and tilt:
  - a 5" mini of the Pioneer-style ring, for fast test prints: 10 × 6 × 3
    bearings (MR106) snap onto split stubs on a carrier that is open
    in the middle, and the carrier screws up under the deck-top stand-in
    from underneath (a 256-count quadrature encoder with bolt-on sensor brackets, so a premade
    slot-sensor board or a bare Vishay TCST1202 can be used; Ø54 window;
    the 1.28" module (Ø42 glass over a Ø36 board, 11.5 tall, a board tab)
    screws to a floor in the hub by its four M2 standoffs, 25 × 19, glass
    0.5 under the window, and the hub's M3 insert holes are Ø4.4 on a 54 mm BCD);
  - a Pioneer-style ring on 16 small rollers, with a ridge under the deck
    top, an optical quadrature encoder (a slotted fin through two opto
    interrupters, 720 counts a turn) and a laser-cut clear window over the
    display;
  - six 608s crossed at ±45° around a ridge on the skirt, each carrying load
    and centring;
  - 608 skateboard bearings on edge under the ring, as on a Pioneer, with
    flat 608s centring it and a lip against tipping;
  - 608s in printed V tyres;
  - bought V-wheels.  Designed and checked in CAD, not printed (docs/design/jog-ring-rollers.md).
- OpenDeck as a **stemd client**, with stems at the playhead first: a
  track with no prepared stems gets them, when its STEMS panel opens, from a
  stemd server on the network (another OpenDeck, or stemd itself on a GPU)
  through stemd's sessions API: the server splits from the deck's playhead
  and the deck plays each stretch as it is ready.  `OPENDECK_STEMD_SERVER`
  names the server, else it is found over mDNS; `OPENDECK_STEMD_CLIENT=0`
  turns it off.  OpenDeck's stemd server serves sessions too.  OpenDeck →
  OpenDeck with the real model on this desktop's CPU: two instances, a
  deck with no model needled 60 s in had stems playable there 2.4 s after
  opening STEMS (the whole 3:51 split in 87 s).
- OpenDeck as a **stemd server** (`crates/stemd`): it answers the HTTP
  contract of [stemd](https://github.com/nsaintot/stemd) on port 8420 and
  announces `_stemd._tcp` over mDNS, so a CDJ-3000 running cdj3k-mods'
  Server Stems (or any stemd client) gets its stems split by this device's
  model (the native HTDemucs, ONNX, or Core ML on an iPad).  The jobs API
  the deck uses (health, upload, poll, fetch, cancel), FLAC / WAV / pcm16
  / pcm32 at 24–96 kHz, and dsp mode 1: stemd's copy of the CDJ-3000's own
  44.1 → 96 kHz filter (ported, MIT), so the deck's rebuilt drums cancel.
  On whenever this device serves live stems (a model, not a Pi);
  `OPENDECK_STEMD=0` turns it off.  The deck's own split comes first.
  Real model: a 3:00 track in 65 s.  Working on an emulated CDJ-3000
  running cdj3k-mods v0.1.1: it found OpenDeck over mDNS, sent the track,
  and its STEMS row played OpenDeck's split (muting drums took them out of
  the waveform).  docs/design/stemd-server.md.
- Live stems between OpenDecks: a deck that loaded a track from another
  OpenDeck, with no prepared stems for it, gets its stems from that
  OpenDeck when its STEMS panel opens.  The source splits the track with
  its own model (an iPad's Core ML, a desktop's HTDemucs) and the deck
  plays each stretch as the source finishes it, the original elsewhere;
  a source without live stems (a CDJ, rekordbox) answers at once and the
  deck splits the track itself.  Still Pro DJ Link and plain NFS files:
  the source serves `CDJMODS/live/<track id>/` with OverCue's page size
  and `wav-44k1` role files plus a page map, so small players need no
  decoder (format in `crates/stems/src/live.rs`).  Tested over loopback
  only.  The source splits from where the player is (the player says by
  looking up `CDJMODS/live/<id>@<page>`) and follows its jumps, and keeps
  finished splits for the next load, up to 2 GB on iOS / Android and 4 GB
  elsewhere (`OPENDECK_LIVE_STEMS_MB`).  The device's own deck always
  comes first: a split for another player waits for, and gives way to,
  its own.  Serving needs a separation model and is off by default on a
  Raspberry Pi, which keeps its cores for its own split
  (`OPENDECK_LIVE_STEMS=1` / `=0`).  Experimental:
  `OPENDECK_LIVE_STEMS_PUBLISH=1` also writes each finished split as a
  standard bank (96 kHz FLAC pages) indexed under our track IDs, for
  readers that only know OverCue's format; `OPENDECK_LIVE_STEMS_ALL=1`
  splits the whole served folder in the background.  docs/design/serving-split-stems.md.

### Changed
- A split (on the device or by a stemd server) waits for the whole
  track: with STEMS already open it used to start on the part streamed in
  so far and start again when the rest arrived.
- Stems fill in steps of whole bank pages: the stems at a mid-track
  playhead no longer wait for the split to wrap round (prepared, live and
  stemd banks).
- The NFS server sees files made while it runs, and a file's current
  size: needed to serve stems as they are split.

### Fixed
- A track reached by FOLDER on a rekordbox stick shows its PREVIEW, BPM
  and key: folder rows now look their file up in the stick's export, as
  the same track does through TRACK.

## [0.3.34] — 2026-11-20

### Added
- OpenDeck as a plugin: a VST3 / CLAP instrument (`crates/plugin`, `make
  plugin`) that runs the whole deck inside the host, its window beside the
  host's and its audio into the host's mixer at the host's rate.  The
  Pitch parameter moves the tempo fader, so host automation plays the deck
  (`make reaper-demo`: an automated ramp and LFO in REAPER).  One deck per
  process; tested in REAPER 7.60 on Linux.  docs/design/plugin.md.
- CDJ-3000 screen, WAVEFORM view: tapping the phase meter switches its slot
  to the tempo master's scrolling waveform, as on the unit (checked on two
  emulated CDJ-3000s, docs/reference/cdj-3000-master-and-waveform-view.md).  It is drawn
  thin above our own, with the other player's beat grid (red on the first
  beat of each bar) moving with it past a line at its playhead, at this
  deck's tempo: tracks in phase line their beats up with our grid, tracks
  that are not drift.  The waveform is read from the other player's
  database (its detail waveform, and its 3-band or RGB colour detail, as
  MENU → WAVEFORM COLOR), follows our zoom, and its playhead is smoothed
  against Wi-Fi packet bursts.  Before the track's waveform arrives, a grid
  from the player's live beats shows the phase.  The other player's active
  loop is shaded amber, as our own is on the main waveform, and the zoom
  works with no track loaded on this deck.  David (@mortalinstrument),
  PR #54.
- CDJ-3000 INFO screen: a tab per player on the network (playing, master
  and loaded marks), showing the selected player's track: title, artist,
  album, key, BPM, length, cover and the stick's colour and player number.
  Tracks stay fetched as tabs switch.  David (@mortalinstrument), PR #54.
- The meter slot starts in the WAVEFORM view (the stacked waveform), as a
  CDJ-3000 ships; a tap switches to the phase meter and back.  The
  waveform runs to the right edge of the screen, as on the unit, with its
  playhead on the enlarged waveform's.
- CDJ-3000 screen: the meter slot reads MASTER PLAYER while this deck is
  the tempo master.  David (@mortalinstrument), PR #54.
- OpenDeck takes tempo master by itself when it plays with no master on
  the network, as a CDJ does: after two seconds (long enough to hear an
  existing master), without MASTER pressed.  With a master present nothing
  changes; MASTER still asks for the role.  Only when LINK sending is on.

### Changed
- MASTER pressed while this deck is master hands the role to a player that
  is playing, as a CDJ-3000 does (it did nothing); with no other player
  playing it stays master.  David (@mortalinstrument) and Dan, PR #54.
- CDJ-3000 screen: pressing the browse encoder on the playback screen does
  nothing, as on the unit (it opened BROWSE, which has its own key).
  David (@mortalinstrument), PR #54.
- binrw 0.15 for the rekordbox export reader, and explicit `f32` literals
  where Rust is phasing out the fallback (a future hard error).  David
  (@mortalinstrument), PR #54.
- The full-size deck's front-right top piece (`hardware/cad/deck7`,
  `top_fr.stl`, the only part that changes): both jog bearings now sit in
  the top's boss, so the base tower is not needed; the AS5600 board snaps
  into a cage under the boss; and the pitch fader screws down from the face
  into its own M3 holes (120 mm centres), straight against the top, instead
  of hanging on a bracket.

### Fixed
- REAPER crashes less on quit with OpenDeck loaded: the plugin pins its
  library in memory, closes its decks at exit, and no longer creates an
  unused EGL instance, whose drop panicked.  One segfault at quit is still
  open (docs/design/plugin.md).
- The REAPER make targets keep track paths with spaces whole (`abspath`
  split them, so the decks booted empty).
- With OpenDeck as tempo master, CDJ-3000s now draw its waveform in their
  WAVEFORM view (Pioneer's "stacked waveform": the master's waveform and
  beat grid above the unit's own).  They never fetched it before: one byte
  of OpenDeck's CDJ-3000 status (0xcd) kept the XDJ-1000MK2's value, and a
  CDJ-3000 does not fetch from a master that sends it.  Now each new track
  OpenDeck loads as master is fetched by every CDJ-3000 within a second.
  Found by bisecting OpenDeck's status against a master CDJ's on two
  emulated CDJ-3000s; not yet seen on a real unit.
- A player loading a track from OpenDeck's library now gets that track's
  rekordbox cues (hot cues, memory cues, loops, colours, comments) when
  the served folder holds a rekordbox stick.  Before, only the track
  loaded on OpenDeck's own deck carried cues; every other track answered
  an empty cue list, because the library scan never read the stick's ANLZ
  cues.  The scan now reads every rekordbox export at the served folder or
  one level down (a folder of sticks).  New end-to-end test: a player asks
  OpenDeck's dbserver for a stick track's cue list.  Cues set on OpenDeck
  for tracks other than the loaded one are still not saved (#65).
- CDJ-3000s on the same LINK network no longer show "Linked to player
  with older firmware.  Please update to latest version."  OpenDeck's
  keep-alive announced itself with an XDJ-1000MK2's protocol byte; it now
  sends the bytes a CDJ-3000 on firmware 3.22 sends.  Tested on two emulated
  CDJ-3000s (cdj3k-emu); not yet on a real unit.
- A track loaded from rekordbox over LINK now brings its memory cues, hot
  cues, hot loops, cue colours and comments.  Before, such a load got
  rekordbox's beat grid and no cues at all: OpenDeck read cues only from a
  stick's ANLZ files, and rekordbox has no stick.  It now asks rekordbox's
  dbserver for the track's cue list (the extended list, else the plain one),
  as a player does.  Loads from a USB stick or a player's stick were not
  affected.  Tested against OpenDeck's own dbserver and the CDJ-3000's
  captured cue list; not yet against a live rekordbox.

## [0.3.33] — 2026-11-18

### Added
- Tests for CUE and PLAY latency: a stand-in low-latency interface records
  every buffer, and CUE held, PLAY from paused, and CUE again after a CUE
  while playing must reach sound within 20 ms (8.7 ms measured: three
  2.9 ms device buffers).
- A Raspberry Pi can boot straight into OpenDeck:
  `deploy/pi/autostart/install.sh` (also in the Linux tarball's
  `pi/autostart`) shows a boot splash from power on, then runs the deck
  fullscreen with no desktop (a systemd service running it in the cage
  kiosk compositor), restarts it if it stops, and points the sound at a
  USB interface (`--audio CODEC` for a Behringer UCA202).  `--desktop`
  keeps the desktop and starts the deck in it instead.
  The splash is the audiodestrukt wordmark (`make_splash.py` draws it from
  audiodestrukt.com's brand); `--screen DSI-1` keeps the deck on one display.
- `OPENDECK_APPLIANCE=1`: this is the hardware deck, not a desktop window.
  Fullscreen; the deck fills a panel of another shape with no bars (the
  normal GPU renderer now squeezes its layout to the panel, sharp, as the SDL
  one did); the CDJ-3000's keys in one row along the top, as on the unit
  (SOURCE … MENU, then BACK and TAG TRACK), the screen below at the panel's
  full width, lists worked by touch; no mouse pointer.  Each part has its own switch too
  (`OPENDECK_FULLSCREEN`, `OPENDECK_CDJ_KEYS=top|side`, `OPENDECK_HIDE_CURSOR`).
  Booted on a Pi 5 with the 7" panel and a Behringer UCA202.
- `OPENDECK_FULLSCREEN=1` now works with the normal renderer too (it was
  the SDL renderer's only): the deck fills the display with no window
  frame, scaled to fit.

### Fixed
- Dropouts on a Raspberry Pi with a USB interface (a Behringer UCA202): the
  deck kept its audio ring only half full, and an ALSA device whose
  callback, woken late, asks for two periods at once (2 x 1 103 frames) ran
  ~2.6 ms short every 200 ms.  The processor now waits only for room for
  what a block puts out at the current speed, so at 1x the ring runs about
  3/4 full; slow speeds keep the worst-case room.  Test: a stand-in device
  taking two periods at once never starves (13 underruns in 2.5 s before).
- BROWSE on a rekordbox stick in the deck: the PREVIEW column draws the
  CDJ-3000's 3-band preview (PWV6, from each track's `.2EX`), as for a
  linked player's tracks; it was the blue PWAV.  A stick exported without
  `.2EX` files keeps the blue preview.
- Linux: started with no track, the deck's own music folder is `~/Music`
  (or `OPENDECK_MUSIC`), for BROWSE and for serving over LINK.  A Pi that
  boots straight into the deck browsed the whole filesystem (`/`, 0 songs)
  and served nothing.
- BROWSE coming back inside a rekordbox stick's folders now comes back
  under its FOLDER category, with the category column (ARTIST, TRACK …)
  beside it.  It came back as bare folders: plain file rows (no PREVIEW
  waveforms) and no way back to the rekordbox lists but the `rekordbox` row.
- FOLDER on a rekordbox stick no longer lists a `rekordbox` row beside the
  stick's own folders (Contents …): the rekordbox lists are already in the
  category column.  The row stays where it is the way in (a stick's top
  level from SOURCE, a local folder with its own export).
- Pulling the USB stick you are browsing takes the deck back to SOURCE, as
  a CDJ does, instead of leaving BROWSE on an empty local folder.  The
  stick's row goes, a loaded track keeps playing (it is in memory; a track
  still reading in when the stick goes plays up to where it got), and the
  stick comes back when plugged in again.  Checked on the Pi 5 by switching
  the stick's USB port off and on mid-browse, and 0.4 s into a load from
  it (the track played to the 17.2 s it had read).  How to repeat it:
  docs/testing/usb-stick-pull.md, and automated in `stick_pull_tests`
  (pulled mid-read, before the first byte, while browsing it);
  `OPENDECK_SCRIPT` gained `source=<label>` to open a SOURCE row.
- Linux: OpenDeck starts without its sound interface (a USB card not
  plugged in, or dropped out in a power dip) instead of exiting: silent,
  keeping time, and playing to the interface within 2 s of it appearing.
  On the Pi appliance the deck used to restart until systemd gave up,
  leaving the console and a flashing pointer.

## [0.3.31] — 2026-11-17

### Added
- MENU → ABOUT, the last row (it shows the version): a screen of its own
  with the version, the support address and every address the deck has on
  the network.  They were a footer under the MENU list, which the last
  rows ran into.
- MENU → OUTPUT GAIN: turns the deck's output down (0 dB to −30 dB in
  steps, saved), for an iPad or interface that is too hot at the volume
  you want to leave it on.  It only trims what leaves the deck, last:
  0 dB is the exact signal as before, there is no boost, and a change
  ramps over one buffer so it never clicks.
- Phrases on the CDJ-3000 screen (MENU → WAVEFORM DIVISIONS: PHRASE, the
  default; TIME SCALE brings the 30-second marks back).  The strip under
  the overall waveform shows the track's phrases as a CDJ-3000 does with
  rekordbox phrase data: a block per phrase in rekordbox's colours (red
  intro, purple up, brown down, green chorus, blue outro; verses and
  bridges for mid and low moods), the label once across a run of the same
  phrase, and a dashed line where a fill-in ends one.  The stick's own
  rekordbox phrases (`.EXT`) when the track has them, else OpenDeck's
  analysis.  A track with neither shows the time scale.

### Fixed
- A linked player that drops off the network mid-browse no longer freezes
  the deck.  Every list opened on it (a category, an artist, a playlist,
  SEARCH) and LOAD's file lookup waited on the player on the UI thread:
  with its cable pulled, one press froze the screen and keys for 15 s
  (a request timeout, a reconnect, a retry).  Lists now load in the
  background behind a "Loading…" row and stay cached for the session, so
  BACK and the category wheel are instant; a player that doesn't answer
  says so in the list; LOAD asks from the loader thread.
- Cues from a CDJ-3000: a track loaded from a linked CDJ-3000 now comes
  with its hot cues, memory points, their colours and comments, and its
  phrases.  The load went through the player's dbserver, which serves a
  CDJ-3000's cue lists empty; they are in the stick's rekordbox analysis,
  which OpenDeck now reads over NFS the way a player does (the export
  once per player, then the track's ANLZ files, about 50 ms).
- rekordbox loops play as loops: a hot loop's pad starts the loop (it only
  jumped), and calling a memory loop sets it as the loop RELOOP enters.
  Both are served on to other players as loops.
- BROWSE on a linked CDJ-3000, WAVEFORM COLOR 3BAND: the track table's
  previews are drawn as the unit draws them, the bands stacked (blue
  lows, amber mids on them, white highs on top) at the unit's heights and
  colours.  They were a third of the height, one colour per column, and
  came out purple-grey: each sub-pixel column's anti-aliased edge let the
  layers under it show through.  Measured against the CDJ-3000's own
  BROWSE and overview in cdj3k-emu (the PWV6 bytes it serves against its
  pixels: the mid counts half the low, the high 0.54, 143 units fill the
  cell).  RGB previews are unchanged and not yet checked against the unit.

## [0.3.30] — 2026-11-17

### Fixed
- The CDJ-3000 emulator image installs in cdj3k-emu (0.3.29's still did
  not): the release builds its rootfs with docker, whose export leaves an
  empty regular file at `/dev/console` instead of the device, and the
  0.3.29 fix only added the device where the name was missing.  It now
  replaces whatever is there.  Rebuilt from 0.3.29's own rootfs, the image
  installs (6,968 entries).

### Added
- `make deckbench`: the Pro DJ Link suite on Deckbench (loopback). A bench
  deck at 126 BPM is master; OpenDeck syncs to it (126 BPM, beats within
  0.05 beat), the bench reads OpenDeck's waveform and beat grid as a player
  would, OpenDeck takes MASTER, and a one-beat loop sends one beat a turn.
  Reading OpenDeck's track info is a known failure (#62).

### Changed
- With `OPENDECK_LINK_IFACE` set, the dbserver listens on that interface's
  address only, as a player answers on its own; so OpenDeck shares the
  port query with other devices bound to their own addresses on one
  machine (Deckbench's decks).  Unpinned, it still listens on every address.

## [0.3.29] — 2026-11-17

### Added
- A 5" deck to print (`hardware/cad/deck5/`): Raspberry Pi 5 with its
  cooler, a 5" 800 × 480 display, a 140 mm jog on two bearings with a
  magnetic encoder, a 100 mm pitch fader, CUE / PLAY, six keys and a
  browse encoder, in a 214 × 280 mm case where every part fits a
  220 mm bed. The top prints in two pieces joined by a tongue and groove
  behind a reveal line, clamped by a base split elsewhere; print-ready
  STLs, renders (hero, cutaway, exploded, the seam), and `make deck5`,
  which checks every part and that nothing in the assembly collides.
- Fullscreen fills a small panel instead of letterboxing it (SDL renderer,
  `OPENDECK_FULLSCREEN=1`): on the Raspberry Pi 7" touch display
  (800 × 480) the CDJ-3000 screen and its key columns cover the whole
  panel, the screen stretched to fit, drawn at the panel's own pixels
  instead of a shrunk image. The beat ticks and cue markers above and below
  the waveform now step with it a whole pixel at a time, so they no longer
  flicker while it scrolls. Why this was hard, and a checklist for new
  displays: docs/design/display-fitting.md.
- LIGHT MODE (MENU → LIGHT MODE, saved; `OPENDECK_LIGHT=1` for one run):
  the screen on a white ground, like the CDJ-3000 light-mode mods. Every
  colour keeps its hue and mirrors its lightness, so black turns white,
  white text black, and the red playhead, blue keys and waveform bands stay
  themselves; album art is left alone and the faceplate stays dark. Works
  on both screens and both render backends. The waveform's edges are
  anti-aliased in light mode, smoothed along the slope between columns, so
  they don't stair-step against the white the way hard edges do (dark mode
  is drawn exactly as before). It costs under 0.1 ms a frame.
- While a track splits, the CDJ-3000 screen shows where its stems are: the
  overview dims the parts not split yet and runs a blue line under the
  parts that are, so you can see where the stem keys will work before you
  jump. The STEMS panel's keys are flat (no checker), one line across them
  says how far the split has got, and when the playhead is in audio not
  split yet it says ORIGINAL HERE in orange and the lit keys go dim; a tap
  there still counts and takes effect when the stems arrive.
- Stems can split with no ONNX Runtime at all: `opendeck-demucs` runs
  Meta's HTDemucs in plain Rust from its own weights (build with
  `separate-native`, point `OPENDECK_SEPARATE_MODEL` at
  `htdemucs.safetensors` from `tools/demucs/export_htdemucs.py`). It
  matches PyTorch more closely than the ONNX export (100-123 dB against
  80-86) and on a Raspberry Pi 5 splits a 7.8 s chunk in 7.5 s, where ONNX
  Runtime takes 15.3: 6.0 s with the transformer in half precision, the
  default on 64-bit ARM (58-67 dB against PyTorch, still closer than the
  ONNX export's vocals; `OPENDECK_DEMUCS_F16=0` keeps it all f32, 6.4 s).
  That is real time on a cool Pi 5; with no cooler it throttles to 1.5 GHz
  and a whole track splits at 0.8x. The aarch64 Pi build
  (`deploy/pi/cross/build.sh`) now includes it.

### Fixed
- Opening a linked player no longer freezes the deck.  Its database
  session (and, for a player without one, its `export.pdb` over NFS) was
  opened on the UI thread: a player that had left the network froze the
  screen and keys for 12 s or more.  The row now reads "Connecting to …",
  the deck keeps drawing and playing, a player that doesn't answer says so
  in its place, and BACK leaves at any point.
- BROWSE → KEY on a linked player: a key's rows (the key, its relatives,
  wider) each list their own set of tracks.  Every row opened the same
  set: the browser asked for the row's id, which a CDJ-3000 sets to the
  key on all three rows (the distance is the row's parent).  Checked on
  a CDJ-3000 in cdj3k-emu: Abm lists 10 tracks, its widest set 36.
- The CDJ-3000 emulator image installs again.  cdj3k-emu stopped with
  "the provisioning guest did not hand back a rootfs" on the released
  `.iso`: the image had no `/dev/console` (a container export leaves
  `/dev` empty), so the emulator's install script ran with no console and
  its "handed back" marker never reached the emulator.  The image now
  carries the node; installed and booted in a fresh slot.
- BROWSE on a linked CDJ-3000: the blue waveform previews in the track
  table now show each track's own shape.  The player's reply (400
  height / whiteness pairs, then a 100-byte tiny preview) was read as its
  last 400 bytes, one column a byte, so the previews looked plausible but
  were wrong.  Checked row by row against the CDJ-3000's own BROWSE in
  cdj3k-emu.
- IN works as the unit's IN/CUE: pressing it also sets the cue point
  there, so CUE afterwards returns to the loop's start (it returned to the
  old cue).  OUT with no IN pressed since the load no longer starts a loop
  from 0:00.
- A Raspberry Pi said NEEDS MORE MEMORY for every split: the check asked
  for the 4.6 GB the iPad's Core ML model needs, where the Rust model
  peaks at 1.2 GB. It now asks for what the model that will run needs
  (1.5 GB for `separate-native`), so a 4 GB Pi splits.
- A split no longer starves the deck's audio. Its threads run at nice 10
  in their own pool, so playback gets the CPU first while every core
  splits (on a Pi without real-time priority, one demo lost 2189 buffers).
- Tempo `+` / `−` keys, and a controller's `tempo_nudge` buttons, went the
  wrong way: `+` slowed the deck by 1 % and `−` sped it up, since the tempo
  fader was turned round (down = faster) in August. They now move the tempo
  the way they say, and stop at the TEMPO RANGE.
- Stems split on the device no longer clip. A stem can be louder than the
  mix it came from (the drums of a techno track peaked at 1.21 against a
  1.0 mix), and the bank's 16-bit samples flattened every peak above full
  scale (532 drum samples on that track). Splits are now stored with 6 dB
  of headroom and played back at their own level; agreement with Meta's
  own HTDemucs rose from 55 to 60 dB on the drums. Prepared stems from a
  stick are unchanged.
- Stems from a split still in progress play again. Since the split started
  from the playhead (0.3.24), the STEMS keys moved but the deck kept playing
  the original until the whole split was done: the mixer also checked an
  empty placeholder range, which asked whether the start of the track was
  split, and a split from the playhead does that last.
- A split no longer slows itself down. With the deck paused it kept giving
  way to its own look-ahead (re-measured from each chunk's time) and
  restarting: 38 chunks and 80 s for a 3-minute track that needs 31. Now a
  split gives way only when the playhead really jumps or overtakes it, and
  aims each new run where the last one did while the playhead stays put: 32
  chunks, 67 s, paused or playing.
- Loading from a linked player: a file the player's library names but its
  NFS export does not resolve by that exact name (a LOOKUP "…" failed, status
  2) is looked up again in the folder's listing ignoring case, and on every
  export the player advertises; each miss is logged with what the folder
  holds.  A load that still fails says so on the screen.
- BROWSE on a linked player: the INFO pane's calendar row shows the date
  added when the library has no year for the track.
- BROWSE on a linked player: FOLDER opens the player's folder tree (folders,
  then the tracks in them) and SEARCH opens the search screen on the
  player's own search; both were listed but did nothing.
- SOURCE on a linked player's USB or SD: the pane shows the stick's songs,
  playlists, date, total and free space as the player reports them (only
  local folders showed them; a linked stick read 0 and 0).
- BROWSE on a linked player: the INFO pane shows the track's cover, and the
  log lists the metadata rows the player sends (key and year come up empty
  when it leaves them out).  Loading a track no longer asks the player for
  its metadata a second time, and the playback screen says LOADING while a
  track is fetched (it only said so in BROWSE, which LOAD leaves).
- TAG TRACK / REMOVE on the playback screen (also INFO and PERFORM) tags, or
  untags, the track on the deck, as on the CDJ-3000.  It only worked in BROWSE.
- Browsing a CDJ-3000 over LINK: every category it lists now opens (GENRE,
  BPM, RATING, YEAR, LABEL, COLOR, TIME, HISTORY, KEY, ORIGINAL ARTIST,
  REMIXER, as well as TRACK, PLAYLIST, ARTIST, ALBUM, FILENAME), with the
  drill-downs the player's own screen uses.  Track rows typed "title and
  artist" (0x0704) and the other 0x??04 forms were dropped, so playlists and
  track lists came up empty; they are tracks now.
- BROWSE on a linked player: the INFO pane shows the highlighted track's
  album, BPM, key and year from the player's library, and SEARCH uses the
  player's own search (two letters or more).
- A track loaded from a linked player takes its title, artist, album, key,
  BPM and year from the player's library when the file's own tags lack them,
  instead of showing the file name.
- BROWSE on a linked player: BPM lists the tempos (they were blank rows:
  a player's BPM rows carry the tempo as a number, not a label), then a
  tempo's ranges ±0 … ±6, then the tracks, as on the unit.  Picking a tempo
  went straight to its tracks at ±0.

### Added
- Stems from the playhead. A track split on the device starts just ahead of
  where it is playing, not at the top, and follows a hot cue or needle jump
  to wherever the deck goes next; the rest of the track is split after.
  Opened 2:00 into a track, the stems were at the playhead about 8 s later on
  a desktop, where they used to wait for the first two minutes to be split.
  Each stretch plays as stems as soon as it is split, the original elsewhere,
  with a short crossfade at the edges.
- The stem model stays loaded while the STEMS panel is open, so the next
  track's split starts without loading it again; it is let go 10 s after the
  panel closes, or at once when iOS warns that memory is short and no split
  is running. A warning during a split is written to the stems test log
  with the memory in use.
- Stems from another player over LINK. A track loaded from a player's USB or
  SD plays the stems prepared for it on that stick, read over the network as
  a local stick is read: found by its rekordbox ID in OverCue's index-v2,
  every page checked. Any player works, stock or running OverCue, since
  only files are read. Against an emulated CDJ-3000 a 2:27 track had its
  first stems in 0.4 s and the whole bank in 6.5 s. A LINK track without
  prepared stems can be split on the device like a local one.
- Prepared stems (from a stick or over LINK) are read from where the deck
  is first, the cue or the playhead, then the rest; a jump to a stretch not
  read yet is read next.
- Dev script: `seek=<seconds>` needle-searches the loaded track.
- CDJ-3000 screen, BROWSE on a linked player: the categories (ARTIST, ALBUM,
  TRACK, KEY, PLAYLIST, HISTORY, …) sit in the left column as on the unit and
  open on the first; SEARCH is its own key and FOLDER is left out.  Track
  lists are the unit's table: a PREVIEW column with the blue or 3-band/RGB
  waveform (as MENU → WAVEFORM COLOR) and the cover, the track, BPM and key
  as the player lists them.  The tab strip above it has PREVIEW (on/off),
  A A A (three text sizes) and INFO (the track's artist, album, length, BPM,
  key, date added, stars, comment and cover beside the list, with LOAD); the
  key beside BPM adds a second column, ARTIST or ALBUM, or switches it off.
- PLAY LIST opens the playlists of the stick being browsed (a linked
  player's, or a local rekordbox stick) in the table layout with no category
  column, and lights instead of BROWSE.  With nothing loaded it does nothing
  but say so.
- BROWSE lists nothing after launch until a source is chosen (SOURCE, USB,
  PLAY LIST), as a unit with nothing loaded does, instead of the app's own
  folder.
- The source tile at the top left (BROWSE, SEARCH, TAG LIST, UTILITY and the
  playback screen) shows what the track or list is on: FOLDER with its icon
  for a local folder, USB, or LINK with the stick's colour label and the
  player number in the corner (SOURCE tints its rows the same way).
- Text too long for its field (titles, list rows, the INFO pane, the playback
  title) scrolls through it like an LED sign, pauses at the end and starts
  again; only the highlighted row of a list scrolls.
- BROWSE on a linked player: the right-hand pane shows what the highlighted
  row opens, as on the unit: a tempo's ranges, a range's tracks, a genre's
  artists, a playlist folder's playlists, an artist's, album's or
  playlist's tracks, a FOLDER level.  It is listed in the background, so
  scrolling never waits on it.

### Changed
- Splitting stems with ONNX Runtime (Linux) takes 1.45 GB instead of
  4.4 GB on a machine without room to spare (under 7 GB available, e.g. a
  4 GB Raspberry Pi 5). ONNX Runtime's constant folding kept about 3 GB of
  pre-computed tensors resident; it is now off there, for chunks about a
  quarter slower and the same stems. A desktop with the memory still folds.
  `OPENDECK_SEPARATE_FOLD=1` / `0` forces it either way.
- Loading from a linked player: the track starts decoding while it still
  downloads, so the deck is playable after the first seconds instead of after
  the whole file (a 65 MB AIFF took about 13 s over Wi-Fi before).  A format
  that needs the end of the file first (some MP3s) still waits for the
  download.
- Prepared stems on a stick with OverCue's `index-v2` (OverCue Desktop
  2.2.40 and later) are found by the track's rekordbox ID with one small
  read, as OverCue's player finds them, instead of reading the whole
  `index.json`. OverCue now lets that file grow to 256 MiB. Sticks without
  index-v2 work as before.
- BROWSE marks a linked player's tracks with "S" too, when the stems are
  prepared on that player's USB or SD (looked up over the network by the
  track's rekordbox ID).
- BROWSE's "S" marks are looked up in the background: scrolling a big
  prepared stick never waits on its stems index. A mark can appear a moment
  after its row.
- BROWSE on a linked player no longer stalls a frame on the network: the
  highlighted tracks' metadata, cover and previews are asked on the
  dbserver session's own thread and fill in a frame or two later; opening
  a category goes ahead of them.
- BACK in BROWSE stops at the source: from a category's list (ALBUM, KEY,
  FOLDER on a stick …) it moves the highlight into the category column on
  the left, where the wheel steps through the categories and pressing it
  goes back into the list.  It used to back out of the stick into the
  folders; FOLDER and SOURCE lead there.  Linked players and rekordbox
  sticks alike.

### Added
- iOS: the log is written to `opendeck-log.txt` in the OpenDeck folder (Files
  app → OpenDeck DJ), the previous launch's beside it, capped at 4 MB.
  Before, it went to stderr, which nothing reads without Xcode attached.

### Fixed
- iOS: sound no longer stops with the ring/silent switch.  The app now
  uses the Playback audio session, as a player must, and picks it up again
  after a call or Siri.
- iOS: the screen is held awake only while a track plays or loads, or for
  five minutes after the last touch, instead of for as long as the app is
  open (an idle iPad stayed lit, hot, and draining).

## [0.3.23] — 2026-11-14

### Added
- MENU (UTILITY) shows the deck's IP address in its footer: the address Pro
  DJ Link uses first, then the device's other networks (up to three on the
  CDJ-3000 screen, two on the XDJ screen; containers, VPNs and Apple's
  peer-to-peer links left out). It follows a network change while MENU is
  open.
- STEMS is on by default (MENU → STEMS turns it off; an older settings file
  with it off turns on once). Loading a track splits nothing: prepared stems
  on a stick play as the track loads, and a track without them is split on
  the device when its STEMS panel (or the XDJ screen's STEMS pad mode) is
  opened, so nobody pays the download, heat or battery for stems they never
  asked for.
- Stems on the iPad and iPhone without copying files: the first time the
  STEMS panel is opened for a track with no prepared stems, the app downloads the stem model
  itself (287 MB, once), checks it against a pinned hash, and starts
  splitting the loaded track as soon as it is in. While it downloads, the
  STEMS panel says so across its keys ("DOWNLOADING STEM MODEL 123 / 287 MB ·
  FIRST USE ONLY"), the STEMS key's bar fills, and MENU reads "ON ·
  DOWNLOADING MODEL n%"; without a network it says it is waiting. It is kept out of iCloud backups. A model
  put in the OpenDeck folder by hand still takes precedence.
- KEY SHIFT on the CDJ-3000 screen. The KEY SHIFT key opens its panel: − and
  + move the key a semitone each (up to an octave either way), labelled with
  the key they lead to; the middle shows the key and its offset, and RESET
  puts the track back in its own key. The tempo does not move. The KEY
  readouts show the shifted key, in the notation the track's key came in.
  Each new track starts in its own key. Works in all three key-lock engines,
  with MASTER TEMPO on or off.
- Hot cue colours from rekordbox. A stick track's hot cues show in the
  colours set in rekordbox, on the pads, the waveform flags and the overview
  of both screens. Cues with no colour stay green. A CDJ-3000 that loads the
  track from OpenDeck gets the same colours (with its HOT CUE COLOR setting
  on), along with memory cue colours and every cue's comment. The colours come
  from the stick's `.EXT` analysis file, read for USB sticks and for tracks
  loaded from another player over LINK. Setting a pad again on the deck makes
  it a plain green cue.
- `fixture_stick` example (opendeck-rekordbox): builds a test rekordbox stick
  from the demo export with your audio and coloured, commented cues; point
  OpenDeck at it with `OPENDECK_MEDIA_DIRS`.
- Stems: play with the stems while a track is still being split on the
  device. The stem keys work as soon as the first stretch is split
  (they still read "SPLITTING n%"), on the part split so far; past it the
  track plays as normal, with a short fade where the split has not reached
  yet. Splitting starts at the top of the track. The enlarged waveform
  follows the stems once the split is done.
- Stems, experimental: a track with no prepared stems can be split on the
  device with the HTDemucs model (MIT). With MENU → STEMS on, the stem keys
  read "SPLITTING n%" after a load, and the stems play once the split
  finishes. On iPad, put `htdemucs.mlpackage` (Core ML, iOS 18+) in the
  OpenDeck folder; each split writes its timing and memory to
  `opendeck-stems-test.txt` there. On a Linux desktop, build with
  `--features separate-onnx` and point `OPENDECK_SEPARATE_MODEL` at
  `htdemucs.onnx`: a 3.5-minute track splits in about 50 s on a 28-thread
  CPU. BROWSE hides model folders. The iPad build asks iOS for its higher
  memory limit, runs the model on the GPU, and logs each step of a split
  with the memory in use, so a split that runs out of memory shows where.
- Stems: the CDJ-3000 STEMS panel now matches OverCue's on the unit: its own
  key spacing, the unit's checkered key faces (blue playing, grey muted),
  and a grey EQ key. In VOLUME mode the blue fills each key from the left
  to its level, with no percentage, as OverCue's manual shows; TOGGLE greys
  out while it can't be chosen.
- Stems: BROWSE marks tracks that have stems with a small "S" beside the ♪,
  as OverCue does on the unit (CDJ-3000 screen, with MENU → STEMS on).
- Stems: the enlarged waveform now shows the stems you're playing, at their
  levels, as OverCue's does. Muting the drums takes the kicks out of the
  picture, and every stem off leaves a thin grey line. The overview below
  stays the full track, and FULL MIX brings the track's own waveform back.
- Cue points for other decks: a player or tool that loads one of our
  tracks now gets its hot cues (A–H) and memory cues, in both the plain cue
  list and the CDJ-3000's extended one, encoded like a CDJ-3000's (byte-exact
  against a capture).  An emulated CDJ-3000 shows them and cues to the first
  memory point. Before, every cue list was served empty.
- Absolute Position packets go straight to each device on the network, as a
  CDJ-3000 sends them (captured), instead of being broadcast.
- CDJ-3000 status: devices other than older players (XDJs, NXS2 and
  earlier) now get the CDJ-3000's 1,152-byte status, with the active loop
  (start, end, length in beats) where a CDJ-3000 reports it; Beat Link and
  tools built on it read loops only from this form.  Older players keep
  the status of their generation.
- Other players' play position: OpenDeck keeps each CDJ-3000's (or
  OpenDeck's) Absolute Position packets and can run the playhead forward
  between them (`LinkState::playhead_ms`), and reads the active loop from a
  CDJ-3000's long status.  Nothing on screen uses them yet.
- **CDJ-3000 Absolute Position packets.** While a track is loaded (playing
  or not) OpenDeck now sends, every 30 ms on UDP 50001, what a CDJ-3000
  sends: the playhead in ms, the track length, pitch and BPM. Lighting,
  video and timecode tools follow a CDJ-3000 by these; a tester's tool saw
  no playback position from OpenDeck. Measured: one every 30.6 ms, the
  playhead in step with playback.
- **WAVEFORM COLOR in MENU** (BLUE, RGB, 3BAND), saved, and **3BAND by
  default**, as the CDJ-3000 shows it: lows blue, mids amber, highs white.
  The waveforms were blue unless you knew the `C` key, which did not stick
  and could not be reached by touch (iPad). `C` now cycles the saved
  setting.
- **DECK VIEW: switch between the whole deck and the screen alone while
  running** (desktop): the `F` key or MENU → DECK VIEW. The window resizes
  to the new shape, fitted to the screen, and the choice is saved; the full
  deck stays the default on the Mac and Windows. `--faceplate` /
  `--screen-only` still override it for one run.

### Changed
- Splitting stems on an iPad or iPhone no longer keeps the model (about 4 GB)
  in memory after the split, so the deck is not carrying it through the rest
  of the set; the next split loads it again in a few seconds. A device that
  cannot give a split the memory it needs shows "NEEDS MORE MEMORY" instead
  of being closed by iOS partway, and downloads nothing.
- Stems from an OverCue stick play almost at once. The bank is read page by
  page from the start of the track, each page checked as it arrives, and the
  deck plays the stems as far as they have got and the track itself beyond
  that. On a real stick: stems playing 0.15 s after a load, where the whole
  bank used to be read first (1.5–2 s on a desktop, longer on an iPad). A
  bank that fails a check partway is dropped and the track plays on.
- Audio decoding moved to Symphonia 0.6.1.  Checked against 0.5 on 23 files
  (MP3 CBR / VBR / 44.1 kHz and two real tracks, AAC, ALAC, FLAC, WAV, AIFF
  at 16 / 24 / 32-bit and float, Vorbis, 5.1): every format decodes to the
  same samples, at the same length and in the same place; AAC and Vorbis
  differ only by float rounding (−134 dB).  MP3 and Vorbis are still clipped
  at full scale and keep the MP3 encoder delay, as before; tags read as before.

### Fixed
- Phase meter: the red bar mark no longer jumps a beat for a frame now and
  then. On the master's row the beat-in-bar changed when each beat packet
  arrived, a few ms either side of the beat the row had drawn; it now counts
  along with the drawn phase. On our own row the phase could round up to a
  whole beat a frame before the beat number did.
- rekordbox sticks with cues: a track with any memory or hot cue point
  failed to read its analysis (the bundled parser took a cue point to be
  type 0; rekordbox writes 1), so its beat grid was lost as well as its cues.
  A cue without a comment, and extended cue entries of other lengths, failed
  the same way. No test stick had cues, so this never showed.
- rekordbox hot cues landed one pad late: A on B, B on C, and H was dropped.
- A CDJ-3000 browsing OpenDeck's music now shows each track's key in its
  track list (the KEY column was empty, though the key showed once the track
  was loaded). The track rows now have the 16 fields a CDJ-3000 sends for
  its own tracks, with the key and the tempo. The player also stops asking
  for every listed track's details one by one.
- Loops over Pro DJ Link: while the deck loops, the players, mixers and
  lighting tools following OpenDeck now see it loop. Short loops (½ or 1
  beat) used to run on through the song over Link, 1–2 beat loops sent no
  beats at all, and every wrap could send a stray beat from before or after
  the loop. Now each time round sends the loop's beat once. Details in
  docs/design/link-loop-timing.md.
- With `OPENDECK_LINK_IFACE=lo`, Link broadcasts go to 127.255.255.255, so
  other programs on the machine hear them (Deckbench, a second OpenDeck).
  `tools/deckbench/loop_beats.py` checks loops over Link on Deckbench.
- SLIP works with hot cues and scratching: hold a hot cue (or hold the
  platter in VINYL mode) with SLIP on and the track carries on underneath;
  letting go returns to where it would have been. Before, SLIP only
  covered loops.
- LOOP IN, LOOP OUT and RELOOP act the moment the key is pressed, as on
  the unit, so a loop point lands where you pressed, not where you lifted.
- Touch keys on the iPad no longer miss taps: a tap counts when the finger
  wobbles a little, when it is very quick, and when another finger is
  already on the jog or the fader (panel tabs such as STEMS / BEAT JUMP,
  faceplate keys and screen keys).
- TEMPO RANGE no longer moves the tempo fader. The fader stays where it
  is and the tempo follows the new range, as on a CDJ: +3 % on ±6 becomes
  +8 % on ±16.
- 32-bit FLAC (FLAC 1.4 and later) loads and plays.  Symphonia 0.5 refused
  it and 0.6.1 decodes its stereo wrongly; the FLAC crate is vendored with
  the open upstream fix (Symphonia PR #587), and decodes bit for bit as the
  reference `flac` does.
- AIFF tags: title, artist and the rest no longer end in a stray NUL, and the
  key, BPM and cover art are read; an AIFF with an ID3 chunk no longer gains
  two frames of noise at the end.  An OGG Vorbis file with tags no longer
  fails to open.
- 96 kHz (and 88.2 / 176.4 / 192 kHz) files got a beat grid about 4 ms
  early when analysed at their own rate, as the media server does for
  tracks it serves to other decks: the beat finder works in fixed
  512-sample steps.  It now halves the rate to 48 kHz or less first; a
  track at 48, 96 and 192 kHz gets the same grid to the hundredth of a ms.
  The analysis cache is rebuilt once to pick this up.
- Beat packets reported a pitched OpenDeck's tempo twice: the BPM field
  carried the heard tempo instead of the track's, and receivers multiply it
  by the pitch.  Synced to a 126 BPM CDJ-3000 at −3 %, OpenDeck advertised
  122.12 BPM instead of 126.  Decks synced to a pitched OpenDeck master, and
  lighting tools, were told the wrong tempo.  It now sends the track's BPM
  and the pitch, as a CDJ-3000 does.
- CDJ-3000 screen: the 16 and 32 under the waveform stay still, 16 and 32
  beats ahead of the playhead, as on the unit; they used to ride on a beat
  and jump back one beat at every beat.
- **Track title and artist for tools that go straight to TCP 1051.** Our
  track database (dbserver) took a random port and reported it on 12523, as
  the protocol allows; rekordbox and the players ask there, but some tools
  connect to 1051, where every player has it, and got nothing. It now
  listens on 1051 whenever the port is free.
- **The shared music folder when started with a bare file name**
  (`opendeck track.mp3`): its folder was taken as an empty path, so nothing
  was served.

## [0.3.10] — 2026-11-12

### Changed
- **The Mac and Windows start with the full deck**: jog, every key and the
  tempo fader around the screen, as on the iPad (`--screen-only` for the
  screen alone; Linux keeps screen-only, `--faceplate` for the deck). The
  CDJ-3000 deck opens at its own aspect (no side bands), and a first window
  taller than the screen is scaled down to fit it.
- **PLAYER No. AUTO tries 4, 5, 6, 3, 2, 1**, the same on every platform:
  OpenDeck starts at 4, above the players a DJ numbers from 1, goes up while
  4 is taken, then down. It used to try the platform's own default (1 on the
  desktop, 3 on an iPad, 4 on an iPhone), then the lowest free number.
- **BROWSE remembers where you were.** Each source (the home folder, each
  stick) keeps its own place, folder and highlighted row: going to another
  source and back, or relaunching, returns there (saved by name in
  `browse.json`, so it survives a rescan or a re-read export). BACK lands on
  the folder it came out of instead of the top of the list. The USB / BROWSE
  key returns to the source browsed last; a stick mounted since no longer
  takes it over (pick the stick from SOURCE, or its row in the list).
- **Tracks are playable almost at once, as on a CDJ.** A load used to
  decode the whole file, resample it, build its waveform and detect its grid
  before the deck could play. Now the deck is playable once the audio four
  seconds past the cue is decoded, and the rest streams in behind playback:
  an 8.8-minute MP3 is playable in 19 ms (fully in after 0.9 s) on a desktop,
  where the deck used to wait for all of it. The full length shows at once,
  and the overview fills in as the track decodes. A grid we detect ourselves
  (no rekordbox data) arrives once the whole track is in.
- **The CDJ-3000 screen is the default** on the desktop, iPad, Android
  tablets and the Pi. An install still on the old default (XDJ-1000MK2)
  moves to it once; MENU → SCREEN switches back, and stays switched.
  Phones keep the XDJ-1000MK2 layout (the CDJ-3000 screen has no phone
  layout yet).

### Fixed
- **A 120–150 ms freeze after loading a track for the first time.** Key
  detection held a lock the screen needed for its whole pass over the track,
  so the first load of any file without a cached analysis froze the UI for
  about a tenth of a second (longer on slow devices).
- **Loading on slow CPUs (cdj3k-emu, Pi Zero).** A track took about a
  minute to load in the emulator, and each further press of LOAD started
  another full decode alongside it, so a few presses looked like loading
  did nothing. LOAD on the track already loading is now ignored, a newer
  LOAD stops the older one at its next stage, and load-time resampling
  (48 to 44.1 kHz) uses an FFT resampler, 3x faster than the 256-tap sinc
  (0.20 s vs 0.59 s for 200 s of audio on a desktop). The log times each
  load stage (`load: decoded / resampled / waveform in … ms`).
- **The playing track's analysis reaches the network at once.** A peer
  that looks up the track our status names (another player's LINK INFO, a
  Beat Link-style lighting controller following the deck) got no grid,
  waveform or phrases until the library's background analysis reached the
  file, and never for a file outside the served folder (a USB stick, a
  Files-app pick, the iOS demo track): those were reported as track 0.
  Every local load now hands the deck's own analysis to the dbserver; a
  file outside the folder gets an unlisted row under its own id (answers
  by id, in no menu), and with no music folder at all a dbserver starts
  for it. The served beat grid is the one the deck plays to: a hand-
  adjusted grid and every GRID ADJUST edit reach peers at once, and the
  library's background analysis no longer overwrites a row the deck
  already filled.

## [0.3.9] — 2026-11-12

### Added
- **OpenDeck as cdj3k-emu firmware (emulator UPD).** `deploy/cdj3000/emu/`
  builds a small Debian arm64 rootfs and `tools/upd/updpack.py` packs it,
  with OpenDeck, into the layout of a CDJ-3000 firmware release
  (`CDJ3Kv###-OPENDECK-EMU.UPD`). Without a key it stops at the plaintext
  ISO; the LUKS1 container is written in Python (no root, no cryptsetup).
  `tools/upd/emucheck` runs cdj3k-emu's own install code over the result:
  decrypt, firmware info, kernel pick, rootfs, and all of its patch
  scripts pass. Booted in QEMU with the emulator's kernel: OpenDeck starts
  in 7 s, opens the panel device and plays, drawing the CDJ-3000 screen.
  Installs with `cdj3k-emu --provision` from the plaintext ISO (no key).
  Releases attach the ISO, and the encrypted UPD when the
  `CDJ3000_UPD_KEY` secret is set (`.github/workflows/cdj3000.yml`).
- **Bare CDJ-3000 screen on the unit.** With `OPENDECK_SUBUCOM` set (the
  real panel is attached) the CDJ-3000 skin draws only the 1280 x 720 LCD,
  no on-screen keys.
- **Framebuffer output for the SDL renderer** (`OPENDECK_FBDEV=/dev/fb0`):
  each frame is copied to a Linux framebuffer, for displays where DRM page
  flips do not reach the screen (cdj3k-emu's virtio-gpu).
- **The CDJ-3000's panel as a control surface.** With
  `OPENDECK_SUBUCOM=/dev/subucom_spi1.0`, OpenDeck reads the CDJ-3000's
  sub-CPU directly, as the deck's own player does: every key, the jog (turn
  and touch), the tempo fader and the browse rotary drive the deck, and the
  key and pad LEDs follow it. It is the same device on a real deck and in
  cdj3k-emu, so OpenDeck running as the emulator's guest gets the drawn
  panel. Keys behave as on the on-screen CDJ-3000 faceplate. Unit-tested on
  synthetic frames; not yet run on the device. LCD touch (frame bytes
  16–19) reaches the screen as pointer presses and drags.
- **Key detection.** Tracks without a key tag get a detected key, named as
  rekordbox names it, shown in KEY and served to players. On a 226-track
  rekordbox stick it agrees exactly with rekordbox on 72 % (cross-validated);
  most misses are the right tonic in the other mode. About 60 ms a track.
- **Phrase analysis (first cut).** OpenDeck detects phrases (intro, verse /
  up / down, chorus, outro; mood high / mid / low) and serves them as
  rekordbox's PSSI section, so a CDJ-3000 can colour the waveform by phrase.
  The encoder matches rekordbox's (masked) format byte for byte; the
  detector is calibrated on 31 phrase-analysed tracks from rekordbox over
  LINK (`examples/phrase_vs_rekordbox_link`): phrase starts within a bar of
  rekordbox's, precision 0.70 / recall 0.73; mood agrees on 25 of 31 (a
  house set, so mood is not yet calibrated for other genres); coarse phrase
  type 65 %. With a rekordbox grid, its downbeats are used for the bars.
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
