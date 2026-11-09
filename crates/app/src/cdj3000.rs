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

use crate::input::{ControlEvent, Event, UiEvent};
use crate::screen::{bars_to_next, cover, needle_touch, tap, wave_touch};
use crate::settings::Settings;
use crate::snapshot::DeckSnapshot;
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Shape, Stroke, Ui, Vec2};

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
const KEY_LIT:     Color32 = Color32::from_rgb(0, 120, 215);
const PANEL_FACE:  Color32 = Color32::from_rgb(40, 40, 40);
const PAGE2_EDGE:  Color32 = Color32::from_rgb(230, 230, 230);
/// Hot-cue flag colour for a cue with no colour of its own.
const CUE_GREEN:   Color32 = Color32::from_rgb(40, 226, 20);

// ── Touch panels ─────────────────────────────────────────────────────────────

/// The panel the top-bar keys open under the waveform. Screen state, so it
/// lives in egui's memory, not in the deck.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Panel {
    #[default]
    None,
    BeatLoop,
    BeatJump,
}

fn panel_id() -> Id { Id::new("cdj3k-panel") }

/// Which panel is open; `render_frame` asks before laying the screen out.
pub fn panel(ctx: &egui::Context) -> Panel {
    ctx.data(|d| d.get_temp(panel_id())).unwrap_or_default()
}

/// Dev / screenshots: `OPENDECK_CDJ_PANEL=loop|jump|info` opens that panel
/// (or INFO) at startup.
pub fn panel_from_env(ctx: &egui::Context) {
    let p = match std::env::var("OPENDECK_CDJ_PANEL").as_deref() {
        Ok("loop") => Panel::BeatLoop,
        Ok("jump") => Panel::BeatJump,
        Ok("info") => return set_info(ctx, true),
        _ => return,
    };
    set_panel(ctx, p);
}

fn set_panel(ctx: &egui::Context, p: Panel) {
    ctx.data_mut(|d| d.insert_temp(panel_id(), p));
}

fn info_id() -> Id { Id::new("cdj3k-info") }

/// The INFO screen is up (ⓘ opens it, its ✕ closes it). `render_frame`
/// leaves the enlarged waveform undrawn while it is.
pub fn info_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(info_id())).unwrap_or(false)
}

fn set_info(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(info_id(), on));
}

/// Page 1 or 2 of a panel; each panel remembers its own, as the unit does.
fn page(ctx: &egui::Context, which: Panel) -> u8 {
    ctx.data(|d| d.get_temp(Id::new(("cdj3k-page", which as u8)))).unwrap_or(1)
}

fn set_page(ctx: &egui::Context, which: Panel, page: u8) {
    ctx.data_mut(|d| d.insert_temp(Id::new(("cdj3k-page", which as u8)), page));
}

/// BEAT LOOP lengths in beats, page 1 then page 2 (read off the unit).
const LOOP_PAGES: [[f32; 8]; 2] = [
    [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0],
    [0.75, 1.0, 1.5, 3.0, 5.0, 6.0, 7.0, 9.0],
];
/// BEAT JUMP distances, page 1 then page 2; each is a ◀ / ▶ pair.
const JUMP_PAGES: [[f32; 4]; 2] = [[0.5, 1.0, 2.0, 4.0], [8.0, 16.0, 32.0, 64.0]];

/// A beat count as the unit labels it: 1/4, 3/4, 1/2, 3/2, 4…
fn beats_label(b: f32) -> String {
    for (v, s) in [(0.25, "1/4"), (0.5, "1/2"), (0.75, "3/4"), (1.5, "3/2")] {
        if (b - v).abs() < 1e-3 { return s.into(); }
    }
    if (b - b.round()).abs() < 1e-3 { format!("{}", b.round() as i64) } else { format!("{b:.2}") }
}

// ── Artwork ──────────────────────────────────────────────────────────────────

