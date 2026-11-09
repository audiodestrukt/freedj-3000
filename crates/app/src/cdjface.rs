//! CDJ-3000 faceplate: the hardware buttons around the CDJ-3000 screen, for
//! builds where the screen is the only control surface (iPad, a desktop
//! without a controller).
//!
//! Two layouts:
//!
//! - **Deck** (`deck_layout`, the iPad and `--faceplate`): the unit's top
//!   panel in its own arrangement. Screen keys left of the screen, BACK / TAG
//!   TRACK and the rotary right of it, eight hot-cue pads under it, the
//!   loop / memory column down the left with CUE and PLAY at the bottom, the
//!   jog in the middle, and SYNC / MASTER / SLIP / QUANTIZE / JOG MODE /
//!   MASTER TEMPO and the tempo fader down the right.
//! - **Side** (`side_layout`, the desktop's screen-only window): only the
//!   key columns either side of the screen, so SOURCE / BROWSE / MENU and
//!   BACK are a click away without the keyboard.
//!
//! Positions follow the unit's arrangement in millimetres of a 329 × 453 mm
//! top panel, placed by eye, not measured: the screen-fidelity rule applies
//! to the screen; the faceplate only has to put each control where a CDJ
//! player's hand expects it. The jog, fader, PLAY / CUE, loop and lamp
//! buttons reuse the XDJ faceplate's shaded chrome (`screen::draw_faceplate`).
//!
//! Not drawn yet, because there is nothing behind them: PLAYLIST, TRACK
//! FILTER, SHORTCUT, KEY SYNC, TRACK SEARCH / SEARCH, DIRECTION
//! (docs/TODO-cdj-3000.md).

use crate::chrome::{ChromeCache, RoundKind, Sprite};
use crate::input::{ControlEvent, Event, Screen as TopScreen, SearchEdit, UiEvent};
use crate::screen::{self, FaceLayout, ScreenView};
use crate::settings::Setting;
use crate::snapshot::DeckSnapshot;
use crate::taglist::TagList;
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// The CDJ-3000's top panel, millimetres (width × depth).
pub const DECK_ASPECT: Vec2 = Vec2::new(329.0, 453.0);
/// The side layout: the 1280 × 720 screen with a key column each side.
pub const SIDE_ASPECT: Vec2 = Vec2::new(1280.0 + 2.0 * SIDE_W, 720.0);
const SIDE_W: f32 = 170.0;

const KEY_FACE: Color32 = Color32::from_rgb(0x22, 0x25, 0x2a);
const KEY_DOWN: Color32 = Color32::from_rgb(0x3a, 0x3f, 0x47);
const KEY_EDGE: Color32 = Color32::from_rgb(0x45, 0x4a, 0x52);
const LEGEND:   Color32 = Color32::from_rgb(0xc8, 0xcc, 0xd2);
const CAPTION:  Color32 = Color32::from_rgb(0x9a, 0xa0, 0xa8);
const LIT_WHITE:  Color32 = Color32::from_rgb(0xf4, 0xf6, 0xf8);
const LIT_BLUE:   Color32 = Color32::from_rgb(0x2f, 0x8f, 0xff);
const LIT_ORANGE: Color32 = Color32::from_rgb(0xf0, 0x8a, 0x1e);
const LIT_RED:    Color32 = Color32::from_rgb(0xe8, 0x30, 0x30);
const PAD_SET:    Color32 = Color32::from_rgb(0x2c, 0x8f, 0x44);   // a set hot cue (the unit's default green)
const PAD_HELD:   Color32 = Color32::from_rgb(0x4c, 0xd0, 0x6a);

/// A flat key on the faceplate and what it does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    // Left of the screen.
    Source, Browse, TagList, Search, Menu,
    // Right of the screen.
    Back, TagTrack,
    // The loop / memory column.
    BeatLoop4, CallPrev, CallNext, Delete, Memory,
    // Right of the jog.
    Slip, Quantize, BeatSync, Master,
}

