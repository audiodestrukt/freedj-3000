//! Musical key detection — a whole-track pitch-class profile matched against
//! major / minor key profiles, named the way rekordbox names keys.
//!
//! Pipeline (streaming; feed interleaved audio with [`KeyDetector::push`]):
//!
//! 1. Mono, Hann-windowed FFT of ~0.37 s (16,384 at 44.1 kHz), half overlap.
//! 2. The power spectrum is resampled to a log-frequency axis of 10-cent
//!    cells from 27.5 Hz to 5 kHz (bins spanning a cell: their maximum; cells
//!    narrower than a bin: interpolated), compressed to magnitude^0.5, and
//!    whitened (each cell minus the mean of the cells within ±3 semitones,
//!    floored at 0) so broadband drums and noise fall out and partials stay.
//! 3. Cells fold into a 120-bin pitch-class histogram, every frame
//!    normalised to sum 1 so loud passages don't outvote quiet ones.
//! 4. At the end the tuning (circular mean of the histogram's offset from the
//!    A440 semitone grid) is estimated and the histogram folds to 12 pitch
//!    classes around it (cos² weights), then is square-root compressed.
//! 5. The chroma is Pearson-correlated with the 24 rotations of the key
//!    profiles, with a small bonus for minor; the best one is the key.
//!
//! The default profiles are the mean chroma of 226 rekordbox-analysed tracks
//! (mostly house / tech-house, 81 % minor), rotated to rekordbox's tonic; with
//! them the detector agrees exactly with rekordbox on 72 % of those tracks
//! (also 72 % 2-fold cross-validated), against 53 % for Krumhansl's
//! profiles.  `examples/key_vs_rekordbox` in opendeck-app measures it.
//!
//! Key names follow rekordbox: sharps only for F# and F#m, flats elsewhere
//! (Db, Eb, Ab, Bb, Dbm, Ebm, Abm, Bbm), as on a rekordbox-exported stick.

use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

/// Front-end settings; the defaults are what measured best against rekordbox.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyConfig {
    /// Frequency range of the log-frequency spectrum.
    pub min_hz: f32,
    pub max_hz: f32,
    /// FFT length, seconds (rounded up to a power of two in samples).
    pub frame_secs: f32,
    /// Cell value = magnitude ^ `exponent`.
    pub exponent: f32,
    /// Whitening: subtract the moving mean over ± this many 10-cent cells (0 = off).
    pub whiten_cells: usize,
    /// Normalise every frame to sum 1 before accumulating.
    pub frame_norm: bool,
}

impl Default for KeyConfig {
    fn default() -> Self {
        KeyConfig { min_hz: 27.5, max_hz: 5000.0, frame_secs: 0.37, exponent: 0.5, whiten_cells: 30, frame_norm: true }
    }
}

/// Fine pitch-class bins per octave (10 cents each).
pub const FINE: usize = 120;
const PER_SEMI: usize = FINE / 12;

/// A musical key: tonic pitch class (0 = C … 11 = B) and mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub tonic: u8,
    pub minor: bool,
}

const MAJOR_NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
const MINOR_NAMES: [&str; 12] = ["Cm", "Dbm", "Dm", "Ebm", "Em", "Fm", "F#m", "Gm", "Abm", "Am", "Bbm", "Bm"];

impl Key {
    pub fn new(tonic: u8, minor: bool) -> Key { Key { tonic: tonic % 12, minor } }

