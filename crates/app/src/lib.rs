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
mod settings;
mod snapshot;
mod taglist;

use anyhow::{bail, Context, Result};
use audio::AudioHandle;
use opendeck_analysis::{BeatAnalyzerImpl, WaveformBuilder, WaveformCache};
use opendeck_types::{BeatAnalyzer, BeatGrid};
use renderer::Renderer;
use input::{ControlEvent, Event, PerformMode, Screen as TopScreen, Source, UiEvent, ZOOM_LEVELS, ZOOM_DEFAULT};
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
enum ScreenMode { Playback, Browse, Info, Perform, TagList, Menu }

struct DeckApp {
    // Provided before event loop starts.
    path:         PathBuf,
    waveform:     WaveformCache,
    audio:        AudioHandle,
    beat_grid:    Option<BeatGrid>,
    screen_mode:  ScreenMode,
    browser:      Browser,
    /// TAG LIST — the on-the-fly playlist; persisted (see taglist.rs).
    tag_list:     taglist::TagList,
    /// MENU / UTILITY settings; persisted (see settings.rs).
    settings:     settings::Settings,
    menu_cursor:  usize,

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
    key_lock:          bool,      // Master Tempo: mirrors audio.key_lock (DSP truth)
    auto_cue:          bool,      // AUTO CUE: cue at first sound on load (on by default, as the unit)
    slip:              bool,
    zoom_level:        usize,     // index into ZOOM_LEVELS
    zoom_grid_mode:    bool,
    source_link:       bool,
    phase_ticks_view:  bool,
    /// Render the full physical faceplate (jog, fader, buttons) around the
    /// screen.  Off by default — the Pi/hardware target runs screen-only.
    faceplate:         bool,
    portrait:          bool,   // iPad 13" portrait chrome (OPENDECK_PORTRAIT=1)
    /// Same-thread sources (keyboard, touch) push here.
    events:            Vec<Event>,
    /// Off-thread sources (MIDI, later HID/serial) send here; drained per frame.
    event_rx:          mpsc::Receiver<Event>,
    /// Jog nudge: a temporary speed offset that snaps back when the wheel stops.
    jog_offset:        f32,
    jog_until:         Option<Instant>,
    cue_point:         u64,   // start cue, source sample index (CDJ CUE)
    /// Memory points (interleaved sample indices, sorted): rekordbox's from the
    /// ANLZ on load, plus any set with MEMORY.  CALL ◀▶ steps through them;
    /// DELETE removes the one the deck is cued at.  In-session for now.
    memory_cues:       Vec<u64>,
    /// The loaded track's own tags (ID3 etc.) — title/artist for the title bar
    /// and the INFO screen.
    track_tags:        opendeck_decode::TrackTags,
    /// Hot cues A–H (interleaved sample index, or empty): rekordbox's from the
    /// ANLZ on load, plus any set from the PERFORM pads.  In-session for now.
    hot_cues:          [Option<u64>; 8],
    /// PERFORM screen state: pad mode, which bank the four pads show (0 = A–D,
    /// 1 = E–H), and whether DELETE –CALL is armed (next pad tap deletes).
    perform_mode:      PerformMode,
    perform_bank:      u8,
    perform_delete:    bool,
    /// Beat length of the active BEAT LOOP (0 = a manual IN/OUT loop), so the
    /// matching pad lights.  The loop itself lives on `audio.loop_*`.
    loop_beats:        f32,
    /// SLIP: while a loop / held hot cue is in progress with SLIP on, the
    /// playhead the track WOULD be at — (audible position when the action
    /// began, audio.source_consumed then).  Shadow = pos + consumed-since.
    /// Ending the action jumps there; a manual seek cancels it.
    slip_anchor:       Option<(f64, u64)>,
    cue_preview:       bool,  // CUE held → previewing from the cue point
    cued:              bool,  // playhead is sitting on the cue (not searched away)
    exit_after_capture: bool,

    // Created on first `resumed`.
    window:      Option<Arc<Window>>,
    renderer:    Option<Renderer>,
    egui_ctx:    egui::Context,
    egui_state:  Option<egui_winit::State>,
    /// Faceplate background photo (loaded once from a path; see load_face_texture).
    face_tex:       Option<egui::TextureHandle>,
    face_tex_tried: bool,

    /// Time of the last rendered frame, used to cap to FRAME_INTERVAL.
    last_render: Instant,
    /// Wall-clock cost of the previous frame's render_frame body, so a spike
    /// detector can split a long inter-frame gap into "our code was slow" vs
    /// "the scheduler/compositor didn't wake us".
    last_frame_total: Duration,
    /// Count of inter-frame gaps that exceeded the spike threshold.
    frame_spikes: u64,
    /// Start of the current frame-rate measurement window (heartbeat log).
    fps_window: Instant,
    /// OPENDECK_PACE=hybrid: add a safety-net timer so a late/missing compositor
    /// frame callback doesn't freeze the UI — we self-drive a frame instead.
    hybrid_pace: bool,
    /// True while the window is occluded (iOS background).  We stop rendering and
    /// stop requesting redraws so the app can quiesce and be suspended cleanly;
    /// see the WindowEvent::Occluded handler for the full rationale.
    occluded: bool,
    /// Last-seen audio glitch counters, to log only on change.
    prev_underruns: u64,
    prev_drops:     u64,
}

/// Display/deck flags copied out of DeckApp so a snapshot can borrow only
/// `path`, `beat_grid` and `audio` — leaving `renderer` free to be borrowed
/// mutably in the same frame.
#[derive(Clone, Copy)]
struct UiFlags {
    key_lock: bool, remain_mode: bool, auto_cue: bool, slip: bool, sync: bool, master: bool,
    zoom_grid_mode: bool, source_link: bool, phase_ticks_view: bool, linked: bool, master_player: u8,
    player: u8,
    cue_point: u64,
    hot_cues: [Option<u64>; 8],
    perform_mode: PerformMode, perform_bank: u8, perform_delete: bool,
    loop_active: bool, loop_start: u64, loop_end: u64, loop_beats: f32,
    tempo_range: f32, quantize: bool,
    slip_shadow: Option<u64>,
}

