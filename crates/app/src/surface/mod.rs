//! Control surfaces: whatever is plugged into this deck besides the screen.
//!
//! A USB MIDI controller, a PCF8574 button board on the Pi's I2C bus, both
//! at once — each is a `[[surface]]` in `controllers.toml` ([`config`]).  A
//! transport ([`midi`], [`i2c`]) turns its traffic into presses, releases,
//! deltas and positions; the [`map::Mapper`] turns those into bus
//! [`Event`]s from the surface's bindings; `DeckApp::apply` does the rest.
//! The other way, deck state comes back as [`Indicators`] bits the
//! transports write to their LEDs.
//!
//! Built-in presets (`surfaces/*.toml` beside this crate) are always loaded;
//! the user's file, `controllers.toml` in the app data dir or
//! `--controllers PATH`, is merged over them by surface name.
//! Reference: docs/hardware/control-surfaces.md.

pub mod config;
pub mod i2c;
pub mod map;
pub mod midi;

use crate::input::Event;
use config::{Deck, Indicator, SurfaceFile};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Built-in surface definitions, compiled in.
pub const PRESETS: &[(&str, &str)] = &[
    ("dj2go", include_str!("../../surfaces/dj2go.toml")),
];

/// Deck state the surfaces mirror on their LEDs.  `DeckApp` stores a fresh
/// set of bits once per frame; transports read them at their own pace.
#[derive(Default)]
pub struct Indicators { bits: AtomicU32 }

pub mod bit {
    pub const PLAYING:  u32 = 1 << 0;
    pub const CUED:     u32 = 1 << 1;
    pub const SYNC:     u32 = 1 << 2;
    pub const MASTER:   u32 = 1 << 3;
    pub const LOOP:     u32 = 1 << 4;
    pub const SLIP:     u32 = 1 << 5;
    pub const KEY_LOCK: u32 = 1 << 6;
    pub const VINYL:    u32 = 1 << 7;
    pub const LINKED:   u32 = 1 << 8;
    /// Hot cue slot `n` is set: `HOT_CUE0 << n`.
    pub const HOT_CUE0: u32 = 1 << 16;
}

impl Indicators {
    pub fn store(&self, bits: u32) { self.bits.store(bits, Ordering::Relaxed); }
    pub fn load(&self) -> u32 { self.bits.load(Ordering::Relaxed) }

    /// Is `ind` true right now?  `local` answers the indicators only the
    /// surface itself knows (SHIFT, the power light).
    pub fn is_on(&self, ind: Indicator, local: impl Fn(Indicator) -> Option<bool>) -> bool {
        if let Some(v) = local(ind) { return v; }
        let b = self.load();
        let on = |m: u32| b & m != 0;
        match ind {
            Indicator::Playing     => on(bit::PLAYING),
            Indicator::Paused      => !on(bit::PLAYING),
            Indicator::Cue         => on(bit::CUED) && !on(bit::PLAYING),
            Indicator::CueSettable => !on(bit::CUED) && !on(bit::PLAYING),
            Indicator::Sync        => on(bit::SYNC),
            Indicator::Master      => on(bit::MASTER),
            Indicator::Loop        => on(bit::LOOP),
            Indicator::Slip        => on(bit::SLIP),
            Indicator::KeyLock     => on(bit::KEY_LOCK),
            Indicator::Vinyl       => on(bit::VINYL),
            Indicator::Linked      => on(bit::LINKED),
            Indicator::HotCue0 => on(bit::HOT_CUE0), Indicator::HotCue1 => on(bit::HOT_CUE0 << 1),
            Indicator::HotCue2 => on(bit::HOT_CUE0 << 2), Indicator::HotCue3 => on(bit::HOT_CUE0 << 3),
            Indicator::HotCue4 => on(bit::HOT_CUE0 << 4), Indicator::HotCue5 => on(bit::HOT_CUE0 << 5),
            Indicator::HotCue6 => on(bit::HOT_CUE0 << 6), Indicator::HotCue7 => on(bit::HOT_CUE0 << 7),
            Indicator::Shift => false,
            Indicator::On    => true,
        }
    }

    /// What a LED should show: `blink_on` wins and blinks at 2 Hz, else
    /// `shows` steady.
    pub fn led_state(&self, shows: Indicator, blink_on: Option<Indicator>, local: impl Fn(Indicator) -> Option<bool> + Copy) -> bool {
        if let Some(b) = blink_on {
            if self.is_on(b, local) { return blink_phase(); }
        }
        self.is_on(shows, local)
    }
}

/// 2 Hz square wave shared by every blinking LED, so they blink together.
pub fn blink_phase() -> bool {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let t = EPOCH.get_or_init(Instant::now).elapsed().as_millis();
    (t / 250).is_multiple_of(2)
}

