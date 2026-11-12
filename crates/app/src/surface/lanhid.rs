//! CDJ HID-over-LINK control surface.
//!
//! A CDJ-3000 (or XDJ) in CONTROL MODE is a control surface for this deck:
//! its jog, tempo fader, pads and buttons drive OpenDeck, and OpenDeck feeds
//! the unit's own screen back. The wire protocol lives in `opendeck_link`
//! (`lanhid` framing/decode, `lanhid_auth` handshake, `lanhid_status`
//! feedback); this module is the bridge from that deck's decoded
//! [`CdjControl`] events onto the app's [`Event`] bus — the same bus the MIDI
//! and I2C surfaces feed, consumed by `DeckApp::apply`.
//!
//! The way back is the [`Feed`]: `DeckApp` publishes the deck's live state
//! (→ the 0x21 status, ~60 Hz) and, when the track, grid or cues change, its
//! display [`Assets`] (→ title, deck rows, RGB overview, scrolling waveform,
//! beat grid and hot cues). The session paces those fragments behind the
//! status. The CDJ shows them once it is in SOURCE → CONTROL MODE.
//!
//! Transport: for now a Unix stream carrying whole 64-byte reports with the
//! emulator/USB-B framing `type(1) | len(2, BE) | payload` (type 1 HID, 3
//! HELLO, 4 IDENTITY). The LAN/UDP tunnel path (the iPhone-as-host variant)
//! reuses the same [`map`] once its socket layer lands.
//!
//! Enabled out of band so no controllers.toml schema change is needed:
//!   OPENDECK_CDJ_HID     = /tmp/cdj3k-emu-1000/instance-1/usb-link.sock
//!   OPENDECK_CDJ_HID_ID  = /path/to/cdj_hid_identity.conf   (kept out of git)
//! The identity (posed-vendor secret + device ID) is loaded at runtime from
//! that conf, never compiled in — see `lanhid_auth`.

use crate::input::{ControlEvent as CE, Event, UiEvent, JOG_SECS_PER_TICK};
use opendeck_link::lanhid::{CdjButton, CdjControl};
use opendeck_link::lanhid_display::{self as disp, DeckRow, HotCue};
use opendeck_link::lanhid_frag::fragment;
use opendeck_link::lanhid_status::{LoopState, Status, TempoRange};
use opendeck_types::BeatGrid;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Tempo-fader raw span reported by the deck (0x15 field is about ±1000).
const TEMPO_RAW_SPAN: f32 = 1000.0;

/// Jog ticks the CDJ-3000 reports per platter revolution (brief, page 4:
/// "9,728 ticks/revolution").
const CDJ_JOG_TICKS_PER_REV: f64 = 9728.0;
/// How much audio a full platter revolution should scrub when paused — the
/// CDJ jog's feel, independent of the deck's dense native resolution.
const CDJ_JOG_REV_SECONDS: f64 = 1.8;

/// Rescales the CDJ's dense native jog ticks to the deck engine's canonical
/// jog tick ([`JOG_SECS_PER_TICK`]), carrying the sub-tick remainder so slow
/// turns aren't lost to rounding. One instance per session (it holds state).
pub struct JogScaler {
    /// Native-tick → canonical-tick factor.
    factor: f64,
    /// Fractional canonical ticks not yet emitted.
    carry: f64,
}

impl JogScaler {
    /// The CDJ scaler: a revolution of native ticks becomes
    /// `CDJ_JOG_REV_SECONDS / JOG_SECS_PER_TICK` canonical ticks.
    pub fn cdj() -> Self {
        let canon_per_rev = CDJ_JOG_REV_SECONDS / JOG_SECS_PER_TICK;
        JogScaler { factor: canon_per_rev / CDJ_JOG_TICKS_PER_REV, carry: 0.0 }
    }

    /// A scaler for a jog that counts `ticks_per_rev` per platter turn, with
    /// the CDJ feel (a turn scrubs `CDJ_JOG_REV_SECONDS`).
    pub fn per_rev(ticks_per_rev: f64) -> Self {
        let canon_per_rev = CDJ_JOG_REV_SECONDS / JOG_SECS_PER_TICK;
        JogScaler { factor: canon_per_rev / ticks_per_rev, carry: 0.0 }
    }

    /// Feed native ticks; return whole canonical ticks, keeping the remainder.
    pub fn step(&mut self, native: i32) -> i32 {
        let v = self.carry + native as f64 * self.factor;
        let whole = v.trunc();
        self.carry = v - whole;
        whole as i32
    }
}

