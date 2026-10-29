//! Two pipeline stage implementations:
//!
//! - `ResampleStage`:      speed change without pitch change (no key-lock).
//!                         Uses `rubato` (pure Rust, sinc interpolation).
//!
//! - `TimestretechStage`:  speed change with constant pitch (key-lock).
//!                         Wraps Rubber Band Library R3 engine via C FFI.
//! - `BungeeStage`:        the same contract over Bungee (feature `bungee`,
//!                         on by default): an adaptive phase vocoder that is
//!                         much cheaper than R3.
//! - `WsolaStage`:         time-domain WSOLA (`wsola.rs`), pure Rust, a few
//!                         percent of a core: for boards that cannot afford
//!                         either of the above.  `Stretcher::from_env` picks
//!                         one at runtime from `OPENDECK_STRETCH`.

mod rubberband_sys;
mod wsola;
pub use wsola::WsolaStage;
#[cfg(feature = "bungee")]
mod bungee_sys;

use opendeck_types::PipelineStage;
use rubberband_sys as rb;

// ── Engine selection ──────────────────────────────────────────────────────────

/// Which key-lock engine the deck runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Rubber Band R3 ("Finer"): the reference quality, ~0.7 of a Pi 4 core.
    RubberBand,
    /// Bungee Basic: adaptive phase vocoder, a fraction of R3's cost.
    Bungee,
    /// WSOLA: time-domain, the SoundTouch family; cheapest, lowest quality.
    Wsola,
}

impl Engine {
    /// `OPENDECK_STRETCH=rubberband|rb|r3`, `bungee` or `wsola`; default Rubber Band.
    /// Unknown values and a build without the `bungee` feature fall back to
    /// Rubber Band with a warning.
    pub fn from_env() -> Engine {
        match std::env::var("OPENDECK_STRETCH").ok().as_deref().map(|v| v.trim().to_ascii_lowercase()) {
            None => Engine::RubberBand,
            Some(v) => match v.as_str() {
                "rubberband" | "rb" | "r3" | "" => Engine::RubberBand,
                "bungee" => {
                    if cfg!(feature = "bungee") {
                        Engine::Bungee
                    } else {
                        log::warn!("OPENDECK_STRETCH=bungee but this build has no bungee feature; using Rubber Band");
                        Engine::RubberBand
                    }
                }
                "wsola" | "soundtouch" => Engine::Wsola,
                other => {
                    log::warn!("OPENDECK_STRETCH={other:?} unknown (rubberband|bungee|wsola); using Rubber Band");
                    Engine::RubberBand
                }
            },
        }
    }

    pub fn name(self) -> &'static str {
        match self { Engine::RubberBand => "Rubber Band R3", Engine::Bungee => "Bungee", Engine::Wsola => "WSOLA" }
    }
}

/// A key-lock stage of either engine.
pub enum Stretcher {
    RubberBand(TimestretechStage),
    #[cfg(feature = "bungee")]
    Bungee(BungeeStage),
    Wsola(WsolaStage),
}

impl Stretcher {
    pub fn new(engine: Engine, sample_rate: u32, channels: u8) -> Self {
        match engine {
            Engine::RubberBand => Stretcher::RubberBand(TimestretechStage::new(sample_rate, channels)),
            #[cfg(feature = "bungee")]
            Engine::Bungee => Stretcher::Bungee(BungeeStage::new(sample_rate, channels)),
            #[cfg(not(feature = "bungee"))]
            Engine::Bungee => Stretcher::RubberBand(TimestretechStage::new(sample_rate, channels)),
            Engine::Wsola => Stretcher::Wsola(WsolaStage::new(sample_rate, channels)),
        }
    }

    /// The engine named by `OPENDECK_STRETCH` (see `Engine::from_env`).
    pub fn from_env(sample_rate: u32, channels: u8) -> Self {
        let engine = Engine::from_env();
        log::info!("key-lock engine: {}", engine.name());
        Self::new(engine, sample_rate, channels)
    }

    pub fn engine(&self) -> Engine {
        match self {
            Stretcher::RubberBand(_) => Engine::RubberBand,
            #[cfg(feature = "bungee")]
            Stretcher::Bungee(_) => Engine::Bungee,
            Stretcher::Wsola(_) => Engine::Wsola,
        }
    }

