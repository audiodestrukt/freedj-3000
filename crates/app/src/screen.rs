//! Screen chrome — everything drawn with egui on top of the waveform pass.
//!
//! Layout follows the Pioneer XDJ-1000MK2 *Normal playback screen (display
//! part)*, manual DRI1396B p.16, callouts 1–24.  Region fractions were measured
//! from that diagram.  The two waveforms are drawn by the shader into the
//! rects this module hands out; everything else is painted here.
//!
//! Takes a [`DeckSnapshot`] and draws; holds no deck state of its own.

use crate::snapshot::DeckSnapshot;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Vec2};

// ── Palette ───────────────────────────────────────────────────────────────────
// The XDJ ground is near-black; touch keys are dark slate; colour is reserved
// for state — blue for engaged, orange/red for the tempo range and MASTER.

const BG:      Color32 = Color32::from_rgb(0x05, 0x07, 0x0a);
const BAR:     Color32 = Color32::from_rgb(0x12, 0x18, 0x21);   // title bar ground
const KEY:     Color32 = Color32::from_rgb(0x1f, 0x27, 0x33);   // touch key face
const KEY_HI:  Color32 = Color32::from_rgb(0x2a, 0x35, 0x45);
const LINE:    Color32 = Color32::from_rgb(0x2e, 0x36, 0x42);
const TEXT:    Color32 = Color32::from_rgb(0xf4, 0xf6, 0xf8);
const DIM:     Color32 = Color32::from_rgb(0x8c, 0x96, 0xa4);
const BLUE:    Color32 = Color32::from_rgb(0x2f, 0x6f, 0xd6);
const ORANGE:  Color32 = Color32::from_rgb(0xf0, 0x8a, 0x1e);
const RED:     Color32 = Color32::from_rgb(0xd8, 0x2c, 0x2c);
const CYAN:    Color32 = Color32::from_rgb(0x00, 0xd8, 0xd8);

// ── Layout ────────────────────────────────────────────────────────────────────

/// Screen regions in logical points, as fractions of the screen so the same
/// proportions hold on the 7" panel and in a desktop window.
pub struct Layout {
    pub screen:    Rect,
    pub keys:      Rect,   // BROWSE / TAG LIST / INFO / MENU / PERFORM
    pub title:     Rect,   // ♪ track name … key
    pub source:    Rect,   // LINK / USB column
    pub phase:     Rect,   // MASTER PLAYER + phase meter + beat countdown
    pub wave:      Rect,   // enlarged waveform (shader)
    pub wave_keys: Rect,   // CUE/LOOP · CALL · ZOOM column right of the waveform
    pub info:      Rect,   // PLAYER · TRACK · cue pills · time · tempo · SYNC/MASTER
    pub bottom:    Rect,   // SLIP · overview · range/loop/BPM
    pub overview:  Rect,   // playing address bar graph (shader)
}

