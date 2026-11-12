//! CDJ-3000 sub-CPU ("subucom") over SPI: the deck's own panel as a surface.
//!
//! On a CDJ-3000 every button, the jog, the tempo fader, the browse rotary
//! and every LED sit behind a sub-CPU that the main SoC polls over SPI at
//! about 850 Hz.  The player app sees it as a char device,
//! `/dev/subucom_spi1.0`: each `read()` returns one 64-byte input frame
//! (MISO), and the `MOSI_TRANSFER` ioctl hands it one 64-byte LED frame.
//! cdj3k-emu replaces the sub-CPU with the same device (its `subucom_virt`
//! kernel module, fed by the emulator's drawn panel), so this one transport
//! serves the emulator and a real deck alike.
//!
//! Frame layout as published by cdj3k-emu (`docs/subucom.md` there, MIT /
//! Apache-2.0): 64 bytes, `01 04` at bytes 2–3, CRC-16/X-25 over 0..62
//! little-endian at 62.  MISO: button bitmasks at 5..12, rotary counter at
//! 14, LCD touch at 16, tempo slider at 22, jog position / velocity / touch
//! at 26..31.  MOSI: single-bit and two-step LEDs at 2..9, the hot-cue pads'
//! RGB at 12..36.
//!
//! The keys go onto the bus by their cdj3k-emu names
//! ([`UiEvent::PanelKey`]): `DeckApp` runs them through the same CDJ-3000
//! key logic as the on-screen faceplate and the dev script (tap, long press,
//! SEARCH scanning while held, CALL/DELETE + pad).  Jog, fader and rotary
//! become deck events here.  The LEDs come back as a frame `DeckApp` builds
//! from the faceplate's lamp rules ([`mosi_frame`]) and parks in [`Panel`].
//!
//! Enabled out of band, like the CDJ HID surface:
//!   OPENDECK_SUBUCOM = /dev/subucom_spi1.0
//! Linux only.

use crate::cdjface::{Key, Lamp};
use crate::input::{ControlEvent as CE, Event, UiEvent};
use crate::screen::ScreenView;
use crate::snapshot::DeckSnapshot;
use super::lanhid::JogScaler;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub const FRAME: usize = 64;

/// Jog position counts per platter turn, as cdj3k-emu's panel encodes them.
const JOG_TICKS_PER_REV: f64 = 3240.0;
/// Tempo slider raw value at the centre detent (the deck's own, short of
/// 0x7FFF); the dead band runs to about 0x7FD0.
const TEMPO_CENTER: u16 = 0x7F50;
const TEMPO_DEAD_TOP: u16 = 0x7FD0;
/// Fader moves smaller than this many raw counts are noise, not a move.
const TEMPO_HYSTERESIS: u16 = 8;

/// Every key the panel reports, by the name cdj3k-emu gives it (which is
/// also what [`Key::from_emu_name`] and the dev script take).
const KEYS: &[(&str, usize, u8)] = &[
    ("PLAY", 5, 0x01), ("CUE", 5, 0x02), ("SEARCH_NEXT", 5, 0x04), ("SEARCH_PREV", 5, 0x08),
    ("TRACK_NEXT", 5, 0x10), ("TRACK_PREV", 5, 0x20), ("BEATJUMP_NEXT", 5, 0x40), ("BEATJUMP_PREV", 5, 0x80),
    ("TEMPO_RESET", 6, 0x01), ("MASTER_TEMPO", 6, 0x02), ("TEMPO_RANGE", 6, 0x04),
    ("KEY_SYNC", 6, 0x10), ("BEAT_SYNC", 6, 0x20), ("MASTER", 6, 0x40),
    ("LOOP_IN", 7, 0x01), ("LOOP_OUT", 7, 0x02), ("RELOOP", 7, 0x04),
    ("BEATLOOP_HALF", 7, 0x10), ("BEATLOOP_2X", 7, 0x20), ("SLIP", 7, 0x80),
    ("MEMORY", 8, 0x01), ("DELETE", 8, 0x02), ("CALL_NEXT", 8, 0x04), ("CALL_PREV", 8, 0x08), ("CALL_DELETE", 8, 0x20),
    ("HOT_A", 9, 0x01), ("HOT_B", 9, 0x02), ("HOT_C", 9, 0x04), ("HOT_D", 9, 0x08),
    ("HOT_E", 9, 0x10), ("HOT_F", 9, 0x20), ("HOT_G", 9, 0x40), ("HOT_H", 9, 0x80),
    ("SOURCE", 10, 0x01), ("BROWSE", 10, 0x02), ("TAG_LIST", 10, 0x04), ("PLAYLIST", 10, 0x08),
    ("SEARCH_MENU", 10, 0x10), ("MENU", 10, 0x20), ("JOG_MODE", 10, 0x80),
    ("BACK", 11, 0x01), ("TAG_TRACK", 11, 0x02), ("TRACK_FILTER", 11, 0x04), ("SHORTCUT", 11, 0x08),
    ("ROTARY_PRESS", 11, 0x10), ("TIME_MODE", 11, 0x40), ("QUANTIZE", 11, 0x80),
];

