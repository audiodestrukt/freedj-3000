//! OpenDeck as a Pro DJ Link **media source**: the library other decks browse
//! over dbserver and read over NFS, built from a music folder.
//!
//! [`scan`] walks the folder into an NFS tree and a [`Library`] of file-name
//! rows in a few milliseconds, so the services can start at once.  Real
//! metadata (tags, duration, tempo, beat grid, waveforms) needs a decode of
//! every file; [`start_analysis`] does that on one low-priority thread, one
//! track at a time, updating the shared library as each finishes and caching
//! the result on disk so the second launch is instant.  Players re-fetch a
//! track's rows when they load it, so a row that starts as a bare file name
//! grows its duration and BPM while the library sits in someone's LINK list.
//!
//! Both the app (`start_media_server`) and the stand-alone `opendeck-serve`
//! binary run exactly this.

use anyhow::{Context, Result};
use opendeck_analysis::{BeatAnalyzerImpl, WaveformBuilder};
use opendeck_dbserver::server::{Library, SharedLibrary, Track};
use opendeck_decode::SymphoniaDecoder;
use opendeck_nfs::server::Tree;
use opendeck_types::{BeatAnalyzer, Decoder};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the decoder plays; anything else in the folder is not a track.
pub const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "aiff", "aif", "ogg"];

/// Waveform preview columns (the 0x4402 blob is one height/whiteness pair each).
pub const PREVIEW_COLUMNS: usize = 400;
/// Waveform detail resolution: half-frames per second (0x4a02).
pub const DETAIL_PER_SEC: u32 = 150;
/// How much of a track the beat detector sees.  Its own analysis span is
/// two minutes (`opendeck-analysis`); holding more would only cost memory.
const LEAD_SECS: u32 = 121;
/// Breather between tracks so a long first-run analysis stays in the
/// background of a running deck.
const PAUSE_BETWEEN: Duration = Duration::from_millis(250);

/// A scanned folder: the NFS tree, the shared library, and each track's
/// local file (by track id) for the analysis thread and the art loader.
pub struct Scanned {
    pub tree:    Tree,
    pub library: SharedLibrary,
    pub files:   Vec<(u32, PathBuf)>,
}

/// Walk `root`; track ids are 1-based in scan order, titles are file stems
/// until analysis replaces them.
pub fn scan(root: &Path, name: &str) -> Result<Scanned> {
    let tree = Tree::scan(root).with_context(|| format!("scan {}", root.display()))?;
    let mut tracks = Vec::new();
    let mut files = Vec::new();
    for (i, (nfs_path, local)) in tree.files().into_iter().enumerate() {
        let ext = local.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !AUDIO_EXTS.contains(&ext.as_str()) { continue; }
        let id = i as u32 + 1;
        let title = local.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        tracks.push(Track { id, title, path: nfs_path, ..Default::default() });
        files.push((id, local));
    }
    let by_id: Arc<Vec<(u32, PathBuf)>> = Arc::new(files.clone());
    let art = Arc::new(move |id: u32| -> Option<Vec<u8>> {
        let (_, path) = by_id.iter().find(|(i, _)| *i == id)?;
        SymphoniaDecoder::open(path).ok()?.tags().artwork.as_ref().map(|(_, bytes)| bytes.clone())
    });
    let library = Library { tracks, name: name.to_string(), art: Some(art) }.shared();
    Ok(Scanned { tree, library, files })
}

/// One track's analysis, as served and as cached.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    pub title:      String,
    pub artist:     String,
    pub album:      String,
    pub comment:    String,
    pub key:        String,
    pub duration_s: u32,
    pub bpm:        f32,
    pub bitrate:    u32,
    pub beats:      Vec<(u8, f32, u32)>,
    pub preview:    Vec<u8>,
    pub detail:     Vec<u8>,
    pub has_art:    bool,
}

impl Analysis {
    fn apply(&self, t: &mut Track) {
        if !self.title.is_empty() { t.title = self.title.clone(); }
        t.artist = self.artist.clone();
        t.album = self.album.clone();
        t.comment = self.comment.clone();
        t.key = self.key.clone();
        t.duration_s = self.duration_s;
        t.bpm = self.bpm;
        t.bitrate = self.bitrate;
        t.beats = self.beats.clone();
        t.preview = self.preview.clone();
        t.detail = self.detail.clone();
        t.has_art = self.has_art;
    }
}

