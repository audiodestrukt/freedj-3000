//! HTDemucs (Meta's hybrid transformer Demucs, the `htdemucs` pretrained
//! model) in plain Rust, from the original weights.
//!
//! One call separates one 7.8 s stereo chunk at 44.1 kHz into drums, bass,
//! other and vocals, exactly as `HTDemucs.forward` does; the caller does the
//! chunking and overlap (see `opendeck-app`'s `separate.rs`).
//!
//! Weights are `htdemucs.safetensors`, written by
//! `tools/demucs/export_htdemucs.py` from the PyTorch checkpoint.
//!
//! Layouts: the frequency branch is `(freq, channel, time)` so that every
//! convolution, along time or along frequency, is a strided matrix multiply
//! over the tensor in place; the time branch is `(channel, time)`; the
//! transformer works on `(token, channel)`.

mod gemm;
mod ops;
pub mod spec;
mod st;

use anyhow::Result;
use gemm::{Out, Packed, View};
use rayon::prelude::*;
use realfft::num_complex::Complex32;
use realfft::RealFftPlanner;
use std::path::Path;

pub use st::Tensors;

/// Samples per chunk: the model's training segment, 7.8 s at 44.1 kHz.
pub const SEGMENT: usize = 343_980;
pub const SOURCES: [&str; 4] = ["drums", "bass", "other", "vocals"];
const DEPTH: usize = 4;
const DIM: usize = 512;
const HEADS: usize = 8;

/// `OPENDECK_DEMUCS_PROF=1` prints the time of each step inside the
/// transformer layers to stderr.
struct Lap(Option<std::time::Instant>);
impl Lap {
    fn new() -> Self {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        Self(ON.get_or_init(|| std::env::var_os("OPENDECK_DEMUCS_PROF").is_some()).then(std::time::Instant::now))
    }
    fn lap(&mut self, what: &str) {
        if let Some(t) = self.0.as_mut() { eprintln!("  {what:10} {:7.1} ms", t.elapsed().as_secs_f64() * 1e3); *t = std::time::Instant::now() }
    }
}

/// Called with each stage's output in PyTorch's layout (batch dim dropped),
/// for comparing against a reference run.
pub type Tap<'a> = &'a mut dyn FnMut(&str, &[usize], &[f32]);

/// A layer's weights: `w` as loaded, packed into `p` (one per tap for
/// transposed convs) and then dropped; `b` the bias.
struct Lin { w: Vec<f32>, b: Vec<f32>, p: Vec<Packed> }

impl Lin {
    fn half(mut self) -> Self { self.p = self.p.into_iter().map(Packed::half).collect(); self }
    /// Pack `w` as a row-major `(m, k)` matrix.
    fn rows(mut self, m: usize, k: usize) -> Self {
        self.p = vec![Packed::new(m, k, |i, kk| self.w[i * k + kk])];
        self.w = Vec::new();
        self
    }
    /// Pack with `A[i][kk] = f(w, i, kk)`, `taps` times (tap index first).
    fn taps(mut self, taps: usize, m: usize, k: usize, f: impl Fn(&[f32], usize, usize, usize) -> f32) -> Self {
        self.p = (0..taps).map(|tap| Packed::new(m, k, |i, kk| f(&self.w, tap, i, kk))).collect();
        self.w = Vec::new();
        self
    }
}
struct Norm { w: Vec<f32>, b: Vec<f32> }

/// One DConv residual layer: conv k3 (dilated) → GN → GELU → conv 1x1 → GN → GLU → scale.
struct DLayer { c1: Lin, n1: Norm, c2: Lin, n2: Norm, scale: Vec<f32>, dil: usize, hidden: usize,
                /// c2's weights row-major `(2c, hidden)`, for [`ops::dconv_tail`].
                w2: Vec<f32> }

/// Hidden widths up to which DConv's second conv is folded into its tail.
const TAIL_MAX_HIDDEN: usize = 12;

struct Enc { cin: usize, cout: usize, conv: Lin, rewrite: Lin, dconv: Vec<DLayer> }
struct Dec { cin: usize, cout: usize, last: bool, conv_tr: Lin, rewrite: Lin, dconv: Vec<DLayer> }

struct Attn { in_p: Vec<Packed>, in_b: Vec<f32>, out: Lin }
struct Layer {
    cross: bool, attn: Attn, lin1: Lin, lin2: Lin,
    norm1: Norm, norm2: Norm, norm3: Option<Norm>, norm_out: Norm, g1: Vec<f32>, g2: Vec<f32>,
}

pub struct HtDemucs {
    enc: Vec<Enc>, tenc: Vec<Enc>, dec: Vec<Dec>, tdec: Vec<Dec>,
    freq_emb: Vec<f32>,
    up: Lin, down: Lin, up_t: Lin, down_t: Lin,
    norm_in: Norm, norm_in_t: Norm,
    layers: Vec<Layer>, layers_t: Vec<Layer>,
}

// ---------------------------------------------------------------- loading

fn lin(t: &mut Tensors, p: &str, w: &[usize]) -> Result<Lin> {
    Ok(Lin { w: t.take(&format!("{p}.weight"), w)?, b: t.take(&format!("{p}.bias"), &[w[0]])?, p: vec![] })
}
fn lin_tr(t: &mut Tensors, p: &str, w: &[usize]) -> Result<Lin> {
    Ok(Lin { w: t.take(&format!("{p}.weight"), w)?, b: t.take(&format!("{p}.bias"), &[w[1]])?, p: vec![] })
}
fn norm(t: &mut Tensors, p: &str, c: usize) -> Result<Norm> {
    Ok(Norm { w: t.take(&format!("{p}.weight"), &[c])?, b: t.take(&format!("{p}.bias"), &[c])? })
}

