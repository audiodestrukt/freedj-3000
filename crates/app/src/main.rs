//! OpenDeck MVP — play an MP3 with waveform visualization.
//!
//! Usage:  opendeck <path/to/file.mp3>
//!
//! Controls:
//!   Space    — play / pause
//!   ← / →   — seek ±10 seconds
//!   Esc / Q  — quit

mod audio;
mod input;
mod midi;
mod prodj;
mod renderer;
mod screen;
mod snapshot;

use anyhow::{bail, Context, Result};
use audio::AudioHandle;
use opendeck_analysis::{BeatAnalyzerImpl, WaveformBuilder, WaveformCache};
use opendeck_types::{BeatAnalyzer, BeatGrid};
use renderer::Renderer;
use input::{ControlEvent, Event, Source, UiEvent, ZOOM_LEVELS, ZOOM_DEFAULT};
use snapshot::DeckSnapshot;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowAttributes},
};

// ── App state ─────────────────────────────────────────────────────────────────

/// Target frame interval — 60 fps.
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

struct DeckApp {
    // Provided before event loop starts.
    path:         PathBuf,
    waveform:     WaveformCache,
    audio:        AudioHandle,
    beat_grid:    Option<BeatGrid>,

    // Second beat grid — tempo controlled by Deck B on the MIDI controller.
    fader_speed:  Arc<AtomicU32>,  // f32 bits; pitch-fader speed (no jog nudge)
    beat2_bpm:    Arc<AtomicU32>,  // f32 bits; BPM of the second grid
    beat2_anchor: Arc<AtomicU64>, // written by MIDI Cue B to signal a phase reset
    beat2_player: Arc<AtomicU32>, // player number of the last Link beat sender
    beat2_start:  Instant,        // wall-clock time of the last phase reset
    prev_beat2_anchor: u64,       // detect changes in beat2_anchor
    prev_beat2_bpm:    f32,       // detect BPM changes for logging
    prev_pos:          u64,       // previous frame's audio position (scroll instrument)
    smoothed_pos:      f64,       // phase-locked playhead, source samples
    heard_avg:         f64,       // low-passed reference the playhead locks to
    refresh_interval:  Duration,  // display period; each frame lands in one of these
    frame_count:       u64,
    remain_mode:       bool,      // time display: REMAIN vs TIME
    key_lock:          bool,      // MT indicator (Rubber Band path is always on today)
    slip:              bool,
    sync:              bool,
    master:            bool,
    zoom_level:        usize,     // index into ZOOM_LEVELS
    zoom_grid_mode:    bool,
    source_link:       bool,
    phase_ticks_view:  bool,
    /// Input bus: every source pushes here; `apply` drains it once per frame.
    events:            Vec<Event>,
    exit_after_capture: bool,

    // Created on first `resumed`.
    window:      Option<Arc<Window>>,
    renderer:    Option<Renderer>,
    egui_ctx:    egui::Context,
    egui_state:  Option<egui_winit::State>,

    /// Time of the last rendered frame, used to cap to FRAME_INTERVAL.
    last_render: Instant,
}

/// Display/deck flags copied out of DeckApp so a snapshot can borrow only
/// `path`, `beat_grid` and `audio` — leaving `renderer` free to be borrowed
/// mutably in the same frame.
#[derive(Clone, Copy)]
struct UiFlags {
    key_lock: bool, remain_mode: bool, slip: bool, sync: bool, master: bool,
    zoom_grid_mode: bool, source_link: bool, phase_ticks_view: bool, linked: bool, master_player: u8,
}

#[allow(clippy::too_many_arguments)]
fn make_snapshot<'a>(
    path: &'a std::path::Path, beat_grid: Option<&'a BeatGrid>, audio: &AudioHandle, f: UiFlags,
    pos: u64, playing: bool, speed: f32, fader_speed: f32, beat2_bpm: f32, beat2_phase_beats: f32,
) -> DeckSnapshot<'a> {
    DeckSnapshot {
        title:         path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown"),
        position:      pos,
        sample_rate:   audio.sample_rate,
        channels:      audio.channels,
        total_samples: audio.samples.len() as u64,
        playing, speed, fader_speed,
        key_lock:      f.key_lock,
        remain_mode:   f.remain_mode,
        slip:          f.slip,
        sync:          f.sync,
        master:        f.master,
        zoom_grid_mode: f.zoom_grid_mode,
        source_link:   f.source_link,
        phase_ticks_view: f.phase_ticks_view,
        linked:        f.linked,
        master_player: f.master_player,
        beat_grid,
        beat2_bpm,
        beat2_phase_beats,
    }
}