#[allow(clippy::too_many_arguments)]
fn make_snapshot<'a>(
    path: &'a std::path::Path, beat_grid: Option<&'a BeatGrid>, memory_cues: &'a [u64],
    tags: &'a opendeck_decode::TrackTags, audio: &AudioHandle, f: UiFlags,
    pos: u64, playing: bool, speed: f32, fader_speed: f32, beat2_bpm: f32, beat2_phase_beats: f32, beat2_beat_in_bar: u8,
) -> DeckSnapshot<'a> {
    DeckSnapshot {
        // Title bar shows the tagged title when the file has one (as the unit
        // does), else the filename.
        title:         if path.as_os_str().is_empty() { "NO TRACK" }
                       else { tags.title.as_deref()
                                .unwrap_or_else(|| path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown")) },
        tags,
        file:          path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        position:      pos,
        sample_rate:   audio.sample_rate,
        channels:      audio.channels,
        total_samples: audio.len() as u64,
        cue_point:     f.cue_point,
        memory_cues,
        hot_cues:      f.hot_cues,
        perform_mode:  f.perform_mode,
        perform_bank:  f.perform_bank,
        perform_delete: f.perform_delete,
        loop_active:   f.loop_active,
        loop_start:    f.loop_start,
        loop_end:      f.loop_end,
        loop_beats:    f.loop_beats,
        tempo_range:   f.tempo_range,
        quantize:      f.quantize,
        slip_shadow:   f.slip_shadow,
        playing, speed, fader_speed,
        key_lock:      f.key_lock,
        remain_mode:   f.remain_mode,
        auto_cue:      f.auto_cue,
        slip:          f.slip,
        sync:          f.sync,
        master:        f.master,
        zoom_grid_mode: f.zoom_grid_mode,
        source_link:   f.source_link,
        phase_ticks_view: f.phase_ticks_view,
        linked:        f.linked,
        master_player: f.master_player,
        player:        f.player,
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
        settings:     settings::Settings,
    ) -> Self {
        let browser = Browser::new(&path, std::sync::Arc::clone(&link));
        let tag_list = taglist::TagList::open();
        let cue_point = std::env::var("OPENDECK_CUE").ok().and_then(|v| v.parse::<f64>().ok())
            .map(|secs| (secs * audio.sample_rate as f64 * audio.channels as f64) as u64).unwrap_or(0);
        // AUTO CUE (on by default, as the unit): with no explicit cue, the
        // startup track cues at its first audible sound rather than 0:00 — the
        // same rule finish_load applies to tracks loaded from the browser.
        let cue_point = if cue_point == 0 {
            first_sound(audio.samples.load().as_slice(), audio.channels as usize, settings.auto_cue_level_db)
        } else { cue_point };
        // Dev: OPENDECK_MEMORY_CUES=1.5,4.0 seeds memory points (seconds) on the
        // startup track — exercises MEMORY/CALL/DELETE and the overview markers
        // headlessly, where no rekordbox ANLZ supplies them.
        let sr_ch = audio.sample_rate as f64 * audio.channels as f64;
        let mut memory_cues: Vec<u64> = std::env::var("OPENDECK_MEMORY_CUES").ok()
            .map(|v| v.split(',').filter_map(|s| s.trim().parse::<f64>().ok())
                     .map(|secs| ((secs * sr_ch) as u64 / audio.channels as u64) * audio.channels as u64)
                     .collect())
            .unwrap_or_default();
        memory_cues.sort_unstable();
        // Dev: OPENDECK_HOT_CUES=1.5,,4.0 seeds hot cues A.. (seconds; blank =
        // empty slot) on the startup track, for exercising the PERFORM pads.
        let mut hot_cues: [Option<u64>; 8] = [None; 8];
        if let Ok(v) = std::env::var("OPENDECK_HOT_CUES") {
            for (i, s) in v.split(',').take(8).enumerate() {
                if let Ok(secs) = s.trim().parse::<f64>() {
                    hot_cues[i] = Some(((secs * sr_ch) as u64 / audio.channels as u64) * audio.channels as u64);
                }
            }
        }
        let track_tags = audio.tags.clone();
        Self {
            path,
            waveform,
            audio,
            beat_grid,
            screen_mode: match std::env::var("OPENDECK_SCREEN").as_deref() {
                Ok("browse")  => ScreenMode::Browse,
                Ok("info")    => ScreenMode::Info,
                Ok("perform") => ScreenMode::Perform,
                Ok("taglist") => ScreenMode::TagList,
                Ok("menu")    => ScreenMode::Menu,
                _             => ScreenMode::Playback,
            },
            browser,
            tag_list,
            settings,
            menu_cursor:       0,
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
            auto_cue:          true,
            slip:              false,
            zoom_level:        ZOOM_DEFAULT,
            zoom_grid_mode:    false,
            source_link:       false,
            // Dev: OPENDECK_PHASE_VIEW=ticks starts in the alignment view (for captures).
            phase_ticks_view:  std::env::var("OPENDECK_PHASE_VIEW").map(|v| v == "ticks").unwrap_or(false),
            faceplate:         std::env::var("OPENDECK_FACEPLATE").map(|v| v == "1").unwrap_or(false),
            portrait:          std::env::var("OPENDECK_PORTRAIT").map(|v| v == "1").unwrap_or(false),
            events:            Vec::new(),
            event_rx,
            jog_offset:        0.0,
            jog_until:         None,
            cue_point,
            memory_cues,
            track_tags,
            hot_cues,
            perform_mode:      PerformMode::HotCue,
            perform_bank:      0,
            perform_delete:    false,
            loop_beats:        0.0,
            slip_anchor:       None,
            cue_preview:       false,
            cued:              true,
            exit_after_capture: false,
            window:      None,
            renderer:    None,
            egui_ctx:    egui::Context::default(),
            egui_state:  None,
            face_tex:       None,
            face_tex_tried: false,
            last_render: Instant::now(),
            last_frame_total: Duration::ZERO,
            frame_spikes: 0,
            fps_window: Instant::now(),
            // iOS defaults to the hybrid pacer.  The plain path relies entirely
            // on the platform calling us back after request_redraw; where that
            // callback doesn't arrive the UI simply stops until the next touch,
            // with no fallback.  The safety-net timer costs nothing when the
            // callback is on time and turns a freeze into a self-driven frame.
            hybrid_pace: std::env::var("OPENDECK_PACE")
                .map(|v| v == "hybrid")
                .unwrap_or(cfg!(target_os = "ios")),
            prev_underruns: 0,
            prev_drops:     0,
            occluded:       false,
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
                let range = self.settings.tempo_range;
                let step = delta / (2.0 * range);
                let f = input::speed_to_fader(f32::from_bits(self.fader_speed.load(Ordering::Relaxed)), range);
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
                        self.cue_point   = self.quantized(self.smoothed_pos);
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
            // ── Memory points (MEMORY / CALL ◀▶ / DELETE) ──────────────────────
            // Matching uses a small tolerance so a point set from a paused cue
            // and one read from an ANLZ (ms-quantised) still count as the same.
            Event::Deck(ControlEvent::MemoryCueSet) => {
                let ch  = self.audio.channels as u64;
                let tol = ch * 64;
                let c   = self.cue_point;
                if self.audio.len() == 0 {
                    log::info!("memory: no track");
                } else if memory_at(&self.memory_cues, c, tol).is_some() {
                    log::info!("memory: point already set here");
                } else if self.memory_cues.len() >= 10 {
                    log::info!("memory: full (10 points)");
                } else {
                    self.memory_cues.push(c);
                    self.memory_cues.sort_unstable();
                    let sr_ch = self.audio.sample_rate as f64 * ch as f64;
                    log::info!("memory: set at {:.2}s ({} points)", c as f64 / sr_ch, self.memory_cues.len());
                }
            }
            Event::Deck(ControlEvent::MemoryCueCall { next }) => {
                // Jump to the previous / next memory point from the playhead and
                // cue there (paused), as CALL does on the unit.
                let ch  = self.audio.channels as u64;
                let tol = ch * 64;
                let pos = self.smoothed_pos.max(0.0) as u64;
                let target = if next { memory_next(&self.memory_cues, pos, tol) }
                             else    { memory_prev(&self.memory_cues, pos, tol) };
                match target {
                    Some(m) => {
                        self.audio.playing.store(false, Ordering::Relaxed);
                        self.cue_point   = m;
                        self.cue_preview = false;
                        self.cued        = true;
                        self.seek_to(m);
                        let sr_ch = self.audio.sample_rate as f64 * ch as f64;
                        log::info!("call {}: cued at {:.2}s", if next { "▶" } else { "◀" }, m as f64 / sr_ch);
                    }
                    None => log::info!("call {}: no memory point that way", if next { "▶" } else { "◀" }),
                }
            }
            Event::Deck(ControlEvent::MemoryCueDelete) => {
                let ch  = self.audio.channels as u64;
                let tol = ch * 64;
                let c   = self.cue_point;
                match memory_at(&self.memory_cues, c, tol) {
                    Some(i) => {
                        self.memory_cues.remove(i);
                        log::info!("memory: deleted point at cue ({} left)", self.memory_cues.len());
                    }
                    None => log::info!("memory: no point at the cue to delete"),
                }
            }
            // ── Hot cues A–H (PERFORM pads) ─────────────────────────────────────
            Event::Deck(ControlEvent::HotCueSet { slot }) => {
                let s = slot as usize;
                if s < 8 && self.audio.len() > 0 {
                    let ch  = self.audio.channels as u64;
                    let pos = self.quantized(self.smoothed_pos);
                    self.hot_cues[s] = Some(pos);
                    let sr_ch = self.audio.sample_rate as f64 * ch as f64;
                    log::info!("hot cue {}: set at {:.2}s", (b'A' + slot) as char, pos as f64 / sr_ch);
                }
            }
            Event::Deck(ControlEvent::HotCueTrigger { slot, .. }) => {
                // A hot cue is jump + play (latching), unlike momentary CUE.
                if let Some(pos) = self.hot_cues.get(slot as usize).copied().flatten() {
                    self.seek_to(pos);
                    self.lock_in_play();
                    log::info!("hot cue {}: play", (b'A' + slot) as char);
                } else {
                    log::info!("hot cue {}: empty", (b'A' + slot) as char);
                }
            }
            Event::Deck(ControlEvent::HotCueDelete { slot }) => {
                if let Some(c) = self.hot_cues.get_mut(slot as usize) {
                    *c = None;
                    log::info!("hot cue {}: deleted", (b'A' + slot) as char);
                }
                self.perform_delete = false;
            }
            // ── Beat jump (PERFORM pads in BEAT JUMP mode) ───────────────────────
            Event::Deck(ControlEvent::BeatJump { beats }) => {
                match &self.beat_grid {
                    Some(g) if g.bpm > 0.0 => {
                        let ch  = self.audio.channels as f64;
                        let per_beat = self.audio.sample_rate as f64 * 60.0 / g.bpm * ch;
                        let cur = self.smoothed_pos.max(0.0);
                        let new = (cur + beats as f64 * per_beat).clamp(0.0, self.audio.len() as f64);
                        let new = ((new as u64) / ch as u64) * ch as u64;
                        self.seek_to(new);   // playing state is untouched: jump in place
                        log::info!("beat jump {beats:+}: → {:.2}s", new as f64 / (self.audio.sample_rate as f64 * ch));
                    }
                    _ => log::info!("beat jump: no beat grid"),
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
                let s = input::fader_to_speed(position, self.settings.tempo_range);
                self.fader_speed.store(s.to_bits(), Ordering::Relaxed);
                self.audio.speed_store(s);
                log::info!("tempo {:+.2}%", (s - 1.0) * 100.0);
            }
            Event::Deck(ControlEvent::KeyLockToggle) => {
                // Master Tempo: on = key lock (time-stretch, pitch held); off =
                // varispeed (pitch tracks speed). The processor reads this flag.
                self.key_lock = !self.key_lock;
                self.audio.key_lock.store(self.key_lock, Ordering::Relaxed);
                log::info!("master tempo {}", if self.key_lock { "ON (key lock)" } else { "OFF (varispeed)" });
            }
            Event::Deck(ControlEvent::SlipToggle) => {
                self.slip = !self.slip;
                if self.slip {
                    // Engaged mid-loop: start shadowing from here.
                    if self.audio.loop_active.load(Ordering::Relaxed) { self.slip_begin(); }
                } else {
                    self.slip_anchor = None;   // disengaged: no jump, just stop shadowing
                }
                log::info!("slip {}", if self.slip { "on" } else { "off" });
            }
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
                match self.screen_mode {
                    ScreenMode::Browse  => self.browser.move_selection(delta),
                    ScreenMode::TagList => self.tag_list.move_selection(delta),
                    ScreenMode::Menu    => {
                        let last = settings::MENU.len() as i32 - 1;
                        self.menu_cursor = (self.menu_cursor as i32 + delta).clamp(0, last) as usize;
                    }
                    // Outside a list the selector nudges zoom, as on the unit.
                    _ => self.apply(Event::Ui(UiEvent::ZoomStep(delta.signum()))),
                }
            }
            Event::Deck(ControlEvent::Load) => {
                match self.screen_mode {
                    ScreenMode::Browse => match self.browser.enter() {
                        Enter::Folder  => {}                       // descended; stay in browser
                        Enter::Nothing => {}
                        Enter::Track(load) => match self.load_selected(load) {
                            Ok(())  => self.screen_mode = ScreenMode::Playback,
                            Err(e)  => log::warn!("load failed: {e:#}"),
                        },
                    },
                    ScreenMode::TagList => {
                        if let Some(e) = self.tag_list.selected_entry().cloned() {
                            match self.load_selected(e.load) {
                                Ok(())  => self.screen_mode = ScreenMode::Playback,
                                Err(e)  => log::warn!("load failed: {e:#}"),
                            }
                        }
                    }
                    // In MENU the selector's press steps the highlighted setting.
                    ScreenMode::Menu => self.settings.cycle(settings::MENU[self.menu_cursor].0, 1),
                    _ => {
                        self.screen_mode = ScreenMode::Browse;
                        self.browser.refresh();
                    }
                }
            }
            Event::Deck(ControlEvent::Back) => {
                match self.screen_mode {
                    ScreenMode::Browse  => self.browser.back(),
                    ScreenMode::TagList | ScreenMode::Menu => self.screen_mode = ScreenMode::Playback,
                    _ => {}
                }
            }
            Event::Ui(UiEvent::MenuTap(i)) => {
                if i < settings::MENU.len() {
                    self.menu_cursor = i;
                    self.settings.cycle(settings::MENU[i].0, 1);
                }
            }
            Event::Ui(UiEvent::TagTrack) => {
                // TAG TRACK / REMOVE: in BROWSE toggle the highlighted track's
                // tag; on the TAG LIST screen drop the highlighted track.
                match self.screen_mode {
                    ScreenMode::Browse => {
                        if let Some(e) = self.browser.selected_entry() {
                            if let Some(load) = e.load() {
                                let entry = taglist::TagEntry { name: e.name.clone(), artist: e.artist.clone(), load: load.clone() };
                                let tagged = self.tag_list.toggle(entry);
                                log::info!("tag list: {} {}", if tagged { "tagged" } else { "removed" }, e.name);
                            }
                        }
                    }
                    ScreenMode::TagList => {
                        if let Some(e) = self.tag_list.remove_selected() {
                            log::info!("tag list: removed {}", e.name);
                        }
                    }
                    _ => {}
                }
            }
            // ── Loops ───────────────────────────────────────────────────────────
            Event::Deck(ControlEvent::BeatLoop { beats, .. }) => {
                let Some(per_beat) = self.samples_per_beat() else {
                    log::info!("beat loop: no beat grid"); return;
                };
                let active = self.audio.loop_active.load(Ordering::Relaxed);
                if active && (self.loop_beats - beats).abs() < 1e-3 {
                    self.exit_loop_slip();                  // same pad again = exit
                } else {
                    // A new length re-uses the running loop's start; a fresh
                    // loop starts at the beat the playhead is in.
                    let start = if active { self.audio.loop_start.load(Ordering::Relaxed) }
                                else { self.beat_floor(self.smoothed_pos) };
                    let ch  = self.audio.channels as u64;
                    let len = (((beats as f64 * per_beat) as u64) / ch) * ch;
                    self.set_loop(start, start + len, beats);
                }
            }
            Event::Deck(ControlEvent::LoopIn) => {
                // Start a manual loop here; OUT closes it.  Any running loop ends.
                let ch = self.audio.channels as u64;
                let pos = self.quantized(self.smoothed_pos);
                self.exit_loop();
                self.audio.loop_start.store(pos, Ordering::Relaxed);
                self.audio.loop_end.store(pos, Ordering::Relaxed);
                self.loop_beats = 0.0;
                log::info!("loop in: {:.2}s", pos as f64 / (self.audio.sample_rate as f64 * ch as f64));
            }
            Event::Deck(ControlEvent::LoopOut) => {
                let pos = self.quantized(self.smoothed_pos);
                let start = self.audio.loop_start.load(Ordering::Relaxed);
                if pos > start { self.set_loop(start, pos, 0.0); }
                else { log::info!("loop out: before the in point — ignored"); }
            }
            Event::Deck(ControlEvent::LoopToggle) | Event::Deck(ControlEvent::Reloop) => {
                // RELOOP/EXIT: leave a running loop, or jump back into the last
                // one and run it again.
                if self.audio.loop_active.load(Ordering::Relaxed) {
                    self.exit_loop_slip();
                } else {
                    let (s, e) = (self.audio.loop_start.load(Ordering::Relaxed), self.audio.loop_end.load(Ordering::Relaxed));
                    if e > s {
                        let beats = self.loop_beats;
                        // With SLIP the shadow starts where we ARE, not at the
                        // loop start we're jumping back to.
                        let anchor = self.slip.then(|| (self.smoothed_pos.max(0.0), self.audio.source_consumed.load(Ordering::Relaxed)));
                        self.seek_to(s);
                        self.set_loop(s, e, beats);
                        if anchor.is_some() { self.slip_anchor = anchor; }
                        log::info!("reloop");
                    } else {
                        log::info!("reloop: no loop stored");
                    }
                }
            }
            Event::Deck(other) => log::info!("unhandled deck event {other:?}"),

            Event::Ui(UiEvent::TimeMode) => {
                self.remain_mode = !self.remain_mode;
                log::info!("time display → {}", if self.remain_mode { "REMAIN" } else { "TIME" });
            }
            Event::Ui(UiEvent::AutoCue) => {
                self.auto_cue = !self.auto_cue;
                log::info!("auto cue {}", if self.auto_cue { "on" } else { "off" });
                // Turning it on applies to the loaded track too when it's cued at
                // 0:00 and paused: re-cue to the first sound so the change shows
                // immediately (turning it off leaves an existing cue alone, as
                // the unit does — AUTO CUE otherwise governs future loads).
                if self.auto_cue && self.cue_point == 0 && !self.audio.playing.load(Ordering::Relaxed) {
                    let ch = self.audio.channels as usize;
                    let lvl = self.settings.auto_cue_level_db;
                    let fs = { let s = self.audio.samples.load(); first_sound(s.as_slice(), ch, lvl) };
                    if fs > 0 { self.cue_point = fs; self.seek_to(fs); self.cued = true; }
                }
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
                TopScreen::Info => {
                    // INFO: the loaded track's details, in the middle band.
                    self.screen_mode = if self.screen_mode == ScreenMode::Info {
                        ScreenMode::Playback
                    } else {
                        ScreenMode::Info
                    };
                    log::info!("screen → {:?}", self.screen_mode);
                }
                TopScreen::TagList => {
                    // TAG LIST: the on-the-fly playlist, browse-style.
                    self.screen_mode = if self.screen_mode == ScreenMode::TagList {
                        ScreenMode::Playback
                    } else {
                        ScreenMode::TagList
                    };
                    log::info!("screen → {:?}", self.screen_mode);
                }
                TopScreen::Menu => {
                    // MENU / UTILITY: the settings list.
                    self.screen_mode = if self.screen_mode == ScreenMode::Menu {
                        ScreenMode::Playback
                    } else {
                        ScreenMode::Menu
                    };
                    log::info!("screen → {:?}", self.screen_mode);
                }
                TopScreen::Perform => {
                    // PERFORM: hot-cue / beat-jump pads over a compact waveform.
                    self.screen_mode = if self.screen_mode == ScreenMode::Perform {
                        ScreenMode::Playback
                    } else {
                        ScreenMode::Perform
                    };
                    self.perform_delete = false;
                    log::info!("screen → {:?}", self.screen_mode);
                }
            },
            Event::Ui(UiEvent::PerformMode(m)) => {
                self.perform_mode   = m;
                self.perform_delete = false;
                log::info!("perform pads → {m:?}");
            }
            Event::Ui(UiEvent::PerformBank) => {
                self.perform_bank ^= 1;
                log::info!("perform bank → {}", if self.perform_bank == 0 { "A–D" } else { "E–H" });
            }
            Event::Ui(UiEvent::PerformDelete) => {
                self.perform_delete = !self.perform_delete;
                log::info!("perform delete {}", if self.perform_delete { "armed" } else { "off" });
            }
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

    // ── Loop helpers ─────────────────────────────────────────────────────────

    /// Interleaved samples per beat from the beat grid; None without a grid.
    fn samples_per_beat(&self) -> Option<f64> {
        let g = self.beat_grid.as_ref()?;
        if g.bpm <= 0.0 { return None; }
        Some(self.audio.sample_rate as f64 * 60.0 / g.bpm * self.audio.channels as f64)
    }

    /// The start of the beat the position is in (floor to the grid), frame-
    /// aligned — where a BEAT LOOP begins, so it includes what's playing now.
    fn beat_floor(&self, pos: f64) -> u64 {
        let ch = self.audio.channels as u64;
        let snapped = match (self.beat_grid.as_ref(), self.samples_per_beat()) {
            (Some(g), Some(per_beat)) => {
                let anchor = g.anchor_sample as f64 * ch as f64;   // frames → interleaved
                let n = ((pos - anchor) / per_beat).floor();
                (anchor + n * per_beat).max(0.0)
            }
            _ => pos.max(0.0),
        };
        ((snapped as u64) / ch) * ch
    }

    /// A point the DJ just set (CUE, hot cue, LOOP IN/OUT): the nearest beat on
    /// the grid when QUANTIZE is on, else the exact position; frame-aligned.
    fn quantized(&self, pos: f64) -> u64 {
        let ch = self.audio.channels as u64;
        let p = match (self.settings.quantize, self.beat_grid.as_ref(), self.samples_per_beat()) {
            (true, Some(g), Some(per_beat)) => {
                let anchor = g.anchor_sample as f64 * ch as f64;
                let n = ((pos - anchor) / per_beat).round();
                (anchor + n * per_beat).max(0.0)
            }
            _ => pos.max(0.0),
        };
        ((p as u64) / ch) * ch
    }

    /// Arm the loop [start, end) — the processor wraps inside its blocks.
    fn set_loop(&mut self, start: u64, end: u64, beats: f32) {
        let end = end.min(self.audio.len() as u64);
        if end <= start { return; }
        self.audio.loop_start.store(start, Ordering::Relaxed);
        self.audio.loop_end.store(end, Ordering::Relaxed);
        let was_active = self.audio.loop_active.swap(true, Ordering::Relaxed);
        self.loop_beats = beats;
        if !was_active { self.slip_begin(); }
        let sr_ch = self.audio.sample_rate as f64 * self.audio.channels as f64;
        log::info!("loop: {:.2}s → {:.2}s ({})", start as f64 / sr_ch, end as f64 / sr_ch,
                   if beats > 0.0 { format!("{beats} beats") } else { "in/out".into() });
    }

    /// Leave the loop; its bounds stay for RELOOP.  (No SLIP jump — a manual
    /// seek out of a loop goes where the DJ pointed.)
    fn exit_loop(&mut self) {
        if self.audio.loop_active.swap(false, Ordering::Relaxed) {
            log::info!("loop: exit");
        }
    }

    /// Leave the loop by the pad / RELOOP: with SLIP engaged, jump to where the
    /// track would have been had it never looped.
    fn exit_loop_slip(&mut self) {
        self.exit_loop();
        self.slip_end();
    }

    // ── SLIP ─────────────────────────────────────────────────────────────────

    /// Where the track would be now if the slip action had never happened.
    fn slip_shadow(&self) -> Option<f64> {
        let (pos, consumed_then) = self.slip_anchor?;
        let consumed = self.audio.source_consumed.load(Ordering::Relaxed);
        Some((pos + consumed.saturating_sub(consumed_then) as f64).min(self.audio.len() as f64))
    }

    /// Start shadowing (SLIP on, and not already shadowing).
    fn slip_begin(&mut self) {
        if self.slip && self.slip_anchor.is_none() {
            self.slip_anchor = Some((self.smoothed_pos.max(0.0), self.audio.source_consumed.load(Ordering::Relaxed)));
            log::info!("slip: shadowing from {:.2}s", self.smoothed_pos / (self.audio.sample_rate as f64 * self.audio.channels as f64));
        }
    }

    /// The slip action ended: snap to the shadow and stop shadowing.
    fn slip_end(&mut self) {
        if let Some(shadow) = self.slip_shadow() {
            let ch = self.audio.channels as u64;
            let target = ((shadow as u64) / ch) * ch;
            self.slip_anchor = None;
            self.seek_to(target);
            log::info!("slip: back to {:.2}s", target as f64 / (self.audio.sample_rate as f64 * ch as f64));
        }
    }

    fn seek_to(&mut self, pos: u64) {
        let pos = pos.min(self.audio.len() as u64);
        // Jumping out of an active loop leaves it (bounds kept for RELOOP), as
        // a hot cue or needle search does on the unit.  A seek also cancels
        // any SLIP shadow: the DJ pointed somewhere, go there.
        if self.audio.loop_active.load(Ordering::Relaxed) {
            let (s, e) = (self.audio.loop_start.load(Ordering::Relaxed), self.audio.loop_end.load(Ordering::Relaxed));
            if pos < s || pos >= e { self.exit_loop(); }
        }
        self.slip_anchor = None;
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
    fn grid_from_anlz(anlz: &std::path::Path, deck_sr: u32, ch: u8) -> Option<AnlzLoad> {
        let a = opendeck_rekordbox::read_anlz(anlz)
            .map_err(|e| log::warn!("ANLZ {}: {e:#}", anlz.display())).ok()?;
        Self::anlz_to_grid(a, deck_sr, ch)
    }

    /// Same as `grid_from_anlz` but from ANLZ bytes read over the network.
    fn grid_from_anlz_bytes(bytes: &[u8], deck_sr: u32, ch: u8) -> Option<AnlzLoad> {
        let a = opendeck_rekordbox::read_anlz_from(&mut std::io::Cursor::new(bytes.to_vec()))
            .map_err(|e| log::warn!("ANLZ (link): {e:#}")).ok()?;
        Self::anlz_to_grid(a, deck_sr, ch)
    }

    fn anlz_to_grid(a: opendeck_rekordbox::RbAnalysis, deck_sr: u32, ch: u8) -> Option<AnlzLoad> {
        let first = *a.beats.first()?;
        let anchor = (first.time_ms as u64 * deck_sr as u64) / 1000;      // frames
        let mut grid = BeatGrid::new_constant(anchor, first.bpm as f64);
        grid.downbeat_offset = first.beat_in_bar.saturating_sub(1) % 4;
        grid.confidence = 1.0;
        // All memory points, as interleaved sample indices (already sorted by
        // the reader); the first doubles as the load cue.
        let to_samples = |ms: u32| (ms as u64 * deck_sr as u64) / 1000 * ch as u64;
        let memory_cues: Vec<u64> = a.memory_cues.iter().map(|c| to_samples(c.time_ms)).collect();
        let cue = memory_cues.first().copied().unwrap_or(0);
        let mut hot_cues = [None; 8];
        for c in &a.hot_cues {
            if let Some(n) = c.hot_cue {
                if (n as usize) < 8 { hot_cues[n as usize] = Some(to_samples(c.time_ms)); }
            }
        }
        Some(AnlzLoad { grid, cue, memory_cues, hot_cues })
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
        let (samples, sr, ch, tags) = audio::decode_file(path)?;
        let deck_sr = self.audio.sample_rate;
        let grid_cue = analyze.and_then(|p| Self::grid_from_anlz(p, deck_sr, ch as u8));
        self.path = path.to_path_buf();
        self.track_tags = tags;
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
        let (samples, sr, ch, tags) = audio::decode_bytes(audio, rel_path.rsplit('.').next())?;
        self.track_tags = tags;
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
                   grid_cue: Option<AnlzLoad>, t0: Instant) -> Result<()> {
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

        // Prefer rekordbox's grid+cues; fall back to freedj's detector otherwise.
        let (beat_grid, cue_pt, memory_cues, hot_cues, grid_src) = match grid_cue {
            Some(AnlzLoad { grid, cue, memory_cues, hot_cues }) => (Some(grid), cue, memory_cues, hot_cues, "rekordbox"),
            None => {
                let mut ba = BeatAnalyzerImpl::new(deck_sr);
                ba.push(&samples, deck_sr);
                (ba.beat_grid().map(|g| (*g).clone()), 0, Vec::new(), [None; 8], "freedj")
            }
        };
        self.memory_cues    = memory_cues;
        self.hot_cues       = hot_cues;
        self.perform_delete = false;
        // AUTO CUE: with no rekordbox memory cue, cue at the first audible sound
        // rather than 0:00 (the unit's behaviour; the A.CUE badge).  A memory
        // cue still wins — that is the DJ's own choice.
        let cue_pt = if self.auto_cue && cue_pt == 0 { first_sound(&samples, ch, self.settings.auto_cue_level_db) } else { cue_pt };

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
        // Park the deck at the cue (a CDJ sits at the cue after load) so PLAY
        // starts there — with AUTO CUE that's the first sound, not the leader.
        if cue_pt > 0 { self.seek_to(cue_pt); }
        log::info!(
            "loaded {} in {:.1}s ({} BPM, {} grid, cue {:.2}s)",
            name, t0.elapsed().as_secs_f32(),
            self.beat_grid.as_ref().map(|g| format!("{:.1}", g.bpm)).unwrap_or_else(|| "?".into()),
            grid_src, cue_pt as f64 / (deck_sr as f64 * ch as f64),
        );
        Ok(())
    }

    fn render_frame(&mut self) {
        // While occluded (iOS background) don't touch the GPU: the surface is
        // invalid and any acquire/present risks the background scene-update
        // watchdog.  A stray redraw in this state is a no-op until Occluded(false)
        // reconfigures and resumes.
        if self.occluded { return; }
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

        let slip_shadow = self.slip_shadow().map(|v| v as u64);
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
        // Audible position.  Inside an active loop the processor's cursor has
        // already wrapped while the loop's tail is still in flight, so walk the
        // in-flight distance back MODULO the loop — otherwise the estimate lands
        // before loop_start and the playhead lurches.
        let loop_active = self.audio.loop_active.load(Ordering::Relaxed);
        let (ls, le) = (self.audio.loop_start.load(Ordering::Relaxed), self.audio.loop_end.load(Ordering::Relaxed));
        let heard = {
            let h = raw_pos as i64 - in_flight as i64;
            if loop_active && le > ls && h < ls as i64 {
                let len = (le - ls) as i64;
                (ls as i64 + ((h - ls as i64) % len + len) % len) as f64
            } else { h.max(0) as f64 }
        };
        // The audible wrap (end → start) is an EXPECTED backward jump: snap the
        // playhead to it instead of riding it out as a transient.
        let wrapped = loop_active && le > ls && (self.smoothed_pos - heard) > (le - ls) as f64 * 0.5;

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
        if self.smoothed_pos <= 0.0 || self.resync_frames >= 4 || wrapped {
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
            key_lock: self.key_lock, remain_mode: self.remain_mode, auto_cue: self.auto_cue, slip: self.slip,
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
            player: self.link.player,
            cue_point: self.cue_point,
            hot_cues: self.hot_cues, perform_mode: self.perform_mode, perform_bank: self.perform_bank, perform_delete: self.perform_delete,
            loop_active: self.audio.loop_active.load(Ordering::Relaxed),
            loop_start: self.audio.loop_start.load(Ordering::Relaxed),
            loop_end: self.audio.loop_end.load(Ordering::Relaxed),
            loop_beats: self.loop_beats,
            tempo_range: self.settings.tempo_range, quantize: self.settings.quantize,
            slip_shadow,
        };
        let _t_snap = Instant::now();
        let snap = make_snapshot(&self.path, self.beat_grid.as_ref(), &self.memory_cues, &self.track_tags, &self.audio, flags, pos, playing, speed, fader_speed, beat2_bpm, beat2_phase_beats, beat2_bib_v);
        perf_accum("make_snapshot", _t_snap.elapsed());

        // Screen layout in logical points; the shader gets its two rects in pixels.
        let ppp  = window.scale_factor() as f32;
        let size = window.inner_size();
        let win  = egui::Rect::from_min_size(egui::Pos2::ZERO,
                       egui::Vec2::new(size.width as f32 / ppp, size.height as f32 / ppp));
        // Load the deck photo for any chrome mode.  Landscape paints it as the
        // whole body; portrait keeps a synthesised body but lifts the jog platter
        // and fader slot out of the photo (see chrome_tex below).
        if (self.faceplate || self.portrait) && !self.face_tex_tried {
            self.face_tex_tried = true;
            self.face_tex = load_face_texture(&self.egui_ctx);
        }
        // Faceplate renders the screen into a sub-rect of the deck body; with
        // --faceplate off we fill the whole window, screen-only.
        // The faceplate's control fractions are measured off the deck photo, so
        // the layout needs a rect with the deck's proportions — but NOT the photo
        // itself, which is optional (not redistributable, so usually absent).
        // Deriving the rect from a fixed aspect when there is no photo keeps the
        // faceplate usable: the screen stays correctly proportioned inside the
        // window instead of stretching to it, and the transport stays reachable.
        let chrome = self.faceplate || self.portrait;
        let base = chrome.then(|| {
            if self.portrait {
                // Portrait chrome is synthesised (no fixed-aspect photo body to
                // preserve), so it should fill the whole screen.  On iOS the
                // window already IS the iPad — use it directly; fit_contain would
                // leave a thin letterbox border whenever the device aspect isn't
                // exactly PORTRAIT_ASPECT.  On desktop, letterbox to the iPad
                // aspect so the preview stays true to the device.
                if cfg!(target_os = "ios") { win } else { fit_contain(screen::PORTRAIT_ASPECT, win) }
            } else {
                let aspect = self.face_tex.as_ref().map_or(screen::FACE_ASPECT, |t| t.size_vec2());
                fit_contain(aspect, win)
            }
        });
        let (screen_rect, face) = match base {
            Some(b) => {
                let (s, f) = if self.portrait { screen::portrait_layout(b) } else { screen::faceplate_layout(b) };
                (s, Some(f))
            }
            None => (win, None),
        };
        let lay  = screen::layout(screen_rect);
        let px   = |r: egui::Rect| [r.min.x * ppp, r.min.y * ppp, r.width() * ppp, r.height() * ppp];
        let perform = self.screen_mode == ScreenMode::Perform;
        // PERFORM shrinks the enlarged waveform to a strip above the pads.
        let wave_rect = if perform { screen::perform_layout(screen_rect).wave } else { lay.wave };
        let vp   = renderer::Viewports { wave: px(wave_rect), overview: px(lay.overview), dim_played: self.remain_mode };

        // Build egui overlay.
        let raw = egui_state.take_egui_input(window.as_ref());
        let mut touch = Vec::new();
        let _t_run = Instant::now();
        let view = match self.screen_mode {
            ScreenMode::Playback => screen::ScreenView::Playback,
            ScreenMode::Browse   => screen::ScreenView::Browse(&self.browser),
            ScreenMode::Info     => screen::ScreenView::Info,
            ScreenMode::Perform  => screen::ScreenView::Perform,
            ScreenMode::TagList  => screen::ScreenView::TagList,
            ScreenMode::Menu     => screen::ScreenView::Menu(&self.settings, self.menu_cursor),
        };
        let face_ref = face.as_ref();
        // Landscape paints the photo as the deck body; portrait doesn't (its body
        // is synthesised) but passes the texture as chrome_tex for jog/fader sprites.
        let face_img = match (self.face_tex.as_ref(), base) {
            (Some(t), Some(b)) if !self.portrait => Some((t, b)),
            _ => None,
        };
        let chrome_tex = if self.portrait { self.face_tex.as_ref() } else { None };
        let mut output = self.egui_ctx.run(raw, |ctx| screen::draw(ctx, &snap, &lay, view, &self.tag_list, face_ref, face_img, chrome_tex, &mut touch));
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
        let slip_shadow = self.slip_shadow().map(|v| v as u64);
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
            key_lock: self.key_lock, remain_mode: self.remain_mode, auto_cue: self.auto_cue, slip: self.slip,
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
            player: self.link.player,
            cue_point: self.cue_point,
            hot_cues: self.hot_cues, perform_mode: self.perform_mode, perform_bank: self.perform_bank, perform_delete: self.perform_delete,
            loop_active: self.audio.loop_active.load(Ordering::Relaxed),
            loop_start: self.audio.loop_start.load(Ordering::Relaxed),
            loop_end: self.audio.loop_end.load(Ordering::Relaxed),
            loop_beats: self.loop_beats,
            tempo_range: self.settings.tempo_range, quantize: self.settings.quantize,
            slip_shadow,
        };
        let snap = make_snapshot(&self.path, self.beat_grid.as_ref(), &self.memory_cues, &self.track_tags, &self.audio, flags, pos, playing, speed, fader_speed, beat2_bpm, beat2_phase_beats, beat2_bib_v);

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

        // Heartbeat: measured frame rate and playhead, every ~5s at 60fps.  The
        // spike detector only fires on gaps, so a UI that has stopped entirely
        // produces no output at all — which is indistinguishable in a log from a
        // UI that is running fine.  This says which.
        if self.frame_count % 300 == 0 {
            let fps = 300.0 / self.fps_window.elapsed().as_secs_f64();
            self.fps_window = Instant::now();
            log::info!(
                "frames: {} @ {:.1} fps, playhead {:.2}s, {} spikes",
                self.frame_count, fps, pos as f64 / sr_ch, self.frame_spikes,
            );
        }
    }
}

/// Keep the iPad screen awake while the deck is in the foreground — a locked
/// screen means no touch control, so DJ apps disable auto-lock while active.
/// iOS resets `idleTimerDisabled` to false on backgrounding, so we re-assert it
/// on every foreground.  Implemented in ios/freedj/main.m; a no-op elsewhere.
#[cfg(target_os = "ios")]
extern "C" {
    fn freedj_set_idle_timer_disabled(disabled: bool);
}
fn set_idle_timer_disabled(disabled: bool) {
    #[cfg(target_os = "ios")]
    unsafe { freedj_set_idle_timer_disabled(disabled); }
    #[cfg(not(target_os = "ios"))]
    let _ = disabled;
}

/// What a rekordbox ANLZ contributes to a load: the beat grid, the load cue
/// (its first memory point), every memory point, and hot cues A–H — all as
/// interleaved samples.
struct AnlzLoad { grid: BeatGrid, cue: u64, memory_cues: Vec<u64>, hot_cues: [Option<u64>; 8] }

// ── Memory-point lookup ────────────────────────────────────────────────────────
// `cues` is sorted, interleaved sample indices.  `tol` is the match tolerance
// (a paused cue and an ms-quantised ANLZ point must count as the same point,
// and CALL must not re-select the point the deck is already sitting on).

/// Index of the memory point at `at` (within `tol`), for DELETE / dedupe.
fn memory_at(cues: &[u64], at: u64, tol: u64) -> Option<usize> {
    cues.iter().position(|&m| m.abs_diff(at) <= tol)
}
/// The first memory point strictly after `pos` (beyond `tol`) — CALL ▶.
fn memory_next(cues: &[u64], pos: u64, tol: u64) -> Option<u64> {
    cues.iter().find(|&&m| m > pos.saturating_add(tol)).copied()
}
/// The last memory point strictly before `pos` (beyond `tol`) — CALL ◀.
fn memory_prev(cues: &[u64], pos: u64, tol: u64) -> Option<u64> {
    cues.iter().rev().find(|&&m| m.saturating_add(tol) < pos).copied()
}

/// Interleaved index of the first sample louder than `level_db` (the AUTO CUE
/// LEVEL menu setting, -36…-78 dB), frame-aligned — where AUTO CUE places the
/// cue on load.  0 if silent.
fn first_sound(samples: &[f32], ch: usize, level_db: f32) -> u64 {
    let thr = 10f32.powf(level_db / 20.0);
    match samples.iter().position(|&s| s.abs() > thr) {
        Some(i) => ((i / ch) * ch) as u64,
        None => 0,
    }
}

impl ApplicationHandler for DeckApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // already initialised (e.g. Android resume)
        }

        // Screen-only is the 7" panel; the faceplate window matches the deck
        // photo's aspect (the image is aspect-fit inside it).
        let (win_w, win_h) = if self.portrait { (966u32, 1288u32) }   // iPad 13" portrait (0.75)
                             else if self.faceplate { (860u32, 1090u32) } else { (1024u32, 600u32) };
        let mut attrs = WindowAttributes::default().with_title("freedj-3000");
        // Only ask for a size where a window manager can grant one.  On iOS the
        // requested size is applied to the UIWindow, so asking for 860x1090 got
        // the deck drawn into a box that size in the corner of the iPad instead
        // of filling the display; the layout aspect-fits the screen it is given,
        // so letting it have the whole thing is both correct and what we want.
        #[cfg(not(target_os = "ios"))]
        {
            attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(win_w, win_h));
        }
        #[cfg(target_os = "ios")]
        let _ = (win_w, win_h);

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        let sz = window.inner_size();
        log::info!("window {}x{} px, scale {:.2}", sz.width, sz.height, window.scale_factor());

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

        // Foreground and active: hold the screen awake (cold-launch case; the
        // Occluded handler re-asserts it on every later foreground).
        set_idle_timer_disabled(true);
    }

    // Deliberately no `suspended()` teardown.  On winit's iOS backend Suspended
    // fires on `applicationWillResignActive` — every Control Center pull, banner,
    // or alert, not just a real background — so tearing the window/surface down
    // here would churn the UIWindow constantly and could leave us frozen on a
    // stale frame.  Background handling instead hangs off WindowEvent::Occluded
    // (applicationDidEnterBackground/willEnterForeground), which is the true,
    // stable foreground/background signal; see the Occluded arm below.

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
                // In a list screen (BROWSE / TAG LIST) the keys navigate the
                // list, not the transport.  G = TAG TRACK / REMOVE.
                if matches!(self.screen_mode, ScreenMode::Browse | ScreenMode::TagList | ScreenMode::Menu) {
                    let here = match self.screen_mode {
                        ScreenMode::Browse => TopScreen::Browse, ScreenMode::TagList => TopScreen::TagList, _ => TopScreen::Menu,
                    };
                    let ev = match code {
                        ArrowDown           => Some(Event::Deck(ControlEvent::BrowseEncoderDelta { delta: 1 })),
                        ArrowUp             => Some(Event::Deck(ControlEvent::BrowseEncoderDelta { delta: -1 })),
                        Enter | NumpadEnter => Some(Event::Deck(ControlEvent::Load)),
                        Backspace           => Some(Event::Deck(ControlEvent::Back)),
                        KeyG                => Some(Event::Ui(UiEvent::TagTrack)),
                        KeyB | Escape       => Some(Event::Ui(UiEvent::Screen(here))),   // toggle off
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
                let range   = self.settings.tempo_range;
                let fader   = input::speed_to_fader(f32::from_bits(self.fader_speed.load(Ordering::Relaxed)), range);
                let step    = 0.01 / (2.0 * range);   // ±1% per press, as the DJ2Go buttons
                let ev = match code {
                    Space      => Some(Event::Deck(if playing { ControlEvent::Pause } else { ControlEvent::Play })),
                    ArrowRight => Some(Event::Deck(ControlEvent::NeedleSearch { position: (frac + ten_s).min(1.0) })),
                    ArrowLeft  => Some(Event::Deck(ControlEvent::NeedleSearch { position: (frac - ten_s).max(0.0) })),
                    Equal | NumpadAdd      => Some(Event::Deck(ControlEvent::TempoFader { position: fader + step })),
                    Minus | NumpadSubtract => Some(Event::Deck(ControlEvent::TempoFader { position: fader - step })),
                    Digit0 | Numpad0       => Some(Event::Deck(ControlEvent::TempoFader { position: 0.5 })),
                    KeyK => Some(Event::Deck(ControlEvent::KeyLockToggle)),
                    // Memory points: N sets one at the cue, [ / ] CALL ◀ / ▶, Delete removes.
                    KeyN         => Some(Event::Deck(ControlEvent::MemoryCueSet)),
                    BracketLeft  => Some(Event::Deck(ControlEvent::MemoryCueCall { next: false })),
                    BracketRight => Some(Event::Deck(ControlEvent::MemoryCueCall { next: true })),
                    Delete       => Some(Event::Deck(ControlEvent::MemoryCueDelete)),
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

            // iOS background/foreground (Occluded true = went to background).
            // In background the CAMetalLayer surface is invalidated AND we must
            // not keep the main thread busy: 0.1.0 spun the redraw loop against
            // the dead surface, burning 86s of CPU until iOS killed it with the
            // background scene-update watchdog (0x8BADF00D).  So: stop rendering
            // and stop requesting redraws while occluded — the run loop goes idle
            // and the app suspends cleanly.  On return, reconfigure the surface
            // (re-establishes drawables the background invalidated) and kick a
            // redraw to resume.  Desktop never emits this, so it's iOS-only.
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                // Release the auto-lock hold in the background, re-assert it on
                // foreground (iOS resets idleTimerDisabled when backgrounding).
                set_idle_timer_disabled(!occluded);
                if occluded {
                    log::info!("occluded (background): pausing render");
                } else {
                    log::info!("un-occluded (foreground): reconfiguring surface, resuming");
                    if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
                        let sz = w.inner_size();
                        r.resize(sz.width, sz.height);   // reconfigure at current size
                        w.request_redraw();
                    }
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
                // vsync slot.  While occluded (iOS background) we stop the loop
                // here so the app can idle and suspend; Occluded(false) restarts
                // it.
                if !self.occluded {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Occluded (iOS background): idle completely — no self-driven frames, no
        // WaitUntil timer to keep waking us.  Occluded(false) requests a redraw
        // to restart the loop.
        if self.occluded {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
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

/// Load the faceplate background image (default `reference/photos/XDJ1000Mk2-
/// faceplate.jpg`, a tracked photo of our own unit; override with
/// OPENDECK_FACEPLATE_IMG).  On iOS the file is bundled flat in the .app, so we
/// also try the basename.  Absent → the deck falls back to drawn primitives.
fn load_face_texture(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let configured = std::env::var("OPENDECK_FACEPLATE_IMG")
        .unwrap_or_else(|_| "reference/photos/XDJ1000Mk2-faceplate.jpg".to_string());
    // Resolve against several roots and take the first that exists:
    //   1. the configured path as-is (desktop, or an absolute override),
    //   2. its basename next to the executable — the flat iOS/.app bundle layout,
    //      resolved from current_exe() so it works even if the process cwd was
    //      never chdir'd into the bundle (the iOS entry point ignores chdir
    //      errors, so we must not depend on cwd), and
    //   3. its basename relative to cwd (the desktop fresh-checkout fallback).
    let base = std::path::Path::new(&configured).file_name().map(|n| n.to_string_lossy().into_owned());
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from));
    let candidates = [
        Some(PathBuf::from(&configured)),
        base.as_ref().and_then(|b| exe_dir.as_ref().map(|d| d.join(b))),
        base.as_ref().map(PathBuf::from),
    ];
    let file = candidates.into_iter().flatten().find(|p| p.exists());

    // Prefer a real file (so OPENDECK_FACEPLATE_IMG can override), but fall back
    // to the copy COMPILED INTO THE BINARY.  On iOS the bundled file + cwd proved
    // unreliable (the jog/fader rendered as a flat circle because the photo never
    // loaded); a baked-in image removes the whole "is it bundled / can we find
    // it" failure class, so the skin is always available on every platform.
    let (rgba, w, h) = file
        .and_then(|p| {
            let path = p.to_string_lossy().into_owned();
            let bytes = std::fs::read(&p)
                .map_err(|e| log::warn!("faceplate image: cannot read {path}: {e}"))
                .ok()?;
            match decode_rgba(&bytes, &path) {
                Some(d) => { log::info!("faceplate image: {path} ({}x{})", d.1, d.2); Some(d) }
                None    => { log::warn!("faceplate image: could not decode {path}"); None }
            }
        })
        .or_else(|| {
            log::info!("faceplate image: using embedded copy ({} bytes)", FACEPLATE_EMBEDDED.len());
            decode_rgba(FACEPLATE_EMBEDDED, "XDJ1000Mk2-faceplate.jpg")
        })?;
    let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
    Some(ctx.load_texture("faceplate", img, egui::TextureOptions::LINEAR))
}

/// The faceplate photo baked into the binary — the guaranteed fallback when no
/// file is found (notably on iOS).  It is the same tracked, branding-redacted
/// photo of our own unit that `bundle-track.sh` copies into the .app.
const FACEPLATE_EMBEDDED: &[u8] =
    include_bytes!("../../../reference/photos/XDJ1000Mk2-faceplate.jpg");

/// Decode a JPEG or PNG to RGBA8 (jpeg-decoder + the png crate already vendored).
fn decode_rgba(bytes: &[u8], path: &str) -> Option<(Vec<u8>, usize, usize)> {
    if path.to_ascii_lowercase().ends_with(".png") {
        let mut reader = png::Decoder::new(bytes).read_info().ok()?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width as usize, info.height as usize);
        let rgba = match info.color_type {
            png::ColorType::Rgba      => buf[..w * h * 4].to_vec(),
            png::ColorType::Rgb       => buf[..w * h * 3].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            png::ColorType::Grayscale => buf[..w * h].iter().flat_map(|&v| [v, v, v, 255]).collect(),
            _ => return None,
        };
        Some((rgba, w, h))
    } else {
        let mut dec = jpeg_decoder::Decoder::new(bytes);
        let pixels = dec.decode().ok()?;
        let info = dec.info()?;
        let (w, h) = (info.width as usize, info.height as usize);
        let rgba = match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => pixels.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            jpeg_decoder::PixelFormat::L8    => pixels.iter().flat_map(|&v| [v, v, v, 255]).collect(),
            _ => return None,
        };
        Some((rgba, w, h))
    }
}

/// Aspect-fit (contain) a texture of size `ts` into `win`, centered.
fn fit_contain(ts: egui::Vec2, win: egui::Rect) -> egui::Rect {
    let s = (win.width() / ts.x).min(win.height() / ts.y);
    egui::Rect::from_center_size(win.center(), ts * s)
}

/// Everything `run` needs to start a deck.  On desktop this is filled from CLI
/// args; a mobile entry point (iOS `main` / Android `android_main`) fills it from
/// bundled resources / platform config and calls `run` directly.
pub struct Config {
    /// The track to load at startup; `None` boots to an empty deck (browse + LOAD).
    pub track:        Option<PathBuf>,
    /// Player number given explicitly (--player / OPENDECK_PLAYER); wins over
    /// the persisted MENU setting.
    pub player:       Option<u8>,
    /// Platform default when neither is set: 1 on the desktop, 3 on the iPad.
    pub default_player: u8,
    pub deck_channel: u8,   // 0 = A/left (MIDI ch 0), 1 = B/right (ch 1)
    pub link_send:    bool,
}

fn init_logging() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .format_timestamp_micros()   // beat and frame timing are measured from the log
    .init();
}

