//! Matrix multiply with the weights packed once, at load.
//!
//! `C (m×n) += A · B`, where A is a weight matrix packed into panels of `MR`
//! rows, and B is a view of an activation tensor: row `r` of B starts at
//! `b[base + row_off[r]]` and its columns are `cs` apart. That covers every
//! convolution here (strided, dilated, 3x3, along frequency) and the token
//! matrices of the transformer, with no im2col copy: each thread packs only
//! the `KC × NC` strip of B it is about to use, which stays in its L2.
//!
//! The inner kernel is 8x12: NEON on aarch64 (24 accumulators in registers),
//! a portable loop the compiler vectorises elsewhere.

use rayon::prelude::*;

pub const MR: usize = 8;
pub const NR: usize = 12;
/// Depth of one pass: a `KC × NC` strip of B is 192 KB.
const KC: usize = 256;
/// Columns of B one task packs and owns.
const NC: usize = 192;
/// Column stride from which B is packed column by column.
const TRANSPOSED: usize = 16;
/// Multiply-adds below which a product stays on the calling thread.
const PAR_MIN: usize = 1 << 22;

/// A weight matrix `m × k`, packed: panel `p` holds rows `p*MR..p*MR+MR`
/// as `[k][MR]` (zero rows past `m`).
pub struct Packed { pub m: usize, pub k: usize, d: Vec<f32> }

impl Packed {
    /// Pack `A[i][kk] = f(i, kk)`.
    pub fn new(m: usize, k: usize, f: impl Fn(usize, usize) -> f32) -> Self {
        let panels = m.div_ceil(MR);
        let mut d = vec![0f32; panels * k * MR];
        for p in 0..panels {
            for kk in 0..k {
                for r in 0..MR {
                    let i = p * MR + r;
                    if i < m { d[(p * k + kk) * MR + r] = f(i, kk) }
                }
            }
        }
        Self { m, k, d }
    }
}

/// B as a view: row `r` at `b[base + row_off[r]]`, columns `cs` apart.
#[derive(Clone, Copy)]
pub struct View<'a> { pub b: &'a [f32], pub base: usize, pub row_off: &'a [usize], pub cs: usize }

/// C as a view: element `(i, j)` at `c[off + i*rs + j*cs]`.
#[derive(Clone, Copy)]
pub struct Out { pub off: usize, pub rs: usize, pub cs: usize }

/// `C += A·B` for the `n` columns of B. Splits the work across threads when
/// the product is big enough; each task writes only its own block of C.
pub fn gemm(a: &Packed, b: View, n: usize, c: &mut [f32], o: Out) { run(a, b, n, c, o, true, false) }

/// [`gemm`] on the calling thread only: for products that are already one
/// task of a parallel loop (a band, a block of queries), where splitting
/// again would only pack the same B several times over.
pub fn gemm1(a: &Packed, b: View, n: usize, c: &mut [f32], o: Out) { run(a, b, n, c, o, false, false) }

/// [`gemm1`] that overwrites C (`C = A·B`) instead of adding to it, so C
/// need not be zeroed first.
pub fn gemm1_set(a: &Packed, b: View, n: usize, c: &mut [f32], o: Out) { run(a, b, n, c, o, false, true) }