    /// The name rekordbox gives this key ("Am", "F#", "Ebm", …).
    pub fn name(self) -> &'static str {
        if self.minor { MINOR_NAMES[self.tonic as usize] } else { MAJOR_NAMES[self.tonic as usize] }
    }

    /// Camelot wheel position: number 1..=12 and `true` for the A (minor) ring.
    pub fn camelot(self) -> (u8, bool) {
        // Major: C = 8B, each fifth up is one step round the wheel.
        // Minor: the relative major's number (tonic + 3 semitones).
        let pc = if self.minor { (self.tonic + 3) % 12 } else { self.tonic } as u32;
        (((7 * pc + 7) % 12 + 1) as u8, self.minor)
    }

    /// Camelot notation, "1A" … "12B".
    pub fn camelot_name(self) -> String {
        let (n, a) = self.camelot();
        format!("{n}{}", if a { 'A' } else { 'B' })
    }

    /// The key id CDJs use over dbserver: `(camelot - 1) * 2 + (1 for A, 2 for B)`,
    /// so Abm = 1, B = 2, Ebm = 3, … E = 24.
    pub fn dbserver_id(self) -> u32 {
        let (n, a) = self.camelot();
        (n as u32 - 1) * 2 + if a { 1 } else { 2 }
    }

    /// The key a dbserver id names (inverse of [`Key::dbserver_id`]).
    pub fn from_dbserver_id(id: u32) -> Option<Key> {
        if !(1..=24).contains(&id) { return None; }
        Key::from_camelot(((id - 1) / 2 + 1) as u8, id % 2 == 1)
    }

    /// The key at Camelot `number` (1..=12) on the A (minor) or B ring.
    pub fn from_camelot(number: u8, a_ring: bool) -> Option<Key> {
        if !(1..=12).contains(&number) { return None; }
        // Invert n = (7 pc + 7) % 12 + 1: pc = 7 (n - 8) mod 12 (7 is its own inverse).
        let major_pc = ((7 * ((number as u32 + 4) % 12)) % 12) as u8;
        Some(if a_ring { Key::new((major_pc + 9) % 12, true) } else { Key::new(major_pc, false) })
    }

    /// Parse a key name: rekordbox's ("F#m", "Ab"), either accidental
    /// ("G#m", "Gbm"), "min"/"minor"/"maj" suffixes, Unicode ♯/♭, or Camelot
    /// ("8A", "12B").  Case-insensitive on the mode suffix.
    pub fn parse(s: &str) -> Option<Key> {
        let s = s.trim().replace('♯', "#").replace('♭', "b");
        if s.is_empty() { return None; }
        // Camelot: digits then A/B.
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let rest = s[digits.len()..].trim();
            let n: u8 = digits.parse().ok()?;
            return match rest { "A" | "a" => Key::from_camelot(n, true), "B" | "b" => Key::from_camelot(n, false), _ => None };
        }
        let mut chars = s.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        let mut pc: i32 = match letter { 'C' => 0, 'D' => 2, 'E' => 4, 'F' => 5, 'G' => 7, 'A' => 9, 'B' => 11, _ => return None };
        let mut rest = chars.as_str();
        if let Some(r) = rest.strip_prefix('#') { pc += 1; rest = r; }
        else if let Some(r) = rest.strip_prefix('b') { pc -= 1; rest = r; }
        let minor = match rest.trim().to_ascii_lowercase().as_str() {
            "" | "maj" | "major" => false,
            "m" | "min" | "minor" | "-" => true,
            _ => return None,
        };
        Some(Key::new(pc.rem_euclid(12) as u8, minor))
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str(self.name()) }
}

/// Camelot position (number, A ring) of a key name, or `None` if it doesn't parse.
pub fn camelot_of(name: &str) -> Option<(u8, bool)> { Key::parse(name).map(Key::camelot) }

/// dbserver key id (1..=24) of a key name, or `None` if it doesn't parse.
pub fn dbserver_key_id(name: &str) -> Option<u32> { Key::parse(name).map(Key::dbserver_id) }


/// A pair of 12-entry key profiles (major, minor), tonic first.
pub type Profiles = ([f32; 12], [f32; 12]);