pub fn desktop_main() -> Result<()> {
    init_logging();

    // Args: <file> [--player N].  OPENDECK_PLAYER also works.
    let mut args = std::env::args().skip(1);
    let mut path: Option<PathBuf> = None;
    let mut player: Option<u8> = std::env::var("OPENDECK_PLAYER").ok().and_then(|v| v.parse().ok());
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
            "--player" => player = Some(args.next().and_then(|v| v.parse().ok()).context("--player needs a number 1-6 (5-6 only on an all-CDJ-3000 network)")?),
            "--deck" => deck_channel = match args.next().as_deref() {
                Some("A") | Some("a") => 0,
                Some("B") | Some("b") => 1,
                _ => bail!("--deck needs A or B"),
            },
            "--link-send" => link_send = true,
            "--link-receive-only" => link_send = false,
            "--faceplate" => std::env::set_var("OPENDECK_FACEPLATE", "1"),
            "--portrait"  => std::env::set_var("OPENDECK_PORTRAIT", "1"),  // iPad 13" portrait chrome
            _ => path = Some(a.into()),
        }
    }
    // A track is optional now: with none, freedj boots to an empty deck.
    if let Some(p) = &path {
        if !p.exists() {
            bail!("file not found: {}", p.display());
        }
    }

    run(Config { track: path, player, default_player: 1, deck_channel, link_send })
}

