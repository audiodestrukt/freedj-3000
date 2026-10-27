//! Offline A/B of the key-lock engines: render a track through Rubber Band
//! R3 and Bungee at a set of speeds, master tempo on, to WAV files for a
//! blind listening test.
//!
//!     cargo run --release -p opendeck-app --example stretch_ab -- track.mp3 out/ [0.92,1.08,...]
//!
//! Output: out/<stem>-<engine>-<speed>.wav (32-bit float, the track's own
//! rate and channel count) plus a line per file with the render time and its
//! real-time factor.  The default speeds are ±4, ±8 and ±16 %.  Blind it with
//! something like `for f in out/*.wav; do cp "$f" "out/$(sha1sum "$f" | cut -c1-6).wav"; done`.

use opendeck_decode::SymphoniaDecoder;
use opendeck_timestretch::{Engine, Stretcher};
use opendeck_types::{Decoder, PipelineStage};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: stretch_ab <track> <out-dir> [speeds, comma-separated]");
        std::process::exit(2);
    }
    let track = Path::new(&args[0]);
    let out = PathBuf::from(&args[1]);
    let speeds: Vec<f32> = match args.get(2) {
        Some(s) => s.split(',').map(|x| x.trim().parse().expect("speed")).collect(),
        None => vec![0.84, 0.92, 0.96, 1.04, 1.08, 1.16],
    };
    std::fs::create_dir_all(&out).expect("out dir");

    let (samples, sr, ch) = decode(track);
    let stem = track.file_stem().unwrap().to_string_lossy().into_owned();
    let secs = samples.len() as f64 / ch as f64 / sr as f64;
    println!("{}: {:.1}s at {}Hz {}ch", track.display(), secs, sr, ch);

    const BLOCK: usize = 512;
    for engine in [Engine::RubberBand, Engine::Bungee] {
        for &speed in &speeds {
            let mut stage = Stretcher::new(engine, sr, ch as u8);
            stage.set_speed(speed);
            let latency = stage.latency_frames();
            // Pre-roll as the deck does, discarding the warm-up output.
            let mut scratch = Vec::new();
            stage.process(&vec![0.0f32; stage.latency_frames() * ch], &mut scratch);
            let mut rendered = Vec::with_capacity((samples.len() as f32 / speed) as usize + 4096);
            let t0 = Instant::now();
            for chunk in samples.chunks(BLOCK * ch) {
                stage.process(chunk, &mut rendered);
            }
            let dt = t0.elapsed().as_secs_f64();
            let name = format!("{stem}-{}-{speed:.2}.wav", match engine { Engine::RubberBand => "r3", Engine::Bungee => "bungee" });
            write_wav(&out.join(&name), &rendered, sr, ch);
            println!(
                "  {name:40} {:7.1}s audio  render {dt:6.2}s  RTF {:.3}  latency {latency} frames ({:.0} ms)",
                rendered.len() as f64 / ch as f64 / sr as f64,
                dt / secs,
                latency as f64 * 1000.0 / sr as f64,
            );
        }
    }
}

fn decode(path: &Path) -> (Vec<f32>, u32, usize) {
    let mut d = SymphoniaDecoder::open(path).expect("open track");
    let (sr, ch) = (d.sample_rate(), d.channels() as usize);
    let mut samples = Vec::new();
    let mut buf = vec![0f32; 4096 * ch];
    loop {
        match d.decode(&mut buf).expect("decode") {
            0 => break,
            n => samples.extend_from_slice(&buf[..n * ch]),
        }
    }
    (samples, sr, ch)
}

/// Minimal WAV writer: IEEE float, 32-bit, interleaved.
fn write_wav(path: &Path, samples: &[f32], sr: u32, ch: usize) {
    let data_len = (samples.len() * 4) as u32;
    let mut w = std::io::BufWriter::new(std::fs::File::create(path).expect("create wav"));
    w.write_all(b"RIFF").unwrap();
    w.write_all(&(36 + data_len).to_le_bytes()).unwrap();
    w.write_all(b"WAVEfmt ").unwrap();
    w.write_all(&16u32.to_le_bytes()).unwrap();
    w.write_all(&3u16.to_le_bytes()).unwrap();            // IEEE float
    w.write_all(&(ch as u16).to_le_bytes()).unwrap();
    w.write_all(&sr.to_le_bytes()).unwrap();
    w.write_all(&(sr * ch as u32 * 4).to_le_bytes()).unwrap();
    w.write_all(&((ch * 4) as u16).to_le_bytes()).unwrap();
    w.write_all(&32u16.to_le_bytes()).unwrap();
    w.write_all(b"data").unwrap();
    w.write_all(&data_len.to_le_bytes()).unwrap();
    for s in samples {
        w.write_all(&s.to_le_bytes()).unwrap();
    }
}
