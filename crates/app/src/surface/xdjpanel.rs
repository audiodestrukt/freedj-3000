//! XDJ-1000MK2 panel CPU over its panel link: the deck's own face as a surface.
//!
//! The XDJ-1000MK2's buttons, jog, tempo fader, browse rotary and LCD touch
//! panel all sit behind a Renesas M16C on the panel board (PNLB), which the
//! main CPU clocks over a synchronous serial link: 32 bytes each way every
//! ~3 ms, paced by the panel's BUSY line.  The frame was read off the live
//! unit with a logic analyser; `docs/reference/xdj-1000mk2-panel-bus.md` has
//! the captures, the field map and the open questions.
//!
//! Panel → main (what [`Decoder`] reads), LSB-first bytes:
//! `81 00` header, tempo fader at 4..6 (big-endian), jog position at 8..10
//! (signed, free-running), jog speed at 10..12, rotary counter at 14, jog
//! flags at 15, keys at 16..21, LCD touch X/Y at 22..26, a ones'-complement
//! sum of 0..30 at 30, and `0x8F` at 31.
//!
//! The keys go onto the bus by their cdj3k-emu names ([`UiEvent::PanelKey`]),
//! as `subucom` does for the CDJ-3000, so `DeckApp` gives them the same
//! faceplate behaviour.  Jog, fader and rotary become deck events here.
//!
//! Main → panel ([`mosi_frame`]): the lamps and the jog ring, built from the
//! deck's state on top of the stock idle frame; only bits identified on the
//! unit are touched (one of the others puts the deck into standby).
//!
//! Transports, enabled out of band with `OPENDECK_XDJPANEL`:
//! - `sniff`: listen to a stock unit through an fx2 logic analyser
//!   (`sigrok-cli` on the PATH; wiring as `tools/panel-bus/capture.sh`): the
//!   XDJ's main board still clocks the panel and owns its lamps, the deck's
//!   controls drive OpenDeck.
//! - `fx2`: OpenDeck is the panel's master through the same analyser running
//!   `tools/panel-bus/fx2/bridge.c` (embedded here and loaded at start; the
//!   main board cut off SCK/RXD/BUSY as in the reference doc, the deck
//!   switched on after OpenDeck starts).  The deck's controls drive OpenDeck
//!   and OpenDeck lights the face.  A [`Link`] is the only part that knows how
//!   frames reach the panel; a Pi's spidev + GPIO is the next one.
//!
//! The XDJ's LCD touch panel is decoded but only passed on with
//! `OPENDECK_XDJPANEL_TOUCH=1`: OpenDeck normally draws on a screen of its own.

use crate::input::{ControlEvent as CE, Event, UiEvent};
use crate::snapshot::DeckSnapshot;
use super::lanhid::JogScaler;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub const FRAME: usize = 32;

/// Jog position counts per platter turn: ~3272, one turn by hand on the unit
/// (the CDJ-3000's is 3240).  The counter counts up turning clockwise.
const JOG_TICKS_PER_REV: f64 = 3272.0;
/// The tempo fader's raw value reads 0x00xx at one end and 0xFFxx at the
/// other, 0x80xx at the centre; this much either side of 0x8000 reads 0 %.
const TEMPO_DEAD: u16 = 0x0200;
/// Fader moves smaller than this many raw counts are noise (the low byte
/// wanders by ~0x40 at rest).
const TEMPO_HYSTERESIS: u16 = 0x80;
/// LCD touch: X counts from the right edge, both axes about 55..950 across
/// the glass; a reading under this on both is no touch.
const TOUCH_X: (f32, f32) = (55.0, 950.0);
const TOUCH_Y: (f32, f32) = (85.0, 900.0);
const TOUCH_MIN: u16 = 16;

/// Every key the panel reports that OpenDeck has an action for, by its
/// cdj3k-emu name.  USB STOP is wired to the main board, not the panel.
const KEYS: &[(&str, usize, u8)] = &[
    ("PLAY", 16, 0x01), ("CUE", 16, 0x02), ("RELOOP", 16, 0x04), ("LOOP_OUT", 16, 0x08), ("LOOP_IN", 16, 0x10),
    ("REV", 16, 0x80),
    ("ROTARY_PRESS", 17, 0x01),
    ("TRACK_PREV", 18, 0x01), ("TRACK_NEXT", 18, 0x02), ("SEARCH_PREV", 18, 0x04), ("SEARCH_NEXT", 18, 0x08),
    ("QUANTIZE", 19, 0x01), ("TIME_MODE", 19, 0x02), ("BACK", 19, 0x04), ("TAG_TRACK", 19, 0x08), ("SHORTCUT", 19, 0x10),
    ("TRACK_FILTER", 19, 0x20),
    ("JOG_MODE", 20, 0x01), ("TEMPO_RANGE", 20, 0x02), ("MASTER_TEMPO", 20, 0x04),
];

/// Key bytes read off analog ladders pass through a neighbour's code on the
/// way in and out (30-90 ms seen on byte 20), so a byte's new value counts
/// only after this many identical frames (~3 ms each).
fn settle_frames(byte: usize) -> u8 {
    match byte { 20 => 34, 18 | 19 => 4, _ => 1 }
}

