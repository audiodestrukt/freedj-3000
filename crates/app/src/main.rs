//! OpenDeck MVP — play an MP3 with waveform visualization.
//!
//! Usage:  opendeck <path/to/file.mp3>
//!
//! Controls:
//!   Space    — play / pause
//!   ← / →   — seek ±10 seconds
//!   Esc / Q  — quit

mod audio;
mod browser;
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
use input::{ControlEvent, Event, Screen as TopScreen, Source, UiEvent, ZOOM_LEVELS, ZOOM_DEFAULT};
use browser::{Browser, Enter, Load};
use snapshot::DeckSnapshot;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
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

/// Which middle-band mode the deck screen is showing.  PERFORM comes later.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScreenMode { Playback, Browse }

struct DeckApp {
    // Provided before event loop starts.
    path:         PathBuf,
    waveform:     WaveformCache,
    audio:        AudioHandle,
    beat_grid:    Option<BeatGrid>,
    screen_mode:  ScreenMode,
    browser:      Browser,

    // Second beat grid — tempo controlled by Deck B on the MIDI controller.
    fader_speed:  Arc<AtomicU32>,  // f32 bits; pitch-fader speed (no jog nudge)
    beat2_bpm:    Arc<AtomicU32>,  // f32 bits; BPM of the second grid
    beat2_anchor: Arc<AtomicU64>, // written by MIDI Cue B to signal a phase reset
    beat2_player: Arc<AtomicU32>, // player number of the last Link beat sender
    beat2_bib:    Arc<AtomicU32>, // external deck's beat within its bar (1-4, 0=unknown)
    link:         Arc<prodj::LinkState>,
    /// Shared with the ProDJ sender; updated on load so Link sync/broadcast use
    /// the current track's grid.
    link_grid:    Arc<arc_swap::ArcSwap<Option<BeatGrid>>>,
    /// Enable ProDJ Link sending (beats/status/master); set true by MASTER.
    link_send:    Arc<AtomicBool>,
    beat2_start:  Instant,        // wall-clock time of the last phase reset
    prev_beat2_anchor: u64,       // detect changes in beat2_anchor
    prev_beat2_bpm:    f32,       // detect BPM changes for logging
    prev_pos:          u64,       // previous frame's audio position (scroll instrument)
    smoothed_pos:      f64,       // phase-locked playhead, source samples
    resync_frames:     u32,       // consecutive frames the reference has been far off
    refresh_interval:  Duration,  // display period; each frame lands in one of these
    frame_count:       u64,
    remain_mode:       bool,      // time display: REMAIN vs TIME
    key_lock:          bool,      // MT indicator (Rubber Band path is always on today)
    slip:              bool,
    zoom_level:        usize,     // index into ZOOM_LEVELS
    zoom_grid_mode:    bool,
    source_link:       bool,
    phase_ticks_view:  bool,
    /// Render the full physical faceplate (jog, fader, buttons) around the
    /// screen.  Off by default — the Pi/hardware target runs screen-only.
    faceplate:         bool,
    /// Same-thread sources (keyboard, touch) push here.
    events:            Vec<Event>,
    /// Off-thread sources (MIDI, later HID/serial) send here; drained per frame.
    event_rx:          mpsc::Receiver<Event>,
    /// Jog nudge: a temporary speed offset that snaps back when the wheel stops.
    jog_offset:        f32,
    jog_until:         Option<Instant>,
    cue_point:         u64,   // start cue, source sample index (CDJ CUE)
    cue_preview:       bool,  // CUE held → previewing from the cue point
    cued:              bool,  // playhead is sitting on the cue (not searched away)
    exit_after_capture: bool,

    // Created on first `resumed`.
    window:      Option<Arc<Window>>,
    renderer:    Option<Renderer>,
    egui_ctx:    egui::Context,
    egui_state:  Option<egui_winit::State>,

    /// Time of the last rendered frame, used to cap to FRAME_INTERVAL.
    last_render: Instant,
    /// Wall-clock cost of the previous frame's render_frame body, so a spike
    /// detector can split a long inter-frame gap into "our code was slow" vs
    /// "the scheduler/compositor didn't wake us".
    last_frame_total: Duration,
    /// Count of inter-frame gaps that exceeded the spike threshold.
    frame_spikes: u64,
    /// OPENDECK_PACE=hybrid: add a safety-net timer so a late/missing compositor
    /// frame callback doesn't freeze the UI — we self-drive a frame instead.
    hybrid_pace: bool,
    /// Last-seen audio glitch counters, to log only on change.
    prev_underruns: u64,
    prev_drops:     u64,
}

/// Display/deck flags copied out of DeckApp so a snapshot can borrow only
/// `path`, `beat_grid` and `audio` — leaving `renderer` free to be borrowed
/// mutably in the same frame.
#[derive(Clone, Copy)]
struct UiFlags {
    key_lock: bool, remain_mode: bool, slip: bool, sync: bool, master: bool,
    zoom_grid_mode: bool, source_link: bool, phase_ticks_view: bool, linked: bool, master_player: u8,
    cue_point: u64,
}

#[allow(clippy::too_many_arguments)]
fn make_snapshot<'a>(
    path: &'a std::path::Path, beat_grid: Option<&'a BeatGrid>, audio: &AudioHandle, f: UiFlags,
    pos: u64, playing: bool, speed: f32, fader_speed: f32, beat2_bpm: f32, beat2_phase_beats: f32, beat2_beat_in_bar: u8,
) -> DeckSnapshot<'a> {
    DeckSnapshot {
        title:         path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown"),
        position:      pos,
        sample_rate:   audio.sample_rate,
        channels:      audio.channels,
        total_samples: audio.len() as u64,
        cue_point:     f.cue_point,
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
        beat2_beat_in_bar,
    }
}