impl Key {
    /// (legend on the key, smaller second line).
    fn legend(self) -> (&'static str, &'static str) {
        match self {
            Key::Source => ("SOURCE", ""),
            Key::Browse => ("BROWSE", ""),
            Key::TagList => ("TAG LIST", ""),
            Key::Search => ("SEARCH", ""),
            Key::Menu => ("MENU", "UTILITY"),
            Key::Back => ("BACK", ""),
            Key::TagTrack => ("TAG TRACK", "REMOVE"),
            Key::BeatLoop4 => ("4 BEAT", "LOOP"),
            Key::CallPrev => ("◀ CALL", ""),
            Key::CallNext => ("CALL ▶", ""),
            Key::Delete => ("DELETE", ""),
            Key::Memory => ("MEMORY", ""),
            Key::Slip => ("SLIP", ""),
            Key::Quantize => ("QUANTIZE", ""),
            Key::BeatSync => ("BEAT SYNC", ""),
            Key::Master => ("MASTER", ""),
        }
    }

    /// The event a tap sends.
    pub fn event(self) -> Event {
        match self {
            Key::Source => Event::Ui(UiEvent::Screen(TopScreen::Source)),
            Key::Browse => Event::Ui(UiEvent::Screen(TopScreen::Browse)),
            Key::TagList => Event::Ui(UiEvent::Screen(TopScreen::TagList)),
            Key::Search => Event::Ui(UiEvent::Search(SearchEdit::Open)),
            Key::Menu => Event::Ui(UiEvent::Screen(TopScreen::Menu)),
            Key::Back => Event::Deck(ControlEvent::Back),
            Key::TagTrack => Event::Ui(UiEvent::TagTrack),
            Key::BeatLoop4 => Event::Deck(ControlEvent::BeatLoop { beats: 4.0, held: false }),
            Key::CallPrev => Event::Deck(ControlEvent::MemoryCueCall { next: false }),
            Key::CallNext => Event::Deck(ControlEvent::MemoryCueCall { next: true }),
            Key::Delete => Event::Deck(ControlEvent::MemoryCueDelete),
            Key::Memory => Event::Deck(ControlEvent::MemoryCueSet),
            Key::Slip => Event::Deck(ControlEvent::SlipToggle),
            Key::Quantize => Event::Ui(UiEvent::CycleSetting(Setting::Quantize)),
            Key::BeatSync => Event::Deck(ControlEvent::SyncToggle),
            Key::Master => Event::Deck(ControlEvent::MasterRequest),
        }
    }

    /// The colour the key is lit in right now, if it is.
    fn lit(self, snap: &DeckSnapshot, view: &ScreenView, sel_tagged: bool) -> Option<Color32> {
        let on = |b: bool, c: Color32| b.then_some(c);
        match self {
            Key::Source => on(matches!(view, ScreenView::Source(..)), LIT_WHITE),
            Key::Browse => on(matches!(view, ScreenView::Browse(b) if !b.in_search()), LIT_WHITE),
            Key::TagList => on(matches!(view, ScreenView::TagList), LIT_WHITE),
            Key::Search => on(matches!(view, ScreenView::Browse(b) if b.in_search()), LIT_WHITE),
            Key::Menu => on(matches!(view, ScreenView::Menu(..)), LIT_WHITE),
            Key::TagTrack => on(sel_tagged, LIT_WHITE),
            Key::BeatLoop4 => on(snap.loop_active && (snap.loop_beats - 4.0).abs() < 1e-3, LIT_ORANGE),
            Key::Slip => on(snap.slip, LIT_RED),
            Key::Quantize => on(snap.quantize, LIT_RED),
            Key::BeatSync => on(snap.sync, LIT_BLUE),
            Key::Master => on(snap.master, LIT_ORANGE),
            Key::Back | Key::CallPrev | Key::CallNext | Key::Delete | Key::Memory => None,
        }
    }
}

/// Everything around the screen.
pub struct CdjFace {
    /// The drawn body (letterboxed in the window).
    pub base: Rect,
    /// The shared XDJ chrome (jog, fader, PLAY / CUE, loops, MASTER TEMPO,
    /// JOG MODE); `None` in the side layout.
    pub deck: Option<FaceLayout>,
    pub keys: Vec<(Rect, Key)>,
    /// The rotary selector: turn = scroll, push = LOAD.
    pub rotary: Rect,
    /// Hot cues A–H, under the screen (deck layout only).
    pub pads: Option<[Rect; 8]>,
    /// Legend size, points.
    pub text: f32,
}

