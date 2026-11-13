//! Stems in the deck: OverCue-prepared stems played through our own engine.
//! Design: `docs/design/stems-overcue.md`.
//!
//! - **Bank.** A track's three stems (drums, vocal, harmonics), converted
//!   to the deck rate and shifted onto the deck's own decode, so stem
//!   index `i` lines up with `samples[i]`.  A bank names the sample buffer
//!   it was built for, so a LOAD can never play the previous track's stems.
//! - **Mix.** The processor builds each source block as
//!   `(1-m)·original + m·Σ gᵢ·stemᵢ` *before* the stretcher, so stems cost
//!   no extra stretching.  `m` is 1 while any stem is off full and 0 when
//!   every stem is at full: then the original plays untouched (FULL MIX is
//!   the real track, not a re-sum).  Gains and `m` ramp across each block.
//! - **Load.** After a track loads, a background job finds the stick's
//!   `CDJMODS`, reads and checks the bank, and measures its offset against
//!   the deck's decode by cross-correlating OverCue's full-mix copy.  It is
//!   refused (the original keeps playing) when anything does not check out.
//! - **Waveform.** The load also builds each stem's waveform columns; the
//!   enlarged waveform then shows the stems playing at their levels
//!   ([`mix_columns`]), as OverCue's does, while the overview stays the
//!   full track.

use anyhow::{bail, ensure, Context, Result};
use arc_swap::ArcSwapOption;
use opendeck_stems::{self as oc, Role};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use opendeck_analysis::waveform::{WaveformBuilder, WaveformColumn};

/// Stem order everywhere in the deck: drums, vocal, harmonics.
pub const ROLES: [Role; 3] = Role::STEMS;

/// A track's stems at the deck rate, aligned to its decode.
pub struct StemBank {
    track: Weak<Vec<f32>>,
    /// Interleaved stereo i16, same length as the track's samples.
    pub data: [Vec<i16>; 3],
    /// Frames the bank was shifted by to line up (for the log and tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub offset_frames: isize,
    /// Each stem's waveform columns (same analysis as the track's own), for
    /// the enlarged waveform while the mix is changed.
    pub waves: [Vec<WaveformColumn>; 3],
}

impl StemBank {
    /// Built for exactly this sample buffer (the one the deck is playing)?
    pub fn is_for(&self, track: &Arc<Vec<f32>>) -> bool {
        std::ptr::eq(self.track.as_ptr(), Arc::as_ptr(track))
    }
}

/// UI ↔ processor: the current bank and the three stem levels (0..1).
pub struct StemControl {
    bank: ArcSwapOption<StemBank>,
    gains: [AtomicU32; 3],
}

impl Default for StemControl {
    fn default() -> Self {
        StemControl { bank: ArcSwapOption::empty(), gains: std::array::from_fn(|_| AtomicU32::new(1f32.to_bits())) }
    }
}

impl StemControl {
    pub fn set_bank(&self, b: Option<Arc<StemBank>>) { self.bank.store(b); }
    pub fn bank(&self) -> Option<Arc<StemBank>> { self.bank.load_full() }
    /// Stems are ready for this track (the STEMS panel greys out until then).
    pub fn ready_for(&self, track: &Arc<Vec<f32>>) -> bool { self.bank.load().as_ref().is_some_and(|b| b.is_for(track)) }
    pub fn set_gain(&self, i: usize, g: f32) { self.gains[i].store(g.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed); }
    pub fn gains(&self) -> [f32; 3] { std::array::from_fn(|i| f32::from_bits(self.gains[i].load(Ordering::Relaxed))) }
    /// Every stem at full: the original plays.
    pub fn full_mix(&self) { for i in 0..3 { self.set_gain(i, 1.0); } }
    /// A new track: no bank, full mix (as OverCue does on every load).
    pub fn reset(&self) { self.set_bank(None); self.full_mix(); }
}