    fn inner(&mut self) -> &mut dyn PipelineStage {
        match self {
            Stretcher::RubberBand(s) => s,
            #[cfg(feature = "bungee")]
            Stretcher::Bungee(s) => s,
            Stretcher::Wsola(s) => s,
        }
    }

    fn inner_ref(&self) -> &dyn PipelineStage {
        match self {
            Stretcher::RubberBand(s) => s,
            #[cfg(feature = "bungee")]
            Stretcher::Bungee(s) => s,
            Stretcher::Wsola(s) => s,
        }
    }
}

impl PipelineStage for Stretcher {
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) { self.inner().process(input, output) }
    fn set_speed(&mut self, speed: f32) { self.inner().set_speed(speed) }
    fn set_pitch_semitones(&mut self, semitones: f32) { self.inner().set_pitch_semitones(semitones) }
    fn latency_frames(&self) -> usize { self.inner_ref().latency_frames() }
    fn reset(&mut self) { self.inner().reset() }
}

// ── ResampleStage (no key-lock) ───────────────────────────────────────────────

pub struct ResampleStage {
    speed:   f32,
}

impl ResampleStage {
    pub fn new(_sample_rate: u32, _channels: u8) -> Self {
        Self { speed: 1.0 }
    }
}

impl PipelineStage for ResampleStage {
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        // TODO: run rubato SincFixedOut resampler at self.speed ratio.
        output.extend_from_slice(input);
    }

    fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
    }

    fn set_pitch_semitones(&mut self, _semitones: f32) {
        // No pitch shifting — pitch follows speed.
    }

    fn latency_frames(&self) -> usize {
        256 // rubato SincFixedOut at quality 5
    }

    fn reset(&mut self) {
        // TODO: flush rubato internal state
    }
}

// ── TimestretechStage (key-lock via Rubber Band R3) ───────────────────────────

/// RAII wrapper around a `RubberBandState` pointer.
///
/// # Safety
/// `RubberBandState` is not Send by default (it is a raw pointer), but
/// Rubber Band's real-time path is explicitly designed to be called from a
/// single audio thread, so we assert Send here and ensure we only ever
/// touch `state` from one thread at a time (enforced by the `&mut self`
/// receiver on every method).
struct RbHandle {
    state: rb::RubberBandState,
}

// SAFETY: we never share the pointer across threads; all access is through
// `&mut self`, which the borrow checker prevents from being aliased.
unsafe impl Send for RbHandle {}

impl Drop for RbHandle {
    fn drop(&mut self) {
        unsafe { rb::rubberband_delete(self.state); }
    }
}

// Block size fed to rubberband_process at a time (frames, not samples).
// 512 gives good latency at 44.1 kHz (~11 ms).
const BLOCK_FRAMES: usize = 512;

pub struct TimestretechStage {
    rb:              RbHandle,
    channels:        usize,
    speed:           f32,
    pitch_semitones: f32,
    // Deinterleaved input scratch buffers (one per channel).
    in_bufs:         Vec<Vec<f32>>,
    in_ptrs:         Vec<*const f32>,
    // Deinterleaved output scratch buffers (one per channel).
    out_bufs:        Vec<Vec<f32>>,
    out_ptrs:        Vec<*mut f32>,
}

// SAFETY: same argument as RbHandle above; all raw pointers point into
// in_bufs/out_bufs which live as long as the struct.
unsafe impl Send for TimestretechStage {}

