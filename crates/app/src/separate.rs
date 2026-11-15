//! Split a track into stems on the device, with a separation model
//! (HTDemucs), when it has no OverCue-prepared stems.
//! Design: `docs/design/stems-on-device.md`.
//!
//! The model takes fixed chunks of stereo 44.1 kHz audio and returns four
//! stems: drums, bass, other, vocals.  This module does everything around
//! it, as Demucs's own `apply_model` does:
//!
//! - **Normalise** the mix by its mean and standard deviation (of the mono
//!   sum, over the whole track), and undo it on the way out.
//! - **Overlap the chunks** by a quarter and crossfade them with Demucs's
//!   triangular weights, so chunk edges do not click.
//! - **Fold the four stems** into the deck's three: drums, vocal, and
//!   harmonics = bass + other (OverCue's split).
//! - **Convert rates** both ways, so the bank lines up with the deck's
//!   own decode sample for sample.
//!
//! Output is streamed: a stretch of audio is finished as soon as no later
//! chunk can touch it, so only one chunk's worth of 44.1 kHz stems is held,
//! not the whole track's.
//!
//! The model itself is behind [`Separator`]; on iOS it is Core ML
//! ([`coreml`]).  Tests drive the pipeline with a known fake split.

#![cfg_attr(not(any(target_os = "ios", feature = "separate-onnx")), allow(dead_code))]

use anyhow::{bail, ensure, Result};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The rate the model was trained at.
pub const MODEL_SR: u32 = 44_100;
/// Demucs's default chunk overlap; `OPENDECK_SEPARATE_OVERLAP` (0.05-0.5)
/// overrides it, to measure what less overlap costs.
const OVERLAP: f64 = 0.25;

fn overlap() -> f64 {
    std::env::var("OPENDECK_SEPARATE_OVERLAP").ok().and_then(|v| v.parse::<f64>().ok())
        .filter(|v| (0.05..=0.5).contains(v)).unwrap_or(OVERLAP)
}

/// A separation model.
pub trait Separator {
    /// Frames per call (the model's fixed input length).
    fn segment(&self) -> usize;
    /// `mix`: planar stereo, `2 × segment`, normalised, 44.1 kHz.
    /// `out`: planar `8 × segment`: drums L R, bass L R, other L R, vocals L R.
    fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<()>;
}

/// Progress and cancel flags shared with the UI.
#[derive(Default)]
pub struct Job {
    /// 0–1000 (tenths of a percent).
    pub progress: AtomicU32,
    pub cancel: AtomicBool,
    /// Called once the input is ready, before the first chunk: (chunks).
    pub on_start: Option<fn(usize)>,
    /// Called after each chunk: (chunk index, chunks, seconds the model took).
    pub on_chunk: Option<fn(usize, usize, f64)>,
    /// The bank a split is filling, published once the model is loaded so
    /// the deck can play the stems already written.
    pub bank: std::sync::OnceLock<std::sync::Arc<crate::stems::StemBank>>,
    /// The deck's playhead (interleaved samples at the deck rate), so the
    /// split starts just ahead of it and follows its jumps.  `None`: split
    /// from the top.
    pub playhead: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}


/// A split refused because the device cannot give it the memory it needs
/// (it would be killed partway); the stem keys say so instead.
#[derive(Debug)]
pub struct NotEnoughMemory { pub need_mb: u64, pub have_mb: u64 }

impl std::fmt::Display for NotEnoughMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "splitting needs about {} MB free; {} MB is", self.need_mb, self.have_mb)
    }
}
impl std::error::Error for NotEnoughMemory {}

/// What a split took, for the test log.
#[derive(Debug, Default, Clone)]
pub struct Timing {
    pub chunks: usize,
    pub model_s: f64,
    pub total_s: f64,
}

/// Split `track` (interleaved stereo at `deck_sr`) into drums, vocal and
/// harmonics, written into `bank` (interleaved stereo i16 at `deck_sr`)
/// as each stretch is finished.
///
/// The split runs in **runs** of consecutive chunks
/// (`docs/design/stems-from-playhead.md`).  A run starts at the first page
/// of the bank not yet written from just ahead of the playhead (the
/// look-ahead: a chunk's compute time plus a second), goes on until it
/// reaches a written page or the track's end, and gives up early when the
/// playhead jumps somewhere it won't reach soon (a hot cue, a needle
/// search).  The next run starts again from the playhead, wrapping round to
/// the top, until every page is written.  Without a playhead
/// ([`Job::playhead`]) it is one run from the top: the classic split.
///
/// Inside a run the chunks overlap and crossfade as Demucs's own does, on
/// a fixed grid, so a run from the top is the whole-track split.  The
/// first chunk of a run that starts mid-track has no earlier chunk to
/// crossfade with; its first quarter comes from it alone.
pub fn separate(track: &[f32], deck_sr: u32, sep: &mut dyn Separator, job: &Job, bank: &crate::stems::StemBank) -> Result<Timing> {
    let t0 = std::time::Instant::now();
    let frames = track.len() / 2;
    ensure!(frames > 0, "empty track");
    let seg = sep.segment();
    ensure!(seg >= 4, "model segment too short ({seg})");

    // Demucs normalises by the mono sum's statistics (of the whole track).
    let (mean, std) = {
        let n = frames as f64;
        let mut s = 0.0f64;
        let mut s2 = 0.0f64;
        for f in track.chunks_exact(2) {
            let m = (f[0] + f[1]) as f64 * 0.5;
            s += m;
            s2 += m * m;
        }
        let mean = s / n;
        let var = (s2 / n - mean * mean).max(0.0);
        (mean as f32, (var.sqrt() as f32).max(1e-8))
    };

    let geo = Geometry::new(deck_sr, frames, seg);
    // Demucs's crossfade: 1, 2, … up to the middle, then back down; never 0.
    let weight: Vec<f32> = {
        let half = seg / 2;
        let w: Vec<f32> = (1..=half).chain((1..=seg - half).rev()).map(|v| v as f32).collect();
        let max = w.iter().cloned().fold(0.0, f32::max);
        w.into_iter().map(|v| v / max).collect()
    };
    let mut timing = Timing { ..Default::default() };
    if let Some(f) = job.on_start { f(geo.chunks); }
    let mut st = RunState { input: vec![0f32; 2 * seg], output: vec![0f32; 8 * seg], chunk_s: None };

    // Where the last run was aimed, and the playhead then.  While the
    // playhead stays put the next run aims at the same place: a look-ahead
    // re-measured from each chunk's time wobbles, and every wobble back
    // left a sliver unsplit that cost a run (and its run-in) of its own.
    let mut aimed: Option<(usize, usize)> = None;
    loop {
        if job.cancel.load(Ordering::Relaxed) { bail!("cancelled"); }
        let ph = job.playhead.as_ref().map(|p| p.load(Ordering::Relaxed) as usize / 2);
        let target = match (aimed, ph) {
            (Some((p0, t0)), Some(p)) if p.abs_diff(p0) < deck_sr as usize / 2 => t0,
            _ => geo.target(job, st.chunk_s),
        };
        if let Some(p) = ph { aimed = Some((p, target)); }
        let Some(page) = bank.first_unready_from(target / crate::stems::PAGE_FRAMES) else { break };
        let from = page * crate::stems::PAGE_FRAMES;
        log::info!("stems: split run from {:.1}s ({}% ready, {:.1}s in)", from as f64 / deck_sr as f64, bank.percent(), t0.elapsed().as_secs_f64());
        run(track, &geo, (mean, std), &weight, sep, job, bank, &mut st, &mut timing, from)?;
        job.progress.store(bank.percent() as u32 * 10, Ordering::Relaxed);
    }
    job.progress.store(1000, Ordering::Relaxed);
    timing.total_s = t0.elapsed().as_secs_f64();
    Ok(timing)
}