/// The CDJ-3000 top panel inside `base`: (screen rect, faceplate). `base`
/// should have about `DECK_ASPECT`; the screen stays exactly 16:9 either way.
pub fn deck_layout(base: Rect) -> (Rect, CdjFace) {
    let (sx, sy) = (base.width() / DECK_ASPECT.x, base.height() / DECK_ASPECT.y);
    let at = |x: f32, y: f32| base.min + Vec2::new(x * sx, y * sy);
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    // Disks are sized by width so they stay round.
    let disk = |cx: f32, cy: f32, rad: f32| Rect::from_center_size(at(cx, cy), Vec2::splat(2.0 * rad * sx));

    // The 9" screen: 200 mm wide, 16:9 in real pixels.
    let screen = Rect::from_min_size(at(64.0, 14.0), Vec2::new(200.0 * sx, 200.0 * sx * 9.0 / 16.0));
    let below = (screen.max.y - base.min.y) / sy;   // the screen's bottom edge, mm

    let mut keys = Vec::new();
    // Left of the screen, top to bottom.
    for (i, k) in [Key::Source, Key::Browse, Key::TagList, Key::Search, Key::Menu].into_iter().enumerate() {
        let y = 16.0 + i as f32 * 20.0;
        keys.push((r(10.0, y, 56.0, y + 14.0), k));
    }
    // Right of the screen: BACK and TAG TRACK over the rotary.
    keys.push((r(271.0, 16.0, 296.0, 29.0), Key::Back));
    keys.push((r(299.0, 16.0, 324.0, 29.0), Key::TagTrack));
    let rotary = disk(297.5, 62.0, 20.0);

    // Hot cues under the screen, across its width.
    let py = below + 8.0;
    let pads = std::array::from_fn(|i| {
        let x = 64.0 + i as f32 * 25.3;
        r(x, py, x + 23.0, py + 16.0)
    });

    // The loop / memory column, under the screen keys: LOOP IN / OUT, then
    // RELOOP/EXIT (round, from the chrome) beside 4 BEAT, CALL ◀ ▶, DELETE
    // and MEMORY.
    let ly = py + 26.0;
    let pair = |y: f32| (r(10.0, y, 32.0, y + 12.0), r(34.0, y, 56.0, y + 12.0));
    keys.push((pair(ly + 20.0).1, Key::BeatLoop4));
    let (a, b) = pair(ly + 40.0);
    keys.extend([(a, Key::CallPrev), (b, Key::CallNext)]);
    let (a, b) = pair(ly + 60.0);
    keys.extend([(a, Key::Delete), (b, Key::Memory)]);

    // Right of the jog.
    keys.push((r(272.0, ly, 296.0, ly + 12.0), Key::Slip));
    keys.push((r(300.0, ly, 324.0, ly + 12.0), Key::Quantize));
    keys.push((r(272.0, ly + 20.0, 296.0, ly + 32.0), Key::BeatSync));
    keys.push((r(300.0, ly + 20.0, 324.0, ly + 32.0), Key::Master));

    let (loop_in, loop_out) = pair(ly);
    let deck = FaceLayout {
        base,
        jog:      disk(164.5, 305.0, 98.0),
        fader:    r(291.0, ly + 70.0, 305.0, 432.0),
        cue:      disk(33.0, 352.0, 19.0),
        play:     disk(33.0, 408.0, 19.0),
        loop_in:  Some(loop_in),
        loop_out: Some(loop_out),
        reloop:   Some(disk(21.0, ly + 26.0, 6.5)),
        browse:   None,
        mt:       disk(312.0, ly + 46.0, 5.5),
        time_mode: None,
        auto_cue:  None,
        tag_track: None,
        back:      None,
        jog_mode:  Some(r(272.0, ly + 40.0, 296.0, ly + 52.0)),
        caption:   3.0 * sx,
    };
    (screen, CdjFace { base, deck: Some(deck), keys, rotary, pads: Some(pads), text: 4.2 * sx })
}

