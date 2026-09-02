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
use crate::browser::Browser;
use crate::taglist::TagList;
use crate::settings::{Settings, MENU};
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
// Faceplate (chrome) — the physical deck body around the screen.
const BODY:    Color32 = Color32::from_rgb(0x18, 0x1a, 0x1d);   // letterbox + redaction fill
const FACE_BODY: Color32 = Color32::from_rgb(0x2b, 0x2e, 0x33); // stand-in deck body (no photo)
const SILVER:  Color32 = Color32::from_rgb(0xc6, 0xca, 0xce);   // fader handle overlay

/// Same RGB, custom alpha — a translucent lit overlay to lay over the photo.
fn tint(c: Color32, a: u8) -> Color32 { Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a) }

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

pub fn layout(screen: Rect) -> Layout {
    let (ox, oy) = (screen.min.x, screen.min.y);
    let w = screen.width();
    let h = screen.height();
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(Pos2::new(ox + x0 * w, oy + y0 * h), Pos2::new(ox + x1 * w, oy + y1 * h))
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

// ── PERFORM screen ────────────────────────────────────────────────────────────

/// PERFORM's sub-layout over the top band (everything above the info row, left
/// of the CUE/LOOP + CALL column), measured off the XDJ-1000MK2 PERFORM photo
/// (reference/photos/xdj-1000mk2-perform.jpg).  The left column becomes the
/// mode pair / DELETE –CALL / BANK, the phase meter moves to the top, the
/// enlarged waveform shrinks to a strip, four pads sit below it, and the
/// BEAT LOOP row below those.
pub struct PerformLayout {
    pub mode_hotcue:   Rect,
    pub mode_beatjump: Rect,
    pub delete_call:   Rect,
    pub bank:          Rect,
    pub phase:         Rect,
    pub bars:          Rect,
    /// Compact enlarged-waveform strip — the shader viewport in PERFORM.
    pub wave:          Rect,
    pub pads:          [Rect; 4],
    /// BEAT LOOP row: 1/2 · 1 · 2 · 4 · 8 · 16 beats, plus its label at left.
    pub beatloop:      [Rect; 6],
    pub beatloop_lbl:  Rect,
    /// The lit PERFORM key, in the tab row's last slot (tap to exit).
    pub perform_key:   Rect,
}

pub fn perform_layout(screen: Rect) -> PerformLayout {
    let (ox, oy) = (screen.min.x, screen.min.y);
    let (w, h) = (screen.width(), screen.height());
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(Pos2::new(ox + x0 * w, oy + y0 * h), Pos2::new(ox + x1 * w, oy + y1 * h))
    };
    let lc = 0.092;
    // Pads: four across, from just left of the waveform to its right edge.
    let (px0, px1, gap) = (0.125, 0.785, 0.010);
    let pw = (px1 - px0 - 3.0 * gap) / 4.0;
    let pad = |i: usize| { let x = px0 + i as f32 * (pw + gap); r(x, 0.333, x + pw, 0.459) };
    // BEAT LOOP row: six across the same span, below the pads.
    let bgap = 0.007;
    let bw = (px1 - px0 - 5.0 * bgap) / 6.0;
    let bl = |i: usize| { let x = px0 + i as f32 * (bw + bgap); r(x, 0.497, x + bw, 0.629) };
    // PERFORM key = the tab row's 5th slot, exactly where draw_keys puts it.
    let keys = r(lc + 0.006, 0.010, 0.996, 0.120);
    let kw = (keys.width() - 2.0 * 4.0) / 5.0;
    let kx = keys.min.x + 4.0 * (kw + 2.0);
    PerformLayout {
        mode_hotcue:   r(0.004, 0.030, lc - 0.004, 0.085),
        mode_beatjump: r(0.004, 0.092, lc - 0.004, 0.147),
        delete_call:   r(0.004, 0.176, lc - 0.004, 0.289),
        bank:          r(0.004, 0.333, lc - 0.004, 0.440),
        phase:         r(0.305, 0.030, 0.595, 0.115),
        bars:          r(0.610, 0.030, 0.805, 0.115),
        wave:          r(0.190, 0.175, 0.785, 0.300),
        pads:          [pad(0), pad(1), pad(2), pad(3)],
        beatloop:      [bl(0), bl(1), bl(2), bl(3), bl(4), bl(5)],
        beatloop_lbl:  r(0.004, 0.540, lc - 0.004, 0.590),
        perform_key:   Rect::from_min_max(Pos2::new(kx, keys.min.y), keys.max),
    }
}

// ── Faceplate (chrome) ────────────────────────────────────────────────────────
//
// The full physical XDJ-1000MK2 deck rendered around the screen, enabled with
// `--faceplate` (default is screen-only, what the Pi/hardware target wants).
// FIRST PASS: proportions are approximate and want tuning against the real unit;
// all region fractions live in `faceplate_layout` so they are easy to nudge.
// Every control emits the same `ControlEvent`s a physical control would, so this
// doubles as the touch adapter and as the dimensioned mockup for the panel.

/// Faceplate regions, placed as fractions of `base` (the photo's drawn rect, or
/// the whole window when there is no photo).  Fractions measured from the
/// XDJ-1000MK2 photo — tune here.  SYNC/MASTER are on the SCREEN (touch), not
/// physical, so they are not chrome.
pub struct FaceLayout {
    /// The deck body's drawn rect — the photo's letterboxed rect, or the same
    /// proportions synthesised when there is no photo.  Kept so the no-photo
    /// path can paint a stand-in body without re-deriving it.
    pub base:     Rect,
    pub jog:      Rect,
    pub fader:    Rect,
    pub play:     Rect,
    pub cue:      Rect,
    /// Loop IN/OUT: present on the landscape faceplate (cropped from the photo),
    /// but None in portrait/iOS until loop support lands — nothing to trigger yet.
    pub loop_in:  Option<Rect>,
    pub loop_out: Option<Rect>,
    /// RELOOP/EXIT: on the landscape photo, but None in portrait/iOS — it needs
    /// the loop engine (unbuilt in the app), same as loop_in/loop_out.
    pub reloop:   Option<Rect>,
    pub browse:   Rect,
    pub mt:       Rect,   // MASTER TEMPO (key lock)
    /// Portrait-only physical buttons left of the screen (None in landscape,
    /// where TIME/AUTO CUE live inside the LCD as on the real XDJ faceplate).
    pub time_mode: Option<Rect>,
    pub auto_cue:  Option<Rect>,
    /// TAG TRACK / REMOVE (portrait): beside the browse knob, as on the unit.
    pub tag_track: Option<Rect>,
    /// BACK: up one level in BROWSE, or leave TAG LIST / MENU.  Beside TAG
    /// TRACK on the unit (the two buttons above the browse knob).
    pub back:      Option<Rect>,
}

/// Proportions of the deck photo the `faceplate_layout` fractions were measured
/// against.  Used to place the faceplate when no photo is available, so the
/// control fractions still land where they should.
pub const FACE_ASPECT: Vec2 = Vec2::new(860.0, 1090.0);

fn face_rect(base: Rect, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    let (w, h) = (base.width(), base.height());
    Rect::from_min_max(base.min + Vec2::new(x0 * w, y0 * h), base.min + Vec2::new(x1 * w, y1 * h))
}

/// Lay out the deck within `base` (the photo's drawn rect). (screen_rect, chrome).
pub fn faceplate_layout(base: Rect) -> (Rect, FaceLayout) {
    let w = base.width();
    let disk = |cx: f32, cy: f32, rw: f32|
        Rect::from_center_size(base.min + Vec2::new(cx * w, cy * base.height()), Vec2::splat(2.0 * rw * w));

    // Coordinates measured off a labelled grid over the photo (2026-08-27).
    let screen = face_rect(base, 0.250, 0.075, 0.735, 0.290);   // the display panel
    let face = FaceLayout {
        base,
        jog:      disk(0.500, 0.645, 0.340),
        fader:    face_rect(base, 0.895, 0.615, 0.930, 0.940),
        play:     disk(0.070, 0.887, 0.057),
        cue:      disk(0.077, 0.771, 0.057),
        loop_in:  Some(face_rect(base, 0.040, 0.345, 0.110, 0.390)),
        loop_out: Some(face_rect(base, 0.125, 0.345, 0.185, 0.390)),
        reloop:   Some(disk(0.255, 0.370, 0.025)),
        browse:   disk(0.845, 0.205, 0.065),
        mt:       disk(0.925, 0.565, 0.018),
        time_mode: None,
        auto_cue:  None,
        // The two rectangular buttons above the browse knob on the photo:
        // BACK (left) and TAG TRACK / REMOVE (right).
        back:      Some(face_rect(base, 0.797, 0.094, 0.841, 0.126)),
        tag_track: Some(face_rect(base, 0.843, 0.094, 0.888, 0.126)),
    };
    (screen, face)
}

/// iPad 13" portrait pixel size — the aspect the portrait dev window and the
/// on-device layout are proportioned to.
pub const PORTRAIT_ASPECT: Vec2 = Vec2::new(2064.0, 2752.0);

