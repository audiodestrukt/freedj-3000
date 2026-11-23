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
use opendeck_app::{ControlEvent, DeckHost, Event, HostSlot, JOG_SECS_PER_TICK};
use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering}, mpsc, Arc, Mutex, RwLock};
use std::time::Duration;

/// The Pitch parameter's reach, in percent: the deck's default TEMPO RANGE.
const PITCH_RANGE: f32 = 16.0;

/// The Platter parameter's reach, in seconds of record either way.
const PLATTER_RANGE: f32 = 8.0;

/// Platter moves (seconds of record) to jog ticks, keeping the remainder so
/// that many small moves add up to exactly the distance the host drew.
#[derive(Default)]
pub struct PlatterTicks { last: Option<f32>, rest: f64 }

impl PlatterTicks {
    /// The ticks to send for the Platter now at `secs` (none on the first call).
    pub fn ticks(&mut self, secs: f32) -> i32 {
        let Some(last) = self.last.replace(secs) else { return 0 };
        self.rest += f64::from(secs - last);
        let t = (self.rest / JOG_SECS_PER_TICK).trunc();
        self.rest -= t * JOG_SECS_PER_TICK;
        t as i32
    }
}

/// The host's button parameters: what an edge of each one presses.
pub fn button_edges(was: [bool; 3], now: [bool; 3]) -> Vec<ControlEvent> {
    let [play, cue, touch] = now;
    let mut out = Vec::new();
    if play != was[0] { out.push(if play { ControlEvent::Play } else { ControlEvent::Pause }); }
    if cue != was[1] { out.push(ControlEvent::Cue { pressed: cue }); }
    if touch != was[2] { out.push(ControlEvent::JogTouch { touched: touch }); }
    out
}

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
    /// What this instance's host is asking for, as the audio thread last
    /// saw it: Pitch, Platter (f32 bits) and the buttons (bit 0 Play, 1 Cue,
    /// 2 Touch).
    pitch: Arc<AtomicU32>,
    platter: Arc<AtomicU32>,
    buttons: Arc<AtomicU32>,
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
    /// PLAY as a level: on plays, off pauses (an edge presses the key).
    #[id = "play"]
    play: BoolParam,
    /// CUE, momentary: pressed while playing it jumps back to the cue and
    /// stops (a back-cue, as a juggle needs).
    #[id = "cue"]
    cue: BoolParam,
    /// The platter touched (VINYL mode: holds a playing deck, as a hand on
    /// the record does).
    #[id = "touch"]
    touch: BoolParam,
    /// The platter's position in seconds of record: moving it turns the jog
    /// by that much, so drawing it scratches.
    #[id = "platter"]
    platter: FloatParam,
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
            play: BoolParam::new("Play", false),
            cue: BoolParam::new("Cue", false),
            touch: BoolParam::new("Platter touch", false),
            platter: FloatParam::new("Platter", 0.0, FloatRange::Linear { min: -PLATTER_RANGE, max: PLATTER_RANGE })
                .with_unit(" s")
                .with_value_to_string(Arc::new(|v| format!("{v:+.3}"))),
            track: Arc::new(RwLock::new(String::new())),
        }
    }
}

/// One instance's link to its deck: the host's Pitch as fader moves (up to
/// 200 Hz), and a newly saved track path as a load, until told to stop.
struct Follow {
    id: u64,
    events: Arc<Mutex<Option<mpsc::Sender<Event>>>>,
    pitch: Arc<AtomicU32>,
    platter: Arc<AtomicU32>,
    buttons: Arc<AtomicU32>,
    track: Arc<RwLock<String>>,
    loaded: String,
    stop: Arc<AtomicBool>,
}

