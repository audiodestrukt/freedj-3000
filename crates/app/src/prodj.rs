//! ProDJ Link: listener, sender, tempo-master handoff, and SYNC follow.
//!
//! Testing without a second deck: docs/reference/link-test-harness.md
//! (`make link-pair` for two freedj instances, `make virtual-cdj` for a full
//! virtual CDJ).  Verified against a real XDJ-1000MK2 on 2026-08-26.
//!
//! Threads:
//!   prodj-rx-50000   announces → peer table
//!   prodj-rx-50001   beats (→ B2 strip, → sync follow), handoff and sync-control packets
//!   prodj-rx-50002   status → who is master, handoff progress
//!   prodj-tx         announce 1.5 s, status 200 ms to each peer, beat at each beat,
//!                    handoff state machine, sync follow
//!
//! All shared state is atomics or a small mutex over the peer table; the
//! audio thread is never touched.

use opendeck_link::prodj::{
    AbsPosition, ActiveLoop, ProDjLink, Status, StatusFields, wants_cdj3000_status, BECOME_MASTER, PORT_ANNOUNCE, PORT_BEAT, PORT_STATUS,
    SYNC_OFF, SYNC_ON, PKT_MEDIA_QUERY, MediaInfo};
use arc_swap::ArcSwap;
use opendeck_types::{BeatGrid, EngineSnapshot};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

// ── Shared link state ─────────────────────────────────────────────────────────

/// A media response older than this no longer counts (the stick was pulled,
/// or the player went away); queries go out every [`MEDIA_QUERY_EVERY`].
const MEDIA_STALE:       Duration = Duration::from_secs(20);
const MEDIA_QUERY_EVERY: Duration = Duration::from_secs(5);

/// Everything the Link threads and the UI share.  Written by the threads,
/// read by the snapshot; `want_master` / `sync` are written by `DeckApp::apply`.
pub struct LinkState {
    /// Our player number: the one we hold once claimed, the one being tried
    /// while claiming.  Read it with `player()`.
    player:            AtomicU8,
    /// PLAYER No. AUTO: pick a free number at start-up, as a CDJ set to AUTO
    /// does, and move to another if a device turns out to hold ours.
    pub auto:          bool,
    /// The number AUTO tries first when it is free (the platform's old
    /// fixed default: "deck 3" on an iPad).
    pub preferred:     u8,
    /// The number claim has finished (no device defended it).
    pub claimed:       AtomicBool,
    /// A device defended this number against our claim (0 = none yet).
    pub defended:      AtomicU8,
    /// A device announced or defended the number we hold: claim again.
    pub reclaim:       AtomicBool,
    /// Our Link IP and the sender's port, so our own broadcasts can be told
    /// from a peer's (two OpenDecks on one machine share the IP).
    pub own_src:       Mutex<Option<(Ipv4Addr, u16)>>,
    /// The loaded track's source and ID, as our status reports it (None =
    /// no track).  Set on every load by the deck.
    pub track:         Mutex<Option<opendeck_link::prodj::TrackRef>>,
    /// The library our media server serves (for HISTORY), once it is up.
    pub library:       Mutex<Option<opendeck_dbserver::server::SharedLibrary>>,
    /// Our media server's tracks: canonical path → the ID peers look it up by.
    pub served:        Mutex<HashMap<std::path::PathBuf, u32>>,
    /// Our Link broadcast address (defenses go there as well as unicast).
    pub own_bcast:     Mutex<Option<Ipv4Addr>>,
    /// We are the tempo master.
    pub master:        AtomicBool,
    /// SYNC engaged: follow the master's tempo and phase.
    pub sync:          AtomicBool,
    /// MASTER pressed; the sender runs the handoff.
    pub want_master:   AtomicBool,
    /// Player we are yielding master to (0 = none).
    pub handoff_to:    AtomicU32,
    /// Peer currently claiming master, from its status (0 = none).
    pub master_player: AtomicU32,
    /// Effective BPM of the master's last beat packet (f32 bits).
    pub master_bpm:    AtomicU32,
    /// Bumped on every beat packet from the master — the phase reference.
    pub master_beat_seq: AtomicU64,
    /// When we last took master, ms since `epoch` (0 = never).
    pub master_since_ms: AtomicU64,
    pub epoch:         Instant,
    /// Largest sync counter seen in any peer's status.
    pub largest_sync:  AtomicU32,
    /// Our sync counter: set to largest_sync + 1 when we take master.
    pub our_sync:      AtomicU32,
    /// The master that agreed (0x27) to yield to us (0 = none).  Beat Link's
    /// `masterYieldedFrom`: once set we stop re-requesting and wait to see our
    /// own number in the master's Mh byte before actually taking master.
    pub yielded_from:  AtomicU32,
    /// player → ip, from announces.
    pub peers:         Mutex<HashMap<u8, Ipv4Addr>>,
    /// player → device name from its announce ("XDJ-1000MK2", "rekordbox", …).
    pub peer_names:    Mutex<HashMap<u8, String>>,
    /// Each player's latest Absolute Position (a CDJ-3000 or an OpenDeck,
    /// ~33 a second, sent straight to us) and when it came.
    pub positions:     Mutex<HashMap<u8, (AbsPosition, Instant)>>,
    /// Each player's latest status (its loaded track, tempo, …).
    pub peer_status:   Mutex<HashMap<u8, Status>>,
    /// When each player's latest status and latest beat packet came: a
    /// FOLLOW deck places an older player's playhead from them (`follow`).
    pub peer_heard:    Mutex<HashMap<u8, PeerHeard>>,
    /// Tracks we serve as a Link media source (0 = not serving).  Non-zero
    /// answers media queries and flags our USB slot loaded in status.
    pub serve_tracks:  AtomicU32,
    /// (player, slot) → what that player said is in the slot, and when.  The
    /// sender asks every player about USB and SD periodically; a stick that
    /// is pulled simply stops being confirmed, see [`LinkState::media`].
    pub peer_media:    Mutex<HashMap<(u8, u8), (MediaInfo, Instant)>>,
    /// When the deck plotted on the phase meter's top row (`beat2_player`)
    /// last sent a beat, and last sent anything (beat or status), in ms since
    /// `epoch`; 0 = never.  The renderer extrapolates the row's phase from the
    /// beat time and drops the row when the peer falls silent.
    pub beat2_beat_ms: AtomicU64,
    pub beat2_seen_ms: AtomicU64,
    /// Whether that deck's last status said PLAY: its beat row free-runs while
    /// this is set even if beats arrive late or bunched (Wi-Fi power save
    /// delivers broadcasts in bursts), and holds when it is not.
    pub beat2_playing: AtomicBool,
    /// When the deck we hold as `master_player` last sent status (ms since
    /// `epoch`).  A master silent for `MASTER_GONE_MS` is forgotten, so a
    /// MASTER press after a long idle takes the role instead of asking a
    /// deck that is no longer there.
    pub master_seen_ms: AtomicU64,
    /// What the sender is speaking from: "ip (iface) → broadcast", for the
    /// INFO page — so a deck on the wrong interface or subnet is visible on
    /// the device itself.
    pub own_addr:      Mutex<String>,
    /// SYNC's held phase offset, in beats (f32 bits). A jog nudge while
    /// synced leaves the deck off the master's beat by this much, and SYNC
    /// keeps it there (as a CDJ does) instead of pulling back to 0. SHIFT
    /// GRID folds it into the beat grid. Cleared when SYNC goes off.
    pub sync_offset:   AtomicU32,
    /// Until when (ms since `epoch`) the jog is nudging a synced deck; SYNC
    /// leaves the phase alone meanwhile and measures the new offset after.
    pub jog_nudge_ms:  AtomicU64,
}

/// When a player's latest status and beat packet arrived.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeerHeard { pub status_at: Option<Instant>, pub beat_at: Option<Instant>, pub beat_bib: Option<u8> }

/// How long after the last jog nudge SYNC waits before measuring the offset
/// the nudge left (the deck is back at the fader speed by then).
const JOG_SETTLE_MS: u64 = 300;

/// The phase error SYNC corrects, in beats within ±0.5: how far `frac` (our
/// position in our own beat when the master's beat lands) is from the held
/// `offset`.
fn phase_err(frac: f64, offset: f64) -> f64 {
    let e = frac - offset;
    e - e.round()
}

/// What SYNC does with the phase on one master beat.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PhaseAct {
    /// Leave the speed alone (on target, or the jog owns it).
    None,
    /// A nudge just settled: hold this offset (beats) from now on.
    Hold(f64),
    /// Run at the fader speed plus this fraction for one beat.
    Correct(f32),
}

/// SYNC's phase decisions, kept out of the Link thread so they can be tested
/// as sequences. A CDJ keeps the offset a jog bend leaves while synced (SHIFT
/// GRID then writes it into the grid); this does the same.
#[derive(Default)]
struct SyncPhase {
    /// A nudge happened; measure the offset at the next beat after it settles.
    pending: bool,
}

impl SyncPhase {
    /// Every loop pass. Returns true when the held offset must be cleared
    /// (SYNC is off).
    fn tick(&mut self, sync: bool, jogging: bool) -> bool {
        if !sync {
            self.pending = false;
            return true;
        }
        if jogging { self.pending = true; }
        false
    }

    /// A master beat landed with us `frac` (0..1) into our own beat.
    fn on_master_beat(&mut self, frac: f64, offset: f64, jogging: bool) -> PhaseAct {
        if jogging { return PhaseAct::None; }
        if self.pending {
            self.pending = false;
            return PhaseAct::Hold(phase_err(frac, 0.0));
        }
        let err = phase_err(frac, offset);
        // Correct half the error per master beat: bounded like a jog nudge,
        // and halving avoids the overshoot a full correction gives (measured
        // ±0.02 beat oscillation at gain 1).
        if err.abs() > 0.01 { PhaseAct::Correct((-(err as f32) * 0.5).clamp(-0.03, 0.03)) } else { PhaseAct::None }
    }
}

impl LinkState {
    /// A fixed player number.
    pub fn new(player: u8) -> Arc<Self> { Self::build(player, false) }

    /// PLAYER No. AUTO, trying `preferred` first.
    pub fn auto(preferred: u8) -> Arc<Self> { Self::build(preferred, true) }

    pub fn player(&self) -> u8 { self.player.load(Ordering::Relaxed) }

    fn build(player: u8, auto: bool) -> Arc<Self> {
        Arc::new(Self {
            player: AtomicU8::new(player),
            auto,
            preferred: player,
            claimed: AtomicBool::new(false),
            defended: AtomicU8::new(0),
            reclaim: AtomicBool::new(false),
            own_src: Mutex::new(None),
            own_bcast: Mutex::new(None),
            track: Mutex::new(None),
            served: Mutex::new(HashMap::new()),
            library: Mutex::new(None),
            master: AtomicBool::new(false),
            sync: AtomicBool::new(false),
            want_master: AtomicBool::new(false),
            handoff_to: AtomicU32::new(0),
            master_player: AtomicU32::new(0),
            master_bpm: AtomicU32::new(0.0f32.to_bits()),
            master_beat_seq: AtomicU64::new(0),
            master_since_ms: AtomicU64::new(0),
            epoch: Instant::now(),
            largest_sync: AtomicU32::new(0),
            our_sync: AtomicU32::new(0),
            yielded_from: AtomicU32::new(0),
            peers: Mutex::new(HashMap::new()),
            peer_names: Mutex::new(HashMap::new()),
            positions: Mutex::new(HashMap::new()),
            peer_status: Mutex::new(HashMap::new()),
            peer_heard: Mutex::new(HashMap::new()),
            serve_tracks: AtomicU32::new(0),
            peer_media: Mutex::new(HashMap::new()),
            beat2_beat_ms: AtomicU64::new(0),
            beat2_seen_ms: AtomicU64::new(0),
            beat2_playing: AtomicBool::new(false),
            sync_offset: AtomicU32::new(0.0f32.to_bits()),
            jog_nudge_ms: AtomicU64::new(0),
            master_seen_ms: AtomicU64::new(0),
            own_addr: Mutex::new(String::new()),
        })
    }

    /// Peers that are OpenDecks (by announce name), for beat unicast.
    pub fn opendeck_peers(&self) -> Vec<(u8, Ipv4Addr)> {
        let names = self.peer_names.lock().map(|n| n.clone()).unwrap_or_default();
        self.peers.lock().map(|p| p.iter()
            .filter(|(pl, _)| names.get(pl).map_or(false, |n| n.starts_with("freedj") || n.starts_with("OpenDeck")))
            .map(|(pl, ip)| (*pl, *ip)).collect()).unwrap_or_default()
    }

    /// The players heard on the network, one line for the INFO page:
    /// "3 XDJ-1000MK2 192.168.1.10 · 4 freedj-3000 192.168.1.57 (master)".
    pub fn peers_summary(&self) -> String {
        let peers = self.peers.lock().map(|p| { let mut v: Vec<_> = p.iter().map(|(k, v)| (*k, *v)).collect(); v.sort(); v }).unwrap_or_default();
        if peers.is_empty() { return "none heard".into(); }
        let names = self.peer_names.lock().map(|n| n.clone()).unwrap_or_default();
        let master = self.master_player.load(Ordering::Relaxed) as u8;
        peers.iter().map(|(pl, ip)| {
            let name = names.get(pl).cloned().unwrap_or_default();
            format!("{pl} {name} {ip}{}", if *pl == master { " (master)" } else { "" })
        }).collect::<Vec<_>>().join(" · ")
    }

    /// Milliseconds since this Link state was created (the clock the
    /// `*_ms` fields use).
    pub fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Media a player currently has, freshest first by slot (USB before SD):
    /// responses confirmed within the last [`MEDIA_STALE`].
    pub fn media(&self, player: u8) -> Vec<MediaInfo> {
        let mut v: Vec<MediaInfo> = self.peer_media.lock().map(|m| m.iter()
            .filter(|((p, _), (_, at))| *p == player && at.elapsed() < MEDIA_STALE)
            .map(|(_, (info, _))| info.clone()).collect()).unwrap_or_default();
        v.sort_by_key(|m| std::cmp::Reverse(m.slot));
        v
    }

    /// MASTER pressed while master: hand the role to another player, as a
    /// CDJ-3000 does (docs/reference/cdj-3000-master-and-waveform-view.md).
    /// The unit names its successor in its status (the handoff byte) and
    /// drops the flag once that player claims it; it only hands to a player
    /// that is playing, and with none it stays master.  Which of several
    /// playing players it picks is not known yet: the lowest number here.
    /// Returns the player handed to.
    pub fn hand_off_master(&self, why: &str) -> Option<u8> {
        let me = self.player();
        let to = self.peer_status.lock().ok()?.iter()
            .filter(|(p, st)| **p != me && st.playing)
            .map(|(p, _)| *p).min();
        match to {
            Some(p) => {
                self.want_master.store(false, Ordering::Relaxed);
                self.handoff_to.store(p as u32, Ordering::Relaxed);
                log::info!("ProDJ Link: handing master to player {p} ({why})");
            }
            None => log::info!("ProDJ Link: keeping master ({why}): no other player is playing"),
        }
        to
    }

    /// Assert the master role with a sync counter newer than anything seen.
    fn take_master(&self, why: &str) {
        let n = self.largest_sync.load(Ordering::Relaxed) + 1;
        self.our_sync.store(n, Ordering::Relaxed);
        self.master.store(true, Ordering::Relaxed);
        self.want_master.store(false, Ordering::Relaxed);
        self.yielded_from.store(0, Ordering::Relaxed);
        self.master_since_ms.store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        log::info!("ProDJ Link: taking master ({why}), sync counter {n}");
    }

    /// The player number a peer at `ip` announced.
    pub fn peer_player(&self, ip: Ipv4Addr) -> Option<u8> {
        self.peers.lock().ok()?.iter().find(|(_, a)| **a == ip).map(|(p, _)| *p)
    }

    fn peer_ip(&self, player: u8) -> Option<Ipv4Addr> {
        self.peers.lock().ok()?.get(&player).copied()
    }
}

/// A position a hair before a beat line is on it.  Loop points are whole
/// frames, so a loop's start can sit a fraction of a frame before the grid's
/// beat; landing there read as the beat before the loop for an instant and
/// sent it ("beat 1", then "beat 2" a millisecond later, on each wrap).
pub fn on_beat_line(beat_f: f64) -> f64 {
    let r = beat_f.round();
    if (r - beat_f).abs() < 1e-3 { r } else { beat_f }
}

/// Within this many frames of a loop's end (≈2 ms) the playhead is at the
/// loop's start: the end is rounded to a whole frame and can sit a hair past
/// the grid's beat, which sent the beat after the loop just before each wrap.
const LOOP_EDGE: f64 = 96.0;

/// The sender's idea of the audible playhead, in frames: the decoder cursor
/// less what is queued is only good to a block (±35 ms), so free-run at the
/// true rate and pull gently toward that reference.  Loop-aware: the
/// reference walks back modulo the loop (the cursor wraps before the tail is
/// heard), and the estimate wraps at the loop's end however short the loop
/// (it used to snap only on a jump over half a second, so ½- and 1-beat
/// loops ran on through the song over Link).
#[derive(Debug, Clone)]
pub struct PlayheadEstimate {
    /// Frames; negative = not running.
    pub frames: f64,
    ahead_avg: f64,
}

impl Default for PlayheadEstimate { fn default() -> Self { PlayheadEstimate { frames: -1.0, ahead_avg: 0.0 } } }

impl PlayheadEstimate {
    /// Move on `step` frames (the last stretch before a beat), going round
    /// the loop at its end.  True when it went round.
    pub fn advance(&mut self, step: f64, lp: Option<(u64, u64)>) -> bool {
        self.frames += step;
        match lp.filter(|(a, b)| b > a) {
            Some((a, b)) if self.frames >= b as f64 - LOOP_EDGE => {
                self.frames = a as f64 + (self.frames - b as f64).max(0.0) % (b - a) as f64;
                true
            }
            _ => false,
        }
    }

    /// One tick: decoder cursor `pos` and `in_flight` (frames), the loop
    /// `[start, end)` if one runs, `step` frames of play since the last tick.
    /// True when the estimate went round the loop.
    pub fn update(&mut self, pos: u64, in_flight: f64, lp: Option<(u64, u64)>, step: f64, sr: u32) -> bool {
        self.ahead_avg += (in_flight - self.ahead_avg) * 0.005;
        let lp = lp.filter(|(a, b)| b > a);
        let mut reference = pos as f64 - self.ahead_avg;
        if let Some((a, b)) = lp {
            let (a, b) = (a as f64, b as f64);
            if reference < a && pos as f64 >= a { reference = b - ((a - reference) % (b - a)); }
        }
        let reference = reference.max(0.0);
        let mut wrapped = false;
        if self.frames < 0.0 {
            self.frames = reference;
        } else {
            self.frames += step;
            if let Some((a, b)) = lp {
                let (a, b) = (a as f64, b as f64);
                if self.frames >= b - LOOP_EDGE && self.frames - step < b + (b - a) {
                    self.frames = a + (self.frames - b).max(0.0) % (b - a);
                    wrapped = true;
                }
            }
            // The pull runs the short way round a loop.
            let mut err = reference - self.frames;
            if let Some((a, b)) = lp {
                let len = (b - a) as f64;
                if err > len / 2.0 { err -= len } else if err < -len / 2.0 { err += len }
            }
            if err.abs() > sr as f64 * 0.5 { self.frames = reference; } else { self.frames += err * 0.005; }
            // Inside a loop the estimate stays inside it.
            if let Some((a, b)) = lp {
                let (a, b) = (a as f64, b as f64);
                if self.frames < a && reference >= a { self.frames += b - a; }
            }
        }
        wrapped
    }
}

