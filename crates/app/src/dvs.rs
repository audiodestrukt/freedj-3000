//! DVS: a timecode record on a turntable drives the deck (MENU → DVS).
//!
//! ```text
//! interface input ─► DvsInput (cpal input callback)
//!                      opendeck_timecode::Vinyl: armed, speed, position
//!                      │ DvsShared (atomics, every 10 ms)
//!                      ▼
//! output callback ─► DvsRender: while the record has the deck, the track
//!                    is read here at the record's signed speed (varispeed,
//!                    backwards, still), not through the processor and ring:
//!                    the sound is one output buffer behind the hand.
//! ```
//!
//! The decoder and the arm / needle-drop / follow rules are freedj-esp's,
//! from its P4 deck (crates/timecode). Modes as Serato's: ABS (needle
//! position is track position, 0:00 at the first groove), REL (speed and
//! direction only; cues and jumps on the deck still work) and INT (the
//! record is ignored, the deck plays on at the record's last speed).
//!
//! While the record drives: key lock is off (the pitch follows the
//! platter), stems and SLIP are not applied, loops wrap going forward and
//! win over ABS, as in Serato.

use crate::audio::NO_SEEK_PUB as NO_SEEK;
use arc_swap::ArcSwap;
use opendeck_timecode::vinyl::{Drive, VinylState};
use std::sync::{
    atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering},
    Arc,
};

/// MENU → DVS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum DvsMode {
    #[default]
    Off,
    Abs,
    Rel,
    Int,
}

impl DvsMode {
    pub fn label(self) -> &'static str {
        match self { DvsMode::Off => "OFF", DvsMode::Abs => "ABS", DvsMode::Rel => "REL", DvsMode::Int => "INT" }
    }
    pub const ALL: [DvsMode; 4] = [DvsMode::Off, DvsMode::Abs, DvsMode::Rel, DvsMode::Int];
    fn from_u8(v: u8) -> DvsMode { *Self::ALL.get(v as usize).unwrap_or(&DvsMode::Off) }
    fn to_u8(self) -> u8 { Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as u8 }
}

/// Between the input callback, the output callback and the app.
#[derive(Default)]
pub struct DvsShared {
    mode: AtomicU8,
    // The record (input → output, app), every 10 ms; a torn read across
    // two windows is harmless.
    armed: AtomicBool,
    signal: AtomicBool,
    locked: AtomicBool,
    settling: AtomicBool,
    speed: AtomicU32,
    peak: AtomicU32,
    pos_ms: AtomicI32,
    raw_ms: AtomicI32,
    drops: AtomicU32,
    /// Input buffer length, µs (half the sound's lag behind the record).
    pub in_us: AtomicU32,
    /// Bumped on every input buffer: the app sees the input is alive.
    pub in_seq: AtomicU64,
    // The output (output → app, processor).
    /// The record has the deck: the output callback renders it, the
    /// processor stands aside.
    pub driving: AtomicBool,
    /// The deck's signed speed as the record drives it (f32 bits).
    pub drive_speed: AtomicU32,
    /// ABS: jumps to the needle, and of those needle drops.
    pub seeks: AtomicU32,
    pub drop_jumps: AtomicU32,
}

impl DvsShared {
    pub fn mode(&self) -> DvsMode { DvsMode::from_u8(self.mode.load(Ordering::Relaxed)) }
    pub fn set_mode(&self, m: DvsMode) { self.mode.store(m.to_u8(), Ordering::Relaxed) }

    pub fn store(&self, s: &VinylState) {
        self.armed.store(s.armed, Ordering::Relaxed);
        self.signal.store(s.signal, Ordering::Relaxed);
        self.locked.store(s.locked, Ordering::Relaxed);
        self.settling.store(s.settling, Ordering::Relaxed);
        self.speed.store(s.speed.to_bits(), Ordering::Relaxed);
        self.peak.store(s.peak.to_bits(), Ordering::Relaxed);
        self.pos_ms.store(s.pos_ms.unwrap_or(-1), Ordering::Relaxed);
        self.raw_ms.store(s.raw_ms.unwrap_or(-1), Ordering::Relaxed);
        self.drops.store(s.drops, Ordering::Relaxed);
    }

