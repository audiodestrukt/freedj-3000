#![cfg_attr(not(feature = "renderer-sdl"), allow(dead_code))]
//! CPU port of the waveform shader (`renderer::WAVEFORM_WGSL`), for render
//! backends without a GPU the shader can run on.  Same `WaveformParams`, same
//! packed columns, same per-pixel decisions in the same order, so the SDL
//! backend's waveforms match the wgpu ones to the pixel (the parity check in
//! `tools/render-parity.sh` holds it to that).
//!
//! The shader is per-fragment; here each pixel column is classified once
//! (playhead / marker / bar + background) and the rows are filled from that,
//! which is where the CPU wins: no per-pixel rect tests, no per-pixel column
//! lookup.  Colours are authored in sRGB bytes, which the shader converts to
//! linear for an sRGB surface and the display converts straight back — so the
//! bytes here go through the same pow(2.2) → sRGB-encode round trip the GPU
//! path makes, to land on the same values.  The round trip is a lookup table
//! and the palette is resolved once per call: on a 1 GHz ARM11 this loop is
//! the frame's biggest cost, so the per-pixel path is a few loads and stores.

use crate::renderer::WaveformParams;

/// Which of the two rects to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Wave, Overview }

/// An integer pixel rect, the rounding the backend applies to the float rect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PxRect { pub x: i32, pub y: i32, pub w: u32, pub h: u32 }

impl PxRect {
    pub fn from_f32(r: [f32; 4]) -> Option<Self> {
        if !(r[2] > 0.5 && r[3] > 0.5) || !r.iter().all(|v| v.is_finite()) { return None; }
        let x0 = r[0].round() as i32;
        let y0 = r[1].round() as i32;
        let x1 = (r[0] + r[2]).round() as i32;
        let y1 = (r[1] + r[3]).round() as i32;
        if x1 <= x0 || y1 <= y0 { return None; }
        Some(Self { x: x0, y: y0, w: (x1 - x0) as u32, h: (y1 - y0) as u32 })
    }
}

type Rgba = [u8; 4];

/// sRGB byte → linear → sRGB-encoded byte: what an authored byte becomes after
/// the shader's pow(2.2) and the sRGB surface's encode.
fn round_trip(v: f32) -> u8 {
    let lin = (v / 255.0).clamp(0.0, 1.0).powf(2.2);
    (srgb_encode(lin) * 255.0 + 0.5) as u8
}

fn srgb_encode(lin: f32) -> f32 {
    if lin <= 0.003_130_8 { lin * 12.92 } else { 1.055 * lin.powf(1.0 / 2.4) - 0.055 }
}

fn srgb_decode(e: f32) -> f32 {
    if e <= 0.040_45 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) }
}

/// The round trip for author values 0..255 at 1/4-level steps.
pub struct Lut([u8; 1024]);

impl Lut {
    pub fn new() -> Self {
        let mut t = [0u8; 1024];
        for (i, v) in t.iter_mut().enumerate() { *v = round_trip(i as f32 * 255.0 / 1023.0); }
        Lut(t)
    }
    /// `v` in author units 0..255.
    #[inline(always)]
    fn at(&self, v: f32) -> u8 { self.0[(v.clamp(0.0, 255.0) * (1023.0 / 255.0) + 0.5) as usize] }
    #[inline(always)]
    fn rgb(&self, r: f32, g: f32, b: f32) -> Rgba { [self.at(r), self.at(g), self.at(b), 255] }
}

impl Default for Lut { fn default() -> Self { Self::new() } }

/// The shader's palette, resolved through the LUT once per draw call.
struct Palette {
    ground: Rgba, playhead: Rgba, cue: Rgba, loop_line: Rgba, loop_bg: Rgba, loop_bg_cdj: Rgba, slip: Rgba,
    white: Rgba, silence: Rgba, band_low: Rgba, band_mid: Rgba, cyan: Rgba, strip_bg: Rgba, tick_hi: Rgba, down_hi: Rgba,
    /// LIGHT MODE: pixels leave through `theme::flip_rgb`, bar edges blend.
    light: bool,
}

impl Palette {
    fn new(l: &Lut, p: &WaveformParams) -> Self {
        Self {
            light:     p.light > 0.5,
            ground:    l.rgb(3.0, 4.0, 6.0),          // matches screen.rs BG
            playhead:  l.rgb(232.0, 40.0, 40.0),      // red on the unit
            cue:       l.rgb(240.0, 138.0, 30.0),     // orange start-cue marker
            loop_line: l.rgb(250.0, 200.0, 40.0),     // amber loop in/out lines
            loop_bg:   l.rgb(16.0, 40.0, 66.0),       // tinted field across the looped span
            loop_bg_cdj: l.rgb(72.0, 52.0, 10.0),     // CDJ-3000 look: amber shade
            slip:      l.rgb(200.0, 110.0, 255.0),    // SLIP shadow playhead (violet)
            white:     l.rgb(244.0, 246.0, 248.0),
            silence:   l.rgb(128.0, 128.0, 128.0),    // stem mix at silence (OverCue's line)
            band_low:  l.rgb(58.0, 123.0, 240.0),     // 3-band blue
            band_mid:  l.rgb(240.0, 160.0, 48.0),     // 3-band amber
            cyan:      l.rgb(0.0, 216.0, 216.0),
            strip_bg:  l.rgb(0.0, 22.0, 40.0),
            tick_hi:   l.rgb(230.0, 232.0, 236.0),
            down_hi:   l.rgb(255.0, 60.0, 60.0),
        }
    }
}

const B2_STRIP_PX: f32 = 22.0;

#[inline(always)]
fn unpack(v: u32) -> [f32; 4] {
    [(v & 0xFF) as f32 / 255.0, ((v >> 8) & 0xFF) as f32 / 255.0,
     ((v >> 16) & 0xFF) as f32 / 255.0, ((v >> 24) & 0xFF) as f32 / 255.0]
}

#[inline(always)]
fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// RGB as a CDJ-3000 draws it, the shader's `rgb_bar_color` (fitted to
/// cdj3k-emu's screen against the track's PWV5, 2026-11-24): colours are
/// rekordbox's RGB levels (the track's own, or our bands through
/// `opendeck_mediaserver::rgb_levels`) normalised to a fixed brightest
/// channel. Over the pixel and the next one, the outer layer is the tallest
/// column's height in its colour at 109; the core is their mean height in
/// the colour of the tallest column of the pixel's own span, at 224.
/// Colours in author units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RgbBar { pub core: f32, pub core_rgb: [f32; 3], pub outer: f32, pub outer_rgb: [f32; 3] }

/// `v` scaled so its brightest channel is `top` (white for a silent column).
pub fn rgb_norm(v: [f32; 3], top: f32) -> [f32; 3] {
    let m = v[0].max(v[1]).max(v[2]);
    if m <= 0.0 { return [top; 3]; }
    [v[0] / m * top, v[1] / m * top, v[2] / m * top]
}

/// A track's RGB waveforms from rekordbox (its `.EXT`): the detail (PWV5:
/// red, green, blue 0-7 and height 0-31, 150 a second) and the preview
/// (PWV4: 1,200 columns of 6 bytes). Either may be empty.
#[derive(Debug, Default, PartialEq)]
pub struct RbWave { pub detail: Vec<[u8; 4]>, pub preview: Vec<[u8; 6]> }

/// Columns in an RGB preview (PWV4), the overall waveform's data.
pub const PREVIEW_N: usize = 1200;

/// The overall waveform's RGB data, `PREVIEW_N` packed words of the
/// preview's bytes 3, 4, 5 (what the CDJ draws from): rekordbox's when the
/// track came with a PWV4, else made once from our columns the way we make
/// the PWV4 we serve (`opendeck_mediaserver::rgb_preview`).
pub fn preview_for(rb: Option<&RbWave>, cols: &[u32], hop: usize, sr: u32) -> Vec<u32> {
    let pack = |e: &[u8]| u32::from_le_bytes([e[3], e[4], e[5], 0]);
    if let Some(p) = rb.map(|r| &r.preview).filter(|p| !p.is_empty()) {
        return (0..PREVIEW_N).map(|i| pack(&p[(i * p.len() / PREVIEW_N).min(p.len() - 1)])).collect();
    }
    let wave = opendeck_analysis::WaveformCache { columns: cols.iter().map(|v| v.to_le_bytes()).collect(), sample_rate: sr, hop_size: hop };
    let bytes = opendeck_mediaserver::rgb_preview(&wave, (cols.len() * hop) as u64, sr);
    if bytes.len() < PREVIEW_N * 6 { return vec![0; PREVIEW_N]; }
    bytes.chunks_exact(6).take(PREVIEW_N).map(pack).collect()
}