/// Portrait chrome for the iPad (`OPENDECK_PORTRAIT=1`): the LCD spans most of
/// the top, the BROWSE rotary sits top-right, and TIME/AUTO CUE stack in a left
/// column beside the screen (as physical buttons on a real XDJ).  Big jog wheel
/// centred below, tempo fader down the right, transport along the bottom.
/// Fractions of `base` (the portrait window rect); tune here against screenshots.
pub fn portrait_layout(base: Rect) -> (Rect, FaceLayout) {
    let w = base.width();
    let disk = |cx: f32, cy: f32, rw: f32|
        Rect::from_center_size(base.min + Vec2::new(cx * w, cy * base.height()), Vec2::splat(2.0 * rw * w));

    // LCD: the real XDJ-1000MK2 display is 800x480 (5:3) and 6" wide.  Size it to
    // 6" (the minimum asked for), keep the 5:3 aspect EXACTLY, and centre it
    // horizontally at the top.  A 13" iPad in portrait is ~7.82" wide (2064 px
    // @264 ppi), so 6" is ~0.767 of the window width.  Deriving the height from
    // the true base w/h ratio keeps the LCD 5:3 regardless of the window.
    const SCREEN_ASPECT: f32 = 800.0 / 480.0;   // 5:3
    const IPAD_W_IN:     f32 = 7.75;            // 13" iPad portrait width, inches (measured)
    let sw  = (6.0 / IPAD_W_IN).min(0.98);                          // 6" of the width
    let sh  = sw * (base.width() / base.height()) / SCREEN_ASPECT;  // 5:3 in pixels
    let sx0 = (1.0 - sw) / 2.0;                                     // centred
    let sy0 = 0.030;
    let screen = face_rect(base, sx0, sy0, sx0 + sw, sy0 + sh);

    let face = FaceLayout {
        base,
        // Jog dropped from 0.610 → 0.650 so its top clears the LCD's lower edge
        // (they were kissing at the old centre); removing LOOP IN/OUT below frees
        // the room for it.
        jog:      disk(0.500, 0.650, 0.300),
        fader:    face_rect(base, 0.886, 0.462, 0.950, 0.818),
        // CUE above PLAY/PAUSE, stacked vertically at the bottom-left, as on the
        // real XDJ (CUE upper, PLAY the bottom-left corner button).
        cue:      disk(0.115, 0.775, 0.060),
        play:     disk(0.115, 0.905, 0.060),
        // LOOP IN / OUT and RELOOP/EXIT: back now the loop engine exists.  Sat
        // in the strip under the jog (its edge is ~0.875), left of the fader.
        loop_in:  Some(face_rect(base, 0.520, 0.900, 0.605, 0.940)),
        loop_out: Some(face_rect(base, 0.625, 0.900, 0.710, 0.940)),
        reloop:   Some(disk(0.775, 0.920, 0.026)),
        // Browse knob in the right margin beside the screen; TIME/AUTO CUE stacked
        // in the left margin.  (Margins are (1-sw)/2 ≈ 0.117 wide at 6".)
        browse:   disk(0.945, 0.105, 0.048),
        mt:       disk(0.845, 0.448, 0.020),
        time_mode: Some(face_rect(base, 0.012, 0.055, 0.104, 0.120)),
        auto_cue:  Some(face_rect(base, 0.012, 0.150, 0.104, 0.215)),
        // Under the BROWSE knob's caption, right of the LCD (which ends ~0.887):
        // TAG TRACK then BACK, stacked (the margin is too narrow for the unit's
        // side-by-side pair).
        tag_track: Some(face_rect(base, 0.892, 0.192, 0.996, 0.238)),
        back:      Some(face_rect(base, 0.892, 0.252, 0.996, 0.298)),
    };
    (screen, face)
}

/// A round touch target with a translucent lit/press overlay — the photo IS the
/// button, so we only tint it.
fn round_btn(ui: &Ui, r: Rect, name: &str, lit: Option<Color32>, out: &mut Vec<Event>, ev: ControlEvent) {
    let resp = ui.interact(r, Id::new(name), Sense::click());
    if let Some(col) = lit { ui.painter().circle_filled(r.center(), r.width() * 0.5, tint(col, 120)); }
    else if resp.is_pointer_button_down_on() { ui.painter().circle_filled(r.center(), r.width() * 0.5, tint(TEXT, 70)); }
    if resp.clicked() { out.push(Event::Deck(ev)); }
}

/// Backlit-button glow: light around the rim and a faint wash across the face,
/// instead of a flat colour fill.  Used for the lit PLAY / CUE state so the
/// button reads as illuminated from within (rim + graphic) rather than painted
/// over.  `col` is the lamp colour; the alphas bake the falloff.
fn edge_glow(p: &egui::Painter, r: Rect, col: Color32) {
    let c = r.center();
    let rad = r.width() * 0.5;
    // Mute the photographed silver face so the lamp reads, then rim light.
    p.circle_filled(c, rad * 0.98, tint(Color32::BLACK, 90));
    p.circle_filled(c, rad * 0.98, tint(col, 30));                            // face wash
    p.circle_stroke(c, rad * 0.88, Stroke::new(rad * 0.18, tint(col, 48)));   // soft inner halo
    p.circle_stroke(c, rad * 0.97, Stroke::new(rad * 0.07, tint(col, 200)));  // bright rim
}

/// Play/pause symbol (triangle + two bars) in `col`, so the button-face graphic
/// lights up.  `s` is the glyph half-height.
fn play_pause_glyph(p: &egui::Painter, c: Pos2, s: f32, col: Color32) {
    let tx = c.x - s * 1.18;
    p.add(egui::Shape::convex_polygon(
        vec![Pos2::new(tx, c.y - s), Pos2::new(tx, c.y + s), Pos2::new(tx + s * 1.1, c.y)],
        col, Stroke::NONE));
    let bw = s * 0.42;
    let gap = s * 0.34;
    let bx = c.x + s * 0.28;
    p.rect_filled(Rect::from_min_size(Pos2::new(bx, c.y - s), Vec2::new(bw, 2.0 * s)), 0.0, col);
    p.rect_filled(Rect::from_min_size(Pos2::new(bx + bw + gap, c.y - s), Vec2::new(bw, 2.0 * s)), 0.0, col);
}

/// Synthetic tempo-fader slot: a recessed dark channel with a centred travel
/// groove.  Portrait draws this instead of cropping the photo (whose crop
/// dragged in the printed pitch scale and sat off-centre).
fn fader_slot(p: &egui::Painter, ft: Rect) {
    let round = ft.width() * 0.22;
    p.rect_filled(ft, round, Color32::from_rgb(0x0a, 0x0b, 0x0d));
    p.rect_stroke(ft, round, Stroke::new(1.5, Color32::from_rgb(0x2c, 0x2e, 0x33)));
    // Centre travel groove.
    let cx = ft.center().x;
    let inset = ft.width() * 0.22;
    p.line_segment(
        [Pos2::new(cx, ft.min.y + inset), Pos2::new(cx, ft.max.y - inset)],
        Stroke::new(ft.width() * 0.07, Color32::from_rgb(0x17, 0x19, 0x1d)),
    );
}

/// Silver fader knob centred at `hy`, with a bright centre indicator line —
/// the pitch handle, drawn synthetically so it carries no scale ticks.
fn fader_knob(p: &egui::Painter, ft: Rect, hy: f32) {
    let kw = ft.width() * 1.9;
    let kh = kw * 0.60;
    let kr = Rect::from_center_size(Pos2::new(ft.center().x, hy), Vec2::new(kw, kh));
    let round = kh * 0.20;
    p.rect_filled(kr, round, Color32::from_rgb(0x3a, 0x3c, 0x40));                 // dark bevel edge
    p.rect_filled(kr.shrink2(Vec2::new(kw * 0.05, kh * 0.12)), round, SILVER);     // silver face
    // Bright centre indicator line (the "position" mark on the real knob).
    p.rect_filled(
        Rect::from_center_size(kr.center(), Vec2::new(kw * 0.86, kh * 0.13)),
        0.0, Color32::from_rgb(0xf2, 0xf4, 0xf6),
    );
    p.rect_stroke(kr, round, Stroke::new(1.0, Color32::from_rgb(0x1c, 0x1e, 0x22)));
}

/// A rectangular touch target, same overlay treatment as `round_btn`.
fn rect_btn(ui: &Ui, r: Rect, name: &str, lit: Option<Color32>, out: &mut Vec<Event>, ev: ControlEvent) {
    let resp = ui.interact(r, Id::new(name), Sense::click());
    if let Some(col) = lit { ui.painter().rect_filled(r, 2.0, tint(col, 120)); }
    else if resp.is_pointer_button_down_on() { ui.painter().rect_filled(r, 2.0, tint(TEXT, 70)); }
    if resp.clicked() { out.push(Event::Deck(ev)); }
}