/// The processor's ramp state: the gains and stem/original blend reached
/// at the end of the last block.
pub struct Mixer { g: [f32; 3], m: f32 }

impl Default for Mixer { fn default() -> Self { Mixer { g: [1.0; 3], m: 0.0 } } }

impl Mixer {
    /// Still blending toward or away from the stems.
    pub fn active(&self) -> bool { self.m > 0.0 }

    /// Build the source block for `ranges` of `x` (interleaved stereo
    /// sample ranges, in play order) into `out`.  Returns false when the
    /// original plays untouched and `out` was not written: no bank, or
    /// every stem at full with the blend already back on the original.
    pub fn mix(&mut self, x: &[f32], bank: Option<&StemBank>, target: [f32; 3], ranges: &[Range<usize>], out: &mut Vec<f32>) -> bool {
        let Some(bank) = bank else { self.m = 0.0; self.g = [1.0; 3]; return false };
        let m_target = if target.iter().all(|g| *g >= 1.0) { 0.0 } else { 1.0 };
        if self.m == 0.0 && m_target == 0.0 { self.g = target; return false; }
        let frames: usize = ranges.iter().map(|r| r.len() / 2).sum();
        out.clear();
        let (g0, m0) = (self.g, self.m);
        let mut k = 0usize;
        for r in ranges {
            for f in (r.start / 2)..(r.end / 2) {
                k += 1;
                let t = k as f32 / frames.max(1) as f32;
                let m = m0 + (m_target - m0) * t;
                let g: [f32; 3] = std::array::from_fn(|s| g0[s] + (target[s] - g0[s]) * t);
                for c in 0..2 {
                    let i = f * 2 + c;
                    let stems: f32 = (0..3).map(|s| g[s] * bank.data[s].get(i).copied().unwrap_or(0) as f32).sum::<f32>() / 32768.0;
                    out.push((1.0 - m) * x[i] + m * stems);
                }
            }
        }
        self.g = target;
        self.m = m_target;
        true
    }

    /// One sample as the mix currently sounds (no ramp): for the paused
    /// scrub, which reads at its own positions.
    pub fn at(&self, x: &[f32], bank: Option<&StemBank>, i: usize) -> f32 {
        match bank {
            Some(b) if self.m > 0.0 => {
                let stems: f32 = (0..3).map(|s| self.g[s] * b.data[s].get(i).copied().unwrap_or(0) as f32).sum::<f32>() / 32768.0;
                (1.0 - self.m) * x[i] + self.m * stems
            }
            _ => x[i],
        }
    }
}

// ── Loading ───────────────────────────────────────────────────────────────────

/// The stick a local file is on, and the file's path from the stick's root
/// ("/Contents/…"): the nearest folder above it with `CDJMODS/index.json`.
pub fn find_stick(file: &Path) -> Option<(PathBuf, String)> {
    let file = file.canonicalize().ok()?;
    for root in file.ancestors().skip(1) {
        if root.join("CDJMODS/index.json").is_file() {
            let rel = file.strip_prefix(root).ok()?;
            let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
            return Some((root.to_path_buf(), format!("/{}", parts?.join("/"))));
        }
    }
    None
}

/// Which local files have prepared stems, for the "S" mark in BROWSE lists
/// (as OverCue marks them on the unit).  Each stick's `CDJMODS/index.json`
/// is read once; each file is looked up once.
#[derive(Default)]
pub struct StemMarks {
    sticks: HashMap<PathBuf, Option<oc::Index>>,
    files:  HashMap<PathBuf, bool>,
}

impl StemMarks {
    pub fn has_stems(&mut self, file: &Path) -> bool {
        if let Some(v) = self.files.get(file) { return *v; }
        let v = find_stick(file).is_some_and(|(root, rel)| {
            let index = self.sticks.entry(root.clone()).or_insert_with(|| {
                oc::Index::read(&mut oc::DirMedia::new(&root)).ok().flatten()
            });
            index.as_ref().is_some_and(|i| i.by_path(&rel).ok().flatten().is_some())
        });
        self.files.insert(file.to_path_buf(), v);
        v
    }

