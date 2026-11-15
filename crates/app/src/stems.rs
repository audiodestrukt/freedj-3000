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
/// A bank can fill while it plays, in any order: a split on the device
/// publishes an empty bank and writes each finished stretch where it
/// belongs, starting near the playhead
/// (`docs/design/stems-from-playhead.md`).  The bank is cut into pages of
/// [`PAGE`] samples. A page counts as ready once every sample in it is
/// written; the processor plays the stems on ready pages and the original
/// elsewhere.  One writer at a time, and each sample written once (writers
/// start on a page boundary and stop at the first page already written);
/// the samples are atomics so the audio thread can read them while it
/// writes.
pub struct StemBank {
    track: Weak<Vec<f32>>,
    /// Interleaved stereo i16, same length as the track's samples.
    data: [Box<[AtomicI16]>; 3],
    /// Samples written in each page.
    pages: Box<[AtomicU32]>,
    /// Samples in ready pages, for the progress readout.
    ready: AtomicUsize,
    /// Where [`append`](Self::append) writes next.
    cursor: AtomicUsize,
    /// Frames the bank was shifted by to line up (for the log and tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub offset_frames: isize,
    /// Each stem's waveform columns (same analysis as the track's own), for
    /// the enlarged waveform while the mix is changed.  Set once the bank
    /// is complete.
    pub waves: OnceLock<[Vec<WaveformColumn>; 3]>,
    /// Prepared stems (a stick bank) read page by page, rather than a split
    /// on the device: the controls count as ready while it fills (it fills
    /// in a second or two, from the top).
    pub prepared: bool,
}

/// Stereo frames per bank page (85 ms at 48 kHz).
pub const PAGE_FRAMES: usize = 4096;
/// Interleaved samples per bank page.
pub const PAGE: usize = PAGE_FRAMES * 2;

/// i16 → atomic in place (the same layout: `collect` reuses the buffer).
fn atomics(v: Vec<i16>) -> Box<[AtomicI16]> { v.into_iter().map(AtomicI16::new).collect() }

impl StemBank {
    fn with_data(track: &Arc<Vec<f32>>, data: [Box<[AtomicI16]>; 3], offset_frames: isize, prepared: bool) -> StemBank {
        let n = data[0].len();
        StemBank {
            track: Arc::downgrade(track), data,
            pages: (0..n.div_ceil(PAGE)).map(|_| AtomicU32::new(0)).collect(),
            ready: AtomicUsize::new(0), cursor: AtomicUsize::new(0),
            offset_frames, waves: OnceLock::new(), prepared,
        }
    }

    /// A complete bank (its waveforms come with [`finish`](Self::finish)).
    pub fn new(track: &Arc<Vec<f32>>, data: [Vec<i16>; 3], offset_frames: isize) -> StemBank {
        let n = data.iter().map(Vec::len).min().unwrap_or(0);
        let b = StemBank::with_data(track, data.map(|mut v| { v.truncate(n); atomics(v) }), offset_frames, true);
        b.mark_ready_from(0);
        b
    }

    /// An empty bank for `track`, to be filled with [`write`](Self::write)
    /// or [`append`](Self::append).
    pub fn filling(track: &Arc<Vec<f32>>) -> StemBank {
        let n = track.len();
        StemBank::with_data(track, std::array::from_fn(|_| (0..n).map(|_| AtomicI16::new(0)).collect()), 0, false)
    }

    /// Built for exactly this sample buffer (the one the deck is playing)?
    pub fn is_for(&self, track: &Arc<Vec<f32>>) -> bool {
        std::ptr::eq(self.track.as_ptr(), Arc::as_ptr(track))
    }

    pub fn len(&self) -> usize { self.data[0].len() }
    /// Samples (interleaved) on ready pages, wherever they are.
    pub fn ready(&self) -> usize { self.ready.load(Ordering::Acquire) }
    pub fn complete(&self) -> bool { self.ready() >= self.len() }
    /// Percent ready, for the panel.
    pub fn percent(&self) -> u8 { (self.ready() as u64 * 100 / self.len().max(1) as u64) as u8 }