/// The screen with its key columns either side, inside `base` (about
/// `SIDE_ASPECT`): (screen rect, faceplate).
pub fn side_layout(base: Rect) -> (Rect, CdjFace) {
    let u = base.height() / SIDE_ASPECT.y;   // points per screen pixel
    let at = |x: f32, y: f32| base.min + Vec2::new(x * u, y * u);
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(at(x0, y0), at(x1, y1));
    let screen = r(SIDE_W, 0.0, SIDE_W + 1280.0, 720.0);
    let mut keys = Vec::new();
    for (i, k) in [Key::Source, Key::Browse, Key::TagList, Key::Search, Key::Menu].into_iter().enumerate() {
        let y = 40.0 + i as f32 * 132.0;
        keys.push((r(22.0, y, SIDE_W - 22.0, y + 96.0), k));
    }
    let x0 = SIDE_W + 1280.0;
    keys.push((r(x0 + 22.0, 40.0, x0 + SIDE_W - 22.0, 136.0), Key::Back));
    keys.push((r(x0 + 22.0, 172.0, x0 + SIDE_W - 22.0, 268.0), Key::TagTrack));
    let rotary = Rect::from_center_size(at(x0 + SIDE_W / 2.0, 420.0), Vec2::splat(116.0 * u));
    (screen, CdjFace { base, deck: None, keys, rotary, pads: None, text: 19.0 * u })
}

/// Draw the faceplate (body, shared chrome, keys, pads, rotary) and collect
/// its taps. Call before the screen, which paints over its own rect.
/// `screen` is left unpainted: egui draws after the waveform pass, so
/// painting over it would hide the waveforms.
#[allow(clippy::too_many_arguments)]
pub fn draw(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, f: &CdjFace, screen: Rect, view: &ScreenView,
            tag_list: &TagList, chrome: &mut ChromeCache, out: &mut Vec<Event>) {
    let p = ui.painter();
    for part in screen::cover(ui.max_rect(), f.base, f.base) { p.rect_filled(part, 0.0, screen::BODY); }
    // The strips around the screen overlap by a pixel vertically, or their
    // anti-aliased edges leave a dark seam where they meet. The overlap into
    // the screen is its header and ruler, which it paints over.
    for part in screen::cover(f.base, screen, screen) {
        p.rect_filled(part.expand2(Vec2::new(0.0, 1.0)).intersect(f.base), 0.0, screen::FACE_BODY);
    }

    let sel_tagged = match view {
        ScreenView::Browse(b) => b.selected_entry().and_then(|e| e.load()).is_some_and(|l| tag_list.contains(l)),
        ScreenView::TagList => true,
        _ => false,
    };
    if let Some(deck) = &f.deck {
        screen::draw_faceplate(ui, ctx, snap, deck, false, chrome, out);
    }

    let t = f.text;
    draw_keys(ui, &f.keys, |k| k.lit(snap, view, sel_tagged), t, out);

    // Rotary selector: the XDJ's browse knob sprite and behaviour.
    crate::chrome::paint(p, ctx, chrome, Sprite::Round(RoundKind::Knob, None), f.rotary);
    p.text(Pos2::new(f.rotary.center().x, f.rotary.max.y + t * 0.5), Align2::CENTER_TOP, "PUSH: LOAD",
           FontId::proportional(t * 0.8), CAPTION);
    screen::browse_knob(ui, f.rotary, out);

    if let Some(pads) = &f.pads { draw_pads(ui, snap, pads, t, out); }
}