/// Krumhansl–Kessler probe-tone ratings.
pub const KRUMHANSL: Profiles = (
    [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88],
    [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17],
);
/// Temperley's Kostka–Payne corpus profiles.
pub const TEMPERLEY: Profiles = (
    [0.748, 0.060, 0.488, 0.082, 0.670, 0.460, 0.096, 0.715, 0.104, 0.366, 0.057, 0.400],
    [0.712, 0.084, 0.474, 0.618, 0.049, 0.460, 0.105, 0.747, 0.404, 0.067, 0.133, 0.330],
);
/// Mean [`chroma_from_fine`] of 226 rekordbox-analysed tracks (42 major, 184
/// minor), each rotated to rekordbox's tonic: what this front end hears for
/// a track rekordbox calls major / minor.  Fitted with `key_vs_rekordbox`.
pub const REKORDBOX_FIT: Profiles = (
    [1.0, 0.6976, 0.6134, 0.5622, 0.7652, 0.6409, 0.5744, 0.8163, 0.5705, 0.5265, 0.5997, 0.7027],
    [1.0, 0.6972, 0.6703, 0.6488, 0.6566, 0.5947, 0.5675, 0.8210, 0.6101, 0.5333, 0.6253, 0.6625],
);
/// The profiles and minor bonus [`KeyDetector::finish`] uses.
pub const DEFAULT_PROFILES: Profiles = REKORDBOX_FIT;
/// Added to every minor key's correlation: rekordbox leans minor where the
/// third is ambiguous (as in most four-to-the-floor tracks).
pub const DEFAULT_MINOR_BIAS: f32 = 0.05;

/// Streaming key detector.  Push interleaved audio, then [`finish`](Self::finish).
pub struct KeyDetector {
    cfg:     KeyConfig,
    n:       usize,
    hop:     usize,
    buf:     Vec<f32>,
    win:     Vec<f32>,
    fft:     Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    work:    Vec<Complex<f32>>,
    mag:     Vec<f32>,
    /// Log-frequency cells: (first bin, last bin, fractional bin of the
    /// centre) per 10-cent cell; first > last when the cell is narrower than a bin.
    cells:   Vec<(usize, usize, f32)>,
    /// MIDI pitch × 10 of the first cell.
    cell0:   i64,
    lspec:   Vec<f32>,
    prefix:  Vec<f32>,
    frame:   [f32; FINE],
    fine:    [f64; FINE],
    frames:  usize,
}

impl KeyDetector {
    pub fn new(sample_rate: u32) -> KeyDetector { KeyDetector::with_config(sample_rate, KeyConfig::default()) }

    pub fn with_config(sample_rate: u32, cfg: KeyConfig) -> KeyDetector {
        let sr = sample_rate.max(8000) as f32;
        let n = ((sr * cfg.frame_secs) as usize).next_power_of_two();
        let fft = FftPlanner::new().plan_fft_forward(n);
        let win = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
        let bin_hz = sr / n as f32;
        let midi10 = |hz: f32| (12.0 * (hz / 440.0).log2() + 69.0) * PER_SEMI as f32;
        let lo = midi10(cfg.min_hz.max(bin_hz * 2.0)).ceil() as i64;
        let hi = midi10(cfg.max_hz.min(sr * 0.45)).floor() as i64;
        let cells: Vec<_> = (lo..hi.max(lo)).map(|j| {
            let bin = |c: f32| 440.0 * 2f32.powf((c / PER_SEMI as f32 - 69.0) / 12.0) / bin_hz;
            let (a, b, mid) = (bin(j as f32 - 0.5), bin(j as f32 + 0.5), bin(j as f32));
            ((a.ceil() as usize).max(1), (b.floor() as usize).min(n / 2 - 1), mid)
        }).collect();
        KeyDetector {
            cfg, n, hop: n / 2, buf: Vec::with_capacity(n * 2), win,
            scratch: vec![Complex::default(); fft.get_inplace_scratch_len()], fft,
            work: vec![Complex::default(); n], mag: vec![0.0; n / 2 + 1],
            lspec: vec![0.0; cells.len()], prefix: vec![0.0; cells.len() + 1], cells, cell0: lo,
            frame: [0.0; FINE], fine: [0.0; FINE], frames: 0,
        }
    }

