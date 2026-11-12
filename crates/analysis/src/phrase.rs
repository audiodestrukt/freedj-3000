//! Phrase analysis ("song structure"): rekordbox's PSSI section, which a
//! CDJ-3000 reads from a track's `.EXT` file to colour the waveform by phrase
//! (intro / verse / chorus / up / down / outro ...) and drive lighting.
//!
//! [`parse_pssi`] / [`encode_pssi`] read and write the section byte for byte
//! as rekordbox exports it, including the XOR mask rekordbox 6+ applies to
//! everything after the entry count.  [`detect`] derives a structure from
//! OpenDeck's own beat grid and waveform columns.

use crate::WaveformCache;

/// The overall structure rekordbox assigns a track; it decides how phrase
/// kinds read.  Values as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u16)]
pub enum Mood { #[default] High = 1, Mid = 2, Low = 3 }

impl Mood {
    pub fn from_u16(v: u16) -> Option<Mood> {
        match v { 1 => Some(Mood::High), 2 => Some(Mood::Mid), 3 => Some(Mood::Low), _ => None }
    }
}

/// Phrase kinds, independent of mood, for display and comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Coarse { Intro, Verse, Up, Down, Chorus, Bridge, Outro, Unknown }

/// One phrase entry (24 bytes in the file).  `index` is implicit (position + 1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Phrase {
    /// Beat (1-based, in the track's beat grid) where the phrase starts.
    pub beat: u16,
    /// Mood-dependent kind id (see [`Phrase::coarse`]).
    pub kind: u16,
    /// High-mood variant flags (Intro 1/2, Up 1/2/3, Chorus 1/2, Outro 1/2).
    pub k1: u8,
    pub k2: u8,
    pub k3: u8,
    /// "Up 3" extra beats: `b` = 0 → `beat2` only, 1 → `beat2..beat4`.
    pub b: u8,
    pub beat2: u16,
    pub beat3: u16,
    pub beat4: u16,
    /// Non-zero when a fill-in ends the phrase, starting at `beat_fill`.
    pub fill: u8,
    pub beat_fill: u16,
    /// The bytes no one has decoded (entry offsets 6, 8, 10, 18, 20), kept so
    /// a parsed section re-encodes identically.
    pub pad: [u8; 5],
}

/// A whole PSSI section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongStructure {
    pub mood: Mood,
    /// Beat at which the last phrase ends.
    pub end_beat: u16,
    /// Lighting bank (0 = none chosen).
    pub bank: u8,
    /// Undecoded header bytes 0x14-0x19, 0x1c-0x1d and 0x1f.
    pub unk1: [u8; 6],
    pub unk2: u16,
    pub unk3: u8,
    pub phrases: Vec<Phrase>,
}

impl Phrase {
    /// The kind as rekordbox labels it, coarsely.
    pub fn coarse(&self, mood: Mood) -> Coarse {
        match (mood, self.kind) {
            (_, 1) => Coarse::Intro,
            (Mood::High, 2) => Coarse::Up,
            (Mood::High, 3) => Coarse::Down,
            (Mood::High, 5) => Coarse::Chorus,
            (Mood::High, 6) => Coarse::Outro,
            (Mood::Mid | Mood::Low, 2..=7) => Coarse::Verse,
            (Mood::Mid | Mood::Low, 8) => Coarse::Bridge,
            (Mood::Mid | Mood::Low, 9) => Coarse::Chorus,
            (Mood::Mid | Mood::Low, 10) => Coarse::Outro,
            _ => Coarse::Unknown,
        }
    }

    /// The label rekordbox shows ("Intro 1", "Up 3", "Verse 2", ...).
    pub fn label(&self, mood: Mood) -> String {
        let v = |k: u8| if k != 0 { 1 } else { 2 };
        match (mood, self.kind) {
            (Mood::High, 1) => format!("Intro {}", v(self.k1)),
            (Mood::High, 2) => format!("Up {}", if self.k2 != 0 { 3 } else if self.k3 != 0 { 2 } else { 1 }),
            (Mood::High, 3) => "Down".into(),
            (Mood::High, 5) => format!("Chorus {}", v(self.k1)),
            (Mood::High, 6) => format!("Outro {}", v(self.k1)),
            (Mood::Mid, k @ 2..=7) => format!("Verse {}", k - 1),
            (Mood::Low, 2..=4) => "Verse 1".into(),
            (Mood::Low, 5..=7) => "Verse 2".into(),
            (_, 1) => "Intro".into(),
            (_, 8) => "Bridge".into(),
            (_, 9) => "Chorus".into(),
            (_, 10) => "Outro".into(),
            (_, k) => format!("? {k}"),
        }
    }
}