mod at {
    pub const TEMPO: usize = 4;
    pub const JOG: usize = 8;
    pub const ROTARY: usize = 14;
    pub const JOG_FLAGS: usize = 15;
    pub const KEYS: std::ops::Range<usize> = 16..21;
    pub const TOUCH: usize = 22;
}

fn u16be(f: &[u8; FRAME], o: usize) -> u16 { u16::from_be_bytes([f[o], f[o + 1]]) }

/// The 8-bit ones'-complement sum the link uses: add, fold the carry back in.
pub fn oc_sum(data: &[u8]) -> u8 {
    let mut s: u32 = 0;
    for &b in data {
        s += b as u32;
        s = (s & 0xFF) + (s >> 8);
    }
    s as u8
}

/// A panel frame we trust: header, trailer and sum.  The header check also
/// drops an all-0xFF frame (TXD idling high), which the sum alone passes.
pub fn valid(f: &[u8; FRAME]) -> bool {
    f[0] == 0x81 && f[31] == 0x8F && oc_sum(&f[..30]) == f[30]
}

/// The fader's raw value → the deck's fader position (0 = bottom, 1 = top),
/// with a dead band at the centre detent.  The raw value runs the other way
/// (checked on the unit: 0x0000 at the top).
pub fn tempo_position(raw: u16) -> f32 {
    let (lo, hi) = (0x8000 - TEMPO_DEAD, 0x8000 + TEMPO_DEAD);
    let travel = if raw < lo {
        0.5 * raw as f32 / lo as f32
    } else if raw <= hi {
        0.5
    } else {
        0.5 + 0.5 * (raw - hi) as f32 / (0xFFFF - hi) as f32
    };
    1.0 - travel
}

/// The LCD touch as screen fractions from the top-left, or `None`.
pub fn lcd_touch(f: &[u8; FRAME]) -> Option<(f32, f32)> {
    let (x, y) = (u16be(f, at::TOUCH), u16be(f, at::TOUCH + 2));
    if x < TOUCH_MIN && y < TOUCH_MIN { return None; }
    let frac = |v: u16, (a, b): (f32, f32)| ((v as f32 - a) / (b - a)).clamp(0.0, 1.0);
    Some((1.0 - frac(x, TOUCH_X), frac(y, TOUCH_Y)))
}

/// A hand on the jog's top plate: both touch bits set (checked on the unit:
/// 00 untouched, 11 touched).
fn jog_touched(flags: u8) -> bool { flags & 0x30 == 0x30 }

/// Panel frames in, bus events out.  Holds the previous frame so each key
/// edge, detent and jog count is reported once.
pub struct Decoder {
    prev: Option<[u8; FRAME]>,
    /// Key bytes as last accepted, and a changed value waiting to settle.
    keys: [u8; 5],
    pending: [(u8, u8); 5],
    jog: JogScaler,
    tempo_sent: u16,
    touch: Option<(f32, f32)>,
    /// Pass the XDJ's own LCD touch panel on as pointer events.  Off unless
    /// asked for (`OPENDECK_XDJPANEL_TOUCH=1`): OpenDeck draws on its own
    /// screen (a Pi's capacitive one), and the XDJ panel's readings would land
    /// on that screen as stray presses.
    lcd_touch: bool,
    pub bad_frames: u64,
}

impl Default for Decoder {
    fn default() -> Self { Self::new() }
}

impl Decoder {
    pub fn new() -> Self {
        Decoder {
            prev: None, keys: [0; 5], pending: [(0, 0); 5],
            jog: JogScaler::per_rev(JOG_TICKS_PER_REV), tempo_sent: 0, touch: None, lcd_touch: false, bad_frames: 0,
        }
    }

    /// Pass the XDJ's LCD touch on as pointer events (see `lcd_touch`).
    pub fn with_lcd_touch(mut self, on: bool) -> Self { self.lcd_touch = on; self }

    /// The decoder the transports use: LCD touch per `OPENDECK_XDJPANEL_TOUCH`.
    fn from_env() -> Self {
        let on = std::env::var("OPENDECK_XDJPANEL_TOUCH").as_deref() == Ok("1");
        if on { log::info!("xdjpanel: the XDJ's LCD touch is a pointer"); }
        Decoder::new().with_lcd_touch(on)
    }