/// Start a deck and run the UI event loop.  Platform-agnostic: every entry point
/// (desktop `main`, iOS/Android) builds a `Config` and calls this.
pub fn run(cfg: Config) -> Result<()> {
    let track        = cfg.track;
    // Pro DJ Link player numbers: 1-4 is the universal space (CDJ-2000/NXS2,
    // XDJ, mixed rigs). The CDJ-3000 raised it to 6, but ONLY when every linked
    // device is a CDJ-3000; against an XDJ/mixed network 5-6 are invalid and the
    // player refuses master handoff to one. We allow 1-6; picking 5-6 only works
    // on an all-CDJ-3000 network. See docs/design/prodj-link-players.md.
    // MENU / UTILITY settings persist across launches; an explicit --player /
    // OPENDECK_PLAYER still wins over the persisted PLAYER No.
    let settings     = settings::Settings::load(cfg.default_player);
    let player       = cfg.player.unwrap_or(settings.player).clamp(1, 6);
    let deck_channel = cfg.deck_channel;
    let link_send    = cfg.link_send;

    // ── 1. Decode audio (or boot to an empty deck when no track is given) ─────
    let audio = match &track {
        Some(p) => AudioHandle::open(p)?,
        None => {
            log::info!("no track given — booting to an empty deck (browse + LOAD to play)");
            AudioHandle::open_empty()?
        }
    };

    // ── 2. Build waveform + detect beat grid (synchronous, before window opens) ─
    let samples_arc = audio.current();
    log::info!("computing waveform ({} samples)...", samples_arc.len());
    let t0 = Instant::now();
    let mut waveform_builder = WaveformBuilder::new(audio.sample_rate);
    let mut beat_analyzer    = BeatAnalyzerImpl::new(audio.sample_rate);
    if !samples_arc.is_empty() {
        waveform_builder.push(&samples_arc);
        beat_analyzer.push(&samples_arc, audio.sample_rate);
    }
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
    // Dev hook: OPENDECK_PITCH=+0.06 sets the initial fader for layout screenshots.
    let init_pitch = std::env::var("OPENDECK_PITCH").ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|p| (1.0 + p).clamp(1.0 - 0.16, 1.0 + 0.16))
        .unwrap_or(1.0);
    let fader_speed  = Arc::new(AtomicU32::new(init_pitch.to_bits()));
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

    let mut app = DeckApp::new(track.unwrap_or_default(), waveform, audio, beat_grid, fader_speed, beat2_bpm, beat2_anchor, beat2_player, beat2_bib, link, link_grid, Arc::clone(&link_send_flag), event_rx, settings);
    // Park the startup track at its cue (AUTO CUE's first sound, or the
    // OPENDECK_CUE override) so PLAY starts there, as after a browser LOAD.
    if app.cue_point > 0 { let c = app.cue_point; app.seek_to(c); }
    // Dev: OPENDECK_SLIP=1 engages SLIP at startup; OPENDECK_LOOP=start_secs,
    // beats arms a beat loop on the startup track (e.g. "30,4") — together they
    // exercise the loop engine, SLIP's shadow and both displays headlessly.
    if std::env::var("OPENDECK_SLIP").as_deref() == Ok("1") { app.slip = true; }
    if let Ok(v) = std::env::var("OPENDECK_LOOP") {
        let parts: Vec<f64> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if let ([secs, beats], Some(per_beat)) = (parts.as_slice(), app.samples_per_beat()) {
            let ch = app.audio.channels as u64;
            let sr_ch = app.audio.sample_rate as f64 * ch as f64;
            let start = app.beat_floor(secs * sr_ch);
            let len = (((beats * per_beat) as u64) / ch) * ch;
            app.seek_to(start);                          // seek first: a seek cancels SLIP
            app.set_loop(start, start + len, *beats as f32);   // …then arm, which anchors the shadow
        }
    }
    event_loop.run_app(&mut app).context("event loop error")?;

    Ok(())
}