impl TimestretechStage {
    /// Create with the R3 (Finer) engine for maximum quality.
    pub fn new(sample_rate: u32, channels: u8) -> Self {
        let ch = channels as usize;

        let state = unsafe {
            rb::rubberband_new(
                sample_rate,
                channels as u32,
                rb::REALTIME_R3_OPTIONS,
                1.0, // time ratio  (1.0 = unchanged)
                1.0, // pitch scale (1.0 = unchanged)
            )
        };
        assert!(!state.is_null(), "rubberband_new returned null");

        unsafe {
            rb::rubberband_set_max_process_size(state, BLOCK_FRAMES as u32);
        }

        let latency = unsafe { rb::rubberband_get_latency(state) };
        log::info!("Rubber Band R3 engine initialised — latency: {latency} frames");

        let in_bufs:  Vec<Vec<f32>> = vec![vec![0.0f32; BLOCK_FRAMES]; ch];
        let out_bufs: Vec<Vec<f32>> = vec![vec![0.0f32; BLOCK_FRAMES]; ch];
        let in_ptrs:  Vec<*const f32> = in_bufs.iter().map(|v| v.as_ptr()).collect();
        let out_ptrs: Vec<*mut f32>   = out_bufs.iter().map(|v| v.as_ptr() as *mut f32).collect();

        Self {
            rb: RbHandle { state },
            channels: ch,
            speed: 1.0,
            pitch_semitones: 0.0,
            in_bufs,
            in_ptrs,
            out_bufs,
            out_ptrs,
        }
    }

    /// Feed one block of interleaved frames into Rubber Band and collect
    /// whatever output is ready into `output`.
    fn push_block(&mut self, frames: &[f32], output: &mut Vec<f32>) {
        let n = frames.len() / self.channels;
        debug_assert!(n <= BLOCK_FRAMES);

        // Deinterleave.
        for ch in 0..self.channels {
            for i in 0..n {
                self.in_bufs[ch][i] = frames[i * self.channels + ch];
            }
            self.in_ptrs[ch] = self.in_bufs[ch].as_ptr();
        }

        unsafe {
            rb::rubberband_process(
                self.rb.state,
                self.in_ptrs.as_ptr(),
                n as u32,
                0, // not final
            );
        }

        // Drain all available output.
        loop {
            let avail = unsafe { rb::rubberband_available(self.rb.state) };
            if avail <= 0 { break; }

            let to_read = (avail as usize).min(BLOCK_FRAMES);

            // Update out_ptrs in case Vec reallocated (shouldn't, but be safe).
            for ch in 0..self.channels {
                self.out_ptrs[ch] = self.out_bufs[ch].as_mut_ptr();
            }

            let got = unsafe {
                rb::rubberband_retrieve(
                    self.rb.state,
                    self.out_ptrs.as_ptr() as *const *mut f32,
                    to_read as u32,
                )
            } as usize;

            // Re-interleave into output.
            let prev_len = output.len();
            output.resize(prev_len + got * self.channels, 0.0);
            for i in 0..got {
                for ch in 0..self.channels {
                    output[prev_len + i * self.channels + ch] = self.out_bufs[ch][i];
                }
            }
        }
    }
}

impl PipelineStage for TimestretechStage {
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        // Feed input in BLOCK_FRAMES-sized chunks.
        let stride = BLOCK_FRAMES * self.channels;
        let mut offset = 0;
        while offset < input.len() {
            let end = (offset + stride).min(input.len());
            self.push_block(&input[offset..end], output);
            offset = end;
        }
    }

    fn set_speed(&mut self, speed: f32) {
        if (speed - self.speed).abs() > 1e-5 {
            self.speed = speed;
            unsafe {
                rb::rubberband_set_time_ratio(self.rb.state, 1.0 / speed as f64);
            }
        }
    }

    fn set_pitch_semitones(&mut self, semitones: f32) {
        if (semitones - self.pitch_semitones).abs() > 1e-4 {
            self.pitch_semitones = semitones;
            let scale = 2f64.powf(semitones as f64 / 12.0);
            unsafe {
                rb::rubberband_set_pitch_scale(self.rb.state, scale);
            }
        }
    }

    fn latency_frames(&self) -> usize {
        unsafe { rb::rubberband_get_latency(self.rb.state) as usize }
    }

    fn reset(&mut self) {
        unsafe { rb::rubberband_reset(self.rb.state); }
    }
}

// ── BungeeStage (key-lock via Bungee Basic) ───────────────────────────────────

/// Key-lock through Bungee's granular API behind a push adapter
/// (`csrc/bungee_shim.cpp`).  Same contract as `TimestretechStage`: feed
/// interleaved frames, collect whatever comes out; `latency_frames()` of
/// input must go in after a reset before anything comes out, and the deck
/// pre-rolls through that exactly as it does for Rubber Band.
#[cfg(feature = "bungee")]
pub struct BungeeStage {
    handle:          BungeeHandle,
    channels:        usize,
    speed:           f32,
    pitch_semitones: f32,
    pitch_scale:     f64,
    in_bufs:         Vec<Vec<f32>>,
    in_ptrs:         Vec<*const f32>,
    out_bufs:        Vec<Vec<f32>>,
    out_ptrs:        Vec<*mut f32>,
}

