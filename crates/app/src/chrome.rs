//! Procedurally shaded faceplate chrome — the jog wheel, transport buttons and
//! knobs — rendered once per pixel size into RGBA sprites.
//!
//! Every control is a **height field** over a disc (or a rounded square), shaded
//! with one shared light rig: a key light up-left, a weak fill down-right, a
//! hemisphere ambient, and a soft "ceiling panel" the glossy parts reflect.  One
//! rig for every control is what makes the panel read as a single object under a
//! single light, instead of a collage of photographs with three different suns.
//!
//! The sprites are baked on the CPU (a few hundred ms for the jog at iPad
//! resolution, spread over all cores) and cached by size in [`ChromeCache`], so
//! the per-frame cost is one textured quad each.  Nothing here is a photograph:
//! the geometry is measured off the XDJ-1000MK2 reference but every pixel is
//! computed, so the app ships no Pioneer imagery.

use egui::{ColorImage, Color32, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::f32::consts::TAU;

// ── Shading ──────────────────────────────────────────────────────────────────

/// A surface sample: what the height field reports at one pixel.
#[derive(Clone, Copy)]
struct Surf {
    /// Height in "radius units" (the control's outer radius = 1.0), so the same
    /// profile scales to any pixel size.
    h:      f32,
    /// Linear-light albedo.
    albedo: [f32; 3],
    /// 0 = mirror, 1 = fully matte.
    rough:  f32,
    /// Reflectance at normal incidence (0.04 plastic … 0.9 polished metal).
    f0:     f32,
    /// Light emitted by the surface itself (lit buttons), linear.
    emit:   [f32; 3],
    /// Coverage 0..1: fractional at the outer edge for antialiasing, 0 outside.
    cover:  f32,
}

impl Surf {
    const NONE: Surf = Surf { h: 0.0, albedo: [0.0; 3], rough: 1.0, f0: 0.0, emit: [0.0; 3], cover: 0.0 };
}

/// Fixed light rig, image space: +x right, +y DOWN (so "up" is −y), +z toward
/// the viewer.  Tuned so a flat matte surface reads mid-dark, a glossy black
/// surface shows one broad highlight up-left, and metal picks up a bright band.
struct Rig {
    key:  [f32; 3], key_col:  [f32; 3],
    fill: [f32; 3], fill_col: [f32; 3],
    /// Direction of the soft ceiling panel the gloss reflects.
    panel: [f32; 3],
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }

const RIG: Rig = Rig {
    key:  [-0.45, -0.62, 0.64], key_col:  [1.05, 1.02, 0.98],
    fill: [ 0.55,  0.45, 0.70], fill_col: [0.16, 0.17, 0.20],
    panel: [-0.22, -0.40, 0.89],
};

/// What the environment looks like in direction `r` (a reflection vector):
/// a soft bright panel up-left-front, a dim sky above, near-black below.
fn env(r: [f32; 3], rough: f32) -> f32 {
    let up = -r[1];
    let base = 0.025 + 0.06 * (up * 0.5 + 0.5);
    // The panel: a lobe that widens as roughness rises (blurred reflection).
    let p = dot(r, norm(RIG.panel)).max(0.0);
    let sharp = 2.0 + 40.0 * (1.0 - rough) * (1.0 - rough);
    base + 1.15 * p.powf(sharp)
}

fn schlick(f0: f32, cos: f32) -> f32 {
    f0 + (1.0 - f0) * (1.0 - cos).clamp(0.0, 1.0).powi(5)
}

/// Shade one sample given its unit normal.  Returns linear RGB.
fn shade(s: &Surf, n: [f32; 3], ao: f32) -> [f32; 3] {
    let v = [0.0, 0.0, 1.0];
    let key = norm(RIG.key);
    let fill = norm(RIG.fill);
    let nv = dot(n, v).max(0.0);

    // Diffuse: key + fill + hemisphere ambient.
    let up = (-n[1]) * 0.5 + 0.5;
    let amb = 0.06 + 0.16 * up;
    let dk = dot(n, key).max(0.0);
    let df = dot(n, fill).max(0.0);
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        out[i] = s.albedo[i] * (dk * RIG.key_col[i] + df * RIG.fill_col[i] + amb) * ao;
    }

    // Specular: Blinn-Phong from the key, plus a reflected-environment term
    // through a Schlick Fresnel.  Metal (high f0) tints its reflections.
    let h = norm([key[0] + v[0], key[1] + v[1], key[2] + v[2]]);
    let shin = 4.0 + 600.0 * (1.0 - s.rough).powi(3);
    let spec = dot(n, h).max(0.0).powf(shin) * (1.0 - s.rough * 0.6);
    let r = [2.0 * n[2] * n[0], 2.0 * n[2] * n[1], 2.0 * n[2] * n[2] - 1.0];
    let refl = env(r, s.rough);
    let f = schlick(s.f0, nv);
    let metal = (s.f0 - 0.04) / 0.86;   // 0 plastic … 1 metal
    for i in 0..3 {
        let tint = 1.0 + metal.max(0.0) * (s.albedo[i] / s.albedo.iter().cloned().fold(1e-3, f32::max) - 1.0);
        out[i] += (spec * f * 1.1 + refl * f * (1.0 - 0.45 * metal.max(0.0))) * tint * ao.sqrt();
        out[i] += s.emit[i];
    }
    out
}