    pub fn load(&self) -> VinylState {
        let opt = |v: i32| (v >= 0).then_some(v);
        VinylState {
            armed: self.armed.load(Ordering::Relaxed),
            signal: self.signal.load(Ordering::Relaxed),
            locked: self.locked.load(Ordering::Relaxed),
            settling: self.settling.load(Ordering::Relaxed),
            speed: f32::from_bits(self.speed.load(Ordering::Relaxed)),
            peak: f32::from_bits(self.peak.load(Ordering::Relaxed)),
            pos_ms: opt(self.pos_ms.load(Ordering::Relaxed)),
            raw_ms: opt(self.raw_ms.load(Ordering::Relaxed)),
            drops: self.drops.load(Ordering::Relaxed),
        }
    }

    /// The input went away (or DVS is off): forget the record.
    pub fn clear(&self) { self.store(&VinylState::default()); }

    pub fn driving(&self) -> bool { self.driving.load(Ordering::Relaxed) }
    pub fn drive_speed(&self) -> f32 { f32::from_bits(self.drive_speed.load(Ordering::Relaxed)) }

    /// What MENU → DVS shows beside the mode: the record's state.
    pub fn status(&self) -> &'static str {
        let s = self.load();
        if self.mode() == DvsMode::Off { return ""; }
        if self.in_seq.load(Ordering::Relaxed) == 0 { return "NO INPUT"; }
        match (s.armed, s.signal, s.locked) {
            (false, _, _) => "NO SIGNAL",
            (true, false, _) => "STOPPED",
            (true, true, true) => "LOCKED",
            (true, true, false) => "SPEED",
        }
    }
}

/// What the output callback needs to play the track itself.
pub(crate) struct DvsRender {
    pub shared: Arc<DvsShared>,
    pub samples: Arc<ArcSwap<Vec<f32>>>,
    pub position: Arc<AtomicU64>,
    pub in_flight: Arc<AtomicU64>,
    pub seek_request: Arc<AtomicU64>,
    pub loop_start: Arc<AtomicU64>,
    pub loop_end: Arc<AtomicU64>,
    pub loop_active: Arc<AtomicBool>,
    pub file_ch: usize,
    pub sample_rate: u32,
    // Callback-owned.
    pub drive: Drive,
    /// Read cursor, frames.
    pub cur: Option<f64>,
    /// Which buffer `cur` is in (its pointer): a LOAD starts again.
    pub buf_id: usize,
    /// Output frames rendered, ever: the drive's clock.
    pub clock: u64,
}