mod at {
    pub const ROTARY: usize = 14;
    pub const TEMPO:  usize = 22;
    pub const JOG:    usize = 26;     // position u16, velocity u16, touch u8
}

/// CRC-16/X-25: reflected 0x1021, init and final XOR 0xFFFF.
pub fn crc16_x25(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    !crc
}

fn u16le(f: &[u8; FRAME], o: usize) -> u16 { u16::from_le_bytes([f[o], f[o + 1]]) }

/// An input frame we trust: right header, right CRC.
pub fn valid(f: &[u8; FRAME]) -> bool {
    f[2] == 0x01 && f[3] == 0x04 && crc16_x25(&f[..62]) == u16le(f, 62)
}

/// The tempo slider's raw value → the deck's fader position (0 = bottom,
/// fastest; 1 = top).  The slider reads 0 at the top and 0xFFFF at the
/// bottom, with the centre detent off the arithmetic middle, so each half is
/// scaled on its own and the dead band reads exactly 0 %.
pub fn tempo_position(raw: u16) -> f32 {
    let travel = if raw <= TEMPO_CENTER {
        0.5 * raw as f32 / TEMPO_CENTER as f32
    } else if raw <= TEMPO_DEAD_TOP {
        0.5
    } else {
        0.5 + 0.5 * (raw - TEMPO_DEAD_TOP) as f32 / (0xFFFF - TEMPO_DEAD_TOP) as f32
    };
    1.0 - travel
}

/// A hand on the platter: the jog touch byte's low two bits both set
/// (0x03 still, 0x07 forward, 0x0b back; 0x00 / 0x04 / 0x08 / 0x0c free).
fn jog_touched(b: u8) -> bool { b & 0x03 == 0x03 }

/// MISO frames in, bus events out.  Holds the previous frame so each key
/// edge, detent and jog count is reported once.
pub struct Decoder {
    prev: Option<[u8; FRAME]>,
    jog: JogScaler,
    tempo_sent: u16,
    pub bad_frames: u64,
}

impl Default for Decoder {
    fn default() -> Self { Self::new() }
}

impl Decoder {
    pub fn new() -> Self {
        Decoder { prev: None, jog: JogScaler::per_rev(JOG_TICKS_PER_REV), tempo_sent: 0, bad_frames: 0 }
    }

    pub fn feed(&mut self, f: &[u8; FRAME]) -> Vec<Event> {
        let mut out = Vec::new();
        if !valid(f) { self.bad_frames += 1; return out; }
        let tempo = u16le(f, at::TEMPO);
        let Some(p) = self.prev.replace(*f) else {
            // First frame: take every key as it is (one held at start-up does
            // not fire), but the fader is where the hand left it, so the deck
            // follows it at once.
            self.tempo_sent = tempo;
            out.push(Event::Deck(CE::TempoFader { position: tempo_position(tempo) }));
            return out;
        };
        let deck = |e| Event::Deck(e);

        for &(key, byte, mask) in KEYS {
            let (was, is) = (p[byte] & mask != 0, f[byte] & mask != 0);
            if was != is { out.push(Event::Ui(UiEvent::PanelKey { key, pressed: is })); }
        }

        let j = at::JOG;
        let (was_t, is_t) = (jog_touched(p[j + 4]), jog_touched(f[j + 4]));
        if was_t != is_t { out.push(deck(CE::JogTouch { touched: is_t })); }
        let moved = u16le(f, j).wrapping_sub(u16le(&p, j)) as i16 as i32;
        if moved != 0 {
            match self.jog.step(moved) {
                0 => {}
                d => out.push(deck(CE::JogDelta { delta: d, velocity_rpm: 0.0 })),
            }
        }

        // Counter up = one detent clockwise = down the list, as on the unit.
        let turned = u16le(f, at::ROTARY).wrapping_sub(u16le(&p, at::ROTARY)) as i16 as i32;
        if turned != 0 { out.push(deck(CE::BrowseEncoderDelta { delta: turned })); }

        if tempo.abs_diff(self.tempo_sent) >= TEMPO_HYSTERESIS
            || (tempo != self.tempo_sent && (tempo == 0 || tempo == 0xFFFF))
        {
            self.tempo_sent = tempo;
            out.push(deck(CE::TempoFader { position: tempo_position(tempo) }));
        }
        out
    }
}

