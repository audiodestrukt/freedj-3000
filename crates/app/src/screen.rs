//! Screen chrome — everything drawn with egui on top of the waveform pass,
//! and every touch target on it.
//!
//! Layout is the Pioneer XDJ-1000MK2 normal playback screen.  Region
//! fractions and type sizes were measured from a photograph of the unit
//! (`reference/pioneer/xdj-1000mk2-photo.png`), cross-checked against the
//! manual's part-names diagram (DRI1396B p.16).  The two waveforms are drawn
//! by the shader into the rects this module hands out; everything else is
//! painted here.
//!
//! Touch: egui folds winit touch and mouse into one pointer, so the mouse is
//! the touch panel until there is a real one.  Every target is
//! `ui.interact` on a Layout rect; hits are pushed onto the input bus as
//! [`Event`]s and applied by `DeckApp::apply`.  This module reads a
//! [`DeckSnapshot`] and holds no deck state of its own.

use crate::input::{ControlEvent, Event, Screen as TopScreen, Source, UiEvent};
use crate::snapshot::DeckSnapshot;
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

// ── Palette ───────────────────────────────────────────────────────────────────
// Sampled from the photo: black ground, slate-blue touch keys, a navy title
// bar, white readouts.  Colour carries state only.

const BG:      Color32 = Color32::from_rgb(0x03, 0x04, 0x06);
const BAR:     Color32 = Color32::from_rgb(0x1a, 0x27, 0x3d);   // title bar
const KEY:     Color32 = Color32::from_rgb(0x3a, 0x46, 0x58);   // touch key face
const KEY_LO:  Color32 = Color32::from_rgb(0x2a, 0x33, 0x40);   // key face, dim
const KEY_HI:  Color32 = Color32::from_rgb(0x55, 0x64, 0x7a);   // key face, pressed
const TEXT:    Color32 = Color32::from_rgb(0xf6, 0xf7, 0xf8);
const DIM:     Color32 = Color32::from_rgb(0x9a, 0xa3, 0xae);
const FAINT:   Color32 = Color32::from_rgb(0x4a, 0x52, 0x5c);
const BLUE:    Color32 = Color32::from_rgb(0x2f, 0x7f, 0xe0);
const ORANGE:  Color32 = Color32::from_rgb(0xf0, 0x8a, 0x1e);
const RED:     Color32 = Color32::from_rgb(0xe0, 0x2a, 0x2a);
const GREEN:   Color32 = Color32::from_rgb(0x3c, 0xc8, 0x50);
const GOLD:    Color32 = Color32::from_rgb(0xf0, 0xb0, 0x20);   // MASTER state

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

/// Draw the screen and collect the touch events it produced this frame.
pub fn draw(ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, out: &mut Vec<Event>) {
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, |ui| {
            let h = lay.screen.height();

            // Ground everything except the two shader rects.  egui paints
            // after the waveform pass, so those must be left alone.
            for r in cover(lay.screen, lay.wave, lay.overview) {
                ui.painter().rect_filled(r, 0.0, BG);
            }

            draw_left(ui, snap, lay, h, out);
            draw_keys(ui, lay, h, out);
            draw_title(ui, snap, lay, h);
            draw_phase(ui, snap, lay, h, out);
            draw_wave_area(ui, snap, lay, h, out);
            draw_info(ui, snap, lay, h, out);
            draw_bottom(ui, snap, lay, h, out);
        });
}

/// Rects that tile `outer` minus two holes `a` and `b` (a above b, non-overlapping).
fn cover(outer: Rect, a: Rect, b: Rect) -> Vec<Rect> {
    let band = |y0: f32, y1: f32| Rect::from_min_max(Pos2::new(outer.min.x, y0), Pos2::new(outer.max.x, y1));
    vec![
        band(outer.min.y, a.min.y),
        Rect::from_min_max(Pos2::new(outer.min.x, a.min.y), Pos2::new(a.min.x, a.max.y)),
        Rect::from_min_max(Pos2::new(a.max.x, a.min.y), Pos2::new(outer.max.x, a.max.y)),
        band(a.max.y, b.min.y),
        Rect::from_min_max(Pos2::new(outer.min.x, b.min.y), Pos2::new(b.min.x, b.max.y)),
        Rect::from_min_max(Pos2::new(b.max.x, b.min.y), Pos2::new(outer.max.x, b.max.y)),
        band(b.max.y, outer.max.y),
    ]
}

