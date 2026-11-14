//! Everything the deck knows, captured once per frame.
//!
//! The renderer and the screen chrome are *consumers* of this struct; neither
//! reads deck state directly.  That keeps the classic CDJ/XDJ layout one view
//! among several rather than the shape of the architecture, and it stops the
//! render signature growing a parameter every time the screen gains an element.

use opendeck_types::BeatGrid;

/// Where stems stand for the loaded track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StemsState {
    /// The MENU's STEMS setting is off: no stem controls anywhere.
    #[default]
    Off,
    /// On, but this track has no prepared stems (or they were refused).
    None,
    /// On; the stems are being read and aligned.  Controls are greyed.
    Loading,
    /// On; the device is splitting the track itself (percent done).  The
    /// controls already work on the part split so far.
    Splitting(u8),
    /// On; the stem model is downloading (percent), first use only.
    Downloading(u8),
    /// On; the downloaded model is being checked and unpacked.
    Preparing,
    /// On, but this device has too little memory to split a track (no
    /// model is downloaded for it either).
    NoMemory,
    /// On; the model download failed (no network…) and will be retried.
    Waiting,
    /// On and playable.
    Ready,
}

impl StemsState {
    /// The stem controls work: ready, or split far enough to start.
    pub fn playable(self) -> bool { matches!(self, StemsState::Ready | StemsState::Splitting(_)) }

    /// While the stem model is being fetched (first use only), the line the
    /// STEMS panel shows across its keys, so it is plain that a one-time
    /// download is under way and how far it has got.
    pub fn fetch_note(self) -> Option<String> {
        let total = (crate::modelfetch::MODEL.zip_bytes / 1_000_000) as u32;   // MB, as the listing says
        match self {
            StemsState::Downloading(p) => Some(format!("DOWNLOADING STEM MODEL  {} / {total} MB  ·  FIRST USE ONLY", total * p as u32 / 100)),
            StemsState::Preparing => Some("PREPARING STEM MODEL  ·  FIRST USE ONLY".into()),
            StemsState::Waiting => Some("STEM MODEL DOWNLOAD: WAITING FOR NETWORK".into()),
            StemsState::NoMemory => Some("THIS DEVICE HAS NOT GOT THE MEMORY TO SPLIT STEMS".into()),
            _ => None,
        }
    }

    /// The MENU's STEMS row while the model is fetched ("ON" otherwise).
    pub fn menu_value(self) -> Option<String> {
        match self {
            StemsState::Downloading(p) => Some(format!("ON · DOWNLOADING MODEL {p}%")),
            StemsState::Preparing => Some("ON · PREPARING MODEL".into()),
            StemsState::Waiting => Some("ON · WAITING FOR NETWORK".into()),
            StemsState::NoMemory => Some("ON · NEEDS MORE MEMORY".into()),
            _ => None,
        }
    }
}

/// The stem controls' view: state plus drums / vocal / harmonics levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StemsView { pub state: StemsState, pub gains: [f32; 3] }

impl Default for StemsView { fn default() -> Self { StemsView { state: StemsState::Off, gains: [1.0; 3] } } }

impl StemsView {
    /// Any stem off full: the panel's key shows its "changed" underline.
    pub fn changed(&self) -> bool { self.gains.iter().any(|g| *g < 1.0) }
    /// Every stem fully on or fully off (TOGGLE mode can show it).
    pub fn all_on_or_off(&self) -> bool { self.gains.iter().all(|g| *g <= 0.0 || *g >= 1.0) }
}

pub struct DeckSnapshot<'a> {
    /// Track name shown in the title bar (tag title, else file name).
    pub title:        &'a str,
    /// A track being fetched / prepared on the loader thread, by name: the
    /// title bar shows a LOADING readout until it lands (`DeckApp::loading`).
    pub loading:      Option<&'a str>,
    /// A message the CDJ-3000 screen shows in a pop-up for a few seconds
    /// (lines split by `\n`), as the unit does ("No LINKED Players found.").
    pub notice:       Option<&'a str>,
    /// Stems: whether their controls show, and their state and levels.
    pub stems:        StemsView,

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
    /// Each hot cue's rekordbox colour (None: the CDJ default green).
    pub hot_rgb:      [Option<(u8, u8, u8)>; 8],
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
    /// JOG MODE = VINYL (hub badge + drag behaviour).
    pub jog_vinyl:    bool,
    /// Time display shows remaining (REMAIN) rather than elapsed (TIME).
    pub remain_mode:  bool,
    /// AUTO CUE engaged: loads cue at the first audible sound (A.CUE badge).
    pub auto_cue:     bool,
    pub slip:         bool,
    pub sync:         bool,
    pub master:       bool,
    /// ZOOM / GRID ADJUST mode indicator.
    pub zoom_grid_mode: bool,
    /// The beat grid differs from the analysed one (GRID ADJUST RESET lights).
    pub grid_edited: bool,
    /// The cue point was set by hand, not only placed at load (SNAP GRID lights).
    pub cue_set: bool,
    /// SYNC's held phase offset in beats (a jog nudge while synced); nonzero
    /// lights SHIFT GRID.
    pub sync_offset: f32,
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
    /// KEY SHIFT in semitones (0 = the track's own key).
    pub key_shift: i8,

    /// INFO page: what Link is speaking from ("ip (iface) → broadcast") and
    /// the players heard.  Empty until the sender has started.
    pub link_addr:    String,
    /// The device's IPv4 addresses for the MENU footer ("IP 192.168.1.23
    /// (en0)"), the one Link uses first.  Filled while MENU is open.
    pub ip_line:      String,
    pub link_peers:   String,

    /// Waveform columns (of `HOP_SIZE` frames) across the enlarged waveform:
    /// the zoom, for turning a drag in points into samples.
    pub cols_visible: f32,
}

