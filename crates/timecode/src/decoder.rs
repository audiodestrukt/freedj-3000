//! Serato Control Vinyl / Control CD decoder.
//!
//! A Rust port of freedj-esp's `src/timecode.cpp` (Dan's ESP32 decoder,
//! proven on a turntable with Serato Control Vinyl through a PCM2902). That
//! one is a clean-room take on xwax's approach, modified on the bench; no
//! xwax code is used. The format constants are published facts about
//! Serato's media.
//!
//! Integer arithmetic follows the C++ exactly (samples shifted up 16 bits,
//! the same EMAs and thresholds) so both decoders read the same signal the
//! same way; `tests::control_cd_parity` checks this against the Control CD
//! recording when it is present.
//!
//! ```text
//! stereo in ─► per-channel zero crossing (DC-tracked, hysteresis)
//!                │ direction: which channel leads at a crossing
//!                │ speed: the primary channel's half-period
//!                ▼
//!            bit at each secondary crossing (loud = 1, quiet = 0)
//!                ▼
//!            20-bit LFSR predictor ► table ► absolute cycle position
//! ```

use std::sync::OnceLock;

/// Invert the direction decision (a record that reads backwards).
pub const SWITCH_PHASE: u32 = 0x1;
/// Swap primary and secondary (L and R swapped in the cabling).
pub const SWITCH_PRIMARY: u32 = 0x2;
/// Flip the bit polarity.
pub const SWITCH_POLARITY: u32 = 0x4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// Serato Control Vinyl (CV02), side A: 1 kHz, 712 000 cycles.
    SeratoVinyl,
    /// Serato Control CD: 1 kHz, 950 000 cycles.
    SeratoCd,
}

struct Def {
    resolution: u32,
    bits: u32,
    seed: u32,
    taps: u32,
    length: u32,
}

const fn def(f: Format) -> Def {
    match f {
        Format::SeratoVinyl => Def { resolution: 1000, bits: 20, seed: 0x59017, taps: 0x361e4, length: 712_000 },
        Format::SeratoCd    => Def { resolution: 1000, bits: 20, seed: 0xd8b40, taps: 0x34d54, length: 950_000 },
    }
}

impl Format {
    pub fn resolution_hz(self) -> u32 { def(self).resolution }
    /// The media's whole length in ms (vinyl 712 s, CD 950 s).
    pub fn total_ms(self) -> u32 { (def(self).length as u64 * 1000 / def(self).resolution as u64) as u32 }
    pub fn label(self) -> &'static str { match self { Format::SeratoVinyl => "SERATO CV02", Format::SeratoCd => "SERATO CD" } }
}

const SAMPLE_SHIFT: u32 = 16;
const ZERO_THRESH: i32 = 128 << 16;
const ZERO_RC: f32 = 0.001;
const REF_PEAKS_AVG: i32 = 48;
const VALID_BITS: u32 = 24;
const SPEED_EMA_ALPHA: f32 = 0.15;

#[inline]
fn parity(code: u32, taps: u32) -> u32 { (code & taps).count_ones() & 1 }

/// Advance: the new bit enters at the MSB.
#[inline]
pub(crate) fn lfsr_fwd(cur: u32, taps: u32, bits: u32) -> u32 {
    let b = parity(cur, taps | 1);
    (cur >> 1) | (b << (bits - 1))
}

/// Rewind: the new bit enters at the LSB; `rev(fwd(x)) == x`.
#[inline]
pub(crate) fn lfsr_rev(cur: u32, taps: u32, bits: u32) -> u32 {
    let mask = (1u32 << bits) - 1;
    let b = parity(cur, (taps >> 1) | (1 << (bits - 1)));
    ((cur << 1) & mask) | b
}

const NONE: u32 = u32::MAX;

/// State → cycle index, dense (2^20 entries, 4 MB), built once per format.
fn table(f: Format) -> &'static [u32] {
    static VINYL: OnceLock<Vec<u32>> = OnceLock::new();
    static CD: OnceLock<Vec<u32>> = OnceLock::new();
    let cell = match f { Format::SeratoVinyl => &VINYL, Format::SeratoCd => &CD };
    cell.get_or_init(|| {
        let d = def(f);
        let mut t = vec![NONE; 1 << d.bits];
        let mut cur = d.seed;
        for i in 0..d.length {
            t[cur as usize] = i;
            cur = lfsr_fwd(cur, d.taps, d.bits);
        }
        t
    })
}

