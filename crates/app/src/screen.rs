//! Screen chrome — everything drawn with egui on top of the waveform pass.
//!
//! Layout is the Pioneer XDJ-1000MK2 normal playback screen.  Region
//! fractions and type sizes were measured from a photograph of the unit
//! (`reference/pioneer/xdj-1000mk2-photo.png`), cross-checked against the
//! manual's part-names diagram (DRI1396B p.16).  The two waveforms are drawn
//! by the shader into the rects this module hands out; everything else is
//! painted here.
//!
//! Takes a [`DeckSnapshot`] and draws; holds no deck state of its own.

use crate::snapshot::DeckSnapshot;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};

// ── Palette ───────────────────────────────────────────────────────────────────
// Sampled from the photo: black ground, slate-blue touch keys, a navy title
// bar, white readouts.  Colour carries state only.

const BG:      Color32 = Color32::from_rgb(0x03, 0x04, 0x06);
const BAR:     Color32 = Color32::from_rgb(0x1a, 0x27, 0x3d);   // title bar
const KEY:     Color32 = Color32::from_rgb(0x3a, 0x46, 0x58);   // touch key face
const KEY_LO:  Color32 = Color32::from_rgb(0x2a, 0x33, 0x40);   // key face, dim
const LINE:    Color32 = Color32::from_rgb(0x12, 0x16, 0x1c);   // gaps between keys
const TEXT:    Color32 = Color32::from_rgb(0xf6, 0xf7, 0xf8);
const DIM:     Color32 = Color32::from_rgb(0x9a, 0xa3, 0xae);
const FAINT:   Color32 = Color32::from_rgb(0x4a, 0x52, 0x5c);
const BLUE:    Color32 = Color32::from_rgb(0x2f, 0x7f, 0xe0);
const ORANGE:  Color32 = Color32::from_rgb(0xf0, 0x8a, 0x1e);
const RED:     Color32 = Color32::from_rgb(0xe0, 0x2a, 0x2a);
const GREEN:   Color32 = Color32::from_rgb(0x3c, 0xc8, 0x50);

// ── Layout ────────────────────────────────────────────────────────────────────

/// Screen regions in logical points, as fractions of the screen so the same
/// proportions hold on the 7" panel and in a desktop window.
pub struct Layout {
    pub screen:   Rect,
    // left column
    pub logo:     Rect,
    pub link:     Rect,
    pub usb:      Rect,
    pub player:   Rect,
    pub slip:     Rect,
    // top
    pub keys:     Rect,
    pub title:    Rect,
    // middle
    pub phase:    Rect,   // phase meter boxes
    pub bars:     Rect,   // "--.- Bars" readouts
    pub wave:     Rect,   // enlarged waveform (shader)
    pub cueloop:  Rect,   // DELETE / MEMORY
    pub call:     Rect,   // ◀ / ▶
    pub zoom:     Rect,   // ZOOM – GRID pill
    // info row
    pub track:    Rect,
    pub acue:     Rect,
    pub time:     Rect,
    pub tempo:    Rect,
    pub sync:     Rect,
    pub master:   Rect,
    // bottom
    pub overview: Rect,   // playing address (shader)
    pub needle:   Rect,   // NEEDLE SEARCH bar
    pub range:    Rect,   // ±N badge
    pub bpm:      Rect,
}