impl DeckApp {
    fn new(
        path:         PathBuf,
        waveform:     WaveformCache,
        audio:        AudioHandle,
        beat_grid:    Option<BeatGrid>,
        fader_speed:  Arc<AtomicU32>,
        beat2_bpm:    Arc<AtomicU32>,
        beat2_anchor: Arc<AtomicU64>,
        beat2_player: Arc<AtomicU32>,
    ) -> Self {
        Self {
            path,
            waveform,
            audio,
            beat_grid,
            fader_speed,
            beat2_bpm,
            beat2_anchor,
            beat2_player,
            beat2_start:       Instant::now(),
            prev_beat2_anchor: 0,
            prev_beat2_bpm:    0.0,
            prev_pos:          0,
            smoothed_pos:      0.0,
            heard_avg:         0.0,
            refresh_interval:  FRAME_INTERVAL,
            frame_count:       0,
            remain_mode:       false,   // the reference unit was in TIME mode
            key_lock:          true,
            slip:              false,
            sync:              false,
            master:            false,
            zoom_level:        ZOOM_DEFAULT,
            zoom_grid_mode:    false,
            source_link:       false,
            // Dev: OPENDECK_PHASE_VIEW=ticks starts in the alignment view (for captures).
            phase_ticks_view:  std::env::var("OPENDECK_PHASE_VIEW").map(|v| v == "ticks").unwrap_or(false),
            events:            Vec::new(),
            exit_after_capture: false,
            window:      None,
            renderer:    None,
            egui_ctx:    egui::Context::default(),
            egui_state:  None,
            last_render: Instant::now(),
        }
    }

    /// The only place input changes deck state.  Every source — keyboard,
    /// touch, later MIDI and scripts — ends up here.
    fn apply(&mut self, ev: Event) {
        log::debug!("event: {ev:?}");
        match ev {
            Event::Deck(ControlEvent::Play)  => { self.audio.playing.store(true,  Ordering::Relaxed); log::info!("playing"); }
            Event::Deck(ControlEvent::Pause) => { self.audio.playing.store(false, Ordering::Relaxed); log::info!("paused"); }
            Event::Deck(ControlEvent::Cue)   => { self.audio.position.store(0, Ordering::Relaxed); }
            Event::Deck(ControlEvent::NeedleSearch { position }) => {
                let total = self.audio.samples.len() as f64;
                // Land on a frame boundary so channels stay interleaved.
                let ch = self.audio.channels as u64;
                let target = ((position.clamp(0.0, 1.0) as f64 * total) as u64 / ch) * ch;
                self.audio.position.store(target.min(self.audio.samples.len() as u64), Ordering::Relaxed);
            }
            Event::Deck(ControlEvent::TempoFader { position }) => {
                let s = input::fader_to_speed(position);
                self.fader_speed.store(s.to_bits(), Ordering::Relaxed);
                self.audio.speed_store(s);
                log::info!("tempo {:+.2}%", (s - 1.0) * 100.0);
            }
            Event::Deck(ControlEvent::KeyLockToggle) => {
                // The Rubber Band path is always engaged today; this is the
                // indicator only, until the resample path exists (A1).
                self.key_lock = !self.key_lock;
                log::info!("master tempo {} (display only for now)", if self.key_lock { "on" } else { "off" });
            }
            Event::Deck(ControlEvent::SlipToggle)    => { self.slip   = !self.slip;   log::info!("slip {} (no engine yet)", self.slip); }
            Event::Deck(ControlEvent::SyncToggle)    => { self.sync   = !self.sync;   log::info!("sync {} (Link send not implemented)", self.sync); }
            Event::Deck(ControlEvent::MasterRequest) => { self.master = !self.master; log::info!("master {} (Link send not implemented)", self.master); }
            Event::Deck(other) => log::info!("unhandled deck event {other:?}"),

            Event::Ui(UiEvent::TimeMode) => {
                self.remain_mode = !self.remain_mode;
                log::info!("time display → {}", if self.remain_mode { "REMAIN" } else { "TIME" });
            }
            Event::Ui(UiEvent::CycleColor) => {
                if let Some(r) = &mut self.renderer {
                    use renderer::ColorMode::*;
                    r.color_mode = match r.color_mode { Rgb => ThreeBand, ThreeBand => Blue, Blue => Rgb };
                    log::info!("waveform colour → {:?}", r.color_mode);
                }
            }
            Event::Ui(UiEvent::ZoomStep(d)) => {
                let n = ZOOM_LEVELS.len() as i32;
                // Positive = zoom in = fewer columns visible.
                self.zoom_level = (self.zoom_level as i32 - d).clamp(0, n - 1) as usize;
                if let Some(r) = &mut self.renderer { r.cols_visible = ZOOM_LEVELS[self.zoom_level]; }
                log::info!("zoom {} cols", ZOOM_LEVELS[self.zoom_level]);
            }
            Event::Ui(UiEvent::ZoomGridMode) => { self.zoom_grid_mode = !self.zoom_grid_mode; }
            Event::Ui(UiEvent::PhaseMeterView) => { self.phase_ticks_view = !self.phase_ticks_view; log::info!("phase meter → {}", if self.phase_ticks_view { "alignment" } else { "beat display" }); }
            Event::Ui(UiEvent::Source(src))  => { self.source_link = src == Source::Link; log::info!("source {src:?}"); }
            Event::Ui(UiEvent::Screen(sc))   => log::info!("{sc:?} screen not implemented"),
        }
    }