    pub fn page_count(&self) -> usize { self.pages.len() }
    fn page_len(&self, p: usize) -> usize { PAGE.min(self.len() - p * PAGE) }
    /// Every sample of page `p` is written.
    #[inline]
    pub fn page_ready(&self, p: usize) -> bool {
        self.pages.get(p).is_some_and(|c| c.load(Ordering::Acquire) as usize == self.page_len(p))
    }
    /// Page `p` has some samples written (by a writer still going, or one
    /// that stopped there).
    pub fn page_started(&self, p: usize) -> bool { self.pages.get(p).is_some_and(|c| c.load(Ordering::Acquire) > 0) }
    /// The stems at interleaved sample `i` are written.
    #[inline]
    pub fn ready_at(&self, i: usize) -> bool { self.page_ready(i / PAGE) }
    /// Every sample of `r` (clipped to the bank) is written.
    pub fn range_ready(&self, r: Range<usize>) -> bool {
        let end = r.end.min(self.len());
        r.start >= end || (r.start / PAGE..=(end - 1) / PAGE).all(|p| self.page_ready(p))
    }
    /// The first page from `p` on that is not ready, wrapping round to the
    /// start; `None` once every page is.
    pub fn first_unready_from(&self, p: usize) -> Option<usize> {
        let n = self.pages.len();
        (p.min(n)..n).chain(0..p.min(n)).find(|&q| !self.page_ready(q))
    }

    /// Stem `s` at sample `i` (0 past the end).  Only meaningful where
    /// [`ready_at`](Self::ready_at).
    #[inline]
    pub fn sample(&self, s: usize, i: usize) -> i16 { self.data[s].get(i).map_or(0, |v| v.load(Ordering::Relaxed)) }

    /// Stem `s` as plain samples (a copy, for tests).
    #[cfg(test)]
    pub fn stem(&self, s: usize) -> Vec<i16> { self.data[s].iter().map(|v| v.load(Ordering::Relaxed)).collect() }

    /// Write all three stems at interleaved sample `at` (same length each,
    /// the part past the track's end dropped).  Each sample is written
    /// once; a page is ready when its last sample lands.
    pub fn write(&self, at: usize, block: [&[i16]; 3]) {
        let n = block.iter().map(|b| b.len()).min().unwrap_or(0).min(self.len().saturating_sub(at));
        if n == 0 { return; }
        for (d, b) in self.data.iter().zip(block) {
            for (slot, v) in d[at..at + n].iter().zip(b) { slot.store(*v, Ordering::Relaxed); }
        }
        let mut i = at;
        while i < at + n {
            let p = i / PAGE;
            let upto = ((p + 1) * PAGE).min(at + n);
            let full = self.page_len(p);
            let was = self.pages[p].fetch_add((upto - i) as u32, Ordering::AcqRel) as usize;
            if was < full && was + (upto - i) >= full { self.ready.fetch_add(full, Ordering::AcqRel); }
            i = upto;
        }
    }

    /// Write the next stretch after the last one (a writer going from the
    /// start to the end).
    pub fn append(&self, block: [&[i16]; 3]) {
        let at = self.cursor.load(Ordering::Relaxed);
        self.write(at, block);
        let n = block.iter().map(|b| b.len()).min().unwrap_or(0);
        self.cursor.store((at + n).min(self.len()), Ordering::Relaxed);
    }

    /// A writer stopped partway through page `p`: forget what it wrote
    /// there, so the next writer can fill the whole page.  (A page that is
    /// not ready is never played.)
    pub fn forget_partial(&self, p: usize) {
        if let Some(c) = self.pages.get(p) {
            if (c.load(Ordering::Acquire) as usize) < self.page_len(p) { c.store(0, Ordering::Release); }
        }
    }

    /// Count every page from the one holding sample `i` to the end as
    /// ready, as written so far (silence where nothing was).
    fn mark_ready_from(&self, i: usize) {
        for p in i / PAGE..self.pages.len() {
            let full = self.page_len(p);
            let was = self.pages[p].swap(full as u32, Ordering::AcqRel) as usize;
            if was < full { self.ready.fetch_add(full, Ordering::AcqRel); }
        }
    }

    /// A writer reached the track's end at sample `i`: the few samples a
    /// rate converter leaves short of it stay silent.
    pub fn end_at(&self, i: usize) { self.mark_ready_from(i.min(self.len())); }