    /// Feed interleaved audio, `channels` wide (the first two are mixed to mono).
    pub fn push(&mut self, samples: &[f32], channels: usize) {
        let ch = channels.max(1);
        for f in samples.chunks_exact(ch) {
            self.buf.push(if ch == 1 { f[0] } else { (f[0] + f[1]) * 0.5 });
            if self.buf.len() >= self.n {
                self.frame();
                self.buf.drain(..self.hop);
            }
        }
    }

    fn frame(&mut self) {
        for (w, (x, h)) in self.work.iter_mut().zip(self.buf.iter().zip(&self.win)) { *w = Complex::new(x * h, 0.0); }
        self.fft.process_with_scratch(&mut self.work, &mut self.scratch);
        for (m, c) in self.mag.iter_mut().zip(&self.work) { *m = c.norm_sqr(); }
        let e = 0.5 * self.cfg.exponent; // mag holds power
        for (v, &(a, b, mid)) in self.lspec.iter_mut().zip(&self.cells) {
            *v = if b >= a {
                self.mag[a..=b].iter().cloned().fold(0.0, f32::max)
            } else {
                let k = mid.floor() as usize;
                let t = mid - k as f32;
                self.mag[k] * (1.0 - t) + self.mag[k + 1] * t
            }.powf(e);
        }
        let peak = self.lspec.iter().cloned().fold(0.0f32, f32::max);
        if peak < 1e-8 { return; }
        let (w, n) = (self.cfg.whiten_cells, self.lspec.len());
        if w > 0 {
            for i in 0..n { self.prefix[i + 1] = self.prefix[i] + self.lspec[i]; }
            for i in 0..n {
                let (a, b) = (i.saturating_sub(w), (i + w + 1).min(n));
                let mean = (self.prefix[b] - self.prefix[a]) / (b - a) as f32;
                self.lspec[i] = (self.lspec[i] - mean).max(0.0);
            }
        }
        self.frame = [0.0; FINE];
        for (i, &v) in self.lspec.iter().enumerate() {
            self.frame[(self.cell0 + i as i64).rem_euclid(FINE as i64) as usize] += v;
        }
        let total: f32 = self.frame.iter().sum();
        if total <= 0.0 { return; }
        let scale = if self.cfg.frame_norm { 1.0 / total } else { 1.0 };
        for (a, b) in self.fine.iter_mut().zip(&self.frame) { *a += (*b * scale) as f64; }
        self.frames += 1;
    }

    /// The accumulated 10-cent pitch-class histogram, bin 0 = C at A440 tuning.
    pub fn fine_chroma(&self) -> [f64; FINE] { self.fine }

    /// The detected key, or `None` for silence / too little audio.
    pub fn finish(&self) -> Option<Key> {
        if self.frames < 4 { return None; }
        Some(key_from_chroma(&chroma_from_fine(&self.fine), &DEFAULT_PROFILES, DEFAULT_MINOR_BIAS))
    }
}

/// Tuning of a fine histogram: offset, in fine bins (-PER_SEMI/2 ..
/// PER_SEMI/2), of its energy from the equal-tempered grid.
pub fn tuning_bins(fine: &[f64; FINE]) -> f64 {
    let (mut s, mut c) = (0.0, 0.0);
    for (i, &v) in fine.iter().enumerate() {
        let a = std::f64::consts::TAU * (i % PER_SEMI) as f64 / PER_SEMI as f64;
        s += v * a.sin();
        c += v * a.cos();
    }
    s.atan2(c) / std::f64::consts::TAU * PER_SEMI as f64
}