    pub fn feed(&mut self, f: &[u8; FRAME]) -> Vec<Event> {
        let mut out = Vec::new();
        if !valid(f) { self.bad_frames += 1; return out; }
        let tempo = u16be(f, at::TEMPO);
        let Some(p) = self.prev.replace(*f) else {
            // First frame: keys held at start-up do not fire, the fader is
            // taken where the hand left it.
            self.keys.copy_from_slice(&f[at::KEYS]);
            self.tempo_sent = tempo;
            out.push(Event::Deck(CE::TempoFader { position: tempo_position(tempo) }));
            return out;
        };
        let deck = |e| Event::Deck(e);

        for (i, byte) in at::KEYS.enumerate() {
            let now = f[byte];
            if now == self.keys[i] { self.pending[i] = (now, 0); continue; }
            let (val, n) = self.pending[i];
            let n = if val == now { n.saturating_add(1) } else { 1 };
            self.pending[i] = (now, n);
            if n < settle_frames(byte) { continue; }
            let was = std::mem::replace(&mut self.keys[i], now);
            for &(key, b, mask) in KEYS {
                if b == byte && (was & mask != 0) != (now & mask != 0) {
                    out.push(Event::Ui(UiEvent::PanelKey { key, pressed: now & mask != 0 }));
                }
            }
        }

        let (was_t, is_t) = (jog_touched(p[at::JOG_FLAGS]), jog_touched(f[at::JOG_FLAGS]));
        if was_t != is_t { out.push(deck(CE::JogTouch { touched: is_t })); }
        // Clockwise (forward) counts up.
        let moved = u16be(f, at::JOG).wrapping_sub(u16be(&p, at::JOG)) as i16 as i32;
        if moved != 0 {
            match self.jog.step(moved) {
                0 => {}
                d => out.push(deck(CE::JogDelta { delta: d, velocity_rpm: 0.0 })),
            }
        }

        // Counter up = one detent clockwise = down the list.
        let turned = f[at::ROTARY].wrapping_sub(p[at::ROTARY]) as i8 as i32;
        if turned != 0 { out.push(deck(CE::BrowseEncoderDelta { delta: turned })); }

        if tempo.abs_diff(self.tempo_sent) >= TEMPO_HYSTERESIS {
            self.tempo_sent = tempo;
            out.push(deck(CE::TempoFader { position: tempo_position(tempo) }));
        }

        let now = if self.lcd_touch { lcd_touch(f) } else { None };
        match (self.touch, now) {
            (_, Some((x, y))) if self.touch != now => out.push(Event::Ui(UiEvent::PanelTouch { x, y, down: true })),
            (Some((x, y)), None) => out.push(Event::Ui(UiEvent::PanelTouch { x, y, down: false })),
            _ => {}
        }
        self.touch = now;
        out
    }
}

// ── Lamps and the jog ring (main → panel) ─────────────────────────────────

/// Main → panel frame bytes OpenDeck writes; the link adds the sum and 0x8F.
pub const OUT: usize = 30;

/// The stock main board's frame with a track loaded and paused (2026-11-19).
/// Bits not named below are left as it sends them: one of the unnamed ones
/// puts the deck into standby.
const STOCK_IDLE: [u8; OUT] = [
    0x30, 0xdf, 0, 0, 0, 0, 0, 0, 0x58, 0x01, 0, 0x10, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0x02, 0x02, 0x02, 0, 0, 0, 0, 0,
];

/// Lamp bits identified on the unit (reference doc, "Lamps").
mod lamp {
    /// PLAY: byte 0 bit 0 (on while playing).
    pub const PLAY: (usize, u8) = (0, 0x01);
    /// REV: byte 0 bit 2, lit while the deck plays in reverse.
    pub const REV: (usize, u8) = (0, 0x04);
    /// MASTER TEMPO: byte 1 bit 2.
    pub const MASTER_TEMPO: (usize, u8) = (1, 0x04);
    /// QUANTIZE: byte 1 bit 3.
    pub const QUANTIZE: (usize, u8) = (1, 0x08);
    /// JOG MODE = VINYL: byte 1 bits 1 and 7 and byte 8 bits 3 and 4 switched
    /// together with the VINYL key in the stock captures (the key's lamp and
    /// the jog display's VINYL mark).
    pub const VINYL: [(usize, u8); 2] = [(1, 0x82), (8, 0x18)];
    /// The jog ring's playhead: byte 9, 1..=135 around the ring.
    pub const RING: usize = 9;
    /// The jog display's red cue mark: byte 7, 1..=80 around the ring (each
    /// step a pair of red segments, wrapping past 80), 0 = none; same origin
    /// as the playhead, so the two meet when the playhead reaches the cue.
    pub const CUE_MARK: usize = 7;
}

/// Jog ring positions in one turn for the playhead and the cue mark, and how
/// long a turn is (33⅓ rpm).
const RING_STEPS: f64 = 135.0;
const CUE_STEPS: f64 = 80.0;
const RING_TURN_S: f64 = 1.8;

fn on_ring(seconds: f64, steps: f64) -> u8 {
    let turn = (seconds / RING_TURN_S).rem_euclid(1.0);
    1 + ((turn * steps) as u8).min(steps as u8 - 1)
}

/// The jog ring position for a playhead, as the stock deck draws it: one turn
/// every 1.8 s of track time, 1..=135.
pub fn ring_position(seconds: f64) -> u8 { on_ring(seconds, RING_STEPS) }

/// The red cue mark's position for a cue point, 1..=80 on the same turn.
pub fn cue_mark_position(seconds: f64) -> u8 { on_ring(seconds, CUE_STEPS) }