fn to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Cheap deterministic hash noise in 0..1 for brushed / matte texture.
fn hash(x: f32, y: f32) -> f32 {
    let mut h = (x * 127.1 + y * 311.7).sin() * 43758.547;
    h = h.fract();
    if h < 0.0 { h + 1.0 } else { h }
}
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Render a height field to an RGBA sprite of `w`×`h` pixels.  `field(x, y)`
/// takes pixel-centred coordinates (0,0 = sprite centre), and `scale` is the
/// pixel length of one radius unit.  `shadow` is a soft drop shadow radius in
/// radius units (0 = none); it is written into the alpha outside the shape.
fn bake(w: usize, h: usize, scale: f32, shadow: f32, field: &(dyn Fn(f32, f32) -> Surf + Sync)) -> ColorImage {
    let mut px = vec![0u8; w * h * 4];
    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.5;
    let eps = 0.75;                       // finite-difference step, pixels
    let rows = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
    let chunk = (h + rows - 1) / rows;
    std::thread::scope(|sc| {
        for (i, band) in px.chunks_mut(chunk * w * 4).enumerate() {
            let y0 = i * chunk;
            sc.spawn(move || {
                for (ry, row) in band.chunks_mut(w * 4).enumerate() {
                    let y = (y0 + ry) as f32 + 0.5 - cy;
                    for (x_i, p) in row.chunks_mut(4).enumerate() {
                        let x = x_i as f32 + 0.5 - cx;
                        let s = field(x, y);
                        if s.cover <= 0.0 {
                            // Drop shadow: light is up-left, so it falls down-right.
                            if shadow > 0.0 {
                                let sx = x - 0.018 * scale;
                                let sy = y - 0.030 * scale;
                                let d = (sx * sx + sy * sy).sqrt() / scale - 1.0;
                                let a = (1.0 - smooth(-0.01, shadow, d)) * 0.55;
                                p[3] = (a * 255.0) as u8;
                            }
                            continue;
                        }
                        // Normal from the height field (heights are in radius
                        // units, so convert the gradient to pixel slope).
                        let hx = (field(x + eps, y).h - field(x - eps, y).h) * scale / (2.0 * eps);
                        let hy = (field(x, y + eps).h - field(x, y - eps).h) * scale / (2.0 * eps);
                        let n = norm([-hx, -hy, 1.0]);
                        // Crude ambient occlusion: steep slopes / recesses darken.
                        let ao = 0.55 + 0.45 * n[2];
                        let c = shade(&s, n, ao);
                        p[0] = to_srgb(c[0]);
                        p[1] = to_srgb(c[1]);
                        p[2] = to_srgb(c[2]);
                        p[3] = (s.cover * 255.0 + 0.5) as u8;
                        if s.cover < 1.0 && shadow > 0.0 {
                            // Blend the edge pixel over the shadow so the rim
                            // antialiases against it, not against transparency.
                            let sx = x - 0.018 * scale;
                            let sy = y - 0.030 * scale;
                            let d = (sx * sx + sy * sy).sqrt() / scale - 1.0;
                            let a = (1.0 - smooth(-0.01, shadow, d)) * 0.55;
                            let tot = s.cover + a * (1.0 - s.cover);
                            let k = s.cover / tot.max(1e-3);
                            for ch in 0..3 { p[ch] = (p[ch] as f32 * k) as u8; }
                            p[3] = (tot * 255.0) as u8;
                        }
                    }
                }
            });
        }
    });
    ColorImage::from_rgba_unmultiplied([w, h], &px)
}

