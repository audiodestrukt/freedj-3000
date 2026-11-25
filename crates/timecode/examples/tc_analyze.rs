//! Read a timecode recording (16-bit stereo WAV) the way the deck would.
//!
//!   cargo run -p opendeck-timecode --example tc_analyze -- rec.wav [vinyl|cd] [hop_ms] [flags]
//!
//! One line per hop: time, the decoder's speed, lock and position, and the
//! vinyl logic's view (armed, signal, settling, track ms). The Rust twin
//! of freedj-esp's native/timecode_analyze, for a recording of your own
//! record through your own interface.
use opendeck_timecode::{Decoder, Format, Vinyl};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = a.get(1).expect("usage: tc_analyze <wav> [vinyl|cd] [hop_ms] [flags]");
    let fmt = if a.get(2).is_some_and(|f| f == "cd") { Format::SeratoCd } else { Format::SeratoVinyl };
    let hop_ms: usize = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(250);
    let flags: u32 = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);
    let b = std::fs::read(path).expect("read");
    let mut i = 12;
    let (mut rate, mut ch, mut bits, mut data) = (0u32, 0u16, 0u16, None);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        if id == b"fmt " {
            ch = u16::from_le_bytes([b[i + 10], b[i + 11]]);
            rate = u32::from_le_bytes(b[i + 12..i + 16].try_into().unwrap());
            bits = u16::from_le_bytes([b[i + 22], b[i + 23]]);
        } else if id == b"data" {
            data = Some(&b[i + 8..(i + 8 + len).min(b.len())]);
            break;
        }
        i += 8 + len + (len & 1);
    }
    let data = data.expect("no data chunk");
    assert!(bits == 16 && ch == 2, "16-bit stereo only (have {bits}-bit, {ch} ch)");
    let pcm: Vec<i16> = data.chunks_exact(2).map(|x| i16::from_le_bytes([x[0], x[1]])).collect();
    let mut dec = Decoder::new(rate, fmt);
    dec.set_flags(flags);
    let mut v = Vinyl::new(rate, fmt);
    v.set_flags(flags);
    let hop = rate as usize * hop_ms / 1000 * 2;
    println!("{path}: {rate} Hz, {:.1} s, {}", pcm.len() as f64 / 2.0 / rate as f64, fmt.label());
    println!("{:>8}  {:>7}  {:>4}  {:>8}   {:>5} {:>6} {:>5}  {:>9}", "t s", "speed", "lock", "pos ms", "armed", "signal", "settl", "track ms");
    for (n, c) in pcm.chunks(hop).enumerate() {
        dec.push_i16(c);
        v.push_i16(c);
        let s = v.state();
        let p = dec.position();
        println!("{:8.2}  {:+7.3}  {:>4}  {:>8}   {:>5} {:>6} {:>5}  {:>9}",
                 (n + 1) as f64 * hop_ms as f64 / 1000.0, dec.speed(), if dec.locked() { "yes" } else { "-" },
                 if p >= 0 { (p as i64 * 1000 / dec.resolution_hz() as i64).to_string() } else { "-".into() },
                 s.armed as u8, s.signal as u8, s.settling as u8, s.pos_ms.map_or("-".into(), |m| m.to_string()));
    }
}
