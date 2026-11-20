//! OpenDeck as a plugin (VST3 and CLAP): an instrument with a stereo output.
//!
//! The whole deck runs inside the host's process: its window (the same UI as
//! the desktop app, on a thread of its own) and its engine.  The host's
//! audio callback takes the place of the sound card's: `process` pulls the
//! deck's output through [`opendeck_app::HostOutput`], so the deck plays into
//! the host's mixer at the host's sample rate.
//!
//! One deck per process: winit allows one event loop per process.  The deck's
//! output lives in a process-wide slot that whichever instance is processing
//! pulls from (a host may make a probe instance before the track's own), and
//! a deck whose window was closed does not come back until the host is
//! restarted.

use nih_plug::prelude::*;
use opendeck_app::{ControlEvent, Event, HostSlot};
use std::num::NonZeroU32;
use std::sync::{atomic::{AtomicBool, AtomicU32, Ordering}, mpsc, Arc, Mutex};
use std::time::Duration;

/// The Pitch parameter's reach, in percent: the deck's default TEMPO RANGE.
const PITCH_RANGE: f32 = 16.0;

/// Pitch in percent (+ = faster) to the deck's tempo fader position, on the
/// default ±16 % range (DJ convention: the bottom of the fader is fastest).
pub fn pitch_to_fader(pct: f32) -> f32 {
    (0.5 - pct.clamp(-PITCH_RANGE, PITCH_RANGE) / (2.0 * PITCH_RANGE)).clamp(0.0, 1.0)
}

/// The Pitch of the instance feeding the host (f32 bits); see `follow_params`.
static PITCH: AtomicU32 = AtomicU32::new(0);

/// Set by the first instance to start a deck; see the module note.
static DECK_STARTED: AtomicBool = AtomicBool::new(false);

/// The deck's output, once it is up; see the module note.
fn deck_slot() -> &'static HostSlot {
    static SLOT: std::sync::OnceLock<HostSlot> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub struct OpenDeck {
    params: Arc<DeckParams>,
    /// Interleaved scratch for one block, sized in `initialize`.
    scratch: Vec<f32>,
}

#[derive(Params)]
struct DeckParams {
    /// The tempo fader, in percent: automatable from the host.
    #[id = "pitch"]
    pitch: FloatParam,
}

impl Default for DeckParams {
    fn default() -> Self {
        DeckParams {
            pitch: FloatParam::new("Pitch", 0.0, FloatRange::Linear { min: -PITCH_RANGE, max: PITCH_RANGE })
                .with_step_size(0.01)
                .with_unit(" %")
                .with_value_to_string(Arc::new(|v| format!("{v:+.2}"))),
        }
    }
}

/// Send the host's parameter changes into the deck's input bus as fader
/// moves, at up to 200 Hz, for as long as the deck runs.
fn follow_params(events: Arc<Mutex<Option<mpsc::Sender<Event>>>>) {
    let mut last = f32::NAN;
    loop {
        std::thread::sleep(Duration::from_millis(5));
        let Some(tx) = events.lock().unwrap().clone() else { continue };   // the deck is booting
        let pct = f32::from_bits(PITCH.load(Ordering::Relaxed));
        if pct == last { continue; }
        if tx.send(Event::Deck(ControlEvent::TempoFader { position: pitch_to_fader(pct) })).is_err() { return; }
        if last.is_nan() || (pct - last).abs() >= 1.0 || pct == 0.0 { log::info!("plugin: Pitch {pct:+.2} %"); }
        last = pct;
    }
}

impl Default for OpenDeck {
    fn default() -> Self {
        OpenDeck { params: Arc::new(DeckParams::default()), scratch: Vec::new() }
    }
}

/// Start the deck (window + engine) on its own thread, at the host's rate.
fn start_deck(sr: u32, slot: HostSlot) {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn"))
        .format_timestamp_micros()
        .try_init();
    let spawned = std::thread::Builder::new().name("opendeck-ui".into()).spawn(move || {
        let events = Arc::new(Mutex::new(None));
        let ev = Arc::clone(&events);
        let _ = std::thread::Builder::new().name("opendeck-params".into()).spawn(move || follow_params(ev));
        let cfg = opendeck_app::Config {
            // OPENDECK_TRACK: a track loaded at start (demos); else empty.
            track: std::env::var_os("OPENDECK_TRACK").map(std::path::PathBuf::from).filter(|p| p.is_file()),
            player: None, default_player: 1, deck_channel: 0,
            // Receive-only, as the desktop app defaults to.
            link_send: std::env::var("OPENDECK_LINK_SEND").map(|v| v == "1").unwrap_or(false),
            browse_root: opendeck_app::default_music_dir(),
            autoplay: std::env::var("OPENDECK_AUTOPLAY").as_deref() == Ok("1"), controllers: None,
            host: Some(opendeck_app::PluginHost { sample_rate: sr, output: slot, events }),
        };
        match opendeck_app::run(cfg) {
            Ok(()) => log::info!("plugin: deck window closed"),
            Err(e) => log::error!("plugin: deck stopped: {e:#}"),
        }
    });
    if let Err(e) = spawned { log::error!("plugin: could not start the deck thread: {e}"); }
}

impl Plugin for OpenDeck {
    const NAME: &'static str = "OpenDeck";
    const VENDOR: &'static str = "Audiodestrukt";
    const URL: &'static str = "https://audiodestrukt.com";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> { self.params.clone() }

    fn initialize(&mut self, _layout: &AudioIOLayout, cfg: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.scratch = vec![0.0; cfg.max_buffer_size as usize * 2];
        if !DECK_STARTED.swap(true, Ordering::SeqCst) {
            start_deck(cfg.sample_rate.round() as u32, Arc::clone(deck_slot()));
        }
        true
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let frames = buffer.samples();
        // Silence until the deck is up, or while another instance is pulling.
        let mut slot = deck_slot().try_lock();
        let Some(out) = slot.as_mut().ok().and_then(|s| s.as_mut()) else {
            for ch in buffer.as_slice() { ch.fill(0.0); }
            return ProcessStatus::Normal;
        };
        PITCH.store(self.params.pitch.value().to_bits(), Ordering::Relaxed);
        if self.scratch.len() < frames * 2 { self.scratch.resize(frames * 2, 0.0); }   // a host breaking max_buffer_size
        let inter = &mut self.scratch[..frames * 2];
        out.fill(inter);
        let chans = buffer.as_slice();
        for (i, frame) in inter.chunks_exact(2).enumerate() {
            for (c, ch) in chans.iter_mut().enumerate() { ch[i] = frame[c.min(1)]; }
        }
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for OpenDeck {
    const CLAP_ID: &'static str = "com.audiodestrukt.opendeck";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A CDJ-style deck with Pro DJ Link");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Stereo];
}

impl Vst3Plugin for OpenDeck {
    const VST3_CLASS_ID: [u8; 16] = *b"AudiodestruktODk";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Stereo];
}

nih_export_clap!(OpenDeck);
nih_export_vst3!(OpenDeck);

#[cfg(test)]
mod tests {
    use super::*;

    /// Pitch +8 % is a quarter of the way down a ±16 % fader, as on a CDJ.
    #[test]
    fn pitch_moves_the_fader_like_a_cdj() {
        assert_eq!(pitch_to_fader(0.0), 0.5);
        assert_eq!(pitch_to_fader(16.0), 0.0, "fastest at the bottom");
        assert_eq!(pitch_to_fader(-16.0), 1.0);
        assert!((pitch_to_fader(8.0) - 0.25).abs() < 1e-6);
        assert_eq!(pitch_to_fader(40.0), 0.0, "clamped to the range");
    }
}
