//! CDJ-3000 faceplate: the hardware buttons around the CDJ-3000 screen, for
//! builds where the screen is the only control surface (iPad, a desktop
//! without a controller).
//!
//! Two layouts:
//!
//! - **Deck** (`deck_layout`, the iPad and `--faceplate`): the unit's whole
//!   top panel. The six screen keys in a row above the screen; BACK, TAG
//!   TRACK, TRACK FILTER and SHORTCUT around the rotary right of it; the hot
//!   cues under it; TIME MODE, SLIP / QUANTIZE, the loop keys, BEAT JUMP,
//!   TRACK SEARCH / SEARCH, CUE and PLAY down the left; CALL / DELETE /
//!   MEMORY over the jog; JOG MODE, BEAT SYNC / MASTER / KEY SYNC, the tempo
//!   keys and the fader down the right.
//! - **Side** (`side_layout`, the desktop's screen-only window): only the
//!   screen keys and BACK / TAG TRACK / rotary either side of the screen, so
//!   SOURCE / BROWSE / MENU are a click away without the keyboard.
//!
//! The deck layout is measured off cdj3k-emu's drawing of the CDJ-3000 panel
//! (`docs/preview.png` in that project, 1255 × 1721 px for the unit's
//! 329 × 453 mm), so every control sits where it does on the unit. Lamps
//! follow the unit as the emulator reports them: the screen keys glow dimly
//! and light fully on their screen, SLIP / QUANTIZE dim off and full on.
//! The jog, fader, PLAY / CUE and MASTER TEMPO reuse the XDJ faceplate's
//! shaded chrome (`screen::draw_faceplate`).
//!
//! Drawn but inert, because there is nothing behind them yet: PLAY LIST,
//! TRACK FILTER, SHORTCUT, KEY SYNC (docs/TODO-cdj-3000.md).

use crate::chrome::{ChromeCache, RoundKind, Sprite};
use crate::input::{ControlEvent, Event, Screen as TopScreen, SearchEdit, UiEvent};
use crate::screen::{self, FaceLayout, ScreenView};
use crate::settings::Setting;
use crate::snapshot::DeckSnapshot;
use crate::taglist::TagList;
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// The CDJ-3000's top panel, in the reference drawing's pixels (the unit is
/// 329 × 453 mm, the same shape).
pub const DECK_ASPECT: Vec2 = Vec2::new(1255.0, 1721.0);
/// The side layout: the 1280 × 720 screen with a key column each side.
pub const SIDE_ASPECT: Vec2 = Vec2::new(1280.0 + 2.0 * SIDE_W, 720.0);
const SIDE_W: f32 = 170.0;

/// How long a press must last to count as a long press (MENU → LOAD
/// PREVIOUS TRACK, TIME MODE → AUTO CUE).
const LONG_PRESS_S: f64 = 1.0;
/// Small round keys take touches within at least this radius (reference
/// px), so a fingertip finds them on the iPad.
const MIN_HIT_R: f32 = 26.0;

const KEY_FACE: Color32 = Color32::from_rgb(0x22, 0x25, 0x2a);
const KEY_DOWN: Color32 = Color32::from_rgb(0x3a, 0x3f, 0x47);
const KEY_EDGE: Color32 = Color32::from_rgb(0x45, 0x4a, 0x52);
const LEGEND:   Color32 = Color32::from_rgb(0x8a, 0x90, 0x98);
const INERT:    Color32 = Color32::from_rgb(0x55, 0x5a, 0x62);
const CAPTION:  Color32 = Color32::from_rgb(0x9a, 0xa0, 0xa8);
const LIT_WHITE:  Color32 = Color32::from_rgb(0xf4, 0xf6, 0xf8);
const LIT_BLUE:   Color32 = Color32::from_rgb(0x2f, 0x8f, 0xff);
const LIT_ORANGE: Color32 = Color32::from_rgb(0xf0, 0x8a, 0x1e);
const LIT_RED:    Color32 = Color32::from_rgb(0xe8, 0x30, 0x30);
const LIT_YELLOW: Color32 = Color32::from_rgb(0xf2, 0xe2, 0x1a);
const LIT_GREEN:  Color32 = Color32::from_rgb(0x2a, 0xd0, 0x6a);
const PAD_SET:    Color32 = Color32::from_rgb(0x00, 0xc8, 0x3c);   // a set hot cue (the unit's default green)
const PAD_EMPTY:  Color32 = Color32::from_rgb(0x5a, 0x60, 0x68);

/// A key on the faceplate (everything but the jog, fader, PLAY / CUE and
/// MASTER TEMPO, which are the shared chrome).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]   // KEY SYNC is the button's name
pub enum Key {
    // The row above the screen.
    Source, Browse, TagList, Playlist, Search, Menu,
    // Around the rotary.
    Back, TagTrack, TrackFilter, Shortcut,
    // The left column.
    TimeMode, Slip, Quantize, CallDelete,
    LoopIn, LoopOut, Reloop, Loop4, Loop8, JumpPrev, JumpNext,
    TrackPrev, TrackNext, SearchPrev, SearchNext,
    // Over the jog.
    CallPrev, CallNext, Delete, Memory,
    // The right column.
    JogMode, BeatSync, Master, KeySync, TempoRange, TempoReset,
}

/// How bright a key's lamp is, as the unit drives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lamp { Off, Dim, Full, Blink }

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// A flat key with its legend on it.
    Slab,
    /// A round key; its legend (if any) on the cap, the rest printed around it.
    Round,
}

impl Key {
    /// Every key, for the lamp report and the tests.
    pub const ALL: [Key; 35] = [
        Key::Source, Key::Browse, Key::TagList, Key::Playlist, Key::Search, Key::Menu,
        Key::Back, Key::TagTrack, Key::TrackFilter, Key::Shortcut,
        Key::TimeMode, Key::Slip, Key::Quantize, Key::CallDelete,
        Key::LoopIn, Key::LoopOut, Key::Reloop, Key::Loop4, Key::Loop8, Key::JumpPrev, Key::JumpNext,
        Key::TrackPrev, Key::TrackNext, Key::SearchPrev, Key::SearchNext,
        Key::CallPrev, Key::CallNext, Key::Delete, Key::Memory,
        Key::JogMode, Key::BeatSync, Key::Master, Key::KeySync, Key::TempoRange, Key::TempoReset,
    ];