#[cfg(feature = "bungee")]
struct BungeeHandle(*mut std::os::raw::c_void);

// SAFETY: as for RbHandle — one owner, all access through `&mut self`.
#[cfg(feature = "bungee")]
unsafe impl Send for BungeeHandle {}
#[cfg(feature = "bungee")]
unsafe impl Send for BungeeStage {}

#[cfg(feature = "bungee")]
impl Drop for BungeeHandle {
    fn drop(&mut self) { unsafe { bungee_sys::od_bungee_destroy(self.0) } }
}

#[cfg(feature = "bungee")]
impl BungeeStage {
    /// Slowest speed the output buffers are sized for without reallocating
    /// (the deck's tempo range is ±100% at most, i.e. 0.5×; vinyl-mode
    /// crawl goes lower, and then the buffer simply grows once).
    const MIN_SPEED_PREALLOC: f32 = 0.25;

    pub fn new(sample_rate: u32, channels: u8) -> Self {
        let ch = channels as usize;
        let p = unsafe { bungee_sys::od_bungee_create(sample_rate as i32, ch as i32, BLOCK_FRAMES as i32) };
        assert!(!p.is_null(), "od_bungee_create returned null");
        let version = unsafe { std::ffi::CStr::from_ptr(bungee_sys::od_bungee_version()) }.to_string_lossy().into_owned();
        let out_cap = (BLOCK_FRAMES as f32 / Self::MIN_SPEED_PREALLOC).ceil() as usize + 1;
        let in_bufs:  Vec<Vec<f32>> = vec![vec![0.0f32; BLOCK_FRAMES]; ch];
        let out_bufs: Vec<Vec<f32>> = vec![vec![0.0f32; out_cap]; ch];
        let in_ptrs  = in_bufs.iter().map(|v| v.as_ptr()).collect();
        let out_ptrs = out_bufs.iter().map(|v| v.as_ptr() as *mut f32).collect();
        let stage = Self {
            handle: BungeeHandle(p), channels: ch, speed: 1.0, pitch_semitones: 0.0, pitch_scale: 1.0,
            in_bufs, in_ptrs, out_bufs, out_ptrs,
        };
        log::info!("Bungee {version} initialised — latency: {} frames", stage.latency_frames());
        stage
    }

    fn push_block(&mut self, frames: &[f32], output: &mut Vec<f32>) {
        let n = frames.len() / self.channels;
        debug_assert!(n <= BLOCK_FRAMES);
        if n == 0 { return; }
        for ch in 0..self.channels {
            for i in 0..n {
                self.in_bufs[ch][i] = frames[i * self.channels + ch];
            }
            self.in_ptrs[ch] = self.in_bufs[ch].as_ptr();
        }
        // Steady state yields n / speed frames; the first push after a reset
        // can carry up to half a block more (the owed fraction), so leave room.
        let need = (n as f64 / self.speed as f64).ceil() as usize + 2;
        for ch in 0..self.channels {
            if self.out_bufs[ch].len() < need { self.out_bufs[ch].resize(need, 0.0); }
            self.out_ptrs[ch] = self.out_bufs[ch].as_mut_ptr();
        }
        let got = unsafe {
            bungee_sys::od_bungee_process(
                self.handle.0,
                self.in_ptrs.as_ptr(),
                self.out_ptrs.as_ptr(),
                n as i32,
                self.speed as f64,
                self.pitch_scale,
                need as i32,
            )
        }.max(0) as usize;
        let prev_len = output.len();
        output.resize(prev_len + got * self.channels, 0.0);
        for i in 0..got {
            for ch in 0..self.channels {
                output[prev_len + i * self.channels + ch] = self.out_bufs[ch][i];
            }
        }
    }
}

