//! CDJ-3000 skin: the Pioneer CDJ-3000's player screens, as the second deck
//! UI beside the XDJ-1000MK2 one in `screen.rs` (`Settings::skin`).
//!
//! Regions are measured in pixels off 1280×720 captures of the unit's player
//! screen (cdj3k-emu `lcd main`, SOURCE → USB; kept locally under
//! `reference/pioneer/cdj3000-emu/`, not in git) and drawn as fractions of the
//! 16:9 screen rect, so the proportions hold at any window size. Colours are
//! sampled from the same captures. Plan and gap list:
//! docs/design/cdj-3000-skin.md.
//!
//! Like `screen.rs`, this reads a [`DeckSnapshot`], holds no deck state, and
//! leaves the two shader rects (enlarged waveform, overview) unpainted. Only
//! things that are on the unit's touch screen are drawn; its hardware
//! buttons arrive as events from the surfaces.

use crate::input::{Event, UiEvent};
use crate::screen::{bars_to_next, cover, needle_touch, tap, wave_touch};
use crate::settings::Settings;
use crate::snapshot::DeckSnapshot;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Ui, Vec2};

/// The unit's screen: 1280×720.
pub const ASPECT: Vec2 = Vec2::new(1280.0, 720.0);

// ── Palette (sampled from the captures) ──────────────────────────────────────

const BG:          Color32 = Color32::BLACK;
const HEADER:      Color32 = Color32::from_rgb(25, 25, 25);
const SOURCE_TILE: Color32 = Color32::from_rgb(15, 50, 95);
const KEY_FACE:    Color32 = Color32::from_rgb(45, 45, 45);
const KEY_EDGE:    Color32 = Color32::from_rgb(125, 125, 125);
const TEXT:        Color32 = Color32::WHITE;
const LABEL:       Color32 = Color32::from_rgb(186, 186, 186);
const DIM:         Color32 = Color32::from_rgb(125, 125, 125);
const BOX:         Color32 = Color32::from_rgb(200, 200, 200);
const TICK:        Color32 = Color32::from_rgb(166, 166, 166);
const RED:         Color32 = Color32::from_rgb(255, 0, 0);
const ORANGE:      Color32 = Color32::from_rgb(255, 70, 25);
const ZOOM_BLUE:   Color32 = Color32::from_rgb(0, 125, 225);
const BARS_ORANGE: Color32 = Color32::from_rgb(255, 115, 0);
const BARS_BLUE:   Color32 = Color32::from_rgb(65, 166, 245);
/// Hot-cue flag colour for a cue with no colour of its own.
const CUE_GREEN:   Color32 = Color32::from_rgb(40, 226, 20);

// ── Layout ───────────────────────────────────────────────────────────────────

/// Playback-screen regions in logical points.
pub struct Layout {
    pub screen:    Rect,
    // top bar
    pub header:    Rect,
    pub source:    Rect,
    pub art:       Rect,
    pub title:     Rect,
    pub track_row: Rect,
    pub info_btn:  Rect,
    pub keys:      [Rect; 3],
    // waveform field
    pub bars:      Rect,
    pub field:     Rect,
    pub ticks_top: Rect,
    pub wave:      Rect,   // enlarged waveform (shader)
    pub ticks_bot: Rect,
    pub zoom:      Rect,
    // info block
    pub player:    Rect,
    pub track:     Rect,
    pub auto_cue:  Rect,
    pub time:      Rect,
    pub tempo:     Rect,
    pub range:     Rect,
    pub bpm:       Rect,
    pub flags:     Rect,   // hot-cue letters above the overview
    pub overview:  Rect,   // overall waveform (shader)
    pub address:   Rect,   // the bar under the overview
    pub ruler:     Rect,
    pub mt:        Rect,
    pub key:       Rect,
}

