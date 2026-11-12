//! Does the decoder's sample rate match the audio it actually produces?
//!
//!   cargo run --release -p opendeck-decode --example probe_rate -- <files or dirs>...
//!
//! Per file: the rate the decoder reports (what the deck resamples from), the
//! rate in the decoded packets, the decoded length, and ffprobe's length when
//! ffprobe is installed. A rate mismatch or a length off by more than 1 %
//! plays the track at the wrong speed.

use opendeck_decode::symphonia_decoder::SymphoniaDecoder;
use opendeck_types::Decoder;
use std::path::{Path, PathBuf};

const EXTS: &[&str] = &["mp3", "m4a", "mp4", "aac", "flac", "wav", "aif", "aiff", "ogg", "alac", "caf"];

fn walk(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) { for e in rd.flatten() { walk(&e.path(), out); } }
    } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        && !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("._")) {
        out.push(p.to_path_buf());
    }
}

fn ffprobe_secs(p: &Path) -> Option<f64> {
    let o = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(p).output().ok()?;
    String::from_utf8_lossy(&o.stdout).trim().parse().ok()
}

fn main() {
    let mut files = Vec::new();
    for a in std::env::args().skip(1) { walk(Path::new(&a), &mut files); }
    let (mut bad, mut n) = (0, 0);
    for f in &files {
        let mut d = match SymphoniaDecoder::open(f) { Ok(d) => d, Err(e) => { println!("ERR  {e}  {}", f.display()); continue } };
        let (sr, ch) = (d.sample_rate(), d.channels() as usize);
        let mut buf = vec![0f32; 8192 * ch];
        let mut frames = 0u64;
        loop { match d.decode(&mut buf) { Ok(0) | Err(_) => break, Ok(k) => frames += k as u64 } }
        let spec = d.decoded_rate();
        let ours = frames as f64 / sr as f64;
        let theirs = ffprobe_secs(f);
        let off = theirs.map(|t| (ours / t - 1.0) * 100.0);
        let rate_bad = spec.is_some_and(|s| s != sr);
        let len_bad = off.is_some_and(|o| o.abs() > 1.0);
        n += 1;
        if rate_bad || len_bad { bad += 1; }
        println!("{}  reported {sr:>6}  decoded {:>6}  {ch}ch  {ours:>7.1}s  ffprobe {:>7}  {:>6}  {}",
                 if rate_bad || len_bad { "BAD " } else { "ok  " },
                 spec.map(|s| s.to_string()).unwrap_or("-".into()),
                 theirs.map(|t| format!("{t:.1}s")).unwrap_or("-".into()),
                 off.map(|o| format!("{o:+.1}%")).unwrap_or_default(), f.display());
    }
    println!("\n{n} files, {bad} wrong");
}