fn text(ui: &Ui, pos: Pos2, a: Align2, s: impl ToString, size: f32, c: Color32) {
    ui.painter().text(pos, a, s, FontId::proportional(size), c);
}

/// A touch target.  Returns true on tap (press + release inside).
fn tap(ui: &Ui, r: Rect, name: &str) -> (bool, bool) {
    let resp = ui.interact(r, Id::new(name), Sense::click());
    (resp.clicked(), resp.is_pointer_button_down_on())
}

/// Touch key: slate face, main label, optional "– SUB" line.  `lit` is the
/// engaged state; the face also brightens while the finger is down.
fn key(ui: &Ui, r: Rect, name: &str, main: &str, sub: &str, h: f32, lit: Option<Color32>) -> bool {
    let (clicked, down) = tap(ui, r, name);
    let face = if let Some(c) = lit { c } else if down { KEY_HI } else { KEY };
    ui.painter().rect_filled(r, 2.0, face);
    let big = h * 0.032;
    if sub.is_empty() {
        text(ui, r.center(), Align2::CENTER_CENTER, main, big, TEXT);
    } else {
        text(ui, Pos2::new(r.center().x, r.center().y - h * 0.013), Align2::CENTER_CENTER, main, big, TEXT);
        text(ui, Pos2::new(r.center().x, r.center().y + h * 0.020), Align2::CENTER_CENTER, format!("– {sub}"), h * 0.019, TEXT);
    }
    clicked
}

/// Caption with bracket ticks either side, as over CUE/LOOP and CALL.
fn bracket_caption(ui: &Ui, r: Rect, s: &str, h: f32) {
    let y = r.min.y - h * 0.016;
    text(ui, Pos2::new(r.center().x, y), Align2::CENTER_CENTER, s, h * 0.018, DIM);
    let tw = s.len() as f32 * h * 0.010;
    let p = ui.painter();
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

fn draw_left(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    let p = ui.painter();
    // rekordbox-style logo cell.
    let l = lay.logo;
    p.rect_filled(l, 2.0, KEY_LO);
    let c = Pos2::new(l.center().x, l.center().y - h * 0.02);
    p.circle_stroke(c, h * 0.022, Stroke::new(2.0, TEXT));
    p.circle_filled(c, h * 0.008, TEXT);
    text(ui, Pos2::new(l.center().x, l.max.y - h * 0.022), Align2::CENTER_CENTER, "freedj", h * 0.018, TEXT);

    if key(ui, lay.link, "src-link", "LINK", "", h, None) {
        out.push(Event::Ui(UiEvent::Source(Source::Link)));
    }
    if key(ui, lay.usb, "src-usb", "FILE", "", h, None) {
        out.push(Event::Ui(UiEvent::Source(Source::Usb)));
    }
    // Green bar down the left edge of the selected source only.
    let sel = if snap.source_link { lay.link } else { lay.usb };
    ui.painter().rect_filled(Rect::from_min_size(sel.min, Vec2::new(h * 0.008, sel.height())), 0.0, GREEN);

    // PLAYER n — dim text standalone; a solid blue box once a Link peer is heard.
    let pl = lay.player;
    let (cap_c, num_c) = if snap.linked {
        p.rect_filled(pl, 2.0, BLUE);
        (TEXT, TEXT)
    } else {
        (FAINT, FAINT)
    };
    text(ui, Pos2::new(pl.center().x, pl.min.y + h * 0.018), Align2::CENTER_CENTER, "PLAYER", h * 0.018, cap_c);
    text(ui, Pos2::new(pl.center().x, pl.center().y + h * 0.020), Align2::CENTER_CENTER, "1", h * 0.075, num_c);

    if key(ui, lay.slip, "slip", "SLIP", "", h, snap.slip.then_some(BLUE)) {
        out.push(Event::Deck(ControlEvent::SlipToggle));
    }
}

// ── Touch-key row ─────────────────────────────────────────────────────────────

fn draw_keys(ui: &Ui, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    let r = lay.keys;
    let keys = [
        ("BROWSE", "SEARCH",    TopScreen::Browse),
        ("TAG LIST", "",        TopScreen::TagList),
        ("INFO", "LINK INFO",   TopScreen::Info),
        ("MENU", "UTILITY",     TopScreen::Menu),
        ("PERFORM", "",         TopScreen::Perform),
    ];
    let gap = 2.0;
    let kw = (r.width() - gap * (keys.len() as f32 - 1.0)) / keys.len() as f32;
    for (i, (m, s, which)) in keys.iter().enumerate() {
        let x0 = r.min.x + i as f32 * (kw + gap);
        let kr = Rect::from_min_max(Pos2::new(x0, r.min.y), Pos2::new(x0 + kw, r.max.y));
        if key(ui, kr, m, m, s, h, None) {
            out.push(Event::Ui(UiEvent::Screen(*which)));
        }
    }
    // PERFORM's expand glyph, top-right.
    let last = Rect::from_min_max(Pos2::new(r.max.x - kw, r.min.y), r.max);
    let c = Pos2::new(last.max.x - h * 0.020, last.min.y + h * 0.020);
    let d = h * 0.009;
    let st = Stroke::new(1.5, TEXT);
    let p = ui.painter();
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x + d, c.y - d)], st);
    p.line_segment([Pos2::new(c.x + d, c.y - d), Pos2::new(c.x + d * 0.2, c.y - d)], st);
    p.line_segment([Pos2::new(c.x + d, c.y - d), Pos2::new(c.x + d, c.y - d * 0.2)], st);
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x - d * 0.2, c.y + d)], st);
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x - d, c.y + d * 0.2)], st);
}