/// Draw the faceplate over the photo: redact the branding, paint the live
/// overlays (jog marker, fader handle, lit states), and register the invisible
/// touch targets that emit `ControlEvent`s.
fn draw_faceplate(ui: &Ui, snap: &DeckSnapshot, f: &FaceLayout, photo: bool, sel_tagged: bool,
                  chrome_tex: Option<&egui::TextureHandle>, out: &mut Vec<Event>) {
    let p = ui.painter();
    // Branding is redacted in the asset itself (reference/photos), so nothing to
    // paint over here — just the live overlays and touch targets.

    // With no photo the controls below are invisible (they only tint what the
    // photo already draws), so outline and label them first.  Drawn under the
    // live overlays, which then read as lit state exactly as they do on the photo.
    if !photo {
        let lbl = f.base.width() * 0.018;
        let ring = |r: Rect| p.circle_stroke(r.center(), r.width() * 0.5, Stroke::new(1.5, FAINT));
        let slab = |r: Rect| {
            p.rect_filled(r, 3.0, KEY_LO);
            p.rect_stroke(r, 3.0, Stroke::new(1.0, FAINT));
        };
        // Jog + fader: lift them straight out of the deck photo when we have it
        // (real platter + fader slot); fall back to drawn primitives otherwise.
        // UV regions match faceplate_layout's landscape jog/fader placements.
        if let Some(tex) = chrome_tex {
            let a = tex.size_vec2();
            let vr = |rw: f32| rw * a.x / a.y;   // circle's UV v-radius (aspect-corrected)
            let disc = |c: Pos2, r: f32, uc: Pos2, rw: f32| textured_disc(p, tex, c, r, uc, rw, vr(rw));
            let crop = |dst: Rect, u0: f32, v0: f32, u1: f32, v1: f32|
                p.image(tex.id(), dst, Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)), Color32::WHITE);

            disc(f.jog.center(), f.jog.width() * 0.5, Pos2::new(0.500, 0.645), 0.340);
            // Tempo fader is drawn synthetically in the handle section below
            // (a clean centred slot + knob); the photo crop dragged in the
            // printed pitch scale and sat off-centre.
            // Silver transport buttons (CUE above PLAY/PAUSE, bottom-left); live
            // green/press tints draw over them.
            disc(f.cue.center(),  f.cue.width()  * 0.5, Pos2::new(0.077, 0.771), 0.057);
            disc(f.play.center(), f.play.width() * 0.5, Pos2::new(0.070, 0.887), 0.057);
            // Browse rotary (top-right), RELOOP, the small MASTER-TEMPO button,
            // and the yellow LOOP IN / OUT buttons — all from the photo.
            disc(f.browse.center(), f.browse.width() * 0.5, Pos2::new(0.840, 0.174), 0.046);
            if let Some(r) = f.reloop { disc(r.center(), r.width() * 0.5, Pos2::new(0.255, 0.370), 0.025); }
            disc(f.mt.center(),     f.mt.width()     * 0.5, Pos2::new(0.925, 0.565), 0.018);
            if let Some(r) = f.loop_in  { crop(r, 0.040, 0.345, 0.110, 0.390); }
            if let Some(r) = f.loop_out { crop(r, 0.125, 0.345, 0.185, 0.390); }
        } else {
            // Jog: platter face plus a rim, so the drag target reads as a wheel.
            p.circle_filled(f.jog.center(), f.jog.width() * 0.5, KEY_LO);
            p.circle_stroke(f.jog.center(), f.jog.width() * 0.5, Stroke::new(2.0, FAINT));
            p.circle_stroke(f.jog.center(), f.jog.width() * 0.17, Stroke::new(1.0, FAINT));
            // Tempo fader: slot with a centre detent mark.
            p.rect_filled(f.fader, 2.0, Color32::BLACK);
            p.rect_stroke(f.fader, 2.0, Stroke::new(1.0, FAINT));
            p.line_segment(
                [Pos2::new(f.fader.min.x, f.fader.center().y), Pos2::new(f.fader.max.x, f.fader.center().y)],
                Stroke::new(1.0, DIM),
            );
        }
        // With the photo, every control above is a real crop; only draw the
        // primitive outlines/slabs as the no-photo fallback.
        if chrome_tex.is_none() {
            if let Some(r) = f.reloop { ring(r); }
            ring(f.browse); ring(f.mt);
            ring(f.play); ring(f.cue);
            if let Some(r) = f.loop_in  { slab(r); }
            if let Some(r) = f.loop_out { slab(r); }
        }
        let cap = |r: Rect, s: &str| text(ui, Pos2::new(r.center().x, r.max.y + lbl), Align2::CENTER_TOP, s, lbl, DIM);
        cap(f.play, "PLAY/PAUSE");
        cap(f.cue,  "CUE");
        cap(f.browse, "BROWSE");
        cap(f.mt,     "MASTER TEMPO");   // key-lock button
        if let Some(r) = f.loop_in  { cap(r, "LOOP IN"); }
        if let Some(r) = f.loop_out { cap(r, "LOOP OUT"); }
        if let Some(r) = f.reloop   { cap(r, "RELOOP"); }
        // Portrait-only left column: TIME (elapsed/remain) + AUTO CUE.  Labelled
        // inside the slab since they sit in open space, not on a photo.
        for (rect, s) in [(f.time_mode, "TIME"), (f.auto_cue, "AUTO CUE"), (f.tag_track, "TAG TRACK"), (f.back, "BACK")] {
            if let Some(r) = rect {
                slab(r);
                text(ui, r.center(), Align2::CENTER_CENTER, s, lbl * 0.85, DIM);
            }
        }
    }

    // ── Jog: spinning centre display (CDJ/XDJ platter position indicator) ────
    let r = f.jog.width() * 0.5;
    // The platter hub sits slightly up-and-left of the jog rect centre in the
    // faceplate photo; nudge the synthetic display onto it (tune via capture).
    let pc = f.jog.center() + Vec2::new(JOG_HUB_DX * r, JOG_HUB_DY * r);
    draw_jog_center(p, pc, r * JOG_HUB_R, snap);
    let jr = ui.interact(f.jog, Id::new("fp-jog"), Sense::click_and_drag());
    if jr.drag_started() { out.push(Event::Deck(ControlEvent::JogTouch { touched: true })); }
    if jr.drag_stopped() { out.push(Event::Deck(ControlEvent::JogTouch { touched: false })); }
    if jr.dragged() {
        let dx = jr.drag_delta().x;
        if dx.abs() > 0.01 { out.push(Event::Deck(ControlEvent::JogDelta { delta: dx as i32, velocity_rpm: dx * 2.0 })); }
    }

    // ── Tempo fader: silver handle at the live pitch ─────────────────────────
    let ft  = f.fader;
    let pos = crate::input::speed_to_fader(snap.fader_speed, snap.tempo_range).clamp(0.0, 1.0);
    let hy  = ft.max.y - pos * ft.height();
    if chrome_tex.is_some() {
        // Portrait: synthetic slot + knob (clean, centred, no scale ticks).
        fader_slot(p, ft);
        fader_knob(p, ft, hy);
    } else {
        // Landscape / no-photo: the deck body already draws the slot; just add
        // the silver handle at the live pitch.
        let hrect = Rect::from_center_size(Pos2::new(ft.center().x, hy), Vec2::new(ft.width() * 2.0, ft.height() * 0.045));
        p.rect_filled(hrect, 2.0, SILVER);
        p.rect_stroke(hrect, 2.0, Stroke::new(1.0, Color32::BLACK));
    }
    let fr = ui.interact(ft, Id::new("fp-fader"), Sense::click_and_drag());
    if fr.dragged() || fr.clicked() {
        if let Some(pp) = fr.interact_pointer_pos() {
            let np = ((ft.max.y - pp.y) / ft.height()).clamp(0.0, 1.0);
            out.push(Event::Deck(ControlEvent::TempoFader { position: np }));
        }
    }

    // ── Transport + buttons (overlays + targets) ─────────────────────────────
    // PLAY / CUE are backlit: the light glows around the rim and through the
    // face graphic (play/pause symbol, CUE lettering), not a flat colour wash.
    {
        let resp = ui.interact(f.play, Id::new("fp-play"), Sense::click());
        let lit = if snap.playing { Some(GREEN) } else if resp.is_pointer_button_down_on() { Some(TEXT) } else { None };
        if let Some(col) = lit {
            edge_glow(p, f.play, col);
            play_pause_glyph(p, f.play.center(), f.play.width() * 0.19, col);
        }
        if resp.clicked() { out.push(Event::Deck(ControlEvent::PlayPause)); }
    }
    let cr = ui.interact(f.cue, Id::new("fp-cue"), Sense::click_and_drag());
    if cr.is_pointer_button_down_on() {
        edge_glow(p, f.cue, ORANGE);
        text(ui, f.cue.center(), Align2::CENTER_CENTER, "CUE", f.cue.width() * 0.34, ORANGE);
    }
    if cr.drag_started() || cr.clicked() { out.push(Event::Deck(ControlEvent::Cue { pressed: true })); }
    if cr.drag_stopped()                 { out.push(Event::Deck(ControlEvent::Cue { pressed: false })); }

    if let Some(r) = f.loop_in  { rect_btn(ui, r, "fp-loopin",  None, out, ControlEvent::LoopIn); }
    if let Some(r) = f.loop_out { rect_btn(ui, r, "fp-loopout", None, out, ControlEvent::LoopOut); }
    if let Some(r) = f.reloop { round_btn(ui, r, "fp-reloop", None, out, ControlEvent::Reloop); }
    round_btn(ui, f.mt,      "fp-mt",      snap.key_lock.then_some(ORANGE), out, ControlEvent::KeyLockToggle);

    // ── Browse rotary ────────────────────────────────────────────────────────
    let brr = ui.interact(f.browse, Id::new("fp-browse"), Sense::click_and_drag());
    if brr.dragged() {
        let d = brr.drag_delta().y;
        if d.abs() > 4.0 { out.push(Event::Deck(ControlEvent::BrowseEncoderDelta { delta: if d > 0.0 { 1 } else { -1 } })); }
    }
    if brr.clicked() { out.push(Event::Deck(ControlEvent::Load)); }

    // ── Portrait left column: TIME toggles elapsed/remain; AUTO CUE toggles
    //    cue-at-first-sound on load (lit while on, mirrored by the A.CUE badge). ──
    if let Some(r) = f.time_mode {
        let resp = ui.interact(r, Id::new("fp-time"), Sense::click());
        if snap.remain_mode { p.rect_filled(r, 3.0, tint(BLUE, 120)); }
        else if resp.is_pointer_button_down_on() { p.rect_filled(r, 3.0, tint(TEXT, 70)); }
        if resp.clicked() { out.push(Event::Ui(UiEvent::TimeMode)); }
    }
    if let Some(r) = f.auto_cue {
        let resp = ui.interact(r, Id::new("fp-acue"), Sense::click());
        if snap.auto_cue { p.rect_filled(r, 3.0, tint(BLUE, 120)); }
        else if resp.is_pointer_button_down_on() { p.rect_filled(r, 3.0, tint(TEXT, 70)); }
        if resp.clicked() { out.push(Event::Ui(UiEvent::AutoCue)); }
    }
    // TAG TRACK / REMOVE: tags/untags the highlighted browse track, or removes
    // the highlighted TAG LIST track; lit while that track is tagged.
    if let Some(r) = f.tag_track {
        let resp = ui.interact(r, Id::new("fp-tag"), Sense::click());
        if sel_tagged { p.rect_filled(r, 3.0, tint(BLUE, 120)); }
        else if resp.is_pointer_button_down_on() { p.rect_filled(r, 3.0, tint(TEXT, 70)); }
        if resp.clicked() { out.push(Event::Ui(UiEvent::TagTrack)); }
    }
    // BACK: up a level in BROWSE (LINK / a folder / a playlist), or out of the
    // TAG LIST / MENU screens.  Momentary — nothing to light.
    if let Some(r) = f.back {
        let resp = ui.interact(r, Id::new("fp-back"), Sense::click());
        if resp.is_pointer_button_down_on() { p.rect_filled(r, 3.0, tint(TEXT, 70)); }
        if resp.clicked() { out.push(Event::Deck(ControlEvent::Back)); }
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────────

/// Draw the screen and collect the touch events it produced this frame.
/// Which screen the LCD shows; the top / middle band follows it.
#[derive(Clone, Copy)]
pub enum ScreenView<'a> { Playback, Browse(&'a Browser), Info, Perform, TagList, Menu(&'a Settings, usize) }

pub fn draw(
    ctx:    &egui::Context,
    snap:   &DeckSnapshot,
    lay:    &Layout,
    view:   ScreenView,
    tag_list: &TagList,                        // tag marks in BROWSE + the TAG LIST screen
    face:   Option<&FaceLayout>,
    face_img: Option<(&egui::TextureHandle, Rect)>,
    chrome_tex: Option<&egui::TextureHandle>,   // photo for jog/fader sprites (portrait)
    out:    &mut Vec<Event>,
) {
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, |ui| {
            let h = lay.screen.height();

            // Faceplate: paint the deck photo behind everything (letterboxing the
            // window). The screen renders into its sub-rect over the photo.
            if let Some((tex, irect)) = face_img {
                // Fill only the letterbox margins (outside the image) — filling
                // the whole window would paint over the waveform shader rects,
                // which render underneath.
                for m in cover(ui.max_rect(), irect, irect) {
                    ui.painter().rect_filled(m, 0.0, BODY);
                }
                // Paint the photo for the deck body only, leaving the whole LCD
                // area (lay.screen) to our GUI.
                for part in cover(irect, lay.screen, lay.screen) {
                    image_part(ui.painter(), tex, irect, part);
                }
            } else if let Some(f) = face {
                // Faceplate without the photo (it is not redistributable, so this
                // is the normal case on a fresh checkout and on mobile).  Paint a
                // plain deck body in its place: same rect, same control
                // positions, so the transport is still reachable — the controls
                // are overlays and would otherwise be invisible.
                for m in cover(ui.max_rect(), f.base, f.base) {
                    ui.painter().rect_filled(m, 0.0, BODY);
                }
                for part in cover(f.base, lay.screen, lay.screen) {
                    ui.painter().rect_filled(part, 0.0, FACE_BODY);
                }
            }

            let perform = matches!(view, ScreenView::Perform);
            // Ground everything except the two shader rects.  egui paints
            // after the waveform pass, so those must be left alone.  In PERFORM
            // the enlarged waveform lives in the compact strip instead.
            let wave_hole = if perform { perform_layout(lay.screen).wave } else { lay.wave };
            for r in cover(lay.screen, wave_hole, lay.overview) {
                ui.painter().rect_filled(r, 0.0, BG);
            }

            draw_left(ui, snap, lay, h, perform, out);
            match view {
                ScreenView::Perform => draw_perform(ui, snap, lay, h, out),
                _ => {
                    let active = match view {
                        ScreenView::Browse(_)  => Some(TopScreen::Browse),
                        ScreenView::Info       => Some(TopScreen::Info),
                        ScreenView::TagList    => Some(TopScreen::TagList),
                        ScreenView::Menu(_, _) => Some(TopScreen::Menu),
                        _ => None,
                    };
                    draw_keys(ui, lay, h, active, out);
                }
            }
            match view {
                ScreenView::Perform => {}   // drawn above
                // BROWSE: the middle band (title + phase + enlarged waveform)
                // becomes the file list.  The source column, info row and the
                // overview keep running — the loaded track plays while you browse.
                ScreenView::Browse(browser) => draw_browse(ui, browser, tag_list, lay, h),
                // TAG LIST: the same list, of the tagged tracks.
                ScreenView::TagList => draw_tag_list(ui, tag_list, lay, h),
                // INFO: the middle band shows the loaded track's details.
                ScreenView::Info => draw_info_screen(ui, snap, lay, h),
                // MENU / UTILITY: the settings list.
                ScreenView::Menu(settings, cursor) => draw_menu(ui, settings, cursor, lay, h, out),
                ScreenView::Playback => {
                    draw_title(ui, snap, lay, h);
                    draw_phase(ui, snap, lay, h, out);
                    draw_wave_area(ui, snap, lay, h, out);
                }
            }
            draw_cue_call_keys(ui, lay, h, out);   // every mode, as on the unit
            draw_info(ui, snap, lay, h, out);
            draw_bottom(ui, snap, lay, h, out);

            if let Some(f) = face {
                // TAG TRACK lights when the highlighted browse track is tagged.
                let sel_tagged = match view {
                    ScreenView::Browse(b) => b.selected_entry().and_then(|e| e.load()).map_or(false, |l| tag_list.contains(l)),
                    ScreenView::TagList   => true,
                    _ => false,
                };
                draw_faceplate(ui, snap, f, face_img.is_some(), sel_tagged, chrome_tex, out);
            }
        });
}

