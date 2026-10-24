//! MIDI surfaces: any class-compliant USB controller.
//!
//! Input is decoded to the mapper's verbs (Note On/Off, CC) on midir's
//! callback thread; LEDs go back out as Note On / CC on the port of the same
//! name.  The supervisor in `super::start` calls [`MidiSurface::reconcile`]
//! every couple of seconds, so a controller plugged in after launch (or
//! re-plugged) just starts working.
//!
//! `RUST_LOG=opendeck_app::surface=debug` logs every message with the
//! surface name — the way to find a controller's note and CC numbers.

use super::config::{Deck, Surface};
use super::map::Mapper;
use super::Indicators;
use crate::input::Event;
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

pub struct MidiSurface {
    name: String,
    port: String,
    mapper: Arc<Mutex<Mapper>>,
    tx: Sender<Event>,
    ind: Arc<Indicators>,
    leds: Vec<super::config::Led>,
    led_state: Vec<Option<bool>>,
    input: Option<MidiInputConnection<()>>,
    output: Option<MidiOutputConnection>,
    /// Port name we are connected to, to notice it vanishing.
    connected_to: Option<String>,
    announced_missing: bool,
}

/// Every MIDI input port name on the system right now.
pub fn port_names() -> Vec<String> {
    let Ok(mi) = MidiInput::new("opendeck-scan") else { return Vec::new() };
    mi.ports().iter().filter_map(|p| mi.port_name(p).ok()).collect()
}

impl MidiSurface {
    pub fn new(s: Surface, deck: Deck, tx: Sender<Event>, ind: Arc<Indicators>) -> Self {
        let port = s.midi.as_ref().map(|m| m.port.clone()).unwrap_or_default();
        let mapper = Mapper::new(&s, deck);
        let leds = s.led.iter().filter(|l| l.deck.is_none_or(|d| d == deck)).cloned().collect::<Vec<_>>();
        let led_state = vec![None; leds.len()];
        MidiSurface { name: s.name, port, mapper: Arc::new(Mutex::new(mapper)), tx, ind, leds, led_state, input: None, output: None, connected_to: None, announced_missing: false }
    }

    /// Connect if the port is there and we are not; drop a connection whose
    /// port has gone.
    pub fn reconcile(&mut self, available: &[String]) {
        if let Some(cur) = &self.connected_to {
            if !available.iter().any(|n| n == cur) {
                log::warn!("surface {}: {cur} unplugged", self.name);
                self.input = None; self.output = None; self.connected_to = None;
                self.led_state.iter_mut().for_each(|s| *s = None);
            } else { return; }
        }
        let Some(name) = available.iter().find(|n| n.contains(&self.port)) else {
            if !self.announced_missing {
                log::info!("surface {}: no MIDI port containing {:?} yet{}", self.name, self.port,
                    if available.is_empty() { String::new() } else { format!(" (have: {})", available.join(", ")) });
                self.announced_missing = true;
            }
            return;
        };
        let name = name.clone();
        // Input.
        let Ok(mi) = MidiInput::new("opendeck") else { return };
        let Some(port) = mi.ports().into_iter().find(|p| mi.port_name(p).ok().as_deref() == Some(&name)) else { return };
        let mapper = Arc::clone(&self.mapper);
        let tx = self.tx.clone();
        let sname = self.name.clone();
        match mi.connect(&port, "opendeck-in", move |_ts, msg, _| {
            log::debug!("midi rx [{sname}]: {:02X?}", msg);
            let evs = {
                let Ok(mut m) = mapper.lock() else { return };
                decode(&mut m, msg)
            };
            for ev in evs { let _ = tx.send(ev); }
        }, ()) {
            Ok(c) => self.input = Some(c),
            Err(e) => { log::warn!("surface {}: cannot open {name}: {e}", self.name); return; }
        }
        // Output, for LEDs — optional; a controller without one is fine.
        if !self.leds.is_empty() {
            if let Ok(mo) = MidiOutput::new("opendeck") {
                if let Some(p) = mo.ports().into_iter().find(|p| mo.port_name(p).ok().as_deref() == Some(&name)) {
                    match mo.connect(&p, "opendeck-out") {
                        Ok(c) => self.output = Some(c),
                        Err(e) => log::warn!("surface {}: no LED output on {name}: {e}", self.name),
                    }
                }
            }
        }
        log::info!("surface {}: connected to {name}{}", self.name, if self.output.is_some() { " (with LED feedback)" } else { "" });
        self.connected_to = Some(name);
        self.announced_missing = false;
    }

    /// Send any LED whose state changed.
    pub fn refresh_leds(&mut self) {
        let Some(out) = self.output.as_mut() else { return };
        let shift = self.mapper.lock().map(|m| m.shift_held()).unwrap_or(false);
        let local = |i| match i { super::config::Indicator::Shift => Some(shift), super::config::Indicator::On => Some(true), _ => None };
        for (l, st) in self.leds.iter().zip(self.led_state.iter_mut()) {
            let want = self.ind.led_state(l.shows, l.blink_on, local);
            if *st == Some(want) { continue; }
            let v = if want { l.on } else { l.off };
            let msg = match (l.note, l.cc) {
                (Some(n), _) => [0x90 | (l.channel & 0x0F), n, v],
                (_, Some(c)) => [0xB0 | (l.channel & 0x0F), c, v],
                _ => continue,
            };
            if let Err(e) = out.send(&msg) { log::debug!("surface {}: LED send: {e}", self.name); }
            *st = Some(want);
        }
    }
}