// ── Title bar ─────────────────────────────────────────────────────────────────

fn draw_title(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32) {
    let r = lay.title;
    ui.painter().rect_filled(r, 0.0, BAR);
    text(ui, Pos2::new(r.min.x + h * 0.02, r.center().y), Align2::LEFT_CENTER, format!("♪ {}", snap.title), h * 0.036, TEXT);
    // Key badge + key name, right.  Detection is not implemented; the slot
    // draws its glyph and a dash.
    let kx = r.max.x - h * 0.02;
    text(ui, Pos2::new(kx, r.center().y), Align2::RIGHT_CENTER, "--", h * 0.034, TEXT);
    let badge = Rect::from_center_size(Pos2::new(kx - h * 0.075, r.center().y), Vec2::new(h * 0.030, h * 0.030));
    ui.painter().rect_filled(badge, 2.0, KEY);
    text(ui, badge.center(), Align2::CENTER_CENTER, "b#", h * 0.017, TEXT);
}

// ── Phase meter + beat countdown ──────────────────────────────────────────────

fn draw_phase(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    let r = lay.phase;
    // Touching the slot switches between the two views, as on the unit.
    let (toggle, _) = tap(ui, r, "phase-meter");
    if toggle { out.push(Event::Ui(UiEvent::PhaseMeterView)); }

    if snap.phase_ticks_view {
        draw_phase_ticks(ui, snap, r, h);
    } else {
        draw_phase_boxes(ui, snap, r, h);
    }

    // "Bars" readouts: orange counts bars.beats to the next memory cue, blue
    // to the next hot cue.  No cues exist yet, so both are None → dashes.
    let b = lay.bars;
    let row = r.height() / 2.0;
    let fs = h * 0.026;
    let fmt = |v: Option<(u32, u32)>| v.map(|(bars, beats)| format!("{bars:02}.{beats}")).unwrap_or_else(|| "--.-".into());
    let memory_cue: Option<(u32, u32)> = None;
    let hot_cue:    Option<(u32, u32)> = None;
    text(ui, Pos2::new(b.min.x, b.min.y + row * 0.5), Align2::LEFT_CENTER, fmt(memory_cue), fs, ORANGE);
    text(ui, Pos2::new(b.min.x + h * 0.078, b.min.y + row * 0.5), Align2::LEFT_CENTER, "Bars", h * 0.020, ORANGE);
    text(ui, Pos2::new(b.min.x, b.min.y + row * 1.5), Align2::LEFT_CENTER, fmt(hot_cue), fs, BLUE);
    text(ui, Pos2::new(b.min.x + h * 0.078, b.min.y + row * 1.5), Align2::LEFT_CENTER, "Bars", h * 0.020, BLUE);
}