// ── Render-thread phase profiler (debug only) ─────────────────────────────────
// Accumulates per-phase timings on the render thread and logs a rolling average
// every 120 frames, so we can see whether the egui closure, tessellation, or GPU
// submit dominates — without per-frame log spam skewing the numbers.
thread_local! {
    static PERF: std::cell::RefCell<std::collections::BTreeMap<&'static str, (f64, u32)>> =
        std::cell::RefCell::new(std::collections::BTreeMap::new());
    static PERF_N: std::cell::Cell<u32> = std::cell::Cell::new(0);
}
pub fn perf_accum(phase: &'static str, dt: std::time::Duration) {
    if !log::log_enabled!(log::Level::Debug) { return; }
    PERF.with(|m| {
        let mut m = m.borrow_mut();
        let e = m.entry(phase).or_insert((0.0, 0));
        e.0 += dt.as_secs_f64() * 1000.0; e.1 += 1;
    });
}
pub fn perf_tick() {
    if !log::log_enabled!(log::Level::Debug) { return; }
    let n = PERF_N.with(|c| { let v = c.get() + 1; c.set(v); v });
    if n % 120 != 0 { return; }
    PERF.with(|m| {
        let mut m = m.borrow_mut();
        let parts: Vec<String> = m.iter()
            .map(|(k, (sum, cnt))| format!("{k} {:.2}ms", if *cnt > 0 { sum / *cnt as f64 } else { 0.0 }))
            .collect();
        log::debug!("perf: {}", parts.join("  "));
        m.clear();
    });
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
        beat2_bib:    Arc<AtomicU32>,
        link:         Arc<prodj::LinkState>,
        link_grid:    Arc<arc_swap::ArcSwap<Option<BeatGrid>>>,
        link_send:    Arc<AtomicBool>,
        event_rx:     mpsc::Receiver<Event>,
    ) -> Self {
        let browser = Browser::new(&path, std::sync::Arc::clone(&link));
        let cue_point = std::env::var("OPENDECK_CUE").ok().and_then(|v| v.parse::<f64>().ok())
            .map(|secs| (secs * audio.sample_rate as f64 * audio.channels as f64) as u64).unwrap_or(0);
        Self {
            path,
            waveform,
            audio,
            beat_grid,
            screen_mode: if std::env::var("OPENDECK_SCREEN").as_deref() == Ok("browse") { ScreenMode::Browse } else { ScreenMode::Playback },
            browser,
            fader_speed,
            beat2_bpm,
            beat2_anchor,
            beat2_player,
            beat2_bib,
            link,
            link_grid,
            link_send,
            beat2_start:       Instant::now(),
            prev_beat2_anchor: 0,
            prev_beat2_bpm:    0.0,
            prev_pos:          0,
            smoothed_pos:      0.0,
            resync_frames:     0,
            refresh_interval:  FRAME_INTERVAL,
            frame_count:       0,
            remain_mode:       false,   // the reference unit was in TIME mode
            key_lock:          true,
            slip:              false,
            zoom_level:        ZOOM_DEFAULT,
            zoom_grid_mode:    false,
            source_link:       false,
            // Dev: OPENDECK_PHASE_VIEW=ticks starts in the alignment view (for captures).
            phase_ticks_view:  std::env::var("OPENDECK_PHASE_VIEW").map(|v| v == "ticks").unwrap_or(false),
            faceplate:         std::env::var("OPENDECK_FACEPLATE").map(|v| v == "1").unwrap_or(false),
            events:            Vec::new(),
            event_rx,
            jog_offset:        0.0,
            jog_until:         None,
            cue_point,
            cue_preview:       false,
            cued:              true,
            exit_after_capture: false,
            window:      None,
            renderer:    None,
            egui_ctx:    egui::Context::default(),
            egui_state:  None,
            last_render: Instant::now(),
            last_frame_total: Duration::ZERO,
            frame_spikes: 0,
            hybrid_pace: std::env::var("OPENDECK_PACE").map(|v| v == "hybrid").unwrap_or(false),
            prev_underruns: 0,
            prev_drops:     0,
        }
    }

    /// The only place input changes deck state.  Every source — keyboard,
    /// touch, later MIDI and scripts — ends up here.
    fn apply(&mut self, ev: Event) {
        log::debug!("event: {ev:?}");
        match ev {
            Event::Deck(ControlEvent::Play)  => { self.lock_in_play(); log::info!("playing"); }
            Event::Deck(ControlEvent::Pause) => { self.audio.playing.store(false, Ordering::Relaxed); log::info!("paused"); }
            Event::Deck(ControlEvent::PlayPause) => {
                if self.cue_preview {
                    // XDJ: pressing PLAY while CUE is held for a preview latches
                    // continuous playback — it does NOT toggle to pause.  After
                    // this, releasing CUE keeps playing instead of returning.
                    self.lock_in_play();
                    log::info!("play locked in from cue preview");
                } else {
                    let was = self.audio.playing.load(Ordering::Relaxed);
                    self.audio.playing.store(!was, Ordering::Relaxed);
                    log::info!("{}", if was { "paused" } else { "playing" });
                }
            }
            Event::Deck(ControlEvent::TempoNudge { delta }) => {
                let step = delta / (2.0 * input::TEMPO_RANGE);
                let f = input::speed_to_fader(f32::from_bits(self.fader_speed.load(Ordering::Relaxed)));
                self.apply(Event::Deck(ControlEvent::TempoFader { position: f + step }));
            }
            Event::Deck(ControlEvent::JogDelta { delta, .. }) => {
                if self.audio.playing.load(Ordering::Relaxed) {
                    // PLAYING → nudge: a temporary speed offset that snaps back
                    // when the wheel goes idle.  Overrides SYNC's phase nudge
                    // while active, as a CDJ does when you touch the jog synced.
                    // (The DJ2Go jog is not touch-sensitive, so play state, not a
                    // touch sensor, selects the mode — see docs/INPUT_PLAN.md.)
                    const NUDGE_PER_TICK: f32 = 0.002;
                    self.jog_offset = (self.jog_offset + delta as f32 * NUDGE_PER_TICK).clamp(-0.5, 0.5);
                    self.jog_until  = Some(Instant::now() + Duration::from_millis(150));
                    let f = f32::from_bits(self.fader_speed.load(Ordering::Relaxed));
                    let spd = (f + self.jog_offset).clamp(0.25, 4.0);
                    self.audio.speed.store(spd.to_bits(), Ordering::Relaxed);
                    log::debug!("jog nudge {delta:+} → {spd:.3}×");
                } else {
                    // PAUSED → vinyl: the wheel scrubs the playhead through the
                    // track (no scrub-audio yet — position + waveform only).
                    const VINYL_SECS_PER_TICK: f64 = 0.020;
                    let sr_ch = self.audio.sample_rate as f64 * self.audio.channels as f64;
                    let step  = (sr_ch * VINYL_SECS_PER_TICK) as i64;
                    let cur   = self.audio.position.load(Ordering::Relaxed) as i64;
                    let new   = (cur + delta as i64 * step).clamp(0, self.audio.len() as i64) as u64;
                    self.seek_to(new);
                    self.cued = false;
                    log::debug!("jog vinyl {delta:+} → {:.2}s", new as f64 / sr_ch);
                }
            }
            Event::Deck(ControlEvent::Cue { pressed }) => {
                // Momentary CDJ CUE.  PRESS: playing → return to the cue and pause;
                // paused → set the cue here and preview (play) while held.
                // RELEASE (while previewing) → jump back to the cue and pause.
                let sr_ch = self.audio.sample_rate as f64 * self.audio.channels as f64;
                if pressed {
                    if self.audio.playing.load(Ordering::Relaxed) {
                        // Playing → return to the cue and pause.
                        self.audio.playing.store(false, Ordering::Relaxed);
                        self.seek_to(self.cue_point);
                        self.cued = true;
                        log::info!("cue: return to {:.2}s", self.cue_point as f64 / sr_ch);
                    } else if self.cued {
                        // Paused, already sitting on the cue → preview from it while
                        // held; do NOT move the cue.  Rapid re-taps always retrigger
                        // the same point (never adopt a raced-forward position).
                        self.cue_preview = true;
                        self.seek_to(self.cue_point);
                        self.audio.playing.store(true, Ordering::Relaxed);
                        log::debug!("cue: preview from {:.2}s", self.cue_point as f64 / sr_ch);
                    } else {
                        // Paused after searching away → set a new cue here, then
                        // preview it while held.
                        //
                        // Capture the cue at the *displayed* playhead, not the raw
                        // decoder cursor.  `position` is the decode cursor, which
                        // sits ~in_flight (≈93 ms) ahead of what is actually heard
                        // and drawn (`smoothed_pos ≈ position − in_flight`).  Using
                        // the raw cursor stored the cue ~93 ms past the transient
                        // under the playhead, so playing from it skipped the kick.
                        // Frame-align so channels stay interleaved.
                        let ch = self.audio.channels as u64;
                        self.cue_point   = ((self.smoothed_pos.max(0.0) as u64) / ch) * ch;
                        self.cue_preview = true;
                        self.cued        = true;
                        self.audio.playing.store(true, Ordering::Relaxed);
                        log::info!("cue: set + preview at {:.2}s", self.cue_point as f64 / sr_ch);
                    }
                } else if self.cue_preview {
                    self.cue_preview = false;
                    self.audio.playing.store(false, Ordering::Relaxed);
                    self.seek_to(self.cue_point);
                    self.cued = true;
                    log::debug!("cue: release → back to cue");
                }
            }
            Event::Deck(ControlEvent::NeedleSearch { position }) => {
                let total = self.audio.len() as f64;
                // Land on a frame boundary so channels stay interleaved.
                let ch = self.audio.channels as u64;
                let target = ((position.clamp(0.0, 1.0) as f64 * total) as u64 / ch) * ch;
                self.seek_to(target);
                self.cued = false;   // searched away from the cue
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
            Event::Deck(ControlEvent::SyncToggle) => {
                let on = !self.link.sync.load(Ordering::Relaxed);
                self.link.sync.store(on, Ordering::Relaxed);
                if !on {
                    // Leaving SYNC: keep the tempo we ended on, drop any nudge.
                    let f = self.fader_speed.load(Ordering::Relaxed);
                    self.audio.speed.store(f, Ordering::Relaxed);
                }
                log::info!("sync {}", if on { "on" } else { "off" });
            }
            Event::Deck(ControlEvent::MasterRequest) => {
                if self.link.master.load(Ordering::Relaxed) {
                    log::info!("already master");
                } else {
                    // Enable Link sending: you can't lead without broadcasting
                    // beats/status. Off by default (pure follower) until MASTER.
                    if !self.link_send.swap(true, Ordering::Relaxed) {
                        log::info!("ProDJ Link send enabled (needed to take master)");
                    }
                    self.link.want_master.store(true, Ordering::Relaxed);
                    log::info!("requesting master");
                }
            }
            Event::Deck(ControlEvent::BrowseEncoderDelta { delta }) => {
                if self.screen_mode == ScreenMode::Browse {
                    self.browser.move_selection(delta);
                } else {
                    // Outside the browser the selector nudges zoom, as on the unit.
                    self.apply(Event::Ui(UiEvent::ZoomStep(delta.signum())));
                }
            }
            Event::Deck(ControlEvent::Load) => {
                if self.screen_mode != ScreenMode::Browse {
                    self.screen_mode = ScreenMode::Browse;
                    self.browser.refresh();
                } else {
                    match self.browser.enter() {
                        Enter::Folder  => {}                       // descended; stay in browser
                        Enter::Nothing => {}
                        Enter::Track(load) => match self.load_selected(load) {
                            Ok(())  => self.screen_mode = ScreenMode::Playback,
                            Err(e)  => log::warn!("load failed: {e:#}"),
                        },
                    }
                }
            }
            Event::Deck(ControlEvent::Back) => {
                if self.screen_mode == ScreenMode::Browse { self.browser.back(); }
            }
            Event::Deck(ControlEvent::LoopIn)  => log::info!("loop in (no loop engine yet)"),
            Event::Deck(ControlEvent::LoopOut) => log::info!("loop out (no loop engine yet)"),
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
            Event::Ui(UiEvent::Screen(sc)) => match sc {
                TopScreen::Browse => {
                    self.screen_mode = if self.screen_mode == ScreenMode::Browse {
                        ScreenMode::Playback
                    } else {
                        self.browser.refresh();
                        ScreenMode::Browse
                    };
                    log::info!("screen → {:?}", self.screen_mode);
                }
                other => log::info!("{other:?} screen not implemented"),
            },
        }
    }

    /// Move the playhead to a source-sample position, keeping the visual
    /// playhead and the audible-position estimate in sync so it tracks crisply
    /// (used by vinyl-scrub and CUE).  in_flight is zeroed — while paused
    /// nothing is queued, and on resume the processor re-derives it.
    /// Latch continuous playback, cancelling any momentary CUE preview.  After
    /// this, releasing CUE no longer jumps back to the cue point — the deck just
    /// keeps playing (the XDJ "hold CUE, tap PLAY to lock in" gesture).  Also the
    /// plain PLAY action, which should always mean sustained play.
    fn lock_in_play(&mut self) {
        self.cue_preview = false;   // release won't return to the cue
        self.cued        = false;   // playhead is leaving the cue
        self.audio.playing.store(true, Ordering::Relaxed);
    }

    fn seek_to(&mut self, pos: u64) {
        let pos = pos.min(self.audio.len() as u64);
        // The seek_request channel is what the processor actually acts on; it
        // can't be clobbered by the processor's own progress store (which lost
        // seeks and made CUE sometimes not return).  `position` is set too, as
        // an immediate UI hint and so a paused cue-set reads the sought spot.
        self.audio.seek_request.store(pos, Ordering::Relaxed);
        self.audio.position.store(pos, Ordering::Relaxed);
        self.audio.in_flight.store(0, Ordering::Relaxed);
        self.smoothed_pos = pos as f64;
        self.prev_pos     = pos;
    }

    /// Decode, analyse, and swap in a new track selected in the browser.
    /// Blocks the render thread for the decode + waveform/beat analysis
    /// (~1-2 s on a Pi) — acceptable for a LOAD, as a CDJ spins up briefly;
    /// can move to a worker thread later.  Leaves the deck paused at the start,
    /// as a CDJ does after loading.
    /// Convert a rekordbox ANLZ file into a constant BeatGrid (anchored on its
    /// first beat) and a start-cue sample index (first memory cue, interleaved).
    /// Returns None if the file has no beat grid.
    fn grid_from_anlz(anlz: &std::path::Path, deck_sr: u32, ch: u8) -> Option<(BeatGrid, u64)> {
        let a = opendeck_rekordbox::read_anlz(anlz)
            .map_err(|e| log::warn!("ANLZ {}: {e:#}", anlz.display())).ok()?;
        Self::anlz_to_grid(a, deck_sr, ch)
    }

    /// Same as `grid_from_anlz` but from ANLZ bytes read over the network.
    fn grid_from_anlz_bytes(bytes: &[u8], deck_sr: u32, ch: u8) -> Option<(BeatGrid, u64)> {
        let a = opendeck_rekordbox::read_anlz_from(&mut std::io::Cursor::new(bytes.to_vec()))
            .map_err(|e| log::warn!("ANLZ (link): {e:#}")).ok()?;
        Self::anlz_to_grid(a, deck_sr, ch)
    }

    fn anlz_to_grid(a: opendeck_rekordbox::RbAnalysis, deck_sr: u32, ch: u8) -> Option<(BeatGrid, u64)> {
        let first = *a.beats.first()?;
        let anchor = (first.time_ms as u64 * deck_sr as u64) / 1000;      // frames
        let mut grid = BeatGrid::new_constant(anchor, first.bpm as f64);
        grid.downbeat_offset = first.beat_in_bar.saturating_sub(1) % 4;
        grid.confidence = 1.0;
        let cue = a.memory_cues.first()
            .map(|c| (c.time_ms as u64 * deck_sr as u64) / 1000 * ch as u64)  // interleaved
            .unwrap_or(0);
        Some((grid, cue))
    }

    /// Dispatch a browser selection: a local file, or a track on a linked player.
    fn load_selected(&mut self, load: Load) -> Result<()> {
        match load {
            Load::Local { path, analyze } => self.load_track(&path, analyze.as_deref()),
            Load::Link { ip, rel_path, analyze_rel } => self.load_track_link(ip, &rel_path, &analyze_rel),
        }
    }

    /// Load a local file: decode from disk, grid from its ANLZ if given.
    fn load_track(&mut self, path: &std::path::Path, analyze: Option<&std::path::Path>) -> Result<()> {
        let t0 = Instant::now();
        let (samples, sr, ch) = audio::decode_file(path)?;
        let deck_sr = self.audio.sample_rate;
        let grid_cue = analyze.and_then(|p| Self::grid_from_anlz(p, deck_sr, ch as u8));
        self.path = path.to_path_buf();
        self.finish_load(&path.display().to_string(), samples, sr, ch, grid_cue, t0)
    }

    /// Load a track from a linked player over NFS: pull the audio + ANLZ off the
    /// wire, decode from memory.  Blocks the UI for the fetch (a few seconds for
    /// a multi-MB read at 8 KB/NFS-read — non-blocking load is #19).
    fn load_track_link(&mut self, ip: std::net::Ipv4Addr, rel_path: &str, analyze_rel: &str) -> Result<()> {
        let t0 = Instant::now();
        let mut nfs = opendeck_nfs::Nfs::connect(ip)?;
        let root = nfs.mount_usb()?;
        let (fh, size) = nfs.lookup_path(&root, rel_path)?;
        let audio = nfs.read_file(&fh, size)?;
        let (samples, sr, ch) = audio::decode_bytes(audio, rel_path.rsplit('.').next())?;
        let deck_sr = self.audio.sample_rate;
        let grid_cue = if analyze_rel.is_empty() {
            None
        } else {
            match nfs.lookup_path(&root, analyze_rel).and_then(|(afh, asz)| nfs.read_file(&afh, asz)) {
                Ok(bytes) => Self::grid_from_anlz_bytes(&bytes, deck_sr, ch as u8),
                Err(e) => { log::warn!("link ANLZ {analyze_rel}: {e:#}"); None }
            }
        };
        self.path = std::path::PathBuf::from(rel_path);
        self.finish_load(rel_path, samples, sr, ch, grid_cue, t0)
    }

    /// Shared load tail: resample to the deck rate, build the waveform, apply the
    /// grid (given, or detect a fallback), swap the samples in, reset transport.
    fn finish_load(&mut self, name: &str, samples: Vec<f32>, sr: u32, ch: usize,
                   grid_cue: Option<(BeatGrid, u64)>, t0: Instant) -> Result<()> {
        let deck_sr = self.audio.sample_rate;
        if ch as u8 != self.audio.channels {
            bail!("track has {} channels but the deck runs {} — channel conversion not yet implemented",
                  ch, self.audio.channels);
        }
        // Offline SRC so a differently-sampled track plays at the right pitch.
        let samples = if sr != deck_sr {
            audio::resample_interleaved(&samples, ch, sr, deck_sr)?
        } else { samples };
        let samples = Arc::new(samples);

        let mut wb = WaveformBuilder::new(deck_sr);
        wb.push(&samples);
        let waveform = wb.finish();

        // Prefer rekordbox's grid+cue; fall back to freedj's detector otherwise.
        let (beat_grid, cue_pt, grid_src) = match grid_cue {
            Some((grid, cue)) => (Some(grid), cue, "rekordbox"),
            None => {
                let mut ba = BeatAnalyzerImpl::new(deck_sr);
                ba.push(&samples, deck_sr);
                (ba.beat_grid().map(|g| (*g).clone()), 0, "freedj")
            }
        };

        self.audio.load_samples(Arc::clone(&samples), deck_sr, ch as u8)?;
        if let Some(r) = self.renderer.as_mut() { r.set_waveform(&waveform); }
        self.waveform     = waveform;
        self.beat_grid    = beat_grid;
        // Keep the ProDJ Link sender's grid in step with the loaded track, so
        // SYNC divides the master BPM by the CURRENT track's BPM (not the one
        // loaded at startup) and we broadcast our real tempo.
        self.link_grid.store(Arc::new(self.beat_grid.clone()));
        self.smoothed_pos = 0.0;
        self.prev_pos     = 0;
        self.cue_point    = cue_pt;
        self.cue_preview  = false;
        self.cued         = true;
        log::info!(
            "loaded {} in {:.1}s ({} BPM, {} grid, cue {:.2}s)",
            name, t0.elapsed().as_secs_f32(),
            self.beat_grid.as_ref().map(|g| format!("{:.1}", g.bpm)).unwrap_or_else(|| "?".into()),
            grid_src, cue_pt as f64 / (deck_sr as f64 * ch as f64),
        );
        Ok(())
    }

    fn render_frame(&mut self) {
        // Dev: OPENDECK_AUTOLOAD=path loads that track once at ~frame 150, to
        // exercise the runtime reload path headlessly (before any borrows).
        if self.frame_count == 150 {
            if let Ok(path) = std::env::var("OPENDECK_AUTOLOAD") {
                match self.load_track(std::path::Path::new(&path), None) {
                    Ok(())  => { self.audio.playing.store(true, Ordering::Relaxed); log::info!("autoload ok"); }
                    Err(e)  => log::error!("autoload failed: {e:#}"),
                }
            }
        }
        let frame_start = Instant::now();
        let frame_dt    = frame_start.duration_since(self.last_render);
        self.last_render = frame_start;

        // ── Frame-spike detector ──────────────────────────────────────────────
        // A UI hitch is an inter-frame gap much longer than the display period.
        // Split it: `last_frame_total` is how long our render body took last
        // frame; the rest of the gap is idle time the OS didn't schedule us.
        // That distinguishes "our code / the GPU stalled" from "the scheduler or
        // compositor didn't wake us" (the OS-level cause the user suspects).
        // Always on (INFO) but only fires on a genuine spike, so a normal run is
        // quiet and a hitch prints one attributed line.
        let refresh = self.refresh_interval.as_secs_f64();
        let dt_s    = frame_dt.as_secs_f64();
        if self.frame_count > 60 && dt_s > refresh * 2.5 {
            self.frame_spikes += 1;
            let ours = self.last_frame_total.as_secs_f64();
            let idle = (dt_s - ours).max(0.0);
            let verdict = if ours > refresh * 1.5 {
                "STALL INSIDE render (our code / GPU submit / present)"
            } else {
                "stall OUTSIDE render (scheduler / compositor / vsync miss)"
            };
            log::info!(
                "frame spike #{}: gap {:.1}ms (~{:.1} refreshes)  prev-render {:.1}ms  idle {:.1}ms → {}",
                self.frame_spikes, dt_s * 1000.0, dt_s / refresh, ours * 1000.0, idle * 1000.0, verdict,
            );
        }

        // ── Audio-integrity watch ─────────────────────────────────────────────
        // Surface RT-path glitches the moment they happen.  An underrun is a
        // real dropout (the callback played silence because the ring was empty);
        // a dropped frame is the producer overrunning a full ring.  Either is a
        // click a performance deck must never hide — logged at WARN on change.
        let underruns = self.audio.stats.underrun_events.load(Ordering::Relaxed);
        if underruns != self.prev_underruns {
            let samples = self.audio.stats.underrun_samples.load(Ordering::Relaxed);
            let ms = samples as f64 / (self.audio.sample_rate as f64 * self.audio.channels as f64) * 1000.0;
            log::warn!(
                "AUDIO UNDERRUN: {} events, {} samples of silence ({:.1} ms total) — ring starved",
                underruns, samples, ms,
            );
            self.prev_underruns = underruns;
        }
        let drops = self.audio.stats.dropped_frames.load(Ordering::Relaxed);
        if drops != self.prev_drops {
            log::warn!("AUDIO OVERRUN: producer dropped {} frames (ring full)", drops);
            self.prev_drops = drops;
        }

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

        // A far-off reference is almost always a transient: `in_flight` is
        // sampled at decode-block boundaries and spikes for a frame whenever the
        // audio-proc thread wakes late, which momentarily craters
        // `heard = raw_pos - in_flight`.  Chasing that single sample is the
        // visible periodic "jump".  Genuine seeks don't come through here — they
        // reset smoothed_pos directly in seek_to — so the only thing this snap
        // ever follows is an unexpected jump we do NOT want on screen.  Require
        // the deviation to persist for a few frames before believing it; a
        // one-frame spike is ignored and the free-run carries through it.
        let far_off = (heard - self.smoothed_pos).abs() > sr_ch * 0.5;
        if far_off { self.resync_frames += 1; } else { self.resync_frames = 0; }
        if self.smoothed_pos <= 0.0 || self.resync_frames >= 4 {
            // First frame, or a sustained desync — snap and reset the filter.
            self.smoothed_pos  = heard;
            self.resync_frames = 0;
        } else if far_off {
            // Transient outlier: hold the free-run, don't poison the reference.
            let period  = self.refresh_interval.as_secs_f64();
            let periods = (frame_dt.as_secs_f64() / period).round().max(1.0);
            if playing {
                self.smoothed_pos += periods * period * sr_ch * speed as f64;
                self.smoothed_pos  = self.smoothed_pos.max(self.prev_pos as f64);
            }
        } else {
            // The phase-lock only runs while PLAYING.  When paused the audio is
            // silent (the cpal callback fills zeros without draining the ring),
            // so the playhead must simply hold where it stopped — running the
            // low-pass + pull while paused made it ease toward the reference for
            // ~1–2 s after STOP, the visible "drift".  While paused, smoothed_pos
            // holds its value (and is moved directly by seek_to for scrub/cue).
            // A proper spin-down (vinyl brake) replaces the hold later — see
            // docs/design/varispeed-engine.md.
            if playing {
                // Free-run at the true playback rate.  Advance by whole display
                // periods, not measured wall-clock: each frame is shown for an
                // integer number of refreshes and the CPU-side frame_dt carries
                // ±2 ms of wake-up slop.  This is the velocity model.
                let period  = self.refresh_interval.as_secs_f64();
                let periods = (frame_dt.as_secs_f64() / period).round().max(1.0);
                self.smoothed_pos += periods * period * sr_ch * speed as f64;

                // Phase-correct toward the FRESH heard position.  The free-run
                // above already supplies the velocity, so this pull only has to
                // null a constant phase offset — which means it has *zero*
                // steady-state lag on the moving playhead.  (The old code pulled
                // toward a 0.05-LPF of `heard`, τ≈0.33 s, which dragged the whole
                // playhead ~0.33 s behind the audio — the perceptible beat-counter
                // delay.)  The low gain still averages `heard`'s ±35 ms block
                // jitter (0.08·35 ≈ 3 ms, sub-pixel) without adding lag, because
                // the averaging is of the *offset*, not the position.
                self.smoothed_pos += (heard - self.smoothed_pos) * 0.08;

                // Never run backwards during playback: a stall reads as a hitch,
                // a reversal as a glitch (worse).
                self.smoothed_pos = self.smoothed_pos.max(self.prev_pos as f64);
            }
        }
        // ── End of track ──────────────────────────────────────────────────────
        // The decoder pins at the end and the ring buffer drains, so the last
        // sample becomes audible; without this the playhead free-runs past the
        // end into blank forever (it is clamped monotonic and cannot return),
        // which reads as the deck "stuck" and still playing.  Stop and pin at
        // the end, as a CDJ does in SINGLE mode.
        let total = self.audio.len() as f64;
        let decoded_to_end = raw_pos >= self.audio.len() as u64;
        let heard_to_end   = heard >= total - sr_ch * 0.05;   // within ~50 ms of the end
        if decoded_to_end && heard_to_end {
            self.smoothed_pos = total;
            if playing {
                self.audio.playing.store(false, Ordering::Relaxed);
                log::info!("end of track — stopped");
            }
        } else {
            self.smoothed_pos = self.smoothed_pos.min(total);   // never overshoot the end
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
        let beat2_bib_v  = self.beat2_bib.load(Ordering::Relaxed) as u8;

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
            sync: self.link.sync.load(Ordering::Relaxed), master: self.link.master.load(Ordering::Relaxed), zoom_grid_mode: self.zoom_grid_mode,
            source_link: self.source_link, phase_ticks_view: self.phase_ticks_view,
            linked: beat2_player > 0,
            // Master indicator (phase-ticks view): the real XDJ shows a flat
            // line (no number) when THIS deck is master, and another deck's
            // number only when following that deck.  So report 0 (→ "-") when we
            // hold master, else the tracked master player (also 0 → "-" if none).
            // No beat2_player fallback — that wrongly named the last-seen deck
            // (the XDJ) as master whenever master_player was 0.
            master_player: if self.link.master.load(Ordering::Relaxed) {
                0
            } else {
                self.link.master_player.load(Ordering::Relaxed) as u8
            },
            cue_point: self.cue_point,
        };
        let _t_snap = Instant::now();
        let snap = make_snapshot(&self.path, self.beat_grid.as_ref(), &self.audio, flags, pos, playing, speed, fader_speed, beat2_bpm, beat2_phase_beats, beat2_bib_v);
        perf_accum("make_snapshot", _t_snap.elapsed());

        // Screen layout in logical points; the shader gets its two rects in pixels.
        let ppp  = window.scale_factor() as f32;
        let size = window.inner_size();
        let win  = egui::Rect::from_min_size(egui::Pos2::ZERO,
                       egui::Vec2::new(size.width as f32 / ppp, size.height as f32 / ppp));
        // Faceplate mode renders the screen into a sub-rect and the deck body
        // around it; screen-only mode fills the whole window as before.
        let (screen_rect, face) = if self.faceplate {
            let (s, f) = screen::faceplate_layout(win);
            (s, Some(f))
        } else {
            (win, None)
        };
        let lay  = screen::layout(screen_rect);
        let px   = |r: egui::Rect| [r.min.x * ppp, r.min.y * ppp, r.width() * ppp, r.height() * ppp];
        let vp   = renderer::Viewports { wave: px(lay.wave), overview: px(lay.overview), dim_played: self.remain_mode };

        // Build egui overlay.
        let raw = egui_state.take_egui_input(window.as_ref());
        let mut touch = Vec::new();
        let _t_run = Instant::now();
        let browse = if self.screen_mode == ScreenMode::Browse { Some(&self.browser) } else { None };
        let face_ref = face.as_ref();
        let mut output = self.egui_ctx.run(raw, |ctx| screen::draw(ctx, &snap, &lay, browse, face_ref, &mut touch));
        perf_accum("egui_run", _t_run.elapsed());
        drop(snap);
        self.events.append(&mut touch);
        let mut pending = std::mem::take(&mut self.events);
        pending.extend(self.event_rx.try_iter());
        for ev in pending {
            self.apply(ev);
        }
        // Jog nudge snap-back: once the wheel has been still past the window,
        // return to the pitch-fader speed.
        if let Some(until) = self.jog_until {
            if Instant::now() >= until {
                self.jog_offset = 0.0;
                self.jog_until  = None;
                let f = self.fader_speed.load(Ordering::Relaxed);
                self.audio.speed.store(f, Ordering::Relaxed);
            }
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
            sync: self.link.sync.load(Ordering::Relaxed), master: self.link.master.load(Ordering::Relaxed), zoom_grid_mode: self.zoom_grid_mode,
            source_link: self.source_link, phase_ticks_view: self.phase_ticks_view,
            linked: beat2_player > 0,
            // Master indicator (phase-ticks view): the real XDJ shows a flat
            // line (no number) when THIS deck is master, and another deck's
            // number only when following that deck.  So report 0 (→ "-") when we
            // hold master, else the tracked master player (also 0 → "-" if none).
            // No beat2_player fallback — that wrongly named the last-seen deck
            // (the XDJ) as master whenever master_player was 0.
            master_player: if self.link.master.load(Ordering::Relaxed) {
                0
            } else {
                self.link.master_player.load(Ordering::Relaxed) as u8
            },
            cue_point: self.cue_point,
        };
        let snap = make_snapshot(&self.path, self.beat_grid.as_ref(), &self.audio, flags, pos, playing, speed, fader_speed, beat2_bpm, beat2_phase_beats, beat2_bib_v);

        // Dev: OPENDECK_SCREENSHOT=path captures frame 90 and exits.
        self.frame_count += 1;
        if self.frame_count == 90 {
            if let Ok(path) = std::env::var("OPENDECK_SCREENSHOT") {
                renderer.request_capture(path.into());
                self.exit_after_capture = true;
            }
        }
        let _t_render = Instant::now();
        renderer.render(&snap, &vp, &self.egui_ctx, output);
        perf_accum("render_call", _t_render.elapsed());
        let total = frame_start.elapsed();
        self.last_frame_total = total;   // fed to next frame's spike detector
        perf_accum("FRAME_TOTAL", total);
        perf_tick();
    }
}

