//! Beat grid detection — works identically for offline (batch) and online (streaming) use.
//!
//! The caller manages chunking.  This struct just sees a stream of mono samples.
//!
//! Pipeline:
//!   1. Tempo detection via MiniBPM (6 s analysis window)
//!   2. Beat phase estimation by maximising onset energy at grid positions
//!   3. Downbeat detection (bar structure) — TODO

use std::sync::Arc;
use opendeck_types::{BeatAnalyzer, BeatGrid};
use minibpm_sys::MiniBpm;

/// Minimum audio (seconds) before a grid can be fit at all.
const AC_WINDOW_SEC: f32 = 6.0;
/// Upper bound (seconds) on the leading span analysed for tempo + phase.  Long
/// enough for a rock-solid global phase fit, bounded so a 10-minute track does
/// not pay for its whole length at load.
const ANALYSIS_SPAN_SEC: f32 = 120.0;
/// Minimum BPM to consider.
const BPM_MIN: f32 = 60.0;
/// Maximum BPM to consider.
const BPM_MAX: f32 = 200.0;
/// How many seconds of audio before we emit a first estimate.
const WARM_UP_SEC: f32 = 5.0;
/// How many seconds before we call the grid "stable".
const STABLE_SEC: f32 = 15.0;

pub struct BeatAnalyzerImpl {
    sample_rate: u32,
    /// Accumulated mono samples (downmixed).
    samples:     Vec<f32>,
    /// Current best grid estimate.
    grid:        Option<Arc<BeatGrid>>,
    /// True once we have STABLE_SEC of audio.
    stable:      bool,
}

impl BeatAnalyzerImpl {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            samples: Vec::with_capacity(sample_rate as usize * 60),
            grid: None,
            stable: false,
        }
    }

    fn seconds_accumulated(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate as f32
    }

    fn run_analysis(&mut self) {
        let sr = self.sample_rate as f32;
        let ac_len = (AC_WINDOW_SEC * sr) as usize;
        if self.samples.len() < ac_len {
            return;
        }

        // Analyse as much of the track as we have — bounded so a very long
        // track can't blow up load time.  Crucially this is the *leading* span,
        // not the trailing one: the earlier code fit the grid to the LAST 6 s
        // of the track (an outro / breakdown / filter sweep — the worst place
        // to find the beat), which threw the phase off for the clean kicks at
        // the start.  Fitting over a long leading span both picks a steadier
        // BPM and least-squares-aligns the phase to every kick, so the grid
        // lines up where the beat is actually visible.
        let span = self.samples.len().min((ANALYSIS_SPAN_SEC * sr) as usize);
        let window = &self.samples[..span];

        // ── Stage 1: tempo via MiniBPM ────────────────────────────────────────
        let mut detector = MiniBpm::new(sr);
        detector.set_bpm_range(BPM_MIN as f64, BPM_MAX as f64);
        let bpm = match detector.estimate_tempo(window) {
            Some(b) => {
                log::info!("MiniBPM: {:.1} BPM", b);
                b as f32
            }
            None => {
                log::debug!("MiniBPM returned no estimate");
                return;
            }
        };

        // ── Stage 2: beat phase ───────────────────────────────────────────────
        // Global phase fit across the whole analysed span: the anchor is chosen
        // to maximise onset energy summed over grid positions spanning the
        // track, so it best-fits every beat rather than one 6 s window.
        let onset = onset_strength(window, sr);
        let (bpm, anchor_sample) = fit_grid(&onset, bpm, sr);

        // ── Stage 3: downbeat ─────────────────────────────────────────────────
        // TODO: implement bar-level autocorrelation

        // MiniBPM doesn't return a confidence score; use a fixed value.
        const CONFIDENCE: f32 = 0.8;
        let mut grid = BeatGrid::new_constant(anchor_sample as u64, bpm);
        grid.confidence = CONFIDENCE;

        self.stable = self.seconds_accumulated() >= STABLE_SEC;
        self.grid = Some(Arc::new(grid));
    }
}

