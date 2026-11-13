//! Decode files the way the deck does and write each one's samples, for
//! comparing decoders (a symphonia upgrade, a patched codec) bit for bit.
//!
//!   cargo run --release -p opendeck-decode --example decode_dump -- out_dir file…
//!
//! Prints rate, channels, the header's frame count, frames decoded, and an
//! FNV-1a hash of the f32 samples; writes `out_dir/<file name>.f32` (raw
//! interleaved little-endian f32) for a closer diff, then the file's tags,
//! then a seek check: seek to 12.345 s, and how far the samples there are
//! from the straight decode's.

use opendeck_decode::symphonia_decoder::SymphoniaDecoder;
use opendeck_types::Decoder;
use std::io::Write;
use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let out_dir = args.next().expect("usage: decode_dump out_dir file…");
    std::fs::create_dir_all(&out_dir).unwrap();
    for f in args {
        let name = Path::new(&f).file_name().unwrap().to_string_lossy().to_string();
        let mut d = match SymphoniaDecoder::open(Path::new(&f)) {
            Ok(d) => d,
            Err(e) => { println!("{name:<24} OPEN ERROR {e}"); continue; }
        };
        let ch = d.channels() as usize;
        let mut buf = vec![0f32; 4096 * ch];
        let mut all: Vec<f32> = Vec::new();
        let mut err = None;
        loop {
            match d.decode(&mut buf) {
                Ok(0) => break,
                Ok(n) => all.extend_from_slice(&buf[..n * ch]),
                Err(e) => { err = Some(e.to_string()); break; }
            }
        }
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for s in &all { for b in s.to_bits().to_le_bytes() { h ^= b as u64; h = h.wrapping_mul(0x100_0000_01b3); } }
        println!("{name:<24} {:>6} Hz {ch} ch  header {:>9}  decoded {:>9}  hash {h:016x}{}",
                 d.sample_rate(), d.total_frames().map_or("-".into(), |t| t.to_string()),
                 all.len() / ch.max(1), err.map_or(String::new(), |e| format!("  ERROR {e}")));
        let t = d.tags();
        println!("{:<24} tags: {:?} | {:?} | {:?} | {:?} | {:?} | {:?} | key {:?} | bpm {:?} | art {:?}",
                 "", t.title, t.artist, t.album, t.genre, t.year, t.comment, t.key, t.bpm,
                 t.artwork.as_ref().map(|(m, b)| (m.clone(), b.len())));
        // Seek: where it lands, and whether the audio there matches.
        let want = (12.345 * d.sample_rate() as f64) as u64;
        if (all.len() / ch.max(1)) as u64 > want + 8192 {
            match d.seek(want) {
                Ok(at) => {
                    let mut got = Vec::new();
                    while got.len() < 4096 * ch {
                        match d.decode(&mut buf) { Ok(0) | Err(_) => break, Ok(n) => got.extend_from_slice(&buf[..n * ch]) }
                    }
                    let start = at as usize * ch;
                    let diff = got.iter().zip(&all[start.min(all.len())..]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
                    println!("{:<24} seek {want} -> {at} ({:+} frames), max |diff| there {diff:.1e}", "", at as i64 - want as i64);
                }
                Err(e) => println!("{:<24} seek ERROR {e}", ""),
            }
        }
        let mut w = std::io::BufWriter::new(std::fs::File::create(Path::new(&out_dir).join(format!("{name}.f32"))).unwrap());
        for s in &all { w.write_all(&s.to_le_bytes()).unwrap(); }
    }
}
