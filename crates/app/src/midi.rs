//! Numark DJ2Go — a USB MIDI adapter on the input bus.
//!
//! This module only *translates* MIDI into [`Event`]s and pushes them onto
//! the bus; it holds no deck state.  The jog *nudge* (accumulate a speed
//! offset, snap back when idle) lives in `DeckApp::apply`, not here — so a
//! wrong jog delta and a wrong nudge can be told apart (docs/INPUT_PLAN.md,
//! the S2 lesson).
//!
//! The full note/CC assignment is `docs/reference/dj2go-midi-map.md`, taken
//! from the Mixxx `Numark DJ2Go.midi.xml` mapping and confirmed against the
//! hardware.  The DJ2Go puts *both* decks on MIDI channel 0 and distinguishes
//! them by note/CC number, so `--deck A|B` selects a note table, not a
//! channel.  (A true two-channel controller like the Kontrol S2 would instead
//! use channel 1 for its right deck; that is a separate adapter.)
//!
//! Run with `RUST_LOG=opendeck::midi=debug` to see every message.

use crate::input::{ControlEvent, Event};
use midir::MidiInput;
use std::sync::mpsc::Sender;

const DEVICE_NAME: &str = "DJ2Go";

/// One deck's note/CC numbers.  All messages arrive on MIDI channel 0; these
/// numbers are what separates deck A (left) from deck B (right).
struct DeckMap {
    play:     u8,   // note
    cue:      u8,   // note
    sync:     u8,   // note
    pfl:      u8,   // note — headphone cue (not implemented; needs a cue bus)
    load:     u8,   // note, sent as Note Off (0x80); Note On same number = Shift
    loop_in:  u8,   // note
    loop_out: u8,   // note
    jog_cc:   u8,   // relative CC
    fader_cc: u8,   // absolute CC, 0–127
}

const DECK_A: DeckMap = DeckMap {
    play: 0x3B, cue: 0x33, sync: 0x40, pfl: 0x65, load: 0x4B,
    loop_in: 0x44, loop_out: 0x43, jog_cc: 0x19, fader_cc: 0x0D,
};
const DECK_B: DeckMap = DeckMap {
    play: 0x42, cue: 0x3C, sync: 0x47, pfl: 0x66, load: 0x34,
    loop_in: 0x46, loop_out: 0x45, jog_cc: 0x18, fader_cc: 0x0E,
};

// ── Global controls (deck-independent) ───────────────────────────────────────
const SELECT_KNOB_CC: u8 = 0x1A;   // browse encoder, relative
const BACK_NOTE:      u8 = 0x59;   // Note Off on press
const ENTER_NOTE:     u8 = 0x5A;   // Note Off on press

pub struct MidiHandle {
    _conn: midir::MidiInputConnection<()>,
}

impl MidiHandle {
    /// Connect to the DJ2Go and forward one deck's controls to `tx`.
    /// `deck_b` selects the right-hand deck's note table.
    pub fn connect(tx: Sender<Event>, deck_b: bool) -> Option<Self> {
        let midi_in = MidiInput::new("opendeck")
            .map_err(|e| log::warn!("MIDI: init failed: {e}"))
            .ok()?;

        let ports = midi_in.ports();
        let port = ports.iter().find(|p| {
            midi_in.port_name(p).map(|n| n.contains(DEVICE_NAME)).unwrap_or(false)
        });
        let port = match port {
            Some(p) => p.clone(),
            None => {
                log::info!("MIDI: {DEVICE_NAME} not found.  Available ports:");
                for p in &ports {
                    if let Ok(name) = midi_in.port_name(p) { log::info!("  - {name}"); }
                }
                return None;
            }
        };

        log::info!("MIDI: found {DEVICE_NAME} — deck {}", if deck_b { "B/right" } else { "A/left" });
        let conn = midi_in
            .connect(&port, "opendeck-dj2go", move |_ts, msg, _| {
                if let Some(ev) = translate(msg, if deck_b { &DECK_B } else { &DECK_A }) {
                    let _ = tx.send(ev);
                }
            }, ())
            .map_err(|e| log::error!("MIDI: connect failed: {e}"))
            .ok()?;

        Some(MidiHandle { _conn: conn })
    }
}

