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
//! path makes, to land on the same values.

use crate::renderer::WaveformParams;

/// Which of the two rects to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Wave, Overview }

/// An integer pixel rect, the rounding the backend applies to the float rect.
#[derive(Clone, Copy, Debug)]
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
    let enc = if lin <= 0.003_130_8 { lin * 12.92 } else { 1.055 * lin.powf(1.0 / 2.4) - 0.055 };
    (enc * 255.0 + 0.5) as u8
}

struct Lut([u8; 1024]);
impl Lut {
    fn new() -> Self {
        let mut t = [0u8; 1024];
        for (i, v) in t.iter_mut().enumerate() { *v = round_trip(i as f32 * 255.0 / 1023.0); }
        Lut(t)
    }
    /// `v` in author units 0..255.
    #[inline] fn at(&self, v: f32) -> u8 { self.0[(v.clamp(0.0, 255.0) * (1023.0 / 255.0) + 0.5) as usize] }
}

thread_local! { static LUT: Lut = Lut::new(); }

fn srgb(r: f32, g: f32, b: f32) -> Rgba { LUT.with(|l| [l.at(r), l.at(g), l.at(b), 255]) }

// The shader's palette, in its order.
fn ground() -> Rgba     { srgb(3.0, 4.0, 6.0) }
fn playhead() -> Rgba   { srgb(232.0, 40.0, 40.0) }
fn cue_color() -> Rgba  { srgb(240.0, 138.0, 30.0) }
fn loop_color() -> Rgba { srgb(250.0, 200.0, 40.0) }
fn loop_bg() -> Rgba    { srgb(16.0, 40.0, 66.0) }
fn slip_color() -> Rgba { srgb(200.0, 110.0, 255.0) }
fn white() -> Rgba      { srgb(244.0, 246.0, 248.0) }
fn band_low() -> Rgba   { srgb(58.0, 123.0, 240.0) }
fn band_mid() -> Rgba   { srgb(240.0, 160.0, 48.0) }
fn cyan() -> Rgba       { srgb(0.0, 216.0, 216.0) }
fn strip_bg() -> Rgba   { srgb(0.0, 22.0, 40.0) }
fn tick_hi() -> Rgba    { srgb(230.0, 232.0, 236.0) }
fn down_hi() -> Rgba    { srgb(255.0, 60.0, 60.0) }

const B2_STRIP_PX: f32 = 22.0;

#[inline]
fn unpack(v: u32) -> [f32; 4] {
    [(v & 0xFF) as f32 / 255.0, ((v >> 8) & 0xFF) as f32 / 255.0,
     ((v >> 16) & 0xFF) as f32 / 255.0, ((v >> 24) & 0xFF) as f32 / 255.0]
}

#[inline]
fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// `bar_color` with the per-column part hoisted: everything that does not
/// depend on the row.
struct Bar {
    mode: u32,
    amp:  f32,          // min(c.w * gain, 1)
    // 3 BAND thresholds (dist < h.z white, < h.y mid, < h.x low).
    h: [f32; 3],
    // RGB / BLUE: base colour in author units, scaled by the row's shade.
    base: [f32; 3],
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
        Self { mode, amp, h, base }
    }

    /// Colour at normalised distance `dist` from the centre line, or None
    /// outside the bar.
    #[inline]
    fn at(&self, dist: f32) -> Option<Rgba> {
        if self.mode == 1 {
            if dist < self.h[2] { return Some(white()); }
            if dist < self.h[1] { return Some(band_mid()); }
            if dist < self.h[0] { return Some(band_low()); }
            return None;
        }
        if dist >= self.amp { return None; }
        let shade = 1.0 - dist / (self.amp + 0.001) * 0.3;
        Some(srgb(self.base[0] * shade, self.base[1] * shade, self.base[2] * shade))
    }
}

