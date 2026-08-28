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
    pub loop_in:  Rect,
    pub loop_out: Rect,
    pub reloop:   Rect,
    pub browse:   Rect,
    pub mt:       Rect,   // MASTER TEMPO (key lock)
    /// Portrait-only physical buttons left of the screen (None in landscape,
    /// where TIME/AUTO CUE live inside the LCD as on the real XDJ faceplate).
    pub time_mode: Option<Rect>,
    pub auto_cue:  Option<Rect>,
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
        loop_in:  face_rect(base, 0.040, 0.345, 0.110, 0.390),
        loop_out: face_rect(base, 0.125, 0.345, 0.185, 0.390),
        reloop:   disk(0.255, 0.370, 0.025),
        browse:   disk(0.845, 0.205, 0.065),
        mt:       disk(0.925, 0.565, 0.018),
        time_mode: None,
        auto_cue:  None,
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
        jog:      disk(0.500, 0.610, 0.300),
        fader:    face_rect(base, 0.886, 0.462, 0.950, 0.818),
        // CUE above PLAY/PAUSE, stacked vertically at the bottom-left, as on the
        // real XDJ (CUE upper, PLAY the bottom-left corner button).
        cue:      disk(0.115, 0.775, 0.060),
        play:     disk(0.115, 0.905, 0.060),
        loop_in:  face_rect(base, 0.520, 0.878, 0.605, 0.918),
        loop_out: face_rect(base, 0.625, 0.878, 0.710, 0.918),
        reloop:   disk(0.775, 0.898, 0.026),
        // Browse knob in the right margin beside the screen; TIME/AUTO CUE stacked
        // in the left margin.  (Margins are (1-sw)/2 ≈ 0.117 wide at 6".)
        browse:   disk(0.945, 0.105, 0.048),
        mt:       disk(0.845, 0.448, 0.020),
        time_mode: Some(face_rect(base, 0.012, 0.055, 0.104, 0.120)),
        auto_cue:  Some(face_rect(base, 0.012, 0.150, 0.104, 0.215)),
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
fn draw_faceplate(ui: &Ui, snap: &DeckSnapshot, f: &FaceLayout, photo: bool,
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
            // Tempo track — the full ridged rail; crop stops above the TEMPO /
            // MULTI PLAYER labels printed below the slider on the photo.
            crop(f.fader, 0.868, 0.582, 0.928, 0.898);
            // Silver transport buttons (CUE above PLAY/PAUSE, bottom-left); live
            // green/press tints draw over them.
            disc(f.cue.center(),  f.cue.width()  * 0.5, Pos2::new(0.077, 0.771), 0.057);
            disc(f.play.center(), f.play.width() * 0.5, Pos2::new(0.070, 0.887), 0.057);
            // Browse rotary (top-right), RELOOP, the small MASTER-TEMPO button,
            // and the yellow LOOP IN / OUT buttons — all from the photo.
            disc(f.browse.center(), f.browse.width() * 0.5, Pos2::new(0.845, 0.205), 0.065);
            disc(f.reloop.center(), f.reloop.width() * 0.5, Pos2::new(0.255, 0.370), 0.025);
            disc(f.mt.center(),     f.mt.width()     * 0.5, Pos2::new(0.925, 0.565), 0.018);
            crop(f.loop_in,  0.040, 0.345, 0.110, 0.390);
            crop(f.loop_out, 0.125, 0.345, 0.185, 0.390);
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
            ring(f.reloop); ring(f.browse); ring(f.mt);
            ring(f.play); ring(f.cue);
            slab(f.loop_in); slab(f.loop_out);
        }
        let cap = |r: Rect, s: &str| text(ui, Pos2::new(r.center().x, r.max.y + lbl), Align2::CENTER_TOP, s, lbl, DIM);
        cap(f.play, "PLAY/PAUSE");
        cap(f.cue,  "CUE");
        cap(f.browse, "BROWSE");
        cap(f.loop_in,  "IN");
        cap(f.loop_out, "OUT");
        // Portrait-only left column: TIME (elapsed/remain) + AUTO CUE.  Labelled
        // inside the slab since they sit in open space, not on a photo.
        for (rect, s) in [(f.time_mode, "TIME"), (f.auto_cue, "AUTO CUE")] {
            if let Some(r) = rect {
                slab(r);
                text(ui, r.center(), Align2::CENTER_CENTER, s, lbl * 0.85, DIM);
            }
        }
    }

    // ── Jog: rotation marker on the photo platter ────────────────────────────
    let c = f.jog.center();
    let r = f.jog.width() * 0.5;
    let secs = snap.position as f32 / (snap.sample_rate as f32 * snap.channels as f32).max(1.0);
    let ang  = secs * 0.6 * std::f32::consts::TAU;
    let dir  = Vec2::new(ang.cos(), ang.sin());
    p.circle_filled(c + dir * (r * 0.70), r * 0.045, if snap.master { ORANGE } else { BLUE });
    let jr = ui.interact(f.jog, Id::new("fp-jog"), Sense::click_and_drag());
    if jr.drag_started() { out.push(Event::Deck(ControlEvent::JogTouch { touched: true })); }
    if jr.drag_stopped() { out.push(Event::Deck(ControlEvent::JogTouch { touched: false })); }
    if jr.dragged() {
        let dx = jr.drag_delta().x;
        if dx.abs() > 0.01 { out.push(Event::Deck(ControlEvent::JogDelta { delta: dx as i32, velocity_rpm: dx * 2.0 })); }
    }

    // ── Tempo fader: silver handle at the live pitch ─────────────────────────
    let ft  = f.fader;
    let pos = crate::input::speed_to_fader(snap.fader_speed).clamp(0.0, 1.0);
    let hy  = ft.max.y - pos * ft.height();
    let hrect = Rect::from_center_size(Pos2::new(ft.center().x, hy), Vec2::new(ft.width() * 2.0, ft.height() * 0.045));
    p.rect_filled(hrect, 2.0, SILVER);
    p.rect_stroke(hrect, 2.0, Stroke::new(1.0, Color32::BLACK));
    let fr = ui.interact(ft, Id::new("fp-fader"), Sense::click_and_drag());
    if fr.dragged() || fr.clicked() {
        if let Some(pp) = fr.interact_pointer_pos() {
            let np = ((ft.max.y - pp.y) / ft.height()).clamp(0.0, 1.0);
            out.push(Event::Deck(ControlEvent::TempoFader { position: np }));
        }
    }

    // ── Transport + buttons (overlays + targets) ─────────────────────────────
    round_btn(ui, f.play, "fp-play", snap.playing.then_some(GREEN), out, ControlEvent::PlayPause);
    let cr = ui.interact(f.cue, Id::new("fp-cue"), Sense::click_and_drag());
    if cr.is_pointer_button_down_on() { p.circle_filled(f.cue.center(), f.cue.width() * 0.5, tint(ORANGE, 120)); }
    if cr.drag_started() || cr.clicked() { out.push(Event::Deck(ControlEvent::Cue { pressed: true })); }
    if cr.drag_stopped()                 { out.push(Event::Deck(ControlEvent::Cue { pressed: false })); }

    rect_btn(ui, f.loop_in,  "fp-loopin",  None, out, ControlEvent::LoopIn);
    rect_btn(ui, f.loop_out, "fp-loopout", None, out, ControlEvent::LoopOut);
    round_btn(ui, f.reloop,  "fp-reloop",  None, out, ControlEvent::Reloop);
    round_btn(ui, f.mt,      "fp-mt",      snap.key_lock.then_some(ORANGE), out, ControlEvent::KeyLockToggle);

    // ── Browse rotary ────────────────────────────────────────────────────────
    let brr = ui.interact(f.browse, Id::new("fp-browse"), Sense::click_and_drag());
    if brr.dragged() {
        let d = brr.drag_delta().y;
        if d.abs() > 4.0 { out.push(Event::Deck(ControlEvent::BrowseEncoderDelta { delta: if d > 0.0 { 1 } else { -1 } })); }
    }
    if brr.clicked() { out.push(Event::Deck(ControlEvent::Load)); }

    // ── Portrait left column: TIME toggles elapsed/remain; AUTO CUE is a
    //    placeholder button until an auto-cue engine exists (#: see backlog). ──
    if let Some(r) = f.time_mode {
        let resp = ui.interact(r, Id::new("fp-time"), Sense::click());
        if snap.remain_mode { p.rect_filled(r, 3.0, tint(BLUE, 120)); }
        else if resp.is_pointer_button_down_on() { p.rect_filled(r, 3.0, tint(TEXT, 70)); }
        if resp.clicked() { out.push(Event::Ui(UiEvent::TimeMode)); }
    }
    if let Some(r) = f.auto_cue {
        let resp = ui.interact(r, Id::new("fp-acue"), Sense::click());
        if resp.is_pointer_button_down_on() { p.rect_filled(r, 3.0, tint(TEXT, 70)); }
        // No auto-cue engine yet — visible/tappable, wired when the feature lands.
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────────

/// Draw the screen and collect the touch events it produced this frame.
pub fn draw(
    ctx:    &egui::Context,
    snap:   &DeckSnapshot,
    lay:    &Layout,
    browse: Option<&Browser>,
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

            // Ground everything except the two shader rects.  egui paints
            // after the waveform pass, so those must be left alone.
            for r in cover(lay.screen, lay.wave, lay.overview) {
                ui.painter().rect_filled(r, 0.0, BG);
            }

            draw_left(ui, snap, lay, h, out);
            draw_keys(ui, lay, h, out);
            if let Some(browser) = browse {
                // BROWSE: the middle band (title + phase + enlarged waveform)
                // becomes the file list.  The source column, info row and the
                // overview keep running — the loaded track plays while you browse.
                draw_browse(ui, browser, lay, h);
            } else {
                draw_title(ui, snap, lay, h);
                draw_phase(ui, snap, lay, h, out);
                draw_wave_area(ui, snap, lay, h, out);
            }
            draw_info(ui, snap, lay, h, out);
            draw_bottom(ui, snap, lay, h, out);

            if let Some(f) = face {
                draw_faceplate(ui, snap, f, face_img.is_some(), chrome_tex, out);
            }
        });
}