/// Reorder a frequency conv `(cout, cin, k, 1)` to `(cout, k, cin)`.
fn freq_kernel(w: &[f32], cout: usize, cin: usize, k: usize) -> Vec<f32> {
    let mut r = vec![0f32; w.len()];
    for co in 0..cout { for ci in 0..cin { for kk in 0..k {
        r[(co * k + kk) * cin + ci] = w[(co * cin + ci) * k + kk];
    }}}
    r
}
/// Reorder a frequency transposed conv `(cin, cout, k, 1)` to `(k, cout, cin)`.
fn freq_kernel_tr(w: &[f32], cin: usize, cout: usize, k: usize) -> Vec<f32> {
    let mut r = vec![0f32; w.len()];
    for ci in 0..cin { for co in 0..cout { for kk in 0..k {
        r[(kk * cout + co) * cin + ci] = w[(ci * cout + co) * k + kk];
    }}}
    r
}
/// Reorder a 3x3 conv `(cout, cin, 3, 3)` (freq, time) to `(dt, cout, df, cin)`.
fn kernel_3x3(w: &[f32], cout: usize, cin: usize) -> Vec<f32> {
    let mut r = vec![0f32; w.len()];
    for co in 0..cout { for ci in 0..cin { for df in 0..3 { for dt in 0..3 {
        r[((dt * cout + co) * 3 + df) * cin + ci] = w[((co * cin + ci) * 3 + df) * 3 + dt];
    }}}}
    r
}

fn dconv(t: &mut Tensors, p: &str, c: usize) -> Result<Vec<DLayer>> {
    let h = c / 8;
    (0..2).map(|d| {
        let q = format!("{p}.dconv.layers.{d}");
        let w2;
        Ok(DLayer {
            c1: lin(t, &format!("{q}.0"), &[h, c, 3])?.rows(h, 3 * c), n1: norm(t, &format!("{q}.1"), h)?,
            c2: { let l = lin(t, &format!("{q}.3"), &[2 * c, h, 1])?; w2 = l.w.clone(); l.rows(2 * c, h) },
            n2: norm(t, &format!("{q}.4"), 2 * c)?,
            scale: t.take(&format!("{q}.6.scale"), &[c])?, dil: 1 << d, hidden: h, w2,
        })
    }).collect()
}

fn attn(t: &mut Tensors, p: &str) -> Result<Attn> {
    let w = t.take(&format!("{p}.in_proj_weight"), &[3 * DIM, DIM])?;
    Ok(Attn {
        in_p: (0..3).map(|part| Packed::new(DIM, DIM, |i, c| w[(part * DIM + i) * DIM + c]).half()).collect(),
        in_b: t.take(&format!("{p}.in_proj_bias"), &[3 * DIM])?,
        out: lin(t, &format!("{p}.out_proj"), &[DIM, DIM])?.rows(DIM, DIM).half(),
    })
}

fn layer(t: &mut Tensors, p: &str, cross: bool) -> Result<Layer> {
    Ok(Layer {
        cross,
        attn: attn(t, &format!("{p}.{}", if cross { "cross_attn" } else { "self_attn" }))?,
        lin1: lin(t, &format!("{p}.linear1"), &[4 * DIM, DIM])?.rows(4 * DIM, DIM).half(),
        lin2: lin(t, &format!("{p}.linear2"), &[DIM, 4 * DIM])?.rows(DIM, 4 * DIM).half(),
        norm1: norm(t, &format!("{p}.norm1"), DIM)?, norm2: norm(t, &format!("{p}.norm2"), DIM)?,
        norm3: if cross { Some(norm(t, &format!("{p}.norm3"), DIM)?) } else { None },
        norm_out: norm(t, &format!("{p}.norm_out"), DIM)?,
        g1: t.take(&format!("{p}.gamma_1.scale"), &[DIM])?,
        g2: t.take(&format!("{p}.gamma_2.scale"), &[DIM])?,
    })
}

impl HtDemucs {
    pub fn load(path: &Path) -> Result<Self> { Self::from_tensors(Tensors::open(path)?) }

