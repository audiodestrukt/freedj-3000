//! How good is the deck's own BPM / beat-grid analysis?  Rekordbox is the
//! answer key: for every track on a rekordbox stick, decode the audio, run
//! the same analyzer the deck runs on a plain file (opendeck-analysis), and
//! compare with the grid rekordbox wrote to the track's ANLZ file.
//!
//!   cargo run --release -p opendeck-app --example analysis_vs_rekordbox -- /run/media/$USER/CDJ2 [out.csv]
//!
//! Per track: rekordbox BPM vs ours (exact / close / half-double / wrong),
//! and the grid's phase error in ms at the first beat, ~60 s and ~120 s in
//! (a BPM a hair off shows up as drift).  Downbeats are not compared: the
//! analyzer does not detect them yet.

use opendeck_analysis::BeatAnalyzerImpl;
use opendeck_decode::symphonia_decoder::SymphoniaDecoder;
use opendeck_rekordbox::{read_anlz, read_export, RbTrack};
use opendeck_types::{BeatAnalyzer, Decoder};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct Row {
    title: String,
    ext: String,
    rb_bpm: f64,
    our_bpm: Option<f64>,
    verdict: &'static str,
    /// Phase error (ms) at rekordbox beats near 0 s, 60 s, 120 s.
    phase: [Option<f64>; 3],
    note: String,
}

fn decode(path: &Path) -> anyhow::Result<(Vec<f32>, u32)> {
    let mut d = SymphoniaDecoder::open(path)?;
    let (sr, ch) = (d.sample_rate(), d.channels() as usize);
    let mut out = Vec::new();
    let mut buf = vec![0f32; 4096 * ch];
    loop {
        match d.decode(&mut buf)? {
            0 => break,
            n => {
                // The analyzer takes interleaved stereo, as the deck feeds it.
                for f in buf[..n * ch].chunks(ch) {
                    let (l, r) = if ch >= 2 { (f[0], f[1]) } else { (f[0], f[0]) };
                    out.push(l);
                    out.push(r);
                }
            }
        }
    }
    Ok((out, sr))
}

fn one(root: &Path, t: &RbTrack) -> Row {
    let ext = Path::new(&t.rel_path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut row = Row { title: format!("{} - {}", t.artist, t.title), ext, rb_bpm: t.bpm as f64, our_bpm: None,
                        verdict: "?", phase: [None; 3], note: String::new() };
    let Some(anlz) = t.analyze_on(root) else { row.note = "no ANLZ".into(); return row };
    let rb = match read_anlz(&anlz) { Ok(a) => a, Err(e) => { row.note = format!("ANLZ: {e:#}"); return row } };
    if rb.beats.len() < 8 { row.note = "rekordbox grid empty".into(); return row; }
    let mut bpms: Vec<f64> = rb.beats.iter().map(|b| b.bpm as f64).collect();
    bpms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    row.rb_bpm = bpms[bpms.len() / 2];

    let (pcm, sr) = match decode(&t.path_on(root)) { Ok(x) => x, Err(e) => { row.note = format!("decode: {e:#}"); return row } };
    let mut an = BeatAnalyzerImpl::new(sr);
    an.push(&pcm, sr);
    let Some(g) = an.beat_grid() else { row.verdict = "no BPM"; return row };
    row.our_bpm = Some(g.bpm);

    let (ours, theirs) = (g.bpm, row.rb_bpm);
    let d = (ours - theirs).abs();
    row.verdict = if d <= 0.05 { "exact" } else if d <= 0.5 { "close" }
        else if (ours * 2.0 - theirs).abs() <= 0.5 || (ours / 2.0 - theirs).abs() <= 0.5 { "half/double" }
        else if (ours * 1.5 - theirs).abs() <= 0.5 || (ours / 1.5 - theirs).abs() <= 0.5 { "3:2" }
        else { "wrong" };

    // Dev: OD_FOLD=<title substring> prints where the energy sits within the
    // beat, folded over rekordbox's grid (0 = its beat), low / mid / high.
    if std::env::var("OD_FOLD").is_ok_and(|f| row.title.contains(&f)) {
        fold(&pcm, sr, &rb.beats, g.anchor_sample as f64 / sr as f64 * 1000.0, g.bpm, &row.title);
    }
    // Phase: our nearest grid line to rekordbox beats near 0 / 60 / 120 s.
    if matches!(row.verdict, "exact" | "close") {
        let period_ms = 60_000.0 / ours;
        let anchor_ms = g.anchor_sample as f64 / sr as f64 * 1000.0;
        for (k, at) in [0.0f64, 60_000.0, 120_000.0].iter().enumerate() {
            let Some(b) = rb.beats.iter().find(|b| b.time_ms as f64 >= *at) else { continue };
            let tb = b.time_ms as f64;
            let n = ((tb - anchor_ms) / period_ms).round();
            row.phase[k] = Some(anchor_ms + n * period_ms - tb);
        }
    }
    row
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("usage: analysis_vs_rekordbox <stick root> [out.csv]"));
    let csv = args.next();
    let exp = read_export(&root)?;
    let tracks: Vec<RbTrack> = exp.tracks.clone();
    eprintln!("{} tracks on {}", tracks.len(), root.display());

    let queue = Arc::new(Mutex::new(tracks.into_iter().enumerate().collect::<Vec<_>>()));
    let rows = Arc::new(Mutex::new(Vec::new()));
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(12);
    std::thread::scope(|s| {
        for _ in 0..threads {
            let (queue, rows, root) = (Arc::clone(&queue), Arc::clone(&rows), root.clone());
            s.spawn(move || loop {
                let Some((i, t)) = queue.lock().unwrap().pop() else { break };
                let r = one(&root, &t);
                eprintln!("{:>4} {:<11} rb {:>6.2}  ours {:>6}  {}", i, r.verdict, r.rb_bpm,
                          r.our_bpm.map(|b| format!("{b:.2}")).unwrap_or("-".into()), r.title);
                rows.lock().unwrap().push((i, r));
            });
        }
    });
    let mut rows = Arc::try_unwrap(rows).unwrap().into_inner().unwrap();
    rows.sort_by_key(|(i, _)| *i);

    let n = rows.len();
    let count = |v: &str| rows.iter().filter(|(_, r)| r.verdict == v).count();
    println!("\n{} tracks", n);
    for v in ["exact", "close", "half/double", "3:2", "wrong", "no BPM", "?"] {
        let c = count(v);
        if c > 0 { println!("  {:<12} {:>4}  ({:.0}%)", v, c, 100.0 * c as f64 / n as f64); }
    }
    for (k, label) in ["first beat", "~60 s", "~120 s"].iter().enumerate() {
        let mut e: Vec<f64> = rows.iter().filter_map(|(_, r)| r.phase[k]).map(f64::abs).collect();
        if e.is_empty() { continue; }
        e.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let within = |ms: f64| e.iter().filter(|x| **x <= ms).count();
        println!("  phase {:<10} n={:<4} median {:>5.1} ms  p90 {:>6.1} ms  ≤10 ms {:>3}  ≤30 ms {:>3}",
                 label, e.len(), e[e.len() / 2], e[e.len() * 9 / 10], within(10.0), within(30.0));
    }
    if let Some(p) = csv {
        let mut s = String::from("title,ext,rb_bpm,our_bpm,verdict,phase0_ms,phase60_ms,phase120_ms,note\n");
        for (_, r) in &rows {
            let f = |x: Option<f64>| x.map(|v| format!("{v:.1}")).unwrap_or_default();
            s += &format!("\"{}\",{},{:.2},{},{},{},{},{},\"{}\"\n", r.title.replace('"', "'"), r.ext, r.rb_bpm,
                          f(r.our_bpm), r.verdict, f(r.phase[0]), f(r.phase[1]), f(r.phase[2]), r.note);
        }
        std::fs::write(&p, s)?;
        println!("wrote {p}");
    }
    Ok(())
}