/// Model frames a run computes before the first frame it writes, beyond
/// the quarter chunk it has to itself: the output converter's start-up.
const RUN_IN: usize = 4096;

/// The fixed geometry of a split: the model's chunk grid at 44.1 kHz and
/// how it maps onto the deck's frames.
struct Geometry {
    deck_sr: u32,
    /// Deck frames.
    frames: usize,
    /// Model frames.
    len: usize,
    seg: usize,
    stride: usize,
    chunks: usize,
    /// `mq` model frames are exactly `dq` deck frames.
    dq: usize,
    mq: usize,
}

impl Geometry {
    fn new(deck_sr: u32, frames: usize, seg: usize) -> Geometry {
        fn gcd(a: usize, b: usize) -> usize { if b == 0 { a } else { gcd(b, a % b) } }
        let g = gcd(deck_sr as usize, MODEL_SR as usize);
        let (dq, mq) = (deck_sr as usize / g, MODEL_SR as usize / g);
        let len = (frames as u64 * mq as u64 / dq as u64) as usize;
        let stride = (((1.0 - overlap()) * seg as f64) as usize).max(1);
        Geometry { deck_sr, frames, len, seg, stride, chunks: len.div_ceil(stride).max(1), dq, mq }
    }
    fn to_model(&self, deck: usize) -> usize { (deck as u64 * self.mq as u64 / self.dq as u64) as usize }
    fn to_deck(&self, model: usize) -> usize { (model as u64 * self.dq as u64 / self.mq as u64) as usize }

    /// Where a run should start now, in deck frames: the playhead plus the
    /// look-ahead (0 without a playhead).
    fn target(&self, job: &Job, chunk_s: Option<f64>) -> usize {
        match &job.playhead {
            None => 0,
            Some(p) => (p.load(Ordering::Relaxed) as usize / 2 + self.look(chunk_s)).min(self.frames.saturating_sub(1)),
        }
    }
    /// Deck frames the playhead moves while a chunk is computed, and a
    /// second's margin: 3 s for the first chunk, before one is timed.
    fn look(&self, chunk_s: Option<f64>) -> usize {
        ((chunk_s.unwrap_or(2.0) * 1.5 + 1.0) * self.deck_sr as f64) as usize
    }
}

struct RunState { input: Vec<f32>, output: Vec<f32>, chunk_s: Option<f64> }

