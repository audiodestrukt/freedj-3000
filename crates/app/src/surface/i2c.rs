//! I2C surfaces: a PCF8574 board on the Pi's header bus.
//!
//! The PCF8574 is the simplest expander there is: no registers, one byte
//! read gives the eight pin levels, one byte written sets them.  A written 1
//! is a weak pull-up (so the pin is an input: a button to ground reads 0
//! when pressed), a written 0 sinks hard (a LED from Vcc through a resistor
//! lights).  So one board carries buttons, encoders and LEDs on the same
//! eight pins, and this thread does both directions per poll:
//!
//!   read byte → debounce buttons, decode quadrature → events on the bus
//!   indicators → LED bits → write byte (only when it changed)
//!
//! Polling, not the INT pin.  The PCF8574 is a 100 kHz part, so a one-byte
//! read is ~0.25 ms of bus: one board at the default 500 Hz uses an eighth
//! of the bus and nothing of a Pi core; the bus as a whole has ~4000 reads/s
//! to share between boards (up to eight by address, A0–A2).  Linux only;
//! elsewhere the surface logs and does nothing.

use super::config::{Deck, Indicator, Surface};
use super::map::{Key, Mapper};
use super::Indicators;
use crate::input::Event;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Gray-code transition table: index (prev << 2 | cur) → step.
const QUAD: [i8; 16] = [0, -1, 1, 0, 1, 0, 0, -1, -1, 0, 0, 1, 0, 1, -1, 0];

struct Button { pin: u8, stable: bool, cand: bool, run: u32 }
struct Encoder { a: u8, b: u8, prev: u8, acc: i32, steps: i32 }
struct Led { pin: u8, active_low: bool, shows: Indicator, blink_on: Option<Indicator> }

/// The pure part: pin levels in, verbs out.  Owns no bus so it can be tested.
pub struct Board {
    active_low: bool,
    debounce_runs: u32,
    buttons: Vec<Button>,
    encoders: Vec<Encoder>,
    leds: Vec<Led>,
    /// Pins we read (buttons, encoders): written as 1.
    input_mask: u8,
    /// Last byte written.
    shadow: u8,
}

impl Board {
    pub fn new(s: &Surface, deck: Deck) -> Self {
        let t = s.i2c.as_ref().expect("i2c surface");
        let ok = |d: Option<Deck>| d.is_none_or(|d| d == deck);
        let buttons: Vec<Button> = s.button.iter().filter(|b| ok(b.deck)).filter_map(|b| b.pin)
            .fold(Vec::new(), |mut v, pin| { if !v.iter().any(|b: &Button| b.pin == pin) { v.push(Button { pin, stable: false, cand: false, run: 0 }); } v });
        let encoders: Vec<Encoder> = s.encoder.iter().filter(|e| ok(e.deck)).filter_map(|e| e.pins.map(|p| (p, e.steps_per_detent)))
            .fold(Vec::new(), |mut v, ([a, b], steps)| { if !v.iter().any(|e: &Encoder| e.a == a && e.b == b) { v.push(Encoder { a, b, prev: 0, acc: 0, steps: steps as i32 }); } v });
        let leds = s.led.iter().filter(|l| ok(l.deck)).filter_map(|l| l.pin.map(|pin| Led { pin, active_low: l.active_low, shows: l.shows, blink_on: l.blink_on })).collect();
        let mut input_mask = 0u8;
        for b in &buttons { input_mask |= 1 << b.pin; }
        for e in &encoders { input_mask |= (1 << e.a) | (1 << e.b); }
        let debounce_runs = ((t.debounce_ms as u64 * t.poll_hz as u64).div_ceil(1000) as u32).max(1);
        let mut board = Board { active_low: t.active_low, debounce_runs, buttons, encoders, leds, input_mask, shadow: 0 };
        board.shadow = board.output_byte(|_, _| false);
        board
    }

