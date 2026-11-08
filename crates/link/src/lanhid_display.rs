//! Display command bodies for the CDJ HID-over-LINK transport: the 0x2A deck
//! list, the 0x22/0x23/0x26 text fields, the 0x2C RGB overview, the 0x2E
//! scrolling waveform, the 0x2F beat grid and the 0x30 hot cues. These are
//! *bodies*; split them with [`crate::lanhid_frag::fragment`] before sending.
//! Formats per the integration brief, pages 4 and 6.

use opendeck_types::BeatGrid;

/// Deck-list rows (command 0x2A).
pub const CMD_DECK_ROWS: u8 = 0x2A;
/// Title text.
pub const CMD_TITLE: u8 = 0x22;
/// Artist text.
pub const CMD_ARTIST: u8 = 0x23;
/// Auxiliary text.
pub const CMD_AUX: u8 = 0x26;

/// RGB overview (the whole-track strip under the time ruler).
pub const CMD_OVERVIEW: u8 = 0x2C;

/// The overview always carries this many records.
pub const OVERVIEW_COLUMNS: usize = 600;
/// Tallest bar the overview draws; heights run 1..=this.
pub const OVERVIEW_MAX_HEIGHT: u8 = 40;

/// Scrolling (enlarged) waveform.
pub const CMD_SCROLL: u8 = 0x2E;
/// Points per second of track in the scrolling waveform (the brief's
/// demonstrated timebase).
pub const SCROLL_POINTS_PER_SEC: u32 = 150;
/// Tallest scrolling-waveform point; heights run 0..=this.
pub const SCROLL_MAX_HEIGHT: u8 = 31;

/// Beat grid.
pub const CMD_GRID: u8 = 0x2F;
/// Hot cues.
pub const CMD_HOT_CUES: u8 = 0x30;

/// Working CDJ-3000 cue colour IDs for pads A..H (brief p.6).
pub const CUE_PALETTE: [u8; 8] = [0x04, 0x12, 0x3A, 0x08, 0x0C, 0x3C, 0x0E, 0x01];

/// The deck list always carries this many rows, empty ones included.
pub const DECK_ROWS: usize = 11;

/// NUL-terminated UTF-16LE, the encoding every display text field uses.
pub fn utf16z(s: &str) -> Vec<u8> {
    let mut out: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    out.extend_from_slice(&[0, 0]);
    out
}

/// Body for 0x22/0x23/0x26.
pub fn text(s: &str) -> Vec<u8> {
    utf16z(s)
}

/// One row of the CDJ's Browse deck list. Metadata the app does not have can
/// stay zero.
#[derive(Debug, Clone, Default)]
pub struct DeckRow {
    pub selected: bool,
    pub color: u8,
    /// BPM × 10 (1280 = 128.0); 0 = none.
    pub bpm_x10: u16,
    pub key: u8,
    pub text: String,
}

impl DeckRow {
    pub fn label(text: &str) -> Self {
        DeckRow { text: text.into(), ..Default::default() }
    }
}

/// Body for 0x2A: [`DECK_ROWS`] rows, each the 8-byte CDJ-3000 prefix
/// `{index, selected/icon (0x80), color, BPM fraction, BPM integer LE16, key,
/// reserved}` then NUL-terminated UTF-16LE text. Missing rows are sent empty;
/// extra rows are dropped. (The older 4-byte prefix corrupts CDJ-3000 text.)
pub fn deck_rows(rows: &[DeckRow]) -> Vec<u8> {
    let mut body = Vec::new();
    for i in 0..DECK_ROWS {
        let row = rows.get(i).cloned().unwrap_or_default();
        body.push(i as u8);
        body.push(if row.selected { 0x80 } else { 0x00 });
        body.push(row.color);
        body.push((row.bpm_x10 % 10) as u8);
        body.extend_from_slice(&(row.bpm_x10 / 10).to_le_bytes());
        body.push(row.key);
        body.push(0);
        body.extend_from_slice(&utf16z(&row.text));
    }
    body
}

