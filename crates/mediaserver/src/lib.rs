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
use opendeck_analysis::{BeatAnalyzerImpl, KeyDetector, WaveformBuilder, WaveformCache};
use opendeck_dbserver::server::{Library, SharedLibrary, Track};
use opendeck_decode::{SymphoniaDecoder, TrackTags};
use opendeck_nfs::server::Tree;
use opendeck_types::{BeatAnalyzer, BeatGrid, Decoder};
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
    let library = Library { tracks, name: name.to_string(), art: Some(art), history: Vec::new() }.shared();
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
    /// 3-band waveforms for CDJ-3000-class players (`.2EX` PWV6 / PWV7):
    /// 1,200 columns and 150 entries a second, 3 bytes each (low, mid, high).
    pub band_preview: Vec<u8>,
    pub band_detail:  Vec<u8>,
    /// RGB waveforms for nexus-2-class players (`.EXT` PWV4 / PWV5): 1,200
    /// columns of 6 bytes, and 150 two-byte entries a second.
    pub rgb_preview: Vec<u8>,
    pub rgb_detail:  Vec<u8>,
    /// Phrase analysis: the `.EXT` PSSI section exactly as rekordbox writes
    /// it (masked), from [`opendeck_analysis::phrase::detect`].  Empty = none.
    pub phrases:    Vec<u8>,
    pub has_art:    bool,
}

impl Analysis {
    /// The same record from what a deck load already produced (its tags, the
    /// grid it resolved, its waveform cache), so a track the deck analysed is
    /// never analysed again for the library, and the other way round.
    /// `frames` and `sr` describe the samples the waveform and grid are in.
    pub fn from_deck(tags: &TrackTags, grid: Option<&BeatGrid>, wave: &WaveformCache, frames: u64, sr: u32, file_bytes: u64) -> Analysis {
        let duration_s = (frames as f64 / sr as f64).round() as u32;
        let (bpm, beats) = match grid {
            Some(g) => (g.bpm as f32, grid_beats(g.anchor_sample, g.bpm, g.downbeat_offset, sr, frames)),
            None => (0.0, Vec::new()),
        };
        let (preview, detail) = waveform_blobs(wave, frames, sr);
        let (band_preview, band_detail) = three_band(wave, frames, sr);
        let (rgb_preview, rgb_detail) = rgb_waveforms(wave, frames, sr);
        let song = opendeck_analysis::phrase::detect(&beats, wave, frames, sr);
        let phrases = if song.phrases.is_empty() { Vec::new() } else { opendeck_analysis::phrase::encode_pssi(&song, true) };
        Analysis {
            title:   tags.title.clone().unwrap_or_default(),
            artist:  tags.artist.clone().unwrap_or_default(),
            album:   tags.album.clone().unwrap_or_default(),
            comment: tags.comment.clone().unwrap_or_default(),
            key:     tags.key.clone().unwrap_or_default(),
            duration_s, bpm, beats, preview, detail, band_preview, band_detail, rgb_preview, rgb_detail, phrases,
            bitrate: if duration_s > 0 { (file_bytes * 8 / duration_s as u64 / 1000) as u32 } else { 0 },
            has_art: tags.artwork.is_some(),
        }
    }

    /// Fill `key` from the audio when the file had no key tag (a tag key is
    /// kept as written).  `samples` are interleaved, `channels` wide, at `sr`.
    pub fn detect_key_if_missing(&mut self, samples: &[f32], channels: usize, sr: u32) {
        if self.key.is_empty() {
            if let Some(k) = opendeck_analysis::detect_key(samples, channels, sr) { self.key = k.name().to_string(); }
        }
    }

    /// Write this analysis into a served library row.
    pub fn apply(&self, t: &mut Track) {
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
        t.band_preview = self.band_preview.clone();
        t.band_detail = self.band_detail.clone();
        t.rgb_preview = self.rgb_preview.clone();
        t.rgb_detail = self.rgb_detail.clone();
        t.phrases = self.phrases.clone();
        t.has_art = self.has_art;
    }
}