/// One run from the chunk holding deck frame `from` (a page boundary):
/// writes from `from` until a written page, the track's end, or the
/// playhead jumps away.
#[allow(clippy::too_many_arguments)]
fn run(track: &[f32], geo: &Geometry, (mean, std): (f32, f32), weight: &[f32], sep: &mut dyn Separator,
       job: &Job, bank: &crate::stems::StemBank, st: &mut RunState, timing: &mut Timing, from: usize) -> Result<()> {
    use crate::stems::PAGE_FRAMES;
    let deck_sr = geo.deck_sr;
    let (seg, stride, len) = (geo.seg, geo.stride, geo.len);
    // Start a quarter chunk and a margin early: the stretch a run's first
    // chunk has to itself (no earlier chunk to crossfade with) and the
    // output converter's start-up fall before `from`, which is all that
    // gets written.
    let k0 = (geo.to_model(from).saturating_sub(seg - stride + RUN_IN) / stride).min(geo.chunks - 1);
    // Overlap-add window: frames [base, base + acc.len()/6) of the three
    // stems (interleaved d L, d R, v L, v R, h L, h R) and their weights.
    let mut acc: Vec<f32> = Vec::new();
    let mut wsum: Vec<f32> = Vec::new();
    let mut base = k0 * stride;
    // The output converter starts on a model frame that maps exactly onto
    // a deck frame, so runs land on the deck's frames without drift.
    let b0 = base.div_ceil(geo.mq) * geo.mq;
    let mut skip = b0 - base;
    let mut deck_pos = geo.to_deck(b0);
    let mut rs = if deck_sr != MODEL_SR { Some(crate::audio::StreamResampler::new(MODEL_SR, deck_sr, 6)?) } else { None };
    // The first written page at or after where this run writes: it stops
    // there (another run filled it).
    let stop_at = |pos: usize| -> usize {
        let p0 = pos.div_ceil(PAGE_FRAMES);
        (p0..bank.page_count()).find(|&p| bank.page_started(p)).map_or(geo.frames, |p| p * PAGE_FRAMES)
    };
    let stop = stop_at(from);
    let mut outs: [Vec<i16>; 3] = Default::default();
    // Stored at 1/scale (headroom: a stem can be louder than its mix).
    let q = 32768.0 / bank.scale;
    // The mix's DC goes back on harmonics, so the three stems add up to it.
    let mut write = |block: &[f32], deck_pos: &mut usize, stop: usize| {
        let n = block.len() / 6;
        let (lo, hi) = ((*deck_pos).max(from), (*deck_pos + n).min(stop));
        if lo < hi {
            for o in &mut outs { o.clear(); }
            for f in block[(lo - *deck_pos) * 6..(hi - *deck_pos) * 6].chunks_exact(6) {
                for s in 0..3 {
                    for c in 0..2 {
                        let v = f[s * 2 + c] * std + if s == 2 { mean } else { 0.0 };
                        outs[s].push((v * q).round().clamp(-32768.0, 32767.0) as i16);
                    }
                }
            }
            bank.write(lo * 2, [&outs[0], &outs[1], &outs[2]]);
        }
        *deck_pos += n;
    };

    let mut wrote_page = false;
    let playhead = || job.playhead.as_ref().map(|p| p.load(Ordering::Relaxed) as usize / 2);
    let (mut last_ph, mut last_t) = (playhead(), std::time::Instant::now());
    // Only a run that began ahead of the playhead can be overtaken; one
    // filling the stretch behind it (the top, after a wrap) is meant to be.
    let began_ahead = last_ph.is_some_and(|p| p <= from);
    for k in k0..geo.chunks {
        if job.cancel.load(Ordering::Relaxed) { bail!("cancelled"); }
        // Give way only when the playhead really moved somewhere this run
        // won't serve: it jumped (further than playing at up to 4x could
        // take it, a hot cue, a needle search) or it overtook the run (a
        // split slower than the music).  The look-ahead alone follows the
        // chunk times, so with a still playhead it wobbles by a page or two;
        // giving way to that restarted runs over and over (38 chunks for a
        // 31-chunk track, paused).  And only once this run has written a
        // page, so a restless playhead can't stall the split.
        if let (true, Some(ph)) = (k > k0 && wrote_page, playhead()) {
            let prev = last_ph.unwrap_or(ph);
            let reach = (last_t.elapsed().as_secs_f64() * 4.0 * deck_sr as f64) as usize + deck_sr as usize / 2;
            let jumped = ph + deck_sr as usize / 2 < prev || ph > prev + reach;
            let overtaken = began_ahead && ph > deck_pos.max(from) + geo.to_deck(stride);
            last_ph = Some(ph);
            last_t = std::time::Instant::now();
            if jumped || overtaken {
                let want = geo.target(job, st.chunk_s) / PAGE_FRAMES;
                if let Some(p) = bank.first_unready_from(want) {
                    let start = p * PAGE_FRAMES;
                    if start < from || start > deck_pos + geo.to_deck(stride) + geo.look(st.chunk_s) {
                        bank.forget_partial(deck_pos.max(from) / PAGE_FRAMES);
                        return Ok(());
                    }
                }
            }
        }
        let off = k * stride;
        let n = seg.min(len - off);
        mix_window(track, geo, (mean, std), off, n, &mut st.input)?;
        let tm = std::time::Instant::now();
        sep.run(&st.input, &mut st.output)?;
        let dt = tm.elapsed().as_secs_f64();
        st.chunk_s = Some(dt);
        timing.model_s += dt;
        timing.chunks += 1;
        if let Some(f) = job.on_chunk { f(k, geo.chunks, dt); }

        let need = off + n - base;
        if acc.len() < need * 6 { acc.resize(need * 6, 0.0); wsum.resize(need, 0.0); }
        let out = &st.output;
        let plane = |p: usize, i: usize| out[p * seg + i];
        for i in 0..n {
            let w = weight[i];
            let a = (off - base + i) * 6;
            for c in 0..2 {
                acc[a + c]     += w * plane(c, i);                                 // drums
                acc[a + 2 + c] += w * (plane(6 + c, i));                           // vocals
                acc[a + 4 + c] += w * (plane(2 + c, i) + plane(4 + c, i));          // bass + other
            }
            wsum[off - base + i] += w;
        }
        // Everything before the next chunk's start is final.
        let last = k + 1 >= geo.chunks;
        let done_to = if last { len } else { (k + 1) * stride };
        let ready = done_to - base;
        let mut block: Vec<f32> = acc.drain(..ready * 6).collect();
        for (f, w) in block.chunks_exact_mut(6).zip(wsum.drain(..ready)) {
            f.iter_mut().for_each(|v| *v /= w);
        }
        base = done_to;
        let s0 = skip.min(block.len() / 6);
        skip -= s0;
        let block = &block[s0 * 6..];
        let mut conv = match rs.as_mut() { Some(r) => r.push(block)?, None => block.to_vec() };
        if last { if let Some(r) = rs.as_mut() { conv.extend(r.finish()?); } }
        write(&conv, &mut deck_pos, stop);
        if deck_pos > from + PAGE_FRAMES { wrote_page = true; }
        if last {
            bank.end_at(deck_pos.max(from).min(stop) * 2);
            return Ok(());
        }
        // Up to a page another run wrote: this run is done.
        if deck_pos >= stop { return Ok(()); }
        job.progress.store(bank.percent() as u32 * 10, Ordering::Relaxed);
    }
    Ok(())
}

/// The normalised model input for model frames `[off, off + n)`, planar
/// into `input` (`2 × seg`, zero past `n`): the deck's audio around them,
/// converted to 44.1 kHz with a margin so the converter's edges fall
/// outside.
fn mix_window(track: &[f32], geo: &Geometry, (mean, std): (f32, f32), off: usize, n: usize, input: &mut [f32]) -> Result<()> {
    let seg = geo.seg;
    input.iter_mut().for_each(|v| *v = 0.0);
    let norm = |v: f32| (v - mean) / std;
    if geo.deck_sr == MODEL_SR {
        for i in 0..n {
            input[i] = norm(track[(off + i) * 2]);
            input[seg + i] = norm(track[(off + i) * 2 + 1]);
        }
        return Ok(());
    }
    const MARGIN: usize = 4096;
    // Window start on the exact grid, so model frame ↔ deck frame is exact.
    let ws = off.saturating_sub(MARGIN) / geo.mq * geo.mq;
    let we = (off + n + MARGIN).div_ceil(geo.mq) * geo.mq;
    let (d0, d1) = (geo.to_deck(ws), geo.to_deck(we));
    let mut win: Vec<f32> = Vec::with_capacity((d1 - d0) * 2);
    for f in d0..d1 {
        if f < geo.frames { win.push(norm(track[f * 2])); win.push(norm(track[f * 2 + 1])); }
        else { win.push(0.0); win.push(0.0); }
    }
    let mut rs = crate::audio::StreamResampler::new(geo.deck_sr, MODEL_SR, 2)?;
    let mut res = rs.push(&win)?;
    res.extend(rs.finish()?);
    for i in 0..n {
        let j = off - ws + i;
        if let Some(f) = res.get(j * 2..j * 2 + 2) {
            input[i] = f[0];
            input[seg + i] = f[1];
        }
    }
    Ok(())
}