/// What a pixel column of the enlarged waveform is, before the rows.
enum Column {
    Solid(Rgba),
    Field { bar: Option<Bar>, bg: Rgba, tick: Option<(bool, f32)> },   // tick: (is_down, len px)
}

/// Rasterise `kind`'s rect into `out` (RGBA8, `rect.w * 4` bytes per row).
pub fn draw(p: &WaveformParams, cols: &[u32], kind: Kind, rect: PxRect, out: &mut Vec<u8>) {
    let (w, h) = (rect.w as usize, rect.h as usize);
    out.clear();
    out.resize(w * h * 4, 0);
    match kind {
        Kind::Wave     => draw_wave(p, cols, rect, out),
        Kind::Overview => draw_overview(p, cols, rect, out),
    }
}

#[inline]
fn in_loop(p: &WaveformParams, col: f32) -> bool {
    p.loop_end_col > p.loop_start_col && col >= p.loop_start_col && col < p.loop_end_col
}

#[inline]
fn fill_col(out: &mut [u8], w: usize, x: usize, rows: std::ops::Range<usize>, c: Rgba) {
    for y in rows { let i = (y * w + x) * 4; out[i..i + 4].copy_from_slice(&c); }
}

fn draw_wave(p: &WaveformParams, cols: &[u32], rect: PxRect, out: &mut [u8]) {
    let r = p.wave_rect;
    let (w, h) = (rect.w as usize, rect.h as usize);
    let strip = p.beat2_period_cols > 0.0;
    let wave_h = r[3] - if strip { B2_STRIP_PX } else { 0.0 };
    let half = p.cols_visible * 0.5;
    let cue_per_px = p.cols_visible / r[2];
    let cols_per_px = p.cols_visible / r[2];
    let num_cols = p.num_cols as i64;
    let bg_plain = ground();

    for x in 0..w {
        let qx = rect.x as f32 + x as f32 + 0.5;
        let sx = (qx - r[0]) / r[2];
        // Playhead line at the horizontal centre — red, full height.
        if (qx - (r[0] + r[2] * 0.5)).abs() < 1.5 {
            fill_col(out, w, x, 0..h, playhead());
            continue;
        }
        let col_f = (p.playhead_col - half) + sx * p.cols_visible;
        let column = if (col_f - p.cue_col).abs() < cue_per_px {
            Column::Solid(cue_color())
        } else if p.loop_end_col > p.loop_start_col
            && ((col_f - p.loop_start_col).abs() < cue_per_px || (col_f - p.loop_end_col).abs() < cue_per_px) {
            Column::Solid(loop_color())
        } else if p.slip_col >= 0.0 && (col_f - p.slip_col).abs() < cue_per_px {
            Column::Solid(slip_color())
        } else {
            let bg = if in_loop(p, col_f) { loop_bg() } else { bg_plain };
            if col_f < 0.0 || col_f >= p.num_cols || num_cols <= 0 {
                Column::Field { bar: None, bg, tick: None }
            } else {
                let col_lo = col_f as usize;
                let col_hi = (col_lo + 1).min((num_cols - 1) as usize);
                let c = mix4(unpack(cols[col_lo.min(cols.len() - 1)]), unpack(cols[col_hi.min(cols.len() - 1)]), col_f - col_lo as f32);
                let tick = if p.beat_period_cols > 0.0 {
                    let rel      = col_f - p.beat_anchor_col;
                    let beat_pos = ((rel % p.beat_period_cols) + p.beat_period_cols) % p.beat_period_cols;
                    let beat_num = (rel / p.beat_period_cols).floor();
                    let bpb      = p.beats_per_bar;
                    let adjusted = ((beat_num + p.downbeat_offset) % bpb + bpb) % bpb;
                    let is_down  = adjusted < 0.5;
                    let tick_w   = if is_down { 2.5 } else { 1.5 } * cols_per_px;
                    (beat_pos < tick_w).then(|| (is_down, if is_down { 0.10 } else { 0.055 } * wave_h))
                } else { None };
                Column::Field { bar: Some(Bar::new(c, p)), bg, tick }
            }
        };
        // External-deck beat strip along the bottom edge (disabled today —
        // beat2_period_cols is 0 — but kept in step with the shader, where it
        // is tested before everything else in the column).
        let strip_px = |qy: f32| -> Option<Rgba> {
            if !(strip && qy > r[1] + r[3] - B2_STRIP_PX) { return None; }
            let pixels_per_beat = p.beat2_period_cols * r[2] / p.cols_visible;
            let phase_px = p.beat2_phase_beats * pixels_per_beat;
            let beat_x = (((qx - r[0]) + phase_px) % pixels_per_beat + pixels_per_beat) % pixels_per_beat;
            Some(if beat_x < 3.0 || beat_x > pixels_per_beat - 3.0 { cyan() } else { strip_bg() })
        };
        match column {
            Column::Solid(c) => {
                if !strip { fill_col(out, w, x, 0..h, c); continue; }
                for y in 0..h {
                    let qy = rect.y as f32 + y as f32 + 0.5;
                    let i = (y * w + x) * 4;
                    out[i..i + 4].copy_from_slice(&strip_px(qy).unwrap_or(c));
                }
            }
            Column::Field { bar, bg, tick } => {
                for y in 0..h {
                    let qy = rect.y as f32 + y as f32 + 0.5;
                    let i = (y * w + x) * 4;
                    let px: Rgba = match strip_px(qy) {
                        Some(c) => c,
                        None => {
                            let sy   = (qy - r[1]) / wave_h;
                            let dist = (sy - 0.5).abs() * 2.0;
                            match bar.as_ref().and_then(|b| b.at(dist)) {
                                Some(c) => c,
                                None => match tick {
                                    Some((is_down, len)) if (qy - r[1]) < len || (qy - r[1]) > wave_h - len =>
                                        if is_down { down_hi() } else { tick_hi() },
                                    _ => bg,
                                },
                            }
                        }
                    };
                    out[i..i + 4].copy_from_slice(&px);
                }
            }
        }
    }
}