/// Decode one file and analyse it: tags, duration, tempo + grid from the
/// leading two minutes, waveform preview and detail from the whole track,
/// and the key from the whole track when the tags have none.
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
    let tag_key = tags.key.as_deref().is_some_and(|k| !k.trim().is_empty());
    let mut kd = (!tag_key).then(|| KeyDetector::new(sr));
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
        if let Some(kd) = &mut kd { kd.push(&stereo, 2); }
        if lead.len() < lead_cap {
            let take = (lead_cap - lead.len()).min(stereo.len());
            lead.extend_from_slice(&stereo[..take]);
        }
    }
    let wave = wb.finish();
    let mut ba = BeatAnalyzerImpl::new(sr);
    ba.push(&lead, sr);
    drop(lead);
    let grid = ba.beat_grid();
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut a = Analysis::from_deck(&tags, grid.as_deref(), &wave, frames, sr, bytes);
    if let Some(k) = kd.and_then(|kd| kd.finish()) { if a.key.is_empty() { a.key = k.name().to_string(); } }
    Ok(a)
}

/// rekordbox's 3-band scale, as percentile pairs (ours → rekordbox) at 0, 5,
/// ..., 100 %, per band (low, mid, high).  Fitted on 30 rekordbox-analysed
/// tracks with `examples/waveform_vs_rekordbox` (correlation over time 0.93 /
/// 0.83 / 0.83 detail, 0.93 / 0.77 / 0.84 preview).  Our band values map
/// through these piecewise-linearly.
const BAND_DETAIL: [([u8; 21], [u8; 21]); 3] = [
    ([0, 9, 20, 31, 41, 51, 61, 72, 84, 97, 110, 120, 130, 138, 145, 153, 160, 168, 177, 189, 228],
     [0, 6, 12, 16, 20, 24, 29, 34, 40, 45, 50, 54, 59, 63, 67, 71, 75, 79, 84, 89, 112]),
    ([0, 15, 22, 26, 29, 32, 35, 37, 39, 42, 44, 46, 48, 50, 53, 55, 58, 61, 66, 72, 110],
     [0, 7, 12, 15, 18, 21, 23, 26, 28, 31, 33, 36, 39, 41, 45, 48, 53, 58, 65, 77, 127]),
    ([0, 3, 6, 8, 9, 11, 12, 13, 15, 16, 17, 18, 19, 21, 22, 23, 25, 27, 30, 34, 50],
     [0, 0, 0, 0, 1, 2, 2, 4, 5, 6, 8, 10, 13, 16, 21, 26, 34, 43, 57, 75, 127]),
];
const BAND_PREVIEW: [([u8; 21], [u8; 21]); 3] = [
    ([0, 11, 26, 39, 49, 57, 66, 75, 87, 101, 115, 124, 131, 136, 142, 148, 154, 160, 167, 175, 203],
     [0, 3, 6, 8, 10, 12, 15, 17, 21, 26, 30, 33, 35, 37, 38, 40, 42, 44, 47, 50, 68]),
    ([0, 21, 27, 31, 34, 36, 38, 40, 41, 43, 44, 46, 48, 49, 51, 53, 54, 57, 59, 63, 90],
     [0, 8, 12, 14, 16, 17, 19, 20, 21, 22, 24, 25, 26, 27, 29, 30, 32, 34, 36, 40, 72]),
    ([0, 5, 8, 10, 12, 13, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 23, 24, 26, 29, 46],
     [0, 3, 6, 9, 12, 15, 17, 19, 21, 22, 24, 26, 27, 29, 30, 32, 34, 37, 40, 45, 84]),
];

/// Piecewise-linear through `(ours, rb)` points; equal `ours` steps are skipped.
fn band_curve(x: f64, (ours, rb): &([u8; 21], [u8; 21])) -> u8 {
    if x <= ours[0] as f64 { return rb[0]; }
    for k in 1..21 {
        let (x0, x1) = (ours[k - 1] as f64, ours[k] as f64);
        if x <= x1 && x1 > x0 {
            return (rb[k - 1] as f64 + (x - x0) / (x1 - x0) * (rb[k] as f64 - rb[k - 1] as f64)).round() as u8;
        }
    }
    let (xl, yl) = (ours[20] as f64, rb[20] as f64);
    (yl + (x - xl) * yl / xl.max(1.0)).round().clamp(0.0, 127.0) as u8
}