/// HTDemucs through Core ML, on iOS (the shim is in `ios/freedj/main.m`).
/// The model is loaded once and kept: loading 400 MB of weights per track
/// would cost seconds.
#[cfg(target_os = "ios")]
pub mod coreml {
    use super::Separator;
    use anyhow::{bail, Result};
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::Mutex;

    extern "C" {
        fn freedj_sep_model_present() -> bool;
        fn freedj_sep_open(err: *mut c_char, errlen: usize) -> *mut c_void;
        fn freedj_sep_close(h: *mut c_void);
        fn freedj_sep_segment(h: *mut c_void) -> usize;
        fn freedj_sep_run(h: *mut c_void, mix: *const f32, out: *mut f32, n: usize, err: *mut c_char, errlen: usize) -> bool;
        fn freedj_mem_footprint() -> u64;
        fn freedj_mem_available() -> u64;
        fn freedj_device_model(buf: *mut c_char, len: usize);
        fn freedj_sep_units(buf: *mut c_char, len: usize);
    }

    /// The loaded model, shared by every split (one at a time).
    pub struct CoreMl { h: *mut c_void, seg: usize }
    unsafe impl Send for CoreMl {}

    static MODEL: Mutex<Option<CoreMl>> = Mutex::new(None);

    /// Is a model in the OpenDeck folder?  (A file check; cheap.)
    pub fn present() -> bool { unsafe { freedj_sep_model_present() } }

    fn err_text(buf: &[u8]) -> String { CStr::from_bytes_until_nul(buf).map(|c| c.to_string_lossy().into_owned()).unwrap_or_default() }

    /// Run `f` with the model, loading (and on first use compiling) it.
    /// Returns how long the load took, 0 if it was already loaded.
    pub fn with<R>(f: impl FnOnce(&mut CoreMl) -> Result<R>) -> Result<(R, f64)> {
        let mut g = MODEL.lock().unwrap_or_else(|e| e.into_inner());
        let mut load_s = 0.0;
        if g.is_none() {
            let t = std::time::Instant::now();
            let mut e = [0u8; 512];
            let h = unsafe { freedj_sep_open(e.as_mut_ptr() as *mut c_char, e.len()) };
            if h.is_null() { bail!("model: {}", err_text(&e)); }
            let seg = unsafe { freedj_sep_segment(h) };
            *g = Some(CoreMl { h, seg });
            load_s = t.elapsed().as_secs_f64();
        }
        Ok((f(g.as_mut().unwrap())?, load_s))
    }

    impl Separator for CoreMl {
        fn segment(&self) -> usize { self.seg }
        fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<()> {
            assert!(mix.len() == 2 * self.seg && out.len() == 8 * self.seg);
            let mut e = [0u8; 512];
            if !unsafe { freedj_sep_run(self.h, mix.as_ptr(), out.as_mut_ptr(), self.seg, e.as_mut_ptr() as *mut c_char, e.len()) } {
                bail!("model: {}", err_text(&e));
            }
            Ok(())
        }
    }

    /// Unload the model: it holds about 4 GB, which the deck should not
    /// carry through a set.  The next split loads it again (~4.5 s).
    pub fn release() {
        let mut g = MODEL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = g.take() { unsafe { freedj_sep_close(m.h) }; }
    }

    /// Process memory: (footprint, still available before iOS kills us), MB.
    pub fn memory_mb() -> (u64, u64) { unsafe { (freedj_mem_footprint() >> 20, freedj_mem_available() >> 20) } }

    pub fn device() -> String {
        let mut b = [0u8; 64];
        unsafe { freedj_device_model(b.as_mut_ptr() as *mut c_char, b.len()) };
        err_text(&b)
    }

    /// Where Core ML runs the model ("cpu+gpu", "cpu", "all").
    pub fn units() -> String {
        let mut b = [0u8; 32];
        unsafe { freedj_sep_units(b.as_mut_ptr() as *mut c_char, b.len()) };
        err_text(&b)
    }
}

/// Whether ONNX Runtime may fold HTDemucs's constants at load.  Folding
/// makes a chunk about a quarter faster but keeps about 3 GB of
/// pre-computed tensors resident: a 4-thread split peaks at 4.4 GB folded,
/// 1.45 GB not (same output to 1e-6).  So fold only with room to spare: a
/// 4 GB Raspberry Pi 5 cannot hold the folded model at all.
/// `OPENDECK_SEPARATE_FOLD=1` / `0` overrides; else fold when at least
/// [`FOLD_MIN_AVAILABLE_MB`] is available (unknown: don't).
pub fn fold_constants(available_mb: Option<u64>, env: Option<&str>) -> bool {
    match env.map(str::trim) {
        Some("1") => true,
        Some("0") => false,
        _ => available_mb.is_some_and(|mb| mb >= FOLD_MIN_AVAILABLE_MB),
    }
}
/// Folded peak (4.4 GB) plus the deck and the rest of the system.
pub const FOLD_MIN_AVAILABLE_MB: u64 = 7_000;

/// `MemAvailable` from `/proc/meminfo`, in MB (Linux; `None` elsewhere).
pub fn mem_available_mb() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = s.lines().find_map(|l| l.strip_prefix("MemAvailable:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kb / 1024)
}

/// HTDemucs in plain Rust (`opendeck-demucs`, cargo feature
/// `separate-native`): Meta's weights from `OPENDECK_SEPARATE_MODEL`
/// (`htdemucs.safetensors`), no ONNX Runtime. The same input and output as
/// the other separators.
#[cfg(feature = "separate-native")]
pub mod native {
    use super::Separator;
    use anyhow::{anyhow, Result};
    use std::sync::Mutex;

    pub struct Native(opendeck_demucs::HtDemucs);
    static MODEL: Mutex<Option<Native>> = Mutex::new(None);

    fn path() -> Option<std::path::PathBuf> {
        std::env::var_os("OPENDECK_SEPARATE_MODEL").map(std::path::PathBuf::from)
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "safetensors"))
    }

    pub fn present() -> bool { path().is_some() }

    pub fn with<R>(f: impl FnOnce(&mut Native) -> Result<R>) -> Result<(R, f64)> {
        let mut g = MODEL.lock().unwrap_or_else(|e| e.into_inner());
        let mut load_s = 0.0;
        if g.is_none() {
            let t = std::time::Instant::now();
            let p = path().ok_or_else(|| anyhow!("OPENDECK_SEPARATE_MODEL is not a .safetensors file"))?;
            *g = Some(Native(opendeck_demucs::HtDemucs::load(&p)?));
            load_s = t.elapsed().as_secs_f64();
            log::info!("stems: native HTDemucs, {} threads, loaded in {load_s:.1} s", rayon_threads());
        }
        Ok((f(g.as_mut().unwrap())?, load_s))
    }

    fn rayon_threads() -> usize { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) }

    impl Separator for Native {
        fn segment(&self) -> usize { opendeck_demucs::SEGMENT }
        fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<()> {
            self.0.run(mix, out);
            Ok(())
        }
    }
}