/// Map one decoded CDJ control to a deck [`Event`], or `None` when the control
/// has no deck action (a pure modifier, or a press we only act on one edge of).
///
/// Pure and side-effect free so it can be unit-tested without a socket.
pub fn map(c: &CdjControl, jog: &mut JogScaler) -> Option<Event> {
    let deck = |e: CE| Some(Event::Deck(e));
    match *c {
        // ── analog ──────────────────────────────────────────────────────
        // The jog's tick delta scrubs (paused / VINYL) or nudges (CDJ mode);
        // the app decides which in `apply`. The deck's dense native ticks are
        // normalised to the engine's canonical jog tick so the CDJ platter
        // feels right at the same `JOG_SECS_PER_TICK` the other surfaces use.
        // The firmware motion field is not calibrated to true rpm, so velocity
        // is left 0 until a velocity-based scratch needs it.
        CdjControl::Jog { delta, .. } => match jog.step(delta) {
            0 => None,
            d => deck(CE::JogDelta { delta: d, velocity_rpm: 0.0 }),
        },
        CdjControl::Tempo { raw } => {
            // Deck geometry: fader bottom (fastest) = position 0, top = 1.
            // raw > 0 is toward the bottom, so position = 0.5 − raw/2span.
            let position = (0.5 - raw as f32 / (2.0 * TEMPO_RAW_SPAN)).clamp(0.0, 1.0);
            deck(CE::TempoFader { position })
        }
        CdjControl::Browse { delta } => deck(CE::BrowseEncoderDelta { delta }),
        // The CDJ's BROWSE key: follow the deck's browse-open level.
        CdjControl::BrowseView { on } => Some(Event::Ui(UiEvent::ShowBrowse(on))),

        // ── pads ────────────────────────────────────────────────────────
        CdjControl::HotCue { slot, pressed } => deck(if pressed {
            CE::HotCueTrigger { slot, held: true }
        } else {
            CE::HotCueRelease { slot }
        }),
        CdjControl::LoopLength { beats } => deck(CE::BeatLoop { beats, held: true }),
        CdjControl::BeatJumpSize { beats, forward } => {
            deck(CE::BeatJump { beats: if forward { beats } else { -beats } })
        }

        // ── buttons ─────────────────────────────────────────────────────
        CdjControl::Button { btn, pressed } => map_button(btn, pressed),
    }
}

fn map_button(btn: CdjButton, pressed: bool) -> Option<Event> {
    use CdjButton as B;
    let deck = |e: CE| Some(Event::Deck(e));
    match btn {
        // Momentary controls report both edges.
        B::Cue => deck(CE::Cue { pressed }),
        B::JogTouch => deck(CE::JogTouch { touched: pressed }),
        // Everything else fires on press only.
        _ if !pressed => None,
        B::Play => deck(CE::PlayPause), // CDJ PLAY/PAUSE is one toggling key
        B::Vinyl => deck(CE::JogModeToggle),
        B::MasterTempo => deck(CE::KeyLockToggle), // "MASTER TEMPO" = key lock
        B::TempoReset => deck(CE::TempoFader { position: 0.5 }),
        B::LoopIn => deck(CE::LoopIn),
        B::LoopOut => deck(CE::LoopOut),
        B::Reloop => deck(CE::Reloop),
        B::Back => deck(CE::Back),
        B::RotaryPush => deck(CE::Load), // rotary push loads the selected track
        B::BeatJumpFwd => deck(CE::BeatJump { beats: 1.0 }),
        B::BeatJumpRev => deck(CE::BeatJump { beats: -1.0 }),
        // Modifiers / settings with no direct deck event yet.
        B::Shift | B::Quantize | B::TempoRange => None,
    }
}


// ── display feed: deck state the CDJ's own screen mirrors ─────────────────

/// The deck's live state, in the units the CDJ wants. Built by `DeckApp`
/// once per frame while a CDJ session is up.
#[derive(Debug, Clone, Default)]
pub struct Live {
    pub loaded: bool,
    pub playing: bool,
    pub position_ms: u32,
    pub duration_ms: u32,
    /// Pitch-fader speed (1.0 = 0 %).
    pub rate: f32,
    /// The track's grid tempo; 0 = no grid.
    pub track_bpm: f64,
    pub cue_ms: Option<u32>,
    /// Active loop: (length in beats, 0 = unknown; start ms; end ms).
    pub loop_ms: Option<(f32, u32, u32)>,
    /// The fader's reach as a fraction (0.06, 0.10, 0.16, 1.00 = WIDE).
    pub tempo_range: f32,
    pub key_lock: bool,
    pub vinyl: bool,
    pub quantize: bool,
}