    /// (legend on the key, smaller second line).
    fn legend(self) -> (&'static str, &'static str) {
        match self {
            Key::Source => ("SOURCE", ""),
            Key::Browse => ("BROWSE", ""),
            Key::TagList => ("TAG LIST", ""),
            Key::Playlist => ("PLAY LIST", ""),
            Key::Search => ("SEARCH", ""),
            Key::Menu => ("MENU", ""),
            Key::Back => ("BACK", ""),
            Key::TagTrack => ("TAG TRACK", "/REMOVE"),
            Key::TrackFilter => ("TRACK", "FILTER"),
            Key::Shortcut => ("SHORT", "CUT"),
            Key::Slip => ("SLIP", ""),
            Key::Quantize => ("QUANTIZE", ""),
            Key::Loop4 => ("4", ""),
            Key::Loop8 => ("8", ""),
            Key::JumpPrev | Key::CallPrev => ("◀", ""),
            Key::JumpNext | Key::CallNext => ("▶", ""),
            Key::TrackPrev => ("|◀◀", ""),
            Key::TrackNext => ("▶▶|", ""),
            Key::SearchPrev => ("◀◀", ""),
            Key::SearchNext => ("▶▶", ""),
            Key::JogMode => ("JOG", "MODE"),
            Key::BeatSync => ("BEAT", "SYNC"),
            Key::Master => ("MASTER", ""),
            Key::KeySync => ("KEY SYNC", ""),
            Key::CallDelete | Key::TimeMode | Key::LoopIn | Key::LoopOut | Key::Reloop | Key::Delete | Key::Memory
            | Key::TempoRange | Key::TempoReset => ("", ""),
        }
    }

    fn shape(self) -> Shape {
        match self {
            Key::TimeMode | Key::LoopIn | Key::LoopOut | Key::Reloop | Key::Loop4 | Key::Loop8
            | Key::TrackPrev | Key::TrackNext | Key::SearchPrev | Key::SearchNext
            | Key::CallPrev | Key::CallNext | Key::Delete | Key::Memory
            | Key::TempoRange | Key::TempoReset => Shape::Round,
            _ => Shape::Slab,
        }
    }

    /// Nothing behind it yet: drawn for the panel's sake, dimmed, and a tap
    /// does nothing.
    pub fn inert(self) -> bool {
        matches!(self, Key::Playlist | Key::TrackFilter | Key::Shortcut | Key::KeySync)
    }

