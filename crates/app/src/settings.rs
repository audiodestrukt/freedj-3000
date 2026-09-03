//! MENU / UTILITY settings — the unit's scrollable list of preferences.
//! Persisted as JSON in the app's data dir (beside the tag list) so they
//! survive a relaunch.  Deliberately minimal: only settings that change real
//! behaviour here; a setting that did nothing would be a fake control.

use crate::taglist::app_data_dir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The rows of the MENU screen, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting { AutoCueLevel, Quantize, TempoRange, Player }

pub const MENU: [(Setting, &str); 4] = [
    (Setting::AutoCueLevel, "AUTO CUE LEVEL"),
    (Setting::Quantize,     "QUANTIZE"),
    (Setting::TempoRange,   "TEMPO RANGE"),
    (Setting::Player,       "PLAYER No."),
];

/// The unit's AUTO CUE LEVEL choices (dB).
pub const AUTO_CUE_LEVELS: [f32; 8] = [-36.0, -42.0, -48.0, -54.0, -60.0, -66.0, -72.0, -78.0];
/// TEMPO RANGE choices as a fraction: ±6 %, ±10 %, ±16 %, WIDE (±100 %).
pub const TEMPO_RANGES: [f32; 4] = [0.06, 0.10, 0.16, 1.00];

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Amplitude (dB) a sample must exceed to count as "sound" for AUTO CUE.
    pub auto_cue_level_db: f32,
    /// Snap CUE / hot-cue / LOOP IN / OUT to the nearest beat on the grid.
    pub quantize: bool,
    /// Tempo fader range as a fraction (0.16 = ±16 %).
    pub tempo_range: f32,
    /// Pro DJ Link player number 1–4.  Read at launch (the sender bakes it
    /// into its packets), so a change applies on the next start.
    pub player: u8,
    /// JOG MODE: true = VINYL (a drag while playing moves the playhead
    /// directly — the platter counts as always pressed, since a touch screen
    /// has no push sensor), false = CDJ (a drag while playing nudges).  Paused,
    /// the platter scrubs in either mode.  Remembered, as on the unit.
    pub jog_vinyl: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_cue_level_db: -48.0, quantize: true, tempo_range: 0.16, player: 1, jog_vinyl: true }
    }
}

impl Settings {
    fn path() -> PathBuf { app_data_dir().join("settings.json") }

    /// Load the persisted settings; `default_player` seeds the player number
    /// when no file exists yet (1 on the desktop, 3 on the iPad).
    pub fn load(default_player: u8) -> Self {
        let path = Self::path();
        match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice::<Settings>(&b) {
                Ok(s)  => { log::info!("settings: {:?} from {}", s, path.display()); s }
                Err(e) => { log::warn!("settings {}: {e} — using defaults", path.display()); Self { player: default_player, ..Self::default() } }
            },
            Err(_) => Self { player: default_player, ..Self::default() },
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) { log::warn!("settings: cannot create {}: {e}", dir.display()); return; }
        }
        match serde_json::to_vec_pretty(self).map_err(anyhow::Error::from)
            .and_then(|b| std::fs::write(&path, b).map_err(anyhow::Error::from))
        {
            Ok(())  => log::debug!("settings: saved"),
            Err(e)  => log::warn!("settings: save failed: {e:#}"),
        }
    }

    /// Step a setting through its choices (`dir` = ±1, wrapping) and persist.
    pub fn cycle(&mut self, s: Setting, dir: i32) {
        *self = self.cycled(s, dir);
        log::info!("settings: {s:?} → {}", self.value(s));
        self.save();
    }

    /// `cycle` as a pure function: the settings with `s` stepped by `dir`.
    pub fn cycled(&self, s: Setting, dir: i32) -> Settings {
        fn step<T: PartialEq + Copy>(choices: &[T], cur: T, dir: i32) -> T {
            let i = choices.iter().position(|c| *c == cur).unwrap_or(0) as i32;
            let n = choices.len() as i32;
            choices[(((i + dir) % n + n) % n) as usize]
        }
        let mut c = self.clone();
        match s {
            Setting::AutoCueLevel => c.auto_cue_level_db = step(&AUTO_CUE_LEVELS, c.auto_cue_level_db, dir),
            Setting::Quantize     => c.quantize = !c.quantize,
            Setting::TempoRange   => c.tempo_range = step(&TEMPO_RANGES, c.tempo_range, dir),
            Setting::Player       => c.player = step(&[1u8, 2, 3, 4], c.player, dir),
        }
        c
    }

    /// The value as the MENU shows it.
    pub fn value(&self, s: Setting) -> String {
        match s {
            Setting::AutoCueLevel => format!("{:.0} dB", self.auto_cue_level_db),
            Setting::Quantize     => if self.quantize { "ON".into() } else { "OFF".into() },
            Setting::TempoRange   => Self::range_label(self.tempo_range),
            Setting::Player       => format!("{}   (applies at next launch)", self.player),
        }
    }

    /// "±16" / "WIDE" — also the badge under the BPM box.
    pub fn range_label(range: f32) -> String {
        if range >= 0.99 { "WIDE".into() } else { format!("±{:.0}", range * 100.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_wraps_both_ways_and_toggles() {
        let s = Settings::default();
        assert_eq!(s.tempo_range, 0.16);
        let s = s.cycled(Setting::TempoRange, 1);  assert_eq!(s.tempo_range, 1.00);
        let s = s.cycled(Setting::TempoRange, 1);  assert_eq!(s.tempo_range, 0.06);   // wraps
        let s = s.cycled(Setting::TempoRange, -1); assert_eq!(s.tempo_range, 1.00);   // and back
        let s = s.cycled(Setting::Quantize, 1);    assert!(!s.quantize);
        let s = s.cycled(Setting::Player, -1);     assert_eq!(s.player, 4);
        let s = s.cycled(Setting::AutoCueLevel, 1); assert_eq!(s.auto_cue_level_db, -54.0);
        assert_eq!(Settings::range_label(1.0), "WIDE");
        assert_eq!(Settings::range_label(0.10), "±10");
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // An older file lacking a key still loads (serde(default)).
        let s: Settings = serde_json::from_str(r#"{"quantize":false}"#).unwrap();
        assert!(!s.quantize);
        assert_eq!(s.tempo_range, 0.16);
        assert_eq!(s.player, 1);
    }
}