impl BeatAnalyzer for BeatAnalyzerImpl {
    fn push(&mut self, samples: &[f32], sample_rate: u32) {
        // Downmix to mono and accumulate.
        debug_assert_eq!(sample_rate, self.sample_rate,
            "sample rate changed mid-stream — re-create the analyzer");

        // Assume interleaved stereo input.
        let sr = self.sample_rate as usize;
        let prev_len = self.samples.len();
        for chunk in samples.chunks(2) {
            let mono = if chunk.len() == 2 {
                (chunk[0] + chunk[1]) * 0.5
            } else {
                chunk[0]
            };
            self.samples.push(mono);
        }

        // Re-run analysis once per second of new data after warm-up.
        let warm_up = (WARM_UP_SEC * self.sample_rate as f32) as usize;
        let new_len = self.samples.len();
        if new_len >= warm_up {
            let prev_sec = prev_len / sr;
            let new_sec  = new_len  / sr;
            if new_sec > prev_sec {
                self.run_analysis();
            }
        }
    }

    fn beat_grid(&self) -> Option<Arc<BeatGrid>> {
        self.grid.clone()
    }

    fn is_stable(&self) -> bool {
        self.stable
    }
}

// ── DSP helpers ───────────────────────────────────────────────────────────────

/// Compute an onset strength signal from mono PCM using half-wave rectified
/// spectral flux.  Operates on sub-bands so kick, snare, and hi-hat all
/// contribute, making the result more robust than a single-band energy
/// differentiator.  Returns ~86 values/sec at hop=512.
/// Spectral-flux onset strength per 512-sample hop, for the beat-phase vote.
///
/// Three bands (bass < 300 Hz, mids to 3 kHz, highs above), each band's flux
/// normalised by its own mean over the track, then weighted mids 2 : bass 1 :
/// highs 0.25.  Summing raw per-bin flux instead let the highs win on bin
/// count alone (~440 bins above 3 kHz against ~7 below 300 Hz), so the open
/// hats on the off-beat out-voted the kick: measured against rekordbox on a
/// 226-track stick, 50 grids sat half a beat out; with the bands normalised,
/// 27 (examples/analysis_vs_rekordbox).  The kick's click lands in the mids,
/// which is why they carry the most weight.
fn onset_strength(samples: &[f32], sr: f32) -> Vec<f32> {
    use rustfft::{num_complex::Complex, FftPlanner};

    const HOP:    usize = 512;
    const WINDOW: usize = 1024;
    const WEIGHTS: [f32; 3] = [1.0, 2.0, 0.25];   // bass, mids, highs

    let n_hops = samples.len().saturating_sub(WINDOW) / HOP;
    if n_hops == 0 {
        return vec![];
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(WINDOW);
    let hann: Vec<f32> = (0..WINDOW)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (WINDOW - 1) as f32).cos()))
        .collect();

    let bin_hz   = sr / WINDOW as f32;
    let n_bins   = WINDOW / 2;
    let bass_end = ((300.0 / bin_hz) as usize).clamp(2, n_bins);
    let mid_end  = ((3000.0 / bin_hz) as usize).clamp(bass_end + 1, n_bins);
    let ranges   = [(1, bass_end), (bass_end, mid_end), (mid_end, n_bins)];

    let mut bands    = [vec![0f32; n_hops], vec![0f32; n_hops], vec![0f32; n_hops]];
    let mut prev_mag = vec![0f32; n_bins];
    for h in 0..n_hops {
        let start = h * HOP;
        let mut buf: Vec<Complex<f32>> = samples[start..start + WINDOW]
            .iter()
            .zip(hann.iter())
            .map(|(&s, &w)| Complex::new(s * w, 0.0))
            .collect();
        fft.process(&mut buf);
        let mag: Vec<f32> = buf[..n_bins].iter().map(|c| c.norm() / WINDOW as f32).collect();
        for (band, &(lo, hi)) in bands.iter_mut().zip(&ranges) {
            band[h] = (lo..hi).map(|b| (mag[b] - prev_mag[b]).max(0.0)).sum();
        }
        prev_mag = mag;
    }

    let mut onset = vec![0f32; n_hops];
    for (band, w) in bands.iter().zip(WEIGHTS) {
        let mean = band.iter().sum::<f32>() / n_hops as f32;
        if mean <= 0.0 { continue; }
        for (o, x) in onset.iter_mut().zip(band) { *o += w * x / mean; }
    }
    onset
}