/// 8-bit RGB → RGB565 (R5 G6 B5).
pub fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// Body for 0x2C from waveform columns `[bass, mid, high, amplitude]` (the
/// app's `WaveformCache` layout, any column rate). The whole track is pooled
/// into [`OVERVIEW_COLUMNS`] records of `{bg RGB565 LE16, fg RGB565 LE16, bg
/// height, fg height, 0}`, preceded by `0xC0`. Each record's background is the
/// span's peak (dimmed), its foreground the span's mean, both scaled so the
/// track's loudest column fills [`OVERVIEW_MAX_HEIGHT`]. Colour is the band
/// mix brought to full brightness: the analysis' band energies are small
/// numbers, so passed through raw they draw as near-black; the hue carries the
/// bass/mid/high balance and the height carries loudness. No columns →
/// [`overview_clear`].
pub fn overview(cols: &[[u8; 4]]) -> Vec<u8> {
    if cols.is_empty() {
        return overview_clear();
    }
    let n = cols.len();
    let max_amp = cols.iter().map(|c| c[3] as u32).max().unwrap_or(0).max(1);
    let height = |a: u32| (1 + a * (OVERVIEW_MAX_HEIGHT as u32 - 1) / max_amp) as u8;
    let mut body = Vec::with_capacity(1 + OVERVIEW_COLUMNS * 7);
    body.push(0xC0);
    for i in 0..OVERVIEW_COLUMNS {
        let a = i * n / OVERVIEW_COLUMNS;
        let b = ((i + 1) * n / OVERVIEW_COLUMNS).max(a + 1).min(n);
        let span = &cols[a..b];
        let mut sum = [0u32; 4];
        let mut peak = 0u32;
        for c in span {
            for k in 0..4 {
                sum[k] += c[k] as u32;
            }
            peak = peak.max(c[3] as u32);
        }
        let len = span.len() as u32;
        let [r, g, bl] = full_brightness([sum[0] / len, sum[1] / len, sum[2] / len]);
        let fg = rgb565(r, g, bl);
        let bg = rgb565(r / 2, g / 2, bl / 2);
        body.extend_from_slice(&bg.to_le_bytes());
        body.extend_from_slice(&fg.to_le_bytes());
        body.push(height(peak));
        body.push(height(sum[3] / len));
        body.push(0);
    }
    body
}

/// Body for 0x2E from waveform columns `[bass, mid, high, amplitude]` that
/// each cover `hop` frames at `sample_rate`: an LE32 point count, then one LE16
/// per point at [`SCROLL_POINTS_PER_SEC`], packed `(R3<<13)|(G3<<10)|(B3<<7)|
/// (height5<<2)`. A point takes the loudest column it overlaps, so transients
/// survive the resample. Colour and height scale as in [`overview`].
pub fn scroll(cols: &[[u8; 4]], hop: usize, sample_rate: u32) -> Vec<u8> {
    let n = cols.len() as u64;
    let hop = hop.max(1) as u64;
    let sr = sample_rate.max(1) as u64;
    let pps = SCROLL_POINTS_PER_SEC as u64;
    // Track length in points: n columns × hop frames ÷ sr × pps.
    let points = n * hop * pps / sr;
    let max_amp = cols.iter().map(|c| c[3] as u32).max().unwrap_or(0).max(1);
    let mut body = Vec::with_capacity(4 + points as usize * 2);
    body.extend_from_slice(&(points as u32).to_le_bytes());
    for j in 0..points {
        // Columns overlapping [j, j+1) points.
        let a = (j * sr / (pps * hop)).min(n - 1) as usize;
        let b = (((j + 1) * sr).div_ceil(pps * hop)).clamp(a as u64 + 1, n) as usize;
        let c = cols[a..b].iter().max_by_key(|c| c[3]).copied().unwrap_or([0; 4]);
        let [r, g, bl] = full_brightness([c[0] as u32, c[1] as u32, c[2] as u32]);
        let h = (c[3] as u32 * SCROLL_MAX_HEIGHT as u32 / max_amp) as u16;
        let word = ((r as u16 >> 5) << 13) | ((g as u16 >> 5) << 10) | ((bl as u16 >> 5) << 7) | (h << 2);
        body.extend_from_slice(&word.to_le_bytes());
    }
    body
}

