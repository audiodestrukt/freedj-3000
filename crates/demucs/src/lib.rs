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

mod ops;
pub mod spec;
mod st;

use anyhow::Result;
use ops::{gemm, pgemm, M};
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

/// Called with each stage's output in PyTorch's layout (batch dim dropped),
/// for comparing against a reference run.
pub type Tap<'a> = &'a mut dyn FnMut(&str, &[usize], &[f32]);

struct Lin { w: Vec<f32>, b: Vec<f32> }
struct Norm { w: Vec<f32>, b: Vec<f32> }

/// One DConv residual layer: conv k3 (dilated) → GN → GELU → conv 1x1 → GN → GLU → scale.
struct DLayer { c1: Lin, n1: Norm, c2: Lin, n2: Norm, scale: Vec<f32>, dil: usize, hidden: usize }

struct Enc { cin: usize, cout: usize, conv: Lin, rewrite: Lin, dconv: Vec<DLayer> }
struct Dec { cin: usize, cout: usize, last: bool, conv_tr: Lin, rewrite: Lin, dconv: Vec<DLayer> }

struct Attn { in_w: Vec<f32>, in_b: Vec<f32>, out: Lin }
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
    Ok(Lin { w: t.take(&format!("{p}.weight"), w)?, b: t.take(&format!("{p}.bias"), &[w[0]])? })
}
fn lin_tr(t: &mut Tensors, p: &str, w: &[usize]) -> Result<Lin> {
    Ok(Lin { w: t.take(&format!("{p}.weight"), w)?, b: t.take(&format!("{p}.bias"), &[w[1]])? })
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
        Ok(DLayer {
            c1: lin(t, &format!("{q}.0"), &[h, c, 3])?, n1: norm(t, &format!("{q}.1"), h)?,
            c2: lin(t, &format!("{q}.3"), &[2 * c, h, 1])?, n2: norm(t, &format!("{q}.4"), 2 * c)?,
            scale: t.take(&format!("{q}.6.scale"), &[c])?, dil: 1 << d, hidden: h,
        })
    }).collect()
}

fn attn(t: &mut Tensors, p: &str) -> Result<Attn> {
    Ok(Attn {
        in_w: t.take(&format!("{p}.in_proj_weight"), &[3 * DIM, DIM])?,
        in_b: t.take(&format!("{p}.in_proj_bias"), &[3 * DIM])?,
        out: lin(t, &format!("{p}.out_proj"), &[DIM, DIM])?,
    })
}