/// 3-band waveforms as a CDJ-3000 reads them from a `.2EX` file: the 1,200-
/// column preview (PWV6) and the 150-a-second detail (PWV7), 3 bytes per
/// entry in the order low, mid, high (as rekordbox writes them; the order
/// "mid, high, low" in the published analysis does not match real files).
/// A column's value belongs to its FFT window's centre, half a window on.
pub fn three_band(wave: &WaveformCache, frames: u64, sr: u32) -> (Vec<u8>, Vec<u8>) {
    let cols = &wave.columns;
    if cols.is_empty() || frames == 0 { return (Vec::new(), Vec::new()); }
    let (hop, half) = (wave.hop_size.max(1) as u64, opendeck_analysis::waveform::FFT_SIZE as u64 / 2);
    let col_at = |frame: u64| ((frame.saturating_sub(half) / hop) as usize).min(cols.len() - 1);
    let band = |c: &[u8; 4], b: usize| c[b] as f64;   // columns are [bass, mid, high, rms]
    let n = (frames * DETAIL_PER_SEC as u64 / sr as u64) as usize;
    let mut detail = Vec::with_capacity(n * 3);
    for j in 0..n {
        let c = &cols[col_at(j as u64 * sr as u64 / DETAIL_PER_SEC as u64)];
        for b in 0..3 { detail.push(band_curve(band(c, b), &BAND_DETAIL[b])); }
    }
    let mut preview = Vec::with_capacity(BAND_PREVIEW_COLUMNS * 3);
    for i in 0..BAND_PREVIEW_COLUMNS {
        let s = col_at(frames * i as u64 / BAND_PREVIEW_COLUMNS as u64);
        let e = col_at(frames * (i as u64 + 1) / BAND_PREVIEW_COLUMNS as u64).max(s);
        let span = &cols[s..=e];
        for b in 0..3 {
            let mean = span.iter().map(|c| band(c, b)).sum::<f64>() / span.len() as f64;
            preview.push(band_curve(mean, &BAND_PREVIEW[b]));
        }
    }
    (preview, detail)
}

/// RGB waveforms (rekordbox's nexus-2 colour waveforms), fitted the same
/// way on the same 30 tracks.  Detail (PWV5) channels and the features they
/// follow (correlation): red ← bass (0.83), green ← mid (0.45), blue ← high
/// (0.60), height ← overall level (0.81).  Preview (PWV4) bytes: 0 ← level
/// (0.80), 1 ← level inverted (-0.48; a "whiteness" byte, 128-255), 2 and 3
/// ← bass (0.87, 0.91), 4 ← mid (0.58), 5 ← high (0.61).
const RMS_DETAIL: [u8; 21] = [0, 14, 20, 23, 26, 28, 30, 32, 34, 36, 38, 40, 42, 43, 45, 47, 48, 50, 52, 55, 66];
const RMS_PREVIEW: [u8; 21] = [0, 18, 23, 25, 28, 29, 31, 33, 34, 36, 38, 40, 41, 43, 44, 46, 47, 48, 50, 52, 59];
const RGB_R: [u8; 21] = [0, 0, 0, 0, 1, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7];
const RGB_G: [u8; 21] = [0, 0, 0, 1, 1, 1, 2, 2, 3, 3, 3, 4, 4, 5, 5, 5, 5, 5, 5, 5, 7];
const RGB_B: [u8; 21] = [0, 0, 0, 1, 2, 2, 3, 4, 4, 5, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7];
const RGB_H: [u8; 21] = [0, 0, 0, 1, 2, 3, 4, 6, 7, 9, 11, 13, 15, 17, 20, 22, 25, 27, 29, 30, 31];
const PV4_B0: [u8; 21] = [0, 34, 56, 71, 83, 93, 100, 106, 111, 115, 119, 121, 122, 124, 125, 126, 127, 127, 127, 127, 127];
/// Byte 1 falls as the level rises: these are rekordbox's percentiles from
/// the top down, against our level's from the bottom up.
const PV4_B1: [u8; 21] = [255, 215, 197, 182, 170, 161, 155, 149, 144, 140, 137, 135, 133, 132, 131, 130, 128, 128, 128, 128, 128];
const PV4_B2: [u8; 21] = [0, 12, 25, 33, 40, 47, 55, 63, 71, 79, 85, 91, 97, 102, 107, 111, 114, 117, 120, 124, 127];
const PV4_B3: [u8; 21] = [0, 1, 4, 10, 15, 21, 28, 38, 49, 60, 68, 75, 81, 87, 92, 98, 104, 109, 113, 119, 127];
const PV4_B4: [u8; 21] = [0, 8, 13, 17, 20, 24, 27, 30, 33, 36, 39, 42, 45, 49, 52, 56, 61, 67, 75, 88, 127];
const PV4_B5: [u8; 21] = [0, 3, 8, 12, 15, 18, 21, 24, 28, 31, 34, 37, 40, 43, 46, 49, 53, 57, 62, 73, 127];