impl ApplicationHandler for DeckApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // already initialised (e.g. Android resume)
        }

        // Screen-only is the 7" panel; the faceplate adds the deck body around
        // it, at roughly the XDJ-1000MK2 face aspect (212 × 320 mm ≈ 1:1.5).
        let (win_w, win_h) = if self.faceplate { (760u32, 1180u32) } else { (1024u32, 600u32) };
        let attrs = WindowAttributes::default()
            .with_title("freedj-3000")
            .with_inner_size(winit::dpi::LogicalSize::new(win_w, win_h));

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
                event: KeyEvent { physical_key: PhysicalKey::Code(code), state, repeat, .. },
                ..
            } => {
                use KeyCode::*;
                // CUE is momentary: handle both edges (playback mode), ignore the
                // OS key-repeat.  Enter still means Load in the browser.
                if self.screen_mode == ScreenMode::Playback && matches!(code, Enter | NumpadEnter) {
                    if !repeat {
                        self.events.push(Event::Deck(ControlEvent::Cue { pressed: state == ElementState::Pressed }));
                        if let Some(w) = &self.window { w.request_redraw(); }
                    }
                    return;
                }
                // Everything else acts on key-down only.
                if state != ElementState::Pressed { return; }
                // In the browser the keys navigate the list, not the transport.
                if self.screen_mode == ScreenMode::Browse {
                    let ev = match code {
                        ArrowDown           => Some(Event::Deck(ControlEvent::BrowseEncoderDelta { delta: 1 })),
                        ArrowUp             => Some(Event::Deck(ControlEvent::BrowseEncoderDelta { delta: -1 })),
                        Enter | NumpadEnter => Some(Event::Deck(ControlEvent::Load)),
                        Backspace           => Some(Event::Deck(ControlEvent::Back)),
                        KeyB | Escape       => Some(Event::Ui(UiEvent::Screen(TopScreen::Browse))), // toggle off
                        _ => None,
                    };
                    if let Some(ev) = ev {
                        self.events.push(ev);
                        if let Some(w) = &self.window { w.request_redraw(); }
                    }
                    return;
                }
                let playing = self.audio.playing.load(Ordering::Relaxed);
                let total   = self.audio.len().max(1) as f32;
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
                    KeyB => Some(Event::Ui(UiEvent::Screen(TopScreen::Browse))),
                    Comma  => Some(Event::Deck(ControlEvent::JogDelta { delta: -4, velocity_rpm: 0.0 })),
                    Period => Some(Event::Deck(ControlEvent::JogDelta { delta:  4, velocity_rpm: 0.0 })),
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
        // Default: redraws are requested from RedrawRequested and paced entirely
        // by the compositor's frame callback (ControlFlow::Wait).  Clean phase-
        // lock, but no fallback — if the compositor is late calling us back the
        // UI freezes for the whole gap (the "stall OUTSIDE render" spikes).
        if !self.hybrid_pace {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }

        // Hybrid pacer: keep the frame callback as the primary clock, but arm a
        // safety-net timer at ~2 display periods.  When a callback arrives on
        // time (every ~1 period) the timer is re-armed well before it fires and
        // nothing changes.  When the callback is late/missing, the WaitUntil
        // wakes us here past the deadline and we self-drive one frame — turning
        // a multi-refresh freeze into a single late frame.  render_frame owns
        // its surface acquire+present, so it runs fine outside RedrawRequested;
        // with Mailbox the extra present is non-blocking and re-locks to the
        // callback as soon as the compositor resumes.
        let net = self.refresh_interval.saturating_mul(2);
        if self.window.is_some() && Instant::now() >= self.last_render + net {
            self.render_frame();                 // updates last_render
            if let Some(w) = &self.window { w.request_redraw(); }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.last_render + net));
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .format_timestamp_micros()   // beat and frame timing are measured from the log
    .init();

    // Args: <file> [--player N].  OPENDECK_PLAYER also works.
    let mut args = std::env::args().skip(1);
    let mut path: Option<PathBuf> = None;
    let mut player: u8 = std::env::var("OPENDECK_PLAYER").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    // Controller side: A = left (MIDI channel 0), B = right (channel 1).
    let mut deck_channel: u8 = match std::env::var("OPENDECK_DECK").ok().as_deref() {
        Some("B") | Some("b") => 1,
        _ => 0,
    };
    // Link SEND (beat/status/master handoff) is OFF by default. An XDJ froze
    // twice during two-deck play; the symptoms point at its USB stick, not our
    // traffic, but receive-only is a safe default and there is a real defect
    // to fix first (status built from the XDJ's own packet as a template —
    // WORKSTREAMS B2). Receive-only still follows a master's tempo. Opt into
    // full send with `--link-send` (or OPENDECK_LINK_SEND=1).
    let mut link_send = std::env::var("OPENDECK_LINK_SEND").map(|v| v == "1").unwrap_or(false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--player" => player = args.next().and_then(|v| v.parse().ok()).context("--player needs a number 1-6")?,
            "--deck" => deck_channel = match args.next().as_deref() {
                Some("A") | Some("a") => 0,
                Some("B") | Some("b") => 1,
                _ => bail!("--deck needs A or B"),
            },
            "--link-send" => link_send = true,
            "--link-receive-only" => link_send = false,
            "--faceplate" => std::env::set_var("OPENDECK_FACEPLATE", "1"),
            _ => path = Some(a.into()),
        }
    }
    let path = path.context("usage: opendeck <path/to/file.mp3> [--player N]")?;
    let player = player.clamp(1, 6);

    if !path.exists() {
        bail!("file not found: {}", path.display());
    }

    // ── 1. Decode audio ───────────────────────────────────────────────────────
    let audio = AudioHandle::open(&path)?;

    // ── 2. Build waveform + detect beat grid (synchronous, before window opens) ─
    let samples_arc = audio.current();
    log::info!("computing waveform ({} samples)...", samples_arc.len());
    let t0 = Instant::now();
    let mut waveform_builder = WaveformBuilder::new(audio.sample_rate);
    let mut beat_analyzer    = BeatAnalyzerImpl::new(audio.sample_rate);
    waveform_builder.push(&samples_arc);
    beat_analyzer.push(&samples_arc, audio.sample_rate);
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
    let beat2_bib    = Arc::new(AtomicU32::new(0));

    // ── 4. Start ProDJ Link listener (optional — app runs fine without it) ────────
    let link = prodj::LinkState::new(player);
    // Dev hooks for headless tests: OPENDECK_SYNC=1 / OPENDECK_MASTER=1.
    if std::env::var("OPENDECK_SYNC").map(|v| v == "1").unwrap_or(false)   { link.sync.store(true, Ordering::Relaxed); }
    if std::env::var("OPENDECK_MASTER").map(|v| v == "1").unwrap_or(false) { link.want_master.store(true, Ordering::Relaxed); }
    let _prodj = prodj::ProDjHandle::listen(
        Arc::clone(&link),
        Arc::clone(&beat2_bpm),
        Arc::clone(&beat2_anchor),
        Arc::clone(&beat2_player),
        Arc::clone(&beat2_bib),
    );
    log::info!("ProDJ Link send: {}", if link_send { "full (beat/status/master)" } else { "receive-only (announce + listen)" });
    // Live beat grid shared with the ProDJ Link sender: the deck updates it on
    // every LOAD so sync divides by the CURRENT track's BPM, not the startup one.
    let link_grid = Arc::new(arc_swap::ArcSwap::from_pointee(beat_grid.clone()));
    // Live send flag: off = pure follower; pressing MASTER turns it on.
    let link_send_flag = Arc::new(AtomicBool::new(link_send));
    let _prodj_tx = prodj::ProDjSender::start(Arc::clone(&link), prodj::SenderState {
        send_full:   Arc::clone(&link_send_flag),
        position:    Arc::clone(&audio.position),
        in_flight:   Arc::clone(&audio.in_flight),
        playing:     Arc::clone(&audio.playing),
        fader_speed: Arc::clone(&fader_speed),
        speed:       Arc::clone(&audio.speed),
        sample_rate: audio.sample_rate,
        channels:    audio.channels,
        grid:        Arc::clone(&link_grid),
    });

    // ── 5. Connect MIDI controller (optional — app runs fine without it) ──────────
    // ── 5. Input bus: MIDI (DJ2Go) forwards controls into this channel ────────
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    log::info!("controller: deck {} (MIDI channel {deck_channel})", if deck_channel == 0 { "A/left" } else { "B/right" });
    let _midi = midi::MidiHandle::connect(event_tx, deck_channel == 1);

    // ── 6. Run the UI event loop ──────────────────────────────────────────────
    let event_loop = EventLoop::new().context("failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = DeckApp::new(path, waveform, audio, beat_grid, fader_speed, beat2_bpm, beat2_anchor, beat2_player, beat2_bib, link, link_grid, Arc::clone(&link_send_flag), event_rx);
    event_loop.run_app(&mut app).context("event loop error")?;

    Ok(())
}

