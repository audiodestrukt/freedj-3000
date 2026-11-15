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
pub struct Packed {
    pub m: usize, pub k: usize, d: Vec<f32>,
    /// Emulate fp16 (OPENDECK_DEMUCS_F16SIM).
    f16: bool,
    /// The same panels in fp16, for the NEON fp16 kernel (OPENDECK_DEMUCS_F16).
    h: Option<Vec<u16>>,
}

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
        let p = Self { m, k, d, f16: false, h: None };
        // OPENDECK_DEMUCS_F16ALL: emulate fp16 for every product (study only).
        if std::env::var_os("OPENDECK_DEMUCS_F16ALL").is_some() { p.half() } else { p }
    }

    /// Mark this product to run in half precision. With
    /// `OPENDECK_DEMUCS_F16SIM=1` that is emulated exactly on any CPU:
    /// operands rounded to f16, the accumulator rounded after every
    /// multiply-add (as the A76's fp16 `fmla`), flushed into f32 every
    /// `F16_RUN` steps. Without it, a no-op.
    pub fn half(mut self) -> Self {
        if f16_sim() {
            self.f16 = true;
            for v in &mut self.d { *v = half::f16::from_f32(*v).to_f32() }
        } else if cfg!(target_arch = "aarch64") && f16_on() {
            self.h = Some(self.d.iter().map(|v| half::f16::from_f32(*v).to_bits()).collect());
        }
        self
    }
}

/// Multiply-adds an fp16 accumulator takes before it is added into f32.
/// 32 measured 57.8-68 dB against PyTorch with the transformer in fp16
/// (64: 44-63, 16: ~70).
pub const F16_STEPS: usize = 32;

pub fn f16_run() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::env::var("OPENDECK_DEMUCS_F16RUN").ok().and_then(|v| v.parse().ok()).unwrap_or(F16_STEPS))
}

/// Real fp16 arithmetic for the products marked [`Packed::half`]
/// (`OPENDECK_DEMUCS_F16=1`; aarch64 only).
pub fn f16_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("OPENDECK_DEMUCS_F16").is_ok_and(|v| v == "1"))
}

pub fn f16_sim() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OPENDECK_DEMUCS_F16SIM").is_some())
}