/// The CDJ-3000's RGB overall waveform at a pixel, from `preview_for`'s
/// words (fitted on cdj3k-emu against the track's PWV4, 2026-11-24): from
/// the bottom up, to max(b3, b4, b5) (r = 0.995; 127 = 93 % of the strip),
/// coloured by (b3, b4, b5) normalised (within 1/255), brightest at the
/// bottom and darker up: below b5's height (r = 0.99) from 240 to 171 of
/// 255, above it from 244 to 117, by the fraction of the column's height.
/// `sx` = where across (0-1), `w` = the strip's width in pixels, `y` = how
/// far up (0 bottom, 1 top). Author units, or None above the column.
pub fn overview_rgb(prev: &[u32], sx: f32, w: f32, y: f32) -> Option<[f32; 3]> {
    overview_col(prev, sx, w)?.at(y)
}

/// One pixel column of the RGB overall waveform (`overview_rgb`), picked
/// once; `at` gives each row.
#[derive(Clone, Copy, Debug)]
pub struct OvCol { hue: [f32; 3], h: f32, inner: f32 }

impl OvCol {
    #[inline]
    pub fn at(&self, y: f32) -> Option<[f32; 3]> {
        if y >= self.h { return None; }
        let rel = y / self.h;
        let bright = if y < self.inner { 240.0 - 69.0 * rel } else { 244.0 - 127.0 * rel };
        Some([self.hue[0] * bright, self.hue[1] * bright, self.hue[2] * bright])
    }
}

/// The entry under the pixel with the most height, as an `OvCol`.
pub fn overview_col(prev: &[u32], sx: f32, w: f32) -> Option<OvCol> {
    if prev.is_empty() { return None; }
    let n = prev.len();
    let a = ((sx * n as f32) as usize).min(n - 1);
    let b = (((sx + 1.0 / w.max(1.0)) * n as f32) as usize).clamp(a + 1, n);
    let v = prev[a..b].iter().copied().max_by_key(|v| { let c = v.to_le_bytes(); c[0].max(c[1]).max(c[2]) })?;
    let c = v.to_le_bytes();
    let h = c[0].max(c[1]).max(c[2]) as f32 / 127.0 * 0.93;
    if h <= 0.0 { return None; }
    Some(OvCol { hue: rgb_norm([c[0] as f32, c[1] as f32, c[2] as f32], 1.0), h, inner: c[2] as f32 / 127.0 * 0.93 })
}

/// A rekordbox height of 31 fills this much of the field (the CDJ's outer
/// layer: 6.3 px per step of a 240 px field, fitted on cdj3k-emu).
pub const RB_FULL: f32 = 0.81;

/// One of our columns of a track's rekordbox RGB waveform, packed in a u32:
/// the tallest entry the column spans (red, green, blue 0-7 at bits 0, 3,
/// 6; its height 0-31 at bit 9) and the span's mean height × 8 (bits
/// 14-21). The CDJ picks the tallest entry for colour and averages heights,
/// so a column keeps both rather than one blended entry.
pub fn rb_pack(e: [u8; 4], mean_h: f32) -> u32 {
    (e[0] as u32 & 7) | (e[1] as u32 & 7) << 3 | (e[2] as u32 & 7) << 6 | (e[3] as u32 & 31) << 9
        | ((mean_h * 8.0).round().clamp(0.0, 255.0) as u32) << 14
}
/// `rb_pack` back: ([r, g, b] levels, tallest height, mean height), heights 0-31.
pub fn rb_unpack(v: u32) -> ([f32; 3], f32, f32) {
    ([(v & 7) as f32, (v >> 3 & 7) as f32, (v >> 6 & 7) as f32], (v >> 9 & 31) as f32, (v >> 14 & 255) as f32 / 8.0)
}

/// A track's rekordbox RGB entries (PWV5, 150 a second) laid on our column
/// grid (`rb_pack` per column). A column's span starts half an FFT after
/// its first sample, as the waveforms we serve are shifted to line up with
/// rekordbox's; an entry counts for the part of it the span covers.
pub fn rgb_columns(entries: &[[u8; 4]], n_cols: usize, hop: usize, sr: u32) -> Vec<u32> {
    if entries.is_empty() || sr == 0 { return vec![0; n_cols]; }
    let half = opendeck_analysis::waveform::FFT_SIZE as f64 / 2.0;
    let last = entries.len() - 1;
    (0..n_cols).map(|i| {
        let e0 = ((i as f64 * hop as f64 + half - hop as f64 / 2.0).max(0.0) / sr as f64 * 150.0).min(last as f64);
        let e1 = (e0 + hop as f64 / sr as f64 * 150.0).min(last as f64 + 1.0).max(e0 + 1e-6);
        let (mut hs, mut w, mut tall) = (0.0f64, 0.0f64, entries[e0.floor() as usize]);
        let mut k = e0.floor() as usize;
        while (k as f64) < e1 && k <= last {
            let cover = (e1.min(k as f64 + 1.0) - e0.max(k as f64)).max(0.0);
            hs += entries[k][3] as f64 * cover;
            w += cover;
            if entries[k][3] > tall[3] { tall = entries[k]; }
            k += 1;
        }
        rb_pack(tall, (hs / w.max(1e-9)) as f32)
    }).collect()
}

/// Our band columns as RGB levels, once (at load, not per frame): each
/// column's bands through the curves fitted to rekordbox's RGB waveforms,
/// `rb_pack`ed with no heights (ours are drawn at our own heights).
pub fn own_rgb_columns(cols: &[u32]) -> Vec<u32> {
    cols.iter().map(|&v| {
        let c = v.to_le_bytes();
        let lv = opendeck_mediaserver::rgb_levels(c[0] as f32, c[1] as f32, c[2] as f32);
        rb_pack([lv[0].round() as u8, lv[1].round() as u8, lv[2].round() as u8, 0], 0.0)
    }).collect()
}

/// The RGB levels a renderer keeps beside a track's columns: rekordbox's
/// own when the track came with them (`rgb_columns`), else ours.
pub fn rgb_for(cols: &[u32], rb: Option<&RbWave>, hop: usize, sr: u32) -> Vec<u32> {
    match rb.map(|r| &r.detail) {
        Some(e) if !e.is_empty() => rgb_columns(e, cols.len(), hop, sr),
        _ => own_rgb_columns(cols),
    }
}

/// The pixel at `col_f` covering `span` columns (sampled at up to 12). With
/// `rgb_off` > 0 the levels are stored from there in `cols` (`rgb_for`):
/// rekordbox's, drawn at its heights when `rb_heights`, or ours converted at
/// load. 0 = converted here from our bands (tests).
pub fn rgb_bar(cols: &[u32], num_cols: i64, col_f: f32, span: f32, gain: f32, rgb_off: usize, rb_heights: bool) -> RgbBar {
    let span = span.max(1.0);
    let n = (span.ceil() as usize).clamp(1, 12);
    let last = (num_cols - 1).max(0) as f32;
    // Over the pixel and the next: the tallest (outer) and the mean height
    // (core); over the pixel alone: the tallest, whose colour the core takes.
    let (mut top, mut top_c, mut own, mut own_c, mut hs) = (-1.0f32, [0.0f32; 3], -1.0f32, [0.0f32; 3], 0.0f32);
    for i in 0..2 * n {
        let cf = (col_f - span * 0.5 + (i as f32 + 0.5) * span / n as f32).clamp(0.0, last) as usize;
        // (levels, height to pick the tallest by, height to average)
        let ours = || (unpack(cols[cf.min(cols.len() - 1)])[3] * gain).min(1.0);
        let (lv, tall_h, mean_h) = match cols.get(rgb_off + cf).filter(|_| rgb_off > 0) {
            // rekordbox's, with its heights on the CDJ's scale.
            Some(&v) if rb_heights => { let (lv, t, m) = rb_unpack(v); (lv, t / 31.0 * RB_FULL, m / 31.0 * RB_FULL) }
            // Levels ready, at our heights.
            Some(&v) => { let h = ours(); (rb_unpack(v).0, h, h) }
            None => {
                let c = unpack(cols[cf.min(cols.len() - 1)]);
                let h = ours();
                (opendeck_mediaserver::rgb_levels(c[0] * 255.0, c[1] * 255.0, c[2] * 255.0), h, h)
            }
        };
        if tall_h > top { top = tall_h; top_c = lv; }
        if i < n && tall_h > own { own = tall_h; own_c = lv; }
        hs += mean_h;
    }
    RgbBar { core: hs / (2 * n) as f32, core_rgb: rgb_norm(own_c, 224.0), outer: top, outer_rgb: rgb_norm(top_c, 109.0) }
}

/// `bar_color` with the per-column part hoisted: everything that does not
/// depend on the row.
struct Bar {
    mode: u32,
    amp:  f32,          // min(c.w * gain, 1)
    // 3 BAND thresholds (dist < h[2] white, < h[1] mid, < h[0] low).
    h: [f32; 3],
    // RGB / BLUE: base colour in author units, scaled by the row's shade.
    base: [f32; 3],
    // LIGHT MODE: each edge's width (low, mid, high, outline) in `dist`
    // units, the shader's `edge_widths`; all 0 = hard edges.
    fw: [f32; 4],
    // RGB: the CDJ-3000's two layers (set by `with_rgb`).
    rgb: Option<RgbBar>,
}

