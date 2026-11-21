//! OpenDeck as a plugin (VST3 and CLAP): an instrument with a stereo output.
//!
//! The deck runs inside the host's process: its window (the same UI as the
//! desktop app) and its engine.  The host's audio callback takes the place of
//! the sound card's: `process` pulls the deck's output through
//! [`opendeck_app::HostOutput`], so the deck plays into the host's mixer at the
//! host's sample rate.
//!
//! Any number of instances: winit allows one event loop per process, so the
//! decks share one ([`opendeck_app::DeckHost`]), each with its own window and
//! its own output.  Removing an instance closes its deck.  Each instance
//! saves its track's path in its state, so a project reopens with its decks
//! loaded.

use nih_plug::prelude::*;
use opendeck_app::{ControlEvent, DeckHost, Event, HostSlot};
use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering}, mpsc, Arc, Mutex, RwLock};
use std::time::Duration;

/// The Pitch parameter's reach, in percent: the deck's default TEMPO RANGE.
const PITCH_RANGE: f32 = 16.0;

/// Pitch in percent (+ = faster) to the deck's tempo fader position, on the
/// default ±16 % range (DJ convention: the bottom of the fader is fastest).
pub fn pitch_to_fader(pct: f32) -> f32 {
    (0.5 - pct.clamp(-PITCH_RANGE, PITCH_RANGE) / (2.0 * PITCH_RANGE)).clamp(0.0, 1.0)
}

/// Deck numbers in use (1 = the first live deck): a new deck takes the lowest
/// free one, which is its PLAYER No. and picks its controller channel.
static NUMBERS: Mutex<BTreeSet<u8>> = Mutex::new(BTreeSet::new());

fn claim_number() -> u8 {
    let mut used = NUMBERS.lock().unwrap();
    let n = (1..=u8::MAX).find(|n| !used.contains(n)).unwrap_or(u8::MAX);
    used.insert(n);
    n
}

fn release_number(n: u8) {
    NUMBERS.lock().unwrap().remove(&n);
}

pub struct OpenDeck {
    params: Arc<DeckParams>,
    /// Interleaved scratch for one block, sized in `initialize`.
    scratch: Vec<f32>,
    /// This deck's output, filled once it is up.
    slot: HostSlot,
    /// The Pitch this instance's host is asking for (f32 bits).
    pitch: Arc<AtomicU32>,
    /// This deck, once started: its id in the deck loop and its number.
    deck: Option<(u64, u8)>,
    /// Tells this instance's parameter thread to stop.
    stop: Arc<AtomicBool>,
}

#[derive(Params)]
struct DeckParams {
    /// The tempo fader, in percent: automatable from the host.
    #[id = "pitch"]
    pitch: FloatParam,
    /// The track this deck loads (saved with the project).  A host or script
    /// that writes it (the plugin's state) loads that track.
    #[persist = "track"]
    track: Arc<RwLock<String>>,
}

impl Default for DeckParams {
    fn default() -> Self {
        DeckParams {
            pitch: FloatParam::new("Pitch", 0.0, FloatRange::Linear { min: -PITCH_RANGE, max: PITCH_RANGE })
                .with_step_size(0.01)
                .with_unit(" %")
                .with_value_to_string(Arc::new(|v| format!("{v:+.2}"))),
            track: Arc::new(RwLock::new(String::new())),
        }
    }
}

/// One instance's link to its deck: the host's Pitch as fader moves (up to
/// 200 Hz), and a newly saved track path as a load, until told to stop.
fn follow_params(id: u64, events: Arc<Mutex<Option<mpsc::Sender<Event>>>>, pitch: Arc<AtomicU32>,
                 track: Arc<RwLock<String>>, mut loaded: String, stop: Arc<AtomicBool>) {
    let mut last = f32::NAN;
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
        let Some(tx) = events.lock().unwrap().clone() else { continue };   // the deck is booting
        let pct = f32::from_bits(pitch.load(Ordering::Relaxed));
        if pct != last {
            if tx.send(Event::Deck(ControlEvent::TempoFader { position: pitch_to_fader(pct) })).is_err() { return; }
            if last.is_nan() || (pct - last).abs() >= 1.0 || pct == 0.0 { log::info!("plugin deck {id}: Pitch {pct:+.2} %"); }
            last = pct;
        }
        let want = track.read().unwrap().clone();
        if want != loaded {
            if PathBuf::from(&want).is_file() {
                log::info!("plugin deck {id}: loading {want}");
                // OPENDECK_AUTOPLAY (demos): play what the host loaded once it
                // is in, as a startup track plays; else it parks at the cue,
                // like a CDJ after a load.
                let play = std::env::var("OPENDECK_AUTOPLAY").as_deref() == Ok("1");
                if tx.send(Event::Load { path: PathBuf::from(&want), play }).is_err() { return; }
            }
            loaded = want;
        }
    }
}