/// Decode one file and analyse it: tags, duration, tempo + grid from the
/// leading two minutes, waveform preview and detail from the whole track.
/// Streams through the waveform builder, so memory is the lead buffer plus
/// the columns, not the decoded track.
pub fn analyze(path: &Path) -> Result<Analysis> {
    let mut dec = SymphoniaDecoder::open(path).with_context(|| format!("open {}", path.display()))?;
    let tags = dec.tags().clone();
    let sr = dec.sample_rate();
    let ch = dec.channels().max(1) as usize;
    let lead_cap = LEAD_SECS as usize * sr as usize * 2;
    let mut lead: Vec<f32> = Vec::with_capacity(lead_cap.min(1 << 26));
    let mut wb = WaveformBuilder::new(sr);
    let mut frames: u64 = 0;
    let mut buf = vec![0f32; 4096 * ch];
    // Both analysers assume interleaved stereo; fold other layouts to that.
    let mut stereo: Vec<f32> = Vec::with_capacity(4096 * 2);
    loop {
        let n = dec.decode(&mut buf)?;
        if n == 0 { break; }
        frames += n as u64;
        stereo.clear();
        match ch {
            2 => stereo.extend_from_slice(&buf[..n * 2]),
            1 => for &x in &buf[..n] { stereo.push(x); stereo.push(x); },
            c => for f in buf[..n * c].chunks(c) { stereo.push(f[0]); stereo.push(f[1.min(c - 1)]); },
        }
        wb.push(&stereo);
        if lead.len() < lead_cap {
            let take = (lead_cap - lead.len()).min(stereo.len());
            lead.extend_from_slice(&stereo[..take]);
        }
    }
    let duration_s = (frames as f64 / sr as f64).round() as u32;
    let wave = wb.finish();

    let mut ba = BeatAnalyzerImpl::new(sr);
    ba.push(&lead, sr);
    drop(lead);
    let (bpm, beats) = match ba.beat_grid() {
        Some(g) => (g.bpm as f32, grid_beats(g.anchor_sample, g.bpm, g.downbeat_offset, sr, frames)),
        None => (0.0, Vec::new()),
    };

    // Column c covers hop_size frames; amplitude and the high band drive the
    // height and whiteness the way a player's blue waveform reads, scaled so
    // the track's loudest column fills the 31 px a player draws.
    let cols = &wave.columns;
    let hop = wave.hop_size as u64;
    let col_at = |frame: u64| ((frame / hop) as usize).min(cols.len().saturating_sub(1));
    let max_amp  = cols.iter().map(|c| c[3] as u32).max().unwrap_or(0).max(1);
    let max_high = cols.iter().map(|c| c[2] as u32).max().unwrap_or(0).max(1);
    let pack = |c: &[u8; 4]| -> (u8, u8) { ((c[3] as u32 * 31 / max_amp) as u8, (c[2] as u32 * 7 / max_high) as u8) };
    let mut preview = Vec::with_capacity(PREVIEW_COLUMNS * 2);
    for i in 0..PREVIEW_COLUMNS * (!cols.is_empty()) as usize {
        let (a, b) = (col_at(frames * i as u64 / PREVIEW_COLUMNS as u64), col_at(frames * (i as u64 + 1) / PREVIEW_COLUMNS as u64));
        let (mut h, mut w) = (0u8, 0u8);
        for c in &cols[a..=b.max(a)] { let (ch, cw) = pack(c); h = h.max(ch); w = w.max(cw); }
        preview.push(h); preview.push(w);
    }
    let half_frames = (frames * DETAIL_PER_SEC as u64 / sr as u64) as usize;
    let mut detail = Vec::with_capacity(half_frames);
    for j in 0..half_frames * (!cols.is_empty()) as usize {
        let c = col_at(j as u64 * sr as u64 / DETAIL_PER_SEC as u64);
        let (h, w) = pack(&cols[c]);
        detail.push((w << 5) | h);
    }

    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let bitrate = if duration_s > 0 { (bytes * 8 / duration_s as u64 / 1000) as u32 } else { 0 };
    Ok(Analysis {
        title:   tags.title.clone().unwrap_or_default(),
        artist:  tags.artist.clone().unwrap_or_default(),
        album:   tags.album.clone().unwrap_or_default(),
        comment: tags.comment.clone().unwrap_or_default(),
        key:     tags.key.clone().unwrap_or_default(),
        duration_s, bpm, bitrate, beats, preview, detail,
        has_art: tags.artwork.is_some(),
    })
}

/// Expand a constant grid into the per-beat list a `0x4602` blob carries:
/// from the anchor to the end of the track, beat-in-bar 1–4.
fn grid_beats(anchor: u64, bpm: f64, downbeat_offset: u8, sr: u32, frames: u64) -> Vec<(u8, f32, u32)> {
    if bpm <= 0.0 { return Vec::new(); }
    let period = 60.0 * sr as f64 / bpm;
    let mut out = Vec::new();
    let mut k = 0u64;
    loop {
        let f = anchor as f64 + k as f64 * period;
        if f >= frames as f64 || k > 100_000 { break; }
        let bib = ((k + downbeat_offset as u64) % 4) as u8 + 1;
        out.push((bib, bpm as f32, (f * 1000.0 / sr as f64).round() as u32));
        k += 1;
    }
    out
}

