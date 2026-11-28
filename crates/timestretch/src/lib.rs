//! Pipeline stage implementations:
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

// The C++ engines are built only where build.rs can build them: it sets
// `cfg(rubberband)` and `cfg(bungee)` (not in a browser; Bungee also needs
// the `bungee` feature).  WSOLA, pure Rust, is always here.
#[cfg(rubberband)]
mod rubberband_sys;
#[cfg(rubberband)]
mod rubberband;
#[cfg(rubberband)]
pub use rubberband::TimestretechStage;
#[cfg(bungee)]
mod bungee_sys;
#[cfg(bungee)]
mod bungee;
#[cfg(bungee)]
pub use bungee::BungeeStage;
mod wsola;
pub use wsola::WsolaStage;

use opendeck_types::PipelineStage;

/// Block size fed to an engine at a time (frames, not samples).  512 gives
/// good latency at 44.1 kHz (~11 ms).
#[allow(dead_code)]   // no C++ engine in this build
pub(crate) const BLOCK_FRAMES: usize = 512;

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
    /// Whether this build has the engine: the C++ ones are not built in a
    /// browser, and Bungee only with the `bungee` feature.
    pub fn built(self) -> bool {
        match self { Engine::RubberBand => cfg!(rubberband), Engine::Bungee => cfg!(bungee), Engine::Wsola => true }
    }

    /// The best engine this build has: Rubber Band, else Bungee, else WSOLA.
    pub fn best() -> Engine {
        [Engine::RubberBand, Engine::Bungee, Engine::Wsola].into_iter().find(|e| e.built()).unwrap_or(Engine::Wsola)
    }

    /// `OPENDECK_STRETCH=rubberband|rb|r3`, `bungee` or `wsola`; default the
    /// best this build has ([`Engine::best`]).  Unknown values, and engines
    /// this build lacks, fall back to that with a warning.
    pub fn from_env() -> Engine {
        let want = match std::env::var("OPENDECK_STRETCH").ok().as_deref().map(|v| v.trim().to_ascii_lowercase()) {
            None => return Engine::best(),
            Some(v) => match v.as_str() {
                "rubberband" | "rb" | "r3" | "" => Engine::RubberBand,
                "bungee" => Engine::Bungee,
                "wsola" | "soundtouch" => Engine::Wsola,
                other => {
                    log::warn!("OPENDECK_STRETCH={other:?} unknown (rubberband|bungee|wsola); using {}", Engine::best().name());
                    return Engine::best();
                }
            },
        };
        if want.built() { return want; }
        log::warn!("OPENDECK_STRETCH asks for {} but this build has none; using {}", want.name(), Engine::best().name());
        Engine::best()
    }

    pub fn name(self) -> &'static str {
        match self { Engine::RubberBand => "Rubber Band R3", Engine::Bungee => "Bungee", Engine::Wsola => "WSOLA" }
    }
}

/// A key-lock stage of whichever engine.
pub enum Stretcher {
    #[cfg(rubberband)]
    RubberBand(TimestretechStage),
    #[cfg(bungee)]
    Bungee(BungeeStage),
    Wsola(WsolaStage),
}

impl Stretcher {
    /// A stage of `engine`, or of [`Engine::best`] if this build lacks it.
    pub fn new(engine: Engine, sample_rate: u32, channels: u8) -> Self {
        let engine = if engine.built() { engine } else { Engine::best() };
        match engine {
            #[cfg(rubberband)]
            Engine::RubberBand => Stretcher::RubberBand(TimestretechStage::new(sample_rate, channels)),
            #[cfg(bungee)]
            Engine::Bungee => Stretcher::Bungee(BungeeStage::new(sample_rate, channels)),
            // WSOLA, and (never chosen above) an engine not built.
            #[allow(unreachable_patterns)]
            _ => Stretcher::Wsola(WsolaStage::new(sample_rate, channels)),
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
            #[cfg(rubberband)]
            Stretcher::RubberBand(_) => Engine::RubberBand,
            #[cfg(bungee)]
            Stretcher::Bungee(_) => Engine::Bungee,
            Stretcher::Wsola(_) => Engine::Wsola,
        }
    }

    fn inner(&mut self) -> &mut dyn PipelineStage {
        match self {
            #[cfg(rubberband)]
            Stretcher::RubberBand(s) => s,
            #[cfg(bungee)]
            Stretcher::Bungee(s) => s,
            Stretcher::Wsola(s) => s,
        }
    }

