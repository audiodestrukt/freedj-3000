//! MENU / UTILITY settings — the unit's scrollable list of preferences.
//! Persisted as JSON in the app's data dir (beside the tag list) so they
//! survive a relaunch.  Deliberately minimal: only settings that change real
//! behaviour here; a setting that did nothing would be a fake control.

use crate::taglist::app_data_dir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The rows of the MENU screen, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting { AutoCueLevel, Quantize, TempoRange, OutputGain, Player, Skin, Light, WaveColor, Divisions, DeckView, Stems, Dvs, PerfLog, About }

/// DECK VIEW is a desktop row: the iPad always shows the whole deck and the
/// phone has its two pages.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub const MENU: &[(Setting, &str)] = &[
    (Setting::AutoCueLevel, "AUTO CUE LEVEL"),
    (Setting::Quantize,     "QUANTIZE"),
    (Setting::TempoRange,   "TEMPO RANGE"),
    (Setting::OutputGain,   "OUTPUT GAIN"),
    (Setting::Player,       "PLAYER No."),
    (Setting::Skin,         "SCREEN"),
    (Setting::Light,        "LIGHT MODE"),
    (Setting::WaveColor,    "WAVEFORM COLOR"),
    (Setting::Divisions,    "WAVEFORM DIVISIONS"),
    (Setting::DeckView,     "DECK VIEW"),
    (Setting::Stems,        "STEMS"),
    (Setting::Dvs,          "DVS"),
    (Setting::PerfLog,      "PERF LOG"),
    (Setting::About,        "ABOUT"),
];
#[cfg(target_os = "ios")]
pub const MENU: &[(Setting, &str)] = &[
    (Setting::AutoCueLevel, "AUTO CUE LEVEL"),
    (Setting::Quantize,     "QUANTIZE"),
    (Setting::TempoRange,   "TEMPO RANGE"),
    (Setting::OutputGain,   "OUTPUT GAIN"),
    (Setting::Player,       "PLAYER No."),
    (Setting::Skin,         "SCREEN"),
    (Setting::Light,        "LIGHT MODE"),
    (Setting::WaveColor,    "WAVEFORM COLOR"),
    (Setting::Divisions,    "WAVEFORM DIVISIONS"),
    (Setting::Stems,        "STEMS"),
    (Setting::Dvs,          "DVS"),
    (Setting::PerfLog,      "PERF LOG"),
    (Setting::About,        "ABOUT"),
];
/// Android: no DVS row yet (the input needs the RECORD_AUDIO permission flow).
#[cfg(target_os = "android")]
pub const MENU: &[(Setting, &str)] = &[
    (Setting::AutoCueLevel, "AUTO CUE LEVEL"),
    (Setting::Quantize,     "QUANTIZE"),
    (Setting::TempoRange,   "TEMPO RANGE"),
    (Setting::OutputGain,   "OUTPUT GAIN"),
    (Setting::Player,       "PLAYER No."),
    (Setting::Skin,         "SCREEN"),
    (Setting::Light,        "LIGHT MODE"),
    (Setting::WaveColor,    "WAVEFORM COLOR"),
    (Setting::Divisions,    "WAVEFORM DIVISIONS"),
    (Setting::Stems,        "STEMS"),
    (Setting::PerfLog,      "PERF LOG"),
    (Setting::About,        "ABOUT"),
];

/// Which unit's screens the deck shows (an OpenDeck setting, not the
/// unit's). Both skins drive the same deck.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub enum Skin {
    Xdj1000Mk2,
    /// The default since seed 3 (0.3.10).
    #[default]
    Cdj3000,
}

impl Skin {
    pub fn label(self) -> &'static str {
        match self { Skin::Xdj1000Mk2 => "XDJ-1000MK2", Skin::Cdj3000 => "CDJ-3000" }
    }

    /// `OPENDECK_SKIN=cdj3000` / `xdj1000mk2` (case-insensitive), for
    /// screenshots and dev runs; it is not saved.
    pub fn from_env() -> Option<Skin> {
        Self::parse(&std::env::var("OPENDECK_SKIN").ok()?)
    }

    /// A skin name as `OPENDECK_SKIN` takes it.
    pub fn parse(v: &str) -> Option<Skin> {
        match v.to_ascii_lowercase().as_str() {
            "cdj3000" | "cdj-3000" | "cdj" => Some(Skin::Cdj3000),
            "xdj1000mk2" | "xdj-1000mk2" | "xdj" => Some(Skin::Xdj1000Mk2),
            _ => None,
        }
    }
}