// ── Jog wheel ────────────────────────────────────────────────────────────────

/// Margin around the platter for the drop shadow, as a fraction of the radius.
pub const JOG_MARGIN: f32 = 0.07;
/// Radius of the centre recess (where the position display lives), fraction of
/// the outer radius.  `screen.rs` draws the spinning display inside this.
pub const JOG_RECESS_R: f32 = 0.345;

/// The XDJ-1000MK2 jog seen from above: a matte dimpled grip rim, a silver
/// bezel ring, a glossy black platter and a stepped-down centre recess.
fn jog_field(x: f32, y: f32, scale: f32) -> Surf {
    let r = (x * x + y * y).sqrt() / scale;
    let aa = 1.0 / scale;                                     // one pixel in r units
    if r > 1.0 + aa { return Surf::NONE; }
    let cover = smooth(1.0 + aa * 0.5, 1.0 - aa * 0.5, r);
    let th = y.atan2(x);

    // Radial zones (fractions of the outer radius), measured off the platter photo.
    const RIM_IN:  f32 = 0.800;   // grip rim inner edge / bezel outer edge
    const BEZ_IN:  f32 = 0.778;   // bezel inner edge / platter outer edge
    const REC:     f32 = JOG_RECESS_R;

    let (mut h, albedo, rough, f0);
    if r >= RIM_IN {
        // Grip rim: flat top that rolls off over the outer 6% into the edge,
        // with 32 concave oval dimples set into it.
        let t = ((r - 0.94) / 0.06).clamp(0.0, 1.0);
        h = 0.060 - 0.075 * (1.0 - (1.0 - t * t).sqrt());
        // A small fillet stepping down onto the bezel.
        h -= 0.012 * (1.0 - smooth(RIM_IN, RIM_IN + 0.012, r));
        const N: f32 = 32.0;
        let k = (th * N / TAU).round();
        let dth = th - k * TAU / N;                         // angular offset to nearest dimple
        let tang = dth * 0.885;                             // arc length at the dimple ring
        let rad  = r - 0.885;
        let q = (tang / 0.070).powi(2) + (rad / 0.046).powi(2);
        if q < 1.0 {
            let d = (1.0 - q).powi(2);
            h -= 0.040 * d;
        }
        // Matte black plastic with a fine speckle.
        let sp = hash(x * 0.7, y * 0.7) * 0.004;
        albedo = [0.020 + sp, 0.020 + sp, 0.021 + sp];
        rough = 0.78;
        f0 = 0.04;
    } else if r >= BEZ_IN {
        // Silver bezel: a raised half-round ring.
        let m = (r - (RIM_IN + BEZ_IN) * 0.5) / ((RIM_IN - BEZ_IN) * 0.5);
        h = 0.046 + 0.016 * (1.0 - m * m).max(0.0).sqrt();
        let brush = (hash(th * 900.0, 0.0) - 0.5) * 0.06;
        albedo = [0.50 + brush, 0.51 + brush, 0.53 + brush];
        rough = 0.28;
        f0 = 0.80;
    } else if r >= REC {
        // Glossy black platter: an almost-flat shallow dome (so the ceiling
        // panel reads as one broad soft highlight up-left) with faint circular
        // grooves and radial brushing.
        let u = (r - REC) / (BEZ_IN - REC);
        h = 0.040 + 0.030 * (1.0 - u * u);
        // Cove down into the recess at the inner edge.
        h -= 0.014 * (1.0 - smooth(REC, REC + 0.02, r));
        let brush = (hash(th * 1400.0, 1.0) - 0.5) * 0.004;
        albedo = [0.010 + brush, 0.010 + brush, 0.011 + brush];
        rough = 0.24 + hash(th * 700.0, 2.0) * 0.05;
        f0 = 0.05;
    } else {
        // Recess floor: matte, very dark, a hair of red like the real unit's
        // display window.
        h = 0.020;
        albedo = [0.009, 0.007, 0.007];
        rough = 0.55;
        f0 = 0.04;
    }
    Surf { h, albedo, rough, f0, emit: [0.0; 3], cover }
}