impl Bar {
    fn new(c_in: [f32; 4], p: &WaveformParams) -> Self {
        let amp  = (c_in[3] * p.amp_gain).min(1.0);
        let mode = (p.color_mode + 0.5) as u32;
        let mut h = [0.0; 3];
        let mut base = [0.0; 3];
        if mode == 1 {
            let m = c_in[0].max(c_in[1]).max(c_in[2]) + 0.001;
            h = [c_in[0] / m * amp, c_in[1] / m * amp, c_in[2] / m * amp];
        } else if mode == 2 {
            // blue_wave(s, hi): s scales all three channels, so it factors out.
            let t = (c_in[2] * 2.4 - 0.15).clamp(0.0, 0.9);
            base = [40.0 + (235.0 - 40.0) * t, 125.0 + (240.0 - 125.0) * t, 235.0 + (250.0 - 235.0) * t];
        } else {
            base = [c_in[0] * 255.0, c_in[1] * 255.0, c_in[2] * 255.0];
        }
        Self { mode, amp, h, base, fw: [0.0; 4], rgb: None }
    }

    /// RGB mode: draw the CDJ-3000's two layers instead.
    fn with_rgb(mut self, rb: RgbBar) -> Self { if self.mode == 0 { self.rgb = Some(rb); } self }

    /// The shader's `bar_edges`: 3 BAND's band bounds and the outline.
    fn edges(&self) -> [f32; 4] {
        if self.mode == 1 { [self.h[0], self.h[1], self.h[2], self.h[0].max(self.h[1]).max(self.h[2])] } else { [self.amp; 4] }
    }

    /// LIGHT MODE: edge widths from the bars a pixel either side.
    fn with_widths(mut self, px: f32, left: &Bar, right: &Bar) -> Self {
        let (l, r) = (left.edges(), right.edges());
        for k in 0..4 { self.fw[k] = px + (r[k] - l[k]).abs() * 0.5; }
        self
    }

    /// Colour at normalised distance `dist` from the centre line, or None
    /// outside the bar.
    #[inline(always)]
    fn at(&self, l: &Lut, pal: &Palette, dist: f32) -> Option<Rgba> {
        if let Some(rb) = &self.rgb {
            if dist < rb.core { return Some(l.rgb(rb.core_rgb[0], rb.core_rgb[1], rb.core_rgb[2])); }
            if dist < rb.outer { return Some(l.rgb(rb.outer_rgb[0], rb.outer_rgb[1], rb.outer_rgb[2])); }
            return None;
        }
        if self.mode == 1 {
            if dist < self.h[2] { return Some(pal.white); }
            if dist < self.h[1] { return Some(pal.band_mid); }
            if dist < self.h[0] { return Some(pal.band_low); }
            return None;
        }
        if dist >= self.amp { return None; }
        let shade = 1.0 - dist / (self.amp + 0.001) * 0.3;
        Some(l.rgb(self.base[0] * shade, self.base[1] * shade, self.base[2] * shade))
    }

    /// LIGHT MODE's `at`: the colour and how much of the pixel the bar
    /// covers, edges blended over `px` (one pixel in `dist` units), as the
    /// shader's `bar_color` with `px` > 0.
    fn at_aa(&self, l: &Lut, pal: &Palette, dist: f32) -> Option<(Rgba, f32)> {
        if self.rgb.is_some() { return self.at(l, pal, dist).map(|c| (c, 1.0)); }
        let e = self.edges();
        let cover = |k: usize| ((e[k] - dist) / self.fw[k] + 0.5).clamp(0.0, 1.0);
        if self.mode == 1 {
            let a = cover(3);
            if a <= 0.0 { return None; }
            let c = blend(pal.band_low, pal.band_mid, cover(1));
            return Some((blend(c, pal.white, cover(2)), a));
        }
        let a = cover(3);
        if a <= 0.0 { return None; }
        let shade = 1.0 - dist.min(self.amp) / (self.amp + 0.001) * 0.3;
        Some((l.rgb(self.base[0] * shade, self.base[1] * shade, self.base[2] * shade), a))
    }
}

/// What a pixel column of the enlarged waveform is, before the rows.
enum Column {
    Solid(Rgba),
    Field { bar: Option<Bar>, bg: Rgba, tick: Option<(bool, f32)> },   // tick: (is_down, len px)
}

/// Rasterise `kind`'s rect into `out`: one RGBA8 pixel per u32 (R in the
/// low byte, i.e. the byte order SDL's ABGR8888 wants), `rect.w` per row.
pub fn draw(lut: &Lut, p: &WaveformParams, cols: &[u32], kind: Kind, rect: PxRect, out: &mut Vec<u32>) {
    let (w, h) = (rect.w as usize, rect.h as usize);
    out.clear();
    out.resize(w * h, 0);
    let pal = Palette::new(lut, p);
    match kind {
        Kind::Wave     => draw_wave(lut, &pal, p, cols, rect, out),
        Kind::Overview => draw_overview(lut, &pal, p, cols, rect, out),
    }
}

#[inline(always)]
fn pack(c: Rgba) -> u32 { u32::from_ne_bytes(c) }

impl Palette {
    /// A finished pixel: flipped in LIGHT MODE (the shader's `finish`).
    #[inline(always)]
    fn px(&self, c: Rgba) -> u32 {
        if !self.light { return pack(c); }
        let [r, g, b] = crate::theme::flip_rgb([c[0], c[1], c[2]]);
        pack([r, g, b, c[3]])
    }
}

/// `mix(a, b, t)` in linear light, as the shader blends, back to sRGB bytes.
fn blend(a: Rgba, b: Rgba, t: f32) -> Rgba {
    if t <= 0.0 { return a; }
    if t >= 1.0 { return b; }
    let f = |x: u8, y: u8| {
        let (x, y) = (srgb_decode(x as f32 / 255.0), srgb_decode(y as f32 / 255.0));
        (srgb_encode(x + (y - x) * t) * 255.0 + 0.5) as u8
    };
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), 255]
}

/// The parameters the overview depends on, so a backend can skip the raster
/// when nothing it shows has moved (the playhead crosses a pixel every few
/// hundred ms; the enlarged waveform scrolls every frame).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OverviewKey {
    rect: PxRect,
    // Half-pixel positions: the shader tests |q.x - x| against fixed widths.
    playhead_x: i32, cue_x: i32, loop_x: (i32, i32), slip_x: i32,
    loop_cols: (i32, i32),
    color_mode: u32, dim: bool, amp_gain: f32, num_cols: f32, light: bool,
}

impl OverviewKey {
    pub fn new(p: &WaveformParams, rect: PxRect) -> Self {
        let r = p.over_rect;
        let px = |col: f32| (((r[0] + col / p.num_cols.max(1.0) * r[2]) * 2.0).round()) as i32;
        Self {
            rect,
            playhead_x: px(p.playhead_col), cue_x: px(p.cue_col),
            loop_x: (px(p.loop_start_col), px(p.loop_end_col)), slip_x: if p.slip_col >= 0.0 { px(p.slip_col) } else { -1 },
            loop_cols: (p.loop_start_col as i32, p.loop_end_col as i32),
            color_mode: p.color_mode as u32, dim: p.dim_played > 0.5, amp_gain: p.amp_gain, num_cols: p.num_cols,
            light: p.light > 0.5,
        }
    }
}

#[inline(always)]
/// The looped span's field: navy (XDJ look) or amber (CDJ-3000 look).
fn loop_bg(pal: &Palette, p: &WaveformParams) -> Rgba {
    if p.style > 0.5 { pal.loop_bg_cdj } else { pal.loop_bg }
}

fn in_loop(p: &WaveformParams, col: f32) -> bool {
    p.loop_end_col > p.loop_start_col && col >= p.loop_start_col && col < p.loop_end_col
}


/// Per-column geometry of the enlarged waveform shared by the full and the
/// incremental raster.
struct WaveGeom { half: f32, cue_per_px: f32, cols_per_px: f32, wave_h: f32, num_cols: i64 }

impl WaveGeom {
    fn new(p: &WaveformParams) -> Self {
        let r = p.wave_rect;
        let strip = p.beat2_period_cols > 0.0;
        Self {
            half: p.cols_visible * 0.5,
            cue_per_px: p.cols_visible / r[2],
            cols_per_px: p.cols_visible / r[2],
            wave_h: r[3] - if strip { B2_STRIP_PX } else { 0.0 },
            num_cols: p.num_cols as i64,
        }
    }
}