/// Beat display: two rows of four outlined boxes.  Top row is the master
/// player (orange), bottom row this deck; the current beat is solid — blue,
/// or orange when this deck is the master.
fn draw_phase_boxes(ui: &Ui, snap: &DeckSnapshot, r: Rect, h: f32) {
    let p = ui.painter();
    let gap = h * 0.006;
    let row_h = (r.height() - gap) / 2.0;
    let cell_w = (r.width() - 3.0 * gap) / 4.0;
    let ours = snap.beat_in_bar();
    let has_master = snap.beat2_bpm > 0.0;
    let master_beat = ours;   // only the master's phase is known; same cell when matched
    let our_lit = if snap.master { ORANGE } else { BLUE };

    for i in 0..4u8 {
        let x = r.min.x + i as f32 * (cell_w + gap);
        let top = Rect::from_min_size(Pos2::new(x, r.min.y),               Vec2::new(cell_w, row_h));
        let bot = Rect::from_min_size(Pos2::new(x, r.min.y + row_h + gap), Vec2::new(cell_w, row_h));
        if has_master && master_beat == Some(i + 1) {
            p.rect_filled(top, 1.0, ORANGE);
        } else {
            p.rect_stroke(top, 1.0, Stroke::new(1.0, if has_master { ORANGE } else { Color32::from_rgb(0x7a, 0x4a, 0x16) }));
        }
        if ours == Some(i + 1) {
            p.rect_filled(bot, 1.0, our_lit);
        } else {
            p.rect_stroke(bot, 1.0, Stroke::new(1.0, BLUE));
        }
    }
    if has_master {
        let cell = master_beat.unwrap_or(1) as f32 - 1.0;
        let x = r.min.x + cell * (cell_w + gap) + snap.beat2_phase_beats * cell_w;
        p.line_segment([Pos2::new(x, r.min.y - 2.0), Pos2::new(x, r.min.y + row_h + 2.0)], Stroke::new(2.0, TEXT));
    }
}

/// Alignment view: `MASTER PLAYER [n]` tag, then two rows of beat ticks —
/// the master's grid above, ours below — scrolling under a fixed white
/// playhead.  Phase offset between the decks reads as horizontal displacement.
fn draw_phase_ticks(ui: &Ui, snap: &DeckSnapshot, r: Rect, h: f32) {
    let p = ui.painter();
    // Tag at the left.
    let tag_w = h * 0.115;
    text(ui, Pos2::new(r.min.x, r.min.y + h * 0.012), Align2::LEFT_CENTER, "MASTER", h * 0.016, TEXT);
    text(ui, Pos2::new(r.min.x, r.min.y + h * 0.032), Align2::LEFT_CENTER, "PLAYER", h * 0.016, TEXT);
    let nb = Rect::from_min_size(Pos2::new(r.min.x + h * 0.070, r.min.y + h * 0.010), Vec2::new(h * 0.028, h * 0.028));
    let master_txt = if snap.master_player > 0 { snap.master_player.to_string() } else { "-".into() };
    p.rect_filled(nb, 1.0, GOLD);
    text(ui, nb.center(), Align2::CENTER_CENTER, master_txt, h * 0.018, Color32::BLACK);

    // Tick rows.
    let x0 = r.min.x + tag_w;
    let w  = r.max.x - x0;
    let beats_visible = 8.0;
    let beat_px = w / beats_visible;
    let cx = x0 + w * 0.5;
    let rows = [
        (r.min.y + r.height() * 0.30, if snap.beat2_bpm > 0.0 { Some(snap.beat2_phase_beats) } else { None }, ORANGE),
        (r.min.y + r.height() * 0.78, snap.beat_phase(), BLUE),
    ];
    let bib = snap.beat_in_bar().unwrap_or(1) as i32;
    for (y, phase, bar_col) in rows {
        p.line_segment([Pos2::new(x0, y), Pos2::new(r.max.x, y)], Stroke::new(1.0, FAINT));
        let Some(ph) = phase else { continue };
        for i in -4..=4i32 {
            let x = cx + (i as f32 - ph) * beat_px;
            if x < x0 || x > r.max.x { continue; }
            // Bar ticks taller; bar position from our own beat-in-bar.
            let is_bar = (i + bib - 1).rem_euclid(4) == 0;
            let len = if is_bar { h * 0.020 } else { h * 0.011 };
            let col = if is_bar { bar_col } else { DIM };
            p.line_segment([Pos2::new(x, y - len), Pos2::new(x, y)], Stroke::new(1.5, col));
        }
    }
    // Playhead.
    p.line_segment([Pos2::new(cx, r.min.y), Pos2::new(cx, r.max.y)], Stroke::new(1.5, TEXT));
}

// ── Enlarged waveform: zoom by wheel; and the column to its right ────────────