/// Bake the jog at `diam_px` pixels across the platter.  The sprite is larger
/// by `JOG_MARGIN` each side for the drop shadow.
pub fn render_jog(diam_px: usize) -> ColorImage {
    let scale = diam_px as f32 * 0.5;
    let w = (diam_px as f32 * (1.0 + 2.0 * JOG_MARGIN)).ceil() as usize;
    bake(w, w, scale, JOG_MARGIN * 0.9, &move |x, y| jog_field(x, y, scale))
}

// ── Round buttons ────────────────────────────────────────────────────────────

/// Button look.  The silver transport buttons (CUE, PLAY/PAUSE), the small
/// black RELOOP, the orange MASTER TEMPO lamp, and the knurled BROWSE rotary.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RoundKind { Silver, Black, Lamp, Knob }

/// Margin for the glow / shadow around a round button, fraction of its radius.
pub const BTN_MARGIN: f32 = 0.22;

fn round_field(x: f32, y: f32, scale: f32, kind: RoundKind, lit: Option<[f32; 3]>) -> Surf {
    let r = (x * x + y * y).sqrt() / scale;
    let aa = 1.0 / scale;
    if r > 1.0 + aa { return Surf::NONE; }
    let cover = smooth(1.0 + aa * 0.5, 1.0 - aa * 0.5, r);
    let th = y.atan2(x);
    let mut emit = [0.0; 3];
    let (h, albedo, rough, f0);
    match kind {
        RoundKind::Silver => {
            // Dark bezel collar sloping down and out; silver cap gently domed.
            const CAP: f32 = 0.86;
            if r >= CAP {
                let t = (r - CAP) / (1.0 - CAP);
                h = 0.10 - 0.16 * t * t;
                albedo = [0.030, 0.030, 0.032]; rough = 0.6; f0 = 0.04;
            } else {
                let u = r / CAP;
                h = 0.12 + 0.07 * (1.0 - u.powi(4)).max(0.0);
                let brush = (hash(th * 500.0, 3.0) - 0.5) * 0.05;
                let base = 0.40 + brush;
                albedo = [base, base + 0.004, base + 0.010];
                rough = 0.34; f0 = 0.70;
                if let Some(c) = lit {
                    // Backlit: the cap glows from within, strongest toward the rim.
                    // A translucent cap over the lamp: an even wash, and a
                    // bright ring where the light escapes at the cap's edge.
                    let g = 0.18 + 0.55 * smooth(0.82, 0.99, u);
                    emit = [c[0] * g, c[1] * g, c[2] * g];
                }
            }
        }
        RoundKind::Black => {
            const CAP: f32 = 0.80;
            if r >= CAP {
                let t = (r - CAP) / (1.0 - CAP);
                h = 0.08 - 0.12 * t * t;
                albedo = [0.025, 0.025, 0.027]; rough = 0.55; f0 = 0.04;
            } else {
                let u = r / CAP;
                h = 0.10 + 0.05 * (1.0 - u.powi(3));
                albedo = [0.040, 0.040, 0.042]; rough = 0.45; f0 = 0.05;
            }
        }
        RoundKind::Lamp => {
            // A small translucent dome; lit = amber from within.
            let u = r.min(1.0);
            h = 0.30 * (1.0 - u * u).max(0.0).sqrt();
            let c = lit.unwrap_or([0.10, 0.045, 0.012]);
            albedo = [c[0] * 0.5, c[1] * 0.5, c[2] * 0.5];
            rough = 0.18; f0 = 0.05;
            if lit.is_some() {
                let g = 0.55 + 0.45 * (1.0 - u * u);
                emit = [c[0] * g, c[1] * g, c[2] * g];
            }
        }
        RoundKind::Knob => {
            // Rotary encoder seen from above: knurled skirt, brushed flat top.
            const TOP: f32 = 0.70;
            if r >= TOP {
                const N: f32 = 40.0;
                let k = (th * N / TAU).round();
                let dth = (th - k * TAU / N) * 0.85;
                let ridge = (1.0 - (dth / (0.5 * TAU / N * 0.85)).abs()).max(0.0);
                let t = (r - TOP) / (1.0 - TOP);
                h = 0.16 - 0.10 * t * t + 0.02 * ridge;
                albedo = [0.30, 0.31, 0.33]; rough = 0.45; f0 = 0.6;
            } else {
                let brush = (hash(th * 800.0, 4.0) - 0.5) * 0.06;
                h = 0.18 + 0.01 * (1.0 - (r / TOP).powi(2));
                let base = 0.40 + brush;
                albedo = [base, base + 0.005, base + 0.01]; rough = 0.32; f0 = 0.70;
            }
        }
    }
    Surf { h, albedo, rough, f0, emit, cover }
}

