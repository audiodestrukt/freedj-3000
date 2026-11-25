//! When the record has the deck, and where it puts the playhead.
//!
//! A port of freedj-esp's P4 deck (`p4/deck/main/vinyl.cpp` and its drive
//! loop in `main.cpp`), with the numbers from the bench:
//!
//! - **Arming.** The record takes over only after 0.3 s of steady carrier
//!   at 0.6–1.5× (33 or 45). An interface plugged in only for its output, or
//!   music on its input, never stops the deck. Once armed, no carrier means
//!   a stopped record.
//! - **Needle drops (ABS).** Carrier back after ≥500 ms without it: the
//!   decoder drops its bit history (keeping its level) and the deck holds
//!   silent until bit lock gives a position (~25 cycles; ≤1 s, then speed
//!   only), rather than scratching the old spot. A shorter stop is a hand
//!   on the platter and plays straight on.
//! - **Following (ABS).** Jump when the track is ≥300 ms off the record (at
//!   most every 250 ms); re-sync 25–300 ms errors once they have held 200 ms.
//!   Steady play stays within a few ms on speed alone.
//! - **Track 0:00** is the first groove: 24.0 s of timecode on Dan's Serato
//!   record (three drops on the bench, 2026-11-05).
//!
//! Time here is counted in input samples, not wall clock, so a test can
//! play a record at any speed and get the same answer every run.

use crate::decoder::{Decoder, Format};

/// Serato's modes. OFF (the deck ignores the input) is the app's, not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Position and speed: a needle drop moves the track.
    Abs,
    /// Speed and direction only; cues and jumps on the deck are kept.
    Rel,
    /// Internal: the record is ignored; the deck plays on at the record's
    /// last speed until the fader moves.
    Int,
}

/// What the record is doing, every 10 ms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VinylState {
    /// Heard 0.3 s of steady carrier since the input appeared.
    pub armed: bool,
    /// Carrier now.
    pub signal: bool,
    /// Signed speed (1.0 = 33⅓ at 0 %); 0 with no signal or while settling.
    pub speed: f32,
    /// Bit lock (position-grade).
    pub locked: bool,
    /// Needle just landed: hold silent until a position.
    pub settling: bool,
    /// Loudest sample over ~100 ms, 0..1.
    pub peak: f32,
    /// Place in the track (ms at 33⅓ past the record's first groove).
    pub pos_ms: Option<i32>,
    /// The decoder's position before the start offset.
    pub raw_ms: Option<i32>,
    /// Needle drops placed so far (settling ended on a bit lock).
    pub drops: u32,
}

const MIN_PEAK: i16 = 600;         // about -35 dBFS
const MAX_SPEED: f32 = 3.0;
const MIN_SPEED: f32 = 0.02;
const ARM_MS: u32 = 300;
const DROP_MS: u32 = 40;
const WINDOW_MS: u32 = 10;
/// A gap this long counts as a needle lift. Shorter is a hand holding the
/// platter between strokes, which must never go silent (150 ms, the P4's,
/// made a scratch skip on every stop).
const LIFT_MS: u32 = 500;
const SETTLE_MAX_MS: u32 = 1000;
/// Dan's Serato Control Vinyl: the first groove reads ~24.0 s.
pub const START_MS: i32 = 24_000;

/// The input side: feed it the interface's stereo input.
pub struct Vinyl {
    dec: Decoder,
    rate: u32,
    absolute: bool,
    start_ms: i32,
    // Clock in input frames.
    now: u64,
    window_left: u32,
    window_peak: i16,
    shown_peak: i16,
    peak_age_ms: u32,
    last_carrier: Option<u64>,
    steady_since: Option<u64>,
    landed: u64,
    signal: bool,
    state: VinylState,
}

impl Vinyl {
    pub fn new(rate: u32, fmt: Format) -> Vinyl {
        crate::decoder::prebuild(fmt);
        Vinyl {
            dec: Decoder::new(rate, fmt), rate, absolute: true, start_ms: START_MS, now: 0,
            window_left: rate * WINDOW_MS / 1000, window_peak: 0, shown_peak: 0, peak_age_ms: 0,
            last_carrier: None, steady_since: None, landed: 0, signal: false, state: VinylState::default(),
        }
    }