/// RGB waveforms as a nexus-2-class player reads them from a `.EXT` file:
/// the 1,200-column preview (PWV4, 6 bytes a column) and the 150-a-second
/// detail (PWV5, 2 bytes big-endian: red 3 bits, green 3, blue 3, height 5,
/// 2 unused).
pub fn rgb_waveforms(wave: &WaveformCache, frames: u64, sr: u32) -> (Vec<u8>, Vec<u8>) {
    let cols = &wave.columns;
    if cols.is_empty() || frames == 0 { return (Vec::new(), Vec::new()); }
    let (hop, half) = (wave.hop_size.max(1) as u64, opendeck_analysis::waveform::FFT_SIZE as u64 / 2);
    let col_at = |frame: u64| ((frame.saturating_sub(half) / hop) as usize).min(cols.len() - 1);
    let f = |x: f64, ours: &[u8; 21], rb: &[u8; 21]| band_curve(x, &(*ours, *rb));
    let n = (frames * DETAIL_PER_SEC as u64 / sr as u64) as usize;
    let mut detail = Vec::with_capacity(n * 2);
    for j in 0..n {
        let c = &cols[col_at(j as u64 * sr as u64 / DETAIL_PER_SEC as u64)];
        let r = f(c[0] as f64, &BAND_DETAIL[0].0, &RGB_R).min(7) as u16;
        let g = f(c[1] as f64, &BAND_DETAIL[1].0, &RGB_G).min(7) as u16;
        let b = f(c[2] as f64, &BAND_DETAIL[2].0, &RGB_B).min(7) as u16;
        let h = f(c[3] as f64, &RMS_DETAIL, &RGB_H).min(31) as u16;
        detail.extend_from_slice(&((r << 13) | (g << 10) | (b << 7) | (h << 2)).to_be_bytes());
    }
    let mut preview = Vec::with_capacity(BAND_PREVIEW_COLUMNS * 6);
    for i in 0..BAND_PREVIEW_COLUMNS {
        let s = col_at(frames * i as u64 / BAND_PREVIEW_COLUMNS as u64);
        let e = col_at(frames * (i as u64 + 1) / BAND_PREVIEW_COLUMNS as u64).max(s);
        let span = &cols[s..=e];
        let m = |k: usize| span.iter().map(|c| c[k] as f64).sum::<f64>() / span.len() as f64;
        let level = m(3);
        // Byte 1 falls as the level rises: the level's percentile rank,
        // read off rekordbox's curve from the top down.
        let rank = band_curve(level, &(RMS_PREVIEW, RANKS)) as f64;
        preview.extend_from_slice(&[
            f(level, &RMS_PREVIEW, &PV4_B0),
            pv4_b1(rank),
            f(m(0), &BAND_PREVIEW[0].0, &PV4_B2),
            f(m(0), &BAND_PREVIEW[0].0, &PV4_B3),
            f(m(1), &BAND_PREVIEW[1].0, &PV4_B4),
            f(m(2), &BAND_PREVIEW[2].0, &PV4_B5),
        ]);
    }
    (preview, detail)
}

/// Percentile ranks 0, 5, ..., 100 % on a 0-127 scale.
const RANKS: [u8; 21] = [0, 6, 13, 19, 25, 32, 38, 44, 51, 57, 64, 70, 76, 83, 89, 95, 102, 108, 114, 121, 127];

/// PWV4 byte 1 from the level's rank (0-127): rekordbox's falling curve.
fn pv4_b1(rank: f64) -> u8 {
    let k = ((rank / 127.0) * 20.0).clamp(0.0, 20.0);
    let (i, t) = (k.floor() as usize, k.fract());
    let (a, b) = (PV4_B1[i] as f64, PV4_B1[(i + 1).min(20)] as f64);
    (a + (b - a) * t).round() as u8
}

/// Columns in a 3-band preview (PWV6), as rekordbox writes it.
pub const BAND_PREVIEW_COLUMNS: usize = 1200;