/// Bake a round button `diam_px` across; `lit` is the lamp colour (linear RGB)
/// for the illuminated variant.  Sprite includes `BTN_MARGIN` for the shadow /
/// halo, which is painted into the alpha outside the shape.
pub fn render_round(diam_px: usize, kind: RoundKind, lit: Option<[f32; 3]>) -> ColorImage {
    let scale = diam_px as f32 * 0.5;
    let w = (diam_px as f32 * (1.0 + 2.0 * BTN_MARGIN)).ceil() as usize;
    let mut img = bake(w, w, scale, 0.08, &move |x, y| round_field(x, y, scale, kind, lit));
    if let Some(c) = lit {
        // Halo: light spilling onto the panel around a lit button.
        let cx = w as f32 * 0.5;
        let col = Color32::from_rgb(to_srgb(c[0] * 0.9), to_srgb(c[1] * 0.9), to_srgb(c[2] * 0.9));
        for y in 0..w {
            for x in 0..w {
                let p = &mut img.pixels[y * w + x];
                if p.a() > 200 { continue; }
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cx).powi(2)).sqrt() / scale - 1.0;
                let a = (1.0 - smooth(0.0, BTN_MARGIN, d)) * 0.7;
                if a <= 0.0 { continue; }
                // Composite the halo under whatever shadow/edge is there.
                let existing = p.a() as f32 / 255.0;
                let tot = existing + a * (1.0 - existing);
                let k = existing / tot.max(1e-3);
                let mix = |e: u8, h: u8| (e as f32 * k + h as f32 * (1.0 - k)) as u8;
                *p = Color32::from_rgba_unmultiplied(mix(p.r(), col.r()), mix(p.g(), col.g()), mix(p.b(), col.b()), (tot * 255.0) as u8);
            }
        }
    }
    img
}

// ── Rectangular (loop) buttons ───────────────────────────────────────────────

/// Width ÷ height of the LOOP IN / OUT buttons.
pub const LOOP_ASPECT: f32 = 1.25;

