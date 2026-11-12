//! How often does OpenDeck's key detector agree with rekordbox?  For tracks on
//! a linked player, fetch the audio over NFS (read-only), decode, detect the
//! key, and compare with the key rekordbox stored in export.pdb.
//!
//!   cargo run --release -p opendeck-app --example key_vs_rekordbox -- <player-ip> [tracks]
//!
//! KEY_CACHE=<dir> keeps the fetched audio files there so a rerun skips NFS;
//! KEY_DUMP=<file> writes every track's 120-bin fine chroma per config.
//! "built-in" is the shipped detector; its profiles were fitted on the
//! 226-track stick this was developed against, so on that stick it is
//! in-sample: "fitted-cv" is the honest out-of-sample number.
//! Several front-end configs run on every decode; each is scored with the
//! built-in profiles and with profiles fitted on the other half of the tracks
//! (2-fold cross-validation, so the fitted numbers are out-of-sample).
//! Categories: exact; relative (same Camelot number, other ring); fifth (same
//! ring, ±1 number); parallel (same tonic, other mode); other.
use opendeck_analysis::key::{self, chroma_from_fine, key_from_chroma, Key, KeyConfig, KeyDetector, Profiles, DEFAULT_MINOR_BIAS, FINE};
use opendeck_nfs::Nfs;
use std::net::Ipv4Addr;
use std::time::Instant;

fn decode(bytes: Vec<u8>, ext: Option<&str>) -> anyhow::Result<(Vec<f32>, u32)> {
    use opendeck_types::Decoder;
    let mut d = opendeck_decode::symphonia_decoder::SymphoniaDecoder::open_bytes(bytes, ext)?;
    let (sr, ch) = (d.sample_rate(), d.channels() as usize);
    let mut out = Vec::new();
    let mut buf = vec![0f32; 4096 * ch];
    loop {
        match d.decode(&mut buf)? {
            0 => break,
            n => for f in buf[..n * ch].chunks(ch) { out.push(f[0]); out.push(if ch > 1 { f[1] } else { f[0] }); },
        }
    }
    Ok((out, sr))
}

/// Front-end variants run on every decode (the default first).  An earlier
/// spectral-peak front end topped out at 53 % (fitted profiles); this one
/// whitens a log-frequency spectrum instead.
fn configs() -> Vec<(&'static str, KeyConfig)> {
    let d = KeyConfig::default();
    vec![
        ("default (27.5-5000 Hz, whiten ±30, mag^0.5)", d),
        ("27.5-2000 Hz", KeyConfig { max_hz: 2000.0, ..d }),
        ("55-5000 Hz", KeyConfig { min_hz: 55.0, ..d }),
        ("no whitening", KeyConfig { whiten_cells: 0, ..d }),
        ("whiten ±60", KeyConfig { whiten_cells: 60, ..d }),
        ("mag^1", KeyConfig { exponent: 1.0, ..d }),
        ("no frame norm", KeyConfig { frame_norm: false, ..d }),
        ("0.74 s frames", KeyConfig { frame_secs: 0.74, ..d }),
    ]
}

#[derive(Default, Clone, Copy)]
struct Tally { exact: u32, relative: u32, fifth: u32, parallel: u32, other: u32 }
impl Tally {
    fn add(&mut self, ours: Key, rb: Key) {
        let ((on, oa), (rn, ra)) = (ours.camelot(), rb.camelot());
        if ours == rb { self.exact += 1 }
        else if on == rn { self.relative += 1 }
        else if oa == ra && ((on % 12) + 1 == rn || (rn % 12) + 1 == on) { self.fifth += 1 }
        else if ours.tonic == rb.tonic { self.parallel += 1 }
        else { self.other += 1 }
    }
    fn line(&self) -> String {
        let n = (self.exact + self.relative + self.fifth + self.parallel + self.other).max(1) as f64;
        let p = |x: u32| 100.0 * x as f64 / n;
        format!("exact {:5.1}%  relative {:5.1}%  fifth {:5.1}%  parallel {:5.1}%  other {:5.1}%",
            p(self.exact), p(self.relative), p(self.fifth), p(self.parallel), p(self.other))
    }
}

