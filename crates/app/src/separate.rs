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
/// Demucs's default chunk overlap.
const OVERLAP: f64 = 0.25;

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
    /// Called after each chunk: (chunk index, chunks, seconds the model took).
    pub on_chunk: Option<fn(usize, usize, f64)>,
}

impl Job {
    pub fn percent(&self) -> u8 { (self.progress.load(Ordering::Relaxed) / 10).min(100) as u8 }
}

/// What a split took, for the test log.
#[derive(Debug, Default, Clone)]
pub struct Timing {
    pub chunks: usize,
    pub model_s: f64,
    pub total_s: f64,
}

/// Split `track` (interleaved stereo at `deck_sr`) into drums, vocal and
/// harmonics: interleaved stereo i16 at `deck_sr`, each exactly as long as
/// `track`.
pub fn separate(track: &[f32], deck_sr: u32, sep: &mut dyn Separator, job: &Job) -> Result<([Vec<i16>; 3], Timing)> {
    let t0 = std::time::Instant::now();
    let frames = track.len() / 2;
    ensure!(frames > 0, "empty track");
    let seg = sep.segment();
    ensure!(seg >= 4, "model segment too short ({seg})");

    // Demucs normalises by the mono sum's statistics.
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

    let mix: Vec<f32> = {
        let norm: Vec<f32> = track.iter().map(|v| (v - mean) / std).collect();
        crate::audio::resample_interleaved(&norm, 2, deck_sr, MODEL_SR)?
    };
    let len = mix.len() / 2;

    // Demucs's crossfade: 1, 2, … up to the middle, then back down; never 0.
    let weight: Vec<f32> = {
        let half = seg / 2;
        let w: Vec<f32> = (1..=half).chain((1..=seg - half).rev()).map(|v| v as f32).collect();
        let max = w.iter().cloned().fold(0.0, f32::max);
        w.into_iter().map(|v| v / max).collect()
    };
    let stride = ((1.0 - OVERLAP) * seg as f64) as usize;
    let chunks = len.div_ceil(stride);

    // Overlap-add window: frames [base, base + acc.len()/6) of the three
    // stems (interleaved d L, d R, v L, v R, h L, h R) and their weights.
    let mut acc: Vec<f32> = Vec::new();
    let mut wsum: Vec<f32> = Vec::new();
    let mut base = 0usize;
    let mut rs = crate::audio::StreamResampler::new(MODEL_SR, deck_sr, 6)?;
    let mut outs: [Vec<i16>; 3] = std::array::from_fn(|_| Vec::with_capacity(track.len()));
    // The mix's DC goes back on harmonics, so the three stems add up to it.
    let emit = |block: &[f32], outs: &mut [Vec<i16>; 3]| {
        for f in block.chunks_exact(6) {
            for s in 0..3 {
                for c in 0..2 {
                    let v = f[s * 2 + c] * std + if s == 2 { mean } else { 0.0 };
                    outs[s].push((v * 32768.0).round().clamp(-32768.0, 32767.0) as i16);
                }
            }
        }
    };

    let mut input = vec![0f32; 2 * seg];
    let mut output = vec![0f32; 8 * seg];
    let mut timing = Timing { chunks, ..Default::default() };
    for k in 0..chunks {
        if job.cancel.load(Ordering::Relaxed) { bail!("cancelled"); }
        let off = k * stride;
        let n = seg.min(len - off);
        input.iter_mut().for_each(|v| *v = 0.0);
        for i in 0..n {
            input[i] = mix[(off + i) * 2];
            input[seg + i] = mix[(off + i) * 2 + 1];
        }
        let tm = std::time::Instant::now();
        sep.run(&input, &mut output)?;
        let dt = tm.elapsed().as_secs_f64();
        timing.model_s += dt;
        if let Some(f) = job.on_chunk { f(k, chunks, dt); }

        let need = off + n - base;
        if acc.len() < need * 6 { acc.resize(need * 6, 0.0); wsum.resize(need, 0.0); }
        let plane = |p: usize, i: usize| output[p * seg + i];
        for i in 0..n {
            let w = weight[i];
            let a = (off - base + i) * 6;
            for c in 0..2 {
                acc[a + c]     += w * plane(c, i);                                 // drums
                acc[a + 2 + c] += w * plane(6 + c, i);                             // vocals
                acc[a + 4 + c] += w * (plane(2 + c, i) + plane(4 + c, i));          // bass + other
            }
            wsum[off - base + i] += w;
        }
        // Everything before the next chunk's start is final.
        let done_to = if k + 1 < chunks { (k + 1) * stride } else { len };
        let ready = done_to - base;
        let mut block: Vec<f32> = acc.drain(..ready * 6).collect();
        for (f, w) in block.chunks_exact_mut(6).zip(wsum.drain(..ready)) {
            f.iter_mut().for_each(|v| *v /= w);
        }
        base = done_to;
        emit(&rs.push(&block)?, &mut outs);
        job.progress.store(((k + 1) * 1000 / chunks) as u32, Ordering::Relaxed);
    }
    emit(&rs.finish()?, &mut outs);
    for o in &mut outs { o.resize(track.len(), 0); }
    timing.total_s = t0.elapsed().as_secs_f64();
    Ok((outs, timing))
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
        fn freedj_sep_segment(h: *mut c_void) -> usize;
        fn freedj_sep_run(h: *mut c_void, mix: *const f32, out: *mut f32, n: usize, err: *mut c_char, errlen: usize) -> bool;
        fn freedj_mem_footprint() -> u64;
        fn freedj_mem_available() -> u64;
        fn freedj_device_model(buf: *mut c_char, len: usize);
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

    /// Process memory: (footprint, still available before iOS kills us), MB.
    pub fn memory_mb() -> (u64, u64) { unsafe { (freedj_mem_footprint() >> 20, freedj_mem_available() >> 20) } }

    pub fn device() -> String {
        let mut b = [0u8; 64];
        unsafe { freedj_device_model(b.as_mut_ptr() as *mut c_char, b.len()) };
        err_text(&b)
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
            let session = ort::session::Session::builder(&env).map_err(|e| anyhow!("{e}"))?
                .with_intra_threads(threads).map_err(|e| anyhow!("{e}"))?
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

/// The platform's model: Core ML on iOS, ONNX Runtime on a desktop built
/// with `separate-onnx`, none elsewhere.
pub mod model {
    #[allow(unused_imports)]
    use super::Separator;
    use anyhow::Result;

    pub fn present() -> bool {
        #[cfg(target_os = "ios")] { return super::coreml::present(); }
        #[cfg(all(not(target_os = "ios"), feature = "separate-onnx"))] { return super::onnx::present(); }
        #[allow(unreachable_code)] false
    }

    /// Run `f` with the model loaded; also returns the load time (0 once
    /// loaded).
    #[allow(unused_variables)]
    pub fn with<R>(f: impl FnOnce(&mut dyn Separator) -> Result<R>) -> Result<(R, f64)> {
        #[cfg(target_os = "ios")] { return super::coreml::with(|m| f(m)); }
        #[cfg(all(not(target_os = "ios"), feature = "separate-onnx"))] { return super::onnx::with(|m| f(m)); }
        #[allow(unreachable_code)] Err(anyhow::anyhow!("no separation model on this platform"))
    }

    /// (process memory, memory left before the OS steps in), MB.
    pub fn memory_mb() -> (u64, u64) {
        #[cfg(target_os = "ios")] { return super::coreml::memory_mb(); }
        #[allow(unreachable_code)] {
            let kb = |file: &str, key: &str| std::fs::read_to_string(file).ok().and_then(|s| s.lines()
                .find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())).unwrap_or(0);
            (kb("/proc/self/status", "VmHWM:") >> 10, kb("/proc/meminfo", "MemAvailable:") >> 10)
        }
    }

    pub fn device() -> String {
        #[cfg(target_os = "ios")] { return super::coreml::device(); }
        #[allow(unreachable_code)] { format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let ([drums, vocal, harm], t) = separate(&track, sr, &mut fake, &job).unwrap();
        assert_eq!(drums.len(), track.len());
        assert_eq!(fake.calls, t.chunks);
        assert_eq!(job.percent(), 100);
        // Away from the very ends (the rate converters' edges).
        let lo = 2_000;
        let hi = track.len() - 2_000;
        let err = |got: &[i16], want: f32| (lo..hi).map(|i| (got[i] as f32 / 32768.0 - want * track[i]).abs()).fold(0.0, f32::max);
        // The fake splits the normalised mix, so drums miss half the DC
        // and harmonics carry it; the tone's DC is small.
        assert!(err(&drums, 0.5) < 0.02, "drums off by {}", err(&drums, 0.5));
        assert!(err(&harm, 0.5) < 0.02, "harmonics off by {}", err(&harm, 0.5));
        assert!(vocal[lo..hi].iter().all(|v| v.unsigned_abs() < 700), "vocal should be near silent");
        // No lag: the drums match the track best unshifted (a one-frame
        // slip would still pass the tolerance above).
        let lag_err = |lag: isize| (lo..hi).step_by(2).map(|i| {
            let j = (i as isize + 2 * lag) as usize;
            (drums[j] as f32 / 32768.0 - 0.5 * track[i]).powi(2)
        }).sum::<f32>();
        let best = (-4..=4).min_by(|a, b| lag_err(*a).total_cmp(&lag_err(*b))).unwrap();
        assert_eq!(best, 0, "drums are {best} frames off the track");
        // Drums + harmonics rebuild the mix.
        let sum_err = (lo..hi).map(|i| ((drums[i] as f32 + harm[i] as f32) / 32768.0 - track[i]).abs()).fold(0.0, f32::max);
        assert!(sum_err < 0.03, "stems do not add back to the mix: {sum_err}");
    }

    #[test]
    fn a_track_shorter_than_one_chunk_splits() {
        let track = tone(3_000, 44_100);
        let mut fake = Fake { seg: 10_000, calls: 0 };
        let ([d, _, h], t) = separate(&track, 44_100, &mut fake, &Job::default()).unwrap();
        assert_eq!(t.chunks, 1);
        assert_eq!(d.len(), track.len());
        assert_eq!(h.len(), track.len());
    }

    #[test]
    fn cancel_stops_a_split() {
        let track = tone(100_000, 44_100);
        let job = Job::default();
        job.cancel.store(true, Ordering::Relaxed);
        assert!(separate(&track, 44_100, &mut Fake { seg: 10_000, calls: 0 }, &job).is_err());
    }
}