/// A rounded-rectangle domed button (LOOP IN / OUT), `aspect` = width ÷ height;
/// `face` is the linear colour of the translucent cap, lit or not.
fn square_field(x: f32, y: f32, scale: f32, aspect: f32, face: [f32; 3], lit: bool) -> Surf {
    // Superellipse cap inside a dark collar.
    let (u, v) = (x / (scale * aspect), y / scale);
    let se = |a: f32, b: f32, p: f32| (a.abs().powf(p) + b.abs().powf(p)).powf(1.0 / p);
    let d_out = se(u, v, 5.0);
    let aa = 1.0 / scale;
    if d_out > 1.0 + aa { return Surf::NONE; }
    let cover = smooth(1.0 + aa * 0.5, 1.0 - aa * 0.5, d_out);
    const CAP: f32 = 0.80;
    let d_cap = se(u, v, 4.0) / CAP;
    let mut emit = [0.0; 3];
    let (h, albedo, rough, f0);
    if d_cap >= 1.0 {
        let t = ((d_out - CAP) / (1.0 - CAP)).clamp(0.0, 1.0);
        h = 0.10 - 0.14 * t * t;
        albedo = [0.030, 0.030, 0.032]; rough = 0.6; f0 = 0.04;
    } else {
        h = 0.12 + 0.10 * (1.0 - d_cap.powi(4));
        albedo = [face[0] * 0.55, face[1] * 0.55, face[2] * 0.55];
        rough = 0.22; f0 = 0.05;
        if lit {
            let g = 0.5 + 0.5 * (1.0 - d_cap * d_cap);
            emit = [face[0] * g, face[1] * g, face[2] * g];
        }
    }
    Surf { h, albedo, rough, f0, emit, cover }
}

/// Bake a rectangular button `h_px` tall and `h_px * aspect` wide.
pub fn render_square(h_px: usize, aspect: f32, face: [f32; 3], lit: bool) -> ColorImage {
    let scale = h_px as f32 * 0.5;
    let h = (h_px as f32 * (1.0 + 2.0 * BTN_MARGIN)).ceil() as usize;
    let w = (h_px as f32 * (aspect + 2.0 * BTN_MARGIN)).ceil() as usize;
    bake(w, h, scale, 0.08, &move |x, y| square_field(x, y, scale, aspect, face, lit))
}

// ── Fader knob ───────────────────────────────────────────────────────────────

/// Width ÷ height of the tempo-fader knob.
pub const KNOB_ASPECT: f32 = 1.65;

/// The pitch-fader handle: a silver rounded-rectangle cap on a dark bevel,
/// with a shallow groove across the middle for the printed centre line.
fn fader_knob_field(x: f32, y: f32, scale: f32) -> Surf {
    let (u, v) = (x / (scale * KNOB_ASPECT), y / scale);
    let se = |a: f32, b: f32, p: f32| (a.abs().powf(p) + b.abs().powf(p)).powf(1.0 / p);
    let d_out = se(u, v, 6.0);
    let aa = 1.0 / scale;
    if d_out > 1.0 + aa { return Surf::NONE; }
    let cover = smooth(1.0 + aa * 0.5, 1.0 - aa * 0.5, d_out);
    const CAP: f32 = 0.84;
    let d_cap = se(u / CAP, v / CAP, 6.0);
    let (mut h, albedo, rough, f0);
    if d_cap >= 1.0 {
        let t = ((d_out - CAP) / (1.0 - CAP)).clamp(0.0, 1.0);
        h = 0.10 - 0.16 * t * t;
        albedo = [0.030, 0.030, 0.032]; rough = 0.6; f0 = 0.04;
    } else {
        h = 0.12 + 0.05 * (1.0 - d_cap.powi(4));
        // Centre groove, the full width of the cap.
        h -= 0.03 * (1.0 - smooth(0.0, 0.10, v.abs()));
        let brush = (hash(0.0, y * 0.9) - 0.5) * 0.05;   // brushed across
        let base = 0.42 + brush;
        albedo = [base, base + 0.004, base + 0.010]; rough = 0.34; f0 = 0.70;
    }
    Surf { h, albedo, rough, f0, emit: [0.0; 3], cover }
}