    pub fn rate(&self) -> u32 { self.rate }
    pub fn state(&self) -> VinylState { self.state }
    /// ABS settles on a needle drop; REL and INT don't.
    pub fn set_absolute(&mut self, on: bool) { self.absolute = on; }
    pub fn set_flags(&mut self, f: u32) { self.dec.set_flags(f); }
    pub fn flags(&self) -> u32 { self.dec.flags() }
    pub fn start_ms(&self) -> i32 { self.start_ms }
    pub fn set_start_ms(&mut self, ms: i32) { self.start_ms = ms; }
    /// The record's first groove is where the needle is now.
    pub fn set_start_here(&mut self) -> bool {
        match self.state.raw_ms { Some(r) => { self.start_ms = r; true } None => false }
    }

    /// Interleaved stereo i16.
    pub fn push_i16(&mut self, stereo: &[i16]) {
        for f in stereo.chunks_exact(2) { self.frame(f[0], f[1]); }
    }

    /// Interleaved f32 with `ch` channels, taking channels `left` and
    /// `left + 1` (an interface's input pair).
    pub fn push_f32(&mut self, data: &[f32], ch: usize, left: usize) {
        if ch == 0 { return; }
        for f in data.chunks_exact(ch) {
            let l = f[left.min(ch - 1)];
            let r = f[(left + 1).min(ch - 1)];
            self.frame((l.clamp(-1.0, 1.0) * 32767.0) as i16, (r.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
    }

    #[inline]
    fn frame(&mut self, l: i16, r: i16) {
        let a = l.unsigned_abs().max(r.unsigned_abs()).min(i16::MAX as u16) as i16;
        if a > self.window_peak { self.window_peak = a; }
        self.dec.push_frame(l, r);
        self.now += 1;
        self.window_left -= 1;
        if self.window_left == 0 {
            self.window_left = self.rate * WINDOW_MS / 1000;
            self.window();
        }
    }

    fn ms(&self, frames: u64) -> u64 { frames * 1000 / self.rate as u64 }

    fn window(&mut self) {
        let now = self.now;
        let sp = self.dec.speed();
        let m = sp.abs();
        let carrier = self.window_peak >= MIN_PEAK && (MIN_SPEED..=MAX_SPEED).contains(&m);
        let gap_ms = self.last_carrier.map_or(u64::MAX, |t| self.ms(now - t));
        if carrier { self.last_carrier = Some(now); }
        let was = self.signal;
        self.signal = self.last_carrier.is_some_and(|t| self.ms(now - t) < DROP_MS as u64);
        if self.signal && !was && gap_ms >= LIFT_MS as u64 && self.absolute {
            self.state.settling = true;
            self.landed = now;
            self.dec.relock();
        }
        if self.state.settling {
            let ms = self.ms(now - self.landed);
            if self.dec.locked() && self.dec.position() >= 0 {
                self.state.settling = false;
                self.state.drops += 1;
                log::info!("vinyl: needle down, bit lock in {ms} ms");
            } else if ms >= SETTLE_MAX_MS as u64 || !self.signal {
                self.state.settling = false;
                log::info!("vinyl: needle down, no bit lock in {ms} ms: following speed only");
            }
        }
        if carrier && m > 0.6 && m < 1.5 {
            let since = *self.steady_since.get_or_insert(now);
            if !self.state.armed && self.ms(now - since) >= ARM_MS as u64 {
                self.state.armed = true;
                log::info!("vinyl: carrier, the record drives the deck (peak {}, speed {sp:.3})", self.window_peak);
            }
        } else {
            self.steady_since = None;
        }
        self.peak_age_ms += WINDOW_MS;
        if self.window_peak > self.shown_peak || self.peak_age_ms >= 100 { self.shown_peak = self.window_peak; self.peak_age_ms = 0; }
        self.window_peak = 0;

        let s = &mut self.state;
        s.signal = self.signal;
        s.speed = if self.signal && !s.settling { sp } else { 0.0 };
        s.locked = self.dec.locked();
        s.peak = self.shown_peak as f32 / 32767.0;
        s.raw_ms = (self.signal && self.dec.locked() && self.dec.position() >= 0)
            .then(|| (self.dec.position() as i64 * 1000 / self.dec.resolution_hz() as i64) as i32);
        s.pos_ms = s.raw_ms.filter(|&r| r >= self.start_ms).map(|r| r - self.start_ms);
    }
}

/// Absolute mode, how the deck keeps to the needle. On the turntable, back
/// cueing and scratching jumped the deck about whenever it re-synced (the
/// deck's speed trails the hand), so a jump is taken only where one is
/// meant: a needle drop, the first lock, or a gap of ≥300 ms once the record
/// has played steadily for STEADY_MS (the deck was moved under it). Smaller
/// gaps, once steady, close by bending the speed (at most ±2 %); while the
/// platter is handled the deck follows speed alone, as Serato does.
pub const SEEK_MS: f64 = 300.0;
pub const SEEK_GAP_MS: f64 = 250.0;
/// Steady: forward or back at 0.5–2×, the reading within 3 % of the
/// smoothed speed, this long.
pub const STEADY_MS: f64 = 300.0;
/// A gap under this is left alone.
pub const DEAD_MS: f64 = 4.0;
/// The bend closes a gap with this time constant, at most BEND_MAX.
pub const BEND_TAU_MS: f64 = 800.0;
pub const BEND_MAX: f64 = 0.02;
/// The record's speed, smoothed with this time constant (the P4's 0.35 per
/// 5.8 ms block): quick enough for a hand on the platter.
pub const SMOOTH_MS: f64 = 13.5;

/// What the deck does this block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriveOut {
    /// Signed playback speed (0 = still).
    pub speed: f64,
    /// Jump the playhead here (ms of track).
    pub seek_ms: Option<f64>,
}

/// The output side: the record's state in, the deck's speed and jumps out.
#[derive(Default)]
pub struct Drive {
    speed: f64,
    steady_ms: f64,
    placed: bool,
    drops_seen: u32,
    last_seek: Option<f64>,
    /// Stats: jumps taken, and of those needle drops (and first locks).
    pub seeks: u32,
    pub drops: u32,
}

impl Drive {
    pub fn speed(&self) -> f64 { self.speed }
    pub fn reset(&mut self) { *self = Drive { seeks: self.seeks, drops: self.drops, ..Default::default() }; }

    /// One block of `dt_ms` at `now_ms`. `pos_ms` is the deck's playhead
    /// (what it renders now); `lead_ms` how far the sound trails the record
    /// (input and output buffers); `follow` = ABS and nothing (a loop)
    /// holds the deck.
    #[allow(clippy::too_many_arguments)]
    pub fn step(&mut self, st: &VinylState, follow: bool, now_ms: f64, dt_ms: f64, pos_ms: f64, lead_ms: f64, len_ms: f64) -> DriveOut {
        let a = 1.0 - (-dt_ms / SMOOTH_MS).exp();
        self.speed += a * (st.speed as f64 - self.speed);
        if st.speed == 0.0 && self.speed.abs() < 0.002 { self.speed = 0.0; }
        let m = (st.speed as f64).abs();
        let steady = (0.5..=2.0).contains(&m) && (st.speed as f64 - self.speed).abs() <= 0.03 * m;
        self.steady_ms = if steady { self.steady_ms + dt_ms } else { 0.0 };
        let mut out = DriveOut { speed: self.speed, seek_ms: None };
        let Some(p) = st.pos_ms.filter(|_| follow) else { return out };
        let target = p as f64 + lead_ms * self.speed;
        let err = target - pos_ms;                    // > 0: the deck is behind the needle
        let dropped = st.drops != self.drops_seen;
        self.drops_seen = st.drops;
        let jump = if dropped || !self.placed {
            err.abs() >= DEAD_MS
        } else {
            self.steady_ms >= STEADY_MS && err.abs() >= SEEK_MS && self.last_seek.is_none_or(|t| now_ms - t >= SEEK_GAP_MS)
        };
        self.placed = true;
        if jump && target < len_ms {
            self.seeks += 1;
            if dropped { self.drops += 1; }
            self.last_seek = Some(now_ms);
            out.seek_ms = Some(target.max(0.0));
        } else if self.steady_ms >= STEADY_MS && err.abs() >= DEAD_MS {
            // Bend toward the needle: the speed's sign is the direction, so
            // the bend goes with it.
            let bend = (err / BEND_TAU_MS).clamp(-BEND_MAX, BEND_MAX) * m;
            out.speed = self.speed + bend;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::synth::Signal;

    const SR: u32 = 48_000;

    /// Play `secs` of record at `speed`, 10 ms at a time.
    fn play(v: &mut Vinyl, sig: &mut Signal, speed: f64, secs: f64) {
        let mut buf = Vec::new();
        for _ in 0..(secs * 100.0).round() as usize {
            buf.clear();
            sig.render(speed, SR as usize / 100, &mut buf);
            v.push_i16(&buf);
        }
    }

    fn silence(v: &mut Vinyl, secs: f64) { v.push_i16(&vec![0; (secs * SR as f64) as usize * 2]); }

    fn at_ms(ms: f64) -> f64 { START_MS as f64 + ms }   // cycles = ms at 1 kHz

    #[test]
    fn arms_after_a_steady_carrier_only() {
        let mut v = Vinyl::new(SR, Format::SeratoVinyl);
        let mut sig = Signal::new(Format::SeratoVinyl, SR, at_ms(60_000.0));
        play(&mut v, &mut sig, 1.0, 0.2);
        assert!(!v.state().armed, "not after 0.2 s");
        play(&mut v, &mut sig, 1.0, 0.3);
        let s = v.state();
        assert!(s.armed && s.signal && s.locked);
        assert!((s.speed - 1.0).abs() < 0.02);
        let p = s.pos_ms.unwrap();
        assert!((p - 60_500).abs() < 20, "track position {p}");
    }

    #[test]
    fn a_scratch_never_arms_it_but_backwards_reads_once_armed() {
        let mut v = Vinyl::new(SR, Format::SeratoVinyl);
        let mut sig = Signal::new(Format::SeratoVinyl, SR, at_ms(30_000.0));
        for _ in 0..10 { play(&mut v, &mut sig, 2.5, 0.05); play(&mut v, &mut sig, -2.5, 0.05); }
        assert!(!v.state().armed);
        play(&mut v, &mut sig, 1.0, 0.5);
        play(&mut v, &mut sig, -1.0, 0.2);
        assert!(v.state().armed);
        assert!(v.state().speed < -0.9, "backwards {}", v.state().speed);
    }

    #[test]
    fn stopping_the_record_reads_zero_and_keeps_it_armed() {
        let mut v = Vinyl::new(SR, Format::SeratoVinyl);
        let mut sig = Signal::new(Format::SeratoVinyl, SR, at_ms(1000.0));
        play(&mut v, &mut sig, 1.0, 0.6);
        silence(&mut v, 0.12);
        let s = v.state();
        assert!(s.armed && !s.signal);
        assert_eq!(s.speed, 0.0);
    }

    #[test]
    fn needle_drop_settles_silent_briefly_then_reads_the_new_place() {
        let mut v = Vinyl::new(SR, Format::SeratoVinyl);
        let mut sig = Signal::new(Format::SeratoVinyl, SR, at_ms(10_000.0));
        play(&mut v, &mut sig, 1.0, 0.6);
        silence(&mut v, 0.6);                        // lift
        let mut sig = Signal::new(Format::SeratoVinyl, SR, at_ms(200_000.0));
        play(&mut v, &mut sig, 1.0, 0.02);
        assert!(v.state().settling, "silent while it finds its place");
        assert_eq!(v.state().speed, 0.0);
        play(&mut v, &mut sig, 1.0, 0.04);
        let s = v.state();
        assert!(!s.settling, "placed within 60 ms (the bit level is kept)");
        assert!((s.pos_ms.unwrap() - 200_060).abs() < 15, "{:?}", s.pos_ms);
    }

    /// A scratch: strokes back and forth with the hand holding the platter
    /// still for 300 ms between them. Every stroke plays at once; nothing
    /// is taken for a needle drop.
    #[test]
    fn a_hand_holding_the_platter_is_not_a_needle_lift() {
        let mut v = Vinyl::new(SR, Format::SeratoVinyl);
        let mut sig = Signal::new(Format::SeratoVinyl, SR, at_ms(60_000.0));
        v.set_absolute(true);
        play(&mut v, &mut sig, 1.0, 0.6);
        for dir in [-1.5, 1.5, -1.0, 2.0] {
            silence(&mut v, 0.3);
            play(&mut v, &mut sig, dir, 0.03);
            let s = v.state();
            assert!(!s.settling, "{dir}: not settling");
            assert!((s.speed as f64 - dir).abs() < 0.3, "{dir}: plays at once, read {}", s.speed);
            play(&mut v, &mut sig, dir, 0.2);
        }
    }

    #[test]
    fn before_the_first_groove_there_is_no_track_position() {
        let mut v = Vinyl::new(SR, Format::SeratoVinyl);
        let mut sig = Signal::new(Format::SeratoVinyl, SR, 5000.0);
        play(&mut v, &mut sig, 1.0, 0.6);
        assert!(v.state().raw_ms.is_some());
        assert_eq!(v.state().pos_ms, None);
        assert!(v.set_start_here());
        play(&mut v, &mut sig, 1.0, 0.1);
        assert!(v.state().pos_ms.unwrap() < 150);
    }

    #[test]
    fn f32_input_from_an_interface_pair() {
        let mut v = Vinyl::new(SR, Format::SeratoVinyl);
        let mut sig = Signal::new(Format::SeratoVinyl, SR, at_ms(5000.0));
        let mut buf = Vec::new();
        sig.render(1.0, SR as usize, &mut buf);
        // Four inputs, the record on 3/4.
        let four: Vec<f32> = buf.chunks(2).flat_map(|f| [0.0, 0.0, f[0] as f32 / 32768.0, f[1] as f32 / 32768.0]).collect();
        v.push_f32(&four, 4, 2);
        assert!(v.state().armed && v.state().locked);
    }

    fn steady(pos: i32) -> VinylState { VinylState { armed: true, signal: true, speed: 1.0, locked: true, pos_ms: Some(pos), ..Default::default() } }

    #[test]
    fn drive_places_on_first_lock_then_follows_speed() {
        let mut d = Drive::default();
        let o = d.step(&steady(10_000), true, 0.0, 10.0, 0.0, 10.0, 300_000.0);
        assert!(o.seek_ms.is_some(), "the first lock places the deck");
        let mut pos = o.seek_ms.unwrap();
        for i in 1..50 {
            let o = d.step(&steady(10_000 + i * 10), true, i as f64 * 10.0, 10.0, pos, 10.0, 300_000.0);
            assert!(o.seek_ms.is_none(), "{i}: no jumps while it keeps up");
            pos += o.speed * 10.0;
        }
    }

    #[test]
    fn drive_jumps_on_a_needle_drop() {
        let mut d = Drive::default();
        d.step(&steady(10_000), true, 0.0, 10.0, 10_000.0, 0.0, 300_000.0);
        let drop = VinylState { drops: 1, ..steady(60_000) };
        let o = d.step(&drop, true, 10.0, 10.0, 10_010.0, 10.0, 300_000.0);
        let m = o.seek_ms.expect("a needle drop jumps");
        assert!((m - 60_008.0).abs() < 3.0, "to the needle plus the lead at the deck's speed: {m}");
    }

    /// Back-cueing: the deck trails the hand and the gap swings hundreds of
    /// ms; it must not jump while the platter is handled.
    #[test]
    fn drive_never_jumps_while_the_platter_is_handled() {
        let mut d = Drive::default();
        d.step(&steady(10_000), true, 0.0, 10.0, 10_000.0, 0.0, 300_000.0);
        for i in 0..200 {
            let sp = if (i / 15) % 2 == 0 { 1.8 } else { -1.8 };
            let st = VinylState { speed: sp, ..steady(10_000 + (i % 7) * 120) };
            assert!(d.step(&st, true, 10.0 + i as f64 * 10.0, 10.0, 10_000.0, 10.0, 300_000.0).seek_ms.is_none(), "{i}");
        }
    }

    #[test]
    fn drive_bends_small_gaps_shut_once_steady_and_jumps_big_ones() {
        let mut d = Drive::default();
        d.step(&steady(10_000), true, 0.0, 10.0, 10_000.0, 0.0, 300_000.0);
        let mut pos = 10_000.0 - 40.0;                    // 40 ms behind after a scratch
        let mut bent = false;
        for i in 1..400 {
            let o = d.step(&steady(10_000 + i * 10), true, i as f64 * 10.0, 10.0, pos, 0.0, 300_000.0);
            assert!(o.seek_ms.is_none());
            if o.speed > 1.001 { bent = true; }
            assert!(o.speed <= 1.0 + BEND_MAX + 1e-9);
            pos += o.speed * 10.0;
        }
        // The needle at the next step is 10 000 + 400 × 10.
        assert!(bent && (14_000.0 - pos).abs() < DEAD_MS + 1.0, "closed by bending: {}", 14_000.0 - pos);
        // Moved 30 s under a steady record: one jump.
        let o = d.step(&steady(14_000), true, 4_000.0, 10.0, 44_000.0, 0.0, 300_000.0);
        assert_eq!(o.seek_ms.map(|m| m.round()), Some(14_000.0));
        // REL never seeks.
        assert!(d.step(&VinylState { drops: 9, ..steady(90_000) }, false, 5_000.0, 10.0, 0.0, 0.0, 300_000.0).seek_ms.is_none());
    }
}