    /// The first read: take the pins as they are, so a button held at power-on
    /// does not fire and the encoders start from their real phase.
    pub fn prime(&mut self, raw: u8) {
        for b in &mut self.buttons { let p = self.active_low ^ (raw >> b.pin & 1 == 1); b.stable = p; b.cand = p; b.run = 0; }
        for e in &mut self.encoders { e.prev = quad_state(raw, e.a, e.b); e.acc = 0; }
    }

    /// One poll's pin levels → events, via the mapper.
    pub fn sample(&mut self, raw: u8, m: &mut Mapper) -> Vec<Event> {
        let mut out = Vec::new();
        for b in &mut self.buttons {
            let pressed = self.active_low ^ (raw >> b.pin & 1 == 1);
            if pressed == b.stable { b.cand = pressed; b.run = 0; continue; }
            if pressed == b.cand { b.run += 1; } else { b.cand = pressed; b.run = 1; }
            if b.run >= self.debounce_runs {
                b.stable = pressed; b.run = 0;
                out.extend(if pressed { m.press(Key::Pin(b.pin), false) } else { m.release(Key::Pin(b.pin)) });
            }
        }
        for e in &mut self.encoders {
            let cur = quad_state(raw, e.a, e.b);
            if cur != e.prev {
                e.acc += QUAD[((e.prev << 2) | cur) as usize] as i32;
                e.prev = cur;
                while e.acc >= e.steps  { e.acc -= e.steps; out.extend(m.delta(Key::Pins(e.a, e.b),  1)); }
                while e.acc <= -e.steps { e.acc += e.steps; out.extend(m.delta(Key::Pins(e.a, e.b), -1)); }
            }
        }
        out
    }

    /// The byte to write: inputs high, LEDs as asked, unused pins high.
    pub fn output_byte(&self, lit: impl Fn(Indicator, Option<Indicator>) -> bool) -> u8 {
        let mut byte = 0xFF;
        for l in &self.leds {
            let on = lit(l.shows, l.blink_on);
            let level = on ^ l.active_low;     // active_low: on = 0
            if level { byte |= 1 << l.pin; } else { byte &= !(1 << l.pin); }
        }
        byte | self.input_mask
    }

    pub fn has_leds(&self) -> bool { !self.leds.is_empty() }
}

fn quad_state(raw: u8, a: u8, b: u8) -> u8 { ((raw >> a & 1) << 1) | (raw >> b & 1) }

/// Start this board's thread.
pub fn spawn(s: Surface, deck: Deck, tx: Sender<Event>, ind: Arc<Indicators>) {
    let name = s.name.clone();
    let r = std::thread::Builder::new().name(format!("i2c-{name}")).spawn(move || run(s, deck, tx, ind));
    if let Err(e) = r { log::warn!("surface {name}: {e}"); }
}

