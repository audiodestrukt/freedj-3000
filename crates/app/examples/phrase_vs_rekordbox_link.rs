//! Compare OpenDeck's phrases with rekordbox's for tracks in a rekordbox
//! collection served over LINK (rekordbox on a computer, LINK on): fetch
//! rekordbox's PSSI, beat grid and audio, run OpenDeck's detector on the
//! audio with rekordbox's own grid, print both phrase lists.
//!
//!   cargo run --release -p opendeck-app --example phrase_vs_rekordbox_link -- <rekordbox-ip> <track-id>...
use opendeck_analysis::phrase::{detect, detect_with, parse_pssi, SongStructure};
use opendeck_analysis::WaveformBuilder;
use opendeck_dbserver::{parse_beat_grid, Client, Slot};
use opendeck_nfs::Nfs;
use std::net::Ipv4Addr;

fn decode(bytes: Vec<u8>, ext: Option<&str>) -> anyhow::Result<(Vec<f32>, u32)> {
    use opendeck_types::Decoder;
    let mut d = opendeck_decode::symphonia_decoder::SymphoniaDecoder::open_bytes(bytes, ext)?;
    let (sr, ch) = (d.sample_rate(), d.channels() as usize);
    let (mut out, mut buf) = (Vec::new(), vec![0f32; 4096 * ch]);
    loop {
        match d.decode(&mut buf)? {
            0 => break,
            n => for f in buf[..n * ch].chunks(ch) { out.push(f[0]); out.push(if ch > 1 { f[1] } else { f[0] }); },
        }
    }
    Ok((out, sr))
}

fn show(name: &str, s: &SongStructure, beat_ms: &dyn Fn(u16) -> f64) {
    println!("  {name}: mood {:?}, end beat {}, {} phrases", s.mood, s.end_beat, s.phrases.len());
    for p in &s.phrases {
        let t = beat_ms(p.beat) / 1000.0;
        println!("    beat {:>4}  {:>5.1} s  {}", p.beat, t, p.label(s.mood));
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let ip: Ipv4Addr = a.next().expect("usage: phrase_vs_rekordbox_link <ip> <track-id>...").parse()?;
    let ids: Vec<u32> = a.map(|s| s.parse()).collect::<Result<_, _>>()?;
    let mut db = Client::discover(ip, 3)?;
    for id in ids {
        let blob = db.anlz_tag(Slot::Collection, id, b"PSSI", b"EXT")?;
        if blob.len() < 8 { println!("track {id}: no phrase analysis in rekordbox"); continue; }
        let theirs = parse_pssi(&blob[4..]).map_err(anyhow::Error::msg)?;
        let grid = parse_beat_grid(&db.beat_grid(Slot::Collection, id)?);
        let path = db.file_path(Slot::Collection, id)?.ok_or_else(|| anyhow::anyhow!("no path"))?;
        let mut nfs = Nfs::connect_at(ip, opendeck_nfs::PORTMAP_REKORDBOX)?;
        let export = nfs.exports()?.into_iter().next().ok_or_else(|| anyhow::anyhow!("no export"))?;
        let root = nfs.mount(&export)?;
        let (fh, size) = nfs.lookup_path(&root, &path)?;
        let (pcm, sr) = decode(nfs.read_file(&fh, size)?, path.rsplit('.').next())?;
        let mut wb = WaveformBuilder::new(sr);
        wb.push(&pcm);
        let wave = wb.finish();
        let beats: Vec<(u8, f32, u32)> = grid.iter().map(|b| (b.beat_in_bar, b.bpm, b.time_ms)).collect();
        let ours = detect(&beats, &wave, (pcm.len() / 2) as u64, sr);
        let ours_db = detect_with(&beats, &wave, (pcm.len() / 2) as u64, sr, true);
        let beat_ms = |b: u16| grid.get((b as usize).saturating_sub(1)).map(|g| g.time_ms as f64).unwrap_or(0.0);
        println!("track {id}: {path}  ({} grid beats)", grid.len());
        show("rekordbox", &theirs, &beat_ms);
        show("OpenDeck (bar phase guessed)", &ours, &beat_ms);
        show("OpenDeck (rekordbox downbeats)", &ours_db, &beat_ms);
    }
    Ok(())
}