/// Lay the playback screen out in `screen` (16:9).
pub fn layout(screen: Rect) -> Layout {
    let (sx, sy) = (screen.width() / ASPECT.x, screen.height() / ASPECT.y);
    // Pixel coordinates on the 1280×720 unit → points in `screen`.
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(
            Pos2::new(screen.min.x + x0 * sx, screen.min.y + y0 * sy),
            Pos2::new(screen.min.x + x1 * sx, screen.min.y + y1 * sy),
        )
    };
    Layout {
        screen,
        header:    r(0.0, 0.0, 1280.0, 90.0),
        source:    r(0.0, 0.0, 90.0, 90.0),
        art:       r(110.0, 15.0, 170.0, 75.0),
        title:     r(190.0, 4.0, 890.0, 50.0),
        track_row: r(190.0, 50.0, 890.0, 90.0),
        info_btn:  r(534.0, 56.0, 562.0, 84.0),
        keys:      [r(898.0, 0.0, 1010.0, 90.0), r(1022.0, 0.0, 1135.0, 90.0), r(1146.0, 0.0, 1260.0, 90.0)],

        bars:      r(160.0, 110.0, 340.0, 185.0),
        field:     r(0.0, 205.0, 1280.0, 440.0),
        ticks_top: r(0.0, 211.0, 1280.0, 227.0),
        wave:      r(0.0, 228.0, 1280.0, 416.0),
        ticks_bot: r(0.0, 417.0, 1280.0, 433.0),
        zoom:      r(1150.0, 445.0, 1206.0, 467.0),

        player:    r(20.0, 488.0, 108.0, 576.0),
        track:     r(150.0, 488.0, 240.0, 576.0),
        auto_cue:  r(283.0, 498.0, 385.0, 525.0),
        time:      r(430.0, 485.0, 740.0, 580.0),
        tempo:     r(840.0, 485.0, 1035.0, 580.0),
        range:     r(947.0, 488.0, 1029.0, 512.0),
        bpm:       r(1075.0, 488.0, 1260.0, 576.0),
        flags:     r(127.0, 584.0, 1150.0, 599.0),
        overview:  r(127.0, 600.0, 1150.0, 670.0),
        address:   r(127.0, 672.0, 1150.0, 678.0),
        ruler:     r(127.0, 680.0, 1150.0, 698.0),
        mt:        r(1172.0, 594.0, 1260.0, 621.0),
        key:       r(1172.0, 630.0, 1260.0, 700.0),
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────────

/// Draw the playback screen. `lay` must come from [`layout`] on the screen
/// rect the shader viewports were taken from.
pub fn draw(ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, out: &mut Vec<Event>) {
    egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
        // Letterbox around the 16:9 screen, then the screen's own ground —
        // everything except the two shader rects.
        let p = ui.painter();
        for part in cover(ui.max_rect(), lay.screen, lay.screen) {
            p.rect_filled(part, 0.0, BG);
        }
        for part in cover(lay.screen, lay.wave, lay.overview) {
            p.rect_filled(part, 0.0, BG);
        }
        let u = lay.screen.height() / ASPECT.y; // points per unit pixel
        draw_header(ui, snap, lay, u);
        draw_field(ui, snap, lay, u, out);
        draw_info_block(ui, snap, lay, u, out);
    });
}

/// Text sized in unit pixels; returns the drawn rect.
fn label(ui: &Ui, pos: Pos2, a: Align2, s: impl ToString, px: f32, u: f32, c: Color32) -> Rect {
    ui.painter().text(pos, a, s, FontId::proportional(px * u), c)
}

fn outline(ui: &Ui, r: Rect, c: Color32, u: f32) {
    ui.painter().rect_stroke(r, 0.0, Stroke::new((1.5 * u).max(1.0), c));
}