/// Build the position table now (tens of ms) rather than on the first
/// locked bit, which would stall the audio input callback.
pub fn prebuild(f: Format) { let _ = table(f); }

#[derive(Clone, Copy, Default)]
struct Channel {
    positive: bool,
    swapped: bool,
    zero: i32,
    crossing_ticker: u32,
}

impl Channel {
    #[inline]
    fn update(&mut self, v: i32, alpha: f32, thr: i32) {
        self.crossing_ticker = self.crossing_ticker.wrapping_add(1);
        self.swapped = false;
        if v > self.zero.saturating_add(thr) && !self.positive {
            self.swapped = true;
            self.positive = true;
            self.crossing_ticker = 0;
        } else if v < self.zero.saturating_sub(thr) && self.positive {
            self.swapped = true;
            self.positive = false;
            self.crossing_ticker = 0;
        }
        self.zero = self.zero.wrapping_add((alpha * v.wrapping_sub(self.zero) as f32) as i32);
    }
}

pub struct Decoder {
    sample_rate: u32,
    fmt: Format,
    bits: u32,
    taps: u32,
    flags: u32,
    resolution: u32,
    zero_alpha: f32,
    ref_period: f32,
    primary: Channel,
    secondary: Channel,
    since_primary_zc: u32,
    forwards: bool,
    ref_level: i32,
    bitstream: u32,
    expected: u32,
    valid: u32,
    speed: f32,
    position: i32,
    locked: bool,
}

impl Decoder {
    pub fn new(sample_rate: u32, fmt: Format) -> Decoder {
        let d = def(fmt);
        let dt = 1.0 / sample_rate as f32;
        let mut dec = Decoder {
            sample_rate, fmt, bits: d.bits, taps: d.taps, flags: 0, resolution: d.resolution,
            zero_alpha: dt / (ZERO_RC + dt),
            ref_period: sample_rate as f32 / d.resolution as f32,
            primary: Channel::default(), secondary: Channel::default(),
            since_primary_zc: 0, forwards: true, ref_level: 0, bitstream: 0, expected: 0, valid: 0,
            speed: 0.0, position: -1, locked: false,
        };
        dec.reset();
        dec
    }

    pub fn reset(&mut self) {
        self.primary = Channel::default();
        self.secondary = Channel::default();
        self.since_primary_zc = 0;
        self.forwards = true;
        self.ref_level = 0;
        self.bitstream = 0;
        self.expected = 0;
        self.valid = 0;
        self.speed = 0.0;
        self.position = -1;
        self.locked = false;
    }

    /// SWITCH_* flags, replacing the old ones; resets the decoder.
    pub fn set_flags(&mut self, flags: u32) { self.flags = flags; self.reset(); }
    pub fn flags(&self) -> u32 { self.flags }
    pub fn format(&self) -> Format { self.fmt }
    pub fn sample_rate(&self) -> u32 { self.sample_rate }

    /// Signed speed (1.0 = the reference speed, negative = backwards).
    pub fn speed(&self) -> f32 { self.speed }
    /// Cycle index on the media once bit-locked, else -1.
    pub fn position(&self) -> i32 { self.position }
    pub fn locked(&self) -> bool { self.locked }
    pub fn resolution_hz(&self) -> u32 { self.resolution }

    /// Interleaved stereo i16.
    pub fn push_i16(&mut self, stereo: &[i16]) {
        for f in stereo.chunks_exact(2) { self.push_frame(f[0], f[1]); }
    }

    /// One stereo frame.
    #[inline]
    pub fn push_frame(&mut self, l: i16, r: i16) {
        let l = (l as i32) << SAMPLE_SHIFT;
        let r = (r as i32) << SAMPLE_SHIFT;
        // Default: primary = R, secondary = L.
        if self.flags & SWITCH_PRIMARY != 0 { self.sample(l, r) } else { self.sample(r, l) }
    }