fn draw_wave_area(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    // Wheel over the waveform zooms, standing in for the rotary selector.
    let wave = ui.interact(lay.wave, Id::new("wave"), Sense::hover());
    if wave.hovered() {
        let dy = ui.input(|i| i.raw_scroll_delta.y);
        if dy.abs() > 0.0 {
            out.push(Event::Ui(UiEvent::ZoomStep(if dy > 0.0 { -1 } else { 1 })));
        }
    }

    let half = |r: Rect, i: usize| {
        let w = (r.width() - 2.0) / 2.0;
        Rect::from_min_size(Pos2::new(r.min.x + i as f32 * (w + 2.0), r.min.y), Vec2::new(w, r.height()))
    };
    bracket_caption(ui, lay.cueloop, "CUE / LOOP", h);
    key(ui, half(lay.cueloop, 0), "cue-delete", "DELETE", "", h, None);
    key(ui, half(lay.cueloop, 1), "cue-memory", "MEMORY", "", h, None);

    bracket_caption(ui, lay.call, "CALL", h);
    key(ui, half(lay.call, 0), "call-prev", "◀", "", h, None);
    key(ui, half(lay.call, 1), "call-next", "▶", "", h, None);

    // ZOOM – GRID pill: tap ZOOM to step the zoom, tap GRID to switch mode.
    let z = lay.zoom;
    let mid = z.center().x;
    let zl = Rect::from_min_max(z.min, Pos2::new(mid, z.max.y));
    let zr = Rect::from_min_max(Pos2::new(mid, z.min.y), z.max);
    let (zoom_tap, _) = tap(ui, zl, "zoom-zoom");
    let (grid_tap, _) = tap(ui, zr, "zoom-grid");
    if zoom_tap { out.push(Event::Ui(UiEvent::ZoomStep(1))); }
    if grid_tap { out.push(Event::Ui(UiEvent::ZoomGridMode)); }
    let p = ui.painter();
    let (zc, gc) = if snap.zoom_grid_mode { (KEY_LO, BLUE) } else { (BLUE, KEY_LO) };
    p.rect_filled(zl, 2.0, zc);
    p.rect_filled(zr, 2.0, gc);
    text(ui, Pos2::new(z.min.x + z.width() * 0.25, z.center().y), Align2::CENTER_CENTER, "ZOOM", h * 0.018, if snap.zoom_grid_mode { DIM } else { TEXT });
    text(ui, Pos2::new(z.min.x + z.width() * 0.75, z.center().y), Align2::CENTER_CENTER, "– GRID", h * 0.018, if snap.zoom_grid_mode { TEXT } else { DIM });
}

// ── Info row ──────────────────────────────────────────────────────────────────

fn draw_info(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    let big = h * 0.085;
    let cap = h * 0.019;
    let base_y = |r: Rect| r.max.y - h * 0.010;   // baseline the big readouts share

    // TRACK
    let t = lay.track;
    text(ui, Pos2::new(t.min.x, t.min.y + h * 0.006), Align2::LEFT_TOP, "TRACK", cap, TEXT);
    text(ui, Pos2::new(t.min.x, base_y(t)), Align2::LEFT_BOTTOM, "01", big, TEXT);

    // A.CUE — shown only when on (auto cue is on by default on the unit).
    let a = lay.acue;
    ui.painter().rect_stroke(a, 2.0, Stroke::new(1.0, TEXT));
    text(ui, a.center(), Align2::CENTER_CENTER, "A.CUE", h * 0.018, TEXT);

    // Time: tap toggles TIME / REMAIN (a hard button on the unit; handy here).
    let tm = lay.time;
    let (time_tap, _) = tap(ui, tm, "time");
    if time_tap { out.push(Event::Ui(UiEvent::TimeMode)); }
    let shown = if snap.remain_mode { snap.remaining_secs() } else { snap.elapsed_secs() };
    text(ui, Pos2::new(tm.center().x, tm.min.y + h * 0.006), Align2::CENTER_TOP, "QUANTIZE : –", cap, ORANGE);
    if snap.remain_mode {
        text(ui, Pos2::new(tm.min.x, tm.min.y + h * 0.006), Align2::LEFT_TOP, "REMAIN", cap, TEXT);
    }
    text(ui, Pos2::new(tm.center().x, base_y(tm)), Align2::CENTER_BOTTOM, fmt_time(shown), big, TEXT);

    // TEMPO caption + MT pill (tap toggles master tempo), then the percentage.
    let te = lay.tempo;
    text(ui, Pos2::new(te.min.x, te.min.y + h * 0.006), Align2::LEFT_TOP, "TEMPO", cap, TEXT);
    let mt = Rect::from_min_size(Pos2::new(te.min.x + h * 0.075, te.min.y + h * 0.004), Vec2::new(h * 0.042, h * 0.026));
    let (mt_tap, _) = tap(ui, mt, "mt");
    if mt_tap { out.push(Event::Deck(ControlEvent::KeyLockToggle)); }
    if snap.key_lock {
        ui.painter().rect_filled(mt, 2.0, RED);
        text(ui, mt.center(), Align2::CENTER_CENTER, "MT", h * 0.018, TEXT);
    } else {
        ui.painter().rect_stroke(mt, 2.0, Stroke::new(1.0, FAINT));
        text(ui, mt.center(), Align2::CENTER_CENTER, "MT", h * 0.018, FAINT);
    }
    let v = snap.tempo_percent();
    let s = if v.abs() < 0.005 { "0.00".to_string() } else { format!("{:+.2}", v) };
    text(ui, Pos2::new(te.max.x - h * 0.030, base_y(te)), Align2::RIGHT_BOTTOM, s, big, TEXT);
    text(ui, Pos2::new(te.max.x, base_y(te) - h * 0.008), Align2::RIGHT_BOTTOM, "%", h * 0.034, TEXT);

    if key(ui, lay.sync, "sync", "SYNC", "INST.D.", h, snap.sync.then_some(BLUE)) {
        out.push(Event::Deck(ControlEvent::SyncToggle));
    }
    if key(ui, lay.master, "master", "MASTER", "", h, snap.master.then_some(GOLD)) {
        out.push(Event::Deck(ControlEvent::MasterRequest));
    }
}