/// Energy folded over rekordbox's beats in 16 slots per beat, per band.
fn fold(pcm: &[f32], sr: u32, beats: &[opendeck_rekordbox::RbBeat], our_anchor_ms: f64, our_bpm: f64, title: &str) {
    // One-pole band splits on the mono signal: low < ~150 Hz, high > ~5 kHz, mid between.
    let mono: Vec<f32> = pcm.chunks(2).map(|f| 0.5 * (f[0] + f[1])).collect();
    let a = |hz: f32| (-2.0 * std::f32::consts::PI * hz / sr as f32).exp();
    let (al, ah) = (a(150.0), a(5000.0));
    let (mut lp, mut lp2) = (0f32, 0f32);
    let mut bands = vec![[0f32; 3]; mono.len()];
    for (i, &x) in mono.iter().enumerate() {
        lp = al * lp + (1.0 - al) * x;          // < 150
        lp2 = ah * lp2 + (1.0 - ah) * x;        // < 5k
        bands[i] = [lp.abs(), (lp2 - lp).abs(), (x - lp2).abs()];
    }
    const SLOTS: usize = 16;
    let mut acc = [[0f64; SLOTS]; 3];
    for w in beats.windows(2).take(400) {
        let (t0, t1) = (w[0].time_ms as f64 / 1000.0 * sr as f64, w[1].time_ms as f64 / 1000.0 * sr as f64);
        for k in 0..SLOTS {
            let s0 = (t0 + (t1 - t0) * k as f64 / SLOTS as f64) as usize;
            let s1 = (t0 + (t1 - t0) * (k + 1) as f64 / SLOTS as f64) as usize;
            for b in 0..3 { acc[b][k] += bands[s0.min(bands.len())..s1.min(bands.len())].iter().map(|v| v[b] as f64).sum::<f64>(); }
        }
    }
    let per = 60_000.0 / our_bpm;
    let b0 = beats[0].time_ms as f64;
    let ours = ((our_anchor_ms - b0).rem_euclid(per) / per * SLOTS as f64) as usize % SLOTS;
    eprintln!("\nFOLD {title}  (slot 0 = rekordbox beat; ^ = our grid)");
    for (b, name) in ["low ", "mid ", "high"].iter().enumerate() {
        let m = acc[b].iter().cloned().fold(0f64, f64::max).max(1e-9);
        let bars: String = acc[b].iter().map(|v| [' ', '.', ':', '-', '=', '+', '*', '#', '@'][((v / m) * 8.0).round() as usize]).collect();
        eprintln!("  {name} |{bars}|");
    }
    eprintln!("        |{}^{}|", " ".repeat(ours), " ".repeat(SLOTS - 1 - ours));
}