// ── BROWSE screen ─────────────────────────────────────────────────────────────

/// Filesystem list: a category header (the current folder) over a scrolling row
/// list with the highlighted row inverted, plus a right-hand detail pane.  Driven
/// by the select encoder / Load / Back (and the keyboard on desktop); the list
/// is presentation-only here — navigation state lives in `Browser`.
/// The middle band that BROWSE / INFO replace: the phase meter + enlarged
/// waveform.  Extends a hair below the waveform so the shader's bottom row of
/// beat ticks can't peek out from under the cover (egui fills to the float
/// edge; the GPU viewport rounds — a 1px sliver otherwise shows).
fn middle_band(lay: &Layout) -> Rect {
    Rect::from_min_max(
        Pos2::new(lay.wave.min.x, lay.phase.min.y),
        Pos2::new(lay.wave.max.x, lay.wave.max.y + 3.0),
    )
}

/// One row of a list screen (BROWSE / TAG LIST).
struct ListRow<'a> { name: &'a str, is_dir: bool, tagged: bool }

/// The browse-style list screen: `header` in the title bar, the rows in the
/// left pane with the highlighted one inverted and tagged tracks marked, and
/// a detail pane on the right (`detail` = kind label, name, action hint).
fn draw_list_screen(ui: &Ui, lay: &Layout, h: f32, header: &str, rows: &[ListRow],
                    selected: usize, empty: &str, detail: Option<(&str, &str, &str)>) {
    let hdr = lay.title;
    ui.painter().rect_filled(hdr, 0.0, BAR);
    text(ui, Pos2::new(hdr.min.x + h * 0.02, hdr.center().y), Align2::LEFT_CENTER, header, h * 0.030, TEXT);

    // Body spans the phase + enlarged-waveform region (covering the shader).
    let body = middle_band(lay);
    ui.painter().rect_filled(body, 0.0, BG);

    // Split: left list pane, right detail pane.
    let split   = body.min.x + body.width() * 0.58;
    let list    = Rect::from_min_max(body.min, Pos2::new(split, body.max.y));
    let detail_r = Rect::from_min_max(Pos2::new(split + h * 0.01, body.min.y), body.max);
    ui.painter().rect_filled(detail_r, 0.0, KEY_LO);

    let row_h = h * 0.052;
    let n_vis = (list.height() / row_h).floor().max(1.0) as usize;
    if rows.is_empty() {
        text(ui, list.center(), Align2::CENTER_CENTER, empty, h * 0.024, DIM);
        return;
    }

    // Scroll so the highlighted row stays roughly centred, clamped to the ends.
    let half  = n_vis / 2;
    let max_first = rows.len().saturating_sub(n_vis);
    let first = selected.saturating_sub(half).min(max_first);
    for slot in 0..n_vis {
        let idx = first + slot;
        if idx >= rows.len() { break; }
        let r   = &rows[idx];
        let y0  = list.min.y + slot as f32 * row_h;
        let row = Rect::from_min_max(Pos2::new(list.min.x, y0), Pos2::new(list.max.x, y0 + row_h));
        let sel = idx == selected;
        if sel { ui.painter().rect_filled(row, 2.0, TEXT); }   // inverted highlight
        let ink = if sel { BG } else if r.is_dir { TEXT } else { DIM };
        // egui's default font is Latin + a few symbols: ♪ renders, most arrows do
        // not.  Folders read as "name/", tracks get a ♪.
        let label = if r.is_dir { format!("{}/", r.name) } else { format!("♪  {}", r.name) };
        text(ui, Pos2::new(row.min.x + h * 0.02, row.center().y), Align2::LEFT_CENTER, label, h * 0.026, ink);
        // Tagged tracks carry a small mark at the row's right edge.
        if r.tagged {
            let m = Rect::from_center_size(Pos2::new(row.max.x - h * 0.03, row.center().y), Vec2::splat(h * 0.016));
            ui.painter().rect_filled(m, 2.0, if sel { BG } else { BLUE });
        }
    }

    if let Some((kind, name, hint)) = detail {
        let x = detail_r.min.x + h * 0.02;
        text(ui, Pos2::new(x, detail_r.min.y + h * 0.05), Align2::LEFT_CENTER, kind, h * 0.020,
             if kind == "FOLDER" { BLUE } else { ORANGE });
        text(ui, Pos2::new(x, detail_r.min.y + h * 0.11), Align2::LEFT_CENTER, name, h * 0.026, TEXT);
        text(ui, Pos2::new(x, detail_r.max.y - h * 0.04), Align2::LEFT_CENTER, hint, h * 0.018, DIM);
    }
}