// ── Bottom row ────────────────────────────────────────────────────────────────

fn draw_bottom(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    // Needle search: press or drag anywhere on the overview jumps there.
    // The unit does the same on the playing-address bar.
    let ov = lay.overview;
    let resp = ui.interact(ov, Id::new("needle"), Sense::click_and_drag());
    if resp.is_pointer_button_down_on() || resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let x = ((pos.x - ov.min.x) / ov.width()).clamp(0.0, 1.0);
            out.push(Event::Deck(ControlEvent::NeedleSearch { position: x }));
        }
    }
    let p = ui.painter();
    // A faint hover line shows where a tap would land.
    if let (true, Some(pos)) = (resp.hovered(), resp.hover_pos()) {
        p.line_segment([Pos2::new(pos.x, ov.min.y), Pos2::new(pos.x, ov.max.y)], Stroke::new(1.0, FAINT));
    }

    // NEEDLE SEARCH bar under the overview.
    let n = lay.needle;
    p.rect_filled(n, 1.0, KEY_LO);
    text(ui, n.center(), Align2::CENTER_CENTER, if snap.linked { "NEEDLE COUNTDOWN" } else { "NEEDLE SEARCH" }, h * 0.018, TEXT);
    // Cue marker triangle (none stored yet): a single start marker, as on the unit.
    let x = ov.min.x + 3.0;
    let y = ov.max.y + h * 0.010;
    p.add(egui::Shape::convex_polygon(
        vec![Pos2::new(x - 4.0, y + 4.0), Pos2::new(x + 4.0, y + 4.0), Pos2::new(x, y - 3.0)],
        ORANGE, Stroke::NONE));

    // ±range badge.
    let rg = lay.range;
    p.rect_filled(rg, 2.0, RED);
    text(ui, rg.center(), Align2::CENTER_CENTER, "±16", h * 0.024, TEXT);

    // BPM box: dark face, big integer part, smaller fraction, "BPM" caption.
    let b = lay.bpm;
    p.rect_filled(b, 2.0, if snap.master { GOLD } else { KEY_LO });
    let ink = if snap.master { Color32::BLACK } else { TEXT };
    let (txt, col) = match (snap.bpm(), snap.beat_grid) {
        (Some(v), Some(g)) => (format!("{:.1}", v), if snap.master { Color32::BLACK } else if g.confidence >= 0.7 { TEXT } else { ORANGE }),
        _ => ("---.-".into(), DIM),
    };
    let dot = txt.find('.').unwrap_or(txt.len());
    let (ip, fp) = txt.split_at(dot);
    let base = b.max.y - h * 0.030;
    let ip_size = h * 0.060;
    text(ui, Pos2::new(b.min.x + h * 0.012, base), Align2::LEFT_BOTTOM, ip, ip_size, col);
    text(ui, Pos2::new(b.min.x + h * 0.012 + ip.len() as f32 * ip_size * 0.56, base), Align2::LEFT_BOTTOM, fp, h * 0.040, col);
    text(ui, Pos2::new(b.max.x - h * 0.010, b.max.y - h * 0.008), Align2::RIGHT_BOTTOM, if snap.master { "MASTER" } else { "BPM" }, h * 0.018, ink);
}
