//! How close is OpenDeck's phrase analysis (song structure, PSSI) to
//! rekordbox's?  For tracks on a linked player that have a `.EXT` file with a
//! PSSI section, fetch the audio, the `.EXT` and the `.DAT` (rekordbox's beat
//! grid) over NFS, run OpenDeck's own analysis (grid + waveform, as the media
//! server does) and the phrase detector, and compare with rekordbox's phrases.
//!
//!   cargo run --release -p opendeck-app --example phrase_vs_rekordbox -- <player-ip> [cache-dir] [max]
//!
//! The fetched sections and our analysis are cached in `cache-dir` (default
//! `target/phrase-cache`), so re-running after a detector change takes
//! seconds and needs no player.  `-` as the ip runs from the cache only.
//!
//! Reports: mood agreement; phrase boundary precision / recall within ±1 bar
//! (compared in time, since the two grids number beats differently); kind
//! agreement on matched phrases; end-beat error.  `PHRASE_VERBOSE=1` prints
//! each track's two phrase lists.
use opendeck_analysis::phrase::{self, Mood, SongStructure};
use opendeck_analysis::{BeatAnalyzerImpl, WaveformBuilder, WaveformCache};
use opendeck_types::{BeatAnalyzer, Decoder};
use std::path::{Path, PathBuf};

fn tag<'a>(file: &'a [u8], fourcc: &[u8; 4]) -> Option<&'a [u8]> {
    let hl = u32::from_be_bytes(file.get(4..8)?.try_into().ok()?) as usize;
    let mut o = hl;
    while o + 12 <= file.len() {
        let len = u32::from_be_bytes(file[o + 8..o + 12].try_into().ok()?) as usize;
        if &file[o..o + 4] == fourcc { return file.get(o..o + len); }
        if len == 0 { break; }
        o += len;
    }
    None
}

fn tags(file: &[u8]) -> Vec<String> {
    let mut v = Vec::new();
    let Some(hl) = file.get(4..8).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize) else { return v };
    let mut o = hl;
    while o + 12 <= file.len() {
        let len = u32::from_be_bytes(file[o + 8..o + 12].try_into().unwrap()) as usize;
        v.push(String::from_utf8_lossy(&file[o..o + 4]).into_owned());
        if len == 0 { break; }
        o += len;
    }
    v
}

/// Our side of a track, as the media server would compute it.
struct Ours { sr: u32, frames: u64, beats: Vec<(u8, f32, u32)>, wave: WaveformCache }

fn analyse(bytes: Vec<u8>, ext: Option<&str>) -> anyhow::Result<Ours> {
    let mut d = opendeck_decode::symphonia_decoder::SymphoniaDecoder::open_bytes(bytes, ext)?;
    let (sr, ch) = (d.sample_rate(), d.channels().max(1) as usize);
    let lead_cap = 121 * sr as usize * 2;
    let mut lead = Vec::new();
    let mut wb = WaveformBuilder::new(sr);
    let mut frames = 0u64;
    let mut buf = vec![0f32; 4096 * ch];
    let mut st = Vec::new();
    loop {
        let n = d.decode(&mut buf)?;
        if n == 0 { break; }
        frames += n as u64;
        st.clear();
        for f in buf[..n * ch].chunks(ch) { st.push(f[0]); st.push(f[1.min(ch - 1)]); }
        wb.push(&st);
        if lead.len() < lead_cap { let k = (lead_cap - lead.len()).min(st.len()); lead.extend_from_slice(&st[..k]); }
    }
    let wave = wb.finish();
    let mut ba = BeatAnalyzerImpl::new(sr);
    ba.push(&lead, sr);
    let beats = match ba.beat_grid() {
        Some(g) => opendeck_mediaserver::grid_beats(g.anchor_sample, g.bpm, g.downbeat_offset, sr, frames),
        None => Vec::new(),
    };
    Ok(Ours { sr, frames, beats, wave })
}