    fn inner_ref(&self) -> &dyn PipelineStage {
        match self {
            #[cfg(rubberband)]
            Stretcher::RubberBand(s) => s,
            #[cfg(bungee)]
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

// ── Performance regression guard ──────────────────────────────────────────────
#[cfg(all(test, rubberband))]
mod perf {
    use super::*;
    use opendeck_types::PipelineStage;
    use opendeck_types::time::Instant;

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
    #[cfg(bungee)]
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
    #[cfg(bungee)]
    #[test]
    fn bungee_frame_accounting_and_signal() {
        for speed in [0.5f32, 0.92, 1.0, 1.08, 2.0] {
            let mut stage = BungeeStage::new(SR, CH as u8);
            frame_accounting_and_signal(&mut stage, speed);
        }
    }

    /// A CUE on a kick: after a reset the stream starts on a hard attack at
    /// its very first frame (no history).  The output's first 5 ms must carry
    /// that attack at full level and on time, at the speeds a DJ plays at.
    /// Before the start pad / trim, R3 output nothing for the first ~53 ms.
    #[test]
    fn rubberband_keeps_the_transient_at_a_cue() {
        let kick = |n: usize| -> Vec<f32> {
            (0..n).flat_map(|i| {
                let t = i as f32 / SR as f32;
                let f = 55.0 + 120.0 * (-t * 40.0).exp();
                let v = 0.7 * (2.0 * std::f32::consts::PI * f * t).sin() * (-t * 12.0).exp()
                      + if i < 40 { 0.4 * (1.0 - i as f32 / 40.0) } else { 0.0 };
                [v, v]
            }).collect()
        };
        let input = kick(SR as usize / 2);
        let peak_in = input[..(SR as usize / 200) * CH].iter().fold(0f32, |m, x| m.max(x.abs()));
        for speed in [0.92f32, 1.0, 1.08] {
            let mut stage = TimestretechStage::new(SR, CH as u8);
            // Something else played first, then CUE: set speed, reset, play.
            stage.set_speed(speed);
            let mut junk = Vec::new();
            stage.process(&signal(0.5), &mut junk);
            stage.reset();
            let mut out = Vec::new();
            for chunk in input.chunks(BLOCK * CH) {
                stage.set_speed(speed);
                stage.process(chunk, &mut out);
            }
            let head = &out[..(SR as usize / 200) * CH];   // first 5 ms
            let peak = head.iter().fold(0f32, |m, x| m.max(x.abs()));
            // R3 softens any attack a little (~0.75× here, as mid-track);
            // the bug was losing it outright (silence for the first 50 ms).
            assert!(peak > 0.6 * peak_in, "@{speed}x: attack peak {peak:.3} vs input {peak_in:.3}");
            let onset = out.iter().position(|x| x.abs() > 0.5 * peak_in).unwrap() / CH;
            assert!(onset < SR as usize / 500, "@{speed}x: attack {onset} frames late");
        }
    }

    /// `in / speed` frames per push, real audio, and after a reset full
    /// level from the seek point.
    /// Key lock: a 440 Hz tone stays 440 Hz at every tempo a DJ plays at,
    /// steady over time, in all three engines.  Pitch is measured from
    /// interpolated zero crossings in 100 ms windows, after 0.5 s of warm-up.
    /// `cargo test --release -p opendeck-timestretch key_lock -- --nocapture`
    /// prints the table.
    #[test]
    fn key_lock_holds_pitch() {
        const HZ: f64 = 440.0;
        let tone: Vec<f32> = (0..SR as usize * 4)
            .flat_map(|i| { let v = 0.5 * (2.0 * std::f64::consts::PI * HZ * i as f64 / SR as f64).sin() as f32; [v, v] })
            .collect();
        let mut engines: Vec<(&str, fn() -> Box<dyn PipelineStage>)> = vec![
            ("rubberband", || Box::new(TimestretechStage::new(SR, CH as u8))),
            ("wsola", || Box::new(WsolaStage::new(SR, CH as u8))),
        ];
        #[cfg(bungee)]
        engines.push(("bungee", || Box::new(BungeeStage::new(SR, CH as u8))));
        for (name, make) in engines {
            for speed in [0.84f32, 0.92, 0.97, 1.03, 1.08, 1.16] {
                let mut st = make();
                let mut out = Vec::new();
                for chunk in tone.chunks(BLOCK * CH) { st.set_speed(speed); st.process(chunk, &mut out); }
                let left: Vec<f32> = out.chunks(CH).map(|f| f[0]).collect();
                let left = &left[SR as usize / 2..];
                // Rising zero crossings, to a fraction of a sample.
                let xs: Vec<f64> = left.windows(2).enumerate()
                    .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
                    .map(|(i, w)| i as f64 + (w[0] / (w[0] - w[1])) as f64)
                    .collect();
                let win = SR as f64 / 10.0;
                let mut cents = Vec::new();
                let mut k = 0;
                while k < xs.len() {
                    let end = xs[k] + win;
                    let j = xs[k..].iter().position(|&x| x >= end).map_or(xs.len(), |p| k + p);
                    if j >= xs.len() { break; }
                    let f = (j - 1 - k) as f64 / ((xs[j - 1] - xs[k]) / SR as f64);
                    cents.push(1200.0 * (f / HZ).log2());
                    k = j;
                }
                let mean = cents.iter().sum::<f64>() / cents.len() as f64;
                let worst = cents.iter().fold(0f64, |m, c| m.max(c.abs()));
                println!("{name:<10} {:+5.1} %  mean {mean:+6.2} cents  worst 100 ms {worst:5.2} cents  ({} windows)",
                         (speed - 1.0) * 100.0, cents.len());
                assert!(cents.len() >= 10, "{name} at {speed}: too little output");
                assert!(mean.abs() < 1.0, "{name} at {speed}: pitch {mean:+.2} cents off");
                assert!(worst < 5.0, "{name} at {speed}: a 100 ms window {worst:.2} cents off");
            }
        }
    }

    /// KEY SHIFT: a 440 Hz tone moved by n semitones comes out at 440·2^(n/12)
    /// at the tempo the fader sets (the length still follows the speed), in
    /// all three engines, with key lock on as the deck plays it.
    #[test]
    fn key_shift_moves_pitch_by_semitones() {
        const HZ: f64 = 440.0;
        let tone: Vec<f32> = (0..SR as usize * 4)
            .flat_map(|i| { let v = 0.5 * (2.0 * std::f64::consts::PI * HZ * i as f64 / SR as f64).sin() as f32; [v, v] })
            .collect();
        let mut engines: Vec<(&str, fn() -> Box<dyn PipelineStage>)> = vec![
            ("rubberband", || Box::new(TimestretechStage::new(SR, CH as u8))),
            ("wsola", || Box::new(WsolaStage::new(SR, CH as u8))),
        ];
        #[cfg(bungee)]
        engines.push(("bungee", || Box::new(BungeeStage::new(SR, CH as u8))));
        for (name, make) in engines {
            for speed in [1.0f32, 1.08] {
                for shift in [-2i32, 1, 3] {
                    let mut st = make();
                    let mut out = Vec::new();
                    for chunk in tone.chunks(BLOCK * CH) {
                        st.set_speed(speed);
                        st.set_pitch_semitones(shift as f32);
                        st.process(chunk, &mut out);
                    }
                    let left: Vec<f32> = out.chunks(CH).map(|f| f[0]).collect();
                    let body = &left[SR as usize / 2..left.len() - SR as usize / 4];
                    let xs: Vec<f64> = body.windows(2).enumerate()
                        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
                        .map(|(i, w)| i as f64 + (w[0] / (w[0] - w[1])) as f64)
                        .collect();
                    let hz = (xs.len() - 1) as f64 / ((xs[xs.len() - 1] - xs[0]) / SR as f64);
                    let cents = 1200.0 * (hz / HZ).log2() - 100.0 * shift as f64;
                    let want_frames = (tone.len() / CH) as f64 / speed as f64;
                    let frames = out.len() as f64 / CH as f64;
                    println!("{name:<10} {:+4.0} %  shift {shift:+}  {hz:7.2} Hz  {cents:+6.2} cents off  {frames:.0}/{want_frames:.0} frames",
                             (speed - 1.0) * 100.0);
                    assert!(cents.abs() < 3.0, "{name} @{speed} shift {shift:+}: {hz:.2} Hz, {cents:+.2} cents off");
                    assert!((frames - want_frames).abs() < 0.05 * want_frames, "{name} @{speed} shift {shift:+}: tempo changed ({frames} frames, want ~{want_frames})");
                }
            }
        }
    }

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