/// Decode embedded cover art (JPEG or PNG bytes) to an egui image.
fn decode_art(mime: &str, bytes: &[u8]) -> Option<egui::ColorImage> {
    if mime.contains("png") || bytes.starts_with(b"\x89PNG") {
        let mut r = png::Decoder::new(std::io::Cursor::new(bytes));
        r.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut rd = r.read_info().ok()?;
        let mut buf = vec![0; rd.output_buffer_size()];
        let info = rd.next_frame(&mut buf).ok()?;
        let px = &buf[..info.buffer_size()];
        let (w, h) = (info.width as usize, info.height as usize);
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => px.to_vec(),
            png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
            png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return None, // EXPAND turns these into RGB(A)
        };
        Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
    } else {
        let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
        let px = d.decode().ok()?;
        let info = d.info()?;
        let (w, h) = (info.width as usize, info.height as usize);
        match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => Some(egui::ColorImage::from_rgb([w, h], &px)),
            jpeg_decoder::PixelFormat::L8 => {
                let rgb: Vec<u8> = px.iter().flat_map(|&g| [g, g, g]).collect();
                Some(egui::ColorImage::from_rgb([w, h], &rgb))
            }
            _ => None,
        }
    }
}

/// The loaded track's cover as a texture, decoded once per track and kept in
/// egui's memory (keyed by a hash of the bytes). None without embedded art.
fn artwork(ctx: &egui::Context, snap: &DeckSnapshot) -> Option<egui::TextureHandle> {
    use std::hash::{Hash, Hasher};
    let (mime, bytes) = snap.tags.artwork.as_ref()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    bytes[..bytes.len().min(4096)].hash(&mut h);
    let key = h.finish();
    let id = Id::new("cdj3k-art");
    if let Some((k, tex)) = ctx.data(|d| d.get_temp::<(u64, Option<egui::TextureHandle>)>(id)) {
        if k == key { return tex; }
    }
    let tex = decode_art(mime, bytes).map(|img| ctx.load_texture("cdj3k-art", img, egui::TextureOptions::LINEAR));
    if tex.is_none() { log::debug!("cdj3000: cover art ({mime}, {} B) did not decode", bytes.len()); }
    ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
    tex
}

/// Cover art into `r`, or the record placeholder without any.
fn draw_art(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, r: Rect) {
    let p = ui.painter();
    match artwork(ctx, snap) {
        Some(tex) => {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            p.image(tex.id(), r, uv, Color32::WHITE);
        }
        None => {
            p.rect_filled(r, 0.0, Color32::from_rgb(40, 40, 40));
            p.circle_filled(r.center(), r.width() * 0.42, Color32::from_rgb(70, 70, 70));
            p.circle_filled(r.center(), r.width() * 0.08, Color32::from_rgb(15, 15, 15));
        }
    }
}

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
    /// Loop marker (triangle) row and the "↻ n" line under it.
    pub loop_mark: Rect,
    // touch panel (when one is open)
    pub panel:     [Rect; 8],
    pub page:      Rect,
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