/// The sender's beat clock: the playhead estimate, the beat it is in, and
/// which beats go out as beat packets.  The sender thread drives it once a
/// millisecond; tests drive it the same way without the sockets or the
/// spin-wait.
#[derive(Debug, Clone, Default)]
pub struct BeatClock {
    pub est: PlayheadEstimate,
    /// The beat the playhead is in (grid index) and how far into it.
    pub beat_now: Option<(i64, f64)>,
    last_beat: Option<i64>,
    wrapped: bool,
}

impl BeatClock {
    /// One tick while playing: decoder cursor and in-flight (frames), the
    /// loop if one runs, `step` frames of play since the last tick at
    /// `rate` frames a second.  Returns the seconds to the next beat line
    /// (the caller spins to it when it is under 1.5 ms, then passes the
    /// frames it moved to [`finish`](Self::finish)).
    pub fn tick(&mut self, grid: &BeatGrid, sr: u32, pos: u64, in_flight: f64, lp: Option<(u64, u64)>, step: f64, rate: f64) -> f64 {
        self.wrapped = self.est.update(pos, in_flight, lp, step, sr);
        let at = self.est.frames.max(0.0) as u64;
        let period = grid.samples_per_beat_at(at, sr);
        let beat_f = grid.beat_at_sample(at, sr);
        ((beat_f.floor() + 1.0) - beat_f) * period / rate.max(1.0)
    }

    /// After the tick (and the spin, which moved `landed` frames): the beat
    /// to send now, if one is due.  One per beat line crossed; a loop going
    /// round (however short) or a seek sends the beat it lands on.
    pub fn finish(&mut self, grid: &BeatGrid, sr: u32, landed: Option<f64>, lp: Option<(u64, u64)>) -> Option<i64> {
        // Landing on a loop's end is landing on its start.
        if let Some(f) = landed { self.wrapped |= self.est.advance(f, lp); }
        let beat_f = on_beat_line(grid.beat_at_sample(self.est.frames.max(0.0) as u64, sr));
        let beat = beat_f.floor() as i64;
        self.beat_now = Some((beat, beat_f - beat_f.floor()));
        let seek = self.wrapped || self.last_beat.map_or(false, |b| beat < b - 2 || beat > b + 8);
        let due = self.last_beat.map_or(true, |b| beat > b) || seek;
        self.wrapped = false;
        if due { self.last_beat = Some(beat); Some(beat) } else { None }
    }

    /// Stopped: no estimate, no beat.
    pub fn stop(&mut self) {
        self.est.frames = -1.0;
        self.beat_now = None;
        self.last_beat = None;
    }
}

/// Deck state the sender reads; all atomics, nothing locked.
pub struct SenderState {
    /// When false, only announce packets go out — no beat, status, or master
    /// handoff.  A pure receiver: can follow the XDJ but never asks it to
    /// follow us, which is the conservative, can't-wedge-the-deck mode.
    /// Whether we send beats/status/master — a live flag so pressing MASTER
    /// can enable sending (you can't lead without sending). Off = pure follower.
    pub send_full:   Arc<AtomicBool>,
    pub position:    Arc<AtomicU64>,   // decoder cursor, interleaved samples
    pub in_flight:   Arc<AtomicU64>,   // samples decoded but not yet audible
    pub playing:     Arc<AtomicBool>,
    pub fader_speed: Arc<AtomicU32>,   // f32 bits; SYNC writes this
    pub speed:       Arc<AtomicU32>,   // f32 bits; phase nudges write this
    pub sample_rate: u32,
    pub channels:    u8,
    /// Live beat grid — updated by the deck on every track load so the
    /// sender's sync/broadcast use the CURRENT track, not the startup one.
    pub grid:        Arc<ArcSwap<Option<BeatGrid>>>,
    /// The loaded track (empty = none) and its expected length while it
    /// streams in: the Absolute Position packet's track length.
    pub samples:     Arc<ArcSwap<Vec<f32>>>,
    pub total_hint:  Arc<AtomicU64>,
    /// The deck's loop, [start, end) in interleaved samples, for the
    /// CDJ-3000 status.
    pub loop_active: Arc<AtomicBool>,
    pub loop_start:  Arc<AtomicU64>,
    pub loop_end:    Arc<AtomicU64>,
}

// ── The other deck's beat phase ───────────────────────────────────────────────

/// The phase-meter row for the deck we follow, as a phase-locked free-run.
///
/// Beat packets over Wi-Fi arrive late and in bursts (an access point holds
/// broadcasts for a power-saving client until its beacon), so the row runs
/// at the peer's tempo on our own clock and each packet only asks for a
/// correction of a quarter of the error toward the beat it marks.  That
/// correction is not applied as a jump: it is bled in as a rate change of at
/// most `SLEW` of the free-run speed, so the row never runs backwards and
/// never jumps — jitter shows as a brief, invisible change of pace.  It holds
/// only when the peer's status says it is paused and no beat has come for two
/// periods.  (A "hold after one beat" rule made the row stop and restart on
/// every late packet.)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeerPhase {
    /// Free-running phase in beats; negative = not started.
    phase:     f64,
    /// Beat-in-bar of free-run beat 0 (0–3), from the packets' beat numbers:
    /// the bar count runs with the free-run instead of jumping when a packet
    /// lands a few ms either side of the beat the row has drawn.
    bar_offset: Option<i64>,
    /// Arrival time (ms) of the last packet folded in.
    folded_ms: u64,
    /// Correction still to be bled in, beats (signed).
    pending:   f64,
}

impl Default for PeerPhase {
    fn default() -> Self { Self::new() }
}

impl PeerPhase {
    /// Fraction of a packet's phase error requested as correction.
    pub const PULL: f64 = 0.25;
    /// Largest change of pace while a correction bleeds in (0.25 = between
    /// 75 % and 125 % of the free-run speed).
    pub const SLEW: f64 = 0.25;

    pub const fn new() -> Self {
        Self { phase: -1.0, bar_offset: None, folded_ms: 0, pending: 0.0 }
    }

    /// One frame: `dt_s` since the last call; `beat_ms` is when the latest
    /// beat packet arrived (same clock as `now_ms`, 0 = none yet) and `bib`
    /// its beat within the bar (1–4, 0 = unknown).  Returns the phase to
    /// draw, 0..1, and the beat in the bar it belongs to, both from the
    /// free-run, so they turn over together.
    pub fn advance(&mut self, now_ms: u64, beat_ms: u64, bpm: f32, bib: u8, peer_playing: bool, dt_s: f64) -> (f32, u8) {
        if bpm <= 0.0 || beat_ms == 0 {
            *self = Self::new();
            return (0.0, bib);
        }
        let set_bar = |s: &mut Self, at_arrival: f64| {
            if (1..=4).contains(&bib) {
                s.bar_offset = Some((bib as i64 - 1 - at_arrival.round() as i64).rem_euclid(4));
            }
        };
        let period_s = 60.0 / bpm as f64;
        let age_s    = now_ms.saturating_sub(beat_ms) as f64 / 1000.0;
        let running  = peer_playing || age_s < 2.0 * period_s;
        if self.phase < 0.0 {
            self.phase = age_s / period_s;          // first packet: the beat was `age` ago
            self.folded_ms = beat_ms;
            self.pending = 0.0;
            set_bar(self, 0.0);
        } else if running {
            let inc = dt_s / period_s;
            let adj = self.pending.clamp(-inc * Self::SLEW, inc * Self::SLEW);
            self.phase += inc + adj;
            self.pending -= adj;
        }
        if beat_ms != self.folded_ms {
            // A new packet marked a beat at (now − age): ask for a pull toward it.
            self.folded_ms = beat_ms;
            let at_arrival = self.phase - age_s / period_s;
            let err = at_arrival.rem_euclid(1.0);
            let err = if err > 0.5 { err - 1.0 } else { err };   // beats, ±0.5
            self.pending -= err * Self::PULL;
            set_bar(self, at_arrival);
        }
        let (k, f) = crate::snapshot::split_beat(self.phase);
        let shown = match self.bar_offset { Some(o) => ((k + o).rem_euclid(4) + 1) as u8, None => bib };
        (f, shown)
    }
}

/// An OpenDeck sends each beat by broadcast AND unicast (the unicast copy is
/// not held back by Wi-Fi power save); the second copy lands within a few
/// ms.  A real beat is never closer than 300 ms (200 BPM), so a beat from the
/// same player inside `TWIN_MS` is the twin.
pub const TWIN_MS: u64 = 100;
pub fn beat_is_twin(last: &mut HashMap<u8, Instant>, player: u8, now: Instant) -> bool {
    if let Some(t) = last.get(&player) {
        if now.duration_since(*t) < Duration::from_millis(TWIN_MS) { return true; }
    }
    last.insert(player, now);
    false
}

// ── Sockets ───────────────────────────────────────────────────────────────────

fn bind_shared(port: u16) -> Option<UdpSocket> {
    // SO_REUSEADDR + SO_REUSEPORT so the port can be shared with other ProDJ
    // Link tools (prolink_virtual_cdj, dysentery, a second freedj instance).
    let raw = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|e| log::warn!("ProDJ Link: socket create failed: {e}"))
        .ok()?;
    raw.set_reuse_address(true).ok();
    #[cfg(unix)]
    raw.set_reuse_port(true).ok();
    raw.set_broadcast(true).ok();
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
    raw.bind(&addr.into())
        .map_err(|e| log::warn!("ProDJ Link: cannot bind port {port}: {e}"))
        .ok()?;
    let sock: UdpSocket = raw.into();
    sock.set_read_timeout(Some(Duration::from_millis(500))).ok();
    Some(sock)
}

/// An IPv4 address is link-local (APIPA) when it is in 169.254.0.0/16.  That
/// means DHCP never answered — which is exactly what an idle USB-C ethernet
/// dongle looks like while the real network is on another interface.  Broadcasts
/// there fail with "no route to host".
fn is_link_local(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 169 && o[1] == 254
}

fn same_subnet(a: Ipv4Addr, b: Ipv4Addr, mask: Ipv4Addr) -> bool {
    let (a, b, m) = (u32::from(a), u32::from(b), u32::from(mask));
    a & m == b & m
}

/// Pick the interface to speak Link on, best first, else loopback (which still
/// reaches a second instance on this machine).  Returns (ip, broadcast, mac,
/// name).
///
/// `peers` are players we have already heard from.  Discovery happens on the
/// listener threads *after* the sender starts, so the first choice is made
/// blind; once a player is known, only an interface on that player's subnet can
/// reach it by broadcast, so such an interface wins outright.  Failing that,
/// prefer a properly configured interface over a link-local one.
///
/// Taking simply the first candidate — as this did — put an iPad's beat
/// broadcasts onto a link-local USB-C dongle that enumerated ahead of Wi-Fi,
/// while the XDJ sat on the Wi-Fi subnet.  Status packets are unicast to each
/// discovered peer, so master handoff and tempo kept working and only the
/// broadcast beat clock was lost.
/// `OPENDECK_LINK_IFACE=<name>` pins Link to one interface: only it is used
/// to send, and packets from outside its subnet are ignored. It is for test
/// rigs on a private segment (an emulated CDJ on a hand-made bridge) where
/// freedj may take master without reaching the real decks on the LAN. An
/// interface that does not exist pins Link to loopback rather than falling
/// back to the LAN. Returns (name, ip, netmask).
/// The address the media server (dbserver) listens on: the pinned Link
/// interface's (`OPENDECK_LINK_IFACE`), else every address.
pub fn media_bind_ip() -> Ipv4Addr { pinned_iface().map(|p| p.1).unwrap_or(Ipv4Addr::UNSPECIFIED) }

fn pinned_iface() -> Option<&'static (String, Ipv4Addr, Ipv4Addr)> {
    static PIN: std::sync::OnceLock<Option<(String, Ipv4Addr, Ipv4Addr)>> = std::sync::OnceLock::new();
    PIN.get_or_init(|| {
        let name = std::env::var("OPENDECK_LINK_IFACE").ok().filter(|n| !n.is_empty())?;
        let found = if_addrs::get_if_addrs().ok().and_then(|ifs| ifs.into_iter().find_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) if i.name == name => Some((v4.ip, v4.netmask)),
            _ => None,
        }));
        match found {
            Some((ip, mask)) => { log::info!("ProDJ Link pinned to {name} ({ip})"); Some((name, ip, mask)) }
            None => {
                log::error!("ProDJ Link: OPENDECK_LINK_IFACE={name} has no IPv4 address; using loopback only");
                Some((name, Ipv4Addr::LOCALHOST, Ipv4Addr::new(255, 0, 0, 0)))
            }
        }
    }).as_ref()
}

/// Whether a Link packet from `src` should be heard (always, unless pinned).
fn heard_from(src: SocketAddr) -> bool {
    heard_on(pinned_iface().map(|(_, ip, mask)| (*ip, *mask)), src)
}

/// [`heard_from`] with the pin given: (our ip, netmask) or none.
fn heard_on(pin: Option<(Ipv4Addr, Ipv4Addr)>, src: SocketAddr) -> bool {
    match (pin, src.ip()) {
        (None, _) => true,
        (Some((ip, mask)), std::net::IpAddr::V4(from)) => same_subnet(ip, from, mask) || from.is_loopback(),
        (Some(_), _) => false,
    }
}

/// The broadcast address to send Link on. An address added without `brd`
/// reports none (or its own address); derive it from the netmask instead of
/// broadcasting to ourselves. None for a host route (/32) or no mask.
fn broadcast_for(ip: Ipv4Addr, mask: Ipv4Addr, reported: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
    match reported {
        Some(b) if b != ip && !b.is_unspecified() => Some(b),
        _ if mask.is_unspecified() || mask.is_broadcast() => None,
        _ => Some(Ipv4Addr::from(u32::from(ip) | !u32::from(mask))),
    }
}

/// Link on the loopback (`OPENDECK_LINK_IFACE=lo`): from 127.0.0.1, with
/// broadcasts to 127.255.255.255, which Linux delivers to every socket on
/// the port, so other programs on this machine hear them (a second
/// OpenDeck, Deckbench's decks on 127.0.0.N).  Sending them to 127.0.0.1
/// reached only one socket bound to the port.
fn loopback_link() -> (Ipv4Addr, Ipv4Addr, [u8; 6], String) {
    (Ipv4Addr::LOCALHOST, Ipv4Addr::new(127, 255, 255, 255), [0x02, 0xfd, 0, 0, 0, 1], "lo".into())
}

fn link_interface(peers: &[Ipv4Addr]) -> (Ipv4Addr, Ipv4Addr, [u8; 6], String) {
    let pin = pinned_iface();
    if let Some((_, ip, _)) = pin {
        if ip.is_loopback() { return loopback_link(); }
    }
    let mut best: Option<(u8, Ipv4Addr, Ipv4Addr, [u8; 6], String)> = None;
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if i.is_loopback() { continue; }
            let if_addrs::IfAddr::V4(v4) = &i.addr else { continue };
            // An address added without `brd` reports no broadcast (or its own
            // address); derive it from the netmask rather than broadcasting to
            // ourselves.
            let Some(bc) = broadcast_for(v4.ip, v4.netmask, v4.broadcast) else { continue };
            if let Some((name, ..)) = pin {
                if i.name != *name { continue; }
            } else if i.name.starts_with("docker") || i.name.starts_with("br-") || i.name.starts_with("lxc") || i.name.starts_with("virbr") {
                continue;
            }
            let reaches_peer = peers.iter().any(|p| same_subnet(v4.ip, *p, v4.netmask));
            let score = match (reaches_peer, is_link_local(v4.ip)) {
                (true,  _)     => 3,   // on a known player's subnet
                (false, false) => 2,   // configured, but no player seen there yet
                (false, true)  => 1,   // link-local: DHCP never answered
            };
            // Strictly greater keeps the original first-wins order on a tie.
            if best.as_ref().is_some_and(|(b, ..)| *b >= score) { continue; }
            let mac = std::fs::read_to_string(format!("/sys/class/net/{}/address", i.name))
                .ok()
                .and_then(|m| {
                    let b: Vec<u8> = m.trim().split(':').filter_map(|h| u8::from_str_radix(h, 16).ok()).collect();
                    b.try_into().ok()
                })
                .unwrap_or([0x02, 0xfd, 0, 0, 0, 1]);
            best = Some((score, v4.ip, bc, mac, i.name.clone()));
        }
    }
    match best {
        Some((_, ip, bc, mac, name)) => (ip, bc, mac, name),
        None => (Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, [0x02, 0xfd, 0, 0, 0, 1], "lo".into()),
    }
}

// ── Listeners ─────────────────────────────────────────────────────────────────

pub struct ProDjHandle {
    _threads: Vec<thread::JoinHandle<()>>,
}

impl ProDjHandle {
    /// Spawn the three listeners.  A port that cannot be bound is skipped
    /// with a warning rather than failing the app.
    pub fn listen(
        link:         Arc<LinkState>,
        beat2_bpm:    Arc<AtomicU32>,
        beat2_anchor: Arc<AtomicU64>,
        beat2_player: Arc<AtomicU32>,
        beat2_bib:    Arc<AtomicU32>,
    ) -> Option<Self> {
        let mut threads = Vec::new();
        if let Some(t) = listen_announce(Arc::clone(&link)) { threads.push(t); }
        if let Some(t) = listen_beat(Arc::clone(&link), beat2_bpm, beat2_anchor, Arc::clone(&beat2_player), beat2_bib) { threads.push(t); }
        if let Some(t) = listen_status(Arc::clone(&link), beat2_player) { threads.push(t); }
        if threads.is_empty() { None } else { Some(ProDjHandle { _threads: threads }) }
    }
}

fn spawn(name: &str, port: u16, sock: UdpSocket, mut f: impl FnMut(&[u8], SocketAddr) + Send + 'static) -> Option<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut sock = sock;
            let mut buf = [0u8; 1500];
            let mut failures = 0u32;
            loop {
                match sock.recv_from(&mut buf) {
                    Ok((n, addr)) => { failures = 0; if heard_from(addr) { f(&buf[..n], addr) } }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                    // Benign on Linux UDP: a prior unicast to a peer that has
                    // gone draws an ICMP port-unreachable, surfaced on the next
                    // recv as ConnectionRefused/Reset.  The listener keeps
                    // running, so this never affects playback or sync.
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset) =>
                        log::debug!("ProDJ Link recv (transient, ignored): {e}"),
                    Err(e) => {
                        // Any other error means the socket itself is dead — iOS
                        // reclaims sockets from an app that sat suspended — and
                        // recv then fails at once, every call.  This loop used
                        // to spin on that (a core each for three listeners: the
                        // "iPad runs hot and Link is gone until a restart"
                        // report).  Back off, then bind afresh.
                        failures += 1;
                        if failures == 1 { log::warn!("ProDJ Link recv on {port}: {e} — rebinding"); }
                        thread::sleep(Duration::from_millis(250));
                        if let Some(s) = bind_shared(port) {
                            sock = s;
                            failures = 0;
                            log::info!("ProDJ Link: port {port} rebound");
                        }
                    }
                }
            }
        })
        .ok()
}

/// 50000: announces → peer table.
/// Diagnostic file lines for the PERF LOG file (iOS: Documents/opendeck-perf.log):
/// the first packet of each (port, type, length) per player goes in as hex so a
/// player generation we have not seen can be replayed through the parser
/// offline (`cargo run -p opendeck-link --example replay_capture`), after that
/// only the summary lines.  Nothing happens while the setting is off.
fn diag_packet(port: u16, addr: SocketAddr, data: &[u8]) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<(u16, u8, u8, usize)>> = Mutex::new(Vec::new());
    if !crate::cpumeter::FILE_LOG.load(Ordering::Relaxed) { return; }
    let Some(t) = ProDjLink::packet_type(data) else { return };
    let player = if t == opendeck_link::prodj::PKT_ANNOUNCE { data.get(0x24) } else { data.get(0x21) }.copied().unwrap_or(0);
    let key = (port, t, player, data.len());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.contains(&key) { return; }
    seen.push(key);
    let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    crate::cpumeter::file_line(&format!("link: {now} {port} {addr} {} {hex}  -- first type {t:#04x} from player {player}", data.len()));
}