fn draw_browse(ui: &Ui, browser: &Browser, tag_list: &TagList, lay: &Layout, h: f32) {
    let rows: Vec<ListRow> = browser.entries().iter().map(|e| ListRow {
        name: &e.name, is_dir: e.is_dir,
        tagged: e.load().map_or(false, |l| tag_list.contains(l)),
    }).collect();
    let header = browser.title().to_uppercase();
    let detail = browser.selected_entry().map(|e| {
        let tagged = e.load().map_or(false, |l| tag_list.contains(l));
        let hint = if e.is_dir { "LOAD: open folder" }
                   else if tagged { "LOAD: play  ·  TAG TRACK: untag" }
                   else { "LOAD: play  ·  TAG TRACK: tag" };
        (if e.is_dir { "FOLDER" } else if tagged { "TRACK  (TAGGED)" } else { "TRACK" }, e.name.as_str(), hint)
    });
    draw_list_screen(ui, lay, h, &header, &rows, browser.selected, "— empty —", detail);
}

/// TAG LIST: the tagged tracks, browse-style.  LOAD plays the highlighted
/// one; TAG TRACK / REMOVE drops it.
fn draw_tag_list(ui: &Ui, tag_list: &TagList, lay: &Layout, h: f32) {
    let rows: Vec<ListRow> = tag_list.entries().iter()
        .map(|e| ListRow { name: &e.name, is_dir: false, tagged: true }).collect();
    let header = format!("TAG LIST   {} / {}", tag_list.len(), TagList::MAX);
    let detail = tag_list.selected_entry().map(|e| ("TAGGED TRACK", e.name.as_str(), "LOAD: play  ·  TAG TRACK: remove"));
    draw_list_screen(ui, lay, h, &header, &rows, tag_list.selected,
                     "— no tagged tracks —   (BROWSE, then TAG TRACK)", detail);
}

/// MENU / UTILITY: the settings, one per row — label left, value right.  The
/// rotary moves the highlight and its press steps the value (as does tapping
/// the row); BACK or the MENU tab leaves.
fn draw_menu(ui: &Ui, settings: &Settings, cursor: usize, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    let p = ui.painter();
    let hdr = lay.title;
    p.rect_filled(hdr, 0.0, BAR);
    text(ui, Pos2::new(hdr.min.x + h * 0.02, hdr.center().y), Align2::LEFT_CENTER, "UTILITY", h * 0.030, TEXT);
    text(ui, Pos2::new(hdr.max.x - h * 0.02, hdr.center().y), Align2::RIGHT_CENTER,
         "select: step value   ·   BACK: exit", h * 0.020, DIM);

    let body = middle_band(lay);
    p.rect_filled(body, 0.0, BG);

    let pad   = h * 0.025;
    let row_h = h * 0.062;
    for (i, (setting, label)) in MENU.iter().enumerate() {
        let y0  = body.min.y + pad + i as f32 * row_h;
        let row = Rect::from_min_max(Pos2::new(body.min.x + pad, y0), Pos2::new(body.max.x - pad, y0 + row_h - h * 0.006));
        let (clicked, down) = tap(ui, row, &format!("menu-{i}"));
        let sel = i == cursor;
        p.rect_filled(row, 3.0, if sel { TEXT } else if down { KEY_HI } else { KEY_LO });
        let ink = if sel { BG } else { TEXT };
        text(ui, Pos2::new(row.min.x + h * 0.02, row.center().y), Align2::LEFT_CENTER, *label, h * 0.028, ink);
        text(ui, Pos2::new(row.max.x - h * 0.02, row.center().y), Align2::RIGHT_CENTER,
             settings.value(*setting), h * 0.028, if sel { BG } else { BLUE });
        if clicked { out.push(Event::Ui(UiEvent::MenuTap(i))); }
    }
}

/// INFO screen: the loaded track's details in the middle band — its own tags
/// (title/artist/album/genre/year/key) plus what the deck knows (analysed BPM,
/// length, format, memory points, file).  Same body region as BROWSE.
fn draw_info_screen(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32) {
    let p = ui.painter();
    // Header replaces the title bar.
    let hdr = lay.title;
    p.rect_filled(hdr, 0.0, BAR);
    text(ui, Pos2::new(hdr.min.x + h * 0.02, hdr.center().y), Align2::LEFT_CENTER,
         "INFO", h * 0.030, TEXT);
    text(ui, Pos2::new(hdr.max.x - h * 0.02, hdr.center().y), Align2::RIGHT_CENTER,
         snap.title, h * 0.026, DIM);

    let body = middle_band(lay);
    p.rect_filled(body, 0.0, BG);

    let t = snap.tags;
    let dash = "—".to_string();
    let or_dash = |v: &Option<String>| v.clone().unwrap_or_else(|| dash.clone());

    // Derived values.
    let sr_ch = (snap.sample_rate as f64 * snap.channels as f64).max(1.0);
    let secs  = snap.total_samples as f64 / sr_ch;
    let length = format!("{}:{:02}", (secs / 60.0) as u32, (secs % 60.0) as u32);
    let bpm = match (snap.bpm(), t.bpm) {
        (Some(a), Some(tg)) if (a - tg as f64).abs() > 0.05 => format!("{a:.1}  (tagged {tg:.1})"),
        (Some(a), _) => format!("{a:.1}"),
        (None, Some(tg)) => format!("{tg:.1}  (tagged)"),
        (None, None) => dash.clone(),
    };
    let format = format!("{} Hz · {} ch", snap.sample_rate, snap.channels);
    let memory = match snap.memory_cues.len() {
        0 => "none".to_string(), 1 => "1 point".to_string(), n => format!("{n} points"),
    };

    let rows: [(&str, String); 11] = [
        ("TITLE",   snap.title.to_string()),
        ("ARTIST",  or_dash(&t.artist)),
        ("ALBUM",   or_dash(&t.album)),
        ("GENRE",   or_dash(&t.genre)),
        ("YEAR",    or_dash(&t.year)),
        ("KEY",     or_dash(&t.key)),
        ("BPM",     bpm),
        ("LENGTH",  length),
        ("FORMAT",  format),
        ("MEMORY",  memory),
        ("FILE",    snap.file.to_string()),
    ];

    // Two columns of label/value rows.
    let pad   = h * 0.025;
    let row_h = (body.height() - pad * 2.0) / rows.len() as f32;
    let lbl_x = body.min.x + pad;
    let val_x = body.min.x + h * 0.16;
    let max_w = body.max.x - pad - val_x;
    let fs    = (row_h * 0.62).min(h * 0.030);
    for (i, (label, value)) in rows.iter().enumerate() {
        let y = body.min.y + pad + row_h * (i as f32 + 0.5);
        text(ui, Pos2::new(lbl_x, y), Align2::LEFT_CENTER, *label, fs * 0.72, DIM);
        // Elide long values so they never overrun the band.
        let mut v = value.clone();
        let est = |s: &str| s.chars().count() as f32 * fs * 0.55;
        if est(&v) > max_w {
            let keep = ((max_w / (fs * 0.55)) as usize).saturating_sub(1).max(1);
            v = format!("{}…", v.chars().take(keep).collect::<String>());
        }
        text(ui, Pos2::new(val_x, y), Align2::LEFT_CENTER, v, fs, TEXT);
        if i + 1 < rows.len() {
            let ly = body.min.y + pad + row_h * (i as f32 + 1.0);
            p.line_segment([Pos2::new(lbl_x, ly), Pos2::new(body.max.x - pad, ly)], Stroke::new(1.0, FAINT));
        }
    }
}

/// Rects that tile `outer` minus two holes `a` and `b` (a above b, non-overlapping).
/// Paint a sub-rect of a texture, mapping `part` (a sub-rect of `irect`) to the
/// matching UV region — used to paint the deck photo around the shader rects.
/// Draw a circular crop of `tex` (a triangle-fan disc) — used to lift the round
/// jog platter out of the deck photo with no square edge.  `uvc` is the crop's
/// centre in texture UV (0..1); `uvrx`/`uvry` its UV radii (different because UV
/// normalises each axis, so a circle in the image is an ellipse in UV).
fn textured_disc(p: &egui::Painter, tex: &egui::TextureHandle, center: Pos2, r: f32,
                 uvc: Pos2, uvrx: f32, uvry: f32) {
    use egui::epaint::{Mesh, Vertex};
    let mut mesh = Mesh::with_texture(tex.id());
    let n = 72u32;
    mesh.vertices.push(Vertex { pos: center, uv: uvc, color: Color32::WHITE });
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let (c, s) = (a.cos(), a.sin());
        mesh.vertices.push(Vertex {
            pos: Pos2::new(center.x + c * r, center.y + s * r),
            uv:  Pos2::new(uvc.x + c * uvrx, uvc.y + s * uvry),
            color: Color32::WHITE,
        });
    }
    for i in 1..=n { mesh.indices.extend_from_slice(&[0, i, i + 1]); }
    p.add(egui::Shape::mesh(mesh));
}

fn image_part(p: &egui::Painter, tex: &egui::TextureHandle, irect: Rect, part: Rect) {
    if part.width() <= 0.5 || part.height() <= 0.5 { return; }
    let uv = Rect::from_min_max(
        Pos2::new((part.min.x - irect.min.x) / irect.width(), (part.min.y - irect.min.y) / irect.height()),
        Pos2::new((part.max.x - irect.min.x) / irect.width(), (part.max.y - irect.min.y) / irect.height()),
    );
    p.image(tex.id(), part, uv, Color32::WHITE);
}

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

// ── Jog-wheel centre display ────────────────────────────────────────────────
//
// The CDJ/XDJ jog centre: a "record" of fine radial spokes with a dark notch
// that sweeps clockwise at the playback position, a red cue-point hash fixed on
// the platter, a white segmented ring, and (in VINYL mode) a blue Vinyl badge at
// the hub.  Tune these against a `--faceplate` capture.

