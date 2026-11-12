//! Compare OpenDeck's phrases with rekordbox's for tracks in a rekordbox
//! collection served over LINK (rekordbox on a computer, LINK on): fetch
//! rekordbox's PSSI, beat grid and audio, run OpenDeck's detector with
//! rekordbox's own grid, and compare.
//!
//!   cargo run --release -p opendeck-app --example phrase_vs_rekordbox_link -- <rekordbox-ip> <track-id>...
//!   cargo run --release -p opendeck-app --example phrase_vs_rekordbox_link -- <rekordbox-ip> all
//!
//! `all` walks the whole collection, skips tracks without phrase analysis,
//! and prints totals: mood agreement, phrase-start precision / recall within
//! one bar, coarse kind agreement — with the bar phase guessed and with
//! rekordbox's downbeats.  Fetched tracks are cached in `PHRASE_LINK_CACHE`
//! (default target/phrase-link-cache), so tuning reruns are fast; `-` as the
//! IP reruns from the cache alone.  `PHRASE_VERBOSE=1` lists phrases.
use opendeck_analysis::phrase::{detect_cfg, detect_with, parse_pssi, Coarse, Mood, PhraseConfig, SongStructure};
use opendeck_analysis::{WaveformBuilder, WaveformCache};
use opendeck_dbserver::{parse_beat_grid, Client, Slot};
use opendeck_nfs::Nfs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

/// What one track needs, cached: rekordbox's PSSI section and grid, our
/// waveform columns.
struct Case { id: u32, name: String, pssi: Vec<u8>, beats: Vec<(u8, f32, u32)>, wave: WaveformCache, frames: u64 }

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

fn put(c: &Case) -> Vec<u8> {
    let mut b = Vec::new();
    let w32 = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
    w32(&mut b, c.id);
    w32(&mut b, c.name.len() as u32); b.extend_from_slice(c.name.as_bytes());
    w32(&mut b, c.pssi.len() as u32); b.extend_from_slice(&c.pssi);
    w32(&mut b, c.beats.len() as u32);
    for &(bib, bpm, ms) in &c.beats { b.push(bib); b.extend_from_slice(&bpm.to_le_bytes()); w32(&mut b, ms); }
    w32(&mut b, c.wave.sample_rate); w32(&mut b, c.wave.hop_size as u32);
    b.extend_from_slice(&c.frames.to_le_bytes());
    w32(&mut b, c.wave.columns.len() as u32);
    for col in &c.wave.columns { b.extend_from_slice(col); }
    b
}

struct R<'a>(&'a [u8], usize);
impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> { let s = self.0.get(self.1..self.1 + n)?; self.1 += n; Some(s) }
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
}

fn get(b: &[u8]) -> Option<Case> {
    let mut r = R(b, 0);
    let id = r.u32()?;
    let n = r.u32()? as usize; let name = String::from_utf8(r.take(n)?.to_vec()).ok()?;
    let n = r.u32()? as usize; let pssi = r.take(n)?.to_vec();
    let n = r.u32()? as usize;
    let mut beats = Vec::with_capacity(n);
    for _ in 0..n {
        let e = r.take(9)?;
        beats.push((e[0], f32::from_le_bytes(e[1..5].try_into().ok()?), u32::from_le_bytes(e[5..9].try_into().ok()?)));
    }
    let sr = r.u32()?; let hop = r.u32()? as usize;
    let frames = u64::from_le_bytes(r.take(8)?.try_into().ok()?);
    let n = r.u32()? as usize;
    let cols = r.take(n * 4)?.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
    Some(Case { id, name, pssi, beats, wave: WaveformCache { columns: cols, sample_rate: sr, hop_size: hop }, frames })
}

