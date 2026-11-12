//! How close are OpenDeck's 3-band waveforms to rekordbox's?  For tracks on a
//! linked player that have a .2EX file, fetch the audio and the .2EX over
//! NFS, run OpenDeck's waveform analysis, and compare band by band with
//! rekordbox's PWV7 (detail, 150/s) and PWV6 (preview, 1200 columns).
//!
//!   cargo run --release -p opendeck-app --example waveform_vs_rekordbox -- <player-ip> [tracks]
//!
//! Prints, per band, the correlation over time and our raw value at
//! rekordbox's percentiles (to fit the mapping in mediaserver::three_band).
use opendeck_analysis::WaveformBuilder;
use opendeck_nfs::Nfs;
use std::net::Ipv4Addr;

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

/// Whole file to interleaved stereo f32 and its rate.
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

fn corr(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len()) as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) { sab += (x - ma) * (y - mb); saa += (x - ma).powi(2); sbb += (y - mb).powi(2); }
    sab / (saa * sbb).sqrt().max(1e-9)
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let ip: Ipv4Addr = a.next().expect("usage: waveform_vs_rekordbox <ip> [tracks]").parse()?;
    let want: usize = a.next().map(|m| m.parse()).transpose()?.unwrap_or(8);
    let mut nfs = Nfs::connect(ip)?;
    let root = nfs.mount_usb()?;
    let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb")?;
    let pdb = nfs.read_file(&fh, size)?;
    let exp = opendeck_rekordbox::read_export_from(&mut std::io::Cursor::new(pdb), "nfs://player".into())?;
    // (ours, theirs) per band [mid, high, low], for detail and preview.
    let mut det: [(Vec<f64>, Vec<f64>); 3] = Default::default();
    let mut pre: [(Vec<f64>, Vec<f64>); 3] = Default::default();
    // RGB (.EXT): PWV5 detail channels [r, g, b, h]; PWV4 preview bytes 0..5.
    // Ours per point: [bass, mid, high, rms].
    let mut rgb_det: (Vec<[f64; 4]>, Vec<[f64; 4]>) = Default::default();
    let mut rgb_pre: (Vec<[f64; 4]>, Vec<[f64; 6]>) = Default::default();
    let mut done = 0;
    for t in &exp.tracks {
        if done >= want { break; }
        let ex = t.analyze_rel.trim_start_matches('/').replace(".DAT", ".2EX");
        let Ok((efh, esz)) = nfs.lookup_path(&root, &ex) else { continue };
        let Ok((afh, asz)) = nfs.lookup_path(&root, t.rel_path.trim_start_matches('/')) else { continue };
        let two = nfs.read_file(&efh, esz)?;
        let audio = nfs.read_file(&afh, asz)?;
        let (Some(p7), Some(p6)) = (tag(&two, b"PWV7"), tag(&two, b"PWV6")) else { continue };
        let ext_path = t.analyze_rel.trim_start_matches('/').replace(".DAT", ".EXT");
        let ext = nfs.lookup_path(&root, &ext_path).ok().and_then(|(f, n)| nfs.read_file(&f, n).ok()).unwrap_or_default();
        let (pcm, sr) = decode(audio, t.rel_path.rsplit('.').next())?;
        let mut wb = WaveformBuilder::new(sr);
        wb.push(&pcm);
        let wave = wb.finish();
        let frames = (pcm.len() / 2) as u64;
        let cols = &wave.columns;
        // A column's value belongs to its window's centre, half an FFT on.
        let col_at = |f: u64| ((f.saturating_sub(opendeck_analysis::waveform::FFT_SIZE as u64 / 2) / wave.hop_size as u64) as usize).min(cols.len() - 1);
        // ours: [R bass, G mid, B high]; theirs, on the wire: [low, mid, high]
        let ours_band = |c: &[u8; 4], b: usize| [c[0], c[1], c[2]][b] as f64;
        let n7 = u32::from_be_bytes(p7[0x10..0x14].try_into()?) as usize;
        // OD_DUMP=<file>: raw pairs for the first track (time, our bass mid high, their bytes 0 1 2).
        if done == 0 { if let Ok(f) = std::env::var("OD_DUMP") {
            let mut s = String::new();
            for j in 0..n7 { let c = &cols[col_at(j as u64 * sr as u64 / 150)];
                s += &format!("{j} {} {} {} {} {} {}\n", c[0], c[1], c[2], p7[0x18 + j * 3], p7[0x18 + j * 3 + 1], p7[0x18 + j * 3 + 2]); }
            std::fs::write(f, s)?;
        } }
        for j in 0..n7 {
            let c = &cols[col_at(j as u64 * sr as u64 / 150)];
            for b in 0..3 { det[b].0.push(ours_band(c, b)); det[b].1.push(p7[0x18 + j * 3 + b] as f64); }
        }
        if let (Some(p5), Some(p4)) = (tag(&ext, b"PWV5"), tag(&ext, b"PWV4")) {
            let n5 = u32::from_be_bytes(p5[0x10..0x14].try_into()?) as usize;
            for j in 0..n5.min(n7) {
                let c = &cols[col_at(j as u64 * sr as u64 / 150)];
                let v = u16::from_be_bytes([p5[0x18 + j * 2], p5[0x19 + j * 2]]);
                rgb_det.0.push([c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64]);
                rgb_det.1.push([(v >> 13) as f64, ((v >> 10) & 7) as f64, ((v >> 7) & 7) as f64, ((v >> 2) & 31) as f64]);
            }
            for i in 0..1200usize {
                let (s, e) = (col_at(frames * i as u64 / 1200), col_at(frames * (i as u64 + 1) / 1200));
                let span = &cols[s..=e.max(s)];
                let m = |k: usize| span.iter().map(|c| c[k] as f64).sum::<f64>() / span.len() as f64;
                rgb_pre.0.push([m(0), m(1), m(2), m(3)]);
                let b = &p4[0x18 + i * 6..0x18 + i * 6 + 6];
                rgb_pre.1.push([b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64, b[4] as f64, b[5] as f64]);
            }
        }
        for i in 0..1200usize {
            let (s, e) = (col_at(frames * i as u64 / 1200), col_at(frames * (i as u64 + 1) / 1200));
            for b in 0..3 {
                let span = &cols[s..=e.max(s)];
                let m = span.iter().map(|c| ours_band(c, b)).sum::<f64>() / span.len() as f64;
                pre[b].0.push(m); pre[b].1.push(p6[0x14 + i * 3 + b] as f64);
            }
        }
        done += 1;
        eprintln!("{:>4} {} - {}  ({} detail entries)", t.id, t.artist, t.title, n7);
    }
    for (name, set) in [("detail PWV7", &det), ("preview PWV6", &pre)] {
        println!("{name}:");
        for (b, band) in ["low ", "mid ", "high"].iter().enumerate() {
            let (o, r) = (&set[b].0, &set[b].1);
            let q = |v: &Vec<f64>, p: f64| { let mut s = v.clone(); s.sort_by(|a, b| a.partial_cmp(b).unwrap()); s[((s.len() - 1) as f64 * p) as usize] };
            println!("  {band} corr {:.3}  | p10/50/90/99 ours {:>3.0} {:>3.0} {:>3.0} {:>3.0}  rekordbox {:>3.0} {:>3.0} {:>3.0} {:>3.0}",
                corr(o, r), q(o, 0.1), q(o, 0.5), q(o, 0.9), q(o, 0.99), q(r, 0.1), q(r, 0.5), q(r, 0.9), q(r, 0.99));
        }
    }
    let pct = |v: Vec<f64>| { let mut s = v; s.sort_by(|a, b| a.partial_cmp(b).unwrap()); (0..=20).map(|k| s[(s.len() - 1) * k / 20].round() as u32).collect::<Vec<_>>() };
    for (name, ours, theirs, labels) in [
        ("PWV5 detail", &rgb_det.0, rgb_det.1.iter().map(|x| x.to_vec()).collect::<Vec<_>>(), vec!["r", "g", "b", "h"]),
        ("PWV4 preview", &rgb_pre.0, rgb_pre.1.iter().map(|x| x.to_vec()).collect::<Vec<_>>(), vec!["b0", "b1", "b2", "b3", "b4", "b5"])] {
        if ours.is_empty() { continue; }
        println!("{name} (corr with our bass / mid / high / rms; percentiles of theirs, then of the best feature):");
        for (k, l) in labels.iter().enumerate() {
            let t: Vec<f64> = theirs.iter().map(|x| x[k]).collect();
            let cs: Vec<f64> = (0..4).map(|f| corr(&ours.iter().map(|o| o[f]).collect::<Vec<_>>(), &t)).collect();
            let best = (0..4).max_by(|a, b| cs[*a].partial_cmp(&cs[*b]).unwrap()).unwrap();
            println!("  {l}: {:.2} {:.2} {:.2} {:.2}  rb {:?}\n      ours[{best}] {:?}", cs[0], cs[1], cs[2], cs[3], pct(t.clone()), pct(ours.iter().map(|o| o[best]).collect()));
        }
    }
    // Percentile breakpoints (0, 5, ..., 100 %) of ours and rekordbox's,
    // pasted into mediaserver's three-band curves.
    for (name, set) in [("DETAIL", &det), ("PREVIEW", &pre)] {
        for (b, band) in ["LOW", "MID", "HIGH"].iter().enumerate() {
            let pts = |v: &Vec<f64>| { let mut s = v.clone(); s.sort_by(|a, b| a.partial_cmp(b).unwrap());
                (0..=20).map(|k| s[(s.len() - 1) * k / 20].round() as u32).collect::<Vec<_>>() };
            println!("{name}_{band}: ours {:?}\n{:>w$}  rb   {:?}", pts(&set[b].0), "", pts(&set[b].1), w = name.len() + band.len() + 1);
        }
    }
    Ok(())
}
