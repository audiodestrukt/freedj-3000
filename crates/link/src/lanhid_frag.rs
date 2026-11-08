//! Canonical 64-byte fragmentation for the CDJ HID-over-LINK display commands
//! (0x22–0x30, including the 0x2A deck rows). Per the integration brief's
//! page 6, a body is split into reports of
//!
//! ```text
//! 00 CMD seqLE16 countLE16 [up to 58 body bytes]
//! ```
//!
//! `seq` starts at 1, `count` is the total, the final report is zero-padded.
//! Byte 0 is left zero for the caller (the LAN tunnel sets the route byte; the
//! emulator socket sends it as is).

/// Body bytes carried by one report: 64 − 6 header bytes.
pub const BODY_PER_FRAGMENT: usize = 58;

/// Split `body` into the canonical reports for command `cmd`. An empty body
/// still yields one (all-zero-body) report.
pub fn fragment(cmd: u8, body: &[u8]) -> Vec<[u8; 64]> {
    let count = body.len().div_ceil(BODY_PER_FRAGMENT).max(1);
    assert!(count <= u16::MAX as usize, "body too large to fragment");
    (0..count)
        .map(|i| {
            let mut r = [0u8; 64];
            r[1] = cmd;
            r[2..4].copy_from_slice(&((i + 1) as u16).to_le_bytes());
            r[4..6].copy_from_slice(&(count as u16).to_le_bytes());
            let chunk = body
                .get(i * BODY_PER_FRAGMENT..)
                .map(|rest| &rest[..rest.len().min(BODY_PER_FRAGMENT)])
                .unwrap_or(&[]);
            r[6..6 + chunk.len()].copy_from_slice(chunk);
            r
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout() {
        let f = fragment(0x22, &[0xAA; 10]);
        assert_eq!(f.len(), 1);
        assert_eq!(&f[0][..6], &[0x00, 0x22, 1, 0, 1, 0]);
        assert_eq!(&f[0][6..16], &[0xAA; 10]);
        assert!(f[0][16..].iter().all(|&b| b == 0), "final report zero-padded");
    }

    #[test]
    fn exact_multiple_has_no_trailing_empty_report() {
        assert_eq!(fragment(0x2E, &[1; 58]).len(), 1);
        assert_eq!(fragment(0x2E, &[1; 116]).len(), 2);
        assert_eq!(fragment(0x2E, &[1; 117]).len(), 3);
    }

    #[test]
    fn overview_is_73_fragments() {
        // 0xC0 + 600 seven-byte records = 4201 B, per the brief.
        let f = fragment(0x2C, &vec![0u8; 4201]);
        assert_eq!(f.len(), 73);
        let last = &f[72];
        assert_eq!(&last[2..6], &[73, 0, 73, 0]);
    }

    #[test]
    fn body_round_trips_through_fragments() {
        let body: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        let f = fragment(0x2A, &body);
        let mut back = Vec::new();
        for r in &f {
            back.extend_from_slice(&r[6..]);
        }
        assert_eq!(&back[..body.len()], &body[..]);
        assert!(back[body.len()..].iter().all(|&b| b == 0));
        for (i, r) in f.iter().enumerate() {
            assert_eq!(u16::from_le_bytes([r[2], r[3]]) as usize, i + 1);
            assert_eq!(u16::from_le_bytes([r[4], r[5]]) as usize, f.len());
        }
    }

    #[test]
    fn empty_body_is_one_report() {
        let f = fragment(0x30, &[]);
        assert_eq!(f.len(), 1);
        assert_eq!(&f[0][..6], &[0, 0x30, 1, 0, 1, 0]);
    }
}