/// rekordbox's mask base; each byte has the entry count added.
const MASK: [u8; 19] = [0xCB, 0xE1, 0xEE, 0xFA, 0xE5, 0xEE, 0xAD, 0xEE, 0xE9, 0xD2, 0xE9, 0xEB, 0xE1, 0xE9, 0xF3, 0xE8, 0xE9, 0xF4, 0xE1];
const HEADER: usize = 0x20;
const ENTRY: usize = 24;

/// XOR everything after the entry count (offset 0x12) with the mask (its own inverse).
fn mask(section: &mut [u8]) {
    let c = u16::from_be_bytes([section[0x10], section[0x11]]) as u8;
    for (i, b) in section[0x12..].iter_mut().enumerate() { *b ^= MASK[i % MASK.len()].wrapping_add(c); }
}

/// Parse a PSSI section (starting at its fourcc), masked or not.
pub fn parse_pssi(section: &[u8]) -> Result<SongStructure, String> {
    if section.len() < HEADER || &section[..4] != b"PSSI" { return Err("not a PSSI section".into()); }
    let be16 = |b: &[u8], o: usize| u16::from_be_bytes([b[o], b[o + 1]]);
    let be32 = |b: &[u8], o: usize| u32::from_be_bytes(b[o..o + 4].try_into().unwrap());
    let len_header = be32(section, 4) as usize;
    let len_tag = (be32(section, 8) as usize).min(section.len());
    let entry_bytes = be32(section, 12) as usize;
    let n = be16(section, 0x10) as usize;
    if len_header != HEADER || entry_bytes != ENTRY || len_tag < HEADER + n * ENTRY {
        return Err(format!("unexpected PSSI layout: header {len_header}, entry {entry_bytes}, {n} entries in {len_tag} bytes"));
    }
    let mut s = section[..len_tag].to_vec();
    // rekordbox 6+ masks; an unmasked mood is 1-3.
    if be16(&s, 0x12) > 20 { mask(&mut s); }
    let mood = Mood::from_u16(be16(&s, 0x12)).ok_or_else(|| format!("unknown mood {}", be16(&s, 0x12)))?;
    let phrases = (0..n).map(|i| {
        let e = &s[HEADER + i * ENTRY..HEADER + (i + 1) * ENTRY];
        Phrase {
            beat: be16(e, 2), kind: be16(e, 4), k1: e[7], k2: e[9], b: e[11],
            beat2: be16(e, 12), beat3: be16(e, 14), beat4: be16(e, 16), k3: e[19], fill: e[21], beat_fill: be16(e, 22),
            pad: [e[6], e[8], e[10], e[18], e[20]],
        }
    }).collect();
    Ok(SongStructure {
        mood, end_beat: be16(&s, 0x1a), bank: s[0x1e],
        unk1: s[0x14..0x1a].try_into().unwrap(), unk2: be16(&s, 0x1c), unk3: s[0x1f], phrases,
    })
}