    /// A stick came or went: read the indexes again.
    pub fn clear(&mut self) { self.sticks.clear(); self.files.clear(); }
}

/// Build the bank for a local file just loaded as `track` (interleaved,
/// `deck_sr`, stereo).  `Ok(None)`: the file has no prepared stems.
pub fn load_local(file: &Path, track: &Arc<Vec<f32>>, deck_sr: u32, ch: usize) -> Result<Option<StemBank>> {
    let Some((root, rel)) = find_stick(file) else { return Ok(None) };
    let mut media = oc::DirMedia::new(&root);
    let Some(index) = oc::Index::read(&mut media)? else { return Ok(None) };
    let Some(entry) = index.by_path(&rel)?.cloned() else { return Ok(None) };
    if let Some(want) = entry.source_sha256 {
        let got = sha256_file(file)?;
        ensure!(got == want, "{rel}: the stems were prepared from a different version of this file; prepare them again");
    }
    build(&mut media, &entry, track, deck_sr, ch).with_context(|| rel.clone()).map(Some)
}

/// Read, convert, align and shift a bank onto `track`.
pub fn build(media: &mut dyn oc::Media, entry: &oc::Entry, track: &Arc<Vec<f32>>, deck_sr: u32, ch: usize) -> Result<StemBank> {
    ensure!(ch == 2, "stems need a stereo deck (deck has {ch} channels)");
    let t0 = std::time::Instant::now();
    let full = role_at(media, entry, Role::FullMix, deck_sr)?;
    let offset = align(track, &full, deck_sr)?;
    drop(full);
    let mut data: [Vec<i16>; 3] = Default::default();
    for (s, r) in ROLES.iter().enumerate() {
        data[s] = shift(&role_at(media, entry, *r, deck_sr)?, offset, track.len());
    }
    let waves = stem_waves(&data, deck_sr);
    log::info!("stems: bank {} ready, offset {offset} frames ({:.2} ms), {:.1}s",
               entry.bundle, offset as f64 * 1e3 / deck_sr as f64, t0.elapsed().as_secs_f32());
    Ok(StemBank { track: Arc::downgrade(track), data, offset_frames: offset, waves })
}

/// Each stem's waveform columns, built as the track's own are (one thread
/// per stem).
pub fn stem_waves(data: &[Vec<i16>; 3], sr: u32) -> [Vec<WaveformColumn>; 3] {
    std::thread::scope(|sc| {
        let jobs: Vec<_> = data.iter().map(|d| sc.spawn(move || {
            let mut b = WaveformBuilder::new(sr);
            let mut buf = Vec::with_capacity(8192);
            for chunk in d.chunks(8192) {
                buf.clear();
                buf.extend(chunk.iter().map(|v| *v as f32 / 32768.0));
                b.push(&buf);
            }
            b.finish().columns
        })).collect();
        let mut out: [Vec<WaveformColumn>; 3] = Default::default();
        for (o, j) in out.iter_mut().zip(jobs) { *o = j.join().unwrap_or_default(); }
        out
    })
}

/// The enlarged waveform for a stem mix: `len` columns of the stems at
/// `gains`.  Each band is an energy (the analysis stores √(rms·boost) in
/// 0–255), and the energies of separate stems add, so a band of the mix is
/// √Σ(gᵢ·eᵢ)², stored back the same way.  One stem at full gives that stem's
/// columns exactly; every stem off gives silence.
pub fn mix_columns(waves: &[Vec<WaveformColumn>; 3], gains: [f32; 3], len: usize) -> Vec<WaveformColumn> {
    (0..len).map(|c| std::array::from_fn(|k| {
        let e2: f32 = (0..3).map(|s| {
            let v = waves[s].get(c).map_or(0.0, |col| (col[k] as f32 / 255.0).powi(2));
            (gains[s].clamp(0.0, 1.0) * v).powi(2)
        }).sum();
        (e2.sqrt().sqrt().clamp(0.0, 1.0) * 255.0).round() as u8
    })).collect()
}