// ── On-disk cache ─────────────────────────────────────────────────────────────
// One file per track, named by a hash of path + size + mtime, so a re-tagged
// or replaced file re-analyses and a moved library does not.

const CACHE_MAGIC: &[u8; 4] = b"ODLA";
const CACHE_VERSION: u16 = 1;

fn cache_key(path: &Path) -> Option<String> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.to_string_lossy().bytes().chain(md.len().to_le_bytes()).chain(mtime.to_le_bytes()) {
        h ^= b as u64; h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Some(format!("{h:016x}.v{CACHE_VERSION}"))
}

fn put_str(out: &mut Vec<u8>, s: &str) { out.extend_from_slice(&(s.len() as u32).to_le_bytes()); out.extend_from_slice(s.as_bytes()); }
fn put_bytes(out: &mut Vec<u8>, b: &[u8]) { out.extend_from_slice(&(b.len() as u32).to_le_bytes()); out.extend_from_slice(b); }

pub fn encode(a: &Analysis) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(CACHE_MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    for s in [&a.title, &a.artist, &a.album, &a.comment, &a.key] { put_str(&mut out, s); }
    out.extend_from_slice(&a.duration_s.to_le_bytes());
    out.extend_from_slice(&a.bpm.to_le_bytes());
    out.extend_from_slice(&a.bitrate.to_le_bytes());
    out.extend_from_slice(&(a.beats.len() as u32).to_le_bytes());
    for &(bib, bpm, ms) in &a.beats { out.push(bib); out.extend_from_slice(&bpm.to_le_bytes()); out.extend_from_slice(&ms.to_le_bytes()); }
    put_bytes(&mut out, &a.preview);
    put_bytes(&mut out, &a.detail);
    out.push(a.has_art as u8);
    out
}

pub fn decode(b: &[u8]) -> Option<Analysis> {
    struct R<'a>(&'a [u8], usize);
    impl R<'_> {
        fn take(&mut self, n: usize) -> Option<&[u8]> { let s = self.0.get(self.1..self.1 + n)?; self.1 += n; Some(s) }
        fn u8(&mut self) -> Option<u8> { Some(self.take(1)?[0]) }
        fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
        fn f32(&mut self) -> Option<f32> { Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
        fn bytes(&mut self) -> Option<Vec<u8>> { let n = self.u32()? as usize; Some(self.take(n)?.to_vec()) }
        fn str(&mut self) -> Option<String> { String::from_utf8(self.bytes()?).ok() }
    }
    let mut r = R(b, 0);
    if r.take(4)? != CACHE_MAGIC || u16::from_le_bytes(r.take(2)?.try_into().ok()?) != CACHE_VERSION { return None; }
    let mut a = Analysis { title: r.str()?, artist: r.str()?, album: r.str()?, comment: r.str()?, key: r.str()?, ..Default::default() };
    a.duration_s = r.u32()?; a.bpm = r.f32()?; a.bitrate = r.u32()?;
    let n = r.u32()? as usize;
    a.beats = (0..n).map(|_| Some((r.u8()?, r.f32()?, r.u32()?))).collect::<Option<Vec<_>>>()?;
    a.preview = r.bytes()?; a.detail = r.bytes()?; a.has_art = r.u8()? != 0;
    Some(a)
}