/// What the 50000 listener should do with a packet, after [`ClaimGuard`].
#[derive(Debug, PartialEq)]
pub enum ClaimAction {
    /// Our own packet, or claim traffic fully handled: stop here.
    Done,
    /// Defend `number` (ours, at `ours`) against the claimant at `to`.
    Defend { number: u8, to: Ipv4Addr, ours: Ipv4Addr },
    /// Not ours and not claim traffic: carry on (an announce to record).
    Continue,
}

/// The device-number rules on port 50000, kept apart from the socket so
/// they can be tested: note a defense of the number we are claiming, defend
/// the number we hold, and react to a device announcing it.
#[derive(Default)]
pub struct ClaimGuard {
    /// Local port of the socket our defenses go out from (our own echo).
    pub defend_port: Option<u16>,
    /// Who we have defended our number against: if one announces with it
    /// anyway, it is the one in the wrong, and we keep the number.
    defended_against: Vec<(Ipv4Addr, u16)>,
    /// Collisions already acted on, once per device.
    warned: Vec<(Ipv4Addr, u16)>,
    /// This host's addresses: a machine on wired and Wi-Fi in one subnet
    /// sends its broadcasts out of either, so our own packets arrive from
    /// any of them (our sender port tells them apart from another program).
    pub local_ips: Vec<Ipv4Addr>,
}

impl ClaimGuard {
    pub fn on_packet(&mut self, link: &LinkState, data: &[u8], from: Option<(Ipv4Addr, u16)>) -> ClaimAction {
        let own_src = link.own_src.lock().ok().and_then(|g| *g);
        // Our own broadcasts come back to us; a peer is anyone else.
        let ours_ip = |ip: Ipv4Addr| Some(ip) == own_src.map(|o| o.0) || self.local_ips.contains(&ip);
        if from.is_some_and(|f| ours_ip(f.0) && (Some(f.1) == own_src.map(|o| o.1) || Some(f.1) == self.defend_port)) {
            return ClaimAction::Done;
        }
        let ours = link.player();
        let holding = link.claimed.load(Ordering::Relaxed);
        if let Some(n) = ProDjLink::parse_defense(data) {
            if !holding && n == ours {
                log::debug!("ProDJ Link: {from:?} defends player {n}");
                link.defended.store(n, Ordering::Relaxed);
            } else if holding && n == ours {
                if link.auto {
                    log::warn!("ProDJ Link: {from:?} says player {n} is its own; claiming a new number");
                    link.reclaim.store(true, Ordering::Relaxed);
                } else {
                    log::warn!("ProDJ Link: {from:?} says player {n} is its own; PLAYER No. is set to {n}, so keeping it");
                }
            }
            return ClaimAction::Done;
        }
        if let Some(n) = ProDjLink::parse_claim(data) {
            if let (true, true, Some(own), Some(f)) = (holding, n == ours, own_src, from) {
                if !self.defended_against.contains(&f) {
                    self.defended_against.push(f);
                    log::info!("ProDJ Link: {f:?} claims player {n}, ours; defending");
                }
                return ClaimAction::Defend { number: n, to: f.0, ours: own.0 };
            }
            return ClaimAction::Done;
        }
        if let Some((player, pkt_ip)) = ProDjLink::parse_announce(data) {
            let they = from.unwrap_or((pkt_ip, 0));
            if player == ours && holding && !self.warned.contains(&they) {
                self.warned.push(they);
                // A device that never claimed (or ignored our defense) is
                // using our number.  AUTO moves; a set number stays put.  Two
                // AUTO OpenDecks that finished claiming the same number at
                // the same moment both see this; only the higher address moves.
                if self.defended_against.contains(&they) {
                    log::warn!("ProDJ Link: {they:?} is announcing as player {player} although we hold it and told it so");
                } else if link.auto && own_src.is_some_and(|me| me > they) {
                    log::warn!("ProDJ Link: {they:?} is also announcing as player {player}; claiming a new number");
                    link.reclaim.store(true, Ordering::Relaxed);
                } else if link.auto {
                    log::info!("ProDJ Link: {they:?} is also announcing as player {player}; keeping it (it should move)");
                } else {
                    log::warn!("ProDJ Link: {they:?} is also announcing as player {player} (PLAYER No. is set to {player})");
                }
            }
        }
        ClaimAction::Continue
    }
}

/// A packet a listener sends back, and where to.
pub(crate) type Reply = (Vec<u8>, SocketAddr);

/// Send a listener's replies from `sock` (none when it could not be bound).
fn send_replies(sock: Option<&UdpSocket>, replies: Vec<Reply>) {
    if let Some(s) = sock {
        for (pkt, to) in replies { let _ = s.send_to(&pkt, to); }
    }
}

/// The 50000 listener's work, without the socket: claim defense, then the
/// peer table.
pub(crate) struct AnnounceRx {
    pub link:  Arc<LinkState>,
    pub guard: ClaimGuard,
}

impl AnnounceRx {
    pub(crate) fn on_packet(&mut self, data: &[u8], addr: SocketAddr) -> Vec<Reply> {
        let link = &self.link;
        let from = match addr { SocketAddr::V4(v) => Some((*v.ip(), v.port())), _ => None };
        match self.guard.on_packet(link, data, from) {
            ClaimAction::Done => return Vec::new(),
            ClaimAction::Defend { number, to, ours } => {
                // Unicast, as the players do, and broadcast too: on one
                // machine several programs share port 50000 and a unicast
                // reaches only one of them.  Others ignore a defense of a
                // number they are not claiming.
                let pkt = ProDjLink::new(number).build_defense(number, ours);
                let mut out = vec![(pkt.clone(), SocketAddr::from((to, PORT_ANNOUNCE)))];
                if let Some(b) = link.own_bcast.lock().ok().and_then(|g| *g) {
                    out.push((pkt, SocketAddr::from((b, PORT_ANNOUNCE))));
                }
                return out;
            }
            ClaimAction::Continue => {}
        }
        if let Some((player, pkt_ip)) = ProDjLink::parse_announce(data) {
            // Reach the peer where its packet came FROM: identical to the IP in
            // the packet on a LAN, and the only address that works when the
            // announce was unicast across a routed link (Tailscale).
            let ip = match addr { SocketAddr::V4(v) => *v.ip(), _ => pkt_ip };
            if let Ok(mut peers) = link.peers.lock() {
                if peers.insert(player, ip) != Some(ip) {
                    let name = String::from_utf8_lossy(&data[0x0c..0x20]).trim_end_matches('\0').to_string();
                    log::info!("ProDJ Link: player {player} \"{name}\" at {ip}");
                    if let Ok(mut names) = link.peer_names.lock() { names.insert(player, name); }
                }
            }
        }
        Vec::new()
    }
}

fn listen_announce(link: Arc<LinkState>) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(PORT_ANNOUNCE)?;
    log::info!("ProDJ Link: listening for announces on port {PORT_ANNOUNCE}");
    // Defenses go back to the claimant's port 50000 from a socket of our own.
    let defend_sock = UdpSocket::bind("0.0.0.0:0").ok();
    if let Some(d) = &defend_sock { let _ = d.set_broadcast(true); }
    let local_ips = if_addrs::get_if_addrs().map(|v| v.into_iter().filter_map(|i| match i.addr { if_addrs::IfAddr::V4(a) => Some(a.ip), _ => None }).collect()).unwrap_or_default();
    let guard = ClaimGuard { defend_port: defend_sock.as_ref().and_then(|d| d.local_addr().ok()).map(|a| a.port()), local_ips, ..Default::default() };
    let mut rx = AnnounceRx { link, guard };
    spawn("prodj-rx-50000", PORT_ANNOUNCE, sock, move |data, addr| {
        log::trace!("ProDJ rx :50000 {} bytes from {addr} — {:02X?}", data.len(), data);
        diag_packet(50000, addr, data);
        send_replies(defend_sock.as_ref(), rx.on_packet(data, addr));
    })
}

/// The deck's loop as a CDJ-3000 status reports it, in ms, with its
/// length in beats when that is a whole number on the grid.
fn active_loop(st: &SenderState, grid: Option<&BeatGrid>) -> Option<ActiveLoop> {
    if !st.loop_active.load(Ordering::Relaxed) { return None; }
    let (a, b) = (st.loop_start.load(Ordering::Relaxed), st.loop_end.load(Ordering::Relaxed));
    loop_fields(a, b, st.channels as u32, st.sample_rate, grid.map(|g| g.bpm))
}

fn loop_fields(start: u64, end: u64, ch: u32, sr: u32, bpm: Option<f64>) -> Option<ActiveLoop> {
    if end <= start { return None; }
    let ms = |s: u64| s as f64 / ch.max(1) as f64 / sr.max(1) as f64 * 1000.0;
    let (start_ms, end_ms) = (ms(start), ms(end));
    let beats = bpm.filter(|b| *b > 0.0).map(|b| (end_ms - start_ms) / (60_000.0 / b)).map_or(0, |n| {
        if (n - n.round()).abs() < 0.02 && n.round() >= 1.0 { n.round() as u16 } else { 0 }
    });
    Some(ActiveLoop { start_ms, end_ms, beats })
}

/// Keep a peer's Absolute Position packet; true if `data` was one.
fn note_position(link: &LinkState, data: &[u8], now: Instant) -> bool {
    let Some(p) = ProDjLink::parse_abs_position(data) else { return false };
    if p.player != link.player() {
        if let Ok(mut m) = link.positions.lock() { m.insert(p.player, (p, now)); }
    }
    true
}

impl LinkState {
    /// A player's playhead in ms now, from its latest position packet run
    /// forward at its pitch while it plays; None without a recent one (1 s).
    /// `playing` comes from its status.
    #[cfg(test)]   // superseded in the app by RemoteWave::smooth_playhead
    pub fn playhead_ms(&self, player: u8, playing: bool, now: Instant) -> Option<f64> {
        let (p, at) = self.positions.lock().ok()?.get(&player).copied()?;
        let age = now.checked_duration_since(at)?;
        if age > Duration::from_secs(1) { return None; }
        Some(p.playhead_ms as f64 + if playing { age.as_secs_f64() * 1000.0 * p.pitch as f64 } else { 0.0 })
    }
}

/// Where a CDJ-3000 sends its Absolute Position packets: every device that
/// has announced itself, except ourselves, each address once.
fn position_targets(peers: &HashMap<u8, Ipv4Addr>, me: u8) -> Vec<Ipv4Addr> {
    let mut v: Vec<Ipv4Addr> = peers.iter().filter(|(p, _)| **p != me).map(|(_, a)| *a).collect();
    v.sort();
    v.dedup();
    v
}

/// 50001: beats from peers (→ B2 strip, → sync phase reference), and the
/// handoff / sync-control packets.
fn listen_beat(
    link:         Arc<LinkState>,
    beat2_bpm:    Arc<AtomicU32>,
    beat2_anchor: Arc<AtomicU64>,
    beat2_player: Arc<AtomicU32>,
    beat2_bib:    Arc<AtomicU32>,
) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(PORT_BEAT)?;
    log::info!("ProDJ Link: listening for beats on port {PORT_BEAT}");
    let tx = UdpSocket::bind("0.0.0.0:0").ok()?;
    let mut rx = BeatRx { link, beat2_bpm, beat2_anchor, beat2_player, beat2_bib, last_beat_at: HashMap::new() };
    spawn("prodj-rx-50001", PORT_BEAT, sock, move |data, addr| {
        log::debug!("ProDJ rx :50001 {} bytes from {addr} — {:02X?}", data.len(), data);
        diag_packet(50001, addr, data);
        send_replies(Some(&tx), rx.on_packet(data, addr, Instant::now()));
    })
}

/// The 50001 listener's work, without the socket.
pub(crate) struct BeatRx {
    pub link:         Arc<LinkState>,
    pub beat2_bpm:    Arc<AtomicU32>,
    pub beat2_anchor: Arc<AtomicU64>,
    pub beat2_player: Arc<AtomicU32>,
    pub beat2_bib:    Arc<AtomicU32>,
    pub last_beat_at: HashMap<u8, Instant>,
}

impl BeatRx {
    pub(crate) fn on_packet(&mut self, data: &[u8], addr: SocketAddr, now: Instant) -> Vec<Reply> {
        let BeatRx { link, beat2_bpm, beat2_anchor, beat2_player, beat2_bib, last_beat_at } = self;
        let mut out = Vec::new();
        if note_position(link, data, now) { return out; }
        if let Some(b) = ProDjLink::parse_beat(data) {
            if b.player == link.player() { return out; }           // our own broadcast
            // An OpenDeck sends each beat by broadcast AND unicast (see the
            // sender); the second copy lands within a few ms.  A real beat is
            // never closer than 300 ms (200 BPM), so drop the twin.
            if beat_is_twin(last_beat_at, b.player, now) { return out; }
            if let Ok(mut m) = link.peer_heard.lock() {
                let h = m.entry(b.player).or_default();
                h.beat_at = Some(now);
                h.beat_bib = Some(b.beat_in_bar);
            }
            let master = link.master_player.load(Ordering::Relaxed) as u8;
            // Effective tempo = track BPM × the sender's pitch.  The beat packet
            // carries them separately; use the product so a pitched master
            // reports its real tempo (the status path already does this — the two
            // must agree or master_bpm hunts between them).
            let eff = b.bpm * b.pitch;
            // The B2 strip follows the master if there is one, else whoever plays.
            if master == 0 || master == b.player {
                let old = f32::from_bits(beat2_bpm.load(Ordering::Relaxed));
                beat2_bpm.store(eff.to_bits(), Ordering::Relaxed);
                beat2_player.store(b.player as u32, Ordering::Relaxed);
                beat2_bib.store(b.beat_in_bar as u32, Ordering::Relaxed);
                beat2_anchor.fetch_add(1, Ordering::Relaxed);
                let now = link.now_ms();
                link.beat2_beat_ms.store(now, Ordering::Relaxed);
                link.beat2_seen_ms.store(now, Ordering::Relaxed);
                if (old - eff).abs() > 0.005 {
                    log::info!("ProDJ beat: player {} @ {:.2} BPM (was {old:.2})", b.player, eff);
                } else {
                    log::debug!("ProDJ beat: player {} @ {:.2} BPM beat {}/4", b.player, eff, b.beat_in_bar);
                }
            }
            if master == b.player {
                link.master_bpm.store(eff.to_bits(), Ordering::Relaxed);
                link.master_beat_seq.fetch_add(1, Ordering::Relaxed);
            }
            return out;
        }

        if let Some(requester) = ProDjLink::parse_master_request(data) {
            if link.master.load(Ordering::Relaxed) {
                log::info!("ProDJ Link: player {requester} asks for master — yielding");
                link.handoff_to.store(requester as u32, Ordering::Relaxed);
                out.push((ProDjLink::new(link.player()).build_master_response(true), SocketAddr::new(addr.ip(), PORT_BEAT)));
            }
            return out;
        }
        if let Some((from, yielded)) = ProDjLink::parse_master_response(data) {
            log::info!("ProDJ Link: player {from} {} master", if yielded { "yields" } else { "refuses" });
            // Beat Link parity (`yieldResponse`): the 0x27 response means only
            // that the master AGREED to yield.  We must NOT assert master here —
            // we record the agreement and wait until we see our own number in
            // the master's Mh (handoff) byte in its status, then take it (see
            // `listen_status`).  Claiming master on the 0x27 jumps the handshake
            // out of order and the XDJ aborts it (reverts Mh, keeps MASTER).
            if yielded && link.want_master.load(Ordering::Relaxed) {
                link.yielded_from.store(from as u32, Ordering::Relaxed);
                log::info!("ProDJ Link: player {from} agreed to yield; waiting for it to hand off (Mh)");
            }
            return out;
        }
        if let Some((from, target, cmd)) = ProDjLink::parse_sync_control(data) {
            if target != link.player() { return out; }
            match cmd {
                SYNC_ON  => { link.sync.store(true,  Ordering::Relaxed); log::info!("ProDJ Link: player {from} turned our SYNC on"); }
                SYNC_OFF => { link.sync.store(false, Ordering::Relaxed); log::info!("ProDJ Link: player {from} turned our SYNC off"); }
                BECOME_MASTER => { link.want_master.store(true, Ordering::Relaxed); log::info!("ProDJ Link: player {from} asks us to become master"); }
                _ => log::info!("ProDJ Link: sync control {cmd:#x} from player {from}"),
            }
        }
        out
    }
}

/// 50002: status from peers → who is master, handoff progress; one log line
/// Our answer to a media query (port 50002) about our USB, and where to send
/// it: built with the number we hold now (AUTO may have moved since start).
pub fn answer_media_query(link: &LinkState, data: &[u8]) -> Option<(Vec<u8>, Ipv4Addr)> {
    let (dev, rip, target, slot) = ProDjLink::parse_media_query(data)?;
    let n = link.serve_tracks.load(Ordering::Relaxed);
    log::info!("ProDJ Link: media query from device {dev} ({rip}) for player {target} slot {slot}; serving {n} tracks");
    (target == link.player() && n > 0 && matches!(slot, 0 | 3)).then(|| {
        let resp = ProDjLink::new(link.player()).build_media_response(rip, 3, "OPENDECK", n.min(u16::MAX as u32) as u16, 0, 32 << 30, 16 << 30);
        (resp, rip)
    })
}

/// per change per player.
fn listen_status(link: Arc<LinkState>, beat2_player: Arc<AtomicU32>) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(PORT_STATUS)?;
    log::info!("ProDJ Link: listening for status on port {PORT_STATUS}");
    let reply_sock = UdpSocket::bind("0.0.0.0:0").ok();
    let mut rx = StatusRx { link, beat2_player, last: HashMap::new() };
    spawn("prodj-rx-50002", PORT_STATUS, sock, move |data, addr| {
        log::trace!("ProDJ rx :50002 {} bytes from {addr} — {:02X?}", data.len(), data);
        diag_packet(50002, addr, data);
        send_replies(reply_sock.as_ref(), rx.on_packet(data, addr));
    })
}

/// The 50002 listener's work, without the socket.
pub(crate) struct StatusRx {
    pub link:         Arc<LinkState>,
    pub beat2_player: Arc<AtomicU32>,
    pub last:         HashMap<u8, Status>,
}