/// The emulated fp16 kernel (see [`Packed::half`]).
unsafe fn kernel_f16_sim(kc: usize, a: *const f32, b: *const f32, out: &mut [f32; MR * NR]) {
    let r16 = |x: f32| half::f16::from_f32(x).to_f32();
    let run = f16_run();
    let mut acc = [0f32; MR * NR];
    let mut c = [0f32; MR * NR];
    for p in 0..kc {
        let ar = std::slice::from_raw_parts(a.add(p * MR), MR);
        let br = std::slice::from_raw_parts(b.add(p * NR), NR);
        for r in 0..MR { for j in 0..NR { c[r * NR + j] = r16(ar[r].mul_add(br[j], c[r * NR + j])) } }
        if (p + 1) % run == 0 { for (x, y) in acc.iter_mut().zip(c.iter_mut()) { *x += *y; *y = 0.0 } }
    }
    for (x, y) in acc.iter_mut().zip(&c) { *x += y }
    *out = acc;
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

/// `C (rows of panels [p_lo, p_hi)) += A[those rows][k_lo..k_hi] · B`, on the
/// calling thread: a tile of a bigger product (attention's key tiles).
#[allow(clippy::too_many_arguments)]
pub fn gemm1_part(a: &Packed, panels: std::ops::Range<usize>, ks: std::ops::Range<usize>, b: View, n: usize,
                  c: &mut [f32], o: Out, set: bool) {
    assert!(panels.end <= a.m.div_ceil(MR) && ks.end <= a.k && b.row_off.len() == ks.len());
    if n == 0 || panels.is_empty() { return }
    let rows = (a.m - panels.start * MR).min(panels.len() * MR);
    let max_row = b.row_off.iter().max().copied().unwrap_or(0);
    assert!(b.base + max_row + (n - 1) * b.cs < b.b.len(), "B view out of bounds");
    assert!(o.off + (rows - 1) * o.rs + (n - 1) * o.cs < c.len(), "C view out of bounds");
    let sub = Sub { prow0: panels.start, k_lo: ks.start, k_hi: ks.end };
    let mut buf = bufs();
    for s in 0..n.div_ceil(NC) {
        // SAFETY: bounds checked above.
        unsafe { strip_any(a, b, s * NC, (n - s * NC).min(NC), panels.start, panels.end, sub, c.as_mut_ptr(), o, &mut buf, set) }
    }
}

fn run(a: &Packed, b: View, n: usize, c: &mut [f32], o: Out, threads: bool, set: bool) {
    assert_eq!(b.row_off.len(), a.k, "B has {} rows, A has {} columns", b.row_off.len(), a.k);
    let sub = Sub { prow0: 0, k_lo: 0, k_hi: a.k };
    if n == 0 || a.m == 0 { return }
    // Bounds: the last element each view can reach.
    let max_row = b.row_off.iter().max().copied().unwrap_or(0);
    assert!(b.base + max_row + (n - 1) * b.cs < b.b.len(), "B view out of bounds");
    assert!(o.off + (a.m - 1) * o.rs + (n - 1) * o.cs < c.len(), "C view out of bounds");
    let strips = n.div_ceil(NC);
    let panels = a.m.div_ceil(MR);
    if !threads || a.m * a.k * n < PAR_MIN {
        let mut buf = bufs();
        for s in 0..strips {
            // SAFETY: bounds checked above.
            unsafe { strip_any(a, b, s * NC, (n - s * NC).min(NC), 0, panels, sub, c.as_mut_ptr(), o, &mut buf, set) }
        }
        return;
    }
    // Too few column strips to keep every thread busy: split the rows too.
    let want = rayon::current_num_threads() * 2;
    let parts = want.div_ceil(strips).clamp(1, panels);
    let per = panels.div_ceil(parts);
    let cp = c.as_mut_ptr() as usize;
    (0..strips * parts).into_par_iter().for_each_init(bufs, |buf, task| {
        let (s, q) = (task / parts, task % parts);
        let (p0, p1) = (q * per, ((q + 1) * per).min(panels));
        if p0 >= p1 { return }
        // SAFETY: bounds checked above; this task writes only rows of panels
        // [p0, p1) in columns [s*NC, s*NC+NC) of C, which no other task touches.
        unsafe { strip_any(a, b, s * NC, (n - s * NC).min(NC), p0, p1, sub, cp as *mut f32, o, buf, set) }
    });
}

/// Packing buffers for one task: f32 for the f32 kernel, u16 for fp16.
fn bufs() -> (Vec<f32>, Vec<u16>) { (vec![0f32; KC * NC.div_ceil(NR) * NR], Vec::new()) }

#[allow(clippy::too_many_arguments)]
unsafe fn strip_any(a: &Packed, b: View, j0: usize, nc: usize, p0: usize, p1: usize, sub: Sub, c: *mut f32, o: Out,
                    buf: &mut (Vec<f32>, Vec<u16>), set: bool) {
    #[cfg(target_arch = "aarch64")]
    if let Some(h) = &a.h { return strip16(a, h, b, j0, nc, p0, p1, sub, c, o, &mut buf.1, set) }
    strip(a, b, j0, nc, p0, p1, sub, c, o, &mut buf.0, set)
}

/// The part of A a product uses: rows from panel `prow0` (C's row 0) and
/// depth `[k_lo, k_hi)` (B's row 0 is A's column `k_lo`).
#[derive(Clone, Copy)]
struct Sub { prow0: usize, k_lo: usize, k_hi: usize }

/// Columns of one fp16 tile.
#[cfg(target_arch = "aarch64")]
const NR16: usize = 24;

/// f32 → fp16 bits, rounded to nearest even (`fcvt`).
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn h16(x: f32) -> u16 {
    let r: u32;
    std::arch::asm!("fcvt {t:h}, {x:s}", "umov {r:w}, {t:v}.h[0]", x = in(vreg) x, t = out(vreg) _, r = out(reg) r,
                    options(pure, nomem, nostack));
    r as u16
}

/// [`strip`] in fp16: B's strip converted to fp16 as it is packed, the
/// 8x24 kernel run [`F16_STEPS`] at a time into an f32 tile.
#[cfg(target_arch = "aarch64")]
#[allow(clippy::too_many_arguments)]
unsafe fn strip16(a: &Packed, h: &[u16], b: View, j0: usize, nc: usize, p0: usize, p1: usize, sub: Sub, c: *mut f32, o: Out,
                  buf: &mut Vec<u16>, set: bool) {
    let tiles = nc.div_ceil(NR16);
    buf.resize(KC * tiles * NR16, 0);
    let mut tile = [0f32; MR * NR16];
    let mut k0 = sub.k_lo;
    while k0 < sub.k_hi {
        let kc = (sub.k_hi - k0).min(KC);
        for t in 0..tiles {
            let dst = &mut buf[t * kc * NR16..(t + 1) * kc * NR16];
            let jn = (nc - t * NR16).min(NR16);
            if b.cs < TRANSPOSED {
                for kk in 0..kc {
                    let src = b.base + b.row_off[k0 - sub.k_lo + kk] + (j0 + t * NR16) * b.cs;
                    let row = &mut dst[kk * NR16..(kk + 1) * NR16];
                    for j in 0..jn { row[j] = h16(*b.b.get_unchecked(src + j * b.cs)) }
                    for v in &mut row[jn..] { *v = 0 }
                }
            } else {
                for j in 0..NR16 {
                    if j < jn {
                        let col = b.base + (j0 + t * NR16 + j) * b.cs;
                        for kk in 0..kc { dst[kk * NR16 + j] = h16(*b.b.get_unchecked(col + b.row_off[k0 - sub.k_lo + kk])) }
                    } else {
                        for kk in 0..kc { dst[kk * NR16 + j] = 0 }
                    }
                }
            }
        }
        for p in p0..p1 {
            let ap = h.as_ptr().add((p * a.k + k0) * MR);
            let mr = (a.m - p * MR).min(MR);
            for t in 0..tiles {
                tile.fill(0.0);
                let bp = buf.as_ptr().add(t * kc * NR16);
                let mut s0 = 0;
                while s0 < kc {
                    let n = (kc - s0).min(F16_STEPS);
                    kernel16(n, ap.add(s0 * MR), bp.add(s0 * NR16), tile.as_mut_ptr());
                    s0 += n;
                }
                let jn = (nc - t * NR16).min(NR16);
                let base = c.add(o.off + (p - sub.prow0) * MR * o.rs + (j0 + t * NR16) * o.cs);
                if set && k0 == 0 {
                    for r in 0..mr { let row = base.add(r * o.rs); for j in 0..jn { *row.add(j * o.cs) = tile[r * NR16 + j] } }
                } else if o.rs < o.cs {
                    for j in 0..jn { let col = base.add(j * o.cs); for r in 0..mr { *col.add(r * o.rs) += tile[r * NR16 + j] } }
                } else {
                    for r in 0..mr { let row = base.add(r * o.rs); for j in 0..jn { *row.add(j * o.cs) += tile[r * NR16 + j] } }
                }
            }
        }
        k0 += kc;
    }
}

/// One column strip `[j0, j0+nc)`: for each KC block, pack B's strip, then
/// run every A panel over it.
#[allow(clippy::too_many_arguments)]
unsafe fn strip(a: &Packed, b: View, j0: usize, nc: usize, p0: usize, p1: usize, sub: Sub, c: *mut f32, o: Out, buf: &mut [f32], set: bool) {
    let tiles = nc.div_ceil(NR);
    let mut tile = [0f32; MR * NR];
    let mut k0 = sub.k_lo;
    while k0 < sub.k_hi {
        let kc = (sub.k_hi - k0).min(KC);
        // Pack B[k0..k0+kc][j0..j0+nc] as [tile][kk][NR], zero past nc.
        for t in 0..tiles {
            let dst = &mut buf[t * kc * NR..(t + 1) * kc * NR];
            let jn = (nc - t * NR).min(NR);
            if b.cs < TRANSPOSED {
                // Rows of B are runs in memory: copy each row's slice.
                for kk in 0..kc {
                    let src = b.base + b.row_off[k0 - sub.k_lo + kk] + (j0 + t * NR) * b.cs;
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
                        for kk in 0..kc { dst[kk * NR + j] = *b.b.get_unchecked(col + b.row_off[k0 - sub.k_lo + kk]) }
                    } else {
                        for kk in 0..kc { dst[kk * NR + j] = 0.0 }
                    }
                }
            }
        }
        if a.f16 { for v in &mut buf[..tiles * kc * NR] { *v = half::f16::from_f32(*v).to_f32() } }
        for p in p0..p1 {
            let ap = a.d.as_ptr().add((p * a.k + k0) * MR);
            let mr = (a.m - p * MR).min(MR);
            for t in 0..tiles {
                if a.f16 { kernel_f16_sim(kc, ap, buf.as_ptr().add(t * kc * NR), &mut tile) }
                else { kernel(kc, ap, buf.as_ptr().add(t * kc * NR), &mut tile) }
                let jn = (nc - t * NR).min(NR);
                let base = c.add(o.off + (p - sub.prow0) * MR * o.rs + (j0 + t * NR) * o.cs);
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

    /// A tile of a product (some of A's rows, some of its depth) against the
    /// same slice of a plain product.
    #[test]
    fn a_part_matches_the_same_slice() {
        let (m, k, n) = (37, 300, 29);
        let a: Vec<f32> = (0..m * k).map(|i| ((i * 31 % 89) as f32 - 44.0) * 0.01).collect();
        let b: Vec<f32> = (0..k * n).map(|i| ((i * 17 % 83) as f32 - 41.0) * 0.02).collect();
        let pa = Packed::new(m, k, |i, kk| a[i * k + kk]);
        let (panels, ks) = (1..4, 40..290);
        let rows: Vec<usize> = ks.clone().map(|r| r * n).collect();
        let mut c = vec![0f32; 24 * n];
        gemm1_part(&pa, panels.clone(), ks.clone(), View { b: &b, base: 0, row_off: &rows, cs: 1 }, n, &mut c,
                   Out { off: 0, rs: n, cs: 1 }, true);
        for i in 0..24 { for j in 0..n {
            let ai = panels.start * MR + i;
            let want: f32 = ks.clone().map(|kk| a[ai * k + kk] * b[kk * n + j]).sum();
            assert!((want - c[i * n + j]).abs() < 1e-3, "{i},{j}: {} vs {want}", c[i * n + j]);
        }}
    }

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

/// fp16 kernel: `n` steps of an 8x24 tile in half precision (24 `.8h`
/// accumulators, lane-indexed `fmla`), then widened and added into the f32
/// tile `out` (`[8][24]`). The caller keeps `n` to [`F16_STEPS`] so the
/// fp16 sums stay short.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn kernel16(n: usize, a: *const u16, b: *const u16, out: *mut f32) {
    std::arch::asm!(
        ".arch_extension fp16",
        "movi v0.16b, #0",
        "movi v1.16b, #0",
        "movi v2.16b, #0",
        "movi v3.16b, #0",
        "movi v4.16b, #0",
        "movi v5.16b, #0",
        "movi v6.16b, #0",
        "movi v7.16b, #0",
        "movi v8.16b, #0",
        "movi v9.16b, #0",
        "movi v10.16b, #0",
        "movi v11.16b, #0",
        "movi v12.16b, #0",
        "movi v13.16b, #0",
        "movi v14.16b, #0",
        "movi v16.16b, #0",
        "movi v17.16b, #0",
        "movi v18.16b, #0",
        "movi v19.16b, #0",
        "movi v20.16b, #0",
        "movi v21.16b, #0",
        "movi v22.16b, #0",
        "movi v23.16b, #0",
        "movi v24.16b, #0",
        "2:",
        "ldr q15, [{a}], #16",
        "ldp q25, q26, [{b}], #32",
        "ldr q27, [{b}], #16",
        "fmla v0.8h, v25.8h, v15.h[0]",
        "fmla v1.8h, v26.8h, v15.h[0]",
        "fmla v2.8h, v27.8h, v15.h[0]",
        "fmla v3.8h, v25.8h, v15.h[1]",
        "fmla v4.8h, v26.8h, v15.h[1]",
        "fmla v5.8h, v27.8h, v15.h[1]",
        "fmla v6.8h, v25.8h, v15.h[2]",
        "fmla v7.8h, v26.8h, v15.h[2]",
        "fmla v8.8h, v27.8h, v15.h[2]",
        "fmla v9.8h, v25.8h, v15.h[3]",
        "fmla v10.8h, v26.8h, v15.h[3]",
        "fmla v11.8h, v27.8h, v15.h[3]",
        "fmla v12.8h, v25.8h, v15.h[4]",
        "fmla v13.8h, v26.8h, v15.h[4]",
        "fmla v14.8h, v27.8h, v15.h[4]",
        "fmla v16.8h, v25.8h, v15.h[5]",
        "fmla v17.8h, v26.8h, v15.h[5]",
        "fmla v18.8h, v27.8h, v15.h[5]",
        "fmla v19.8h, v25.8h, v15.h[6]",
        "fmla v20.8h, v26.8h, v15.h[6]",
        "fmla v21.8h, v27.8h, v15.h[6]",
        "fmla v22.8h, v25.8h, v15.h[7]",
        "fmla v23.8h, v26.8h, v15.h[7]",
        "fmla v24.8h, v27.8h, v15.h[7]",
        "subs {n}, {n}, #1",
        "b.ne 2b",
        "fcvtl v28.4s, v0.4h",
        "fcvtl2 v29.4s, v0.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v1.4h",
        "fcvtl2 v29.4s, v1.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v2.4h",
        "fcvtl2 v29.4s, v2.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v3.4h",
        "fcvtl2 v29.4s, v3.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v4.4h",
        "fcvtl2 v29.4s, v4.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v5.4h",
        "fcvtl2 v29.4s, v5.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v6.4h",
        "fcvtl2 v29.4s, v6.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v7.4h",
        "fcvtl2 v29.4s, v7.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v8.4h",
        "fcvtl2 v29.4s, v8.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v9.4h",
        "fcvtl2 v29.4s, v9.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v10.4h",
        "fcvtl2 v29.4s, v10.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v11.4h",
        "fcvtl2 v29.4s, v11.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v12.4h",
        "fcvtl2 v29.4s, v12.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v13.4h",
        "fcvtl2 v29.4s, v13.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v14.4h",
        "fcvtl2 v29.4s, v14.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v16.4h",
        "fcvtl2 v29.4s, v16.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v17.4h",
        "fcvtl2 v29.4s, v17.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v18.4h",
        "fcvtl2 v29.4s, v18.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v19.4h",
        "fcvtl2 v29.4s, v19.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v20.4h",
        "fcvtl2 v29.4s, v20.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v21.4h",
        "fcvtl2 v29.4s, v21.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v22.4h",
        "fcvtl2 v29.4s, v22.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v23.4h",
        "fcvtl2 v29.4s, v23.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        "fcvtl v28.4s, v24.4h",
        "fcvtl2 v29.4s, v24.8h",
        "ldp q30, q31, [{o}]",
        "fadd v30.4s, v30.4s, v28.4s",
        "fadd v31.4s, v31.4s, v29.4s",
        "stp q30, q31, [{o}], #32",
        a = inout(reg) a => _, b = inout(reg) b => _, n = inout(reg) n => _, o = inout(reg) out => _,
        out("v0") _, out("v1") _, out("v2") _, out("v3") _, out("v4") _, out("v5") _, out("v6") _, out("v7") _, out("v8") _, out("v9") _, out("v10") _, out("v11") _, out("v12") _, out("v13") _, out("v14") _, out("v16") _, out("v17") _, out("v18") _, out("v19") _, out("v20") _, out("v21") _, out("v22") _, out("v23") _, out("v24") _, out("v15") _, out("v25") _, out("v26") _, out("v27") _, out("v28") _, out("v29") _, out("v30") _, out("v31") _,
        options(nostack),
    );
}