fn draw_header(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, u: f32) {
    let p = ui.painter();
    p.rect_filled(lay.header, 0.0, HEADER);

    // Source tile: where the loaded track came from.
    let s = lay.source;
    p.rect_filled(s, 0.0, SOURCE_TILE);
    let src = if snap.source_link { "LINK" } else { "USB" };
    label(ui, Pos2::new(s.center().x, s.min.y + 66.0 * u), Align2::CENTER_CENTER, src, 18.0, u, TEXT);
    label(ui, Pos2::new(s.center().x, s.min.y + 38.0 * u), Align2::CENTER_CENTER, "▣", 24.0, u, TEXT);

    // Artwork: a record until the loader carries the track's own picture.
    let a = lay.art;
    p.rect_filled(a, 0.0, Color32::from_rgb(40, 40, 40));
    p.circle_filled(a.center(), a.width() * 0.42, Color32::from_rgb(70, 70, 70));
    p.circle_filled(a.center(), a.width() * 0.08, Color32::from_rgb(15, 15, 15));

    // Title, then length · BPM · key.
    let clip = ui.painter().with_clip_rect(lay.title.union(lay.track_row));
    let t = lay.title;
    clip.text(Pos2::new(t.min.x, t.center().y), Align2::LEFT_CENTER, format!("♪ {}", snap.title), FontId::proportional(33.0 * u), TEXT);
    let row = lay.track_row;
    let y = row.center().y;
    if snap.total_samples > 0 {
        let secs = snap.total_secs().round() as u64;
        clip.text(Pos2::new(row.min.x + 35.0 * u, y), Align2::LEFT_CENTER, format!("{:02}:{:02}", secs / 60, secs % 60), FontId::proportional(33.0 * u), TEXT);
        let bpm = snap.beat_grid.map(|g| format!("{:.1}", g.bpm)).unwrap_or_else(|| "0.0".into());
        clip.text(Pos2::new(row.min.x + 170.0 * u, y), Align2::LEFT_CENTER, bpm, FontId::proportional(33.0 * u), TEXT);
        if let Some(k) = snap.tags.key.as_deref() {
            clip.text(Pos2::new(row.min.x + 265.0 * u, y), Align2::LEFT_CENTER, k, FontId::proportional(33.0 * u), TEXT);
        }
    }
    // ⓘ
    let i = lay.info_btn;
    p.circle_filled(i.center(), i.width() * 0.5, TEXT);
    label(ui, i.center(), Align2::CENTER_CENTER, "i", 20.0, u, Color32::BLACK);

    // BEAT LOOP / KEY SHIFT / BEAT JUMP touch keys. Their panels are the next
    // phase; drawn so the bar is the unit's.
    for (k, (a, b)) in lay.keys.iter().zip([("BEAT", "LOOP"), ("KEY", "SHIFT"), ("BEAT", "JUMP")]) {
        p.rect_filled(*k, 0.0, KEY_FACE);
        let bar = Rect::from_center_size(Pos2::new(k.center().x, k.max.y - 3.0 * u), Vec2::new(56.0 * u, 3.0 * u));
        p.rect_filled(bar, 0.0, KEY_EDGE);
        label(ui, Pos2::new(k.center().x, k.min.y + 33.0 * u), Align2::CENTER_CENTER, a, 21.0, u, TEXT);
        label(ui, Pos2::new(k.center().x, k.min.y + 57.0 * u), Align2::CENTER_CENTER, b, 21.0, u, TEXT);
    }
}

/// Bars.beats to the next point, as the countdown shows it ("--.-" if none).
fn bars_text(v: Option<(u32, u32)>) -> String {
    match v {
        Some((bars, beats)) => format!("{bars}.{beats}"),
        None => "--.-".into(),
    }
}

