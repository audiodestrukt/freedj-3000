//! The handful of kernels HTDemucs needs. Every convolution is expressed as a
//! strided matrix multiply over the input in place (no im2col copies).

/// A matrix view: `d[off + i*rs + j*cs]`.
#[derive(Clone, Copy)]
pub struct M<'a> { pub d: &'a [f32], pub off: usize, pub rs: usize, pub cs: usize }

impl<'a> M<'a> {
    pub fn new(d: &'a [f32], off: usize, rs: usize, cs: usize) -> Self { Self { d, off, rs, cs } }
    fn check(&self, rows: usize, cols: usize) {
        if rows == 0 || cols == 0 { return }
        let last = self.off + (rows - 1) * self.rs + (cols - 1) * self.cs;
        assert!(last < self.d.len(), "matrix view out of bounds: {last} >= {}", self.d.len());
    }
}

/// `C[m×n] = alpha·A[m×k]·B[k×n] + beta·C`, C at `c[coff + i*rsc + j*csc]`.
#[allow(clippy::too_many_arguments)]
pub fn gemm(m: usize, k: usize, n: usize, alpha: f32, a: M, b: M, beta: f32,
            c: &mut [f32], coff: usize, rsc: usize, csc: usize) {
    if m == 0 || n == 0 { return }
    a.check(m, k);
    b.check(k, n);
    M::new(c, coff, rsc, csc).check(m, n);
    // SAFETY: every element each view can touch was bounds-checked above.
    unsafe {
        matrixmultiply::sgemm(m, k, n, alpha,
            a.d.as_ptr().add(a.off), a.rs as isize, a.cs as isize,
            b.d.as_ptr().add(b.off), b.rs as isize, b.cs as isize,
            beta, c.as_mut_ptr().add(coff), rsc as isize, csc as isize);
    }
}

use rayon::prelude::*;

/// Elements below which an elementwise pass stays on one thread.
const PAR_ELEMS: usize = 1 << 15;

/// e^x, relative error < 2e-7 over [-87, 88] (clamped outside). Branch-free,
/// so loops over it vectorise (libm's expf does not).
#[inline(always)]
pub fn exp(x: f32) -> f32 {
    const ROUND: f32 = 12_582_912.0;                  // 1.5·2^23: x + ROUND - ROUND rounds to nearest
    let x = x.clamp(-87.0, 88.0);
    let n = (x * std::f32::consts::LOG2_E + ROUND) - ROUND;
    let r = (x - n * 0.693_145_75) - n * 1.428_606_8e-6; // Cody-Waite ln 2
    let p = 1.0 + r * (1.0 + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0 + r * (1.0 / 720.0 + r * (1.0 / 5040.0)))))));
    p * f32::from_bits(((n as i32 + 127) as u32) << 23)
}

/// erf(x), absolute error < 1.5e-7 (Abramowitz & Stegun 7.1.26), branch-free.
#[inline(always)]
pub fn erf(x: f32) -> f32 {
    let a = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * a);
    let poly = ((((1.061_405_4 * t - 1.453_152) * t + 1.421_413_8) * t - 0.284_496_74) * t + 0.254_829_6) * t;
    (1.0 - poly * exp(-a * a)).copysign(x)
}

/// Exact (erf) GELU, as `F.gelu` / `nn.GELU()`.
#[inline(always)]
pub fn gelu(x: f32) -> f32 { 0.5 * x * (1.0 + erf(x * std::f32::consts::FRAC_1_SQRT_2)) }

pub fn gelu_all(x: &mut [f32]) {
    if x.len() < PAR_ELEMS { for v in x { *v = gelu(*v) } return }
    x.par_chunks_mut(PAR_ELEMS).for_each(|c| for v in c { *v = gelu(*v) });
}

#[inline(always)]
fn sigmoid(x: f32) -> f32 { 1.0 / (1.0 + exp(-x)) }

/// GLU over the channel axis of a `(2c, t)` block: `a * sigmoid(b)` → `(c, t)`.
pub fn glu(x: &[f32], c: usize, t: usize, out: &mut [f32]) {
    let (a, b) = x[..2 * c * t].split_at(c * t);
    let o = &mut out[..c * t];
    let run = |o: &mut [f32], a: &[f32], b: &[f32]| for ((o, a), b) in o.iter_mut().zip(a).zip(b) { *o = a * sigmoid(*b) };
    if o.len() < PAR_ELEMS { return run(o, a, b) }
    o.par_chunks_mut(PAR_ELEMS).zip(a.par_chunks(PAR_ELEMS)).zip(b.par_chunks(PAR_ELEMS))
        .for_each(|((o, a), b)| run(o, a, b));
}

