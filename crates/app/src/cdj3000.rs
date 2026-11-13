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
//! buttons arrive as events from the surfaces, or from the faceplate drawn
//! around the screen (`cdjface.rs`).

use crate::browser::Browser;
use crate::input::{ControlEvent, Event, Screen as TopScreen, UiEvent};
use crate::screen::ScreenView;
use crate::settings::MENU;
use crate::taglist::TagList;
use crate::screen::{bars_to_next, cover, needle_touch, stem_touch, tap, wave_touch, STEM_NAMES};
use crate::settings::Settings;
use crate::snapshot::{DeckSnapshot, StemsState};
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
/// TEMPO range badges: ±6 green, ±10 orange-red, ±16 white (WIDE is red
/// text, no badge), read off the emulated unit.
const RANGE_6:     Color32 = Color32::from_rgb(0, 255, 0);
const RANGE_16:    Color32 = Color32::WHITE;
const POPUP:       Color32 = Color32::from_rgb(26, 26, 26);
const POPUP_EDGE:  Color32 = Color32::from_rgb(170, 170, 170);
const POPUP_TEXT:  Color32 = Color32::from_rgb(200, 200, 200);

// ── Touch panels ─────────────────────────────────────────────────────────────

/// The panel the top-bar keys open under the waveform. Screen state, so it
/// lives in egui's memory, not in the deck.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Panel {
    #[default]
    None,
    BeatLoop,
    BeatJump,
    /// Stems (MENU → STEMS on): FULL MIX, TOGGLE / VOLUME, and the stems.
    Stems,
}

fn panel_id() -> Id { Id::new("cdj3k-panel") }

/// Which panel is open; `render_frame` asks before laying the screen out.
pub fn panel(ctx: &egui::Context) -> Panel {
    ctx.data(|d| d.get_temp(panel_id())).unwrap_or_default()
}

/// Dev / screenshots: `OPENDECK_CDJ_PANEL=loop|jump` opens that panel at
/// startup (INFO, BROWSE and the other screens are deck screen modes).
pub fn panel_from_env(ctx: &egui::Context) {
    let p = match std::env::var("OPENDECK_CDJ_PANEL").as_deref() {
        Ok("loop") => Panel::BeatLoop,
        Ok("jump") => Panel::BeatJump,
        Ok("stems") => Panel::Stems,
        _ => return,
    };
    set_panel(ctx, p);
}

/// Open a panel from outside the screen (the faceplate's 4 / 8 loop keys).
pub(crate) fn open_panel(ctx: &egui::Context, p: Panel) { set_panel(ctx, p); }

fn set_panel(ctx: &egui::Context, p: Panel) {
    ctx.data_mut(|d| d.insert_temp(panel_id(), p));
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

/// A track's cover as a texture, decoded once per cover and kept in egui's
/// memory under `slot` (keyed by a hash of the bytes; the loaded track and the
/// BROWSE highlight each keep one). None without embedded art.
fn artwork(ctx: &egui::Context, tags: &opendeck_decode::TrackTags, slot: &str) -> Option<egui::TextureHandle> {
    use std::hash::{Hash, Hasher};
    let (mime, bytes) = tags.artwork.as_ref()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    bytes[..bytes.len().min(4096)].hash(&mut h);
    let key = h.finish();
    let id = Id::new(("cdj3k-art", slot));
    if let Some((k, tex)) = ctx.data(|d| d.get_temp::<(u64, Option<egui::TextureHandle>)>(id)) {
        if k == key { return tex; }
    }
    let tex = decode_art(mime, bytes).map(|img| ctx.load_texture(format!("cdj3k-art-{slot}"), img, egui::TextureOptions::LINEAR));
    if tex.is_none() { log::debug!("cdj3000: cover art ({mime}, {} B) did not decode", bytes.len()); }
    ctx.data_mut(|d| d.insert_temp(id, (key, tex.clone())));
    tex
}

/// Cover art into `r`, or the record placeholder without any.
fn draw_art(ui: &Ui, ctx: &egui::Context, tags: &opendeck_decode::TrackTags, slot: &str, r: Rect) {
    let p = ui.painter();
    match artwork(ctx, tags, slot) {
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
    /// STEMS key, left of BEAT LOOP (only with the MENU's STEMS setting on).
    pub stems_key: Rect,
    // waveform field
    pub bars:      Rect,
    /// The master's player-number badge left of the Bars rows, and the slot
    /// right of them that shows the phase meter (callout 11).
    pub master_badge: Rect,
    pub meter:     Rect,
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
    pub a_hot_cue: Rect,
    pub auto_cue:  Rect,
    pub quantize:  Rect,
    pub beat_jump: Rect,
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
    // list screens: the status strip along the bottom
    pub strip_overview: Rect,
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
        stems_key: r(774.0, 0.0, 886.0, 90.0),

        bars:      r(160.0, 110.0, 340.0, 185.0),
        master_badge: r(124.0, 117.0, 145.0, 138.0),
        meter:     r(272.0, 110.0, 1008.0, 182.0),
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
        a_hot_cue: r(283.0, 497.0, 385.0, 525.0),
        auto_cue:  r(283.0, 537.0, 385.0, 565.0),
        quantize:  r(20.0, 586.0, 108.0, 637.0),
        beat_jump: r(20.0, 647.0, 108.0, 698.0),
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
        strip_overview: r(260.0, 650.0, 890.0, 690.0),
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────────

/// Draw the screen for the deck's `view`. `lay` must come from [`layout`] on
/// the screen rect the shader viewports were taken from. The CDJ has no
/// PERFORM screen (its pads are hardware), so PERFORM shows playback.
#[allow(clippy::too_many_arguments)]
pub fn draw(ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, view: &ScreenView, tag_list: &TagList,
            face: Option<&crate::cdjface::CdjFace>, chrome: &mut crate::chrome::ChromeCache, out: &mut Vec<Event>) {
    egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
        // The faceplate around the screen (its buttons), or a plain
        // letterbox; then the screen's own ground, everything except the two
        // shader rects.
        let p = ui.painter();
        match face {
            Some(f) => crate::cdjface::draw(ui, ctx, snap, f, lay.screen, view, tag_list, chrome, out),
            None => for part in cover(ui.max_rect(), lay.screen, lay.screen) { p.rect_filled(part, 0.0, BG); },
        }
        for part in cover(lay.screen, lay.wave, lay.overview) {
            p.rect_filled(part, 0.0, BG);
        }
        let u = lay.screen.height() / ASPECT.y; // points per unit pixel
        match view {
            ScreenView::Browse(b) => return draw_browse(ui, snap, lay, b, tag_list, u, out),
            ScreenView::Source(b, cursor, loaded) => return draw_source(ui, snap, lay, b, *cursor, *loaded, u, out),
            ScreenView::TagList => return draw_tag_list(ui, snap, lay, tag_list, u, out),
            ScreenView::Menu(settings, cursor) => return draw_utility(ui, snap, lay, settings, *cursor, u, out),
            ScreenView::Info => draw_info(ui, ctx, snap, lay, u, out),
            ScreenView::Playback | ScreenView::Perform | ScreenView::History(..) => {
                draw_header(ui, ctx, snap, lay, u, out);
                draw_field(ui, snap, lay, u, out);
                match panel(ctx) {
                    // GRID ADJUST's keys take the panel row.
                    _ if snap.zoom_grid_mode => draw_grid_keys(ui, snap, lay, u, out),
                    Panel::BeatLoop => draw_loop_panel(ui, ctx, snap, lay, u, out),
                    Panel::BeatJump => draw_jump_panel(ui, ctx, lay, u, out),
                    Panel::Stems if snap.stems.state != StemsState::Off => draw_stems_panel(ui, ctx, snap, lay, u, out),
                    Panel::Stems | Panel::None => {}
                }
                if let ScreenView::History(items, cursor) = view {
                    draw_info_block(ui, snap, lay, u, out);
                    return draw_history(ui, ctx, lay, items, *cursor, u, out);
                }
            }
        }
        draw_info_block(ui, snap, lay, u, out);
    });
    if let Some(msg) = snap.notice { draw_notice(ctx, lay, msg); }
}

/// The unit's pop-up: a dark box with a light edge over the middle of the
/// screen, its lines left-aligned (measured off "No LINKED Players found.").
fn draw_notice(ctx: &egui::Context, lay: &Layout, msg: &str) {
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("cdj3k-notice")));
    let (s, u) = (lay.screen, lay.screen.height() / ASPECT.y);
    let at = |x: f32, y: f32| Pos2::new(s.min.x + x * u, s.min.y + y * u);
    let lines: Vec<&str> = msg.lines().collect();
    let h = 70.0 + 48.0 * lines.len() as f32;
    let r = Rect::from_min_max(at(200.0, 160.0), at(1080.0, 160.0 + h));
    p.rect_filled(r, 0.0, POPUP);
    p.rect_stroke(r, 0.0, Stroke::new((2.0 * u).max(1.0), POPUP_EDGE));
    for (i, l) in lines.iter().enumerate() {
        p.text(at(257.0, 160.0 + 59.0 + 48.0 * i as f32), Align2::LEFT_CENTER, *l, FontId::proportional(31.0 * u), POPUP_TEXT);
    }
}

/// Text sized in unit pixels; returns the drawn rect.
fn label(ui: &Ui, pos: Pos2, a: Align2, s: impl ToString, px: f32, u: f32, c: Color32) -> Rect {
    ui.painter().text(pos, a, s, FontId::proportional(px * u), c)
}

fn outline(ui: &Ui, r: Rect, c: Color32, u: f32) {
    ui.painter().rect_stroke(r, 0.0, Stroke::new((1.5 * u).max(1.0), c));
}

fn draw_header(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    let p = ui.painter();
    p.rect_filled(lay.header, 0.0, HEADER);

    // Source tile: where the loaded track came from.
    let s = lay.source;
    p.rect_filled(s, 0.0, SOURCE_TILE);
    let src = if snap.source_link { "LINK" } else { "USB" };
    label(ui, Pos2::new(s.center().x, s.min.y + 66.0 * u), Align2::CENTER_CENTER, src, 18.0, u, TEXT);
    label(ui, Pos2::new(s.center().x, s.min.y + 38.0 * u), Align2::CENTER_CENTER, "▣", 24.0, u, TEXT);

    draw_art(ui, ctx, snap.tags, "deck", lay.art);

    // Title, then length · BPM · key.
    let stems_on = snap.stems.state != StemsState::Off;
    let mut title_area = lay.title.union(lay.track_row);
    if stems_on { title_area.max.x = lay.stems_key.min.x - 8.0 * u; }
    let clip = ui.painter().with_clip_rect(title_area);
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
    if tap(ui, i.expand(10.0 * u), "cdj-info").0 { out.push(Event::Ui(UiEvent::Screen(TopScreen::Info))); }
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

    // STEMS (MENU → STEMS on): opens the stems panel.  Grey while the
    // track's stems load or when it has none; a blue underline while the
    // mix is changed and the panel is closed.
    if stems_on {
        let k = lay.stems_key;
        if tap(ui, k, "cdj-key-stems").0 {
            set_panel(ctx, if open == Panel::Stems { Panel::None } else { Panel::Stems });
        }
        let ready = snap.stems.state == StemsState::Ready;
        p.rect_filled(k, 0.0, if open == Panel::Stems { KEY_LIT } else { KEY_FACE });
        let changed = ready && snap.stems.changed() && open != Panel::Stems;
        let bar = Rect::from_center_size(Pos2::new(k.center().x, k.max.y - 3.0 * u), Vec2::new(if changed { 90.0 } else { 56.0 } * u, 3.0 * u));
        p.rect_filled(bar, 0.0, if changed { KEY_LIT } else { KEY_EDGE });
        label(ui, Pos2::new(k.center().x, k.min.y + 45.0 * u), Align2::CENTER_CENTER, "STEMS", 21.0, u, if ready { TEXT } else { DIM });
    }
}

/// Bars.beats to the next point, as the countdown shows it ("--.-" if none).
fn bars_text(v: Option<(u32, u32)>) -> String {
    match v {
        Some((bars, beats)) => format!("{bars:02}.{beats}"),
        None => "--.-".into(),
    }
}

