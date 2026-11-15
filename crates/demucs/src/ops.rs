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
    let r = n.mul_add(-1.428_606_8e-6, n.mul_add(-0.693_145_75, x)); // Cody-Waite ln 2
    // Cephes expf's degree-5 polynomial, as fused multiply-adds (fmla).
    let q = 1.987_569_1e-4f32.mul_add(r, 1.398_199_9e-3).mul_add(r, 8.333_452e-3).mul_add(r, 4.166_579_6e-2)
        .mul_add(r, 1.666_666_5e-1).mul_add(r, 5.000_000_1e-1);
    let p = q.mul_add(r * r, r) + 1.0;
    // 2^n without a float→int cast (a saturating `as` stops the loop
    // vectoriser): n + 127 + 2^23 holds n + 127 in its low mantissa bits.
    p * f32::from_bits((n + 8_388_735.0).to_bits() << 23)
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

/// Mean and biased variance in one pass (sum and sum of squares, f64).
fn moments1(x: &[f32]) -> (f64, f64) {
    let part = |c: &[f32]| c.iter().fold((0f64, 0f64), |(s, q), &v| { let v = v as f64; (s + v, q + v * v) });
    let (s, q) = if x.len() < PAR_ELEMS { part(x) } else {
        x.par_chunks(PAR_ELEMS).map(part).reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1))
    };
    let n = x.len() as f64;
    let mean = s / n;
    (mean, (q / n - mean * mean).max(0.0))
}

/// Per-channel scale and offset of `GroupNorm(1, c)` on a `(c, t)` block.
fn gn_affine(x: &[f32], c: usize, w: &[f32], b: &[f32]) -> Vec<(f32, f32)> {
    let (mean, var) = moments1(x);
    let inv = 1.0 / (var + 1e-5).sqrt();
    (0..c).map(|ch| ((w[ch] as f64 * inv) as f32, (b[ch] as f64 - mean * w[ch] as f64 * inv) as f32)).collect()
}

/// `GELU(GroupNorm(1, c)(x))` in place on `(c, t)`: one pass for the
/// statistics, one to apply both.
pub fn gn_gelu(x: &mut [f32], c: usize, t: usize, w: &[f32], b: &[f32]) {
    let ab = gn_affine(&x[..c * t], c, w, b);
    let run = |(ch, row): (usize, &mut [f32])| { let (s, o) = ab[ch]; for v in row { *v = gelu(*v * s + o) } };
    if c * t < PAR_ELEMS { x[..c * t].chunks_mut(t).enumerate().for_each(run) }
    else { x[..c * t].par_chunks_mut(t).enumerate().for_each(run) }
}

/// The end of a DConv layer in one pass after the statistics:
/// `x += scale · GLU(GroupNorm(1, 2c)(z))`, z `(2c, t)`, x `(c, t)`.
#[allow(clippy::too_many_arguments)]
pub fn gn_glu_add(z: &[f32], c: usize, t: usize, w: &[f32], b: &[f32], scale: &[f32], x: &mut [f32]) {
    let ab = gn_affine(&z[..2 * c * t], 2 * c, w, b);
    let run = |(ch, xr): (usize, &mut [f32])| {
        let ((sa, oa), (sb, ob), k) = (ab[ch], ab[c + ch], scale[ch]);
        let (za, zb) = (&z[ch * t..(ch + 1) * t], &z[(c + ch) * t..(c + ch + 1) * t]);
        for ((xv, a), bv) in xr.iter_mut().zip(za).zip(zb) { *xv += k * (a * sa + oa) * sigmoid(bv * sb + ob) }
    };
    if c * t < PAR_ELEMS { x[..c * t].chunks_mut(t).enumerate().for_each(run) }
    else { x[..c * t].par_chunks_mut(t).enumerate().for_each(run) }
}