/// The PSSI section as rekordbox writes it in a `.EXT` file: header, entries,
/// masked when `masked` (rekordbox 6+ exports, and what players serve).
pub fn encode_pssi(s: &SongStructure, masked: bool) -> Vec<u8> {
    let n = s.phrases.len();
    let len = HEADER + n * ENTRY;
    let mut b = Vec::with_capacity(len);
    b.extend_from_slice(b"PSSI");
    for v in [HEADER as u32, len as u32, ENTRY as u32] { b.extend_from_slice(&v.to_be_bytes()); }
    b.extend_from_slice(&(n as u16).to_be_bytes());
    b.extend_from_slice(&(s.mood as u16).to_be_bytes());
    b.extend_from_slice(&s.unk1);
    b.extend_from_slice(&s.end_beat.to_be_bytes());
    b.extend_from_slice(&s.unk2.to_be_bytes());
    b.push(s.bank);
    b.push(s.unk3);
    for (i, p) in s.phrases.iter().enumerate() {
        b.extend_from_slice(&(i as u16 + 1).to_be_bytes());
        b.extend_from_slice(&p.beat.to_be_bytes());
        b.extend_from_slice(&p.kind.to_be_bytes());
        b.extend_from_slice(&[p.pad[0], p.k1, p.pad[1], p.k2, p.pad[2], p.b]);
        for v in [p.beat2, p.beat3, p.beat4] { b.extend_from_slice(&v.to_be_bytes()); }
        b.extend_from_slice(&[p.pad[3], p.k3, p.pad[4], p.fill]);
        b.extend_from_slice(&p.beat_fill.to_be_bytes());
    }
    if masked { mask(&mut b); }
    b
}

// ── Detection ────────────────────────────────────────────────────────────────
//
// Bars come from the beat grid (the bar phase is re-estimated here, since the
// grid's downbeat is not analysed); each bar gets a feature vector from the
// waveform columns (bass, mid, high, level, normalised per track).  A Foote
// checkerboard novelty over bars marks section changes; boundaries are its
// peaks, at least `MIN_BARS` apart, nudged onto 4-bar multiples of the
// previous boundary, and sections longer than `MAX_BARS` are split into
// 8-bar phrases.  Each phrase's energy (bass-weighted) against the track's
// spread labels it: the quiet lead-in is the intro, the quiet tail the
// outro, loud phrases choruses, and in between up / down (high mood) or
// verse / bridge (mid, low).  The mood follows how bass-driven the track is.

/// The detector's tunables (defaults are the shipped values).
#[derive(Clone, Copy, Debug)]
pub struct PhraseConfig {
    /// Sections longer than this many bars are split...
    pub max_bars: usize,
    /// ...into phrases of this many bars.
    pub split_bars: usize,
    /// Novelty peaks count above this multiple of the track's median.
    pub nov_mult: f64,
    /// High mood: at least this share of bars bass-heavy...
    pub high_driven: f64,
    /// ...and the median bass level (0-255 columns) above this.
    pub high_bass: f64,
    /// Low mood: median bass below this.
    pub low_bass: f64,
}

impl Default for PhraseConfig {
    // Calibrated on 31 rekordbox-analysed tracks over LINK (examples/
    // phrase_vs_rekordbox_link): rekordbox's phrases are mostly 8 bars, so
    // sections over 12 bars split into 8 (phrase-start F1 0.61 -> 0.72).
    // Mood moved only modestly: that set was 28 High / 3 Mid, and our
    // features did not separate its Mid tracks, so it cannot calibrate mood.
    fn default() -> Self { Self { max_bars: 12, split_bars: 8, nov_mult: 2.0, high_driven: 0.45, high_bass: 60.0, low_bass: 40.0 } }
}

/// What the mood decision saw, for calibration.
#[derive(Clone, Copy, Debug, Default)]
pub struct MoodFeatures { pub driven: f64, pub bass_med: f64 }

const MIN_BARS: usize = 4;
/// Novelty kernel half-width, bars.
const KERNEL: usize = 4;

fn sorted(mut v: Vec<f64>) -> Vec<f64> { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v }

/// Phrase structure from OpenDeck's analysis: the beat list a player gets
/// (beat-in-bar, bpm, ms per beat; beat 1 is the first entry) and the
/// waveform columns ([bass, mid, high, rms] per `hop_size` frames).
/// Empty (no phrases) without a grid or with under 8 bars.
pub fn detect(beats: &[(u8, f32, u32)], wave: &WaveformCache, frames: u64, sr: u32) -> SongStructure {
    detect_with(beats, wave, frames, sr, false)
}