/// Mean and biased variance, in f64, threaded for big blocks.
fn moments(x: &[f32]) -> (f64, f64) {
    let n = x.len() as f64;
    let sum = |c: &[f32]| c.iter().map(|&v| v as f64).sum::<f64>();
    let mean = if x.len() < PAR_ELEMS { sum(x) } else { x.par_chunks(PAR_ELEMS).map(sum).sum() } / n;
    let dev = |c: &[f32]| c.iter().map(|&v| { let d = v as f64 - mean; d * d }).sum::<f64>();
    let var = if x.len() < PAR_ELEMS { dev(x) } else { x.par_chunks(PAR_ELEMS).map(dev).sum() } / n;
    (mean, var)
}

/// `x += s[row] * g` over `(c, t)` (DConv's LayerScale residual).
pub fn add_scaled_rows(x: &mut [f32], g: &[f32], s: &[f32], t: usize) {
    let run = |(ch, (xr, gr)): (usize, (&mut [f32], &[f32]))| { let k = s[ch]; for (a, b) in xr.iter_mut().zip(gr) { *a += k * b } };
    if x.len() < PAR_ELEMS { x.chunks_mut(t).zip(g.chunks(t)).enumerate().for_each(run) }
    else { x.par_chunks_mut(t).zip(g.par_chunks(t)).enumerate().for_each(run) }
}

/// `x += y`, threaded for big buffers.
pub fn add(x: &mut [f32], y: &[f32]) {
    if x.len() < PAR_ELEMS { for (a, b) in x.iter_mut().zip(y) { *a += b } return }
    x.par_chunks_mut(PAR_ELEMS).zip(y.par_chunks(PAR_ELEMS)).for_each(|(x, y)| for (a, b) in x.iter_mut().zip(y) { *a += b });
}

/// `GroupNorm(1, c)` on one `(c, t)` block in place: statistics over the whole
/// block, then a per-channel affine. eps 1e-5, biased variance.
pub fn group_norm1(x: &mut [f32], c: usize, t: usize, w: &[f32], b: &[f32]) {
    let (mean, var) = moments(&x[..c * t]);
    let inv = 1.0 / (var + 1e-5).sqrt();
    let run = |(ch, row): (usize, &mut [f32])| {
        let (s, o) = ((w[ch] as f64 * inv) as f32, (b[ch] as f64 - mean * w[ch] as f64 * inv) as f32);
        for v in row { *v = *v * s + o }
    };
    if c * t < PAR_ELEMS { x[..c * t].chunks_mut(t).enumerate().for_each(run) }
    else { x[..c * t].par_chunks_mut(t).enumerate().for_each(run) }
}

/// `GroupNorm(1, c)` on tokens `(n, c)` (the transformer's `norm_out`).
pub fn group_norm1_tokens(x: &mut [f32], c: usize, w: &[f32], b: &[f32]) {
    let (mean, var) = moments(x);
    let inv = 1.0 / (var + 1e-5).sqrt();
    let s: Vec<f32> = (0..c).map(|ch| (w[ch] as f64 * inv) as f32).collect();
    let o: Vec<f32> = (0..c).map(|ch| (b[ch] as f64 - mean * w[ch] as f64 * inv) as f32).collect();
    x.par_chunks_exact_mut(c).for_each(|row| {
        for ((v, s), o) in row.iter_mut().zip(&s).zip(&o) { *v = *v * s + o }
    });
}

/// `LayerNorm(c)` on each token of `(n, c)`, into `out`. eps 1e-5.
pub fn layer_norm(x: &[f32], c: usize, w: &[f32], b: &[f32], out: &mut [f32]) {
    x.par_chunks_exact(c).zip(out.par_chunks_exact_mut(c)).for_each(|(row, o)| {
        let mean = row.iter().map(|&v| v as f64).sum::<f64>() / c as f64;
        let var = row.iter().map(|&v| { let d = v as f64 - mean; d * d }).sum::<f64>() / c as f64;
        let inv = (1.0 / (var + 1e-5).sqrt()) as f32;
        let mean = mean as f32;
        for i in 0..c { o[i] = (row[i] - mean) * inv * w[i] + b[i] }
    });
}

/// Softmax of each row of `(n, m)` in place.
pub fn softmax_rows(x: &mut [f32], m: usize) {
    for row in x.chunks_exact_mut(m) {
        let max = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut s = 0f32;
        // Below e^-80 a weight is under 1e-34 of the row's largest: write 0,
        // not a subnormal (subnormals make the next matrix multiply crawl).
        for v in row.iter_mut() { let d = *v - max; *v = if d < -80.0 { 0.0 } else { exp(d) } }
        for v in row.iter() { s += *v }
        let inv = 1.0 / s;
        for v in row.iter_mut() { *v *= inv }
    }
}