    /// The button's name in cdj3k-emu's control API (`press <name>`).
    pub fn emu_name(self) -> &'static str {
        match self {
            Key::Source => "SOURCE", Key::Browse => "BROWSE", Key::TagList => "TAG_LIST",
            Key::Playlist => "PLAYLIST", Key::Search => "SEARCH_MENU", Key::Menu => "MENU",
            Key::Back => "BACK", Key::TagTrack => "TAG_TRACK", Key::TrackFilter => "TRACK_FILTER",
            Key::Shortcut => "SHORTCUT", Key::TimeMode => "TIME_MODE", Key::Slip => "SLIP",
            Key::Quantize => "QUANTIZE", Key::CallDelete => "CALL_DELETE", Key::LoopIn => "LOOP_IN",
            Key::LoopOut => "LOOP_OUT", Key::Reloop => "RELOOP", Key::Loop4 => "BEATLOOP_HALF",
            Key::Loop8 => "BEATLOOP_2X", Key::JumpPrev => "BEATJUMP_PREV", Key::JumpNext => "BEATJUMP_NEXT",
            Key::TrackPrev => "TRACK_PREV", Key::TrackNext => "TRACK_NEXT", Key::SearchPrev => "SEARCH_PREV",
            Key::SearchNext => "SEARCH_NEXT", Key::CallPrev => "CALL_PREV", Key::CallNext => "CALL_NEXT",
            Key::Delete => "DELETE", Key::Memory => "MEMORY", Key::JogMode => "JOG_MODE",
            Key::BeatSync => "BEAT_SYNC", Key::Master => "MASTER", Key::KeySync => "KEY_SYNC",
            Key::TempoRange => "TEMPO_RANGE", Key::TempoReset => "TEMPO_RESET",
        }
    }

    pub fn from_emu_name(name: &str) -> Option<Key> {
        let n = name.to_ascii_uppercase();
        Key::ALL.into_iter().find(|k| k.emu_name() == n)
    }

    /// What a short press sends. The loop keys depend on the deck: with no
    /// loop running, 4 and 8 start one of that length; with one running they
    /// halve and double it, the unit's 1/2X and 2X.
    /// `running_loop` is the length in beats of the loop playing, if any.
    pub fn tap(self, running_loop: Option<f32>) -> Option<Event> {
        let deck = |e| Some(Event::Deck(e));
        let ui = |e| Some(Event::Ui(e));
        let loop_by = |beats: f32, factor: f32| {
            let b = match running_loop { Some(l) if l > 0.0 => (l * factor).clamp(1.0 / 32.0, 512.0), _ => beats };
            deck(ControlEvent::BeatLoop { beats: b, held: false })
        };
        match self {
            Key::Source => ui(UiEvent::Screen(TopScreen::Source)),
            Key::Browse => ui(UiEvent::Screen(TopScreen::Browse)),
            Key::TagList => ui(UiEvent::Screen(TopScreen::TagList)),
            Key::Search => ui(UiEvent::Search(SearchEdit::Open)),
            // A tap opens UTILITY (the deck's settings, STEMS among them).
            // The unit has it the other way round (tap: LOAD PREVIOUS TRACK,
            // the manual p.24; hold: UTILITY), but on a touchscreen the
            // settings must be one tap away; LOAD PREVIOUS TRACK is the hold.
            Key::Menu => ui(UiEvent::Screen(TopScreen::Menu)),
            Key::Back => deck(ControlEvent::Back),
            Key::TagTrack => ui(UiEvent::TagTrack),
            Key::TimeMode => ui(UiEvent::TimeMode),
            Key::Slip => deck(ControlEvent::SlipToggle),
            Key::Quantize => ui(UiEvent::CycleSetting(Setting::Quantize)),
            Key::LoopIn => deck(ControlEvent::LoopIn),
            Key::LoopOut => deck(ControlEvent::LoopOut),
            Key::Reloop => deck(ControlEvent::Reloop),
            Key::Loop4 => loop_by(4.0, 0.5),
            Key::Loop8 => loop_by(8.0, 2.0),
            Key::JumpPrev => deck(ControlEvent::BeatJump { beats: -16.0 }),
            Key::JumpNext => deck(ControlEvent::BeatJump { beats: 16.0 }),
            Key::TrackPrev => ui(UiEvent::TrackSearch { next: false }),
            Key::TrackNext => ui(UiEvent::TrackSearch { next: true }),
            Key::CallPrev => deck(ControlEvent::MemoryCueCall { next: false }),
            Key::CallNext => deck(ControlEvent::MemoryCueCall { next: true }),
            Key::Delete => deck(ControlEvent::MemoryCueDelete),
            Key::Memory => deck(ControlEvent::MemoryCueSet),
            Key::JogMode => deck(ControlEvent::JogModeToggle),
            Key::BeatSync => deck(ControlEvent::SyncToggle),
            Key::Master => deck(ControlEvent::MasterRequest),
            Key::TempoRange => ui(UiEvent::CycleSetting(Setting::TempoRange)),
            // The fader lives on the screen, so TEMPO RESET puts it at 0 %.
            Key::TempoReset => deck(ControlEvent::TempoFader { position: 0.5 }),
            // Held, not tapped (CALL / DELETE is a modifier for the pads,
            // SEARCH scans while held), or inert.
            Key::CallDelete | Key::SearchPrev | Key::SearchNext
            | Key::Playlist | Key::TrackFilter | Key::Shortcut | Key::KeySync => None,
        }
    }

    /// What a long press sends instead of the tap, if the key has one.
    pub fn long(self) -> Option<Event> {
        match self {
            Key::Menu => Some(Event::Ui(UiEvent::Screen(TopScreen::History))),
            Key::TimeMode => Some(Event::Ui(UiEvent::AutoCue)),
            _ => None,
        }
    }

    /// SEARCH ◀◀ / ▶▶: while held (`held` seconds so far), one frame (`dt`
    /// seconds) of scanning, as an absolute position.
    pub fn scan(self, snap: &DeckSnapshot, dt: f32, held: f64) -> Option<Event> {
        let dir = match self { Key::SearchPrev => -1.0, Key::SearchNext => 1.0, _ => return None };
        if snap.total_samples == 0 || snap.sample_rate == 0 { return None; }
        let secs = snap.total_secs();
        let at = snap.position as f64 / snap.total_samples as f64 + dir * scan_speed(held) * dt as f64 / secs;
        Some(Event::Deck(ControlEvent::NeedleSearch { position: at.clamp(0.0, 1.0) as f32 }))
    }

    /// The lamp's colour.
    fn colour(self) -> Color32 {
        match self {
            Key::Browse | Key::TagList | Key::Playlist | Key::Search | Key::BeatSync | Key::KeySync => LIT_BLUE,
            Key::Slip | Key::Quantize => LIT_RED,
            Key::Master | Key::CallPrev | Key::CallNext => LIT_ORANGE,
            Key::LoopIn | Key::LoopOut | Key::Loop4 | Key::Loop8
            | Key::TrackPrev | Key::TrackNext | Key::SearchPrev | Key::SearchNext => LIT_YELLOW,
            Key::TempoReset => LIT_GREEN,
            _ => LIT_WHITE,
        }
    }

    /// The lamp now, by the unit's rules (read off cdj3k-emu's `lamps`): the
    /// screen keys glow dimly and light fully on their own screen; SLIP and
    /// QUANTIZE are dim when off; the loop and jump keys light once a track
    /// is loaded.
    pub fn lamp(self, snap: &DeckSnapshot, view: &ScreenView, sel_tagged: bool) -> Lamp {
        let level = |on: bool| if on { Lamp::Full } else { Lamp::Dim };
        let bit = |on: bool| if on { Lamp::Full } else { Lamp::Off };
        let loaded = snap.total_samples > 0;
        match self {
            Key::Source => level(matches!(view, ScreenView::Source(..))),
            Key::Browse => level(matches!(view, ScreenView::Browse(b) if !b.in_search())),
            Key::TagList => level(matches!(view, ScreenView::TagList)),
            Key::Playlist => Lamp::Dim,
            Key::Search => level(matches!(view, ScreenView::Browse(b) if b.in_search())),
            Key::Menu => level(matches!(view, ScreenView::Menu(..))),
            Key::TagTrack => bit(sel_tagged),
            Key::Slip => level(snap.slip),
            Key::Quantize => level(snap.quantize),
            Key::BeatSync => bit(snap.sync),
            Key::Master => bit(snap.master),
            Key::JogMode => bit(snap.jog_vinyl),
            // IN / OUT blink while a loop runs; RELOOP lights once one is stored.
            Key::LoopIn | Key::LoopOut if snap.loop_active => Lamp::Blink,
            Key::Reloop => bit(loaded && snap.loop_end > snap.loop_start),
            Key::LoopIn | Key::LoopOut | Key::Loop4 | Key::Loop8 | Key::JumpPrev | Key::JumpNext
            | Key::TrackPrev | Key::TrackNext => bit(loaded),
            Key::Back | Key::TrackFilter | Key::Shortcut | Key::TimeMode | Key::CallDelete
            | Key::SearchPrev | Key::SearchNext | Key::CallPrev | Key::CallNext | Key::Delete | Key::Memory
            | Key::KeySync | Key::TempoRange | Key::TempoReset => Lamp::Off,
        }
    }
}