    fn sample(&mut self, primary: i32, secondary: i32) {
        self.primary.update(primary, self.zero_alpha, ZERO_THRESH);
        self.secondary.update(secondary, self.zero_alpha, ZERO_THRESH);

        if self.primary.swapped || self.secondary.swapped {
            let mut fwd = if self.primary.swapped {
                self.primary.positive != self.secondary.positive
            } else {
                self.primary.positive == self.secondary.positive
            };
            if self.flags & SWITCH_PHASE != 0 { fwd = !fwd; }
            if fwd != self.forwards {
                self.forwards = fwd;
                self.valid = 0;
            }
        }

        self.since_primary_zc += 1;
        if self.primary.swapped {
            let half = self.since_primary_zc;
            self.since_primary_zc = 0;
            if half > 0 {
                let ratio = self.ref_period / (2.0 * half as f32);
                let s = if self.forwards { ratio } else { -ratio };
                self.speed = (1.0 - SPEED_EMA_ALPHA) * self.speed + SPEED_EMA_ALPHA * s;
            }
        }

        let polarity = self.flags & SWITCH_POLARITY == 0;
        if self.secondary.swapped && self.primary.positive == polarity {
            let m = (primary / 2 - self.primary.zero / 2).wrapping_abs();
            self.bit(m);
        }

        let silence = self.sample_rate / 10;
        if self.primary.crossing_ticker > silence && self.secondary.crossing_ticker > silence {
            self.speed = 0.0;
            self.locked = false;
            return;
        }
        self.locked = self.valid > VALID_BITS && self.speed.abs() > 0.05 && self.speed.abs() < 4.0;
    }

    fn bit(&mut self, m: i32) {
        if self.ref_level == 0 { self.ref_level = i32::MAX / 2; }
        let b = (m > self.ref_level) as u32;
        if self.forwards {
            self.expected = lfsr_fwd(self.expected, self.taps, self.bits);
            self.bitstream = (self.bitstream >> 1) | (b << (self.bits - 1));
        } else {
            let mask = (1u32 << self.bits) - 1;
            self.expected = lfsr_rev(self.expected, self.taps, self.bits);
            self.bitstream = ((self.bitstream << 1) & mask) | b;
        }
        if self.expected == self.bitstream {
            self.valid += 1;
        } else {
            self.expected = self.bitstream;
            self.valid = 0;
        }
        self.ref_level -= self.ref_level / REF_PEAKS_AVG;
        self.ref_level += m / REF_PEAKS_AVG;
        if self.valid > VALID_BITS {
            let p = table(self.fmt)[self.bitstream as usize];
            // The table names a window by its newest bit going forward.
            // Backwards the newest bit is the window's oldest, `bits - 1`
            // cycles earlier: without this a reversed record read 19 ms
            // ahead of the needle, and ABS kept re-syncing (jerky reverse).
            // freedj-esp's C++ has the same offset.
            if p != NONE {
                self.position = if self.forwards { p as i32 } else { (p as i32 - (self.bits as i32 - 1)).max(0) };
            }
        }
    }
}

/// Test signals: a Serato-shaped carrier whose loud/quiet cycles spell the
/// LFSR sequence, so the decoder can be checked for speed, direction and
/// position without a record. Primary (R) leads secondary (L) by 90°
/// going forward; a 1 bit's primary peak is louder. Public for the app's
/// tests.
pub mod synth {
    use super::*;

    pub struct Signal {
        sr: f64,
        resolution: f64,
        /// The bit each cycle carries: the MSB of the LFSR state there.
        bits: Vec<bool>,
        /// Phase in cycles (one cycle = one bit).
        pub phase: f64,
    }

    impl Signal {
        /// Starting at cycle `at` of the media.
        pub fn new(fmt: Format, sample_rate: u32, at: f64) -> Signal {
            let d = def(fmt);
            let mut bits = Vec::with_capacity(d.length as usize);
            let mut s = d.seed;
            for _ in 0..d.length { bits.push((s >> (d.bits - 1)) & 1 == 1); s = lfsr_fwd(s, d.taps, d.bits); }
            Signal { sr: sample_rate as f64, resolution: d.resolution as f64, bits, phase: at }
        }

        /// `frames` stereo frames at `speed` (signed), interleaved i16,
        /// at `level` (1.0 = a healthy line-level record).
        pub fn render_at(&mut self, speed: f64, level: f64, frames: usize, out: &mut Vec<i16>) {
            let step = speed * self.resolution / self.sr;
            for _ in 0..frames {
                let c = (self.phase.floor().max(0.0) as usize).min(self.bits.len() - 1);
                let amp = if self.bits[c] { 20000.0 } else { 14000.0 } * level;
                let a = self.phase * std::f64::consts::TAU;
                // Primary (R) = cos, secondary (L) = sin: the bit is read
                // as L crosses zero going up, where R peaks.
                out.push((17000.0 * level * a.sin()) as i16);
                out.push((amp * a.cos()) as i16);
                self.phase += step;
            }
        }