pub fn layout(size: Vec2) -> Layout {
    let w = size.x;
    let h = size.y;
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(Pos2::new(x0 * w, y0 * h), Pos2::new(x1 * w, y1 * h))
    };
    let lc = 0.092;   // left column right edge
    let rc = 0.805;   // right column left edge
    Layout {
        screen:   r(0.0, 0.0, 1.0, 1.0),
        logo:     r(0.004, 0.010, lc - 0.004, 0.225),
        link:     r(0.004, 0.240, lc - 0.004, 0.440),
        usb:      r(0.004, 0.452, lc - 0.004, 0.655),
        player:   r(0.004, 0.690, lc - 0.004, 0.830),
        slip:     r(0.004, 0.865, lc - 0.004, 0.985),

        keys:     r(lc + 0.006, 0.010, 0.996, 0.120),
        title:    r(lc + 0.006, 0.130, 0.996, 0.200),

        phase:    r(0.305, 0.232, 0.595, 0.308),
        bars:     r(0.610, 0.232, rc,    0.308),
        wave:     r(0.190, 0.340, 0.785, 0.660),
        cueloop:  r(rc + 0.006, 0.318, 0.996, 0.440),
        call:     r(rc + 0.006, 0.478, 0.996, 0.600),
        zoom:     r(rc + 0.030, 0.622, 0.972, 0.655),

        track:    r(0.125, 0.690, 0.215, 0.830),
        acue:     r(0.230, 0.768, 0.320, 0.808),
        time:     r(0.350, 0.690, 0.600, 0.830),
        tempo:    r(0.630, 0.690, 0.790, 0.830),
        sync:     r(rc + 0.006, 0.692, 0.897, 0.808),
        master:   r(0.903, 0.692, 0.996, 0.808),

        overview: r(0.127, 0.846, 0.776, 0.934),
        needle:   r(0.127, 0.948, 0.776, 0.982),
        range:    r(0.806, 0.853, 0.867, 0.892),
        bpm:      r(0.885, 0.846, 0.996, 0.945),
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────────

pub fn draw(ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout) {
    let p = ctx.layer_painter(egui::LayerId::background());
    let h = lay.screen.height();

    // Ground everything except the two shader rects.  egui paints after the
    // waveform pass, so those must be left alone.
    for r in cover(lay.screen, lay.wave, lay.overview) {
        p.rect_filled(r, 0.0, BG);
    }

    draw_left(&p, lay, h);
    draw_keys(&p, lay, h);
    draw_title(&p, snap, lay, h);
    draw_phase(&p, snap, lay, h);
    draw_wave_side(&p, lay, h);
    draw_info(&p, snap, lay, h);
    draw_bottom(&p, snap, lay, h);
}

/// Rects that tile `outer` minus two holes `a` and `b` (a above b, non-overlapping).
fn cover(outer: Rect, a: Rect, b: Rect) -> Vec<Rect> {
    let mut v = Vec::with_capacity(7);
    let band = |y0: f32, y1: f32| Rect::from_min_max(Pos2::new(outer.min.x, y0), Pos2::new(outer.max.x, y1));
    v.push(band(outer.min.y, a.min.y));
    v.push(Rect::from_min_max(Pos2::new(outer.min.x, a.min.y), Pos2::new(a.min.x, a.max.y)));
    v.push(Rect::from_min_max(Pos2::new(a.max.x, a.min.y), Pos2::new(outer.max.x, a.max.y)));
    v.push(band(a.max.y, b.min.y));
    v.push(Rect::from_min_max(Pos2::new(outer.min.x, b.min.y), Pos2::new(b.min.x, b.max.y)));
    v.push(Rect::from_min_max(Pos2::new(b.max.x, b.min.y), Pos2::new(outer.max.x, b.max.y)));
    v.push(band(b.max.y, outer.max.y));
    v
}

fn text(p: &egui::Painter, pos: Pos2, a: Align2, s: impl ToString, size: f32, c: Color32) {
    p.text(pos, a, s, FontId::proportional(size), c);
}

/// Touch key: slate face, main label, optional "– SUB" line.
fn key(p: &egui::Painter, r: Rect, main: &str, sub: &str, h: f32, lit: bool) {
    p.rect_filled(r, 2.0, if lit { BLUE } else { KEY });
    let big = h * 0.032;
    if sub.is_empty() {
        text(p, r.center(), Align2::CENTER_CENTER, main, big, TEXT);
    } else {
        text(p, Pos2::new(r.center().x, r.center().y - h * 0.013), Align2::CENTER_CENTER, main, big, TEXT);
        text(p, Pos2::new(r.center().x, r.center().y + h * 0.020), Align2::CENTER_CENTER, format!("– {sub}"), h * 0.019, TEXT);
    }
}

/// Caption with bracket ticks either side, as over CUE/LOOP and CALL.
fn bracket_caption(p: &egui::Painter, r: Rect, s: &str, h: f32) {
    let y = r.min.y - h * 0.016;
    text(p, Pos2::new(r.center().x, y), Align2::CENTER_CENTER, s, h * 0.018, DIM);
    let tw = s.len() as f32 * h * 0.010;
    for (x0, x1) in [(r.min.x, r.center().x - tw), (r.center().x + tw, r.max.x)] {
        p.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], Stroke::new(1.0, FAINT));
        p.line_segment([Pos2::new(x0, y), Pos2::new(x0, y + 4.0)], Stroke::new(1.0, FAINT));
        p.line_segment([Pos2::new(x1, y), Pos2::new(x1, y + 4.0)], Stroke::new(1.0, FAINT));
    }
}