/// The two waveform blobs a player asks for, from a deck-style waveform
/// cache.  Column c covers `hop_size` frames; amplitude and the high band
/// drive height and whiteness the way a player's blue waveform reads, scaled
/// so the track's loudest column fills the 31 px a player draws.
pub fn waveform_blobs(wave: &WaveformCache, frames: u64, sr: u32) -> (Vec<u8>, Vec<u8>) {
    let cols = &wave.columns;
    if cols.is_empty() || frames == 0 { return (Vec::new(), Vec::new()); }
    let hop = wave.hop_size.max(1) as u64;
    let col_at = |frame: u64| ((frame / hop) as usize).min(cols.len() - 1);
    let max_amp  = cols.iter().map(|c| c[3] as u32).max().unwrap_or(0).max(1);
    let max_high = cols.iter().map(|c| c[2] as u32).max().unwrap_or(0).max(1);
    let pack = |c: &[u8; 4]| -> (u8, u8) { ((c[3] as u32 * 31 / max_amp) as u8, (c[2] as u32 * 7 / max_high) as u8) };
    let mut preview = Vec::with_capacity(PREVIEW_COLUMNS * 2);
    for i in 0..PREVIEW_COLUMNS {
        let (a, b) = (col_at(frames * i as u64 / PREVIEW_COLUMNS as u64), col_at(frames * (i as u64 + 1) / PREVIEW_COLUMNS as u64));
        let (mut h, mut w) = (0u8, 0u8);
        for c in &cols[a..=b.max(a)] { let (ch, cw) = pack(c); h = h.max(ch); w = w.max(cw); }
        preview.push(h); preview.push(w);
    }
    let half_frames = (frames * DETAIL_PER_SEC as u64 / sr as u64) as usize;
    let mut detail = Vec::with_capacity(half_frames);
    for j in 0..half_frames {
        let (h, w) = pack(&cols[col_at(j as u64 * sr as u64 / DETAIL_PER_SEC as u64)]);
        detail.push((w << 5) | h);
    }
    (preview, detail)
}