#[cfg(test)]
mod anlz_grid_tests {
    use super::*;
    // Gated: OPENDECK_TEST_RB=/run/media/dan/CDJ1
    #[test]
    fn grid_from_anlz_matches_rekordbox() {
        let Ok(root) = std::env::var("OPENDECK_TEST_RB") else { return };
        let root = std::path::PathBuf::from(root);
        let exp = opendeck_rekordbox::read_export(&root).unwrap();
        let t = exp.tracks.iter().find(|t| t.title.contains("OG Sins")).unwrap();
        let ap = t.analyze_on(&root).unwrap();
        let (grid, _cue) = DeckApp::grid_from_anlz(&ap, 48_000, 2).expect("grid");
        assert!((grid.bpm - 125.0).abs() < 0.5, "bpm {}", grid.bpm);
        assert_eq!(grid.downbeat_offset, 0, "first beat is a downbeat");
        // first beat at 56 ms → 56*48000/1000 = 2688 frames
        assert_eq!(grid.anchor_sample, 2688, "anchor frames");
    }
}

#[cfg(test)]
mod nfs_link_tests {
    // Gated: OPENDECK_TEST_NFS=192.168.68.58 (a linked XDJ with a USB)
    #[test]
    fn browse_linked_library_over_nfs() {
        let Ok(ip) = std::env::var("OPENDECK_TEST_NFS") else { return };
        let ip: std::net::Ipv4Addr = ip.parse().unwrap();
        let mut nfs = opendeck_nfs::Nfs::connect(ip).expect("connect");
        let root = nfs.mount_usb().expect("mount");
        let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb").expect("lookup");
        let bytes = nfs.read_file(&fh, size).expect("read pdb");
        assert_eq!(bytes.len() as u32, size, "read whole pdb");
        let exp = opendeck_rekordbox::read_export_from(
            &mut std::io::Cursor::new(bytes),
            std::path::PathBuf::from("nfs://linked"),
        ).expect("parse over-the-wire pdb");
        assert!(exp.tracks.len() > 200, "tracks: {}", exp.tracks.len());
        println!("OK: browsed {} tracks + {} playlists off the XDJ over NFS",
            exp.tracks.len(), exp.playlists.len());
    }
}