/// The main → panel frame for the deck as it is now.  `blink` is the shared
/// blink phase (`super::blink_phase()`), passed in so a test can pin it.
pub fn mosi_frame(snap: &DeckSnapshot, blink: bool) -> [u8; OUT] {
    let mut f = STOCK_IDLE;
    let mut set = |(byte, mask): (usize, u8), on: bool| {
        if on { f[byte] |= mask } else { f[byte] &= !mask }
    };
    let loaded = snap.total_samples > 0;
    set(lamp::PLAY, if snap.playing { true } else { loaded && blink });
    set(lamp::REV, snap.reverse);
    set(lamp::MASTER_TEMPO, snap.key_lock);
    set(lamp::QUANTIZE, snap.quantize);
    for bits in lamp::VINYL { set(bits, snap.jog_vinyl); }
    let per_sec = snap.sample_rate as f64 * snap.channels.max(1) as f64;
    f[lamp::RING] = if loaded && per_sec > 0.0 { ring_position(snap.position as f64 / per_sec) } else { 1 };
    f[lamp::CUE_MARK] = if loaded && per_sec > 0.0 { cue_mark_position(snap.cue_point as f64 / per_sec) } else { 0 };
    f
}

/// Where `DeckApp` leaves the lamp frame for the link thread.  `active` is set
/// once a panel is being driven, so the frame is built only when someone reads it.
#[derive(Default)]
pub struct Lamps {
    active: AtomicBool,
    frame: Mutex<Option<[u8; OUT]>>,
}

impl Lamps {
    pub fn active(&self) -> bool { self.active.load(Ordering::Relaxed) }
    pub fn publish(&self, f: [u8; OUT]) {
        if let Ok(mut l) = self.frame.lock() { *l = Some(f); }
    }
    fn latest(&self) -> Option<[u8; OUT]> { self.frame.lock().ok().and_then(|l| *l) }
}

/// How frames reach the panel: one exchange is "send this, here is the
/// panel's latest reply".  The fx2 bridge today; a Pi's spidev + GPIO next.
pub trait Link {
    /// Send `out` (the link adds the sum and trailer) if it differs from the
    /// last one sent, and return the next panel reply, or `None` if none came
    /// in time.
    fn exchange(&mut self, out: &[u8; OUT]) -> std::io::Result<Option<[u8; FRAME]>>;
}

/// Mapping aid: `OPENDECK_XDJPANEL_POKE=<file>` overrides bits of the frame
/// OpenDeck sends, re-read whenever the file changes.  One edit per line:
/// `10 = 40` sets byte 10, `8 |= 0x20` sets bits, `1 &= 0xfd` clears them;
/// `#` starts a comment.  An empty or missing file overrides nothing.
struct Poke {
    path: Option<std::path::PathBuf>,
    stamp: Option<std::time::SystemTime>,
    checked: std::time::Instant,
    edits: Vec<(usize, char, u8)>,
}

impl Poke {
    fn from_env() -> Self {
        let path = std::env::var_os("OPENDECK_XDJPANEL_POKE").map(std::path::PathBuf::from);
        if let Some(p) = &path { log::info!("xdjpanel: frame overrides from {}", p.display()); }
        Poke { path, stamp: None, checked: std::time::Instant::now(), edits: Vec::new() }
    }

    fn parse(text: &str) -> Vec<(usize, char, u8)> {
        let num = |s: &str| {
            let s = s.trim();
            s.strip_prefix("0x").map_or_else(|| s.parse().ok(), |h| u8::from_str_radix(h, 16).ok())
        };
        text.lines().filter_map(|l| {
            let l = l.split('#').next()?.trim();
            let (lhs, op, rhs) = if let Some((a, b)) = l.split_once("|=") { (a, '|', b) }
                else if let Some((a, b)) = l.split_once("&=") { (a, '&', b) }
                else { let (a, b) = l.split_once('=')?; (a, '=', b) };
            let byte: usize = lhs.trim().parse().ok().filter(|&b| b < OUT)?;
            Some((byte, op, num(rhs)?))
        }).collect()
    }

    fn apply(&mut self, out: &mut [u8; OUT]) {
        let Some(path) = &self.path else { return };
        if self.checked.elapsed() > std::time::Duration::from_millis(200) {
            self.checked = std::time::Instant::now();
            let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if stamp != self.stamp {
                self.stamp = stamp;
                self.edits = std::fs::read_to_string(path).map(|t| Self::parse(&t)).unwrap_or_default();
                log::info!("xdjpanel: {} frame override(s)", self.edits.len());
            }
        }
        for &(b, op, v) in &self.edits {
            match op { '|' => out[b] |= v, '&' => out[b] &= v, _ => out[b] = v }
        }
    }
}

/// The master loop every [`Link`] shares: lamps out, replies through the
/// [`Decoder`] onto the bus.  Returns when the app has gone away.
fn drive(link: &mut dyn Link, tx: &std::sync::mpsc::Sender<Event>, lamps: &Lamps) -> std::io::Result<()> {
    let mut dec = Decoder::from_env();
    let mut poke = Poke::from_env();
    lamps.active.store(true, Ordering::Relaxed);
    let r = (|| loop {
        let mut out = lamps.latest().unwrap_or(STOCK_IDLE);
        poke.apply(&mut out);
        if let Some(reply) = link.exchange(&out)? {
            for ev in dec.feed(&reply) {
                if tx.send(ev).is_err() { return Ok(()); }
            }
        }
    })();
    lamps.active.store(false, Ordering::Relaxed);
    r
}