fn fetch(ip: Ipv4Addr, ids: &[u32], dir: &Path) -> anyhow::Result<()> {
    let mut db = Client::discover(ip, 3)?;
    let ids: Vec<(u32, String)> = if ids.is_empty() {
        db.all_tracks(Slot::Collection, 0)?.into_iter().map(|t| (t.id, format!("{} - {}", t.label2, t.label))).collect()
    } else { ids.iter().map(|&i| (i, String::new())).collect() };
    let mut nfs: Option<(Nfs, opendeck_nfs::Fh)> = None;
    for (id, name) in ids {
        let file = dir.join(format!("{id}.case"));
        if file.exists() { continue; }
        let blob = db.anlz_tag(Slot::Collection, id, b"PSSI", b"EXT").unwrap_or_default();
        if blob.len() < 8 { continue; }   // no phrase analysis in rekordbox
        let grid = parse_beat_grid(&db.beat_grid(Slot::Collection, id)?);
        let Some(path) = db.file_path(Slot::Collection, id)? else { continue };
        if nfs.is_none() {
            let mut n = Nfs::connect_at(ip, opendeck_nfs::PORTMAP_REKORDBOX)?;
            let export = n.exports()?.into_iter().next().ok_or_else(|| anyhow::anyhow!("no export"))?;
            let root = n.mount(&export)?;
            nfs = Some((n, root));
        }
        let (n, root) = nfs.as_mut().unwrap();
        let (fh, size) = n.lookup_path(root, &path)?;
        let (pcm, sr) = match decode(n.read_file(&fh, size)?, path.rsplit('.').next()) { Ok(x) => x, Err(e) => { eprintln!("{id}: decode: {e}"); continue } };
        let mut wb = WaveformBuilder::new(sr);
        wb.push(&pcm);
        let c = Case { id, name: if name.is_empty() { path.clone() } else { name }, pssi: blob[4..].to_vec(),
                       beats: grid.iter().map(|b| (b.beat_in_bar, b.bpm, b.time_ms)).collect(), wave: wb.finish(), frames: (pcm.len() / 2) as u64 };
        std::fs::write(&file, put(&c))?;
        eprintln!("fetched {id} {}", c.name);
    }
    Ok(())
}

#[derive(Default)]
struct Tally { n: u32, mood_ok: u32, n_rb: u32, n_ours: u32, tp: u32, kind_ok: u32, kind_n: u32, mood_conf: [[u32; 3]; 3] }

impl Tally {
    /// Both structures use the same (rekordbox) grid, so compare in beats:
    /// a start matches within one bar (4 beats).
    fn add(&mut self, rb: &SongStructure, ours: &SongStructure) {
        self.n += 1;
        self.mood_conf[rb.mood as usize - 1][ours.mood as usize - 1] += 1;
        if rb.mood == ours.mood { self.mood_ok += 1; }
        let rs: Vec<u16> = rb.phrases.iter().map(|p| p.beat).collect();
        let os: Vec<u16> = ours.phrases.iter().map(|p| p.beat).collect();
        self.n_rb += rs.len().saturating_sub(1) as u32;
        self.n_ours += os.len().saturating_sub(1) as u32;
        let mut used = vec![false; os.len()];
        for &r in rs.iter().skip(1) {
            let best = os.iter().enumerate().skip(1).filter(|(j, &o)| !used[*j] && (o as i32 - r as i32).abs() <= 4)
                .min_by_key(|(_, &o)| (o as i32 - r as i32).abs());
            if let Some((j, _)) = best { used[j] = true; self.tp += 1; }
        }
        for (i, p) in rb.phrases.iter().enumerate() {
            let end = rb.phrases.get(i + 1).map(|q| q.beat).unwrap_or(rb.end_beat);
            let mid = (p.beat + end) / 2;
            let theirs = p.coarse(rb.mood);
            let mine: Coarse = ours.phrases.iter().rev().find(|q| q.beat <= mid).or(ours.phrases.first()).map(|q| q.coarse(ours.mood)).unwrap_or(Coarse::Unknown);
            self.kind_n += 1;
            if mine == theirs { self.kind_ok += 1; }
        }
    }
    fn print(&self, label: &str) {
        let p = self.tp as f64 / self.n_ours.max(1) as f64;
        let r = self.tp as f64 / self.n_rb.max(1) as f64;
        println!("{label}: tracks {}  mood {}/{} {:?}  starts ±1 bar P {:.2} R {:.2} F1 {:.2} (rb {}, ours {})  kind {:.0}%",
                 self.n, self.mood_ok, self.n, self.mood_conf, p, r, 2.0 * p * r / (p + r).max(1e-9), self.n_rb, self.n_ours,
                 100.0 * self.kind_ok as f64 / self.kind_n.max(1) as f64);
    }
}

fn score(cases: &[(Case, SongStructure)], cfg: &PhraseConfig) -> (Tally, Vec<(f64, f64, Mood, Mood)>) {
    let mut t = Tally::default();
    let mut feats = Vec::new();
    for (c, rb) in cases {
        let (ours, f) = detect_cfg(&c.beats, &c.wave, c.frames, c.wave.sample_rate, true, cfg);
        t.add(rb, &ours);
        feats.push((f.driven, f.bass_med, rb.mood, ours.mood));
    }
    (t, feats)
}

