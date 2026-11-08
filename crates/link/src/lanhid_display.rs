//! Display command bodies for the CDJ HID-over-LINK transport: the 0x2A deck
//! list, the 0x22/0x23/0x26 text fields and the 0x2C RGB overview. These are *bodies*; split them
//! with [`crate::lanhid_frag::fragment`] before sending. Formats per the
//! integration brief, pages 4 and 6.

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
}