/// Waveform Color, as the CDJ-3000 offers it: BLUE (one colour), RGB
/// (low red, mid green, high blue: the nexus look) and 3BAND (low blue, mid
/// amber, high white: what the CDJ-3000's manual shows, the default here).
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub enum WaveColor {
    Blue,
    Rgb,
    #[default]
    ThreeBand,
}

impl WaveColor {
    pub fn label(self) -> &'static str {
        match self { WaveColor::Blue => "BLUE", WaveColor::Rgb => "RGB", WaveColor::ThreeBand => "3BAND" }
    }
}

/// Waveform Divisions: what the strip under the overall waveform shows, as
/// on a CDJ-3000 — marks every 30 s, or the track's phrases (rekordbox's
/// from the stick when it has them, else OpenDeck's own analysis).  A track
/// with no phrase data shows the time scale either way, as on the unit.
#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub enum Divisions {
    TimeScale,
    #[default]
    Phrase,
}

impl Divisions {
    pub fn label(self) -> &'static str {
        match self { Divisions::TimeScale => "TIME SCALE", Divisions::Phrase => "PHRASE" }
    }
}

/// The unit's AUTO CUE LEVEL choices (dB).
pub const AUTO_CUE_LEVELS: [f32; 8] = [-36.0, -42.0, -48.0, -54.0, -60.0, -66.0, -72.0, -78.0];
/// OUTPUT GAIN choices (dB): a trim on the deck's output, never a boost.
pub const OUTPUT_GAINS: [f32; 9] = [0.0, -3.0, -6.0, -9.0, -12.0, -15.0, -18.0, -24.0, -30.0];
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
    /// Pro DJ Link player number 1–6, or 0 for AUTO (claim a free one on
    /// the network, as a CDJ set to AUTO does).  Read at launch, so a change
    /// applies on the next start.
    pub player: u8,
    /// JOG MODE: true = VINYL (a drag while playing moves the playhead
    /// directly — the platter counts as always pressed, since a touch screen
    /// has no push sensor), false = CDJ (a drag while playing nudges).  Paused,
    /// the platter scrubs in either mode.  Remembered, as on the unit.
    pub jog_vinyl: bool,
    /// Write the CPU meter's lines to a file the user can see (iOS:
    /// Documents/opendeck-perf.log, in the Files app).  Off by default — a
    /// production install should not grow a log in the user's folder; turn
    /// it on to investigate heat, off again when done (the file is removed).
    pub perf_log: bool,
    /// Which unit's screens to show.
    pub skin: Skin,
    /// Desktop: the whole deck (jog, keys, tempo fader) around the screen,
    /// or the screen alone.  Defaults to the whole deck on the Mac and
    /// Windows; `--faceplate` / `--screen-only` override it for one run.
    pub full_deck: bool,
    /// LIGHT MODE: the screen on a white ground (`theme.rs`), as the
    /// CDJ-3000 light-mode mods do.  Off by default.
    pub light: bool,
    /// Waveform Color (both waveforms); the C key cycles it too.
    pub wave_color: WaveColor,
    /// Waveform Divisions (CDJ-3000 screen): TIME SCALE or PHRASE.
    pub divisions: Divisions,
    /// OUTPUT GAIN (dB, 0 or below): turns the deck's output down,
    /// independent of the device's volume.
    pub output_gain_db: f32,
    /// Stems: a STEMS key and panel on the CDJ-3000 screen, a STEMS pad mode
    /// in the XDJ's PERFORM screen.  Prepared stems on a stick play as the
    /// track loads; a track without them is split on the device once its
    /// STEMS panel is opened.  On by default since seed 4 (people install
    /// the app for stems); OFF gives the stock units' look.
    pub stems: bool,
    /// DVS: a timecode record on a turntable drives the deck, through the
    /// audio interface's input (`dvs.rs`). OFF by default: the input is
    /// opened (and on iOS the microphone permission asked) only when it is
    /// turned on.
    pub dvs: crate::dvs::DvsMode,
    /// The record's state for the DVS row (not saved; the app sets it).
    #[serde(skip)]
    pub dvs_status: &'static str,
    /// Which player-number seeding this file has had (0 = written before
    /// 0.2.1, when every iOS device was seeded to 3).  Lets a later default
    /// re-seed a device once without touching a number the user chose.
    pub seed: u8,
    /// Running on a phone (not saved): the CDJ-3000 screen has no phone
    /// layout yet, so SCREEN stays on the XDJ-1000MK2 there.
    #[serde(skip)]
    pub phone: bool,
}