/// HTDemucs through ONNX Runtime, on the desktop (cargo feature
/// `separate-onnx`).  The model file is `OPENDECK_SEPARATE_MODEL`
/// (`htdemucs.onnx`); the runtime is the system's `libonnxruntime`, loaded
/// at run time (`ORT_DYLIB_PATH` to pick another).  Inputs and outputs are
/// the Core ML build's: `mix` [1, 2, n] → `stems` [1, 4, 2, n].
#[cfg(feature = "separate-onnx")]
pub mod onnx {
    use super::Separator;
    use anyhow::{anyhow, Context, Result};
    use std::sync::Mutex;

    pub struct Onnx { session: ort::session::Session, seg: usize, _env: ort::environment::Environment }

    static MODEL: Mutex<Option<Onnx>> = Mutex::new(None);
    /// HTDemucs's fixed chunk: 7.8 s at 44.1 kHz.
    const SEGMENT: usize = 343_980;

    fn path() -> Option<std::path::PathBuf> {
        std::env::var_os("OPENDECK_SEPARATE_MODEL").map(Into::into).filter(|p: &std::path::PathBuf| p.is_file())
    }

    pub fn present() -> bool { path().is_some() }

    pub fn with<R>(f: impl FnOnce(&mut Onnx) -> Result<R>) -> Result<(R, f64)> {
        let mut g = MODEL.lock().unwrap_or_else(|e| e.into_inner());
        let mut load_s = 0.0;
        if g.is_none() {
            let t = std::time::Instant::now();
            let p = path().ok_or_else(|| anyhow!("OPENDECK_SEPARATE_MODEL is not a file"))?;
            let lib = std::env::var("ORT_DYLIB_PATH").unwrap_or_else(|_| "libonnxruntime.so.1.23".into());
            let env = ort::init_from(&lib).map_err(|e| anyhow!("{lib}: {e}"))?.build().map_err(|e| anyhow!("{e}"))?;
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
            let avail = super::mem_available_mb();
            let fold = super::fold_constants(avail, std::env::var("OPENDECK_SEPARATE_FOLD").ok().as_deref());
            log::info!("stems: ONNX model, {threads} threads, constant folding {} ({} MB available)",
                       if fold { "on" } else { "off" }, avail.map_or("?".into(), |m| m.to_string()));
            let mut b = ort::session::Session::builder(&env).map_err(|e| anyhow!("{e}"))?
                .with_intra_threads(threads).map_err(|e| anyhow!("{e}"))?;
            if !fold { b = b.with_disabled_optimizers("ConstantFolding").map_err(|e| anyhow!("{e}"))?; }
            let session = b
                // ONNX Runtime by default keeps a growing arena and a
                // whole-graph buffer plan; both off, a 3.5-min split peaks
                // at 4.9 GB instead of 9 GB, at the same speed.
                .with_memory_pattern(false).map_err(|e| anyhow!("{e}"))?
                .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()]).map_err(|e| anyhow!("{e}"))?
                .commit_from_file(&p).map_err(|e| anyhow!("{e}")).with_context(|| p.display().to_string())?;
            *g = Some(Onnx { session, seg: SEGMENT, _env: env });
            load_s = t.elapsed().as_secs_f64();
        }
        Ok((f(g.as_mut().unwrap())?, load_s))
    }

    impl Separator for Onnx {
        fn segment(&self) -> usize { self.seg }
        fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<()> {
            let n = self.seg;
            let input = ort::value::Tensor::from_array(([1usize, 2, n], mix.to_vec().into_boxed_slice())).map_err(|e| anyhow!("{e}"))?;
            let res = self.session.run(ort::inputs!["mix" => input]).map_err(|e| anyhow!("{e}"))?;
            let (shape, data) = res["stems"].try_extract_tensor::<f32>().map_err(|e| anyhow!("{e}"))?;
            anyhow::ensure!(data.len() == 8 * n, "output shape {shape:?}");
            out.copy_from_slice(data);   // [1, 4, 2, n] is [8, n] planar
            Ok(())
        }
    }
}

/// The platform's model: Core ML on iOS; elsewhere `opendeck-demucs` when
/// built with `separate-native` and given a `.safetensors` model, else ONNX
/// Runtime with `separate-onnx`; none otherwise.
pub mod model {
    #[allow(unused_imports)]
    use super::Separator;
    use anyhow::Result;

    pub fn present() -> bool {
        #[cfg(target_os = "ios")] { return super::coreml::present(); }
        #[cfg(all(not(target_os = "ios"), feature = "separate-native"))] { if super::native::present() { return true } }
        #[cfg(all(not(target_os = "ios"), feature = "separate-onnx"))] { return super::onnx::present(); }
        #[allow(unreachable_code)] false
    }

    /// Run `f` with the model loaded; also returns the load time (0 once
    /// loaded).
    #[allow(unused_variables)]
    pub fn with<R>(f: impl FnOnce(&mut dyn Separator) -> Result<R>) -> Result<(R, f64)> {
        #[cfg(target_os = "ios")] { return super::coreml::with(|m| f(m)); }
        #[cfg(all(not(target_os = "ios"), feature = "separate-native"))] { if super::native::present() { return super::native::with(|m| f(m)) } }
        #[cfg(all(not(target_os = "ios"), feature = "separate-onnx"))] { return super::onnx::with(|m| f(m)); }
        #[allow(unreachable_code)] Err(anyhow::anyhow!("no separation model on this platform"))
    }

    /// Unload the model after a split, where it is big (iOS).
    pub fn release() {
        #[cfg(target_os = "ios")] { super::coreml::release(); }
    }

    /// Memory a split needs free before it starts, MB.  iPad Air 13", Core
    /// ML on CPU + GPU, 0.3.12 (standard memory limit): the model loaded at
    /// 893 MB, then the app was killed in the first chunk with 4226 MB
    /// still left, so a split needs more than ~4.2 GB beyond the loaded
    /// model (~0.2 GB).  With the increased limit (0.3.13) it splits.  A
    /// guess from one failure, to be replaced with a measured peak.  (Linux,
    /// ONNX Runtime on the CPU, is different: 548 → 5219 MB.)
    pub const SPLIT_NEEDS_MB: u64 = 4_600;