#[cfg(feature = "bungee")]
impl PipelineStage for BungeeStage {
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        let stride = BLOCK_FRAMES * self.channels;
        let mut offset = 0;
        while offset < input.len() {
            let end = (offset + stride).min(input.len());
            self.push_block(&input[offset..end], output);
            offset = end;
        }
    }

    fn set_speed(&mut self, speed: f32) {
        // Stream is forward-only; clamp away from zero so `in / speed` stays finite.
        self.speed = speed.max(1e-3);
    }

    fn set_pitch_semitones(&mut self, semitones: f32) {
        if (semitones - self.pitch_semitones).abs() > 1e-4 {
            self.pitch_semitones = semitones;
            self.pitch_scale = 2f64.powf(semitones as f64 / 12.0);
        }
    }

    fn latency_frames(&self) -> usize {
        unsafe { bungee_sys::od_bungee_latency(self.handle.0) }.max(0.0).round() as usize
    }

    fn reset(&mut self) {
        unsafe { bungee_sys::od_bungee_reset(self.handle.0) }
    }
}

// ── Performance regression guard ──────────────────────────────────────────────
#[cfg(test)]
mod perf {
    use super::*;
    use opendeck_types::PipelineStage;
    use std::time::Instant;

    const SR: u32 = 48_000;
    const CH: usize = 2;
    const BLOCK: usize = 512;          // matches the app's BLOCK_FRAMES

    /// Deterministic broadband stereo test signal (interleaved f32), `secs` long.
    /// A handful of detuned partials plus a cheap LCG "noise" floor so Rubber Band
    /// does representative work (silence would let it short-circuit).
    fn signal(secs: f32) -> Vec<f32> {
        let n = (secs * SR as f32) as usize;
        let mut out = Vec::with_capacity(n * CH);
        let mut lcg: u32 = 0x1234_5678;
        for i in 0..n {
            let t = i as f32 / SR as f32;
            let mut s = 0.0f32;
            for (f, a) in [(110.0, 0.30), (277.3, 0.22), (523.7, 0.16), (1500.9, 0.10)] {
                s += a * (2.0 * std::f32::consts::PI * f * t).sin();
            }
            lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (lcg >> 9) as f32 / (1u32 << 23) as f32 - 0.5; // ~[-0.5,0.5)
            s += 0.05 * noise;
            out.push(s * 0.9);       // L
            out.push(s * 0.9);       // R
        }
        out
    }

    /// Process `input` through the stage in BLOCK-frame chunks; return wall time.
    fn run(stage: &mut dyn PipelineStage, input: &[f32]) -> std::time::Duration {
        let mut scratch = Vec::with_capacity(BLOCK * CH * 8);
        let t0 = Instant::now();
        for chunk in input.chunks(BLOCK * CH) {
            scratch.clear();
            stage.process(chunk, &mut scratch);
        }
        t0.elapsed()
    }

    /// Machine-speed calibration independent of Rubber Band: time a fixed batch
    /// of 1024-point FFTs (rustfft), the same class of SIMD/FP DSP work R3 does.
    /// Normalising the timestretch time by this makes the guard roughly
    /// machine-independent — a real R3 regression shifts `rb_time / calib` on any
    /// box, while a faster/slower CPU moves both together and cancels out.
    fn calibrate() -> std::time::Duration {
        use rustfft::{num_complex::Complex, FftPlanner};
        const N: usize = 1024;
        // Sized so the calibration takes ~200 ms — comparable to the R3
        // measurement, so both are equally stable. A too-short calibration is
        // dominated by turbo-ramp / scheduling noise and the ratio jitters.
        const ITERS: usize = 160_000;
        let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
        let mut buf: Vec<Complex<f32>> = (0..N)
            .map(|i| Complex::new((i as f32 * 0.017).sin(), 0.0))
            .collect();
        let t0 = Instant::now();
        for _ in 0..ITERS {
            fft.process(&mut buf);
            // Re-normalise so values don't blow up over 4000 forward FFTs
            // (keeps the work honest without allocating).
            let s = 1.0 / N as f32;
            for c in buf.iter_mut() { *c *= s; }
        }
        t0.elapsed()
    }

