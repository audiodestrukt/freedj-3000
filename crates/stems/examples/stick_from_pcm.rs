//! Write a prepared stick from raw role audio, for end-to-end runs of the
//! deck with a real track (the stems can be any split that sums to the mix):
//!
//! ```sh
//! cargo run -p opendeck-stems --example stick_from_pcm -- <stick dir> \
//!     /Contents/Artist/Track.mp3 <original file> drums.raw vocal.raw harmonics.raw full.raw
//! ```
//! Role files are 96 kHz stereo s16le.  The original is copied onto the
//! stick at the given path and hashed into the index.

use opendeck_stems::synth::{write_stick, SynthCodec, SynthTrack};
use opendeck_stems::Role;
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    anyhow::ensure!(a.len() == 8, "usage: <stick dir> <stick path> <original> drums.raw vocal.raw harmonics.raw full.raw");
    let read = |p: &str| -> anyhow::Result<Vec<i16>> {
        Ok(std::fs::read(p)?.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
    };
    let mut roles = BTreeMap::new();
    for (r, p) in [(Role::Drums, &a[4]), (Role::Vocal, &a[5]), (Role::Harmonics, &a[6]), (Role::FullMix, &a[7])] {
        roles.insert(r, read(p)?);
    }
    let track = SynthTrack { id: 1, onelibrary_id: None, file_path: a[2].clone(), source: std::fs::read(&a[3])?, roles };
    write_stick(std::path::Path::new(&a[1]), &[track], SynthCodec::Flac96, Some(4))?;
    println!("wrote {}", a[1]);
    Ok(())
}