    pub fn from_tensors(mut t: Tensors) -> Result<Self> {
        let t = &mut t;
        let (mut enc, mut tenc, mut dec, mut tdec) = (vec![], vec![], vec![], vec![]);
        for i in 0..DEPTH {
            let (cin_z, cin) = if i == 0 { (4, 2) } else { (48 << (i - 1), 48 << (i - 1)) };
            let cout = 48 << i;
            let mut conv = lin(t, &format!("encoder.{i}.conv"), &[cout, cin_z, 8, 1])?;
            conv.w = freq_kernel(&conv.w, cout, cin_z, 8);
            let conv = conv.rows(cout, 8 * cin_z);
            let rewrite = lin(t, &format!("encoder.{i}.rewrite"), &[2 * cout, cout, 1, 1])?.rows(2 * cout, cout);
            enc.push(Enc { cin: cin_z, cout, conv, rewrite, dconv: dconv(t, &format!("encoder.{i}"), cout)? });
            let conv = lin(t, &format!("tencoder.{i}.conv"), &[cout, cin, 8])?.rows(cout, 8 * cin);
            let rewrite = lin(t, &format!("tencoder.{i}.rewrite"), &[2 * cout, cout, 1])?.rows(2 * cout, cout);
            tenc.push(Enc { cin, cout, conv, rewrite, dconv: dconv(t, &format!("tencoder.{i}"), cout)? });
        }
        for i in 0..DEPTH {
            // decoder.i undoes encoder.(DEPTH-1-i)
            let cin = 384 >> i;
            let last = i == DEPTH - 1;
            let (cout_z, cout) = if last { (16, 8) } else { (cin / 2, cin / 2) };
            let mut conv_tr = lin_tr(t, &format!("decoder.{i}.conv_tr"), &[cin, cout_z, 8, 1])?;
            conv_tr.w = freq_kernel_tr(&conv_tr.w, cin, cout_z, 8);
            // All 8 taps as one (8·cout, cin) matrix, rows (tap, cout).
            let conv_tr = conv_tr.taps(1, 8 * cout_z, cin, |w, _, r, ci| w[r * cin + ci]);
            let mut rewrite = lin(t, &format!("decoder.{i}.rewrite"), &[2 * cin, cin, 3, 3])?;
            rewrite.w = kernel_3x3(&rewrite.w, 2 * cin, cin);
            // Column (dt·3 + df)·cin + ci of the (dt, cout, df, cin) layout.
            let co2 = 2 * cin;
            let rewrite = rewrite.taps(1, co2, 9 * cin, |w, _, co, r| w[((r / (3 * cin)) * co2 + co) * 3 * cin + r % (3 * cin)]);
            dec.push(Dec { cin, cout: cout_z, last, conv_tr, rewrite, dconv: dconv(t, &format!("decoder.{i}"), cin)? });
            let conv_tr = lin_tr(t, &format!("tdecoder.{i}.conv_tr"), &[cin, cout, 8])?
                .taps(8, cout, cin, |w, k, co, ci| w[(ci * cout + co) * 8 + k]);
            let rewrite = lin(t, &format!("tdecoder.{i}.rewrite"), &[2 * cin, cin, 3])?.rows(2 * cin, 3 * cin);
            tdec.push(Dec { cin, cout, last, conv_tr, rewrite, dconv: dconv(t, &format!("tdecoder.{i}"), cin)? });
        }
        let freq_emb = t.take("freq_emb.embedding.weight", &[512, 48])?;
        let up = lin(t, "channel_upsampler", &[DIM, 384, 1])?.rows(DIM, 384);
        let down = lin(t, "channel_downsampler", &[384, DIM, 1])?.rows(384, DIM);
        let up_t = lin(t, "channel_upsampler_t", &[DIM, 384, 1])?.rows(DIM, 384);
        let down_t = lin(t, "channel_downsampler_t", &[384, DIM, 1])?.rows(384, DIM);
        let norm_in = norm(t, "crosstransformer.norm_in", DIM)?;
        let norm_in_t = norm(t, "crosstransformer.norm_in_t", DIM)?;
        let mut layers = vec![];
        let mut layers_t = vec![];
        for i in 0..5 {
            layers.push(layer(t, &format!("crosstransformer.layers.{i}"), i % 2 == 1)?);
            layers_t.push(layer(t, &format!("crosstransformer.layers_t.{i}"), i % 2 == 1)?);
        }
        if let Some(n) = t.names().next() { anyhow::bail!("unused tensor {n}: not the htdemucs model?") }
        Ok(Self { enc, tenc, dec, tdec, freq_emb, up, down, up_t, down_t, norm_in, norm_in_t, layers, layers_t })
    }
}

// ------------------------------------------------------------ convolutions

/// Conv1d over `(cin, t)`: weight packed `(cout, cin·k)`, stride, padding, dilation.
#[allow(clippy::too_many_arguments)]
fn conv1d(x: &[f32], cin: usize, t: usize, l: &Lin, cout: usize, k: usize, stride: usize, pad: usize, dil: usize) -> (Vec<f32>, usize) {
    let tp = t + 2 * pad;
    let tout = (tp - dil * (k - 1) - 1) / stride + 1;
    let padded;
    let (xp, rs) = if pad == 0 { (x, t) } else {
        let mut p = vec![0f32; cin * tp];
        for c in 0..cin { p[c * tp + pad..c * tp + pad + t].copy_from_slice(&x[c * t..(c + 1) * t]) }
        padded = p;
        (&padded[..], tp)
    };
    let mut out = vec![0f32; cout * tout];
    fill_bias(&mut out, &l.b, tout);
    let rows: Vec<usize> = (0..cin * k).map(|r| (r / k) * rs + (r % k) * dil).collect();
    gemm::gemm(&l.p[0], View { b: xp, base: 0, row_off: &rows, cs: stride }, tout, &mut out, Out { off: 0, rs: tout, cs: 1 });
    (out, tout)
}

/// ConvTranspose1d over `(cin, t)`: one packed `(cout, cin)` weight per tap,
/// then the `[pad, pad+len)` window.
#[allow(clippy::too_many_arguments)]
fn conv_tr1d(x: &[f32], cin: usize, t: usize, l: &Lin, cout: usize, k: usize, stride: usize, pad: usize, len: usize) -> Vec<f32> {
    let mut out = vec![0f32; cout * len];
    fill_bias(&mut out, &l.b, len);
    let rows: Vec<usize> = (0..cin).map(|c| c * t).collect();
    for kk in 0..k {
        // Input sample i lands at i·stride + kk - pad; keep the ones inside [0, len).
        let lo = (pad.saturating_sub(kk)).div_ceil(stride);
        let hi = t.min((len + pad - kk - 1) / stride + 1);
        if lo >= hi { continue }
        gemm::gemm(&l.p[kk], View { b: x, base: lo, row_off: &rows, cs: 1 }, hi - lo, &mut out,
                   Out { off: lo * stride + kk - pad, rs: len, cs: stride });
    }
    out
}

/// Fill each row (length `t`) of a `(c, t)` block with that channel's bias.
fn fill_bias(band: &mut [f32], b: &[f32], t: usize) {
    for (co, row) in band.chunks_exact_mut(t).enumerate() { row.fill(b[co % b.len()]) }
}

/// Conv along frequency of `(f, cin, t)`: kernel 8 stride 4 pad 2, weight
/// packed `(cout, 8·cin)`. One product per output band.
fn conv_freq(x: &[f32], f: usize, cin: usize, t: usize, l: &Lin, cout: usize) -> (Vec<f32>, usize) {
    let (k, s, p) = (8, 4, 2);
    let fout = (f + 2 * p - k) / s + 1;
    let mut xp = vec![0f32; (f + 2 * p) * cin * t];
    xp[p * cin * t..(p + f) * cin * t].copy_from_slice(&x[..f * cin * t]);
    let rows: Vec<usize> = (0..k * cin).map(|r| r * t).collect();
    let mut out = vec![0f32; fout * cout * t];
    out.par_chunks_mut(cout * t).enumerate().for_each(|(fo, band)| {
        fill_bias(band, &l.b, t);
        gemm::gemm1(&l.p[0], View { b: &xp, base: fo * s * cin * t, row_off: &rows, cs: 1 }, t, band, Out { off: 0, rs: t, cs: 1 });
    });
    (out, fout)
}