fn run(a: &Packed, b: View, n: usize, c: &mut [f32], o: Out, threads: bool, set: bool) {
    assert_eq!(b.row_off.len(), a.k, "B has {} rows, A has {} columns", b.row_off.len(), a.k);
    if n == 0 || a.m == 0 { return }
    // Bounds: the last element each view can reach.
    let max_row = b.row_off.iter().max().copied().unwrap_or(0);
    assert!(b.base + max_row + (n - 1) * b.cs < b.b.len(), "B view out of bounds");
    assert!(o.off + (a.m - 1) * o.rs + (n - 1) * o.cs < c.len(), "C view out of bounds");
    let strips = n.div_ceil(NC);
    let panels = a.m.div_ceil(MR);
    if !threads || a.m * a.k * n < PAR_MIN {
        let mut buf = vec![0f32; KC * NC.div_ceil(NR) * NR];
        for s in 0..strips {
            // SAFETY: bounds checked above.
            unsafe { strip(a, b, s * NC, (n - s * NC).min(NC), 0, panels, c.as_mut_ptr(), o, &mut buf, set) }
        }
        return;
    }
    // Too few column strips to keep every thread busy: split the rows too.
    let want = rayon::current_num_threads() * 2;
    let parts = want.div_ceil(strips).clamp(1, panels);
    let per = panels.div_ceil(parts);
    let cp = c.as_mut_ptr() as usize;
    (0..strips * parts).into_par_iter().for_each_init(|| vec![0f32; KC * NC.div_ceil(NR) * NR], |buf, task| {
        let (s, q) = (task / parts, task % parts);
        let (p0, p1) = (q * per, ((q + 1) * per).min(panels));
        if p0 >= p1 { return }
        // SAFETY: bounds checked above; this task writes only rows of panels
        // [p0, p1) in columns [s*NC, s*NC+NC) of C, which no other task touches.
        unsafe { strip(a, b, s * NC, (n - s * NC).min(NC), p0, p1, cp as *mut f32, o, buf, set) }
    });
}