/// Bits off the analyser, frames out: SCK on bit 0, the main board's data on
/// bit 1, the panel's on bit 2, BUSY on bit 3, one byte per sample.  A frame
/// is 256 bits sampled on SCK's rising edge, LSB first; a pause in the clock
/// longer than `gap` samples ends it (a short one is dropped).
pub struct Sniffer {
    gap: u32,
    since_edge: u32,
    last_sck: bool,
    bits: u32,
    miso: [u8; FRAME],
}

impl Sniffer {
    /// `gap`: samples of clock silence that end a frame; the frames are
    /// ~2.7 ms apart and bytes inside one can be ~80 µs apart.
    pub fn new(gap: u32) -> Self {
        Sniffer { gap, since_edge: 0, last_sck: true, bits: 0, miso: [0; FRAME] }
    }

    /// Feed raw samples; each complete panel frame goes to `out`.
    pub fn push(&mut self, samples: &[u8], mut out: impl FnMut(&[u8; FRAME])) {
        for &s in samples {
            let sck = s & 1 != 0;
            if sck && !self.last_sck {
                if self.since_edge > self.gap { self.bits = 0; }
                if (self.bits as usize) < FRAME * 8 {
                    let (byte, bit) = (self.bits as usize / 8, self.bits % 8);
                    if bit == 0 { self.miso[byte] = 0; }
                    self.miso[byte] |= ((s >> 2) & 1) << bit;
                }
                self.bits += 1;
                self.since_edge = 0;
            } else {
                self.since_edge = self.since_edge.saturating_add(1);
                if self.since_edge == self.gap + 1 {
                    if self.bits as usize == FRAME * 8 { out(&self.miso); }
                    self.bits = 0;
                }
            }
            self.last_sck = sck;
        }
    }
}

pub fn spawn_from_env(tx: std::sync::mpsc::Sender<Event>, ind: std::sync::Arc<super::Indicators>) {
    let spawn = |f: Box<dyn FnOnce() + Send>| {
        if let Err(e) = std::thread::Builder::new().name("xdjpanel".into()).spawn(f) { log::warn!("xdjpanel: {e}"); }
    };
    match std::env::var("OPENDECK_XDJPANEL").as_deref() {
        Ok("sniff") => spawn(Box::new(move || sniff::run(tx))),
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        Ok("fx2") => spawn(Box::new(move || fx2::run(tx, ind))),
        Ok(other) if !other.is_empty() => {
            let _ = ind;
            log::warn!("OPENDECK_XDJPANEL={other}: \"sniff\" or \"fx2\" (macOS / Linux)")
        }
        _ => {}
    }
}