/// Seeding generation written by this build; see `Settings::seed`.
/// 2: PLAYER No. AUTO arrived; a number still at the device's default
/// (never changed by the user) becomes AUTO.
/// 3: the CDJ-3000 screen became the default; a file still on the old
/// default (XDJ-1000MK2) moves to it once.  SCREEN switches back.
/// 4: STEMS became on by default; a file with it off (the old default)
/// turns it on once.  MENU → STEMS turns it off again.
const SEED: u8 = 4;

impl Default for Settings {
    fn default() -> Self {
        Self { auto_cue_level_db: -48.0, quantize: true, tempo_range: 0.16, player: 0, jog_vinyl: true, perf_log: false, skin: Skin::Cdj3000,
               full_deck: cfg!(any(target_os = "macos", target_os = "windows")), wave_color: WaveColor::ThreeBand, divisions: Divisions::Phrase, output_gain_db: 0.0, light: false, stems: true, dvs: crate::dvs::DvsMode::Off, dvs_status: "", seed: 0, phone: false }
    }
}

impl Settings {
    fn path() -> PathBuf { app_data_dir().join("settings.json") }

    /// Bring a file written by an older build up to this one's defaults,
    /// once (see [`SEED`]); only values still at an old default move.
    fn migrate(&mut self, default_player: u8) {
        if self.seed < 1 && cfg!(target_os = "ios") && self.player == 3 && default_player != 3 {
            log::info!("settings: PLAYER No. re-seeded {} → {default_player} for this device", self.player);
            self.player = default_player;
        }
        if self.seed < 2 && self.player == default_player {
            log::info!("settings: PLAYER No. {} was the default; now AUTO", self.player);
            self.player = 0;
        }
        if self.seed < 3 && self.skin == Skin::Xdj1000Mk2 {
            log::info!("settings: SCREEN was the old default (XDJ-1000MK2); now CDJ-3000");
            self.skin = Skin::Cdj3000;
        }
        if self.seed < 4 && !self.stems {
            log::info!("settings: STEMS was the old default (off); now on");
            self.stems = true;
        }
        self.seed = SEED;
    }

