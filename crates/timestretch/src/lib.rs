//! Two pipeline stage implementations:
//!
//! - `ResampleStage`:      speed change without pitch change (no key-lock).
//!                         Uses `rubato` (pure Rust, sinc interpolation).
//!
//! - `TimestretechStage`:  speed change with constant pitch (key-lock).
//!                         Wraps Rubber Band Library R3 engine via C FFI.

mod rubberband_sys;

use opendeck_types::PipelineStage;
use rubberband_sys as rb;

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
    fn run(stage: &mut TimestretechStage, input: &[f32]) -> std::time::Duration {
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
}