#[cfg(test)]
mod memory_cue_tests {
    use super::{memory_at, memory_next, memory_prev, first_sound};

    // Points at 1s, 2s, 3s (interleaved, 44.1k stereo); tolerance 64 frames.
    const CUES: [u64; 3] = [88_200, 176_400, 264_600];
    const TOL: u64 = 2 * 64;

    #[test]
    fn call_steps_past_the_point_the_deck_sits_on() {
        // Sitting exactly on the 2s point: ▶ must go to 3s, ◀ to 1s — never
        // re-select 2s (which would make CALL feel stuck).
        assert_eq!(memory_next(&CUES, 176_400, TOL), Some(264_600));
        assert_eq!(memory_prev(&CUES, 176_400, TOL), Some(88_200));
    }

    #[test]
    fn call_from_between_points() {
        assert_eq!(memory_next(&CUES, 100_000, TOL), Some(176_400));
        assert_eq!(memory_prev(&CUES, 100_000, TOL), Some(88_200));
    }

    #[test]
    fn call_at_the_ends_has_nowhere_to_go() {
        assert_eq!(memory_next(&CUES, 264_600, TOL), None);
        assert_eq!(memory_prev(&CUES, 88_200, TOL), None);
        assert_eq!(memory_prev(&CUES, 0, TOL), None);
    }