// ── LEDs ──────────────────────────────────────────────────────────────────

/// A two-step lamp: `medium` alone is dim, `medium | full` is bright.
const STEP: &[(Key, usize, u8, u8)] = &[
    (Key::Slip, 3, 0x10, 0x30), (Key::Quantize, 3, 0x40, 0xc0),
    (Key::Source, 4, 0x01, 0x03), (Key::Browse, 4, 0x04, 0x0c),
    (Key::TagList, 4, 0x10, 0x30), (Key::Playlist, 4, 0x40, 0xc0),
    (Key::Search, 5, 0x01, 0x03), (Key::Menu, 5, 0x04, 0x0c),
];
/// A one-bit lamp (a blinking one follows the shared 2 Hz phase).
const BIT: &[(Key, usize, u8)] = &[
    (Key::KeySync, 2, 0x03), (Key::BeatSync, 2, 0x08), (Key::Master, 2, 0x20),
    (Key::LoopIn, 7, 0x08), (Key::LoopOut, 7, 0x10), (Key::Reloop, 7, 0x20),
    (Key::Loop4, 7, 0x40), (Key::Loop8, 7, 0x80),
    (Key::JumpNext, 8, 0x01), (Key::JumpPrev, 8, 0x02), (Key::TempoReset, 8, 0x08),
    (Key::TrackPrev, 9, 0x04),
];
const PLAY: (usize, u8) = (7, 0x01);
const CUE: (usize, u8) = (7, 0x02);
const MASTER_TEMPO: (usize, u8) = (8, 0x10);
const JOG_CDJ: (usize, u8) = (8, 0x40);
const JOG_VINYL: (usize, u8) = (8, 0x80);
/// The jog ring: dim and bright bits.
const JOG_RING: (usize, u8, u8) = (3, 0x04, 0x08);
/// The rotary's own light.
const ENCODER: (usize, u8) = (9, 0x01);
/// HOT CUE A–H, an (R, G, B) PWM triple each.
const PADS: [usize; 8] = [12, 15, 18, 21, 24, 27, 30, 33];

/// The LED frame for the deck as it is now, by the faceplate's lamp rules.
/// `cued` is the transport's "parked on the cue point"; `blink` is the shared
/// blink phase (`super::blink_phase()`), passed in so a test can pin it.
pub fn mosi_frame(snap: &DeckSnapshot, view: &ScreenView, cued: bool, blink: bool) -> [u8; FRAME] {
    let mut f = [0u8; FRAME];
    let mut set = |(byte, mask): (usize, u8), on: bool| if on { f[byte] |= mask };
    let lit = |l: Lamp| match l { Lamp::Off => false, Lamp::Dim | Lamp::Full => true, Lamp::Blink => blink };
    for &(k, byte, mask) in BIT { set((byte, mask), lit(k.lamp(snap, view, false))); }
    let loaded = snap.total_samples > 0;
    // PLAY lit while playing and blinking while a loaded deck waits; CUE lit
    // at the cue point and blinking where a new one would be set.
    set(PLAY, if snap.playing { true } else { loaded && blink });
    set(CUE, loaded && !snap.playing && (cued || blink));
    set(MASTER_TEMPO, snap.key_lock);
    set(JOG_CDJ, !snap.jog_vinyl);
    set(JOG_VINYL, snap.jog_vinyl);
    set(ENCODER, true);
    let (byte, dim, bright) = JOG_RING;
    f[byte] |= if snap.playing { dim | bright } else { dim };
    for &(k, byte, medium, full) in STEP {
        f[byte] |= match k.lamp(snap, view, false) {
            Lamp::Off => 0,
            Lamp::Dim => medium,
            Lamp::Full => full,
            Lamp::Blink => if blink { full } else { medium },
        };
    }
    for (i, &base) in PADS.iter().enumerate() {
        let rgb = if snap.hot_cues[i].is_some() { [0x00, 0x7f, 0x00] } else { [0x06, 0x0c, 0x0c] };
        f[base..base + 3].copy_from_slice(&rgb);
    }
    let crc = crc16_x25(&f[..62]);
    f[62..].copy_from_slice(&crc.to_le_bytes());
    f
}

