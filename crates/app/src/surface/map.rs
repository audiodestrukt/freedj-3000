//! Controls → bus events, independent of how the control arrived.
//!
//! A transport (MIDI, I2C) reduces its traffic to four verbs on a
//! [`Key`] — press, release, delta, absolute — and the [`Mapper`] turns them
//! into [`Event`]s from the surface's bindings.  It holds the little state a
//! surface needs and the deck must not: the SHIFT layer, which binding a held
//! button was pressed under, the MSB of a 14-bit fader.  Nothing here touches
//! deck state; `DeckApp::apply` does that.

use super::config::{self, Action, Deck, EncoderMode, Surface};
use crate::input::{self, ControlEvent, Event, UiEvent};

/// A control's address on its transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// MIDI note number + channel.
    Note(u8, u8),
    /// MIDI CC number + channel.
    Cc(u8, u8),
    /// Expander pin.
    Pin(u8),
    /// Quadrature pair.
    Pins(u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MidiSrc { Note(u8), NoteOff(u8), Cc(u8) }

#[derive(Debug, Clone)]
struct ButtonBind {
    midi: Option<(MidiSrc, Option<u8>)>,
    pin: Option<u8>,
    action: Action,
    shift: bool,
}

#[derive(Debug, Clone)]
struct EncoderBind {
    cc: Option<(u8, Option<u8>)>,
    mode: EncoderMode,
    pins: Option<[u8; 2]>,
    reverse: bool,
    scale: f32,
    action: Action,
    shift: bool,
}

#[derive(Debug, Clone)]
struct FaderBind {
    cc: u8,
    cc_lsb: Option<u8>,
    channel: Option<u8>,
    invert: bool,
    center_detent: bool,
    action: Action,
    /// Last MSB seen, for 14-bit pairs.
    msb: u8,
}

/// Bindings for one surface, filtered to the deck this instance runs as.
pub struct Mapper {
    buttons:  Vec<ButtonBind>,
    encoders: Vec<EncoderBind>,
    faders:   Vec<FaderBind>,
    shift: bool,
    /// Buttons currently down: which bindings fired on their press, so the
    /// release goes to the same ones even if SHIFT changed in between.
    held: Vec<(Key, Vec<usize>)>,
}

fn deck_ok(d: Option<Deck>, mine: Deck) -> bool { d.is_none_or(|d| d == mine) }

impl Mapper {
    pub fn new(s: &Surface, deck: Deck) -> Self {
        let buttons = s.button.iter().filter(|b| deck_ok(b.deck, deck)).map(|b| ButtonBind {
            midi: b.note.map(MidiSrc::Note).or(b.note_off.map(MidiSrc::NoteOff)).or(b.cc.map(MidiSrc::Cc)).map(|m| (m, b.channel)),
            pin: b.pin,
            action: b.action().expect("validated at parse"),
            shift: b.shift,
        }).collect();
        let encoders = s.encoder.iter().filter(|e| deck_ok(e.deck, deck)).map(|e| EncoderBind {
            cc: e.cc.map(|c| (c, e.channel)), mode: e.cc_mode, pins: e.pins, reverse: e.reverse, scale: e.scale,
            action: e.action().expect("validated at parse"), shift: e.shift,
        }).collect();
        let faders = s.fader.iter().filter(|f| deck_ok(f.deck, deck)).filter_map(|f| Some(FaderBind {
            cc: f.cc?, cc_lsb: f.cc_lsb, channel: f.channel, invert: f.invert, center_detent: f.center_detent,
            action: f.action().expect("validated at parse"), msb: 0,
        })).collect();
        Mapper { buttons, encoders, faders, shift: false, held: Vec::new() }
    }

    pub fn shift_held(&self) -> bool { self.shift }

    // ── the four verbs ───────────────────────────────────────────────────────

    /// A button went down.  `as_note_off`: this is a MIDI Note Off, which is
    /// the *press* of a `note_off` binding (and the release of a `note` one —
    /// the transport calls [`Mapper::release`] for that side itself).
    pub fn press(&mut self, key: Key, as_note_off: bool) -> Vec<Event> {
        let idx = self.pick_buttons(key, as_note_off);
        let mut out = Vec::new();
        for &i in &idx {
            let action = self.buttons[i].action.clone();
            if action == Action::Shift { self.shift = true; }
            out.extend(press_events(&action));
        }
        if !idx.is_empty() && !as_note_off {
            self.held.retain(|(k, _)| *k != key);
            self.held.push((key, idx));
        }
        out
    }

    pub fn release(&mut self, key: Key) -> Vec<Event> {
        let Some(pos) = self.held.iter().position(|(k, _)| *k == key) else { return Vec::new() };
        let (_, idx) = self.held.remove(pos);
        let mut out = Vec::new();
        for i in idx {
            let action = self.buttons[i].action.clone();
            if action == Action::Shift { self.shift = false; }
            out.extend(release_events(&action));
        }
        out
    }

    /// An encoder moved by `steps` detents (sign = direction, before `reverse`).
    pub fn delta(&mut self, key: Key, steps: i32) -> Vec<Event> {
        if steps == 0 { return Vec::new(); }
        let shift = self.shift;
        let cands: Vec<usize> = self.encoders.iter().enumerate().filter(|(_, e)| e.matches(key)).map(|(i, _)| i).collect();
        let chosen = layer(&cands, |i| self.encoders[i].shift, shift);
        let mut out = Vec::new();
        for i in chosen {
            let e = &self.encoders[i];
            let d = if e.reverse { -steps } else { steps };
            out.extend(delta_events(&e.action, d, e.scale));
        }
        out
    }

    /// An absolute control sits at `v` ∈ 0..=1.
    pub fn absolute(&mut self, key: Key, v: f32) -> Vec<Event> {
        let mut out = Vec::new();
        for f in &self.faders {
            if !f.matches(key) { continue; }
            let p = if f.invert { 1.0 - v } else { v };
            out.extend(abs_events(&f.action, p.clamp(0.0, 1.0)));
        }
        out
    }

    // ── MIDI conveniences (CC dispatch depends on what the CC is bound as) ───

    pub fn note_on(&mut self, ch: u8, note: u8, vel: u8) -> Vec<Event> {
        let key = Key::Note(note, ch);
        if vel == 0 { self.note_off(ch, note) } else { self.press(key, false) }
    }

    pub fn note_off(&mut self, ch: u8, note: u8) -> Vec<Event> {
        let key = Key::Note(note, ch);
        let mut out = self.release(key);
        out.extend(self.press(key, true));
        out
    }

    pub fn cc(&mut self, ch: u8, cc: u8, value: u8) -> Vec<Event> {
        let key = Key::Cc(cc, ch);
        let mut out = Vec::new();
        // Buttons on a CC: ≥ 64 is down.
        if self.buttons.iter().any(|b| b.matches(key, false)) {
            if value >= 64 { out.extend(self.press(key, false)); } else { out.extend(self.release(key)); }
        }
        // Relative encoders.
        if let Some(mode) = self.encoders.iter().find(|e| e.matches(key)).map(|e| e.mode) {
            let steps = match mode {
                EncoderMode::TwosComplement => if value < 64 { value as i32 } else { value as i32 - 128 },
                EncoderMode::Offset64       => value as i32 - 64,
                EncoderMode::SignBit        => if value & 0x40 != 0 { -((value & 0x3F) as i32) } else { value as i32 },
            };
            out.extend(self.delta(key, steps));
        }
        // Absolute faders, 7- or 14-bit.
        let mut fire: Vec<(Key, f32)> = Vec::new();
        for f in &mut self.faders {
            if f.matches(key) {
                f.msb = value;
                if f.cc_lsb.is_none() {
                    let v = if f.center_detent && (value == 63 || value == 64) { 0.5 } else { value as f32 / 127.0 };
                    fire.push((key, v));
                }
            } else if f.matches_lsb(key) {
                let raw = ((f.msb as u32) << 7) | value as u32;
                let v = if f.center_detent && (8191..=8192).contains(&raw) { 0.5 } else { raw as f32 / 16383.0 };
                fire.push((Key::Cc(f.cc, ch), v));
            }
        }
        for (k, v) in fire { out.extend(self.absolute(k, v)); }
        out
    }

    fn pick_buttons(&self, key: Key, as_note_off: bool) -> Vec<usize> {
        let cands: Vec<usize> = self.buttons.iter().enumerate()
            .filter(|(_, b)| b.matches(key, as_note_off)).map(|(i, _)| i).collect();
        layer(&cands, |i| self.buttons[i].shift, self.shift)
    }
}

/// Shift layering: while SHIFT is held, bindings marked `shift` win and
/// shadow the plain ones for the same control; otherwise plain bindings fire.
/// A control with only plain bindings keeps working under SHIFT — a board
/// that has SHIFT does not lose its unshifted buttons.
fn layer(cands: &[usize], is_shift: impl Fn(usize) -> bool, shift: bool) -> Vec<usize> {
    if shift && cands.iter().any(|&i| is_shift(i)) {
        cands.iter().copied().filter(|&i| is_shift(i)).collect()
    } else {
        cands.iter().copied().filter(|&i| !is_shift(i)).collect()
    }
}

impl ButtonBind {
    fn matches(&self, key: Key, as_note_off: bool) -> bool {
        match (key, self.midi, self.pin) {
            (Key::Pin(p), _, Some(q)) => p == q,
            (Key::Note(n, ch), Some((MidiSrc::Note(m), c)), _) => !as_note_off && n == m && c.is_none_or(|c| c == ch),
            (Key::Note(n, ch), Some((MidiSrc::NoteOff(m), c)), _) => as_note_off && n == m && c.is_none_or(|c| c == ch),
            (Key::Cc(n, ch), Some((MidiSrc::Cc(m), c)), _) => n == m && c.is_none_or(|c| c == ch),
            _ => false,
        }
    }
}

impl EncoderBind {
    fn matches(&self, key: Key) -> bool {
        match (key, self.cc, self.pins) {
            (Key::Pins(a, b), _, Some([x, y])) => a == x && b == y,
            (Key::Cc(n, ch), Some((m, c)), _) => n == m && c.is_none_or(|c| c == ch),
            _ => false,
        }
    }
}

impl FaderBind {
    fn matches(&self, key: Key) -> bool {
        matches!(key, Key::Cc(n, ch) if n == self.cc && self.channel.is_none_or(|c| c == ch))
    }
    fn matches_lsb(&self, key: Key) -> bool {
        matches!((key, self.cc_lsb), (Key::Cc(n, ch), Some(l)) if n == l && self.channel.is_none_or(|c| c == ch))
    }
}

fn deck(e: ControlEvent) -> Event { Event::Deck(e) }
fn ui(e: UiEvent) -> Event { Event::Ui(e) }

fn press_events(a: &Action) -> Vec<Event> {
    use Action::*;
    let one = |e| vec![e];
    match a {
        PlayPause => one(deck(ControlEvent::PlayPause)),
        Play      => one(deck(ControlEvent::Play)),
        Pause     => one(deck(ControlEvent::Pause)),
        Sync      => one(deck(ControlEvent::SyncToggle)),
        Master    => one(deck(ControlEvent::MasterRequest)),
        Slip      => one(deck(ControlEvent::SlipToggle)),
        KeyLock   => one(deck(ControlEvent::KeyLockToggle)),
        JogMode   => one(deck(ControlEvent::JogModeToggle)),
        LoopIn    => one(deck(ControlEvent::LoopIn)),
        LoopOut   => one(deck(ControlEvent::LoopOut)),
        Loop      => one(deck(ControlEvent::LoopToggle)),
        Reloop    => one(deck(ControlEvent::Reloop)),
        BeatLoop { beats } => one(deck(ControlEvent::BeatLoop { beats: *beats, held: true })),
        BeatJump { beats } => one(deck(ControlEvent::BeatJump { beats: *beats })),
        HotCueSet { slot }    => one(deck(ControlEvent::HotCueSet { slot: *slot })),
        HotCueDelete { slot } => one(deck(ControlEvent::HotCueDelete { slot: *slot })),
        MemorySet    => one(deck(ControlEvent::MemoryCueSet)),
        MemoryPrev   => one(deck(ControlEvent::MemoryCueCall { next: false })),
        MemoryNext   => one(deck(ControlEvent::MemoryCueCall { next: true })),
        MemoryDelete => one(deck(ControlEvent::MemoryCueDelete)),
        TempoNudge { delta } => one(deck(ControlEvent::TempoNudge { delta: *delta })),
        Load  => one(deck(ControlEvent::Load)),
        Back  => one(deck(ControlEvent::Back)),
        Eject => one(deck(ControlEvent::Eject)),
        Shift => Vec::new(),
        Cue   => one(deck(ControlEvent::Cue { pressed: true })),
        HotCue { slot } => one(deck(ControlEvent::HotCueTrigger { slot: *slot, held: true })),
        JogTouch => one(deck(ControlEvent::JogTouch { touched: true })),
        TimeMode  => one(ui(UiEvent::TimeMode)),
        AutoCue   => one(ui(UiEvent::AutoCue)),
        WaveColor => one(ui(UiEvent::CycleColor)),
        PhaseView => one(ui(UiEvent::PhaseMeterView)),
        Zoom { step } => one(ui(UiEvent::ZoomStep(*step))),
        Screen { name } => one(ui(UiEvent::Screen(match name {
            config::ScreenName::Browse  => input::Screen::Browse,
            config::ScreenName::TagList => input::Screen::TagList,
            config::ScreenName::Info    => input::Screen::Info,
            config::ScreenName::Menu    => input::Screen::Menu,
            config::ScreenName::Perform => input::Screen::Perform,
        }))),
        PerformMode { mode } => one(ui(UiEvent::PerformMode(match mode {
            config::PerformModeName::HotCue   => input::PerformMode::HotCue,
            config::PerformModeName::BeatJump => input::PerformMode::BeatJump,
        }))),
        PerformBank   => one(ui(UiEvent::PerformBank)),
        PerformDelete => one(ui(UiEvent::PerformDelete)),
        TagTrack      => one(ui(UiEvent::TagTrack)),
        // Continuous actions have no press.
        Jog | Browse | Tempo | Needle => Vec::new(),
    }
}

fn release_events(a: &Action) -> Vec<Event> {
    match a {
        Action::Cue => vec![deck(ControlEvent::Cue { pressed: false })],
        Action::HotCue { slot } => vec![deck(ControlEvent::HotCueRelease { slot: *slot })],
        Action::JogTouch => vec![deck(ControlEvent::JogTouch { touched: false })],
        Action::BeatLoop { beats } => vec![deck(ControlEvent::BeatLoop { beats: *beats, held: false })],
        _ => Vec::new(),
    }
}

fn delta_events(a: &Action, steps: i32, scale: f32) -> Vec<Event> {
    let scaled = ((steps as f32) * scale).round() as i32;
    match a {
        Action::Jog => {
            let d = if scaled == 0 { steps.signum() } else { scaled };
            vec![deck(ControlEvent::JogDelta { delta: d, velocity_rpm: d as f32 * 2.0 })]
        }
        Action::Browse => vec![deck(ControlEvent::BrowseEncoderDelta { delta: if scaled == 0 { steps.signum() } else { scaled } })],
        Action::Zoom { step } => vec![ui(UiEvent::ZoomStep(step * steps.signum()))],
        Action::TempoNudge { delta } => vec![deck(ControlEvent::TempoNudge { delta: delta * steps as f32 })],
        Action::BeatJump { beats } => vec![deck(ControlEvent::BeatJump { beats: beats * steps.signum() as f32 })],
        _ => Vec::new(),
    }
}

fn abs_events(a: &Action, p: f32) -> Vec<Event> {
    match a {
        Action::Tempo  => vec![deck(ControlEvent::TempoFader { position: p })],
        Action::Needle => vec![deck(ControlEvent::NeedleSearch { position: p })],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::ControlEvent as CE;
    use super::super::config::SurfaceFile;

    fn mapper(toml: &str, deck: Deck) -> Mapper {
        let f = SurfaceFile::parse(toml).unwrap();
        Mapper::new(&f.surface[0], deck)
    }

    fn decks(evs: Vec<Event>) -> Vec<CE> {
        evs.into_iter().filter_map(|e| match e { Event::Deck(d) => Some(d), _ => None }).collect()
    }

    const BOARD: &str = r#"
        [[surface]]
        name = "board"
        [surface.i2c]
        address = 0x20
        [[surface.button]]
        pin = 0
        action = "play_pause"
        [[surface.button]]
        pin = 0
        action = "cue"
        shift = true
        [[surface.button]]
        pin = 1
        action = "shift"
        [[surface.button]]
        pin = 2
        action = "hot_cue"
        slot = 3
        [[surface.encoder]]
        pins = [4, 5]
        action = "browse"
        [[surface.encoder]]
        pins = [4, 5]
        action = "zoom"
        step = 1
        shift = true
        [[surface.encoder]]
        pins = [6, 7]
        action = "jog"
        scale = 3
        reverse = true
    "#;

    #[test]
    fn shift_layers_a_button_and_releases_what_was_pressed() {
        let mut m = mapper(BOARD, Deck::A);
        assert!(matches!(decks(m.press(Key::Pin(0), false))[..], [CE::PlayPause]));
        assert!(m.release(Key::Pin(0)).is_empty());
        // Hold SHIFT: the same pin is now CUE, and its release is CUE's release
        // even though SHIFT let go first.
        m.press(Key::Pin(1), false);
        assert!(m.shift_held());
        assert!(matches!(decks(m.press(Key::Pin(0), false))[..], [CE::Cue { pressed: true }]));
        m.release(Key::Pin(1));
        assert!(!m.shift_held());
        assert!(matches!(decks(m.release(Key::Pin(0)))[..], [CE::Cue { pressed: false }]));
        // A pin with only a plain binding still works under SHIFT.
        m.press(Key::Pin(1), false);
        assert!(matches!(decks(m.press(Key::Pin(2), false))[..], [CE::HotCueTrigger { slot: 3, held: true }]));
        assert!(matches!(decks(m.release(Key::Pin(2)))[..], [CE::HotCueRelease { slot: 3 }]));
    }

    #[test]
    fn encoders_scale_reverse_and_layer() {
        let mut m = mapper(BOARD, Deck::A);
        assert!(matches!(decks(m.delta(Key::Pins(4, 5), 1))[..], [CE::BrowseEncoderDelta { delta: 1 }]));
        assert!(matches!(decks(m.delta(Key::Pins(6, 7), 2))[..], [CE::JogDelta { delta: -6, .. }]));
        m.press(Key::Pin(1), false);
        assert!(matches!(m.delta(Key::Pins(4, 5), -1)[..], [Event::Ui(UiEvent::ZoomStep(-1))]));
    }

    const MIDI: &str = r#"
        [[surface]]
        name = "ctl"
        [surface.midi]
        port = "X"
        [[surface.button]]
        note = 0x10
        action = "cue"
        deck = "A"
        [[surface.button]]
        note = 0x11
        action = "cue"
        deck = "B"
        [[surface.button]]
        note_off = 0x20
        action = "load"
        [[surface.button]]
        cc = 0x30
        action = "sync"
        [[surface.encoder]]
        cc = 0x40
        action = "browse"
        cc_mode = "offset64"
        [[surface.fader]]
        cc = 0x50
        action = "tempo"
        invert = true
        [[surface.fader]]
        cc = 0x51
        cc_lsb = 0x52
        action = "needle"
        center_detent = false
    "#;

    #[test]
    fn midi_verbs() {
        let mut m = mapper(MIDI, Deck::A);
        assert!(matches!(decks(m.note_on(0, 0x10, 100))[..], [CE::Cue { pressed: true }]));
        assert!(matches!(decks(m.note_on(0, 0x10, 0))[..], [CE::Cue { pressed: false }]));   // vel 0 = release
        assert!(m.note_on(0, 0x11, 100).is_empty());                                        // deck B's note
        assert!(m.note_on(0, 0x20, 100).is_empty());                                        // note_off source: the On is not it
        assert!(matches!(decks(m.note_off(0, 0x20))[..], [CE::Load]));
        assert!(matches!(decks(m.cc(0, 0x30, 127))[..], [CE::SyncToggle]));
        assert!(m.cc(0, 0x30, 0).is_empty());
        assert!(matches!(decks(m.cc(0, 0x40, 62))[..], [CE::BrowseEncoderDelta { delta: -2 }]));
        assert!(matches!(decks(m.cc(0, 0x50, 127))[..], [CE::TempoFader { position }] if position < 0.001));
        assert!(matches!(decks(m.cc(0, 0x50, 64))[..], [CE::TempoFader { position }] if (position - 0.5).abs() < 1e-6));
        // 14-bit: MSB arms, LSB fires.
        assert!(m.cc(0, 0x51, 0x7F).is_empty());
        assert!(matches!(decks(m.cc(0, 0x52, 0x7F))[..], [CE::NeedleSearch { position }] if (position - 1.0).abs() < 1e-6));
    }

    #[test]
    fn deck_b_sees_its_own_table() {
        let mut m = mapper(MIDI, Deck::B);
        assert!(m.note_on(0, 0x10, 100).is_empty());
        assert!(matches!(decks(m.note_on(0, 0x11, 100))[..], [CE::Cue { pressed: true }]));
        assert!(matches!(decks(m.note_off(0, 0x20))[..], [CE::Load]));                       // untagged: both decks
    }
}