    /// (process memory, memory left before the OS steps in), MB.
    pub fn memory_mb() -> (u64, u64) {
        #[cfg(target_os = "ios")] { return super::coreml::memory_mb(); }
        #[allow(unreachable_code)] {
            let kb = |file: &str, key: &str| std::fs::read_to_string(file).ok().and_then(|s| s.lines()
                .find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())).unwrap_or(0);
            (kb("/proc/self/status", "VmHWM:") >> 10, kb("/proc/meminfo", "MemAvailable:") >> 10)
        }
    }

    /// Where the model runs, for the log.
    pub fn units() -> String {
        #[cfg(target_os = "ios")] { return super::coreml::units(); }
        #[allow(unreachable_code)] { "cpu".into() }
    }

    pub fn device() -> String {
        #[cfg(target_os = "ios")] { return super::coreml::device(); }
        #[allow(unreachable_code)] { format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A split through the deck's own path: into a filling bank, as the
    /// app does, then read back.
    pub(super) fn split(track: &[f32], sr: u32, sep: &mut dyn Separator, job: &Job) -> Result<([Vec<f32>; 3], Timing)> {
        let (bank, timing) = split_bank(track, sr, sep, job)?;
        Ok(([bank.stem_levels(0), bank.stem_levels(1), bank.stem_levels(2)], timing))
    }
    pub(super) fn split_bank(track: &[f32], sr: u32, sep: &mut dyn Separator, job: &Job) -> Result<(crate::stems::StemBank, Timing)> {
        let t = std::sync::Arc::new(track.to_vec());
        let bank = crate::stems::StemBank::filling_split(&t);
        let timing = separate(track, sr, sep, job, &bank)?;
        bank.finish(sr);
        Ok((bank, timing))
    }

    /// The chunks a split ran, in order, for the run tests (on_chunk runs on
    /// the split's own thread).
    thread_local! {
        static RAN: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
        static JUMP: std::cell::RefCell<Option<(usize, std::sync::Arc<std::sync::atomic::AtomicU64>, u64)>> = const { std::cell::RefCell::new(None) };
    }
    fn record(k: usize, _n: usize, _dt: f64) {
        let ran = RAN.with(|r| { r.borrow_mut().push(k); r.borrow().len() });
        JUMP.with(|j| if let Some((after, ph, to)) = j.borrow().as_ref() {
            if ran == *after { ph.store(*to, Ordering::Relaxed); }
        });
    }

    /// The split with the deck's playhead: it starts just ahead of it (the
    /// look-ahead, 4 s before a chunk is timed), runs to the end, wraps to
    /// the top and stops where it began; the bank is the classic split.
    #[test]
    fn a_split_starts_ahead_of_the_playhead_and_wraps() {
        let sr = 48_000;
        let track = tone(sr as usize * 20, sr);
        let (classic, _) = split(&track, sr, &mut Fake { seg: 10_000, calls: 0 }, &Job::default()).unwrap();
        let ph = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(2 * sr as u64 * 2));
        let job = Job { playhead: Some(ph), on_chunk: Some(record), ..Default::default() };
        RAN.with(|r| r.borrow_mut().clear());
        let (got, t) = split(&track, sr, &mut Fake { seg: 10_000, calls: 0 }, &job).unwrap();
        let ran = RAN.with(|r| r.borrow().clone());
        let stride = 7_500usize;
        let first_s = ran[0] as f64 * stride as f64 / 44_100.0;
        assert!((5.5..6.0).contains(&first_s), "first chunk at {first_s:.2}s (playhead 2 s + 4 s look-ahead, less the run-in)");
        // Two runs: from the playhead plus the (untimed, 4 s) look-ahead to
        // the end, then the top.  The playhead stays put, so the second run
        // aims where the first did and wraps to the top: no slivers.
        let starts: Vec<f64> = std::iter::once(ran[0]).chain(ran.windows(2).filter(|w| w[1] != w[0] + 1).map(|w| w[1]))
            .map(|k| k as f64 * stride as f64 / 44_100.0).collect();
        assert_eq!(starts.len(), 2, "two runs: {starts:?}");
        assert_eq!(starts[1], 0.0, "then the top: {starts:?}");
        let _ = t;
        assert!(ran.len() <= 118 + 2 * 2, "one pass plus each run's run-in ({} chunks)", ran.len());
        assert_eq!(job.progress.load(Ordering::Relaxed), 1000);
        for s in 0..3 {
            let worst = (0..track.len()).map(|i| (got[s][i] as i32 - classic[s][i] as i32).abs()).max().unwrap();
            assert!(worst <= 2, "stem {s} differs from the classic split by {worst}");
        }
    }

    /// A still playhead (paused) and chunk times that vary: the look-ahead
    /// wobbles, but the split doesn't give way to that; about one pass of
    /// chunks, not a run restarted every few chunks.
    #[test]
    fn a_still_playhead_and_uneven_chunks_dont_restart_runs() {
        struct Uneven { inner: Fake, n: usize }
        impl Separator for Uneven {
            fn segment(&self) -> usize { self.inner.seg }
            fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<()> {
                self.n += 1;
                // 0 ms and 300 ms in turn: the look-ahead moves by ~0.45 s.
                if self.n % 2 == 0 { std::thread::sleep(std::time::Duration::from_millis(300)); }
                self.inner.run(mix, out)
            }
        }
        let sr = 48_000;
        let track = tone(sr as usize * 20, sr);
        let classic = Geometry::new(sr, track.len() / 2, 40_000).chunks;
        let ph = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(sr as u64 * 2));
        let job = Job { playhead: Some(ph), on_chunk: Some(record), ..Default::default() };
        RAN.with(|r| r.borrow_mut().clear());
        let (_, t) = split(&track, sr, &mut Uneven { inner: Fake { seg: 40_000, calls: 0 }, n: 0 }, &job).unwrap();
        let runs = 1 + RAN.with(|r| r.borrow().windows(2).filter(|w| w[1] != w[0] + 1).count());
        // At most a run-in chunk or two per run beyond one pass.
        assert!(runs <= 3 && t.chunks <= classic + 2 * runs, "{} chunks in {runs} runs for a {classic}-chunk track", t.chunks);
    }

    /// A hot cue during a split: the run stops and the next starts ahead of
    /// the new playhead.
    #[test]
    fn a_split_follows_a_jump() {
        let sr = 48_000;
        let track = tone(sr as usize * 30, sr);
        let ph = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let job = Job { playhead: Some(std::sync::Arc::clone(&ph)), on_chunk: Some(record), ..Default::default() };
        RAN.with(|r| r.borrow_mut().clear());
        JUMP.with(|j| *j.borrow_mut() = Some((5, std::sync::Arc::clone(&ph), 20 * sr as u64 * 2)));
        let (got, _) = split(&track, sr, &mut Fake { seg: 10_000, calls: 0 }, &job).unwrap();
        JUMP.with(|j| *j.borrow_mut() = None);
        let ran = RAN.with(|r| r.borrow().clone());
        let secs = |k: usize| k as f64 * 7_500.0 / 44_100.0;
        assert!(secs(ran[0]) < 4.5, "first run near the start: {:.2}s", secs(ran[0]));
        let next = ran[6..].first().copied().unwrap();
        assert!((20.0..22.5).contains(&secs(next)), "after the jump to 20 s the split moves to {:.2}s", secs(next));
        let (classic, _) = split(&track, sr, &mut Fake { seg: 10_000, calls: 0 }, &Job::default()).unwrap();
        for s in 0..3 {
            let worst = (0..track.len()).map(|i| (got[s][i] as i32 - classic[s][i] as i32).abs()).max().unwrap();
            assert!(worst <= 2, "stem {s} differs from the classic split by {worst}");
        }
    }

    /// A model with a known answer: drums = ½ mix, bass = other = ¼ mix,
    /// vocals = silence, with a different short segment so chunk edges
    /// land everywhere.
    struct Fake { seg: usize, calls: usize }
    impl Separator for Fake {
        fn segment(&self) -> usize { self.seg }
        fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<()> {
            self.calls += 1;
            let s = self.seg;
            for c in 0..2 {
                for i in 0..s {
                    let x = mix[c * s + i];
                    out[c * s + i] = 0.5 * x;
                    out[(2 + c) * s + i] = 0.25 * x;
                    out[(4 + c) * s + i] = 0.25 * x;
                    out[(6 + c) * s + i] = 0.0;
                }
            }
            Ok(())
        }
    }

    fn tone(frames: usize, sr: u32) -> Vec<f32> {
        (0..frames).flat_map(|n| {
            let t = n as f32 / sr as f32;
            let l = 0.4 * (2.0 * std::f32::consts::PI * 220.0 * t).sin() + 0.05;
            let r = 0.3 * (2.0 * std::f32::consts::PI * 330.0 * t).sin() - 0.02;
            [l, r]
        }).collect()
    }

    /// The split lines up with the track sample for sample, at the deck's
    /// rate, through normalisation, overlap-add and both rate conversions.
    #[test]
    fn a_known_split_comes_back_aligned_at_the_deck_rate() {
        let sr = 48_000;
        let track = tone(sr as usize * 3 + 123, sr);
        let mut fake = Fake { seg: 10_000, calls: 0 };
        let job = Job::default();
        let ([drums, vocal, harm], t) = split(&track, sr, &mut fake, &job).unwrap();
        assert_eq!(drums.len(), track.len());
        assert_eq!(fake.calls, t.chunks);
        assert_eq!(job.progress.load(Ordering::Relaxed), 1000);
        // Away from the very ends (the rate converters' edges).
        let lo = 2_000;
        let hi = track.len() - 2_000;
        let err = |got: &[f32], want: f32| (lo..hi).map(|i| (got[i] - want * track[i]).abs()).fold(0.0, f32::max);
        // The fake splits the normalised mix, so drums miss half the DC
        // and harmonics carry it; the tone's DC is small.
        assert!(err(&drums, 0.5) < 0.02, "drums off by {}", err(&drums, 0.5));
        assert!(err(&harm, 0.5) < 0.02, "harmonics off by {}", err(&harm, 0.5));
        assert!(vocal[lo..hi].iter().all(|v| v.abs() < 700.0 / 32768.0), "vocal should be near silent");
        // No lag: the drums match the track best unshifted (a one-frame
        // slip would still pass the tolerance above).
        let lag_err = |lag: isize| (lo..hi).step_by(2).map(|i| {
            let j = (i as isize + 2 * lag) as usize;
            (drums[j] - 0.5 * track[i]).powi(2)
        }).sum::<f32>();
        let best = (-4..=4).min_by(|a, b| lag_err(*a).total_cmp(&lag_err(*b))).unwrap();
        assert_eq!(best, 0, "drums are {best} frames off the track");
        // Drums + harmonics rebuild the mix.
        let sum_err = (lo..hi).map(|i| (drums[i] + harm[i] - track[i]).abs()).fold(0.0, f32::max);
        assert!(sum_err < 0.03, "stems do not add back to the mix: {sum_err}");
    }

    #[test]
    fn a_track_shorter_than_one_chunk_splits() {
        let track = tone(3_000, 44_100);
        let mut fake = Fake { seg: 10_000, calls: 0 };
        let ([d, _, h], t) = split(&track, 44_100, &mut fake, &Job::default()).unwrap();
        assert_eq!(t.chunks, 1);
        assert_eq!(d.len(), track.len());
        assert_eq!(h.len(), track.len());
    }

    /// A separator whose drums come out louder than the mix (as real
    /// drums do: 1.21 against a 1.0 mix on techno.mp3), the rest
    /// cancelling it back down to the mix.
    struct Loud { seg: usize }
    impl Separator for Loud {
        fn segment(&self) -> usize { self.seg }
        fn run(&mut self, mix: &[f32], out: &mut [f32]) -> Result<()> {
            let s = self.seg;
            for c in 0..2 {
                for i in 0..s {
                    let x = mix[c * s + i];
                    out[c * s + i] = 1.6 * x;           // drums
                    out[(2 + c) * s + i] = -0.6 * x;    // bass
                    out[(4 + c) * s + i] = 0.0;         // other
                    out[(6 + c) * s + i] = 0.0;         // vocals
                }
            }
            Ok(())
        }
    }

    /// Drums soloed on the deck play a stem louder than its mix at its own
    /// level: the 16-bit bank used to clamp it at full scale.
    #[test]
    fn a_stem_louder_than_its_mix_plays_unclipped() {
        let sr = 44_100;
        // A mix peaking at 0.9: the drums peak at 1.44.
        let track: Vec<f32> = (0..sr as usize * 2).flat_map(|n| {
            let v = 0.9 * (2.0 * std::f32::consts::PI * 110.0 * n as f32 / sr as f32).sin();
            [v, v]
        }).collect();
        let (bank, _) = split_bank(&track, sr, &mut Loud { seg: 10_000 }, &Job::default()).unwrap();
        let mut mixer = crate::stems::Mixer::default();
        let mut out = Vec::new();
        let (a, b) = (sr as usize, sr as usize + 8192);   // a block mid-track
        // The first block fades into the stems; the second plays them.
        mixer.mix(&track, Some(&bank), [1.0, 0.0, 0.0], &[a - 8192..a, 0..0], &mut out);
        assert!(mixer.mix(&track, Some(&bank), [1.0, 0.0, 0.0], &[a..b, 0..0], &mut out));
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 1.4, "drums solo peaks at {peak}, want ~1.44 (clipped at 1.0 before)");
        let worst = out.iter().zip(&track[a..b]).map(|(o, x)| (o - 1.6 * x).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "drums solo off the 1.6x mix by {worst}");
        // All three at full rebuild the mix, through the same scale.
        let all: f32 = (a..b).map(|i| (0..3).map(|s| bank.stem_levels(s)[i]).sum::<f32>() - track[i]).map(f32::abs).fold(0.0, f32::max);
        assert!(all < 1e-3, "stems add back to the mix within {all}");
    }

    #[test]
    fn cancel_stops_a_split() {
        let track = tone(100_000, 44_100);
        let job = Job::default();
        job.cancel.store(true, Ordering::Relaxed);
        assert!(split(&track, 44_100, &mut Fake { seg: 10_000, calls: 0 }, &job).is_err());
    }

    /// A 4 GB Pi 5 (~3.5 GB available) loads the model unfolded, at 1.45 GB;
    /// a desktop with room folds for speed; the env var wins either way.
    #[test]
    fn constants_fold_only_with_room() {
        assert!(!fold_constants(Some(3_538), None), "4 GB Pi 5");
        assert!(!fold_constants(Some(6_500), None), "8 GB machine, little free");
        assert!(fold_constants(Some(40_000), None), "desktop");
        assert!(!fold_constants(None, None), "unknown memory: play safe");
        assert!(fold_constants(Some(3_538), Some("1")));
        assert!(!fold_constants(Some(40_000), Some("0")));
        assert!(!fold_constants(Some(3_538), Some("yes")), "unrecognised value: by memory");
    }

    #[test]
    fn reads_mem_available() {
        if cfg!(target_os = "linux") { assert!(mem_available_mb().is_some_and(|m| m > 0)); }
    }
}