/// Hub offset from the jog rect centre onto the platter's photographed hub,
/// and the spoke-ring outer radius — both as fractions of the jog radius.
const JOG_HUB_DX: f32 = -0.013;
const JOG_HUB_DY: f32 = -0.006;
const JOG_HUB_R:  f32 =  0.280;

/// Draw the jog centre display. `center` is the platter hub in screen pixels,
/// `radius` the outer edge of the spoke ring.
fn draw_jog_center(p: &egui::Painter, center: Pos2, radius: f32, snap: &DeckSnapshot) {
    use std::f32::consts::{PI, TAU};
    let sr       = (snap.sample_rate as f32 * snap.channels as f32).max(1.0);
    let secs     = snap.position  as f32 / sr;
    let cue_secs = snap.cue_point as f32 / sr;
    const RPS: f32 = 100.0 / 3.0 / 60.0;              // 33⅓ rpm, like real vinyl
    let pos_ang = (secs     * RPS).fract() * TAU;     // playback notch angle
    let cue_ang = (cue_secs * RPS).fract() * TAU;     // cue hash angle (fixed)

    let spoke_out = radius;
    let spoke_in  = radius * 0.60;
    let ring_r    = radius * 0.52;
    let badge_r   = radius * 0.34;
    let dir = |a: f32| Vec2::new(a.cos(), a.sin());   // +angle = clockwise (y down)

    // Dark backing disc so the display reads on any platter photo.
    p.circle_filled(center, radius * 1.08, Color32::from_rgb(0x0c, 0x0c, 0x0e));

    // Spoke "record", with a swept dark notch at the playback position.
    let spokes   = 128;
    let gap_half = 0.10;                              // notch half-width, radians
    let grey     = Color32::from_rgb(0x96, 0x98, 0x9e);
    for i in 0..spokes {
        let a = i as f32 / spokes as f32 * TAU;
        let mut d = (a - pos_ang).rem_euclid(TAU);
        if d > PI { d -= TAU; }
        if d.abs() < gap_half { continue; }          // leave the notch dark
        p.line_segment([center + dir(a) * spoke_in, center + dir(a) * spoke_out],
                       Stroke::new(1.2, grey));
    }
    // Bright leading edge of the notch = exact playback position (kept in-band).
    p.line_segment([center + dir(pos_ang) * spoke_in, center + dir(pos_ang) * spoke_out],
                   Stroke::new(2.2, Color32::WHITE));

    // Cue-point hash (red): a short bold tick on the outer edge, fixed on the
    // platter at the cue angle.
    p.line_segment([center + dir(cue_ang) * (spoke_in + (spoke_out - spoke_in) * 0.45),
                    center + dir(cue_ang) * spoke_out],
                   Stroke::new(3.2, RED));

    // White segmented inner ring.
    let segs = 36;
    for i in 0..segs {
        let a0 = i as f32 / segs as f32 * TAU;
        let a1 = a0 + (TAU / segs as f32) * 0.55;
        let mut prev = center + dir(a0) * ring_r;
        for s in 1..=3 {
            let a = a0 + (a1 - a0) * s as f32 / 3.0;
            let cur = center + dir(a) * ring_r;
            p.line_segment([prev, cur], Stroke::new(2.0, Color32::from_gray(0xeb)));
            prev = cur;
        }
    }
    p.circle_stroke(center, ring_r * 0.86, Stroke::new(1.5, Color32::from_gray(0xd2)));

    // VINYL badge at the hub.
    // TODO: gate on the real JOG MODE = VINYL state once the input layer exposes
    // it; the reference (and the default) is vinyl mode.
    p.circle_filled(center, badge_r, Color32::from_rgb(0x60, 0x96, 0xc4));
    p.circle_stroke(center, badge_r, Stroke::new(1.5, Color32::from_gray(0xe6)));
    p.text(center, Align2::CENTER_CENTER, "Vinyl",
           FontId::proportional(badge_r * 0.62), Color32::from_rgb(0x14, 0x1e, 0x2d));
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

/// The left column.  In PERFORM the top (logo / LINK / FILE) gives way to the
/// perform controls, but PLAYER and SLIP below stay, as on the unit.
fn draw_left(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, perform: bool, out: &mut Vec<Event>) {
    let p = ui.painter();
    if !perform {
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
    }

    // PLAYER n — dim text standalone; a solid blue box once a Link peer is heard.
    let pl = lay.player;
    let (cap_c, num_c) = if snap.linked {
        p.rect_filled(pl, 2.0, BLUE);
        (TEXT, TEXT)
    } else {
        (FAINT, FAINT)
    };
    text(ui, Pos2::new(pl.center().x, pl.min.y + h * 0.018), Align2::CENTER_CENTER, "PLAYER", h * 0.018, cap_c);
    text(ui, Pos2::new(pl.center().x, pl.center().y + h * 0.020), Align2::CENTER_CENTER, &snap.player.to_string(), h * 0.075, num_c);

    // SLIP: blue when engaged; brighter while actually shadowing (a loop or
    // held hot cue is in progress), as the unit's key blinks then.
    let slip_lit = if snap.slip_shadow.is_some() { Some(Color32::from_rgb(0x6f, 0xb0, 0xff)) }
                   else if snap.slip { Some(BLUE) } else { None };
    if key(ui, lay.slip, "slip", "SLIP", "", h, slip_lit) {
        out.push(Event::Deck(ControlEvent::SlipToggle));
    }
}

// ── Touch-key row ─────────────────────────────────────────────────────────────

/// The top row of screen keys.  `active` lights the key of the screen that is
/// currently showing (BROWSE / INFO), so the tab reads as selected.
fn draw_keys(ui: &Ui, lay: &Layout, h: f32, active: Option<TopScreen>, out: &mut Vec<Event>) {
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
        let lit = (active == Some(*which)).then_some(BLUE);
        if key(ui, kr, m, m, s, h, lit) {
            out.push(Event::Ui(UiEvent::Screen(*which)));
        }
    }
    // PERFORM's expand glyph, top-right.
    let last = Rect::from_min_max(Pos2::new(r.max.x - kw, r.min.y), r.max);
    expand_glyph(ui, last, h);
}

/// The little ⤢ "expand" glyph in the top-right corner of the PERFORM key.
fn expand_glyph(ui: &Ui, key_rect: Rect, h: f32) {
    let c = Pos2::new(key_rect.max.x - h * 0.020, key_rect.min.y + h * 0.020);
    let d = h * 0.009;
    let st = Stroke::new(1.5, TEXT);
    let p = ui.painter();
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x + d, c.y - d)], st);
    p.line_segment([Pos2::new(c.x + d, c.y - d), Pos2::new(c.x + d * 0.2, c.y - d)], st);
    p.line_segment([Pos2::new(c.x + d, c.y - d), Pos2::new(c.x + d, c.y - d * 0.2)], st);
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x - d * 0.2, c.y + d)], st);
    p.line_segment([Pos2::new(c.x - d, c.y + d), Pos2::new(c.x - d, c.y + d * 0.2)], st);
}