fn follow_params(f: Follow) {
    let Follow { id, events, pitch, platter, buttons, track, mut loaded, stop } = f;
    let mut last = f32::NAN;
    let mut was = [false; 3];
    let mut jog = PlatterTicks::default();
    while !stop.load(Ordering::Relaxed) {
        // 1 ms: a scratch drawn in automation turns the jog in small steps.
        std::thread::sleep(Duration::from_millis(1));
        let Some(tx) = events.lock().unwrap().clone() else { continue };   // the deck is booting
        let b = buttons.load(Ordering::Relaxed);
        let now = [b & 1 != 0, b & 2 != 0, b & 4 != 0];
        for e in button_edges(was, now) {
            log::debug!("plugin deck {id}: {e:?}");
            if tx.send(Event::Deck(e)).is_err() { return; }
        }
        was = now;
        let ticks = jog.ticks(f32::from_bits(platter.load(Ordering::Relaxed)));
        if ticks != 0 && tx.send(Event::Deck(ControlEvent::JogDelta { delta: ticks, velocity_rpm: 0.0 })).is_err() { return; }
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
            platter: Arc::new(AtomicU32::new(0)),
            buttons: Arc::new(AtomicU32::new(0)),
            deck: None,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// At exit: close the decks and end their loop before the process's
/// destructors run (a deck thread still drawing while the GPU driver tears
/// down crashed REAPER's quit with SIGSEGV / SIGILL).
extern "C" fn shutdown_decks() {
    DeckHost::shutdown(Duration::from_secs(2));
}

/// Keep this library mapped until the process exits.  The deck loop's thread
/// outlives any one instance; a host that unloads the library at quit would
/// otherwise pull its code out from under it (REAPER crashed on quit).  And
/// close the decks at exit (`shutdown_decks`): registered after the GPU
/// driver has loaded, it runs before the driver's destructors.
fn pin_library() {
    static PINNED: std::sync::Once = std::sync::Once::new();
    PINNED.call_once(|| unsafe {
        libc::atexit(shutdown_decks);
        let mut info: libc::Dl_info = std::mem::zeroed();
        if libc::dladdr(pin_library as *const libc::c_void, &mut info) != 0 && !info.dli_fname.is_null() {
            let h = libc::dlopen(info.dli_fname, libc::RTLD_NOW | libc::RTLD_NODELETE);
            if h.is_null() { log::warn!("plugin: could not pin the library in memory"); }
        }
    });
}

/// This deck's screen from the host's `OVERDECK_DISPLAY` template, if the
/// host made one (Overdeck's display contract).
fn display_path(template: Option<&str>, number: u8) -> Option<PathBuf> {
    let p = overdeck_display::Screen::path_for(template.filter(|t| !t.is_empty())?, u32::from(number));
    p.is_file().then_some(p)
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
            let f = Follow {
                id, events: Arc::clone(&events),
                pitch: Arc::clone(&self.pitch), platter: Arc::clone(&self.platter), buttons: Arc::clone(&self.buttons),
                track: Arc::clone(&self.params.track),
                loaded: track.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or(saved),
                stop: Arc::clone(&self.stop),
            };
            let _ = std::thread::Builder::new().name(format!("opendeck-params-{id}")).spawn(move || follow_params(f));
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
            host: Some(opendeck_app::PluginHost {
                sample_rate: sr, output: Arc::clone(&self.slot), events,
                // OPENDECK_TILE=N: N decks side by side, in the order they started.
                tile: std::env::var("OPENDECK_TILE").ok().and_then(|v| v.parse::<u8>().ok()).filter(|&n| n > 0).map(|n| (number - 1, n)),
                // OVERDECK_DISPLAY=template: the host shows this deck's screen
                // in its own; `{n}` is the deck's number.
                display: display_path(std::env::var("OVERDECK_DISPLAY").ok().as_deref(), number),
            }),
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
        let p = &self.params;
        self.pitch.store(p.pitch.value().to_bits(), Ordering::Relaxed);
        self.platter.store(p.platter.value().to_bits(), Ordering::Relaxed);
        self.buttons.store(u32::from(p.play.value()) | u32::from(p.cue.value()) << 1 | u32::from(p.touch.value()) << 2, Ordering::Relaxed);
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

    /// A scratch drawn as many small Platter moves turns the jog by exactly
    /// the distance drawn: no ticks lost to rounding, either way.
    #[test]
    fn platter_moves_turn_the_jog_by_the_distance_drawn() {
        let mut j = PlatterTicks::default();
        assert_eq!(j.ticks(0.0), 0, "the first reading only sets the origin");
        let total: i32 = (1..=1000).map(|i| j.ticks(i as f32 * 0.0013)).sum();   // 1.3 s forward in 1000 steps
        let want = f64::from(1000.0f32 * 0.0013) / JOG_SECS_PER_TICK;     // the distance drawn, in f32 as the host draws it
        assert!((f64::from(total) - want).abs() < 1.0, "{total} ticks for {want} (a part tick waits for the next move)");
        let back: i32 = (0..=1000).rev().map(|i| j.ticks(i as f32 * 0.0013)).sum();
        assert_eq!(back, -total, "and back to where it started");
    }

    /// Play is a level, Cue and Touch press while held.
    #[test]
    fn button_parameters_press_on_their_edges() {
        use ControlEvent as C;
        assert!(button_edges([false; 3], [false; 3]).is_empty());
        assert!(matches!(button_edges([false; 3], [true, false, false]).as_slice(), [C::Play]));
        assert!(matches!(button_edges([true, false, false], [false; 3]).as_slice(), [C::Pause]));
        assert!(matches!(button_edges([true, false, false], [true, true, false]).as_slice(), [C::Cue { pressed: true }]));
        assert!(matches!(button_edges([true, true, false], [true, false, false]).as_slice(), [C::Cue { pressed: false }]));
        assert!(matches!(button_edges([false; 3], [false, false, true]).as_slice(), [C::JogTouch { touched: true }]));
    }

    /// tools/reaper/opendeck_mixer.lua writes each deck's track into the
    /// saved state as `"fields":{"track":"\"<path>\""}`: pin that shape.
    #[test]
    fn the_saved_track_has_the_shape_the_reaper_script_writes() {
        let p = DeckParams::default();
        *p.track.write().unwrap() = "/music/a b.mp3".into();
        let fields = p.serialize_fields();
        assert_eq!(fields.get("track").map(String::as_str), Some("\"/music/a b.mp3\""));
        let mut q = DeckParams::default();
        q.deserialize_fields(&fields);
        assert_eq!(*q.track.read().unwrap(), "/music/a b.mp3", "and it reads back");
        let _ = &mut q;
    }

    /// Decks take the lowest free number, and a closed deck's number is free again.
    #[test]
    fn a_deck_draws_into_the_screen_the_host_made_for_its_number() {
        let dir = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let t = dir.join(format!("plugin-{}-deck{{n}}.screen", std::process::id()));
        let t = t.to_str().unwrap();
        let two = overdeck_display::Screen::path_for(t, 2);
        let _s = overdeck_display::Screen::create(&two, 8, 4).unwrap();
        assert_eq!(display_path(Some(t), 2), Some(two.clone()));
        assert_eq!(display_path(Some(t), 3), None, "no screen made for deck 3: it opens a window");
        assert_eq!(display_path(None, 2), None);
        let _ = std::fs::remove_file(two);
    }

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