#[cfg(target_os = "linux")]
fn run(s: Surface, deck: Deck, tx: Sender<Event>, ind: Arc<Indicators>) {
    let t = s.i2c.clone().expect("i2c surface");
    let mut board = Board::new(&s, deck);
    let mut mapper = Mapper::new(&s, deck);
    let period = Duration::from_secs_f64(1.0 / t.poll_hz as f64);
    let mut failed_since: Option<Instant> = None;
    loop {
        let mut bus = match Bus::open(&t.bus, t.address) {
            Ok(b) => b,
            Err(e) => {
                if failed_since.is_none() {
                    log::warn!("surface {}: cannot open {} @0x{:02X}: {e} (is I2C enabled and are you in the i2c group?) — retrying", s.name, t.bus, t.address);
                    failed_since = Some(Instant::now());
                }
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        // Set the pins up and take the first reading.
        let init = board.output_byte(|_, _| false);
        if let Err(e) = bus.write_byte(init).and_then(|_| bus.read_byte()).map(|raw| board.prime(raw)) {
            if failed_since.is_none() {
                log::warn!("surface {}: {} @0x{:02X}: {e} — no device answering (check wiring, address pins, `i2cdetect -y 1`); retrying quietly", s.name, t.bus, t.address);
                failed_since = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        board.shadow = init;
        log::info!("surface {}: {} @0x{:02X} ({:?}), {} button(s) {} encoder(s) {} LED(s), {} Hz{}",
            s.name, t.bus, t.address, t.chip, board.buttons.len(), board.encoders.len(), board.leds.len(), t.poll_hz,
            failed_since.map(|t0| format!(" — up after {:.0} s", t0.elapsed().as_secs_f32())).unwrap_or_default());
        failed_since = None;
        let mut next = Instant::now();
        let mut last_raw: Option<u8> = None;
        loop {
            next += period;
            let raw = match bus.read_byte() {
                Ok(r) => r,
                Err(e) => { log::warn!("surface {}: read failed: {e} — reopening", s.name); break; }
            };
            if last_raw != Some(raw) { log::debug!("i2c rx [{}]: {raw:08b}", s.name); last_raw = Some(raw); }
            for ev in board.sample(raw, &mut mapper) { let _ = tx.send(ev); }
            if board.has_leds() {
                let shift = mapper.shift_held();
                let local = |i| match i { Indicator::Shift => Some(shift), Indicator::On => Some(true), _ => None };
                let byte = board.output_byte(|shows, blink| ind.led_state(shows, blink, local));
                if byte != board.shadow {
                    if let Err(e) = bus.write_byte(byte) { log::warn!("surface {}: write failed: {e} — reopening", s.name); break; }
                    board.shadow = byte;
                }
            }
            let now = Instant::now();
            if next > now { std::thread::sleep(next - now); } else { next = now; }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(not(target_os = "linux"))]
fn run(s: Surface, _deck: Deck, _tx: Sender<Event>, _ind: Arc<Indicators>) {
    log::warn!("surface {}: I2C surfaces need Linux (/dev/i2c-*); skipped", s.name);
}

/// `/dev/i2c-N` with one slave address selected: the kernel's i2c-dev
/// interface, no library needed.
#[cfg(target_os = "linux")]
struct Bus { file: std::fs::File }

#[cfg(target_os = "linux")]
impl Bus {
    const I2C_SLAVE: libc::c_ulong = 0x0703;

    fn open(path: &str, addr: u8) -> std::io::Result<Self> {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let r = unsafe { libc::ioctl(file.as_raw_fd(), Self::I2C_SLAVE as _, addr as libc::c_ulong) };
        if r < 0 { return Err(std::io::Error::last_os_error()); }
        Ok(Bus { file })
    }

    fn read_byte(&mut self) -> std::io::Result<u8> {
        use std::io::Read;
        let mut b = [0u8; 1];
        match self.file.read(&mut b)? {
            1 => Ok(b[0]),
            n => Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, format!("read {n} bytes"))),
        }
    }

    fn write_byte(&mut self, b: u8) -> std::io::Result<()> {
        use std::io::Write;
        self.file.write_all(&[b])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::config::SurfaceFile;
    use crate::input::ControlEvent as CE;

    const BOARD: &str = r#"
        [[surface]]
        name = "b"
        [surface.i2c]
        address = 0x20
        poll_hz = 1000
        debounce_ms = 3
        [[surface.button]]
        pin = 0
        action = "play_pause"
        [[surface.button]]
        pin = 1
        action = "cue"
        [[surface.encoder]]
        pins = [2, 3]
        action = "browse"
        steps_per_detent = 4
        [[surface.led]]
        pin = 6
        shows = "playing"
        [[surface.led]]
        pin = 7
        shows = "sync"
        active_low = false
    "#;

    fn setup() -> (Board, Mapper) {
        let f = SurfaceFile::parse(BOARD).unwrap();
        let s = &f.surface[0];
        let mut b = Board::new(s, Deck::A);
        b.prime(0xFF);   // nothing pressed (active low), encoder at 11
        (b, Mapper::new(s, Deck::A))
    }

    fn decks(evs: Vec<Event>) -> Vec<CE> { evs.into_iter().filter_map(|e| match e { Event::Deck(d) => Some(d), _ => None }).collect() }

    #[test]
    fn buttons_debounce_then_fire_once() {
        let (mut b, mut m) = setup();
        assert_eq!(b.debounce_runs, 3);
        // Pin 0 goes low: two samples are not enough, the third fires.
        assert!(b.sample(0xFE, &mut m).is_empty());
        assert!(b.sample(0xFE, &mut m).is_empty());
        assert!(matches!(decks(b.sample(0xFE, &mut m))[..], [CE::PlayPause]));
        assert!(b.sample(0xFE, &mut m).is_empty());                        // held: nothing more
        // A 1-sample glitch high is ignored.
        assert!(b.sample(0xFF, &mut m).is_empty());
        assert!(b.sample(0xFE, &mut m).is_empty());
        assert!(b.sample(0xFE, &mut m).is_empty());
        // Release, CUE on pin 1 with its release.
        for _ in 0..3 { b.sample(0xFF, &mut m); }
        for _ in 0..2 { assert!(b.sample(0xFD, &mut m).is_empty()); }
        assert!(matches!(decks(b.sample(0xFD, &mut m))[..], [CE::Cue { pressed: true }]));
        for _ in 0..2 { assert!(b.sample(0xFF, &mut m).is_empty()); }
        assert!(matches!(decks(b.sample(0xFF, &mut m))[..], [CE::Cue { pressed: false }]));
    }

    #[test]
    fn held_at_power_on_does_not_fire() {
        let f = SurfaceFile::parse(BOARD).unwrap();
        let mut b = Board::new(&f.surface[0], Deck::A);
        let mut m = Mapper::new(&f.surface[0], Deck::A);
        b.prime(0xFE);                                                      // play held at boot
        for _ in 0..5 { assert!(b.sample(0xFE, &mut m).is_empty()); }
        for _ in 0..3 { b.sample(0xFF, &mut m); }                           // release: nothing (play_pause has no release)
        for _ in 0..2 { b.sample(0xFE, &mut m); }
        assert!(matches!(decks(b.sample(0xFE, &mut m))[..], [CE::PlayPause]));
    }

    #[test]
    fn quadrature_one_detent_per_four_transitions() {
        let (mut b, mut m) = setup();
        // A = pin 2, B = pin 3.  Clockwise Gray sequence 11 → 01 → 00 → 10 → 11
        // in (A,B) terms, keeping the button pins high.
        let seq = |a: u8, bb: u8| 0xFF & !((1 - a) << 2) & !((1 - bb) << 3);
        let mut evs = Vec::new();
        for (a, bb) in [(0, 1), (0, 0), (1, 0), (1, 1)] { evs.extend(b.sample(seq(a, bb), &mut m)); }
        assert!(matches!(decks(evs)[..], [CE::BrowseEncoderDelta { delta: 1 }]));
        let mut evs = Vec::new();
        for (a, bb) in [(1, 0), (0, 0), (0, 1), (1, 1)] { evs.extend(b.sample(seq(a, bb), &mut m)); }
        assert!(matches!(decks(evs)[..], [CE::BrowseEncoderDelta { delta: -1 }]));
        // Half a detent and back: nothing.
        let mut evs = Vec::new();
        for (a, bb) in [(0, 1), (0, 0), (0, 1), (1, 1)] { evs.extend(b.sample(seq(a, bb), &mut m)); }
        assert!(evs.is_empty());
    }

    #[test]
    fn output_byte_keeps_inputs_high_and_drives_leds() {
        let (b, _) = setup();
        assert_eq!(b.input_mask, 0b0000_1111);
        // Nothing lit: pin 6 (active low) idles high, pin 7 (active high) idles low.
        assert_eq!(b.output_byte(|_, _| false), 0b0111_1111);
        // Playing lit → pin 6 low; sync lit → pin 7 high.
        assert_eq!(b.output_byte(|s, _| s == Indicator::Playing), 0b0011_1111);
        assert_eq!(b.output_byte(|s, _| s == Indicator::Sync),    0b1111_1111);
    }
}