/// Body for 0x2F: LE16 beat count, then per beat `{beat-in-bar 1..4, total
/// ms LE24}`. `beats` is `(beat_in_bar, ms)` in track order; anything past
/// the LE16 count is dropped.
pub fn beat_grid(beats: &[(u8, u32)]) -> Vec<u8> {
    let beats = &beats[..beats.len().min(u16::MAX as usize)];
    let mut body = Vec::with_capacity(2 + beats.len() * 4);
    body.extend_from_slice(&(beats.len() as u16).to_le_bytes());
    for &(bib, ms) in beats {
        body.push(bib.clamp(1, 4));
        body.extend_from_slice(&ms.to_le_bytes()[..3]);
    }
    body
}

/// Every beat of `grid` inside a track of `total_frames`, as `(beat_in_bar,
/// ms)` for [`beat_grid`]. Grid positions are **frames** at `sample_rate` (what
/// the analyser produces). A variable-tempo grid lists its own beats; a
/// constant one is extended from the anchor both ways, so beats before the
/// anchor are included too.
pub fn grid_beats(grid: &BeatGrid, sample_rate: u32, total_frames: u64) -> Vec<(u8, u32)> {
    let sr = sample_rate.max(1) as f64;
    let ms = |frame: f64| (frame * 1000.0 / sr).round() as u32;
    let bib = |k: i64| ((k + grid.downbeat_offset as i64).rem_euclid(4) + 1) as u8;
    if !grid.beats.is_empty() {
        return grid
            .beats
            .iter()
            .take_while(|&&f| f < total_frames)
            .enumerate()
            .map(|(k, &f)| (bib(k as i64), ms(f as f64)))
            .collect();
    }
    if grid.bpm <= 0.0 {
        return Vec::new();
    }
    let period = 60.0 * sr / grid.bpm;
    let first = -((grid.anchor_sample as f64 / period).floor() as i64);
    let mut out = Vec::new();
    for k in first.. {
        let f = grid.anchor_sample as f64 + k as f64 * period;
        if f >= total_frames as f64 || out.len() >= u16::MAX as usize {
            break;
        }
        out.push((bib(k), ms(f)));
    }
    out
}

/// One stored hot cue for [`hot_cues`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotCue {
    /// CDJ colour ID, e.g. from [`CUE_PALETTE`]. 0 = no colour (the pad and
    /// marker go dark).
    pub color: u8,
    pub ms: u32,
}

/// Body for 0x30 (52 bytes): the track duration `{min, sec, ms LE16}`, then
/// pads A..H as `{type, colour, min, sec, ms LE16}` — type 1 for a stored
/// cue, six zeros for an empty pad. Send on every track change and cue edit,
/// all eight pads every time (a cue-free track clears them all).
pub fn hot_cues(duration_ms: u32, cues: &[Option<HotCue>; 8]) -> Vec<u8> {
    use crate::lanhid_status::time4;
    let mut body = Vec::with_capacity(52);
    body.extend_from_slice(&time4(duration_ms));
    for cue in cues {
        match cue {
            Some(c) => {
                body.push(1);
                body.push(c.color);
                body.extend_from_slice(&time4(c.ms));
            }
            None => body.extend_from_slice(&[0; 6]),
        }
    }
    body
}

/// Scale a band mix so its strongest band is 255, keeping the ratio (hue).
/// Silence stays black.
fn full_brightness(rgb: [u32; 3]) -> [u8; 3] {
    let m = rgb[0].max(rgb[1]).max(rgb[2]);
    if m == 0 {
        return [0; 3];
    }
    rgb.map(|v| (v * 255 / m) as u8)
}

