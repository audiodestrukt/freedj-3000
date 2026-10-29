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
    ground: Rgba, playhead: Rgba, cue: Rgba, loop_line: Rgba, loop_bg: Rgba, slip: Rgba,
    white: Rgba, band_low: Rgba, band_mid: Rgba, cyan: Rgba, strip_bg: Rgba, tick_hi: Rgba, down_hi: Rgba,
}

impl Palette {
    fn new(l: &Lut) -> Self {
        Self {
            ground:    l.rgb(3.0, 4.0, 6.0),          // matches screen.rs BG
            playhead:  l.rgb(232.0, 40.0, 40.0),      // red on the unit
            cue:       l.rgb(240.0, 138.0, 30.0),     // orange start-cue marker
            loop_line: l.rgb(250.0, 200.0, 40.0),     // amber loop in/out lines
            loop_bg:   l.rgb(16.0, 40.0, 66.0),       // tinted field across the looped span
            slip:      l.rgb(200.0, 110.0, 255.0),    // SLIP shadow playhead (violet)
            white:     l.rgb(244.0, 246.0, 248.0),
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

/// `bar_color` with the per-column part hoisted: everything that does not
/// depend on the row.
struct Bar {
    mode: u32,
    amp:  f32,          // min(c.w * gain, 1)
    // 3 BAND thresholds (dist < h[2] white, < h[1] mid, < h[0] low).
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
    #[inline(always)]
    fn at(&self, l: &Lut, pal: &Palette, dist: f32) -> Option<Rgba> {
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
    let pal = Palette::new(lut);
    match kind {
        Kind::Wave     => draw_wave(lut, &pal, p, cols, rect, out),
        Kind::Overview => draw_overview(lut, &pal, p, cols, rect, out),
    }
}

#[inline(always)]
fn pack(c: Rgba) -> u32 { u32::from_ne_bytes(c) }

/// The parameters the overview depends on, so a backend can skip the raster
/// when nothing it shows has moved (the playhead crosses a pixel every few
/// hundred ms; the enlarged waveform scrolls every frame).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OverviewKey {
    rect: PxRect,
    // Half-pixel positions: the shader tests |q.x - x| against fixed widths.
    playhead_x: i32, cue_x: i32, loop_x: (i32, i32), slip_x: i32,
    loop_cols: (i32, i32),
    color_mode: u32, dim: bool, amp_gain: f32, num_cols: f32,
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
        }
    }
}

#[inline(always)]
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
    } else if p.loop_end_col > p.loop_start_col
        && ((col_f - p.loop_start_col).abs() < g.cue_per_px || (col_f - p.loop_end_col).abs() < g.cue_per_px) {
        Column::Solid(pal.loop_line)
    } else if p.slip_col >= 0.0 && (col_f - p.slip_col).abs() < g.cue_per_px {
        Column::Solid(pal.slip)
    } else {
        let bg = if in_loop(p, col_f) { pal.loop_bg } else { pal.ground };
        if col_f < 0.0 || col_f >= p.num_cols || g.num_cols <= 0 || cols.is_empty() {
            Column::Field { bar: None, bg, tick: None }
        } else {
            let col_lo = col_f as usize;
            let col_hi = (col_lo + 1).min((g.num_cols - 1) as usize);
            let c = mix4(unpack(cols[col_lo.min(cols.len() - 1)]), unpack(cols[col_hi.min(cols.len() - 1)]), col_f - col_lo as f32);
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
            Column::Field { bar: Some(Bar::new(c, p)), bg, tick }
        }
    }
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
                *px = pack(if beat_x < 3.0 || beat_x > pixels_per_beat - 3.0 { pal.cyan } else { pal.strip_bg });
            }
            continue;
        }
        for (px, column) in row.iter_mut().zip(columns) {
            *px = pack(match column {
                Column::Solid(c) => *c,
                Column::Field { bar, bg, tick } => match bar.as_ref().and_then(|b| b.at(l, pal, dist)) {
                    Some(c) => c,
                    None => match tick {
                        Some((is_down, len)) if yy < *len || yy > g.wave_h - *len => if *is_down { pal.down_hi } else { pal.tick_hi },
                        _ => *bg,
                    },
                },
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
pub fn playhead_rgb(lut: &Lut) -> [u8; 4] { Palette::new(lut).playhead }

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
    let ph = pack(playhead_rgb(lut));
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
    let pal = Palette::new(lut);
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
        out.fill(pack(pal.ground));
        return;
    }
    let num_cols = p.num_cols as usize;
    let ph_x  = r[0] + p.playhead_col / p.num_cols * r[2];
    let cue_x = r[0] + p.cue_col / p.num_cols * r[2];
    let cols_per_px = p.num_cols / r[2];
    let n = ((cols_per_px as u32) + 1).min(64) as usize;
    let dim = p.dim_played > 0.5;

    // Per column: a solid marker, or (bar, field, played) for the rows.
    enum Over { Solid(Rgba), Body { bar: Bar, field: Rgba, played: bool } }
    let columns: Vec<Over> = (0..w).map(|x| {
        let qx = rect.x as f32 + x as f32 + 0.5;
        let sx = (qx - r[0]) / r[2];
        if (qx - ph_x).abs() < 1.0 { return Over::Solid(pal.white); }
        if (qx - cue_x).abs() < 1.5 { return Over::Solid(pal.cue); }
        if p.loop_end_col > p.loop_start_col {
            let ls_x = r[0] + p.loop_start_col / p.num_cols * r[2];
            let le_x = r[0] + p.loop_end_col   / p.num_cols * r[2];
            if (qx - ls_x).abs() < 1.5 || (qx - le_x).abs() < 1.5 { return Over::Solid(pal.loop_line); }
        }
        if p.slip_col >= 0.0 {
            let sl_x = r[0] + p.slip_col / p.num_cols * r[2];
            if (qx - sl_x).abs() < 1.5 { return Over::Solid(pal.slip); }
        }
        let col_here = sx * p.num_cols;
        let field = if in_loop(p, col_here) { pal.loop_bg } else { pal.ground };
        // Peak of each band over the columns this pixel covers.
        let c0 = (sx * p.num_cols).max(0.0) as usize;
        let mut c = [0.0f32; 4];
        for i in 0..n {
            let idx = (c0 + i).min(num_cols - 1).min(cols.len() - 1);
            let u = unpack(cols[idx]);
            for k in 0..4 { c[k] = c[k].max(u[k]); }
        }
        Over::Body { bar: Bar::new(c, p), field, played: qx < ph_x && dim }
    }).collect();

    for y in 0..h {
        let qy = rect.y as f32 + y as f32 + 0.5;
        let dist = ((qy - r[1]) / r[3] - 0.5).abs() * 2.0;
        let row = &mut out[y * w..(y + 1) * w];
        for (px, column) in row.iter_mut().zip(&columns) {
            *px = pack(match column {
                Over::Solid(c) => *c,
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
            dim_played: 0.0, cue_col: 10.0, loop_start_col: 0.0, loop_end_col: 0.0, slip_col: -1.0, _pad: [0.0; 2],
        }
    }

    fn px(out: &[u32], w: usize, x: usize, y: usize) -> [u8; 4] { out[y * w + x].to_ne_bytes() }

    #[test]
    fn playhead_cue_and_bars_land_where_the_shader_puts_them() {
        let lut = Lut::new(); let pal = Palette::new(&lut);
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

    #[test]
    fn overview_marks_the_playhead_and_fills_the_body() {
        let lut = Lut::new(); let pal = Palette::new(&lut);
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

    #[test]
    fn empty_rects_are_rejected() {
        assert!(PxRect::from_f32([0.0, 0.0, 0.0, 0.0]).is_none());
        assert!(PxRect::from_f32([f32::INFINITY, 0.0, 10.0, 10.0]).is_none());
    }
}