#[cfg(test)]
mod nfs_load_tests {
    // Gated: OPENDECK_TEST_NFS=192.168.68.58
    #[test]
    fn load_track_audio_and_anlz_over_nfs() {
        let Ok(ip) = std::env::var("OPENDECK_TEST_NFS") else { return };
        let ip: std::net::Ipv4Addr = ip.parse().unwrap();
        let mut nfs = opendeck_nfs::Nfs::connect(ip).unwrap();
        let root = nfs.mount_usb().unwrap();

        // Parse the library off the wire, pick an mp3 track we know.
        let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb").unwrap();
        let pdb = nfs.read_file(&fh, size).unwrap();
        let exp = opendeck_rekordbox::read_export_from(
            &mut std::io::Cursor::new(pdb), std::path::PathBuf::from("nfs")).unwrap();
        let t = exp.tracks.iter().find(|t| t.title.contains("OG Sins")).unwrap();
        println!("track: {} — {}  {}", t.artist, t.title, t.rel_path);

        // Read the audio file over NFS and decode from memory.
        let (afh, asize) = nfs.lookup_path(&root, &t.rel_path).expect("audio lookup");
        let audio = nfs.read_file(&afh, asize).unwrap();
        assert_eq!(audio.len() as u32, asize);
        let ext = t.rel_path.rsplit('.').next();
        let (samples, sr, ch) = crate::audio::decode_bytes(audio, ext).expect("decode over-wire audio");
        assert!(samples.len() > sr as usize * ch, "> 1s of audio decoded");
        let secs = samples.len() as f64 / ch as f64 / sr as f64;

        // Read the ANLZ over NFS and parse the beat grid.
        let (nfh, nsize) = nfs.lookup_path(&root, &t.analyze_rel).expect("anlz lookup");
        let anlz = nfs.read_file(&nfh, nsize).unwrap();
        let a = opendeck_rekordbox::read_anlz_from(&mut std::io::Cursor::new(anlz)).unwrap();
        assert!(!a.beats.is_empty());
        println!("OK over NFS: decoded {secs:.0}s @ {sr}Hz/{ch}ch, {} beats @ {:.1} BPM",
            a.beats.len(), a.beats[0].bpm);
    }
}