/// Expand a constant grid into the per-beat list a `0x4602` blob carries:
/// from the anchor to the end of the track, beat-in-bar 1–4.
pub fn grid_beats(anchor: u64, bpm: f64, downbeat_offset: u8, sr: u32, frames: u64) -> Vec<(u8, f32, u32)> {
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
// 2: MP3s decode with the encoder delay kept (the rekordbox / player timeline).
// 3: tempo + phase fitted together, grids on rekordbox's timing.
// 4: 3-band waveforms, 5: RGB waveforms, 6: phrases, 7: detected keys for untagged tracks.
// 8: grids of 88.2 kHz+ files analysed at ≤48 kHz (were ~4 ms early).
const CACHE_VERSION: u16 = 8;

fn cache_key(path: &Path) -> Option<String> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    // The same file reached by a relative and an absolute path (the startup
    // argument vs a browser LOAD) must hit the same entry: on a Pi Zero a
    // miss is half a minute of re-analysis.
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in canonical.to_string_lossy().bytes().chain(md.len().to_le_bytes()).chain(mtime.to_le_bytes()) {
        h ^= b as u64; h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Some(format!("{h:016x}.v{CACHE_VERSION}"))
}

/// The cached analysis of `path`, if the file is unchanged since it was written.
pub fn load_cached(cache_dir: &Path, path: &Path) -> Option<Analysis> {
    let key = cache_key(path)?;
    decode(&std::fs::read(cache_dir.join(key)).ok()?)
}

/// Write `a` as the cached analysis of `path` (as it is now).
pub fn store_cached(cache_dir: &Path, path: &Path, a: &Analysis) -> Result<()> {
    let key = cache_key(path).context("stat for cache key")?;
    std::fs::create_dir_all(cache_dir)?;
    std::fs::write(cache_dir.join(key), encode(a))?;
    Ok(())
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
    put_bytes(&mut out, &a.band_preview);
    put_bytes(&mut out, &a.band_detail);
    put_bytes(&mut out, &a.rgb_preview);
    put_bytes(&mut out, &a.rgb_detail);
    put_bytes(&mut out, &a.phrases);
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
    a.band_preview = r.bytes()?; a.band_detail = r.bytes()?;
    a.rgb_preview = r.bytes()?; a.rgb_detail = r.bytes()?;
    a.phrases = r.bytes()?;
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
            let from_cache = cache_dir.as_ref().and_then(|d| load_cached(d, path));
            let a = match from_cache {
                Some(a) => { cached += 1; a }
                None => match analyze(path) {
                    Ok(a) => {
                        fresh += 1;
                        if let Some(d) = &cache_dir { if let Err(e) = store_cached(d, path, &a) { log::warn!("media: cache {}: {e:#}", path.display()); } }
                        log::info!("media: analysed [{id}] {:?}: {}:{:02} {:.1} BPM, {} beats{}", if a.title.is_empty() { path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { a.title.clone() },
                                   a.duration_s / 60, a.duration_s % 60, a.bpm, a.beats.len(), if a.has_art { ", art" } else { "" });
                        std::thread::sleep(PAUSE_BETWEEN);
                        a
                    }
                    Err(e) => { failed += 1; log::warn!("media: [{id}] {}: {e:#}", path.display()); continue; }
                },
            };
            // A row the deck already filled (it loaded this file before the
            // thread got here) keeps the deck's analysis and grid.
            if let Ok(mut l) = lib.write() { if let Some(t) = l.track_mut(*id) { if t.beats.is_empty() && t.detail.is_empty() { a.apply(t); } } }
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
            band_preview: vec![5; 6], band_detail: vec![7; 9], rgb_preview: vec![1; 12], rgb_detail: vec![2; 4], phrases: vec![3; 32],
        };
        assert_eq!(decode(&encode(&a)), Some(a.clone()));
        let mut short = encode(&a); short.truncate(20);
        assert_eq!(decode(&short), None);
        assert_eq!(decode(b"nope"), None);
    }

    #[test]
    fn three_band_layout_order_and_scale() {
        // 10 s at 44.1 kHz, hop 512: bass only for the first half, highs only after.
        let sr = 44_100u32;
        let frames = 10 * sr as u64;
        let n = (frames / 512) as usize;
        let columns = (0..n).map(|i| if i < n / 2 { [200u8, 0, 0, 0] } else { [0u8, 0, 40, 0] }).collect();
        let wave = WaveformCache { columns, sample_rate: sr, hop_size: 512 };
        let (pre, det) = three_band(&wave, frames, sr);
        assert_eq!(pre.len(), 1200 * 3, "PWV6: 1,200 columns of low, mid, high");
        assert_eq!(det.len(), 1500 * 3, "PWV7: 150 entries a second");
        // Early: low only.  Late: high only.  Bytes are low, mid, high.
        assert!(det[300] > 60 && det[301] == 0 && det[302] == 0, "{:?}", &det[300..303]);
        let late = 1400 * 3;
        assert!(det[late] == 0 && det[late + 1] == 0 && det[late + 2] > 50, "{:?}", &det[late..late + 3]);
        assert!(pre[100 * 3] > 30 && pre[1100 * 3 + 2] > 30);
        // Within rekordbox's 7-bit range.
        assert!(det.iter().chain(&pre).all(|&v| v <= 127));

        // RGB: 6-byte preview columns, 2-byte detail entries; bass reads red,
        // highs blue, and byte 1 sits in rekordbox's 128-255 band.
        let (pre, det) = rgb_waveforms(&wave, frames, sr);
        assert_eq!((pre.len(), det.len()), (1200 * 6, 1500 * 2));
        let e = |j: usize| u16::from_be_bytes([det[j * 2], det[j * 2 + 1]]);
        let rb = |v: u16| (v >> 13, (v >> 7) & 7);   // (red, blue)
        assert!(rb(e(100)).0 >= 6 && rb(e(100)).1 == 0, "bass is red: {:016b}", e(100));
        assert!(rb(e(1400)).0 == 0 && rb(e(1400)).1 >= 4, "highs are blue: {:016b}", e(1400));
        assert!(pre.chunks(6).all(|c| c[1] >= 128));
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
        // No key tag: the key comes from the audio (a 440 Hz tone over clicks).
        assert!(opendeck_analysis::Key::parse(&a.key).is_some(), "detected key {:?}", a.key);
        let mut tagged = Analysis { key: "8A".into(), ..Default::default() };
        tagged.detect_key_if_missing(&[0.1; 44_100 * 8], 2, 44_100);
        assert_eq!(tagged.key, "8A", "a tag key is kept");

        // Scan + cache: the served row picks the analysis up.
        let sc = scan(&dir, "T").unwrap();
        assert_eq!(sc.library.read().unwrap().tracks[0].title, "clicks");
        start_analysis(Arc::clone(&sc.library), sc.files.clone(), Some(dir.join("cache"))).unwrap().join().unwrap();
        let l = sc.library.read().unwrap();
        assert_eq!(l.tracks[0].duration_s, 2);
        assert_eq!(l.tracks[0].detail.len(), a.detail.len());
        assert_eq!(l.tracks[0].key, a.key);
        drop(l);
        assert_eq!(std::fs::read_dir(dir.join("cache")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