/// One role, checked, at the deck rate.
fn role_at(media: &mut dyn oc::Media, entry: &oc::Entry, role: Role, deck_sr: u32) -> Result<Vec<i16>> {
    let pcm = oc::RoleReader::open(media, entry, role)?.read_all(media)?;
    Ok(match (entry.sample_rate, deck_sr) {
        (a, b) if a == b => pcm,
        (96_000, 48_000) => oc::decimate::decimate2(&pcm),
        (from, to) => {
            let f: Vec<f32> = pcm.iter().map(|s| *s as f32 / 32768.0).collect();
            crate::audio::resample_interleaved(&f, 2, from, to)?
                .iter().map(|v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16).collect()
        }
    })
}

/// `out[n] = stem[n + offset]` (frames), silence outside, `len` samples.
fn shift(stem: &[i16], offset: isize, len: usize) -> Vec<i16> {
    let mut out = vec![0i16; len];
    for (n, o) in out.chunks_exact_mut(2).enumerate() {
        let src = n as isize + offset;
        if src >= 0 && (src as usize) * 2 + 1 < stem.len() {
            o.copy_from_slice(&stem[src as usize * 2..src as usize * 2 + 2]);
        }
    }
    out
}

/// How far (in frames) OverCue's full mix runs ahead of our decode:
/// `full[n + d] ≈ deck[n]`.  Measured at three points in the track; they
/// must agree and correlate strongly, else the bank is refused.
pub fn align(deck: &[f32], full: &[i16], sr: u32) -> Result<isize> {
    let mono_d: Vec<f32> = deck.chunks_exact(2).map(|f| f[0] + f[1]).collect();
    let mono_f: Vec<f32> = full.chunks_exact(2).map(|f| (f[0] as f32 + f[1] as f32) / 32768.0).collect();
    let win = sr as usize;          // 1 s windows
    let max_lag = sr as usize / 4;  // ±250 ms
    let n = mono_d.len().min(mono_f.len());
    ensure!(n > win + 2 * max_lag + 64, "track too short to align stems");
    let mut found = Vec::new();
    for frac in [0.25, 0.5, 0.75] {
        let start = ((n as f64 * frac) as usize).clamp(max_lag, n - win - max_lag);
        let a = &mono_d[start..start + win];
        if a.iter().map(|v| v * v).sum::<f32>() < 1e-3 * win as f32 { continue; } // near silence
        // Coarse: 8× box-averaged signals, then refine ±12 frames at full rate.
        const D: usize = 8;
        let dec = |s: &[f32]| -> Vec<f32> { s.chunks(D).map(|c| c.iter().sum::<f32>() / D as f32).collect() };
        let a8 = dec(a);
        let lag8 = (max_lag / D) as isize;
        let b8 = dec(&mono_f[start - max_lag..start + win + max_lag]);
        let coarse = (-lag8..=lag8).max_by(|x, y| ncc(&a8, &b8, x + lag8).total_cmp(&ncc(&a8, &b8, y + lag8))).unwrap() * D as isize;
        let b = &mono_f[start - max_lag..start + win + max_lag];
        let (best, score) = (coarse - 12..=coarse + 12)
            .filter(|l| l.unsigned_abs() <= max_lag)
            .map(|l| (l, ncc(a, b, l + max_lag as isize)))
            .max_by(|x, y| x.1.total_cmp(&y.1)).unwrap();
        log::debug!("stems: align at {:.0}% → {best} frames, ncc {score:.4}", frac * 100.0);
        if score < 0.9 { bail!("the prepared full mix does not match this track (correlation {score:.2})"); }
        found.push(best);
    }
    ensure!(!found.is_empty(), "no audible window to align stems on");
    let (lo, hi) = (*found.iter().min().unwrap(), *found.iter().max().unwrap());
    ensure!(hi - lo <= 1, "stems drift against this track ({found:?} frames)");
    found.sort();
    Ok(found[found.len() / 2])
}