    fn render_frame(&mut self) {
        let frame_start = Instant::now();
        let frame_dt    = frame_start.duration_since(self.last_render);
        self.last_render = frame_start;

        let (egui_state, window) = match (self.egui_state.as_mut(), self.window.as_ref()) {
            (Some(s), Some(w)) => (s, w),
            _ => return,
        };
        if self.renderer.is_none() { return; }

        let raw_pos      = self.audio.position.load(Ordering::Relaxed);
        let in_flight    = self.audio.in_flight.load(Ordering::Relaxed);
        let playing      = self.audio.playing.load(Ordering::Relaxed);
        let speed        = self.audio.speed_load();

        // ── Smooth, latency-compensated playhead ──────────────────────────────
        // `position` is the decoder's cursor: it advances in BLOCK_FRAMES steps
        // on a thread whose wake-up is scheduler-dependent.  Sampling it once
        // per frame makes the waveform freeze for a frame then lurch several
        // blocks — measured at 37% of frames showing zero movement, with jumps
        // quantised to multiples of 11.6 ms.  The shader interpolates between
        // columns perfectly well; the input was the staircase.
        //
        // So free-run a local playhead at the true playback rate and phase-lock
        // it to the decoder, correcting a fraction of the error each frame.
        // Motion becomes continuous and the audio thread only has to supply a
        // reference, not a per-frame value.
        //
        // `in_flight` is what has been decoded but is still queued in the ring
        // buffer and stretcher, so subtracting it puts the playhead on what the
        // listener actually hears rather than ~93 ms ahead of it.
        let sr_ch  = self.audio.sample_rate as f64 * self.audio.channels as f64;
        let heard  = raw_pos.saturating_sub(in_flight) as f64;

        if self.smoothed_pos <= 0.0 || (heard - self.smoothed_pos).abs() > sr_ch * 0.5 {
            // First frame, or a seek — snap, and reset the reference filter.
            self.smoothed_pos = heard;
            self.heard_avg    = heard;
        } else {
            // `in_flight` is sampled once per decode block, right after a push,
            // so it is both biased high and noisy — measured swinging ±35 ms.
            // Low-pass it before phase-locking, or that jitter goes straight
            // into the playhead and the waveform visibly walks backwards.
            self.heard_avg += (heard - self.heard_avg) * 0.05;

            if playing {
                // Advance by whole display periods, not by measured wall-clock.
                // Each rendered frame is shown for an integer number of
                // refreshes; the CPU-side `frame_dt` carries ±2ms of wake-up
                // slop that is not reflected on screen.  Snapping dt to the
                // nearest multiple of the refresh period keeps motion exact
                // per refresh and still accounts for a genuinely dropped frame.
                let period  = self.refresh_interval.as_secs_f64();
                let periods = (frame_dt.as_secs_f64() / period).round().max(1.0);
                self.smoothed_pos += periods * period * sr_ch * speed as f64;
            }
            // Gentle pull toward the filtered reference: enough to stop drift
            // over minutes, far too slow to see as a step.
            self.smoothed_pos += (self.heard_avg - self.smoothed_pos) * 0.02;

            // Never let the playhead run backwards during playback.  A stall
            // reads as a hitch; reversal reads as a glitch, which is worse.
            if playing {
                self.smoothed_pos = self.smoothed_pos.max(self.prev_pos as f64);
            }
        }
        let pos = self.smoothed_pos.max(0.0) as u64;

        // Scroll-smoothness instrument: audio time advanced per frame should
        // track wall-clock time per frame.  Departure from ratio 1.0 is judder.
        if log::log_enabled!(log::Level::Debug) && self.prev_pos != 0 {
            let d_audio_ms = (pos as f64 - self.prev_pos as f64) / sr_ch * 1000.0;
            let d_wall_ms  = frame_dt.as_secs_f64() * 1000.0;
            log::debug!(
                "frame: dt {:5.2}ms  audio {:6.2}ms  ratio {:5.2}  lag {:5.1}ms",
                d_wall_ms, d_audio_ms,
                if d_wall_ms > 0.0 { d_audio_ms / d_wall_ms } else { 0.0 },
                in_flight as f64 / sr_ch * 1000.0,
            );
        }
        self.prev_pos = pos;
        let fader_speed  = f32::from_bits(self.fader_speed.load(Ordering::Relaxed));
        let beat2_bpm    = f32::from_bits(self.beat2_bpm.load(Ordering::Relaxed));
        let beat2_anchor = self.beat2_anchor.load(Ordering::Relaxed);
        let beat2_player = self.beat2_player.load(Ordering::Relaxed);

        // Log when beat2_bpm changes (confirms ProDJ data is reaching the renderer).
        if (beat2_bpm - self.prev_beat2_bpm).abs() > 0.01 {
            log::info!("render: beat2_bpm updated {:.2} → {:.2}", self.prev_beat2_bpm, beat2_bpm);
            self.prev_beat2_bpm = beat2_bpm;
        }

        // Reset the phase timer whenever the MIDI Cue B button is pressed.
        if beat2_anchor != self.prev_beat2_anchor {
            self.beat2_start       = Instant::now();
            self.prev_beat2_anchor = beat2_anchor;
        }
        let beat2_phase_beats = if beat2_bpm > 0.0 {
            let elapsed = self.beat2_start.elapsed().as_secs_f32();
            (elapsed * beat2_bpm / 60.0).fract()
        } else {
            0.0
        };
        let flags = UiFlags {
            key_lock: self.key_lock, remain_mode: self.remain_mode, slip: self.slip,
            sync: self.sync, master: self.master, zoom_grid_mode: self.zoom_grid_mode,
            source_link: self.source_link, phase_ticks_view: self.phase_ticks_view,
            linked: beat2_player > 0, master_player: beat2_player as u8,
        };
        let snap = make_snapshot(&self.path, self.beat_grid.as_ref(), &self.audio, flags, pos, playing, speed, fader_speed, beat2_bpm, beat2_phase_beats);

        // Screen layout in logical points; the shader gets its two rects in pixels.
        let ppp  = window.scale_factor() as f32;
        let size = window.inner_size();
        let lay  = screen::layout(egui::Vec2::new(size.width as f32 / ppp, size.height as f32 / ppp));
        let px   = |r: egui::Rect| [r.min.x * ppp, r.min.y * ppp, r.width() * ppp, r.height() * ppp];
        let vp   = renderer::Viewports { wave: px(lay.wave), overview: px(lay.overview), dim_played: self.remain_mode };

        // Build egui overlay.
        let raw = egui_state.take_egui_input(window.as_ref());
        let mut touch = Vec::new();
        let mut output = self.egui_ctx.run(raw, |ctx| screen::draw(ctx, &snap, &lay, &mut touch));
        drop(snap);
        self.events.append(&mut touch);
        let pending = std::mem::take(&mut self.events);
        for ev in pending {
            self.apply(ev);
        }

        let platform_output = std::mem::take(&mut output.platform_output);
        let (renderer, egui_state, window) = match (
            self.renderer.as_mut(),
            self.egui_state.as_mut(),
            self.window.as_ref(),
        ) {
            (Some(r), Some(s), Some(w)) => (r, s, w),
            _ => return,
        };
        egui_state.handle_platform_output(window.as_ref(), platform_output);
        let flags = UiFlags {
            key_lock: self.key_lock, remain_mode: self.remain_mode, slip: self.slip,
            sync: self.sync, master: self.master, zoom_grid_mode: self.zoom_grid_mode,
            source_link: self.source_link, phase_ticks_view: self.phase_ticks_view,
            linked: beat2_player > 0, master_player: beat2_player as u8,
        };
        let snap = make_snapshot(&self.path, self.beat_grid.as_ref(), &self.audio, flags, pos, playing, speed, fader_speed, beat2_bpm, beat2_phase_beats);

        // Dev: OPENDECK_SCREENSHOT=path captures frame 90 and exits.
        self.frame_count += 1;
        if self.frame_count == 90 {
            if let Ok(path) = std::env::var("OPENDECK_SCREENSHOT") {
                renderer.request_capture(path.into());
                self.exit_after_capture = true;
            }
        }
        renderer.render(&snap, &vp, &self.egui_ctx, output);
    }
}

