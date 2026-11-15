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

/// Exact (erf) GELU, as `F.gelu` / `nn.GELU()`.
#[inline]
pub fn gelu(x: f32) -> f32 { 0.5 * x * (1.0 + libm::erff(x * std::f32::consts::FRAC_1_SQRT_2)) }

pub fn gelu_all(x: &mut [f32]) { for v in x { *v = gelu(*v) } }

#[inline]
fn sigmoid(x: f32) -> f32 { 1.0 / (1.0 + (-x).exp()) }

/// GLU over the channel axis of a `(2c, t)` block: `a * sigmoid(b)` → `(c, t)`.
pub fn glu(x: &[f32], c: usize, t: usize, out: &mut [f32]) {
    let (a, b) = x[..2 * c * t].split_at(c * t);
    for ((o, a), b) in out[..c * t].iter_mut().zip(a).zip(b) { *o = a * sigmoid(*b) }
}

/// `GroupNorm(1, c)` on one `(c, t)` block in place: statistics over the whole
/// block, then a per-channel affine. eps 1e-5, biased variance.
pub fn group_norm1(x: &mut [f32], c: usize, t: usize, w: &[f32], b: &[f32]) {
    let n = (c * t) as f64;
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = x.iter().map(|&v| { let d = v as f64 - mean; d * d }).sum::<f64>() / n;
    let inv = 1.0 / (var + 1e-5).sqrt();
    for ch in 0..c {
        let (s, o) = ((w[ch] as f64 * inv) as f32, (b[ch] as f64 - mean * w[ch] as f64 * inv) as f32);
        for v in &mut x[ch * t..(ch + 1) * t] { *v = *v * s + o }
    }
}

/// `GroupNorm(1, c)` on tokens `(n, c)` (the transformer's `norm_out`).
pub fn group_norm1_tokens(x: &mut [f32], c: usize, w: &[f32], b: &[f32]) {
    let n = x.len() as f64;
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = x.iter().map(|&v| { let d = v as f64 - mean; d * d }).sum::<f64>() / n;
    let inv = 1.0 / (var + 1e-5).sqrt();
    let s: Vec<f32> = (0..c).map(|ch| (w[ch] as f64 * inv) as f32).collect();
    let o: Vec<f32> = (0..c).map(|ch| (b[ch] as f64 - mean * w[ch] as f64 * inv) as f32).collect();
    for row in x.chunks_exact_mut(c) {
        for ((v, s), o) in row.iter_mut().zip(&s).zip(&o) { *v = *v * s + o }
    }
}

/// `LayerNorm(c)` on each token of `(n, c)`, into `out`. eps 1e-5.
pub fn layer_norm(x: &[f32], c: usize, w: &[f32], b: &[f32], out: &mut [f32]) {
    for (row, o) in x.chunks_exact(c).zip(out.chunks_exact_mut(c)) {
        let mean = row.iter().map(|&v| v as f64).sum::<f64>() / c as f64;
        let var = row.iter().map(|&v| { let d = v as f64 - mean; d * d }).sum::<f64>() / c as f64;
        let inv = (1.0 / (var + 1e-5).sqrt()) as f32;
        let mean = mean as f32;
        for i in 0..c { o[i] = (row[i] - mean) * inv * w[i] + b[i] }
    }
}

/// Softmax of each row of `(n, m)` in place.
pub fn softmax_rows(x: &mut [f32], m: usize) {
    for row in x.chunks_exact_mut(m) {
        let max = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut s = 0f32;
        for v in row.iter_mut() { *v = (*v - max).exp(); s += *v }
        let inv = 1.0 / s;
        for v in row.iter_mut() { *v *= inv }
    }
}

/// Mean and unbiased standard deviation (torch's `x.std()`), as f32.
pub fn mean_std(x: &[f32]) -> (f32, f32) {
    let n = x.len() as f64;
    let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n;
    let var = x.iter().map(|&v| { let d = v as f64 - mean; d * d }).sum::<f64>() / (n - 1.0);
    (mean as f32, var.sqrt() as f32)
}
