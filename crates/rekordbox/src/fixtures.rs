//! rekordbox analysis files built byte for byte, for tests here and in the
//! app: a `.DAT` with a beat grid and cue lists, a `.EXT` with coloured,
//! commented cues.  Layouts follow Deep Symmetry's `rekordbox_anlz.ksy`;
//! the byte values are what rekordbox writes (a cue point is type 1, an
//! empty comment has length 0, A is hot cue 1).

fn be32(v: u32) -> [u8; 4] { v.to_be_bytes() }

/// A whole ANLZ file (`PMAI` header) around `sections`.
pub fn anlz_file(sections: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = sections.concat();
    let mut f = b"PMAI".to_vec();
    f.extend(be32(0x1c));
    f.extend(be32(0x1c + body.len() as u32));
    f.extend([0u8; 16]);
    f.extend(body);
    f
}

/// A constant-tempo beat grid (PQTZ): `n` beats at `bpm` from `first_ms`,
/// the first one a downbeat.
pub fn pqtz(bpm: f64, first_ms: u32, n: u32) -> Vec<u8> {
    let mut b = Vec::new();
    for i in 0..n {
        b.extend(((i % 4 + 1) as u16).to_be_bytes());
        b.extend(((bpm * 100.0).round() as u16).to_be_bytes());
        b.extend(be32(first_ms + (i as f64 * 60_000.0 / bpm).round() as u32));
    }
    let mut s = b"PQTZ".to_vec(); s.extend(be32(0x18)); s.extend(be32(0x18 + b.len() as u32));
    s.extend(be32(0)); s.extend(be32(0x80000)); s.extend(be32(n));
    s.extend(b);
    s
}

/// A cue in a fixture: hot cue number (0 = memory point, 1 = A …), start,
/// loop end, and what only the `.EXT` holds: colour code, RGB, comment.
#[derive(Clone, Debug, Default)]
pub struct FxCue {
    pub hot: u32,
    pub ms: u32,
    pub loop_end_ms: Option<u32>,
    pub code: u8,
    pub rgb: (u8, u8, u8),
    pub comment: &'static str,
}

/// The `.DAT` cue list (PCOB of PCPT entries); `hot` picks the hot cue list.
pub fn pcob(hot: bool, cues: &[FxCue]) -> Vec<u8> {
    let mut e = Vec::new();
    for c in cues {
        e.extend(b"PCPT"); e.extend(be32(0x1c)); e.extend(be32(0x38));
        e.extend(be32(c.hot)); e.extend(be32(1)); e.extend(be32(0x10000));
        e.extend([0xff, 0xff, 0xff, 0xff]);
        e.push(if c.loop_end_ms.is_some() { 2 } else { 1 }); e.push(0); e.extend(1000u16.to_be_bytes());
        e.extend(be32(c.ms)); e.extend(be32(c.loop_end_ms.unwrap_or(u32::MAX)));
        e.extend([0u8; 16]);
    }
    let mut s = b"PCOB".to_vec(); s.extend(be32(0x18)); s.extend(be32(0x18 + e.len() as u32));
    s.extend(be32(hot as u32)); s.extend([0, 0]); s.extend((cues.len() as u16).to_be_bytes()); s.extend(be32(u32::MAX));
    s.extend(e);
    s
}

/// The `.EXT` extended cue list (PCO2 of PCP2 entries).
pub fn pco2(hot: bool, cues: &[FxCue]) -> Vec<u8> {
    let mut e = Vec::new();
    for c in cues {
        let mut cm: Vec<u8> = c.comment.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        if !c.comment.is_empty() { cm.extend([0, 0]); }
        let total = 0x2c + cm.len() as u32 + 24;
        e.extend(b"PCP2"); e.extend(be32(0x10)); e.extend(be32(total));
        e.extend(be32(c.hot)); e.push(if c.loop_end_ms.is_some() { 2 } else { 1 }); e.push(0); e.extend(1000u16.to_be_bytes());
        e.extend(be32(c.ms)); e.extend(be32(c.loop_end_ms.unwrap_or(u32::MAX)));
        e.push(if hot { 0 } else { c.code }); e.extend([0, 0, 0]); e.extend([0u8; 4]);
        e.extend([0u8; 4]);
        e.extend(be32(cm.len() as u32)); e.extend(&cm);
        e.push(if hot { c.code } else { 0 }); e.extend([c.rgb.0, c.rgb.1, c.rgb.2]); e.extend([0u8; 20]);
    }
    let mut s = b"PCO2".to_vec(); s.extend(be32(0x14)); s.extend(be32(0x14 + e.len() as u32));
    s.extend(be32(hot as u32)); s.extend((cues.len() as u16).to_be_bytes()); s.extend([0, 0]);
    s.extend(e);
    s
}

/// A track's `.DAT` and `.EXT`: grid plus memory and hot cues.
pub fn dat_and_ext(grid: Vec<u8>, memory: &[FxCue], hot: &[FxCue]) -> (Vec<u8>, Vec<u8>) {
    (anlz_file(&[grid, pcob(false, memory), pcob(true, hot)]),
     anlz_file(&[pco2(false, memory), pco2(true, hot)]))
}

/// rekordbox's RGB detail waveform (PWV5, in the `.EXT`): one entry per
/// 1/150 s, each red, green, blue (0-7) and height (0-31), packed big-endian
/// as r:3 g:3 b:3 h:5 and 2 unused bits.
pub fn pwv5(entries: &[[u8; 4]]) -> Vec<u8> {
    let mut b = Vec::new();
    for e in entries {
        let v = ((e[0] as u16 & 7) << 13) | ((e[1] as u16 & 7) << 10) | ((e[2] as u16 & 7) << 7) | ((e[3] as u16 & 31) << 2);
        b.extend(v.to_be_bytes());
    }
    let mut s = b"PWV5".to_vec(); s.extend(be32(0x18)); s.extend(be32(0x18 + b.len() as u32));
    s.extend(be32(2)); s.extend(be32(entries.len() as u32)); s.extend(be32(0x0096_0305));
    s.extend(b);
    s
}

/// rekordbox's RGB preview (PWV4, in the `.EXT`): 6 bytes per column,
/// 1,200 columns for the whole track.
pub fn pwv4(entries: &[[u8; 6]]) -> Vec<u8> {
    let b: Vec<u8> = entries.iter().flatten().copied().collect();
    let mut s = b"PWV4".to_vec(); s.extend(be32(0x18)); s.extend(be32(0x18 + b.len() as u32));
    s.extend(be32(6)); s.extend(be32(entries.len() as u32)); s.extend(be32(0));
    s.extend(b);
    s
}