/// How fast SEARCH runs, in track seconds per second held: the unit covers
/// about 21 s in a one-second hold, 34 s in 1.5 s and 58 s in 2 s.
pub fn scan_speed(held: f64) -> f64 { if held < 1.5 { 20.0 } else { 50.0 } }

/// Track seconds a SEARCH hold of `held` seconds covers.
pub fn scan_distance(held: f64) -> f64 { 20.0 * held.min(1.5) + 50.0 * (held - 1.5).max(0.0) }

/// The lamps by cdj3k-emu's names and in its values (`"medium"` / `"full"`
/// steps, `true` / `false`, hot cues as `[r, g, b]`), as one JSON object,
/// so a test can hold freedj's faceplate against the emulated unit's.
pub fn lamp_report(snap: &DeckSnapshot, view: &ScreenView, sel_tagged: bool) -> String {
    let step = |l: Lamp| match l { Lamp::Off => "\"off\"", Lamp::Dim => "\"medium\"", Lamp::Full | Lamp::Blink => "\"full\"" };
    // A blinking lamp is either state when the emulator reads it.
    let flag = |l: Lamp| match l { Lamp::Full => "true", Lamp::Blink => "\"blink\"", _ => "false" };
    let mut v: Vec<(String, String)> = Vec::new();
    for k in [Key::Source, Key::Browse, Key::TagList, Key::Playlist, Key::Menu, Key::Slip, Key::Quantize] {
        v.push((k.emu_name().into(), step(k.lamp(snap, view, sel_tagged)).into()));
    }
    v.push(("SEARCH".into(), step(Key::Search.lamp(snap, view, sel_tagged)).into()));
    for (name, k) in [("BEAT_SYNC", Key::BeatSync), ("MASTER", Key::Master), ("KEY_SYNC", Key::KeySync),
                      ("TEMPO_RESET", Key::TempoReset), ("LOOP_IN", Key::LoopIn), ("LOOP_OUT", Key::LoopOut),
                      ("RELOOP", Key::Reloop), ("BEATJUMP_4", Key::Loop4), ("BEATJUMP_8", Key::Loop8),
                      ("BEATJUMP_PREV", Key::JumpPrev), ("BEATJUMP_NEXT", Key::JumpNext), ("TRACK_SEARCH", Key::TrackPrev)] {
        v.push((name.into(), flag(k.lamp(snap, view, sel_tagged)).into()));
    }
    v.push(("MASTER_TEMPO".into(), snap.key_lock.to_string()));
    v.push(("JOG_MODE_VINYL".into(), snap.jog_vinyl.to_string()));
    v.push(("JOG_MODE_CDJ".into(), (!snap.jog_vinyl).to_string()));
    for (i, c) in snap.hot_cues.iter().enumerate() {
        let rgb = if c.is_some() { "[0,127,0]" } else { "[6,12,12]" };
        v.push((format!("HOT_{}", (b'A' + i as u8) as char), rgb.into()));
    }
    let body: Vec<String> = v.into_iter().map(|(k, val)| format!("\"{k}\":{val}")).collect();
    format!("{{{}}}", body.join(","))
}

