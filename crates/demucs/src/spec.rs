//! HTDemucs's spectrogram and its inverse, matching `HTDemucs._spec` /
//! `_ispec` (torch.stft / istft, n_fft 4096, hop 1024, periodic Hann,
//! `normalized=True`, `center=True` with reflect padding).

use realfft::num_complex::Complex32;
use realfft::RealFftPlanner;

pub const NFFT: usize = 4096;
pub const HOP: usize = NFFT / 4;
/// Frequency bins kept (the Nyquist bin is dropped).
pub const BINS: usize = NFFT / 2;

fn hann() -> Vec<f32> {
    (0..NFFT).map(|n| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / NFFT as f64).cos()) as f32).collect()
}

/// Reflect-pad (numpy/torch "reflect": the edge sample is not repeated).
fn reflect(x: &[f32], left: usize, right: usize) -> Vec<f32> {
    let n = x.len() as isize;
    assert!(left < x.len() && right < x.len());
    (-(left as isize)..n + right as isize)
        .map(|i| x[if i < 0 { -i } else if i >= n { 2 * (n - 1) - i } else { i } as usize])
        .collect()
}

/// Number of spectrogram frames for `len` samples.
pub fn frames(len: usize) -> usize { len.div_ceil(HOP) }

/// One channel's spectrogram: `(BINS, frames(len))` complex, frequency-major.
pub fn spec(x: &[f32], planner: &mut RealFftPlanner<f32>) -> Vec<Complex32> {
    let le = frames(x.len());
    let pad = HOP / 2 * 3;
    let x = reflect(x, pad, pad + le * HOP - x.len());
    let x = reflect(&x, NFFT / 2, NFFT / 2);            // center=True
    let win = hann();
    let fft = planner.plan_fft_forward(NFFT);
    let mut buf = fft.make_input_vec();
    let mut out = fft.make_output_vec();
    let scale = 1.0 / (NFFT as f32).sqrt();              // normalized=True
    let mut z = vec![Complex32::new(0.0, 0.0); BINS * le];
    for t in 0..le {
        let s = (t + 2) * HOP;                             // frames [2, 2+le) of le+4
        for (i, b) in buf.iter_mut().enumerate() { *b = x[s + i] * win[i] }
        fft.process(&mut buf, &mut out).expect("fft");
        for f in 0..BINS { z[f * le + t] = out[f] * scale }
    }
    z
}

/// Inverse of [`spec`] for one channel: `(BINS, le)` complex → `len` samples.
pub fn ispec(z: &[Complex32], le: usize, len: usize, planner: &mut RealFftPlanner<f32>) -> Vec<f32> {
    let pad = HOP / 2 * 3;
    let target = HOP * len.div_ceil(HOP) + 2 * pad;       // istft `length`
    let nfr = le + 4;                                      // two zero frames each side
    let win = hann();
    let ifft = planner.plan_fft_inverse(NFFT);
    let mut spec = ifft.make_input_vec();
    let mut frame = ifft.make_output_vec();
    let full = NFFT + HOP * (nfr - 1);
    let mut y = vec![0f32; full];
    let mut env = vec![0f32; full];
    // irfft is unnormalised (×N); torch's is ×1/N, then ×sqrt(N) for normalized.
    let scale = (NFFT as f32).sqrt() / NFFT as f32;
    for fr in 0..nfr {
        let s = fr * HOP;
        for (i, w) in win.iter().enumerate() { env[s + i] += w * w }
        if fr < 2 || fr >= le + 2 { continue }             // zero frames add nothing
        let t = fr - 2;
        for f in 0..BINS { spec[f] = z[f * le + t] }
        spec[BINS] = Complex32::new(0.0, 0.0);             // the dropped Nyquist bin
        spec[0].im = 0.0;
        ifft.process(&mut spec, &mut frame).expect("ifft");
        for i in 0..NFFT { y[s + i] += frame[i] * scale * win[i] }
    }
    let start = NFFT / 2;                                  // center=True
    (0..len).map(|i| { let j = start + pad + i; debug_assert!(j < start + target); y[j] / env[j] }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spectrogram_inverts_to_the_same_audio() {
        let len = 50_000;
        let x: Vec<f32> = (0..len).map(|i| ((i as f32 * 0.013).sin() + 0.3 * (i as f32 * 0.31).cos()) * 0.5).collect();
        let mut p = RealFftPlanner::new();
        let z = spec(&x, &mut p);
        assert_eq!(z.len(), BINS * frames(len));
        let y = ispec(&z, frames(len), len, &mut p);
        // HTDemucs drops the two outermost frames at each end (`_spec`) and
        // puts zeros back (`_ispec`), so the first and last 1.5 hops come back
        // dimmed; chunk overlap hides that. Everything between is exact.
        let edge = 2 * HOP;
        let err = x[edge..len - edge].iter().zip(&y[edge..len - edge]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(err < 1e-4, "round trip error {err}");
    }
}