/// [`Live`] → the 0x21 status. `flash_on` alternates every 250 ms so IN/OUT
/// flash while a loop is active. The status table has no WIDE code, so WIDE
/// shows as ±16.
pub fn status(l: &Live, flash_on: bool) -> Status {
    let tempo_range = match l.tempo_range {
        r if r < 0.08 => TempoRange::Six,
        r if r < 0.13 => TempoRange::Ten,
        _ => TempoRange::Sixteen,
    };
    let x10 = |bpm: f64| (bpm * 10.0).round().clamp(0.0, u16::MAX as f64) as u16;
    let loop_active = l.loop_ms.map(|(beats, start_ms, end_ms)| {
        // A manual IN/OUT loop has no beat count: derive it from the tempo.
        let beats = if beats > 0.0 || l.track_bpm <= 0.0 {
            beats
        } else {
            (end_ms.saturating_sub(start_ms) as f64 * l.track_bpm / 60_000.0) as f32
        };
        LoopState { beats, start_ms, end_ms }
    });
    Status {
        playing: l.playing,
        track_loaded: l.loaded,
        tempo_range,
        master_tempo: l.key_lock,
        vinyl: l.vinyl,
        quantize: l.quantize,
        rate: l.rate,
        current_ms: l.position_ms,
        duration_ms: l.duration_ms,
        bpm_x10: x10(l.track_bpm * l.rate as f64),
        original_bpm_x10: x10(l.track_bpm),
        loop_active,
        loop_flash_on: flash_on,
        cue_ms: l.cue_ms,
        ..Default::default()
    }
}

/// What the CDJ's screen shows of the loaded track. Rebuilt by `DeckApp` only
/// when the track, its grid or its cues change; the session turns it into
/// display fragments.
#[derive(Debug, Clone, Default)]
pub struct Assets {
    pub title: String,
    pub artist: String,
    /// Waveform columns `[bass, mid, high, amplitude]`, `hop` frames each.
    pub columns: Vec<[u8; 4]>,
    pub hop: usize,
    pub sample_rate: u32,
    pub total_frames: u64,
    /// The beat grid, positions in **frames**.
    pub grid: Option<BeatGrid>,
    /// Hot cues A–H in ms.
    pub hot_cues_ms: [Option<u32>; 8],
}

impl Assets {
    pub fn duration_ms(&self) -> u32 {
        (self.total_frames * 1000 / self.sample_rate.max(1) as u64) as u32
    }

    /// The small, cheap-to-repeat set: deck rows, title, artist, hot cues.
    /// Pads take their CDJ palette colour by position (A–H).
    pub fn light_frames(&self) -> Vec<[u8; 64]> {
        let bpm_x10 = self.grid.as_ref().map(|g| (g.bpm * 10.0).round() as u16).unwrap_or(0);
        let rows = [DeckRow::label("OPENDECK"), DeckRow { bpm_x10, text: "Deck 1".into(), ..Default::default() }];
        let mut cues = [None; 8];
        for (i, c) in self.hot_cues_ms.iter().enumerate() {
            cues[i] = c.map(|ms| HotCue { color: disp::CUE_PALETTE[i], ms });
        }
        let mut f = fragment(disp::CMD_DECK_ROWS, &disp::deck_rows(&rows));
        f.extend(fragment(disp::CMD_TITLE, &disp::text(&self.title)));
        f.extend(fragment(disp::CMD_ARTIST, &disp::text(&self.artist)));
        f.extend(fragment(disp::CMD_HOT_CUES, &disp::hot_cues(self.duration_ms(), &cues)));
        f
    }

    /// The bulk transfer: RGB overview, scrolling waveform, beat grid.
    pub fn heavy_frames(&self) -> Vec<[u8; 64]> {
        let beats = self
            .grid
            .as_ref()
            .map(|g| disp::grid_beats(g, self.sample_rate, self.total_frames))
            .unwrap_or_default();
        let mut f = fragment(disp::CMD_OVERVIEW, &disp::overview(&self.columns));
        f.extend(fragment(disp::CMD_SCROLL, &disp::scroll(&self.columns, self.hop, self.sample_rate)));
        f.extend(fragment(disp::CMD_GRID, &disp::beat_grid(&beats)));
        f
    }
}