/// Fold a fine histogram to 12 pitch classes (0 = C) around its tuning,
/// weighting by cos² of the distance to each semitone centre, normalised to
/// sum 1 and square-root compressed: what the profiles are matched against.
pub fn chroma_from_fine(fine: &[f64; FINE]) -> [f32; 12] {
    let tune = tuning_bins(fine);
    let mut out = [0f64; 12];
    for (i, &v) in fine.iter().enumerate() {
        let semi = (i as f64 - tune) / PER_SEMI as f64;
        let n = semi.round();
        let w = (std::f64::consts::PI * (semi - n)).cos().powi(2);
        out[(n as i64).rem_euclid(12) as usize] += v * w;
    }
    let total = out.iter().sum::<f64>().max(1e-12);
    out.map(|v| (v / total).sqrt() as f32)
}

fn pearson(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let ma = a.iter().sum::<f32>() / 12.0;
    let mb = b.iter().sum::<f32>() / 12.0;
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for i in 0..12 { let (x, y) = (a[i] - ma, b[i] - mb); ab += x * y; aa += x * x; bb += y * y; }
    ab / (aa * bb).sqrt().max(1e-12)
}

/// Correlation of `chroma` with every key (index = tonic, then +12 for minor).
pub fn key_scores(chroma: &[f32; 12], profiles: &Profiles) -> [f32; 24] {
    let mut out = [0f32; 24];
    for t in 0..12 {
        let mut rot = [0f32; 12];
        for i in 0..12 { rot[i] = chroma[(i + t) % 12]; }
        out[t] = pearson(&rot, &profiles.0);
        out[t + 12] = pearson(&rot, &profiles.1);
    }
    out
}

/// Best-matching key for a 12-bin chroma (0 = C), `minor_bias` added to minor keys.
pub fn key_from_chroma(chroma: &[f32; 12], profiles: &Profiles, minor_bias: f32) -> Key {
    let mut s = key_scores(chroma, profiles);
    for v in &mut s[12..] { *v += minor_bias; }
    let best = (0..24).max_by(|&a, &b| s[a].total_cmp(&s[b])).unwrap();
    Key::new((best % 12) as u8, best >= 12)
}