impl StatusRx {
    pub(crate) fn on_packet(&mut self, data: &[u8], addr: SocketAddr) -> Vec<Reply> {
        let StatusRx { link, beat2_player, last } = self;
        let mut out = Vec::new();
        // Media query: a peer asks what is in one of our slots.  Answer when we
        // are serving (the media response names the library and its size).
        if ProDjLink::packet_type(data) == Some(PKT_MEDIA_QUERY) {
            if let Some((resp, rip)) = answer_media_query(link, data) {
                // To the address inside the packet and, across a routed link
                // where that is the peer's LAN address, to where the query
                // came from — on the status port either way.
                out.push((resp.clone(), SocketAddr::from((rip, PORT_STATUS))));
                if let SocketAddr::V4(f) = addr { if *f.ip() != rip { out.push((resp, SocketAddr::from((*f.ip(), PORT_STATUS)))); } }
            }
            return out;
        }
        // Media response: a player answering our query about one of its slots.
        if let Some(info) = ProDjLink::parse_media_response(data) {
            if let Ok(mut m) = link.peer_media.lock() {
                let key = (info.player, info.slot);
                if m.get(&key).map_or(true, |(old, _)| *old != info) {
                    log::info!("ProDJ Link: player {} {}: {:?}, {} tracks, {} playlists, colour {}", info.player, info.slot_name(), info.name, info.tracks, info.playlists, info.color);
                }
                m.insert(key, (info, Instant::now()));
            }
            return out;
        }
        let Some(st) = ProDjLink::parse_status(data) else { return out };
        if let Ok(mut m) = link.peer_status.lock() { m.insert(st.player, st); }
        if let Ok(mut m) = link.peer_heard.lock() { m.entry(st.player).or_default().status_at = Some(Instant::now()); }
        if st.player == link.player() { return out; }

        let changed = last.get(&st.player).map_or(true, |p| {
            p.play != st.play || p.master != st.master || p.sync != st.sync
                || p.bpm != st.bpm || p.track_loaded != st.track_loaded
                || p.handoff_to != st.handoff_to || (p.pitch - st.pitch).abs() > 0.0005
        });
        if changed {
            crate::cpumeter::file_line(&format!(
                "link: status player {} fw {} len {:#x} {:?} play={} master={} sync={} onair={} pitch {:+.2}% bpm {:?} handoff {:?} syncN {}",
                st.player, String::from_utf8_lossy(&st.firmware).trim(), data.len(), st.play,
                st.playing, st.master, st.sync, st.on_air, (st.pitch - 1.0) * 100.0, st.bpm, st.handoff_to, st.sync_counter));
            log::info!(
                "ProDJ status: player {} fw {} len {:#x} {:?} {}{}{}{}pitch {:+.2}% bpm {} beat {:?}/{:?} handoff {:?}",
                st.player, String::from_utf8_lossy(&st.firmware).trim(), data.len(), st.play,
                if st.playing { "PLAY " } else { "" }, if st.master { "MASTER " } else { "" },
                if st.sync { "SYNC " } else { "" }, if st.on_air { "ONAIR " } else { "" },
                (st.pitch - 1.0) * 100.0,
                st.bpm.map(|b| format!("{b:.2}")).unwrap_or_else(|| "-".into()),
                st.beat, st.beat_in_bar, st.handoff_to,
            );
            last.insert(st.player, st);
        }

        link.largest_sync.fetch_max(st.sync_counter, Ordering::Relaxed);

        // Linked as soon as any peer sends us status.  Status keeps coming
        // every 200 ms from a paused deck, so it is what proves the tracked
        // peer is still there once its beats stop.
        if beat2_player.load(Ordering::Relaxed) == 0 {
            beat2_player.store(st.player as u32, Ordering::Relaxed);
        }
        if beat2_player.load(Ordering::Relaxed) == st.player as u32 {
            link.beat2_seen_ms.store(link.now_ms(), Ordering::Relaxed);
            link.beat2_playing.store(st.playing, Ordering::Relaxed);
        }

        // Master bookkeeping.  Its effective tempo comes from status as well
        // as beats: a master that is paused or has ended sends no beats.
        if st.master {
            if let Some(bpm) = st.bpm {
                link.master_bpm.store((bpm * st.pitch).to_bits(), Ordering::Relaxed);
            }
        }
        let cur = link.master_player.load(Ordering::Relaxed) as u8;
        let handing_to_us = st.handoff_to == Some(link.player());
        if st.master || st.player == cur {
            link.master_seen_ms.store(link.now_ms(), Ordering::Relaxed);
        }

        // The master names us as its successor: take the role.  It keeps
        // reporting MASTER (with Mh = us) until it sees our status with the
        // master bit set, then drops — so while it is handing to us, its
        // MASTER flag is not a claim against ours.
        if st.master && handing_to_us {
            if !link.master.load(Ordering::Relaxed) {
                link.take_master(&format!("player {} is handing off", st.player));
            }
            if cur != st.player { link.master_player.store(st.player as u32, Ordering::Relaxed); }
            return out;
        }

        if st.master && cur != st.player {
            link.master_player.store(st.player as u32, Ordering::Relaxed);
            log::info!("ProDJ Link: player {} is tempo master", st.player);
        }
        if st.master && link.master.load(Ordering::Relaxed) && link.handoff_to.load(Ordering::Relaxed) == st.player as u32 {
            // A peer we granted the role to has claimed it.
            link.master.store(false, Ordering::Relaxed);
            link.handoff_to.store(0, Ordering::Relaxed);
            log::info!("ProDJ Link: handed master to player {}", st.player);
        } else if st.master && link.master.load(Ordering::Relaxed) {
            // A peer still reports master while we do.  Per the DJ Link spec the
            // **handshake decides mastery, not the sync counter** — and the
            // *outgoing* master deliberately bumps its own Syncn above everyone
            // and keeps its master flag until it sees the new master (us) assert
            // Mm=1.  So during a handoff the old master transiently shows
            // master=true WITH a higher counter; abdicating on that counter was
            // the bug (we dropped Mm before it saw us assert → it never yielded).
            // We took master through the handoff (its 0x27 / Mh=us); HOLD, and it
            // drops its own flag once it sees our status.
            log::debug!("ProDJ Link: player {} still shows master (sync {} vs ours {}); holding — the handoff decides",
                        st.player, st.sync_counter, link.our_sync.load(Ordering::Relaxed));
        }
        if !st.master && cur == st.player {
            link.master_player.store(0, Ordering::Relaxed);
            log::info!("ProDJ Link: player {} released master", st.player);
        }
        out
    }
}

/// A master that has sent no status for [`MASTER_GONE_MS`] is forgotten.
/// Status comes every 200 ms from any deck, paused or not.  Without this,
/// two decks that both went quiet (iOS reclaimed their sockets; the other
/// side slept) each kept asking the other for a handoff that never came,
/// and MASTER could not be set on either.  A synced deck whose master
/// vanishes promotes itself, so the synced group keeps a tempo reference.
const MASTER_GONE_MS: u64 = 5000;

fn forget_silent_master(link: &LinkState, now_ms: u64, send_full: bool) {
    let cur = link.master_player.load(Ordering::Relaxed) as u8;
    let seen = link.master_seen_ms.load(Ordering::Relaxed);
    if cur != 0 && cur != link.player() && seen > 0 && now_ms.saturating_sub(seen) > MASTER_GONE_MS {
        log::info!("ProDJ Link: master player {cur} silent for {}s — forgetting it", MASTER_GONE_MS / 1000);
        link.master_player.store(0, Ordering::Relaxed);
        link.yielded_from.store(0, Ordering::Relaxed);
        if link.sync.load(Ordering::Relaxed) && send_full {
            link.take_master("our master vanished while we were synced");
        }
    }
}

/// MASTER pressed: the sender's side of the handoff.
///
/// With no master on the network for two seconds, take the role.  With one,
/// send it the 0x26 takeover request and WAIT for the handshake: the master
/// replies 0x27 (agreed, `yielded_from`) and then names us in its status's
/// Mh byte, and only then do we assert master (see [`StatusRx`] and
/// [`BeatRx`]).  A real CDJ only takes master after the current one agrees;
/// asserting early leaves a rogue second master the decks ignore.  Requests
/// repeat about once a second until the master agrees, then stop, as Beat
/// Link does.
struct Handoff {
    /// When MASTER was pressed: the current master's status may not have
    /// been heard yet (status every ~200 ms), so wait before concluding
    /// there is none.
    want_since:   Option<Instant>,
    last_request: Instant,
}

impl Default for Handoff {
    fn default() -> Self { Handoff { want_since: None, last_request: Instant::now() - Duration::from_secs(5) } }
}

impl Handoff {
    fn tick(&mut self, link: &LinkState, now: Instant, send_full: bool, player: u8) -> Option<Reply> {
        if !(send_full && link.want_master.load(Ordering::Relaxed) && !link.master.load(Ordering::Relaxed)) {
            self.want_since = None;
            return None;
        }
        let since = *self.want_since.get_or_insert(now);
        let cur = link.master_player.load(Ordering::Relaxed) as u8;
        match (cur, link.peer_ip(cur)) {
            (0, _) if now.duration_since(since) >= Duration::from_secs(2) => {
                link.take_master("no master on the network");
                self.want_since = None;
                None
            }
            (0, _) => None,
            (p, Some(ip)) => {
                if link.yielded_from.load(Ordering::Relaxed) == 0
                    && now.duration_since(self.last_request) >= Duration::from_millis(1000) {
                    log::info!("ProDJ Link: requesting master from player {p} at {ip}");
                    self.last_request = now;
                    return Some((ProDjLink::new(player).build_master_request(), SocketAddr::from((ip, PORT_BEAT))));
                }
                None
            }
            (p, None) => {
                if now.duration_since(self.last_request) >= Duration::from_millis(500) {
                    log::warn!("ProDJ Link: master is player {p} but its address is unknown yet");
                    self.last_request = now;
                }
                None
            }
        }
    }
}

/// A CDJ takes master by itself when it plays with no master on the
/// network (seen on two emulated CDJ-3000s, 2026-11-20: a unit that started
/// playing alone lit MASTER PLAYER; Dan's tester saw the same on a real
/// one).  We do the same: sending, playing a loaded track, nobody master
/// and no handoff under way for [`AUTO_MASTER_AFTER`] (long enough to hear
/// an existing master's status, which comes every 200 ms), take the role.
/// With a master on the network nothing happens; MASTER asks for it.
const AUTO_MASTER_AFTER: Duration = Duration::from_secs(2);

#[derive(Default)]
struct AutoMaster { since: Option<Instant> }

impl AutoMaster {
    fn tick(&mut self, link: &LinkState, now: Instant, send_full: bool, playing: bool, track_loaded: bool) {
        let free = send_full && playing && track_loaded
            && !link.master.load(Ordering::Relaxed)
            && link.master_player.load(Ordering::Relaxed) == 0
            && link.yielded_from.load(Ordering::Relaxed) == 0;
        if !free { self.since = None; return; }
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) >= AUTO_MASTER_AFTER {
            link.take_master("playing with no master on the network");
            self.since = None;
        }
    }
}

/// Our status to every peer, each in its generation's form: a CDJ-3000's
/// long status (with the loop) to all but older players, which get `short`.
fn status_replies(link: &LinkState, short: &[u8], long: &[u8]) -> Vec<Reply> {
    let peers: Vec<(u8, Ipv4Addr)> = link.peers.lock().map(|p| p.iter().map(|(k, v)| (*k, *v)).collect()).unwrap_or_default();
    let names = link.peer_names.lock().map(|n| n.clone()).unwrap_or_default();
    peers.into_iter().map(|(pl, ip)| {
        let name = names.get(&pl).map(String::as_str).unwrap_or("");
        ((if wants_cdj3000_status(name) { long } else { short }).to_vec(), SocketAddr::from((ip, PORT_STATUS)))
    }).collect()
}

// ── Sender ────────────────────────────────────────────────────────────────────

/// How long AUTO listens for keep-alives before choosing (players send one
/// every 1.5–2 s, so this hears everyone already on the network).
const CLAIM_WATCH: Duration = Duration::from_millis(3000);
/// Gap between the packets of each claim stage, as the players space them.
const CLAIM_STEP: Duration = Duration::from_millis(300);

/// The order AUTO tries player numbers in: 4 first, up through 5 and 6,
/// then down through 3, 2, 1, so OpenDeck sits above the players a DJ
/// numbers from 1 and leaves the low numbers to them.
pub const AUTO_ORDER: [u8; 6] = [4, 5, 6, 3, 2, 1];

/// The next number AUTO should try: the first in [`AUTO_ORDER`] that no
/// device is using and that has not been defended against us.  `None` when
/// all are taken.
pub fn pick_number(used: &[u8], tried: &[u8]) -> Option<u8> {
    AUTO_ORDER.into_iter().find(|n| !used.contains(n) && !tried.contains(n))
}

/// Run the device-number claim: hello ×3 (first time only), then stages
/// 1–3, three packets each.  AUTO picks a free number and moves on when one
/// is defended; a set number is claimed with the "specific number" flag and
/// kept even if defended (with a warning), as before.  Returns the number.
fn claim_number(link: &LinkState, sock: &UdpSocket, ip: Ipv4Addr, bcast: Ipv4Addr, mac: [u8; 6], hello: bool) -> u8 {
    let send = |p: &[u8]| { let _ = sock.send_to(p, (bcast, PORT_ANNOUNCE)); };
    link.claimed.store(false, Ordering::Relaxed);
    if hello {
        let me = ProDjLink::new(0);
        for _ in 0..3 { send(&me.build_hello()); thread::sleep(CLAIM_STEP); }
    }
    if link.auto {
        // Hello took ~0.9 s; listen out the rest of the window.
        thread::sleep(CLAIM_WATCH.saturating_sub(if hello { CLAIM_STEP * 3 } else { Duration::ZERO }));
    }
    let mut tried: Vec<u8> = Vec::new();
    let taken = |n: u8| link.peers.lock().map(|p| p.contains_key(&n)).unwrap_or(false);
    'numbers: loop {
        let n = if link.auto {
            let used: Vec<u8> = link.peers.lock().map(|p| p.keys().copied().collect()).unwrap_or_default();
            match pick_number(&used, &tried) {
                Some(n) => n,
                None => {
                    log::warn!("ProDJ Link: no free player number (in use: {used:?}); using {}", link.preferred);
                    link.preferred
                }
            }
        } else { link.preferred };
        if n > 4 { log::info!("ProDJ Link: trying player {n} (4 is in use; 5 and 6 need a 6-player network: CDJ-3000s, no older players)"); }
        link.player.store(n, Ordering::Relaxed);
        link.defended.store(0, Ordering::Relaxed);
        let me = ProDjLink::new(n);
        log::info!("ProDJ Link: claiming player {n}{}", if link.auto { " (AUTO)" } else { "" });
        for stage in 1..=3u8 {
            for counter in 1..=3u8 {
                send(&match stage {
                    1 => me.build_claim1(counter, mac),
                    2 => me.build_claim2(counter, ip, mac, n, link.auto),
                    _ => me.build_claim3(counter, n),
                });
                thread::sleep(CLAIM_STEP);
                let lost = link.defended.load(Ordering::Relaxed) == n || (link.auto && taken(n));
                if lost {
                    if link.auto && tried.len() < 6 {
                        log::info!("ProDJ Link: player {n} is taken; trying another number");
                        tried.push(n);
                        continue 'numbers;
                    }
                    log::warn!("ProDJ Link: player {n} is in use by another device; PLAYER No. is set to {n}, keeping it");
                    break 'numbers;
                }
            }
        }
        break;
    }
    let n = link.player();
    link.claimed.store(true, Ordering::Relaxed);
    log::info!("ProDJ Link: player {n} is ours");
    n
}

/// Announces this deck, unicasts status to every peer, sends a beat packet at
/// every beat of the *audible* position, runs the master handoff, and
/// follows the master when SYNC is on.
pub struct ProDjSender {
    _thread: thread::JoinHandle<()>,
}