/// The end of a DConv layer without materialising `z = W·y + b` (`(2c, t)`):
/// GroupNorm(1, 2c)'s statistics follow exactly from y's channel sums and
/// its `h × h` Gram matrix (z is linear in y), then one pass computes each
/// gated pair of z on the fly: `x += scale · GLU(GN(z))`. For small `h`,
/// where z is the biggest tensor of the block and costs more to write and
/// read than to recompute. `w2` is `(2c, h)` row-major.
#[allow(clippy::too_many_arguments)]
pub fn dconv_tail(y: &[f32], h: usize, t: usize, w2: &[f32], b2: &[f32], gw: &[f32], gb: &[f32],
                  scale: &[f32], c: usize, x: &mut [f32]) {
    // Σ_t y and Σ_t y·yᵀ, in f64, threaded over time.
    let part = |t0: usize, t1: usize| {
        let mut sum = vec![0f64; h];
        let mut g = vec![0f64; h * h];
        for tt in t0..t1 {
            for i in 0..h {
                let yi = y[i * t + tt] as f64;
                sum[i] += yi;
                for j in i..h { g[i * h + j] += yi * y[j * t + tt] as f64 }
            }
        }
        (sum, g)
    };
    const SPAN: usize = 4096;
    let (sum, g) = (0..t.div_ceil(SPAN)).into_par_iter().map(|s| part(s * SPAN, ((s + 1) * SPAN).min(t)))
        .reduce(|| (vec![0f64; h], vec![0f64; h * h]), |mut a, b| {
            for (x, y) in a.0.iter_mut().zip(&b.0) { *x += y }
            for (x, y) in a.1.iter_mut().zip(&b.1) { *x += y }
            a
        });
    let gram = |i: usize, j: usize| if i <= j { g[i * h + j] } else { g[j * h + i] };
    let n = t as f64;
    let (mut zs, mut zq) = (0f64, 0f64);
    for co in 0..2 * c {
        let wr = &w2[co * h..(co + 1) * h];
        let bc = b2[co] as f64;
        let ws: f64 = (0..h).map(|i| wr[i] as f64 * sum[i]).sum();
        let mut wgw = 0f64;
        for i in 0..h { for j in 0..h { wgw += wr[i] as f64 * gram(i, j) * wr[j] as f64 } }
        zs += ws + bc * n;
        zq += wgw + 2.0 * bc * ws + bc * bc * n;
    }
    let cnt = n * (2 * c) as f64;
    let mean = zs / cnt;
    let var = (zq / cnt - mean * mean).max(0.0);
    let inv = 1.0 / (var + 1e-5).sqrt();
    let aff = |co: usize| ((gw[co] as f64 * inv) as f32, (gb[co] as f64 - mean * gw[co] as f64 * inv) as f32);
    let run = |(ch, xr): (usize, &mut [f32])| {
        let ((sa, oa), (sb, ob), k) = (aff(ch), aff(c + ch), scale[ch]);
        let (wa, wb) = (&w2[ch * h..(ch + 1) * h], &w2[(c + ch) * h..(c + ch + 1) * h]);
        // z rows for this channel, a block of time at a time, then the gate.
        const B: usize = 256;
        let (mut za, mut zb) = ([0f32; B], [0f32; B]);
        for t0 in (0..t).step_by(B) {
            let nb = (t - t0).min(B);
            za[..nb].fill(b2[ch]);
            zb[..nb].fill(b2[c + ch]);
            for i in 0..h {
                let yr = &y[i * t + t0..i * t + t0 + nb];
                let (ka, kb) = (wa[i], wb[i]);
                for ((a, b), v) in za[..nb].iter_mut().zip(zb[..nb].iter_mut()).zip(yr) { *a += ka * v; *b += kb * v }
            }
            for ((xv, a), b) in xr[t0..t0 + nb].iter_mut().zip(&za[..nb]).zip(&zb[..nb]) {
                *xv += k * (a * sa + oa) * sigmoid(b * sb + ob);
            }
        }
    };
    if c * t < PAR_ELEMS { x[..c * t].chunks_mut(t).enumerate().for_each(run) }
    else { x[..c * t].par_chunks_mut(t).enumerate().for_each(run) }
}


/// `x += y`, threaded for big buffers.
pub fn add(x: &mut [f32], y: &[f32]) {
    if x.len() < PAR_ELEMS { for (a, b) in x.iter_mut().zip(y) { *a += b } return }
    x.par_chunks_mut(PAR_ELEMS).zip(y.par_chunks(PAR_ELEMS)).for_each(|(x, y)| for (a, b) in x.iter_mut().zip(y) { *a += b });
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

/// One key tile of an online softmax (flash attention), on scores kept
/// keys × queries (`(keys, cols)`). Folds the tile into each query's running
/// max `mx` and sum `l`, overwrites the tile with its weights against the new
/// max, and leaves in `alpha` the factor the output so far must be scaled by.
pub fn softmax_tile(x: &mut [f32], cols: usize, mx: &mut [f32], l: &mut [f32], alpha: &mut [f32]) {
    let mut tmax = vec![f32::NEG_INFINITY; cols];
    for row in x.chunks_exact(cols) { for (m, v) in tmax.iter_mut().zip(row) { *m = m.max(*v) } }
    for i in 0..cols {
        let m_new = mx[i].max(tmax[i]);
        alpha[i] = if mx[i] == f32::NEG_INFINITY { 0.0 } else { exp(mx[i] - m_new) };
        l[i] *= alpha[i];
        mx[i] = m_new;
    }
    for row in x.chunks_exact_mut(cols) {
        // Below e^-80 a weight is under 1e-34 of the largest so far: 0, not a subnormal.
        for ((v, m), s) in row.iter_mut().zip(mx.iter()).zip(l.iter_mut()) {
            let d = *v - m;
            *v = if d < -80.0 { 0.0 } else { exp(d) };
            *s += *v;
        }
    }
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