/// Everything around the screen.
pub struct CdjFace {
    /// The drawn body (letterboxed in the window).
    pub base: Rect,
    /// The shared XDJ chrome (jog, fader, PLAY / CUE, MASTER TEMPO); `None`
    /// in the side layout.
    pub deck: Option<FaceLayout>,
    pub keys: Vec<(Rect, Key)>,
    /// The rotary selector: turn = scroll, push = LOAD.
    pub rotary: Rect,
    /// Hot cues A–H, under the screen (deck layout only).
    pub pads: Option<[Rect; 8]>,
    /// Words printed on the panel: (anchor, alignment, text).
    pub captions: Vec<(Pos2, Align2, &'static str)>,
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

    // The screen: the glass's width, 16:9 in real pixels.
    let screen = Rect::from_min_size(at(250.0, 140.0), Vec2::new(755.0 * sx, 755.0 * sx * 9.0 / 16.0));

    let mut keys = Vec::new();
    // Above the screen.
    for (i, k) in [Key::Source, Key::Browse, Key::TagList, Key::Playlist, Key::Search, Key::Menu].into_iter().enumerate() {
        let x = 233.0 + i as f32 * 136.0;
        keys.push((r(x, 75.0, x + 108.0, 102.0), k));
    }
    // Around the rotary.
    keys.push((r(1062.0, 195.0, 1112.0, 235.0), Key::Back));
    keys.push((r(1117.0, 195.0, 1177.0, 235.0), Key::TagTrack));
    keys.push((r(1062.0, 430.0, 1112.0, 475.0), Key::TrackFilter));
    keys.push((r(1117.0, 430.0, 1177.0, 475.0), Key::Shortcut));
    let rotary = disk(1120.0, 332.0, 62.0);

    // The left column.
    keys.push((disk(97.0, 445.0, 13.0), Key::TimeMode));
    keys.push((r(32.0, 507.0, 86.0, 532.0), Key::Slip));
    keys.push((r(108.0, 507.0, 162.0, 532.0), Key::Quantize));
    keys.push((r(35.0, 620.0, 115.0, 660.0), Key::CallDelete));
    keys.push((disk(67.0, 733.0, 30.0), Key::LoopIn));
    keys.push((disk(165.0, 733.0, 30.0), Key::LoopOut));
    keys.push((disk(281.0, 733.0, 24.0), Key::Reloop));
    keys.push((disk(68.0, 833.0, 22.0), Key::Loop4));
    keys.push((disk(165.0, 833.0, 22.0), Key::Loop8));
    keys.push((r(38.0, 900.0, 93.0, 948.0), Key::JumpPrev));
    keys.push((r(138.0, 900.0, 194.0, 948.0), Key::JumpNext));
    keys.push((disk(67.0, 1193.0, 28.0), Key::TrackPrev));
    keys.push((disk(143.0, 1193.0, 28.0), Key::TrackNext));
    keys.push((disk(67.0, 1295.0, 28.0), Key::SearchPrev));
    keys.push((disk(143.0, 1295.0, 28.0), Key::SearchNext));

    // Over the jog.
    keys.push((disk(802.0, 733.0, 15.0), Key::CallPrev));
    keys.push((disk(869.0, 733.0, 15.0), Key::CallNext));
    keys.push((disk(952.0, 733.0, 25.0), Key::Delete));
    keys.push((disk(1020.0, 733.0, 13.0), Key::Memory));

    // The right column.
    keys.push((r(1158.0, 710.0, 1215.0, 760.0), Key::JogMode));
    keys.push((r(1082.0, 818.0, 1140.0, 866.0), Key::BeatSync));
    keys.push((r(1157.0, 818.0, 1215.0, 866.0), Key::Master));
    keys.push((r(1108.0, 889.0, 1190.0, 920.0), Key::KeySync));
    keys.push((disk(1148.0, 1030.0, 18.0), Key::TempoRange));
    keys.push((disk(1030.0, 1407.0, 22.0), Key::TempoReset));

    // The hot cues, under the screen.
    let pads = std::array::from_fn(|i| {
        let x = 193.0 + i as f32 * 113.0;
        r(x, 618.0, x + 76.0, 662.0)
    });

    let c = |x: f32, y: f32, a: Align2, s: &'static str| (at(x, y), a, s);
    let mid = Align2::CENTER_CENTER;
    let captions = vec![
        c(52.0, 440.0, mid, "TIME"), c(52.0, 454.0, mid, "MODE"),
        c(140.0, 440.0, mid, "AUTO"), c(140.0, 454.0, mid, "CUE"),
        c(30.0, 601.0, Align2::LEFT_CENTER, "• CALL / DELETE"),
        c(627.0, 600.0, mid, "HOT CUE"),
        c(67.0, 689.0, mid, "IN/CUE"), c(165.0, 689.0, mid, "OUT"),
        c(281.0, 689.0, mid, "RELOOP/EXIT"), c(395.0, 733.0, mid, "LOOP"),
        c(67.0, 781.0, mid, "IN ADJUST"), c(165.0, 781.0, mid, "OUT ADJUST"),
        c(68.0, 871.0, mid, "1/2X"), c(165.0, 871.0, mid, "2X"),
        c(277.0, 832.0, mid, "BEAT LOOP"),
        c(115.0, 916.0, mid, "BEAT"), c(115.0, 931.0, mid, "JUMP"),
        c(105.0, 1145.0, mid, "TRACK SEARCH"), c(105.0, 1247.0, mid, "SEARCH"),
        c(835.0, 687.0, mid, "CUE/LOOP"), c(835.0, 702.0, mid, "CALL"),
        c(952.0, 687.0, mid, "DELETE"), c(1023.0, 687.0, mid, "MEMORY"),
        c(1145.0, 794.0, mid, "INST. DOUBLES"),
        c(1148.0, 987.0, mid, "TEMPO"), c(1148.0, 1001.0, mid, "±6/±10/±16/WIDE"),
        c(1030.0, 1445.0, mid, "TEMPO"), c(1030.0, 1459.0, mid, "RESET"),
        c(1148.0, 1620.0, mid, "TEMPO"),
    ];

    let deck = FaceLayout {
        base,
        jog:      disk(627.0, 1176.0, 400.0),
        fader:    r(1120.0, 1195.0, 1178.0, 1600.0),
        cue:      disk(105.0, 1440.0, 64.0),
        play:     disk(105.0, 1616.0, 64.0),
        loop_in:  None,
        loop_out: None,
        reloop:   None,
        browse:   None,
        mt:       disk(1148.0, 1120.0, 18.0),
        time_mode: None,
        auto_cue:  None,
        tag_track: None,
        back:      None,
        jog_mode:  None,
        captions_above: true,
        caption:   13.0 * sx,
    };
    (screen, CdjFace { base, deck: Some(deck), keys, rotary, pads: Some(pads), captions, text: 15.0 * sx })
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
    (screen, CdjFace { base, deck: None, keys, rotary, pads: None, captions: Vec::new(), text: 19.0 * u })
}

/// Where a key takes touches: its own rect, grown for the small round keys
/// (`sx` = points per reference px).
fn hit_rect(r: Rect, k: Key, sx: f32) -> Rect {
    let min = MIN_HIT_R * sx;
    if k.shape() != Shape::Round || r.width() * 0.5 >= min { r }
    else { Rect::from_center_size(r.center(), Vec2::splat(2.0 * min)) }
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
    let t = f.text;
    for (pos, a, s) in &f.captions {
        p.text(*pos, *a, *s, FontId::proportional(t * 0.75), CAPTION);
    }
    if let Some(deck) = &f.deck {
        screen::draw_faceplate(ui, ctx, snap, deck, false, chrome, out);
        // VINYL / CDJ beside JOG MODE: which one the jog is in.
        if let Some((jm, _)) = f.keys.iter().find(|(_, k)| *k == Key::JogMode) {
            let x = jm.min.x - jm.width() * 0.75;
            let (v, c) = if snap.jog_vinyl { (LIT_BLUE, INERT) } else { (INERT, LIT_WHITE) };
            p.text(Pos2::new(x, jm.center().y - t * 0.6), Align2::CENTER_CENTER, "VINYL", FontId::proportional(t * 0.85), v);
            p.text(Pos2::new(x, jm.center().y + t * 0.6), Align2::CENTER_CENTER, "CDJ", FontId::proportional(t * 0.85), c);
        }
    }

    let sx = f.base.width() / DECK_ASPECT.x;
    let call_delete_held = draw_keys(ui, ctx, snap, &f.keys, |k| k.lamp(snap, view, sel_tagged), sx, t, out);

    // Rotary selector: the XDJ's browse knob sprite and behaviour.
    crate::chrome::paint(p, ctx, chrome, Sprite::Round(RoundKind::Knob, None), f.rotary);
    p.text(Pos2::new(f.rotary.center().x, f.rotary.max.y + t * 0.5), Align2::CENTER_TOP, "PUSH: LOAD",
           FontId::proportional(t * 0.75), CAPTION);
    screen::browse_knob(ui, f.rotary, out);

    if let Some(pads) = &f.pads {
        let cd = f.keys.iter().find(|(_, k)| *k == Key::CallDelete).map(|(r, _)| *r);
        draw_pads(ui, snap, pads, cd, call_delete_held, t, out);
    }
}

/// When a key's press began, and whether its long press has fired.
#[derive(Clone, Copy, Default)]
struct Press { since: f64, long_fired: bool }

/// The keys: a slab or a round cap, glowing at its lamp's level. Returns
/// whether CALL / DELETE is held (by the first finger).
#[allow(clippy::too_many_arguments)]
fn draw_keys(ui: &Ui, ctx: &egui::Context, snap: &DeckSnapshot, keys: &[(Rect, Key)], lamp_of: impl Fn(Key) -> Lamp,
             sx: f32, t: f32, out: &mut Vec<Event>) -> bool {
    let p = ui.painter();
    let now = ui.input(|i| i.time);
    let dt = ui.input(|i| i.stable_dt).min(0.1);
    let mut call_delete_held = false;
    for (r, k) in keys.iter() {
        let id = Id::new(("cdjface-key", k.emu_name()));
        let hit = hit_rect(*r, *k, sx);
        // Press / release (Sense::drag): a still finger counts at once.
        let resp = ui.interact(hit, id, Sense::drag());
        let down = resp.is_pointer_button_down_on();
        let lamp = match lamp_of(*k) {
            Lamp::Blink => { ctx.request_repaint_after(std::time::Duration::from_millis(100)); if (now * 2.0) as i64 % 2 == 0 { Lamp::Full } else { Lamp::Off } }
            l => l,
        };
        let col = k.colour();
        let glow = |a: u8| Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a);
        let face = if down { KEY_DOWN } else { KEY_FACE };
        match k.shape() {
            Shape::Slab => {
                let round = r.height() * 0.12;
                p.rect_filled(*r, round, face);
                p.rect_stroke(*r, round, Stroke::new(1.0, KEY_EDGE));
                if matches!(lamp, Lamp::Full | Lamp::Blink) {
                    p.rect_filled(r.shrink(1.5), round, glow(90));
                    p.rect_stroke(r.expand(1.0), round, Stroke::new(2.0, col));
                }
            }
            Shape::Round => {
                let (c, rad) = (r.center(), r.width() * 0.5);
                p.circle_filled(c, rad, face);
                p.circle_stroke(c, rad, Stroke::new(1.0, KEY_EDGE));
                match lamp {
                    Lamp::Full | Lamp::Blink => {
                        p.circle_filled(c, rad - 1.5, glow(110));
                        p.circle_stroke(c, rad + 1.0, Stroke::new(2.0, col));
                    }
                    Lamp::Dim => { p.circle_stroke(c, rad + 0.5, Stroke::new(1.5, glow(120))); }
                    Lamp::Off => {}
                }
            }
        }
        let (main, sub) = k.legend();
        let legend = if k.inert() { INERT } else {
            match lamp { Lamp::Full | Lamp::Blink => LIT_WHITE, Lamp::Dim => glow(210), Lamp::Off => LEGEND }
        };
        if !main.is_empty() {
            let size = fit(main, t, r.width());
            if sub.is_empty() {
                p.text(r.center(), Align2::CENTER_CENTER, main, FontId::proportional(size), legend);
            } else {
                let sub_size = fit(sub, t * 0.8, r.width());
                p.text(Pos2::new(r.center().x, r.center().y - size * 0.45), Align2::CENTER_CENTER, main,
                       FontId::proportional(size), legend);
                p.text(Pos2::new(r.center().x, r.center().y + size * 0.6), Align2::CENTER_CENTER, sub,
                       FontId::proportional(sub_size), legend);
            }
        }

        if k.inert() { continue; }
        if *k == Key::CallDelete { call_delete_held = down; continue; }
        // A tap acts on the release. A key with a long press acts at the
        // threshold instead, and then not again on the release.
        let pid = id.with("press");
        let tap = screen::same_frame_tap(ui, hit);
        if resp.drag_started() { ctx.data_mut(|d| d.insert_temp(pid, Press { since: now, long_fired: false })); }
        let mut st: Press = ctx.data(|d| d.get_temp(pid)).unwrap_or_default();
        if down { if let Some(e) = k.scan(snap, dt, now - st.since) { out.push(e); ctx.request_repaint(); } }
        if down && !st.long_fired && now - st.since >= LONG_PRESS_S {
            if let Some(e) = k.long() {
                st.long_fired = true;
                ctx.data_mut(|d| d.insert_temp(pid, st));
                out.push(e);
            }
        }
        if (resp.drag_stopped() && !st.long_fired) || tap {
            if let Some(e) = k.tap(snap.loop_active.then_some(snap.loop_beats)) {
                // The unit's 4 / 8 keys bring up the BEAT LOOP panel too.
                if matches!(k, Key::Loop4 | Key::Loop8) { crate::cdj3000::open_panel(ctx, crate::cdj3000::Panel::BeatLoop); }
                out.push(e);
            }
        }
        if down && k.long().is_some() { ctx.request_repaint(); }   // so the long press fires on time
    }
    call_delete_held
}