/// The 0x2C body that empties the overview: a zero lead byte and 4200 zeros.
pub fn overview_clear() -> Vec<u8> {
    vec![0u8; 1 + OVERVIEW_COLUMNS * 7]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_nul_terminated_utf16le() {
        assert_eq!(text("Hi"), vec![b'H', 0, b'i', 0, 0, 0]);
        assert_eq!(text(""), vec![0, 0]);
        // Non-ASCII survives as UTF-16.
        assert_eq!(text("é"), vec![0xE9, 0x00, 0, 0]);
    }

    #[test]
    fn deck_rows_always_eleven() {
        let body = deck_rows(&[DeckRow::label("OPENDECK")]);
        // Row 0: 8 prefix + "OPENDECK" (16) + NUL (2); rows 1..10: 8 + NUL.
        assert_eq!(body.len(), (8 + 16 + 2) + 10 * (8 + 2));
        // Second row starts right after the first and carries index 1.
        assert_eq!(body[26], 1);
    }

    #[test]
    fn row_prefix_layout() {
        let row = DeckRow {
            selected: true,
            color: 0x04,
            bpm_x10: 1281,
            key: 7,
            text: "Deck 1".into(),
        };
        let body = deck_rows(&[DeckRow::label("H"), row]);
        let r1 = &body[(8 + 2 + 2)..];
        assert_eq!(&r1[..8], &[1, 0x80, 0x04, 1, 128, 0, 7, 0]);
        assert_eq!(&r1[8..8 + 14], &utf16z("Deck 1")[..]);
    }

    #[test]
    fn rgb565_packs_r5_g6_b5() {
        assert_eq!(rgb565(255, 255, 255), 0xFFFF);
        assert_eq!(rgb565(255, 0, 0), 0xF800);
        assert_eq!(rgb565(0, 255, 0), 0x07E0);
        assert_eq!(rgb565(0, 0, 255), 0x001F);
    }

    #[test]
    fn overview_is_4201_bytes_whatever_the_column_count() {
        for n in [1usize, 599, 600, 601, 20_000] {
            let cols = vec![[10, 20, 30, 40]; n];
            let body = overview(&cols);
            assert_eq!(body.len(), 4201, "n={n}");
            assert_eq!(body[0], 0xC0);
        }
        assert_eq!(overview(&[]), overview_clear());
        assert_eq!(overview_clear().len(), 4201);
    }

    #[test]
    fn overview_heights_scale_to_loudest_and_stay_in_range() {
        // Quiet first half, loud second half.
        let mut cols = vec![[0, 0, 0, 10]; 600];
        cols.extend(vec![[200, 0, 0, 250]; 600]);
        let body = overview(&cols);
        let rec = |i: usize| &body[1 + i * 7..1 + i * 7 + 7];
        assert_eq!(rec(599)[5], OVERVIEW_MAX_HEIGHT, "loudest fills the strip");
        assert!(rec(0)[5] >= 1 && rec(0)[5] < 5, "quiet stays low but visible");
        for i in 0..OVERVIEW_COLUMNS {
            let r = rec(i);
            assert!((1..=OVERVIEW_MAX_HEIGHT).contains(&r[4]) && (1..=OVERVIEW_MAX_HEIGHT).contains(&r[5]));
            assert!(r[4] >= r[5], "peak envelope never below the mean");
            assert_eq!(r[6], 0);
        }
        // Bass-heavy span reads red: fg is the full mix, bg the dimmed one.
        let fg = u16::from_le_bytes([rec(599)[2], rec(599)[3]]);
        let bg = u16::from_le_bytes([rec(599)[0], rec(599)[1]]);
        assert_eq!(fg, rgb565(255, 0, 0));
        assert_eq!(bg, rgb565(127, 0, 0));
    }

    #[test]
    fn colour_keeps_hue_at_full_brightness() {
        assert_eq!(full_brightness([20, 10, 0]), [255, 127, 0]);
        assert_eq!(full_brightness([3, 3, 3]), [255, 255, 255]);
        assert_eq!(full_brightness([0, 0, 0]), [0, 0, 0]);
    }

    #[test]
    fn overview_pools_peaks_not_point_samples() {
        // One transient among 100 quiet columns per bin must survive.
        let mut cols = vec![[0, 0, 0, 1]; 60_000];
        cols[30_050] = [0, 0, 255, 255];
        let body = overview(&cols);
        let bin = 30_050 * 600 / 60_000;
        assert_eq!(body[1 + bin * 7 + 4], OVERVIEW_MAX_HEIGHT);
    }

    #[test]
    fn scroll_length_follows_the_track_at_150_per_second() {
        // 10 s at 44.1 kHz, 512-frame hop.
        let n = 10 * 44_100 / 512;
        let body = scroll(&vec![[0, 0, 255, 100]; n], 512, 44_100);
        let pts = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
        assert!((1495..=1500).contains(&pts), "pts={pts}");
        assert_eq!(body.len(), 4 + pts * 2);
    }

    #[test]
    fn scroll_word_packing() {
        // One loud blue column, one silent: blue at full height, then nothing.
        let cols = [[0, 0, 255, 200], [0, 0, 0, 0]];
        let body = scroll(&cols, 44_100 / 150, 44_100); // one column per point
        let w0 = u16::from_le_bytes([body[4], body[5]]);
        let w1 = u16::from_le_bytes([body[6], body[7]]);
        assert_eq!(w0, (7 << 7) | (31 << 2));
        assert_eq!(w1, 0);
        assert_eq!(w0 & 0b11, 0, "low two bits stay clear");
    }

    #[test]
    fn scroll_keeps_transients_when_columns_outnumber_points() {
        // 4 columns per point; a single loud column must still peak its point.
        let mut cols = vec![[0, 0, 0, 1]; 400];
        cols[201] = [255, 0, 0, 255];
        let body = scroll(&cols, 44_100 / 600, 44_100);
        let w = u16::from_le_bytes([body[4 + 50 * 2], body[5 + 50 * 2]]);
        assert_eq!((w >> 2) & 31, 31);
        assert_eq!(w >> 13, 7);
    }

    #[test]
    fn scroll_of_nothing_is_an_empty_count() {
        assert_eq!(scroll(&[], 512, 44_100), vec![0, 0, 0, 0]);
    }

    #[test]
    fn beat_grid_records() {
        let body = beat_grid(&[(1, 0), (2, 469), (3, 0x01_02_03)]);
        assert_eq!(&body[..2], &[3, 0]);
        assert_eq!(&body[2..6], &[1, 0, 0, 0]);
        assert_eq!(&body[6..10], &[2, 0xD5, 0x01, 0]);
        assert_eq!(&body[10..14], &[3, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn constant_grid_covers_the_track_including_pre_anchor_beats() {
        // 120 BPM at 48 kHz → a beat every 24 000 frames; anchor on beat 3.
        let mut g = BeatGrid::new_constant(60_000, 120.0);
        g.downbeat_offset = 0;
        let beats = grid_beats(&g, 48_000, 48_000 * 10);
        // 10 s at 2 beats/s, first beat 12 000 frames (250 ms) in.
        assert_eq!(beats.len(), 20);
        assert_eq!(beats[0].1, 250);
        assert_eq!(beats[1].1, 750);
        // The anchor (1250 ms) is a downbeat; beats before it count back.
        let anchor = beats.iter().position(|b| b.1 == 1250).unwrap();
        assert_eq!(beats[anchor].0, 1);
        assert_eq!(beats[anchor - 1].0, 4);
        assert_eq!(beats[anchor + 1].0, 2);
        assert!(beats.iter().all(|b| (1..=4).contains(&b.0)));
    }

    #[test]
    fn variable_grid_uses_its_own_beats() {
        let mut g = BeatGrid::new_constant(0, 120.0);
        g.beats = vec![0, 22_000, 47_000, 900_000];
        g.downbeat_offset = 1;
        let beats = grid_beats(&g, 44_000, 100_000);
        assert_eq!(beats, vec![(2, 0), (3, 500), (4, 1068)]);
    }

    #[test]
    fn hot_cues_body_is_52_bytes() {
        let mut cues = [None; 8];
        cues[0] = Some(HotCue { color: CUE_PALETTE[0], ms: 61_250 });
        cues[7] = Some(HotCue { color: CUE_PALETTE[7], ms: 999 });
        let body = hot_cues(527_328, &cues);
        assert_eq!(body.len(), 52);
        assert_eq!(&body[..4], &[8, 47, 0x48, 0x01]); // 8:47.328
        assert_eq!(&body[4..10], &[1, 0x04, 1, 1, 250, 0]); // A at 1:01.250
        assert!(body[10..46].iter().all(|&b| b == 0), "B..G empty");
        assert_eq!(&body[46..52], &[1, 0x01, 0, 0, 0xE7, 0x03]);
        assert_eq!(hot_cues(0, &[None; 8]), vec![0u8; 52]);
    }
}