/// One column strip `[j0, j0+nc)`: for each KC block, pack B's strip, then
/// run every A panel over it.
#[allow(clippy::too_many_arguments)]
unsafe fn strip(a: &Packed, b: View, j0: usize, nc: usize, p0: usize, p1: usize, c: *mut f32, o: Out, buf: &mut [f32], set: bool) {
    let tiles = nc.div_ceil(NR);
    let mut tile = [0f32; MR * NR];
    let mut k0 = 0;
    while k0 < a.k {
        let kc = (a.k - k0).min(KC);
        // Pack B[k0..k0+kc][j0..j0+nc] as [tile][kk][NR], zero past nc.
        for t in 0..tiles {
            let dst = &mut buf[t * kc * NR..(t + 1) * kc * NR];
            let jn = (nc - t * NR).min(NR);
            if b.cs < TRANSPOSED {
                // Rows of B are runs in memory: copy each row's slice.
                for kk in 0..kc {
                    let src = b.base + b.row_off[k0 + kk] + (j0 + t * NR) * b.cs;
                    let row = &mut dst[kk * NR..(kk + 1) * NR];
                    if b.cs == 1 && jn == NR {
                        // A fixed length: three vector moves, not a memcpy call.
                        row.copy_from_slice(b.b.get_unchecked(src..src + NR));
                    } else if b.cs == 1 {
                        row[..jn].copy_from_slice(b.b.get_unchecked(src..src + jn));
                    } else {
                        for j in 0..jn { row[j] = *b.b.get_unchecked(src + j * b.cs) }
                    }
                    for v in &mut row[jn..] { *v = 0.0 }
                }
            } else {
                // Columns far apart (a transposed view): walk down each
                // column, where neighbouring rows sit next to each other.
                for j in 0..NR {
                    if j < jn {
                        let col = b.base + (j0 + t * NR + j) * b.cs;
                        for kk in 0..kc { dst[kk * NR + j] = *b.b.get_unchecked(col + b.row_off[k0 + kk]) }
                    } else {
                        for kk in 0..kc { dst[kk * NR + j] = 0.0 }
                    }
                }
            }
        }
        for p in p0..p1 {
            let ap = a.d.as_ptr().add((p * a.k + k0) * MR);
            let mr = (a.m - p * MR).min(MR);
            for t in 0..tiles {
                kernel(kc, ap, buf.as_ptr().add(t * kc * NR), &mut tile);
                let jn = (nc - t * NR).min(NR);
                let base = c.add(o.off + p * MR * o.rs + (j0 + t * NR) * o.cs);
                if set && k0 == 0 {
                    // First pass of an overwrite: store, don't add.
                    for r in 0..mr { let row = base.add(r * o.rs); for j in 0..jn { *row.add(j * o.cs) = tile[r * NR + j] } }
                } else if mr == MR && jn == NR && o.cs == 1 {
                    // A whole tile into row-major C: fixed sizes, so it vectorises.
                    for r in 0..MR {
                        let row = std::slice::from_raw_parts_mut(base.add(r * o.rs), NR);
                        for (d, v) in row.iter_mut().zip(&tile[r * NR..(r + 1) * NR]) { *d += v }
                    }
                } else if o.rs < o.cs {
                    // C transposed: each column's rows are adjacent.
                    for j in 0..jn { let col = base.add(j * o.cs); for r in 0..mr { *col.add(r * o.rs) += tile[r * NR + j] } }
                } else {
                    for r in 0..mr { let row = base.add(r * o.rs); for j in 0..jn { *row.add(j * o.cs) += tile[r * NR + j] } }
                }
            }
        }
        k0 += kc;
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn kernel(kc: usize, a: *const f32, b: *const f32, out: &mut [f32; MR * NR]) {
    use core::arch::aarch64::*;
    let z = vdupq_n_f32(0.0);
    let mut c = [[z; 3]; 8];
    for p in 0..kc {
        let a0 = vld1q_f32(a.add(p * MR));
        let a1 = vld1q_f32(a.add(p * MR + 4));
        let b0 = vld1q_f32(b.add(p * NR));
        let b1 = vld1q_f32(b.add(p * NR + 4));
        let b2 = vld1q_f32(b.add(p * NR + 8));
        macro_rules! row { ($r:expr, $a:ident, $l:literal) => {
            c[$r][0] = vfmaq_laneq_f32::<$l>(c[$r][0], b0, $a);
            c[$r][1] = vfmaq_laneq_f32::<$l>(c[$r][1], b1, $a);
            c[$r][2] = vfmaq_laneq_f32::<$l>(c[$r][2], b2, $a);
        } }
        row!(0, a0, 0); row!(1, a0, 1); row!(2, a0, 2); row!(3, a0, 3);
        row!(4, a1, 0); row!(5, a1, 1); row!(6, a1, 2); row!(7, a1, 3);
    }
    for r in 0..MR {
        for j in 0..3 { vst1q_f32(out.as_mut_ptr().add(r * NR + j * 4), c[r][j]) }
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
unsafe fn kernel(kc: usize, a: *const f32, b: *const f32, out: &mut [f32; MR * NR]) {
    let mut c = [[0f32; NR]; MR];
    for p in 0..kc {
        let ar = std::slice::from_raw_parts(a.add(p * MR), MR);
        let br = std::slice::from_raw_parts(b.add(p * NR), NR);
        for r in 0..MR { for j in 0..NR { c[r][j] += ar[r] * br[j] } }
    }
    for r in 0..MR { out[r * NR..(r + 1) * NR].copy_from_slice(&c[r]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against a plain triple loop, at awkward sizes (edges in every dimension,
    /// several KC passes, many strips) with strided B and transposed C.
    #[test]
    fn matches_a_plain_product() {
        for &(m, k, n, cs, threaded) in &[(5, 3, 7, 1, false), (17, 300, 50, 3, false), (40, 600, 1000, 1, true), (9, 520, 410, 2, true)] {
            let a: Vec<f32> = (0..m * k).map(|i| ((i * 37 % 101) as f32 - 50.0) * 0.01).collect();
            let row_off: Vec<usize> = (0..k).map(|r| r * (n * cs + 5)).collect();
            let blen = row_off[k - 1] + (n - 1) * cs + 1;
            let b: Vec<f32> = (0..blen).map(|i| ((i * 53 % 97) as f32 - 48.0) * 0.02).collect();
            let pa = Packed::new(m, k, |i, kk| a[i * k + kk]);
            // C transposed: (i, j) at j*m + i, starting from 1.0.
            let mut c = vec![1f32; m * n];
            let _ = threaded;
            gemm(&pa, View { b: &b, base: 0, row_off: &row_off, cs }, n, &mut c, Out { off: 0, rs: 1, cs: m });
            for i in 0..m { for j in 0..n {
                let want: f32 = 1.0 + (0..k).map(|kk| a[i * k + kk] * b[row_off[kk] + j * cs]).sum::<f32>();
                let got = c[j * m + i];
                assert!((want - got).abs() < 1e-3 * (1.0 + want.abs()), "({m},{k},{n}) at {i},{j}: {got} vs {want}");
            }}
        }
    }
}