/// ConvTranspose along frequency of `(f, cin, t)`: kernel 8 stride 4, then
/// trim 2 bands each side → `(4f, cout, t)`. One packed `(cout, cin)` per tap.
/// Computed per output band: band `o` (untrimmed `o+2`) gets tap `o+2-4i`
/// of input bands `i = (o+2)/4` and the one before.
fn conv_tr_freq(x: &[f32], f: usize, cin: usize, t: usize, l: &Lin, cout: usize) -> Vec<f32> {
    let (k, s, p): (usize, usize, usize) = (8, 4, 2);
    debug_assert_eq!(cout % gemm::MR, 0);
    let rows: Vec<usize> = (0..cin).map(|c| c * t).collect();
    let fo = f * s;
    let mut out = vec![0f32; fo * cout * t];
    out.par_chunks_mut(cout * t).for_each(|band| fill_bias(band, &l.b, t));
    // Input band i, all 8 taps in one product, lands on output bands
    // 4i-2 .. 4i+6. Bands i and i+2 never share an output band, so the even
    // inputs run in parallel, then the odd ones.
    let op = out.as_mut_ptr() as usize;
    for parity in 0..2 {
        (parity..f).step_by(2).collect::<Vec<_>>().into_par_iter().for_each(|i| {
            let k_lo = p.saturating_sub(i * s);
            let k_hi = k.min(fo + p - i * s);
            let band0 = i * s + k_lo - p;
            // SAFETY: this task writes only output bands [band0, band0 + k_hi - k_lo),
            // which no other input band of the same parity touches (see above).
            let dst = unsafe { std::slice::from_raw_parts_mut((op as *mut f32).add(band0 * cout * t), (k_hi - k_lo) * cout * t) };
            gemm::gemm1_part(&l.p[0], k_lo * cout / gemm::MR..k_hi * cout / gemm::MR, 0..cin,
                             View { b: x, base: i * cin * t, row_off: &rows, cs: 1 }, t, dst, Out { off: 0, rs: t, cs: 1 }, false);
        });
    }
    out
}

/// 1x1 conv on every band of `(f, cin, t)`.
fn conv1x1_freq(x: &[f32], f: usize, cin: usize, t: usize, l: &Lin, cout: usize) -> Vec<f32> {
    let rows: Vec<usize> = (0..cin).map(|c| c * t).collect();
    let mut out = vec![0f32; f * cout * t];
    out.par_chunks_mut(cout * t).enumerate().for_each(|(ff, band)| {
        fill_bias(band, &l.b, t);
        gemm::gemm1(&l.p[0], View { b: x, base: ff * cin * t, row_off: &rows, cs: 1 }, t, band, Out { off: 0, rs: t, cs: 1 });
    });
    out
}

/// 3x3 conv (pad 1) over (freq, time) of `(f, cin, t)`; weight packed
/// `(cout, 9·cin)`, column `(dt·3 + df)·cin + ci`.
fn conv3x3_freq(x: &[f32], f: usize, cin: usize, t: usize, l: &Lin, cout: usize) -> Vec<f32> {
    let tp = t + 2;
    let mut xp = vec![0f32; (f + 2) * cin * tp];
    xp[cin * tp..(f + 1) * cin * tp].par_chunks_mut(tp).zip(x.par_chunks(t))
        .for_each(|(d, s)| d[1..1 + t].copy_from_slice(s));
    let rows: Vec<usize> = (0..9 * cin).map(|r| (r % (3 * cin)) * tp + r / (3 * cin)).collect();
    let mut out = vec![0f32; f * cout * t];
    out.par_chunks_mut(cout * t).enumerate().for_each(|(ff, band)| {
        fill_bias(band, &l.b, t);
        gemm::gemm1(&l.p[0], View { b: &xp, base: ff * cin * tp, row_off: &rows, cs: 1 }, t, band, Out { off: 0, rs: t, cs: 1 });
    });
    out
}

/// DConv residual branch on one `(c, t)` block, in place.
fn dconv_block(x: &mut [f32], c: usize, t: usize, layers: &[DLayer]) {
    for d in layers {
        let (mut y, _) = conv1d(x, c, t, &d.c1, d.hidden, 3, 1, d.dil, d.dil);
        ops::gn_gelu(&mut y, d.hidden, t, &d.n1.w, &d.n1.b);
        if d.hidden <= TAIL_MAX_HIDDEN {
            ops::dconv_tail(&y, d.hidden, t, &d.w2, &d.c2.b, &d.n2.w, &d.n2.b, &d.scale, c, x);
        } else {
            let (z, _) = conv1d(&y, d.hidden, t, &d.c2, 2 * c, 1, 1, 0, 1);
            ops::gn_glu_add(&z, c, t, &d.n2.w, &d.n2.b, &d.scale, x);
        }
    }
}

// --------------------------------------------------------- encoder/decoder

/// Frequency encoder layer: `(f, cin, t)` → `(f/4, cout, t)`.
fn enc_freq(e: &Enc, x: &[f32], f: usize, t: usize) -> (Vec<f32>, usize) {
    let mut lap = Lap::new();
    let (mut y, fo) = conv_freq(x, f, e.cin, t, &e.conv, e.cout);
    lap.lap("e.conv");
    ops::gelu_all(&mut y);
    y.par_chunks_mut(e.cout * t).for_each(|band| dconv_block(band, e.cout, t, &e.dconv));
    lap.lap("e.dconv");
    let z = conv1x1_freq(&y, fo, e.cout, t, &e.rewrite, 2 * e.cout);
    lap.lap("e.rewrite");
    let mut out = vec![0f32; fo * e.cout * t];
    out.par_chunks_mut(e.cout * t).zip(z.par_chunks(2 * e.cout * t))
        .for_each(|(o, zb)| ops::glu(zb, e.cout, t, o));
    (out, fo)
}