/// Mean chroma per mode, rotated to the tonic, each track's normalised to sum 1.
fn fit(rows: &[([f32; 12], Key)]) -> Profiles {
    let (mut maj, mut min) = ([0f32; 12], [0f32; 12]);
    for (c, k) in rows {
        let s: f32 = c.iter().sum::<f32>().max(1e-9);
        let p = if k.minor { &mut min } else { &mut maj };
        for i in 0..12 { p[i] += c[(i + k.tonic as usize) % 12] / s; }
    }
    (maj, min)
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let ip: Ipv4Addr = a.next().expect("usage: key_vs_rekordbox <ip> [tracks]").parse()?;
    let want: usize = a.next().map(|m| m.parse()).transpose()?.unwrap_or(usize::MAX);
    let cache = std::env::var("KEY_CACHE").ok().map(std::path::PathBuf::from);
    if let Some(c) = &cache { std::fs::create_dir_all(c)?; }
    let mut nfs = Nfs::connect(ip)?;
    let root = nfs.mount_usb()?;
    let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb")?;
    let pdb = nfs.read_file(&fh, size)?;
    let exp = opendeck_rekordbox::read_export_from(&mut std::io::Cursor::new(pdb), "nfs://player".into())?;
    let mut names: Vec<&str> = exp.tracks.iter().map(|t| t.key.as_str()).collect();
    names.sort(); names.dedup();
    println!("rekordbox key names on this stick: {names:?}");
    for n in &names { if !n.is_empty() && Key::parse(n).map(Key::name) != Some(n) { println!("  !! {n} doesn't round-trip"); } }

    // Fetch sequentially (one NFS client), analyse on a pool.
    let cfgs = configs();
    let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, Vec<u8>, Option<String>)>(8);
    let tracks: Vec<_> = exp.tracks.iter().filter(|t| Key::parse(&t.key).is_some()).take(want).cloned().collect();
    type Row = (usize, Vec<[f64; FINE]>, f64, f64, f64); // idx, fine per config, decode s, detect s, audio s
    let results = std::sync::Mutex::new(Vec::<Row>::new());
    let rx = std::sync::Mutex::new(rx);
    std::thread::scope(|s| -> anyhow::Result<()> {
        for _ in 0..std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(12) {
            s.spawn(|| loop {
                let Ok((i, bytes, ext)) = rx.lock().unwrap().recv() else { break };
                let t0 = Instant::now();
                let Ok((pcm, sr)) = decode(bytes, ext.as_deref()) else { eprintln!("decode failed: track {i}"); continue };
                let dec_s = t0.elapsed().as_secs_f64();
                let mut fines = Vec::new();
                let mut det_s = 0.0;
                for (k, (_, c)) in cfgs.iter().enumerate() {
                    let t1 = Instant::now();
                    let mut d = KeyDetector::with_config(sr, *c);
                    d.push(&pcm, 2);
                    let _ = d.finish();
                    if k == 0 { det_s = t1.elapsed().as_secs_f64(); }
                    fines.push(d.fine_chroma());
                }
                results.lock().unwrap().push((i, fines, dec_s, det_s, pcm.len() as f64 / 2.0 / sr as f64));
            });
        }
        for (i, t) in tracks.iter().enumerate() {
            let ext = t.rel_path.rsplit('.').next().map(str::to_string);
            let cached = cache.as_ref().map(|c| c.join(format!("{}.{}", t.id, ext.as_deref().unwrap_or("bin"))));
            let bytes = match cached.as_ref().and_then(|p| std::fs::read(p).ok()) {
                Some(b) => b,
                None => {
                    let Ok((afh, asz)) = nfs.lookup_path(&root, t.rel_path.trim_start_matches('/')) else { eprintln!("missing: {}", t.rel_path); continue };
                    let b = nfs.read_file(&afh, asz)?;
                    if let Some(p) = &cached { std::fs::write(p, &b)?; }
                    b
                }
            };
            eprintln!("{:>4}/{} {:>4} {:<4} {} - {}", i + 1, tracks.len(), t.id, t.key, t.artist, t.title);
            tx.send((i, bytes, ext))?;
        }
        drop(tx);
        Ok(())
    })?;
    let mut rows = results.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    let n = rows.len();
    let rb: Vec<Key> = rows.iter().map(|r| Key::parse(&tracks[r.0].key).unwrap()).collect();
    let (dec, det, secs): (f64, f64, f64) = rows.iter().fold((0.0, 0.0, 0.0), |acc, r| (acc.0 + r.2, acc.1 + r.3, acc.2 + r.4));
    println!("\nN = {n} tracks ({:.0} min of audio)", secs / 60.0);
    println!("speed: decode {:.0} ms/track, detect (default config) {:.0} ms/track ({:.0}x realtime)",
        1e3 * dec / n as f64, 1e3 * det / n as f64, secs / det.max(1e-9));
    let mut modes = [0usize; 2];
    for k in &rb { modes[k.minor as usize] += 1; }
    println!("rekordbox: {} major, {} minor\n", modes[0], modes[1]);
    // KEY_DUMP=<file>: one line per track and config (rekordbox key, config index, 120 fine bins).
    if let Ok(f) = std::env::var("KEY_DUMP") {
        let mut s = String::new();
        for (r, k) in rows.iter().zip(&rb) { for (ci, fine) in r.1.iter().enumerate() {
            s += &format!("{} {ci} {}\n", k.name(), fine.iter().map(|v| format!("{v:.5}")).collect::<Vec<_>>().join(" "));
        } }
        std::fs::write(f, s)?;
    }

    for (ci, (cname, _)) in cfgs.iter().enumerate() {
        let chromas: Vec<[f32; 12]> = rows.iter().map(|r| chroma_from_fine(&r.1[ci])).collect();
        let score = |pick: &dyn Fn(usize) -> Key| { let mut t = Tally::default(); for i in 0..n { t.add(pick(i), rb[i]); } t };
        println!("config {cname}:");
        let b = DEFAULT_MINOR_BIAS;
        println!("  built-in   {}  (profiles fitted on this stick: in-sample)", score(&|i| key_from_chroma(&chromas[i], &key::DEFAULT_PROFILES, b)).line());
        for (pname, p) in [("krumhansl", key::KRUMHANSL), ("temperley", key::TEMPERLEY)] {
            println!("  {pname:<10} {}", score(&|i| key_from_chroma(&chromas[i], &p, 0.0)).line());
            println!("  {:<10} {}", format!("  +{b}"), score(&|i| key_from_chroma(&chromas[i], &p, b)).line());
        }
        // 2-fold CV: fit on the even tracks, test on the odd, and the other way round.
        let fold = |par: usize| fit(&(0..n).filter(|i| i % 2 == par).map(|i| (chromas[i], rb[i])).collect::<Vec<_>>());
        let (f0, f1) = (fold(0), fold(1));
        for bias in [0.0, b] {
            println!("  fitted-cv  {}  (minor bias {bias})", score(&|i| key_from_chroma(&chromas[i], if i % 2 == 0 { &f1 } else { &f0 }, bias)).line());
        }
        if ci == 0 || std::env::var("KEY_PRINT_FIT").is_ok() {
            let all = fit(&(0..n).map(|i| (chromas[i], rb[i])).collect::<Vec<_>>());
            let norm = |p: [f32; 12]| { let m = p.iter().cloned().fold(0.0, f32::max); p.map(|x| (x / m * 10000.0).round() / 10000.0) };
            println!("  fit on all: major {:?}\n              minor {:?}", norm(all.0), norm(all.1));
        }
    }
    Ok(())
}
