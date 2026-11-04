//! Status feedback report (command 0x21) for the CDJ HID-over-LINK transport.
//!
//! Once a session is authenticated, the host sends this 64-byte report to each
//! CDJ at ~60 Hz so the unit's own screen and lamps follow the software deck:
//! play state, time, BPM, pitch, loop, cue and the tempo/quantize indicators.
//! Encoding per the integration brief's page-5 table, at canonical HID
//! offsets. The caller applies the host route byte `(p<<4)|0x01` to byte 0 and
//! wraps the result in a host tunnel frame.

/// Tempo range the deck shows; the code goes in the high nibble of byte 0x04.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempoRange {
    Six,
    Ten,
    Sixteen,
}

impl TempoRange {
    fn code(self) -> u8 {
        match self {
            TempoRange::Six => 1,
            TempoRange::Ten => 2,
            TempoRange::Sixteen => 3,
        }
    }
    pub fn percent(self) -> f32 {
        match self {
            TempoRange::Six => 6.0,
            TempoRange::Ten => 10.0,
            TempoRange::Sixteen => 16.0,
        }
    }
}

/// The deck's playback state, as the app knows it. [`encode`] turns it into a
/// 0x21 report.
#[derive(Debug, Clone)]
pub struct Status {
    pub playing: bool,
    pub track_loaded: bool,
    /// Tempo fader range and the two tempo-mode lamps.
    pub tempo_range: TempoRange,
    pub tempo_reset: bool,
    pub master_tempo: bool,
    pub vinyl: bool,
    pub quantize: bool,
    /// Physical beat-jump size index 0..7 → 1,2,4,8,16,32,64,128 beats.
    pub beat_jump_index: u8,
    /// Playback rate; 1.0 = 0 %. Pitch = round((rate-1)*10000).
    pub rate: f32,
    pub current_ms: u32,
    pub duration_ms: u32,
    /// Current BPM × 10 (e.g. 1281 = 128.1 BPM).
    pub bpm_x10: u16,
    /// Original (track) BPM × 10.
    pub original_bpm_x10: u16,
    /// Active loop, if any: its length in beats and its endpoints in ms.
    pub loop_active: Option<LoopState>,
    /// Caller toggles this every 250 ms to flash IN/OUT while a loop is active.
    pub loop_flash_on: bool,
    pub cue_ms: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct LoopState {
    pub beats: f32,
    pub start_ms: u32,
    pub end_ms: u32,
}

impl Default for Status {
    fn default() -> Self {
        Status {
            playing: false,
            track_loaded: false,
            tempo_range: TempoRange::Ten,
            tempo_reset: false,
            master_tempo: false,
            vinyl: true,
            quantize: false,
            beat_jump_index: 0,
            rate: 1.0,
            current_ms: 0,
            duration_ms: 0,
            bpm_x10: 0,
            original_bpm_x10: 0,
            loop_active: None,
            loop_flash_on: false,
            cue_ms: None,
        }
    }
}

/// minutes, seconds, ms-within-second LE16 — the 4-byte time fields.
fn time4(ms: u32) -> [u8; 4] {
    let total_s = ms / 1000;
    let m = (total_s / 60) as u8;
    let s = (total_s % 60) as u8;
    let msw = (ms % 1000) as u16;
    [m, s, msw as u8, (msw >> 8) as u8]
}

fn le24(v: u32) -> [u8; 3] {
    [v as u8, (v >> 8) as u8, (v >> 16) as u8]
}

/// Loop length in beats → the byte-0x0A enum.
fn loop_enum(beats: f32) -> u8 {
    match beats {
        b if b < 0.375 => 8,  // 1/4
        b if b < 0.75 => 10,  // 1/2
        b if b < 1.5 => 12,   // 1
        b if b < 3.0 => 13,   // 2
        b if b < 6.0 => 14,   // 4
        b if b < 12.0 => 15,  // 8
        b if b < 24.0 => 16,  // 16
        b if b < 48.0 => 17,  // 32
        b if b < 96.0 => 18,  // 64
        _ => 19,              // 128
    }
}

/// Encode a 64-byte 0x21 status report. Byte 0 (route) is left zero for the
/// caller to set.
pub fn encode(st: &Status) -> [u8; 64] {
    let mut r = [0u8; 64];
    r[1] = 0x21;

    r[0x02] = (if st.playing { 0x80 } else { 0x40 }) | 0x04;

    // Loop indicator: inactive steady, active flashes IN/OUT.
    r[0x03] = if st.loop_active.is_some() {
        if st.loop_flash_on { 0xE1 } else { 0x21 }
    } else {
        0xC1
    };

    r[0x04] = (st.tempo_range.code() << 4)
        | if st.tempo_reset { 8 } else { 0 }
        | if st.master_tempo { 4 } else { 0 };

    r[0x05] = if st.vinyl { 1 } else { 0 };
    r[0x06] = if st.quantize { 0x18 } else { 0x10 };

    r[0x07] = (st.beat_jump_index.min(7) + 7) << 4;

    // Loaded-state and valid-track-number fields; without them the CDJ shows
    // "Not Loaded" and a frozen time even as assets arrive.
    if st.track_loaded {
        r[0x09] = 0xF4;
        r[0x13] = 1;
    }

    r[0x0A] = match st.loop_active {
        Some(l) => loop_enum(l.beats),
        None => 0x80, // inactive bit
    };

    r[0x0B..0x0F].copy_from_slice(&time4(st.current_ms));
    r[0x0F..0x13].copy_from_slice(&time4(st.duration_ms));

    r[0x15] = (st.bpm_x10 % 10) as u8;
    r[0x16..0x18].copy_from_slice(&(st.bpm_x10 / 10).to_le_bytes());

    let pitch = ((st.rate - 1.0) * 10000.0).round() as i16;
    r[0x18..0x1A].copy_from_slice(&pitch.to_le_bytes());

    if let Some(l) = st.loop_active {
        r[0x1A..0x1D].copy_from_slice(&le24(l.start_ms));
        r[0x1D..0x20].copy_from_slice(&le24(l.end_ms));
        r[0x1C] &= 0x7F; // active: clear inverse-shading bit
    } else {
        r[0x1C] |= 0x80; // inactive: set it
    }

    if let Some(c) = st.cue_ms {
        r[0x20..0x23].copy_from_slice(&le24(c));
    }

    // No beat-countdown target.
    r[0x2D..0x31].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);