/// The flat keys: a dark slab, lit in the lamp colour when on; a click
/// sends the key's event.
fn draw_keys(ui: &Ui, keys: &[(Rect, Key)], lit_of: impl Fn(Key) -> Option<Color32>, t: f32, out: &mut Vec<Event>) {
    let p = ui.painter();
    for (i, (r, k)) in keys.iter().enumerate() {
        let resp = ui.interact(*r, Id::new(("cdjface-key", i)), Sense::click());
        let lit = lit_of(*k);
        let down = resp.is_pointer_button_down_on();
        let round = r.height() * 0.12;
        p.rect_filled(*r, round, if down { KEY_DOWN } else { KEY_FACE });
        p.rect_stroke(*r, round, Stroke::new(1.0, lit.unwrap_or(KEY_EDGE)));
        // Lit: the lamp colour glows through the key and its edge; the
        // legend goes white so it reads on any lamp colour.
        if let Some(c) = lit {
            p.rect_filled(r.shrink(1.5), round, Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 90));
            p.rect_stroke(r.expand(1.0), round, Stroke::new(2.0, c));
        }
        let (main, sub) = k.legend();
        let col = if lit.is_some() { LIT_WHITE } else { LEGEND };
        let size = fit(main, t, r.width());
        if sub.is_empty() {
            p.text(r.center(), Align2::CENTER_CENTER, main, FontId::proportional(size), col);
        } else {
            p.text(Pos2::new(r.center().x, r.center().y - size * 0.45), Align2::CENTER_CENTER, main, FontId::proportional(size), col);
            p.text(Pos2::new(r.center().x, r.center().y + size * 0.6), Align2::CENTER_CENTER, sub,
                   FontId::proportional(size * 0.75), CAPTION);
        }
        if resp.clicked() { out.push(k.event()); }
    }

}

/// Hot cues A–H: an empty pad sets one at the playhead; a set pad plays from
/// it while held (and, with SLIP, returns on release).
fn draw_pads(ui: &Ui, snap: &DeckSnapshot, pads: &[Rect; 8], t: f32, out: &mut Vec<Event>) {
    let p = ui.painter();
    for (i, r) in pads.iter().enumerate() {
        let slot = i as u8;
        // Press / release, not click: Sense::drag reports a still finger at once.
        let resp = ui.interact(*r, Id::new(("cdjface-pad", i)), Sense::drag());
        let tap = screen::same_frame_tap(ui, *r);
        let (pressed, released, down) = (resp.drag_started() || tap, resp.drag_stopped() || tap, resp.is_pointer_button_down_on());
        let set = snap.hot_cues[i];
        let face = match (set, down) {
            (Some(_), true) => PAD_HELD,
            (Some(_), false) => PAD_SET,
            (None, true) => KEY_DOWN,
            (None, false) => KEY_FACE,
        };
        let round = r.height() * 0.14;
        p.rect_filled(*r, round, face);
        p.rect_stroke(*r, round, Stroke::new(1.0, KEY_EDGE));
        let letter = ((b'A' + slot) as char).to_string();
        p.text(r.center(), Align2::CENTER_CENTER, letter, FontId::proportional(t * 1.3),
               if set.is_some() { LIT_WHITE } else { LEGEND });
        if pressed {
            out.push(Event::Deck(match set {
                Some(_) => ControlEvent::HotCueTrigger { slot, held: true },
                None => ControlEvent::HotCueSet { slot },
            }));
        }
        if released && set.is_some() { out.push(Event::Deck(ControlEvent::HotCueRelease { slot })); }
    }
    let caption = Pos2::new(pads[0].min.x, pads[0].max.y + t * 0.4);
    p.text(caption, Align2::LEFT_TOP, "HOT CUE", FontId::proportional(t * 0.8), CAPTION);
}