/// Time encoder layer: `(cin, t)` → `(cout, ceil(t/4))`.
fn enc_time(e: &Enc, x: &[f32], t: usize) -> (Vec<f32>, usize) {
    let tt = t.div_ceil(4) * 4;
    let padded;
    let x = if tt == t { x } else {
        let mut p = vec![0f32; e.cin * tt];
        for c in 0..e.cin { p[c * tt..c * tt + t].copy_from_slice(&x[c * t..(c + 1) * t]) }
        padded = p;
        &padded[..]
    };
    let mut lap = Lap::new();
    let (mut y, to) = conv1d(x, e.cin, tt, &e.conv, e.cout, 8, 4, 2, 1);
    lap.lap("te.conv");
    ops::gelu_all(&mut y);
    dconv_block(&mut y, e.cout, to, &e.dconv);
    lap.lap("te.dconv");
    let (z, _) = conv1d(&y, e.cout, to, &e.rewrite, 2 * e.cout, 1, 1, 0, 1);
    lap.lap("te.rewrite");
    let mut out = vec![0f32; e.cout * to];
    ops::glu(&z, e.cout, to, &mut out);
    (out, to)
}

/// Frequency decoder layer: `x + skip` `(f, cin, t)` → `(4f, cout, t)`.
fn dec_freq(d: &Dec, x: &mut [f32], skip: &[f32], f: usize, t: usize) -> Vec<f32> {
    let mut lap = Lap::new();
    ops::add(x, skip);
    let z = conv3x3_freq(x, f, d.cin, t, &d.rewrite, 2 * d.cin);
    lap.lap("d.rewrite");
    let mut y = vec![0f32; f * d.cin * t];
    y.par_chunks_mut(d.cin * t).zip(z.par_chunks(2 * d.cin * t)).for_each(|(yb, zb)| {
        ops::glu(zb, d.cin, t, yb);
        dconv_block(yb, d.cin, t, &d.dconv);
    });
    lap.lap("d.dconv");
    let mut out = conv_tr_freq(&y, f, d.cin, t, &d.conv_tr, d.cout);
    lap.lap("d.conv_tr");
    if !d.last { ops::gelu_all(&mut out) }
    out
}

/// Time decoder layer: `x + skip` `(cin, t)` → `(cout, len)`.
fn dec_time(d: &Dec, x: &mut [f32], skip: &[f32], t: usize, len: usize) -> Vec<f32> {
    let mut lap = Lap::new();
    ops::add(x, skip);
    let (z, _) = conv1d(x, d.cin, t, &d.rewrite, 2 * d.cin, 3, 1, 1, 1);
    lap.lap("td.rewrite");
    let mut y = vec![0f32; d.cin * t];
    ops::glu(&z, d.cin, t, &mut y);
    dconv_block(&mut y, d.cin, t, &d.dconv);
    lap.lap("td.dconv");
    let mut out = conv_tr1d(&y, d.cin, t, &d.conv_tr, d.cout, 8, 4, 2, len);
    lap.lap("td.conv_tr");
    if !d.last { ops::gelu_all(&mut out) }
    out
}

// ------------------------------------------------------------- transformer

/// `(n, in)` → `(n, out)`: `x·Wᵀ + b`, W `(out, in)`.
fn linear(x: &[f32], n: usize, l: &Lin, din: usize, dout: usize) -> Vec<f32> {
    let mut y = vec![0f32; n * dout];
    for r in y.chunks_exact_mut(dout) { r.copy_from_slice(&l.b[..dout]) }
    tokens_times(&l.p[0], x, n, din, &mut y, dout);
    y
}

/// `y (n, dout) += x (n, din) · Aᵀ` for a packed `A (dout, din)`: computed as
/// `yᵀ = A·xᵀ`, reading x and writing y in place through strides.
fn tokens_times(a: &Packed, x: &[f32], n: usize, din: usize, y: &mut [f32], dout: usize) {
    let rows: Vec<usize> = (0..din).collect();
    gemm::gemm(a, View { b: x, base: 0, row_off: &rows, cs: din }, n, y, Out { off: 0, rs: 1, cs: dout });
}

/// Queries per attention block, and keys per tile within it.
const BLOCK: usize = 128;
const TK: usize = 256;

