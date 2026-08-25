# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Fixed — 2026-08-25
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

### Added — 2026-08-25
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