impl DvsRender {
    /// Render `out` (interleaved, `device_ch`) if the record has the deck.
    /// False: the ring plays as usual. Real-time safe.
    pub fn fill(&mut self, out: &mut [f32], device_ch: usize) -> bool {
        let mode = self.shared.mode();
        let st = self.shared.load();
        let wants = matches!(mode, DvsMode::Abs | DvsMode::Rel) && st.armed;
        let buf = self.samples.load();
        let ch = self.file_ch.max(1);
        let frames = buf.len() / ch;
        if !wants || frames < 2 {
            if let Some(c) = self.cur.take() {
                // Hand back: the processor carries on from the record's spot.
                let at = (c.round().max(0.0) as u64).min(frames as u64) * ch as u64;
                self.seek_request.store(at, Ordering::Release);
                self.position.store(at, Ordering::Relaxed);
                self.shared.driving.store(false, Ordering::Release);
                self.drive.reset();
                log::info!("dvs: the deck has it back at frame {}", at / ch as u64);
            }
            return false;
        }
        let id = Arc::as_ptr(&*buf) as usize;
        let mut cur = match self.cur {
            Some(c) if id == self.buf_id => c,
            _ => {
                // Starting (or a new track): from the heard playhead, or a
                // pending jump.
                let req = self.seek_request.swap(NO_SEEK, Ordering::AcqRel);
                let p = if req != NO_SEEK { req } else {
                    self.position.load(Ordering::Relaxed).saturating_sub(self.in_flight.load(Ordering::Relaxed))
                };
                if self.cur.is_none() { log::info!("dvs: the record drives the deck ({:?})", mode); }
                self.buf_id = id;
                self.shared.driving.store(true, Ordering::Release);
                (p / ch as u64) as f64
            }
        };
        // A jump on the deck (cue, hot cue, search): REL takes it; in ABS
        // the needle decides.
        let req = self.seek_request.swap(NO_SEEK, Ordering::AcqRel);
        if req != NO_SEEK && mode == DvsMode::Rel { cur = (req / ch as u64) as f64; }

        let n = out.len() / device_ch.max(1);
        let sr = self.sample_rate as f64;
        let dt_ms = n as f64 * 1000.0 / sr;
        let now_ms = self.clock as f64 * 1000.0 / sr;
        let lead_ms = dt_ms + self.shared.in_us.load(Ordering::Relaxed) as f64 / 1000.0;
        let looping = self.loop_active.load(Ordering::Relaxed);
        let o = self.drive.step(&st, mode == DvsMode::Abs && !looping, now_ms, dt_ms,
                                cur * 1000.0 / sr, lead_ms, frames as f64 * 1000.0 / sr);
        if let Some(ms) = o.seek_ms { cur = ms * sr / 1000.0; }
        let (ls, le) = (self.loop_start.load(Ordering::Relaxed) / ch as u64, self.loop_end.load(Ordering::Relaxed) / ch as u64);
        let lp = (looping && le > ls).then_some((ls as f64, le as f64));
        let last = (frames - 1) as f64;
        // Below a crawl the sound fades out: a still record is silence, not
        // a held sample.
        let gain = (o.speed.abs() / 0.05).min(1.0) as f32;
        for f in out.chunks_mut(device_ch.max(1)) {
            let i = cur.floor() as usize;
            let j = (i + 1).min(frames - 1);
            let t = (cur - i as f64) as f32;
            for (d, s) in f.iter_mut().enumerate() {
                let c = d.min(ch - 1);
                let a = buf[i * ch + c];
                let b = buf[j * ch + c];
                *s = (a + (b - a) * t) * gain;
            }
            cur += o.speed;
            if let Some((s, e)) = lp { if o.speed > 0.0 && cur >= e { cur -= e - s; } }
            cur = cur.clamp(0.0, last);
        }
        self.clock += n as u64;
        self.cur = Some(cur);
        self.position.store(cur as u64 * ch as u64, Ordering::Relaxed);
        self.in_flight.store(0, Ordering::Relaxed);
        self.shared.drive_speed.store((o.speed as f32).to_bits(), Ordering::Relaxed);
        self.shared.seeks.store(self.drive.seeks, Ordering::Relaxed);
        self.shared.drop_jumps.store(self.drive.drops, Ordering::Relaxed);
        true
    }
}

/// The interface's input, decoded. Dropping it closes the stream.
pub struct DvsInput {
    _stream: cpal::Stream,
    pub device: String,
}

/// Which input pair carries the record (0 = inputs 1/2), from
/// OPENDECK_DVS_INPUTS ("3/4" or "3").
fn input_pair() -> usize {
    std::env::var("OPENDECK_DVS_INPUTS").ok()
        .and_then(|v| v.split(['/', '-']).next().and_then(|n| n.trim().parse::<usize>().ok()))
        .map_or(0, |n| n.saturating_sub(1))
}

