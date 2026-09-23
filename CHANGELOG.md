# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Fixed
- **No click when playback stops.** Releasing CUE, pausing, or a platter or
  waveform hold used to cut to silence on the next sample, a step you hear
  as a click. The output now fades over 3 ms on stop, on the audio already
  buffered, with a soft landing. Starting is untouched: no fade-in, so cue
  latency is exactly as before.

## [0.2.9] — 2026-09-22

TestFlight build.

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
