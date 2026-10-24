//! The controller file: `controllers.toml`.
//!
//! One file describes every control surface attached to this deck — a USB
//! MIDI controller, a PCF8574 button board on the Pi's I2C bus, several of
//! each.  Each `[[surface]]` names a transport and lists its controls by kind
//! (`button`, `encoder`, `fader`, `led`), and each control names the deck
//! action it drives.  The schema is validated on load so a typo fails with
//! a line number, not a dead button at the gig.
//!
//! Full reference with wiring notes: docs/hardware/control-surfaces.md.

use serde::Deserialize;
use std::fmt;

/// Which of the app's two deck tables a control belongs to (`--deck A|B`).
/// Controllers that carry two decks on one port (the DJ2Go) tag each control;
/// a control without a deck is always live.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deck { A, B }

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct SurfaceFile {
    #[serde(default)]
    pub surface: Vec<Surface>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    /// Shown in the log; a user surface with the same name replaces the
    /// built-in preset of that name.
    pub name: String,
    pub midi: Option<MidiTransport>,
    pub i2c:  Option<I2cTransport>,
    #[serde(default)] pub button:  Vec<Button>,
    #[serde(default)] pub encoder: Vec<Encoder>,
    #[serde(default)] pub fader:   Vec<Fader>,
    #[serde(default)] pub led:     Vec<Led>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MidiTransport {
    /// Substring of the MIDI port name (as `aconnect -l` / the log shows it).
    pub port: String,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Chip {
    /// NXP/TI PCF8574 / PCF8574A: 8 quasi-bidirectional pins, one byte in,
    /// one byte out, no registers.
    Pcf8574,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct I2cTransport {
    /// Linux bus device; the Pi's header bus is `/dev/i2c-1`.
    #[serde(default = "default_bus")]
    pub bus: String,
    /// 7-bit address (PCF8574: 0x20–0x27; PCF8574A: 0x38–0x3F).
    pub address: u8,
    #[serde(default = "default_chip")]
    pub chip: Chip,
    /// How often the pins are read.  Buttons are happy at 200 Hz; a browse
    /// encoder wants 1000.
    #[serde(default = "default_poll_hz")]
    pub poll_hz: u32,
    /// A button must read the same for this long before it counts.
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u32,
    /// Buttons pull the pin to ground when pressed (the PCF8574's own weak
    /// pull-up makes this the natural wiring).
    #[serde(default = "default_true")]
    pub active_low: bool,
}

fn default_bus() -> String { "/dev/i2c-1".into() }
fn default_chip() -> Chip { Chip::Pcf8574 }
fn default_poll_hz() -> u32 { 500 }
fn default_debounce_ms() -> u32 { 5 }
fn default_true() -> bool { true }
fn default_scale() -> f32 { 1.0 }
fn default_steps() -> u8 { 4 }
fn default_on() -> u8 { 127 }

/// A momentary control.  Exactly one source field is set.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Button {
    /// MIDI: Note On (velocity > 0) is the press, Note Off / velocity 0 the release.
    pub note: Option<u8>,
    /// MIDI: a button that only ever sends Note Off (the DJ2Go's LOAD / BACK /
    /// ENTER) — the Note Off is the press; there is no release.
    pub note_off: Option<u8>,
    /// MIDI: a button sent as Control Change — value ≥ 64 is the press.
    pub cc: Option<u8>,
    /// MIDI channel 0–15 to match; unset = any.
    pub channel: Option<u8>,
    /// I2C: the expander pin, 0–7.
    pub pin: Option<u8>,
    pub action: ActionName,
    // Parameters some actions take (see [`Action`]).
    pub slot: Option<u8>,
    pub beats: Option<f32>,
    pub step: Option<i32>,
    pub delta: Option<f32>,
    pub name: Option<ScreenName>,
    pub mode: Option<PerformModeName>,
    /// Only while the surface's SHIFT button is held.
    #[serde(default)]
    pub shift: bool,
    pub deck: Option<Deck>,
}

impl Button {
    pub fn action(&self) -> Result<Action, String> {
        resolve(self.action, Params { slot: self.slot, beats: self.beats, step: self.step, delta: self.delta, name: self.name, mode: self.mode })
    }
}

/// Relative encoder modes seen on MIDI controllers.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EncoderMode {
    /// 1..63 = clockwise, 127..65 = counter-clockwise (two's complement).
    #[default]
    TwosComplement,
    /// 65.. = clockwise by (v − 64), ..63 = counter-clockwise by (64 − v).
    Offset64,
    /// Bit 6 set = counter-clockwise; low 6 bits the magnitude.
    SignBit,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Encoder {
    /// MIDI: relative Control Change.
    pub cc: Option<u8>,
    pub channel: Option<u8>,
    /// MIDI: how the controller encodes direction in the CC value.
    #[serde(default)]
    pub cc_mode: EncoderMode,
    /// I2C: the quadrature pair [A, B].
    pub pins: Option<[u8; 2]>,
    /// I2C: transitions per detent (a mechanical EC11 is 4; some are 2).
    #[serde(default = "default_steps")]
    pub steps_per_detent: u8,
    /// Swap direction.
    #[serde(default)]
    pub reverse: bool,
    /// Multiply each step before it reaches the deck (jog counts per detent).
    #[serde(default = "default_scale")]
    pub scale: f32,
    pub action: ActionName,
    pub beats: Option<f32>,
    pub step: Option<i32>,
    pub delta: Option<f32>,
    #[serde(default)]
    pub shift: bool,
    pub deck: Option<Deck>,
}

impl Encoder {
    pub fn action(&self) -> Result<Action, String> {
        resolve(self.action, Params { beats: self.beats, step: self.step, delta: self.delta, ..Params::default() })
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Fader {
    /// MIDI: absolute Control Change, 0–127.
    pub cc: Option<u8>,
    /// MIDI: the LSB CC of a 14-bit pair, when the controller sends one.
    pub cc_lsb: Option<u8>,
    pub channel: Option<u8>,
    /// Flip travel: 127 becomes the bottom of the fader.
    #[serde(default)]
    pub invert: bool,
    /// Snap the two centre codes to exactly the middle (a tempo fader's detent).
    #[serde(default = "default_true")]
    pub center_detent: bool,
    pub action: ActionName,
    pub deck: Option<Deck>,
}

impl Fader {
    pub fn action(&self) -> Result<Action, String> { resolve(self.action, Params::default()) }
}

/// An indicator on the surface that follows deck state.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Led {
    /// MIDI: lit by Note On with `on` velocity, cleared with `off`.
    pub note: Option<u8>,
    /// MIDI: lit by a Control Change value instead.
    pub cc: Option<u8>,
    #[serde(default)]
    pub channel: u8,
    #[serde(default = "default_on")]
    pub on: u8,
    #[serde(default)]
    pub off: u8,
    /// I2C: the expander pin.
    pub pin: Option<u8>,
    /// I2C: the LED's anode is on Vcc and the pin sinks it (write 0 = lit) —
    /// the PCF8574's strong direction.  `false` for a pin that sources.
    #[serde(default = "default_true")]
    pub active_low: bool,
    /// Lit steadily while this is true.
    pub shows: Indicator,
    /// Blinks while this is true (takes precedence over `shows`).
    pub blink_on: Option<Indicator>,
    pub deck: Option<Deck>,
}

/// Deck state a LED can follow.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Indicator {
    Playing,
    Paused,
    /// The deck is parked on its cue point (the XDJ's steady CUE lamp).
    Cue,
    /// Paused somewhere other than the cue, so CUE would set a new one (the
    /// XDJ blinks its CUE lamp here).
    CueSettable,
    Sync,
    Master,
    Loop,
    Slip,
    KeyLock,
    Vinyl,
    /// Another player is on the Link network.
    Linked,
    HotCue0, HotCue1, HotCue2, HotCue3, HotCue4, HotCue5, HotCue6, HotCue7,
    /// This surface's own SHIFT is held.
    Shift,
    /// Always on (a power light).
    On,
}

/// The `action = "…"` names, exactly as written in the file.  Resolved to
/// an [`Action`] with the parameters on the same table by [`resolve`].
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionName {
    PlayPause, Play, Pause, Sync, Master, Slip, KeyLock, JogMode, LoopIn, LoopOut, Loop, Reloop,
    BeatLoop, BeatJump, HotCueSet, HotCueDelete, MemorySet, MemoryPrev, MemoryNext, MemoryDelete,
    TempoNudge, Load, Back, Eject, Shift, Cue, HotCue, JogTouch,
    TimeMode, AutoCue, WaveColor, PhaseView, Zoom, Screen, PerformMode, PerformBank, PerformDelete, TagTrack,
    Jog, Browse, Tempo, Needle,
}

/// The parameter fields a control table may carry.
#[derive(Debug, Default, Clone, Copy)]
pub struct Params {
    pub slot: Option<u8>,
    pub beats: Option<f32>,
    pub step: Option<i32>,
    pub delta: Option<f32>,
    pub name: Option<ScreenName>,
    pub mode: Option<PerformModeName>,
}

/// Pair a name with its parameters, insisting on the ones it needs and
/// refusing ones it does not take (a stray `slot = 2` on `play_pause` is a
/// mistake worth hearing about).
pub fn resolve(n: ActionName, p: Params) -> Result<Action, String> {
    use ActionName as N;
    let need = |what: &str| format!("{} needs `{what} = …`", action_name_str(n));
    let a = match n {
        N::PlayPause => Action::PlayPause, N::Play => Action::Play, N::Pause => Action::Pause,
        N::Sync => Action::Sync, N::Master => Action::Master, N::Slip => Action::Slip, N::KeyLock => Action::KeyLock,
        N::JogMode => Action::JogMode, N::LoopIn => Action::LoopIn, N::LoopOut => Action::LoopOut,
        N::Loop => Action::Loop, N::Reloop => Action::Reloop,
        N::BeatLoop => Action::BeatLoop { beats: p.beats.ok_or_else(|| need("beats"))? },
        N::BeatJump => Action::BeatJump { beats: p.beats.ok_or_else(|| need("beats"))? },
        N::HotCueSet => Action::HotCueSet { slot: p.slot.ok_or_else(|| need("slot"))? },
        N::HotCueDelete => Action::HotCueDelete { slot: p.slot.ok_or_else(|| need("slot"))? },
        N::MemorySet => Action::MemorySet, N::MemoryPrev => Action::MemoryPrev, N::MemoryNext => Action::MemoryNext,
        N::MemoryDelete => Action::MemoryDelete,
        N::TempoNudge => Action::TempoNudge { delta: p.delta.ok_or_else(|| need("delta"))? },
        N::Load => Action::Load, N::Back => Action::Back, N::Eject => Action::Eject, N::Shift => Action::Shift,
        N::Cue => Action::Cue,
        N::HotCue => Action::HotCue { slot: p.slot.ok_or_else(|| need("slot"))? },
        N::JogTouch => Action::JogTouch,
        N::TimeMode => Action::TimeMode, N::AutoCue => Action::AutoCue, N::WaveColor => Action::WaveColor,
        N::PhaseView => Action::PhaseView,
        N::Zoom => Action::Zoom { step: p.step.ok_or_else(|| need("step"))? },
        N::Screen => Action::Screen { name: p.name.ok_or_else(|| need("name"))? },
        N::PerformMode => Action::PerformMode { mode: p.mode.ok_or_else(|| need("mode"))? },
        N::PerformBank => Action::PerformBank, N::PerformDelete => Action::PerformDelete, N::TagTrack => Action::TagTrack,
        N::Jog => Action::Jog, N::Browse => Action::Browse, N::Tempo => Action::Tempo, N::Needle => Action::Needle,
    };
    // Anything set that the action did not consume is a stray.
    let takes = |f: &str| matches!((n, f),
        (N::BeatLoop | N::BeatJump, "beats") | (N::HotCue | N::HotCueSet | N::HotCueDelete, "slot")
        | (N::TempoNudge, "delta") | (N::Zoom, "step") | (N::Screen, "name") | (N::PerformMode, "mode"));
    for (f, set) in [("slot", p.slot.is_some()), ("beats", p.beats.is_some()), ("step", p.step.is_some()),
                     ("delta", p.delta.is_some()), ("name", p.name.is_some()), ("mode", p.mode.is_some())] {
        if set && !takes(f) { return Err(format!("`{f}` is not a parameter of {}", action_name_str(n))); }
    }
    Ok(a)
}

fn action_name_str(n: ActionName) -> String { snake(&format!("{n:?}")) }

fn snake(head: &str) -> String {
    let mut out = String::new();
    for (i, c) in head.chars().enumerate() {
        if c.is_uppercase() && i > 0 { out.push('_'); }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// What a control does, resolved from `action = "…"` and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // ── buttons: taps ────────────────────────────────────────────────────────
    PlayPause,
    Play,
    Pause,
    Sync,
    Master,
    Slip,
    KeyLock,
    JogMode,
    LoopIn,
    LoopOut,
    /// LOOP toggle / RELOOP.
    Loop,
    Reloop,
    BeatLoop { beats: f32 },
    BeatJump { beats: f32 },
    HotCueSet { slot: u8 },
    HotCueDelete { slot: u8 },
    MemorySet,
    MemoryPrev,
    MemoryNext,
    MemoryDelete,
    /// Pitch ± as a fraction per press (0.01 = 1 %).  On an encoder: per step.
    TempoNudge { delta: f32 },
    Load,
    Back,
    Eject,
    /// Hold for this surface's shifted layer.
    Shift,
    // ── buttons: held ────────────────────────────────────────────────────────
    /// CUE: back-cue while playing, set + preview while paused; release matters.
    Cue,
    /// A hot-cue pad: plays from the cue while held (SLIP returns on release).
    HotCue { slot: u8 },
    /// Platter touch: holds the transport while down.
    JogTouch,
    // ── display ──────────────────────────────────────────────────────────────
    TimeMode,
    AutoCue,
    WaveColor,
    PhaseView,
    /// Zoom the enlarged waveform; on a button `step` whole steps, on an
    /// encoder `step` per click.
    Zoom { step: i32 },
    Screen { name: ScreenName },
    PerformMode { mode: PerformModeName },
    PerformBank,
    PerformDelete,
    TagTrack,
    // ── encoders ─────────────────────────────────────────────────────────────
    /// The jog wheel: counts become `JogDelta`; `scale` sets counts per step.
    Jog,
    /// The browse knob.
    Browse,
    // ── faders ───────────────────────────────────────────────────────────────
    /// The tempo fader (absolute).
    Tempo,
    /// The needle-search strip (absolute track position).
    Needle,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScreenName { Browse, TagList, Info, Menu, Perform }

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PerformModeName { HotCue, BeatJump }

/// Which control kind an action may sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Button, Encoder, Fader }

impl Action {
    pub fn allowed_on(&self, kind: Kind) -> bool {
        use Action::*;
        match self {
            Jog | Browse => kind == Kind::Encoder,
            Tempo | Needle => kind == Kind::Fader,
            TempoNudge { .. } | Zoom { .. } | BeatJump { .. } => matches!(kind, Kind::Button | Kind::Encoder),
            _ => kind == Kind::Button,
        }
    }

    /// The `action = "…"` spelling, for messages.
    pub fn name(&self) -> String {
        let d = format!("{self:?}");
        snake(d.split(|c: char| !c.is_alphanumeric()).next().unwrap_or(""))
    }
}

#[derive(Debug)]
pub struct ConfigError(pub String);
impl fmt::Display for ConfigError { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) } }
impl std::error::Error for ConfigError {}

impl SurfaceFile {
    /// Parse and validate one file's text.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let file: SurfaceFile = toml::from_str(text).map_err(|e| ConfigError(e.to_string()))?;
        file.validate()?;
        Ok(file)
    }

    /// Merge `other` over `self`: a surface with a name already present
    /// replaces it; new names are appended.
    pub fn merge(&mut self, other: SurfaceFile) {
        for s in other.surface {
            match self.surface.iter_mut().find(|x| x.name == s.name) {
                Some(slot) => *slot = s,
                None => self.surface.push(s),
            }
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let err = |s: &Surface, m: String| Err(ConfigError(format!("surface \"{}\": {m}", s.name)));
        for s in &self.surface {
            let is_midi = s.midi.is_some();
            let is_i2c = s.i2c.is_some();
            if is_midi == is_i2c { return err(s, "needs exactly one of [surface.midi] or [surface.i2c]".into()); }
            if let Some(i) = &s.i2c {
                if i.poll_hz == 0 || i.poll_hz > 5000 { return err(s, format!("poll_hz {} must be 1–5000", i.poll_hz)); }
                if i.address > 0x7F { return err(s, format!("address 0x{:02X} is not a 7-bit I2C address", i.address)); }
            }
            let one = |fields: &[(&str, bool)], what: String| -> Result<(), ConfigError> {
                let set: Vec<&str> = fields.iter().filter(|(_, b)| *b).map(|(n, _)| *n).collect();
                match set.len() {
                    1 => Ok(()),
                    0 => Err(ConfigError(format!("surface \"{}\": {what} has no source — set one of {}", s.name,
                            fields.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")))),
                    _ => Err(ConfigError(format!("surface \"{}\": {what} sets more than one source ({})", s.name, set.join(", ")))),
                }
            };
            let transport_ok = |midi_src: bool, i2c_src: bool, what: String| -> Result<(), ConfigError> {
                if is_midi && i2c_src { return Err(ConfigError(format!("surface \"{}\": {what} uses an I2C pin on a MIDI surface", s.name))); }
                if is_i2c && midi_src { return Err(ConfigError(format!("surface \"{}\": {what} uses a MIDI note/cc on an I2C surface", s.name))); }
                Ok(())
            };
            let pin_ok = |p: Option<u8>, what: &str| -> Result<(), ConfigError> {
                match p { Some(p) if p > 7 => Err(ConfigError(format!("surface \"{}\": {what} pin {p} — a PCF8574 has pins 0–7", s.name))), _ => Ok(()) }
            };
            let seven = |v: Option<u8>, what: &str, field: &str| -> Result<(), ConfigError> {
                match v { Some(v) if v > 127 => Err(ConfigError(format!("surface \"{}\": {what} {field} {v} — MIDI data is 0–127", s.name))), _ => Ok(()) }
            };
            let chan = |c: Option<u8>, what: &str| -> Result<(), ConfigError> {
                match c { Some(c) if c > 15 => Err(ConfigError(format!("surface \"{}\": {what} channel {c} — MIDI channels are 0–15", s.name))), _ => Ok(()) }
            };
            for (n, b) in s.button.iter().enumerate() {
                let what = format!("button {} ({})", n + 1, action_name_str(b.action));
                let action = b.action().map_err(|m| ConfigError(format!("surface \"{}\": {what}: {m}", s.name)))?;
                one(&[("note", b.note.is_some()), ("note_off", b.note_off.is_some()), ("cc", b.cc.is_some()), ("pin", b.pin.is_some())], what.clone())?;
                transport_ok(b.note.is_some() || b.note_off.is_some() || b.cc.is_some(), b.pin.is_some(), what.clone())?;
                pin_ok(b.pin, &what)?; seven(b.note, &what, "note")?; seven(b.note_off, &what, "note_off")?; seven(b.cc, &what, "cc")?; chan(b.channel, &what)?;
                if !action.allowed_on(Kind::Button) { return err(s, format!("{what}: \"{}\" is not a button action", action.name())); }
                if let Action::HotCue { slot } | Action::HotCueSet { slot } | Action::HotCueDelete { slot } = action {
                    if slot > 7 { return err(s, format!("{what}: hot cue slot {slot} — slots are 0–7 (A–H)")); }
                }
            }
            for (n, e) in s.encoder.iter().enumerate() {
                let what = format!("encoder {} ({})", n + 1, action_name_str(e.action));
                let action = e.action().map_err(|m| ConfigError(format!("surface \"{}\": {what}: {m}", s.name)))?;
                one(&[("cc", e.cc.is_some()), ("pins", e.pins.is_some())], what.clone())?;
                transport_ok(e.cc.is_some(), e.pins.is_some(), what.clone())?;
                if let Some([a, b]) = e.pins {
                    pin_ok(Some(a), &what)?; pin_ok(Some(b), &what)?;
                    if a == b { return err(s, format!("{what}: pins [{a}, {b}] must differ")); }
                }
                if e.steps_per_detent == 0 { return err(s, format!("{what}: steps_per_detent must be ≥ 1")); }
                seven(e.cc, &what, "cc")?; chan(e.channel, &what)?;
                if !action.allowed_on(Kind::Encoder) { return err(s, format!("{what}: \"{}\" is not an encoder action", action.name())); }
            }
            for (n, f) in s.fader.iter().enumerate() {
                let what = format!("fader {} ({})", n + 1, action_name_str(f.action));
                let action = f.action().map_err(|m| ConfigError(format!("surface \"{}\": {what}: {m}", s.name)))?;
                if is_i2c { return err(s, format!("{what}: a PCF8574 has no analog input; a fader needs an ADC (not supported yet)")); }
                one(&[("cc", f.cc.is_some())], what.clone())?;
                seven(f.cc, &what, "cc")?; seven(f.cc_lsb, &what, "cc_lsb")?; chan(f.channel, &what)?;
                if !action.allowed_on(Kind::Fader) { return err(s, format!("{what}: \"{}\" is not a fader action", action.name())); }
            }
            for (n, l) in s.led.iter().enumerate() {
                let what = format!("led {} ({:?})", n + 1, l.shows);
                one(&[("note", l.note.is_some()), ("cc", l.cc.is_some()), ("pin", l.pin.is_some())], what.clone())?;
                transport_ok(l.note.is_some() || l.cc.is_some(), l.pin.is_some(), what.clone())?;
                pin_ok(l.pin, &what)?; seven(l.note, &what, "note")?; seven(l.cc, &what, "cc")?; chan(Some(l.channel), &what)?;
                seven(Some(l.on), &what, "on")?; seven(Some(l.off), &what, "off")?;
            }
            // A pin is a button, half of one encoder, or a LED — not two of
            // those.  The same button pin (or encoder pair) bound twice is
            // fine: that is how the SHIFT layer is written.
            if is_i2c {
                let mut used: Vec<(u8, String, String)> = Vec::new();   // pin, role, description
                let mut claim = |pin: u8, role: String, who: String| -> Result<(), ConfigError> {
                    if let Some((_, r, other)) = used.iter().find(|(p, _, _)| *p == pin) {
                        if *r != role {
                            return Err(ConfigError(format!("surface \"{}\": pin {pin} is both {other} and {who}", s.name)));
                        }
                        return Ok(());
                    }
                    used.push((pin, role, who)); Ok(())
                };
                for b in &s.button { if let Some(p) = b.pin { claim(p, "button".into(), format!("button {}", action_name_str(b.action)))?; } }
                for e in &s.encoder { if let Some([a, b]) = e.pins {
                    let role = format!("encoder {a},{b}");
                    claim(a, role.clone(), format!("encoder {} A", action_name_str(e.action)))?;
                    claim(b, role, format!("encoder {} B", action_name_str(e.action)))?;
                } }
                for (n, l) in s.led.iter().enumerate() { if let Some(p) = l.pin { claim(p, format!("led #{n}"), format!("led {:?}", l.shows))?; } }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_i2c_board() {
        let f = SurfaceFile::parse(r#"
            [[surface]]
            name = "board"
            [surface.i2c]
            address = 0x20
            [[surface.button]]
            pin = 0
            action = "play_pause"
            [[surface.button]]
            pin = 1
            action = "hot_cue"
            slot = 2
            shift = true
            [[surface.encoder]]
            pins = [2, 3]
            action = "browse"
            [[surface.led]]
            pin = 7
            shows = "playing"
            blink_on = "paused"
        "#).unwrap();
        let s = &f.surface[0];
        assert_eq!(s.i2c.as_ref().unwrap().bus, "/dev/i2c-1");
        assert_eq!(s.i2c.as_ref().unwrap().poll_hz, 500);
        assert_eq!(s.button[1].action().unwrap(), Action::HotCue { slot: 2 });
        assert!(s.button[1].shift);
        assert_eq!(s.encoder[0].steps_per_detent, 4);
        assert_eq!(s.led[0].blink_on, Some(Indicator::Paused));
    }

    #[test]
    fn rejects_the_usual_mistakes() {
        let bad = |t: &str, needle: &str| {
            let e = SurfaceFile::parse(t).err().map(|e| e.0).unwrap_or_else(|| panic!("accepted:\n{t}"));
            assert!(e.contains(needle), "wanted {needle:?} in {e:?}");
        };
        bad(r#"[[surface]]
             name = "x""#, "exactly one of");
        bad(r#"[[surface]]
             name = "x"
             [surface.i2c]
             address = 0x20
             [[surface.button]]
             pin = 9
             action = "play""#, "pins 0–7");
        bad(r#"[[surface]]
             name = "x"
             [surface.i2c]
             address = 0x20
             [[surface.button]]
             note = 1
             action = "play""#, "MIDI note/cc on an I2C surface");
        bad(r#"[[surface]]
             name = "x"
             [surface.midi]
             port = "p"
             [[surface.button]]
             note = 1
             action = "jog""#, "not a button action");
        bad(r#"[[surface]]
             name = "x"
             [surface.i2c]
             address = 0x20
             [[surface.button]]
             pin = 1
             action = "play"
             [[surface.led]]
             pin = 1
             shows = "playing""#, "pin 1 is both");
        bad(r#"[[surface]]
             name = "x"
             [surface.midi]
             port = "p"
             [[surface.button]]
             note = 1
             action = "plya""#, "unknown variant");
        bad(r#"[[surface]]
             name = "x"
             [surface.midi]
             port = "p"
             [[surface.button]]
             note = 1
             action = "hot_cue"
             slot = 8"#, "slots are 0–7");
        bad(r#"[[surface]]
             name = "x"
             [surface.midi]
             port = "p"
             [[surface.button]]
             note = 1
             action = "hot_cue""#, "needs `slot");
        bad(r#"[[surface]]
             name = "x"
             [surface.midi]
             port = "p"
             [[surface.button]]
             note = 1
             action = "play_pause"
             slot = 2"#, "not a parameter of play_pause");
        bad(r#"[[surface]]
             name = "x"
             [surface.midi]
             port = "p"
             [[surface.button]]
             nte = 1
             action = "play_pause""#, "unknown field `nte`");
        bad(r#"[[surface]]
             name = "x"
             [surface.i2c]
             address = 0x20
             [[surface.led]]
             pin = 3
             shows = "playing"
             [[surface.led]]
             pin = 3
             shows = "sync""#, "pin 3 is both");
    }

    #[test]
    fn merge_replaces_by_name() {
        let mut a = SurfaceFile::parse(r#"
            [[surface]]
            name = "dj2go"
            [surface.midi]
            port = "DJ2Go"
        "#).unwrap();
        let b = SurfaceFile::parse(r#"
            [[surface]]
            name = "dj2go"
            [surface.midi]
            port = "Other"
            [[surface]]
            name = "board"
            [surface.i2c]
            address = 0x21
        "#).unwrap();
        a.merge(b);
        assert_eq!(a.surface.len(), 2);
        assert_eq!(a.surface[0].midi.as_ref().unwrap().port, "Other");
    }

    #[test]
    fn action_names_read_like_the_file() {
        assert_eq!(Action::PlayPause.name(), "play_pause");
        assert_eq!(Action::HotCue { slot: 1 }.name(), "hot_cue");
        assert_eq!(Action::Jog.name(), "jog");
    }
}