/// Normalised correlation of `a` with `b[off..off + a.len()]`.
fn ncc(a: &[f32], b: &[f32], off: isize) -> f32 {
    let b = &b[off as usize..off as usize + a.len()];
    let (mut ab, mut aa, mut bb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) { ab += (*x * *y) as f64; aa += (*x * *x) as f64; bb += (*y * *y) as f64; }
    if aa == 0.0 || bb == 0.0 { 0.0 } else { (ab / (aa * bb).sqrt()) as f32 }
}

fn sha256_file(p: &Path) -> Result<[u8; 32]> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // one-range blocks are the common case
mod tests {
    use super::*;
    use opendeck_stems::synth::{write_stick, SynthCodec, SynthTrack};
    use std::collections::BTreeMap;

    /// Music-like noise: a few tones plus a pseudo-random part, per seed.
    fn sig(seed: u32, frames: usize, sr: f64) -> Vec<i16> {
        let mut r = seed.wrapping_mul(2_654_435_761) | 1;
        (0..frames).flat_map(|i| {
            r ^= r << 13; r ^= r >> 17; r ^= r << 5;
            let t = i as f64 / sr;
            let v = 3000.0 * (std::f64::consts::TAU * (110.0 * seed as f64) * t).sin()
                + ((r % 4001) as f64 - 2000.0);
            [v as i16, (v * 0.8) as i16]
        }).collect()
    }

    fn sum(stems: &[Vec<i16>]) -> Vec<i16> {
        (0..stems[0].len()).map(|i| stems.iter().map(|s| s[i] as i32).sum::<i32>().clamp(-32768, 32767) as i16).collect()
    }

    #[test]
    fn mixer_passes_the_original_untouched_at_full() {
        let x: Vec<f32> = (0..200).map(|i| i as f32 / 1000.0).collect();
        let t = Arc::new(x.clone());
        let bank = StemBank { track: Arc::downgrade(&t), data: [vec![100; 200], vec![200; 200], vec![300; 200]], offset_frames: 0, waves: Default::default() };
        let mut m = Mixer::default();
        let mut out = Vec::new();
        assert!(!m.mix(&x, Some(&bank), [1.0; 3], &[0..200], &mut out));
        assert!(!m.mix(&x, None, [0.0; 3], &[0..200], &mut out), "no bank: original");
    }