/// A legend size that fits `s` inside `width` (a rough per-character width).
fn fit(s: &str, size: f32, width: f32) -> f32 {
    let w = s.chars().count() as f32 * size * 0.62;
    if w > width * 0.9 { size * width * 0.9 / w } else { size }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: Rect, b: Rect) -> bool { a.intersects(b) && a.intersect(b).area() > 0.5 }

    /// Every control lies inside the body and none overlaps another or the
    /// screen, in both layouts and at the iPad's aspect as well as the unit's.
    #[test]
    fn controls_stay_in_the_body_and_apart() {
        let bases = [
            ("deck", Rect::from_min_size(Pos2::ZERO, DECK_ASPECT * 2.0), true),
            ("ipad", Rect::from_min_size(Pos2::ZERO, Vec2::new(1032.0, 1376.0)), true),
            ("side", Rect::from_min_size(Pos2::ZERO, SIDE_ASPECT), false),
        ];
        for (name, base, deck) in bases {
            let (screen, f) = if deck { deck_layout(base) } else { side_layout(base) };
            let mut all: Vec<(String, Rect)> = vec![("screen".into(), screen), ("rotary".into(), f.rotary)];
            for (r, k) in &f.keys { all.push((format!("{k:?}"), *r)); }
            if let Some(pads) = f.pads { for (i, r) in pads.iter().enumerate() { all.push((format!("pad{i}"), *r)); } }
            if let Some(d) = &f.deck {
                all.extend([("jog".into(), d.jog), ("fader".into(), d.fader), ("cue".into(), d.cue),
                            ("play".into(), d.play), ("mt".into(), d.mt)]);
                for (n, r) in [("loop_in", d.loop_in), ("loop_out", d.loop_out), ("reloop", d.reloop), ("jog_mode", d.jog_mode)] {
                    all.push((n.into(), r.expect(n)));
                }
            }
            for (n, r) in &all {
                assert!(base.expand(0.5).contains_rect(*r), "{name}: {n} {r:?} leaves the body {base:?}");
            }
            for i in 0..all.len() {
                for j in i + 1..all.len() {
                    assert!(!overlaps(all[i].1, all[j].1), "{name}: {} overlaps {}", all[i].0, all[j].0);
                }
            }
            let a = screen.width() / screen.height();
            assert!((a - 16.0 / 9.0).abs() < 1e-3, "{name}: screen is {a}, not 16:9");
        }
    }

    /// The buttons the CDJ's screen has no key for are all on the faceplate.
    #[test]
    fn screen_keys_reach_every_list_screen() {
        for deck in [true, false] {
            let base = Rect::from_min_size(Pos2::ZERO, if deck { DECK_ASPECT } else { SIDE_ASPECT });
            let f = if deck { deck_layout(base).1 } else { side_layout(base).1 };
            let have: Vec<Key> = f.keys.iter().map(|(_, k)| *k).collect();
            for k in [Key::Source, Key::Browse, Key::TagList, Key::Search, Key::Menu, Key::Back, Key::TagTrack] {
                assert!(have.contains(&k), "{k:?} missing (deck {deck})");
            }
        }
        assert!(matches!(Key::Menu.event(), Event::Ui(UiEvent::Screen(TopScreen::Menu))));
        assert!(matches!(Key::Back.event(), Event::Deck(ControlEvent::Back)));
        assert!(matches!(Key::Quantize.event(), Event::Ui(UiEvent::CycleSetting(Setting::Quantize))));
    }

    /// A pointer press and release on MENU in the side layout sends
    /// Screen(Menu), and on BACK sends Back: the keys work, not just draw.
    #[test]
    fn clicking_menu_and_back_sends_their_events() {
        use egui::{Event as E, PointerButton, RawInput};
        let base = Rect::from_min_size(Pos2::ZERO, SIDE_ASPECT * 0.5);
        let (_, f) = side_layout(base);
        let ctx = egui::Context::default();
        let screen_rect = Some(Rect::from_min_size(Pos2::ZERO, base.size()));
        let frame = |events: Vec<E>| {
            let mut out = Vec::new();
            let _ = ctx.run(RawInput { events, screen_rect, ..Default::default() }, |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::none())
                    .show(ctx, |ui| draw_keys(ui, &f.keys, |_| None, f.text, &mut out));
            });
            out
        };
        let click = |k: Key| {
            let at = f.keys.iter().find(|(_, kk)| *kk == k).unwrap().0.center();
            let btn = |pressed| E::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            frame(vec![E::PointerMoved(at)]);
            frame(vec![btn(true)]);
            frame(vec![btn(false)])
        };
        let out = click(Key::Menu);
        assert!(matches!(out.as_slice(), [Event::Ui(UiEvent::Screen(TopScreen::Menu))]), "{out:?}");
        let out = click(Key::Back);
        assert!(matches!(out.as_slice(), [Event::Deck(ControlEvent::Back)]), "{out:?}");
    }
}