pub fn layout(size: Vec2) -> Layout {
    let w = size.x;
    let h = size.y;
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(Pos2::new(x0 * w, y0 * h), Pos2::new(x1 * w, y1 * h))
    };
    let col = 0.104;   // left column width
    let rc  = 0.800;   // right column starts
    Layout {
        screen:    r(0.0, 0.0, 1.0, 1.0),
        keys:      r(0.0,   0.000, 1.0,  0.130),
        title:     r(0.0,   0.130, 1.0,  0.212),
        source:    r(0.0,   0.212, col,  0.665),
        phase:     r(col,   0.212, rc,   0.325),
        wave:      r(col,   0.325, rc,   0.665),
        wave_keys: r(rc,    0.212, 1.0,  0.665),
        info:      r(0.0,   0.665, 1.0,  0.825),
        bottom:    r(0.0,   0.825, 1.0,  1.000),
        overview:  r(0.122, 0.862, 0.795, 0.945),
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────────

pub fn draw(ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout) {
    let p = ctx.layer_painter(egui::LayerId::background());

    // Ground everything the shader does not own.  egui paints *after* the
    // waveform pass, so the overview rect inside `bottom` must be left alone
    // or the whole-track waveform is painted over.
    for r in [lay.keys, lay.title, lay.source, lay.phase, lay.wave_keys, lay.info] {
        p.rect_filled(r, 0.0, BG);
    }
    let (b, o) = (lay.bottom, lay.overview);
    for r in [
        Rect::from_min_max(b.min, Pos2::new(b.max.x, o.min.y)),                       // above
        Rect::from_min_max(Pos2::new(b.min.x, o.max.y), b.max),                       // below
        Rect::from_min_max(Pos2::new(b.min.x, o.min.y), Pos2::new(o.min.x, o.max.y)), // left
        Rect::from_min_max(Pos2::new(o.max.x, o.min.y), Pos2::new(b.max.x, o.max.y)), // right
    ] {
        p.rect_filled(r, 0.0, BG);
    }

    draw_keys(&p, lay);
    draw_title(&p, snap, lay);
    draw_source(&p, lay);
    draw_phase(&p, snap, lay);
    draw_wave_keys(&p, lay);
    draw_info(&p, snap, lay);
    draw_bottom(&p, snap, lay);
}

fn text(p: &egui::Painter, pos: Pos2, a: Align2, s: impl ToString, size: f32, c: Color32) {
    p.text(pos, a, s, FontId::proportional(size), c);
}
fn mono(p: &egui::Painter, pos: Pos2, a: Align2, s: impl ToString, size: f32, c: Color32) {
    p.text(pos, a, s, FontId::monospace(size), c);
}

/// A touch key: slate face, two-line label (main / sub).
fn key(p: &egui::Painter, r: Rect, main: &str, sub: &str, on: bool) {
    p.rect_filled(r, 2.0, if on { BLUE } else { KEY });
    p.rect_stroke(r, 2.0, Stroke::new(1.0, LINE));
    if sub.is_empty() {
        text(p, r.center(), Align2::CENTER_CENTER, main, 11.0, TEXT);
    } else {
        text(p, Pos2::new(r.center().x, r.center().y - 5.0), Align2::CENTER_CENTER, main, 11.0, TEXT);
        text(p, Pos2::new(r.center().x, r.center().y + 8.0), Align2::CENTER_CENTER, sub, 7.5, DIM);
    }
}

/// Small state pill: filled when on, outlined when off.
fn pill(p: &egui::Painter, r: Rect, s: &str, on: bool, on_col: Color32) {
    if on {
        p.rect_filled(r, 2.0, on_col);
        text(p, r.center(), Align2::CENTER_CENTER, s, 8.0, TEXT);
    } else {
        p.rect_stroke(r, 2.0, Stroke::new(1.0, LINE));
        text(p, r.center(), Align2::CENTER_CENTER, s, 8.0, DIM);
    }
}

fn fmt_time(secs: f64) -> (String, String) {
    let m  = (secs / 60.0).floor();
    let s  = (secs - m * 60.0).floor();
    let ms = ((secs - m * 60.0 - s) * 1000.0).floor();
    (format!("{:02.0}:{:02.0}", m, s), format!("{:03.0}", ms))
}

// ── Touch-key row (top) ───────────────────────────────────────────────────────

fn draw_keys(p: &egui::Painter, lay: &Layout) {
    let r = lay.keys;
    let pad = 3.0;
    // rekordbox / source glyph cell on the far left.
    let cell = Rect::from_min_max(r.min + Vec2::splat(pad), Pos2::new(r.min.x + r.width() * 0.104 - pad, r.max.y - pad));
    p.rect_filled(cell, 2.0, KEY);
    p.rect_stroke(cell, 2.0, Stroke::new(1.0, LINE));
    p.circle_stroke(cell.center() - Vec2::new(0.0, 4.0), 8.0, Stroke::new(1.5, DIM));
    text(p, Pos2::new(cell.center().x, cell.max.y - 8.0), Align2::CENTER_CENTER, "freedj", 7.0, DIM);

    let keys = [("BROWSE", "SEARCH"), ("TAG LIST", ""), ("INFO", "LINK INFO"), ("MENU", "UTILITY"), ("PERFORM", "")];
    let x0 = r.min.x + r.width() * 0.104;
    let kw = (r.max.x - x0) / keys.len() as f32;
    for (i, (m, s)) in keys.iter().enumerate() {
        let kr = Rect::from_min_max(Pos2::new(x0 + i as f32 * kw + pad, r.min.y + pad),
                                    Pos2::new(x0 + (i as f32 + 1.0) * kw - pad, r.max.y - pad));
        key(p, kr, m, s, false);
    }
}

// ── Title row ─────────────────────────────────────────────────────────────────

fn draw_title(p: &egui::Painter, snap: &DeckSnapshot, lay: &Layout) {
    let r = lay.title;
    p.rect_filled(r, 0.0, BAR);
    let x = r.min.x + r.width() * 0.104 + 8.0;
    text(p, Pos2::new(x, r.center().y), Align2::LEFT_CENTER, format!("♪ {}", snap.title), 15.0, TEXT);
    // Key, right-aligned (detection not implemented — slot kept, dim).
    text(p, Pos2::new(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, "--", 14.0, DIM);
    text(p, Pos2::new(r.max.x - 34.0, r.center().y), Align2::RIGHT_CENTER, "♫", 11.0, DIM);
}

// ── Source column (LINK / USB) ────────────────────────────────────────────────

fn draw_source(p: &egui::Painter, lay: &Layout) {
    let r = lay.source;
    let pad = 3.0;
    let h = r.height() / 2.0;
    let link = Rect::from_min_max(Pos2::new(r.min.x + pad, r.min.y + pad), Pos2::new(r.max.x - pad, r.min.y + h - pad));
    let usb  = Rect::from_min_max(Pos2::new(r.min.x + pad, r.min.y + h + pad), Pos2::new(r.max.x - pad, r.max.y - pad));
    key(p, link, "LINK", "", false);
    key(p, usb,  "FILE", "", true);
    // Green "connected" bar down the inside edge, as on the unit.
    p.rect_filled(Rect::from_min_size(Pos2::new(r.max.x - 3.0, r.min.y + pad), Vec2::new(2.0, r.height() - 2.0 * pad)), 0.0, Color32::from_rgb(0x2c, 0xb0, 0x5c));
}

// ── Phase row: MASTER PLAYER · phase meter · beat countdown ───────────────────

fn draw_phase(p: &egui::Painter, snap: &DeckSnapshot, lay: &Layout) {
    let r = lay.phase;
    let cy = r.center().y;

    // MASTER PLAYER n label + number tile.
    let lx = r.min.x + 8.0;
    text(p, Pos2::new(lx, cy - 6.0), Align2::LEFT_CENTER, "MASTER", 7.5, DIM);
    text(p, Pos2::new(lx, cy + 5.0), Align2::LEFT_CENTER, "PLAYER", 7.5, DIM);
    let tile = Rect::from_center_size(Pos2::new(lx + 52.0, cy), Vec2::new(14.0, 16.0));
    p.rect_filled(tile, 1.0, if snap.beat2_bpm > 0.0 { ORANGE } else { KEY });
    mono(p, tile.center(), Align2::CENTER_CENTER, "2", 11.0, if snap.beat2_bpm > 0.0 { Color32::BLACK } else { DIM });

    // Phase meter: 4 beat cells in two rows — top row is the master (external)
    // player's beat, bottom row is ours.  Bar/beat divergence is what you read.
    let m0 = lx + 68.0;
    let m1 = r.min.x + r.width() * 0.72;
    let cell_w = (m1 - m0) / 4.0;
    let row_h = 7.0;
    let ours = snap.beat_in_bar();
    for i in 0..4u8 {
        let x = m0 + i as f32 * cell_w;
        let top = Rect::from_min_size(Pos2::new(x, cy - row_h - 1.5), Vec2::new(cell_w - 2.0, row_h));
        let bot = Rect::from_min_size(Pos2::new(x, cy + 1.5),         Vec2::new(cell_w - 2.0, row_h));
        p.rect_filled(top, 1.0, KEY);
        p.rect_filled(bot, 1.0, KEY);
        // Ours: lit cell fills with intra-beat progress.
        if ours == Some(i + 1) {
            if let Some(ph) = snap.beat_phase() {
                p.rect_filled(Rect::from_min_size(bot.min, Vec2::new(bot.width() * ph, row_h)), 1.0, BLUE);
            }
        }
    }
    // Master (external) phase: one continuous marker across the bar, in the
    // same cell as ours so divergence reads as horizontal offset.
    if snap.beat2_bpm > 0.0 {
        let cell = ours.unwrap_or(1) as f32 - 1.0;
        let x = m0 + (cell + snap.beat2_phase_beats) * cell_w;
        p.rect_filled(Rect::from_min_size(Pos2::new(m0 + cell * cell_w, cy - row_h - 1.5), Vec2::new((x - (m0 + cell * cell_w)).max(0.0), row_h)), 1.0, ORANGE);
        p.line_segment([Pos2::new(x, cy - row_h - 4.0), Pos2::new(x, cy + row_h + 4.0)], Stroke::new(2.0, CYAN));
    }

    // Beat countdown: orange = bars to the nearest stored cue (none yet),
    // blue = bars.beats to the next downbeat.
    let cx = m1 + 10.0;
    mono(p, Pos2::new(cx, cy - 6.0), Align2::LEFT_CENTER, "--.-", 12.0, ORANGE);
    text(p, Pos2::new(cx + 38.0, cy - 6.0), Align2::LEFT_CENTER, "Bars", 7.5, ORANGE);
    let blue = match (snap.beat_in_bar(), snap.beat_phase()) {
        (Some(b), Some(_)) => format!("00.{}", 4 - b),
        _ => "--.-".into(),
    };
    mono(p, Pos2::new(cx, cy + 6.0), Align2::LEFT_CENTER, blue, 12.0, BLUE);
    text(p, Pos2::new(cx + 38.0, cy + 6.0), Align2::LEFT_CENTER, "Bars", 7.5, BLUE);
}

// ── Column right of the waveform ──────────────────────────────────────────────

fn draw_wave_keys(p: &egui::Painter, lay: &Layout) {
    let r = lay.wave_keys;
    let pad = 4.0;
    let x0 = r.min.x + pad;
    let x1 = r.max.x - pad;
    let w  = x1 - x0;

    // CUE/LOOP  [DELETE] [MEMORY]
    let y = r.min.y + r.height() * 0.30;
    text(p, Pos2::new(r.center().x, y - 8.0), Align2::CENTER_CENTER, "CUE / LOOP", 7.0, DIM);
    let bh = 18.0;
    key(p, Rect::from_min_size(Pos2::new(x0, y), Vec2::new(w / 2.0 - 2.0, bh)), "DELETE", "", false);
    key(p, Rect::from_min_size(Pos2::new(x0 + w / 2.0 + 2.0, y), Vec2::new(w / 2.0 - 2.0, bh)), "MEMORY", "", false);

    // CALL  [◀] [▶]
    let y = r.min.y + r.height() * 0.58;
    text(p, Pos2::new(r.center().x, y - 8.0), Align2::CENTER_CENTER, "CALL", 7.0, DIM);
    key(p, Rect::from_min_size(Pos2::new(x0, y), Vec2::new(w / 2.0 - 2.0, bh)), "◀", "", false);
    key(p, Rect::from_min_size(Pos2::new(x0 + w / 2.0 + 2.0, y), Vec2::new(w / 2.0 - 2.0, bh)), "▶", "", false);

    // ZOOM / GRID mode indicator.
    let y = r.max.y - 18.0;
    let zr = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(w, 13.0));
    p.rect_stroke(zr, 1.0, Stroke::new(1.0, LINE));
    text(p, Pos2::new(zr.min.x + 6.0, zr.center().y), Align2::LEFT_CENTER, "ZOOM", 7.0, TEXT);
    text(p, Pos2::new(zr.max.x - 6.0, zr.center().y), Align2::RIGHT_CENTER, "GRID", 7.0, DIM);
}

// ── Info row ──────────────────────────────────────────────────────────────────

fn draw_info(p: &egui::Painter, snap: &DeckSnapshot, lay: &Layout) {
    let r = lay.info;
    let w = r.width();
    let cap_y = r.min.y + 8.0;          // caption baseline
    let big_y = r.center().y + 8.0;     // big readout centre
    let cap = |p: &egui::Painter, x: f32, s: &str, c: Color32| text(p, Pos2::new(x, cap_y), Align2::LEFT_TOP, s, 8.0, c);

    // PLAYER
    let x = r.min.x + 10.0;
    cap(p, x, "PLAYER", DIM);
    mono(p, Pos2::new(x + 14.0, big_y), Align2::LEFT_CENTER, "1", 30.0, TEXT);

    // TRACK
    let x = r.min.x + w * 0.113;
    cap(p, x, "TRACK", DIM);
    mono(p, Pos2::new(x, big_y), Align2::LEFT_CENTER, "01", 30.0, TEXT);

    // A.HOT CUE / A.CUE pills, stacked.
    let x = r.min.x + w * 0.22;
    pill(p, Rect::from_min_size(Pos2::new(x, r.min.y + 14.0), Vec2::new(62.0, 14.0)), "A.HOT CUE", false, BLUE);
    pill(p, Rect::from_min_size(Pos2::new(x, r.min.y + 32.0), Vec2::new(62.0, 14.0)), "A.CUE",     false, BLUE);

    // REMAIN · QUANTIZE captions over the time.
    let x = r.min.x + w * 0.355;
    cap(p, x, "REMAIN", TEXT);
    cap(p, x + 50.0, "QUANTIZE : -", ORANGE);
    let (mmss, ms) = fmt_time(snap.remaining_secs());
    mono(p, Pos2::new(x, big_y), Align2::LEFT_CENTER, &mmss, 36.0, TEXT);
    mono(p, Pos2::new(x + 112.0, big_y + 7.0), Align2::LEFT_CENTER, format!(".{ms}"), 17.0, TEXT);

    // TEMPO · MT
    let x = r.min.x + w * 0.615;
    cap(p, x, "TEMPO", DIM);
    let mt = Rect::from_min_size(Pos2::new(x + 42.0, cap_y - 1.0), Vec2::new(20.0, 11.0));
    pill(p, mt, "MT", snap.key_lock, RED);
    let t = snap.tempo_percent();
    let t_txt = if t.abs() < 0.005 { "0.00".into() } else { format!("{:+.2}", t) };
    mono(p, Pos2::new(x, big_y), Align2::LEFT_CENTER, t_txt, 30.0, TEXT);
    text(p, Pos2::new(x + 98.0, big_y + 8.0), Align2::LEFT_CENTER, "%", 11.0, DIM);

    // SYNC / MASTER keys.
    let x = r.min.x + w * 0.81;
    let kw = (r.max.x - 4.0 - x) / 2.0;
    let ky = r.min.y + 10.0;
    let kh = r.height() - 20.0;
    key(p, Rect::from_min_size(Pos2::new(x, ky), Vec2::new(kw - 3.0, kh)), "SYNC", "INST.D.", false);
    key(p, Rect::from_min_size(Pos2::new(x + kw + 3.0, ky), Vec2::new(kw - 3.0, kh)), "MASTER", "", false);
    let _ = KEY_HI;
}

// ── Bottom row: SLIP · overview · range / loop / BPM ──────────────────────────

fn draw_bottom(p: &egui::Painter, snap: &DeckSnapshot, lay: &Layout) {
    let r = lay.bottom;
    let w = r.width();

    // SLIP key on the left.
    let sr = Rect::from_min_max(Pos2::new(r.min.x + 4.0, r.min.y + 10.0), Pos2::new(r.min.x + w * 0.104 - 4.0, r.max.y - 12.0));
    key(p, sr, "SLIP", "", false);

    // Overview frame and cue-marker rows above and below (no cues wired yet).
    let ov = lay.overview;
    p.rect_stroke(ov, 0.0, Stroke::new(1.0, LINE));
    text(p, Pos2::new(ov.center().x, r.max.y - 7.0), Align2::CENTER_CENTER, "NEEDLE COUNTDOWN", 7.0, DIM);

    // Scale: 1-minute ticks of remaining time along the top edge of the overview.
    let total = snap.total_secs();
    if total > 0.0 {
        let mut rem = 60.0;
        while rem < total {
            let x = ov.max.x - (rem / total) as f32 * ov.width();
            p.line_segment([Pos2::new(x, ov.min.y - 4.0), Pos2::new(x, ov.min.y)], Stroke::new(1.0, DIM));
            rem += 60.0;
        }
    }

    // Track number and cache meter, bottom-left under the overview.
    mono(p, Pos2::new(ov.min.x, r.max.y - 7.0), Align2::LEFT_CENTER, "01", 8.0, DIM);
    p.rect_filled(Rect::from_min_size(Pos2::new(ov.min.x + 16.0, r.max.y - 9.0), Vec2::new(24.0, 4.0)), 0.0, BLUE);

    // Right column: ±range badge, loop beats, BPM.
    let x = r.min.x + w * 0.81;
    let badge = Rect::from_min_size(Pos2::new(x, r.min.y + 6.0), Vec2::new(34.0, 14.0));
    p.rect_filled(badge, 2.0, RED);
    text(p, badge.center(), Align2::CENTER_CENTER, "±16", 9.0, TEXT);
    mono(p, Pos2::new(x + 6.0, r.max.y - 22.0), Align2::LEFT_CENTER, "-", 16.0, DIM);   // loop beat count

    let bx = r.min.x + w * 0.88;
    let br = Rect::from_min_max(Pos2::new(bx, r.min.y + 6.0), Pos2::new(r.max.x - 4.0, r.max.y - 8.0));
    p.rect_filled(br, 2.0, KEY);
    let (bpm_txt, col) = match (snap.bpm(), snap.beat_grid) {
        (Some(b), Some(g)) => (format!("{:.1}", b), if g.confidence >= 0.7 { TEXT } else { ORANGE }),
        _ => ("---.-".into(), DIM),
    };
    let dot = bpm_txt.find('.').unwrap_or(bpm_txt.len());
    let (ip, fp) = bpm_txt.split_at(dot);
    mono(p, Pos2::new(br.min.x + 6.0, br.center().y - 4.0), Align2::LEFT_CENTER, ip, 26.0, col);
    mono(p, Pos2::new(br.min.x + 6.0 + ip.len() as f32 * 15.7, br.center().y + 2.0), Align2::LEFT_CENTER, fp, 14.0, col);
    text(p, Pos2::new(br.max.x - 5.0, br.max.y - 6.0), Align2::RIGHT_CENTER, "BPM", 7.5, DIM);
}