fn fmt_time(secs: f64) -> String {
    let m  = (secs / 60.0).floor();
    let s  = (secs - m * 60.0).floor();
    let ms = ((secs - m * 60.0 - s) * 1000.0).floor();
    format!("{:02.0}:{:02.0}.{:03.0}", m, s, ms)
}

// ── Left column ───────────────────────────────────────────────────────────────

fn draw_left(p: &egui::Painter, lay: &Layout, h: f32) {
    // rekordbox-style logo cell.
    let l = lay.logo;
    p.rect_filled(l, 2.0, KEY_LO);
    let c = Pos2::new(l.center().x, l.center().y - h * 0.02);
    p.circle_stroke(c, h * 0.022, Stroke::new(2.0, TEXT));
    p.circle_filled(c, h * 0.008, TEXT);
    text(p, Pos2::new(l.center().x, l.max.y - h * 0.022), Align2::CENTER_CENTER, "freedj", h * 0.018, TEXT);

    key(p, lay.link, "LINK", "", h, false);
    key(p, lay.usb,  "FILE", "", h, false);
    // Green bar down the left edge of the selected source only.
    p.rect_filled(Rect::from_min_size(lay.usb.min, Vec2::new(h * 0.008, lay.usb.height())), 0.0, GREEN);

    // PLAYER n — dim on the unit unless lit by link state.
    let pl = lay.player;
    text(p, Pos2::new(pl.center().x, pl.min.y + h * 0.018), Align2::CENTER_CENTER, "PLAYER", h * 0.018, FAINT);
    text(p, Pos2::new(pl.center().x, pl.center().y + h * 0.020), Align2::CENTER_CENTER, "1", h * 0.075, FAINT);

    key(p, lay.slip, "SLIP", "", h, false);
}

// ── Touch-key row ─────────────────────────────────────────────────────────────

fn draw_keys(p: &egui::Painter, lay: &Layout, h: f32) {
    let r = lay.keys;
    let keys = [("BROWSE", "SEARCH"), ("TAG LIST", ""), ("INFO", "LINK INFO"), ("MENU", "UTILITY"), ("PERFORM", "")];
    let gap = 2.0;
    let kw = (r.width() - gap * (keys.len() as f32 - 1.0)) / keys.len() as f32;
    for (i, (m, s)) in keys.iter().enumerate() {
        let x0 = r.min.x + i as f32 * (kw + gap);
        let kr = Rect::from_min_max(Pos2::new(x0, r.min.y), Pos2::new(x0 + kw, r.max.y));
        key(p, kr, m, s, h, false);
    }
    // PERFORM's expand glyph, top-right.
    let last = Rect::from_min_max(Pos2::new(r.max.x - kw, r.min.y), r.max);
    let c = Pos2::new(last.max.x - h * 0.020, last.min.y + h * 0.020);
    let d = h * 0.009;
    let st = Stroke::new(1.5, TEXT);
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x + d, c.y - d)], st);
    p.line_segment([Pos2::new(c.x + d, c.y - d), Pos2::new(c.x + d * 0.2, c.y - d)], st);
    p.line_segment([Pos2::new(c.x + d, c.y - d), Pos2::new(c.x + d, c.y - d * 0.2)], st);
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x - d * 0.2, c.y + d)], st);
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x - d, c.y + d * 0.2)], st);
    let _ = LINE;
}

