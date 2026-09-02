//! Everything the deck knows, captured once per frame.
//!
//! The renderer and the screen chrome are *consumers* of this struct; neither
//! reads deck state directly.  That keeps the classic CDJ/XDJ layout one view
//! among several rather than the shape of the architecture, and it stops the
//! render signature growing a parameter every time the screen gains an element.

use opendeck_types::BeatGrid;

pub struct DeckSnapshot<'a> {
    /// Track name shown in the title bar (tag title, else file name).
    pub title:        &'a str,

    /// Playhead in interleaved source samples — latency-compensated and
    /// phase-locked, i.e. what the listener is hearing right now.
    pub position:     u64,
    pub sample_rate:  u32,
    pub channels:     u8,
    pub total_samples: u64,
    /// Start-cue position in source samples (CDJ CUE).
    pub cue_point:    u64,

    /// Populated for completeness; the renderer derives play/tempo from other
    /// fields today. Kept for a future TEMPO/eject readout, hence allow(dead_code).
    #[allow(dead_code)]
    pub playing:      bool,
    /// Instantaneous playback speed including jog nudges.
    #[allow(dead_code)]
    pub speed:        f32,
    /// Stable pitch-fader speed — what the TEMPO readout shows.
    pub fader_speed:  f32,
    /// Memory points (interleaved sample indices, sorted) — drawn as markers on
    /// the waveforms; CALL ◀▶ steps through them.
    pub memory_cues:  &'a [u64],
    /// The track's own tags (ID3 etc.) for the INFO screen.
    pub tags:         &'a opendeck_decode::TrackTags,
    /// The file name (INFO shows it even when the title bar uses a tagged title).
    pub file:         &'a str,
    /// Hot cues A–H (interleaved sample index), for the PERFORM pads + markers.
    pub hot_cues:     [Option<u64>; 8],
    /// PERFORM screen state.
    pub perform_mode:   crate::input::PerformMode,
    pub perform_bank:   u8,
    pub perform_delete: bool,
    /// Loop: active flag, bounds (interleaved samples), and the beat length a
    /// BEAT LOOP pad set (0 = manual IN/OUT loop) so the pad can light.
    pub loop_active:    bool,
    pub loop_start:     u64,
    pub loop_end:       u64,
    pub loop_beats:     f32,
    /// MENU settings the screen reflects: the tempo fader's range (badge +
    /// knob position) and QUANTIZE (readout).
    pub tempo_range:    f32,
    pub quantize:       bool,
    /// SLIP: the shadow playhead while a loop / held hot cue is slipping
    /// (interleaved samples) — drawn as a marker on both waveforms.
    pub slip_shadow:    Option<u64>,
    /// Master Tempo (key lock) engaged.
    pub key_lock:     bool,
    /// Time display shows remaining (REMAIN) rather than elapsed (TIME).
    pub remain_mode:  bool,
    /// AUTO CUE engaged: loads cue at the first audible sound (A.CUE badge).
    pub auto_cue:     bool,
    pub slip:         bool,
    pub sync:         bool,
    pub master:       bool,
    /// ZOOM / GRID ADJUST mode indicator.
    pub zoom_grid_mode: bool,
    /// LINK selected in the source column (else the local file / USB).
    pub source_link:  bool,
    /// Phase-meter slot shows the master-alignment tick view.
    pub phase_ticks_view: bool,
    /// A ProDJ Link peer has been heard from.
    pub linked:       bool,
    /// Player number of the external master (0 = unknown / none).
    pub master_player: u8,
    /// Our own Pro DJ Link player number (1-6).
    pub player:       u8,

    pub beat_grid:    Option<&'a BeatGrid>,

    /// External deck (ProDJ Link / Deck B) tempo; 0 = none.
    pub beat2_bpm:    f32,
    /// External deck beat phase 0.0–1.0, wall-clock driven.
    pub beat2_phase_beats: f32,
    /// External deck's beat within its bar, 1–4 (0 = unknown).
    pub beat2_beat_in_bar: u8,
}

impl DeckSnapshot<'_> {
    #[inline]
    fn samples_to_secs(&self, s: u64) -> f64 {
        s as f64 / self.channels as f64 / self.sample_rate as f64
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.samples_to_secs(self.position)
    }

    pub fn total_secs(&self) -> f64 {
        self.samples_to_secs(self.total_samples)
    }

    pub fn remaining_secs(&self) -> f64 {
        (self.total_secs() - self.elapsed_secs()).max(0.0)
    }

    /// Effective tempo at the current fader position, as a CDJ displays it.
    pub fn bpm(&self) -> Option<f64> {
        self.beat_grid.map(|g| g.bpm * self.fader_speed as f64)
    }

    /// Pitch-fader offset from the original tempo, in percent.
    pub fn tempo_percent(&self) -> f32 {
        (self.fader_speed - 1.0) * 100.0
    }

    /// Fractional position within the current beat, 0.0–1.0, from our own grid.
    pub fn beat_phase(&self) -> Option<f32> {
        let g = self.beat_grid?;
        let frames = self.position as f64 / self.channels as f64;
        let anchor = g.anchor_sample as f64;
        let period = self.sample_rate as f64 * 60.0 / g.bpm;
        let rel = (frames - anchor) / period;
        Some(rel.rem_euclid(1.0) as f32)
    }

    /// Beat number within the bar, 1–4, from our own grid.
    /// Bar within a 4-bar phrase, 1–4; advances once per bar.
    pub fn bar_in_phrase(&self) -> Option<u8> {
        let g = self.beat_grid?;
        let frames = self.position as f64 / self.channels as f64;
        let period = self.sample_rate as f64 * 60.0 / g.bpm;
        let beat = ((frames - g.anchor_sample as f64) / period).floor() as i64;
        let bar  = (beat + g.downbeat_offset as i64).div_euclid(4);
        Some((bar.rem_euclid(4) + 1) as u8)
    }

    pub fn beat_in_bar(&self) -> Option<u8> {
        let g = self.beat_grid?;
        let frames = self.position as f64 / self.channels as f64;
        let period = self.sample_rate as f64 * 60.0 / g.bpm;
        let beat = ((frames - g.anchor_sample as f64) / period).floor() as i64;
        Some(((beat + g.downbeat_offset as i64).rem_euclid(4) + 1) as u8)
    }
}