    #[test]
    fn at_matches_within_tolerance_only() {
        // A paused cue a few frames off an ANLZ point is the same point …
        assert_eq!(memory_at(&CUES, 176_400 + 100, TOL), Some(1));
        // … but not one well away from it.
        assert_eq!(memory_at(&CUES, 176_400 + 10_000, TOL), None);
        assert_eq!(memory_at(&[], 5, TOL), None);
    }

    #[test]
    fn first_sound_skips_the_leader_and_frame_aligns() {
        // Stereo: 3 silent frames, then sound at frame 3 (index 6/7).
        let s = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.1, 0.1];
        assert_eq!(first_sound(&s, 2, -48.0), 6);
        // Sound first appears in the RIGHT channel (odd index) → still frame-aligned.
        let r = [0.0, 0.0, 0.0, 0.5];
        assert_eq!(first_sound(&r, 2, -48.0), 2);
        assert_eq!(first_sound(&[0.0; 8], 2, -48.0), 0);
    }
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
        let AnlzLoad { grid, .. } = DeckApp::grid_from_anlz(&ap, 48_000, 2).expect("grid");
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
        let (samples, sr, ch, _tags) = crate::audio::decode_bytes(audio, ext).expect("decode over-wire audio");
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

// ── iOS entry point ───────────────────────────────────────────────────────────
//
// Built into the staticlib and called from the Xcode app's `main.m`.  winit's
// `EventLoop::run_app` drives `UIApplicationMain` internally and never returns.
// Resource paths resolve against the app bundle (the process's working dir on
// iOS); see ios/README.md.
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn freedj_ios_main() {
    init_logging();

    // iOS gives the process no useful working directory, so every relative path
    // the desktop build uses (the track, `reference/photos/...`) would miss.
    // Resources live flat inside `freedj.app`, which is the executable's parent
    // — chdir there and the existing relative paths resolve against the bundle.
    let bundle = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from));
    match &bundle {
        Some(dir) => {
            let _ = std::env::set_current_dir(dir);
            log::info!("bundle: {}", dir.display());
        }
        None => log::warn!("could not locate the app bundle — relative resources will not resolve"),
    }

    // Demo config: portrait iPad chrome, player 3 (the ADK-1000 is a drop-in
    // "deck 3" next to CDJs 1-2, so 3 avoids the default collision), Link send on.
    // OPENDECK_PLAYER overrides, matching the desktop `--player` arg; otherwise
    // the MENU's persisted PLAYER No. (seeded to 3 on first run).
    std::env::set_var("OPENDECK_PORTRAIT", "1");
    let player: Option<u8> = std::env::var("OPENDECK_PLAYER").ok().and_then(|v| v.parse().ok());

    let track = bundle.as_deref().and_then(bundled_track);
    match &track {
        Some(t) => log::info!("track: {}", t.display()),
        None => log::info!(
            "no audio file in the app bundle — booting to an empty deck; add one to \
             the target's Copy Bundle Resources phase to preload (see ios/README.md)"
        ),
    }
    if let Err(e) = run(Config { track, player, default_player: 3, deck_channel: 0, link_send: true }) {
        log::error!("freedj_ios_main: {e:#}");
    }
}

/// First playable audio file sitting in the app bundle, so whichever track is
/// dragged into Copy Bundle Resources is the one that loads — no rebuild of the
/// Rust side to change it.  Sorted for a stable pick when there is more than one.
#[cfg(target_os = "ios")]
fn bundled_track(bundle: &std::path::Path) -> Option<PathBuf> {
    const EXTS: [&str; 6] = ["mp3", "m4a", "aac", "wav", "aiff", "flac"];
    let mut found: Vec<PathBuf> = std::fs::read_dir(bundle)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}