fn put_ours(o: &Ours) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&o.sr.to_le_bytes());
    b.extend_from_slice(&o.frames.to_le_bytes());
    b.extend_from_slice(&(o.wave.hop_size as u32).to_le_bytes());
    b.extend_from_slice(&(o.beats.len() as u32).to_le_bytes());
    for &(bib, bpm, ms) in &o.beats { b.push(bib); b.extend_from_slice(&bpm.to_le_bytes()); b.extend_from_slice(&ms.to_le_bytes()); }
    b.extend_from_slice(&(o.wave.columns.len() as u32).to_le_bytes());
    for c in &o.wave.columns { b.extend_from_slice(c); }
    b
}

fn get_ours(b: &[u8]) -> Option<Ours> {
    let mut p = 0usize;
    let mut take = |n: usize| { let s = b.get(p..p + n); p += n; s };
    let u32_ = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap());
    let sr = u32_(take(4)?);
    let frames = u64::from_le_bytes(take(8)?.try_into().ok()?);
    let hop = u32_(take(4)?) as usize;
    let nb = u32_(take(4)?) as usize;
    let mut beats = Vec::with_capacity(nb);
    for _ in 0..nb { let s = take(9)?; beats.push((s[0], f32::from_le_bytes(s[1..5].try_into().ok()?), u32_(&s[5..9]))); }
    let nc = u32_(take(4)?) as usize;
    let cols = take(nc * 4)?;
    let columns = cols.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
    Some(Ours { sr, frames, beats, wave: WaveformCache { columns, sample_rate: sr, hop_size: hop } })
}

/// One cached track: rekordbox's PSSI section, its beat times (ms), ours.
struct Case { id: u32, name: String, pssi: Vec<u8>, rb_beats: Vec<(u8, u32)>, ours: Ours }

fn fetch(ip: &str, dir: &Path, max: usize) -> anyhow::Result<()> {
    use opendeck_nfs::Nfs;
    let mut nfs = Nfs::connect(ip.parse()?)?;
    let root = nfs.mount_usb()?;
    let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb")?;
    let pdb = nfs.read_file(&fh, size)?;
    let exp = opendeck_rekordbox::read_export_from(&mut std::io::Cursor::new(pdb), "nfs://player".into())?;
    let mut n = 0;
    for t in &exp.tracks {
        if n >= max { break; }
        if t.analyze_rel.is_empty() { continue; }
        let base = dir.join(t.id.to_string());
        if base.with_extension("ours").exists() { n += 1; continue; }
        let ext_p = t.analyze_rel.trim_start_matches('/').replace(".DAT", ".EXT");
        let Ok((efh, esz)) = nfs.lookup_path(&root, &ext_p) else { continue };
        let ext = nfs.read_file(&efh, esz)?;
        // PHRASE_ALL=1 keeps tracks without phrase analysis too (ours only).
        let pssi = match tag(&ext, b"PSSI") {
            Some(p) => p,
            None if std::env::var("PHRASE_ALL").is_ok() => &[][..],
            None => { eprintln!("{:>4} no PSSI (tags {:?})", t.id, tags(&ext)); continue }
        };
        let (dfh, dsz) = nfs.lookup_path(&root, t.analyze_rel.trim_start_matches('/'))?;
        let dat = nfs.read_file(&dfh, dsz)?;
        let (afh, asz) = nfs.lookup_path(&root, t.rel_path.trim_start_matches('/'))?;
        let audio = nfs.read_file(&afh, asz)?;
        let ours = match analyse(audio, t.rel_path.rsplit('.').next()) { Ok(o) => o, Err(e) => { eprintln!("{:>4} decode: {e:#}", t.id); continue } };
        std::fs::write(base.with_extension("pssi"), pssi)?;
        std::fs::write(base.with_extension("dat"), &dat)?;
        std::fs::write(base.with_extension("ours"), put_ours(&ours))?;
        std::fs::write(base.with_extension("name"), format!("{} - {}", t.artist, t.title))?;
        n += 1;
        eprintln!("{:>4} fetched {} - {}", t.id, t.artist, t.title);
    }
    Ok(())
}