fn lens(s: &SongStructure, h: &mut std::collections::BTreeMap<u16, u32>) {
    for w in s.phrases.windows(2) { *h.entry((w[1].beat - w[0].beat) / 4).or_insert(0) += 1; }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let ip = a.next().expect("usage: phrase_vs_rekordbox_link <ip|-> <track-id>...|all");
    let rest: Vec<String> = a.collect();
    let dir = PathBuf::from(std::env::var("PHRASE_LINK_CACHE").unwrap_or_else(|_| "target/phrase-link-cache".into()));
    std::fs::create_dir_all(&dir)?;
    let ids: Vec<u32> = rest.iter().filter(|s| *s != "all").map(|s| s.parse()).collect::<Result<_, _>>()?;
    if ip != "-" { fetch(ip.parse()?, &ids, &dir)?; }
    let verbose = std::env::var("PHRASE_VERBOSE").is_ok() || (!ids.is_empty() && ids.len() <= 3);
    let (mut guessed, mut known) = (Tally::default(), Tally::default());
    let mut files: Vec<_> = std::fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "case")).collect();
    files.sort();
    for f in files {
        let Some(c) = get(&std::fs::read(&f)?) else { continue };
        if !ids.is_empty() && !ids.contains(&c.id) { continue; }
        let Ok(rb) = parse_pssi(&c.pssi) else { continue };
        if rb.phrases.is_empty() { continue; }
        let g = detect_with(&c.beats, &c.wave, c.frames, c.wave.sample_rate, false);
        let k = detect_with(&c.beats, &c.wave, c.frames, c.wave.sample_rate, true);
        guessed.add(&rb, &g);
        known.add(&rb, &k);
        if verbose {
            let fmt = |s: &SongStructure| s.phrases.iter().map(|p| format!("{}:{}", p.beat, p.label(s.mood))).collect::<Vec<_>>().join(" ");
            println!("{} {}\n  rekordbox {:?}: {}\n  ours      {:?}: {}", c.id, c.name, rb.mood, fmt(&rb), k.mood, fmt(&k));
        }
    }
    guessed.print("bar phase guessed     ");
    known.print("rekordbox downbeats   ");

    // Calibration: cases in memory, mood features, phrase lengths, a sweep.
    let mut cases = Vec::new();
    for f in std::fs::read_dir(&dir)?.flatten().map(|e| e.path()) {
        let Some(c) = std::fs::read(&f).ok().and_then(|b| get(&b)) else { continue };
        if !ids.is_empty() && !ids.contains(&c.id) { continue; }
        if let Ok(rb) = parse_pssi(&c.pssi) { if !rb.phrases.is_empty() { cases.push((c, rb)); } }
    }
    let (_, feats) = score(&cases, &PhraseConfig::default());
    if std::env::var("PHRASE_FEATS").is_ok() {
        for ((c, _), (d, b, rm, om)) in cases.iter().zip(&feats) { println!("  driven {d:.2} bass {b:>5.1}  rb {rm:?} ours {om:?}  {}", c.name); }
    }
    let (mut hr, mut ho) = (std::collections::BTreeMap::new(), std::collections::BTreeMap::new());
    for (c, rb) in &cases { lens(rb, &mut hr); lens(&detect_with(&c.beats, &c.wave, c.frames, c.wave.sample_rate, true), &mut ho); }
    println!("phrase lengths in bars (length: count)\n  rekordbox {hr:?}\n  ours      {ho:?}");
    if std::env::var("PHRASE_SWEEP").is_ok() {
        let mut best: Vec<(f64, String)> = Vec::new();
        for &max_bars in &[8usize, 12, 16] { for &split_bars in &[4usize, 8] { for &nov_mult in &[1.5, 2.0, 2.5] {
            for &high_driven in &[0.2, 0.35, 0.45, 0.55] { for &high_bass in &[40.0, 60.0, 75.0, 90.0] {
                let cfg = PhraseConfig { max_bars, split_bars, nov_mult, high_driven, high_bass, ..Default::default() };
                let (t, _) = score(&cases, &cfg);
                let p = t.tp as f64 / t.n_ours.max(1) as f64; let r = t.tp as f64 / t.n_rb.max(1) as f64;
                let f1 = 2.0 * p * r / (p + r).max(1e-9);
                let (m, k) = (t.mood_ok as f64 / t.n.max(1) as f64, t.kind_ok as f64 / t.kind_n.max(1) as f64);
                best.push((f1 + m + k, format!("F1 {f1:.2} mood {m:.2} kind {k:.2}  {cfg:?}")));
            } }
        } } }
        best.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (_, l) in best.iter().take(8) { println!("  {l}"); }
    }
    Ok(())
}