pub fn render_fader_knob(h_px: usize) -> ColorImage {
    let scale = h_px as f32 * 0.5;
    let h = (h_px as f32 * (1.0 + 2.0 * BTN_MARGIN)).ceil() as usize;
    let w = (h_px as f32 * (KNOB_ASPECT + 2.0 * BTN_MARGIN)).ceil() as usize;
    bake(w, h, scale, 0.10, &move |x, y| fader_knob_field(x, y, scale))
}

// ── Cache ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Sprite {
    Jog,
    Round(RoundKind, /* lit */ Option<Lamp>),
    Square(/* lit */ bool),
    FaderKnob,
}

/// Lamp colours for lit variants (kept as an enum so the key is hashable).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Lamp { Green, Orange, White }

impl Lamp {
    fn linear(self) -> [f32; 3] {
        match self {
            Lamp::Green  => [0.10, 0.95, 0.25],
            Lamp::Orange => [1.00, 0.42, 0.06],
            Lamp::White  => [0.85, 0.87, 0.90],
        }
    }
}

/// Sprites baked so far, keyed by (what, pixel size).  Sizes only change on a
/// window resize, so this holds a handful of textures.
#[derive(Default)]
pub struct ChromeCache {
    tex: HashMap<(Sprite, usize), TextureHandle>,
}

impl ChromeCache {
    /// The sprite for `what` at `px` pixels across the control (not counting
    /// its margin), baking it on first use.
    pub fn get(&mut self, ctx: &egui::Context, what: Sprite, px: usize) -> &TextureHandle {
        let px = px.max(8);
        self.tex.entry((what, px)).or_insert_with(|| {
            let t = std::time::Instant::now();
            let img = match what {
                Sprite::Jog => render_jog(px),
                Sprite::Round(kind, lit) => render_round(px, kind, lit.map(Lamp::linear)),
                Sprite::Square(lit) => render_square(px, LOOP_ASPECT, [1.0, 0.62, 0.10], lit),
                Sprite::FaderKnob => render_fader_knob(px),
            };
            log::info!("chrome: baked {what:?} @ {px}px in {:.0} ms", t.elapsed().as_secs_f32() * 1e3);
            ctx.load_texture(format!("chrome-{what:?}-{px}"), img, TextureOptions::LINEAR)
        })
    }

    /// The margin a sprite carries around the control, as a fraction of its size.
    pub fn margin(what: Sprite) -> f32 {
        match what { Sprite::Jog => JOG_MARGIN, _ => BTN_MARGIN }
    }
}

/// Paint `what` centred on `r`, sized from `r`'s height (the sprite's own
/// aspect wins over `r`'s width, and its margin extends beyond `r`).
pub fn paint(p: &egui::Painter, ctx: &egui::Context, cache: &mut ChromeCache, what: Sprite, r: egui::Rect) {
    let ppp = ctx.pixels_per_point();
    let px = (r.height() * ppp).round() as usize;
    let tex = cache.get(ctx, what, px);
    let size = tex.size_vec2() / ppp;
    let dst = egui::Rect::from_center_size(r.center(), size);
    p.image(tex.id(), dst, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)), Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jog_bakes_with_transparent_corners_and_opaque_centre() {
        let img = render_jog(200);
        let w = img.size[0];
        assert_eq!(img.pixels[0].a(), 0, "corner should be transparent");
        assert_eq!(img.pixels[(w / 2) * w + w / 2].a(), 255, "centre should be opaque");
    }

    #[test]
    fn lit_button_is_brighter_than_unlit() {
        let off = render_round(120, RoundKind::Silver, None);
        let on  = render_round(120, RoundKind::Silver, Some(Lamp::Green.linear()));
        let w = off.size[0];
        let c = (w / 2) * w + w / 2;
        assert!(on.pixels[c].g() > off.pixels[c].g());
    }
}