/// Lay the playback screen out in `screen` (16:9). With a touch panel open
/// the waveform field shrinks to make room for its row of keys.
pub fn layout(screen: Rect, panel_open: bool) -> Layout {
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
        field:     if panel_open { r(0.0, 205.0, 1280.0, 346.0) } else { r(0.0, 205.0, 1280.0, 440.0) },
        ticks_top: r(0.0, 211.0, 1280.0, 227.0),
        wave:      if panel_open { r(0.0, 228.0, 1280.0, 318.0) } else { r(0.0, 228.0, 1280.0, 416.0) },
        ticks_bot: if panel_open { r(0.0, 322.0, 1280.0, 336.0) } else { r(0.0, 417.0, 1280.0, 433.0) },
        zoom:      if panel_open { r(1150.0, 351.0, 1206.0, 371.0) } else { r(1150.0, 445.0, 1206.0, 467.0) },
        loop_mark: if panel_open { r(0.0, 330.0, 1280.0, 374.0) } else { r(0.0, 425.0, 1280.0, 469.0) },
        panel:     std::array::from_fn(|i| { let x = 117.0 + 130.0 * i as f32; r(x, 383.0, x + 114.0, 466.0) }),
        page:      r(1175.0, 400.0, 1258.0, 447.0),

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
        if info_open(ctx) {
            draw_info(ui, ctx, snap, lay, u);
        } else {
            draw_header(ui, ctx, snap, lay, u);
            draw_field(ui, snap, lay, u, out);
            match panel(ctx) {
                Panel::BeatLoop => draw_loop_panel(ui, ctx, snap, lay, u, out),
                Panel::BeatJump => draw_jump_panel(ui, ctx, lay, u, out),
                Panel::None => {}
            }
        }
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

fn draw_header(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, u: f32) {
    let p = ui.painter();
    p.rect_filled(lay.header, 0.0, HEADER);

    // Source tile: where the loaded track came from.
    let s = lay.source;
    p.rect_filled(s, 0.0, SOURCE_TILE);
    let src = if snap.source_link { "LINK" } else { "USB" };
    label(ui, Pos2::new(s.center().x, s.min.y + 66.0 * u), Align2::CENTER_CENTER, src, 18.0, u, TEXT);
    label(ui, Pos2::new(s.center().x, s.min.y + 38.0 * u), Align2::CENTER_CENTER, "▣", 24.0, u, TEXT);

    draw_art(ui, ctx, snap, lay.art);

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
    // ⓘ opens INFO.
    let i = lay.info_btn;
    if tap(ui, i.expand(10.0 * u), "cdj-info").0 { set_info(ctx, true); }
    p.circle_filled(i.center(), i.width() * 0.5, TEXT);
    label(ui, i.center(), Align2::CENTER_CENTER, "i", 20.0, u, Color32::BLACK);

    // BEAT LOOP / KEY SHIFT / BEAT JUMP touch keys: each opens its panel
    // under the waveform (again, or another key, closes or swaps it). KEY
    // SHIFT has no panel until the deck can shift key.
    let open = panel(ctx);
    let targets = [Some(Panel::BeatLoop), None, Some(Panel::BeatJump)];
    for ((k, (a, b)), target) in lay.keys.iter().zip([("BEAT", "LOOP"), ("KEY", "SHIFT"), ("BEAT", "JUMP")]).zip(targets) {
        if let Some(t) = target {
            if tap(ui, *k, &format!("cdj-key-{a}{b}")).0 {
                set_panel(ctx, if open == t { Panel::None } else { t });
            }
        }
        let lit = target.is_some() && target == Some(open);
        p.rect_filled(*k, 0.0, if lit { KEY_LIT } else { KEY_FACE });
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
        let stroke = |c| Stroke::new((1.5 * u).max(1.0), c);
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

    // Active loop: an orange marker at its start and "↻ n" under it, kept
    // while the loop runs (the shader shades the looped span).
    if snap.loop_active && snap.total_samples > 0 {
        let hop = opendeck_analysis::waveform::HOP_SIZE as f64;
        let ch = snap.channels.max(1) as f64;
        let w = lay.wave;
        let cols = snap.cols_visible.max(1.0) as f64;
        let play_col = snap.position as f64 / ch / hop;
        let x = w.min.x + ((snap.loop_start as f64 / ch / hop - play_col) / cols + 0.5) as f32 * w.width();
        if x >= w.min.x && x <= w.max.x {
            let m = lay.loop_mark;
            let tri = vec![Pos2::new(x - 9.0 * u, m.min.y + 16.0 * u), Pos2::new(x + 9.0 * u, m.min.y + 16.0 * u), Pos2::new(x, m.min.y)];
            p.add(Shape::convex_polygon(tri, BARS_ORANGE, Stroke::NONE));
            let y = m.min.y + 32.0 * u;
            loop_icon(ui, Pos2::new(x + 2.0 * u, y), u, BARS_ORANGE);
            if snap.loop_beats > 0.0 {
                label(ui, Pos2::new(x + 30.0 * u, y), Align2::LEFT_CENTER, beats_label(snap.loop_beats), 24.0, u, BARS_ORANGE);
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

/// The loop glyph: a ↻-like hook with an arrowhead, drawn (not a font glyph).
fn loop_icon(ui: &Ui, c: Pos2, u: f32, col: Color32) {
    let s = Stroke::new(2.5 * u, col);
    let p = ui.painter();
    let (l, r, t, b) = (c.x - 9.0 * u, c.x + 11.0 * u, c.y - 6.0 * u, c.y + 7.0 * u);
    p.line_segment([Pos2::new(l, b), Pos2::new(r, b)], s);
    p.line_segment([Pos2::new(r, b), Pos2::new(r, t)], s);
    p.line_segment([Pos2::new(r, t), Pos2::new(l + 4.0 * u, t)], s);
    let head = vec![Pos2::new(l + 6.0 * u, t - 6.0 * u), Pos2::new(l + 6.0 * u, t + 6.0 * u), Pos2::new(l - 2.0 * u, t)];
    p.add(Shape::convex_polygon(head, col, Stroke::NONE));
}

/// The 1 | 2 page switch at the right of a panel.
fn page_switch(ui: &Ui, ctx: &egui::Context, which: Panel, r: Rect, u: f32) {
    let cur = page(ctx, which);
    if let Some(pos) = tap_pos(ui, r, &format!("cdj-page-{which:?}")) {
        set_page(ctx, which, if pos.x < r.center().x { 1 } else { 2 });
    }
    let p = ui.painter();
    outline(ui, r, DIM, u);
    let half = |n: u8| {
        let x0 = if n == 1 { r.min.x } else { r.center().x };
        Rect::from_min_max(Pos2::new(x0, r.min.y), Pos2::new(x0 + r.width() / 2.0, r.max.y)).shrink(3.0 * u)
    };
    p.rect_filled(half(cur), 0.0, DIM);
    for n in [1u8, 2] {
        let c = if n == cur { Color32::from_rgb(40, 40, 40) } else { DIM };
        label(ui, half(n).center(), Align2::CENTER_CENTER, n, 24.0, u, c);
    }
}

/// Where a tap on `r` landed, if it was tapped this frame.
fn tap_pos(ui: &Ui, r: Rect, name: &str) -> Option<Pos2> {
    let resp = ui.interact(r, Id::new(name), egui::Sense::click());
    resp.clicked().then(|| resp.interact_pointer_pos()).flatten()
}

/// BEAT LOOP: eight lengths per page; the running loop's key is lit, and
/// tapping it again ends the loop (the deck's BeatLoop does both).
fn draw_loop_panel(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    let pg = page(ctx, Panel::BeatLoop);
    let edge = if pg == 1 { BARS_ORANGE } else { PAGE2_EDGE };
    for (i, (r, beats)) in lay.panel.iter().zip(LOOP_PAGES[pg as usize - 1]).enumerate() {
        if tap(ui, *r, &format!("cdj-loop-{pg}-{i}")).0 {
            out.push(Event::Deck(ControlEvent::BeatLoop { beats, held: false }));
        }
        let lit = snap.loop_active && (snap.loop_beats - beats).abs() < 1e-3;
        ui.painter().rect_filled(*r, 0.0, if lit { BARS_ORANGE } else { PANEL_FACE });
        outline(ui, *r, edge, u);
        label(ui, r.center(), Align2::CENTER_CENTER, beats_label(beats), 26.0, u, TEXT);
    }
    page_switch(ui, ctx, Panel::BeatLoop, lay.page, u);
}

/// BEAT JUMP: four distances per page, each a ◀◀ / ▶▶ pair of keys.
fn draw_jump_panel(ui: &Ui, ctx: &egui::Context, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    let pg = page(ctx, Panel::BeatJump);
    let s = lay.screen.height() / ASPECT.y;
    let at = |x: f32| Rect::from_min_max(
        Pos2::new(lay.screen.min.x + x * s, lay.panel[0].min.y),
        Pos2::new(lay.screen.min.x + (x + 114.0) * s, lay.panel[0].max.y),
    );
    for (i, beats) in JUMP_PAGES[pg as usize - 1].iter().copied().enumerate() {
        let x = 127.0 + 262.0 * i as f32;
        for (r, fwd) in [(at(x), false), (at(x + 126.0), true)] {
            if tap(ui, r, &format!("cdj-jump-{pg}-{i}-{fwd}")).0 {
                out.push(Event::Deck(ControlEvent::BeatJump { beats: if fwd { beats } else { -beats } }));
            }
            let p = ui.painter();
            p.rect_filled(r, 0.0, PANEL_FACE);
            outline(ui, r, DIM, u);
            // Direction tab: top-right ◀◀ on the back key, top-left ▶▶ on the forward key.
            let tab = if fwd {
                Rect::from_min_size(r.min, Vec2::new(32.0 * u, 14.0 * u))
            } else {
                Rect::from_min_size(Pos2::new(r.max.x - 32.0 * u, r.min.y), Vec2::new(32.0 * u, 14.0 * u))
            };
            p.rect_filled(tab, 0.0, DIM);
            label(ui, tab.center(), Align2::CENTER_CENTER, if fwd { "▶▶" } else { "◀◀" }, 10.0, u, Color32::from_rgb(40, 40, 40));
            label(ui, r.center(), Align2::CENTER_CENTER, beats_label(beats), 26.0, u, TEXT);
        }
    }
    page_switch(ui, ctx, Panel::BeatJump, lay.page, u);
}

// ── INFO ─────────────────────────────────────────────────────────────────────

/// Small line icons standing in for the unit's glyphs (the default font has
/// none of them).
#[derive(Clone, Copy)]
enum Icon { Disc, Person, Building, Bubble, Calendar, Clock }

fn icon(ui: &Ui, kind: Icon, c: Pos2, u: f32) {
    let p = ui.painter();
    let s = Stroke::new(1.5 * u, TEXT);
    let half = 11.0 * u;
    let sq = Rect::from_center_size(c, Vec2::splat(2.0 * half));
    match kind {
        Icon::Disc => {
            p.rect_stroke(sq, 0.0, s);
            p.circle_stroke(c, half * 0.7, s);
            p.circle_stroke(c, half * 0.2, s);
        }
        Icon::Person => {
            p.circle_stroke(Pos2::new(c.x, c.y - half * 0.35), half * 0.4, s);
            p.line_segment([Pos2::new(c.x - half * 0.8, c.y + half), Pos2::new(c.x - half * 0.5, c.y + half * 0.25)], s);
            p.line_segment([Pos2::new(c.x - half * 0.5, c.y + half * 0.25), Pos2::new(c.x + half * 0.5, c.y + half * 0.25)], s);
            p.line_segment([Pos2::new(c.x + half * 0.5, c.y + half * 0.25), Pos2::new(c.x + half * 0.8, c.y + half)], s);
        }
        Icon::Building => {
            let b = Rect::from_center_size(c, Vec2::new(half * 1.4, half * 2.0));
            p.rect_stroke(b, 0.0, s);
            for i in 0..3 { for j in 0..3 {
                p.circle_filled(Pos2::new(b.min.x + b.width() * (0.25 + 0.25 * i as f32), b.min.y + b.height() * (0.25 + 0.2 * j as f32)), 1.2 * u, TEXT);
            } }
        }
        Icon::Bubble => {
            let b = Rect::from_center_size(Pos2::new(c.x, c.y - 2.0 * u), Vec2::new(half * 2.0, half * 1.4));
            p.rect_stroke(b, 2.0 * u, s);
            p.line_segment([Pos2::new(b.min.x + 5.0 * u, b.max.y), Pos2::new(b.min.x + 5.0 * u, b.max.y + 6.0 * u)], s);
        }
        Icon::Calendar => {
            let b = Rect::from_center_size(c, Vec2::new(half * 2.0, half * 1.7));
            p.rect_stroke(b, 0.0, s);
            p.line_segment([Pos2::new(b.min.x, b.min.y + 5.0 * u), Pos2::new(b.max.x, b.min.y + 5.0 * u)], s);
        }
        Icon::Clock => {
            p.circle_stroke(c, half, s);
            p.line_segment([c, Pos2::new(c.x, c.y - half * 0.6)], s);
            p.line_segment([c, Pos2::new(c.x + half * 0.5, c.y)], s);
        }
    }
}

/// The unit's boxed-text icons (BPM, b#, bps).
fn boxed(ui: &Ui, s: &str, c: Pos2, u: f32) {
    let r = Rect::from_center_size(c, Vec2::new(24.0 * u, 18.0 * u));
    ui.painter().rect_stroke(r, 0.0, Stroke::new(1.5 * u, TEXT));
    label(ui, c, Align2::CENTER_CENTER, s, 10.0, u, TEXT);
}

fn star(ui: &Ui, c: Pos2, r: f32, u: f32) {
    let pts: Vec<Pos2> = (0..10).map(|i| {
        let a = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
        let rr = if i % 2 == 0 { r } else { r * 0.45 };
        Pos2::new(c.x + rr * a.cos(), c.y + rr * a.sin())
    }).collect();
    ui.painter().add(Shape::closed_line(pts, Stroke::new(1.5 * u, TEXT)));
}

/// INFO: the loaded track's details over the top of the screen; the info
/// block and overview stay live under it. Other players' tabs stay dim until
/// their details come over the link.
fn draw_info(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, u: f32) {
    let sc = lay.screen;
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(
        Pos2::new(sc.min.x + x0 * u, sc.min.y + y0 * u), Pos2::new(sc.min.x + x1 * u, sc.min.y + y1 * u));
    let at = |x: f32, y: f32| Pos2::new(sc.min.x + x * u, sc.min.y + y * u);
    let p = ui.painter();
    const BODY: Color32 = Color32::from_rgb(48, 48, 48);
    const RULE: Color32 = Color32::from_rgb(90, 90, 90);

    // Header: INFO, EDIT RATING (no ratings yet: drawn disabled), ✕.
    p.rect_filled(r(0.0, 0.0, 1280.0, 90.0), 0.0, HEADER);
    label(ui, at(45.0, 45.0), Align2::LEFT_CENTER, "INFO", 32.0, u, TEXT);
    let er = r(1022.0, 0.0, 1135.0, 90.0);
    p.rect_filled(er, 0.0, KEY_FACE);
    label(ui, Pos2::new(er.center().x, sc.min.y + 33.0 * u), Align2::CENTER_CENTER, "EDIT", 21.0, u, DIM);
    label(ui, Pos2::new(er.center().x, sc.min.y + 57.0 * u), Align2::CENTER_CENTER, "RATING", 21.0, u, DIM);
    let x = r(1183.0, 22.0, 1230.0, 70.0);
    if tap(ui, x, "cdj-info-close").0 { set_info(ctx, false); }
    p.rect_filled(x, 0.0, Color32::from_rgb(130, 130, 130));
    let xs = Stroke::new(2.0 * u, Color32::from_rgb(40, 40, 40));
    let xi = x.shrink(12.0 * u);
    p.line_segment([xi.left_top(), xi.right_bottom()], xs);
    p.line_segment([xi.right_top(), xi.left_bottom()], xs);

    // Player tabs.
    for n in 1..=6u8 {
        let y = 92.0 + 64.0 * (n - 1) as f32;
        let t = r(0.0, y, 100.0, y + 62.0);
        let me = n == snap.player;
        p.rect_filled(t, 0.0, if me { KEY_LIT } else { Color32::from_rgb(25, 25, 25) });
        let c = if me { TEXT } else { Color32::from_rgb(80, 80, 80) };
        label(ui, Pos2::new(t.center().x, t.min.y + 15.0 * u), Align2::CENTER_CENTER, "PLAYER", 12.0, u, c);
        label(ui, Pos2::new(t.center().x, t.min.y + 40.0 * u), Align2::CENTER_CENTER, n, 28.0, u, c);
    }

    p.rect_filled(r(102.0, 92.0, 1280.0, 475.0), 0.0, BODY);
    let t = &snap.tags;
    let dash = |o: &Option<String>| o.clone().unwrap_or_default();

    // Source + artwork, rating, then BPM / key / length / colour.
    let src = r(132.0, 112.0, 222.0, 202.0);
    p.rect_filled(src, 0.0, SOURCE_TILE);
    label(ui, Pos2::new(src.center().x, src.min.y + 64.0 * u), Align2::CENTER_CENTER, if snap.source_link { "LINK" } else { "USB" }, 18.0, u, TEXT);
    draw_art(ui, ctx, snap, r(222.0, 112.0, 312.0, 202.0));
    for i in 0..5 { star(ui, at(155.0 + 33.0 * i as f32, 237.0), 13.0 * u, u); }
    for y in [262.0, 313.0, 364.0, 415.0] {
        p.line_segment([at(140.0, y), at(312.0, y)], Stroke::new(1.0, RULE));
    }
    boxed(ui, "BPM", at(159.0, 288.0), u);
    if let Some(g) = snap.beat_grid {
        label(ui, at(185.0, 288.0), Align2::LEFT_CENTER, format!("{:.1}", g.bpm), 24.0, u, TEXT);
    }
    boxed(ui, "b#", at(159.0, 338.0), u);
    label(ui, at(185.0, 338.0), Align2::LEFT_CENTER, dash(&t.key), 24.0, u, TEXT);
    icon(ui, Icon::Clock, at(159.0, 390.0), u);
    if snap.total_samples > 0 {
        let secs = snap.total_secs().round() as u64;
        label(ui, at(180.0, 390.0), Align2::LEFT_CENTER, format!("{:02}:{:02}", secs / 60, secs % 60), 30.0, u, TEXT);
    }
    p.circle_filled(at(159.0, 441.0), 11.0 * u, SOURCE_TILE);

    // Title, album, artist across the page.
    let rows: [(Option<Icon>, String); 3] = [
        (None, snap.title.to_string()),
        (Some(Icon::Disc), dash(&t.album)),
        (Some(Icon::Person), dash(&t.artist)),
    ];
    let clip = p.with_clip_rect(r(362.0, 110.0, 1228.0, 262.0));
    for (i, (ic, text)) in rows.iter().enumerate() {
        let y = 135.0 + 51.0 * i as f32;
        match ic {
            Some(k) => icon(ui, *k, at(379.0, y), u),
            None => { label(ui, at(379.0, y), Align2::CENTER_CENTER, "♪", 30.0, u, TEXT); }
        }
        clip.text(at(400.0, y), Align2::LEFT_CENTER, text, FontId::proportional(30.0 * u), TEXT);
        p.line_segment([at(362.0, y + 25.0), at(1228.0, y + 25.0)], Stroke::new(1.0, RULE));
    }

    // Detail columns: genre / label / remixer / original artist; bitrate,
    // comment; date added; release date.
    for (y, k, v) in [(288.0, Icon::Disc, dash(&t.genre)), (338.0, Icon::Building, String::new()),
                      (390.0, Icon::Person, String::new()), (441.0, Icon::Person, String::new())] {
        icon(ui, k, at(379.0, y), u);
        label(ui, at(400.0, y), Align2::LEFT_CENTER, v, 24.0, u, TEXT);
        p.line_segment([at(362.0, y + 25.0), at(650.0, y + 25.0)], Stroke::new(1.0, RULE));
    }
    boxed(ui, "bps", at(716.0, 288.0), u);
    p.line_segment([at(700.0, 313.0), at(860.0, 313.0)], Stroke::new(1.0, RULE));
    icon(ui, Icon::Bubble, at(716.0, 340.0), u);
    p.with_clip_rect(r(740.0, 320.0, 1228.0, 360.0))
        .text(at(740.0, 338.0), Align2::LEFT_CENTER, dash(&t.comment), FontId::proportional(22.0 * u), TEXT);
    icon(ui, Icon::Calendar, at(907.0, 288.0), u);
    p.line_segment([at(893.0, 313.0), at(1003.0, 313.0)], Stroke::new(1.0, RULE));
    icon(ui, Icon::Calendar, at(1067.0, 288.0), u);
    label(ui, at(1090.0, 288.0), Align2::LEFT_CENTER, dash(&t.year), 22.0, u, TEXT);
    p.line_segment([at(1050.0, 313.0), at(1228.0, 313.0)], Stroke::new(1.0, RULE));
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