/// Softmax down each column of `(m, cols)` in place (the attention scores
/// kept keys × queries). Every pass runs along a row, across all columns,
/// so it vectorises.
pub fn softmax_cols(x: &mut [f32], cols: usize) {
    let mut max = vec![f32::NEG_INFINITY; cols];
    for row in x.chunks_exact(cols) { for (m, v) in max.iter_mut().zip(row) { *m = m.max(*v) } }
    let mut sum = vec![0f32; cols];
    for row in x.chunks_exact_mut(cols) {
        // Below e^-80 a weight is under 1e-34 of the column's largest: 0, not a subnormal.
        for ((v, m), s) in row.iter_mut().zip(&max).zip(sum.iter_mut()) {
            let d = *v - m;
            *v = if d < -80.0 { 0.0 } else { exp(d) };
            *s += *v;
        }
    }
    for s in sum.iter_mut() { *s = 1.0 / *s }
    for row in x.chunks_exact_mut(cols) { for (v, s) in row.iter_mut().zip(&sum) { *v *= s } }
}

/// Mean and unbiased standard deviation (torch's `x.std()`), as f32.
pub fn mean_std(x: &[f32]) -> (f32, f32) {
    let n = x.len() as f64;
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = x.iter().map(|&v| { let d = v as f64 - mean; d * d }).sum::<f64>() / (n - 1.0);
    (mean as f32, var.sqrt() as f32)
}

/// Multiply-adds below which a matrix multiply stays on one thread.
const PAR_MIN: usize = 1 << 21;

/// [`gemm`] split across the thread pool along its longer side (`m` or `n`).
/// Each part writes its own rectangle of C, so the parts never overlap.
#[allow(clippy::too_many_arguments)]
pub fn pgemm(m: usize, k: usize, n: usize, alpha: f32, a: M, b: M, beta: f32,
             c: &mut [f32], coff: usize, rsc: usize, csc: usize) {
    let parts = if m * k * n < PAR_MIN { 1 } else { rayon::current_num_threads() * 2 };
    if parts == 1 || m.max(n) < 2 * parts { return gemm(m, k, n, alpha, a, b, beta, c, coff, rsc, csc) }
    if m == 0 || n == 0 { return }
    a.check(m, k);
    b.check(k, n);
    M::new(c, coff, rsc, csc).check(m, n);
    let split_n = n >= m;
    let len = if split_n { n } else { m };
    let w = len.div_ceil(parts);
    let cp = c.as_mut_ptr() as usize;
    (0..len.div_ceil(w)).into_par_iter().for_each(|i| {
        let (j0, j1) = (i * w, ((i + 1) * w).min(len));
        // SAFETY: bounds checked above; part i only touches rows/columns [j0, j1).
        unsafe {
            if split_n {
                matrixmultiply::sgemm(m, k, j1 - j0, alpha,
                    a.d.as_ptr().add(a.off), a.rs as isize, a.cs as isize,
                    b.d.as_ptr().add(b.off + j0 * b.cs), b.rs as isize, b.cs as isize,
                    beta, (cp as *mut f32).add(coff + j0 * csc), rsc as isize, csc as isize);
            } else {
                matrixmultiply::sgemm(j1 - j0, k, n, alpha,
                    a.d.as_ptr().add(a.off + j0 * a.rs), a.rs as isize, a.cs as isize,
                    b.d.as_ptr().add(b.off), b.rs as isize, b.cs as isize,
                    beta, (cp as *mut f32).add(coff + j0 * rsc), rsc as isize, csc as isize);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn fast_exp_and_erf_match_libm() {
        let mut worst = (0f64, 0f64);
        for i in -80_000..=80_000 {
            let x = i as f32 * 1e-3;
            let e = (super::exp(x) as f64 - (x as f64).exp()).abs() / (x as f64).exp();
            let r = (super::erf(x) as f64 - libm::erf(x as f64)).abs();
            worst = (worst.0.max(e), worst.1.max(r));
        }
        assert!(worst.0 < 3e-7, "exp relative error {}", worst.0);
        // 1.5e-7 from the formula plus f32 rounding; the model still matches
        // PyTorch at 100+ dB with it (tests/reference.rs).
        assert!(worst.1 < 6e-7, "erf absolute error {}", worst.1);
    }
}