impl ProDjSender {
    pub fn start(link: Arc<LinkState>, st: SenderState) -> Option<Self> {
        let mut sock = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| log::warn!("ProDJ Link: sender socket failed: {e}"))
            .ok()?;
        if let Err(e) = sock.set_broadcast(true) {
            log::warn!("ProDJ Link: set_broadcast failed: {e} (announces/beats will not go out)");
        }
        let (ip, bcast, mac, iface) = link_interface(&[]);
        if let Ok(mut a) = link.own_addr.lock() { *a = format!("{ip} ({iface}) to {bcast}"); }
        // OPENDECK_LINK_UNICAST=ip,ip — also send announces straight to these
        // devices (broadcast does not cross a VPN such as Tailscale).
        let unicast_peers: Vec<Ipv4Addr> = std::env::var("OPENDECK_LINK_UNICAST").ok()
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect()).unwrap_or_default();
        if !unicast_peers.is_empty() { log::info!("ProDJ Link: announcing by unicast to {unicast_peers:?}"); }
        log::info!("ProDJ Link: sending from {ip} ({iface}) to {bcast}");
        let t = thread::Builder::new()
            .name("prodj-tx".into())
            .spawn(move || {
                // Re-selected below once players are discovered.
                let (mut ip, mut bcast, mut mac) = (ip, bcast, mac);
                // PLAYER No. AUTO: listen first, so the numbers already on
                // the network are known, and so the interface can be picked
                // by where the players are.
                if link.auto {
                    thread::sleep(CLAIM_WATCH);
                    let peers: Vec<Ipv4Addr> = link.peers.lock().map(|p| p.values().copied().collect()).unwrap_or_default();
                    let (nip, nbc, nmac, niface) = link_interface(&peers);
                    if (nip, nbc) != (ip, bcast) {
                        if let Ok(mut a) = link.own_addr.lock() { *a = format!("{nip} ({niface}) to {nbc}"); }
                    }
                    (ip, bcast, mac) = (nip, nbc, nmac);
                }
                let port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
                if let Ok(mut o) = link.own_src.lock() { *o = Some((ip, port)); }
                if let Ok(mut o) = link.own_bcast.lock() { *o = Some(bcast); }
                let mut player = claim_number(&link, &sock, ip, bcast, mac, true);
                let mut me = ProDjLink::new(player);
                let mut announce = me.build_announce(ip, mac);
                let mut known_peers: Vec<Ipv4Addr>;
                let mut bcast_warned = false;
                let mut send_failures = 0u32;
                let mut last_announce = Instant::now() - Duration::from_secs(5);
                let mut last_media_query = Instant::now() - Duration::from_secs(5);
                let mut last_status   = Instant::now() - Duration::from_secs(5);
                let mut last_abs      = Instant::now() - Duration::from_secs(5);
                // When MASTER is pressed we may not have heard the current
                // master's status yet (it arrives every ~200 ms, announces
                // every 1.5 s).  Wait before concluding nobody is master.
                let mut handoff = Handoff::default();
                let mut auto_master = AutoMaster::default();
                let mut counter: u32  = 0;
                let mut last_sent = Instant::now();
                // Audible-position estimate.  The decoder cursor advances a
                // 512-frame block at a time and `in_flight` swings ±35 ms
                // between blocks, so beat crossings read straight off
                // `position - in_flight` land on block boundaries.  Free-run
                // an estimate at the true rate and pull it gently toward the
                // low-passed reference; τ ≈ 200 ms on both at the 1 ms tick.
                // Measured against an XDJ-1000MK2 at the same receiver:
                // sd 1.23 ms vs its 1.33 ms.  See PERFORMANCE.md.
                let mut clock = BeatClock::default();
                let mut est_frames: f64 = -1.0;
                let mut last_tick = Instant::now();
                // Sync follow: phase nudge in progress (speed offset, until).
                let mut nudge: Option<(f32, Instant)> = None;
                let mut seen_master_beat = link.master_beat_seq.load(Ordering::Relaxed);
                // A jog nudge happened while synced: measure the offset it
                // left at the next master beat after the wheel settles.
                let mut phase = SyncPhase::default();
                // Status goes out immediately when our master/sync flags change:
                // the old master waits only briefly for the new one's claim.
                let mut last_flags = (false, false);

                loop {
                    // Someone else holds our number: claim another (AUTO).
                    if link.reclaim.swap(false, Ordering::Relaxed) {
                        player = claim_number(&link, &sock, ip, bcast, mac, false);
                        me = ProDjLink::new(player);
                        announce = me.build_announce(ip, mac);
                        last_announce = Instant::now() - Duration::from_secs(5);
                    }
                    let now = Instant::now();
                    let dt  = now.duration_since(last_tick).as_secs_f64();
                    // Load the current track's grid (updated on each LOAD).
                    let grid_arc = st.grid.load_full();
                    let cur_grid: &Option<BeatGrid> = &grid_arc;
                    last_tick = now;
                    let playing = st.playing.load(Ordering::Relaxed);
                    let fader   = f32::from_bits(st.fader_speed.load(Ordering::Relaxed)) as f64;
                    let rate    = st.sample_rate as f64 * fader;

                    // ── Audible position estimate ────────────────────────────
                    let mut beat_due: Option<i64> = None;
                    if playing {
                        if let Some(grid) = cur_grid {
                            let ch = st.channels.max(1) as u64;
                            let lp = st.loop_active.load(Ordering::Relaxed).then(|| (
                                st.loop_start.load(Ordering::Relaxed) / ch, st.loop_end.load(Ordering::Relaxed) / ch));
                            let to_next = clock.tick(grid, st.sample_rate, st.position.load(Ordering::Relaxed) / ch,
                                                     st.in_flight.load(Ordering::Relaxed) as f64 / ch as f64, lp, dt * rate, rate);
                            // Sleep-to-deadline for the last stretch before a beat.
                            let landed = if to_next > 0.0 && to_next < 0.0015 {
                                let target = now + Duration::from_secs_f64(to_next);
                                while Instant::now() < target { std::hint::spin_loop(); }
                                Some(to_next * rate)
                            } else { None };
                            beat_due = clock.finish(grid, st.sample_rate, landed, lp);
                            est_frames = clock.est.frames;
                        }
                    } else {
                        clock.stop();
                        est_frames = -1.0;
                    }

                    // ── Beat packet ──────────────────────────────────────────
                    let send_full = st.send_full.load(Ordering::Relaxed);
                    if send_full { if let (Some(grid), Some(beat)) = (cur_grid, beat_due) {
                        {
                            let bib  = ((beat + grid.downbeat_offset as i64).rem_euclid(4) + 1) as u8;
                            let snap = EngineSnapshot {
                                position: 0, ghost_position: 0, speed: fader as f32,
                                bpm: grid.bpm as f32 * fader as f32,
                                beat_phase: 0.0, bar_phase: (bib - 1) as f32 / 4.0,
                                is_playing: true, slip_active: false, key_lock: true,
                                deck_id: player, timestamp_ns: 0,
                            };
                            let pkt = me.build_beat(&snap, bib);
                            let sent_at = Instant::now();
                            if let Err(e) = sock.send_to(&pkt, (bcast, PORT_BEAT)) {
                                if !bcast_warned {
                                    bcast_warned = true;
                                    log::warn!("ProDJ Link: beat broadcast to {bcast}:{PORT_BEAT} failed: {e}");
                                }
                            }
                            // Also straight to each OpenDeck peer.  An access
                            // point holds broadcasts for a power-saving Wi-Fi
                            // client until its beacon interval, so an iPad
                            // gets beats late and in bursts; unicast is not
                            // held that way.  Only our own kind: a CDJ must
                            // not see each beat twice.  The receiver drops
                            // the duplicate (see listen_beat).
                            for (pl, pip) in link.opendeck_peers() {
                                if pl != player { let _ = sock.send_to(&pkt, (pip, PORT_BEAT)); }
                            }
                            log::debug!("ProDJ tx: beat {beat} ({bib}/4) @ {:.2} BPM  +{:.2}ms", snap.bpm, sent_at.duration_since(last_sent).as_secs_f64() * 1000.0);
                            last_sent = sent_at;
                        }
                    } }

                    // ── Absolute Position (CDJ-3000) ─────────────────────────
                    // Every 30 ms while a track is loaded, playing or not: the
                    // heard playhead in ms, the track length, pitch and
                    // effective BPM.  Lighting, video and timecode tools follow
                    // a CDJ-3000 by these, not beats.  Like a CDJ-3000, sent
                    // straight to each device that has announced itself on
                    // 50001, not broadcast (captured from one, 2026-11-12); a
                    // tool announces itself to receive them.
                    if send_full && now.duration_since(last_abs) >= Duration::from_millis(30) {
                        let decoded = st.samples.load().len() as u64;
                        if decoded > 0 {
                            last_abs = now;
                            let ch = st.channels.max(1) as u64;
                            let sr = st.sample_rate.max(1) as f64;
                            let total = decoded.max(st.total_hint.load(Ordering::Relaxed)) / ch;
                            // The heard position: the free-running beat
                            // estimate while it runs (smooth between decoder
                            // blocks), else the decoder cursor less what is
                            // still queued.
                            let frames = if playing && est_frames >= 0.0 { est_frames } else {
                                let pos = st.position.load(Ordering::Relaxed);
                                (pos.saturating_sub(st.in_flight.load(Ordering::Relaxed)) / ch) as f64
                            };
                            let pkt = me.build_abs_position(
                                (total as f64 / sr) as u32,
                                (frames.max(0.0) * 1000.0 / sr) as u32,
                                ((fader - 1.0) * 10_000.0).round() as i32,
                                cur_grid.as_ref().map(|g| (g.bpm * fader * 10.0).round() as u32),
                            );
                            let peers = link.peers.lock().map(|p| p.clone()).unwrap_or_default();
                            for pip in position_targets(&peers, player) {
                                let _ = sock.send_to(&pkt, (pip, PORT_BEAT));
                            }
                        }
                    }

                    // ── Announce ─────────────────────────────────────────────
                    if now.duration_since(last_announce) >= Duration::from_millis(1500) {
                        // The interface was chosen before any player was known.
                        // Now that some are, re-check: only an interface on a
                        // player's subnet can reach it by broadcast.  Cheap, and
                        // only when the peer set actually changed.
                        // Re-check the interface on EVERY announce, not only
                        // when the peer set changes: an iOS app is kept alive
                        // for days, and one carried from one Wi-Fi to another
                        // without a restart kept announcing to the old
                        // network's broadcast address.  get_if_addrs is cheap.
                        let mut peers: Vec<Ipv4Addr> = link.peers.lock()
                            .map(|p| p.values().copied().collect()).unwrap_or_default();
                        peers.sort();
                        known_peers = peers;
                        let (nip, nbc, nmac, niface) = link_interface(&known_peers);
                        if nbc != bcast || nip != ip {
                            log::info!(
                                "ProDJ Link: moving to {nip} ({niface}) to {nbc} — reaches {} player(s), was {ip} to {bcast}",
                                known_peers.len(),
                            );
                            ip = nip; bcast = nbc; mac = nmac;
                            if let Ok(mut a) = link.own_addr.lock() { *a = format!("{ip} ({niface}) to {bcast}"); }
                            if let Ok(mut o) = link.own_src.lock() { *o = Some((ip, sock.local_addr().map(|a| a.port()).unwrap_or(0))); }
                            if let Ok(mut o) = link.own_bcast.lock() { *o = Some(bcast); }
                            announce = me.build_announce(ip, mac);
                            bcast_warned = false;   // re-warn if the new one also fails
                        }
                        for p in &unicast_peers { let _ = sock.send_to(&announce, (*p, PORT_ANNOUNCE)); }
                        match sock.send_to(&announce, (bcast, PORT_ANNOUNCE)) {
                            Ok(_) => send_failures = 0,
                            Err(e) => {
                                if !bcast_warned {
                                    bcast_warned = true;
                                    log::warn!("ProDJ Link: announce broadcast to {bcast}:{PORT_ANNOUNCE} failed: {e}");
                                }
                                // Three announces in a row failing = a dead
                                // socket (iOS reclaims them after a suspension);
                                // open a new one.
                                send_failures += 1;
                                if send_failures >= 3 {
                                    if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
                                        let _ = s.set_broadcast(true);
                                        sock = s;
                                        if let Ok(mut o) = link.own_src.lock() { *o = Some((ip, sock.local_addr().map(|a| a.port()).unwrap_or(0))); }
                                        send_failures = 0;
                                        bcast_warned = false;
                                        log::info!("ProDJ Link: sender socket reopened");
                                    }
                                }
                            }
                        }
                        last_announce = now;
                    }

                    // ── Media queries ────────────────────────────────────────
                    // Ask every player what is in its USB and SD slots, the way
                    // a CDJ fills its LINK list.  Players without media in a
                    // slot do not answer; see LinkState::media for staleness.
                    if now.duration_since(last_media_query) >= MEDIA_QUERY_EVERY {
                        let peers: Vec<(u8, Ipv4Addr)> = link.peers.lock()
                            .map(|p| p.iter().map(|(k, v)| (*k, *v)).collect()).unwrap_or_default();
                        for (p, pip) in peers {
                            if p == player || p > 16 { continue; }
                            for slot in [3u8, 2] {
                                let _ = sock.send_to(&me.build_media_query(ip, p, slot), (pip, PORT_STATUS));
                            }
                        }
                        last_media_query = now;
                    }

                    // ── A master that fell silent is forgotten ───────────────
                    forget_silent_master(&link, link.now_ms(), send_full);

                    // ── Master handoff ───────────────────────────────────────
                    let req = handoff.tick(&link, now, send_full, player);
                    send_replies(Some(&sock), req.into_iter().collect());
                    let has_track = cur_grid.is_some() || !st.samples.load().is_empty();
                    auto_master.tick(&link, now, send_full, st.playing.load(Ordering::Relaxed), has_track);

                    // ── SYNC follow ──────────────────────────────────────────
                    let sync = link.sync.load(Ordering::Relaxed) && !link.master.load(Ordering::Relaxed);
                    let now_ms = link.epoch.elapsed().as_millis() as u64;
                    let jogging = now_ms < link.jog_nudge_ms.load(Ordering::Relaxed).saturating_add(JOG_SETTLE_MS);
                    if phase.tick(sync, jogging) && link.sync_offset.load(Ordering::Relaxed) != 0.0f32.to_bits() {
                        link.sync_offset.store(0.0f32.to_bits(), Ordering::Relaxed);
                    }
                    if sync && cur_grid.is_some() {
                        let grid = cur_grid.as_ref().unwrap();
                        let master_bpm = f32::from_bits(link.master_bpm.load(Ordering::Relaxed));
                        if master_bpm > 0.0 {
                            // Tempo: set the fader so our effective BPM equals the master's.
                            let want = (master_bpm / grid.bpm as f32).clamp(1.0 - 0.16, 1.0 + 0.16);
                            let have = fader as f32;
                            if (want - have).abs() > 0.0002 {
                                st.fader_speed.store(want.to_bits(), Ordering::Relaxed);
                                if nudge.is_none() { st.speed.store(want.to_bits(), Ordering::Relaxed); }
                                log::debug!("ProDJ sync: tempo → {:+.2}% ({master_bpm:.2} BPM)", (want - 1.0) * 100.0);
                            }
                            // Phase: on each master beat, nudge toward phase 0.
                            let seq = link.master_beat_seq.load(Ordering::Relaxed);
                            if seq != seen_master_beat {
                                seen_master_beat = seq;
                                if let Some((_, frac)) = clock.beat_now {
                                    let held = f32::from_bits(link.sync_offset.load(Ordering::Relaxed)) as f64;
                                    // One line per master beat: the end-to-end SHIFT GRID
                                    // test (tools/test_shift_grid.py) reads these.
                                    log::debug!("ProDJ sync: beat phase {:+.3} held {held:+.3}{}", phase_err(frac, 0.0), if jogging { " jog" } else { "" });
                                    match phase.on_master_beat(frac, held, jogging) {
                                        PhaseAct::None => {}
                                        PhaseAct::Hold(off) => {
                                            link.sync_offset.store((off as f32).to_bits(), Ordering::Relaxed);
                                            log::info!("ProDJ sync: holding phase offset {off:+.3} beat");
                                        }
                                        PhaseAct::Correct(offset) => {
                                            let period_s = 60.0 / master_bpm.max(1.0);
                                            nudge = Some((offset, now + Duration::from_secs_f32(period_s)));
                                            st.speed.store((want + offset).to_bits(), Ordering::Relaxed);
                                            log::debug!("ProDJ sync: phase {frac:.3} vs held {held:+.3} → nudge {:+.2}%", offset * 100.0);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some((_, until)) = nudge {
                        if now >= until {
                            nudge = None;
                            st.speed.store(st.fader_speed.load(Ordering::Relaxed), Ordering::Relaxed);
                        }
                    }

                    // ── Status ───────────────────────────────────────────────
                    let flags = (link.master.load(Ordering::Relaxed), link.sync.load(Ordering::Relaxed));
                    let flags_changed = flags != last_flags;
                    last_flags = flags;
                    if send_full && (flags_changed || now.duration_since(last_status) >= Duration::from_millis(200)) {
                        let (beat_num, bib) = match (cur_grid, clock.beat_now) {
                            (Some(grid), Some((beat, _))) => {
                                let first = grid.beat_at_sample(0, st.sample_rate).floor() as i64;
                                (Some((beat - first).max(0) as u32), Some(((beat + grid.downbeat_offset as i64).rem_euclid(4) + 1) as u8))
                            }
                            _ => (None, None),
                        };
                        let track = link.track.lock().ok().and_then(|t| *t);
                        let fields = StatusFields {
                            track,
                            serving: link.serve_tracks.load(Ordering::Relaxed) > 0,
                            playing,
                            track_loaded: track.is_some() || cur_grid.is_some(),
                            master:  link.master.load(Ordering::Relaxed),
                            sync:    link.sync.load(Ordering::Relaxed),
                            on_air:  false,
                            pitch:   fader as f32,
                            bpm:     cur_grid.as_ref().map(|g| g.bpm as f32),
                            beat:    beat_num,
                            beat_in_bar: bib,
                            handoff_to: match link.handoff_to.load(Ordering::Relaxed) { 0 => None, p => Some(p as u8) },
                            counter,
                            sync_counter: link.our_sync.load(Ordering::Relaxed),
                        };
                        counter = counter.wrapping_add(1);
                        let active_loop = active_loop(&st, cur_grid.as_ref());
                        let pkt = me.build_status(&fields);
                        let long = me.build_status_cdj3000(&fields, active_loop);
                        let out = status_replies(&link, &pkt, &long);
                        if flags_changed {
                            log::info!("ProDJ tx: status master={} sync={} → {} peer(s)", fields.master, fields.sync, out.len());
                        }
                        send_replies(Some(&sock), out);
                        last_status = now;
                    }

                    // Tick at 1 ms while playing AND sending — the beat clock
                    // is read off this loop.  Paused, or receive-only (no beat
                    // packets to time), sleep to the next periodic deadline
                    // instead: a thousand wakeups a second, not the work they
                    // do, is what keeps a phone's core out of its idle state
                    // (and cost 9 % of a Pi Zero's one core).  Capped so flag
                    // changes (MASTER, SYNC) and a fresh peer are still picked
                    // up within 100 ms.
                    let nap = if playing && send_full { Duration::from_millis(1) } else {
                        let left = |last: Instant, every: Duration| every.saturating_sub(now.duration_since(last));
                        let mut n = left(last_announce, Duration::from_millis(1500))
                            .min(left(last_media_query, MEDIA_QUERY_EVERY));
                        if send_full { n = n.min(left(last_status, Duration::from_millis(200))); }
                        n.clamp(Duration::from_millis(1), Duration::from_millis(100))
                    };
                    thread::sleep(nap);
                }
            })
            .ok()?;
        Some(ProDjSender { _thread: t })
    }
}

#[cfg(test)]
mod claim_tests {
    use super::{pick_number, ClaimAction, ClaimGuard, LinkState};
    use opendeck_link::ProDjLink;
    use std::net::Ipv4Addr;
    use std::sync::atomic::Ordering;

    const US: (Ipv4Addr, u16) = (Ipv4Addr::new(169, 254, 1, 1), 40000);
    const LOW: (Ipv4Addr, u16) = (Ipv4Addr::new(169, 254, 0, 9), 41000);
    const HIGH: (Ipv4Addr, u16) = (Ipv4Addr::new(169, 254, 9, 9), 42000);

    /// A deck at US holding (or claiming) `n`.
    fn deck(n: u8, auto: bool, holding: bool) -> std::sync::Arc<LinkState> {
        let l = if auto { LinkState::auto(n) } else { LinkState::new(n) };
        *l.own_src.lock().unwrap() = Some(US);
        l.claimed.store(holding, Ordering::Relaxed);
        l
    }
    fn announce(n: u8, ip: Ipv4Addr) -> Vec<u8> { ProDjLink::new(n).build_announce(ip, [0; 6]) }

    #[test]
    fn a_claim_for_our_number_is_defended_once_held() {
        let link = deck(2, true, true);
        let mut g = ClaimGuard::default();
        let claim = ProDjLink::new(2).build_claim3(1, 2);
        assert_eq!(g.on_packet(&link, &claim, Some(HIGH)), ClaimAction::Defend { number: 2, to: HIGH.0, ours: US.0 });
        let other = ProDjLink::new(3).build_claim3(1, 3);
        assert_eq!(g.on_packet(&link, &other, Some(HIGH)), ClaimAction::Done, "not our number");
        // Still claiming: a claim for the same number is not ours to defend.
        let claiming = deck(2, true, false);
        assert_eq!(ClaimGuard::default().on_packet(&claiming, &claim, Some(HIGH)), ClaimAction::Done);
    }

    #[test]
    fn a_defense_while_claiming_marks_the_number_taken() {
        let link = deck(3, true, false);
        let d = ProDjLink::new(3).build_defense(3, HIGH.0);
        assert_eq!(ClaimGuard::default().on_packet(&link, &d, Some(HIGH)), ClaimAction::Done);
        assert_eq!(link.defended.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn a_defense_of_the_number_we_hold_moves_auto_but_not_a_set_number() {
        let d = ProDjLink::new(1).build_defense(1, HIGH.0);
        let auto = deck(1, true, true);
        ClaimGuard::default().on_packet(&auto, &d, Some(HIGH));
        assert!(auto.reclaim.load(Ordering::Relaxed));
        let set = deck(1, false, true);
        ClaimGuard::default().on_packet(&set, &d, Some(HIGH));
        assert!(!set.reclaim.load(Ordering::Relaxed));
    }

    #[test]
    fn our_own_packets_are_ignored() {
        let link = deck(1, true, false);
        let d = ProDjLink::new(1).build_defense(1, US.0);
        assert_eq!(ClaimGuard::default().on_packet(&link, &d, Some(US)), ClaimAction::Done);
        let mut g = ClaimGuard { defend_port: Some(50555), ..Default::default() };
        assert_eq!(g.on_packet(&link, &d, Some((US.0, 50555))), ClaimAction::Done, "our defense socket's echo");
        // Our broadcast back through the host's other interface (Wi-Fi beside
        // ethernet): same sender port, another of our addresses.
        let wifi = Ipv4Addr::new(192, 168, 68, 57);
        let mut g = ClaimGuard { local_ips: vec![wifi], ..Default::default() };
        let held = deck(1, true, true);
        assert_eq!(g.on_packet(&held, &announce(1, US.0), Some((wifi, US.1))), ClaimAction::Done);
        assert!(!held.reclaim.load(Ordering::Relaxed), "not a collision with ourselves");
        assert_eq!(link.defended.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_announce_collision_moves_only_the_higher_address() {
        // We are at US; a lower address announcing our number keeps it.
        let link = deck(4, true, true);
        assert_eq!(ClaimGuard::default().on_packet(&link, &announce(4, LOW.0), Some(LOW)), ClaimAction::Continue);
        assert!(link.reclaim.load(Ordering::Relaxed), "we are higher: we move");
        let link = deck(4, true, true);
        ClaimGuard::default().on_packet(&link, &announce(4, HIGH.0), Some(HIGH));
        assert!(!link.reclaim.load(Ordering::Relaxed), "they are higher: they move");
    }

    #[test]
    fn media_answers_carry_the_number_we_hold_now() {
        let link = LinkState::auto(1);
        link.serve_tracks.store(3, Ordering::Relaxed);
        link.player.store(2, Ordering::Relaxed);      // AUTO claimed 2, not the preferred 1
        let peer = Ipv4Addr::new(169, 254, 5, 55);
        let q = ProDjLink::new(1).build_media_query(peer, 2, 3);
        let (resp, to) = super::answer_media_query(&link, &q).expect("answers a query for us");
        assert_eq!(to, peer);
        assert_eq!(ProDjLink::parse_media_response(&resp).unwrap().player, 2);
        assert!(super::answer_media_query(&link, &ProDjLink::new(1).build_media_query(peer, 1, 3)).is_none(), "not our number");
        link.serve_tracks.store(0, Ordering::Relaxed);
        assert!(super::answer_media_query(&link, &q).is_none(), "nothing served");
    }

    #[test]
    fn a_device_we_defended_against_does_not_move_us() {
        let link = deck(1, true, true);
        let mut g = ClaimGuard::default();
        g.on_packet(&link, &ProDjLink::new(1).build_claim2(1, LOW.0, [0; 6], 1, false), Some(LOW));
        g.on_packet(&link, &announce(1, LOW.0), Some(LOW));
        assert!(!link.reclaim.load(Ordering::Relaxed));
    }

    #[test]
    fn auto_starts_at_four_goes_up_then_down() {
        assert_eq!(pick_number(&[], &[]), Some(4));
        assert_eq!(pick_number(&[1, 2], &[]), Some(4));
        assert_eq!(pick_number(&[4], &[]), Some(5));
        assert_eq!(pick_number(&[4, 5], &[]), Some(6));
        assert_eq!(pick_number(&[4, 5, 6], &[]), Some(3));
        assert_eq!(pick_number(&[3, 4, 5, 6], &[]), Some(2));
        assert_eq!(pick_number(&[2, 3, 4, 5, 6], &[]), Some(1));
        assert_eq!(pick_number(&[1, 2, 3, 4, 5, 6], &[]), None);
        // Every number in the order, each tried once.
        let mut tried = Vec::new();
        while let Some(n) = pick_number(&[], &tried) { tried.push(n); }
        assert_eq!(tried, [4, 5, 6, 3, 2, 1]);
    }

    #[test]
    fn auto_skips_defended_numbers_and_ignores_non_players() {
        assert_eq!(pick_number(&[], &[4]), Some(5), "skips a number already defended");
        assert_eq!(pick_number(&[5], &[4, 6]), Some(3));
        // A mixer (33) or rekordbox (17+) does not use a player number.
        assert_eq!(pick_number(&[33, 17], &[]), Some(4));
    }
}

#[cfg(test)]
mod interface_tests {
    use super::*;

    #[test]
    fn link_local_is_recognised() {
        // What an idle USB-C ethernet dongle self-assigns when DHCP is silent.
        assert!(is_link_local("169.254.247.12".parse().unwrap()));
        assert!(!is_link_local("192.168.68.51".parse().unwrap()));
        assert!(!is_link_local("10.0.0.4".parse().unwrap()));
        // Neighbouring /16s must not be swept up.
        assert!(!is_link_local("169.253.0.1".parse().unwrap()));
        assert!(!is_link_local("169.255.0.1".parse().unwrap()));
    }

    #[test]
    fn subnet_match_picks_the_players_network() {
        let mask: Ipv4Addr = "255.255.255.0".parse().unwrap();
        let wifi: Ipv4Addr = "192.168.68.51".parse().unwrap();
        let xdj:  Ipv4Addr = "192.168.68.58".parse().unwrap();
        assert!(same_subnet(wifi, xdj, mask));

        // The dongle the iPad was broadcasting onto cannot reach the XDJ.
        let dongle: Ipv4Addr = "169.254.247.12".parse().unwrap();
        assert!(!same_subnet(dongle, xdj, "255.255.0.0".parse().unwrap()));

        // A wider mask genuinely does cover it.
        assert!(same_subnet("192.168.1.1".parse().unwrap(), xdj, "255.255.0.0".parse().unwrap()));
        // ...and a narrower one splits them: /29 puts .51 in .48-.55 and .58 in
        // .56-.63.  (/28 would NOT — both sit in .48-.63.)
        assert!(!same_subnet(wifi, xdj, "255.255.255.248".parse().unwrap()));
        assert!(same_subnet(wifi, xdj, "255.255.255.240".parse().unwrap()));
    }
}

#[cfg(test)]
mod peer_phase_tests {
    use super::*;

    const BPM: f32 = 120.0;              // 500 ms per beat
    const DT: f64 = 1.0 / 60.0;          // one 60 Hz frame

    /// Run frames from `from_ms` to `to_ms`, delivering packets whose ARRIVAL
    /// times are in `arrivals`, and return the drawn phase per frame.
    fn run(pp: &mut PeerPhase, from_ms: u64, to_ms: u64, arrivals: &[u64], playing: bool) -> Vec<(u64, f32)> {
        let mut out = Vec::new();
        let mut t = from_ms as f64;
        while (t as u64) < to_ms {
            let now = t as u64;
            let beat_ms = arrivals.iter().copied().filter(|a| *a <= now).max().unwrap_or(0);
            out.push((now, pp.advance(now, beat_ms, BPM, 0, playing, DT).0));
            t += DT * 1000.0;
        }
        out
    }

    /// Phase advanced (mod 1) between two frames.
    fn step(a: f32, b: f32) -> f32 { (b - a).rem_euclid(1.0) }

    /// The master row's red bar mark must never jump: beat packets land a
    /// few ms either side of the beat the free-run has drawn, and their
    /// beat-in-bar used to switch the bar count on arrival, a beat apart
    /// from the drawn phase for a frame or two.  The drawn bar position
    /// (beat in bar + phase) must advance smoothly through every beat.
    #[test]
    fn bar_count_turns_over_with_the_phase_not_the_packet() {
        let mut pp = PeerPhase::new();
        // Beats every 500 ms from 1 s, beat-in-bar 1,2,3,4,1…, each packet
        // landing 0–24 ms late or early in a fixed pseudo-random pattern.
        let jitter = |i: u64| ((i * 7919) % 25) as i64 - 12;
        let beats: Vec<(u64, u8)> = (0..40u64).map(|i| ((1000 + i as i64 * 500 + jitter(i)) as u64, (i % 4 + 1) as u8)).collect();
        let mut t = 1000.0;
        let mut prev: Option<f64> = None;
        while t < 20_000.0 {
            let now = t as u64;
            let (beat_ms, bib) = beats.iter().copied().filter(|(a, _)| *a <= now).last().unwrap_or((0, 0));
            let (ph, shown) = pp.advance(now, beat_ms, BPM, bib, true, DT);
            let bar = (shown as f64 - 1.0) + ph as f64;
            if let Some(p) = prev {
                let step = (bar - p).rem_euclid(4.0);
                assert!(step < 0.1, "bar position jumped {p:.3} → {bar:.3} at {now} ms");
            }
            prev = Some(bar);
            t += DT * 1000.0;
        }
        // And it is the right bar: at the beat due at 10 s (i = 18, beat 3).
        let mut pp2 = PeerPhase::new();
        let (mut t, mut shown) = (1000.0, 0);
        while t < 10_250.0 {
            let now = t as u64;
            let (beat_ms, bib) = beats.iter().copied().filter(|(a, _)| *a <= now).last().unwrap_or((0, 0));
            shown = pp2.advance(now, beat_ms, BPM, bib, true, DT).1;
            t += DT * 1000.0;
        }
        assert_eq!(shown, 3, "follows the master's bar");
    }

    #[test]
    fn steady_beats_run_at_tempo() {
        let mut pp = PeerPhase::new();
        let arrivals: Vec<u64> = (0..20).map(|i| 1000 + i * 500).collect();
        let frames = run(&mut pp, 1000, 8000, &arrivals, true);
        // 60 Hz at 500 ms/beat: 1/30 beat per frame, every frame.
        for w in frames.windows(2).skip(5) {
            let s = step(w[0].1, w[1].1);
            assert!((s - 1.0 / 30.0).abs() < 0.01, "step {s} at {} ms", w[1].0);
        }
    }

    /// Wi-Fi delivers beats late and bunched: packets held up to 400 ms and
    /// then released together.  The row must keep moving through it.
    #[test]
    fn late_bunched_packets_do_not_stall_the_row() {
        let mut pp = PeerPhase::new();
        // Beats every 500 ms from t=1000; the AP releases them at 1.4 s intervals.
        let mut arrivals = Vec::new();
        for i in 0..20u64 {
            let due = 1000 + i * 500;
            arrivals.push(due + (400 - (due % 1400).min(400)));   // 0..400 ms late, bunched
        }
        let frames = run(&mut pp, 1000, 11000, &arrivals, true);
        let inc = (DT / 0.5) as f32;
        for w in frames.windows(2).skip(10) {
            let s = step(w[0].1, w[1].1);
            // Never backwards, never stalled, never a jump: the pace stays
            // within SLEW of the free-run.
            assert!(s >= inc * (1.0 - PeerPhase::SLEW as f32) - 1e-4, "row stalled/reversed ({s}) at {} ms", w[1].0);
            assert!(s <= inc * (1.0 + PeerPhase::SLEW as f32) + 1e-4, "row jumped ({s}) at {} ms", w[1].0);
        }
    }

    /// One packet 40 ms late (0.08 beat) asks for a quarter of that, 0.02
    /// beat, and that is all the row ends up moved by once it has bled in.
    #[test]
    fn a_packet_forty_ms_late_moves_phase_by_a_quarter_of_the_error() {
        let mut pp = PeerPhase::new();
        let mut arrivals: Vec<u64> = (0..10).map(|i| 1000 + i * 500).collect();   // on time, to 5500
        arrivals.push(6040);                                                    // the 6 s beat, 40 ms late
        let frames = run(&mut pp, 1000, 8000, &arrivals, true);                 // status PLAY keeps it running
        let free_run = (frames.len() as f64 - 1.0) * DT / 0.5;                  // beats a pure free-run adds (the first frame only initialises)
        let moved = pp.phase - free_run;                                        // started at phase 0
        let want = -0.08 * PeerPhase::PULL;
        assert!((moved - want).abs() < 0.003, "moved {moved:.4}, expected {want:.4}");
        assert!(pp.pending.abs() < 1e-6, "correction not fully bled in: {}", pp.pending);
    }

    #[test]
    fn a_paused_peer_holds_after_two_periods() {
        let mut pp = PeerPhase::new();
        let arrivals: Vec<u64> = (0..6).map(|i| 1000 + i * 500).collect();   // last beat at 3500
        run(&mut pp, 1000, 3600, &arrivals, true);
        // Status now says paused, no more beats: after two periods it must hold.
        let frames = run(&mut pp, 3600, 6000, &arrivals, false);
        let held: Vec<_> = frames.iter().filter(|(t, _)| *t > 4600).map(|(_, p)| *p).collect();
        assert!(held.windows(2).all(|w| w[0] == w[1]), "row still moving while the peer is paused");
    }

    #[test]
    fn a_playing_peer_with_late_beats_keeps_running() {
        let mut pp = PeerPhase::new();
        let arrivals: Vec<u64> = (0..6).map(|i| 1000 + i * 500).collect();
        run(&mut pp, 1000, 3600, &arrivals, true);
        // Status says PLAY, beats simply stop arriving for 3 s: keep running.
        let frames = run(&mut pp, 3600, 6600, &arrivals, true);
        assert!(frames.windows(2).all(|w| step(w[0].1, w[1].1) > 0.0));
    }

    #[test]
    fn twin_beats_within_100ms_are_dropped_and_real_ones_kept() {
        let mut last = HashMap::new();
        let t0 = Instant::now();
        assert!(!beat_is_twin(&mut last, 3, t0));
        assert!(beat_is_twin(&mut last, 3, t0 + Duration::from_millis(5)));    // the unicast copy
        assert!(!beat_is_twin(&mut last, 4, t0 + Duration::from_millis(5)));   // another player
        assert!(!beat_is_twin(&mut last, 3, t0 + Duration::from_millis(300))); // the next beat (200 BPM)
    }
}

#[cfg(test)]
mod sync_offset_tests {
    use super::{phase_err, PhaseAct, SyncPhase};
    use super::{broadcast_for, heard_on};
    use std::net::{Ipv4Addr, SocketAddr};

    #[test]
    fn broadcast_is_derived_when_the_interface_reports_none() {
        let ip = Ipv4Addr::new(169, 254, 77, 1);
        let m16 = Ipv4Addr::new(255, 255, 0, 0);
        // `ip addr add 169.254.77.1/16` with no brd: none, or our own address.
        assert_eq!(broadcast_for(ip, m16, None), Some(Ipv4Addr::new(169, 254, 255, 255)));
        assert_eq!(broadcast_for(ip, m16, Some(ip)), Some(Ipv4Addr::new(169, 254, 255, 255)));
        assert_eq!(broadcast_for(ip, m16, Some(Ipv4Addr::UNSPECIFIED)), Some(Ipv4Addr::new(169, 254, 255, 255)));
        // A reported broadcast is trusted.
        let lan = Ipv4Addr::new(192, 168, 68, 64);
        let b = Ipv4Addr::new(192, 168, 71, 255);
        assert_eq!(broadcast_for(lan, Ipv4Addr::new(255, 255, 252, 0), Some(b)), Some(b));
        // A host route has nowhere to broadcast.
        assert_eq!(broadcast_for(ip, Ipv4Addr::BROADCAST, None), None);
    }

    #[test]
    fn a_pinned_interface_hears_only_its_subnet() {
        let at = |a: [u8; 4]| SocketAddr::from((Ipv4Addr::from(a), 50000));
        let pin = Some((Ipv4Addr::new(169, 254, 77, 1), Ipv4Addr::new(255, 255, 0, 0)));
        assert!(heard_on(pin, at([169, 254, 5, 55])), "the emulated CDJ on the bridge");
        assert!(!heard_on(pin, at([192, 168, 68, 20])), "a deck on the LAN is ignored");
        assert!(heard_on(pin, at([127, 0, 0, 1])), "loopback still works");
        assert!(heard_on(None, at([192, 168, 68, 20])), "unpinned hears everything");
    }

    /// Synced, no jog: the classic pull toward the master's beat.
    #[test]
    fn without_a_nudge_sync_pulls_to_the_beat() {
        let mut p = SyncPhase::default();
        assert!(!p.tick(true, false));
        match p.on_master_beat(0.1, 0.0, false) {
            PhaseAct::Correct(o) => assert!(o < 0.0, "ahead → slow down, got {o}"),
            other => panic!("{other:?}"),
        }
        match p.on_master_beat(0.9, 0.0, false) {
            PhaseAct::Correct(o) => assert!(o > 0.0, "behind → speed up, got {o}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(p.on_master_beat(0.005, 0.0, false), PhaseAct::None, "inside the dead band");
    }

    /// The CDJ behaviour SHIFT GRID depends on: a nudge while synced is kept.
    #[test]
    fn a_nudge_while_synced_is_held_not_undone() {
        let mut p = SyncPhase::default();
        // The jog nudges for a few beats: SYNC keeps its hands off.
        for frac in [0.05, 0.12, 0.2] {
            p.tick(true, true);
            assert_eq!(p.on_master_beat(frac, 0.0, true), PhaseAct::None);
        }
        // Wheel settles; the next master beat measures where it landed.
        p.tick(true, false);
        assert_eq!(p.on_master_beat(0.2, 0.0, false), PhaseAct::Hold(0.2));
        // Sitting at the held offset is on target: no pull back to 0.
        assert_eq!(p.on_master_beat(0.2, 0.2, false), PhaseAct::None);
        // Drifting off it corrects toward the offset, not toward 0.
        match p.on_master_beat(0.3, 0.2, false) {
            PhaseAct::Correct(o) => assert!(o < 0.0),
            other => panic!("{other:?}"),
        }
        match p.on_master_beat(0.1, 0.2, false) {
            PhaseAct::Correct(o) => assert!(o > 0.0, "0.1 is behind a 0.2 hold"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_backward_nudge_holds_a_negative_offset() {
        let mut p = SyncPhase::default();
        p.tick(true, true);
        p.tick(true, false);
        match p.on_master_beat(0.85, 0.0, false) {
            PhaseAct::Hold(h) => assert!((h + 0.15).abs() < 1e-9, "0.85 into the beat is 0.15 early, got {h}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sync_off_clears_the_hold_and_forgets_a_pending_nudge() {
        let mut p = SyncPhase::default();
        p.tick(true, true);                // nudging...
        assert!(p.tick(false, false), "SYNC off asks to clear the offset");
        p.tick(true, false);               // SYNC back on, no new nudge
        assert!(matches!(p.on_master_beat(0.2, 0.0, false), PhaseAct::Correct(_)),
                "no stale Hold from the nudge before SYNC went off");
    }

    /// Closed loop: each master beat applies the correction for one beat
    /// (a speed offset of f moves the phase f beats), plus some drift. SYNC
    /// must settle on the held offset, from either side.
    #[test]
    fn closed_loop_settles_on_the_held_offset() {
        for (start, held, drift) in [(0.5, 0.2, 0.0), (0.0, 0.2, 0.004), (0.9, -0.15, -0.003)] {
            let mut p = SyncPhase::default();
            let mut frac: f64 = start;
            for _ in 0..80 {
                p.tick(true, false);
                let corr = match p.on_master_beat(frac.rem_euclid(1.0), held, false) {
                    PhaseAct::Correct(o) => o as f64,
                    PhaseAct::None => 0.0,
                    PhaseAct::Hold(_) => unreachable!(),
                };
                frac += corr + drift;
            }
            let err = phase_err(frac.rem_euclid(1.0), held);
            assert!(err.abs() < 0.02, "start {start} hold {held} drift {drift}: ended {err:+.3} off");
        }
    }

    #[test]
    fn phase_err_wraps_around_the_held_offset() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // No offset: just past a beat is +, just before the next is −.
        assert!(close(phase_err(0.1, 0.0), 0.1));
        assert!(close(phase_err(0.9, 0.0), -0.1));
        // Holding +0.2: sitting at 0.2 is on target; 0.1 is 0.1 behind it.
        assert!(close(phase_err(0.2, 0.2), 0.0));
        assert!(close(phase_err(0.1, 0.2), -0.1));
        // Holding −0.2 (0.8 in beat terms) wraps across the beat.
        assert!(close(phase_err(0.8, -0.2), 0.0));
        assert!(close(phase_err(0.05, -0.2), 0.25));
        // Always within half a beat.
        for i in 0..100 {
            let f = i as f64 / 100.0;
            assert!(phase_err(f, 0.37).abs() <= 0.5 + 1e-9);
        }
    }
}

#[cfg(test)]
mod position_target_tests {
    use super::position_targets;
    use std::collections::HashMap;
    use std::net::Ipv4Addr;

    /// Like a CDJ-3000: every announced device gets the position packet
    /// unicast, a tool or mixer included, but not we ourselves, and an
    /// address shared by two numbers only once.
    #[test]
    fn every_other_device_once() {
        let a = Ipv4Addr::new(169, 254, 5, 55);
        let tool = Ipv4Addr::new(169, 254, 9, 9);
        let me = Ipv4Addr::new(169, 254, 77, 1);
        let peers = HashMap::from([(1u8, a), (2, me), (0x21, tool), (5, a)]);
        assert_eq!(position_targets(&peers, 2), vec![a, tool]);
        assert!(position_targets(&HashMap::from([(2u8, me)]), 2).is_empty(), "alone: nothing sent, as a CDJ");
    }
}

#[cfg(test)]
mod loop_field_tests {
    use super::loop_fields;
    use opendeck_link::prodj::ActiveLoop;

    /// A deck loop in stereo samples at 48 kHz becomes ms, with its length
    /// in beats when whole (4 beats at 120 BPM), else 0.
    #[test]
    fn deck_loop_to_status_loop() {
        let s = |secs: f64| (secs * 48_000.0) as u64 * 2;
        assert_eq!(loop_fields(s(1.0), s(3.0), 2, 48_000, Some(120.0)), Some(ActiveLoop { start_ms: 1000.0, end_ms: 3000.0, beats: 4 }));
        assert_eq!(loop_fields(s(1.0), s(1.3), 2, 48_000, Some(120.0)).unwrap().beats, 0, "not whole beats");
        assert_eq!(loop_fields(s(2.0), s(1.0), 2, 48_000, None), None, "no loop");
    }
}

#[cfg(test)]
mod position_rx_tests {
    use super::*;

    /// A peer's position packet is kept (ours ignored) and its playhead runs
    /// forward at its pitch while it plays; stale after a second.
    #[test]
    fn keeps_peer_positions() {
        let link = LinkState::new(2);
        let t0 = Instant::now();
        assert!(note_position(&link, &ProDjLink::new(1).build_abs_position(220, 25_000, 1000, Some(1386)), t0));
        assert!(note_position(&link, &ProDjLink::new(2).build_abs_position(10, 5, 0, None), t0));
        assert!(!note_position(&link, &ProDjLink::new(1).build_announce(Ipv4Addr::new(169, 254, 1, 1), [0; 6]), t0), "not a position packet");
        let at = |ms: u64, playing: bool| link.playhead_ms(1, playing, t0 + Duration::from_millis(ms));
        assert_eq!(at(0, true), Some(25_000.0));
        assert!((at(100, true).unwrap() - 25_110.0).abs() < 0.01, "+10 %: 110 ms in 100");
        assert_eq!(at(100, false), Some(25_000.0));
        assert_eq!(at(1500, true), None, "stale");
        assert_eq!(link.playhead_ms(2, true, t0), None, "our own is not kept");
    }
}

#[cfg(test)]
mod playhead_estimate_tests {
    use super::PlayheadEstimate;

    const SR: u32 = 48_000;

    /// A deck looping `[a, b)` (frames) at normal speed: the decoder cursor
    /// runs `ahead` frames in front of what is heard, in 512-frame blocks,
    /// and wraps at `b`.  Returns every estimate (1 ms ticks) and the wraps.
    fn run_loop(a: u64, b: u64, ahead: u64, secs: f64) -> (Vec<f64>, usize) {
        let mut est = PlayheadEstimate::default();
        let step = SR as f64 / 1000.0;
        let (mut heard, mut out, mut wraps) = (a as f64, Vec::new(), 0);
        for _ in 0..(secs * 1000.0) as usize {
            heard += step;
            if heard >= b as f64 { heard -= (b - a) as f64; }
            // The cursor: heard + ahead, wrapped into the loop, on a block.
            let mut cur = heard + ahead as f64;
            while cur >= b as f64 { cur -= (b - a) as f64; }
            let cur = ((cur as u64) / 512) * 512;
            if est.update(cur.max(a), ahead as f64, Some((a, b)), step, SR) { wraps += 1; }
            out.push(est.frames);
        }
        (out, wraps)
    }

    #[test]
    fn a_short_loop_keeps_the_link_position_inside_it() {
        // A half-beat loop at 128 BPM (0.23 s), well under the old 0.5 s snap.
        let (a, b) = (480_000, 480_000 + 11_250);
        let (est, wraps) = run_loop(a, b, 4_000, 3.0);
        assert!(wraps >= 12, "went round the loop: {wraps} wraps in 3 s");
        for (i, f) in est.iter().enumerate().skip(50) {
            assert!(*f >= a as f64 - 1.0 && *f < b as f64 + 50.0, "tick {i}: {f} outside {a}..{b}");
        }
    }

    #[test]
    fn a_long_loop_wraps_without_dipping_before_its_start() {
        // A 4-beat loop at 120 BPM (2 s), with ~90 ms queued: when the cursor
        // wraps, the old reference read back before the loop start.
        let (a, b) = (960_000, 960_000 + 96_000);
        let (est, wraps) = run_loop(a, b, 4_300, 7.0);
        assert!(wraps >= 3, "{wraps} wraps");
        assert!(est.iter().skip(50).all(|f| *f >= a as f64 - 1.0), "never before the loop start");
    }

    /// The step to a beat that is also the loop's end lands on the loop's
    /// start: the beat sent is the loop's first, not the one after the loop
    /// (a stray "beat 3" then "beat 2" in the same instant, seen on Deckbench).
    #[test]
    fn stepping_onto_the_loop_end_lands_on_its_start() {
        let mut est = PlayheadEstimate { frames: 10_000.0 - 30.0, ..Default::default() };
        assert!(est.advance(30.0, Some((7_000, 10_000))));
        assert_eq!(est.frames, 7_000.0);
        assert!(!est.advance(30.0, Some((7_000, 10_000))));
        assert!(!est.advance(5_000.0, None));
        // A loop end a few frames past the grid's beat: the step to that
        // beat is the loop going round, not the next beat.
        let mut est = PlayheadEstimate { frames: 10_000.0 - 30.0, ..Default::default() };
        assert!(est.advance(30.0, Some((7_000, 10_003))));
        assert_eq!(est.frames, 7_000.0);
    }

    #[test]
    fn a_hair_before_a_beat_is_on_it() {
        use super::on_beat_line;
        assert_eq!(on_beat_line(17.9996).floor(), 18.0, "a loop start a frame before beat 18 is beat 18");
        assert_eq!(on_beat_line(18.0004).floor(), 18.0);
        assert_eq!(on_beat_line(17.98).floor(), 17.0, "a real position before the beat stays before it");
    }

    #[test]
    fn without_a_loop_it_follows_the_track() {
        let mut est = PlayheadEstimate::default();
        let step = SR as f64 / 1000.0;
        for i in 1..=2000u64 {
            let heard = 100_000 + i * 48;
            est.update(heard + 4_000, 4_000.0, None, step, SR);
        }
        assert!((est.frames - (100_000.0 + 2000.0 * 48.0)).abs() < 600.0, "{}", est.frames);
    }
}

#[cfg(test)]
mod loopback_tests {
    #[test]
    fn loopback_link_broadcasts_to_the_whole_loopback() {
        let (ip, bc, _, name) = super::loopback_link();
        assert_eq!((ip.to_string(), bc.to_string(), name.as_str()), ("127.0.0.1".into(), "127.255.255.255".into(), "lo"));
    }
}

/// The beats OpenDeck sends over Link while it plays and loops, from a
/// simulated deck at 1 ms ticks: the cursor runs ~85 ms ahead in 512-frame
/// blocks, loop points are whole frames (as quantize makes them) and the
/// grid is 127 BPM, so beat lines fall between frames.  Each glitch here was
/// first seen on Deckbench (tools/deckbench/loop_beats.py).
#[cfg(test)]
mod beat_clock_tests {
    use super::*;

    const SR: u32 = 48_000;
    const AHEAD: f64 = 4_096.0;

    fn grid() -> BeatGrid { BeatGrid::new_constant(1_234, 127.0) }
    /// Grid beat `n`'s position, in frames (not a whole number).
    fn line(n: i64) -> f64 { 1_234.0 + n as f64 * SR as f64 * 60.0 / 127.0 }

    /// Play from `from` for `secs`; loop `[a, b)` from `loop_at` on, if
    /// given.  Every beat sent: (ms, grid beat).
    fn play(from: f64, secs: f64, lp: Option<(u64, u64)>, loop_at: f64) -> Vec<(usize, i64)> {
        let g = grid();
        let mut clock = BeatClock::default();
        let step = SR as f64 / 1000.0;
        let mut heard = from;
        let mut sent = Vec::new();
        for ms in 0..(secs * 1000.0) as usize {
            let lp_now = lp.filter(|_| ms as f64 / 1000.0 >= loop_at);
            heard += step;
            let mut cur = heard + AHEAD;
            if let Some((a, b)) = lp_now {
                let (a, b) = (a as f64, b as f64);
                if heard >= b { heard -= b - a; }
                while cur >= b { cur -= b - a; }
            }
            let cur = (cur as u64 / 512) * 512;
            let to_next = clock.tick(&g, SR, cur, AHEAD, lp_now, step, SR as f64);
            let landed = (to_next > 0.0 && to_next < 0.0015).then(|| to_next * SR as f64);
            if let Some(b) = clock.finish(&g, SR, landed, lp_now) { sent.push((ms, b)); }
        }
        sent
    }

    #[test]
    fn playing_sends_each_beat_once_in_order() {
        let sent = play(line(10) + 1_000.0, 5.0, None, 0.0);
        let beats: Vec<i64> = sent.iter().map(|s| s.1).skip(1).collect();
        assert!(beats.windows(2).all(|w| w[1] == w[0] + 1), "{beats:?}");
        assert!((9..=11).contains(&beats.len()), "{} beats in 5 s at 127 BPM", beats.len());
    }

    #[test]
    fn a_one_beat_loop_sends_its_beat_once_each_time_round() {
        // Beat 17 looped, its points rounded down to whole frames.
        let (a, b) = (line(17) as u64, line(18) as u64);
        let sent = play(line(15), 9.0, Some((a, b)), 1.5);
        let in_loop: Vec<_> = sent.iter().filter(|s| s.0 > 2_000).collect();
        assert!(in_loop.iter().all(|s| s.1 == 17), "only beat 17 while looping it (no stray 16 or 18): {in_loop:?}");
        assert!(in_loop.windows(2).all(|w| w[1].0 - w[0].0 > 300), "one beat each time round, never two together: {in_loop:?}");
        assert!((13..=15).contains(&in_loop.len()), "{} wraps in ~7 s of a 0.47 s loop", in_loop.len());
    }

    #[test]
    fn a_half_beat_loop_keeps_sending_its_beat() {
        // Half a beat inside beat 20: under the old 0.5 s snap, so the
        // estimate used to run on through the song.
        let a = line(20) as u64;
        let b = (line(20) + (line(21) - line(20)) / 2.0) as u64;
        let sent = play(line(18), 6.0, Some((a, b)), 1.0);
        let in_loop: Vec<_> = sent.iter().filter(|s| s.0 > 1_600).collect();
        assert!(in_loop.iter().all(|s| s.1 == 20), "{in_loop:?}");
        assert!(in_loop.len() >= 15, "a beat each time round: {}", in_loop.len());
    }

    #[test]
    fn a_four_beat_loop_counts_its_four_beats_round_and_round() {
        let (a, b) = (line(40) as u64, line(44) as u64);
        let sent = play(line(38), 10.0, Some((a, b)), 1.0);
        let in_loop: Vec<i64> = sent.iter().filter(|s| s.0 > 1_400).map(|s| s.1).collect();
        assert!(in_loop.iter().all(|b| (40..44).contains(b)), "{in_loop:?}");
        for w in in_loop.windows(2) {
            assert!(w[1] == w[0] + 1 || (w[0] == 43 && w[1] == 40), "40 41 42 43 40 …: {in_loop:?}");
        }
    }
}

/// The three Link listeners, fed packets as the players send them: master
/// tracking and handoff, the beat clock SYNC follows, sync control, media
/// queries, positions and the peer table.  The listeners' threads only add
/// a socket around these handlers.
#[cfg(test)]
mod listener_tests {
    use super::*;
    use opendeck_link::prodj::TrackRef;

    const ME: u8 = 3;
    const CDJ: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 20);
    const XDJ: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 30);
    fn from(ip: Ipv4Addr, port: u16) -> SocketAddr { SocketAddr::from((ip, port)) }

    fn status_rx(link: &Arc<LinkState>) -> StatusRx {
        StatusRx { link: Arc::clone(link), beat2_player: Arc::new(AtomicU32::new(0)), last: HashMap::new() }
    }
    fn beat_rx(link: &Arc<LinkState>) -> BeatRx {
        BeatRx {
            link: Arc::clone(link),
            beat2_bpm: Arc::new(AtomicU32::new(0)), beat2_anchor: Arc::new(AtomicU64::new(0)),
            beat2_player: Arc::new(AtomicU32::new(0)), beat2_bib: Arc::new(AtomicU32::new(0)),
            last_beat_at: HashMap::new(),
        }
    }
    /// A playing deck's status, with a track at `bpm` and `pitch`.
    fn fields(master: bool, bpm: f32, pitch: f32, handoff_to: Option<u8>, sync_counter: u32) -> StatusFields {
        StatusFields {
            track: Some(TrackRef { player: 2, slot: 3, kind: 1, id: 7 }), serving: false,
            playing: true, track_loaded: true, master, sync: false, on_air: true,
            pitch, bpm: Some(bpm), beat: Some(17), beat_in_bar: Some(1),
            handoff_to, counter: 1, sync_counter,
        }
    }
    fn status(player: u8, f: StatusFields) -> Vec<u8> { ProDjLink::new(player).build_status(&f) }
    fn beat(player: u8, bpm: f32, speed: f32) -> Vec<u8> {
        let snap = EngineSnapshot {
            position: 0, ghost_position: 0, speed, bpm: bpm * speed, beat_phase: 0.0, bar_phase: 0.0,
            is_playing: true, slip_active: false, key_lock: false, deck_id: player, timestamp_ns: 0,
        };
        ProDjLink::new(player).build_beat(&snap, 2)
    }
    fn bpm_of(a: &AtomicU32) -> f32 { f32::from_bits(a.load(Ordering::Relaxed)) }

    // ── Status (50002) ─────────────────────────────────────────────────────

    #[test]
    fn a_peer_claiming_master_is_tracked_with_its_pitched_tempo() {
        let link = LinkState::new(ME);
        let mut rx = status_rx(&link);
        rx.on_packet(&status(2, fields(true, 128.0, 1.02, None, 5)), from(CDJ, PORT_STATUS));
        assert_eq!(link.master_player.load(Ordering::Relaxed), 2);
        assert!((bpm_of(&link.master_bpm) - 130.56).abs() < 0.01, "track BPM × pitch: {}", bpm_of(&link.master_bpm));
        assert_eq!(link.largest_sync.load(Ordering::Relaxed), 5, "the largest sync counter on the network");
        assert!(!link.master.load(Ordering::Relaxed), "seeing a master does not make us one");

        rx.on_packet(&status(2, fields(false, 128.0, 1.02, None, 5)), from(CDJ, PORT_STATUS));
        assert_eq!(link.master_player.load(Ordering::Relaxed), 0, "it released the role");
    }

    #[test]
    fn our_own_status_is_ignored() {
        let link = LinkState::new(ME);
        let mut rx = status_rx(&link);
        rx.on_packet(&status(ME, fields(true, 128.0, 1.0, None, 9)), from(CDJ, PORT_STATUS));
        assert_eq!(link.master_player.load(Ordering::Relaxed), 0);
        assert_eq!(link.largest_sync.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_master_naming_us_in_its_handoff_byte_hands_us_the_role() {
        let link = LinkState::new(ME);
        link.want_master.store(true, Ordering::Relaxed);
        link.yielded_from.store(2, Ordering::Relaxed);
        let mut rx = status_rx(&link);
        rx.on_packet(&status(2, fields(true, 128.0, 1.0, None, 4)), from(CDJ, PORT_STATUS));
        assert!(!link.master.load(Ordering::Relaxed), "an agreement to yield is not the handoff");

        rx.on_packet(&status(2, fields(true, 128.0, 1.0, Some(ME), 4)), from(CDJ, PORT_STATUS));
        assert!(link.master.load(Ordering::Relaxed), "Mh = us: take master");
        assert_eq!(link.our_sync.load(Ordering::Relaxed), 5, "a sync counter above everything seen");
        assert!(!link.want_master.load(Ordering::Relaxed) && link.yielded_from.load(Ordering::Relaxed) == 0, "the request is done");

        // The old master keeps MASTER, with a higher counter, until it sees
        // ours: that is not a claim against us (the abdication bug).
        rx.on_packet(&status(2, fields(true, 128.0, 1.0, None, 9)), from(CDJ, PORT_STATUS));
        assert!(link.master.load(Ordering::Relaxed), "the handshake decides mastery, not the counter");
        rx.on_packet(&status(2, fields(false, 128.0, 1.0, None, 9)), from(CDJ, PORT_STATUS));
        assert!(link.master.load(Ordering::Relaxed));
        assert_eq!(link.master_player.load(Ordering::Relaxed), 0);
    }

    /// MASTER pressed while we are master, as a CDJ-3000 does it (seen on two
    /// emulated units): the role goes to a playing player through the
    /// handoff byte, and we drop it once that player claims it.  With no
    /// other player playing, we stay master.
    #[test]
    fn master_pressed_while_master_hands_the_role_to_a_playing_player() {
        let link = LinkState::new(ME);
        link.master.store(true, Ordering::Relaxed);
        let mut rx = status_rx(&link);
        let stopped = |p| StatusFields { playing: false, ..fields(false, 128.0, 1.0, None, 1) };
        rx.on_packet(&status(2, stopped(2)), from(CDJ, PORT_STATUS));
        assert_eq!(link.hand_off_master("test"), None, "nobody playing: keep it");
        assert!(link.master.load(Ordering::Relaxed));
        assert_eq!(link.handoff_to.load(Ordering::Relaxed), 0);

        // Two players playing: the role goes to one of them (the lowest
        // number; which one the unit picks is not known yet).
        rx.on_packet(&status(4, fields(false, 126.0, 1.0, None, 1)), from(XDJ, PORT_STATUS));
        rx.on_packet(&status(2, fields(false, 128.0, 1.0, None, 1)), from(CDJ, PORT_STATUS));
        assert_eq!(link.hand_off_master("test"), Some(2));
        assert_eq!(link.handoff_to.load(Ordering::Relaxed), 2, "our status names it");
        assert!(link.master.load(Ordering::Relaxed), "we keep the flag until it claims the role");

        rx.on_packet(&status(2, fields(true, 128.0, 1.0, None, 2)), from(CDJ, PORT_STATUS));
        assert!(!link.master.load(Ordering::Relaxed), "handed over");
        assert_eq!(link.handoff_to.load(Ordering::Relaxed), 0);
        assert_eq!(link.master_player.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_peer_we_yielded_to_takes_the_role_from_us() {
        let link = LinkState::new(ME);
        link.master.store(true, Ordering::Relaxed);
        let mut beats = beat_rx(&link);
        let replies = beats.on_packet(&ProDjLink::new(2).build_master_request(), from(CDJ, 50111), Instant::now());
        assert_eq!(link.handoff_to.load(Ordering::Relaxed), 2);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].1, from(CDJ, PORT_BEAT), "the answer goes to the requester's beat port");
        assert_eq!(ProDjLink::parse_master_response(&replies[0].0), Some((ME, true)));

        let mut rx = status_rx(&link);
        rx.on_packet(&status(2, fields(true, 128.0, 1.0, None, 6)), from(CDJ, PORT_STATUS));
        assert!(!link.master.load(Ordering::Relaxed), "handed over");
        assert_eq!(link.handoff_to.load(Ordering::Relaxed), 0);
        assert_eq!(link.master_player.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn the_phase_row_follows_the_first_deck_heard_and_its_play_state() {
        let link = LinkState::new(ME);
        let mut rx = status_rx(&link);
        rx.on_packet(&status(2, fields(false, 128.0, 1.0, None, 0)), from(CDJ, PORT_STATUS));
        let mut paused = fields(false, 124.0, 1.0, None, 0);
        paused.playing = false;
        rx.on_packet(&status(1, paused), from(XDJ, PORT_STATUS));
        assert_eq!(rx.beat2_player.load(Ordering::Relaxed), 2, "the first deck that sent status");
        assert!(link.beat2_playing.load(Ordering::Relaxed), "its PLAY, not the other deck's");
        rx.on_packet(&status(2, paused), from(CDJ, PORT_STATUS));
        assert!(!link.beat2_playing.load(Ordering::Relaxed));
    }

    #[test]
    fn a_media_query_is_answered_only_while_serving_and_for_our_number() {
        let link = LinkState::new(ME);
        let mut rx = status_rx(&link);
        let ask = |target, slot| ProDjLink::new(2).build_media_query(CDJ, target, slot);
        assert!(rx.on_packet(&ask(ME, 3), from(CDJ, PORT_STATUS)).is_empty(), "nothing served yet");

        link.serve_tracks.store(42, Ordering::Relaxed);
        assert!(rx.on_packet(&ask(1, 3), from(CDJ, PORT_STATUS)).is_empty(), "another player's slot");
        assert!(rx.on_packet(&ask(ME, 2), from(CDJ, PORT_STATUS)).is_empty(), "our SD: we never report one");
        let r = rx.on_packet(&ask(ME, 3), from(CDJ, PORT_STATUS));
        assert_eq!(r.len(), 1, "same address inside and outside: one answer");
        assert_eq!(r[0].1, from(CDJ, PORT_STATUS));
        let info = ProDjLink::parse_media_response(&r[0].0).expect("a media response");
        assert_eq!((info.player, info.tracks), (ME, 42));

        // Across a routed link the packet names the peer's LAN address; the
        // answer goes there and to where it came from.
        let r = rx.on_packet(&ask(ME, 3), from(Ipv4Addr::new(100, 64, 0, 9), 50002));
        let to: Vec<SocketAddr> = r.iter().map(|x| x.1).collect();
        assert_eq!(to, vec![from(CDJ, PORT_STATUS), from(Ipv4Addr::new(100, 64, 0, 9), PORT_STATUS)]);
    }

    #[test]
    fn a_players_media_answer_lists_its_stick() {
        let link = LinkState::new(ME);
        let mut rx = status_rx(&link);
        let resp = ProDjLink::new(2).build_media_response(Ipv4Addr::new(192, 168, 1, 5), 3, "DAN USB", 226, 4, 32 << 30, 1 << 30);
        assert!(rx.on_packet(&resp, from(CDJ, PORT_STATUS)).is_empty());
        let media = link.media(2);
        assert_eq!(media.len(), 1);
        assert_eq!((media[0].slot, media[0].tracks, media[0].playlists), (3, 226, 4));
        assert_eq!(media[0].name, "DAN USB");
        assert!(link.media(1).is_empty());
    }

    // ── Beats (50001) ──────────────────────────────────────────────────────

    #[test]
    fn the_masters_beats_drive_the_sync_clock_at_its_pitched_tempo() {
        let link = LinkState::new(ME);
        link.master_player.store(2, Ordering::Relaxed);
        let mut rx = beat_rx(&link);
        let t0 = Instant::now();
        rx.on_packet(&beat(2, 128.0, 1.02), from(CDJ, PORT_BEAT), t0);
        assert!((bpm_of(&link.master_bpm) - 130.56).abs() < 0.02, "{}", bpm_of(&link.master_bpm));
        assert_eq!(link.master_beat_seq.load(Ordering::Relaxed), 1);
        assert_eq!(rx.beat2_player.load(Ordering::Relaxed), 2);
        assert_eq!(rx.beat2_bib.load(Ordering::Relaxed), 2);

        // Its unicast twin a few ms later is the same beat.
        rx.on_packet(&beat(2, 128.0, 1.02), from(CDJ, PORT_BEAT), t0 + Duration::from_millis(4));
        assert_eq!(link.master_beat_seq.load(Ordering::Relaxed), 1, "twin dropped");
        rx.on_packet(&beat(2, 128.0, 1.02), from(CDJ, PORT_BEAT), t0 + Duration::from_millis(460));
        assert_eq!(link.master_beat_seq.load(Ordering::Relaxed), 2, "the next beat counts");
    }

    #[test]
    fn a_deck_that_is_not_master_does_not_move_the_sync_clock() {
        let link = LinkState::new(ME);
        link.master_player.store(2, Ordering::Relaxed);
        let mut rx = beat_rx(&link);
        rx.on_packet(&beat(1, 140.0, 1.0), from(XDJ, PORT_BEAT), Instant::now());
        assert_eq!(link.master_beat_seq.load(Ordering::Relaxed), 0);
        assert_eq!(bpm_of(&link.master_bpm), 0.0);
        assert_eq!(rx.beat2_player.load(Ordering::Relaxed), 0, "the row shows the master");
        rx.on_packet(&beat(ME, 140.0, 1.0), from(XDJ, PORT_BEAT), Instant::now());
        assert_eq!(rx.beat2_player.load(Ordering::Relaxed), 0, "our own beat broadcast");

        // With no master, the row follows whoever plays.
        link.master_player.store(0, Ordering::Relaxed);
        rx.on_packet(&beat(1, 140.0, 1.0), from(XDJ, PORT_BEAT), Instant::now() + Duration::from_secs(1));
        assert_eq!(rx.beat2_player.load(Ordering::Relaxed), 1);
        assert!((bpm_of(&rx.beat2_bpm) - 140.0).abs() < 0.01);
    }

    #[test]
    fn a_master_request_is_ignored_when_we_are_not_master() {
        let link = LinkState::new(ME);
        let mut rx = beat_rx(&link);
        assert!(rx.on_packet(&ProDjLink::new(2).build_master_request(), from(CDJ, 50001), Instant::now()).is_empty());
        assert_eq!(link.handoff_to.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_yield_is_recorded_only_while_we_asked_for_master() {
        let link = LinkState::new(ME);
        let mut rx = beat_rx(&link);
        let yes = ProDjLink::new(2).build_master_response(true);
        rx.on_packet(&yes, from(CDJ, PORT_BEAT), Instant::now());
        assert_eq!(link.yielded_from.load(Ordering::Relaxed), 0, "we never asked");
        link.want_master.store(true, Ordering::Relaxed);
        rx.on_packet(&ProDjLink::new(2).build_master_response(false), from(CDJ, PORT_BEAT), Instant::now());
        assert_eq!(link.yielded_from.load(Ordering::Relaxed), 0, "a refusal");
        rx.on_packet(&yes, from(CDJ, PORT_BEAT), Instant::now());
        assert_eq!(link.yielded_from.load(Ordering::Relaxed), 2);
        assert!(!link.master.load(Ordering::Relaxed), "agreement is not the handoff: wait for Mh");
    }

    #[test]
    fn another_player_can_turn_our_sync_on_and_off_and_make_us_master() {
        let link = LinkState::new(ME);
        let mut rx = beat_rx(&link);
        let cmd = |target, c| ProDjLink::new(2).build_sync_control(target, c);
        rx.on_packet(&cmd(1, SYNC_ON), from(CDJ, PORT_BEAT), Instant::now());
        assert!(!link.sync.load(Ordering::Relaxed), "meant for player 1");
        rx.on_packet(&cmd(ME, SYNC_ON), from(CDJ, PORT_BEAT), Instant::now());
        assert!(link.sync.load(Ordering::Relaxed));
        rx.on_packet(&cmd(ME, SYNC_OFF), from(CDJ, PORT_BEAT), Instant::now());
        assert!(!link.sync.load(Ordering::Relaxed));
        rx.on_packet(&cmd(ME, BECOME_MASTER), from(CDJ, PORT_BEAT), Instant::now());
        assert!(link.want_master.load(Ordering::Relaxed));
    }

    #[test]
    fn a_cdj3000s_position_runs_forward_at_its_pitch() {
        let link = LinkState::new(ME);
        let mut rx = beat_rx(&link);
        let t0 = Instant::now();
        rx.on_packet(&ProDjLink::new(2).build_abs_position(180, 60_000, 200, Some(1305)), from(CDJ, PORT_BEAT), t0);
        let at = |ms: u64, playing| link.playhead_ms(2, playing, t0 + Duration::from_millis(ms)).map(|p| p.round());
        assert_eq!(at(0, true), Some(60_000.0));
        assert_eq!(at(500, true), Some(60_510.0), "+2 %: 500 ms of wall clock is 510 ms of track");
        assert_eq!(at(500, false), Some(60_000.0), "paused: held");
        assert_eq!(at(1500, true), None, "stale after a second");
        rx.on_packet(&ProDjLink::new(ME).build_abs_position(180, 1, 0, None), from(CDJ, PORT_BEAT), t0);
        assert_eq!(link.playhead_ms(ME, true, t0), None, "our own position is not a peer's");
    }

    // ── Announces (50000) ──────────────────────────────────────────────────

    #[test]
    fn announces_fill_the_peer_table_at_the_address_they_came_from() {
        let link = LinkState::new(ME);
        let mut rx = AnnounceRx { link: Arc::clone(&link), guard: ClaimGuard::default() };
        rx.on_packet(&ProDjLink::new(2).build_announce(CDJ, [0; 6]), from(CDJ, PORT_ANNOUNCE));
        // Unicast across a routed link: the packet carries the LAN address.
        let routed = Ipv4Addr::new(100, 64, 0, 9);
        rx.on_packet(&ProDjLink::new(1).build_announce(XDJ, [0; 6]), from(routed, PORT_ANNOUNCE));
        assert_eq!(link.peer_ip(2), Some(CDJ));
        assert_eq!(link.peer_ip(1), Some(routed), "reach it where it came from");
        assert_eq!(link.peer_player(CDJ), Some(2));
        assert!(link.peers_summary().contains("2 freedj-3000 192.168.1.20"), "{}", link.peers_summary());
        assert_eq!(link.opendeck_peers().len(), 2, "both announce as OpenDecks");
    }

    #[test]
    fn a_claim_on_the_number_we_hold_is_defended_to_the_claimant_and_by_broadcast() {
        let link = LinkState::new(ME);
        link.claimed.store(true, Ordering::Relaxed);
        *link.own_src.lock().unwrap() = Some((Ipv4Addr::new(192, 168, 1, 5), 40000));
        *link.own_bcast.lock().unwrap() = Some(Ipv4Addr::new(192, 168, 1, 255));
        let mut rx = AnnounceRx { link: Arc::clone(&link), guard: ClaimGuard::default() };
        let r = rx.on_packet(&ProDjLink::new(ME).build_claim3(1, ME), from(XDJ, 50000));
        let to: Vec<SocketAddr> = r.iter().map(|x| x.1).collect();
        assert_eq!(to, vec![from(XDJ, PORT_ANNOUNCE), from(Ipv4Addr::new(192, 168, 1, 255), PORT_ANNOUNCE)]);
        assert!(r.iter().all(|(p, _)| ProDjLink::parse_defense(p) == Some(ME)));
        assert!(link.peers.lock().unwrap().is_empty(), "a claim is not an announce");
    }
}

/// The sender's master rules: MASTER pressed, a silent master, and where
/// our status goes.
#[cfg(test)]
mod sender_tests {
    use super::*;

    const ME: u8 = 3;
    const CDJ: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 20);
    const XDJ: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 30);
    fn pressed() -> Arc<LinkState> {
        let link = LinkState::new(ME);
        link.want_master.store(true, Ordering::Relaxed);
        link
    }
    fn peer(link: &LinkState, n: u8, ip: Ipv4Addr, name: &str) {
        link.peers.lock().unwrap().insert(n, ip);
        link.peer_names.lock().unwrap().insert(n, name.into());
    }

    #[test]
    fn with_no_master_heard_for_two_seconds_master_is_taken() {
        let link = pressed();
        let mut h = Handoff::default();
        let t0 = Instant::now();
        assert!(h.tick(&link, t0, true, ME).is_none());
        assert!(h.tick(&link, t0 + Duration::from_millis(1900), true, ME).is_none());
        assert!(!link.master.load(Ordering::Relaxed), "the master's status may not have come yet");
        h.tick(&link, t0 + Duration::from_millis(2000), true, ME);
        assert!(link.master.load(Ordering::Relaxed));
        assert!(!link.want_master.load(Ordering::Relaxed));
    }

    #[test]
    fn with_a_master_we_ask_once_a_second_until_it_agrees_and_never_just_take_it() {
        let link = pressed();
        link.master_player.store(2, Ordering::Relaxed);
        peer(&link, 2, CDJ, "CDJ-3000");
        let mut h = Handoff::default();
        let t0 = Instant::now();
        let ask = h.tick(&link, t0, true, ME).expect("a takeover request");
        assert_eq!(ask.1, SocketAddr::from((CDJ, PORT_BEAT)));
        assert_eq!(ProDjLink::parse_master_request(&ask.0), Some(ME));
        assert!(h.tick(&link, t0 + Duration::from_millis(500), true, ME).is_none(), "not before a second");
        assert!(h.tick(&link, t0 + Duration::from_millis(1000), true, ME).is_some(), "asked again");
        link.yielded_from.store(2, Ordering::Relaxed);
        assert!(h.tick(&link, t0 + Duration::from_millis(5000), true, ME).is_none(), "it agreed: wait for Mh");
        assert!(!link.master.load(Ordering::Relaxed), "never asserted without the handoff, however long");
    }

    #[test]
    fn a_master_whose_address_is_unknown_is_not_asked_or_overridden() {
        let link = pressed();
        link.master_player.store(2, Ordering::Relaxed);
        let mut h = Handoff::default();
        let t0 = Instant::now();
        for ms in [0, 600, 2500, 6000] {
            assert!(h.tick(&link, t0 + Duration::from_millis(ms), true, ME).is_none());
        }
        assert!(!link.master.load(Ordering::Relaxed));
    }

    /// Playing alone on the network: master after two seconds, as a CDJ
    /// takes it, without MASTER pressed.
    #[test]
    fn playing_with_no_master_takes_it_after_two_seconds() {
        let link = LinkState::new(ME);
        let mut a = AutoMaster::default();
        let t0 = Instant::now();
        a.tick(&link, t0, true, true, true);
        a.tick(&link, t0 + Duration::from_millis(1900), true, true, true);
        assert!(!link.master.load(Ordering::Relaxed), "a master's status may not have come yet");
        a.tick(&link, t0 + Duration::from_millis(2000), true, true, true);
        assert!(link.master.load(Ordering::Relaxed));
        assert!(link.our_sync.load(Ordering::Relaxed) > link.largest_sync.load(Ordering::Relaxed), "a newer sync counter than any seen");
    }

    /// Not taken while paused, without a track, receive-only, with another
    /// master on the network, or mid-handoff; and pausing restarts the wait.
    #[test]
    fn auto_master_only_when_playing_alone() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(3);
        for (send, playing, track) in [(true, false, true), (true, true, false), (false, true, true)] {
            let link = LinkState::new(ME);
            let mut a = AutoMaster::default();
            a.tick(&link, t0, send, playing, track);
            a.tick(&link, later, send, playing, track);
            assert!(!link.master.load(Ordering::Relaxed), "send {send} playing {playing} track {track}");
        }
        let link = LinkState::new(ME);
        link.master_player.store(2, Ordering::Relaxed);
        let mut a = AutoMaster::default();
        a.tick(&link, t0, true, true, true);
        a.tick(&link, later, true, true, true);
        assert!(!link.master.load(Ordering::Relaxed), "another deck is master: ask with MASTER");
        let link = LinkState::new(ME);
        link.yielded_from.store(2, Ordering::Relaxed);
        let mut a = AutoMaster::default();
        a.tick(&link, t0, true, true, true);
        a.tick(&link, later, true, true, true);
        assert!(!link.master.load(Ordering::Relaxed), "a handoff is under way");
        let link = LinkState::new(ME);
        let mut a = AutoMaster::default();
        a.tick(&link, t0, true, true, true);
        a.tick(&link, t0 + Duration::from_millis(1500), true, false, true);
        a.tick(&link, t0 + Duration::from_millis(2500), true, true, true);
        assert!(!link.master.load(Ordering::Relaxed), "the pause restarted the wait");
        a.tick(&link, t0 + Duration::from_millis(4600), true, true, true);
        assert!(link.master.load(Ordering::Relaxed));
    }

    #[test]
    fn a_receive_only_deck_never_takes_master() {
        let link = pressed();
        let mut h = Handoff::default();
        let t0 = Instant::now();
        h.tick(&link, t0, false, ME);
        h.tick(&link, t0 + Duration::from_secs(3), false, ME);
        assert!(!link.master.load(Ordering::Relaxed), "send_full off (no master on the LAN from test runs)");
    }

    #[test]
    fn letting_go_of_master_restarts_the_wait() {
        let link = pressed();
        let mut h = Handoff::default();
        let t0 = Instant::now();
        h.tick(&link, t0, true, ME);
        link.want_master.store(false, Ordering::Relaxed);
        h.tick(&link, t0 + Duration::from_millis(1500), true, ME);
        link.want_master.store(true, Ordering::Relaxed);
        h.tick(&link, t0 + Duration::from_millis(2100), true, ME);
        h.tick(&link, t0 + Duration::from_millis(4000), true, ME);
        assert!(!link.master.load(Ordering::Relaxed), "the two seconds count from the new press");
        h.tick(&link, t0 + Duration::from_millis(4100), true, ME);
        assert!(link.master.load(Ordering::Relaxed));
    }

    #[test]
    fn a_master_silent_for_five_seconds_is_forgotten() {
        let link = LinkState::new(ME);
        link.master_player.store(2, Ordering::Relaxed);
        link.yielded_from.store(2, Ordering::Relaxed);
        link.master_seen_ms.store(1_000, Ordering::Relaxed);
        forget_silent_master(&link, 6_000, true);
        assert_eq!(link.master_player.load(Ordering::Relaxed), 2, "5 s is not yet silent");
        forget_silent_master(&link, 6_001, true);
        assert_eq!(link.master_player.load(Ordering::Relaxed), 0);
        assert_eq!(link.yielded_from.load(Ordering::Relaxed), 0, "a stale agreement goes with it");
        assert!(!link.master.load(Ordering::Relaxed), "not synced: no promotion");
    }

    #[test]
    fn a_synced_deck_whose_master_vanishes_becomes_master() {
        let link = LinkState::new(ME);
        link.sync.store(true, Ordering::Relaxed);
        link.master_player.store(2, Ordering::Relaxed);
        link.master_seen_ms.store(1_000, Ordering::Relaxed);
        link.largest_sync.store(7, Ordering::Relaxed);
        forget_silent_master(&link, 9_000, true);
        assert!(link.master.load(Ordering::Relaxed));
        assert_eq!(link.our_sync.load(Ordering::Relaxed), 8);

        // Receive-only: forget, but never promote.
        let quiet = LinkState::new(ME);
        quiet.sync.store(true, Ordering::Relaxed);
        quiet.master_player.store(2, Ordering::Relaxed);
        quiet.master_seen_ms.store(1_000, Ordering::Relaxed);
        forget_silent_master(&quiet, 9_000, false);
        assert_eq!(quiet.master_player.load(Ordering::Relaxed), 0);
        assert!(!quiet.master.load(Ordering::Relaxed));
    }

    #[test]
    fn a_master_never_heard_is_not_timed_out() {
        let link = LinkState::new(ME);
        link.master_player.store(2, Ordering::Relaxed);
        forget_silent_master(&link, 60_000, true);
        assert_eq!(link.master_player.load(Ordering::Relaxed), 2, "seen_ms 0 = no status yet, not silence");
    }

    #[test]
    fn status_goes_to_each_peer_in_its_generations_form() {
        let link = LinkState::new(ME);
        peer(&link, 1, XDJ, "XDJ-1000MK2");
        peer(&link, 2, CDJ, "CDJ-3000");
        let mut out = status_replies(&link, b"short", b"long");
        out.sort_by_key(|r| r.1);
        assert_eq!(out, vec![
            (b"long".to_vec(), SocketAddr::from((CDJ, PORT_STATUS))),
            (b"short".to_vec(), SocketAddr::from((XDJ, PORT_STATUS))),
        ]);
        assert!(status_replies(&LinkState::new(ME), b"s", b"l").is_empty(), "nobody to tell");
    }
}