// ── BROWSE screen ─────────────────────────────────────────────────────────────

/// Filesystem list: a category header (the current folder) over a scrolling row
/// list with the highlighted row inverted, plus a right-hand detail pane.  Driven
/// by the select encoder / Load / Back (and the keyboard on desktop); the list
/// is presentation-only here — navigation state lives in `Browser`.
fn draw_browse(ui: &Ui, browser: &Browser, lay: &Layout, h: f32) {
    // Header replaces the title bar with the current folder name.
    let hdr = lay.title;
    ui.painter().rect_filled(hdr, 0.0, BAR);
    let folder = browser.title();
    text(ui, Pos2::new(hdr.min.x + h * 0.02, hdr.center().y), Align2::LEFT_CENTER,
         folder.to_uppercase(), h * 0.030, TEXT);

    // Body spans the phase + enlarged-waveform region (covering the shader).
    let body = Rect::from_min_max(
        Pos2::new(lay.wave.min.x, lay.phase.min.y),
        Pos2::new(lay.wave.max.x, lay.wave.max.y),
    );
    ui.painter().rect_filled(body, 0.0, BG);

    // Split: left list pane, right detail pane.
    let split   = body.min.x + body.width() * 0.58;
    let list    = Rect::from_min_max(body.min, Pos2::new(split, body.max.y));
    let detail  = Rect::from_min_max(Pos2::new(split + h * 0.01, body.min.y), body.max);
    ui.painter().rect_filled(detail, 0.0, KEY_LO);

    let entries = browser.entries();
    let row_h   = h * 0.052;
    let n_vis   = (list.height() / row_h).floor().max(1.0) as usize;

    if entries.is_empty() {
        text(ui, list.center(), Align2::CENTER_CENTER, "— empty —", h * 0.026, DIM);
        return;
    }

    // Scroll so the highlighted row stays roughly centred, clamped to the ends.
    let sel   = browser.selected;
    let half  = n_vis / 2;
    let max_first = entries.len().saturating_sub(n_vis);
    let first = sel.saturating_sub(half).min(max_first);

    for slot in 0..n_vis {
        let idx = first + slot;
        if idx >= entries.len() { break; }
        let e   = &entries[idx];
        let y0  = list.min.y + slot as f32 * row_h;
        let row = Rect::from_min_max(Pos2::new(list.min.x, y0), Pos2::new(list.max.x, y0 + row_h));
        let selected = idx == sel;
        if selected {
            ui.painter().rect_filled(row, 2.0, TEXT);   // inverted highlight
        }
        let ink = if selected { BG } else if e.is_dir { TEXT } else { DIM };
        // egui's default font is Latin + a few symbols: ♪ renders, most arrows do
        // not.  Folders read as "name/", tracks get a ♪.
        let label = if e.is_dir { format!("{}/", e.name) } else { format!("♪  {}", e.name) };
        text(ui, Pos2::new(row.min.x + h * 0.02, row.center().y), Align2::LEFT_CENTER,
             label, h * 0.026, ink);
    }

    // Detail pane: the highlighted item.
    if let Some(e) = browser.selected_entry() {
        let x = detail.min.x + h * 0.02;
        text(ui, Pos2::new(x, detail.min.y + h * 0.05), Align2::LEFT_CENTER,
             if e.is_dir { "FOLDER" } else { "TRACK" }, h * 0.020, if e.is_dir { BLUE } else { ORANGE });
        text(ui, Pos2::new(x, detail.min.y + h * 0.11), Align2::LEFT_CENTER,
             &e.name, h * 0.026, TEXT);
        let hint = if e.is_dir { "LOAD: open folder" } else { "LOAD: play track" };
        text(ui, Pos2::new(x, detail.max.y - h * 0.04), Align2::LEFT_CENTER, hint, h * 0.018, DIM);
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
    text(ui, Pos2::new(pl.center().x, pl.center().y + h * 0.020), Align2::CENTER_CENTER, &snap.player.to_string(), h * 0.075, num_c);

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
    // Play mode, as the XDJ shows it here: SINGLE = stop at end of track,
    // CONTINUE = roll on to the next in the list.  freedj plays one loaded track
    // and stops, so it's SINGLE (revisit if auto-continue is ever added).  The
    // number is the track's position in the browsed list (placeholder for now).
    text(ui, Pos2::new(t.min.x, t.min.y + h * 0.006), Align2::LEFT_TOP, "SINGLE", cap, TEXT);
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