/// What pixel column `x` of the rect is.  `with_playhead` = false leaves the
/// red centre line out (the incremental cache overlays it at copy time).
fn classify(pal: &Palette, p: &WaveformParams, g: &WaveGeom, cols: &[u32], rect: PxRect, x: usize, with_playhead: bool) -> Column {
    let r = p.wave_rect;
    let qx = rect.x as f32 + x as f32 + 0.5;
    let sx = (qx - r[0]) / r[2];
    let col_f = (p.playhead_col - g.half) + sx * p.cols_visible;
    if with_playhead && (qx - (r[0] + r[2] * 0.5)).abs() < 1.5 {
        Column::Solid(pal.playhead)
    } else if (col_f - p.cue_col).abs() < g.cue_per_px {
        Column::Solid(pal.cue)
    } else if p.style < 0.5 && p.loop_end_col > p.loop_start_col
        && ((col_f - p.loop_start_col).abs() < g.cue_per_px || (col_f - p.loop_end_col).abs() < g.cue_per_px) {
        Column::Solid(pal.loop_line)
    } else if p.slip_col >= 0.0 && (col_f - p.slip_col).abs() < g.cue_per_px {
        Column::Solid(pal.slip)
    } else {
        let bg = if in_loop(p, col_f) { loop_bg(pal, p) } else { pal.ground };
        if col_f < 0.0 || col_f >= p.num_cols || g.num_cols <= 0 || cols.is_empty() {
            Column::Field { bar: None, bg, tick: None }
        } else {
            let c = wave_sample(cols, g.num_cols, col_f);
            let tick = if p.beat_period_cols > 0.0 {
                let rel      = col_f - p.beat_anchor_col;
                let beat_pos = ((rel % p.beat_period_cols) + p.beat_period_cols) % p.beat_period_cols;
                let beat_num = (rel / p.beat_period_cols).floor();
                let bpb      = p.beats_per_bar;
                let adjusted = ((beat_num + p.downbeat_offset) % bpb + bpb) % bpb;
                let is_down  = adjusted < 0.5;
                let tick_w   = if is_down { 2.5 } else { 1.5 } * g.cols_per_px;
                (beat_pos < tick_w).then(|| (is_down, if is_down { 0.10 } else { 0.055 } * g.wave_h))
            } else { None };
            let mut bar = Bar::new(c, p);
            if pal.light {
                let side = |d: f32| Bar::new(wave_sample(cols, g.num_cols, col_f + d), p);
                bar = bar.with_widths(2.0 / g.wave_h, &side(-g.cue_per_px), &side(g.cue_per_px));
            }
            if bar.mode == 0 { bar = bar.with_rgb(rgb_bar(cols, g.num_cols, col_f, g.cue_per_px, p.amp_gain, p.rgb_off as usize, p.rgb_heights > 0.5)); }
            Column::Field { bar: Some(bar), bg, tick }
        }
    }
}

/// The shader's `wave_sample`: bands at `col_f`, bilinear, clamped to the track.
fn wave_sample(cols: &[u32], num_cols: i64, col_f: f32) -> [f32; 4] {
    let n = (num_cols - 1).max(0) as f32;
    let cf = col_f.clamp(0.0, n);
    let col_lo = cf as usize;
    let col_hi = (col_lo + 1).min(n as usize);
    mix4(unpack(cols[col_lo.min(cols.len() - 1)]), unpack(cols[col_hi.min(cols.len() - 1)]), cf - col_lo as f32)
}

/// Fill the rect's pixel columns `x0..x0+columns.len()` of every row from
/// their classification, into `out` with `stride` pixels per row at
/// `out_x` (so a strip buffer can take a few columns with its own stride).
fn fill_columns(l: &Lut, pal: &Palette, p: &WaveformParams, g: &WaveGeom, rect: PxRect, columns: &[Column], x0: usize, stride: usize, out_x: usize, out: &mut [u32]) {
    let r = p.wave_rect;
    let h = rect.h as usize;
    let strip = p.beat2_period_cols > 0.0;
    for y in 0..h {
        let qy   = rect.y as f32 + y as f32 + 0.5;
        let yy   = qy - r[1];
        let dist = (yy / g.wave_h - 0.5).abs() * 2.0;
        let silence_line = p.detail_off > 0.0;
        let row  = &mut out[y * stride + out_x..y * stride + out_x + columns.len()];
        if strip && qy > r[1] + r[3] - B2_STRIP_PX {
            // External-deck beat strip along the bottom edge (disabled today —
            // beat2_period_cols is 0 — but kept in step with the shader, where
            // it is tested before everything else in the column).
            let pixels_per_beat = p.beat2_period_cols * r[2] / p.cols_visible;
            let phase_px = p.beat2_phase_beats * pixels_per_beat;
            for (i, px) in row.iter_mut().enumerate() {
                let qx = rect.x as f32 + (x0 + i) as f32 + 0.5;
                let beat_x = (((qx - r[0]) + phase_px) % pixels_per_beat + pixels_per_beat) % pixels_per_beat;
                *px = pal.px(if beat_x < 3.0 || beat_x > pixels_per_beat - 3.0 { pal.cyan } else { pal.strip_bg });
            }
            continue;
        }
        for (px, column) in row.iter_mut().zip(columns) {
            *px = pal.px(match column {
                Column::Solid(c) => *c,
                Column::Field { bar, bg, tick } => {
                    let under = || {
                        // Stem mix: a thin grey line where it is silent.
                        if silence_line && bar.is_some() && (yy - g.wave_h * 0.5).abs() < 1.0 { return pal.silence; }
                        match tick {
                            Some((is_down, len)) if yy < *len || yy > g.wave_h - *len => if *is_down { pal.down_hi } else { pal.tick_hi },
                            _ => *bg,
                        }
                    };
                    if pal.light {
                        match bar.as_ref().and_then(|b| b.at_aa(l, pal, dist)) {
                            Some((c, a)) if a >= 1.0 => c,
                            Some((c, a)) => blend(under(), c, a),
                            None => under(),
                        }
                    } else {
                        match bar.as_ref().and_then(|b| b.at(l, pal, dist)) { Some(c) => c, None => under() }
                    }
                }
            });
        }
    }
}

fn draw_wave(l: &Lut, pal: &Palette, p: &WaveformParams, cols: &[u32], rect: PxRect, out: &mut [u32]) {
    let g = WaveGeom::new(p);
    let w = rect.w as usize;
    // Columns first (everything that does not depend on the row), then rows,
    // writing each row sequentially: one cache line per 8 px, not one per
    // pixel as a column walk would cost.
    let columns: Vec<Column> = (0..w).map(|x| classify(pal, p, &g, cols, rect, x, true)).collect();
    fill_columns(l, pal, p, &g, rect, &columns, 0, w, 0, out);
}

// ── Incremental (whole-pixel scroll) raster ───────────────────────────────────

/// Everything the enlarged waveform depends on except the playhead.  A
/// change means the cached field is stale.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WaveKey {
    rect: PxRect, wave_rect: [i32; 4],
    cols_visible: f32, num_cols: f32, amp_gain: f32, color_mode: u32,
    beat: (f32, f32, f32, f32),
    // The external-deck strip's phase ticks every frame; it only matters when
    // the strip is on, and then the cache is bypassed anyway.
    beat2_period: f32,
    cue_col: f32, loop_cols: (f32, f32), slip_col: f32,
    cols_ptr: usize, cols_len: usize,
    light: bool,
}

impl WaveKey {
    fn new(p: &WaveformParams, rect: PxRect, cols: &[u32]) -> Self {
        Self {
            rect, wave_rect: p.wave_rect.map(|v| (v * 4.0) as i32),
            cols_visible: p.cols_visible, num_cols: p.num_cols, amp_gain: p.amp_gain, color_mode: p.color_mode as u32,
            beat: (p.beat_anchor_col, p.beat_period_cols, p.downbeat_offset, p.beats_per_bar),
            beat2_period: p.beat2_period_cols,
            cue_col: p.cue_col, loop_cols: (p.loop_start_col, p.loop_end_col), slip_col: p.slip_col,
            cols_ptr: cols.as_ptr() as usize, cols_len: cols.len(),
            light: p.light > 0.5,
        }
    }
}

/// The cached field (no playhead) of the enlarged waveform at a playhead
/// quantised to whole pixels, so a frame is a row shift plus the columns
/// that scrolled in.  Trades the shader's sub-pixel scroll for ~10× less
/// CPU: a one-core board's choice.
#[derive(Default)]
pub struct WaveCache {
    key: Option<WaveKey>,
    /// Quantised playhead, in pixels (playhead_col / cols_per_px).
    q:   i64,
    /// The field, `rect.w` pixels per row, no playhead.  Only kept current
    /// when the backend scrolls on the CPU (`keep_field`).
    pub pix: Vec<u32>,
    /// With `keep_field` off: the columns that entered this frame, row-major,
    /// `|d|` pixels per row — all the CPU rasterises per scrolled frame.
    pub strip: Vec<u32>,
}

