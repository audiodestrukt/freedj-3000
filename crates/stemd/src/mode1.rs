//! stemd's DSP mode 1: a fixed 44.1 → 96 kHz polyphase resampler that
//! reproduces the CDJ-3000's own converter, so a deck that rebuilds
//! `drums = mix − harmonics − vocals` from a mix it resampled itself gets
//! an exact cancel.  `L = 320`, `M = 147`, 59 taps a branch, an 18,880-tap
//! prototype.
//!
//! Ported from stemd (`crates/stemd-core/src/resample/mode1.rs`, commit
//! 8f5e143a1812), © 2026 Nicolas Saintot, used under the MIT licence
//! (`data/LICENSE-MIT-stemd`).  The prototype and the reference vectors in
//! `data/` are stemd's: measured by its author by driving the converter
//! with impulses (black-box), not taken from any firmware.  Nothing here
//! was read from Pioneer's software.

use std::sync::OnceLock;

pub const IN_RATE: u32 = 44_100;
pub const OUT_RATE: u32 = 96_000;
const L: usize = 320;
const M: usize = 147;
const BRANCH: usize = 59;

static PROTO_LE: &[u8] = include_bytes!("../data/mode1_proto_f32.bin");

/// The prototype split into its 320 branches, each reversed:
/// `branch[r][i] = p[r + 320 * (58 - i)]`.
fn branches() -> &'static [[f32; BRANCH]] {
    static B: OnceLock<Vec<[f32; BRANCH]>> = OnceLock::new();
    B.get_or_init(|| {
        let p: Vec<f32> = PROTO_LE.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        assert_eq!(p.len(), L * BRANCH, "prototype is not 320 x 59 taps");
        (0..L).map(|r| std::array::from_fn(|i| p[r + L * (BRANCH - 1 - i)])).collect()
    })
}

/// Output frames `m0 ..` of one channel (`dst.len()` of them).
fn fill(src: &[f32], m0: usize, dst: &mut [f32], b: &[[f32; BRANCH]]) {
    let start = m0 * M;
    let mut newest = start / L;
    let mut r = start % L;
    for o in dst.iter_mut() {
        let taps = &b[r];
        let acc: f64 = if newest >= BRANCH - 1 && newest < src.len() {
            src[newest + 1 - BRANCH..=newest].iter().zip(taps).map(|(x, t)| f64::from(*x) * f64::from(*t)).sum()
        } else {
            // The window hangs off the signal: zeros there, as on the device.
            (0..BRANCH).filter_map(|j| {
                let k = newest.checked_sub(j)?;
                Some(f64::from(*src.get(k)?) * f64::from(taps[BRANCH - 1 - j]))
            }).sum()
        };
        *o = acc as f32;
        r += M;
        if r >= L { r -= L; newest += 1; }
    }
}

/// Output frames for `frames` input frames: the ratio, truncated.
pub fn out_frames(frames: usize) -> usize { frames * L / M }

/// Interleaved stereo at 44.1 kHz → interleaved stereo at 96 kHz, as the
/// CDJ-3000 converts it.
pub fn resample_stereo(x: &[f32]) -> Vec<f32> {
    let b = branches();
    let planes: [Vec<f32>; 2] = std::array::from_fn(|c| x.iter().skip(c).step_by(2).copied().collect());
    let n_out = out_frames(planes[0].len());
    let par = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = n_out.div_ceil(par.div_ceil(2).max(1)).max(1);
    let mut outs: [Vec<f32>; 2] = std::array::from_fn(|_| vec![0f32; n_out]);
    std::thread::scope(|s| {
        for (src, out) in planes.iter().zip(outs.iter_mut()) {
            for (i, dst) in out.chunks_mut(chunk).enumerate() {
                s.spawn(move || fill(src, i * chunk, dst, b));
            }
        }
    });
    let mut y = Vec::with_capacity(n_out * 2);
    for i in 0..n_out { y.push(outs[0][i]); y.push(outs[1][i]); }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32s(bytes: &[u8]) -> Vec<f32> { bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect() }

    /// stemd's reference: full-band noise captured through the device's
    /// converter (2205 input frames, every branch used).  Our port nulls
    /// against it to better than −120 dB, as stemd's own test requires.
    #[test]
    fn matches_the_reference_exactly() {
        let input = f32s(include_bytes!("../data/mode1_ref_in_f32.bin"));
        let want = f32s(include_bytes!("../data/mode1_ref_out_f32.bin"));
        let got = resample_stereo(&input);
        assert_eq!(got.len(), want.len(), "frames: {} vs {}", got.len() / 2, want.len() / 2);
        let (mut err, mut sig) = (0f64, 0f64);
        for (g, w) in got.iter().zip(&want) { err += f64::from(g - w).powi(2); sig += f64::from(*w).powi(2); }
        let db = 10.0 * (err / sig).log10();
        assert!(db < -120.0, "{db:.1} dB against the device");
    }
}