/// Analyse every file on a background thread, cached under `cache_dir` when
/// given.  Each result lands in the shared library as soon as it is ready.
pub fn start_analysis(lib: SharedLibrary, files: Vec<(u32, PathBuf)>, cache_dir: Option<PathBuf>) -> Result<std::thread::JoinHandle<()>> {
    if let Some(d) = &cache_dir { let _ = std::fs::create_dir_all(d); }
    let h = std::thread::Builder::new().name("media-analysis".into()).spawn(move || {
        let t0 = Instant::now();
        let (mut cached, mut fresh, mut failed) = (0, 0, 0);
        for (id, path) in &files {
            let cache_file = cache_dir.as_ref().and_then(|d| cache_key(path).map(|k| d.join(k)));
            let from_cache = cache_file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| decode(&b));
            let a = match from_cache {
                Some(a) => { cached += 1; a }
                None => match analyze(path) {
                    Ok(a) => {
                        fresh += 1;
                        if let Some(f) = &cache_file { if let Err(e) = std::fs::write(f, encode(&a)) { log::warn!("media: cache {}: {e}", f.display()); } }
                        log::info!("media: analysed [{id}] {:?}: {}:{:02} {:.1} BPM, {} beats{}", if a.title.is_empty() { path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { a.title.clone() },
                                   a.duration_s / 60, a.duration_s % 60, a.bpm, a.beats.len(), if a.has_art { ", art" } else { "" });
                        std::thread::sleep(PAUSE_BETWEEN);
                        a
                    }
                    Err(e) => { failed += 1; log::warn!("media: [{id}] {}: {e:#}", path.display()); continue; }
                },
            };
            if let Ok(mut l) = lib.write() { if let Some(t) = l.track_mut(*id) { a.apply(t); } }
        }
        log::info!("media: library ready — {} tracks ({cached} cached, {fresh} analysed, {failed} failed) in {:.1}s", files.len(), t0.elapsed().as_secs_f32());
    })?;
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trips() {
        let a = Analysis {
            title: "T".into(), artist: "A".into(), album: "".into(), comment: "c".into(), key: "Am".into(),
            duration_s: 200, bpm: 128.0, bitrate: 320, beats: vec![(1, 128.0, 25), (2, 128.0, 494)],
            preview: vec![1, 2, 3, 4], detail: vec![0x21; 10], has_art: true,
        };
        assert_eq!(decode(&encode(&a)), Some(a.clone()));
        let mut short = encode(&a); short.truncate(20);
        assert_eq!(decode(&short), None);
        assert_eq!(decode(b"nope"), None);
    }

    #[test]
    fn grid_beats_count_bars_from_the_anchor() {
        // 120 BPM at 1000 Hz: a beat every 500 frames; 4 s of audio.
        let b = grid_beats(100, 120.0, 0, 1000, 4000);
        assert_eq!(b.len(), 8);
        assert_eq!(b[0], (1, 120.0, 100));
        assert_eq!(b[4], (1, 120.0, 2100));
        assert_eq!(b[5].0, 2);
        // Beat 0 is the second beat of its bar.
        assert_eq!(grid_beats(0, 120.0, 1, 1000, 1000)[0].0, 2);
        assert!(grid_beats(0, 0.0, 0, 1000, 1000).is_empty());
    }

    #[test]
    fn analyses_a_generated_tone() {
        // A 2 s stereo WAV of clicks at 120 BPM: duration, waveform sizes and
        // a plausible tempo without touching any real music.
        let sr = 44_100u32;
        let frames = sr as usize * 2;
        let mut pcm = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f32 / sr as f32;
            let click = if (t * 2.0).fract() < 0.02 { 0.8 } else { 0.0 };
            let s = ((click + 0.05 * (t * 440.0 * std::f32::consts::TAU).sin()) * 32767.0) as i16;
            pcm.push(s); pcm.push(s);
        }
        let mut wav = Vec::new();
        let data_len = (pcm.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF"); wav.extend_from_slice(&(36 + data_len).to_le_bytes()); wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); wav.extend_from_slice(&1u16.to_le_bytes()); wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&sr.to_le_bytes()); wav.extend_from_slice(&(sr * 4).to_le_bytes()); wav.extend_from_slice(&4u16.to_le_bytes()); wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data"); wav.extend_from_slice(&data_len.to_le_bytes());
        for s in pcm { wav.extend_from_slice(&s.to_le_bytes()); }
        let dir = std::env::temp_dir().join(format!("opendeck-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clicks.wav");
        std::fs::write(&path, wav).unwrap();

        let a = analyze(&path).unwrap();
        assert_eq!(a.duration_s, 2);
        assert_eq!(a.preview.len(), PREVIEW_COLUMNS * 2);
        assert_eq!(a.detail.len(), 2 * DETAIL_PER_SEC as usize);
        assert_eq!(a.preview.iter().step_by(2).max(), Some(&31), "loudest column fills the height");
        assert!(a.preview.iter().step_by(2).all(|&h| h < 32) && a.preview.iter().skip(1).step_by(2).all(|&w| w < 8));
        assert!(!a.has_art);

        // Scan + cache: the served row picks the analysis up.
        let sc = scan(&dir, "T").unwrap();
        assert_eq!(sc.library.read().unwrap().tracks[0].title, "clicks");
        start_analysis(Arc::clone(&sc.library), sc.files.clone(), Some(dir.join("cache"))).unwrap().join().unwrap();
        let l = sc.library.read().unwrap();
        assert_eq!(l.tracks[0].duration_s, 2);
        assert_eq!(l.tracks[0].detail.len(), a.detail.len());
        drop(l);
        assert_eq!(std::fs::read_dir(dir.join("cache")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