/// One MIDI message → bus events.
pub fn decode(m: &mut Mapper, msg: &[u8]) -> Vec<Event> {
    if msg.len() < 3 { return Vec::new(); }
    let (status, ch, a, b) = (msg[0] & 0xF0, msg[0] & 0x0F, msg[1], msg[2]);
    match status {
        0x90 => m.note_on(ch, a, b),
        0x80 => m.note_off(ch, a),
        0xB0 => m.cc(ch, a, b),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    //! The DJ2Go preset must do exactly what the hand-written adapter it
    //! replaced did (docs/reference/dj2go-midi-map.md).
    use super::*;
    use super::super::config::SurfaceFile;
    use crate::input::ControlEvent as CE;

    fn dj2go(deck: Deck) -> Mapper {
        let text = super::super::PRESETS.iter().find(|(n, _)| *n == "dj2go").unwrap().1;
        Mapper::new(&SurfaceFile::parse(text).unwrap().surface[0], deck)
    }

    fn ev(m: &mut Mapper, msg: &[u8]) -> Option<CE> {
        let evs = decode(m, msg);
        assert!(evs.len() <= 1, "{evs:?}");
        evs.into_iter().find_map(|e| match e { Event::Deck(d) => Some(d), _ => None })
    }

    #[test]
    fn deck_a_controls() {
        let mut m = dj2go(Deck::A);
        assert!(matches!(ev(&mut m, &[0x90, 0x3B, 0x7F]), Some(CE::PlayPause)));
        assert!(matches!(ev(&mut m, &[0x90, 0x33, 0x7F]), Some(CE::Cue { pressed: true })));
        assert!(matches!(ev(&mut m, &[0x80, 0x33, 0x00]), Some(CE::Cue { pressed: false })));
        assert!(matches!(ev(&mut m, &[0x90, 0x40, 0x7F]), Some(CE::SyncToggle)));
        assert!(matches!(ev(&mut m, &[0x90, 0x44, 0x7F]), Some(CE::LoopIn)));
        assert!(matches!(ev(&mut m, &[0x90, 0x43, 0x7F]), Some(CE::LoopOut)));
        assert!(matches!(ev(&mut m, &[0x80, 0x4B, 0x00]), Some(CE::Load)));           // load fires on Note Off
        assert!(ev(&mut m, &[0x90, 0x4B, 0x7F]).is_none());                           // …its Note On is the (unused) shift
        assert!(matches!(ev(&mut m, &[0xB0, 0x19, 5]),    Some(CE::JogDelta { delta: 5, .. })));
        assert!(matches!(ev(&mut m, &[0xB0, 0x19, 123]),  Some(CE::JogDelta { delta: -5, .. })));
        assert!(matches!(ev(&mut m, &[0xB0, 0x0D, 0]),    Some(CE::TempoFader { position }) if position > 0.99));
        assert!(matches!(ev(&mut m, &[0xB0, 0x0D, 64]),   Some(CE::TempoFader { position }) if (position - 0.5).abs() < 1e-6));
        assert!(matches!(ev(&mut m, &[0xB0, 0x0D, 63]),   Some(CE::TempoFader { position }) if (position - 0.5).abs() < 1e-6));
    }

    #[test]
    fn deck_b_uses_its_own_numbers() {
        let mut m = dj2go(Deck::B);
        assert!(matches!(ev(&mut m, &[0x90, 0x42, 0x7F]), Some(CE::PlayPause)));
        assert!(matches!(ev(&mut m, &[0x90, 0x3C, 0x7F]), Some(CE::Cue { pressed: true })));
        assert!(matches!(ev(&mut m, &[0x90, 0x47, 0x7F]), Some(CE::SyncToggle)));
        assert!(matches!(ev(&mut m, &[0xB0, 0x18, 5]),    Some(CE::JogDelta { delta: 5, .. })));
        assert!(matches!(ev(&mut m, &[0xB0, 0x0E, 64]),   Some(CE::TempoFader { .. })));
        assert!(ev(&mut m, &[0x90, 0x3B, 0x7F]).is_none());                           // deck A's play
    }

    #[test]
    fn browse_controls_are_shared() {
        for d in [Deck::A, Deck::B] {
            let mut m = dj2go(d);
            assert!(matches!(ev(&mut m, &[0xB0, 0x1A, 1]),   Some(CE::BrowseEncoderDelta { delta:  1 })));
            assert!(matches!(ev(&mut m, &[0xB0, 0x1A, 127]), Some(CE::BrowseEncoderDelta { delta: -1 })));
            assert!(matches!(ev(&mut m, &[0x80, 0x59, 0]),   Some(CE::Back)));
            assert!(matches!(ev(&mut m, &[0x80, 0x5A, 0]),   Some(CE::Load)));
        }
    }
}