impl ApplicationHandler for DeckApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // already initialised (e.g. Android resume)
        }

        let attrs = WindowAttributes::default()
            .with_title("freedj-3000")
            .with_inner_size(winit::dpi::LogicalSize::new(1024u32, 600u32));   // XDJ-1000MK2 7" panel

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        // The display's refresh period is the unit the playhead advances in.
        // On Wayland `current_monitor()` is None until the surface has entered
        // an output, which is after this point; fall back to the first monitor.
        let monitor = window.current_monitor().or_else(|| window.available_monitors().next());
        if let Some(mhz) = monitor.and_then(|m| m.refresh_rate_millihertz()) {
            self.refresh_interval = Duration::from_secs_f64(1000.0 / mhz as f64);
            log::info!("display refresh {:.3} Hz → {:.3} ms per frame",
                       mhz as f64 / 1000.0, self.refresh_interval.as_secs_f64() * 1000.0);
        } else {
            log::warn!("display refresh rate unknown — assuming 60 Hz");
        }

        let renderer =
            pollster::block_on(Renderer::new(Arc::clone(&window), &self.waveform))
                .expect("failed to create renderer");

        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            self.egui_ctx.viewport_id(),
            &*window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        self.window     = Some(Arc::clone(&window));
        self.renderer   = Some(renderer);
        self.egui_state = Some(egui_state);

        // Kick off the first redraw.
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id:  winit::window::WindowId,
        event:       WindowEvent,
    ) {
        // Forward all events to egui first.
        if let (Some(state), Some(window)) = (&mut self.egui_state, &self.window) {
            let resp = state.on_window_event(window.as_ref(), &event);
            if resp.repaint {
                window.request_redraw();
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Pressed, .. },
                ..
            } => {
                use KeyCode::*;
                let playing = self.audio.playing.load(Ordering::Relaxed);
                let total   = self.audio.samples.len().max(1) as f32;
                let frac    = self.audio.position.load(Ordering::Relaxed) as f32 / total;
                let ten_s   = (self.audio.sample_rate as f32 * self.audio.channels as f32 * 10.0) / total;
                let fader   = input::speed_to_fader(f32::from_bits(self.fader_speed.load(Ordering::Relaxed)));
                let step    = 0.01 / (2.0 * input::TEMPO_RANGE);   // ±1% per press, as the DJ2Go buttons
                let ev = match code {
                    Space      => Some(Event::Deck(if playing { ControlEvent::Pause } else { ControlEvent::Play })),
                    ArrowRight => Some(Event::Deck(ControlEvent::NeedleSearch { position: (frac + ten_s).min(1.0) })),
                    ArrowLeft  => Some(Event::Deck(ControlEvent::NeedleSearch { position: (frac - ten_s).max(0.0) })),
                    Equal | NumpadAdd      => Some(Event::Deck(ControlEvent::TempoFader { position: fader + step })),
                    Minus | NumpadSubtract => Some(Event::Deck(ControlEvent::TempoFader { position: fader - step })),
                    Digit0 | Numpad0       => Some(Event::Deck(ControlEvent::TempoFader { position: 0.5 })),
                    KeyK => Some(Event::Deck(ControlEvent::KeyLockToggle)),
                    KeyS => Some(Event::Deck(ControlEvent::SlipToggle)),
                    KeyY => Some(Event::Deck(ControlEvent::SyncToggle)),
                    KeyM => Some(Event::Deck(ControlEvent::MasterRequest)),
                    KeyT => Some(Event::Ui(UiEvent::TimeMode)),
                    KeyP => Some(Event::Ui(UiEvent::PhaseMeterView)),
                    KeyC => Some(Event::Ui(UiEvent::CycleColor)),
                    KeyZ => Some(Event::Ui(UiEvent::ZoomStep(1))),
                    KeyX => Some(Event::Ui(UiEvent::ZoomStep(-1))),
                    Escape | KeyQ => { event_loop.exit(); None }
                    _ => None,
                };
                if let Some(ev) = ev {
                    self.events.push(ev);
                    if let Some(w) = &self.window { w.request_redraw(); }
                }
            }

            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }

            WindowEvent::RedrawRequested => {
                self.render_frame();
                if self.exit_after_capture {
                    event_loop.exit();
                    return;
                }
                // Ask for the next frame immediately.  On Wayland winit defers
                // this to the compositor's frame callback, so redraws arrive
                // phase-locked to the display instead of to a CPU timer.  The
                // previous WaitUntil(last_render + 16.667ms) pacing was not
                // locked to anything: measured over 400 frames, only 59% were
                // rendered within 2ms of the vsync period — 17 were rendered
                // <2ms after the previous one and 50 arrived 20–65ms late.
                // The compositor shows those twice or skips them.  No amount of
                // position smoothing can fix a frame that lands in the wrong
                // vsync slot.
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Nothing to do between events — redraws are requested from the
        // RedrawRequested handler and paced by the compositor.
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .init();

    let path: PathBuf = std::env::args()
        .nth(1)
        .context("usage: opendeck <path/to/file.mp3>")?
        .into();

    if !path.exists() {
        bail!("file not found: {}", path.display());
    }

    // ── 1. Decode audio ───────────────────────────────────────────────────────
    let audio = AudioHandle::open(&path)?;

    // ── 2. Build waveform + detect beat grid (synchronous, before window opens) ─
    log::info!("computing waveform ({} samples)...", audio.samples.len());
    let t0 = Instant::now();
    let mut waveform_builder = WaveformBuilder::new(audio.sample_rate);
    let mut beat_analyzer    = BeatAnalyzerImpl::new(audio.sample_rate);
    waveform_builder.push(&audio.samples);
    beat_analyzer.push(&audio.samples, audio.sample_rate);
    let waveform  = waveform_builder.finish();
    let beat_grid = beat_analyzer.beat_grid().map(|g| (*g).clone());
    match &beat_grid {
        Some(g) => log::info!(
            "waveform done: {} columns, {:.1} BPM (confidence {:.2}) in {:.1}s",
            waveform.len(), g.bpm, g.confidence, t0.elapsed().as_secs_f32()
        ),
        None => log::info!(
            "waveform done: {} columns, BPM detection failed in {:.1}s",
            waveform.len(), t0.elapsed().as_secs_f32()
        ),
    }

    // ── 3. Create second beat grid state ─────────────────────────────────────
    let base_bpm     = beat_grid.as_ref().map(|g| g.bpm as f32).unwrap_or(120.0);
    let fader_speed  = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    let beat2_bpm    = Arc::new(AtomicU32::new(base_bpm.to_bits()));
    let beat2_anchor = Arc::new(AtomicU64::new(0));
    let beat2_player = Arc::new(AtomicU32::new(0));

    // ── 4. Start ProDJ Link listener (optional — app runs fine without it) ────────
    let _prodj = prodj::ProDjHandle::listen(
        Arc::clone(&beat2_bpm),
        Arc::clone(&beat2_anchor),
        Arc::clone(&beat2_player),
    );

    // ── 5. Connect MIDI controller (optional — app runs fine without it) ──────────
    let _midi = midi::MidiHandle::connect(
        Arc::clone(&audio.playing),
        Arc::clone(&audio.position),
        Arc::clone(&audio.speed),
        Arc::clone(&fader_speed),
        audio.sample_rate,
        audio.channels,
        audio.samples.len(),
        Arc::clone(&beat2_bpm),
        Arc::clone(&beat2_anchor),
        base_bpm,
    );

    // ── 6. Run the UI event loop ──────────────────────────────────────────────
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = DeckApp::new(path, waveform, audio, beat_grid, fader_speed, beat2_bpm, beat2_anchor, beat2_player);
    event_loop.run_app(&mut app).context("event loop error")?;

    Ok(())
}