fn draw_field(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    let p = ui.painter();

    // Beat countdowns: orange to the next memory point, blue to the next hot cue.
    let b = lay.bars;
    let mem = bars_to_next(snap, snap.memory_cues.iter().copied());
    let hot = bars_to_next(snap, snap.hot_cues.iter().flatten().copied());
    for (row, v, c) in [(0.25, mem, BARS_ORANGE), (0.75, hot, BARS_BLUE)] {
        let y = b.min.y + b.height() * row;
        let r = label(ui, Pos2::new(b.min.x + 62.0 * u, y), Align2::RIGHT_CENTER, bars_text(v), 22.0, u, c);
        label(ui, Pos2::new(r.max.x + 7.0 * u, y), Align2::LEFT_CENTER, "Bars", 15.0, u, c);
    }

    wave_touch(ui, snap, lay.wave, out);

    // Grid ticks above and below the waveform, red on downbeats, with the
    // beat numbers 16 / 32 under them — on the same column → x mapping the
    // shader uses, so they sit on the waveform's beats.
    if let (Some(g), true) = (snap.beat_grid, snap.total_samples > 0) {
        let hop = opendeck_analysis::waveform::HOP_SIZE as f64;
        let ch = snap.channels.max(1) as f64;
        let w = lay.wave;
        let cols = snap.cols_visible.max(1.0) as f64;
        let play_col = snap.position as f64 / ch / hop;
        let x_of = |frame: f64| w.min.x + ((frame / hop - play_col) / cols + 0.5) as f32 * w.width();
        let period = snap.sample_rate as f64 * 60.0 / g.bpm;
        let first_frame = (play_col - cols / 2.0) * hop;
        let k0 = ((first_frame - g.anchor_sample as f64) / period).floor() as i64;
        let k1 = k0 + (cols * hop / period).ceil() as i64 + 2;
        let stroke = |c| Stroke::new((2.0 * u).max(1.0), c);
        // The beat the playhead is in; the unit numbers the 16th and 32nd
        // beats ahead of it.
        let here = ((play_col * hop - g.anchor_sample as f64) / period).floor() as i64;
        for k in k0..=k1 {
            let x = x_of(g.sample_of_beat(k, snap.sample_rate) as f64);
            if x < w.min.x || x > w.max.x { continue; }
            let down = (k + g.downbeat_offset as i64).rem_euclid(4) == 0;
            for t in [lay.ticks_top, lay.ticks_bot] {
                p.line_segment([Pos2::new(x, t.min.y), Pos2::new(x, t.max.y)], stroke(if down { RED } else { TICK }));
            }
            if k == here + 16 || k == here + 32 {
                let num = if k == here + 16 { "16" } else { "32" };
                label(ui, Pos2::new(x, lay.ticks_bot.max.y + 8.0 * u), Align2::CENTER_CENTER, num, 17.0, u, TEXT);
            }
        }
    }

    // Playhead: white, through the whole field.
    let f = lay.field;
    p.line_segment([Pos2::new(f.center().x, f.min.y), Pos2::new(f.center().x, f.max.y)], Stroke::new((3.0 * u).max(1.0), TEXT));

    // ZOOM | GRID: the rotary's mode, a tap flips it.
    let z = lay.zoom;
    let (flip, _) = tap(ui, z.union(Rect::from_min_size(Pos2::new(z.max.x, z.min.y), Vec2::new(64.0 * u, z.height()))), "cdj-zoom-grid");
    if flip { out.push(Event::Ui(UiEvent::ZoomGridMode)); }
    let (zc, gc) = if snap.zoom_grid_mode { (DIM, ZOOM_BLUE) } else { (ZOOM_BLUE, DIM) };
    outline(ui, z, zc, u);
    label(ui, z.center(), Align2::CENTER_CENTER, "ZOOM", 15.0, u, zc);
    label(ui, Pos2::new(z.max.x + 9.0 * u, z.center().y), Align2::LEFT_CENTER, "– GRID", 15.0, u, gc);
}

