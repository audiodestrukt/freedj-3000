//! Display command bodies for the CDJ HID-over-LINK transport: the 0x2A deck
//! list and the 0x22/0x23/0x26 text fields. These are *bodies*; split them
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
}
