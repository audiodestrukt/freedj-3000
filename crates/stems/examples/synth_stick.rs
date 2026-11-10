//! Write a synthetic prepared stick, to check our test writer against
//! OverCue's reference validator:
//!
//! ```sh
//! cargo run -p opendeck-stems --example synth_stick -- /tmp/stick flac96 8
//! python3 verify_usb.py /tmp/stick /Contents/Artist/Track.mp3
//! ```
//! Arguments: output dir, codec (flac96 | zlib96 | flac44 | wav44),
//! optional index-v2 bucket bits.

use opendeck_stems::synth::{write_stick, SynthCodec, SynthTrack};
use opendeck_stems::Role;
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::Path::new(args.get(1).map(String::as_str).unwrap_or("synth-stick"));
    let codec = match args.get(2).map(String::as_str).unwrap_or("flac96") {
        "zlib96" => SynthCodec::Zlib96, "flac44" => SynthCodec::Flac44, "wav44" => SynthCodec::Wav44, _ => SynthCodec::Flac96,
    };
    let bits = args.get(3).and_then(|b| b.parse().ok());
    let frames = 100_000;
    let tone = |f: f64, amp: f64| -> Vec<i16> {
        (0..frames).flat_map(|i| { let v = (amp * (std::f64::consts::TAU * f * i as f64 / 96_000.0).sin()) as i16; [v, v] }).collect()
    };
    let mut roles = BTreeMap::new();
    roles.insert(Role::Drums, tone(60.0, 6000.0));
    roles.insert(Role::Vocal, tone(440.0, 6000.0));
    roles.insert(Role::Harmonics, tone(880.0, 6000.0));
    let full = (0..frames * 2).map(|i| roles.values().map(|v: &Vec<i16>| v[i] as i32).sum::<i32>() as i16).collect();
    roles.insert(Role::FullMix, full);
    let tracks = [SynthTrack { id: 123, onelibrary_id: Some(456), file_path: "/Contents/Artist/Track.mp3".into(), source: b"not really an mp3".to_vec(), roles }];
    std::fs::create_dir_all(out)?;
    write_stick(out, &tracks, codec, bits)?;
    println!("wrote {}", out.display());
    Ok(())
}