impl WaveCache {
    pub fn invalidate(&mut self) { self.key = None; }
}

/// What `update_wave_cache` did to the cached field this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveUpdate {
    /// Nothing moved: the field is as last frame.
    Unchanged,
    /// Rebuilt from scratch (first frame, zoom/track/marker change, big jump).
    Full,
    /// Scrolled by `d` pixels (positive = picture moved left): only the last
    /// `d` columns (or the first `-d`) are new.
    Shifted(i64),
}

/// The playhead colour (sRGB bytes), for a backend drawing it as a rect.
pub fn playhead_rgb(lut: &Lut, p: &WaveformParams) -> [u8; 4] {
    let pal = Palette::new(lut, p);
    pal.px(pal.playhead).to_ne_bytes()
}

/// Pixel columns of `rect` the playhead line covers (the shader's
/// `|q.x - centre| < 1.5`).
pub fn playhead_columns(p: &WaveformParams, rect: PxRect) -> std::ops::Range<usize> {
    let r = p.wave_rect;
    let centre = r[0] + r[2] * 0.5;
    let (mut lo, mut hi) = (usize::MAX, 0usize);
    for x in 0..rect.w as usize {
        let qx = rect.x as f32 + x as f32 + 0.5;
        if (qx - centre).abs() < 1.5 { lo = lo.min(x); hi = x + 1; }
    }
    if lo == usize::MAX { 0..0 } else { lo..hi }
}

/// Draw the enlarged waveform through the cache into `out`.  The playhead
/// is quantised to `cols_per_px` (one pixel of scroll); the rest of the
/// frame matches `draw(.., Kind::Wave, ..)` at that quantised playhead.
/// Returns false when the picture is the same as last time (nothing moved),
/// so the caller can keep its texture and skip the upload.
pub fn draw_wave_cached(lut: &Lut, p: &WaveformParams, cols: &[u32], rect: PxRect, cache: &mut WaveCache, out: &mut Vec<u32>) -> bool {
    let (w, h) = (rect.w as usize, rect.h as usize);
    if update_wave_cache(lut, p, cols, rect, cache, true) == WaveUpdate::Unchanged && out.len() == w * h { return false; }
    // Copy out with the playhead line on top (the shader draws it first in
    // every column it covers, over everything but the strip).
    out.clear();
    out.extend_from_slice(&cache.pix);
    let ph = pack(playhead_rgb(lut, p));
    for x in playhead_columns(p, rect) {
        for y in 0..h { out[y * w + x] = ph; }
    }
    true
}

/// Bring the cache up to date for this frame's playhead and say how the
/// picture changed.  `keep_field` shifts `pix` on the CPU (for a backend
/// that uploads the whole field); off, a scroll only rasterises the new
/// columns into `strip` and the backend shifts its texture on the GPU.
pub fn update_wave_cache(lut: &Lut, p: &WaveformParams, cols: &[u32], rect: PxRect, cache: &mut WaveCache, keep_field: bool) -> WaveUpdate {
    let (w, h) = (rect.w as usize, rect.h as usize);
    let pal = Palette::new(lut, p);
    let g = WaveGeom::new(p);
    // Quantise: the column at pixel x only depends on (playhead - half) +
    // x*cols_per_px, so moving the playhead by k*cols_per_px moves the
    // picture by exactly k pixels.
    let q = (p.playhead_col / g.cols_per_px).round() as i64;
    let pq = WaveformParams { playhead_col: q as f32 * g.cols_per_px, ..*p };
    let key = WaveKey::new(&pq, rect, cols);
    let strip = p.beat2_period_cols > 0.0;   // the strip is wall-clock driven: no caching
    let fresh = cache.key != Some(key) || cache.pix.len() != w * h || strip;
    let mut update = if fresh { WaveUpdate::Full } else { WaveUpdate::Unchanged };
    if fresh {
        cache.pix.clear();
        cache.pix.resize(w * h, 0);
        let columns: Vec<Column> = (0..w).map(|x| classify(&pal, &pq, &g, cols, rect, x, false)).collect();
        fill_columns(lut, &pal, &pq, &g, rect, &columns, 0, w, 0, &mut cache.pix);
        cache.key = Some(key);
        cache.q = q;
    } else {
        let d = q - cache.q;   // pixels the picture moved left
        if d != 0 {
            let dd = d.unsigned_abs() as usize;
            if dd >= w {
                let columns: Vec<Column> = (0..w).map(|x| classify(&pal, &pq, &g, cols, rect, x, false)).collect();
                fill_columns(lut, &pal, &pq, &g, rect, &columns, 0, w, 0, &mut cache.pix);
                update = WaveUpdate::Full;
            } else {
                let x0 = if d > 0 { w - dd } else { 0 };
                let columns: Vec<Column> = (x0..x0 + dd).map(|x| classify(&pal, &pq, &g, cols, rect, x, false)).collect();
                if keep_field {
                    if d > 0 { for y in 0..h { cache.pix[y * w..(y + 1) * w].copy_within(dd.., 0); } }
                    else     { for y in 0..h { cache.pix[y * w..(y + 1) * w].copy_within(..w - dd, dd); } }
                    fill_columns(lut, &pal, &pq, &g, rect, &columns, x0, w, x0, &mut cache.pix);
                } else {
                    cache.strip.clear();
                    cache.strip.resize(dd * h, 0);
                    fill_columns(lut, &pal, &pq, &g, rect, &columns, x0, dd, 0, &mut cache.strip);
                }
            }
            cache.q = q;
            if update == WaveUpdate::Unchanged { update = WaveUpdate::Shifted(d); }
        }
    }
    update
}

fn draw_overview(l: &Lut, pal: &Palette, p: &WaveformParams, cols: &[u32], rect: PxRect, out: &mut [u32]) {
    let r = p.over_rect;
    let (w, h) = (rect.w as usize, rect.h as usize);
    if p.num_cols <= 0.0 || cols.is_empty() {
        out.fill(pal.px(pal.ground));
        return;
    }
    let num_cols = p.num_cols as usize;
    let ph_x  = r[0] + p.playhead_col / p.num_cols * r[2];
    let cue_x = r[0] + p.cue_col / p.num_cols * r[2];
    let cols_per_px = p.num_cols / r[2];
    let n = ((cols_per_px as u32) + 1).min(64) as usize;
    let dim = p.dim_played > 0.5;

    // Per column: a solid marker, or (bar, field, played) for the rows.
    enum Over { Solid(Rgba), Body { bar: Bar, field: Rgba, played: bool }, Rgb { col: Option<OvCol>, field: Rgba, played: bool } }
    let columns: Vec<Over> = (0..w).map(|x| {
        let qx = rect.x as f32 + x as f32 + 0.5;
        let sx = (qx - r[0]) / r[2];
        if (qx - ph_x).abs() < 1.0 { return Over::Solid(pal.white); }
        if (qx - cue_x).abs() < 1.5 { return Over::Solid(pal.cue); }
        if p.loop_end_col > p.loop_start_col {
            let ls_x = r[0] + p.loop_start_col / p.num_cols * r[2];
            let le_x = r[0] + p.loop_end_col   / p.num_cols * r[2];
            if p.style < 0.5 && ((qx - ls_x).abs() < 1.5 || (qx - le_x).abs() < 1.5) { return Over::Solid(pal.loop_line); }
        }
        if p.slip_col >= 0.0 {
            let sl_x = r[0] + p.slip_col / p.num_cols * r[2];
            if (qx - sl_x).abs() < 1.5 { return Over::Solid(pal.slip); }
        }
        let col_here = sx * p.num_cols;
        let field = if in_loop(p, col_here) { loop_bg(pal, p) } else { pal.ground };
        // Peak of each band over the columns this pixel covers.
        let peak = |sx: f32| {
            let c0 = (sx * p.num_cols).max(0.0) as usize;
            let mut c = [0.0f32; 4];
            for i in 0..n {
                let idx = (c0 + i).min(num_cols - 1).min(cols.len() - 1);
                let u = unpack(cols[idx]);
                for k in 0..4 { c[k] = c[k].max(u[k]); }
            }
            c
        };
        let mut bar = Bar::new(peak(sx), p);
        if pal.light {
            let side = |d: f32| Bar::new(peak(sx + d / r[2]), p);
            bar = bar.with_widths(2.0 / r[3], &side(-1.0), &side(1.0));
        }
        if bar.mode == 0 && p.ov_off > 0.5 {
            // RGB: the CDJ-3000's overall waveform, from the bottom up.
            let prev = cols.get(p.ov_off as usize..(p.ov_off as usize + PREVIEW_N)).unwrap_or(&[]);
            return Over::Rgb { col: overview_col(prev, sx, r[2]), field, played: qx < ph_x && dim };
        }
        Over::Body { bar, field, played: qx < ph_x && dim }
    }).collect();

    for y in 0..h {
        let qy = rect.y as f32 + y as f32 + 0.5;
        let dist = ((qy - r[1]) / r[3] - 0.5).abs() * 2.0;
        let row = &mut out[y * w..(y + 1) * w];
        for (px, column) in row.iter_mut().zip(&columns) {
            *px = pal.px(match column {
                Over::Solid(c) => *c,
                Over::Body { bar, field, played } if pal.light => match bar.at_aa(l, pal, dist) {
                    Some((c, a)) => blend(*field, if *played { dim_bar(c) } else { c }, a),
                    None => *field,
                },
                Over::Rgb { col, field, played } => match col.and_then(|c| c.at(1.0 - (qy - r[1]) / r[3])) {
                    Some(c) if *played => dim_bar(l.rgb(c[0], c[1], c[2])),
                    Some(c) => l.rgb(c[0], c[1], c[2]),
                    None => *field,
                },
                Over::Body { bar, field, played } => match bar.at(l, pal, dist) {
                    Some(c) if *played => dim_bar(c),
                    Some(c) => c,
                    None => *field,
                },
            });
        }
    }
}