fn draw_field(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    let p = ui.painter();

    // Beat countdowns. The lower (blue) row counts to the next memory point
    // and reads a red 00.0 while the playhead sits on one; hot cues are not
    // counted. The upper (orange) row stays "--.-" here (on the unit it
    // counts to a touched overview point, which freedj has not got).
    let b = lay.bars;
    let on_memory = snap.beat_grid.filter(|g| g.bpm > 0.0).is_some_and(|g| {
        let half_beat = snap.sample_rate as f64 * 30.0 / g.bpm * snap.channels as f64;
        snap.memory_cues.iter().any(|&m| (m as f64 - snap.position as f64).abs() < half_beat)
    });
    let mem = if on_memory { Some((0, 0)) } else { bars_to_next(snap, snap.memory_cues.iter().copied()) };
    for (row, v, c) in [(0.25, None, BARS_ORANGE), (0.75, mem, if on_memory { RED } else { BARS_BLUE })] {
        let y = b.min.y + b.height() * row;
        let r = label(ui, Pos2::new(b.min.x + 62.0 * u, y), Align2::RIGHT_CENTER, bars_text(v), 22.0, u, c);
        label(ui, Pos2::new(r.max.x + 7.0 * u, y), Align2::LEFT_CENTER, "Bars", 15.0, u, c);
    }

    draw_phase_meter(ui, snap, lay, u);

    wave_touch(ui, snap, lay.wave, out);

    // Grid ticks above and below the waveform, red on downbeats — on the
    // same column → x mapping the shader uses, so they sit on the waveform's
    // beats — and the 16 / 32 marks under them.
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
        for k in k0..=k1 {
            let x = x_of(g.sample_of_beat(k, snap.sample_rate) as f64);
            if x < w.min.x || x > w.max.x { continue; }
            let down = (k + g.downbeat_offset as i64).rem_euclid(4) == 0;
            for t in [lay.ticks_top, lay.ticks_bot] {
                p.line_segment([Pos2::new(x, t.min.y), Pos2::new(x, t.max.y)], stroke(if down { RED } else { TICK }));
            }
        }
        // 16 and 32 beats ahead of the playhead, not on a particular beat: a
        // CDJ-3000 keeps them still while the waveform scrolls under them,
        // past the track's end too (2026-11-12, at 3.1, 5.3 and 8.0 s).
        for (n, num) in [(16.0, "16"), (32.0, "32")] {
            let x = ahead_x(w, n * period / hop, cols);
            if x <= w.max.x {
                label(ui, Pos2::new(x, lay.ticks_bot.max.y + 8.0 * u), Align2::CENTER_CENTER, num, 17.0, u, TEXT);
            }
        }
    }

    // Hot cues, memory points and the cue on the enlarged waveform: a green
    // lettered box and a red ▼ over the top ticks, an orange ▲ under the
    // bottom ones.
    if snap.total_samples > 0 {
        let hop = opendeck_analysis::waveform::HOP_SIZE as f64;
        let ch = snap.channels.max(1) as f64;
        let w = lay.wave;
        let cols = snap.cols_visible.max(1.0) as f64;
        let play_col = snap.position as f64 / ch / hop;
        let x_of = |s: u64| w.min.x + ((s as f64 / ch / hop - play_col) / cols + 0.5) as f32 * w.width();
        let on = |x: f32| x >= w.min.x - 20.0 * u && x <= w.max.x;
        let top = lay.ticks_top.min.y - 6.0 * u;
        for &m in snap.memory_cues {
            let x = x_of(m);
            if !on(x) { continue; }
            p.add(Shape::convex_polygon(vec![Pos2::new(x - 8.0 * u, top), Pos2::new(x + 8.0 * u, top), Pos2::new(x, top + 14.0 * u)],
                                        RED, Stroke::NONE));
        }
        for (i, c) in snap.hot_cues.iter().enumerate() {
            let Some(pos) = c else { continue };
            let x = x_of(*pos);
            if !on(x) { continue; }
            let b = Rect::from_min_size(Pos2::new(x, top), Vec2::new(17.0 * u, 16.0 * u));
            p.rect_filled(b, 0.0, CUE_GREEN);
            label(ui, b.center(), Align2::CENTER_CENTER, (b'A' + i as u8) as char, 13.0, u, Color32::BLACK);
        }
        let x = x_of(snap.cue_point);
        if on(x) {
            let y = lay.ticks_bot.max.y;
            p.add(Shape::convex_polygon(vec![Pos2::new(x, y), Pos2::new(x + 9.0 * u, y + 16.0 * u), Pos2::new(x - 9.0 * u, y + 16.0 * u)],
                                        BARS_ORANGE, Stroke::NONE));
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
    // The boxed word is the live mode: "[ZOOM] – GRID" or "– ZOOM [GRID]".
    if snap.zoom_grid_mode {
        let g = Rect::from_min_size(Pos2::new(z.min.x + 68.0 * u, z.min.y), Vec2::new(47.0 * u, z.height()));
        outline(ui, g, ZOOM_BLUE, u);
        label(ui, g.center(), Align2::CENTER_CENTER, "GRID", 15.0, u, ZOOM_BLUE);
        label(ui, Pos2::new(g.min.x - 9.0 * u, z.center().y), Align2::RIGHT_CENTER, "– ZOOM", 15.0, u, DIM);
    } else {
        outline(ui, z, ZOOM_BLUE, u);
        label(ui, z.center(), Align2::CENTER_CENTER, "ZOOM", 15.0, u, ZOOM_BLUE);
        label(ui, Pos2::new(z.max.x + 9.0 * u, z.center().y), Align2::LEFT_CENTER, "– GRID", 15.0, u, DIM);
    }
}

/// Whether the phase-meter slot shows the meter (the unit's PHASE METER view)
/// or is left to the waveform view. Screen state, like the panels; a tap on
/// the slot switches, as on the unit.
fn meter_shown(ctx: &egui::Context) -> bool {
    !ctx.data(|d| d.get_temp::<bool>(Id::new("cdj3k-meter-off"))).unwrap_or(false)
}

/// The master's player number in an orange badge beside the Bars rows, and
/// the phase meter (callout 11), measured off the emulated unit: an orange
/// line for the master and a blue one for this deck, about 73 px a beat
/// (five beats either side of the white centre line), white beat ticks above
/// each line and red ones through it on the first beat of a bar. On the
/// emulator the master's line carried no ticks, most likely because freedj
/// as master sends no CDJ-3000 position packets; freedj draws the master's
/// beats from its Link beat packets so the meter shows the phase.
fn draw_phase_meter(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, u: f32) {
    let p = ui.painter();
    let other_master = snap.master_player != 0 && snap.master_player != snap.player && !snap.master;
    if other_master {
        let b = lay.master_badge;
        p.rect_filled(b, 0.0, BARS_ORANGE);
        label(ui, b.center(), Align2::CENTER_CENTER, snap.master_player, 17.0, u, Color32::BLACK);
    }
    let r = lay.meter;
    let resp = ui.interact(r, Id::new("cdj3k-meter"), egui::Sense::click());
    if resp.clicked() {
        let off = !meter_shown(ui.ctx());
        ui.ctx().data_mut(|d| d.insert_temp(Id::new("cdj3k-meter-off"), !off));
    }
    // Shown once there is another player to follow; the meter of a deck
    // alone has nothing to measure against.
    if !meter_shown(ui.ctx()) || !(snap.linked || snap.beat2_bpm > 0.0) { return; }
    let beat_px = 73.0 * u;
    let cx = r.center().x;
    let master_bib = if snap.beat2_beat_in_bar > 0 { snap.beat2_beat_in_bar as i32 } else { 1 };
    let our_bib = snap.beat_in_bar().unwrap_or(1) as i32;
    let rows = [
        (r.min.y + 19.0 * u, (!snap.master && snap.beat2_bpm > 0.0).then_some(snap.beat2_phase_beats), master_bib, BARS_ORANGE),
        (r.min.y + 52.0 * u, snap.beat_phase(), our_bib, ZOOM_BLUE),
    ];
    for (y, phase, bib, col) in rows {
        p.line_segment([Pos2::new(r.min.x, y), Pos2::new(r.max.x, y)], Stroke::new(4.0 * u, col));
        let Some(ph) = phase else { continue };
        for i in -6..=6i32 {
            let x = cx + (i as f32 - ph) * beat_px;
            if x < r.min.x || x > r.max.x { continue; }
            if (i + bib - 1).rem_euclid(4) == 0 {
                p.line_segment([Pos2::new(x, y - 15.0 * u), Pos2::new(x, y + 15.0 * u)], Stroke::new(2.0 * u, RED));
            } else {
                p.line_segment([Pos2::new(x, y - 14.0 * u), Pos2::new(x, y - 2.0 * u)], Stroke::new(2.0 * u, TEXT));
            }
        }
    }
    p.line_segment([Pos2::new(cx, r.min.y + 6.0 * u), Pos2::new(cx, r.max.y - 4.0 * u)], Stroke::new(3.0 * u, TEXT));
}

/// Which GRID ADJUST keys are lit: SNAP GRID, SHIFT GRID, ◀◀ 1/2, ▶▶ 1/2,
/// RESET. The unit's rules (docs/design/cdj-grid-adjust.md): SNAP after a
/// hand-set cue, SHIFT while SYNC holds a jog offset, RESET once edited.
fn grid_keys_lit(have_grid: bool, cue_set: bool, sync_offset: f32, grid_edited: bool) -> [bool; 5] {
    [
        have_grid && cue_set,
        have_grid && sync_offset.abs() > 0.005,
        have_grid,
        have_grid,
        grid_edited,
    ]
}

/// Where a mark `cols_ahead` waveform columns past the playhead sits on the
/// enlarged waveform `w` showing `cols` columns: the playhead is centred, so
/// it depends on the tempo and zoom only, never on the position.
fn ahead_x(w: Rect, cols_ahead: f64, cols: f64) -> f32 {
    w.min.x + (cols_ahead / cols.max(1.0) + 0.5) as f32 * w.width()
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    /// The 16 / 32 marks: 16 and 32 beats right of the centred playhead,
    /// 32 twice as far as 16, wherever the playhead is (they are computed
    /// without it), as a CDJ-3000 draws them.
    #[test]
    fn beat_marks_sit_still_ahead_of_the_playhead() {
        let w = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(1280.0, 200.0));
        let (x16, x32) = (ahead_x(w, 160.0, 1280.0), ahead_x(w, 320.0, 1280.0));
        assert_eq!((x16, x32), (800.0, 960.0));
        assert!((x32 - 640.0 - 2.0 * (x16 - 640.0)).abs() < 1e-3);
    }

    /// The panels' pages as read off the unit (page 2 of BEAT LOOP is not in
    /// the manual).
    #[test]
    fn loop_and_jump_pages_match_the_unit() {
        let labels = |p: &[f32]| p.iter().map(|b| beats_label(*b)).collect::<Vec<_>>();
        assert_eq!(labels(&LOOP_PAGES[0]), ["1/4", "1/2", "1", "2", "4", "8", "16", "32"]);
        assert_eq!(labels(&LOOP_PAGES[1]), ["3/4", "1", "3/2", "3", "5", "6", "7", "9"]);
        assert_eq!(labels(&JUMP_PAGES[0]), ["1/2", "1", "2", "4"]);
        assert_eq!(labels(&JUMP_PAGES[1]), ["8", "16", "32", "64"]);
        assert_eq!(beats_label(2.5), "2.50", "an odd length still prints");
    }

    #[test]
    fn readouts_format_like_the_unit() {
        // Two-digit bars: the unit reads "00.0" on a memory point.
        assert_eq!(bars_text(Some((4, 2))), "04.2");
        assert_eq!(bars_text(Some((0, 0))), "00.0");
        assert_eq!(bars_text(Some((82, 2))), "82.2");
        assert_eq!(bars_text(None), "--.-");
        assert_eq!(size_text(768 << 20), ("0.8".to_string(), "GB"));
        assert_eq!(size_text(700_000_000), ("0.7".to_string(), "GB"));
        assert_eq!(size_text(31_457_280_000), ("31.5".to_string(), "GB"));
        assert_eq!(size_text(0), ("0.0".to_string(), "GB"));
    }

    /// Every region sits on the screen, scales with it, and a touch panel
    /// (or GRID ADJUST) shrinks the waveform field and lifts the ZOOM pill.
    #[test]
    fn layout_fits_scales_and_shrinks_for_panels() {
        let screen = Rect::from_min_size(Pos2::new(40.0, 20.0), ASPECT);
        let all = |l: &Layout| vec![l.header, l.source, l.art, l.title, l.track_row, l.info_btn, l.bars, l.field,
                                    l.ticks_top, l.wave, l.ticks_bot, l.zoom, l.loop_mark, l.page, l.player, l.track,
                                    l.time, l.tempo, l.bpm, l.overview, l.ruler, l.mt, l.key, l.strip_overview];
        for open in [false, true] {
            let l = layout(screen, open);
            for (i, r) in all(&l).iter().enumerate() {
                assert!(screen.expand(0.5).contains_rect(*r), "region {i} {r:?} off screen (panel {open})");
            }
            for k in l.keys.iter().chain(l.panel.iter()) { assert!(screen.contains_rect(*k)); }
            assert!(l.wave.max.y <= l.field.max.y && l.wave.min.y >= l.field.min.y);
        }
        let (shut, open) = (layout(screen, false), layout(screen, true));
        assert!(open.wave.height() < shut.wave.height());
        assert!(open.zoom.max.y < open.panel[0].min.y, "pill above the key row");
        assert!(shut.zoom.min.y > shut.wave.max.y);
        // Twice the size: every region scales about the origin.
        let big = layout(Rect::from_min_size(Pos2::ZERO, ASPECT * 2.0), false);
        let one = layout(Rect::from_min_size(Pos2::ZERO, ASPECT), false);
        assert!((big.wave.width() - 2.0 * one.wave.width()).abs() < 0.01);
        assert!((big.overview.min.y - 2.0 * one.overview.min.y).abs() < 0.01);
    }

    #[test]
    fn cover_art_decodes_png_and_rejects_garbage() {
        let mut png_bytes = Vec::new();
        {
            let mut e = png::Encoder::new(&mut png_bytes, 2, 2);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]).unwrap();
        }
        let img = decode_art("image/png", &png_bytes).expect("png decodes");
        assert_eq!(img.size, [2, 2]);
        assert_eq!(img.pixels[0], Color32::from_rgb(255, 0, 0));
        assert!(decode_art("image/jpeg", b"not an image").is_none());
        assert!(decode_art("image/png", &png_bytes[..10]).is_none(), "truncated");
    }
}