/// One MIDI message → at most one bus event, for the given deck's controls
/// plus the shared browse controls.
fn translate(msg: &[u8], m: &DeckMap) -> Option<Event> {
    if msg.len() < 3 { return None; }
    let status = msg[0] & 0xF0;
    let (a, b) = (msg[1], msg[2]);
    log::debug!("MIDI rx: {:02X?}", msg);
    let deck = |e| Some(Event::Deck(e));

    match status {
        // ── Note On (button press; velocity 0 = release) ────────────────────
        0x90 if b > 0 => match a {
            n if n == m.play     => deck(ControlEvent::PlayPause),
            n if n == m.cue      => deck(ControlEvent::Cue { pressed: true }),
            n if n == m.sync     => deck(ControlEvent::SyncToggle),
            n if n == m.loop_in  => deck(ControlEvent::LoopIn),
            n if n == m.loop_out => deck(ControlEvent::LoopOut),
            n if n == m.pfl      => { log::info!("DJ2Go: headphone cue — no cue bus yet"); None }
            // Note On for load/back/enter = the Shift layer; ignore for now.
            _ => { log::debug!("MIDI note-on 0x{a:02X} unmapped"); None }
        },
        // ── Note Off — Load / Back / Enter fire here on the DJ2Go ───────────
        // Some controllers send Note On velocity 0 as release; the DJ2Go's CUE
        // release matters for the momentary preview.
        0x90 => match a {
            n if n == m.cue => deck(ControlEvent::Cue { pressed: false }),
            _ => None,
        },
        0x80 => match a {
            n if n == m.cue  => deck(ControlEvent::Cue { pressed: false }),
            n if n == m.load => deck(ControlEvent::Load),
            BACK_NOTE        => deck(ControlEvent::Back),
            ENTER_NOTE       => deck(ControlEvent::Load),   // Enter loads the selection
            _ => None,
        },
        // ── Control Change (faders, jog, browse knob) ───────────────────────
        0xB0 => match a {
            cc if cc == m.jog_cc => {
                let delta = if b < 64 { b as i32 } else { b as i32 - 128 };
                Some(Event::Deck(ControlEvent::JogDelta { delta, velocity_rpm: delta as f32 * 2.0 }))
            }
            cc if cc == m.fader_cc => {
                // Centre 64 = 0%; lower value = fader up = faster.
                let position = 1.0 - b as f32 / 127.0;
                Some(Event::Deck(ControlEvent::TempoFader { position }))
            }
            SELECT_KNOB_CC => {
                let delta = if b < 64 { b as i32 } else { b as i32 - 128 };
                Some(Event::Deck(ControlEvent::BrowseEncoderDelta { delta }))
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{ControlEvent as CE, Event};

    fn ev(msg: &[u8], deck_b: bool) -> Option<CE> {
        match translate(msg, if deck_b { &DECK_B } else { &DECK_A }) {
            Some(Event::Deck(e)) => Some(e),
            _ => None,
        }
    }

    #[test]
    fn deck_a_controls() {
        assert!(matches!(ev(&[0x90, 0x3B, 0x7F], false), Some(CE::PlayPause)));      // play
        assert!(matches!(ev(&[0x90, 0x33, 0x7F], false), Some(CE::Cue { pressed: true })));  // cue press
        assert!(matches!(ev(&[0x90, 0x40, 0x7F], false), Some(CE::SyncToggle)));     // sync
        assert!(matches!(ev(&[0x90, 0x44, 0x7F], false), Some(CE::LoopIn)));         // loop in
        assert!(matches!(ev(&[0x90, 0x43, 0x7F], false), Some(CE::LoopOut)));        // loop out
        assert!(matches!(ev(&[0x80, 0x4B, 0x00], false), Some(CE::Load)));           // load (note off)
        assert!(matches!(ev(&[0xB0, 0x19, 5],    false), Some(CE::JogDelta { delta: 5, .. })));
        assert!(matches!(ev(&[0xB0, 0x0D, 0],    false), Some(CE::TempoFader { position }) if position > 0.99));
    }

    #[test]
    fn deck_b_uses_its_own_numbers() {
        assert!(matches!(ev(&[0x90, 0x42, 0x7F], true), Some(CE::PlayPause)));       // play B
        assert!(matches!(ev(&[0x90, 0x3C, 0x7F], true), Some(CE::Cue { pressed: true })));   // cue B press
        assert!(matches!(ev(&[0x90, 0x47, 0x7F], true), Some(CE::SyncToggle)));      // sync B
        assert!(matches!(ev(&[0xB0, 0x18, 5],    true), Some(CE::JogDelta { delta: 5, .. })));
        assert!(matches!(ev(&[0xB0, 0x0E, 64],   true), Some(CE::TempoFader { .. })));
        // Deck A's play note does nothing when we are deck B.
        assert!(ev(&[0x90, 0x3B, 0x7F], true).is_none());
    }

    #[test]
    fn browse_controls_are_shared() {
        assert!(matches!(ev(&[0xB0, 0x1A, 1],   false), Some(CE::BrowseEncoderDelta { delta:  1 })));
        assert!(matches!(ev(&[0xB0, 0x1A, 127], false), Some(CE::BrowseEncoderDelta { delta: -1 })));
        assert!(matches!(ev(&[0x80, 0x59, 0],   false), Some(CE::Back)));
        assert!(matches!(ev(&[0x80, 0x5A, 0],   false), Some(CE::Load)));
    }
}