impl Default for OpenDeck {
    fn default() -> Self {
        OpenDeck {
            params: Arc::new(DeckParams::default()),
            scratch: Vec::new(),
            slot: Arc::new(Mutex::new(None)),
            pitch: Arc::new(AtomicU32::new(0)),
            deck: None,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Keep this library mapped until the process exits.  The deck loop's thread
/// outlives any one instance; a host that unloads the library at quit would
/// otherwise pull its code out from under it (REAPER crashed on quit).
fn pin_library() {
    static PINNED: std::sync::Once = std::sync::Once::new();
    PINNED.call_once(|| unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(pin_library as *const libc::c_void, &mut info) != 0 && !info.dli_fname.is_null() {
            let h = libc::dlopen(info.dli_fname, libc::RTLD_NOW | libc::RTLD_NODELETE);
            if h.is_null() { log::warn!("plugin: could not pin the library in memory"); }
        }
    });
}

impl OpenDeck {
    /// Start this instance's deck in the shared loop, at the host's rate.
    fn start_deck(&mut self, sr: u32) {
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn"))
            .format_timestamp_micros()
            .try_init();
        pin_library();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let number = claim_number();
        // The saved track, else (the first deck, for demos) OPENDECK_TRACK.
        let saved = self.params.track.read().unwrap().clone();
        let track = Some(PathBuf::from(&saved)).filter(|p| p.is_file())
            .or_else(|| (number == 1).then(|| std::env::var_os("OPENDECK_TRACK").map(PathBuf::from)).flatten().filter(|p| p.is_file()));
        let events = Arc::new(Mutex::new(None));
        {
            let (ev, pitch, tr, stop) = (Arc::clone(&events), Arc::clone(&self.pitch), Arc::clone(&self.params.track), Arc::clone(&self.stop));
            let loaded = track.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or(saved);
            let _ = std::thread::Builder::new().name(format!("opendeck-params-{id}"))
                .spawn(move || follow_params(id, ev, pitch, tr, loaded, stop));
        }
        let cfg = opendeck_app::Config {
            track,
            // Deck 1 keeps its MENU PLAYER No.; the others take their number.
            player: (number > 1).then_some(number.min(6)), default_player: 1,
            deck_channel: u8::from(number > 1),
            // Receive-only, as the desktop app defaults to.
            link_send: std::env::var("OPENDECK_LINK_SEND").map(|v| v == "1").unwrap_or(false),
            browse_root: opendeck_app::default_music_dir(),
            autoplay: std::env::var("OPENDECK_AUTOPLAY").as_deref() == Ok("1"), controllers: None,
            host: Some(opendeck_app::PluginHost { sample_rate: sr, output: Arc::clone(&self.slot), events }),
        };
        match DeckHost::get() {
            Ok(h) => { h.add(id, cfg); self.deck = Some((id, number)); log::info!("plugin: deck {id} (No. {number}) starting"); }
            Err(e) => { log::error!("plugin: no deck loop: {e:#}"); release_number(number); }
        }
    }
}

impl Drop for OpenDeck {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some((id, number)) = self.deck.take() {
            if let Ok(h) = DeckHost::get() { h.remove(id); }
            release_number(number);
        }
    }
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
        if self.deck.is_none() { self.start_deck(cfg.sample_rate.round() as u32); }
        true
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let frames = buffer.samples();
        self.pitch.store(self.params.pitch.value().to_bits(), Ordering::Relaxed);
        // Silence until this deck is up.
        let slot = Arc::clone(&self.slot);
        let mut slot = slot.try_lock();
        let Some(out) = slot.as_mut().ok().and_then(|s| s.as_mut()) else {
            for ch in buffer.as_slice() { ch.fill(0.0); }
            return ProcessStatus::Normal;
        };
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

    /// Decks take the lowest free number, and a closed deck's number is free again.
    #[test]
    fn decks_take_the_lowest_free_number() {
        let (a, b, c) = (claim_number(), claim_number(), claim_number());
        assert!(a < b && b < c);
        release_number(b);
        assert_eq!(claim_number(), b, "the gap is reused");
        for n in [a, b, c] { release_number(n); }
    }

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
