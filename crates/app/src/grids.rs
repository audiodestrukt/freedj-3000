//! GRID ADJUST: shifting a track's beat grid by hand, and remembering it.
//!
//! The unit's GRID ADJUST mode (the "– GRID" half of the ZOOM pill) turns the
//! browse knob into a grid nudge and offers three touch keys: RESET (back to
//! the analysed grid), SNAP GRID (CUE) (the nearest beat moves onto the cue
//! point) and SHIFT GRID (CUE) (the cue point becomes beat 1 of a bar).  A
//! corrected grid is written back to the library on the unit; here it goes
//! to `grids.json` in the app data dir, keyed by the track path, and wins over
//! the analysis on the next load.

use std::collections::HashMap;
use std::path::PathBuf;
use opendeck_types::BeatGrid;
use crate::taglist::app_data_dir;

/// Grid period in frames for a constant-tempo grid.
pub fn period_frames(g: &BeatGrid, sample_rate: u32) -> f64 {
    sample_rate as f64 * 60.0 / g.bpm
}

/// Move every beat by `delta` frames (negative = earlier).  The anchor stays
/// non-negative by wrapping whole periods; the downbeat offset follows so
/// beat-in-bar numbering is unchanged (anchor + period ⇒ every frame's beat
/// index drops by one ⇒ offset + 1).
pub fn shift(g: &mut BeatGrid, delta: i64, period: f64) {
    let per = period.round().max(1.0) as i64;
    let mut a = g.anchor_sample as i64 + delta;
    let mut off = g.downbeat_offset as i64;
    while a < 0 { a += per; off += 1; }
    g.anchor_sample = a as u64;
    g.downbeat_offset = off.rem_euclid(4) as u8;
    for b in &mut g.beats {
        *b = (*b as i64 + delta).max(0) as u64;
    }
    g.locked = true;
}

/// SNAP GRID (CUE): slide the grid so its nearest beat lands on `frame`.
/// With `downbeat`, that beat also becomes beat 1 (SHIFT GRID (CUE)).
pub fn snap_to(g: &mut BeatGrid, frame: u64, period: f64, downbeat: bool) {
    let rel = frame as f64 - g.anchor_sample as f64;
    let k = (rel / period).round();
    let residual = rel - k * period;
    shift(g, residual.round() as i64, period);
    if downbeat {
        // beat_in_bar = ((k + offset) mod 4) + 1 == 1  ⇒  offset ≡ −k (mod 4)
        g.downbeat_offset = (-(k as i64)).rem_euclid(4) as u8;
    }
}

/// Hand-corrected grids, persisted across launches.
pub struct GridStore {
    path: PathBuf,
    map:  HashMap<String, BeatGrid>,
}

impl GridStore {
    pub fn open() -> Self {
        let path = app_data_dir().join("grids.json");
        let map = std::fs::read(&path).ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        GridStore { path, map }
    }

    pub fn get(&self, key: &str) -> Option<&BeatGrid> { self.map.get(key) }

    pub fn set(&mut self, key: &str, g: &BeatGrid) {
        self.map.insert(key.to_string(), g.clone());
        self.save();
    }

    pub fn remove(&mut self, key: &str) {
        if self.map.remove(key).is_some() { self.save(); }
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() { let _ = std::fs::create_dir_all(dir); }
        match serde_json::to_vec_pretty(&self.map) {
            Ok(b) => if let Err(e) = std::fs::write(&self.path, b) {
                log::warn!("grids.json: {e}");
            },
            Err(e) => log::warn!("grids.json: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(anchor: u64, off: u8) -> BeatGrid {
        let mut g = BeatGrid::new_constant(anchor, 120.0);
        g.downbeat_offset = off;
        g
    }
    const P: f64 = 22050.0;   // 120 BPM @ 44.1k

    fn beat_in_bar(g: &BeatGrid, frame: u64) -> u8 {
        let beat = ((frame as f64 - g.anchor_sample as f64) / P).floor() as i64;
        ((beat + g.downbeat_offset as i64).rem_euclid(4) + 1) as u8
    }

    #[test]
    fn shift_moves_anchor_and_locks() {
        let mut g = grid(1000, 0);
        shift(&mut g, 250, P);
        assert_eq!(g.anchor_sample, 1250);
        assert!(g.locked);
        shift(&mut g, -50, P);
        assert_eq!(g.anchor_sample, 1200);
    }

    #[test]
    fn shift_wraps_negative_anchor_keeping_bar_numbering() {
        let mut g = grid(1000, 2);
        let before = beat_in_bar(&g, 100_000);
        shift(&mut g, -1500, P);            // anchor would be −500
        assert_eq!(g.anchor_sample, 22050 - 500);
        // Same frame, same beat-in-bar (allowing that the beat itself moved 1500
        // frames earlier: probe a frame well inside its beat).
        assert_eq!(beat_in_bar(&g, 100_000 - 1500), before);
    }

    #[test]
    fn snap_puts_nearest_beat_on_frame() {
        let mut g = grid(1000, 0);
        // Frame just after beat 3 (anchor + 3P + 300): grid slides +300.
        snap_to(&mut g, 1000 + 3 * 22050 + 300, P, false);
        assert_eq!(g.anchor_sample, 1300);
        assert_eq!(g.downbeat_offset, 0);
        // Frame just before beat 3: grid slides −200.
        let mut g = grid(1000, 0);
        snap_to(&mut g, 1000 + 3 * 22050 - 200, P, false);
        assert_eq!(g.anchor_sample, 800);
    }

    #[test]
    fn shift_grid_cue_makes_frame_beat_one() {
        let mut g = grid(1000, 1);
        let f = 1000 + 5 * 22050 + 100;
        snap_to(&mut g, f, P, true);
        assert_eq!(beat_in_bar(&g, f + 10), 1);
        assert_eq!(beat_in_bar(&g, f + 10 + 22050), 2);
    }
}
