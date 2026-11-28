//! Where the deck's sound goes.
//!
//! The deck (`audio.rs`) never opens a device itself.  It is handed an
//! [`AudioOut`], which says what format it runs at and then pulls the
//! deck's output from its own real-time callback through a [`Render`]:
//!
//! - the sound card, through cpal (`audio_cpal.rs`, the native platforms);
//! - a plugin host's audio callback ([`HostOut`], the VST3 build);
//! - the tests' stand-in device, paced at real time ([`PacedOut`]).
//!
//! A browser build adds an AudioWorklet here without the deck knowing.

use crate::audio::{Render, StreamParts};
use anyhow::Result;
use std::{
    any::Any,
    sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex},
};

/// The rate and channel count an output runs at.  The deck runs at it too:
/// a track at another rate is resampled on LOAD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub sample_rate: u32,
    pub channels: usize,
}

/// Somewhere the deck's sound can go.
pub trait AudioOut {
    /// What the deck should run at.
    fn format(&self) -> Format;
    /// Start pulling the deck's output.  The returned [`Running`] keeps the
    /// output going; dropping it stops it.
    fn start(self: Box<Self>, src: RenderSource) -> Result<Running>;
}

/// Hands out the deck's output callback.  An output that rebuilds its
/// stream (Linux, after ALSA gives up) takes a fresh [`Render`] each time;
/// only one renders at once, since the ring is behind a `try_lock`.
pub struct RenderSource(pub(crate) StreamParts);

impl RenderSource {
    pub fn format(&self) -> Format {
        Format { sample_rate: self.0.device_sr, channels: self.0.device_ch }
    }

    /// A callback for one stream: call [`Render::fill`] once per buffer,
    /// from the real-time thread.
    pub fn render(&self) -> Render { Render::new(&self.0) }
}

/// A started output.  Whatever keeps it alive lives in here: a device
/// stream, or a flag its thread watches, set when this is dropped.
pub struct Running {
    _keep: Box<dyn Any>,
}

impl Running {
    /// Keep `t` (a device stream) until the deck is dropped.
    pub fn holding<T: 'static>(t: T) -> Running { Running { _keep: Box::new(t) } }

    /// Set `stop` when the deck is dropped: the output's thread watches it.
    #[allow(dead_code)]   // used by the Linux device output and the tests
    pub fn stop_on_drop(stop: Arc<AtomicBool>) -> Running { Running::holding(SetOnDrop(stop)) }

    /// Nothing to keep: someone else's callback pulls the output.
    pub fn nothing() -> Running { Running::holding(()) }
}

#[allow(dead_code)]
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) { self.0.store(true, Ordering::Relaxed); }
}

// ── A plugin host ────────────────────────────────────────────────────────────

/// A plugin host's audio callback, standing in for the sound card's: the
/// deck's output, interleaved stereo at the rate the deck was opened at.
pub type HostOutput = Render;

/// Where a [`HostOut`] deck leaves its [`HostOutput`] for the host to take.
pub type HostSlot = Arc<Mutex<Option<HostOutput>>>;

/// Output pulled by a plugin host instead of a sound card: stereo at the
/// host's rate.  The host's audio callback takes the [`HostOutput`] left
/// in `slot` and calls its `fill` once per block.
pub struct HostOut {
    pub sample_rate: u32,
    pub slot: HostSlot,
}

impl AudioOut for HostOut {
    fn format(&self) -> Format { Format { sample_rate: self.sample_rate, channels: 2 } }

    fn start(self: Box<Self>, src: RenderSource) -> Result<Running> {
        *self.slot.lock().unwrap() = Some(src.render());
        Ok(Running::nothing())
    }
}

// ── The tests' stand-in device ───────────────────────────────────────────────

/// Tests: what a stand-in device heard, pull by pull (time, loudest sample).
#[cfg(test)]
pub type Trace = Arc<Mutex<Vec<(std::time::Instant, f32)>>>;

/// Tests: a stand-in device at `sample_rate` (stereo) that takes `frames`
/// every `every_us` from a thread of its own, as a sound card's callback
/// would.  `trace`, when given, records each pull's time and loudest sample.
#[cfg(test)]
pub struct PacedOut {
    pub sample_rate: u32,
    pub frames: usize,
    pub every_us: u64,
    pub trace: Option<Trace>,
}

#[cfg(test)]
impl PacedOut {
    /// 10 ms of output every 10 ms.
    pub fn realtime(sample_rate: u32) -> PacedOut {
        PacedOut { sample_rate, frames: sample_rate as usize / 100, every_us: 10_000, trace: None }
    }
}

#[cfg(test)]
impl AudioOut for PacedOut {
    fn format(&self) -> Format { Format { sample_rate: self.sample_rate, channels: 2 } }

    fn start(self: Box<Self>, src: RenderSource) -> Result<Running> {
        use anyhow::Context;
        use std::time::{Duration, Instant};
        let stop = Arc::new(AtomicBool::new(false));
        let sd = Arc::clone(&stop);
        let mut fill = src.render();
        let PacedOut { frames, every_us, trace, .. } = *self;
        let ch = src.format().channels;
        std::thread::Builder::new().name("audio-test-out".into()).spawn(move || {
            let mut buf = vec![0f32; frames * ch];
            let t0 = Instant::now();
            let mut n = 0u32;
            while !sd.load(Ordering::Relaxed) {
                fill.fill(&mut buf);
                if let Some(t) = &trace {
                    let peak = buf.iter().fold(0f32, |m, v| m.max(v.abs()));
                    t.lock().unwrap().push((Instant::now(), peak));
                }
                n += 1;
                let due = t0 + Duration::from_micros(every_us) * n;
                if let Some(w) = due.checked_duration_since(Instant::now()) { std::thread::sleep(w); }
            }
        }).context("failed to spawn audio-test-out thread")?;
        Ok(Running::stop_on_drop(stop))
    }
}
