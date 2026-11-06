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

use crate::input::{ControlEvent as CE, Event, JOG_SECS_PER_TICK};
use opendeck_link::lanhid::{CdjButton, CdjControl};

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
        B::Browse => deck(CE::Load), // rotary push loads the selected track
        B::BeatJumpFwd => deck(CE::BeatJump { beats: 1.0 }),
        B::BeatJumpRev => deck(CE::BeatJump { beats: -1.0 }),
        // Modifiers / settings with no direct deck event yet.
        B::Shift | B::Quantize | B::TempoRange => None,
    }
}

// ── socket transport (Unix only; the LAN/UDP path is separate) ────────────
#[cfg(unix)]
pub use imp::spawn_from_env;

#[cfg(not(unix))]
pub fn spawn_from_env(_tx: std::sync::mpsc::Sender<Event>) {}

#[cfg(unix)]
mod imp {
    use super::{map, Event, JogScaler};
    use opendeck_link::lanhid::InputDecoder;
    use opendeck_link::lanhid_auth::{Handshake, Identity, Step};
    use opendeck_link::lanhid_status::{self, Status};
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Sender;
    use std::time::{Duration, Instant};

    /// If OPENDECK_CDJ_HID (+ _ID) are set, bring up a CDJ HID surface on its
    /// own thread. A missing/invalid identity is logged and the surface is
    /// skipped — the rest of the app runs regardless.
    pub fn spawn_from_env(tx: Sender<Event>) {
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
            .spawn(move || run(sock, identity, tx));
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

    fn run(sock: PathBuf, identity: Identity, tx: Sender<Event>) {
        loop {
            match session(&sock, &identity, &tx) {
                Ok(()) => log::info!("cdj-hid: session ended, retrying"),
                Err(e) => log::warn!("cdj-hid: {e}; retrying in 1s"),
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn session(sock: &Path, identity: &Identity, tx: &Sender<Event>) -> std::io::Result<()> {
        let mut stream = UnixStream::connect(sock)?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        log::info!("cdj-hid: connected to {}", sock.display());

        // A fixed challenge seed is fine for a control session; the deck's
        // seed E supplies the per-session entropy.
        let mut hs = Handshake::new(identity.clone(), [0x5A; 8]);
        let mut dec = InputDecoder::new();
        let mut jog = JogScaler::cdj();
        let mut established = false;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let mut out = Vec::with_capacity(72);
        let mut last_keepalive = Instant::now();
        let mut next_hello = Instant::now();

        loop {
            // Nudge the deck until it announces; keepalive once authenticated.
            if !established && Instant::now() >= next_hello {
                frame(3, &[], &mut out);
                stream.write_all(&out)?;
                next_hello = Instant::now() + Duration::from_millis(300);
            }
            if established && last_keepalive.elapsed() >= Duration::from_millis(300) {
                // A valid 0x21 host report doubles as the keepalive and keeps
                // the CDJ's own screen alive. Route byte: player 0, host kind.
                let mut st = lanhid_status::encode(&Status::default());
                st[0] = 0x01;
                frame(1, &st, &mut out);
                stream.write_all(&out)?;
                last_keepalive = Instant::now();
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
                            last_keepalive = Instant::now();
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
}