/// REMAIN mode: `bar * 0.18` in linear space, then encoded like the rest.
fn dim_bar(c: Rgba) -> Rgba {
    let f = |v: u8| -> u8 { (srgb_encode(srgb_decode(v as f32 / 255.0) * 0.18) * 255.0 + 0.5) as u8 };
    [f(c[0]), f(c[1]), f(c[2]), 255]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{ColorMode, WaveformParams};

    fn params(num_cols: u32, wave: [f32; 4], over: [f32; 4]) -> WaveformParams {
        WaveformParams {
            playhead_col: num_cols as f32 / 2.0, cols_visible: 600.0, num_cols: num_cols as f32,
            screen_w: 1024.0, screen_h: 600.0, beat_anchor_col: 0.0, beat_period_cols: 100.0,
            downbeat_offset: 0.0, beats_per_bar: 4.0, beat2_phase_beats: 0.0, beat2_period_cols: 0.0,
            color_mode: ColorMode::Blue as u32 as f32, wave_rect: wave, over_rect: over, amp_gain: 1.0,
            dim_played: 0.0, cue_col: 10.0, loop_start_col: 0.0, loop_end_col: 0.0, slip_col: -1.0, style: 0.0, detail_off: 0.0,
            light: 0.0, rgb_off: 0.0, rgb_heights: 0.0, ov_off: 0.0,
        }
    }

    fn px(out: &[u32], w: usize, x: usize, y: usize) -> [u8; 4] { out[y * w + x].to_ne_bytes() }

    /// A silent stem mix draws OverCue's thin grey line through the middle
    /// of the track's span (2 px), and nothing outside the track; the track's
    /// own silent columns draw nothing.
    #[test]
    fn silent_stem_mix_draws_a_thin_line() {
        let lut = Lut::new(); let pal = Palette::new(&lut, &params(1, [0.0; 4], [0.0; 4]));
        let cols = vec![0u32; 2000];
        let mut p = params(2000, [100.0, 50.0, 800.0, 200.0], [0.0; 4]);
        p.beat_period_cols = 0.0;
        let rect = PxRect::from_f32(p.wave_rect).unwrap();
        let w = rect.w as usize;
        let mut out = Vec::new();
        draw(&lut, &p, &cols, Kind::Wave, rect, &mut out);
        assert_eq!(px(&out, w, 200, 100), pal.ground, "track's own silence: no line");
        p.detail_off = 2000.0;
        draw(&lut, &p, &cols, Kind::Wave, rect, &mut out);
        let line: Vec<usize> = (0..200).filter(|&y| px(&out, w, 200, y) == pal.silence).collect();
        assert_eq!(line, vec![99, 100], "a 2-px line at the centre");
        // Before the track starts (playhead at 0, left half): no line.
        p.playhead_col = 0.0;
        draw(&lut, &p, &cols, Kind::Wave, rect, &mut out);
        assert_eq!(px(&out, w, 100, 100), pal.ground);
    }

    #[test]
    fn playhead_cue_and_bars_land_where_the_shader_puts_them() {
        let lut = Lut::new(); let pal = Palette::new(&lut, &params(1, [0.0; 4], [0.0; 4]));
        let cols: Vec<u32> = (0..2000u32).map(|_| u32::from_le_bytes([80, 120, 200, 180])).collect();
        let p = params(2000, [100.0, 50.0, 800.0, 200.0], [0.0; 4]);
        let rect = PxRect::from_f32(p.wave_rect).unwrap();
        let mut out = Vec::new();
        draw(&lut, &p, &cols, Kind::Wave, rect, &mut out);
        let w = rect.w as usize;
        // Centre column is the red playhead, full height.
        assert_eq!(px(&out, w, 400, 0), pal.playhead);
        assert_eq!(px(&out, w, 400, 199), pal.playhead);
        // The middle row of a bar column is waveform blue, the top row is ground.
        let mid = px(&out, w, 200, 100);
        assert!(mid[2] > mid[0] && mid[2] > 100, "blue body, got {mid:?}");
        assert_eq!(px(&out, w, 200, 0), pal.ground);
    }

    /// A loop on the enlarged waveform: the XDJ shades it navy between two
    /// loop lines; the CDJ-3000 look shades it amber with no lines.
    #[test]
    fn loop_shading_follows_the_skin_style() {
        let lut = Lut::new(); let pal = Palette::new(&lut, &params(1, [0.0; 4], [0.0; 4]));
        let cols: Vec<u32> = vec![0; 2000];   // silence: the field shows through
        let mut p = params(2000, [0.0, 0.0, 800.0, 200.0], [0.0; 4]);
        p.playhead_col = 1000.0;              // centre x = 400; 0.75 cols per px
        p.beat_period_cols = 0.0;
        p.cue_col = -100.0;
        p.loop_start_col = 900.0;             // x ≈ 267
        p.loop_end_col = 1100.0;              // x ≈ 533
        let rect = PxRect::from_f32(p.wave_rect).unwrap();
        let w = rect.w as usize;
        let mut out = Vec::new();

        draw(&lut, &p, &cols, Kind::Wave, rect, &mut out);
        assert_eq!(px(&out, w, 267, 100), pal.loop_line, "XDJ: a loop line at the start");
        assert_eq!(px(&out, w, 533, 100), pal.loop_line, "XDJ: and at the end");
        assert_eq!(px(&out, w, 330, 100), pal.loop_bg, "XDJ: navy inside");
        assert_eq!(px(&out, w, 150, 100), pal.ground, "outside the loop");

        p.style = 1.0;
        draw(&lut, &p, &cols, Kind::Wave, rect, &mut out);
        assert_eq!(px(&out, w, 267, 100), pal.loop_bg_cdj, "CDJ: no line, shade from the start");
        assert_eq!(px(&out, w, 330, 100), pal.loop_bg_cdj, "CDJ: amber inside");
        assert_eq!(px(&out, w, 150, 100), pal.ground);
        assert_ne!(pal.loop_bg_cdj, pal.loop_bg);
    }

    #[test]
    fn overview_marks_the_playhead_and_fills_the_body() {
        let lut = Lut::new(); let pal = Palette::new(&lut, &params(1, [0.0; 4], [0.0; 4]));
        let cols: Vec<u32> = (0..5000u32).map(|_| u32::from_le_bytes([200, 200, 200, 255])).collect();
        let p = params(5000, [0.0; 4], [10.0, 500.0, 1000.0, 60.0]);
        let rect = PxRect::from_f32(p.over_rect).unwrap();
        let mut out = Vec::new();
        draw(&lut, &p, &cols, Kind::Overview, rect, &mut out);
        let w = rect.w as usize;
        assert_eq!(px(&out, w, 500, 5), pal.white);        // playhead at the middle
        assert_ne!(px(&out, w, 300, 30), pal.ground);      // body at the centre row
        assert_eq!(out.len(), 1000 * 60);
    }

    #[test]
    fn overview_key_changes_only_when_something_visible_moves() {
        let mut p = params(5000, [0.0; 4], [10.0, 500.0, 1000.0, 60.0]);
        let rect = PxRect::from_f32(p.over_rect).unwrap();
        let a = OverviewKey::new(&p, rect);
        p.playhead_col += 0.5;                     // a tenth of a pixel: same key
        assert_eq!(a, OverviewKey::new(&p, rect));
        p.playhead_col += 5.0;                     // a pixel: new key
        assert_ne!(a, OverviewKey::new(&p, rect));
    }

    #[test]
    fn incremental_scroll_matches_a_full_raster() {
        let lut = Lut::new();
        let cols: Vec<u32> = (0..4000u32).map(|i| u32::from_le_bytes([(i % 251) as u8, (i * 7 % 199) as u8, (i * 3 % 173) as u8, 150 + (i % 100) as u8])).collect();
        let mut p = params(4000, [100.0, 50.0, 600.0, 185.0], [0.0; 4]);
        p.cue_col = 1210.0;
        let rect = PxRect::from_f32(p.wave_rect).unwrap();
        let mut cache = WaveCache::default();
        let (mut inc, mut full) = (Vec::new(), Vec::new());
        // Scroll forward in uneven steps, then back, then jump.
        for step in [0.0f32, 0.4, 1.3, 2.9, 0.7, 5.0, -3.2, 40.0, 900.0, -1.0] {
            p.playhead_col += step;
            draw_wave_cached(&lut, &p, &cols, rect, &mut cache, &mut inc);
            let g = WaveGeom::new(&p);
            let q = (p.playhead_col / g.cols_per_px).round();
            let pq = WaveformParams { playhead_col: q * g.cols_per_px, ..p };
            draw(&lut, &pq, &cols, Kind::Wave, rect, &mut full);
            assert_eq!(inc, full, "step {step}: incremental raster differs from the full one");
        }
        // A zoom change invalidates and still matches.
        p.cols_visible = 300.0;
        draw_wave_cached(&lut, &p, &cols, rect, &mut cache, &mut inc);
        let g = WaveGeom::new(&p);
        let q = (p.playhead_col / g.cols_per_px).round();
        let pq = WaveformParams { playhead_col: q * g.cols_per_px, ..p };
        draw(&lut, &pq, &cols, Kind::Wave, rect, &mut full);
        assert_eq!(inc, full, "after zoom change");
    }

    /// LIGHT MODE: the field is white, the bar keeps its hue, and its edge
    /// is blended instead of a hard step; dark mode keeps hard edges.
    #[test]
    fn light_mode_draws_white_ground_and_soft_edges() {
        let lut = Lut::new();
        // A ramp of loudness, so the outline slopes from column to column.
        let cols: Vec<u32> = (0..2000u32).map(|i| u32::from_le_bytes([60, 60, 20, (60 + i % 120) as u8])).collect();
        let mut p = params(2000, [0.0, 0.0, 800.0, 200.0], [0.0; 4]);
        p.beat_period_cols = 0.0;
        p.cue_col = -100.0;
        let rect = PxRect::from_f32(p.wave_rect).unwrap();
        let w = rect.w as usize;
        let mut out = Vec::new();
        // The sharpest step between neighbouring rows of the column's top
        // half (blue channel), as a share of the step from field to bar
        // centre: 1.0-ish for a hard edge, smaller when the edge blends.
        let sharpest = |out: &[u32], x: usize| {
            let b = |y: usize| px(out, w, x, y)[2] as f32;
            let span = (b(100) - b(0)).abs().max(1.0);
            (0..99).map(|y| (b(y + 1) - b(y)).abs()).fold(0.0, f32::max) / span
        };

        draw(&lut, &p, &cols, Kind::Wave, rect, &mut out);
        let dark_ground = px(&out, w, 200, 0);
        let dark_step: f32 = (190..210).map(|x| sharpest(&out, x)).sum::<f32>() / 20.0;

        p.light = 1.0;
        draw(&lut, &p, &cols, Kind::Wave, rect, &mut out);
        let ground = px(&out, w, 200, 0);
        assert!(ground[0] > 245 && ground[1] > 245 && ground[2] > 245, "white field, got {ground:?}");
        let [r, g, b] = crate::theme::flip_rgb([dark_ground[0], dark_ground[1], dark_ground[2]]);
        assert_eq!(&ground[..3], &[r, g, b], "the dark field, mirrored");
        let mid = px(&out, w, 200, 100);
        assert!(mid[2] > mid[0] && mid[2] > 100, "the bar is still blue, got {mid:?}");
        // The edge spreads over more than one row.
        let light_step: f32 = (190..210).map(|x| sharpest(&out, x)).sum::<f32>() / 20.0;
        assert!(light_step < dark_step * 0.8, "edges blend: steepest step {light_step:.2} of the span vs {dark_step:.2} in dark");
        // The playhead stays red.
        let ph = px(&out, w, 400, 0);
        assert!(ph[0] > 200 && ph[1] < 80, "red playhead, got {ph:?}");
    }

    #[test]
    fn empty_rects_are_rejected() {
        assert!(PxRect::from_f32([0.0, 0.0, 0.0, 0.0]).is_none());
        assert!(PxRect::from_f32([f32::INFINITY, 0.0, 10.0, 10.0]).is_none());
    }
}