/// PERFORM screen: left-column controls (HOT CUE / BEAT JUMP mode pair,
/// DELETE –CALL, BANK), the phase meter at the top, a compact waveform strip
/// (the shader viewport moves there), and four pads — hot cues A–D or E–H
/// (BANK), or ±1/±4-beat jumps in BEAT JUMP mode — then the BEAT LOOP row.
/// PERFORM stays lit in the tab row's last slot; tapping it exits.  CUE/LOOP + CALL, PLAYER/SLIP and
/// the info row are drawn by the caller as in every mode.
fn draw_perform(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    use crate::input::PerformMode as PM;
    let pl = perform_layout(lay.screen);
    let p = ui.painter();
    let hot = snap.perform_mode == PM::HotCue;

    // ── Left column ──────────────────────────────────────────────────────────
    // Pad-mode pair: the selected mode is a light key with dark text, as on the
    // unit; small type so "BEAT JUMP" fits the narrow column.
    let light = Color32::from_rgb(0xd8, 0xdc, 0xe2);
    for (r, name, label, sel, mode) in [
        (pl.mode_hotcue,   "pf-hotcue",   "HOT CUE",   hot,  PM::HotCue),
        (pl.mode_beatjump, "pf-beatjump", "BEAT JUMP", !hot, PM::BeatJump),
    ] {
        let (clicked, down) = tap(ui, r, name);
        p.rect_filled(r, 2.0, if sel { light } else if down { KEY_HI } else { KEY });
        text(ui, r.center(), Align2::CENTER_CENTER, label, h * 0.019, if sel { Color32::BLACK } else { TEXT });
        if clicked { out.push(Event::Ui(UiEvent::PerformMode(mode))); }
    }
    if key(ui, pl.delete_call, "pf-delete", "DELETE", "CALL", h, snap.perform_delete.then_some(RED)) {
        out.push(Event::Ui(UiEvent::PerformDelete));
    }
    if key(ui, pl.bank, "pf-bank", "BANK", if snap.perform_bank == 0 { "A–D" } else { "E–H" }, h,
           (snap.perform_bank == 1).then_some(BLUE)) {
        out.push(Event::Ui(UiEvent::PerformBank));
    }

    // ── Phase meter + Bars, moved to the top ─────────────────────────────────
    draw_phase_at(ui, snap, pl.phase, pl.bars, h, out);

    // ── Pads ────────────────────────────────────────────────────────────────
    let sr_ch = (snap.sample_rate as f32 * snap.channels as f32).max(1.0);
    let pad_green = Color32::from_rgb(0x2c, 0x8f, 0x44);   // a set hot cue (the unit's default colour)
    for (i, r) in pl.pads.iter().enumerate() {
        let name = format!("pf-pad{i}");
        // Pads are press/release (not click): a set hot cue plays on press and,
        // under SLIP, its release returns to the shadow.
        let resp = ui.interact(*r, Id::new(&name), Sense::click_and_drag());
        let (pressed, released, down) = (resp.drag_started() || resp.clicked(), resp.drag_stopped(), resp.is_pointer_button_down_on());
        let clicked = pressed;
        match snap.perform_mode {
            PM::HotCue => {
                let slot   = snap.perform_bank * 4 + i as u8;
                let letter = (b'A' + slot) as char;
                let set    = snap.hot_cues[slot as usize];
                let face = match set {
                    Some(_) if snap.perform_delete => tint(RED, 210),
                    Some(_)                        => pad_green,
                    None if down                   => KEY_HI,
                    None                           => KEY,
                };
                p.rect_filled(*r, 3.0, face);
                let lift = if set.is_some() { h * 0.012 } else { 0.0 };
                text(ui, Pos2::new(r.center().x, r.center().y - lift), Align2::CENTER_CENTER, letter, h * 0.046, TEXT);
                if let Some(pos) = set {
                    let s = pos as f32 / sr_ch;
                    text(ui, Pos2::new(r.center().x, r.max.y - h * 0.018), Align2::CENTER_CENTER,
                         format!("{}:{:04.1}", (s / 60.0) as u32, s % 60.0), h * 0.017, TEXT);
                }
                if clicked {
                    let ev = match (set, snap.perform_delete) {
                        (Some(_), true)  => ControlEvent::HotCueDelete  { slot },
                        (Some(_), false) => ControlEvent::HotCueTrigger { slot, held: true },
                        (None, _)        => ControlEvent::HotCueSet     { slot },
                    };
                    out.push(Event::Deck(ev));
                }
                if released && set.is_some() && !snap.perform_delete {
                    out.push(Event::Deck(ControlEvent::HotCueRelease { slot }));
                }
            }
            PM::BeatJump => {
                let (label, beats) = [("◀ 4", -4.0f32), ("◀ 1", -1.0), ("1 ▶", 1.0), ("4 ▶", 4.0)][i];
                p.rect_filled(*r, 3.0, if down { KEY_HI } else { KEY });
                text(ui, Pos2::new(r.center().x, r.center().y - h * 0.010), Align2::CENTER_CENTER, label, h * 0.040, TEXT);
                text(ui, Pos2::new(r.center().x, r.max.y - h * 0.018), Align2::CENTER_CENTER, "BEATS", h * 0.016, DIM);
                if clicked { out.push(Event::Deck(ControlEvent::BeatJump { beats })); }
            }
        }
    }

    // ── BEAT LOOP row ───────────────────────────────────────────────────────
    // Tap a length to loop that many beats from the current beat; the running
    // loop's pad lights amber (the unit's loop colour); tap it again to exit.
    text(ui, pl.beatloop_lbl.center(), Align2::CENTER_CENTER, "BEAT LOOP", h * 0.016, DIM);
    let amber = Color32::from_rgb(0xfa, 0xc8, 0x28);
    for (i, r) in pl.beatloop.iter().enumerate() {
        let (label, beats) = [("1/2", 0.5f32), ("1", 1.0), ("2", 2.0), ("4", 4.0), ("8", 8.0), ("16", 16.0)][i];
        let running = snap.loop_active && (snap.loop_beats - beats).abs() < 1e-3;
        let (clicked, down) = tap(ui, *r, &format!("pf-bl{i}"));
        p.rect_filled(*r, 3.0, if running { amber } else if down { KEY_HI } else { KEY });
        p.rect_stroke(*r, 3.0, Stroke::new(1.0, if running { amber } else { tint(amber, 110) }));
        text(ui, r.center(), Align2::CENTER_CENTER, label, h * 0.036, if running { Color32::BLACK } else { TEXT });
        if clicked { out.push(Event::Deck(ControlEvent::BeatLoop { beats, held: false })); }
    }

    // ── PERFORM key, lit, in its tab slot; tap to exit ───────────────────────
    if key(ui, pl.perform_key, "PERFORM", "PERFORM", "", h, Some(BLUE)) {
        out.push(Event::Ui(UiEvent::Screen(TopScreen::Perform)));
    }
    expand_glyph(ui, pl.perform_key, h);
}

// ── Title bar ─────────────────────────────────────────────────────────────────

fn draw_title(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32) {
    let r = lay.title;
    ui.painter().rect_filled(r, 0.0, BAR);
    text(ui, Pos2::new(r.min.x + h * 0.02, r.center().y), Align2::LEFT_CENTER, format!("♪ {}", snap.title), h * 0.036, TEXT);
    // Key badge + key name, right.  We don't detect key; show the file's own
    // key tag when it has one (ID3 TKEY / INITIALKEY), else a dash.
    let kx = r.max.x - h * 0.02;
    let key = snap.tags.key.as_deref().unwrap_or("--");
    text(ui, Pos2::new(kx, r.center().y), Align2::RIGHT_CENTER, key, h * 0.034, TEXT);
    let badge = Rect::from_center_size(Pos2::new(kx - h * 0.075, r.center().y), Vec2::new(h * 0.030, h * 0.030));
    ui.painter().rect_filled(badge, 2.0, KEY);
    text(ui, badge.center(), Align2::CENTER_CENTER, "b#", h * 0.017, TEXT);
}

// ── Phase meter + beat countdown ──────────────────────────────────────────────

fn draw_phase(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    draw_phase_at(ui, snap, lay.phase, lay.bars, h, out);
}

/// Bars.beats from the playhead to the next of `cues` (sorted or not), on the
/// beat grid — the "Bars" countdown beside the phase meter.  None without a
/// grid or with no cue ahead.
fn bars_to_next(snap: &DeckSnapshot, cues: impl Iterator<Item = u64>) -> Option<(u32, u32)> {
    let g = snap.beat_grid?;
    if g.bpm <= 0.0 { return None; }
    let pos = snap.position;
    let next = cues.filter(|&c| c > pos).min()?;
    let per_beat = snap.sample_rate as f64 * 60.0 / g.bpm * snap.channels as f64;
    let beats = ((next - pos) as f64 / per_beat).round() as u32;
    Some((beats / 4, beats % 4))
}

/// Phase meter + Bars readouts at explicit rects (PERFORM moves them to the
/// top of the screen).
fn draw_phase_at(ui: &Ui, snap: &DeckSnapshot, r: Rect, b: Rect, h: f32, out: &mut Vec<Event>) {
    // Touching the slot switches between the two views, as on the unit.
    let (toggle, _) = tap(ui, r, "phase-meter");
    if toggle { out.push(Event::Ui(UiEvent::PhaseMeterView)); }

    if snap.phase_ticks_view {
        draw_phase_ticks(ui, snap, r, h);
    } else {
        draw_phase_boxes(ui, snap, r, h);
    }

    // GRID ADJUST: the Bars slot carries the three grid keys instead, as on
    // the unit — RESET, SNAP GRID (CUE), SHIFT GRID (CUE).
    if snap.zoom_grid_mode {
        use crate::input::GridAdjust as GA;
        let gap = h * 0.006;
        let kw = (b.width() - 2.0 * gap) / 3.0;
        for (i, (main, sub, op)) in [("RESET", "", GA::Reset), ("SNAP", "GRID(CUE)", GA::SnapCue), ("SHIFT", "GRID(CUE)", GA::ShiftCue)].into_iter().enumerate() {
            let kr = Rect::from_min_size(Pos2::new(b.min.x + i as f32 * (kw + gap), b.min.y), Vec2::new(kw, b.height()));
            let (clicked, down) = tap(ui, kr, &format!("grid-key-{i}"));
            ui.painter().rect_filled(kr, 2.0, if down { KEY_HI } else { KEY });
            if sub.is_empty() {
                text(ui, kr.center(), Align2::CENTER_CENTER, main, h * 0.022, TEXT);
            } else {
                text(ui, Pos2::new(kr.center().x, kr.center().y - h * 0.011), Align2::CENTER_CENTER, main, h * 0.022, TEXT);
                text(ui, Pos2::new(kr.center().x, kr.center().y + h * 0.012), Align2::CENTER_CENTER, sub, h * 0.015, DIM);
            }
            if clicked { out.push(Event::Ui(UiEvent::GridAdjust(op))); }
        }
        return;
    }

    // "Bars" readouts: orange counts bars.beats to the next memory point,
    // blue to the next hot cue.
    let row = r.height() / 2.0;
    let fs = h * 0.026;
    let fmt = |v: Option<(u32, u32)>| v.map(|(bars, beats)| format!("{bars:02}.{beats}")).unwrap_or_else(|| "--.-".into());
    let memory_cue = bars_to_next(snap, snap.memory_cues.iter().copied());
    let hot_cue    = bars_to_next(snap, snap.hot_cues.iter().flatten().copied());
    text(ui, Pos2::new(b.min.x, b.min.y + row * 0.5), Align2::LEFT_CENTER, fmt(memory_cue), fs, ORANGE);
    text(ui, Pos2::new(b.min.x + h * 0.078, b.min.y + row * 0.5), Align2::LEFT_CENTER, "Bars", h * 0.020, ORANGE);
    text(ui, Pos2::new(b.min.x, b.min.y + row * 1.5), Align2::LEFT_CENTER, fmt(hot_cue), fs, BLUE);
    text(ui, Pos2::new(b.min.x + h * 0.078, b.min.y + row * 1.5), Align2::LEFT_CENTER, "Bars", h * 0.020, BLUE);
}