fn draw_info_block(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    let p = ui.painter();
    // Hairline between the waveform field and the info block.
    let y = lay.screen.min.y + 476.0 * u;
    p.line_segment([Pos2::new(lay.screen.min.x, y), Pos2::new(lay.screen.max.x, y)], Stroke::new(2.0 * u, HEADER));

    // PLAYER
    let r = lay.player;
    outline(ui, r, BOX, u);
    label(ui, Pos2::new(r.center().x, r.min.y + 14.0 * u), Align2::CENTER_CENTER, "PLAYER", 13.0, u, TEXT);
    label(ui, Pos2::new(r.center().x, r.min.y + 54.0 * u), Align2::CENTER_CENTER, snap.player, 52.0, u, TEXT);

    // TRACK (no track numbers yet: the loaded track is 01)
    let r = lay.track;
    label(ui, Pos2::new(r.center().x, r.min.y + 17.0 * u), Align2::CENTER_CENTER, "TRACK", 16.0, u, TEXT);
    label(ui, Pos2::new(r.center().x, r.min.y + 57.0 * u), Align2::CENTER_CENTER, "01", 56.0, u, TEXT);

    // AUTO CUE caption
    if snap.auto_cue {
        let r = lay.auto_cue;
        outline(ui, r, RED, u);
        label(ui, r.center(), Align2::CENTER_CENTER, "AUTO CUE", 17.0, u, RED);
    }

    // Time: REMAIN / TIME, SINGLE, mm:ss + .msec
    let r = lay.time;
    label(ui, Pos2::new(r.min.x + 11.0 * u, r.min.y + 8.0 * u), Align2::LEFT_CENTER, if snap.remain_mode { "REMAIN" } else { "TIME" }, 15.0, u, TEXT);
    label(ui, Pos2::new(r.min.x + 266.0 * u, r.min.y + 9.0 * u), Align2::CENTER_CENTER, "SINGLE", 15.0, u, TEXT);
    let secs = if snap.remain_mode { snap.remaining_secs() } else { snap.elapsed_secs() };
    let (m, s) = ((secs / 60.0).floor() as u64, (secs % 60.0).floor() as u64);
    let ms = ((secs.fract()) * 1000.0).floor() as u64;
    let big = label(ui, Pos2::new(r.min.x + 9.0 * u, r.min.y + 57.0 * u), Align2::LEFT_CENTER, format!("{m:02}:{s:02}"), 84.0, u, TEXT);
    label(ui, Pos2::new(big.max.x + 2.0 * u, r.min.y + 72.0 * u), Align2::LEFT_CENTER, format!(".{ms:03}"), 44.0, u, TEXT);

    // TEMPO: range badge, ±% readout
    let r = lay.tempo;
    label(ui, Pos2::new(r.min.x + 29.0 * u, r.min.y + 11.0 * u), Align2::CENTER_CENTER, "TEMPO", 15.0, u, TEXT);
    p.rect_filled(lay.range, 0.0, ORANGE);
    label(ui, lay.range.center(), Align2::CENTER_CENTER, Settings::range_label(snap.tempo_range), 22.0, u, TEXT);
    let pct = snap.tempo_percent();
    let digits = if pct.abs() < 0.005 { "0.00".to_string() } else { format!("{:+.2}", pct) };
    let pc = label(ui, Pos2::new(r.max.x - 2.0 * u, r.min.y + 80.0 * u), Align2::RIGHT_BOTTOM, "%", 22.0, u, TEXT);
    label(ui, Pos2::new(pc.min.x - 3.0 * u, r.min.y + 85.0 * u), Align2::RIGHT_BOTTOM, digits, 54.0, u, TEXT);

    // BPM (orange box and digits while this deck is the sync master)
    let r = lay.bpm;
    let c = if snap.master { ORANGE } else { BOX };
    outline(ui, r, c, u);
    label(ui, Pos2::new(r.min.x + 9.0 * u, r.min.y + 11.0 * u), Align2::LEFT_CENTER, "BPM", 14.0, u, TEXT);
    if snap.master {
        let badge = Rect::from_min_max(Pos2::new(r.max.x - 58.0 * u, r.min.y), Pos2::new(r.max.x, r.min.y + 17.0 * u));
        p.rect_filled(badge, 0.0, ORANGE);
        label(ui, badge.center(), Align2::CENTER_CENTER, "MASTER", 13.0, u, TEXT);
    } else if snap.sync {
        let badge = Rect::from_min_max(Pos2::new(r.max.x - 50.0 * u, r.min.y), Pos2::new(r.max.x, r.min.y + 17.0 * u));
        p.rect_filled(badge, 0.0, ZOOM_BLUE);
        label(ui, badge.center(), Align2::CENTER_CENTER, "SYNC", 13.0, u, TEXT);
    }
    if let Some(bpm) = snap.bpm() {
        let tenths = (bpm * 10.0).round() as u64;
        let digit_c = if snap.master { ORANGE } else { TEXT };
        let dec = label(ui, Pos2::new(r.max.x - 12.0 * u, r.min.y + 80.0 * u), Align2::RIGHT_BOTTOM, format!(".{}", tenths % 10), 36.0, u, digit_c);
        label(ui, Pos2::new(dec.min.x, r.min.y + 86.0 * u), Align2::RIGHT_BOTTOM, tenths / 10, 66.0, u, digit_c);
    }

    // Overall waveform: needle search, hot-cue flags, the address bar and the
    // REMAIN time scale.
    needle_touch(ui, lay.overview, out);
    let ov = lay.overview;
    let total = snap.total_samples.max(1) as f32;
    for (i, c) in snap.hot_cues.iter().enumerate() {
        let Some(pos) = c else { continue };
        let x = ov.min.x + *pos as f32 / total * ov.width();
        let flag = Rect::from_min_size(Pos2::new(x, lay.flags.min.y), Vec2::new(13.0 * u, lay.flags.height()));
        p.rect_filled(flag, 0.0, CUE_GREEN);
        label(ui, flag.center(), Align2::CENTER_CENTER, (b'A' + i as u8) as char, 12.0, u, Color32::BLACK);
    }
    if snap.total_samples > 0 {
        p.rect_filled(lay.address, 0.0, RED);
        // Major ticks every minute counted back from the end, labelled −m:00;
        // a grey tick at each half minute between them.
        let dur = snap.total_secs();
        let ru = lay.ruler;
        let x_at = |t: f64| ru.min.x + (t / dur) as f32 * ru.width();
        let mut half = 1u32;
        loop {
            let t = dur - half as f64 * 30.0;
            if t < 0.0 { break; }
            let x = x_at(t);
            if half % 2 == 0 {
                p.line_segment([Pos2::new(x, ru.min.y), Pos2::new(x, ru.max.y - 2.0 * u)], Stroke::new(3.0 * u, TEXT));
                label(ui, Pos2::new(x + 6.0 * u, ru.center().y), Align2::LEFT_CENTER, format!("-{}:00", half / 2), 14.0, u, LABEL);
            } else {
                p.line_segment([Pos2::new(x, ru.min.y + 2.0 * u), Pos2::new(x, ru.max.y - 5.0 * u)], Stroke::new(1.0, DIM));
            }
            half += 1;
        }
        p.line_segment([Pos2::new(ru.max.x, ru.min.y), Pos2::new(ru.max.x, ru.max.y - 2.0 * u)], Stroke::new(3.0 * u, TEXT));
    }

    // MT, KEY
    if snap.key_lock {
        outline(ui, lay.mt, ORANGE, u);
        label(ui, lay.mt.center(), Align2::CENTER_CENTER, "MT", 21.0, u, ORANGE);
    }
    let k = lay.key;
    label(ui, Pos2::new(k.center().x, k.min.y + 15.0 * u), Align2::CENTER_CENTER, "KEY", 15.0, u, TEXT);
    if let Some(key) = snap.tags.key.as_deref() {
        label(ui, Pos2::new(k.center().x, k.min.y + 50.0 * u), Align2::CENTER_CENTER, key, 34.0, u, TEXT);
    }
}