/// Hot cues A–H: an empty pad sets one at the playhead; a set pad plays from
/// it while held (and, with SLIP, returns on release). With CALL / DELETE
/// held, a set pad is deleted instead, as on the unit. A second finger can
/// press a pad or hold CALL / DELETE.
fn draw_pads(ui: &Ui, snap: &DeckSnapshot, pads: &[Rect; 8], call_delete: Option<Rect>, call_delete_held: bool,
             t: f32, out: &mut Vec<Event>) {
    let p = ui.painter();
    let mut rects: Vec<(Rect, bool)> = pads.iter().map(|r| (*r, true)).collect();
    if let Some(cd) = call_delete { rects.push((cd, true)); }
    let sec = screen::second_touches_in(ui, "cdj-pads", &rects);
    let deleting = call_delete_held || sec.get(8).is_some_and(|s| s.down);
    for (i, r) in pads.iter().enumerate() {
        let slot = i as u8;
        // Press / release, not click: Sense::drag reports a still finger at once.
        let resp = ui.interact(*r, Id::new(("cdjface-pad", i)), Sense::drag());
        let tap = screen::same_frame_tap(ui, *r);
        let pressed = resp.drag_started() || tap || sec[i].pressed;
        let released = resp.drag_stopped() || tap || sec[i].released;
        let down = resp.is_pointer_button_down_on() || sec[i].down;
        let set = snap.hot_cues[i];
        let round = r.height() * 0.14;
        p.rect_filled(*r, round, if down { KEY_DOWN } else { KEY_FACE });
        p.rect_stroke(*r, round, Stroke::new(1.0, KEY_EDGE));
        // The letter top-left and the lamp bar along the bottom, as on the unit.
        let bar = Rect::from_min_max(Pos2::new(r.min.x + r.width() * 0.08, r.max.y - r.height() * 0.2),
                                     Pos2::new(r.max.x - r.width() * 0.08, r.max.y - r.height() * 0.12));
        p.rect_filled(bar, 1.0, if set.is_some() { PAD_SET } else { PAD_EMPTY });
        if set.is_some() && down { p.rect_filled(r.shrink(1.5), round, Color32::from_rgba_unmultiplied(0, 200, 60, 70)); }
        let letter = ((b'A' + slot) as char).to_string();
        p.text(Pos2::new(r.min.x + r.width() * 0.1, r.min.y + r.height() * 0.1), Align2::LEFT_TOP, letter,
               FontId::proportional(t * 0.9), if set.is_some() { PAD_SET } else { LEGEND });
        if pressed {
            out.push(Event::Deck(match (set, deleting) {
                (Some(_), true) => ControlEvent::HotCueDelete { slot },
                (Some(_), false) => ControlEvent::HotCueTrigger { slot, held: true },
                (None, _) => ControlEvent::HotCueSet { slot },
            }));
        }
        if released && set.is_some() && !deleting { out.push(Event::Deck(ControlEvent::HotCueRelease { slot })); }
    }
}