/// Where `DeckApp` leaves the LED frame for the SPI thread.  `active` is set
/// once the device is open, so the frame is built only when someone reads it.
#[derive(Default)]
pub struct Panel {
    active: AtomicBool,
    leds: Mutex<Option<[u8; FRAME]>>,
}

impl Panel {
    pub fn active(&self) -> bool { self.active.load(Ordering::Relaxed) }
    pub fn publish(&self, f: [u8; FRAME]) {
        if let Ok(mut l) = self.leds.lock() { *l = Some(f); }
    }
    fn latest(&self) -> Option<[u8; FRAME]> { self.leds.lock().ok().and_then(|l| *l) }
}

#[cfg(not(target_os = "linux"))]
pub fn spawn_from_env(_tx: std::sync::mpsc::Sender<Event>, _ind: std::sync::Arc<super::Indicators>) {
    if std::env::var_os("OPENDECK_SUBUCOM").is_some() { log::warn!("OPENDECK_SUBUCOM: the sub-CPU surface is Linux only"); }
}

#[cfg(target_os = "linux")]
pub use linux::spawn_from_env;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::surface::Indicators;
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::path::PathBuf;
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// `_IOW(0x70, 0, 16 bytes)`: hand the sub-CPU one LED frame.
    const MOSI_TRANSFER: libc::c_ulong = 0x4010_7000;
    /// The stock player rewrites its LED frame about every 10 ms.
    const LED_EVERY: Duration = Duration::from_millis(10);

    #[repr(C)]
    struct MosiCmd { magic: u32, size: u32, data: u64 }

    fn send_leds(dev: &File, frame: &[u8; FRAME]) -> std::io::Result<()> {
        let cmd = MosiCmd { magic: 0x0100_0000, size: FRAME as u32, data: frame.as_ptr() as u64 };
        // SAFETY: the driver copies `size` bytes from `data` (a live 64-byte
        // array) and 16 bytes from `cmd`, both valid for the call.
        let r = unsafe { libc::ioctl(dev.as_raw_fd(), MOSI_TRANSFER as _, &cmd as *const MosiCmd) };
        if r < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
    }

    pub fn spawn_from_env(tx: Sender<Event>, ind: Arc<Indicators>) {
        let Some(path) = std::env::var_os("OPENDECK_SUBUCOM").map(PathBuf::from) else { return };
        let r = std::thread::Builder::new().name("subucom".into()).spawn(move || run(path, tx, ind));
        if let Err(e) = r { log::warn!("subucom: {e}"); }
    }

    fn run(path: PathBuf, tx: Sender<Event>, ind: Arc<Indicators>) {
        loop {
            match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(dev) => {
                    log::info!("subucom: panel on {}", path.display());
                    ind.panel.active.store(true, Ordering::Relaxed);
                    let e = session(dev, &tx, &ind);
                    ind.panel.active.store(false, Ordering::Relaxed);
                    match e {
                        Ok(()) => return,     // the app is gone
                        Err(e) => log::warn!("subucom: {}: {e}; retrying", path.display()),
                    }
                }
                Err(e) => log::warn!("subucom: cannot open {}: {e}; retrying", path.display()),
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    fn session(mut dev: File, tx: &Sender<Event>, ind: &Indicators) -> std::io::Result<()> {
        let mut dec = Decoder::new();
        let mut frame = [0u8; FRAME];
        let mut last_leds = Instant::now() - LED_EVERY;
        let mut reported_bad = 0;
        loop {
            // Blocks one poll period (~1.2 ms) in the driver, like the stock app.
            dev.read_exact(&mut frame)?;
            for ev in dec.feed(&frame) {
                if tx.send(ev).is_err() { return Ok(()); }
            }
            if dec.bad_frames > reported_bad {
                reported_bad = dec.bad_frames;
                if reported_bad == 1 || reported_bad.is_power_of_two() { log::warn!("subucom: {reported_bad} bad frame(s) dropped"); }
            }
            if last_leds.elapsed() >= LED_EVERY {
                last_leds = Instant::now();
                if let Some(leds) = ind.panel.latest() { send_leds(&dev, &leds)?; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel at rest, as cdj3k-emu's `cdj3k::IDLE` lays it out.
    fn idle() -> [u8; FRAME] {
        let mut f = [0u8; FRAME];
        f[2] = 0x01; f[3] = 0x04; f[4] = 0x03; f[12] = 0x81;
        f[14] = 0xff; f[15] = 0xff; f[21] = 0x7f;
        f[22..24].copy_from_slice(&TEMPO_CENTER.to_le_bytes());
        f[26] = 0xff; f[27] = 0xff; f[28] = 0xff; f[29] = 0xff; f[30] = 0x04;
        f[32] = 0x51; f[33] = 0x01;
        for (i, b) in [0xd5, 0xd6, 0xd6, 0xd5, 0xd5, 0xd6, 0xd6, 0xd5, 0xd5, 0xd6].into_iter().enumerate() { f[34 + i] = b; }
        stamp(f)
    }
    fn stamp(mut f: [u8; FRAME]) -> [u8; FRAME] {
        let c = crc16_x25(&f[..62]);
        f[62..].copy_from_slice(&c.to_le_bytes());
        f
    }
    fn with(mut f: [u8; FRAME], edit: impl FnOnce(&mut [u8; FRAME])) -> [u8; FRAME] { edit(&mut f); stamp(f) }
    fn keys(evs: &[Event]) -> Vec<(&'static str, bool)> {
        evs.iter().filter_map(|e| match e { Event::Ui(UiEvent::PanelKey { key, pressed }) => Some((*key, *pressed)), _ => None }).collect()
    }

    #[test]
    fn crc_is_x25() {
        // The CRC-16/X-25 check value.
        assert_eq!(crc16_x25(b"123456789"), 0x906E);
    }

    #[test]
    fn first_frame_primes_and_sends_the_fader() {
        let mut d = Decoder::new();
        let held = with(idle(), |f| f[5] |= 0x01);   // PLAY held at start-up
        let evs = d.feed(&held);
        assert!(keys(&evs).is_empty());
        assert!(matches!(evs[..], [Event::Deck(CE::TempoFader { position })] if (position - 0.5).abs() < 1e-6));
        // Its release still reports.
        assert_eq!(keys(&d.feed(&idle())), [("PLAY", false)]);
    }

    #[test]
    fn keys_report_both_edges_once() {
        let mut d = Decoder::new();
        d.feed(&idle());
        let cue = with(idle(), |f| f[5] |= 0x02);
        assert_eq!(keys(&d.feed(&cue)), [("CUE", true)]);
        assert!(keys(&d.feed(&cue)).is_empty());
        let both = with(idle(), |f| { f[5] |= 0x02; f[9] |= 0x08; f[11] |= 0x10; });
        assert_eq!(keys(&d.feed(&both)), [("HOT_D", true), ("ROTARY_PRESS", true)]);
        assert_eq!(keys(&d.feed(&idle())), [("CUE", false), ("HOT_D", false), ("ROTARY_PRESS", false)]);
    }

    #[test]
    fn every_key_name_is_one_the_app_knows() {
        let special = ["PLAY", "CUE", "MASTER_TEMPO", "ROTARY_PRESS"];
        for &(k, _, _) in KEYS {
            assert!(special.contains(&k) || k.starts_with("HOT_") || Key::from_emu_name(k).is_some(), "{k}");
        }
    }

    #[test]
    fn bad_crc_and_wrong_header_are_dropped() {
        let mut d = Decoder::new();
        d.feed(&idle());
        let mut f = with(idle(), |f| f[5] |= 0x01);
        f[62] ^= 1;
        assert!(d.feed(&f).is_empty());
        let g = with(idle(), |f| { f[3] = 0; f[5] |= 0x01; });
        assert!(d.feed(&g).is_empty());
        assert_eq!(d.bad_frames, 2);
    }

    #[test]
    fn jog_turns_wrap_and_scale_one_turn_to_rev_seconds() {
        let mut d = Decoder::new();
        d.feed(&idle());
        let mut pos: u16 = 0xffff;
        let mut total = 0;
        let mut touched = Vec::new();
        // One full turn forward in 30-count steps, hand on the platter, across the wrap.
        for _ in 0..(JOG_TICKS_PER_REV as usize / 30) {
            pos = pos.wrapping_add(30);
            let f = with(idle(), |f| { f[26..28].copy_from_slice(&pos.to_le_bytes()); f[30] = 0x07; });
            for e in d.feed(&f) {
                match e {
                    Event::Deck(CE::JogDelta { delta, .. }) => total += delta,
                    Event::Deck(CE::JogTouch { touched: t }) => touched.push(t),
                    _ => {}
                }
            }
        }
        assert_eq!(touched, [true]);
        let want = (1.8 / crate::input::JOG_SECS_PER_TICK).round() as i32;   // CDJ_JOG_REV_SECONDS
        assert!((total - want).abs() <= 1, "{total} vs {want}");
        // Backwards is negative; letting go reports the release.
        let back = with(idle(), |f| f[26..28].copy_from_slice(&pos.wrapping_sub(300).to_le_bytes()));
        let evs = d.feed(&back);
        assert!(evs.iter().any(|e| matches!(e, Event::Deck(CE::JogDelta { delta, .. }) if *delta < 0)));
        assert!(evs.iter().any(|e| matches!(e, Event::Deck(CE::JogTouch { touched: false }))));
    }

    #[test]
    fn rotary_detents_move_the_list() {
        let mut d = Decoder::new();
        d.feed(&idle());
        let down = with(idle(), |f| f[14..16].copy_from_slice(&0x0000u16.to_le_bytes()));   // 0xffff + 1
        assert!(matches!(d.feed(&down)[..], [Event::Deck(CE::BrowseEncoderDelta { delta: 1 })]));
        let up = with(idle(), |f| f[14..16].copy_from_slice(&0xfffeu16.to_le_bytes()));
        assert!(matches!(d.feed(&up)[..], [Event::Deck(CE::BrowseEncoderDelta { delta: -2 })]));
    }

    #[test]
    fn tempo_halves_meet_at_the_detent() {
        assert_eq!(tempo_position(0), 1.0);           // top: slowest
        assert_eq!(tempo_position(0xFFFF), 0.0);      // bottom: fastest
        assert_eq!(tempo_position(TEMPO_CENTER), 0.5);
        assert_eq!(tempo_position(0x7F90), 0.5);      // dead band
        assert!(tempo_position(0x4000) > 0.5 && tempo_position(0xC000) < 0.5);
        let mut d = Decoder::new();
        d.feed(&idle());
        let jitter = with(idle(), |f| f[22..24].copy_from_slice(&(TEMPO_CENTER + 3).to_le_bytes()));
        assert!(d.feed(&jitter).is_empty());
        let moved = with(idle(), |f| f[22..24].copy_from_slice(&0xFFFFu16.to_le_bytes()));
        assert!(matches!(d.feed(&moved)[..], [Event::Deck(CE::TempoFader { position })] if position == 0.0));
    }

    /// The LED frame sits where cdj3k-emu reads each lamp, with its CRC.
    #[test]
    fn leds_land_on_the_emulators_bits() {
        let tags = opendeck_decode::TrackTags::default();
        let mut snap = DeckSnapshot::test(&tags);
        snap.quantize = true;
        snap.master = true;
        snap.hot_cues[2] = Some(1000);
        let f = mosi_frame(&snap, &ScreenView::Playback, false, false);
        assert_eq!(f[3] & 0xc0, 0xc0, "QUANTIZE full");
        assert_eq!(f[3] & 0x30, 0x10, "SLIP dim");
        assert_eq!(f[4] & 0x03, 0x01, "SOURCE dim on the playback screen");
        assert_ne!(f[2] & 0x20, 0, "MASTER");
        assert_eq!(f[2] & 0x08, 0, "BEAT SYNC off");
        assert_eq!(&f[18..21], &[0x00, 0x7f, 0x00], "pad C set");
        assert_eq!(&f[12..15], &[0x06, 0x0c, 0x0c], "pad A empty");
        assert_eq!(crc16_x25(&f[..62]), u16le(&f, 62));
        let f = mosi_frame(&snap, &ScreenView::TagList, false, false);
        assert_eq!(f[4] & 0x30, 0x30, "TAG LIST full on its screen");
    }
}
