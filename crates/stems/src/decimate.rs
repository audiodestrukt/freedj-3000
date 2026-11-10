//! 96 kHz → 48 kHz: an exact 2:1 decimation with a half-band FIR.
//!
//! The filter is centred (zero phase): output frame `m` lines up with
//! input frame `2m` exactly, so decimated stems stay sample-aligned with
//! the deck's own 48 kHz decode.  A half-band filter has every other tap
//! zero, so each output sample costs 16 multiply-adds.

/// Half-length of the filter: taps run from `-HALF` to `+HALF`.
const HALF: usize = 31;

/// Taps at odd offsets 1, 3, …, 31 (even offsets are zero; the centre is 0.5).
fn taps() -> [f32; HALF.div_ceil(2)] {
    // Kaiser-windowed sinc at half the input Nyquist (24 kHz at 96 kHz):
    // passband to ~20 kHz, anything above 28 kHz down > 80 dB.
    let beta = 8.0f64;
    let i0 = |x: f64| { // modified Bessel I0, series
        let (mut s, mut t) = (1.0, 1.0);
        for k in 1..40 { t *= (x / 2.0) * (x / 2.0) / (k * k) as f64; s += t; }
        s
    };
    let mut h = [0f32; HALF.div_ceil(2)];
    let mut sum = 0.5f64;
    for (i, tap) in h.iter_mut().enumerate() {
        let n = (2 * i + 1) as f64;
        let x = std::f64::consts::PI * n / 2.0;
        let w = i0(beta * (1.0 - (n / (HALF as f64 + 1.0)).powi(2)).sqrt()) / i0(beta);
        let v = 0.5 * x.sin() / x * w;
        *tap = v as f32;
        sum += 2.0 * v;
    }
    // Unity gain at DC: centre 0.5 plus both wings must sum to 1.
    let scale = 0.5 / (sum - 0.5);
    for tap in h.iter_mut() { *tap = (*tap as f64 * scale) as f32; }
    h
}

/// Decimate interleaved stereo i16 by 2.  Samples beyond the ends count as
/// silence.  Output has `ceil(frames / 2)` frames.
pub fn decimate2(x: &[i16]) -> Vec<i16> {
    let h = taps();
    let frames = x.len() / 2;
    let out_frames = frames.div_ceil(2);
    let mut out = Vec::with_capacity(out_frames * 2);
    let at = |f: isize, c: usize| -> f32 {
        if f < 0 || f as usize >= frames { 0.0 } else { x[f as usize * 2 + c] as f32 }
    };
    for m in 0..out_frames {
        let centre = (2 * m) as isize;
        for c in 0..2 {
            let mut acc = 0.5 * at(centre, c);
            // Inner loop without bounds juggling when fully inside.
            if centre >= HALF as isize && (centre as usize) + HALF < frames {
                let base = centre as usize;
                for (i, t) in h.iter().enumerate() {
                    let k = 2 * i + 1;
                    acc += t * (x[(base - k) * 2 + c] as f32 + x[(base + k) * 2 + c] as f32);
                }
            } else {
                for (i, t) in h.iter().enumerate() {
                    let k = (2 * i + 1) as isize;
                    acc += t * (at(centre - k, c) + at(centre + k, c));
                }
            }
            out.push(acc.round().clamp(-32768.0, 32767.0) as i16);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, rate: f64, frames: usize, amp: f64) -> Vec<i16> {
        (0..frames).flat_map(|i| {
            let v = (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin()).round() as i16;
            [v, -v]
        }).collect()
    }

    fn rms(x: &[i16]) -> f64 {
        (x.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn unity_dc_gain() {
        let x = vec![10_000i16; 2 * 4000];
        let y = decimate2(&x);
        assert_eq!(y.len(), 4000);
        assert!(y[1000..3000].iter().all(|s| (*s - 10_000).abs() <= 1), "{:?}", &y[1000..1004]);
    }

    #[test]
    fn passes_audio_and_keeps_alignment() {
        // 1 kHz at 96 kHz should come out as the same 1 kHz at 48 kHz,
        // sample-aligned (zero phase): compare with a tone made at 48 kHz.
        let x = tone(1000.0, 96_000.0, 96_000, 20_000.0);
        let y = decimate2(&x);
        let want = tone(1000.0, 48_000.0, 48_000, 20_000.0);
        let mid = 2000 * 2..46_000 * 2;
        let err = y[mid.clone()].iter().zip(&want[mid]).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(err <= 4, "max error {err}");
        // 18 kHz still passes at full level.
        let y = decimate2(&tone(18_000.0, 96_000.0, 96_000, 20_000.0));
        let r = rms(&y[2000..90_000]) / (20_000.0 / 2f64.sqrt());
        assert!((r - 1.0).abs() < 0.02, "18 kHz gain {r}");
    }

    #[test]
    fn rejects_what_would_alias() {
        // 40 kHz would fold to 8 kHz: it must be gone.
        let y = decimate2(&tone(40_000.0, 96_000.0, 96_000, 20_000.0));
        assert!(rms(&y[2000..90_000]) < 20_000.0 * 1e-4, "rms {}", rms(&y[2000..90_000]));
    }
}