/// RGB as the CDJ-3000 draws it (cdj3k-emu, 2026-11-24), on the model both
/// renderers share.
#[cfg(test)]
mod rgb_tests {
    use super::*;

    fn col(low: u8, mid: u8, high: u8, amp: u8) -> u32 { u32::from_le_bytes([low, mid, high, amp]) }

    #[test]
    fn colours_are_normalised_hues_not_raw_energy() {
        // A bass-heavy column with some highs: raw it is mostly red; on the
        // CDJ the hue comes from rekordbox's levels, brightest channel 224.
        let b = rgb_bar(&[col(200, 40, 25, 255)], 1, 0.0, 1.0, 1.0, 0, false);
        let m = b.core_rgb.iter().cloned().fold(0.0, f32::max);
        assert!((m - 224.0).abs() < 1e-3, "the brightest channel is 224: {:?}", b.core_rgb);
        assert!(b.core_rgb[2] > 112.0, "the highs show as blue (magenta, as the unit draws a kick), not red alone: {:?}", b.core_rgb);
        // A quiet column with little bass: blue.
        let q = rgb_bar(&[col(20, 30, 30, 80)], 1, 0.0, 1.0, 1.0, 0, false);
        assert!(q.core_rgb[2] >= q.core_rgb[0], "quiet and bright on top reads blue: {:?}", q.core_rgb);
    }

    /// The CDJ's two layers, as fitted on cdj3k-emu: over this pixel's 4
    /// columns and the next pixel's 4, the outer is the tallest (its height,
    /// its colour at 109) and the core is the mean height, coloured as the
    /// tallest of this pixel's own 4 (at 224).
    #[test]
    fn outer_is_the_tallest_of_two_pixels_core_the_mean_in_its_own_tallest_colour() {
        let (kick, hat, loud_next) = (col(200, 40, 25, 200), col(30, 60, 40, 120), col(10, 120, 60, 255));
        let cols = [hat, kick, hat, hat, hat, loud_next, hat, hat];
        // The pixel covers columns 0-3 (centre 2, span 4); the next, 4-7.
        let b = rgb_bar(&cols, 8, 2.0, 4.0, 1.0, 0, false);
        assert!((b.outer - 1.0).abs() < 1e-6, "outer = the tallest over both pixels (the next one's loud column)");
        assert_eq!(b.outer_rgb, rgb_norm(opendeck_mediaserver::rgb_levels(10.0, 120.0, 60.0), 109.0), "in that column's colour");
        let mean = (200.0 + 255.0 + 6.0 * 120.0) / 8.0 / 255.0;
        assert!((b.core - mean).abs() < 1e-3, "core = the mean height over both: {} vs {mean}", b.core);
        assert_eq!(b.core_rgb, rgb_norm(opendeck_mediaserver::rgb_levels(200.0, 40.0, 25.0), 224.0), "coloured as this pixel's tallest (the kick)");
        let m = b.outer_rgb.iter().cloned().fold(0.0, f32::max);
        assert!((m - 109.0).abs() < 1e-3 && (b.core_rgb.iter().cloned().fold(0.0, f32::max) - 224.0).abs() < 1e-3);
    }

    /// rekordbox's entries (150 a second) land on our columns: each keeps
    /// the tallest entry its span covers and the span's mean height.
    #[test]
    fn stick_rgb_lands_on_our_columns_keeping_the_tallest_and_the_mean() {
        let (hop, sr) = (512usize, 48_000u32);
        // Entry k: height = k % 32 (a sawtooth), green = (k / 100) % 8.
        let entries: Vec<[u8; 4]> = (0..2000).map(|k| [7, ((k / 100) % 8) as u8, 3, (k % 32) as u8]).collect();
        let cols = rgb_columns(&entries, 1000, hop, sr);
        for i in [0usize, 1, 37, 500, 999] {
            let start = (i * hop + opendeck_analysis::waveform::FFT_SIZE / 2 - hop / 2) as f64 / sr as f64 * 150.0;
            let (lv, tall, mean) = rb_unpack(cols[i]);
            assert_eq!((lv[0], lv[2]), (7.0, 3.0), "column {i}: an entry's colour");
            assert!((lv[1] - ((start as usize / 100) % 8) as f32).abs() <= 1.0, "column {i} is around entry {start:.0}");
            assert!(tall >= mean, "column {i}: tallest {tall} ≥ mean {mean}");
        }
        // The tallest wins, the mean is the mean: heights 31 and 1 alternating.
        let alt: Vec<[u8; 4]> = (0..600).map(|k| if k % 2 == 0 { [7, 0, 0, 31] } else { [0, 0, 7, 1] }).collect();
        let (lv, tall, mean) = rb_unpack(rgb_columns(&alt, 100, hop, sr)[50]);
        assert_eq!((lv, tall), ([7.0, 0.0, 0.0], 31.0), "the tall entry's colour and height, not a blend");
        assert!(mean > 8.0 && mean < 24.0, "the mean height of the span: {mean}");
        assert_eq!(rgb_columns(&[], 4, hop, sr), vec![0; 4], "none: zeros");
        assert_eq!(rb_unpack(rb_pack([5, 6, 7, 30], 12.5)), ([5.0, 6.0, 7.0], 30.0, 12.5), "packs both ways");
    }

