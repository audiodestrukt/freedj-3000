# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added — 2026-09-03
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