#[cfg(test)]
mod grid_key_tests {
    use super::grid_keys_lit;

    #[test]
    fn grid_keys_follow_the_units_rules() {
        // Fresh load: only the 1/2 keys.
        assert_eq!(grid_keys_lit(true, false, 0.0, false), [false, false, true, true, false]);
        // A hand-set cue lights SNAP.
        assert_eq!(grid_keys_lit(true, true, 0.0, false)[0], true);
        // A held sync offset (either sign) lights SHIFT; jitter below the
        // threshold does not.
        assert!(grid_keys_lit(true, false, 0.2, false)[1]);
        assert!(grid_keys_lit(true, false, -0.2, false)[1]);
        assert!(!grid_keys_lit(true, false, 0.003, false)[1]);
        // An edited grid lights RESET.
        assert!(grid_keys_lit(true, false, 0.0, true)[4]);
        // No grid: nothing to snap, shift or slide.
        assert_eq!(grid_keys_lit(false, true, 0.2, false), [false, false, false, false, false]);
    }
}

/// GRID ADJUST's key row (replaces a touch panel): SNAP GRID (CUE), SHIFT
/// GRID, ◀◀ 1/2, ▶▶ 1/2, RESET. Measured on the unit: 116×84 keys at y 383.
/// SNAP GRID lights once a cue has been set by hand (the unit keeps it dim
/// while the only cue is the one placed at load). SHIFT GRID lights while
/// SYNC holds a phase offset left by a jog nudge and folds it into the grid;
/// RESET lights once the grid is edited.
fn draw_grid_keys(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    use crate::input::GridAdjust as G;
    let sc = lay.screen;
    let key = |x: f32| Rect::from_min_max(Pos2::new(sc.min.x + x * u, sc.min.y + 383.0 * u), Pos2::new(sc.min.x + (x + 116.0) * u, sc.min.y + 467.0 * u));
    let lit = grid_keys_lit(snap.beat_grid.is_some(), snap.cue_set, snap.sync_offset, snap.grid_edited);
    let keys: [(f32, &[&str], Option<G>, bool); 5] = [
        (315.0, &["SNAP", "GRID"], Some(G::ShiftCue), lit[0]),
        (451.0, &["SHIFT", "GRID"], Some(G::ShiftSync), lit[1]),
        (587.0, &["1/2"], Some(G::HalfBeat { forward: false }), lit[2]),
        (713.0, &["1/2"], Some(G::HalfBeat { forward: true }), lit[3]),
        (849.0, &["RESET"], Some(G::Reset), lit[4]),
    ];
    let p = ui.painter();
    for (i, (x, words, op, on)) in keys.iter().enumerate() {
        let r = key(*x);
        let (bg, edge, ink) = if *on { (Color32::from_rgb(50, 50, 50), Color32::from_rgb(125, 125, 125), TEXT) }
                              else { (Color32::from_rgb(15, 15, 15), Color32::from_rgb(38, 38, 38), Color32::from_rgb(77, 77, 77)) };
        p.rect_filled(r, 0.0, bg);
        p.rect_stroke(r.shrink(1.0 * u), 0.0, Stroke::new(2.0 * u, edge));
        if *on {
            if let Some(op) = op {
                if tap(ui, r, &format!("cdj-grid-{i}")).0 { out.push(Event::Ui(UiEvent::GridAdjust(*op))); }
            }
        }
        let n = words.len() as f32;
        let top = if i == 0 { r.center().y - 18.0 * u } else { r.center().y - (n - 1.0) * 12.0 * u };
        for (k, w) in words.iter().enumerate() {
            let px = if *w == "1/2" { 26.0 } else { 20.0 };
            label(ui, Pos2::new(r.center().x, top + 24.0 * u * k as f32), Align2::CENTER_CENTER, *w, px, u, ink);
        }
        if i == 0 { label(ui, Pos2::new(r.center().x, r.max.y - 16.0 * u), Align2::CENTER_CENTER, "(CUE)", 12.0, u, ink); }
        // The 1/2 keys carry a ◀◀ / ▶▶ tab on their inner top corner.
        if i == 2 || i == 3 {
            let fwd = i == 3;
            let (x0, x1) = if fwd { (r.min.x + 2.0 * u, r.min.x + 34.0 * u) } else { (r.max.x - 34.0 * u, r.max.x - 2.0 * u) };
            let (y0, y1) = (r.min.y + 2.0 * u, r.min.y + 16.0 * u);
            let slant = 6.0 * u;
            let tab = if fwd { vec![Pos2::new(x0, y0), Pos2::new(x1, y0), Pos2::new(x1 - slant, y1), Pos2::new(x0, y1)] }
                      else { vec![Pos2::new(x0, y0), Pos2::new(x1, y0), Pos2::new(x1, y1), Pos2::new(x0 + slant, y1)] };
            p.add(Shape::convex_polygon(tab, Color32::from_rgb(80, 80, 80), Stroke::NONE));
            let c = Pos2::new((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            for d in [-4.0f32, 3.0] {
                let ox = c.x + d * u;
                let s = if fwd { 1.0 } else { -1.0 };
                let tri = vec![Pos2::new(ox - 3.5 * u * s, c.y - 4.0 * u), Pos2::new(ox + 3.5 * u * s, c.y), Pos2::new(ox - 3.5 * u * s, c.y + 4.0 * u)];
                p.add(Shape::convex_polygon(tri, Color32::BLACK, Stroke::NONE));
            }
        }
    }
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

fn stems_volume_id() -> Id { Id::new("cdj3k-stems-volume") }

/// STEMS: FULL MIX, the TOGGLE / VOLUME selector, then DRUMS, VOCAL and
/// HARMONICS two slots wide each.  TOGGLE: a tap mutes a stem or brings it
/// back (dark = muted).  VOLUME: each stem is a horizontal fader; a drag
/// moves its level from where it is.  The selector goes back to TOGGLE
/// only when every stem is fully on or off.  Everything is grey and inert
/// until the track's stems are ready.
fn draw_stems_panel(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    let ready = snap.stems.state == StemsState::Ready;
    let mut volume: bool = ctx.data(|d| d.get_temp(stems_volume_id())).unwrap_or(false);
    if !snap.stems.all_on_or_off() { volume = true; }
    let text_c = if ready { TEXT } else { DIM };

    // FULL MIX
    let full = lay.panel[0];
    if tap(ui, full, "cdj-stems-full").0 && ready { out.push(Event::Ui(UiEvent::StemsFullMix)); }
    ui.painter().rect_filled(full, 0.0, PANEL_FACE);
    outline(ui, full, DIM, u);
    label(ui, Pos2::new(full.center().x, full.center().y - 12.0 * u), Align2::CENTER_CENTER, "FULL", 22.0, u, text_c);
    label(ui, Pos2::new(full.center().x, full.center().y + 14.0 * u), Align2::CENTER_CENTER, "MIX", 22.0, u, text_c);

    // TOGGLE / VOLUME: a tap anywhere flips it.
    let sel = lay.panel[1];
    if tap(ui, sel, "cdj-stems-mode").0 && ready && (!volume || snap.stems.all_on_or_off()) {
        ctx.data_mut(|d| d.insert_temp(stems_volume_id(), !volume));
    }
    ui.painter().rect_filled(sel, 0.0, PANEL_FACE);
    outline(ui, sel, DIM, u);
    for (i, (name, on)) in [("TOGGLE", !volume), ("VOLUME", volume)].into_iter().enumerate() {
        let half = Rect::from_min_max(Pos2::new(sel.min.x, sel.min.y + sel.height() * i as f32 / 2.0),
                                      Pos2::new(sel.max.x, sel.min.y + sel.height() * (i + 1) as f32 / 2.0)).shrink(4.0 * u);
        if on { ui.painter().rect_filled(half, 0.0, if ready { KEY_LIT } else { KEY_FACE }); }
        label(ui, half.center(), Align2::CENTER_CENTER, name, 17.0, u, if on { TEXT } else { DIM });
    }

    // DRUMS / VOCAL / HARMONICS
    for (i, name) in STEM_NAMES.iter().enumerate() {
        let r = lay.panel[2 + 2 * i].union(lay.panel[3 + 2 * i]);
        let g = snap.stems.gains[i];
        if let Some(gain) = stem_touch(ui, r, &format!("cdj-stem{i}"), g, true, !volume, ready) {
            // TOGGLE takes taps only; a drag there changes nothing.
            if volume || gain == 0.0 || gain == 1.0 { out.push(Event::Ui(UiEvent::StemGain { stem: i, gain })); }
        }
        let p = ui.painter();
        p.rect_filled(r, 0.0, PANEL_FACE);
        if ready && g > 0.0 {
            let fill = if volume { Rect::from_min_max(r.min, Pos2::new(r.min.x + r.width() * g, r.max.y)) } else { r };
            p.rect_filled(fill.shrink(2.0 * u), 0.0, KEY_LIT);
        }
        outline(ui, r, if ready && g > 0.0 { KEY_LIT } else { DIM }, u);
        let c = if !ready { DIM } else if g > 0.0 { TEXT } else { LABEL };
        let lift = if volume || !ready { 10.0 } else { 0.0 };
        label(ui, Pos2::new(r.center().x, r.center().y - lift * u), Align2::CENTER_CENTER, *name, 24.0, u, c);
        let sub = match snap.stems.state {
            StemsState::Ready if volume => format!("{:.0}%", g * 100.0),
            StemsState::Ready => String::new(),
            StemsState::Loading => "LOADING".into(),
            _ => "NO STEMS".into(),
        };
        if !sub.is_empty() { label(ui, Pos2::new(r.center().x, r.center().y + 20.0 * u), Align2::CENTER_CENTER, sub, 17.0, u, c); }
    }
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
enum Icon { Disc, Person, Building, Bubble, Calendar, Clock, List, Rings }

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
        Icon::List => {
            p.rect_stroke(sq, 0.0, s);
            for k in 0..3 {
                let y = sq.min.y + sq.height() * (0.3 + 0.2 * k as f32);
                p.circle_filled(Pos2::new(sq.min.x + 5.0 * u, y), 1.3 * u, TEXT);
                p.line_segment([Pos2::new(sq.min.x + 9.0 * u, y), Pos2::new(sq.max.x - 4.0 * u, y)], s);
            }
        }
        Icon::Rings => {
            p.circle_stroke(Pos2::new(c.x - half * 0.3, c.y + half * 0.2), half * 0.65, s);
            p.circle_stroke(Pos2::new(c.x + half * 0.3, c.y - half * 0.2), half * 0.65, s);
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
fn draw_info(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
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
    if tap(ui, x, "cdj-info-close").0 { out.push(Event::Ui(UiEvent::Screen(TopScreen::Info))); }
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
    draw_art(ui, ctx, snap.tags, "deck", r(222.0, 112.0, 312.0, 202.0));
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

// ── List screens (BROWSE, TAG LIST, UTILITY) ─────────────────────────────────

const ROW_H: f32 = 48.0;
const ROWS: usize = 10;
const ROW_GREY: Color32 = Color32::from_rgb(38, 38, 38);
const PANE_HEAD: Color32 = Color32::from_rgb(58, 58, 58);
const COLUMN: Color32 = Color32::from_rgb(55, 55, 55);
/// SOURCE rows and info panel ground; the highlighted row.
const PANE: Color32 = Color32::from_rgb(35, 35, 35);
const SEL_BLUE: Color32 = Color32::from_rgb(0, 125, 225);
/// A track already played this session, in BROWSE lists.
const PLAYED: Color32 = Color32::from_rgb(0, 255, 0);

#[derive(Clone, Copy, PartialEq)]
enum RowIcon { Folder, Track, Person, Disc, Key, List, History, None }

/// One row of a list: icon, text, and an optional right-hand value.
struct Row<'a> { icon: RowIcon, text: &'a str, value: Option<String>, tagged: bool, played: bool }

/// The status strip every list screen keeps at the bottom: PLAYER, time,
/// the overview (the shader draws it into `lay.strip_overview`), MT, range,
/// tempo and BPM.
fn draw_status_strip(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, u: f32, out: &mut Vec<Event>) {
    let sc = lay.screen;
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(
        Pos2::new(sc.min.x + x0 * u, sc.min.y + y0 * u), Pos2::new(sc.min.x + x1 * u, sc.min.y + y1 * u));
    let p = ui.painter();
    p.line_segment([Pos2::new(sc.min.x, sc.min.y + 629.0 * u), Pos2::new(sc.max.x, sc.min.y + 629.0 * u)], Stroke::new(2.0 * u, HEADER));
    let pl = r(20.0, 643.0, 80.0, 703.0);
    outline(ui, pl, BOX, u);
    label(ui, Pos2::new(pl.center().x, pl.min.y + 12.0 * u), Align2::CENTER_CENTER, "PLAYER", 10.0, u, TEXT);
    label(ui, Pos2::new(pl.center().x, pl.min.y + 38.0 * u), Align2::CENTER_CENTER, snap.player, 32.0, u, TEXT);
    let secs = if snap.remain_mode { snap.remaining_secs() } else { snap.elapsed_secs() };
    label(ui, Pos2::new(sc.min.x + 100.0 * u, sc.min.y + 682.0 * u), Align2::LEFT_CENTER,
          format!("{:02}:{:02}", (secs / 60.0) as u64, (secs % 60.0) as u64), 50.0, u, TEXT);
    needle_touch(ui, lay.strip_overview, out);
    if snap.total_samples > 0 {
        let ov = lay.strip_overview;
        for i in 0..=10 {
            let x = ov.min.x + ov.width() * i as f32 / 10.0;
            let tall = i % 2 == 0;
            p.line_segment([Pos2::new(x, ov.max.y + 2.0 * u), Pos2::new(x, ov.max.y + if tall { 10.0 } else { 6.0 } * u)],
                           Stroke::new(if tall { 2.0 } else { 1.0 } * u, if tall { TEXT } else { DIM }));
        }
    }
    if snap.key_lock {
        let mt = r(915.0, 644.0, 985.0, 666.0);
        outline(ui, mt, ORANGE, u);
        label(ui, mt.center(), Align2::CENTER_CENTER, "MT", 17.0, u, ORANGE);
    }
    let rg = r(1000.0, 644.0, 1055.0, 668.0);
    p.rect_filled(rg, 0.0, ORANGE);
    label(ui, rg.center(), Align2::CENTER_CENTER, Settings::range_label(snap.tempo_range), 18.0, u, TEXT);
    let pct = snap.tempo_percent();
    let digits = if pct.abs() < 0.005 { "0.00".to_string() } else { format!("{:+.2}", pct) };
    let pc = label(ui, Pos2::new(sc.min.x + 1116.0 * u, sc.min.y + 703.0 * u), Align2::RIGHT_BOTTOM, "%", 16.0, u, TEXT);
    label(ui, Pos2::new(pc.min.x - 2.0 * u, sc.min.y + 706.0 * u), Align2::RIGHT_BOTTOM, digits, 30.0, u, TEXT);
    let bb = r(1133.0, 643.0, 1260.0, 703.0);
    outline(ui, bb, if snap.master { ORANGE } else { BOX }, u);
    label(ui, Pos2::new(bb.min.x + 6.0 * u, bb.min.y + 8.0 * u), Align2::LEFT_CENTER, "BPM", 10.0, u, TEXT);
    if let Some(bpm) = snap.bpm() {
        let t = (bpm * 10.0).round() as u64;
        let c = if snap.master { ORANGE } else { TEXT };
        let dec = label(ui, Pos2::new(bb.max.x - 8.0 * u, bb.max.y - 6.0 * u), Align2::RIGHT_BOTTOM, format!(".{}", t % 10), 24.0, u, c);
        label(ui, Pos2::new(dec.min.x, bb.max.y - 2.0 * u), Align2::RIGHT_BOTTOM, t / 10, 44.0, u, c);
    }
}

/// The header of a list screen: source tile, ← BACK, the title with its icon.
fn draw_list_header(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, title: &str, folder_icon: bool, u: f32, out: &mut Vec<Event>) {
    let p = ui.painter();
    p.rect_filled(lay.header, 0.0, HEADER);
    let s = lay.source;
    p.rect_filled(s, 0.0, SOURCE_TILE);
    label(ui, Pos2::new(s.center().x, s.min.y + 66.0 * u), Align2::CENTER_CENTER, if snap.source_link { "LINK" } else { "USB" }, 18.0, u, TEXT);
    label(ui, Pos2::new(s.center().x, s.min.y + 38.0 * u), Align2::CENTER_CENTER, "▣", 24.0, u, TEXT);
    let back = Rect::from_min_max(Pos2::new(lay.screen.min.x + 121.0 * u, lay.screen.min.y + 21.0 * u),
                                  Pos2::new(lay.screen.min.x + 169.0 * u, lay.screen.min.y + 69.0 * u));
    if tap(ui, back, "cdj-list-back").0 { out.push(Event::Deck(ControlEvent::Back)); }
    p.rect_filled(back, 0.0, Color32::from_rgb(35, 35, 35));
    outline(ui, back, Color32::from_rgb(100, 100, 100), u);
    // ← (drawn; the default font has no arrow glyph)
    let (c, a) = (back.center(), 11.0 * u);
    let st = Stroke::new(2.5 * u, TEXT);
    ui.painter().line_segment([Pos2::new(c.x - a, c.y), Pos2::new(c.x + a, c.y)], st);
    ui.painter().line_segment([Pos2::new(c.x - a, c.y), Pos2::new(c.x - a * 0.3, c.y - a * 0.7)], st);
    ui.painter().line_segment([Pos2::new(c.x - a, c.y), Pos2::new(c.x - a * 0.3, c.y + a * 0.7)], st);
    let x = lay.screen.min.x + 190.0 * u;
    let y = lay.screen.min.y + 45.0 * u;
    let tx = if folder_icon {
        folder(ui, Pos2::new(x + 9.0 * u, y), u, TEXT);
        x + 30.0 * u
    } else { x };
    ui.painter().with_clip_rect(Rect::from_min_max(Pos2::new(tx, lay.screen.min.y), Pos2::new(lay.screen.min.x + 1000.0 * u, lay.screen.min.y + 90.0 * u)))
        .text(Pos2::new(tx, y), Align2::LEFT_CENTER, title, FontId::proportional(32.0 * u), TEXT);
}

fn folder(ui: &Ui, c: Pos2, u: f32, col: Color32) {
    let b = Rect::from_center_size(Pos2::new(c.x, c.y + 1.0 * u), Vec2::new(18.0 * u, 13.0 * u));
    ui.painter().rect_filled(b, 1.0, col);
    ui.painter().rect_filled(Rect::from_min_size(Pos2::new(b.min.x, b.min.y - 3.0 * u), Vec2::new(8.0 * u, 4.0 * u)), 1.0, col);
}

/// The tags of a file highlighted in BROWSE, read once per highlighted file
/// (a metadata probe, no decoding) and kept in egui memory.
fn browse_tags(ctx: &egui::Context, path: &std::path::Path) -> Option<opendeck_decode::TrackTags> {
    let id = Id::new("cdj3k-browse-tags");
    if let Some((p, t)) = ctx.data(|d| d.get_temp::<(std::path::PathBuf, Option<opendeck_decode::TrackTags>)>(id)) {
        if p == path { return t; }
    }
    let t = opendeck_decode::SymphoniaDecoder::open(path).ok().map(|d| d.tags().clone());
    ctx.data_mut(|d| d.insert_temp(id, (path.to_path_buf(), t.clone())));
    t
}

/// The category column at the left (only the categories the source has).
fn draw_categories(ui: &Ui, lay: &Layout, cats: &[&str], u: f32) {
    let sc = lay.screen;
    let col = Rect::from_min_max(Pos2::new(sc.min.x, sc.min.y + 92.0 * u), Pos2::new(sc.min.x + 90.0 * u, sc.min.y + 628.0 * u));
    ui.painter().rect_filled(col, 0.0, COLUMN);
    for (i, c) in cats.iter().enumerate() {
        let t = Rect::from_min_size(Pos2::new(col.min.x, col.min.y + 2.0 * u + 68.0 * u * i as f32), Vec2::new(90.0 * u, 66.0 * u));
        ui.painter().rect_filled(t, 0.0, if i == 0 { Color32::from_rgb(85, 85, 85) } else { COLUMN });
        folder(ui, Pos2::new(t.center().x, t.min.y + 24.0 * u), u, TEXT);
        label(ui, Pos2::new(t.center().x, t.min.y + 52.0 * u), Align2::CENTER_CENTER, *c, 13.0, u, TEXT);
    }
}

/// A list pane: header bar, then up to [`ROWS`] rows scrolled to keep
/// `selected` in view. Tapping a row selects it; tapping the selected row
/// loads it (the rotary's press).
fn draw_list(ui: &Ui, pane: Rect, head: &str, rows: &[Row], selected: usize, empty: &str, u: f32, out: Option<&mut Vec<Event>>) {
    let p = ui.painter();
    let hb = Rect::from_min_size(pane.min, Vec2::new(pane.width(), 60.0 * u));
    p.rect_filled(hb, 0.0, PANE_HEAD);
    label(ui, Pos2::new(hb.min.x + 11.0 * u, hb.center().y), Align2::LEFT_CENTER, head, 21.0, u, LABEL);
    // usize::MAX = nothing highlighted (a preview pane).
    let first = if selected == usize::MAX { 0 } else { selected.saturating_sub(ROWS / 2).min(rows.len().saturating_sub(ROWS)) };
    let mut out = out;
    for k in 0..ROWS {
        let y = hb.max.y + ROW_H * u * k as f32;
        let rr = Rect::from_min_size(Pos2::new(pane.min.x, y), Vec2::new(pane.width(), ROW_H * u));
        if rr.max.y > pane.max.y + 1.0 { break; }
        let i = first + k;
        let sel = i == selected && i < rows.len();
        p.rect_filled(rr, 0.0, if sel { KEY_LIT } else if k % 2 == 0 { ROW_GREY } else { BG });
        let Some(row) = rows.get(i) else { continue };
        if let Some(out) = out.as_deref_mut() {
            if tap(ui, rr, &format!("cdj-row-{head}-{i}")).0 {
                if sel { out.push(Event::Deck(ControlEvent::Load)); }
                else { out.push(Event::Deck(ControlEvent::BrowseEncoderDelta { delta: i as i32 - selected as i32 })); }
            }
        }
        let cy = rr.center().y;
        match row.icon {
            RowIcon::Folder => folder(ui, Pos2::new(rr.min.x + 22.0 * u, cy), u, TEXT),
            RowIcon::Track => { label(ui, Pos2::new(rr.min.x + 22.0 * u, cy), Align2::CENTER_CENTER, "♪", 26.0, u, TEXT); }
            RowIcon::Person => icon(ui, Icon::Person, Pos2::new(rr.min.x + 22.0 * u, cy), u),
            RowIcon::Disc => icon(ui, Icon::Disc, Pos2::new(rr.min.x + 22.0 * u, cy), u),
            RowIcon::Key => boxed(ui, "b#", Pos2::new(rr.min.x + 22.0 * u, cy), u),
            RowIcon::List => icon(ui, Icon::List, Pos2::new(rr.min.x + 22.0 * u, cy), u),
            RowIcon::History => {
                let c = Pos2::new(rr.min.x + 22.0 * u, cy);
                icon(ui, Icon::List, c, u);
                ui.painter().circle_filled(Pos2::new(c.x + 8.0 * u, c.y + 8.0 * u), 6.0 * u, BG);
                ui.painter().circle_stroke(Pos2::new(c.x + 8.0 * u, c.y + 8.0 * u), 5.0 * u, Stroke::new(1.5 * u, TEXT));
            }
            RowIcon::None => {}
        }
        let tx = if row.icon == RowIcon::None { 14.0 } else { 45.0 };
        let right = if let Some(v) = &row.value {
            label(ui, Pos2::new(rr.max.x - 14.0 * u, cy), Align2::RIGHT_CENTER, v, 24.0, u, TEXT).min.x - 12.0 * u
        } else { rr.max.x - 10.0 * u };
        p.with_clip_rect(Rect::from_min_max(Pos2::new(rr.min.x, rr.min.y), Pos2::new(right, rr.max.y)))
            .text(Pos2::new(rr.min.x + tx * u, cy), Align2::LEFT_CENTER, row.text, FontId::proportional(27.0 * u), if row.played { PLAYED } else { TEXT });
        if row.tagged {
            p.circle_filled(Pos2::new(right - 6.0 * u, cy), 5.0 * u, BARS_ORANGE);
        }
    }
    if rows.is_empty() {
        label(ui, Pos2::new(pane.center().x, hb.max.y + 60.0 * u), Align2::CENTER_CENTER, empty, 22.0, u, DIM);
    }
    // Scroll position along the right edge.
    if rows.len() > ROWS {
        let track = Rect::from_min_max(Pos2::new(pane.max.x - 5.0 * u, hb.max.y), Pos2::new(pane.max.x, pane.max.y));
        let h = track.height() * ROWS as f32 / rows.len() as f32;
        let y0 = track.min.y + (track.height() - h) * first as f32 / (rows.len() - ROWS) as f32;
        p.rect_filled(Rect::from_min_size(Pos2::new(track.min.x, y0), Vec2::new(track.width(), h)), 0.0, DIM);
    }
}

/// "0.7" GB, as the SOURCE info panel shows sizes.
fn size_text(bytes: u64) -> (String, &'static str) {
    // Always GB, one decimal: the unit shows a 768 MB stick as "0.7 GB".
    (format!("{:.1}", bytes as f64 / 1e9), "GB")
}

/// A source's total/free bytes, read at most every two seconds per path.
fn cached_space(ctx: &egui::Context, path: &std::path::Path) -> Option<(u64, u64)> {
    let id = Id::new(("cdj3k-space", path));
    let now = ctx.input(|i| i.time);
    if let Some((t, v)) = ctx.data(|d| d.get_temp::<(f64, Option<(u64, u64)>)>(id)) {
        if now - t < 2.0 { return v; }
    }
    let v = crate::browser::disk_space(path);
    ctx.data_mut(|d| d.insert_temp(id, (now, v)));
    v
}

/// SOURCE: one 90 px row per medium (icon tile + name, "Loaded" on the
/// source of the loaded track), and the highlighted source's details on the
/// right. Measured from the unit's SOURCE screen.
fn draw_source(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, b: &Browser, cursor: usize,
               loaded: Option<&crate::browser::Load>, u: f32, out: &mut Vec<Event>) {
    use crate::browser::SourceKind;
    let sc = lay.screen;
    let at = |x: f32, y: f32| Pos2::new(sc.min.x + x * u, sc.min.y + y * u);
    let p = ui.painter();
    p.rect_filled(Rect::from_min_max(at(0.0, 0.0), at(1280.0, 628.0)), 0.0, HEADER);
    label(ui, at(36.0, 45.0), Align2::LEFT_CENTER, "SOURCE", 34.0, u, TEXT);

    let rows = b.sources();
    const PITCH: f32 = 92.0;
    let visible = 6usize;
    let first = cursor.saturating_sub(visible - 1);
    let green = Color32::from_rgb(0, 255, 0);
    for k in 0..visible {
        let y = 91.0 + PITCH * k as f32;
        let bottom = (y + 90.0).min(628.0);
        let row = Rect::from_min_max(at(0.0, y), at(928.0, bottom));
        let i = first + k;
        let sel = i == cursor && i < rows.len();
        p.rect_filled(row, 0.0, if sel { SEL_BLUE } else { PANE });
        let Some(r) = rows.get(i) else { continue };
        if tap(ui, row, &format!("cdj-source-{i}")).0 {
            out.push(Event::Deck(if sel { ControlEvent::Load }
                                 else { ControlEvent::BrowseEncoderDelta { delta: i as i32 - cursor as i32 } }));
        }
        // Icon tile (inset by the selection border when highlighted).
        let tile = Rect::from_min_max(at(0.0, y), at(90.0, bottom));
        p.rect_filled(if sel { tile.shrink(2.0 * u) } else { tile }, 0.0, SOURCE_TILE);
        let c = Pos2::new(tile.center().x, tile.min.y + 33.0 * u);
        let kind = match r.kind { SourceKind::Local => "FOLDER", SourceKind::Usb => "USB", SourceKind::Link if r.rekordbox => "PC", SourceKind::Link => "LINK" };
        match r.kind {
            SourceKind::Local => folder(ui, c, u * 1.2, TEXT),
            SourceKind::Usb => usb_icon(ui, c, u),
            SourceKind::Link => { label(ui, c, Align2::CENTER_CENTER, "▣", 24.0, u, TEXT); }
        }
        label(ui, Pos2::new(tile.center().x, tile.min.y + 64.0 * u), Align2::CENTER_CENTER, kind, 17.0, u, TEXT);
        if let Some(n) = r.player {
            let badge = Rect::from_min_size(at(2.0, y + 2.0), Vec2::splat(22.0 * u));
            p.rect_filled(badge, 0.0, Color32::BLACK);
            outline(ui, badge, TEXT, u);
            label(ui, badge.center(), Align2::CENTER_CENTER, n, 15.0, u, TEXT);
        }
        let held = loaded.is_some_and(|l| r.holds(l));
        let cy = row.center().y;
        p.with_clip_rect(Rect::from_min_max(at(100.0, y), at(700.0, bottom)))
            .text(Pos2::new(sc.min.x + 116.0 * u, cy), Align2::LEFT_CENTER, &r.label, FontId::proportional(30.0 * u), if held { green } else { TEXT });
        if held {
            // [ Loaded ] — corner brackets, not a full box.
            let bx = Rect::from_min_max(Pos2::new(sc.min.x + 710.0 * u, cy - 25.0 * u), Pos2::new(sc.min.x + 850.0 * u, cy + 25.0 * u));
            let st = Stroke::new(1.5 * u, Color32::from_rgb(75, 75, 75));
            for x in [bx.min.x, bx.max.x] {
                let d = if x == bx.min.x { 8.0 * u } else { -8.0 * u };
                p.line_segment([Pos2::new(x, bx.min.y), Pos2::new(x, bx.max.y)], st);
                p.line_segment([Pos2::new(x, bx.min.y), Pos2::new(x + d, bx.min.y)], st);
                p.line_segment([Pos2::new(x, bx.max.y), Pos2::new(x + d, bx.max.y)], st);
            }
            label(ui, bx.center(), Align2::CENTER_CENTER, "Loaded", 26.0, u, green);
        }
        if sel {
            let (cx, a) = (sc.min.x + 873.0 * u, 9.0 * u);
            let st = Stroke::new(3.0 * u, TEXT);
            p.line_segment([Pos2::new(cx - a * 0.5, cy - a), Pos2::new(cx + a * 0.5, cy)], st);
            p.line_segment([Pos2::new(cx + a * 0.5, cy), Pos2::new(cx - a * 0.5, cy + a)], st);
        }
    }

    // Right panel: the highlighted source's details.
    let pane = Rect::from_min_max(at(930.0, 91.0), at(1280.0, 628.0));
    p.rect_filled(pane, 0.0, PANE);
    if let Some(r) = rows.get(cursor) {
        let counts = r.path.as_deref().and_then(|root| b.rekordbox_counts(root));
        let (songs, lists) = match counts {
            Some((s, l)) => (s.to_string(), l.to_string()),
            None if r.rekordbox => ("-".into(), "-".into()),
            None => ("0".into(), "0".into()),
        };
        let space = r.path.as_deref().and_then(|root| cached_space(ui.ctx(), root));
        let line = |y: f32, name: &str, value: Option<(String, &str)>| {
            label(ui, at(949.0, y), Align2::LEFT_CENTER, name, 17.0, u, LABEL);
            if let Some((v, unit)) = value {
                let right = if unit.is_empty() { at(1255.0, y + 2.0) } else {
                    label(ui, at(1255.0, y + 8.0), Align2::RIGHT_CENTER, unit, 20.0, u, LABEL).left_center() - Vec2::new(6.0 * u, 6.0 * u)
                };
                label(ui, right, Align2::RIGHT_CENTER, v, 28.0, u, TEXT);
            }
            ui.painter().line_segment([at(949.0, y + 22.0), at(1260.0, y + 22.0)], Stroke::new(1.0, Color32::BLACK));
        };
        line(120.0, "Songs", Some((songs, "")));
        line(166.0, "Playlists", Some((lists, "")));
        line(219.0, "Date", None);
        if let Some((total, free)) = space {
            let (t, tu) = size_text(total);
            let (f, fu) = size_text(free);
            line(270.0, "Total", Some((t, tu)));
            line(322.0, "Available", Some((f, fu)));
        } else {
            line(270.0, "Total", None);
            line(322.0, "Available", None);
        }
    }
    draw_status_strip(ui, snap, lay, u, out);
}

/// Tags of a local file, read once per path and kept in egui memory (the
/// LOAD PREVIOUS TRACK rows each keep their own).
fn tags_at(ctx: &egui::Context, path: &std::path::Path) -> Option<opendeck_decode::TrackTags> {
    let id = Id::new(("cdj3k-tags", path));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<opendeck_decode::TrackTags>>(id)) { return t; }
    let t = opendeck_decode::SymphoniaDecoder::open(path).ok().map(|d| d.tags().clone());
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// What a history row says: the title tag, else the file name without its
/// extension, else what the source called it.
fn load_title(ctx: &egui::Context, l: &crate::browser::Load) -> (String, Option<opendeck_decode::TrackTags>) {
    use crate::browser::Load;
    let stem = |p: &std::path::Path| p.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
    match l {
        Load::Local { path, .. } => {
            let tags = tags_at(ctx, path);
            (tags.as_ref().and_then(|t| t.title.clone()).unwrap_or_else(|| stem(path)), tags)
        }
        Load::Link { rel_path, .. } => (stem(std::path::Path::new(rel_path)), None),
        Load::Db { title, .. } => (title.clone(), None),
    }
}

/// LOAD PREVIOUS TRACK: the pop-up over the right of the waveform screen
/// (568×464 at x 712), 50 px rows of cover + title, newest first, with the
/// two empty value columns the unit draws. Tap a row to highlight it, tap
/// it again to load.
fn draw_history(ui: &Ui, ctx: &egui::Context, lay: &Layout, items: &[crate::browser::Load], cursor: Option<usize>, u: f32, out: &mut Vec<Event>) {
    let sc = lay.screen;
    let at = |x: f32, y: f32| Pos2::new(sc.min.x + x * u, sc.min.y + y * u);
    let p = ui.painter();
    let frame = Rect::from_min_max(at(712.0, 0.0), at(1280.0, 464.0));
    p.rect_filled(frame, 0.0, Color32::BLACK);
    let grey = Color32::from_rgb(128, 128, 128);
    p.rect_filled(Rect::from_min_max(at(714.0, 2.0), at(1278.0, 60.0)), 0.0, Color32::from_rgb(75, 75, 75));
    label(ui, at(735.0, 32.0), Align2::LEFT_CENTER, "LOAD PREVIOUS TRACK", 20.0, u, Color32::from_rgb(200, 200, 200));
    const ROW: f32 = 50.0;
    const SHOWN: usize = 8;
    let sel = cursor.unwrap_or(usize::MAX);
    let first = if sel == usize::MAX { 0 } else { sel.saturating_sub(SHOWN - 1) };
    for k in 0..SHOWN {
        let y = 60.0 + ROW * k as f32;
        let rr = Rect::from_min_max(at(714.0, y), at(1278.0, (y + ROW).min(462.0)));
        let i = first + k;
        let lit = i == sel && i < items.len();
        p.rect_filled(rr, 0.0, if lit { SEL_BLUE } else if k % 2 == 0 { HEADER } else { Color32::BLACK });
        let Some(l) = items.get(i) else { continue };
        if tap(ui, rr, &format!("cdj-history-{i}")).0 {
            out.push(Event::Deck(if lit { ControlEvent::Load } else {
                ControlEvent::BrowseEncoderDelta { delta: if sel == usize::MAX { 1 } else { i as i32 - sel as i32 } }
            }));
            // The first tap from "nothing highlighted" lands on row 0; step on.
            if sel == usize::MAX && i > 0 { out.push(Event::Deck(ControlEvent::BrowseEncoderDelta { delta: i as i32 })); }
        }
        let (title, tags) = load_title(ctx, l);
        let art = Rect::from_min_size(at(714.0, y), Vec2::splat(ROW * u));
        let slot = format!("hist-{i}");
        match tags.as_ref() {
            Some(t) => draw_art(ui, ctx, t, &slot, art),
            None => draw_art(ui, ctx, &Default::default(), &slot, art),
        }
        let cy = rr.center().y;
        label(ui, Pos2::new(sc.min.x + 784.0 * u, cy), Align2::CENTER_CENTER, "♪", 26.0, u, TEXT);
        p.with_clip_rect(Rect::from_min_max(at(798.0, y), at(1090.0, y + ROW)))
            .text(Pos2::new(sc.min.x + 798.0 * u, cy), Align2::LEFT_CENTER, title, FontId::proportional(27.0 * u), TEXT);
    }
    // Column rules and the frame.
    for x in [1093.0, 1193.0] {
        p.line_segment([at(x, 60.0), at(x, 462.0)], Stroke::new(1.0 * u, grey));
    }
    p.rect_stroke(frame.shrink(1.0 * u), 0.0, Stroke::new(2.0 * u, grey));
    if items.is_empty() {
        label(ui, at(996.0, 120.0), Align2::CENTER_CENTER, "— no earlier tracks —", 20.0, u, DIM);
    }
}

/// The USB plug glyph on a source tile.
fn usb_icon(ui: &Ui, c: Pos2, u: f32) {
    let p = ui.painter();
    let body = Rect::from_center_size(Pos2::new(c.x, c.y + 4.0 * u), Vec2::new(14.0 * u, 18.0 * u));
    p.rect_stroke(body, 1.0, Stroke::new(2.0 * u, TEXT));
    let plug = Rect::from_center_size(Pos2::new(c.x, c.y - 9.0 * u), Vec2::new(10.0 * u, 8.0 * u));
    p.rect_stroke(plug, 0.0, Stroke::new(1.5 * u, TEXT));
}

/// BROWSE: the source's lists, two panes. The right pane shows what LOAD on
/// the highlighted row would act on: a track's details and a LOAD key.
fn draw_browse(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, b: &Browser, tags: &TagList, u: f32, out: &mut Vec<Event>) {
    use crate::browser::RbField;
    if b.in_search() { return draw_search(ui, snap, lay, b, tags, u, out); }
    let sc = lay.screen;
    let cats = b.rb_categories();
    let names = b.rb_names_field();
    // rekordbox: name rows get their category's icon; playlist folders a list.
    let dir_icon = match names {
        Some(RbField::Artist) => RowIcon::Person,
        Some(RbField::Album) => RowIcon::Disc,
        Some(RbField::Key) => RowIcon::Key,
        None if b.on_rb_history() => RowIcon::History,
        None if cats.is_some() => RowIcon::List,
        None => RowIcon::Folder,
    };
    let rows: Vec<Row> = b.entries().iter().map(|e| Row {
        icon: if e.is_dir { dir_icon } else { RowIcon::Track }, text: &e.name, value: None,
        tagged: e.load().map_or(false, |l| tags.contains(l)),
        played: e.load().map_or(false, |l| b.was_played(l)),
    }).collect();
    // The unit titles a category's name list BROWSE, a track list by its name.
    let title = if names.is_some() || b.on_rb_history() { "BROWSE".to_string() } else { b.title() };
    draw_list_header(ui, snap, lay, &title, cats.is_none(), u, out);
    match &cats {
        Some((names, open)) => draw_rb_categories(ui, lay, names, *open, u, out),
        None => draw_categories(ui, lay, &["FOLDER"], u),
    }
    // A rekordbox track list is one full-width table (PREVIEW, TRACK, BPM, KEY).
    if cats.is_some() && !b.entries().is_empty() && b.entries().iter().all(|e| !e.is_dir) {
        draw_track_table(ui, lay, b, &rows, 92.0, 628.0, 4, u, out);
        if let Some(name) = snap.loading {
            label(ui, Pos2::new(sc.min.x + 1000.0 * u, sc.min.y + 45.0 * u), Align2::RIGHT_CENTER, format!("LOADING  {name}"), 18.0, u, BARS_ORANGE);
        }
        return draw_status_strip(ui, snap, lay, u, out);
    }
    let head = match names {
        Some(f) => f_label(f),
        None if b.on_rb_history() => "HISTORY".to_string(),
        None if cats.is_some() => "PLAYLIST".to_string(),
        None => "FOLDER".to_string(),
    };
    let left = Rect::from_min_max(Pos2::new(sc.min.x + 92.0 * u, sc.min.y + 92.0 * u), Pos2::new(sc.min.x + 730.0 * u, sc.min.y + 628.0 * u));
    draw_list(ui, left, &head, &rows, b.selected, "— empty —", u, Some(out));
    let right = Rect::from_min_max(Pos2::new(sc.min.x + 732.0 * u, sc.min.y + 92.0 * u), Pos2::new(sc.max.x, sc.min.y + 628.0 * u));
    ui.painter().rect_filled(right, 0.0, BG);
    ui.painter().line_segment([Pos2::new(right.min.x - 1.0 * u, right.min.y), Pos2::new(right.min.x - 1.0 * u, right.max.y)], Stroke::new(2.0 * u, PANE_HEAD));
    match b.selected_entry() {
        Some(e) if !e.is_dir => {
            // A track: its details (tags for a local file; what the source
            // listed otherwise), the cover, and LOAD, on the pane's grey.
            ui.painter().rect_filled(right, 0.0, Color32::from_rgb(50, 50, 50));
            let hb = Rect::from_min_size(right.min, Vec2::new(right.width(), 60.0 * u));
            ui.painter().rect_filled(hb, 0.0, PANE_HEAD);
            label(ui, Pos2::new(hb.min.x + 11.0 * u, hb.center().y), Align2::LEFT_CENTER, "INFO", 21.0, u, LABEL);
            let tags = match e.load() {
                Some(crate::browser::Load::Local { path, .. }) => browse_tags(ui.ctx(), path),
                _ => None,
            }.unwrap_or_default();
            let title = tags.title.clone().unwrap_or_else(|| e.name.clone());
            let artist = tags.artist.clone().or_else(|| e.artist.clone()).unwrap_or_default();
            let bpm = tags.bpm.or(e.bpm).map(|b| format!("{b:.1}")).unwrap_or_default();
            let at = |x: f32, y: f32| Pos2::new(sc.min.x + x * u, sc.min.y + y * u);
            let rule = |y: f32, x1: f32| ui.painter().line_segment([at(742.0, y), at(x1, y)], Stroke::new(1.0, Color32::from_rgb(70, 70, 70)));
            let clip = ui.painter().with_clip_rect(Rect::from_min_max(at(742.0, 152.0), at(1270.0, 248.0)));
            label(ui, at(758.0, 176.0), Align2::CENTER_CENTER, "♪", 26.0, u, TEXT);
            clip.text(at(780.0, 176.0), Align2::LEFT_CENTER, &title, FontId::proportional(26.0 * u), TEXT);
            rule(200.0, 1270.0);
            icon(ui, Icon::Person, at(758.0, 224.0), u);
            clip.text(at(780.0, 224.0), Align2::LEFT_CENTER, &artist, FontId::proportional(26.0 * u), TEXT);
            rule(248.0, 1270.0);
            let small = ui.painter().with_clip_rect(Rect::from_min_max(at(742.0, 248.0), at(1062.0, 440.0)));
            let rows: [(&str, String); 4] = [
                ("disc", tags.album.clone().unwrap_or_default()),
                ("BPM", bpm),
                ("b#", tags.key.clone().unwrap_or_default()),
                ("cal", tags.year.clone().unwrap_or_default()),
            ];
            for (i, (k, v)) in rows.iter().enumerate() {
                let y = 272.0 + 48.0 * i as f32;
                match *k {
                    "disc" => icon(ui, Icon::Disc, at(758.0, y), u),
                    "cal" => icon(ui, Icon::Calendar, at(758.0, y), u),
                    t => boxed(ui, t, at(758.0, y), u),
                }
                small.text(at(780.0, y), Align2::LEFT_CENTER, v, FontId::proportional(22.0 * u), TEXT);
                rule(y + 24.0, 1062.0);
            }
            draw_art(ui, ui.ctx(), &tags, "browse", Rect::from_min_max(at(1073.0, 252.0), at(1260.0, 439.0)));
            icon(ui, Icon::Bubble, at(758.0, 513.0), u);
            ui.painter().with_clip_rect(Rect::from_min_max(at(742.0, 490.0), at(1270.0, 536.0)))
                .text(at(780.0, 513.0), Align2::LEFT_CENTER, tags.comment.clone().unwrap_or_default(), FontId::proportional(20.0 * u), TEXT);
            rule(537.0, 1270.0);
            let load = Rect::from_min_max(at(935.0, 552.0), at(1085.0, 602.0));
            if tap(ui, load, "cdj-browse-load").0 { out.push(Event::Deck(ControlEvent::Load)); }
            ui.painter().rect_filled(load, 0.0, Color32::from_rgb(225, 225, 225));
            label(ui, load.center(), Align2::CENTER_CENTER, "LOAD", 22.0, u, Color32::from_rgb(30, 30, 30));
        }
        _ if b.rb_preview().is_some() => {
            // A rekordbox name: an artist's albums, else its tracks.
            let (head, items) = b.rb_preview().unwrap();
            let icon = if head == "ALBUM" { RowIcon::Disc } else { RowIcon::Track };
            let rows: Vec<Row> = items.iter().map(|n| Row { icon, text: n, value: None, tagged: false, played: false }).collect();
            draw_list(ui, right, head, &rows, usize::MAX, "", u, None);
        }
        _ => {
            // A folder: what is in it, as the next level would list it.
            let peek = b.preview().unwrap_or_default();
            let rows: Vec<Row> = peek.iter().map(|(d, n)| Row {
                icon: if *d { RowIcon::Folder } else { RowIcon::Track }, text: n, value: None, tagged: false, played: false,
            }).collect();
            draw_list(ui, right, "FOLDER", &rows, usize::MAX, "", u, None);
        }
    }
    if let Some(name) = snap.loading {
        label(ui, Pos2::new(sc.min.x + 1000.0 * u, sc.min.y + 45.0 * u), Align2::RIGHT_CENTER, format!("LOADING  {name}"), 18.0, u, BARS_ORANGE);
    }
    draw_status_strip(ui, snap, lay, u, out);
}

fn f_label(f: crate::browser::RbField) -> String {
    use crate::browser::RbField::*;
    match f { Artist => "ARTIST", Album => "ALBUM", Key => "KEY" }.to_string()
}

/// A rekordbox source's category column (unit: 90×66 tiles on a 67 px
/// pitch from y 94, grey 60, the open one blue). Tapping one opens it.
fn draw_rb_categories(ui: &Ui, lay: &Layout, names: &[String], open: Option<usize>, u: f32, out: &mut Vec<Event>) {
    let sc = lay.screen;
    let col = Rect::from_min_max(Pos2::new(sc.min.x, sc.min.y + 92.0 * u), Pos2::new(sc.min.x + 90.0 * u, sc.min.y + 628.0 * u));
    ui.painter().rect_filled(col, 0.0, Color32::BLACK);
    for (i, name) in names.iter().enumerate() {
        let t = Rect::from_min_size(Pos2::new(col.min.x, sc.min.y + (94.0 + 67.0 * i as f32) * u), Vec2::new(88.0 * u, 66.0 * u));
        if t.max.y > col.max.y + 1.0 { break; }
        let lit = open == Some(i);
        ui.painter().rect_filled(t, 0.0, if lit { SEL_BLUE } else { Color32::from_rgb(60, 60, 60) });
        if tap(ui, t, &format!("cdj-rbcat-{i}")).0 { out.push(Event::Ui(UiEvent::BrowseCategory(i))); }
        let c = Pos2::new(t.center().x, t.min.y + 24.0 * u);
        match name.as_str() {
            "ARTIST" => icon(ui, Icon::Person, c, u),
            "ALBUM" => icon(ui, Icon::Disc, c, u),
            "TRACK" => { label(ui, c, Align2::CENTER_CENTER, "♪", 28.0, u, TEXT); }
            "KEY" => boxed(ui, "b#", c, u),
            "PLAYLIST" => icon(ui, Icon::List, c, u),
            "HISTORY" => icon(ui, Icon::Clock, c, u),
            "MATCHING" => icon(ui, Icon::Rings, c, u),
            _ => folder(ui, c, u * 1.3, TEXT),
        }
        label(ui, Pos2::new(t.center().x, t.min.y + 52.0 * u), Align2::CENTER_CENTER, name, 13.0, u, TEXT);
    }
}

/// A rekordbox track's blue preview (height 0–31, whiteness 0–7 per column),
/// read from its analysis file once and kept in egui memory.
fn rb_wave(ctx: &egui::Context, anlz: &std::path::Path) -> Option<std::sync::Arc<Vec<(u8, u8)>>> {
    let id = Id::new(("cdj3k-rbwave", anlz));
    if let Some(w) = ctx.data(|d| d.get_temp::<Option<std::sync::Arc<Vec<(u8, u8)>>>>(id)) { return w; }
    let w = opendeck_rekordbox::read_anlz(anlz).ok().map(|a| a.preview).filter(|p| !p.is_empty()).map(std::sync::Arc::new);
    ctx.data_mut(|d| d.insert_temp(id, w.clone()));
    w
}

/// A cover image file (rekordbox artwork) as a texture, decoded once per file.
fn file_art(ctx: &egui::Context, path: &std::path::Path) -> Option<egui::TextureHandle> {
    let id = Id::new(("cdj3k-fileart", path));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)) { return t; }
    let mime = match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        _ => "image/jpeg",
    };
    let tex = std::fs::read(path).ok().and_then(|b| decode_art(mime, &b))
        .map(|img| ctx.load_texture(format!("cdj3k-fileart-{}", path.display()), img, egui::TextureOptions::LINEAR));
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// The PREVIEW cell of a track row: the blue preview bottom-anchored on black
/// (unit: 192×44 at x 92), then the cover in the 47 px square at x 284.
fn draw_row_preview(ui: &Ui, e: &crate::browser::Entry, x0: f32, y0: f32, u: f32) {
    let p = ui.painter();
    let wave = Rect::from_min_max(Pos2::new(x0 + 1.0 * u, y0 + 2.0 * u), Pos2::new(x0 + 191.0 * u, y0 + 46.0 * u));
    p.rect_filled(wave, 0.0, Color32::BLACK);
    let anlz = match e.load() { Some(crate::browser::Load::Local { analyze: Some(a), .. }) => Some(a.clone()), _ => None };
    if let Some(w) = anlz.as_deref().and_then(|a| rb_wave(ui.ctx(), a)) {
        let n = w.len().max(1);
        let cols = wave.width().floor().max(1.0) as usize;
        for c in 0..cols {
            // The loudest source column under each pixel column.
            let (a, b) = (c * n / cols, ((c + 1) * n / cols).max(c * n / cols + 1).min(n));
            let (h, wh) = w[a..b].iter().fold((0u8, 0u8), |m, &(h, wh)| if h > m.0 { (h, wh) } else { m });
            if h == 0 { continue; }
            let top = wave.max.y - wave.height() * h as f32 / 31.0;
            let x = wave.min.x + c as f32 + 0.5;
            // Unit colours: blue (18,96,151) brightening with whiteness.
            let k = wh as f32 / 7.0;
            let col = Color32::from_rgb((18.0 + 90.0 * k) as u8, (96.0 + 90.0 * k) as u8, (151.0 + 80.0 * k) as u8);
            p.line_segment([Pos2::new(x, top), Pos2::new(x, wave.max.y)], Stroke::new(1.0, col));
        }
    }
    let art = Rect::from_min_size(Pos2::new(x0 + 192.0 * u, y0 + 0.5 * u), Vec2::splat(47.0 * u));
    match e.art.as_deref().and_then(|a| file_art(ui.ctx(), a)) {
        Some(tex) => { p.image(tex.id(), art, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE); }
        None => { p.rect_filled(art, 0.0, Color32::from_rgb(30, 30, 30)); }
    }
}

/// A rekordbox track list as the unit draws it: PREVIEW (blue preview +
/// cover), TRACK ▲ [count], BPM, KEY; the highlighted row carries
/// LOAD in the BPM/KEY cells. Tap selects, a second tap (or LOAD) loads.
fn draw_track_table(ui: &Ui, lay: &Layout, b: &Browser, rows: &[Row], left: f32, bottom: f32, count_digits: usize, u: f32, out: &mut Vec<Event>) {
    // Columns from the left edge: PREVIEW 239, then TRACK from +240.
    let (pv, tr) = (left + 239.0, left + 240.0);
    let vis = (((bottom - 152.0) / ROW_H).floor() as usize).max(1);
    let sc = lay.screen;
    let at = |x: f32, y: f32| Pos2::new(sc.min.x + x * u, sc.min.y + y * u);
    let p = ui.painter();
    let grey = Color32::from_rgb(60, 60, 60);
    let hb = Rect::from_min_max(at(left, 92.0), at(1280.0, 152.0));
    p.rect_filled(hb, 0.0, grey);
    for x in [pv, 1079.0, 1189.0] {
        p.line_segment([at(x, 92.0), at(x, 152.0)], Stroke::new(1.5 * u, Color32::BLACK));
    }
    // ▲ drawn: the default font has no triangle glyph.
    let up = |r: Rect, c: Color32| {
        let (x, y, h) = (r.max.x + 12.0 * u, r.center().y, 7.0 * u);
        ui.painter().add(Shape::convex_polygon(vec![Pos2::new(x, y + h), Pos2::new(x + 2.0 * h, y + h), Pos2::new(x + h, y - h)], c, Stroke::NONE));
        x + 2.0 * h
    };
    label(ui, at(left + 11.0, 122.0), Align2::LEFT_CENTER, "PREVIEW", 21.0, u, LABEL);
    let th = label(ui, at(tr + 11.0, 122.0), Align2::LEFT_CENTER, "TRACK", 21.0, u, ZOOM_BLUE);
    let tx = up(th, ZOOM_BLUE);
    label(ui, Pos2::new(tx + 22.0 * u, th.center().y), Align2::LEFT_CENTER, format!("[{:0w$}]", rows.len(), w = count_digits), 21.0, u, LABEL);
    up(label(ui, at(1090.0, 122.0), Align2::LEFT_CENTER, "BPM", 21.0, u, LABEL), LABEL);
    up(label(ui, at(1200.0, 122.0), Align2::LEFT_CENTER, "KEY", 21.0, u, LABEL), LABEL);
    // The sort-order key (inert): a box with ⌃⌄.
    let sb = Rect::from_min_max(at(1011.0, 98.0), at(1069.0, 146.0));
    p.rect_filled(sb, 0.0, Color32::from_rgb(50, 50, 50));
    outline(ui, sb, Color32::BLACK, u);
    let (cx, cy) = (sb.center().x, sb.center().y);
    let st = Stroke::new(1.5 * u, LABEL);
    p.line_segment([Pos2::new(cx - 6.0 * u, cy - 4.0 * u), Pos2::new(cx, cy - 10.0 * u)], st);
    p.line_segment([Pos2::new(cx, cy - 10.0 * u), Pos2::new(cx + 6.0 * u, cy - 4.0 * u)], st);
    p.line_segment([Pos2::new(cx - 6.0 * u, cy + 4.0 * u), Pos2::new(cx, cy + 10.0 * u)], st);
    p.line_segment([Pos2::new(cx, cy + 10.0 * u), Pos2::new(cx + 6.0 * u, cy + 4.0 * u)], st);
    let sel = b.selected;
    let first = sel.saturating_sub(vis / 2).min(rows.len().saturating_sub(vis));
    for k in 0..vis {
        let y = 152.0 + ROW_H * k as f32;
        if y + ROW_H > bottom + 1.0 { break; }
        let i = first + k;
        let row = Rect::from_min_max(at(left, y), at(1280.0, y + ROW_H));
        // Column rules, as the unit draws them (not through the LOAD key).
        let rules = |skip_value_cols: bool| for x in [pv, 1079.0, 1189.0] {
            if skip_value_cols && x > pv { continue; }
            ui.painter().line_segment([at(x, y), at(x, y + ROW_H)], Stroke::new(1.0 * u, Color32::from_rgb(70, 70, 70)));
        };
        let (Some(r), Some(e)) = (rows.get(i), b.entries().get(i)) else {
            p.rect_filled(row, 0.0, if k % 2 == 0 { ROW_GREY } else { BG });
            rules(false);
            continue;
        };
        let lit = i == sel;
        let stripe = if k % 2 == 0 { Color32::from_rgb(40, 40, 40) } else { BG };
        p.rect_filled(Rect::from_min_max(at(left, y), at(pv, y + ROW_H)), 0.0, stripe);
        draw_row_preview(ui, e, sc.min.x + left * u, sc.min.y + y * u, u);
        p.rect_filled(Rect::from_min_max(at(tr, y), at(1280.0, y + ROW_H)), 0.0, if lit { SEL_BLUE } else { stripe });
        let cy = sc.min.y + (y + ROW_H / 2.0) * u;
        let title = Rect::from_min_max(at(tr, y), at(1079.0, y + ROW_H));
        if tap(ui, title, &format!("cdj-trk-{i}")).0 {
            out.push(Event::Deck(if lit { ControlEvent::Load } else { ControlEvent::BrowseEncoderDelta { delta: i as i32 - sel as i32 } }));
        }
        label(ui, Pos2::new(sc.min.x + (tr + 23.0) * u, cy), Align2::CENTER_CENTER, "♪", 26.0, u, TEXT);
        p.with_clip_rect(title.shrink2(Vec2::new(0.0, 0.0)).intersect(Rect::from_min_max(at(tr, y), at(1075.0, y + ROW_H))))
            .text(Pos2::new(sc.min.x + (tr + 46.0) * u, cy), Align2::LEFT_CENTER, r.text, FontId::proportional(27.0 * u), if r.played { PLAYED } else { TEXT });
        if r.tagged { p.circle_filled(Pos2::new(sc.min.x + 1068.0 * u, cy), 5.0 * u, BARS_ORANGE); }
        rules(lit);
        if lit {
            let load = Rect::from_min_max(at(1080.0, y), at(1280.0, y + ROW_H));
            p.rect_filled(load, 0.0, Color32::from_rgb(225, 225, 225));
            label(ui, load.center(), Align2::CENTER_CENTER, "LOAD", 22.0, u, Color32::BLACK);
            if tap(ui, load, &format!("cdj-trk-load-{i}")).0 { out.push(Event::Deck(ControlEvent::Load)); }
        } else {
            if let Some(bpm) = e.bpm.filter(|b| *b > 0.0) {
                label(ui, Pos2::new(sc.min.x + 1178.0 * u, cy), Align2::RIGHT_CENTER, format!("{bpm:.1}"), 27.0, u, TEXT);
            }
            if let Some(k) = &e.key {
                label(ui, Pos2::new(sc.min.x + 1200.0 * u, cy), Align2::LEFT_CENTER, k, 27.0, u, TEXT);
            }
        }
    }
    // Scroll position.
    if rows.len() > vis {
        let track = Rect::from_min_max(at(1275.0, 152.0), at(1280.0, bottom));
        let h = track.height() * vis as f32 / rows.len() as f32;
        let y0 = track.min.y + (track.height() - h) * first as f32 / (rows.len() - vis) as f32;
        p.rect_filled(Rect::from_min_size(Pos2::new(track.min.x, y0), Vec2::new(track.width(), h)), 0.0, DIM);
    }
}

/// SEARCH (measured on the unit): the text field in the header, the results
/// as the track table (six rows, no category column) and the keyboard below,
/// with a 123 page for digits and symbols.
fn draw_search(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, b: &Browser, tags: &TagList, u: f32, out: &mut Vec<Event>) {
    use crate::input::SearchEdit as S;
    let sc = lay.screen;
    let at = |x: f32, y: f32| Pos2::new(sc.min.x + x * u, sc.min.y + y * u);
    let p = ui.painter();
    // Header: source tile, then the search field.
    p.rect_filled(lay.header, 0.0, HEADER);
    let st = lay.source;
    p.rect_filled(st, 0.0, SOURCE_TILE);
    label(ui, Pos2::new(st.center().x, st.min.y + 66.0 * u), Align2::CENTER_CENTER, if snap.source_link { "LINK" } else { "USB" }, 18.0, u, TEXT);
    label(ui, Pos2::new(st.center().x, st.min.y + 38.0 * u), Align2::CENTER_CENTER, "▣", 24.0, u, TEXT);
    let field = Rect::from_min_max(at(109.0, 20.0), at(874.0, 70.0));
    p.rect_filled(field, 0.0, Color32::BLACK);
    let m = at(137.0, 43.0);
    p.circle_stroke(m, 7.0 * u, Stroke::new(2.0 * u, LABEL));
    p.line_segment([Pos2::new(m.x + 5.0 * u, m.y + 5.0 * u), Pos2::new(m.x + 11.0 * u, m.y + 11.0 * u)], Stroke::new(2.5 * u, LABEL));
    let q = b.search_query();
    let t = ui.painter().with_clip_rect(field).text(at(173.0, 45.0), Align2::LEFT_CENTER, q, FontId::proportional(28.0 * u), TEXT);
    let cx = if q.is_empty() { at(176.0, 0.0).x } else { t.max.x + 3.0 * u };
    p.line_segment([Pos2::new(cx, sc.min.y + 31.0 * u), Pos2::new(cx, sc.min.y + 59.0 * u)], Stroke::new(2.0 * u, TEXT));

    // Results.
    let rows: Vec<Row> = b.entries().iter().map(|e| Row {
        icon: RowIcon::Track, text: &e.name, value: None,
        tagged: e.load().map_or(false, |l| tags.contains(l)),
        played: e.load().map_or(false, |l| b.was_played(l)),
    }).collect();
    draw_track_table(ui, lay, b, &rows, 0.0, 439.0, 3, u, out);

    // Keyboard.
    let kb = Rect::from_min_max(at(0.0, 439.0), at(1280.0, 720.0));
    p.rect_filled(kb, 0.0, Color32::from_rgb(35, 35, 35));
    p.line_segment([at(0.0, 439.0), at(1280.0, 439.0)], Stroke::new(1.5 * u, Color32::from_rgb(75, 75, 75)));
    let page_id = Id::new("cdj3k-search-123");
    let digits = ui.ctx().data(|d| d.get_temp::<bool>(page_id)).unwrap_or(false);
    let rows_k: [(&str, f32); 3] = if digits {
        [("1234567890", 145.0), ("#+-/:()&@", 195.0), (".,?!'\"~", 295.0)]
    } else {
        [("QWERTYUIOP", 145.0), ("ASDFGHJKL", 195.0), ("ZXCVBNM", 295.0)]
    };
    let key = |r: Rect, text: &str, id: &str| -> bool {
        let p = ui.painter();
        p.rect_filled(r, 0.0, Color32::from_rgb(48, 48, 48));
        p.line_segment([Pos2::new(r.min.x, r.max.y), Pos2::new(r.max.x, r.max.y)], Stroke::new(2.0 * u, Color32::from_rgb(17, 17, 17)));
        p.line_segment([Pos2::new(r.max.x, r.min.y), Pos2::new(r.max.x, r.max.y)], Stroke::new(2.0 * u, Color32::from_rgb(17, 17, 17)));
        label(ui, r.center(), Align2::CENTER_CENTER, text, 26.0, u, TEXT);
        tap(ui, r, id).0
    };
    for (row, (chars, x0)) in rows_k.iter().enumerate() {
        let y = 459.0 + 84.0 * row as f32;
        for (i, ch) in chars.chars().enumerate() {
            let x = x0 + 100.0 * i as f32;
            if key(Rect::from_min_max(at(x, y), at(x + 90.0, y + 74.0)), &ch.to_string(), &format!("cdj-kb-{row}-{i}")) {
                out.push(Event::Ui(UiEvent::Search(S::Char(ch))));
            }
        }
    }
    // ⌫: a key with a pointed left end.
    let bk = Rect::from_min_max(at(1169.0, 459.0), at(1260.0, 533.0));
    let pts = vec![Pos2::new(bk.min.x, bk.center().y), Pos2::new(bk.min.x + 16.0 * u, bk.min.y), Pos2::new(bk.max.x, bk.min.y),
                   Pos2::new(bk.max.x, bk.max.y), Pos2::new(bk.min.x + 16.0 * u, bk.max.y)];
    p.add(Shape::convex_polygon(pts, Color32::from_rgb(48, 48, 48), Stroke::NONE));
    let (c, a) = (Pos2::new(bk.center().x + 6.0 * u, bk.center().y), 8.0 * u);
    p.line_segment([Pos2::new(c.x - a, c.y - a), Pos2::new(c.x + a, c.y + a)], Stroke::new(2.5 * u, TEXT));
    p.line_segment([Pos2::new(c.x - a, c.y + a), Pos2::new(c.x + a, c.y - a)], Stroke::new(2.5 * u, TEXT));
    if tap(ui, bk, "cdj-kb-back").0 { out.push(Event::Ui(UiEvent::Search(S::Backspace))); }
    if key(Rect::from_min_max(at(1139.0, 543.0), at(1260.0, 617.0)), "CLEAR", "cdj-kb-clear") { out.push(Event::Ui(UiEvent::Search(S::Clear))); }
    if key(Rect::from_min_max(at(1139.0, 627.0), at(1260.0, 701.0)), "SPACE", "cdj-kb-space") { out.push(Event::Ui(UiEvent::Search(S::Char(' ')))); }
    if key(Rect::from_min_max(at(20.0, 627.0), at(140.0, 701.0)), if digits { "ABC" } else { "123" }, "cdj-kb-page") {
        ui.ctx().data_mut(|d| d.insert_temp(page_id, !digits));
    }
}

/// TAG LIST: the tagged tracks; LOAD plays one, TAG TRACK removes it.
fn draw_tag_list(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, tags: &TagList, u: f32, out: &mut Vec<Event>) {
    let sc = lay.screen;
    let rows: Vec<Row> = tags.entries().iter().map(|e| Row {
        icon: RowIcon::Track, text: &e.name, value: e.artist.clone(), tagged: false, played: false,
    }).collect();
    draw_list_header(ui, snap, lay, &format!("TAG LIST   {} / {}", tags.len(), TagList::MAX), false, u, out);
    let pane = Rect::from_min_max(Pos2::new(sc.min.x + 92.0 * u, sc.min.y + 92.0 * u), Pos2::new(sc.max.x, sc.min.y + 628.0 * u));
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(sc.min.x, pane.min.y), Pos2::new(pane.min.x, pane.max.y)), 0.0, COLUMN);
    draw_list(ui, pane, "TRACK", &rows, tags.selected, "No tagged tracks. Tag one in BROWSE.", u, Some(out));
    draw_status_strip(ui, snap, lay, u, out);
}

/// UTILITY: the deck's settings, one per row with its value at the right. The
/// rotary moves the highlight; its press (or a tap on the row) steps the value.
fn draw_utility(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, settings: &Settings, cursor: usize, u: f32, out: &mut Vec<Event>) {
    let sc = lay.screen;
    let rows: Vec<Row> = MENU.iter().map(|(s, name)| Row {
        icon: RowIcon::None, text: name, value: Some(settings.value(*s)), tagged: false, played: false,
    }).collect();
    draw_list_header(ui, snap, lay, "UTILITY", false, u, out);
    let pane = Rect::from_min_max(Pos2::new(sc.min.x + 92.0 * u, sc.min.y + 92.0 * u), Pos2::new(sc.max.x, sc.min.y + 628.0 * u));
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(sc.min.x, pane.min.y), Pos2::new(pane.min.x, pane.max.y)), 0.0, COLUMN);
    draw_list(ui, pane, "SETTINGS", &rows, cursor, "", u, None);
    // A tap on a row steps that setting, as on the XDJ's MENU.
    for (i, _) in MENU.iter().enumerate() {
        let rr = Rect::from_min_size(Pos2::new(pane.min.x, pane.min.y + 60.0 * u + ROW_H * u * i as f32), Vec2::new(pane.width(), ROW_H * u));
        if tap(ui, rr, &format!("cdj-util-{i}")).0 { out.push(Event::Ui(UiEvent::MenuTap(i))); }
    }
    // Version and support, as the XDJ skin's UTILITY footer (and the unit's
    // own UTILITY, which shows its firmware version).
    let foot = Color32::from_rgb(170, 170, 170);   // DIM is lost on the row stripes
    let x = pane.min.x + 16.0 * u;
    label(ui, Pos2::new(x, pane.max.y - 40.0 * u), Align2::LEFT_BOTTOM,
          format!("OpenDeck DJ  {}", crate::screen::APP_VERSION), 18.0, u, foot);
    label(ui, Pos2::new(x, pane.max.y - 14.0 * u), Align2::LEFT_BOTTOM, crate::screen::SUPPORT_LINE, 18.0, u, foot);
    draw_status_strip(ui, snap, lay, u, out);
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

    // A.HOT CUE (hot cues load with the track: always on here) and AUTO CUE.
    let r = lay.a_hot_cue;
    outline(ui, r, RED, u);
    label(ui, r.center(), Align2::CENTER_CENTER, "A.HOT CUE", 17.0, u, RED);
    if snap.auto_cue {
        let r = lay.auto_cue;
        outline(ui, r, TEXT, u);
        label(ui, r.center(), Align2::CENTER_CENTER, "AUTO CUE", 17.0, u, TEXT);
    }

    // QUANTIZE (shown only while on, as MT is) and the BEAT JUMP distance.
    if snap.quantize {
        let r = lay.quantize;
        outline(ui, r, RED, u);
        label(ui, Pos2::new(r.center().x, r.min.y + 14.0 * u), Align2::CENTER_CENTER, "QUANTIZE", 13.0, u, RED);
        label(ui, Pos2::new(r.center().x, r.min.y + 36.0 * u), Align2::CENTER_CENTER, "1", 24.0, u, RED);
    }
    let r = lay.beat_jump;
    outline(ui, r, TEXT, u);
    label(ui, Pos2::new(r.center().x, r.min.y + 14.0 * u), Align2::CENTER_CENTER, "BEAT JUMP", 13.0, u, TEXT);
    label(ui, Pos2::new(r.center().x, r.min.y + 36.0 * u), Align2::CENTER_CENTER, "16", 26.0, u, TEXT);

    // Time: REMAIN (elapsed time has no caption), SINGLE, mm:ss + .msec
    let r = lay.time;
    if snap.remain_mode {
        label(ui, Pos2::new(r.min.x + 11.0 * u, r.min.y + 8.0 * u), Align2::LEFT_CENTER, "REMAIN", 15.0, u, TEXT);
    }
    label(ui, Pos2::new(r.min.x + 266.0 * u, r.min.y + 9.0 * u), Align2::CENTER_CENTER, "SINGLE", 15.0, u, TEXT);
    let secs = if snap.remain_mode { snap.remaining_secs() } else { snap.elapsed_secs() };
    let (m, s) = ((secs / 60.0).floor() as u64, (secs % 60.0).floor() as u64);
    let ms = ((secs.fract()) * 1000.0).floor() as u64;
    let big = label(ui, Pos2::new(r.min.x + 9.0 * u, r.min.y + 57.0 * u), Align2::LEFT_CENTER, format!("{m:02}:{s:02}"), 84.0, u, TEXT);
    label(ui, Pos2::new(big.max.x + 2.0 * u, r.min.y + 72.0 * u), Align2::LEFT_CENTER, format!(".{ms:03}"), 44.0, u, TEXT);

    // TEMPO: range badge, ±% readout
    let r = lay.tempo;
    label(ui, Pos2::new(r.min.x + 29.0 * u, r.min.y + 11.0 * u), Align2::CENTER_CENTER, "TEMPO", 15.0, u, TEXT);
    let wide = snap.tempo_range >= 0.99;
    let fill = if wide { None } else if snap.tempo_range < 0.08 { Some(RANGE_6) } else if snap.tempo_range < 0.13 { Some(ORANGE) } else { Some(RANGE_16) };
    match fill {
        Some(c) => {
            p.rect_filled(lay.range, 0.0, c);
            label(ui, lay.range.center(), Align2::CENTER_CENTER, Settings::range_label(snap.tempo_range), 22.0, u, Color32::BLACK);
        }
        None => { label(ui, lay.range.center(), Align2::CENTER_CENTER, "WIDE", 22.0, u, RED); }
    }
    // WIDE shows one decimal, the narrow ranges two.
    let pct = snap.tempo_percent();
    let digits = match (wide, pct.abs() < if wide { 0.05 } else { 0.005 }) {
        (true, true) => "0.0".to_string(),
        (true, false) => format!("{:+.1}", pct),
        (false, true) => "0.00".to_string(),
        (false, false) => format!("{:+.2}", pct),
    };
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
    // Memory points: red ▼ over the overview.
    for &m in snap.memory_cues {
        let x = ov.min.x + m as f32 / total * ov.width();
        let f = lay.flags;
        p.add(Shape::convex_polygon(vec![Pos2::new(x - 7.0 * u, f.min.y), Pos2::new(x + 7.0 * u, f.min.y), Pos2::new(x, f.max.y)],
                                    RED, Stroke::NONE));
    }
    if snap.total_samples > 0 {
        // The address bar: white where the track has played, red ahead.
        p.rect_filled(lay.address, 0.0, RED);
        let a = lay.address;
        let played = (snap.position as f32 / total).clamp(0.0, 1.0);
        p.rect_filled(Rect::from_min_max(a.min, Pos2::new(a.min.x + played * a.width(), a.max.y)), 0.0, TEXT);
        // Major ticks every minute with a grey tick at each half minute:
        // counted back from the end (−m:00, the caption after the tick) in
        // REMAIN, from the start (m:00, the caption before it) otherwise.
        let dur = snap.total_secs();
        let ru = lay.ruler;
        let x_at = |t: f64| ru.min.x + (t / dur) as f32 * ru.width();
        let mut half = 1u32;
        loop {
            let t = if snap.remain_mode { dur - half as f64 * 30.0 } else { half as f64 * 30.0 };
            if t < 0.0 || t > dur { break; }
            let x = x_at(t);
            if half % 2 == 0 {
                p.line_segment([Pos2::new(x, ru.min.y), Pos2::new(x, ru.max.y - 2.0 * u)], Stroke::new(3.0 * u, TEXT));
                if snap.remain_mode {
                    label(ui, Pos2::new(x + 6.0 * u, ru.center().y), Align2::LEFT_CENTER, format!("-{}:00", half / 2), 14.0, u, LABEL);
                } else {
                    label(ui, Pos2::new(x - 4.0 * u, ru.center().y), Align2::RIGHT_CENTER, format!("{}:00", half / 2), 14.0, u, LABEL);
                }
            } else {
                p.line_segment([Pos2::new(x, ru.min.y + 2.0 * u), Pos2::new(x, ru.max.y - 5.0 * u)], Stroke::new(1.0, DIM));
            }
            half += 1;
        }
        for x in [ru.min.x, ru.max.x] {
            p.line_segment([Pos2::new(x, ru.min.y), Pos2::new(x, ru.max.y - 2.0 * u)], Stroke::new(3.0 * u, TEXT));
        }
        // The cue point: an orange ▲ under the overview.
        let x = ov.min.x + snap.cue_point as f32 / total * ov.width();
        p.add(Shape::convex_polygon(vec![Pos2::new(x, ru.min.y), Pos2::new(x + 9.0 * u, ru.max.y), Pos2::new(x - 9.0 * u, ru.max.y)],
                                    BARS_ORANGE, Stroke::NONE));
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

#[cfg(test)]
mod stems_panel_tests {
    use super::*;
    use crate::snapshot::StemsView;
    use egui::{Event as E, PointerButton, RawInput};

    /// One frame of the header and, when open, the stems panel.
    fn frame(ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, events: Vec<E>) -> Vec<Event> {
        let mut out = Vec::new();
        let u = lay.screen.height() / ASPECT.y;
        let _ = ctx.run(RawInput { events, screen_rect: Some(lay.screen), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
                draw_header(ui, ctx, snap, lay, u, &mut out);
                if panel(ctx) == Panel::Stems { draw_stems_panel(ui, ctx, snap, lay, u, &mut out); }
            });
        });
        out
    }

    fn click(ctx: &egui::Context, snap: &DeckSnapshot, lay: &Layout, at: Pos2) -> Vec<Event> {
        let btn = |pressed| E::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(ctx, snap, lay, vec![E::PointerMoved(at)]);
        let mut out = frame(ctx, snap, lay, vec![btn(true)]);
        out.extend(frame(ctx, snap, lay, vec![btn(false)]));
        out.extend(frame(ctx, snap, lay, vec![]));
        out
    }

    #[test]
    fn the_stems_key_shows_only_with_the_setting_on() {
        let tags = opendeck_decode::TrackTags::default();
        let lay = layout(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0)), true);
        let ctx = egui::Context::default();
        let mut snap = DeckSnapshot::test(&tags);
        // Off (the default): the key is not there; a tap on its spot opens nothing.
        click(&ctx, &snap, &lay, lay.stems_key.center());
        assert_eq!(panel(&ctx), Panel::None);
        // On and ready: it opens the panel, and a tap on DRUMS mutes them.
        snap.stems = StemsView { state: StemsState::Ready, gains: [1.0; 3] };
        click(&ctx, &snap, &lay, lay.stems_key.center());
        assert_eq!(panel(&ctx), Panel::Stems);
        let drums = lay.panel[2].union(lay.panel[3]).center();
        let out = click(&ctx, &snap, &lay, drums);
        assert!(matches!(out.as_slice(), [Event::Ui(UiEvent::StemGain { stem: 0, gain })] if *gain == 0.0), "{out:?}");
        let out = click(&ctx, &snap, &lay, lay.panel[0].center());
        assert!(matches!(out.as_slice(), [Event::Ui(UiEvent::StemsFullMix)]), "{out:?}");
        // While loading, the stems are inert.
        snap.stems.state = StemsState::Loading;
        assert!(click(&ctx, &snap, &lay, drums).is_empty());
    }
}
