//! Read a timecode recording (16-bit stereo WAV) the way the deck would.
//!
//!   cargo run -p opendeck-timecode --example tc_analyze -- rec.wav [vinyl|cd] [hop_ms] [flags]
//!
//! One line per hop: time, the decoder's speed, lock and position, and the
//! vinyl logic's view (armed, signal, settling, track ms). The Rust twin
//! of freedj-esp's native/timecode_analyze, for a recording of your own
//! record through your own interface.
use opendeck_timecode::{vinyl::Drive, Decoder, Format, Vinyl};

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
    if std::env::var("TC_DRIVE").is_ok() {
        // The deck in ABS: a cursor moved by the drive's speed, 10 ms
        // blocks, 20 ms lead; print every jump.
        let blk = rate as usize / 100;
        let mut d = Drive::default();
        let mut cur_ms = 0.0f64;
        for (n, c) in pcm.chunks(blk * 2).enumerate() {
            v.push_i16(c);
            let s = v.state();
            if !s.armed { continue; }
            let t = n as f64 * 10.0;
            let o = d.step(&s, true, t, 10.0, cur_ms, 20.0, 1e9);
            if let Some(m) = o.seek_ms { println!("{:8.2} s  jump {:+.0} ms (to {m:.0}), speed {:+.3}, record {:?}", t / 1000.0, m - cur_ms, o.speed, s.pos_ms); cur_ms = m; }
            cur_ms += o.speed * 10.0;
        }
        println!("{} jumps, {} of them re-syncs", d.seeks, d.trims);
        return;
    }
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