/// Beat display: two rows of four outlined boxes, as on the unit's phase
/// meter.  Top row = bar within the 4-bar phrase; bottom row = beat within
/// the bar.  Each advances its own current cell solid; the bottom cycles
/// every bar, the top every phrase.
fn draw_phase_boxes(ui: &Ui, snap: &DeckSnapshot, r: Rect, h: f32) {
    let p = ui.painter();
    let gap = h * 0.006;
    let row_h = (r.height() - gap) / 2.0;
    let cell_w = (r.width() - 3.0 * gap) / 4.0;
    let bar  = snap.bar_in_phrase();   // top row: 1–4 bars
    let beat = snap.beat_in_bar();     // bottom row: 1–4 beats
    // When we are master the lit beat cell is orange, else blue (matches the
    // unit); the bar row is always the calmer blue.
    let beat_lit = if snap.master { ORANGE } else { BLUE };

    for i in 0..4u8 {
        let x = r.min.x + i as f32 * (cell_w + gap);
        let top = Rect::from_min_size(Pos2::new(x, r.min.y),               Vec2::new(cell_w, row_h));
        let bot = Rect::from_min_size(Pos2::new(x, r.min.y + row_h + gap), Vec2::new(cell_w, row_h));
        // Top: bar within phrase.
        if bar == Some(i + 1) { p.rect_filled(top, 1.0, BLUE); }
        else                  { p.rect_stroke(top, 1.0, Stroke::new(1.0, Color32::from_rgb(0x24, 0x3a, 0x5c))); }
        // Bottom: beat within bar.
        if beat == Some(i + 1) { p.rect_filled(bot, 1.0, beat_lit); }
        else                   { p.rect_stroke(bot, 1.0, Stroke::new(1.0, BLUE)); }
    }
}

/// Alignment view: `MASTER PLAYER [n]` tag, then two rows of beat ticks —
/// the master's grid above, ours below — scrolling under a fixed white
/// playhead.  Phase offset between the decks reads as horizontal displacement.
fn draw_phase_ticks(ui: &Ui, snap: &DeckSnapshot, r: Rect, h: f32) {
    let p = ui.painter();
    // "MASTER PLAYER" tag + number at the left — but only when following ANOTHER
    // deck.  When we are the master the XDJ hides this label entirely (you are
    // the reference; there is no other player to name).
    let tag_w = h * 0.115;
    if !snap.master {
        text(ui, Pos2::new(r.min.x, r.min.y + h * 0.012), Align2::LEFT_CENTER, "MASTER", h * 0.016, TEXT);
        text(ui, Pos2::new(r.min.x, r.min.y + h * 0.032), Align2::LEFT_CENTER, "PLAYER", h * 0.016, TEXT);
        let nb = Rect::from_min_size(Pos2::new(r.min.x + h * 0.070, r.min.y + h * 0.010), Vec2::new(h * 0.028, h * 0.028));
        let master_txt = if snap.master_player > 0 { snap.master_player.to_string() } else { "-".into() };
        p.rect_filled(nb, 1.0, GOLD);
        text(ui, nb.center(), Align2::CENTER_CENTER, master_txt, h * 0.018, Color32::BLACK);
    }

    // Tick rows.
    let x0 = r.min.x + tag_w;
    let w  = r.max.x - x0;
    let beats_visible = 8.0;
    let beat_px = w / beats_visible;
    let cx = x0 + w * 0.5;
    // Two tick rows.  Top = the remote/master deck; bottom = this (local)
    // deck.  Each row's tall "one" (downbeat) tick comes from *that deck's own*
    // beat-in-bar — the top row from the master's beat packets, the bottom from
    // our grid — so the downbeat marker stays put instead of dancing (it used
    // to derive both from our beat count while the master row scrolled on the
    // master's phase).
    let master_bib = if snap.beat2_beat_in_bar > 0 { snap.beat2_beat_in_bar as i32 } else { 1 };
    let our_bib    = snap.beat_in_bar().unwrap_or(1) as i32;
    // Top row = the master deck's grid; bottom = ours.  When WE are master
    // there is no external master to plot, so the top row's phase is None and
    // it renders as a flat baseline (no ticks) — matching the XDJ, which flattens
    // that grid when it holds master rather than tracking a follower's beats.
    let rows = [
        (r.min.y + r.height() * 0.30, (!snap.master && snap.beat2_bpm > 0.0).then_some(snap.beat2_phase_beats), master_bib, ORANGE),
        (r.min.y + r.height() * 0.78, snap.beat_phase(), our_bib, BLUE),
    ];
    for (y, phase, row_bib, bar_col) in rows {
        p.line_segment([Pos2::new(x0, y), Pos2::new(r.max.x, y)], Stroke::new(1.0, FAINT));
        let Some(ph) = phase else { continue };
        for i in -4..=4i32 {
            let x = cx + (i as f32 - ph) * beat_px;
            if x < x0 || x > r.max.x { continue; }
            // The tick at offset i is a downbeat when this deck's beat-in-bar
            // there is 1.
            let is_bar = (i + row_bib - 1).rem_euclid(4) == 0;
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

/// The right-column CUE/LOOP (DELETE / MEMORY) and CALL (◀ / ▶) keys.  Drawn
/// in every screen mode — the unit keeps them up in BROWSE, INFO and PERFORM.
fn draw_cue_call_keys(ui: &Ui, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    let half = |r: Rect, i: usize| {
        let w = (r.width() - 2.0) / 2.0;
        Rect::from_min_size(Pos2::new(r.min.x + i as f32 * (w + 2.0), r.min.y), Vec2::new(w, r.height()))
    };
    // Memory points: MEMORY stores the cue, DELETE removes the one at the cue,
    // CALL ◀▶ steps to the previous / next point and cues there.
    bracket_caption(ui, lay.cueloop, "CUE / LOOP", h);
    if key(ui, half(lay.cueloop, 0), "cue-delete", "DELETE", "", h, None) {
        out.push(Event::Deck(ControlEvent::MemoryCueDelete));
    }
    if key(ui, half(lay.cueloop, 1), "cue-memory", "MEMORY", "", h, None) {
        out.push(Event::Deck(ControlEvent::MemoryCueSet));
    }

    bracket_caption(ui, lay.call, "CALL", h);
    if key(ui, half(lay.call, 0), "call-prev", "◀", "", h, None) {
        out.push(Event::Deck(ControlEvent::MemoryCueCall { next: false }));
    }
    if key(ui, half(lay.call, 1), "call-next", "▶", "", h, None) {
        out.push(Event::Deck(ControlEvent::MemoryCueCall { next: true }));
    }
}

// ── Info row ──────────────────────────────────────────────────────────────────

fn draw_info(ui: &Ui, snap: &DeckSnapshot, lay: &Layout, h: f32, out: &mut Vec<Event>) {
    let big = h * 0.085;
    let cap = h * 0.019;
    let base_y = |r: Rect| r.max.y - h * 0.010;   // baseline the big readouts share

    // TRACK
    let t = lay.track;
    // Play mode, as the XDJ shows it here: SINGLE = stop at end of track,
    // CONTINUE = roll on to the next in the list.  freedj plays one loaded track
    // and stops, so it's SINGLE (revisit if auto-continue is ever added).  The
    // number is the track's position in the browsed list (placeholder for now).
    text(ui, Pos2::new(t.min.x, t.min.y + h * 0.006), Align2::LEFT_TOP, "SINGLE", cap, TEXT);
    text(ui, Pos2::new(t.min.x, base_y(t)), Align2::LEFT_BOTTOM, "01", big, TEXT);

    // A.CUE — shown only while AUTO CUE is on (on by default on the unit).
    if snap.auto_cue {
        let a = lay.acue;
        ui.painter().rect_stroke(a, 2.0, Stroke::new(1.0, TEXT));
        text(ui, a.center(), Align2::CENTER_CENTER, "A.CUE", h * 0.018, TEXT);
    }

    // Time: tap toggles TIME / REMAIN (a hard button on the unit; handy here).
    let tm = lay.time;
    let (time_tap, _) = tap(ui, tm, "time");
    if time_tap { out.push(Event::Ui(UiEvent::TimeMode)); }
    let shown = if snap.remain_mode { snap.remaining_secs() } else { snap.elapsed_secs() };
    text(ui, Pos2::new(tm.center().x, tm.min.y + h * 0.006), Align2::CENTER_TOP,
         if snap.quantize { "QUANTIZE : 1" } else { "QUANTIZE : –" }, cap, ORANGE);
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
    // Memory points: one triangle per point under the overview at its position
    // on the playing-address bar, as on the unit.  The one the deck is cued at
    // is drawn brighter so DELETE's target is obvious.
    let y = ov.max.y + h * 0.010;
    let total = snap.total_samples.max(1) as f32;
    let tol = snap.channels as u64 * 64;
    for &m in snap.memory_cues {
        let x = ov.min.x + 3.0 + (m as f32 / total) * (ov.width() - 6.0);
        let at_cue = m.abs_diff(snap.cue_point) <= tol;
        let col = if at_cue { ORANGE } else { tint(ORANGE, 150) };
        p.add(egui::Shape::convex_polygon(
            vec![Pos2::new(x - 4.0, y + 4.0), Pos2::new(x + 4.0, y + 4.0), Pos2::new(x, y - 3.0)],
            col, Stroke::NONE));
    }
    // Hot cues: green, hanging from the overview's top edge (memory points sit
    // below it), lettered so they map to the PERFORM pads.
    let ty = ov.min.y - h * 0.004;
    let pad_green = Color32::from_rgb(0x3c, 0xc8, 0x50);
    for (i, c) in snap.hot_cues.iter().enumerate() {
        if let Some(pos) = c {
            let x = ov.min.x + 3.0 + (*pos as f32 / total) * (ov.width() - 6.0);
            p.add(egui::Shape::convex_polygon(
                vec![Pos2::new(x - 4.0, ty - 4.0), Pos2::new(x + 4.0, ty - 4.0), Pos2::new(x, ty + 3.0)],
                pad_green, Stroke::NONE));
            text(ui, Pos2::new(x, ty - h * 0.014), Align2::CENTER_CENTER, (b'A' + i as u8) as char, h * 0.014, pad_green);
        }
    }

    // ±range badge.
    let rg = lay.range;
    p.rect_filled(rg, 2.0, RED);
    text(ui, rg.center(), Align2::CENTER_CENTER, crate::settings::Settings::range_label(snap.tempo_range), h * 0.024, TEXT);

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