/// Split a real track with the real model and write the stems as WAVs:
///   OPENDECK_SEPARATE_MODEL=…/htdemucs.onnx SPLIT_IN=track.mp3 SPLIT_OUT=dir \
///   cargo test --release -p opendeck-app --features separate-onnx split_a_real_track -- --ignored --nocapture
#[cfg(all(test, feature = "separate-onnx"))]
mod real {
    use super::*;
    use opendeck_types::Decoder;

    #[test]
    #[ignore]
    fn split_a_real_track() {
        let (Some(inp), Some(out)) = (std::env::var_os("SPLIT_IN"), std::env::var_os("SPLIT_OUT")) else { panic!("set SPLIT_IN and SPLIT_OUT") };
        let sr = 48_000;
        let mut d = opendeck_decode::symphonia_decoder::SymphoniaDecoder::open(std::path::Path::new(&inp)).unwrap();
        let ch = d.channels() as usize;
        assert_eq!(ch, 2);
        let mut buf = vec![0f32; 4096 * ch];
        let mut pcm = Vec::new();
        loop { match d.decode(&mut buf).unwrap() { 0 => break, n => pcm.extend_from_slice(&buf[..n * ch]) } }
        let track = crate::audio::resample_interleaved(&pcm, 2, d.sample_rate(), sr).unwrap();
        let job = Job::default();
        let ((stems, t), load_s) = model::with(|m| separate(&track, sr, m, &job)).unwrap();
        let secs = track.len() as f64 / 2.0 / sr as f64;
        println!("{secs:.0}s track | load {load_s:.1}s | {} chunks, model {:.1}s, total {:.1}s = {:.1}x real time | peak {} MB",
                 t.chunks, t.model_s, t.total_s, secs / t.total_s, model::memory_mb().0);
        // On a stick with OverCue stems for this file: how close are ours?
        // SDR with OverCue's stem as the reference (higher is closer).
        let track = std::sync::Arc::new(track);
        if let Ok(Some(oc)) = crate::stems::load_local(std::path::Path::new(&inp), &track, sr, 2) {
            for (i, name) in ["drums", "vocal", "harmonics"].iter().enumerate() {
                let (mut num, mut den) = (0f64, 0f64);
                for (a, b) in oc.data[i].iter().zip(&stems[i]) {
                    let (a, b) = (*a as f64, *b as f64);
                    num += a * a;
                    den += (a - b) * (a - b);
                }
                println!("vs OverCue {name:<9} SDR {:5.1} dB", 10.0 * (num / den.max(1.0)).log10());
            }
        }
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        for (name, s) in ["drums", "vocal", "harmonics"].iter().zip(&stems) {
            // 16-bit stereo WAV.
            let bytes = (s.len() * 2) as u32;
            let mut w = Vec::with_capacity(44 + bytes as usize);
            w.extend_from_slice(b"RIFF"); w.extend_from_slice(&(36 + bytes).to_le_bytes()); w.extend_from_slice(b"WAVEfmt ");
            for v in [16u32] { w.extend_from_slice(&v.to_le_bytes()); }
            for v in [1u16, 2] { w.extend_from_slice(&v.to_le_bytes()); }
            for v in [sr, sr * 4] { w.extend_from_slice(&v.to_le_bytes()); }
            for v in [4u16, 16] { w.extend_from_slice(&v.to_le_bytes()); }
            w.extend_from_slice(b"data"); w.extend_from_slice(&bytes.to_le_bytes());
            for v in s { w.extend_from_slice(&v.to_le_bytes()); }
            std::fs::write(out.join(format!("{name}.wav")), w).unwrap();
        }
    }
}
