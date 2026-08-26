//! Numark DJ2Go — a USB MIDI adapter on the input bus.
//!
//! Class-compliant MIDI: Note On/Off for buttons, Control Change for the jog
//! wheel (relative encoder) and pitch slider.  This module only *translates*
//! MIDI into [`Event`]s and pushes them onto the bus; it holds no deck state.
//! The jog *nudge* (accumulate a speed offset, snap back when idle) lives in
//! `DeckApp::apply`, not here — so a wrong jog delta and a wrong nudge can be
//! told apart (see docs/INPUT_PLAN.md, the S2 lesson).
//!
//! Run with `RUST_LOG=opendeck::midi=debug` to see every incoming message and
//! check the constants below against a different controller.

use crate::input::{ControlEvent, Event};
use midir::MidiInput;
use std::sync::mpsc::Sender;

const DEVICE_NAME: &str = "DJ2Go";

// ── Deck A mappings — (channel 0-indexed, note/cc) ───────────────────────────
const MAP_PLAY:       (u8, u8) = (0, 0x33);
const MAP_CUE:        (u8, u8) = (0, 0x3B);
const MAP_PITCH_UP:   (u8, u8) = (0, 0x43);
const MAP_PITCH_DOWN: (u8, u8) = (0, 0x44);

// The DJ2Go jog sends two CCs: 0x18 (touch plate) and 0x19 (outer ring).
// Both are relative; treat either as a jog delta.
const JOG_CC_A:   u8 = 0x18; // 24
const JOG_CC_B:   u8 = 0x19; // 25, relative: 1–63 = CW (+), 65–127 = CCW (−)
const PITCH_CC:   u8 = 0x0D; // 13, absolute 0–127, centre 64
const CH_A:       u8 = 0;

/// Pitch step for the ± buttons and the fader range, matching a CDJ.
const PITCH_STEP:  f32 = 0.01;

pub struct MidiHandle {
    _conn: midir::MidiInputConnection<()>,
}

impl MidiHandle {
    /// Connect to the DJ2Go and forward its controls to `tx` as bus events.
    pub fn connect(tx: Sender<Event>) -> Option<Self> {
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

        log::info!("MIDI: found {DEVICE_NAME} — connecting");
        let conn = midi_in
            .connect(&port, "opendeck-dj2go", move |_ts, msg, _| {
                if let Some(ev) = translate(msg) {
                    let _ = tx.send(ev);
                }
            }, ())
            .map_err(|e| log::error!("MIDI: connect failed: {e}"))
            .ok()?;

        Some(MidiHandle { _conn: conn })
    }
}

/// One MIDI message → at most one bus event.  Deck A only; the second-deck
/// simulation retired when real ProDJ Link began driving the B2 strip.
fn translate(msg: &[u8]) -> Option<Event> {
    if msg.len() < 3 { return None; }
    let kind = msg[0] & 0xF0;
    let ch   = msg[0] & 0x0F;
    log::debug!("MIDI rx: {:02X?}", msg);

    match kind {
        // Note On (velocity 0 = Note Off, ignored).
        0x90 if msg[2] > 0 => {
            let note = msg[1];
            let deck = |e| Some(Event::Deck(e));
            match (ch, note) {
                MAP_PLAY       => deck(ControlEvent::PlayPause),
                MAP_CUE        => deck(ControlEvent::Cue),
                MAP_PITCH_UP   => deck(ControlEvent::TempoNudge { delta:  PITCH_STEP }),
                MAP_PITCH_DOWN => deck(ControlEvent::TempoNudge { delta: -PITCH_STEP }),
                _ => { log::debug!("MIDI note ch{ch} 0x{note:02X} unmapped"); None }
            }
        }
        // Control Change.
        0xB0 if ch == CH_A => {
            let (cc, value) = (msg[1], msg[2]);
            match cc {
                JOG_CC_A | JOG_CC_B => {
                    // Relative two's-complement delta.
                    let delta = if value < 64 { value as i32 } else { value as i32 - 128 };
                    // The DJ2Go gives no velocity; approximate one for scratch
                    // mode from the delta (33.3 RPM reference).
                    Some(Event::Deck(ControlEvent::JogDelta { delta, velocity_rpm: delta as f32 * 2.0 }))
                }
                PITCH_CC => {
                    // Centre 64 = 0%; lower value = fader up = faster.
                    let position = 1.0 - value as f32 / 127.0;
                    Some(Event::Deck(ControlEvent::TempoFader { position }))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{ControlEvent as CE, Event};

    fn deck(msg: &[u8]) -> Option<CE> {
        match translate(msg) { Some(Event::Deck(e)) => Some(e), _ => None }
    }

    #[test]
    fn dj2go_controls_map_to_deck_events() {
        assert!(matches!(deck(&[0x90, 0x33, 0x7F]), Some(CE::PlayPause)));
        assert!(matches!(deck(&[0x90, 0x3B, 0x7F]), Some(CE::Cue)));
        assert!(matches!(deck(&[0x90, 0x43, 0x7F]), Some(CE::TempoNudge { delta }) if (delta - 0.01).abs() < 1e-6));
        assert!(matches!(deck(&[0x90, 0x44, 0x7F]), Some(CE::TempoNudge { delta }) if (delta + 0.01).abs() < 1e-6));
        // Note-off (velocity 0) produces nothing.
        assert!(translate(&[0x90, 0x33, 0x00]).is_none());
        // Jog: +5 clockwise, −5 as two's complement (123).
        assert!(matches!(deck(&[0xB0, 0x19, 5]),   Some(CE::JogDelta { delta:  5, .. })));
        assert!(matches!(deck(&[0xB0, 0x18, 5]),   Some(CE::JogDelta { delta:  5, .. })));
        assert!(matches!(deck(&[0xB0, 0x19, 123]), Some(CE::JogDelta { delta: -5, .. })));
        // Fader: centre 64 → ~0.5, top (0) → 1.0, bottom (127) → 0.0.
        assert!(matches!(deck(&[0xB0, 0x0D, 64]),  Some(CE::TempoFader { position }) if (position - 0.496).abs() < 0.02));
        assert!(matches!(deck(&[0xB0, 0x0D, 0]),   Some(CE::TempoFader { position }) if position > 0.99));
    }
}