    /// Two guards on the R3 timestretch cost, checked at 1.0×, 0.5× (heaviest —
    /// it emits ~2× frames), and 2.0×:
    ///
    /// 1. **Normalised ratio** `rb_time / fft_calibration` — the primary
    ///    regression catch. Because both are f32 DSP measured on the same box in
    ///    the same run, a faster/slower CPU cancels out and the ratio is stable
    ///    to ~1% (desktop: 2.49 / 4.57 / 1.94). A real R3 regression shifts it on
    ///    *any* machine, so the band can be tight (≈1.7× the desktop value —
    ///    catches a ~70% slowdown while tolerating cross-CPU drift).
    /// 2. **Absolute RTF** `rb_time / audio_duration` — the viability floor:
    ///    < 1.0 means the DSP keeps up. Sized for the Pi 5 target with margin;
    ///    also the backstop if the FFT calibration itself ever regressed.
    ///
    /// Both are always printed (`make perf` / `--nocapture`) so a sub-threshold
    /// creep shows as a trend. `OPENDECK_RTF_CEIL=<scale>` relaxes/tightens both
    /// (e.g. on hardware slower than the Pi 5, or for a stricter local check).
    #[test]
    fn timestretch_realtime_factor() {
        let secs = 8.0f32;
        let audio = signal(secs);

        let scale: f64 = std::env::var("OPENDECK_RTF_CEIL")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);

        // Warm the calibration path once, then measure it — the machine-speed
        // reference the R3 cost is normalised against.
        calibrate();
        let calib = calibrate().as_secs_f64();