// ── Title bar ─────────────────────────────────────────────────────────────────

fn draw_title(p: &egui::Painter, snap: &DeckSnapshot, lay: &Layout, h: f32) {
    let r = lay.title;
    p.rect_filled(r, 0.0, BAR);
    text(p, Pos2::new(r.min.x + h * 0.02, r.center().y), Align2::LEFT_CENTER, format!("♪ {}", snap.title), h * 0.036, TEXT);
    // Key badge + key name, right.  Detection is not implemented; the slot
    // draws its glyph and a dash.
    let kx = r.max.x - h * 0.02;
    text(p, Pos2::new(kx, r.center().y), Align2::RIGHT_CENTER, "--", h * 0.034, TEXT);
    let badge = Rect::from_center_size(Pos2::new(kx - h * 0.075, r.center().y), Vec2::new(h * 0.030, h * 0.030));
    p.rect_filled(badge, 2.0, KEY);
    text(p, badge.center(), Align2::CENTER_CENTER, "b#", h * 0.017, TEXT);
}

// ── Phase meter + beat countdown ──────────────────────────────────────────────

fn draw_phase(p: &egui::Painter, snap: &DeckSnapshot, lay: &Layout, h: f32) {
    let r = lay.phase;
    let gap = h * 0.006;
    let row_h = (r.height() - gap) / 2.0;
    let cell_w = (r.width() - 3.0 * gap) / 4.0;
    let ours = snap.beat_in_bar();
    let has_master = snap.beat2_bpm > 0.0;
    // Master's beat in bar: we only know its phase, so advance a counter from
    // our own beat as the reference cell (same cell when beatmatched).
    let master_beat = ours;

    for i in 0..4u8 {
        let x = r.min.x + i as f32 * (cell_w + gap);
        let top = Rect::from_min_size(Pos2::new(x, r.min.y),               Vec2::new(cell_w, row_h));
        let bot = Rect::from_min_size(Pos2::new(x, r.min.y + row_h + gap), Vec2::new(cell_w, row_h));
        // Top row: master player, orange outline; lit solid on its beat.
        if has_master && master_beat == Some(i + 1) {
            p.rect_filled(top, 1.0, ORANGE);
        } else {
            p.rect_stroke(top, 1.0, Stroke::new(1.0, if has_master { ORANGE } else { Color32::from_rgb(0x7a, 0x4a, 0x16) }));
        }
        // Bottom row: this player, blue outline; current beat solid.
        if ours == Some(i + 1) {
            p.rect_filled(bot, 1.0, BLUE);
        } else {
            p.rect_stroke(bot, 1.0, Stroke::new(1.0, BLUE));
        }
    }
    // Master phase marker across the bar — the divergence you read.
    if has_master {
        let cell = master_beat.unwrap_or(1) as f32 - 1.0;
        let x = r.min.x + cell * (cell_w + gap) + snap.beat2_phase_beats * cell_w;
        p.line_segment([Pos2::new(x, r.min.y - 2.0), Pos2::new(x, r.min.y + row_h + 2.0)], Stroke::new(2.0, TEXT));
    }

    // "--.- Bars" readouts: orange = to nearest stored cue (none yet),
    // blue = bars.beats to the next downbeat.
    let b = lay.bars;
    let fs = h * 0.026;
    let blue = match (snap.beat_in_bar(), snap.beat_phase()) {
        (Some(bb), Some(_)) => format!("--.{}", 4 - bb),
        _ => "--.-".into(),
    };
    text(p, Pos2::new(b.min.x, b.min.y + row_h / 2.0), Align2::LEFT_CENTER, "--.-", fs, ORANGE);
    text(p, Pos2::new(b.min.x + h * 0.078, b.min.y + row_h / 2.0), Align2::LEFT_CENTER, "Bars", h * 0.020, ORANGE);
    text(p, Pos2::new(b.min.x, b.min.y + row_h + gap + row_h / 2.0), Align2::LEFT_CENTER, blue, fs, BLUE);
    text(p, Pos2::new(b.min.x + h * 0.078, b.min.y + row_h + gap + row_h / 2.0), Align2::LEFT_CENTER, "Bars", h * 0.020, BLUE);
}