/// OpenDeck as the panel's master through the fx2 analyser running
/// `tools/panel-bus/fx2/bridge.c`: 30 bytes out on EP1 OUT, each valid panel
/// reply back on EP1 IN (32 bytes, then frames sent and bad replies, u16 each).
/// The firmware is embedded and loaded once at start with the FX2's own 0xA0
/// request, so the analyser must be freshly plugged in (its default
/// descriptors carry EP1) and the deck switched on afterwards: the panel only
/// answers a master that has had it from power-up.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod fx2 {
    use super::*;
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    use std::time::Duration;

    const VID: u16 = 0x0925;
    const PID: u16 = 0x3881;
    const FIRMWARE: &str = include_str!("../../firmware/xdj-fx2-bridge.ihx");
    const EP_OUT: u8 = 0x01;
    const EP_IN: u8 = 0x81;

    struct Fx2 {
        usb: rusb::DeviceHandle<rusb::GlobalContext>,
        sent: Option<[u8; OUT]>,
    }

    fn io(e: rusb::Error) -> std::io::Error { std::io::Error::other(e) }

    /// Load the bridge firmware: CPU held in reset, image written, released.
    fn load(usb: &rusb::DeviceHandle<rusb::GlobalContext>) -> rusb::Result<usize> {
        let t = Duration::from_secs(1);
        usb.write_control(0x40, 0xA0, 0xE600, 0, &[1], t)?;
        let mut bytes = 0;
        for line in FIRMWARE.lines() {
            let Some(hex) = line.strip_prefix(':') else { continue };
            let rec: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect();
            if rec.len() < 5 || rec[3] != 0 { continue; }
            let (n, addr) = (rec[0] as usize, u16::from_be_bytes([rec[1], rec[2]]));
            usb.write_control(0x40, 0xA0, addr, 0, &rec[4..4 + n], t)?;
            bytes += n;
        }
        usb.write_control(0x40, 0xA0, 0xE600, 0, &[0], t)?;
        Ok(bytes)
    }

    impl Fx2 {
        fn open(load_firmware: bool) -> rusb::Result<Self> {
            let usb = rusb::open_device_with_vid_pid(VID, PID).ok_or(rusb::Error::NoDevice)?;
            if load_firmware {
                let n = load(&usb)?;
                log::info!("xdjpanel: bridge firmware loaded ({n} bytes); switch the deck on now");
            }
            let _ = usb.set_active_configuration(1);
            usb.claim_interface(0)?;
            usb.set_alternate_setting(0, 1)?;
            Ok(Fx2 { usb, sent: None })
        }
    }

    impl Link for Fx2 {
        fn exchange(&mut self, out: &[u8; OUT]) -> std::io::Result<Option<[u8; FRAME]>> {
            if self.sent.as_ref() != Some(out) {
                self.usb.write_bulk(EP_OUT, out, Duration::from_millis(100)).map_err(io)?;
                self.sent = Some(*out);
            }
            let mut buf = [0u8; 64];
            match self.usb.read_bulk(EP_IN, &mut buf, Duration::from_millis(20)) {
                Ok(n) if n >= FRAME => Ok(Some(buf[..FRAME].try_into().expect("32 bytes"))),
                Ok(_) | Err(rusb::Error::Timeout) => Ok(None),
                Err(e) => Err(io(e)),
            }
        }
    }

    /// `OPENDECK_XDJPANEL_LOAD=0` skips loading the firmware: the bridge is
    /// already running and has the panel (restarting OpenDeck then needs no
    /// power cycle of the deck).
    pub fn run(tx: Sender<Event>, ind: Arc<crate::surface::Indicators>) {
        let mut load_firmware = std::env::var("OPENDECK_XDJPANEL_LOAD").as_deref() != Ok("0");
        loop {
            match Fx2::open(load_firmware) {
                Ok(mut link) => {
                    load_firmware = false;      // reloading would need the deck power-cycled again
                    log::info!("xdjpanel: driving the panel through the fx2 bridge");
                    match drive(&mut link, &tx, &ind.xdj) {
                        Ok(()) => return,       // the app is gone
                        Err(e) => log::warn!("xdjpanel: fx2: {e}; reopening"),
                    }
                }
                Err(e) => log::warn!("xdjpanel: fx2 analyser (0925:3881): {e}; retrying"),
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

mod sniff {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    const RATE: u32 = 4_000_000;
    /// 100 µs of clock silence ends a frame.
    const GAP: u32 = RATE / 10_000;

    pub fn run(tx: Sender<Event>) {
        loop {
            match session(&tx) {
                Ok(true) => return,   // the app is gone
                Ok(false) => log::warn!("xdjpanel: the analyser stream ended; restarting"),
                Err(e) => log::warn!("xdjpanel: sigrok-cli: {e}; retrying"),
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// One `sigrok-cli` run.  `Ok(true)` once the app has gone away.
    fn session(tx: &Sender<Event>) -> std::io::Result<bool> {
        // A long fixed run, not --continuous: sigrok-cli writes nothing to a
        // pipe in continuous mode.  An hour, then the loop starts another.
        let mut child = Command::new("sigrok-cli")
            .args(["-d", "fx2lafw", "--config", &format!("samplerate={RATE}"),
                   "--channels", "D0,D1,D2,D3", "--time", "3600000", "-O", "binary"])
            .stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let mut stdout = child.stdout.take().expect("piped");
        log::info!("xdjpanel: listening to the panel link through the analyser");
        let (mut sniffer, mut dec) = (Sniffer::new(GAP), Decoder::from_env());
        let (mut frames, mut reported_bad) = (0u64, 0u64);
        let mut buf = vec![0u8; 1 << 16];
        let mut gone = false;
        loop {
            let n = stdout.read(&mut buf)?;
            if n == 0 { break; }
            sniffer.push(&buf[..n], |f| {
                frames += 1;
                for ev in dec.feed(f) {
                    if tx.send(ev).is_err() { gone = true; }
                }
            });
            if gone { break; }
            if dec.bad_frames > reported_bad {
                reported_bad = dec.bad_frames;
                if reported_bad.is_power_of_two() { log::warn!("xdjpanel: {reported_bad} bad frame(s) of {frames} dropped"); }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        Ok(gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdjface::Key;

    /// The panel at rest, from the idle capture (2026-11-19).
    fn idle() -> [u8; FRAME] {
        stamp([0x81, 0x00, 0x00, 0x00, 0x80, 0x88, 0x80, 0x7c, 0xf0, 0x9a, 0xff, 0xff, 0x00, 0x00, 0xfe, 0x32,
               0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8f])
    }
    fn stamp(mut f: [u8; FRAME]) -> [u8; FRAME] { f[30] = oc_sum(&f[..30]); f }
    fn with(mut f: [u8; FRAME], edit: impl FnOnce(&mut [u8; FRAME])) -> [u8; FRAME] { edit(&mut f); stamp(f) }
    fn keys(evs: &[Event]) -> Vec<(&'static str, bool)> {
        evs.iter().filter_map(|e| match e { Event::Ui(UiEvent::PanelKey { key, pressed }) => Some((*key, *pressed)), _ => None }).collect()
    }

    #[test]
    fn checksum_matches_captured_frames() {
        // Two frames off the unit, main → panel and panel → main.
        let mosi = [0x30, 0xdf, 0, 0, 0, 0, 0, 0, 0x58, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x02, 0x02, 0x02, 0, 0, 0, 0, 0];
        assert_eq!(oc_sum(&mosi), 0x7e);
        let miso = [0x81, 0, 0, 0, 0x80, 0x9c, 0x80, 0x80, 0xf0, 0xaa, 0xff, 0xff, 0, 0, 0xfe, 0x42, 0, 0, 0, 0, 0, 0, 0, 0x01, 0, 0x01, 0, 0, 0, 0];
        assert_eq!(oc_sum(&miso), 0x7e);
    }

    #[test]
    fn all_ff_and_bad_sum_are_dropped() {
        let mut d = Decoder::new();
        d.feed(&idle());
        assert!(d.feed(&[0xff; FRAME]).is_empty());
        let mut f = with(idle(), |f| f[16] |= 0x01);
        f[30] ^= 1;
        assert!(d.feed(&f).is_empty());
        assert_eq!(d.bad_frames, 2);
    }

    #[test]
    fn first_frame_primes_and_sends_the_fader() {
        let mut d = Decoder::new();
        let held = with(idle(), |f| f[16] |= 0x01);   // PLAY held at start-up
        let evs = d.feed(&held);
        assert!(keys(&evs).is_empty());
        assert!(matches!(evs[..], [Event::Deck(CE::TempoFader { position })] if (position - 0.5).abs() < 1e-6));
        assert_eq!(keys(&d.feed(&idle())), [("PLAY", false)]);
    }

    #[test]
    fn keys_report_both_edges_once() {
        let mut d = Decoder::new();
        d.feed(&idle());
        let cue = with(idle(), |f| f[16] |= 0x02);
        assert_eq!(keys(&d.feed(&cue)), [("CUE", true)]);
        assert!(keys(&d.feed(&cue)).is_empty());
        let both = with(idle(), |f| { f[16] |= 0x02; f[17] |= 0x01; });
        assert_eq!(keys(&d.feed(&both)), [("ROTARY_PRESS", true)]);
        assert_eq!(keys(&d.feed(&idle())), [("CUE", false), ("ROTARY_PRESS", false)]);
    }

    #[test]
    fn ladder_keys_wait_out_the_neighbours_code() {
        let mut d = Decoder::new();
        d.feed(&idle());
        // TEMPO RANGE pressed, passing through MASTER TEMPO's code for 20 frames.
        let mt = with(idle(), |f| f[20] = 0x04);
        let tr = with(idle(), |f| f[20] = 0x02);
        for _ in 0..20 { assert!(keys(&d.feed(&mt)).is_empty()); }
        let mut got = Vec::new();
        for _ in 0..40 { got.extend(keys(&d.feed(&tr))); }
        assert_eq!(got, [("TEMPO_RANGE", true)]);
    }

    #[test]
    fn every_key_name_is_one_the_app_knows() {
        let special = ["PLAY", "CUE", "MASTER_TEMPO", "ROTARY_PRESS"];
        for &(k, _, _) in KEYS {
            assert!(special.contains(&k) || Key::from_emu_name(k).is_some(), "{k}");
        }
    }

    #[test]
    fn jog_counts_wrap_and_one_turn_is_rev_seconds() {
        let mut d = Decoder::new();
        d.feed(&idle());
        // From the idle frame's position (0xf09a), then across the wrap.
        let mut pos = u16be(&idle(), at::JOG);
        let mut total = 0;
        for _ in 0..(JOG_TICKS_PER_REV as usize / 30) {
            pos = pos.wrapping_add(30);   // clockwise: counting up
            let f = with(idle(), |f| f[8..10].copy_from_slice(&pos.to_be_bytes()));
            for e in d.feed(&f) {
                if let Event::Deck(CE::JogDelta { delta, .. }) = e { total += delta; }
            }
        }
        let want = (1.8 / crate::input::JOG_SECS_PER_TICK).round() as i32;
        assert!((total - want).abs() <= 1, "{total} vs {want}");
    }

    #[test]
    fn jog_touch_is_both_bits_set() {
        let mut d = Decoder::new();
        d.feed(&idle());                     // byte 15 = 0x32: touch bits 11
        let free = with(idle(), |f| f[15] = 0x02);
        assert!(matches!(d.feed(&free)[..], [Event::Deck(CE::JogTouch { touched: false })]));
        let held = with(idle(), |f| f[15] = 0x32);
        assert!(matches!(d.feed(&held)[..], [Event::Deck(CE::JogTouch { touched: true })]));
    }

    #[test]
    fn rotary_counter_wraps() {
        let mut d = Decoder::new();
        d.feed(&idle());   // 0xfe
        let f = with(idle(), |f| f[14] = 0x01);
        assert!(matches!(d.feed(&f)[..], [Event::Deck(CE::BrowseEncoderDelta { delta: 3 })]));
        let f = with(idle(), |f| f[14] = 0xff);
        assert!(matches!(d.feed(&f)[..], [Event::Deck(CE::BrowseEncoderDelta { delta: -2 })]));
    }

    #[test]
    fn tempo_dead_band_and_ends() {
        assert_eq!(tempo_position(0x8040), 0.5);
        assert_eq!(tempo_position(0), 1.0);          // top
        assert_eq!(tempo_position(0xFFFF), 0.0);     // bottom
        let mut d = Decoder::new();
        d.feed(&idle());
        let jitter = with(idle(), |f| f[5] = 0xc0);
        assert!(d.feed(&jitter).is_empty());
        let moved = with(idle(), |f| { f[4] = 0xff; f[5] = 0xff; });
        assert!(matches!(d.feed(&moved)[..], [Event::Deck(CE::TempoFader { position })] if position == 0.0));
    }

    #[test]
    fn lcd_touch_corners() {
        let mut f = idle();
        assert_eq!(lcd_touch(&f), None);
        f[22..24].copy_from_slice(&0x3b5u16.to_be_bytes());    // top-left, as measured
        f[24..26].copy_from_slice(&0x057u16.to_be_bytes());
        let (x, y) = lcd_touch(&f).unwrap();
        assert!(x < 0.02 && y < 0.02, "{x} {y}");
        f[22..24].copy_from_slice(&0x03bu16.to_be_bytes());    // bottom-right
        f[24..26].copy_from_slice(&0x380u16.to_be_bytes());
        let (x, y) = lcd_touch(&f).unwrap();
        assert!(x > 0.98 && y > 0.95, "{x} {y}");
    }

    #[test]
    fn ring_turns_once_per_1_8_seconds() {
        assert_eq!(ring_position(0.0), 1);
        assert_eq!(ring_position(0.9), 68);          // half a turn
        assert_eq!(ring_position(1.8), 1);           // a full turn wraps
        assert!(ring_position(1.79) >= 134);
        assert!((1..=135).contains(&ring_position(-0.5)));
        assert_eq!(cue_mark_position(0.0), 1);
        assert_eq!(cue_mark_position(0.9), 41);       // half of 80
        assert_eq!(cue_mark_position(1.8), 1);
    }

    #[test]
    fn lamps_follow_the_deck_and_leave_unknown_bits_alone() {
        let tags = opendeck_decode::TrackTags::default();
        let mut snap = DeckSnapshot::test(&tags);
        snap.total_samples = 44_100 * 2 * 60;
        snap.sample_rate = 44_100;
        snap.channels = 2;
        snap.position = 44_100 * 2 * 9 / 10;          // 0.9 s in
        snap.playing = true;
        snap.quantize = true;
        snap.key_lock = false;
        let f = mosi_frame(&snap, false);
        assert_ne!(f[0] & 0x01, 0, "PLAY lit while playing");
        assert_eq!(f[0] & 0x04, 0, "REV dark");
        assert_ne!(f[1] & 0x08, 0, "QUANTIZE");
        assert_eq!(f[1] & 0x04, 0, "MASTER TEMPO off");
        assert_eq!(f[9], 68, "ring half way");
        assert_eq!(f[7], 1, "cue mark at the track start's place");
        for i in (2..7).chain(10..30) { assert_eq!(f[i], STOCK_IDLE[i], "byte {i} untouched"); }
        assert_eq!(f[8] & !0x18, STOCK_IDLE[8] & !0x18, "byte 8 beyond the VINYL bits untouched");
        snap.playing = false;
        assert_ne!(mosi_frame(&snap, true)[0] & 0x01, 0, "PLAY blinks while paused");
        assert_eq!(mosi_frame(&snap, false)[0] & 0x01, 0);
    }

    #[test]
    fn lcd_touch_is_off_unless_asked_for() {
        let mut f = idle();
        f[22..24].copy_from_slice(&0x200u16.to_be_bytes());
        f[24..26].copy_from_slice(&0x200u16.to_be_bytes());
        let f = stamp(f);
        let touches = |evs: Vec<Event>| evs.into_iter().filter(|e| matches!(e, Event::Ui(UiEvent::PanelTouch { .. }))).count();
        let mut off = Decoder::new();
        off.feed(&idle());
        assert_eq!(touches(off.feed(&f)), 0);
        let mut on = Decoder::new().with_lcd_touch(true);
        on.feed(&idle());
        assert_eq!(touches(on.feed(&f)), 1);
    }

    #[test]
    fn poke_lines_parse() {
        assert_eq!(Poke::parse("10 = 40\n8 |= 0x20 # cue?\n1 &= 0xfd\n# all\n99 = 1\nnonsense"),
                   vec![(10, '=', 40), (8, '|', 0x20), (1, '&', 0xfd)]);
    }

    #[test]
    fn vinyl_lamps_follow_jog_mode() {
        let tags = opendeck_decode::TrackTags::default();
        let mut snap = DeckSnapshot::test(&tags);
        snap.jog_vinyl = true;
        let f = mosi_frame(&snap, false);
        assert_eq!((f[1] & 0x82, f[8] & 0x18), (0x82, 0x18));
        snap.jog_vinyl = false;
        let f = mosi_frame(&snap, false);
        assert_eq!((f[1] & 0x82, f[8] & 0x18), (0, 0));
    }

    /// Samples as the analyser streams them: one frame clocked out, LSB first.
    #[test]
    fn sniffer_rebuilds_a_frame_from_samples() {
        let frame = idle();
        let mut s = vec![0x01u8; 50];   // SCK idle high
        for i in 0..FRAME * 8 {
            let d = ((frame[i / 8] >> (i % 8)) & 1) << 2;
            s.extend([d, d, 0x01 | d, 0x01 | d, 0x01]);   // low, then rise with the bit
        }
        s.extend(std::iter::repeat_n(0x01, 500));
        let mut got = Vec::new();
        let mut sn = Sniffer::new(400);
        sn.push(&s[..700], |f| got.push(*f));
        sn.push(&s[700..], |f| got.push(*f));
        assert_eq!(got, vec![frame]);
    }
}