/// Detect the key of a whole decoded track (interleaved, `channels` wide).
pub fn detect_key(samples: &[f32], channels: usize, sample_rate: u32) -> Option<Key> {
    let mut d = KeyDetector::new(sample_rate);
    d.push(samples, channels);
    d.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tones(sr: u32, secs: f32, notes: &[(f32, f32)], detune_cents: f32) -> Vec<f32> {
        // notes: (MIDI pitch, amplitude); a few harmonics each, stereo.
        let n = (sr as f32 * secs) as usize;
        let mut out = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f32 / sr as f32;
            let mut x = 0.0;
            for &(p, a) in notes {
                let f = 440.0 * 2f32.powf((p - 69.0 + detune_cents / 100.0) / 12.0);
                for h in 1..=4 { x += a / h as f32 * (std::f32::consts::TAU * f * h as f32 * t).sin(); }
            }
            out.push(x * 0.05);
            out.push(x * 0.05);
        }
        out
    }

    /// A progression in `tonic`, as (MIDI, amp) chord lists: I IV V I (major) or i iv v i.
    fn cadence(sr: u32, root: f32, minor: bool, detune: f32) -> Vec<f32> {
        let third = if minor { 3.0 } else { 4.0 };
        let chords = [
            vec![(root, 1.0), (root + third, 0.8), (root + 7.0, 0.8), (root - 12.0, 1.0)],
            vec![(root + 5.0, 0.8), (root + 5.0 + third, 0.6), (root + 12.0, 0.8), (root - 7.0, 1.0)],
            vec![(root + 7.0, 0.8), (root + 11.0, 0.6), (root + 14.0, 0.6), (root - 5.0, 1.0)],
            vec![(root, 1.0), (root + third, 0.8), (root + 7.0, 0.8), (root - 12.0, 1.0)],
        ];
        chords.iter().flat_map(|c| tones(sr, 1.5, c, detune)).collect()
    }

    #[test]
    fn c_major_triad() {
        let s = tones(44100, 4.0, &[(60.0, 1.0), (64.0, 0.8), (67.0, 0.8), (48.0, 1.0)], 0.0);
        assert_eq!(detect_key(&s, 2, 44100).map(Key::name), Some("C"));
    }

    #[test]
    fn a_minor_triad() {
        let s = tones(44100, 4.0, &[(57.0, 1.0), (60.0, 0.8), (64.0, 0.8), (45.0, 1.0)], 0.0);
        assert_eq!(detect_key(&s, 2, 44100).map(Key::name), Some("Am"));
    }

    #[test]
    fn cadences_in_several_keys_and_tunings() {
        for (root, minor, want, detune) in [
            (60.0, false, "C", 0.0), (57.0, true, "Am", 0.0), (66.0, false, "F#", 30.0),
            (65.0, true, "Fm", -35.0), (56.0, true, "Abm", 15.0), (63.0, false, "Eb", 0.0),
        ] {
            let s = cadence(48000, root, minor, detune);
            assert_eq!(detect_key(&s, 2, 48000).map(Key::name), Some(want), "root {root} minor {minor} detune {detune}");
        }
    }

    #[test]
    fn mono_and_silence() {
        let st = tones(22050, 4.0, &[(62.0, 1.0), (66.0, 0.8), (69.0, 0.8)], 0.0);
        let mono: Vec<f32> = st.chunks(2).map(|f| f[0]).collect();
        assert_eq!(detect_key(&mono, 1, 22050).map(Key::name), Some("D"));
        assert_eq!(detect_key(&vec![0.0; 44100 * 4], 2, 44100), None);
        assert_eq!(detect_key(&[], 2, 44100), None);
    }

    #[test]
    fn names_camelot_and_ids() {
        let want = [
            ("Abm", "1A", 1), ("B", "1B", 2), ("Ebm", "2A", 3), ("F#", "2B", 4), ("Bbm", "3A", 5),
            ("Db", "3B", 6), ("Fm", "4A", 7), ("Ab", "4B", 8), ("Cm", "5A", 9), ("Eb", "5B", 10),
            ("Gm", "6A", 11), ("Bb", "6B", 12), ("Dm", "7A", 13), ("F", "7B", 14), ("Am", "8A", 15),
            ("C", "8B", 16), ("Em", "9A", 17), ("G", "9B", 18), ("Bm", "10A", 19), ("D", "10B", 20),
            ("F#m", "11A", 21), ("A", "11B", 22), ("Dbm", "12A", 23), ("E", "12B", 24),
        ];
        for (name, cam, id) in want {
            let k = Key::parse(name).unwrap();
            assert_eq!(k.name(), name);
            assert_eq!(k.camelot_name(), cam, "{name}");
            assert_eq!(k.dbserver_id(), id, "{name}");
            assert_eq!(dbserver_key_id(name), Some(id));
            assert_eq!(Key::from_dbserver_id(id), Some(k));
            assert_eq!(Key::parse(cam), Some(k));
        }
        // Other spellings normalise to rekordbox's.
        for (alt, name) in [("G#m", "Abm"), ("C#m", "Dbm"), ("Gb", "F#"), ("C#", "Db"), ("D#m", "Ebm"), ("A#", "Bb"),
                            ("a minor", "Am"), ("Amin", "Am"), ("F♯m", "F#m"), ("E♭", "Eb"), ("cm", "Cm")] {
            assert_eq!(Key::parse(alt).map(Key::name), Some(name), "{alt}");
        }
        assert_eq!(Key::parse(""), None);
        assert_eq!(Key::parse("H"), None);
        assert_eq!(Key::parse("13A"), None);
        assert_eq!(Key::from_dbserver_id(0), None);
        assert_eq!(camelot_of("Am"), Some((8, true)));
    }
}