/// The deck → CDJ channel. Lives in [`super::Indicators`], so it reaches the
/// session the same way the LED bits reach the other surfaces.
#[derive(Default)]
pub struct Feed {
    /// A CDJ session is authenticated. `DeckApp` does no CDJ work while false.
    active: AtomicBool,
    live: Mutex<Live>,
    assets: Mutex<Option<(u64, Arc<Assets>)>>,
}

impl Feed {
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    fn set_active(&self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
        if !on {
            // A new session must be sent everything again.
            *self.assets.lock().unwrap() = None;
        }
    }

    pub fn publish_live(&self, l: Live) {
        *self.live.lock().unwrap() = l;
    }

    fn live(&self) -> Live {
        self.live.lock().unwrap().clone()
    }

    /// Publish the track's display assets under `key`; `build` runs only when
    /// the key differs from what was last published.
    pub fn publish_assets(&self, key: u64, build: impl FnOnce() -> Assets) {
        let mut a = self.assets.lock().unwrap();
        if a.as_ref().map(|(k, _)| *k) != Some(key) {
            *a = Some((key, Arc::new(build())));
        }
    }

    fn assets(&self) -> Option<(u64, Arc<Assets>)> {
        self.assets.lock().unwrap().clone()
    }
}

// ── socket transport (Unix only; the LAN/UDP path is separate) ────────────
// Not in the iOS build: the session handshake presents as rekordbox, which
// stays out of anything shipped until that is settled (docs/TODO-cdj-3000.md).
#[cfg(all(unix, not(target_os = "ios")))]
pub use imp::spawn_from_env;

#[cfg(any(not(unix), target_os = "ios"))]
pub fn spawn_from_env(_tx: std::sync::mpsc::Sender<Event>, _ind: Arc<super::Indicators>) {}