/// Multi-head attention: queries from `q (n, DIM)`, keys/values from `kv (m, DIM)`.
///
/// Each head's K and Vᵀ are packed once per layer. Blocks of queries run in
/// parallel, all heads each, and walk the keys in tiles of [`TK`] with an
/// online softmax (flash attention): `Sᵀ = K_tile·Qᵀ` (keys × queries), the
/// tile folded into each query's running max and sum, `Oᵀ += Vᵀ_tile·Pᵀ`
/// after scaling what is there. A task's working set (one K tile, one Vᵀ
/// tile, the scores, Oᵀ) stays inside the A76's 512 KB L2.
fn mha(a: &Attn, q: &[f32], n: usize, kv: &[f32], m: usize) -> Vec<f32> {
    let proj = |x: &[f32], rows: usize, part: usize| {
        let mut y = vec![0f32; rows * DIM];
        for r in y.chunks_exact_mut(DIM) { r.copy_from_slice(&a.in_b[part * DIM..(part + 1) * DIM]) }
        tokens_times(&a.in_p[part], x, rows, DIM, &mut y, DIM);
        y
    };
    let mut lap = Lap::new();
    let (qq, kk, vv) = (proj(q, n, 0), proj(kv, m, 1), proj(kv, m, 2));
    lap.lap("qkv");
    let hd = DIM / HEADS;
    let scale = 1.0 / (hd as f32).sqrt();
    let heads: Vec<(Packed, Packed)> = (0..HEADS).into_par_iter().map(|h| (
        Packed::new(m, hd, |j, d| kk[j * DIM + h * hd + d] * scale).half(),
        Packed::new(hd, m, |d, j| vv[j * DIM + h * hd + d]).half(),
    )).collect();
    lap.lap("pack kv");
    let qrows: Vec<usize> = (0..hd).collect();
    let mut o = vec![0f32; n * DIM];
    o.par_chunks_mut(BLOCK * DIM).enumerate().for_each(|(blk, ob)| {
        let rows = ob.len() / DIM;
        let r0 = blk * BLOCK;
        let mut s = vec![0f32; TK * rows];
        let mut ot = vec![0f32; hd * rows];
        let (mut mx, mut l, mut alpha) = (vec![0f32; rows], vec![0f32; rows], vec![0f32; rows]);
        let prows: Vec<usize> = (0..TK).map(|j| j * rows).collect();
        for (h, (kp, vp)) in heads.iter().enumerate() {
            mx.fill(f32::NEG_INFINITY);
            l.fill(0.0);
            ot.fill(0.0);
            for k0 in (0..m).step_by(TK) {
                let nk = (m - k0).min(TK);
                // Sᵀ tile (nk × rows) = K[k0..k0+nk] · Q_blkᵀ.
                gemm::gemm1_part(kp, k0 / gemm::MR..(k0 + nk).div_ceil(gemm::MR), 0..hd,
                                 View { b: &qq, base: r0 * DIM + h * hd, row_off: &qrows, cs: DIM }, rows,
                                 &mut s, Out { off: 0, rs: rows, cs: 1 }, true);
                let st = &mut s[..nk * rows];
                ops::softmax_tile(st, rows, &mut mx, &mut l, &mut alpha);
                for row in ot.chunks_exact_mut(rows) { for (v, a) in row.iter_mut().zip(&alpha) { *v *= a } }
                // Oᵀ (hd × rows) += Vᵀ[.., k0..k0+nk] · Pᵀ.
                gemm::gemm1_part(vp, 0..hd / gemm::MR, k0..k0 + nk, View { b: st, base: 0, row_off: &prows[..nk], cs: 1 },
                                 rows, &mut ot, Out { off: 0, rs: rows, cs: 1 }, false);
            }
            for (d, row) in ot.chunks_exact(rows).enumerate() {
                for r in 0..rows { ob[r * DIM + h * hd + d] = row[r] / l[r] }
            }
        }
    });
    lap.lap("attention");
    let r = linear(&o, n, &a.out, DIM, DIM);
    lap.lap("out proj");
    r
}

/// One transformer layer (norm first, layer scale, GroupNorm out), in place.
fn tlayer(l: &Layer, x: &mut [f32], n: usize, kv: Option<(&[f32], usize)>) {
    let mut lap = Lap::new();
    let mut h = vec![0f32; n * DIM];
    ops::layer_norm(x, DIM, &l.norm1.w, &l.norm1.b, &mut h);
    let a = match kv {
        None => mha(&l.attn, &h, n, &h, n),
        Some((k, m)) => {
            let mut hk = vec![0f32; m * DIM];
            ops::layer_norm(k, DIM, &l.norm2.w, &l.norm2.b, &mut hk);
            mha(&l.attn, &h, n, &hk, m)
        }
    };
    for (row, ar) in x.chunks_exact_mut(DIM).zip(a.chunks_exact(DIM)) {
        for i in 0..DIM { row[i] += l.g1[i] * ar[i] }
    }
    lap.lap("mha+res");
    let nf = if l.cross { l.norm3.as_ref().unwrap() } else { &l.norm2 };
    ops::layer_norm(x, DIM, &nf.w, &nf.b, &mut h);
    lap.lap("norm");
    let mut f1 = linear(&h, n, &l.lin1, DIM, 4 * DIM);
    lap.lap("linear1");
    ops::gelu_all(&mut f1);
    lap.lap("gelu");
    let f2 = linear(&f1, n, &l.lin2, 4 * DIM, DIM);
    lap.lap("linear2");
    for (row, fr) in x.chunks_exact_mut(DIM).zip(f2.chunks_exact(DIM)) {
        for i in 0..DIM { row[i] += l.g2[i] * fr[i] }
    }
    ops::group_norm1_tokens(x, DIM, &l.norm_out.w, &l.norm_out.b);
    lap.lap("res+gn");
}

/// `create_2d_sin_embedding(DIM, fr, t1)` at token `t*fr + f`.
fn pos_2d(fr: usize, t1: usize) -> Vec<f32> {
    let d = DIM / 2;
    let mut pe = vec![0f32; fr * t1 * DIM];
    for j in 0..d / 2 {
        let div = (((2 * j) as f32) * -(10000f32.ln() / d as f32)).exp();
        for t in 0..t1 { for f in 0..fr {
            let row = &mut pe[(t * fr + f) * DIM..];
            let (w, h) = (t as f32 * div, f as f32 * div);
            row[2 * j] = w.sin();
            row[2 * j + 1] = w.cos();
            row[d + 2 * j] = h.sin();
            row[d + 2 * j + 1] = h.cos();
        }}
    }
    pe
}

/// `create_sin_embedding(t, DIM)`.
fn pos_1d(t: usize) -> Vec<f32> {
    let half = DIM / 2;
    let mut pe = vec![0f32; t * DIM];
    for j in 0..half {
        let period = 10000f32.powf(j as f32 / (half - 1) as f32);
        for p in 0..t {
            let ph = p as f32 / period;
            pe[p * DIM + j] = ph.cos();
            pe[p * DIM + half + j] = ph.sin();
        }
    }
    pe
}

// ------------------------------------------------------------------- model

/// `(f, c, t)` → PyTorch's `(c, f, t)`.
fn fct_to_cft(x: &[f32], f: usize, c: usize, t: usize) -> Vec<f32> {
    let mut r = vec![0f32; x.len()];
    for ff in 0..f { for cc in 0..c { r[(cc * f + ff) * t..(cc * f + ff + 1) * t].copy_from_slice(&x[(ff * c + cc) * t..(ff * c + cc + 1) * t]) } }
    r
}

impl HtDemucs {
    /// Separate one chunk. `mix` is planar stereo `[2][SEGMENT]`; `out` is
    /// planar `[4 sources][2][SEGMENT]` (drums, bass, other, vocals).
    pub fn run(&self, mix: &[f32], out: &mut [f32]) { self.run_tapped(mix, out, None) }