/// A legend size that fits `s` inside `width` (a rough per-character width).
fn fit(s: &str, size: f32, width: f32) -> f32 {
    let w = s.chars().count() as f32 * size * 0.62;
    if w > width * 0.9 { size * width * 0.9 / w } else { size }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Overlap of two controls, a round one taken as its circle.
    fn overlaps((a, ra): (Rect, bool), (b, rb): (Rect, bool)) -> bool {
        let circle_rect = |c: Rect, r: Rect| {
            let p = c.center().clamp(r.min, r.max);
            p.distance(c.center()) < c.width() * 0.5 - 0.5
        };
        match (ra, rb) {
            (true, true) => a.center().distance(b.center()) < (a.width() + b.width()) * 0.5 - 0.5,
            (true, false) => circle_rect(a, b),
            (false, true) => circle_rect(b, a),
            (false, false) => a.intersects(b) && a.intersect(b).area() > 0.5,
        }
    }

    /// Every control lies inside the body and none overlaps another or the
    /// screen (touch areas included), in both layouts and at the iPad's
    /// aspect as well as the unit's.
    #[test]
    fn controls_stay_in_the_body_and_apart() {
        let bases = [
            ("deck", Rect::from_min_size(Pos2::ZERO, DECK_ASPECT), true),
            ("ipad", Rect::from_min_size(Pos2::ZERO, Vec2::new(1032.0, 1376.0)), true),
            ("side", Rect::from_min_size(Pos2::ZERO, SIDE_ASPECT), false),
        ];
        for (name, base, deck) in bases {
            let (screen, f) = if deck { deck_layout(base) } else { side_layout(base) };
            let sx = base.width() / DECK_ASPECT.x;
            let mut all: Vec<(String, (Rect, bool))> = vec![("screen".into(), (screen, false)), ("rotary".into(), (f.rotary, true))];
            for (r, k) in &f.keys {
                all.push((format!("{k:?}"), (if deck { hit_rect(*r, *k, sx) } else { *r }, k.shape() == Shape::Round)));
            }
            if let Some(pads) = f.pads { for (i, r) in pads.iter().enumerate() { all.push((format!("pad{i}"), (*r, false))); } }
            if let Some(d) = &f.deck {
                all.extend([("jog".into(), (d.jog, true)), ("fader".into(), (d.fader, false)), ("cue".into(), (d.cue, true)),
                            ("play".into(), (d.play, true)), ("mt".into(), (d.mt, true))]);
            }
            for (n, (r, _)) in &all {
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

    /// The deck layout has every key the unit has, once; the side layout the
    /// ones the screen has no key for.
    #[test]
    fn every_key_is_on_the_panel() {
        let (_, f) = deck_layout(Rect::from_min_size(Pos2::ZERO, DECK_ASPECT));
        let have: Vec<Key> = f.keys.iter().map(|(_, k)| *k).collect();
        for k in Key::ALL { assert!(have.contains(&k), "{k:?} missing from the deck layout"); }
        assert_eq!(have.len(), Key::ALL.len(), "a key is placed twice");
        let (_, f) = side_layout(Rect::from_min_size(Pos2::ZERO, SIDE_ASPECT));
        let have: Vec<Key> = f.keys.iter().map(|(_, k)| *k).collect();
        for k in [Key::Source, Key::Browse, Key::TagList, Key::Search, Key::Menu, Key::Back, Key::TagTrack] {
            assert!(have.contains(&k), "{k:?} missing from the side layout");
        }
    }

    #[test]
    fn emulator_names_round_trip() {
        for k in Key::ALL { assert_eq!(Key::from_emu_name(k.emu_name()), Some(k)); }
        assert_eq!(Key::from_emu_name("hot_a"), None);
    }

    /// 4 and 8 start a loop of that length, or halve / double a running one;
    /// MENU and TIME MODE have their long presses; the inert keys send nothing.
    #[test]
    fn key_events_follow_the_unit() {
        let beats = |e: Option<Event>| match e { Some(Event::Deck(ControlEvent::BeatLoop { beats, .. })) => beats, e => panic!("{e:?}") };
        assert_eq!(beats(Key::Loop4.tap(None)), 4.0);
        assert_eq!(beats(Key::Loop8.tap(None)), 8.0);
        assert_eq!(beats(Key::Loop4.tap(Some(4.0))), 2.0);
        assert_eq!(beats(Key::Loop8.tap(Some(4.0))), 8.0);
        assert_eq!(beats(Key::Loop8.tap(Some(16.0))), 32.0);
        assert_eq!(beats(Key::Loop4.tap(Some(0.0))), 4.0);   // a manual loop of unknown length
        assert!(matches!(Key::Menu.long(), Some(Event::Ui(UiEvent::Screen(TopScreen::History)))));
        assert!(matches!(Key::Menu.tap(None), Some(Event::Ui(UiEvent::Screen(TopScreen::Menu)))));
        assert!(matches!(Key::TimeMode.long(), Some(Event::Ui(UiEvent::AutoCue))));
        assert!(matches!(Key::Quantize.tap(None), Some(Event::Ui(UiEvent::CycleSetting(Setting::Quantize)))));
        for k in [Key::Playlist, Key::TrackFilter, Key::Shortcut, Key::KeySync] { assert!(k.tap(None).is_none()); }
    }

    /// The lamps by the emulator's rules: screen keys dim until their screen,
    /// SLIP / QUANTIZE dim when off, set hot cues green.
    #[test]
    fn lamps_follow_the_unit() {
        let tags = opendeck_decode::TrackTags::default();
        let mut snap = DeckSnapshot::test(&tags);
        snap.quantize = true;
        snap.hot_cues[0] = Some(1000);
        let v = lamp_report(&snap, &ScreenView::Playback, false);
        for want in ["\"SOURCE\":\"medium\"", "\"MENU\":\"medium\"", "\"QUANTIZE\":\"full\"", "\"SLIP\":\"medium\"",
                     "\"HOT_A\":[0,127,0]", "\"HOT_B\":[6,12,12]", "\"BEAT_SYNC\":false"] {
            assert!(v.contains(want), "{want} not in {v}");
        }
        assert_eq!(Key::TagList.lamp(&snap, &ScreenView::TagList, false), Lamp::Full);
        assert_eq!(Key::Browse.lamp(&snap, &ScreenView::TagList, false), Lamp::Dim);
    }

    /// SEARCH covers what the unit does: ~21 s in a 1 s hold, ~58 s in 2 s.
    #[test]
    fn search_accelerates_like_the_unit() {
        assert!((scan_distance(1.0) - 20.0).abs() < 2.0);
        assert!((scan_distance(1.5) - 30.0).abs() < 5.0);
        assert!((scan_distance(2.0) - 55.0).abs() < 5.0);
        assert!(scan_speed(2.0) > scan_speed(0.5));
    }

    /// IN / OUT blink while a loop runs; RELOOP lights once one is stored.
    #[test]
    fn loop_lamps_follow_the_unit() {
        let tags = opendeck_decode::TrackTags::default();
        let mut snap = DeckSnapshot::test(&tags);
        snap.total_samples = 1_000_000;
        let v = ScreenView::Playback;
        assert_eq!(Key::Reloop.lamp(&snap, &v, false), Lamp::Off);
        assert_eq!(Key::LoopIn.lamp(&snap, &v, false), Lamp::Full);
        snap.loop_active = true;
        snap.loop_start = 1000;
        snap.loop_end = 5000;
        assert_eq!(Key::LoopIn.lamp(&snap, &v, false), Lamp::Blink);
        assert_eq!(Key::Reloop.lamp(&snap, &v, false), Lamp::Full);
        snap.loop_active = false;
        assert_eq!(Key::LoopOut.lamp(&snap, &v, false), Lamp::Full);
        assert_eq!(Key::Reloop.lamp(&snap, &v, false), Lamp::Full, "a stored loop keeps RELOOP lit");
        assert!(lamp_report(&snap, &v, false).contains("\"RELOOP\":true"));
    }

    /// A pointer press and release on MENU in the side layout sends
    /// Screen(Menu), and on BACK sends Back: the keys work, not just draw.
    #[test]
    fn clicking_menu_and_back_sends_their_events() {
        use egui::{Event as E, PointerButton, RawInput};
        let base = Rect::from_min_size(Pos2::ZERO, SIDE_ASPECT * 0.5);
        let (_, f) = side_layout(base);
        let tags = opendeck_decode::TrackTags::default();
        let snap = DeckSnapshot::test(&tags);
        let ctx = egui::Context::default();
        let screen_rect = Some(Rect::from_min_size(Pos2::ZERO, base.size()));
        let frame = |events: Vec<E>| {
            let mut out = Vec::new();
            let _ = ctx.run(RawInput { events, screen_rect, ..Default::default() }, |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::none())
                    .show(ctx, |ui| draw_keys(ui, ctx, &snap, &f.keys, |_| Lamp::Off, 1.0, f.text, &mut out));
            });
            out
        };
        let click = |k: Key| {
            let at = f.keys.iter().find(|(_, kk)| *kk == k).unwrap().0.center();
            let btn = |pressed| E::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            frame(vec![E::PointerMoved(at)]);
            let mut out = frame(vec![btn(true)]);
            out.extend(frame(vec![btn(false)]));
            out
        };
        let out = click(Key::Menu);
        assert!(matches!(out.as_slice(), [Event::Ui(UiEvent::Screen(TopScreen::Menu))]), "{out:?}");
        let out = click(Key::Back);
        assert!(matches!(out.as_slice(), [Event::Deck(ControlEvent::Back)]), "{out:?}");
    }
}