    let obpm = st.original_bpm_x10 / 10;
    let otenths = (st.original_bpm_x10 % 10) as u8;
    r[0x33] = obpm as u8;
    r[0x34] = (otenths << 4) | ((obpm >> 8) as u8 & 0x0F);

    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_state_and_markers() {
        let mut st = Status { playing: true, track_loaded: true, ..Default::default() };
        let r = encode(&st);
        assert_eq!(r[1], 0x21);
        assert_eq!(r[0x02], 0x84); // playing | 0x04
        assert_eq!(r[0x09], 0xF4);
        assert_eq!(r[0x13], 1);

        st.playing = false;
        assert_eq!(encode(&st)[0x02], 0x44);
    }

    #[test]
    fn not_loaded_leaves_markers_clear() {
        let r = encode(&Status::default());
        assert_eq!(r[0x09], 0);
        assert_eq!(r[0x13], 0);
    }

    #[test]
    fn bpm_and_pitch() {
        let st = Status { bpm_x10: 1281, rate: 1.02, ..Default::default() };
        let r = encode(&st);
        assert_eq!(r[0x15], 1); // tenths
        assert_eq!(u16::from_le_bytes([r[0x16], r[0x17]]), 128);
        assert_eq!(i16::from_le_bytes([r[0x18], r[0x19]]), 200); // +2% = +200
    }

    #[test]
    fn time_fields() {
        // 1:23.456
        let st = Status { current_ms: 83_456, ..Default::default() };
        let r = encode(&st);
        assert_eq!(r[0x0B], 1); // minutes
        assert_eq!(r[0x0C], 23); // seconds
        assert_eq!(u16::from_le_bytes([r[0x0D], r[0x0E]]), 456);
    }

    #[test]
    fn loop_shading_bit() {
        let active = Status {
            loop_active: Some(LoopState { beats: 4.0, start_ms: 1000, end_ms: 2000 }),
            ..Default::default()
        };
        let r = encode(&active);
        assert_eq!(r[0x0A], 14); // 4-beat enum
        assert_eq!(r[0x1C] & 0x80, 0); // active: clear
        assert_eq!(u32::from_le_bytes([r[0x1A], r[0x1B], r[0x1C], 0]), 1000);

        let r = encode(&Status::default());
        assert_eq!(r[0x0A] & 0x80, 0x80); // inactive bit
        assert_eq!(r[0x1C] & 0x80, 0x80);
    }

    #[test]
    fn no_beat_countdown() {
        let r = encode(&Status::default());
        assert_eq!(&r[0x2D..0x31], &[0xFF, 0xFF, 0xFF, 0xFF]);
    }
}