/// Split a real track with the real model and write the stems as WAVs:
///   OPENDECK_SEPARATE_MODEL=…/htdemucs.onnx (or .safetensors with separate-native) SPLIT_IN=track.mp3 SPLIT_OUT=dir \
///   cargo test --release -p opendeck-app --features separate-onnx split_a_real_track -- --ignored --nocapture
#[cfg(all(test, any(feature = "separate-onnx", feature = "separate-native")))]
mod real {
    use super::*;
    use opendeck_types::Decoder;
    use super::tests::split;

    #[test]
    #[ignore]
    fn split_a_real_track() {
        let (Some(inp), Some(out)) = (std::env::var_os("SPLIT_IN"), std::env::var_os("SPLIT_OUT")) else { panic!("set SPLIT_IN and SPLIT_OUT") };
        // The deck rate (SPLIT_SR, default 48 kHz); 44100 skips both
        // conversions, to compare against Demucs's own output.
        let sr: u32 = std::env::var("SPLIT_SR").ok().and_then(|s| s.parse().ok()).unwrap_or(48_000);
        let mut d = opendeck_decode::symphonia_decoder::SymphoniaDecoder::open(std::path::Path::new(&inp)).unwrap();
        let ch = d.channels() as usize;
        assert_eq!(ch, 2);
        let mut buf = vec![0f32; 4096 * ch];
        let mut pcm = Vec::new();
        loop { match d.decode(&mut buf).unwrap() { 0 => break, n => pcm.extend_from_slice(&buf[..n * ch]) } }
        let track = crate::audio::resample_interleaved(&pcm, 2, d.sample_rate(), sr).unwrap();
        let job = Job::default();
        let ((stems, t), load_s) = model::with(|m| split(&track, sr, m, &job)).unwrap();
        let secs = track.len() as f64 / 2.0 / sr as f64;
        println!("{secs:.0}s track | load {load_s:.1}s | {} chunks, model {:.1}s, total {:.1}s = {:.1}x real time | peak {} MB",
                 t.chunks, t.model_s, t.total_s, secs / t.total_s, model::memory_mb().0);
        // On a stick with OverCue stems for this file: how close are ours?
        // SDR with OverCue's stem as the reference (higher is closer).
        let track = std::sync::Arc::new(track);
        if let Ok(Some(oc)) = crate::stems::load_local(std::path::Path::new(&inp), &track, sr, 2, None) {
            for (i, name) in ["drums", "vocal", "harmonics"].iter().enumerate() {
                let (mut num, mut den) = (0f64, 0f64);
                for (a, b) in oc.stem_levels(i).iter().zip(&stems[i]) {
                    let (a, b) = (*a as f64, *b as f64);
                    num += a * a;
                    den += (a - b) * (a - b);
                }
                println!("vs OverCue {name:<9} SDR {:5.1} dB", 10.0 * (num / den.max(1e-12)).log10());
            }
        }
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        for (name, s) in ["drums", "vocal", "harmonics"].iter().zip(&stems) {
            // 32-bit float stereo WAV: a stem can be louder than full scale.
            let bytes = (s.len() * 4) as u32;
            let mut w = Vec::with_capacity(44 + bytes as usize);
            w.extend_from_slice(b"RIFF"); w.extend_from_slice(&(36 + bytes).to_le_bytes()); w.extend_from_slice(b"WAVEfmt ");
            for v in [16u32] { w.extend_from_slice(&v.to_le_bytes()); }
            for v in [3u16, 2] { w.extend_from_slice(&v.to_le_bytes()); }
            for v in [sr, sr * 8] { w.extend_from_slice(&v.to_le_bytes()); }
            for v in [8u16, 32] { w.extend_from_slice(&v.to_le_bytes()); }
            w.extend_from_slice(b"data"); w.extend_from_slice(&bytes.to_le_bytes());
            for v in s { w.extend_from_slice(&v.to_le_bytes()); }
            std::fs::write(out.join(format!("{name}.wav")), w).unwrap();
        }
    }
}