    pub fn run_tapped(&self, mix: &[f32], out: &mut [f32], mut tap: Option<Tap>) {
        let len = SEGMENT;
        assert_eq!(mix.len(), 2 * len);
        assert_eq!(out.len(), 8 * len);
        let mut emit = |name: &str, shape: &[usize], d: &dyn Fn() -> Vec<f32>| {
            if let Some(t) = tap.as_mut() { t(name, shape, &d()) }
        };
        let mut planner = RealFftPlanner::<f32>::new();

        // Spectrogram, complex as channels: (F, 4, T), channels ch0.re ch0.im ch1.re ch1.im.
        let z: Vec<Vec<Complex32>> = (0..2).map(|c| spec::spec(&mix[c * len..(c + 1) * len], &mut planner)).collect();
        let (fq, t) = (spec::BINS, spec::frames(len));
        emit("spec", &[2, fq, t, 2], &|| z.iter().flat_map(|zc| zc.iter().flat_map(|v| [v.re, v.im])).collect());
        let mut x0 = vec![0f32; fq * 4 * t];
        for f in 0..fq { for c in 0..2 { for tt in 0..t {
            let v = z[c][f * t + tt];
            x0[(f * 4 + 2 * c) * t + tt] = v.re;
            x0[(f * 4 + 2 * c + 1) * t + tt] = v.im;
        }}}
        let (mean, std) = ops::mean_std(&x0);
        for v in &mut x0 { *v = (*v - mean) / (1e-5 + std) }
        let (meant, stdt) = ops::mean_std(mix);
        let xt0: Vec<f32> = mix.iter().map(|v| (v - meant) / (1e-5 + stdt)).collect();

        // Encoders.
        let (mut f, mut tl) = (fq, len);
        let (mut saved, mut saved_t, mut lens_t): (Vec<Vec<f32>>, Vec<Vec<f32>>, Vec<usize>) = (vec![], vec![], vec![]);
        for i in 0..DEPTH {
            lens_t.push(tl);
            let (y, to) = enc_time(&self.tenc[i], saved_t.last().unwrap_or(&xt0), tl);
            tl = to;
            emit(&format!("tenc{i}"), &[self.tenc[i].cout, tl], &|| y.clone());
            saved_t.push(y);
            let (mut x, fo) = enc_freq(&self.enc[i], saved.last().unwrap_or(&x0), f, t);
            f = fo;
            let c = self.enc[i].cout;
            emit(&format!("enc{i}"), &[c, f, t], &|| fct_to_cft(&x, f, c, t));
            if i == 0 {
                let c = self.enc[0].cout;
                for ff in 0..f { for cc in 0..c {
                    let e = 0.2 * 10.0 * self.freq_emb[ff * c + cc];
                    for v in &mut x[(ff * c + cc) * t..(ff * c + cc + 1) * t] { *v += e }
                }}
            }
            saved.push(x);
        }
        let xt = saved_t.last().unwrap();
        let x = saved.last().unwrap();

        // Transformer: tokens (t*f + f_idx, DIM) for the spectrogram branch.
        let (n, m) = (f * t, tl);
        let mut tok = vec![0f32; n * DIM];
        let rows: Vec<usize> = (0..384).map(|c| c * t).collect();
        for ff in 0..f {
            gemm::gemm(&self.up.p[0], View { b: &x, base: ff * 384 * t, row_off: &rows, cs: 1 }, t, &mut tok,
                       Out { off: ff * DIM, rs: 1, cs: f * DIM });
        }
        for r in tok.chunks_exact_mut(DIM) { for i in 0..DIM { r[i] += self.up.b[i] } }
        let mut tok_t = vec![0f32; m * DIM];
        for r in tok_t.chunks_exact_mut(DIM) { r.copy_from_slice(&self.up_t.b) }
        let rows: Vec<usize> = (0..384).map(|c| c * m).collect();
        gemm::gemm(&self.up_t.p[0], View { b: &xt, base: 0, row_off: &rows, cs: 1 }, m, &mut tok_t, Out { off: 0, rs: 1, cs: DIM });
        emit("up_t", &[DIM, m], &|| { let mut r = vec![0f32; DIM * m]; for p in 0..m { for c in 0..DIM { r[c * m + p] = tok_t[p * DIM + c] } } r });
        let mut h = vec![0f32; n * DIM];
        ops::layer_norm(&tok, DIM, &self.norm_in.w, &self.norm_in.b, &mut h);
        for (a, p) in h.iter_mut().zip(pos_2d(f, t)) { *a += p }
        tok = h;
        let mut h = vec![0f32; m * DIM];
        ops::layer_norm(&tok_t, DIM, &self.norm_in_t.w, &self.norm_in_t.b, &mut h);
        for (a, p) in h.iter_mut().zip(pos_1d(m)) { *a += p }
        tok_t = h;
        for i in 0..5 {
            if self.layers[i].cross {
                let old = tok.clone();
                tlayer(&self.layers[i], &mut tok, n, Some((&tok_t, m)));
                tlayer(&self.layers_t[i], &mut tok_t, m, Some((&old, n)));
            } else {
                tlayer(&self.layers[i], &mut tok, n, None);
                tlayer(&self.layers_t[i], &mut tok_t, m, None);
            }
            emit(&format!("tf{i}"), &[n, DIM], &|| tok.clone());
            emit(&format!("tft{i}"), &[m, DIM], &|| tok_t.clone());
        }
        let mut x = vec![0f32; f * 384 * t];
        let dims: Vec<usize> = (0..DIM).collect();
        for ff in 0..f {
            for co in 0..384 { x[(ff * 384 + co) * t..(ff * 384 + co + 1) * t].fill(self.down.b[co]) }
            gemm::gemm(&self.down.p[0], View { b: &tok, base: ff * DIM, row_off: &dims, cs: f * DIM }, t, &mut x,
                       Out { off: ff * 384 * t, rs: t, cs: 1 });
        }
        let mut xt = vec![0f32; 384 * m];
        for co in 0..384 { xt[co * m..(co + 1) * m].fill(self.down_t.b[co]) }
        gemm::gemm(&self.down_t.p[0], View { b: &tok_t, base: 0, row_off: &dims, cs: DIM }, m, &mut xt, Out { off: 0, rs: m, cs: 1 });
        emit("down_t", &[384, m], &|| xt.clone());

        // Decoders.
        let mut tl = m;
        for i in 0..DEPTH {
            let skip = saved.pop().unwrap();
            let y = dec_freq(&self.dec[i], &mut x, &skip, f, t);
            x = y; f *= 4;
            let c = self.dec[i].cout;
            emit(&format!("dec{i}"), &[c, f, t], &|| fct_to_cft(&x, f, c, t));
            let skip = saved_t.pop().unwrap();
            let lt = lens_t.pop().unwrap();
            let y = dec_time(&self.tdec[i], &mut xt, &skip, tl, lt);
            xt = y; tl = lt;
            emit(&format!("tdec{i}"), &[self.tdec[i].cout, tl], &|| xt.clone());
        }

        // Back to audio: spectrogram branch through the iSTFT, plus the time branch.
        for v in &mut x { *v = *v * std + mean }
        for s in 0..4 { for c in 0..2 {
            let mut zc = vec![Complex32::new(0.0, 0.0); fq * t];
            for ff in 0..fq { for tt in 0..t {
                zc[ff * t + tt] = Complex32::new(x[(ff * 16 + s * 4 + 2 * c) * t + tt], x[(ff * 16 + s * 4 + 2 * c + 1) * t + tt]);
            }}
            let w = spec::ispec(&zc, t, len, &mut planner);
            let o = &mut out[(s * 2 + c) * len..(s * 2 + c + 1) * len];
            let xtc = &xt[(s * 2 + c) * len..(s * 2 + c + 1) * len];
            for ((o, a), b) in o.iter_mut().zip(&w).zip(xtc) { *o = a + (b * stdt + meant) }
        }}
        emit("stems", &[4, 2, len], &|| out.to_vec());
    }
}