        pub fn render(&mut self, speed: f64, frames: usize, out: &mut Vec<i16>) { self.render_at(speed, 1.0, frames, out) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn carrier(freq: f64, lead: f64, secs: f64) -> Vec<i16> {
        let sr = 44_100.0;
        let n = (secs * sr) as usize;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f64 / sr * freq * std::f64::consts::TAU;
            v.push((8000.0 * t.sin()) as i16);
            v.push((8000.0 * (t + lead).sin()) as i16);
        }
        v
    }

    #[test]
    fn lfsr_steps_back_exactly() {
        let d = def(Format::SeratoVinyl);
        let mut s = d.seed;
        for _ in 0..5000 {
            let n = lfsr_fwd(s, d.taps, d.bits);
            assert_eq!(lfsr_rev(n, d.taps, d.bits), s);
            s = n;
        }
    }

    #[test]
    fn speed_and_direction_from_a_plain_carrier() {
        for (freq, lead, want) in [(1000.0, std::f64::consts::FRAC_PI_2, 1.0), (500.0, std::f64::consts::FRAC_PI_2, 0.5),
                                   (1500.0, std::f64::consts::FRAC_PI_2, 1.5), (1000.0, -std::f64::consts::FRAC_PI_2, -1.0)] {
            let mut d = Decoder::new(44_100, Format::SeratoVinyl);
            d.push_i16(&carrier(freq, lead, 0.5));
            assert!((d.speed() - want as f32).abs() < 0.03, "{freq} Hz lead {lead}: speed {}", d.speed());
        }
    }

    #[test]
    fn silence_is_stopped() {
        let mut d = Decoder::new(44_100, Format::SeratoVinyl);
        d.push_i16(&carrier(1000.0, std::f64::consts::FRAC_PI_2, 0.3));
        d.push_i16(&vec![0; 44_100]);
        assert_eq!(d.speed(), 0.0);
        assert!(!d.locked());
    }

    #[test]
    fn synthetic_record_locks_and_reads_its_position() {
        for sr in [44_100u32, 48_000] {
            let at = 100_000.0;
            let mut sig = synth::Signal::new(Format::SeratoVinyl, sr, at);
            let mut d = Decoder::new(sr, Format::SeratoVinyl);
            let mut buf = Vec::new();
            sig.render(1.0, sr as usize / 2, &mut buf);
            d.push_i16(&buf);
            assert!(d.locked(), "{sr}: locked");
            let expect = sig.phase as i32;
            assert!((d.position() - expect).abs() <= 2, "{sr}: position {} vs {expect}", d.position());
            assert!((d.speed() - 1.0).abs() < 0.02);
        }
    }

    #[test]
    fn backwards_reads_the_needle_not_19_ms_ahead() {
        let sr = 48_000;
        let mut sig = synth::Signal::new(Format::SeratoVinyl, sr, 100_000.0);
        let mut d = Decoder::new(sr, Format::SeratoVinyl);
        let mut buf = Vec::new();
        sig.render(1.0, sr as usize / 2, &mut buf);
        for sp in [-1.0, -0.5, -2.0] {
            sig.render(sp, sr as usize / 2, &mut buf);
            d.push_i16(&buf);
            buf.clear();
            assert!(d.locked(), "{sp}");
            assert!((d.position() - sig.phase as i32).abs() <= 1, "at {sp}x: {} vs the needle {}", d.position(), sig.phase as i32);
        }
    }

    /// Bit-for-bit with the C++ decoder on the Serato Control CD recording
    /// (freedj-esp/timecode.wav; never committed): the same positions and
    /// speed as freedj-esp's `native/timecode_analyze <wav> seek <s> <dur>`
    /// at five places. Set OPENDECK_TC_WAV (an absolute path) to run.
    #[test]
    fn control_cd_parity() {
        let Ok(path) = std::env::var("OPENDECK_TC_WAV") else { return };
        let bytes = std::fs::read(path).unwrap();
        let data = bytes.windows(4).position(|w| w == b"data").unwrap() + 8;
        let pcm: Vec<i16> = bytes[data..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        // (offset s, window s, the C++ analyser's position at the window's end)
        for (at, secs, want) in [(120, 2, 122_604), (10, 1, 11_604), (300, 1, 301_604), (600, 1, 601_604), (900, 1, -1)] {
            let start = at * 44_100 * 2;
            let mut d = Decoder::new(44_100, Format::SeratoCd);
            d.push_i16(&pcm[start..start + secs * 44_100 * 2]);
            assert_eq!(d.position(), want, "from {at} s");
            assert!((d.speed() - 0.996).abs() < 0.0006, "from {at} s: speed {}", d.speed());
        }
    }
}