fn load(dir: &Path) -> anyhow::Result<Vec<Case>> {
    let mut ids: Vec<u32> = std::fs::read_dir(dir)?.filter_map(|e| {
        let p = e.ok()?.path();
        (p.extension()? == "ours").then(|| p.file_stem()?.to_str()?.parse().ok())?
    }).collect();
    ids.sort();
    let mut out = Vec::new();
    for id in ids {
        let base = dir.join(id.to_string());
        let dat = std::fs::read(base.with_extension("dat"))?;
        let rb = opendeck_rekordbox::read_anlz_from(&mut std::io::Cursor::new(dat))?;
        let Some(ours) = get_ours(&std::fs::read(base.with_extension("ours"))?) else { continue };
        out.push(Case {
            id, name: std::fs::read_to_string(base.with_extension("name")).unwrap_or_default(),
            pssi: std::fs::read(base.with_extension("pssi"))?,
            rb_beats: rb.beats.iter().map(|b| (b.beat_in_bar, b.time_ms)).collect(), ours,
        });
    }
    Ok(out)
}

/// A phrase as a time span and a coarse kind, for comparison across grids.
#[derive(Clone, Copy, Debug)]
struct Span { start_ms: f64, kind: phrase::Coarse }

fn spans(s: &SongStructure, beat_ms: &dyn Fn(u16) -> f64) -> (Vec<Span>, f64) {
    let v = s.phrases.iter().map(|p| Span { start_ms: beat_ms(p.beat), kind: p.coarse(s.mood) }).collect();
    (v, beat_ms(s.end_beat))
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let ip = a.next().expect("usage: phrase_vs_rekordbox <player-ip|-> [cache-dir] [max]");
    let dir = PathBuf::from(a.next().unwrap_or_else(|| "target/phrase-cache".into()));
    let max: usize = a.next().map(|m| m.parse()).transpose()?.unwrap_or(1000);
    std::fs::create_dir_all(&dir)?;
    if ip != "-" { fetch(&ip, &dir, max)?; }
    let cases = load(&dir)?;
    let verbose = std::env::var("PHRASE_VERBOSE").is_ok();

    let (mut n, mut mood_ok, mut tp, mut n_rb, mut n_ours, mut kind_ok, mut kind_n) = (0, 0, 0, 0, 0, 0, 0);
    let mut end_err = Vec::new();
    let mut mood_conf = [[0u32; 3]; 3];
    let mut kind_conf = std::collections::BTreeMap::<(String, String), u32>::new();
    let (mut our_moods, mut our_lens) = (std::collections::BTreeMap::new(), std::collections::BTreeMap::new());
    for c in &cases {
        let ours = phrase::detect(&c.ours.beats, &c.ours.wave, c.ours.frames, c.ours.sr);
        if c.pssi.is_empty() {
            // No rekordbox answer: show ours, and tally what we produce.
            *our_moods.entry(format!("{:?}", ours.mood)).or_insert(0u32) += 1;
            for w in ours.phrases.windows(2) { *our_lens.entry((w[1].beat - w[0].beat) / 4).or_insert(0u32) += 1; }
            if verbose {
                let ms = |b: u16| c.ours.beats.get(b.max(1) as usize - 1).map(|x| x.2 as f64 / 1000.0).unwrap_or(0.0);
                println!("{:>4} {}  ours {:?}, end {:.0}s:\n      {}", c.id, c.name, ours.mood, ms(ours.end_beat),
                    ours.phrases.iter().map(|p| format!("{:.0}:{}", ms(p.beat), p.label(ours.mood))).collect::<Vec<_>>().join(" "));
            }
            continue;
        }
        let rb = match phrase::parse_pssi(&c.pssi) { Ok(s) => s, Err(e) => { eprintln!("{:>4} parse: {e}", c.id); continue } };
        if rb.phrases.is_empty() { continue; }
        let rb_ms = |b: u16| { let i = (b.max(1) as usize - 1).min(c.rb_beats.len().saturating_sub(1)); c.rb_beats.get(i).map(|x| x.1 as f64).unwrap_or(0.0) };
        let our_ms = |b: u16| { let i = (b.max(1) as usize - 1).min(c.ours.beats.len().saturating_sub(1)); c.ours.beats.get(i).map(|x| x.2 as f64).unwrap_or(0.0) };
        let (rs, rend) = spans(&rb, &rb_ms);
        let (os, oend) = spans(&ours, &our_ms);
        n += 1;
        let mi = |m: Mood| m as usize - 1;
        mood_conf[mi(rb.mood)][mi(ours.mood)] += 1;
        if rb.mood == ours.mood { mood_ok += 1; }
        let bar_ms = 4.0 * 60_000.0 / c.rb_beats.first().map(|_| {
            let k = c.rb_beats.len().min(64);
            if k < 2 { 120.0 } else { 60_000.0 * (k - 1) as f64 / (c.rb_beats[k - 1].1 - c.rb_beats[0].1) as f64 }
        }).unwrap_or(120.0);
        // Boundaries (excluding the first phrase start, always at the top).
        n_rb += rs.len().saturating_sub(1);
        n_ours += os.len().saturating_sub(1);
        let mut used = vec![false; os.len()];
        for r in rs.iter().skip(1) {
            if let Some((j, _)) = os.iter().enumerate().skip(1).filter(|(j, o)| !used[*j] && (o.start_ms - r.start_ms).abs() <= bar_ms * 1.01)
                .min_by(|a, b| (a.1.start_ms - r.start_ms).abs().partial_cmp(&(b.1.start_ms - r.start_ms).abs()).unwrap()) {
                used[j] = true; tp += 1;
            }
        }
        // Kinds: at each rekordbox phrase's midpoint, which kind do we say?
        for (i, r) in rs.iter().enumerate() {
            let end = rs.get(i + 1).map(|x| x.start_ms).unwrap_or(rend);
            let mid = (r.start_ms + end) / 2.0;
            if let Some(o) = os.iter().rev().find(|o| o.start_ms <= mid).or(os.first()) {
                kind_n += 1;
                if o.kind == r.kind { kind_ok += 1; }
                *kind_conf.entry((format!("{:?}", r.kind), format!("{:?}", o.kind))).or_default() += 1;
            }
        }
        end_err.push((oend - rend) / bar_ms);
        if verbose {
            println!("{:>4} {}  rb {:?} end {:.0}s / ours {:?} end {:.0}s", c.id, c.name, rb.mood, rend / 1000.0, ours.mood, oend / 1000.0);
            let fmt = |v: &[Span]| v.iter().map(|s| format!("{:.0}:{:?}", s.start_ms / 1000.0, s.kind)).collect::<Vec<_>>().join(" ");
            println!("      rb   {}\n      ours {}", fmt(&rs), fmt(&os));
        }
    }
    let p = tp as f64 / n_ours.max(1) as f64;
    let r = tp as f64 / n_rb.max(1) as f64;
    end_err.sort_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap());
    let med = end_err.get(end_err.len() / 2).copied().unwrap_or(0.0);
    let within = end_err.iter().filter(|e| e.abs() <= 1.01).count();
    if !our_moods.is_empty() {
        println!("without rekordbox phrases ({} tracks): our moods {our_moods:?}; phrase lengths (bars: count) {our_lens:?}", our_moods.values().sum::<u32>());
    }
    println!("tracks {n}");
    println!("mood agreement {mood_ok}/{n} ({:.0}%)   rows rekordbox high/mid/low, cols ours: {:?}", 100.0 * mood_ok as f64 / n.max(1) as f64, mood_conf);
    println!("boundaries ±1 bar: precision {:.2} recall {:.2} F1 {:.2}  (rb {n_rb}, ours {n_ours}, matched {tp})", p, r, 2.0 * p * r / (p + r).max(1e-9));
    println!("kind agreement (coarse) {kind_ok}/{kind_n} ({:.0}%)", 100.0 * kind_ok as f64 / kind_n.max(1) as f64);
    println!("end beat: median |err| {:.1} bars, within ±1 bar {within}/{}", med.abs(), end_err.len());
    if std::env::var("PHRASE_CONF").is_ok() { for ((r, o), k) in &kind_conf { println!("  rb {r:>8} ours {o:>8}: {k}"); } }
    Ok(())
}