    #[test]
    fn mixer_ramps_in_and_out_without_steps() {
        let n = 2000;
        let x = vec![0.5f32; n];
        let t = Arc::new(x.clone());
        let s = |v: i16| vec![v; n];
        let bank = StemBank { track: Arc::downgrade(&t), data: [s(8192), s(4096), s(4096)], offset_frames: 0, waves: Default::default() }; // sums to 0.5
        let mut m = Mixer::default();
        let mut out = Vec::new();
        // Mute drums: ramps from the original (0.5) to vocal+harmonics (0.25) across the block.
        assert!(m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[0..n], &mut out));
        assert!((out[0] - 0.5).abs() < 0.001 && (out[n - 1] - 0.25).abs() < 0.001);
        assert!(out.windows(2).all(|w| (w[0] - w[1]).abs() < 0.001), "a ramp, not a step");
        // Steady: stays muted.
        m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[0..n], &mut out);
        assert!(out.iter().all(|v| (v - 0.25).abs() < 0.001));
        // Back to full: ramps to the original, then the original plays untouched.
        assert!(m.mix(&x, Some(&bank), [1.0; 3], &[0..n], &mut out));
        assert!((out[n - 1] - 0.5).abs() < 0.001);
        assert!(!m.mix(&x, Some(&bank), [1.0; 3], &[0..n], &mut out));
    }

    #[test]
    fn mixer_follows_loop_ranges() {
        let x: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let t = Arc::new(x.clone());
        let bank = StemBank { track: Arc::downgrade(&t), data: [(0..100).map(|i| i as i16).collect(), vec![0; 100], vec![0; 100]], offset_frames: 0, waves: Default::default() };
        let mut m = Mixer { g: [1.0, 0.0, 0.0], m: 1.0 };
        let mut out = Vec::new();
        m.mix(&x, Some(&bank), [1.0, 0.0, 0.0], &[80..84, 10..12], &mut out);
        let got: Vec<f32> = out.iter().map(|v| v * 32768.0).collect();
        assert_eq!(got, vec![80.0, 81.0, 82.0, 83.0, 10.0, 11.0]);
    }

    #[test]
    fn a_bank_only_plays_with_its_own_track() {
        let a = Arc::new(vec![0f32; 4]);
        let b = Arc::new(vec![0f32; 4]);
        let bank = StemBank { track: Arc::downgrade(&a), data: Default::default(), offset_frames: 0, waves: Default::default() };
        assert!(bank.is_for(&a));
        assert!(!bank.is_for(&b));
    }

    #[test]
    fn align_finds_a_known_offset() {
        let sr = 48_000;
        let full = sig(3, sr as usize * 6, sr as f64);
        for d in [0isize, 1, -1, 37, 1248, -555] {
            // deck[n] = full[n + d], scaled (OverCue applies headroom).
            let deck: Vec<f32> = (0..full.len() / 2).flat_map(|n| {
                let s = n as isize + d;
                let f = |c: usize| if s >= 0 && (s as usize) < full.len() / 2 { full[s as usize * 2 + c] as f32 / 32768.0 * 1.4 } else { 0.0 };
                [f(0), f(1)]
            }).collect();
            assert_eq!(align(&deck, &full, sr).unwrap(), d);
        }
        let other = sig(7, sr as usize * 6, sr as f64);
        let deck: Vec<f32> = other.iter().map(|v| *v as f32 / 32768.0).collect();
        assert!(align(&deck, &full, sr).is_err(), "a different track is refused");
    }

    /// A stick prepared at 96 kHz; the deck decodes the original at 48 kHz
    /// a little late (an MP3 delay).  The bank must line up with the deck.
    #[test]
    fn loads_aligns_and_checks_a_local_stick() {
        let (sr96, frames96) = (96_000.0, 96_000 * 6);
        let stems: Vec<Vec<i16>> = (1..=3).map(|s| sig(s, frames96, sr96).iter().map(|v| v / 3).collect()).collect();
        let full = sum(&stems);
        let d = tempfile::tempdir().unwrap();
        let mut roles = BTreeMap::new();
        for (r, s) in ROLES.iter().zip(&stems) { roles.insert(*r, s.clone()); }
        roles.insert(Role::FullMix, full.clone());
        let original = b"original file bytes".to_vec();
        let path = "/Contents/Artist/Track.mp3";
        write_stick(d.path(), &[SynthTrack { id: 1, onelibrary_id: None, file_path: path.into(), source: original, roles }],
                    SynthCodec::Flac96, Some(2)).unwrap();
        let file = d.path().join("Contents/Artist/Track.mp3");
        assert_eq!(find_stick(&file).unwrap().1, path);

        // The deck's decode: the full mix at 48 kHz, 100 frames later.
        let full48 = oc::decimate::decimate2(&full);
        let late = 100usize;
        let mut deck = vec![0f32; late * 2];
        deck.extend(full48.iter().map(|v| *v as f32 / 32768.0));
        let track = Arc::new(deck);
        let bank = load_local(&file, &track, 48_000, 2).unwrap().unwrap();
        assert_eq!(bank.offset_frames, -(late as isize));
        assert!(bank.is_for(&track));
        assert_eq!(bank.data[0].len(), track.len());
        // Stems line up with the deck: their sum matches it sample for sample.
        let mid = track.len() / 2;
        for i in mid..mid + 2000 {
            let s: i32 = bank.data.iter().map(|b| b[i] as i32).sum();
            assert!((s as f32 / 32768.0 - track[i]).abs() < 4.0 / 32768.0, "sample {i}");
        }

        // BROWSE marks the prepared track "S"; a track beside it without
        // stems, and a file on no stick, are not marked.
        let plain = d.path().join("Contents/Artist/Other.mp3");
        std::fs::write(&plain, b"x").unwrap();
        let elsewhere = tempfile::NamedTempFile::new().unwrap();
        let mut marks = StemMarks::default();
        assert!(marks.has_stems(&file));
        assert!(!marks.has_stems(&plain));
        assert!(!marks.has_stems(elsewhere.path()));
        assert!(marks.has_stems(&file), "cached answer");
        // A changed original (re-exported) is refused.
        std::fs::write(&file, b"another version").unwrap();
        assert!(load_local(&file, &track, 48_000, 2).is_err());
        // A file not on a prepared stick has no stems.
        assert!(load_local(elsewhere.path(), &track, 48_000, 2).unwrap().is_none());
    }

    /// The enlarged waveform follows the stems: one stem alone is that
    /// stem's columns, every stem off is silence, and two equal stems are
    /// louder than either (their energies add) but not double.
    #[test]
    fn stem_waveform_follows_the_mix() {
        let waves: [Vec<WaveformColumn>; 3] = [
            vec![[200, 10, 0, 180], [0, 0, 0, 0]],
            vec![[0, 120, 90, 100], [0, 50, 50, 50]],
            vec![[0, 120, 90, 100], [7, 7, 7, 7]],
        ];
        assert_eq!(mix_columns(&waves, [1.0, 0.0, 0.0], 2), waves[0]);
        assert_eq!(mix_columns(&waves, [0.0, 1.0, 0.0], 2), waves[1]);
        assert_eq!(mix_columns(&waves, [0.0; 3], 2), vec![[0u8; 4]; 2]);
        let both = mix_columns(&waves, [0.0, 1.0, 1.0], 2)[0];
        assert!(both[3] > 100 && both[3] < 200, "two equal stems: {both:?}");
        assert_eq!(both[0], 0, "no stem has bass here");
        // Half level: the energy halves, so the stored value drops by √2.
        let half = mix_columns(&waves, [0.5, 0.0, 0.0], 2)[0];
        assert!((half[3] as f32 - 180.0 / 2f32.sqrt()).abs() <= 1.0, "{half:?}");
        // Shorter stems than the track pad with silence.
        assert_eq!(mix_columns(&waves, [1.0, 0.0, 0.0], 3)[2], [0; 4]);
    }

    /// A bank's stem waveforms come out the same length as the track's own
    /// (the same analysis over the same number of frames).
    #[test]
    fn stem_waves_match_the_track_columns() {
        let sr = 48_000;
        let tone: Vec<i16> = (0..sr as usize * 2).flat_map(|n| {
            let v = ((n as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() * 8000.0) as i16;
            [v, v]
        }).collect();
        let data = [tone.clone(), vec![0; tone.len()], tone.clone()];
        let w = stem_waves(&data, sr);
        let mut b = WaveformBuilder::new(sr);
        b.push(&tone.iter().map(|v| *v as f32 / 32768.0).collect::<Vec<_>>());
        let own = b.finish().columns;
        assert_eq!(w[0], own);
        assert_eq!(w[2], own);
        assert!(w[1].iter().all(|c| c[3] == 0), "a silent stem draws nothing");
    }

}