fn layer(t: &mut Tensors, p: &str, cross: bool) -> Result<Layer> {
    Ok(Layer {
        cross,
        attn: attn(t, &format!("{p}.{}", if cross { "cross_attn" } else { "self_attn" }))?,
        lin1: lin(t, &format!("{p}.linear1"), &[4 * DIM, DIM])?,
        lin2: lin(t, &format!("{p}.linear2"), &[DIM, 4 * DIM])?,
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
            let rewrite = lin(t, &format!("encoder.{i}.rewrite"), &[2 * cout, cout, 1, 1])?;
            enc.push(Enc { cin: cin_z, cout, conv, rewrite, dconv: dconv(t, &format!("encoder.{i}"), cout)? });
            let conv = lin(t, &format!("tencoder.{i}.conv"), &[cout, cin, 8])?;
            let rewrite = lin(t, &format!("tencoder.{i}.rewrite"), &[2 * cout, cout, 1])?;
            tenc.push(Enc { cin, cout, conv, rewrite, dconv: dconv(t, &format!("tencoder.{i}"), cout)? });
        }
        for i in 0..DEPTH {
            // decoder.i undoes encoder.(DEPTH-1-i)
            let cin = 384 >> i;
            let last = i == DEPTH - 1;
            let (cout_z, cout) = if last { (16, 8) } else { (cin / 2, cin / 2) };
            let mut conv_tr = lin_tr(t, &format!("decoder.{i}.conv_tr"), &[cin, cout_z, 8, 1])?;
            conv_tr.w = freq_kernel_tr(&conv_tr.w, cin, cout_z, 8);
            let mut rewrite = lin(t, &format!("decoder.{i}.rewrite"), &[2 * cin, cin, 3, 3])?;
            rewrite.w = kernel_3x3(&rewrite.w, 2 * cin, cin);
            dec.push(Dec { cin, cout: cout_z, last, conv_tr, rewrite, dconv: dconv(t, &format!("decoder.{i}"), cin)? });
            let conv_tr = lin_tr(t, &format!("tdecoder.{i}.conv_tr"), &[cin, cout, 8])?;
            let rewrite = lin(t, &format!("tdecoder.{i}.rewrite"), &[2 * cin, cin, 3])?;
            tdec.push(Dec { cin, cout, last, conv_tr, rewrite, dconv: dconv(t, &format!("tdecoder.{i}"), cin)? });
        }
        let freq_emb = t.take("freq_emb.embedding.weight", &[512, 48])?;
        let up = lin(t, "channel_upsampler", &[DIM, 384, 1])?;
        let down = lin(t, "channel_downsampler", &[384, DIM, 1])?;
        let up_t = lin(t, "channel_upsampler_t", &[DIM, 384, 1])?;
        let down_t = lin(t, "channel_downsampler_t", &[384, DIM, 1])?;
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

/// Conv1d over `(cin, t)`: weight `(cout, cin, k)`, stride, padding, dilation.
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
    for co in 0..cout { out[co * tout..(co + 1) * tout].fill(l.b[co]) }
    for kk in 0..k {
        pgemm(cout, cin, tout, 1.0, M::new(&l.w, kk, cin * k, k), M::new(xp, kk * dil, rs, stride),
              1.0, &mut out, 0, tout, 1);
    }
    (out, tout)
}

/// ConvTranspose1d over `(cin, t)`: weight `(cin, cout, k)`, then the
/// `[pad, pad+len)` window.
#[allow(clippy::too_many_arguments)]
fn conv_tr1d(x: &[f32], cin: usize, t: usize, l: &Lin, cout: usize, k: usize, stride: usize, pad: usize, len: usize) -> Vec<f32> {
    let full = (t - 1) * stride + k;
    let mut out = vec![0f32; cout * full];
    for co in 0..cout { out[co * full..(co + 1) * full].fill(l.b[co]) }
    for kk in 0..k {
        pgemm(cout, cin, t, 1.0, M::new(&l.w, kk, k, cout * k), M::new(x, 0, t, 1),
              1.0, &mut out, kk, full, stride);
    }
    let mut r = vec![0f32; cout * len];
    for co in 0..cout { r[co * len..(co + 1) * len].copy_from_slice(&out[co * full + pad..co * full + pad + len]) }
    r
}

/// Fill each `(c, t)` band of a `(f, c, t)` buffer with the bias.
fn fill_bias(band: &mut [f32], b: &[f32], t: usize) {
    for (co, row) in band.chunks_exact_mut(t).enumerate() { row.fill(b[co]) }
}

/// Conv along frequency of `(f, cin, t)`: kernel 8 stride 4 pad 2, weight
/// reordered `(cout, 8, cin)`. One matrix multiply per output band.
fn conv_freq(x: &[f32], f: usize, cin: usize, t: usize, l: &Lin, cout: usize) -> (Vec<f32>, usize) {
    let (k, s, p) = (8, 4, 2);
    let fout = (f + 2 * p - k) / s + 1;
    let mut xp = vec![0f32; (f + 2 * p) * cin * t];
    xp[p * cin * t..(p + f) * cin * t].copy_from_slice(&x[..f * cin * t]);
    let mut out = vec![0f32; fout * cout * t];
    out.par_chunks_mut(cout * t).enumerate().for_each(|(fo, band)| {
        fill_bias(band, &l.b, t);
        gemm(cout, k * cin, t, 1.0, M::new(&l.w, 0, k * cin, 1), M::new(&xp, fo * s * cin * t, t, 1),
             1.0, band, 0, t, 1);
    });
    (out, fout)
}

/// ConvTranspose along frequency of `(f, cin, t)`: kernel 8 stride 4, then
/// trim 2 bands each side → `(4f, cout, t)`. Weight reordered `(8, cout, cin)`.
/// Computed per output band: band `o` (untrimmed `o+2`) gets tap `o+2-4i`
/// of input bands `i = (o+2)/4` and the one before.
fn conv_tr_freq(x: &[f32], f: usize, cin: usize, t: usize, l: &Lin, cout: usize) -> Vec<f32> {
    let (k, s, p) = (8, 4, 2);
    let mut out = vec![0f32; f * s * cout * t];
    out.par_chunks_mut(cout * t).enumerate().for_each(|(o, band)| {
        fill_bias(band, &l.b, t);
        let full = o + p;
        for i in [full / s, (full / s).wrapping_sub(1)] {
            if i >= f { continue }
            let tap = full - i * s;
            if tap >= k { continue }
            gemm(cout, cin, t, 1.0, M::new(&l.w, tap * cout * cin, cin, 1), M::new(x, i * cin * t, t, 1),
                 1.0, band, 0, t, 1);
        }
    });
    out
}

/// 1x1 conv on every band of `(f, cin, t)`.
fn conv1x1_freq(x: &[f32], f: usize, cin: usize, t: usize, l: &Lin, cout: usize) -> Vec<f32> {
    let mut out = vec![0f32; f * cout * t];
    out.par_chunks_mut(cout * t).enumerate().for_each(|(ff, band)| {
        fill_bias(band, &l.b, t);
        gemm(cout, cin, t, 1.0, M::new(&l.w, 0, cin, 1), M::new(x, ff * cin * t, t, 1), 1.0, band, 0, t, 1);
    });
    out
}

/// 3x3 conv (pad 1) over (freq, time) of `(f, cin, t)`; weight `(dt, cout, df, cin)`.
fn conv3x3_freq(x: &[f32], f: usize, cin: usize, t: usize, l: &Lin, cout: usize) -> Vec<f32> {
    let tp = t + 2;
    let mut xp = vec![0f32; (f + 2) * cin * tp];
    xp[cin * tp..(f + 1) * cin * tp].par_chunks_mut(tp).zip(x.par_chunks(t))
        .for_each(|(d, s)| d[1..1 + t].copy_from_slice(s));
    let mut out = vec![0f32; f * cout * t];
    out.par_chunks_mut(cout * t).enumerate().for_each(|(ff, band)| {
        fill_bias(band, &l.b, t);
        for dt in 0..3 {
            gemm(cout, 3 * cin, t, 1.0, M::new(&l.w, dt * cout * 3 * cin, 3 * cin, 1),
                 M::new(&xp, ff * cin * tp + dt, tp, 1), 1.0, band, 0, t, 1);
        }
    });
    out
}

/// DConv residual branch on one `(c, t)` block, in place.
fn dconv_block(x: &mut [f32], c: usize, t: usize, layers: &[DLayer]) {
    for d in layers {
        let (mut y, _) = conv1d(x, c, t, &d.c1, d.hidden, 3, 1, d.dil, d.dil);
        ops::group_norm1(&mut y, d.hidden, t, &d.n1.w, &d.n1.b);
        ops::gelu_all(&mut y);
        let (mut z, _) = conv1d(&y, d.hidden, t, &d.c2, 2 * c, 1, 1, 0, 1);
        ops::group_norm1(&mut z, 2 * c, t, &d.n2.w, &d.n2.b);
        let mut g = vec![0f32; c * t];
        ops::glu(&z, c, t, &mut g);
        ops::add_scaled_rows(x, &g, &d.scale, t);
    }
}

// --------------------------------------------------------- encoder/decoder

/// Frequency encoder layer: `(f, cin, t)` → `(f/4, cout, t)`.
fn enc_freq(e: &Enc, x: &[f32], f: usize, t: usize) -> (Vec<f32>, usize) {
    let (mut y, fo) = conv_freq(x, f, e.cin, t, &e.conv, e.cout);
    ops::gelu_all(&mut y);
    y.par_chunks_mut(e.cout * t).for_each(|band| dconv_block(band, e.cout, t, &e.dconv));
    let z = conv1x1_freq(&y, fo, e.cout, t, &e.rewrite, 2 * e.cout);
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
    let (mut y, to) = conv1d(x, e.cin, tt, &e.conv, e.cout, 8, 4, 2, 1);
    ops::gelu_all(&mut y);
    dconv_block(&mut y, e.cout, to, &e.dconv);
    let (z, _) = conv1d(&y, e.cout, to, &e.rewrite, 2 * e.cout, 1, 1, 0, 1);
    let mut out = vec![0f32; e.cout * to];
    ops::glu(&z, e.cout, to, &mut out);
    (out, to)
}

/// Frequency decoder layer: `x + skip` `(f, cin, t)` → `(4f, cout, t)`.
fn dec_freq(d: &Dec, x: &mut [f32], skip: &[f32], f: usize, t: usize) -> Vec<f32> {
    ops::add(x, skip);
    let z = conv3x3_freq(x, f, d.cin, t, &d.rewrite, 2 * d.cin);
    let mut y = vec![0f32; f * d.cin * t];
    y.par_chunks_mut(d.cin * t).zip(z.par_chunks(2 * d.cin * t)).for_each(|(yb, zb)| {
        ops::glu(zb, d.cin, t, yb);
        dconv_block(yb, d.cin, t, &d.dconv);
    });
    let mut out = conv_tr_freq(&y, f, d.cin, t, &d.conv_tr, d.cout);
    if !d.last { ops::gelu_all(&mut out) }
    out
}

/// Time decoder layer: `x + skip` `(cin, t)` → `(cout, len)`.
fn dec_time(d: &Dec, x: &mut [f32], skip: &[f32], t: usize, len: usize) -> Vec<f32> {
    ops::add(x, skip);
    let (z, _) = conv1d(x, d.cin, t, &d.rewrite, 2 * d.cin, 3, 1, 1, 1);
    let mut y = vec![0f32; d.cin * t];
    ops::glu(&z, d.cin, t, &mut y);
    dconv_block(&mut y, d.cin, t, &d.dconv);
    let mut out = conv_tr1d(&y, d.cin, t, &d.conv_tr, d.cout, 8, 4, 2, len);
    if !d.last { ops::gelu_all(&mut out) }
    out
}

// ------------------------------------------------------------- transformer

/// `(n, in)` → `(n, out)`: `x·Wᵀ + b`, W `(out, in)`.
fn linear(x: &[f32], n: usize, l: &Lin, din: usize, dout: usize) -> Vec<f32> {
    let mut y = vec![0f32; n * dout];
    for r in y.chunks_exact_mut(dout) { r.copy_from_slice(&l.b[..dout]) }
    pgemm(n, din, dout, 1.0, M::new(x, 0, din, 1), M::new(&l.w, 0, 1, din), 1.0, &mut y, 0, dout, 1);
    y
}

/// Queries per attention block: a block's scores are `BLOCK × m`.
const BLOCK: usize = 128;

/// Multi-head attention: queries from `q (n, DIM)`, keys/values from `kv (m, DIM)`.
/// Runs in blocks of queries (all heads each), so the scores never exceed
/// `BLOCK × m` per thread.
fn mha(a: &Attn, q: &[f32], n: usize, kv: &[f32], m: usize) -> Vec<f32> {
    let proj = |x: &[f32], rows: usize, part: usize| {
        let mut y = vec![0f32; rows * DIM];
        for r in y.chunks_exact_mut(DIM) { r.copy_from_slice(&a.in_b[part * DIM..(part + 1) * DIM]) }
        pgemm(rows, DIM, DIM, 1.0, M::new(x, 0, DIM, 1), M::new(&a.in_w, part * DIM * DIM, 1, DIM), 1.0, &mut y, 0, DIM, 1);
        y
    };
    let (qq, kk, vv) = (proj(q, n, 0), proj(kv, m, 1), proj(kv, m, 2));
    let hd = DIM / HEADS;
    let scale = 1.0 / (hd as f32).sqrt();
    let mut o = vec![0f32; n * DIM];
    o.par_chunks_mut(BLOCK * DIM).enumerate().for_each(|(blk, ob)| {
        let rows = ob.len() / DIM;
        let r0 = blk * BLOCK;
        let mut s = vec![0f32; rows * m];
        for h in 0..HEADS {
            gemm(rows, hd, m, scale, M::new(&qq, r0 * DIM + h * hd, DIM, 1), M::new(&kk, h * hd, 1, DIM), 0.0, &mut s, 0, m, 1);
            ops::softmax_rows(&mut s, m);
            gemm(rows, m, hd, 1.0, M::new(&s, 0, m, 1), M::new(&vv, h * hd, DIM, 1), 0.0, ob, h * hd, DIM, 1);
        }
    });
    linear(&o, n, &a.out, DIM, DIM)
}

/// One transformer layer (norm first, layer scale, GroupNorm out), in place.
fn tlayer(l: &Layer, x: &mut [f32], n: usize, kv: Option<(&[f32], usize)>) {
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
    let nf = if l.cross { l.norm3.as_ref().unwrap() } else { &l.norm2 };
    ops::layer_norm(x, DIM, &nf.w, &nf.b, &mut h);
    let mut f1 = linear(&h, n, &l.lin1, DIM, 4 * DIM);
    ops::gelu_all(&mut f1);
    let f2 = linear(&f1, n, &l.lin2, 4 * DIM, DIM);
    for (row, fr) in x.chunks_exact_mut(DIM).zip(f2.chunks_exact(DIM)) {
        for i in 0..DIM { row[i] += l.g2[i] * fr[i] }
    }
    ops::group_norm1_tokens(x, DIM, &l.norm_out.w, &l.norm_out.b);
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
        let mut x = vec![0f32; fq * 4 * t];
        for f in 0..fq { for c in 0..2 { for tt in 0..t {
            let v = z[c][f * t + tt];
            x[(f * 4 + 2 * c) * t + tt] = v.re;
            x[(f * 4 + 2 * c + 1) * t + tt] = v.im;
        }}}
        let (mean, std) = ops::mean_std(&x);
        for v in &mut x { *v = (*v - mean) / (1e-5 + std) }
        let (meant, stdt) = ops::mean_std(mix);
        let mut xt: Vec<f32> = mix.iter().map(|v| (v - meant) / (1e-5 + stdt)).collect();

        // Encoders.
        let (mut f, mut tl) = (fq, len);
        let (mut saved, mut saved_t, mut lens_t) = (vec![], vec![], vec![]);
        for i in 0..DEPTH {
            lens_t.push(tl);
            let (y, to) = enc_time(&self.tenc[i], &xt, tl);
            xt = y; tl = to;
            emit(&format!("tenc{i}"), &[self.tenc[i].cout, tl], &|| xt.clone());
            saved_t.push(xt.clone());
            let (y, fo) = enc_freq(&self.enc[i], &x, f, t);
            x = y; f = fo;
            let c = self.enc[i].cout;
            emit(&format!("enc{i}"), &[c, f, t], &|| fct_to_cft(&x, f, c, t));
            if i == 0 {
                let c = self.enc[0].cout;
                for ff in 0..f { for cc in 0..c {
                    let e = 0.2 * 10.0 * self.freq_emb[ff * c + cc];
                    for v in &mut x[(ff * c + cc) * t..(ff * c + cc + 1) * t] { *v += e }
                }}
            }
            saved.push(x.clone());
        }

        // Transformer: tokens (t*f + f_idx, DIM) for the spectrogram branch.
        let (n, m) = (f * t, tl);
        let mut tok = vec![0f32; n * DIM];
        for ff in 0..f {
            pgemm(t, 384, DIM, 1.0, M::new(&x, ff * 384 * t, 1, t), M::new(&self.up.w, 0, 1, 384),
                 0.0, &mut tok, ff * DIM, f * DIM, 1);
        }
        for r in tok.chunks_exact_mut(DIM) { for i in 0..DIM { r[i] += self.up.b[i] } }
        let mut tok_t = vec![0f32; m * DIM];
        for r in tok_t.chunks_exact_mut(DIM) { r.copy_from_slice(&self.up_t.b) }
        pgemm(m, 384, DIM, 1.0, M::new(&xt, 0, 1, m), M::new(&self.up_t.w, 0, 1, 384), 1.0, &mut tok_t, 0, DIM, 1);
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
        for ff in 0..f {
            for co in 0..384 { x[(ff * 384 + co) * t..(ff * 384 + co + 1) * t].fill(self.down.b[co]) }
            pgemm(384, DIM, t, 1.0, M::new(&self.down.w, 0, DIM, 1), M::new(&tok, ff * DIM, 1, f * DIM),
                 1.0, &mut x, ff * 384 * t, t, 1);
        }
        let mut xt = vec![0f32; 384 * m];
        for co in 0..384 { xt[co * m..(co + 1) * m].fill(self.down_t.b[co]) }
        pgemm(384, DIM, m, 1.0, M::new(&self.down_t.w, 0, DIM, 1), M::new(&tok_t, 0, 1, DIM), 1.0, &mut xt, 0, m, 1);
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
}