// ── Column right of the waveform ──────────────────────────────────────────────

fn draw_wave_side(p: &egui::Painter, lay: &Layout, h: f32) {
    let half = |r: Rect, i: usize| {
        let w = (r.width() - 2.0) / 2.0;
        Rect::from_min_size(Pos2::new(r.min.x + i as f32 * (w + 2.0), r.min.y), Vec2::new(w, r.height()))
    };
    bracket_caption(p, lay.cueloop, "CUE / LOOP", h);
    key(p, half(lay.cueloop, 0), "DELETE", "", h, false);
    key(p, half(lay.cueloop, 1), "MEMORY", "", h, false);

    bracket_caption(p, lay.call, "CALL", h);
    key(p, half(lay.call, 0), "◀", "", h, false);
    key(p, half(lay.call, 1), "▶", "", h, false);

    // ZOOM – GRID pill: the active mode is the blue half.
    let z = lay.zoom;
    let mid = z.center().x;
    p.rect_filled(Rect::from_min_max(z.min, Pos2::new(mid, z.max.y)), 2.0, BLUE);
    p.rect_filled(Rect::from_min_max(Pos2::new(mid, z.min.y), z.max), 2.0, KEY_LO);
    text(p, Pos2::new(z.min.x + z.width() * 0.25, z.center().y), Align2::CENTER_CENTER, "ZOOM", h * 0.018, TEXT);
    text(p, Pos2::new(z.min.x + z.width() * 0.75, z.center().y), Align2::CENTER_CENTER, "– GRID", h * 0.018, DIM);
}

// ── Info row ──────────────────────────────────────────────────────────────────