impl DeckSnapshot<'_> {
    /// The key to show: the track's key moved by KEY SHIFT (as the unit's
    /// KEY readout does), in the notation the track's key came in.
    pub fn shown_key(&self) -> Option<String> {
        let k = self.tags.key.as_deref()?;
        Some(opendeck_analysis::key::Key::shift_name(k, self.key_shift as i32).unwrap_or_else(|| k.to_string()))
    }

    /// Hot cue `i`'s colour: rekordbox's, else `default` (the skin's green).
    pub fn hot_color(&self, i: usize, default: egui::Color32) -> egui::Color32 {
        match self.hot_rgb.get(i).copied().flatten() {
            Some((r, g, b)) => egui::Color32::from_rgb(r, g, b),
            None => default,
        }
    }
}

#[cfg(test)]
mod stems_state_tests {
    use super::StemsState as S;

    /// The first-use download is spelt out: size, progress, one time only.
    #[test]
    fn the_model_fetch_says_what_it_is() {
        assert_eq!(S::Downloading(43).fetch_note().as_deref(), Some("DOWNLOADING STEM MODEL  123 / 287 MB  ·  FIRST USE ONLY"));
        assert_eq!(S::Downloading(43).menu_value().as_deref(), Some("ON · DOWNLOADING MODEL 43%"));
        assert!(S::Waiting.fetch_note().unwrap().contains("WAITING FOR NETWORK"));
        assert!(S::Ready.fetch_note().is_none() && S::Ready.menu_value().is_none());
    }
}

#[cfg(test)]
impl<'a> DeckSnapshot<'a> {

    /// A stopped, empty deck for unit tests; set the fields a test needs.
    pub fn test(tags: &'a opendeck_decode::TrackTags) -> Self {
        DeckSnapshot {
            title: "", loading: None, notice: None, stems: StemsView::default(), tags, file: "", position: 0, sample_rate: 44_100, channels: 2,
            total_samples: 0, cue_point: 0, memory_cues: &[], hot_cues: [None; 8], hot_rgb: [None; 8],
            perform_mode: crate::input::PerformMode::HotCue, perform_bank: 0, perform_delete: false,
            loop_active: false, loop_start: 0, loop_end: 0, loop_beats: 0.0, tempo_range: 10.0, quantize: false,
            slip_shadow: None, playing: false, speed: 1.0, fader_speed: 1.0, key_lock: false, jog_vinyl: false,
            remain_mode: true, auto_cue: false, slip: false, sync: false, master: false, zoom_grid_mode: false,
            grid_edited: false, cue_set: false, sync_offset: 0.0, source_link: false, phase_ticks_view: false,
            linked: false, master_player: 0, player: 1, link_addr: String::new(), ip_line: String::new(), link_peers: String::new(),
            cols_visible: 1.0, beat_grid: None, beat2_bpm: 0.0, beat2_phase_beats: 0.0, beat2_beat_in_bar: 0, key_shift: 0,
        }
    }
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
        self.beat_pos().map(|(_, f)| f)
    }

    /// The beat the playhead is in (from the grid's anchor) and how far
    /// through it, from one reading, so the phase and the beat number never
    /// disagree (see [`split_beat`]).
    fn beat_pos(&self) -> Option<(i64, f32)> {
        let g = self.beat_grid?;
        let frames = self.position as f64 / self.channels as f64;
        let period = self.sample_rate as f64 * 60.0 / g.bpm;
        Some(split_beat((frames - g.anchor_sample as f64) / period))
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
        let (beat, _) = self.beat_pos()?;
        Some(((beat + g.downbeat_offset as i64).rem_euclid(4) + 1) as u8)
    }
}

/// A position in beats → (whole beat, fraction 0..1).  The fraction is an
/// f32 for drawing; just under a beat (…7.99999999) it would round up to
/// 1.0 while the whole beat still read 7, and the phase meter drew that
/// frame a beat off (its red bar mark jumped).  Such a value counts as the
/// next beat at 0.
pub fn split_beat(rel: f64) -> (i64, f32) {
    let k = rel.floor();
    let f = (rel - k) as f32;
    if f >= 1.0 { (k as i64 + 1, 0.0) } else { (k as i64, f) }
}

#[cfg(test)]
mod split_beat_tests {
    use super::split_beat;

    /// Just under a beat the f32 fraction rounds up to 1.0; the whole beat
    /// must move on with it, or the phase meter draws that frame a beat off.
    #[test]
    fn a_fraction_that_rounds_to_one_is_the_next_beat() {
        assert_eq!(split_beat(7.999_999_999), (8, 0.0));
        assert_eq!(split_beat(7.5), (7, 0.5));
        assert_eq!(split_beat(-0.25), (-1, 0.75));
        for i in 0..200_000 {
            let rel = 3.0 + i as f64 * 1e-5 - 1e-9;
            let (k, f) = split_beat(rel);
            assert!(f < 1.0 && (k as f64 + f as f64 - rel).abs() < 1e-6, "{rel}: {k} + {f}");
        }
    }
}