/// As [`detect`]; with `downbeats_known` the grid's beat-in-bar numbers are
/// trusted (a rekordbox grid), so bars start where it says beat 1 is rather
/// than where the bar phase is guessed.  Guessing put every phrase of
/// rekordbox's Demo Track 1 two beats off rekordbox's own.
pub fn detect_with(beats: &[(u8, f32, u32)], wave: &WaveformCache, frames: u64, sr: u32, downbeats_known: bool) -> SongStructure {
    detect_cfg(beats, wave, frames, sr, downbeats_known, &PhraseConfig::default()).0
}

/// As [`detect_with`], with explicit tunables; also returns the mood features.
pub fn detect_cfg(beats: &[(u8, f32, u32)], wave: &WaveformCache, frames: u64, sr: u32, downbeats_known: bool, cfg: &PhraseConfig) -> (SongStructure, MoodFeatures) {
    let mut feats = MoodFeatures::default();
    let out = detect_inner(beats, wave, frames, sr, downbeats_known, cfg, &mut feats);
    (out, feats)
}

fn detect_inner(beats: &[(u8, f32, u32)], wave: &WaveformCache, frames: u64, sr: u32, downbeats_known: bool, cfg: &PhraseConfig, feats: &mut MoodFeatures) -> SongStructure {
    let mut out = SongStructure { unk2: 0x0100, ..Default::default() };
    let cols = &wave.columns;
    if beats.len() < 32 || cols.is_empty() || sr == 0 { return out; }
    let track_ms = frames as f64 * 1000.0 / sr as f64;
    let inside = beats.iter().take_while(|b| (b.2 as f64) < track_ms).count();
    let beats = &beats[..inside.min(u16::MAX as usize - 8)];
    if beats.len() < 32 { return out; }
    // Per-beat features: mean column over the beat (columns sit at their
    // window centre, half an FFT later than column * hop).
    let hop = wave.hop_size.max(1) as f64;
    let half = crate::waveform::FFT_SIZE as f64 / 2.0;
    let col_at = |ms: f64| (((ms / 1000.0 * sr as f64 - half).max(0.0) / hop) as usize).min(cols.len() - 1);
    let period = |i: usize| if i + 1 < beats.len() { beats[i + 1].2.saturating_sub(beats[i].2) as f64 } else { 60_000.0 / beats[i].1.max(1.0) as f64 };
    let beat_feat: Vec<[f64; 4]> = (0..beats.len()).map(|i| {
        let s = beats[i].2 as f64;
        let a = col_at(s);
        let b = col_at(s + period(i)).max(a + 1).min(cols.len());
        let span = &cols[a.min(b - 1)..b];
        let mut f = [0.0; 4];
        for c in span { for k in 0..4 { f[k] += c[k] as f64 / span.len() as f64; } }
        f
    }).collect();

    // Bar phase: section changes and crashes land on downbeats, so the beat
    // whose rise in bass + highs over the beat before is largest, summed
    // across the track, starts the bars.
    let mut rise = [0.0f64; 4];
    for i in 1..beat_feat.len() {
        rise[i % 4] += (beat_feat[i][0] - beat_feat[i - 1][0]).max(0.0) + (beat_feat[i][2] - beat_feat[i - 1][2]).max(0.0);
    }
    let guessed = (0..4).max_by(|a, b| rise[*a].partial_cmp(&rise[*b]).unwrap()).unwrap();
    let phase = if downbeats_known { beats.iter().take(4).position(|b| b.0 == 1).unwrap_or(guessed) } else { guessed };

    // Bars from `phase`; a partial first bar joins the first phrase.
    let bar_start: Vec<usize> = (phase..beats.len()).step_by(4).filter(|&b| b + 4 <= beats.len()).collect();
    if bar_start.len() < 8 { return out; }
    let raw: Vec<[f64; 4]> = bar_start.iter().map(|&b| {
        let mut f = [0.0; 4];
        for x in &beat_feat[b..b + 4] { for k in 0..4 { f[k] += x[k] / 4.0; } }
        f
    }).collect();
    // Normalise each feature by the track's 95th percentile.
    let mut bar = raw.clone();
    for k in 0..4 {
        let v = sorted(raw.iter().map(|f| f[k]).collect());
        let p95 = v[(v.len() - 1) * 95 / 100].max(1.0);
        for f in &mut bar { f[k] = (f[k] / p95).min(1.5); }
    }
    let energy: Vec<f64> = bar.iter().map(|f| 0.45 * f[0] + 0.2 * f[1] + 0.15 * f[2] + 0.2 * f[3]).collect();

    // End: the last bar with real level; the phrases stop there.
    let loud = energy.iter().cloned().fold(0.0, f64::max);
    let nbars = energy.iter().rposition(|&e| e > 0.15 * loud).unwrap_or(bar.len() - 1) + 1;
    if nbars < 8 { return out; }
    let (bar, energy) = (&bar[..nbars], &energy[..nbars]);

    // Checkerboard novelty: distance between the KERNEL bars before and after.
    let mean = |r: std::ops::Range<usize>| {
        let mut m = [0.0; 4];
        for f in &bar[r.clone()] { for k in 0..4 { m[k] += f[k] / r.len() as f64; } }
        m
    };
    let nov: Vec<f64> = (0..nbars).map(|i| {
        if i < 2 || i + 2 > nbars { return 0.0; }
        let (x, y) = (mean(i.saturating_sub(KERNEL)..i), mean(i..(i + KERNEL).min(nbars)));
        (0..4).map(|k| (x[k] - y[k]).powi(2)).sum::<f64>().sqrt()
    }).collect();
    let thresh = (sorted(nov.clone())[nbars / 2] * cfg.nov_mult).max(0.08);

    // Peaks, strongest first, at least MIN_BARS from each other and the ends.
    let mut order: Vec<usize> = (1..nbars)
        .filter(|&i| nov[i] >= thresh && nov[i] >= nov[i - 1] && (i + 1 >= nbars || nov[i] >= nov[i + 1])).collect();
    order.sort_by(|a, b| nov[*b].partial_cmp(&nov[*a]).unwrap());
    let mut cuts: Vec<usize> = Vec::new();
    for i in order {
        if i >= MIN_BARS && i + MIN_BARS <= nbars && cuts.iter().all(|&c| c.abs_diff(i) >= MIN_BARS) { cuts.push(i); }
    }
    cuts.sort();
    // Nudge onto 4-bar multiples of the previous boundary when a bar away.
    let mut prev = 0;
    for c in cuts.iter_mut() {
        match (*c - prev) % 4 { 1 => *c -= 1, 3 => *c += 1, _ => {} }
        prev = *c;
    }
    cuts.dedup();
    cuts.retain(|&c| c >= MIN_BARS && c + MIN_BARS <= nbars);
    // Sections, long ones split into 8-bar phrases.
    let mut bounds = vec![0];
    for &c in cuts.iter().chain(std::iter::once(&nbars)) {
        let s = *bounds.last().unwrap();
        if c <= s { continue; }
        let mut x = s;
        while c - x > cfg.max_bars { x += cfg.split_bars; bounds.push(x); }
        bounds.push(c);
    }
    bounds.dedup();
    let segs: Vec<(usize, usize)> = bounds.windows(2).map(|w| (w[0], w[1])).collect();
    let n = segs.len();

    // Levels: each phrase's mean energy against the track's spread.
    let seg_e: Vec<f64> = segs.iter().map(|&(a, b)| energy[a..b].iter().sum::<f64>() / (b - a) as f64).collect();
    let hi = seg_e.iter().cloned().fold(0.0, f64::max);
    let lo = seg_e.iter().cloned().fold(f64::MAX, f64::min);
    let lv: Vec<u8> = seg_e.iter().map(|&e| {
        if hi - lo < 0.08 { return 1; }
        let t = (e - lo) / (hi - lo);
        if t >= 0.7 { 2 } else if t >= 0.35 { 1 } else { 0 }
    }).collect();
    // Intro: leading phrases below the top level (at most two); outro likewise.
    let first_hi = lv.iter().position(|&l| l == 2).unwrap_or(0);
    let last_hi = lv.iter().rposition(|&l| l == 2).unwrap_or(n - 1);
    let intro_end = if first_hi == 0 { usize::from(n > 2 && lv[0] < 2) } else { first_hi.min(2) };
    let outro_start = if last_hi + 1 >= n { n } else { (last_hi + 1).max(n.saturating_sub(2)) };

    // Mood: mostly bass-heavy bars read as high (EDM's up / down / chorus);
    // little bass as low; the rest mid.  Absolute levels are our waveform's
    // sqrt-scaled 0-255 columns.
    let bass_med = sorted(raw[..nbars].iter().map(|f| f[0]).collect())[nbars / 2];
    let driven = bar.iter().filter(|f| f[0] > 0.6).count() as f64 / nbars as f64;
    *feats = MoodFeatures { driven, bass_med };
    out.mood = if driven > cfg.high_driven && bass_med > cfg.high_bass { Mood::High } else if bass_med < cfg.low_bass { Mood::Low } else { Mood::Mid };

    let mut verse = 0u16;
    let mut prev_kind = 0u16;
    for (i, &(a, _)) in segs.iter().enumerate() {
        let l = lv[i];
        let mut p = Phrase::default();
        p.kind = match out.mood {
            Mood::High => {
                if i < intro_end { p.k1 = 1; 1 }
                else if i >= outro_start { p.k1 = 1; 6 }
                else if l == 2 { p.k1 = 1; 5 }
                else if lv.get(i + 1) == Some(&2) { 2 }
                else { 3 }
            }
            Mood::Mid | Mood::Low => {
                if i < intro_end { 1 }
                else if i >= outro_start { 10 }
                else if l == 2 { 9 }
                else if l == 0 { 8 }
                else {
                    // A new verse after a chorus or bridge; mid has six, low two.
                    if verse == 0 || !(2..=7).contains(&prev_kind) { verse += 1; }
                    match out.mood { Mood::Mid => 1 + verse.min(6), _ => if verse <= 1 { 2 } else { 5 } }
                }
            }
        };
        prev_kind = p.kind;
        p.beat = if i == 0 { 1 } else { bar_start[a] as u16 + 1 };
        out.phrases.push(p);
    }
    out.end_beat = ((bar_start[nbars - 1] + 4).min(beats.len()) as u16 + 1).min(beats.len() as u16);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO1: &[u8] = include_bytes!("../tests/fixtures/pssi_demo1.bin");
    const DEMO2: &[u8] = include_bytes!("../tests/fixtures/pssi_demo2.bin");

    #[test]
    fn parses_rekordbox_masked_sections() {
        // rekordbox 6's demo tracks (rekordcrate's export): mid mood, masked.
        let s = parse_pssi(DEMO1).unwrap();
        assert_eq!((s.mood, s.end_beat, s.bank, s.phrases.len()), (Mood::Mid, 353, 0, 19));
        assert_eq!((s.phrases[0].beat, s.phrases[1].beat), (1, 37));
        assert_eq!(s.phrases[1].label(s.mood), "Verse 1");
        assert_eq!(s.phrases[3].label(s.mood), "Verse 2");
        assert_eq!(s.phrases[18].coarse(s.mood), Coarse::Chorus);
        let s = parse_pssi(DEMO2).unwrap();
        assert_eq!((s.mood, s.end_beat, s.phrases.len()), (Mood::Mid, 257, 11));
        assert_eq!(s.phrases[0].label(s.mood), "Intro");
        assert_eq!(s.phrases[10].label(s.mood), "Outro");
        assert_eq!(s.phrases[5].coarse(s.mood), Coarse::Bridge);
        assert!(s.phrases.windows(2).all(|w| w[0].beat < w[1].beat));
    }

    #[test]
    fn re_encodes_byte_for_byte() {
        for fx in [DEMO1, DEMO2] {
            let s = parse_pssi(fx).unwrap();
            assert_eq!(encode_pssi(&s, true), fx);
            let plain = encode_pssi(&s, false);
            assert_eq!(u16::from_be_bytes([plain[0x12], plain[0x13]]), 2, "unmasked mood reads plainly");
            assert_eq!(parse_pssi(&plain).unwrap(), s);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_pssi(b"PSSI").is_err());
        assert!(parse_pssi(&[0u8; 64]).is_err());
        let mut bad = DEMO2.to_vec();
        bad[0x10] = 0x40;   // more entries than bytes
        assert!(parse_pssi(&bad).is_err());
    }

    #[test]
    fn labels_high_mood_variants() {
        let p = |kind, k1, k2, k3| Phrase { kind, k1, k2, k3, ..Default::default() };
        assert_eq!(p(1, 1, 0, 0).label(Mood::High), "Intro 1");
        assert_eq!(p(1, 0, 0, 0).label(Mood::High), "Intro 2");
        assert_eq!(p(2, 0, 0, 0).label(Mood::High), "Up 1");
        assert_eq!(p(2, 0, 0, 1).label(Mood::High), "Up 2");
        assert_eq!(p(2, 0, 1, 0).label(Mood::High), "Up 3");
        assert_eq!(p(6, 0, 0, 0).label(Mood::High), "Outro 2");
        assert_eq!(p(5, 0, 0, 0).label(Mood::Low), "Verse 2");
    }

    /// 120 BPM, 96 bars of 16-bar sections: quiet intro, loud, mid, loud,
    /// mid, quiet outro.
    fn synthetic() -> (Vec<(u8, f32, u32)>, WaveformCache, u64, u32) {
        let sr = 44_100u32;
        let beats: Vec<(u8, f32, u32)> = (0..384u32).map(|k| ((k % 4) as u8 + 1, 120.0, k * 500)).collect();
        let frames = 384 * sr as u64 / 2;
        let columns = (0..(frames / 512) as usize + 4).map(|c| {
            let bar = ((c * 512 + 1024) as f64 / sr as f64 / 2.0) as usize;
            match bar / 16 { 0 => [20, 60, 40, 50], 1 | 3 => [200, 120, 120, 200], 2 | 4 => [90, 110, 60, 120], _ => [20, 50, 30, 40] }
        }).collect();
        (beats, WaveformCache { columns, sample_rate: sr, hop_size: 512 }, frames, sr)
    }

    #[test]
    fn trusts_the_grids_downbeats_when_told() {
        // Same track, but the grid says bar 1 starts on the third beat: with
        // downbeats known, every phrase starts on a beat the grid calls 1.
        let (mut beats, wave, frames, sr) = synthetic();
        for (i, b) in beats.iter_mut().enumerate() { b.0 = ((i + 2) % 4 + 1) as u8; }   // beat 3 (index 2) is a 1
        let s = detect_with(&beats, &wave, frames, sr, true);
        assert!(!s.phrases.is_empty());
        for p in &s.phrases[1..] {
            assert_eq!(beats[p.beat as usize - 1].0, 1, "phrase at beat {} is not on a downbeat", p.beat);
        }
    }

    #[test]
    fn detects_sections_of_a_synthetic_track() {
        let (beats, wave, frames, sr) = synthetic();
        let s = detect(&beats, &wave, frames, sr);
        let starts: Vec<u16> = s.phrases.iter().map(|p| p.beat).collect();
        for b in [65u16, 129, 193, 257, 321] { assert!(starts.contains(&b), "{b} not in {starts:?}"); }
        assert!(starts.windows(2).all(|w| (w[1] - w[0]) % 4 == 0 && w[1] - w[0] >= 16), "{starts:?}");
        let kinds: Vec<Coarse> = s.phrases.iter().map(|p| p.coarse(s.mood)).collect();
        assert_eq!(kinds.first(), Some(&Coarse::Intro), "{kinds:?}");
        assert_eq!(kinds.last(), Some(&Coarse::Outro), "{kinds:?}");
        let at = |b: u16| s.phrases.iter().rev().find(|p| p.beat <= b).unwrap().coarse(s.mood);
        assert_eq!(at(70), Coarse::Chorus, "{kinds:?}");
        assert_ne!(at(140), Coarse::Chorus, "{kinds:?}");
        assert!(s.end_beat > 321 && s.end_beat as usize <= beats.len());
        assert_eq!(parse_pssi(&encode_pssi(&s, true)).unwrap(), s);
    }

    #[test]
    fn nothing_without_a_grid() {
        let (_, wave, frames, sr) = synthetic();
        assert!(detect(&[], &wave, frames, sr).phrases.is_empty());
    }
}
