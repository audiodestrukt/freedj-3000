//! The deck's analysis of one file: decoded length, BPM, and the grid's first
//! beat (ms), as the deck would compute it for a plain file.
//!
//!   cargo run --release -p opendeck-app --example analyze_file -- track.mp3 [more…]
use opendeck_analysis::BeatAnalyzerImpl;
use opendeck_decode::symphonia_decoder::SymphoniaDecoder;
use opendeck_types::{BeatAnalyzer, Decoder};

fn main() -> anyhow::Result<()> {
    for path in std::env::args().skip(1) {
        let mut d = SymphoniaDecoder::open(std::path::Path::new(&path))?;
        let (sr, ch) = (d.sample_rate(), d.channels() as usize);
        let mut pcm = Vec::new();
        let mut buf = vec![0f32; 4096 * ch];
        loop {
            match d.decode(&mut buf)? {
                0 => break,
                n => for f in buf[..n * ch].chunks(ch) { pcm.push(f[0]); pcm.push(if ch > 1 { f[1] } else { f[0] }); },
            }
        }
        let frames = pcm.len() / 2;
        let first = pcm.chunks(2).position(|f| f[0].abs() > 0.01).unwrap_or(0);
        let mut an = BeatAnalyzerImpl::new(sr);
        an.push(&pcm, sr);
        match an.beat_grid() {
            Some(g) => {
                let per = 60_000.0 / g.bpm;
                let a = g.anchor_sample as f64 / sr as f64 * 1000.0;
                println!("{path}: {frames} frames @ {sr} Hz, first sound {:.2} ms, {:.3} BPM, first grid beat {:.2} ms",
                         first as f64 / sr as f64 * 1000.0, g.bpm, a % per);
            }
            None => println!("{path}: {frames} frames @ {sr} Hz, no BPM"),
        }
    }
    Ok(())
}