fn draw_overview(p: &WaveformParams, cols: &[u32], rect: PxRect, out: &mut [u8]) {
    let r = p.over_rect;
    let (w, h) = (rect.w as usize, rect.h as usize);
    if p.num_cols <= 0.0 || cols.is_empty() {
        for x in 0..w { fill_col(out, w, x, 0..h, ground()); }
        return;
    }
    let num_cols = p.num_cols as usize;
    let ph_x  = r[0] + p.playhead_col / p.num_cols * r[2];
    let cue_x = r[0] + p.cue_col / p.num_cols * r[2];
    let cols_per_px = p.num_cols / r[2];
    let n = ((cols_per_px as u32) + 1).min(64) as usize;
    let dim = p.dim_played > 0.5;

    for x in 0..w {
        let qx = rect.x as f32 + x as f32 + 0.5;
        let sx = (qx - r[0]) / r[2];
        if (qx - ph_x).abs() < 1.0 { fill_col(out, w, x, 0..h, white()); continue; }
        if (qx - cue_x).abs() < 1.5 { fill_col(out, w, x, 0..h, cue_color()); continue; }
        if p.loop_end_col > p.loop_start_col {
            let ls_x = r[0] + p.loop_start_col / p.num_cols * r[2];
            let le_x = r[0] + p.loop_end_col   / p.num_cols * r[2];
            if (qx - ls_x).abs() < 1.5 || (qx - le_x).abs() < 1.5 { fill_col(out, w, x, 0..h, loop_color()); continue; }
        }
        if p.slip_col >= 0.0 {
            let sl_x = r[0] + p.slip_col / p.num_cols * r[2];
            if (qx - sl_x).abs() < 1.5 { fill_col(out, w, x, 0..h, slip_color()); continue; }
        }
        let col_here = sx * p.num_cols;
        let field = if in_loop(p, col_here) { loop_bg() } else { ground() };
        // Peak of each band over the columns this pixel covers.
        let c0 = (sx * p.num_cols).max(0.0) as usize;
        let mut c = [0.0f32; 4];
        for i in 0..n {
            let idx = (c0 + i).min(num_cols - 1).min(cols.len() - 1);
            let u = unpack(cols[idx]);
            for k in 0..4 { c[k] = c[k].max(u[k]); }
        }
        let bar = Bar::new(c, p);
        let played = qx < ph_x && dim;
        for y in 0..h {
            let qy = rect.y as f32 + y as f32 + 0.5;
            let sy = (qy - r[1]) / r[3];
            let dist = (sy - 0.5).abs() * 2.0;
            let px = match bar.at(dist) {
                Some(c) if played => dim_bar(c),
                Some(c) => c,
                None => field,
            };
            let i = (y * w + x) * 4;
            out[i..i + 4].copy_from_slice(&px);
        }
    }
}