fn draw_info(p: &egui::Painter, snap: &DeckSnapshot, lay: &Layout, h: f32) {
    let big = h * 0.085;
    let cap = h * 0.019;
    let base_y = |r: Rect| r.max.y - h * 0.010;   // baseline the big readouts share

    // TRACK
    let t = lay.track;
    text(p, Pos2::new(t.min.x, t.min.y + h * 0.006), Align2::LEFT_TOP, "TRACK", cap, TEXT);
    text(p, Pos2::new(t.min.x, base_y(t)), Align2::LEFT_BOTTOM, "01", big, TEXT);

    // A.CUE — shown only when on (auto cue is on by default on the unit).
    // A.HOT CUE would sit above it and is hidden when off.
    let a = lay.acue;
    p.rect_stroke(a, 2.0, Stroke::new(1.0, TEXT));
    text(p, a.center(), Align2::CENTER_CENTER, "A.CUE", h * 0.018, TEXT);

    // Time, with QUANTIZE caption centred above and REMAIN at left when set.
    let tm = lay.time;
    let shown = if snap.remain_mode { snap.remaining_secs() } else { snap.elapsed_secs() };
    text(p, Pos2::new(tm.center().x, tm.min.y + h * 0.006), Align2::CENTER_TOP, "QUANTIZE : –", cap, ORANGE);
    if snap.remain_mode {
        text(p, Pos2::new(tm.min.x, tm.min.y + h * 0.006), Align2::LEFT_TOP, "REMAIN", cap, TEXT);
    }
    text(p, Pos2::new(tm.center().x, base_y(tm)), Align2::CENTER_BOTTOM, fmt_time(shown), big, TEXT);

    // TEMPO caption + MT pill, then the percentage.
    let te = lay.tempo;
    text(p, Pos2::new(te.min.x, te.min.y + h * 0.006), Align2::LEFT_TOP, "TEMPO", cap, TEXT);
    let mt = Rect::from_min_size(Pos2::new(te.min.x + h * 0.075, te.min.y + h * 0.004), Vec2::new(h * 0.042, h * 0.026));
    if snap.key_lock {
        p.rect_filled(mt, 2.0, RED);
        text(p, mt.center(), Align2::CENTER_CENTER, "MT", h * 0.018, TEXT);
    }
    let v = snap.tempo_percent();
    let s = if v.abs() < 0.005 { "0.00".to_string() } else { format!("{:+.2}", v) };
    text(p, Pos2::new(te.max.x - h * 0.030, base_y(te)), Align2::RIGHT_BOTTOM, s, big, TEXT);
    text(p, Pos2::new(te.max.x, base_y(te) - h * 0.008), Align2::RIGHT_BOTTOM, "%", h * 0.034, TEXT);

    key(p, lay.sync,   "SYNC",   "INST.D.", h, false);
    key(p, lay.master, "MASTER", "",        h, false);
}

// ── Bottom row ────────────────────────────────────────────────────────────────

fn draw_bottom(p: &egui::Painter, snap: &DeckSnapshot, lay: &Layout, h: f32) {
    // NEEDLE SEARCH bar under the overview.
    let n = lay.needle;
    p.rect_filled(n, 1.0, KEY_LO);
    text(p, n.center(), Align2::CENTER_CENTER, "NEEDLE SEARCH", h * 0.018, TEXT);
    // Cue marker triangles (none stored yet): a single start marker, as on the unit.
    let tri = |p: &egui::Painter, x: f32, y: f32| {
        p.add(egui::Shape::convex_polygon(
            vec![Pos2::new(x - 4.0, y + 4.0), Pos2::new(x + 4.0, y + 4.0), Pos2::new(x, y - 3.0)],
            ORANGE, Stroke::NONE));
    };
    tri(p, lay.overview.min.x + 3.0, lay.overview.max.y + h * 0.010);
    let _ = snap;

    // ±range badge.
    let rg = lay.range;
    p.rect_filled(rg, 2.0, RED);
    text(p, rg.center(), Align2::CENTER_CENTER, "±16", h * 0.024, TEXT);

    // BPM box: dark face, big integer part, smaller fraction, "BPM" caption.
    let b = lay.bpm;
    p.rect_filled(b, 2.0, KEY_LO);
    let (txt, col) = match (snap.bpm(), snap.beat_grid) {
        (Some(v), Some(g)) => (format!("{:.1}", v), if g.confidence >= 0.7 { TEXT } else { ORANGE }),
        _ => ("---.-".into(), DIM),
    };
    let dot = txt.find('.').unwrap_or(txt.len());
    let (ip, fp) = txt.split_at(dot);
    let base = b.max.y - h * 0.030;
    let ip_size = h * 0.060;
    text(p, Pos2::new(b.min.x + h * 0.012, base), Align2::LEFT_BOTTOM, ip, ip_size, col);
    text(p, Pos2::new(b.min.x + h * 0.012 + ip.len() as f32 * ip_size * 0.56, base), Align2::LEFT_BOTTOM, fp, h * 0.040, col);
    text(p, Pos2::new(b.max.x - h * 0.010, b.max.y - h * 0.008), Align2::RIGHT_BOTTOM, "BPM", h * 0.018, TEXT);
}
