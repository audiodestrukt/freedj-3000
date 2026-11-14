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
use std::sync::atomic::{AtomicI16, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use opendeck_analysis::waveform::{WaveformBuilder, WaveformColumn};

/// Stem order everywhere in the deck: drums, vocal, harmonics.
pub const ROLES: [Role; 3] = Role::STEMS;

/// A track's stems at the deck rate, aligned to its decode.
///
/// A bank can fill while it plays: a split on the device publishes an
/// empty bank and writes each finished stretch in order, from the start.
/// `ready` marks how far it has got (interleaved samples); the processor
/// plays the stems before it and the original after it.  One writer; the
/// samples are atomics so the audio thread can read them while it writes.
pub struct StemBank {
    track: Weak<Vec<f32>>,
    /// Interleaved stereo i16, same length as the track's samples.
    data: [Box<[AtomicI16]>; 3],
    ready: AtomicUsize,
    /// Frames the bank was shifted by to line up (for the log and tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub offset_frames: isize,
    /// Each stem's waveform columns (same analysis as the track's own), for
    /// the enlarged waveform while the mix is changed.  Set once the bank
    /// is complete.
    pub waves: OnceLock<[Vec<WaveformColumn>; 3]>,
}

/// i16 → atomic in place (the same layout: `collect` reuses the buffer).
fn atomics(v: Vec<i16>) -> Box<[AtomicI16]> { v.into_iter().map(AtomicI16::new).collect() }

impl StemBank {
    /// A complete bank (its waveforms come with [`finish`](Self::finish)).
    pub fn new(track: &Arc<Vec<f32>>, data: [Vec<i16>; 3], offset_frames: isize) -> StemBank {
        let ready = data.iter().map(Vec::len).min().unwrap_or(0);
        StemBank { track: Arc::downgrade(track), data: data.map(atomics), ready: AtomicUsize::new(ready), offset_frames, waves: OnceLock::new() }
    }

    /// An empty bank for `track`, to be filled with [`append`](Self::append).
    pub fn filling(track: &Arc<Vec<f32>>) -> StemBank {
        let n = track.len();
        StemBank {
            track: Arc::downgrade(track),
            data: std::array::from_fn(|_| (0..n).map(|_| AtomicI16::new(0)).collect()),
            ready: AtomicUsize::new(0), offset_frames: 0, waves: OnceLock::new(),
        }
    }

    /// Built for exactly this sample buffer (the one the deck is playing)?
    pub fn is_for(&self, track: &Arc<Vec<f32>>) -> bool {
        std::ptr::eq(self.track.as_ptr(), Arc::as_ptr(track))
    }

    pub fn len(&self) -> usize { self.data[0].len() }
    /// Samples (interleaved) written so far, from the start.
    pub fn ready(&self) -> usize { self.ready.load(Ordering::Acquire) }
    pub fn complete(&self) -> bool { self.ready() >= self.len() }
    /// Percent written, for the panel.
    pub fn percent(&self) -> u8 { (self.ready() as u64 * 100 / self.len().max(1) as u64) as u8 }

    /// Stem `s` at sample `i` (0 past the end).  Only meaningful below
    /// [`ready`](Self::ready).
    #[inline]
    pub fn sample(&self, s: usize, i: usize) -> i16 { self.data[s].get(i).map_or(0, |v| v.load(Ordering::Relaxed)) }

    /// Stem `s` as plain samples (a copy, for tests).
    #[cfg(test)]
    pub fn stem(&self, s: usize) -> Vec<i16> { self.data[s].iter().map(|v| v.load(Ordering::Relaxed)).collect() }

    /// Write the next stretch of all three stems (same length each, the
    /// part past the track's end dropped).  The single writer only.
    pub fn append(&self, block: [&[i16]; 3]) {
        let at = self.ready.load(Ordering::Relaxed);
        let n = block.iter().map(|b| b.len()).min().unwrap_or(0).min(self.len().saturating_sub(at));
        for (d, b) in self.data.iter().zip(block) {
            for (slot, v) in d[at..at + n].iter().zip(b) { slot.store(*v, Ordering::Relaxed); }
        }
        self.ready.store(at + n, Ordering::Release);
    }

    /// The writer is done: whatever was not written stays silent, and the
    /// stems' waveforms are built.
    pub fn finish(&self, sr: u32) {
        self.ready.store(self.len(), Ordering::Release);
        let _ = self.waves.set(stem_waves(self, sr));
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
        // While a split is still writing the bank: a block whose next block
        // would reach past what is written ramps back to the original, so
        // the fade happens over written stems and never steps at the edge.
        // The stems take over again once the split is a block ahead.
        let ready = bank.ready();
        let span: usize = ranges.iter().map(|r| r.len()).sum();
        let written = ranges.iter().all(|r| r.end + span <= ready) || ready >= bank.len();
        let m_target = if !written || target.iter().all(|g| *g >= 1.0) { 0.0 } else { 1.0 };
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
                    let stems: f32 = if i < ready { (0..3).map(|s| g[s] * bank.sample(s, i) as f32).sum::<f32>() / 32768.0 } else { x[i] };
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
            Some(b) if self.m > 0.0 && i < b.ready() => {
                let stems: f32 = (0..3).map(|s| self.g[s] * b.sample(s, i) as f32).sum::<f32>() / 32768.0;
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
/// `deck_sr`, stereo).  `Ok(None)`: the file has no prepared stems.  With a
/// `job`, the bank is published there as soon as it starts filling, so the
/// deck plays the stems read so far (see [`build`]).
pub fn load_local(file: &Path, track: &Arc<Vec<f32>>, deck_sr: u32, ch: usize, job: Option<&crate::separate::Job>) -> Result<Option<Arc<StemBank>>> {
    let Some((root, rel)) = find_stick(file) else { return Ok(None) };
    let mut media = oc::DirMedia::new(&root);
    let Some(index) = oc::Index::read(&mut media)? else { return Ok(None) };
    let Some(entry) = index.by_path(&rel)?.cloned() else { return Ok(None) };
    if let Some(want) = entry.source_sha256 {
        let got = sha256_file(file)?;
        ensure!(got == want, "{rel}: the stems were prepared from a different version of this file; prepare them again");
    }
    build(&mut media, &entry, track, deck_sr, ch, job).with_context(|| rel.clone()).map(Some)
}

/// Seconds of stems each step of a paged load reads, converts and appends.
const PAGED_STEP_SECS: usize = 4;
/// Source frames read either side of a step, so the rate converter has the
/// audio around it and a step's edges match a whole-role conversion.
const PAGED_MARGIN: usize = 8192;

/// Read, convert, align and shift a bank onto `track`.
///
/// Paged (`.pgz` banks, the spec's real-time reading): the page tables are
/// checked first, the offset is measured on the few full-mix pages it
/// needs, then an empty bank is published (on `job`) and filled from the
/// start a few seconds at a time, each page checked against its hash as it
/// is read.  The deck plays the stems up to where the bank has got and the
/// track itself after that, so stems are there within a second of a load
/// instead of after the whole bank.  Once every page has gone by, each
/// role's whole-PCM hash is checked too; a mismatch drops the bank.
pub fn build(media: &mut dyn oc::Media, entry: &oc::Entry, track: &Arc<Vec<f32>>, deck_sr: u32, ch: usize, job: Option<&crate::separate::Job>) -> Result<Arc<StemBank>> {
    ensure!(ch == 2, "stems need a stereo deck (deck has {ch} channels)");
    if entry.layout != oc::index::Layout::Wav {
        return build_paged(media, entry, track, deck_sr, job);
    }
    let t0 = std::time::Instant::now();
    let full = role_at(media, entry, Role::FullMix, deck_sr)?;
    let offset = align(track, &full, deck_sr)?;
    drop(full);
    let mut data: [Vec<i16>; 3] = Default::default();
    for (s, r) in ROLES.iter().enumerate() {
        data[s] = shift(&role_at(media, entry, *r, deck_sr)?, offset, track.len());
    }
    let bank = StemBank::new(track, data, offset);
    bank.finish(deck_sr);
    log::info!("stems: bank {} ready, offset {offset} frames ({:.2} ms), {:.1}s",
               entry.bundle, offset as f64 * 1e3 / deck_sr as f64, t0.elapsed().as_secs_f32());
    Ok(Arc::new(bank))
}

/// One role read page by page: pages kept while a step still needs them,
/// and the whole-role hash taken as the pages first go by, in order.
struct PagedRole {
    reader: oc::RoleReader,
    cache: std::collections::BTreeMap<usize, Vec<i16>>,
    hasher: sha2::Sha256,
    hashed: usize,
}

impl PagedRole {
    fn open(media: &mut dyn oc::Media, entry: &oc::Entry, role: Role) -> Result<PagedRole> {
        use sha2::Digest;
        Ok(PagedRole { reader: oc::RoleReader::open(media, entry, role)?, cache: Default::default(), hasher: sha2::Sha256::new(), hashed: 0 })
    }

    fn frames(&self) -> usize { self.reader.frames() }

    /// Page `i`, read and checked once.
    fn page(&mut self, media: &mut dyn oc::Media, i: usize) -> Result<&Vec<i16>> {
        use sha2::Digest;
        if !self.cache.contains_key(&i) {
            let p = self.reader.page(media, i)?;
            if i == self.hashed {
                for v in &p { self.hasher.update(v.to_le_bytes()); }
                self.hashed += 1;
            }
            self.cache.insert(i, p);
        }
        Ok(&self.cache[&i])
    }

    /// Source frames `[a, b)` (clamped to the role) as interleaved i16.
    fn source(&mut self, media: &mut dyn oc::Media, a: usize, b: usize) -> Result<Vec<i16>> {
        let pf = self.reader.page_frames(0).max(1);
        let b = b.min(self.frames());
        let mut out = Vec::with_capacity(b.saturating_sub(a) * 2);
        if a >= b { return Ok(out); }
        for i in a / pf..=(b - 1) / pf {
            let start = i * pf;
            let p = self.page(media, i)?;
            let lo = a.max(start) - start;
            let hi = (b.min(start + p.len() / 2)) - start;
            out.extend_from_slice(&p[lo * 2..hi * 2]);
        }
        Ok(out)
    }

    /// Forget pages before source frame `a`.
    fn drop_before(&mut self, a: usize) {
        let pf = self.reader.page_frames(0).max(1);
        let keep = self.cache.split_off(&(a / pf));
        self.cache = keep;
    }

    /// Read any pages not yet hashed and check the whole-role hash.
    fn finish(mut self, media: &mut dyn oc::Media) -> Result<()> {
        use sha2::Digest;
        while self.hashed < self.reader.pages.len() {
            let i = self.hashed;
            self.cache.remove(&i);
            self.page(media, i)?;
            self.cache.remove(&i);
        }
        let got: [u8; 32] = self.hasher.finalize().into();
        ensure!(got == self.reader.full_sha256(), "{}: full PCM checksum mismatch", self.reader.path);
        Ok(())
    }
}

/// Deck-rate frames `[a, b)` (may run past either end: silence there) of a
/// role stored at `from` Hz, converted to `to` Hz exactly as converting the
/// whole role would: the source span is widened by [`PAGED_MARGIN`] and
/// started on a frame that maps to a whole deck frame.
fn deck_span(role: &mut PagedRole, media: &mut dyn oc::Media, from: u32, to: u32, a: isize, b: isize) -> Result<Vec<i16>> {
    let mut out = vec![0i16; (b - a).max(0) as usize * 2];
    let total_out = match (from, to) {
        (x, y) if x == y => role.frames(),
        (96_000, 48_000) => role.frames().div_ceil(2),
        _ => (role.frames() as f64 * to as f64 / from as f64).round() as usize,
    };
    let (lo, hi) = (a.max(0) as usize, (b.max(0) as usize).min(total_out));
    if lo >= hi { return Ok(out); }
    let g = gcd(from as u64, to as u64);
    let (q, r) = ((from as u64 / g) as usize, (to as u64 / g) as usize);   // q source frames = r deck frames
    let s0 = ((lo * from as usize / to as usize).saturating_sub(PAGED_MARGIN)) / q * q;
    let s1 = (hi * from as usize).div_ceil(to as usize) + PAGED_MARGIN;
    let src = role.source(media, s0, s1)?;
    let conv: Vec<i16> = match (from, to) {
        (x, y) if x == y => src,
        (96_000, 48_000) => oc::decimate::decimate2(&src),
        _ => {
            let f: Vec<f32> = src.iter().map(|s| *s as f32 / 32768.0).collect();
            crate::audio::resample_interleaved(&f, 2, from, to)?
                .iter().map(|v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16).collect()
        }
    };
    let d0 = s0 / q * r;   // the deck frame conv[0] lands on
    for f in lo..hi {
        let k = f - d0;
        if k * 2 + 1 < conv.len() {
            let o = (f as isize - a) as usize * 2;
            out[o..o + 2].copy_from_slice(&conv[k * 2..k * 2 + 2]);
        }
    }
    Ok(out)
}

fn gcd(a: u64, b: u64) -> u64 { if b == 0 { a } else { gcd(b, a % b) } }

fn build_paged(media: &mut dyn oc::Media, entry: &oc::Entry, track: &Arc<Vec<f32>>, deck_sr: u32, job: Option<&crate::separate::Job>) -> Result<Arc<StemBank>> {
    let t0 = std::time::Instant::now();
    let from = entry.sample_rate;
    // Every table first: a bad bank is refused before anything plays.
    let mut full = PagedRole::open(media, entry, Role::FullMix)?;
    let mut roles = ROLES.map(|r| PagedRole::open(media, entry, r)).into_iter().collect::<Result<Vec<_>>>()?;
    let full_frames = match (from, deck_sr) {
        (x, y) if x == y => full.frames(),
        (96_000, 48_000) => full.frames().div_ceil(2),
        _ => (full.frames() as f64 * deck_sr as f64 / from as f64).round() as usize,
    };
    let offset = align_by(track, full_frames, |a, b| deck_span(&mut full, media, from, deck_sr, a as isize, b as isize), deck_sr)?;
    drop(full);
    let mut bank = StemBank::filling(track);
    bank.offset_frames = offset;
    let bank = Arc::new(bank);
    if let Some(j) = job { let _ = j.bank.set(Arc::clone(&bank)); }
    let frames = track.len() / 2;
    let step = deck_sr as usize * PAGED_STEP_SECS;
    let mut first = None;
    for o0 in (0..frames).step_by(step) {
        if job.is_some_and(|j| j.cancel.load(Ordering::Relaxed)) { bail!("cancelled"); }
        let o1 = (o0 + step).min(frames);
        let (a, b) = (o0 as isize + offset, o1 as isize + offset);
        let mut got: [Vec<i16>; 3] = Default::default();
        for (s, role) in roles.iter_mut().enumerate() {
            got[s] = deck_span(role, media, from, deck_sr, a, b)?;
            role.drop_before((a.max(0) as usize * from as usize / deck_sr as usize).saturating_sub(2 * PAGED_MARGIN));
        }
        bank.append([&got[0], &got[1], &got[2]]);
        if let Some(j) = job { j.progress.store((o1 as u64 * 1000 / frames.max(1) as u64) as u32, Ordering::Relaxed); }
        first.get_or_insert(t0.elapsed());
    }
    for role in roles { role.finish(media)?; }
    bank.finish(deck_sr);
    log::info!("stems: bank {} ready, offset {offset} frames ({:.2} ms); first {}s playable in {:.2}s, whole bank {:.1}s",
               entry.bundle, offset as f64 * 1e3 / deck_sr as f64, PAGED_STEP_SECS,
               first.unwrap_or_default().as_secs_f32(), t0.elapsed().as_secs_f32());
    Ok(bank)
}

/// Each stem's waveform columns, built as the track's own are (one thread
/// per stem).
pub fn stem_waves(bank: &StemBank, sr: u32) -> [Vec<WaveformColumn>; 3] {
    std::thread::scope(|sc| {
        let jobs: Vec<_> = bank.data.iter().map(|d| sc.spawn(move || {
            let mut b = WaveformBuilder::new(sr);
            let mut buf = Vec::with_capacity(8192);
            for chunk in d.chunks(8192) {
                buf.clear();
                buf.extend(chunk.iter().map(|v| v.load(Ordering::Relaxed) as f32 / 32768.0));
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
    align_by(deck, full.len() / 2, |a, b| Ok(full[a * 2..(b * 2).min(full.len())].to_vec()), sr)
}

/// [`align`] reading the full mix through `full_at(a, b)` (deck-rate frames
/// `[a, b)`, interleaved), so a paged load reads only the pages it needs.
pub fn align_by(deck: &[f32], full_frames: usize, mut full_at: impl FnMut(usize, usize) -> Result<Vec<i16>>, sr: u32) -> Result<isize> {
    let mono_d: Vec<f32> = deck.chunks_exact(2).map(|f| f[0] + f[1]).collect();
    let win = sr as usize;          // 1 s windows
    let max_lag = sr as usize / 4;  // ±250 ms
    let n = mono_d.len().min(full_frames);
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
        let mono_f: Vec<f32> = full_at(start - max_lag, start + win + max_lag)?
            .chunks_exact(2).map(|f| (f[0] as f32 + f[1] as f32) / 32768.0).collect();
        ensure!(mono_f.len() == win + 2 * max_lag, "full mix shorter than its index says");
        let b8 = dec(&mono_f);
        let coarse = (-lag8..=lag8).max_by(|x, y| ncc(&a8, &b8, x + lag8).total_cmp(&ncc(&a8, &b8, y + lag8))).unwrap() * D as isize;
        let b = &mono_f[..];
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
        let bank = StemBank::new(&t, [vec![100; 200], vec![200; 200], vec![300; 200]], 0);
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
        let bank = StemBank::new(&t, [s(8192), s(4096), s(4096)], 0); // sums to 0.5
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
    fn a_filling_bank_plays_what_is_written_and_the_original_after() {
        let n = 2000;
        let x = vec![0.5f32; 3 * n];
        let t = Arc::new(x.clone());
        let bank = StemBank::filling(&t);
        assert_eq!((bank.ready(), bank.percent(), bank.complete()), (0, 0, false));
        let (d, v, h) = (vec![8192i16; n], vec![4096i16; n], vec![4096i16; n]);  // sums to 0.5
        bank.append([&d, &v, &h]);
        assert_eq!((bank.ready(), bank.percent()), (n, 33));

        let mut m = Mixer::default();
        let mut out = Vec::new();
        let h2 = n / 2;
        // Drums muted, well inside the written stretch: the stems play.
        m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[0..h2], &mut out);
        m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[0..h2], &mut out);
        assert!(out.iter().all(|s| (s - 0.25).abs() < 0.001));
        // The block before the unwritten part ramps back to the original,
        // over written stems: no step.
        assert!(m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[h2..n], &mut out));
        assert!(out.windows(2).all(|w| (w[0] - w[1]).abs() < 0.001), "a ramp, not a step");
        assert!((out[h2 - 1] - 0.5).abs() < 0.001);
        // Past it: the original, untouched.
        assert!(!m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[n..n + h2], &mut out));
        assert_eq!(m.at(&x, Some(&bank), n + 5), 0.5);
        // Once the split is a block ahead, the stems ramp back in there.
        bank.append([&d, &v, &h]);
        assert!(m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[n..n + h2], &mut out));
        assert!(out.windows(2).all(|w| (w[0] - w[1]).abs() < 0.001), "a ramp, not a step");
        m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[n..n + h2], &mut out);
        assert!(out.iter().all(|s| (s - 0.25).abs() < 0.001));
        // Appending past the end is dropped; finish completes the bank.
        bank.append([&d, &v, &h]);
        bank.append([&d, &v, &h]);
        assert_eq!(bank.ready(), 3 * n);
        bank.finish(48_000);
        assert!(bank.complete() && bank.waves.get().is_some());
    }

    #[test]
    fn mixer_follows_loop_ranges() {
        let x: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let t = Arc::new(x.clone());
        let bank = StemBank::new(&t, [(0..100).map(|i| i as i16).collect(), vec![0; 100], vec![0; 100]], 0);
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
        let bank = StemBank::new(&a, Default::default(), 0);
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
        let bank = load_local(&file, &track, 48_000, 2, None).unwrap().unwrap();
        assert_eq!(bank.offset_frames, -(late as isize));
        assert!(bank.is_for(&track));
        assert_eq!(bank.len(), track.len());
        assert!(bank.complete() && bank.waves.get().is_some());
        // Stems line up with the deck: their sum matches it sample for sample.
        let mid = track.len() / 2;
        for i in mid..mid + 2000 {
            let s: i32 = (0..3).map(|k| bank.sample(k, i) as i32).sum();
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
        assert!(load_local(&file, &track, 48_000, 2, None).is_err());
        // A file not on a prepared stick has no stems.
        assert!(load_local(elsewhere.path(), &track, 48_000, 2, None).unwrap().is_none());
    }

    /// A stick bank read page by page is the bank a whole read gives: at
    /// 48 kHz (exact 2:1 from 96 kHz), at 44.1 kHz (the general converter)
    /// and from a 44.1 kHz stick; published on the job before it is done;
    /// and a damaged page still refuses it.
    #[test]
    fn a_paged_load_is_the_whole_load_and_plays_while_it_fills() {
        for (codec, src, deck_sr, exact) in [(SynthCodec::Flac96, 96_000u32, 48_000u32, true),
                                             (SynthCodec::Flac96, 96_000, 44_100, false),
                                             (SynthCodec::Flac44, 44_100, 48_000, false)] {
            let frames = src as usize * 13;
            let stems: Vec<Vec<i16>> = (1..=3).map(|s| sig(s, frames, src as f64).iter().map(|v| v / 3).collect()).collect();
            let full = sum(&stems);
            let d = tempfile::tempdir().unwrap();
            let mut roles = BTreeMap::new();
            for (r, s) in ROLES.iter().zip(&stems) { roles.insert(*r, s.clone()); }
            roles.insert(Role::FullMix, full.clone());
            let path = "/Contents/A/T.mp3";
            write_stick(d.path(), &[SynthTrack { id: 1, onelibrary_id: None, file_path: path.into(), source: b"orig".to_vec(), roles }],
                        codec, None).unwrap();
            let file = d.path().join("Contents/A/T.mp3");
            // The deck's decode: the full mix at the deck rate, 70 frames late.
            let mut media = oc::DirMedia::new(d.path());
            let entry = oc::Index::read(&mut media).unwrap().unwrap().by_path(path).unwrap().cloned().unwrap();
            let full_deck = role_at(&mut media, &entry, Role::FullMix, deck_sr).unwrap();
            let mut deck = vec![0f32; 140];
            deck.extend(full_deck.iter().map(|v| *v as f32 / 32768.0));
            let track = Arc::new(deck);
            // The whole read, as the loader did before paging.
            let offset = align(&track, &full_deck, deck_sr).unwrap();
            let whole: Vec<Vec<i16>> = ROLES.iter().map(|r| shift(&role_at(&mut media, &entry, *r, deck_sr).unwrap(), offset, track.len())).collect();

            let job = crate::separate::Job::default();
            let bank = load_local(&file, &track, deck_sr, 2, Some(&job)).unwrap().unwrap();
            assert!(job.bank.get().is_some_and(|b| Arc::ptr_eq(b, &bank)), "published on the job while filling");
            assert_eq!(job.progress.load(Ordering::Relaxed), 1000);
            assert_eq!(bank.offset_frames, offset);
            for (k, w) in whole.iter().enumerate() {
                let got = bank.stem(k);
                let worst = got.iter().zip(w).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
                assert!(if exact { worst == 0 } else { worst <= 2 }, "{codec:?} → {deck_sr}: stem {k} differs by up to {worst}");
            }

            // A damaged page: refused, though it had started playing.
            fn find(dir: &Path) -> Option<PathBuf> {
                for e in std::fs::read_dir(dir).ok()?.flatten() {
                    let p = e.path();
                    if p.is_dir() { if let Some(f) = find(&p) { return Some(f); } }
                    else if p.file_name().is_some_and(|n| n.to_string_lossy().contains("drums")) { return Some(p); }
                }
                None
            }
            let drums = find(&d.path().join("CDJMODS")).expect("drums file");
            let mut bytes = std::fs::read(&drums).unwrap();
            let n = bytes.len();
            bytes[n - 100] ^= 0x55;
            std::fs::write(&drums, bytes).unwrap();
            let job = crate::separate::Job::default();
            assert!(load_local(&file, &track, deck_sr, 2, Some(&job)).is_err(), "{codec:?}: a damaged page is refused");
            assert!(job.bank.get().is_some(), "it had been published, so the deck drops it (poll_stems)");
        }
    }

    /// A real OverCue track: how soon stems play, against the whole bank.
    /// `OPENDECK_REAL_STEMS=/run/media/$USER/CDJ2/Contents/…/track.mp3
    ///  cargo test --release -p opendeck-app real_stems_paged -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_stems_paged() {
        use opendeck_types::Decoder;
        let Some(file) = std::env::var_os("OPENDECK_REAL_STEMS") else { return };
        let file = PathBuf::from(file);
        let mut dec = opendeck_decode::symphonia_decoder::SymphoniaDecoder::open(&file).unwrap();
        let mut buf = vec![0f32; 4096 * 2];
        let mut pcm = Vec::new();
        loop { match dec.decode(&mut buf).unwrap() { 0 => break, n => pcm.extend_from_slice(&buf[..n * 2]) } }
        let sr = 48_000;
        let track = Arc::new(crate::audio::resample_interleaved(&pcm, 2, dec.sample_rate(), sr).unwrap());
        let job = Arc::new(crate::separate::Job::default());
        let (j, t, f) = (Arc::clone(&job), Arc::clone(&track), file.clone());
        let t0 = std::time::Instant::now();
        let h = std::thread::spawn(move || load_local(&f, &t, sr, 2, Some(&j)).unwrap().unwrap());
        let mut first = None;
        while !h.is_finished() {
            if first.is_none() && job.bank.get().is_some_and(|b| b.ready() >= sr as usize * 2 * 4) { first = Some(t0.elapsed()); }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let bank = h.join().unwrap();
        let first = first.unwrap_or(t0.elapsed());
        println!("{}: {:.0}s track, offset {} frames | first 4 s of stems playing after {:.2}s | whole bank {:.2}s",
                 file.file_name().unwrap().to_string_lossy(), track.len() as f64 / 2.0 / sr as f64, bank.offset_frames,
                 first.as_secs_f64(), t0.elapsed().as_secs_f64());
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
        let t = Arc::new(vec![0f32; tone.len()]);
        let bank = StemBank::new(&t, [tone.clone(), vec![0; tone.len()], tone.clone()], 0);
        let w = stem_waves(&bank, sr);
        let mut b = WaveformBuilder::new(sr);
        b.push(&tone.iter().map(|v| *v as f32 / 32768.0).collect::<Vec<_>>());
        let own = b.finish().columns;
        assert_eq!(w[0], own);
        assert_eq!(w[2], own);
        assert!(w[1].iter().all(|c| c[3] == 0), "a silent stem draws nothing");
    }

}