/// REMAIN mode: `bar * 0.18` in linear space, then encoded like the rest.
fn dim_bar(c: Rgba) -> Rgba {
    let f = |v: u8| -> u8 {
        let lin = srgb_decode(v as f32 / 255.0) * 0.18;
        let enc = if lin <= 0.003_130_8 { lin * 12.92 } else { 1.055 * lin.powf(1.0 / 2.4) - 0.055 };
        (enc * 255.0 + 0.5) as u8
    };
    [f(c[0]), f(c[1]), f(c[2]), 255]
}

fn srgb_decode(e: f32) -> f32 {
    if e <= 0.040_45 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) }
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
            dim_played: 0.0, cue_col: 10.0, loop_start_col: 0.0, loop_end_col: 0.0, slip_col: -1.0, _pad: [0.0; 2],
        }
    }

    fn px(out: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4; [out[i], out[i + 1], out[i + 2], out[i + 3]]
    }

    #[test]
    fn playhead_cue_and_bars_land_where_the_shader_puts_them() {
        let cols: Vec<u32> = (0..2000u32).map(|_| u32::from_le_bytes([80, 120, 200, 180])).collect();
        let p = params(2000, [100.0, 50.0, 800.0, 200.0], [0.0; 4]);
        let rect = PxRect::from_f32(p.wave_rect).unwrap();
        let mut out = Vec::new();
        draw(&p, &cols, Kind::Wave, rect, &mut out);
        let w = rect.w as usize;
        // Centre column is the red playhead, full height.
        assert_eq!(px(&out, w, 400, 0), playhead());
        assert_eq!(px(&out, w, 400, 199), playhead());
        // The middle row of a bar column is waveform blue, the top row is ground.
        let mid = px(&out, w, 200, 100);
        assert!(mid[2] > mid[0] && mid[2] > 100, "blue body, got {mid:?}");
        assert_eq!(px(&out, w, 200, 0), ground());
    }

    #[test]
    fn overview_marks_the_playhead_and_fills_the_body() {
        let cols: Vec<u32> = (0..5000u32).map(|_| u32::from_le_bytes([200, 200, 200, 255])).collect();
        let p = params(5000, [0.0; 4], [10.0, 500.0, 1000.0, 60.0]);
        let rect = PxRect::from_f32(p.over_rect).unwrap();
        let mut out = Vec::new();
        draw(&p, &cols, Kind::Overview, rect, &mut out);
        let w = rect.w as usize;
        assert_eq!(px(&out, w, 500, 5), white());          // playhead at the middle
        assert_ne!(px(&out, w, 300, 30), ground());         // body at the centre row
        assert_eq!(out.len(), 1000 * 60 * 4);
    }

    #[test]
    fn empty_rects_are_rejected() {
        assert!(PxRect::from_f32([0.0, 0.0, 0.0, 0.0]).is_none());
        assert!(PxRect::from_f32([f32::INFINITY, 0.0, 10.0, 10.0]).is_none());
    }
}