/// Tempo and phase together: the constant grid that collects the most onset
/// energy over the whole analysed span, searched within ±0.4 BPM of
/// MiniBPM's estimate, to a thousandth of a BPM and a tenth of a hop.
/// MiniBPM alone reads a little off (124.032 for an exact 124; 125.75 where
/// rekordbox says 126.00), which drifts a long track off its grid.
///
/// The anchor is then moved `REKORDBOX_LEAD_MS` later: rekordbox (whose grids
/// the players and SYNC use) puts a beat ~6 ms before the attack, and our
/// flux peak lands ~12 ms before it (the 1024-sample window sees an attack
/// half a window early).  Measured on a 226-track stick: tempo exact on
/// 222 (was 198), grid within 10 ms of rekordbox's on ~198 (was 164).
/// Returns (BPM, anchor sample).
fn fit_grid(onset: &[f32], bpm0: f32, sr: f32) -> (f64, usize) {
    const HOP: usize = 512;
    let hop_rate = sr as f64 / HOP as f64;
    if onset.len() < 8 { return (bpm0 as f64, 0); }
    // A little smoothing so a beat a few ms off its hop still scores.
    let sm: Vec<f32> = (0..onset.len()).map(|i| {
        let g = |d: isize| onset.get((i as isize + d).max(0) as usize).copied().unwrap_or(0.0);
        0.25 * g(-2) + 0.5 * g(-1) + g(0) + 0.5 * g(1) + 0.25 * g(2)
    }).collect();
    let at = |x: f64| -> f32 {
        let i = x.floor() as usize;
        if i + 1 >= sm.len() { return 0.0; }
        let f = (x - i as f64) as f32;
        sm[i] * (1.0 - f) + sm[i + 1] * f
    };
    let score = |bpm: f64, phase: f64| -> f32 {
        let period = hop_rate * 60.0 / bpm;
        let mut s = 0f32;
        let mut pos = phase;
        while pos + 1.0 < sm.len() as f64 { s += at(pos); pos += period; }
        s
    };
    // Best phase for one tempo, to `step` hops.
    let best_phase = |bpm: f64, lo: f64, hi: f64, step: f64| -> (f64, f32) {
        let mut best = (lo, f32::NEG_INFINITY);
        let mut ph = lo;
        while ph < hi { let sc = score(bpm, ph); if sc > best.1 { best = (ph, sc); } ph += step; }
        best
    };
    let period0 = |bpm: f64| hop_rate * 60.0 / bpm;

    // Coarse: ±0.4 BPM in 0.01, phase in half hops.
    let b0 = bpm0 as f64;
    let mut best = (b0, 0.0, f32::NEG_INFINITY);
    let mut b = b0 - 0.4;
    while b <= b0 + 0.4 + 1e-9 {
        let (ph, sc) = best_phase(b, 0.0, period0(b), 0.5);
        if sc > best.2 { best = (b, ph, sc); }
        b += 0.01;
    }
    // Fine: ±0.01 BPM in 0.001, phase ±1 hop in 0.1.
    let (cb, cp) = (best.0, best.1);
    let mut b = cb - 0.01;
    while b <= cb + 0.01 + 1e-9 {
        let (ph, sc) = best_phase(b, cp - 1.0, cp + 1.0, 0.1);
        if sc > best.2 { best = (b, ph, sc); }
        b += 0.001;
    }
    const REKORDBOX_LEAD_MS: f64 = 6.0;
    let period = period0(best.0);
    let phase = (best.1 + REKORDBOX_LEAD_MS / 1000.0 * hop_rate).rem_euclid(period);
    (best.0, (phase * HOP as f64).round() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    /// `secs` of a four-on-the-floor loop at `bpm`, first kick at `first` s:
    /// a kick (pitch-dropping sine + click) on every beat and, with
    /// `hats`, a loud open hat (decaying noise) on every off-beat.
    fn groove(bpm: f64, first: f64, secs: f64, hats: f32) -> Vec<f32> {
        let n = (secs * SR as f64) as usize;
        let mut x = vec![0f32; n];
        let per = 60.0 / bpm;
        let mut lcg: u32 = 7;
        let mut k = 0;
        loop {
            let t0 = first + k as f64 * per;
            if t0 + per >= secs { break; }
            let s = (t0 * SR as f64).round() as usize;
            for i in 0..(0.25 * SR as f64) as usize {
                let t = i as f32 / SR as f32;
                // Body: a pitch-dropping sine.  Knock: a short 1.5 kHz burst,
                // where a real kick's click sits (not a broadband spike).
                let v = (2.0 * std::f32::consts::PI * (50.0 + 110.0 * (-t * 35.0).exp()) * t).sin() * (-t * 14.0).exp()
                      + 0.5 * (2.0 * std::f32::consts::PI * 1500.0 * t).sin() * (-t * 250.0).exp();
                if s + i < n { x[s + i] += v; }
            }
            let h = ((t0 + per / 2.0) * SR as f64).round() as usize;
            // Open hat: high-passed noise (first difference), ringing ~0.2 s.
            let mut prev = 0f32;
            for i in 0..(0.2 * SR as f64) as usize {
                lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (lcg >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                let hp = noise - prev;
                prev = noise;
                if h + i < n { x[h + i] += hats * hp * (-(i as f32) / SR as f32 * 15.0).exp(); }
            }
            k += 1;
        }
        x.into_iter().flat_map(|v| [0.5 * v, 0.5 * v]).collect()
    }

    fn analyse(pcm: &[f32]) -> BeatGrid {
        let mut a = BeatAnalyzerImpl::new(SR);
        a.push(pcm, SR);
        (*a.beat_grid().expect("a grid")).clone()
    }

    /// Distance (ms) from the grid's nearest line to the true beat phase.
    fn phase_err_ms(g: &BeatGrid, first: f64) -> f64 {
        let per = 60_000.0 / g.bpm;
        let d = (g.anchor_sample as f64 / SR as f64 * 1000.0 - first * 1000.0).rem_euclid(per);
        d.min(per - d)
    }

    #[test]
    fn finds_tempo_and_phase_of_a_plain_kick_loop() {
        for bpm in [96.0, 124.0, 128.0, 140.0] {
            let g = analyse(&groove(bpm, 0.25, 40.0, 0.0));
            // MiniBPM alone was up to ~0.3 off; the grid fit pins it.
            assert!((g.bpm - bpm).abs() < 0.02, "{bpm}: got {}", g.bpm);
            assert!(phase_err_ms(&g, 0.25) < 15.0, "{bpm}: phase {} ms off", phase_err_ms(&g, 0.25));
        }
    }

    /// Open hats on the off-beat, as loud as the kick: the grid stays on the
    /// kick.  Before the per-band normalisation the hats' many high bins
    /// out-voted it and the grid sat half a beat out.
    #[test]
    fn grid_stays_on_the_kick_under_loud_offbeat_hats() {
        for bpm in [122.0, 126.0, 130.0] {
            let g = analyse(&groove(bpm, 0.1, 40.0, 1.5));
            let err = phase_err_ms(&g, 0.1);
            assert!(err < 15.0, "{bpm}: grid {err:.1} ms from the kick (half a beat = {:.0})", 30_000.0 / bpm);
        }
    }

    /// Known gap: drum & bass reads at half tempo (174 → 87), because the
    /// tempo range is 60–200 and MiniBPM prefers the lower octave.  Rekordbox
    /// shows 174.  Next on the BPM list (with the 4:5 and 2:3 misreads).
    #[test]
    #[ignore]
    fn drum_and_bass_reads_at_full_tempo() {
        let g = analyse(&groove(174.0, 0.25, 40.0, 0.0));
        assert!((g.bpm - 174.0).abs() < 0.1, "got {}", g.bpm);
    }

    /// Under the warm-up (5 s) there is no grid yet; the deck shows none.
    #[test]
    fn too_short_for_a_grid() {
        let mut a = BeatAnalyzerImpl::new(SR);
        a.push(&groove(120.0, 0.0, 4.0, 0.0), SR);
        assert!(a.beat_grid().is_none());
    }
}