#[cfg(all(unix, not(target_os = "ios")))]
mod imp {
    use super::{map, status, Event, Feed, JogScaler};
    use crate::surface::Indicators;
    use opendeck_link::lanhid::InputDecoder;
    use opendeck_link::lanhid_auth::{Handshake, Identity, Step};
    use opendeck_link::lanhid_status;
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Status cadence (the brief's ~60 Hz).
    const STATUS_EVERY: Duration = Duration::from_millis(16);
    /// Deck rows, text and cues are re-offered this often: the CDJ only takes
    /// them once it is in Control Mode, which can happen at any time.
    const LIGHT_EVERY: Duration = Duration::from_secs(1);
    /// The waveforms and grid are re-sent this often for the same reason.
    const HEAVY_EVERY: Duration = Duration::from_secs(30);
    /// Display fragments sent per loop pass, so a ~3000-fragment waveform
    /// drains within a second or two without starving the status.
    const BURST: usize = 64;

    /// Marks the feed live for the session's lifetime.
    struct Active<'a>(&'a Feed);
    impl Drop for Active<'_> {
        fn drop(&mut self) {
            self.0.set_active(false);
        }
    }

    /// If OPENDECK_CDJ_HID (+ _ID) are set, bring up a CDJ HID surface on its
    /// own thread. A missing/invalid identity is logged and the surface is
    /// skipped — the rest of the app runs regardless.
    pub fn spawn_from_env(tx: Sender<Event>, ind: Arc<Indicators>) {
        let Some(sock) = std::env::var_os("OPENDECK_CDJ_HID") else { return };
        let sock = PathBuf::from(sock);
        let id_path = match std::env::var_os("OPENDECK_CDJ_HID_ID") {
            Some(p) => PathBuf::from(p),
            None => {
                log::error!("OPENDECK_CDJ_HID set but OPENDECK_CDJ_HID_ID (identity conf) is not");
                return;
            }
        };
        let identity = match Identity::load(&id_path) {
            Ok(id) => id,
            Err(e) => {
                log::error!("cdj-hid: cannot load identity {}: {e}", id_path.display());
                return;
            }
        };
        let r = std::thread::Builder::new()
            .name("cdj-hid".into())
            .spawn(move || run(sock, identity, tx, ind));
        if let Err(e) = r {
            log::warn!("cdj-hid: {e}");
        }
    }

    fn frame(ty: u8, payload: &[u8], out: &mut Vec<u8>) {
        let n = payload.len() as u16;
        out.clear();
        out.extend_from_slice(&[ty, (n >> 8) as u8, n as u8]);
        out.extend_from_slice(payload);
    }

    fn run(sock: PathBuf, identity: Identity, tx: Sender<Event>, ind: Arc<Indicators>) {
        loop {
            match session(&sock, &identity, &tx, &ind.cdj) {
                Ok(()) => log::info!("cdj-hid: session ended, retrying"),
                Err(e) => log::warn!("cdj-hid: {e}; retrying in 1s"),
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn session(sock: &Path, identity: &Identity, tx: &Sender<Event>, feed: &Feed) -> std::io::Result<()> {
        let mut stream = UnixStream::connect(sock)?;
        stream.set_read_timeout(Some(Duration::from_millis(5)))?;
        log::info!("cdj-hid: connected to {}", sock.display());

        // A fixed challenge seed is fine for a control session; the deck's
        // seed E supplies the per-session entropy.
        let mut hs = Handshake::new(identity.clone(), [0x5A; 8]);
        let mut dec = InputDecoder::new();
        let mut jog = JogScaler::cdj();
        let mut established = false;
        let mut _active = None;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let mut out = Vec::with_capacity(72);
        let mut next_hello = Instant::now();
        let mut last_status = Instant::now();
        let started = Instant::now();

        // Display fragments waiting to go out, and what they were built from.
        let mut queue: VecDeque<[u8; 64]> = VecDeque::new();
        let mut sent_key: Option<u64> = None;
        let mut last_light = Instant::now();
        let mut last_heavy = Instant::now();

        loop {
            // Nudge the deck until it announces.
            if !established && Instant::now() >= next_hello {
                frame(3, &[], &mut out);
                stream.write_all(&out)?;
                next_hello = Instant::now() + Duration::from_millis(300);
            }
            if established && last_status.elapsed() >= STATUS_EVERY {
                // The 0x21 status is also the session keepalive. Byte 0 (the
                // LAN route) stays zero on this socket.
                let flash_on = (started.elapsed().as_millis() / 250) % 2 == 0;
                let st = lanhid_status::encode(&status(&feed.live(), flash_on));
                frame(1, &st, &mut out);
                stream.write_all(&out)?;
                last_status = Instant::now();

                if let Some((key, assets)) = feed.assets() {
                    if sent_key != Some(key) {
                        // New track / grid / cues: drop whatever of the old
                        // track is still queued (the brief: cancel stale
                        // transfers), then send everything.
                        queue.clear();
                        queue.extend(assets.light_frames());
                        queue.extend(assets.heavy_frames());
                        log::info!("cdj-hid: sending display for {:?} ({} fragments)", assets.title, queue.len());
                        sent_key = Some(key);
                        last_light = Instant::now();
                        last_heavy = Instant::now();
                    } else if last_heavy.elapsed() >= HEAVY_EVERY && queue.is_empty() {
                        queue.extend(assets.light_frames());
                        queue.extend(assets.heavy_frames());
                        last_light = Instant::now();
                        last_heavy = Instant::now();
                    } else if last_light.elapsed() >= LIGHT_EVERY {
                        queue.extend(assets.light_frames());
                        last_light = Instant::now();
                    }
                }
            }
            for _ in 0..BURST {
                let Some(f) = queue.pop_front() else { break };
                frame(1, &f, &mut out);
                stream.write_all(&out)?;
            }

            match stream.read(&mut tmp) {
                Ok(0) => return Ok(()), // EOF
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => return Err(e),
            }

            while buf.len() >= 3 {
                let len = ((buf[1] as usize) << 8) | buf[2] as usize;
                if buf.len() < 3 + len {
                    break;
                }
                let ty = buf[0];
                let payload = buf[3..3 + len].to_vec();
                buf.drain(..3 + len);
                if ty != 1 || payload.len() < 2 {
                    continue;
                }
                if !established && payload[1] == 0xF0 {
                    match hs.on_frame(&payload) {
                        Step::Send(f) => {
                            frame(1, &f, &mut out);
                            stream.write_all(&out)?;
                        }
                        Step::Established => {
                            established = true;
                            feed.set_active(true);
                            _active = Some(Active(feed));
                            last_status = Instant::now();
                            log::info!("cdj-hid: handshake established");
                        }
                        Step::Waiting => {}
                        Step::Failed(why) => log::debug!("cdj-hid: handshake {why}"),
                    }
                } else if established && payload[1] == 0x20 {
                    for c in dec.decode(&payload) {
                        if let Some(ev) = map(&c, &mut jog) {
                            let _ = tx.send(ev);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::ControlEvent as CE;

    fn deck(c: CdjControl) -> Option<CE> {
        match map(&c, &mut JogScaler::cdj()) {
            Some(Event::Deck(e)) => Some(e),
            _ => None,
        }
    }

    #[test]
    fn browse_key_opens_and_closes_browse_and_rotary_push_loads() {
        let ui = |c| match map(&c, &mut JogScaler::cdj()) {
            Some(Event::Ui(e)) => Some(e),
            _ => None,
        };
        assert!(matches!(ui(CdjControl::BrowseView { on: true }), Some(UiEvent::ShowBrowse(true))));
        assert!(matches!(ui(CdjControl::BrowseView { on: false }), Some(UiEvent::ShowBrowse(false))));
        assert!(matches!(
            deck(CdjControl::Button { btn: CdjButton::RotaryPush, pressed: true }),
            Some(CE::Load)
        ));
    }

    #[test]
    fn jog_is_normalised_not_passed_through() {
        // A single dense native delta scales down to the canonical tick; the
        // factor is well under 1, so a small delta carries to nothing yet.
        assert!(deck(CdjControl::Jog { delta: 37, motion: 5, position: 100 }).is_none());
        // A larger delta crosses a whole canonical tick.
        let big = match deck(CdjControl::Jog { delta: 300, motion: 0, position: 0 }) {
            Some(CE::JogDelta { delta, .. }) => delta,
            other => panic!("{other:?}"),
        };
        assert!(big >= 1 && big <= 4, "300 native ticks → {big} canonical");
    }

    #[test]
    fn jog_calibration_one_rev_is_rev_seconds() {
        // One platter revolution (9728 native ticks), fed in realistic chunks,
        // must accumulate to CDJ_JOG_REV_SECONDS / JOG_SECS_PER_TICK canonical
        // ticks — i.e. a revolution scrubs CDJ_JOG_REV_SECONDS of audio.
        let mut jog = JogScaler::cdj();
        let mut total = 0i64;
        let mut fed = 0;
        while fed < 9728 {
            let n = 100.min(9728 - fed);
            if let Some(Event::Deck(CE::JogDelta { delta, .. })) =
                map(&CdjControl::Jog { delta: n, motion: 0, position: 0 }, &mut jog)
            {
                total += delta as i64;
            }
            fed += n;
        }
        let expected = (CDJ_JOG_REV_SECONDS / JOG_SECS_PER_TICK).round() as i64; // = 90
        // Within one tick: the accumulator leaves the final sub-tick pending
        // (it emits on the next movement), so a lone revolution yields 89–90.
        assert!((total - expected).abs() <= 1, "one rev → {total} canonical ticks");
        // And that many canonical ticks is ~CDJ_JOG_REV_SECONDS of audio.
        assert!((total as f64 * JOG_SECS_PER_TICK - CDJ_JOG_REV_SECONDS).abs() < 0.05);
    }

    #[test]
    fn tempo_polarity() {
        // Center is 0.5; positive raw (toward the bottom) speeds up → lower
        // position; negative raw → higher.
        let pos = |raw| match deck(CdjControl::Tempo { raw }) {
            Some(CE::TempoFader { position }) => position,
            _ => panic!(),
        };
        assert!((pos(0) - 0.5).abs() < 1e-6);
        assert!(pos(1000) < 0.5);
        assert!(pos(-1000) > 0.5);
    }

    #[test]
    fn play_is_press_only_toggle() {
        assert!(matches!(
            deck(CdjControl::Button { btn: CdjButton::Play, pressed: true }),
            Some(CE::PlayPause)
        ));
        assert!(deck(CdjControl::Button { btn: CdjButton::Play, pressed: false }).is_none());
    }

    #[test]
    fn cue_reports_both_edges() {
        assert!(matches!(
            deck(CdjControl::Button { btn: CdjButton::Cue, pressed: true }),
            Some(CE::Cue { pressed: true })
        ));
        assert!(matches!(
            deck(CdjControl::Button { btn: CdjButton::Cue, pressed: false }),
            Some(CE::Cue { pressed: false })
        ));
    }

    #[test]
    fn hot_cue_press_and_release() {
        assert!(matches!(
            deck(CdjControl::HotCue { slot: 2, pressed: true }),
            Some(CE::HotCueTrigger { slot: 2, held: true })
        ));
        assert!(matches!(
            deck(CdjControl::HotCue { slot: 2, pressed: false }),
            Some(CE::HotCueRelease { slot: 2 })
        ));
    }

    #[test]
    fn beat_jump_direction() {
        assert!(matches!(
            deck(CdjControl::BeatJumpSize { beats: 4.0, forward: false }),
            Some(CE::BeatJump { beats }) if beats == -4.0
        ));
    }

    #[test]
    fn modifiers_have_no_deck_event() {
        for b in [CdjButton::Shift, CdjButton::Quantize, CdjButton::TempoRange] {
            assert!(deck(CdjControl::Button { btn: b, pressed: true }).is_none());
        }
    }

    #[test]
    fn status_maps_tempo_bpm_and_range() {
        let l = Live {
            loaded: true,
            playing: true,
            position_ms: 1234,
            duration_ms: 527_328,
            rate: 1.05,
            track_bpm: 141.8,
            tempo_range: 0.10,
            key_lock: true,
            ..Default::default()
        };
        let st = status(&l, false);
        assert!(st.track_loaded && st.playing && st.master_tempo);
        assert_eq!(st.tempo_range, TempoRange::Ten);
        assert_eq!(st.original_bpm_x10, 1418);
        assert_eq!(st.bpm_x10, 1489, "current BPM follows the pitch fader");
        assert_eq!((st.current_ms, st.duration_ms), (1234, 527_328));
        // WIDE has no status code; it shows as ±16.
        assert_eq!(status(&Live { tempo_range: 1.0, ..l.clone() }, false).tempo_range, TempoRange::Sixteen);
        assert_eq!(status(&Live { tempo_range: 0.06, ..l }, false).tempo_range, TempoRange::Six);
    }

    #[test]
    fn manual_loop_gets_its_length_in_beats_from_the_tempo() {
        let l = Live { track_bpm: 120.0, loop_ms: Some((0.0, 1000, 3000)), ..Default::default() };
        let lp = status(&l, true).loop_active.unwrap();
        assert!((lp.beats - 4.0).abs() < 1e-3, "2 s at 120 BPM = 4 beats, got {}", lp.beats);
        let l = Live { track_bpm: 120.0, loop_ms: Some((8.0, 0, 4000)), ..Default::default() };
        assert_eq!(status(&l, true).loop_active.unwrap().beats, 8.0);
    }

    fn assets() -> Assets {
        let mut g = BeatGrid::new_constant(0, 120.0);
        g.downbeat_offset = 0;
        let mut hot = [None; 8];
        hot[0] = Some(1_000);
        Assets {
            title: "Amplificate".into(),
            artist: "Dan Newcome".into(),
            columns: vec![[200, 40, 10, 180]; 10 * 44_100 / 512],
            hop: 512,
            sample_rate: 44_100,
            total_frames: 10 * 44_100,
            grid: Some(g),
            hot_cues_ms: hot,
        }
    }

    #[test]
    fn asset_fragments_carry_every_display_command() {
        let a = assets();
        let cmds = |f: &[[u8; 64]]| f.iter().map(|r| r[1]).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(cmds(&a.light_frames()).into_iter().collect::<Vec<_>>(), vec![0x22, 0x23, 0x2A, 0x30]);
        let heavy = a.heavy_frames();
        assert_eq!(cmds(&heavy).into_iter().collect::<Vec<_>>(), vec![0x2C, 0x2E, 0x2F]);
        // 10 s: overview 73 + scroll (4 + 1500×2 B → 52) + grid (2 + 20×4 B → 2).
        assert_eq!(heavy.len(), 73 + 52 + 2);
        assert_eq!(a.duration_ms(), 10_000);
    }

    #[test]
    fn feed_rebuilds_assets_only_when_the_key_changes() {
        let feed = Feed::default();
        let mut builds = 0;
        for key in [1, 1, 1, 2, 2] {
            feed.publish_assets(key, || {
                builds += 1;
                assets()
            });
        }
        assert_eq!(builds, 2);
        assert_eq!(feed.assets().unwrap().0, 2);
        // A session ending forgets them so the next one is sent everything.
        feed.set_active(true);
        feed.set_active(false);
        assert!(feed.assets().is_none());
    }
}