    /// With the stick's columns after the track's, RGB takes rekordbox's
    /// colours and heights, not our bands': here bands that read red-heavy,
    /// a stick that says a blue column of height 31.
    #[test]
    fn the_sticks_levels_win_over_our_bands() {
        let mut cols = vec![col(200, 40, 25, 100); 4];
        let ours = rgb_bar(&cols, 4, 1.0, 1.0, 1.0, 0, false);
        assert!(ours.core_rgb[0] > 200.0, "our bands alone: red-heavy {:?}", ours.core_rgb);
        cols.extend(std::iter::repeat(rb_pack([0, 0, 7, 31], 31.0)).take(4));
        let rb = rgb_bar(&cols, 4, 1.0, 1.0, 1.0, 4, true);
        assert_eq!(rb.core_rgb, [0.0, 0.0, 224.0], "the stick's blue");
        assert_eq!(rb.outer_rgb, [0.0, 0.0, 109.0]);
        assert!((rb.outer - RB_FULL).abs() < 1e-6 && (rb.core - RB_FULL).abs() < 1e-6, "rekordbox's heights on the CDJ's scale");
    }

    /// Our bands converted once at load draw the colours converting them
    /// per frame did (to rekordbox's 0-7 steps), at our heights.
    #[test]
    fn levels_converted_at_load_match_converting_per_frame() {
        let mut cols: Vec<u32> = (0..64).map(|i| col((i * 37 % 230) as u8, (i * 11 % 120) as u8, (i * 5 % 50) as u8, (40 + i * 3) as u8)).collect();
        let fresh: Vec<RgbBar> = (0..60).map(|i| rgb_bar(&cols, 64, i as f32, 2.0, 1.0, 0, false)).collect();
        cols.extend(own_rgb_columns(&cols.clone()));
        for (i, f) in fresh.iter().enumerate() {
            let s = rgb_bar(&cols, 64, i as f32, 2.0, 1.0, 64, false);
            assert_eq!((s.core, s.outer), (f.core, f.outer), "pixel {i}: our heights");
            for k in 0..3 {
                assert!((s.core_rgb[k] - f.core_rgb[k]).abs() <= 224.0 / 7.0 * 0.6 + 1.0, "pixel {i}: core {:?} vs {:?}", s.core_rgb, f.core_rgb);
            }
        }
    }

    /// The overall waveform, as fitted on the CDJ-3000: from the bottom up
    /// to max(b3, b4, b5), coloured by them normalised, brighter below b5's
    /// height and darker going up.
    #[test]
    fn overview_rgb_grows_from_the_bottom_in_the_previews_colour() {
        let prev = vec![u32::from_le_bytes([127, 64, 32, 0]); PREVIEW_N];
        let h = 0.93;                                   // 127 = 93 % of the strip
        assert!(overview_rgb(&prev, 0.5, 1000.0, h + 0.01).is_none(), "nothing above the column");
        let low = overview_rgb(&prev, 0.5, 1000.0, 0.0).unwrap();
        assert_eq!(low, rgb_norm([127.0, 64.0, 32.0], 240.0), "at the bottom, inside b5's height: 240");
        let high = overview_rgb(&prev, 0.5, 1000.0, h * 0.9).unwrap();
        assert!((high[0] - (244.0 - 127.0 * 0.9)).abs() < 1e-3, "near the top, above b5: darker ({high:?})");
        assert!(high[1] / high[0] - 64.0 / 127.0 < 1e-4, "one hue all the way up");
        // A blue-heavy column is mostly the brighter inner part.
        let blue = vec![u32::from_le_bytes([20, 40, 100, 0]); PREVIEW_N];
        let at = |y: f32| overview_rgb(&blue, 0.1, 1000.0, y).unwrap()[2];
        assert!(at(0.6) > 240.0 - 69.0 * 0.82 - 1.0, "below b5's height: the inner, brighter ramp");
    }

    /// The overall waveform's data: rekordbox's PWV4 bytes 3-5 when the track
    /// came with them, else made from our columns as the PWV4 we serve is.
    #[test]
    fn the_overview_takes_the_sticks_preview_else_ours() {
        let cols: Vec<u32> = (0..4000).map(|i| u32::from_le_bytes([(i % 200) as u8, 60, 20, 200])).collect();
        let ours = preview_for(None, &cols, 512, 44_100);
        assert_eq!(ours.len(), PREVIEW_N);
        assert!(ours.iter().any(|&v| v != 0), "made from our columns");
        let stick: Vec<[u8; 6]> = (0..PREVIEW_N).map(|i| [0, 0, 1, (i % 128) as u8, 50, 90]).collect();
        let rb = RbWave { detail: Vec::new(), preview: stick };
        let theirs = preview_for(Some(&rb), &cols, 512, 44_100);
        assert_eq!(theirs[5].to_le_bytes(), [5, 50, 90, 0], "bytes 3, 4, 5 of the stick's column");
        let empty = RbWave { detail: vec![[1, 1, 1, 1]], preview: Vec::new() };
        assert_eq!(preview_for(Some(&empty), &cols, 512, 44_100), ours, "a stick without a preview: ours");
    }

    /// The shader compiles and validates.
    #[test]
    fn the_waveform_shader_validates() {
        let src = crate::renderer::waveform_wgsl();
        let module = naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&src)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module).expect("valid WGSL");
    }
}

/// How long a full raster of each kind takes, per colour mode (the CPU
/// renderer, as the Pi Zero runs it): `cargo test --release -p opendeck-app
/// raster_cost -- --ignored --nocapture`.
#[cfg(test)]
mod bench {
    use super::*;
    use crate::renderer::{ColorMode, WaveformParams};

    #[test]
    #[ignore]
    fn raster_cost() {
        let lut = Lut::new();
        let n = 20_000usize;
        let mut cols: Vec<u32> = (0..n).map(|i| u32::from_le_bytes([(i * 7 % 230) as u8, (i * 13 % 120) as u8, (i * 5 % 50) as u8, (i * 3 % 255) as u8])).collect();
        let t0 = std::time::Instant::now();
        let rgb = own_rgb_columns(&cols);
        println!("RGB levels for {n} columns, once at load: {:.2} ms", t0.elapsed().as_secs_f64() * 1000.0);
        let t1 = std::time::Instant::now();
        let prev = preview_for(None, &cols, 512, 44_100);
        println!("overview RGB data, once at load: {:.2} ms", t1.elapsed().as_secs_f64() * 1000.0);
        cols.extend(rgb);   // as the renderers keep them (`rgb_off`, `ov_off`)
        cols.extend(prev);
        for (name, mode) in [("BLUE", ColorMode::Blue), ("3BAND", ColorMode::ThreeBand), ("RGB", ColorMode::Rgb)] {
            let p = WaveformParams {
                playhead_col: n as f32 / 2.0, cols_visible: 2900.0, num_cols: n as f32, screen_w: 1280.0, screen_h: 720.0,
                beat_anchor_col: 0.0, beat_period_cols: 40.0, downbeat_offset: 0.0, beats_per_bar: 4.0, beat2_phase_beats: 0.0,
                beat2_period_cols: 0.0, color_mode: mode as u32 as f32, wave_rect: [0.0, 200.0, 1280.0, 240.0], over_rect: [130.0, 600.0, 1020.0, 70.0],
                amp_gain: 1.0, dim_played: 0.0, cue_col: 10.0, loop_start_col: 0.0, loop_end_col: 0.0, slip_col: -1.0, style: 1.0,
                detail_off: 0.0, light: 0.0, rgb_off: n as f32, rgb_heights: 0.0, ov_off: 2.0 * n as f32,
            };
            let mut out = Vec::new();
            for (kind, rect, label) in [(Kind::Wave, PxRect { x: 0, y: 200, w: 1280, h: 240 }, "wave"), (Kind::Overview, PxRect { x: 130, y: 600, w: 1020, h: 70 }, "overview")] {
                let t0 = std::time::Instant::now();
                let reps = 20;
                for _ in 0..reps { draw(&lut, &p, &cols, kind, rect, &mut out); }
                println!("{name:5} {label:8} {:.2} ms a raster", t0.elapsed().as_secs_f64() * 1000.0 / reps as f64);
            }
        }
    }
}
