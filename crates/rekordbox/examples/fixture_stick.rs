//! Build a rekordbox stick to test against: rekordcrate's two-track demo
//! export, with your audio files at the tracks' paths and analysis files
//! (`.DAT` + `.EXT`) carrying a beat grid and coloured, commented hot and
//! memory cues.  Point OpenDeck at it with `OPENDECK_MEDIA_DIRS=<out>/..`.
//!
//! cargo run -p opendeck-rekordbox --example fixture_stick -- <out dir> <audio1> [audio2] [--bpm N] [--first-ms M]

use opendeck_rekordbox::fixtures::{dat_and_ext, pqtz, FxCue};
use std::path::{Path, PathBuf};

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        if e.file_type()?.is_dir() { copy_dir(&e.path(), &dst)?; } else { std::fs::copy(e.path(), dst)?; }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut opt = |name: &str, default: f64| -> anyhow::Result<f64> {
        Ok(match args.iter().position(|a| a == name) {
            Some(i) => { let v = args[i + 1].parse()?; args.drain(i..i + 2); v }
            None => default,
        })
    };
    let bpm = opt("--bpm", 120.0)?;
    // Where the first beat is, so the grid sits on the music (the deck's own
    // analysis: `cargo run -p opendeck-app --example analyze_file`).
    let first_ms = opt("--first-ms", 0.0)?.round() as u32;
    let [out, audio @ ..] = args.as_slice() else {
        anyhow::bail!("usage: fixture_stick <out dir> <audio1> [audio2] [--bpm N] [--first-ms M]");
    };
    let out = PathBuf::from(out);
    let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/rekordcrate/data/complete_export/demo_tracks");
    copy_dir(&demo, &out)?;
    let exp = opendeck_rekordbox::read_export(&out)?;
    // Track 1: every hot cue colour case; track 2: plain cues only.
    let red = FxCue { hot: 1, ms: 4_000, code: 0x2a, rgb: (0xe6, 0x28, 0x28), comment: "drop", ..Default::default() };
    let cues: [(Vec<FxCue>, Vec<FxCue>); 2] = [
        (vec![FxCue { ms: 0, code: 2, comment: "intro", ..Default::default() }],
         vec![red,
              FxCue { hot: 2, ms: 12_000, code: 0x09, rgb: (0x00, 0xe0, 0xff), ..Default::default() },
              FxCue { hot: 3, ms: 20_000, code: 0x22, rgb: (0xff, 0xa0, 0x00), comment: "vocal", ..Default::default() },
              FxCue { hot: 4, ms: 28_000, ..Default::default() },
              FxCue { hot: 8, ms: 60_000, code: 0x38, rgb: (0xb4, 0x32, 0xff), comment: "outro", ..Default::default() }]),
        (vec![FxCue { ms: 0, ..Default::default() }],
         vec![FxCue { hot: 1, ms: 8_000, ..Default::default() }]),
    ];
    for ((t, (mem, hot)), src) in exp.tracks.iter().zip(cues).zip(audio) {
        let dst = t.path_on(&out);
        std::fs::create_dir_all(dst.parent().unwrap())?;
        std::fs::copy(src, &dst)?;
        let dat = t.analyze_on(&out).ok_or_else(|| anyhow::anyhow!("{} has no analysis path", t.title))?;
        let (d, e) = dat_and_ext(pqtz(bpm, first_ms, 2000), &mem, &hot);
        std::fs::write(&dat, d)?;
        std::fs::write(dat.with_extension("EXT"), e)?;
        println!("{} ← {} ({} hot cues)", t.title, src, hot.len());
    }
    Ok(())
}