    /// Load the persisted settings; `default_player` seeds the player number
    /// when no file exists yet (1 on the desktop, 3 on an iPad, 4 on an
    /// iPhone).  An iOS file from before 0.2.1 still carrying that era's
    /// blanket seed of 3 is re-seeded once to the device's default, so an
    /// iPhone installed as 0.2.0 stops colliding with the iPad.
    pub fn load(default_player: u8) -> Self {
        let path = Self::path();
        let fresh = || Self { player: 0, seed: SEED, ..Self::default() };
        match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice::<Settings>(&b) {
                Ok(mut s) => {
                    log::info!("settings: {:?} from {}", s, path.display());
                    if s.seed < SEED {
                        s.migrate(default_player);
                        s.save();
                    }
                    s
                }
                Err(e) => { log::warn!("settings {}: {e} — using defaults", path.display()); fresh() }
            },
            Err(_) => fresh(),
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
            Setting::OutputGain   => c.output_gain_db = step(&OUTPUT_GAINS, c.output_gain_db, dir),
            Setting::Player       => c.player = step(&[0u8, 1, 2, 3, 4, 5, 6], c.player, dir),
            Setting::Skin if c.phone => {}
            Setting::Skin         => c.skin = step(&[Skin::Xdj1000Mk2, Skin::Cdj3000], c.skin, dir),
            Setting::DeckView     => c.full_deck = !c.full_deck,
            Setting::WaveColor    => c.wave_color = step(&[WaveColor::Blue, WaveColor::Rgb, WaveColor::ThreeBand], c.wave_color, dir),
            Setting::Divisions    => c.divisions = step(&[Divisions::TimeScale, Divisions::Phrase], c.divisions, dir),
            Setting::Stems        => c.stems = !c.stems,
            Setting::Dvs          => c.dvs = step(&crate::dvs::DvsMode::ALL, c.dvs, dir),
            Setting::Light        => c.light = !c.light,
            Setting::PerfLog      => c.perf_log = !c.perf_log,
            // Not a setting: the row opens the ABOUT screen.
            Setting::About        => {}
        }
        c
    }

    /// The value as the MENU shows it.
    pub fn value(&self, s: Setting) -> String {
        match s {
            Setting::AutoCueLevel => format!("{:.0} dB", self.auto_cue_level_db),
            Setting::Quantize     => if self.quantize { "ON".into() } else { "OFF".into() },
            Setting::TempoRange   => Self::range_label(self.tempo_range),
            Setting::OutputGain   => if self.output_gain_db == 0.0 { "0 dB".into() } else { format!("{:.0} dB", self.output_gain_db) },
            Setting::Player if self.player == 0 => "AUTO   (applies at next launch)".into(),
            Setting::Player       => format!("{}   (applies at next launch)", self.player),
            Setting::Skin if self.phone => format!("{}   (CDJ-3000: iPad only)", Skin::Xdj1000Mk2.label()),
            Setting::Skin         => self.skin.label().into(),
            Setting::WaveColor    => format!("{}   (C)", self.wave_color.label()),
            Setting::Divisions    => self.divisions.label().into(),
            Setting::Dvs if self.dvs_status.is_empty() => self.dvs.label().into(),
            Setting::Dvs          => format!("{}   ({})", self.dvs.label(), self.dvs_status),
            Setting::DeckView     => if self.full_deck { "FULL DECK   (F)".into() } else { "SCREEN ONLY   (F)".into() },
            Setting::Stems        => if self.stems { "ON".into() } else { "OFF".into() },
            Setting::Light        => if self.light { "ON".into() } else { "OFF".into() },
            Setting::About        => format!("{}   ▶", crate::screen::APP_VERSION),
            Setting::PerfLog      => if self.perf_log { "ON   (Documents/opendeck-perf.log)".into() } else { "OFF".into() },
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

    /// OUTPUT GAIN: 0 dB by default, steps down to −30 dB and wraps, never
    /// a boost, saved.
    #[test]
    fn output_gain_steps_down_from_zero() {
        let s = Settings::default();
        assert_eq!(s.output_gain_db, 0.0);
        assert_eq!(s.value(Setting::OutputGain), "0 dB");
        let t = s.cycled(Setting::OutputGain, 1).cycled(Setting::OutputGain, 1);
        assert_eq!((t.output_gain_db, t.value(Setting::OutputGain).as_str()), (-6.0, "-6 dB"));
        assert!(OUTPUT_GAINS.iter().all(|&g| g <= 0.0));
        assert_eq!(s.cycled(Setting::OutputGain, -1).output_gain_db, -30.0, "down from 0 wraps to the quietest");
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap().output_gain_db, -6.0);
    }

    /// WAVEFORM DIVISIONS: PHRASE by default, a MENU row that toggles to
    /// TIME SCALE and back, saved with the rest.
    #[test]
    fn waveform_divisions_default_to_phrase_and_toggle() {
        let s = Settings::default();
        assert_eq!(s.divisions, Divisions::Phrase);
        assert!(MENU.iter().any(|(k, n)| *k == Setting::Divisions && *n == "WAVEFORM DIVISIONS"));
        let t = s.cycled(Setting::Divisions, 1);
        assert_eq!((t.divisions, t.value(Setting::Divisions).as_str()), (Divisions::TimeScale, "TIME SCALE"));
        assert_eq!(t.cycled(Setting::Divisions, 1).divisions, Divisions::Phrase);
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap().divisions, Divisions::TimeScale);
    }

    #[test]
    fn light_mode_is_a_menu_row_off_by_default() {
        let s = Settings::default();
        assert!(!s.light);
        assert!(MENU.iter().any(|(m, n)| *m == Setting::Light && *n == "LIGHT MODE"));
        assert_eq!(s.value(Setting::Light), "OFF");
        let on = s.cycled(Setting::Light, 1);
        assert!(on.light && on.value(Setting::Light) == "ON");
        assert!(!on.cycled(Setting::Light, -1).light);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert!(back.light, "saved with the other settings");
        let old: Settings = serde_json::from_str(r#"{"quantize":true}"#).unwrap();
        assert!(!old.light, "a file from before the row reads as off");
    }

    #[test]
    fn stems_are_on_by_default_and_turn_off() {
        let s = Settings::default();
        assert!(s.stems, "on by default: people install the app for stems");
        assert!(MENU.iter().any(|(m, n)| *m == Setting::Stems && *n == "STEMS"));
        let off = s.cycled(Setting::Stems, 1);
        assert!(!off.stems);
        assert!(off.cycled(Setting::Stems, -1).stems);
        // A file from before the row existed reads as on.
        let old: Settings = serde_json::from_str(r#"{"quantize":true}"#).unwrap();
        assert!(old.stems);
        // A file saved with the old default (off) turns on once; turned off
        // again after that, it stays off.
        let mut was: Settings = serde_json::from_str(r#"{"stems":false,"seed":3}"#).unwrap();
        was.migrate(1);
        assert!(was.stems && was.seed == SEED);
        was.stems = false;
        was.migrate(1);
        assert!(!was.stems, "migrated once only");
    }

    #[test]
    fn skin_switch_names_toggles_and_persists() {
        assert_eq!(Settings::default().skin, Skin::Cdj3000, "the CDJ-3000 screen is the default");
        for v in ["cdj3000", "CDJ-3000", "cdj"] { assert_eq!(Skin::parse(v), Some(Skin::Cdj3000), "{v}"); }
        for v in ["xdj1000mk2", "XDJ-1000MK2", "xdj"] { assert_eq!(Skin::parse(v), Some(Skin::Xdj1000Mk2), "{v}"); }
        assert_eq!(Skin::parse("cdj2000"), None);
        // The MENU row: SCREEN, stepping either way flips the skin.
        assert!(MENU.iter().any(|(s, n)| *s == Setting::Skin && *n == "SCREEN"));
        let s = Settings::default();
        assert_eq!((s.skin, s.value(Setting::Skin).as_str()), (Skin::Cdj3000, "CDJ-3000"));
        assert_eq!(s.cycled(Setting::Skin, 1).skin, Skin::Xdj1000Mk2);
        assert_eq!(s.cycled(Setting::Skin, -1).skin, Skin::Xdj1000Mk2);
        // A file from before the switch (no skin saved, or the old default) is
        // read as CDJ-3000 by serde's default; the seed-3 migration moves a
        // saved XDJ-1000MK2 (load() covers it; here the field default).
        let old: Settings = serde_json::from_str(r#"{"quantize":true,"seed":2}"#).unwrap();
        assert_eq!(old.skin, Skin::Cdj3000);
        // Saved and read back with the other settings.
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.skin, Skin::Cdj3000);
    }

    #[test]
    fn cycle_wraps_both_ways_and_toggles() {
        let s = Settings::default();
        assert_eq!(s.tempo_range, 0.16);
        let s = s.cycled(Setting::TempoRange, 1);  assert_eq!(s.tempo_range, 1.00);
        let s = s.cycled(Setting::TempoRange, 1);  assert_eq!(s.tempo_range, 0.06);   // wraps
        let s = s.cycled(Setting::TempoRange, -1); assert_eq!(s.tempo_range, 1.00);   // and back
        let s = s.cycled(Setting::Quantize, 1);    assert!(!s.quantize);
        assert_eq!(s.player, 0);
        assert!(s.value(Setting::Player).starts_with("AUTO"));
        let s = s.cycled(Setting::Player, 1);      assert_eq!(s.player, 1);
        let s = s.cycled(Setting::Player, -1);     assert_eq!(s.player, 0);      // back to AUTO
        let s = s.cycled(Setting::Player, -1);     assert_eq!(s.player, 6);      // wraps
        let s = s.cycled(Setting::AutoCueLevel, 1); assert_eq!(s.auto_cue_level_db, -54.0);
        assert_eq!(Settings::range_label(1.0), "WIDE");
        assert_eq!(Settings::range_label(0.10), "±10");
        let phone = Settings { phone: true, ..Settings::default() };
        assert_eq!(phone.cycled(Setting::Skin, 1).skin, phone.skin, "no CDJ-3000 screen on a phone yet: SCREEN does not step");
        assert!(phone.value(Setting::Skin).contains("iPad only"));
        let s = s.cycled(Setting::Skin, 1);        assert_eq!(s.skin, Skin::Xdj1000Mk2);
        let s = s.cycled(Setting::Skin, 1);        assert_eq!(s.skin, Skin::Cdj3000);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // An older file lacking a key still loads (serde(default)).
        let s: Settings = serde_json::from_str(r#"{"quantize":false}"#).unwrap();
        assert!(!s.quantize);
        assert_eq!(s.tempo_range, 0.16);
        assert_eq!(s.player, 0);                                   // AUTO
        assert_eq!(s.skin, Skin::Cdj3000);
    }
}