impl DvsInput {
    /// Open the input (OPENDECK_DVS_DEVICE: part of its name; else the
    /// default) and decode it into `shared`.
    pub fn open(shared: Arc<DvsShared>) -> anyhow::Result<DvsInput> {
        use anyhow::Context;
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = match std::env::var("OPENDECK_DVS_DEVICE") {
            Ok(want) => host.input_devices()?.find(|d| d.name().is_ok_and(|n| n.to_lowercase().contains(&want.to_lowercase())))
                .with_context(|| format!("no input device matching {want:?}"))?,
            Err(_) => host.default_input_device().context("no audio input")?,
        };
        let name = device.name().unwrap_or_else(|_| "?".into());
        let cfg = device.default_input_config().context("input config")?;
        let ch = cfg.channels() as usize;
        let rate = cfg.sample_rate().0;
        let left = input_pair().min(ch.saturating_sub(2));
        let fmt = match std::env::var("OPENDECK_DVS_FORMAT").as_deref() {
            Ok("cd") | Ok("CD") => opendeck_timecode::Format::SeratoCd,
            _ => opendeck_timecode::Format::SeratoVinyl,
        };
        let mut vinyl = opendeck_timecode::Vinyl::new(rate, fmt);
        log::info!("dvs: input {name:?}, {ch} ch at {rate} Hz, record on inputs {}/{}, {}", left + 1, left + 2, fmt.label());
        let err = |e| log::warn!("dvs: input stream: {e}");
        let sh = Arc::clone(&shared);
        let mut feed = move |data: &[f32]| {
            vinyl.set_absolute(sh.mode() == DvsMode::Abs);
            vinyl.push_f32(data, ch, left);
            sh.store(&vinyl.state());
            let frames = data.len() / ch.max(1);
            sh.in_us.store((frames as u64 * 1_000_000 / rate as u64) as u32, Ordering::Relaxed);
            sh.in_seq.fetch_add(1, Ordering::Relaxed);
        };
        let config: cpal::StreamConfig = cfg.clone().into();
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::I16 => {
                let mut tmp: Vec<f32> = Vec::with_capacity(8192);
                device.build_input_stream(&config, move |d: &[i16], _: &_| {
                    tmp.clear();
                    tmp.extend(d.iter().map(|&s| s as f32 / 32768.0));
                    feed(&tmp);
                }, err, None)?
            }
            _ => device.build_input_stream(&config, move |d: &[f32], _: &_| feed(d), err, None)?,
        };
        stream.play()?;
        Ok(DvsInput { _stream: stream, device: name })
    }
}

/// Feed a test's record straight into `shared`, as the input callback does.
#[cfg(test)]
pub fn feed_for_test(vinyl: &mut opendeck_timecode::Vinyl, shared: &DvsShared, stereo: &[i16]) {
    vinyl.set_absolute(shared.mode() == DvsMode::Abs);
    vinyl.push_i16(stereo);
    shared.store(&vinyl.state());
    shared.in_us.store((stereo.len() as u64 / 2 * 1_000_000 / vinyl.rate() as u64) as u32, Ordering::Relaxed);
    shared.in_seq.fetch_add(1, Ordering::Relaxed);
}

/// Tests: a record on a turntable, feeding a deck's DVS input in real time.
#[cfg(test)]
pub(crate) mod turntable {
    use super::*;
    use opendeck_timecode::{decoder::synth::Signal, vinyl::START_MS, Format, Vinyl};
    use std::time::{Duration, Instant};

    pub const SR: u32 = 48_000;

    /// A record on the turntable: renders `speed(t)` into the deck's DVS
    /// input in real time, 10 ms at a time, until told to stop.
    pub struct Turntable { stop: Arc<AtomicBool>, t: Option<std::thread::JoinHandle<()>> }