/// Load the presets, then the user's file over them.  `explicit` is
/// `--controllers PATH` / `OPENDECK_CONTROLLERS`; otherwise the data dir's
/// `controllers.toml` is used when it exists.  A broken user file is
/// reported and skipped — the presets still work.
pub fn load(explicit: Option<&Path>) -> SurfaceFile {
    let mut file = SurfaceFile::default();
    for (name, text) in PRESETS {
        match SurfaceFile::parse(text) {
            Ok(f) => file.merge(f),
            Err(e) => log::error!("surface preset {name}: {e}"),   // a build bug, caught by tests
        }
    }
    let path: Option<PathBuf> = explicit.map(Path::to_path_buf).or_else(|| {
        let p = default_path();
        p.is_file().then_some(p)
    });
    if let Some(p) = path {
        match std::fs::read_to_string(&p) {
            Ok(text) => match SurfaceFile::parse(&text) {
                Ok(f) => { log::info!("controllers: {} surface(s) from {}", f.surface.len(), p.display()); file.merge(f); }
                Err(e) => log::error!("controllers: {}: {e}", p.display()),
            },
            Err(e) => log::error!("controllers: cannot read {}: {e}", p.display()),
        }
    }
    file
}

pub fn default_path() -> PathBuf { crate::taglist::app_data_dir().join("controllers.toml") }

/// Bring every surface up.  MIDI surfaces are (re)connected as their ports
/// appear; each I2C board gets its own polling thread.  Returns at once.
pub fn start(file: SurfaceFile, deck: Deck, tx: Sender<Event>, ind: Arc<Indicators>) {
    let mut midis = Vec::new();
    for s in file.surface {
        if s.i2c.is_some() {
            i2c::spawn(s, deck, tx.clone(), Arc::clone(&ind));
        } else if s.midi.is_some() {
            midis.push(midi::MidiSurface::new(s, deck, tx.clone(), Arc::clone(&ind)));
        }
    }
    if midis.is_empty() { return; }
    let r = std::thread::Builder::new().name("surfaces".into()).spawn(move || {
        // 25 Hz: LED refresh every tick, port scan every 2 s.
        let tick = Duration::from_millis(40);
        let mut n: u32 = 0;
        loop {
            if n.is_multiple_of(50) {
                let avail = midi::port_names();
                for m in &mut midis { m.reconcile(&avail); }
            }
            for m in &mut midis { m.refresh_leds(); }
            n = n.wrapping_add(1);
            std::thread::sleep(tick);
        }
    });
    if let Err(e) = r { log::warn!("surfaces: {e}"); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_parse() {
        for (name, text) in PRESETS {
            SurfaceFile::parse(text).unwrap_or_else(|e| panic!("preset {name}: {e}"));
        }
    }

    /// The example files in the guide must parse: a reader will copy them.
    #[test]
    fn documented_examples_parse() {
        for (name, text) in [
            ("pcf8574-transport", include_str!("../../../../docs/hardware/surfaces/pcf8574-transport.toml")),
            ("pcf8574-pads",      include_str!("../../../../docs/hardware/surfaces/pcf8574-pads.toml")),
            ("generic-midi",      include_str!("../../../../docs/hardware/surfaces/generic-midi.toml")),
        ] {
            let f = SurfaceFile::parse(text).unwrap_or_else(|e| panic!("example {name}: {e}"));
            assert_eq!(f.surface.len(), 1, "{name}");
        }
        // …and two of them together, as one controllers.toml.
        let both = format!("{}\n{}", include_str!("../../../../docs/hardware/surfaces/pcf8574-transport.toml"),
                                      include_str!("../../../../docs/hardware/surfaces/pcf8574-pads.toml"));
        assert_eq!(SurfaceFile::parse(&both).unwrap().surface.len(), 2);
    }

    #[test]
    fn indicator_logic() {
        let i = Indicators::default();
        let none = |_| None;
        i.store(bit::CUED);
        assert!(i.is_on(Indicator::Cue, none));
        assert!(i.is_on(Indicator::Paused, none));
        assert!(!i.is_on(Indicator::CueSettable, none));
        i.store(bit::PLAYING | bit::CUED);
        assert!(!i.is_on(Indicator::Cue, none));          // playing: CUE lamp off
        i.store(0);
        assert!(i.is_on(Indicator::CueSettable, none));
        i.store(bit::HOT_CUE0 << 3);
        assert!(i.is_on(Indicator::HotCue3, none));
        assert!(!i.is_on(Indicator::HotCue2, none));
        assert!(i.is_on(Indicator::Shift, |x| (x == Indicator::Shift).then_some(true)));
    }
}
