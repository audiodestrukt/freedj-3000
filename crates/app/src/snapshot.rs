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

    pub playing:      bool,
    /// Instantaneous playback speed including jog nudges.
    pub speed:        f32,
    /// Stable pitch-fader speed — what the TEMPO readout shows.
    pub fader_speed:  f32,
    /// Master Tempo (key lock) engaged.
    pub key_lock:     bool,

    pub beat_grid:    Option<&'a BeatGrid>,

    /// External deck (ProDJ Link / Deck B) tempo; 0 = none.
    pub beat2_bpm:    f32,
    /// External deck beat phase 0.0–1.0, wall-clock driven.
    pub beat2_phase_beats: f32,
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
    pub fn beat_in_bar(&self) -> Option<u8> {
        let g = self.beat_grid?;
        let frames = self.position as f64 / self.channels as f64;
        let period = self.sample_rate as f64 * 60.0 / g.bpm;
        let beat = ((frames - g.anchor_sample as f64) / period).floor() as i64;
        Some(((beat + g.downbeat_offset as i64).rem_euclid(4) + 1) as u8)
    }
}