/// Kernels exposed for `examples/kernels.rs`.
#[doc(hidden)]
pub mod bench {
    use super::ops::{self, M};
    pub fn gemm(m: usize, k: usize, n: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
        ops::gemm(m, k, n, 1.0, M::new(a, 0, k, 1), M::new(b, 0, n, 1), 0.0, c, 0, n, 1)
    }
    pub fn pgemm(m: usize, k: usize, n: usize, a: &[f32], b: &[f32], c: &mut [f32]) {
        ops::pgemm(m, k, n, 1.0, M::new(a, 0, k, 1), M::new(b, 0, n, 1), 0.0, c, 0, n, 1)
    }
    pub fn softmax(x: &mut [f32], m: usize) { ops::softmax_rows(x, m) }
    /// ns per element: exp over a slice; and softmax_cols' passes on (m, cols).
    pub fn exp_and_softmax(m: usize, cols: usize) -> (f64, f64) {
        let mut x: Vec<f32> = (0..m * cols).map(|i| ((i * 7919) % 1000) as f32 * -0.003).collect();
        let t = std::time::Instant::now();
        for _ in 0..3 { for v in x.iter_mut() { *v = ops::exp(*v) - 1.0 } }
        let e = t.elapsed().as_secs_f64() / 3.0 / x.len() as f64 * 1e9;
        let t = std::time::Instant::now();
        for _ in 0..3 { ops::softmax_cols(&mut x, cols) }
        (e, t.elapsed().as_secs_f64() / 3.0 / x.len() as f64 * 1e9)
    }
    /// One attention block on one thread (`rows` queries against `m` keys,
    /// one head of 64): seconds for the scores, the softmax and P·V.
    pub fn attention_block(rows: usize, m: usize) -> [f64; 3] {
        use super::gemm::{gemm1_set, Out, Packed, View};
        let hd = 64;
        let kp = Packed::new(m, hd, |j, d| ((j * 7 + d) % 13) as f32 * 0.01);
        let vp = Packed::new(hd, m, |d, j| ((j * 3 + d) % 11) as f32 * 0.01);
        let q: Vec<f32> = (0..rows * super::DIM).map(|i| (i % 17) as f32 * 0.01).collect();
        let qrows: Vec<usize> = (0..hd).collect();
        let prows: Vec<usize> = (0..m).map(|j| j * rows).collect();
        let mut s = vec![0f32; m * rows];
        let mut o = vec![0f32; rows * super::DIM];
        let mut t = [0f64; 3];
        for rep in 0..4 {
            let t0 = std::time::Instant::now();
            gemm1_set(&kp, View { b: &q, base: 0, row_off: &qrows, cs: super::DIM }, rows, &mut s, Out { off: 0, rs: rows, cs: 1 });
            let t1 = std::time::Instant::now();
            ops::softmax_cols(&mut s, rows);
            let t2 = std::time::Instant::now();
            gemm1_set(&vp, View { b: &s, base: 0, row_off: &prows, cs: 1 }, rows, &mut o, Out { off: 0, rs: 1, cs: super::DIM });
            let t3 = std::time::Instant::now();
            if rep > 0 { t[0] += (t1 - t0).as_secs_f64(); t[1] += (t2 - t1).as_secs_f64(); t[2] += (t3 - t2).as_secs_f64() }
        }
        t.map(|v| v / 3.0)
    }
    /// `C (m×n) = A·B` through the packed multiply (A packed outside the timing).
    pub fn packed(m: usize, k: usize, n: usize) -> impl FnMut() {
        let a = super::gemm::Packed::new(m, k, |i, kk| ((i + kk) % 7) as f32 * 0.1);
        let b = vec![0.25f32; k * n];
        let rows: Vec<usize> = (0..k).map(|r| r * n).collect();
        let mut c = vec![0f32; m * n];
        move || super::gemm::gemm(&a, super::gemm::View { b: &b, base: 0, row_off: &rows, cs: 1 }, n, &mut c,
                                  super::gemm::Out { off: 0, rs: n, cs: 1 })
    }
}