    impl Turntable {
        pub fn play(shared: Arc<DvsShared>, at_ms: f64, speed: impl Fn(f64) -> Option<(f64, f64)> + Send + 'static) -> Turntable {
            let stop = Arc::new(AtomicBool::new(false));
            let sd = Arc::clone(&stop);
            let t = std::thread::spawn(move || {
                let mut v = Vinyl::new(SR, Format::SeratoVinyl);
                let mut sig = Signal::new(Format::SeratoVinyl, SR, START_MS as f64 + at_ms);
                let mut buf = Vec::new();
                let t0 = Instant::now();
                let mut n = 0u32;
                while !sd.load(Ordering::Relaxed) {
                    let t = n as f64 / 100.0;
                    buf.clear();
                    match speed(t) {
                        // (speed, needle jump to ms: NaN = none)
                        Some((s, jump)) => {
                            if !jump.is_nan() { sig = Signal::new(Format::SeratoVinyl, SR, START_MS as f64 + jump); }
                            sig.render(s, SR as usize / 100, &mut buf);
                        }
                        None => buf.resize(SR as usize / 100 * 2, 0),   // needle up
                    }
                    feed_for_test(&mut v, &shared, &buf);
                    n += 1;
                    if let Some(w) = (t0 + Duration::from_millis(10) * n).checked_duration_since(Instant::now()) { std::thread::sleep(w); }
                }
            });
            Turntable { stop, t: Some(t) }
        }
    }
    impl Drop for Turntable {
        fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); if let Some(t) = self.t.take() { let _ = t.join(); } }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use super::turntable::{Turntable, SR};
    use crate::audio::AudioHandle;
    use std::time::{Duration, Instant};

    fn track() -> Vec<f32> {
        // 5 minutes of a 10 Hz ramp, ±0.1.
        let frames = SR as usize * 300;
        let mut s = Vec::with_capacity(frames * 2);
        for i in 0..frames { let v = ((i % 4800) as f32 / 4800.0 - 0.5) * 0.2; s.push(v); s.push(v); }
        s
    }

    fn deck() -> AudioHandle { AudioHandle::open_test(track(), SR).unwrap() }

    /// A deck whose stand-in device records each 10 ms pull's loudest sample.
    fn traced() -> (AudioHandle, crate::audio::Trace) {
        let trace: crate::audio::Trace = Default::default();
        (AudioHandle::open_test_traced(track(), SR, 480, 10_000, Arc::clone(&trace)).unwrap(), trace)
    }

    /// Loudest sample heard in the last `secs`.
    fn heard(trace: &crate::audio::Trace, secs: f64) -> f32 {
        let since = Instant::now() - Duration::from_secs_f64(secs);
        trace.lock().unwrap().iter().filter(|(t, _)| *t >= since).fold(0.0, |m, (_, p)| m.max(*p))
    }

    fn pos_ms(d: &AudioHandle) -> f64 { d.position.load(Ordering::Relaxed) as f64 / 2.0 / SR as f64 * 1000.0 }

    fn wait(secs: f64) { std::thread::sleep(Duration::from_secs_f64(secs)); }

    #[test]
    fn rel_the_record_plays_the_deck_forward_backward_and_still() {
        let (d, trace) = traced();
        d.dvs.set_mode(DvsMode::Rel);
        d.seek(SR as u64 * 2 * 10);             // the deck sits at 0:10
        let _tt = Turntable::play(Arc::clone(&d.dvs), 100_000.0, |t| Some((if t < 1.5 { 1.0 } else if t < 2.0 { -1.0 } else { 0.0 }, f64::NAN)));
        wait(0.6);
        assert!(d.dvs.driving(), "armed after 0.3 s of carrier");
        let a = pos_ms(&d);
        assert!((9_900.0..10_700.0).contains(&a), "REL starts where the deck was, not at the needle: {a}");
        wait(0.8);
        let b = pos_ms(&d);
        assert!((b - a - 800.0).abs() < 60.0, "forward at 1x: moved {} ms in 800", b - a);
        assert!(heard(&trace, 0.3) > 0.09, "the track is heard while the record turns");
        wait(0.5);                               // 1.4 → 1.9 s: backwards from 1.5
        let c = pos_ms(&d);
        assert!(c < b, "backwards: {b} → {c}");
        wait(0.5);
        let e = pos_ms(&d);
        wait(0.3);
        assert!((pos_ms(&d) - e).abs() < 5.0, "a still record holds the deck");
        assert!(heard(&trace, 0.2) < 1e-4, "and is silent");
        assert!(d.dvs.drive_speed().abs() < 0.01);
    }

    #[test]
    fn abs_needle_position_is_track_position_and_a_drop_jumps() {
        let d = deck();
        d.dvs.set_mode(DvsMode::Abs);
        let _tt = Turntable::play(Arc::clone(&d.dvs), 30_000.0, |t| {
            if t < 1.0 { Some((1.0, f64::NAN)) }
            else if t < 1.3 { None }                                     // lift
            else if t < 1.31 { Some((1.0, 120_000.0)) }                  // drop at 2:00
            else { Some((1.0, f64::NAN)) }
        });
        wait(0.9);
        let p = pos_ms(&d);
        assert!((p - 30_900.0).abs() < 120.0, "ABS: the needle's place, 0:30 + 0.9 s: {p}");
        wait(1.0);                                // 1.9 s: landed at 2:00 + ~0.6 s
        let q = pos_ms(&d);
        assert!((q - 120_600.0).abs() < 200.0, "after the drop: {q}");
    }

    /// Back-cueing in ABS: the hand rocks the record back and forth over a
    /// beat; the deck follows the hand and never jumps.
    #[test]
    fn abs_back_cueing_follows_the_hand_without_jumps() {
        let d = deck();
        d.dvs.set_mode(DvsMode::Abs);
        let _tt = Turntable::play(Arc::clone(&d.dvs), 30_000.0, |t| {
            Some((if t < 0.6 { 1.0 } else if t < 2.6 { 1.8 * (t * 6.0).sin() } else { 1.0 }, f64::NAN))
        });
        wait(0.6);
        let placed = d.dvs.seeks.load(Ordering::Relaxed);
        let mut last = pos_ms(&d);
        let mut biggest = 0.0f64;
        for _ in 0..200 {
            wait(0.01);
            let p = pos_ms(&d);
            biggest = biggest.max((p - last).abs());
            last = p;
        }
        assert_eq!(d.dvs.seeks.load(Ordering::Relaxed), placed, "no jumps while rocking");
        assert!(biggest < 40.0, "the playhead only moved as the hand did: biggest step {biggest:.1} ms in 10 ms");
    }

    #[test]
    fn int_and_off_hand_the_deck_back_where_the_record_left_it() {
        let d = deck();
        d.dvs.set_mode(DvsMode::Rel);
        let _tt = Turntable::play(Arc::clone(&d.dvs), 50_000.0, |_| Some((1.0, f64::NAN)));
        wait(1.0);
        assert!(d.dvs.driving());
        d.dvs.set_mode(DvsMode::Int);
        wait(0.1);
        assert!(!d.dvs.driving());
        let at = pos_ms(&d);
        d.set_playing(true);
        wait(0.5);
        let later = pos_ms(&d);
        assert!(later > at + 300.0 && later < at + 700.0, "the deck plays on from there: {at} → {later}");
    }

    #[test]
    fn music_on_the_input_never_takes_the_deck() {
        let d = deck();
        d.dvs.set_mode(DvsMode::Abs);
        // A wobbly 1 kHz-ish tone that is no steady record: 0.3x..2.7x.
        let _tt = Turntable::play(Arc::clone(&d.dvs), 0.0, |t| Some((1.5 + 1.2 * (t * 40.0).sin(), f64::NAN)));
        wait(1.0);
        assert!(!d.dvs.driving());
    }
}