        // speed, normalised-ratio ceiling, absolute-RTF ceiling.
        for (speed, norm_ceil, rtf_ceil) in
            [(1.0f32, 4.2f64, 0.80f64), (0.5, 7.5, 1.10), (2.0, 3.3, 0.60)]
        {
            let mut stage = TimestretechStage::new(SR, CH as u8);
            stage.set_speed(speed);
            run(&mut stage, &signal(1.0));            // warm up

            let elapsed = run(&mut stage, &audio).as_secs_f64();
            let rtf  = elapsed / secs as f64;
            let norm = elapsed / calib;
            let (nc, rc) = (norm_ceil * scale, rtf_ceil * scale);
            println!(
                "@ {:.2}x  norm(rb/fft) = {:.3} (ceil {:.2})   RTF = {:.4} (ceil {:.2})   \
                 rb {:.0}ms calib {:.0}ms",
                speed, norm, nc, rtf, rc, elapsed * 1000.0, calib * 1000.0,
            );
            assert!(
                norm < nc,
                "timestretch regression @ {speed:.2}x: normalised cost {norm:.2} ≥ {nc:.2} \
                 — R3 got slower relative to the FFT calibration (a DSP change or a \
                 heavier Rubber Band config; independent of CPU speed)",
            );
            assert!(
                rtf < rc,
                "timestretch not real-time @ {speed:.2}x: RTF {rtf:.3} ≥ {rc:.2} \
                 — either an un-optimised build or hardware slower than the Pi 5 target",
            );
        }
    }

    /// Bungee's cost next to R3's, same signal, same chunking.  Printed by
    /// `make perf`; asserted only against the real-time floor (it is the
    /// cheap engine, so the ceilings are the R3 ones).
    #[cfg(feature = "bungee")]
    #[test]
    fn bungee_realtime_factor() {
        let secs = 8.0f32;
        let audio = signal(secs);
        calibrate();
        let calib = calibrate().as_secs_f64();
        for (speed, rtf_ceil) in [(1.0f32, 0.80f64), (0.5, 1.10), (2.0, 0.60)] {
            let mut stage = BungeeStage::new(SR, CH as u8);
            stage.set_speed(speed);
            run(&mut stage, &signal(1.0));
            let elapsed = run(&mut stage, &audio).as_secs_f64();
            let rtf = elapsed / secs as f64;
            println!(
                "bungee @ {:.2}x  norm(bungee/fft) = {:.3}   RTF = {:.4} (ceil {:.2})   bungee {:.0}ms calib {:.0}ms",
                speed, elapsed / calib, rtf, rtf_ceil, elapsed * 1000.0, calib * 1000.0,
            );
            assert!(rtf < rtf_ceil, "bungee not real-time @ {speed:.2}x: RTF {rtf:.3}");
        }
    }

    /// WSOLA: the cheap engine.  Printed alongside the others under
    /// `make perf`; asserted against a tight real-time floor since being
    /// cheap is its whole point.
    #[test]
    fn wsola_realtime_factor() {
        let secs = 8.0f32;
        let audio = signal(secs);
        calibrate();
        let calib = calibrate().as_secs_f64();
        for (speed, rtf_ceil) in [(1.0f32, 0.10f64), (0.5, 0.15), (2.0, 0.10), (0.92, 0.15)] {
            let mut stage = WsolaStage::new(SR, CH as u8);
            stage.set_speed(speed);
            run(&mut stage, &signal(1.0));
            let elapsed = run(&mut stage, &audio).as_secs_f64();
            let rtf = elapsed / secs as f64;
            println!(
                "wsola @ {:.2}x  norm(wsola/fft) = {:.3}   RTF = {:.4} (ceil {:.2})   wsola {:.1}ms calib {:.0}ms",
                speed, elapsed / calib, rtf, rtf_ceil, elapsed * 1000.0, calib * 1000.0,
            );
            assert!(rtf < rtf_ceil, "wsola not cheap @ {speed:.2}x: RTF {rtf:.3}");
        }
    }

    /// WSOLA keeps the same frame contract as the other engines.
    #[test]
    fn wsola_frame_accounting_and_signal() {
        for speed in [0.5f32, 0.92, 1.0, 1.08, 2.0] {
            let mut stage = WsolaStage::new(SR, CH as u8);
            frame_accounting_and_signal(&mut stage, speed);
        }
    }

    /// Bungee renders `in / speed` frames per push and real audio, not silence.
    #[cfg(feature = "bungee")]
    #[test]
    fn bungee_frame_accounting_and_signal() {
        for speed in [0.5f32, 0.92, 1.0, 1.08, 2.0] {
            let mut stage = BungeeStage::new(SR, CH as u8);
            frame_accounting_and_signal(&mut stage, speed);
        }
    }

    /// `in / speed` frames per push, real audio, and after a reset nothing
    /// until the latency is covered — then full level from the seek point.
    fn frame_accounting_and_signal(stage: &mut dyn PipelineStage, speed: f32) {
        {
            stage.set_speed(speed);
            let input = signal(2.0);
            let mut out = Vec::new();
            for chunk in input.chunks(BLOCK * CH) {
                stage.process(chunk, &mut out);
            }
            let in_frames = input.len() / CH;
            let out_frames = out.len() / CH;
            let latency = stage.latency_frames();
            let expect = (in_frames - latency) as f32 / speed;
            assert!(
                (out_frames as f32 - expect).abs() < 0.01 * expect + 2.0,
                "@{speed}x: {out_frames} frames out, expected ~{expect} (latency {latency})",
            );
            // Skip the lead-in (the stretcher starts from muted history), then
            // the output must carry energy comparable to the input.
            let tail = &out[out.len() / 2..];
            let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
            let in_rms = (input.iter().map(|x| x * x).sum::<f32>() / input.len() as f32).sqrt();
            assert!(rms > 0.3 * in_rms, "@{speed}x: output rms {rms} vs input {in_rms}");
            let peak = tail.iter().fold(0f32, |m, x| m.max(x.abs()));
            assert!(peak < 2.0, "@{speed}x: peak {peak}, output blew up");
            // After a reset nothing comes out until the latency is covered —
            // then the first frames are the seek point at full level, not a
            // fade-in (a CUE press must hit the transient).
            stage.reset();
            let latency = stage.latency_frames();
            let mut after = Vec::new();
            stage.process(&input[..BLOCK * CH], &mut after);
            assert!(after.is_empty(), "@{speed}x: output right after reset");
            let mut fed = BLOCK;
            while fed < latency + 4 * BLOCK {
                stage.process(&input[fed * CH..(fed + BLOCK) * CH], &mut after);
                fed += BLOCK;
            }
            let head = &after[..256 * CH];
            let head_rms = (head.iter().map(|x| x * x).sum::<f32>() / head.len() as f32).sqrt();
            assert!(head_rms > 0.5 * in_rms, "@{speed}x: weak start after reset, rms {head_rms} vs input {in_rms}");
        }
    }
}
