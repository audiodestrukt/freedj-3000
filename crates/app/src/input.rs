//! The input bus.
//!
//! Every source — keyboard, the touch screen (mouse for now), MIDI, a script,
//! the control surface — produces [`Event`]s.  `DeckApp::apply` is the only
//! place they change state.  Sources own no deck state and never touch the
//! audio atomics directly.  See docs/INPUT_PLAN.md.
//!
//! Deck actions use `opendeck_protocol::ControlEvent`, the same type the MCU
//! adapter and the engine crate speak.  Display-only settings are `UiEvent`.

pub use opendeck_protocol::ControlEvent;

#[derive(Debug, Clone)]
pub enum Event {
    /// Something a physical deck control would do.
    Deck(ControlEvent),
    /// A display setting; never reaches the audio path.
    Ui(UiEvent),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiEvent {
    /// Toggle TIME (elapsed) ↔ REMAIN.
    TimeMode,
    /// Toggle AUTO CUE: on load, cue at the first audible sound instead of 0:00.
    AutoCue,
    /// Cycle Waveform Color: RGB → 3 BAND → BLUE.
    CycleColor,
    /// Zoom the enlarged waveform by whole steps; negative = out.
    ZoomStep(i32),
    /// Toggle the ZOOM / GRID ADJUST mode indicator.
    ZoomGridMode,
    /// Phase-meter slot: 4-box beat display ↔ master alignment ticks.
    PhaseMeterView,
    /// Select the source column key: LINK or the local file/USB.
    Source(Source),
    /// A top-row touch key or a screen we do not have yet.
    Screen(Screen),
    /// PERFORM: what the pads do — HOT CUE or BEAT JUMP.
    PerformMode(PerformMode),
    /// PERFORM: BANK — flip the pads between hot cues A–D and E–H.
    PerformBank,
    /// PERFORM: DELETE –CALL — arm/disarm "the next pad tap deletes that hot cue".
    PerformDelete,
    /// TAG TRACK / REMOVE: in BROWSE, tag/untag the highlighted track; on the
    /// TAG LIST screen, remove the highlighted one.
    TagTrack,
    /// MENU: a row was tapped — select it and step its value.
    MenuTap(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source { Link, Usb }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen { Browse, TagList, Info, Menu, Perform }

/// PERFORM pad mode: the pads are hot cues, or beat jumps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerformMode { HotCue, BeatJump }

/// Zoom levels for the enlarged waveform, in waveform columns across the
/// field.  Index 2 is the default the display has always used.
pub const ZOOM_LEVELS: [f32; 6] = [300.0, 450.0, 600.0, 900.0, 1200.0, 1800.0];
pub const ZOOM_DEFAULT: usize = 2;

// The fader's range is the TEMPO RANGE menu setting (`Settings::tempo_range`,
// default ±16 %).

// DJ convention: pushing the tempo fader DOWN speeds up, UP slows down.
// `position` is geometric (0 = bottom of travel, 1 = top), so bottom = fastest.
// `range` is the fader's reach as a fraction (0.16 = ±16 %).
pub fn fader_to_speed(position: f32, range: f32) -> f32 {
    1.0 + (0.5 - position.clamp(0.0, 1.0)) * 2.0 * range
}

pub fn speed_to_fader(speed: f32, range: f32) -> f32 {
    (0.5 - (speed - 1.0) / (2.0 * range)).clamp(0.0, 1.0)
}