    /// The writer is done: whatever was not written stays silent, and the
    /// stems' waveforms are built.
    pub fn finish(&self, sr: u32) {
        self.mark_ready_from(0);
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
        // would reach an unwritten page ramps back to the original, so the
        // fade happens over written stems and never steps at the edge of a
        // written stretch.  The stems take over again once the pages a
        // block ahead are written.
        let span: usize = ranges.iter().map(|r| r.len()).sum();
        let written = bank.complete() || ranges.iter().all(|r| bank.range_ready(r.start..r.end + span));
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
                    let stems: f32 = if bank.ready_at(i) { (0..3).map(|s| g[s] * bank.sample(s, i) as f32).sum::<f32>() / 32768.0 } else { x[i] };
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
            Some(b) if self.m > 0.0 && b.ready_at(i) => {
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

/// What a prepared stick needs to find one track's stems, kept per stick.
///
/// With `index-v2` (OverCue Desktop 2.2.40 and later) a track is found by
/// its rekordbox ID with one small bucket read, as OverCue's player does;
/// the IDs come from the stick's `export.pdb`, read once.  `index.json` is
/// read (once) only for a stick without index-v2, or a file the export
/// doesn't list.  It can be up to 256 MiB, so it is never read on the
/// UI thread (see [`StemMarks`]).
struct Stick {
    /// Modification times of `index.json` and `index-v2/current` when read:
    /// a stick prepared again is read again.
    stamp: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    v2: bool,
    ids: HashMap<String, u32>,
    index: OnceLock<Option<Arc<oc::Index>>>,
}

static STICKS: std::sync::Mutex<Option<HashMap<PathBuf, Arc<Stick>>>> = std::sync::Mutex::new(None);

fn stick_stamp(root: &Path) -> (Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
    let m = |p: &str| std::fs::metadata(root.join(p)).and_then(|m| m.modified()).ok();
    (m("CDJMODS/index.json"), m("CDJMODS/index-v2/current"))
}

fn stick(root: &Path) -> Arc<Stick> {
    let stamp = stick_stamp(root);
    let mut all = STICKS.lock().unwrap_or_else(|e| e.into_inner());
    let all = all.get_or_insert_with(HashMap::new);
    if let Some(s) = all.get(root).filter(|s| s.stamp == stamp) { return Arc::clone(s); }
    let v2 = match oc::v2::current(&mut oc::DirMedia::new(root)) {
        Ok(c) => c.is_some(),
        Err(e) => { log::warn!("stems: {}: {e:#}", root.display()); false }
    };
    let ids = if !v2 { HashMap::new() } else {
        match opendeck_rekordbox::read_export(root) {
            Ok(exp) => exp.tracks.iter().map(|t| (t.rel_path.clone(), t.id)).collect(),
            Err(e) => { log::info!("stems: {} has index-v2 but no readable export.pdb ({e:#}); using index.json", root.display()); HashMap::new() }
        }
    };
    let s = Arc::new(Stick { stamp, v2, ids, index: OnceLock::new() });
    all.insert(root.to_path_buf(), Arc::clone(&s));
    s
}

/// Forget every stick (one came or went).
fn forget_sticks() { *STICKS.lock().unwrap_or_else(|e| e.into_inner()) = None; }

/// The stems entry for the file at `rel` ("/Contents/…") on the stick at
/// `root`.  `Ok(None)`: no stems for it.  Blocking file reads: keep it off
/// the UI thread.
pub fn entry_for(root: &Path, rel: &str) -> Result<Option<oc::Entry>> {
    let s = stick(root);
    let mut media = oc::DirMedia::new(root);
    if s.v2 {
        if let Some(&id) = s.ids.get(rel) {
            // A damaged index-v2 fails closed (no stems), as the spec asks.
            return oc::v2::lookup(&mut media, oc::index::Namespace::DeviceLibrary, id);
        }
    }
    let index = s.index.get_or_init(|| match oc::Index::read(&mut media) {
        Ok(i) => i.map(Arc::new),
        Err(e) => { log::warn!("stems: {}: index.json: {e:#}", root.display()); None }
    });
    match index {
        Some(i) => Ok(i.by_path(rel)?.cloned()),
        None => Ok(None),
    }
}

/// A row to mark: a local file, or a track on a linked player's media by
/// its rekordbox ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MarkKey {
    File(PathBuf),
    Link { ip: std::net::Ipv4Addr, export: &'static str, id: u32 },
}

/// Which tracks have prepared stems, for the "S" mark in BROWSE lists (as
/// OverCue marks them on the unit): local files, and tracks on a linked
/// player's USB / SD (looked up over NFS by ID).  The lookups run on a
/// worker thread: the first ask answers "no" and queues it, and the mark
/// appears on a later frame once the answer is in.  The list's drawing
/// never waits on a stick or the network (OverCue lets `index.json` reach
/// 256 MiB).
#[derive(Default)]
pub struct StemMarks {
    files: HashMap<MarkKey, bool>,
    asked: std::collections::HashSet<MarkKey>,
    worker: Option<(std::sync::mpsc::Sender<MarkKey>, std::sync::mpsc::Receiver<(MarkKey, bool)>)>,
}

impl StemMarks {
    /// A local file known to have stems.  Never blocks (see [`has`](Self::has)).
    pub fn has_stems(&mut self, file: &Path) -> bool { self.has(MarkKey::File(file.to_path_buf())) }

    /// Known to have stems.  Never blocks: an unknown row is looked up in
    /// the background and reads `false` until the answer arrives.
    pub fn has(&mut self, key: MarkKey) -> bool {
        self.take_answers();
        if let Some(v) = self.files.get(&key) { return *v; }
        if self.asked.insert(key.clone()) {
            let (tx, _) = self.worker.get_or_insert_with(spawn_marks_worker);
            if tx.send(key.clone()).is_err() { self.worker = None; self.asked.remove(&key); }
        }
        false
    }

    fn take_answers(&mut self) {
        if let Some((_, rx)) = &self.worker {
            while let Ok((p, v)) = rx.try_recv() { self.files.insert(p, v); }
        }
    }

    /// A stick came or went: look everything up again.
    pub fn clear(&mut self) {
        self.files.clear();
        self.asked.clear();
        self.worker = None;  // the old worker's answers are dropped with it
        forget_sticks();
    }
}

/// A linked player's media as the marks worker keeps it: one NFS mount,
/// whether it has index-v2, and its index.json once read (without index-v2).
struct PeerMarks { media: NfsMedia, v2: bool, index: Option<Option<oc::Index>> }

impl PeerMarks {
    fn open(ip: std::net::Ipv4Addr, export: &str) -> Result<PeerMarks> {
        let mut media = NfsMedia::connect(ip, export)?;
        let v2 = oc::v2::current(&mut media)?.is_some();
        Ok(PeerMarks { media, v2, index: None })
    }
    fn has(&mut self, id: u32) -> Result<bool> {
        if self.v2 { return Ok(oc::v2::lookup(&mut self.media, oc::index::Namespace::DeviceLibrary, id)?.is_some()); }
        if self.index.is_none() {
            let bytes = oc::Media::read_all(&mut self.media, "/CDJMODS/index.json", LINK_INDEX_MAX)?;
            self.index = Some(bytes.map(|b| oc::Index::parse(&b)).transpose()?);
        }
        Ok(self.index.as_ref().unwrap().as_ref().is_some_and(|i| i.by_id(oc::index::Namespace::DeviceLibrary, id).is_some()))
    }
}

fn spawn_marks_worker() -> (std::sync::mpsc::Sender<MarkKey>, std::sync::mpsc::Receiver<(MarkKey, bool)>) {
    let (tx, jobs) = std::sync::mpsc::channel::<MarkKey>();
    let (answer, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("stem-marks".into()).spawn(move || {
        // One mount per player and export; a player that can't be reached
        // stays unmarked until the marks are cleared.
        let mut peers: HashMap<(std::net::Ipv4Addr, &'static str), Option<PeerMarks>> = HashMap::new();
        for key in jobs {
            let v = match &key {
                MarkKey::File(file) => find_stick(file).is_some_and(|(root, rel)| entry_for(&root, &rel).ok().flatten().is_some()),
                MarkKey::Link { ip, export, id } => {
                    let peer = peers.entry((*ip, *export)).or_insert_with(|| PeerMarks::open(*ip, export)
                        .map_err(|e| log::info!("stems: marks for {ip}{export}: {e:#}")).ok());
                    match peer.as_mut().map(|p| p.has(*id)) {
                        Some(Ok(v)) => v,
                        Some(Err(e)) => { log::info!("stems: mark {ip}{export} #{id}: {e:#}"); peers.remove(&(*ip, *export)); false }
                        None => false,
                    }
                }
            };
            if answer.send((key, v)).is_err() { break; }
        }
    });
    (tx, rx)
}

/// Build the bank for a local file just loaded as `track` (interleaved,
/// `deck_sr`, stereo).  `Ok(None)`: the file has no prepared stems.  With a
/// `job`, the bank is published there as soon as it starts filling, so the
/// deck plays the stems read so far (see [`build`]).
pub fn load_local(file: &Path, track: &Arc<Vec<f32>>, deck_sr: u32, ch: usize, job: Option<&crate::separate::Job>) -> Result<Option<Arc<StemBank>>> {
    let Some((root, rel)) = find_stick(file) else { return Ok(None) };
    let mut media = oc::DirMedia::new(&root);
    let Some(entry) = entry_for(&root, &rel)? else { return Ok(None) };
    if let Some(want) = entry.source_sha256 {
        let got = sha256_file(file)?;
        ensure!(got == want, "{rel}: the stems were prepared from a different version of this file; prepare them again");
    }
    build(&mut media, &entry, track, deck_sr, ch, job).with_context(|| rel.clone()).map(Some)
}

// ── Stems on a linked player's media ─────────────────────────────────────────

/// Where a track loaded over LINK lives: the player, its export (`/C/` USB,
/// `/B/` SD), the track's path on that media, and its rekordbox ID when the
/// load knew it.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkStick { pub ip: std::net::Ipv4Addr, pub export: &'static str, pub rel: String, pub id: Option<u32> }

/// Tests: the loopback NFS server's portmap port, for `NfsMedia::connect`.
#[cfg(test)]
static TEST_PORTMAP: std::sync::Mutex<Option<u16>> = std::sync::Mutex::new(None);

/// A linked player's media over NFS, as the stems reader wants it: the same
/// page tables, hashes and checks as a local stick, each page an offset read
/// (32 reads in flight) rather than the whole file.
pub struct NfsMedia { nfs: opendeck_nfs::Nfs, root: opendeck_nfs::Fh, files: HashMap<String, Option<(opendeck_nfs::Fh, u32)>> }

impl NfsMedia {
    /// Connect to `ip` (portmap on 111, else 50111) and mount `export`.
    pub fn connect(ip: std::net::Ipv4Addr, export: &str) -> Result<NfsMedia> {
        #[cfg(test)]
        if let Some(port) = *TEST_PORTMAP.lock().unwrap() { return NfsMedia::mount(opendeck_nfs::Nfs::connect_at(ip, port)?, export); }
        NfsMedia::mount(opendeck_nfs::Nfs::connect_any(ip)?, export)
    }

    fn mount(mut nfs: opendeck_nfs::Nfs, export: &str) -> Result<NfsMedia> {
        let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect() };
        let exports = nfs.exports()?;
        let want = utf16(export);
        let e = exports.iter().find(|e| **e == want).or_else(|| exports.first()).cloned().unwrap_or(want);
        let root = nfs.mount(&e)?;
        Ok(NfsMedia { nfs, root, files: HashMap::new() })
    }

    fn file(&mut self, path: &str) -> Result<Option<(opendeck_nfs::Fh, u32)>> {
        oc::media::components(path)?;
        if let Some(f) = self.files.get(path) { return Ok(*f); }
        let f = self.nfs.lookup_path_opt(&self.root, path)?;
        self.files.insert(path.to_string(), f);
        Ok(f)
    }
}

impl oc::Media for NfsMedia {
    fn size(&mut self, path: &str) -> Result<Option<u64>> { Ok(self.file(path)?.map(|(_, n)| n as u64)) }
    fn read_at(&mut self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
        let (fh, size) = self.file(path)?.with_context(|| format!("{path}: not found"))?;
        ensure!(offset + len as u64 <= size as u64, "{path}: {offset}+{len} past its {size} bytes");
        self.nfs.read_range(&fh, offset as u32, len as u32)
    }
}

/// `index.json` read whole over LINK only up to this size; a bigger one
/// (OverCue allows 256 MiB) needs index-v2 and the track's ID.
const LINK_INDEX_MAX: u64 = 32 * 1024 * 1024;

/// The stems entry for a track on linked media: by rekordbox ID through
/// index-v2 when the stick has it, else by path in `index.json`.
pub fn link_entry(media: &mut dyn oc::Media, rel: &str, id: Option<u32>) -> Result<Option<oc::Entry>> {
    if let Some(id) = id {
        if oc::v2::current(media)?.is_some() {
            return oc::v2::lookup(media, oc::index::Namespace::DeviceLibrary, id);
        }
    }
    match media.read_all("/CDJMODS/index.json", LINK_INDEX_MAX)? {
        Some(bytes) => Ok(oc::Index::parse(&bytes)?.by_path(rel)?.cloned()),
        None => Ok(None),
    }
}

/// Build the bank for a track loaded from a linked player (`Ok(None)`: its
/// media has no stems for it), reading the stick over NFS page by page as
/// a local one is read.  Works with any player whose media was prepared,
/// stock or modded: only files are read.  `source_sha256` is not checked
/// (that would mean reading the track a second time); the alignment check
/// still refuses stems made from a different file.
pub fn load_link(stick: &LinkStick, track: &Arc<Vec<f32>>, deck_sr: u32, ch: usize, job: Option<&crate::separate::Job>) -> Result<Option<Arc<StemBank>>> {
    let mut media = NfsMedia::connect(stick.ip, stick.export)?;
    load_from(&mut media, stick, track, deck_sr, ch, job)
}

fn load_from(media: &mut dyn oc::Media, stick: &LinkStick, track: &Arc<Vec<f32>>, deck_sr: u32, ch: usize, job: Option<&crate::separate::Job>) -> Result<Option<Arc<StemBank>>> {
    let t0 = std::time::Instant::now();
    let rel = if stick.rel.starts_with('/') { stick.rel.clone() } else { format!("/{}", stick.rel) };
    let Some(entry) = link_entry(media, &rel, stick.id)? else { return Ok(None) };
    log::info!("stems: {} has stems for {rel} on {} (found in {:.0} ms)", stick.ip, stick.export, t0.elapsed().as_secs_f64() * 1e3);
    build(media, &entry, track, deck_sr, ch, job).with_context(|| format!("{} {rel}", stick.ip)).map(Some)
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
    bank.prepared = true;
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
        let n = 2 * PAGE;  // whole pages: a page counts once all of it is in
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

    /// Pages written out of order (a split that started mid-track): the
    /// stems play on ready pages only, ramping at the edges; a partial page
    /// a stopped writer left is forgotten and filled again whole.
    #[test]
    fn a_bank_written_out_of_order_plays_ready_pages_only() {
        let n = 8 * PAGE;
        let x = vec![0.5f32; n];
        let bank = StemBank::filling(&Arc::new(x.clone()));
        let stems = |k: usize| (vec![8192i16; k], vec![4096i16; k], vec![4096i16; k]);
        let (d, v, h) = stems(2 * PAGE);
        bank.write(4 * PAGE, [&d, &v, &h]);                     // pages 4 and 5
        assert!(!bank.ready_at(0) && bank.ready_at(4 * PAGE) && bank.ready_at(6 * PAGE - 1) && !bank.ready_at(6 * PAGE));
        assert!(bank.range_ready(4 * PAGE..6 * PAGE) && !bank.range_ready(3 * PAGE..5 * PAGE));
        assert_eq!((bank.percent(), bank.first_unready_from(4), bank.first_unready_from(7)), (25, Some(6), Some(7)));
        // Drums muted: the original before, the stems on pages 4–5 once a
        // block ahead is ready, the original after.
        let mut m = Mixer::default();
        let mut out = Vec::new();
        let blk = PAGE / 2;
        assert!(!m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[2 * PAGE..2 * PAGE + blk], &mut out), "not ready: the original");
        m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[4 * PAGE..4 * PAGE + blk], &mut out);
        m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[4 * PAGE + blk..5 * PAGE], &mut out);
        assert!(out.iter().all(|s| (s - 0.25).abs() < 0.001), "the stems on ready pages");
        assert!(m.mix(&x, Some(&bank), [0.0, 1.0, 1.0], &[6 * PAGE - blk..6 * PAGE], &mut out));
        assert!((out[blk - 1] - 0.5).abs() < 0.001 && out.windows(2).all(|w| (w[0] - w[1]).abs() < 0.001), "ramps out before page 6");
        // A writer stopped halfway into page 0: forgotten, then written whole.
        let (d, v, h) = stems(PAGE / 2);
        bank.write(0, [&d, &v, &h]);
        assert!(!bank.ready_at(0) && bank.page_started(0));
        bank.forget_partial(0);
        assert!(!bank.page_started(0));
        let (d, v, h) = stems(PAGE);
        bank.write(0, [&d, &v, &h]);
        assert!(bank.ready_at(0) && bank.percent() == 37);
        // The end: whatever a converter left short counts as written.
        bank.end_at(7 * PAGE + 10);
        assert!(bank.ready_at(n - 1) && !bank.complete());
        bank.finish(48_000);
        assert!(bank.complete());
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

    /// The "S" mark for `file`, once the background lookup has answered.
    fn mark(marks: &mut StemMarks, file: &Path) -> bool { mark_key(marks, MarkKey::File(file.to_path_buf())) }

    fn mark_key(marks: &mut StemMarks, key: MarkKey) -> bool {
        let t0 = std::time::Instant::now();
        loop {
            let v = marks.has(key.clone());
            marks.take_answers();
            if v || marks.files.contains_key(&key) { return marks.files.get(&key).copied().unwrap_or(v); }
            assert!(t0.elapsed() < std::time::Duration::from_secs(10), "no answer for {key:?}");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// BROWSE marks a linked player's tracks "S" by rekordbox ID, over NFS:
    /// through index-v2, and through index.json on a stick without it.
    #[test]
    fn link_rows_are_marked_by_id_over_nfs() {
        let roles = || {
            let s: Vec<Vec<i16>> = (1..=3).map(|k| sig(k, 96_000, 96_000.0).iter().map(|v| v / 3).collect()).collect();
            let mut r = BTreeMap::new();
            for (role, x) in ROLES.iter().zip(&s) { r.insert(*role, x.clone()); }
            r.insert(Role::FullMix, sum(&s));
            r
        };
        let lo = std::net::Ipv4Addr::LOCALHOST;
        for v2 in [Some(2), None] {
            let d = tempfile::tempdir().unwrap();
            write_stick(d.path(), &[SynthTrack { id: 5, onelibrary_id: None, file_path: "/Contents/A.mp3".into(), source: b"a".to_vec(), roles: roles() }],
                        SynthCodec::Flac96, v2).unwrap();
            let srv = opendeck_nfs::server::NfsServer::start(opendeck_nfs::server::Tree::scan(d.path()).unwrap(), "/C/", 0, 0).unwrap();
            *TEST_PORTMAP.lock().unwrap() = Some(srv.portmap);
            let mut marks = StemMarks::default();
            assert!(!marks.has(MarkKey::Link { ip: lo, export: "/C/", id: 5 }), "the first ask answers at once");
            assert!(mark_key(&mut marks, MarkKey::Link { ip: lo, export: "/C/", id: 5 }), "prepared (index-v2: {v2:?})");
            assert!(!mark_key(&mut marks, MarkKey::Link { ip: lo, export: "/C/", id: 6 }), "another track");
        }
        *TEST_PORTMAP.lock().unwrap() = None;
    }

    /// Prepared stems on a rekordbox stick with index-v2: a track is found
    /// by its export.pdb ID in index-v2, without reading index.json (here
    /// made unreadable to prove it); a track without stems is not marked.
    /// The first S-mark ask never blocks the list: it answers at once.
    #[test]
    fn index_v2_finds_stems_by_rekordbox_id_without_index_json() {
        index_v2_case();
    }

    /// A real OverCue stick (`OPENDECK_REAL_STICK=<mount>`): every track
    /// index.json lists is found through index-v2 (by its export.pdb ID)
    /// with the same bank.
    #[test]
    fn real_stick_index_v2_agrees_with_index_json() {
        let Some(root) = std::env::var_os("OPENDECK_REAL_STICK") else { return };
        let root = PathBuf::from(root);
        let s = stick(&root);
        assert!(s.v2, "the stick has index-v2");
        let index = oc::Index::read(&mut oc::DirMedia::new(&root)).unwrap().unwrap();
        let mut n = 0;
        for e in index.device.values() {
            let path = e.file_path.as_deref().unwrap();
            assert!(s.ids.contains_key(path), "{path} in export.pdb");
            let t = std::time::Instant::now();
            let got = entry_for(&root, path).unwrap().expect("found");
            assert_eq!(got.bundle, e.bundle, "{path}");
            eprintln!("{path}: {} via index-v2 in {:.1} ms", got.bundle, t.elapsed().as_secs_f64() * 1e3);
            n += 1;
        }
        assert!(n > 0);
    }

    fn index_v2_case() {
        let d = tempfile::tempdir().unwrap();
        crate::media::tests::copy_tree(&crate::media::tests::demo_export(), d.path());
        let exp = opendeck_rekordbox::read_export(d.path()).unwrap();
        let (with, without) = (&exp.tracks[0], &exp.tracks[1]);
        let frames = 96_000 * 2;
        let stems: Vec<Vec<i16>> = (1..=3).map(|s| sig(s, frames, 96_000.0).iter().map(|v| v / 3).collect()).collect();
        let mut roles = BTreeMap::new();
        for (r, s) in ROLES.iter().zip(&stems) { roles.insert(*r, s.clone()); }
        roles.insert(Role::FullMix, sum(&stems));
        write_stick(d.path(), &[SynthTrack { id: with.id, onelibrary_id: None, file_path: with.rel_path.clone(), source: b"orig".to_vec(), roles }],
                    SynthCodec::Flac96, Some(4)).unwrap();
        let other = without.path_on(d.path());
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"plain").unwrap();
        std::fs::write(d.path().join("CDJMODS/index.json"), b"not json").unwrap();

        let file = with.path_on(d.path());
        let (root, rel) = find_stick(&file).unwrap();
        assert!(entry_for(&root, &rel).unwrap().is_some(), "found through index-v2");
        let mut marks = StemMarks::default();
        let t = std::time::Instant::now();
        assert!(!marks.has_stems(&file), "the first ask answers at once, before the lookup");
        assert!(t.elapsed() < std::time::Duration::from_millis(50));
        assert!(mark(&mut marks, &file));
        assert!(!mark(&mut marks, &other), "listed in the export, no stems");
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
        assert!(mark(&mut marks, &file));
        assert!(!mark(&mut marks, &plain));
        assert!(!mark(&mut marks, elsewhere.path()));
        assert!(marks.has_stems(&file), "cached answer");
        // A changed original (re-exported) is refused.
        std::fs::write(&file, b"another version").unwrap();
        assert!(load_local(&file, &track, 48_000, 2, None).is_err());
        // A file not on a prepared stick has no stems.
        assert!(load_local(elsewhere.path(), &track, 48_000, 2, None).unwrap().is_none());
    }

    /// Stems on a linked player's media, read over NFS (our own server on
    /// loopback standing in for the player): found by rekordbox ID through
    /// index-v2 or by path through index.json, and the bank is the one the
    /// same stick gives read locally.
    #[test]
    fn stems_load_over_link_from_a_players_media() {
        let (sr96, frames96) = (96_000.0, 96_000 * 6);
        let stems: Vec<Vec<i16>> = (1..=3).map(|s| sig(s, frames96, sr96).iter().map(|v| v / 3).collect()).collect();
        let full = sum(&stems);
        let d = tempfile::tempdir().unwrap();
        let mut roles = BTreeMap::new();
        for (r, s) in ROLES.iter().zip(&stems) { roles.insert(*r, s.clone()); }
        roles.insert(Role::FullMix, full.clone());
        let path = "/Contents/Artist/Track.mp3";
        write_stick(d.path(), &[SynthTrack { id: 7, onelibrary_id: None, file_path: path.into(), source: b"orig".to_vec(), roles }],
                    SynthCodec::Flac96, Some(3)).unwrap();
        let full48 = oc::decimate::decimate2(&full);
        let mut deck = vec![0f32; 200];
        deck.extend(full48.iter().map(|v| *v as f32 / 32768.0));
        let track = Arc::new(deck);
        let local = load_local(&d.path().join("Contents/Artist/Track.mp3"), &track, 48_000, 2, None).unwrap().unwrap();

        let srv = opendeck_nfs::server::NfsServer::start(opendeck_nfs::server::Tree::scan(d.path()).unwrap(), "/C/", 0, 0).unwrap();
        let media = || NfsMedia::mount(opendeck_nfs::Nfs::connect_at(std::net::Ipv4Addr::LOCALHOST, srv.portmap).unwrap(), "/C/").unwrap();
        let stick = |id: Option<u32>| LinkStick { ip: std::net::Ipv4Addr::LOCALHOST, export: "/C/", rel: path.trim_start_matches('/').into(), id };
        for id in [Some(7), None] {
            let bank = load_from(&mut media(), &stick(id), &track, 48_000, 2, None).unwrap().expect("stems over LINK");
            assert_eq!(bank.offset_frames, local.offset_frames, "{id:?}");
            assert!(bank.complete());
            for s in 0..3 { assert!(bank.stem(s) == local.stem(s), "stem {s} over LINK = local ({id:?})"); }
        }
        assert!(load_from(&mut media(), &stick(Some(8)), &track, 48_000, 2, None).unwrap().is_none(), "another ID: none");
        // A page range read is exactly those bytes.
        let mut m = media();
        let idx = std::fs::read(d.path().join("CDJMODS/index.json")).unwrap();
        assert_eq!(oc::Media::read_at(&mut m, "/CDJMODS/index.json", 10, 100).unwrap(), idx[10..110]);
        assert!(oc::Media::read_at(&mut m, "/CDJMODS/index.json", idx.len() as u64 - 5, 10).is_err(), "past the end");
        assert_eq!(oc::Media::size(&mut m, "/CDJMODS/missing.json").unwrap(), None);
    }

    /// A real player with a prepared stick (`OPENDECK_TEST_LINK_STEMS=<ip>`,
    /// e.g. the emulated CDJ-3000 on its private bridge): every track
    /// index.json lists is found over NFS through index-v2 by ID with the
    /// same bank, and one role reads page by page; prints the times.
    #[test]
    fn real_player_stems_over_link() {
        let Ok(ip) = std::env::var("OPENDECK_TEST_LINK_STEMS") else { return };
        let ip: std::net::Ipv4Addr = ip.parse().unwrap();
        let t = std::time::Instant::now();
        let mut m = NfsMedia::connect(ip, "/C/").unwrap();
        let index = oc::Index::parse(&oc::Media::read_all(&mut m, "/CDJMODS/index.json", LINK_INDEX_MAX).unwrap().expect("a prepared stick")).unwrap();
        eprintln!("index.json over NFS: {} tracks, {:.0} ms", index.device.len(), t.elapsed().as_secs_f64() * 1e3);
        for (id, e) in &index.device {
            let t = std::time::Instant::now();
            let got = link_entry(&mut m, e.file_path.as_deref().unwrap(), Some(*id)).unwrap().expect("found by ID");
            assert_eq!(got.bundle, e.bundle);
            eprintln!("  [{id}] {} via index-v2: {:.1} ms", got.bundle, t.elapsed().as_secs_f64() * 1e3);
        }
        let e = index.device.values().next().unwrap();
        let t = std::time::Instant::now();
        let r = oc::RoleReader::open(&mut m, e, Role::Drums).unwrap();
        let mut bytes = 0usize;
        let n = (r.frames() * 4).div_ceil(e.page_bytes as usize);
        for i in 0..n { bytes += r.page(&mut m, i).unwrap().len() * 2; }
        let s = t.elapsed().as_secs_f64();
        eprintln!("drums of [{}]: {n} pages, {:.1} MB PCM in {s:.2}s = {:.0}x real time", e.bundle, bytes as f64 / 1e6,
                  r.frames() as f64 / e.sample_rate as f64 / s);
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
