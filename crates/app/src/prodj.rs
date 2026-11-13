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
        Self { phase: -1.0, folded_ms: 0, pending: 0.0 }
    }

    /// One frame: `dt_s` since the last call; `beat_ms` is when the latest
    /// beat packet arrived (same clock as `now_ms`, 0 = none yet).  Returns
    /// the phase to draw, 0..1.
    pub fn advance(&mut self, now_ms: u64, beat_ms: u64, bpm: f32, peer_playing: bool, dt_s: f64) -> f32 {
        if bpm <= 0.0 || beat_ms == 0 {
            *self = Self::new();
            return 0.0;
        }
        let period_s = 60.0 / bpm as f64;
        let age_s    = now_ms.saturating_sub(beat_ms) as f64 / 1000.0;
        let running  = peer_playing || age_s < 2.0 * period_s;
        if self.phase < 0.0 {
            self.phase = age_s / period_s;          // first packet: the beat was `age` ago
            self.folded_ms = beat_ms;
            self.pending = 0.0;
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
        }
        self.phase.rem_euclid(1.0) as f32
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

fn link_interface(peers: &[Ipv4Addr]) -> (Ipv4Addr, Ipv4Addr, [u8; 6], String) {
    let pin = pinned_iface();
    if let Some((_, ip, _)) = pin {
        if ip.is_loopback() { return (Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, [0x02, 0xfd, 0, 0, 0, 1], "lo".into()); }
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

fn listen_announce(link: Arc<LinkState>) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(PORT_ANNOUNCE)?;
    log::info!("ProDJ Link: listening for announces on port {PORT_ANNOUNCE}");
    // Defenses go back to the claimant's port 50000 from a socket of our own.
    let defend_sock = UdpSocket::bind("0.0.0.0:0").ok();
    let local_ips = if_addrs::get_if_addrs().map(|v| v.into_iter().filter_map(|i| match i.addr { if_addrs::IfAddr::V4(a) => Some(a.ip), _ => None }).collect()).unwrap_or_default();
    let mut guard = ClaimGuard { defend_port: defend_sock.as_ref().and_then(|d| d.local_addr().ok()).map(|a| a.port()), local_ips, ..Default::default() };
    spawn("prodj-rx-50000", PORT_ANNOUNCE, sock, move |data, addr| {
        log::trace!("ProDJ rx :50000 {} bytes from {addr} — {:02X?}", data.len(), data);
        diag_packet(50000, addr, data);
        let from = match addr { SocketAddr::V4(v) => Some((*v.ip(), v.port())), _ => None };
        match guard.on_packet(&link, data, from) {
            ClaimAction::Done => return,
            ClaimAction::Defend { number, to, ours } => {
                // Unicast, as the players do, and broadcast too: on one
                // machine several programs share port 50000 and a unicast
                // reaches only one of them.  Others ignore a defense of a
                // number they are not claiming.
                if let Some(ds) = defend_sock.as_ref() {
                    let pkt = ProDjLink::new(number).build_defense(number, ours);
                    let _ = ds.send_to(&pkt, (to, PORT_ANNOUNCE));
                    if let Some(b) = link.own_bcast.lock().ok().and_then(|g| *g) {
                        let _ = ds.set_broadcast(true);
                        let _ = ds.send_to(&pkt, (b, PORT_ANNOUNCE));
                    }
                }
                return;
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
    let mut last_beat_at: HashMap<u8, Instant> = HashMap::new();
    spawn("prodj-rx-50001", PORT_BEAT, sock, move |data, addr| {
        log::debug!("ProDJ rx :50001 {} bytes from {addr} — {:02X?}", data.len(), data);
        diag_packet(50001, addr, data);

        if note_position(&link, data, Instant::now()) { return; }
        if let Some(b) = ProDjLink::parse_beat(data) {
            if b.player == link.player() { return; }           // our own broadcast
            // An OpenDeck sends each beat by broadcast AND unicast (see the
            // sender); the second copy lands within a few ms.  A real beat is
            // never closer than 300 ms (200 BPM), so drop the twin.
            if beat_is_twin(&mut last_beat_at, b.player, Instant::now()) { return; }
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
            return;
        }

        if let Some(requester) = ProDjLink::parse_master_request(data) {
            if link.master.load(Ordering::Relaxed) {
                log::info!("ProDJ Link: player {requester} asks for master — yielding");
                link.handoff_to.store(requester as u32, Ordering::Relaxed);
                let _ = tx.send_to(&ProDjLink::new(link.player()).build_master_response(true), (addr.ip(), PORT_BEAT));
            }
            return;
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
            return;
        }
        if let Some((from, target, cmd)) = ProDjLink::parse_sync_control(data) {
            if target != link.player() { return; }
            match cmd {
                SYNC_ON  => { link.sync.store(true,  Ordering::Relaxed); log::info!("ProDJ Link: player {from} turned our SYNC on"); }
                SYNC_OFF => { link.sync.store(false, Ordering::Relaxed); log::info!("ProDJ Link: player {from} turned our SYNC off"); }
                BECOME_MASTER => { link.want_master.store(true, Ordering::Relaxed); log::info!("ProDJ Link: player {from} asks us to become master"); }
                _ => log::info!("ProDJ Link: sync control {cmd:#x} from player {from}"),
            }
        }
    })
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
    let mut last: HashMap<u8, Status> = HashMap::new();
    let reply_sock = UdpSocket::bind("0.0.0.0:0").ok();
    spawn("prodj-rx-50002", PORT_STATUS, sock, move |data, addr| {
        log::trace!("ProDJ rx :50002 {} bytes from {addr} — {:02X?}", data.len(), data);
        diag_packet(50002, addr, data);
        // Media query: a peer asks what is in one of our slots.  Answer when we
        // are serving (the media response names the library and its size).
        if ProDjLink::packet_type(data) == Some(PKT_MEDIA_QUERY) {
            if let Some((resp, rip)) = answer_media_query(&link, data) {
                // To the address inside the packet and, across a routed link
                // where that is the peer's LAN address, to where the query
                // came from — on the status port either way.
                if let Some(s) = &reply_sock {
                    let _ = s.send_to(&resp, (rip, PORT_STATUS));
                    if let std::net::SocketAddr::V4(f) = addr { if *f.ip() != rip { let _ = s.send_to(&resp, (*f.ip(), PORT_STATUS)); } }
                }
            }
            return;
        }
        // Media response: a player answering our query about one of its slots.
        if let Some(info) = ProDjLink::parse_media_response(data) {
            if let Ok(mut m) = link.peer_media.lock() {
                let key = (info.player, info.slot);
                if m.get(&key).map_or(true, |(old, _)| *old != info) {
                    log::info!("ProDJ Link: player {} {}: {:?}, {} tracks, {} playlists", info.player, info.slot_name(), info.name, info.tracks, info.playlists);
                }
                m.insert(key, (info, Instant::now()));
            }
            return;
        }
        let Some(st) = ProDjLink::parse_status(data) else { return };
        if st.player == link.player() { return; }

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
            return;
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
    })
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
                let mut last_request  = Instant::now() - Duration::from_secs(5);
                let mut last_abs      = Instant::now() - Duration::from_secs(5);
                // When MASTER is pressed we may not have heard the current
                // master's status yet (it arrives every ~200 ms, announces
                // every 1.5 s).  Wait before concluding nobody is master.
                let mut want_since: Option<Instant> = None;
                let mut counter: u32  = 0;
                let mut last_beat: Option<i64> = None;
                let mut last_sent = Instant::now();
                // Audible-position estimate.  The decoder cursor advances a
                // 512-frame block at a time and `in_flight` swings ±35 ms
                // between blocks, so beat crossings read straight off
                // `position - in_flight` land on block boundaries.  Free-run
                // an estimate at the true rate and pull it gently toward the
                // low-passed reference; τ ≈ 200 ms on both at the 1 ms tick.
                // Measured against an XDJ-1000MK2 at the same receiver:
                // sd 1.23 ms vs its 1.33 ms.  See PERFORMANCE.md.
                let mut ahead_avg: f64 = 0.0;
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
                    let mut beat_now: Option<(i64, f64)> = None;   // (index, fractional)
                    if playing {
                        if let Some(grid) = cur_grid {
                            let pos   = st.position.load(Ordering::Relaxed);
                            let ahead = st.in_flight.load(Ordering::Relaxed) as f64;
                            ahead_avg += (ahead - ahead_avg) * 0.005;
                            let reference = (pos as f64 - ahead_avg).max(0.0) / st.channels as f64;
                            if est_frames < 0.0 || (reference - est_frames).abs() > st.sample_rate as f64 * 0.5 {
                                est_frames = reference;
                            } else {
                                est_frames += dt * rate;
                                est_frames += (reference - est_frames) * 0.005;
                            }
                            let period = grid.samples_per_beat_at(est_frames.max(0.0) as u64, st.sample_rate);
                            let beat_f = grid.beat_at_sample(est_frames.max(0.0) as u64, st.sample_rate);
                            // Sleep-to-deadline for the last stretch before a beat.
                            let to_next = ((beat_f.floor() + 1.0) - beat_f) * period / rate;
                            if to_next > 0.0 && to_next < 0.0015 {
                                let target = now + Duration::from_secs_f64(to_next);
                                while Instant::now() < target { std::hint::spin_loop(); }
                                est_frames += to_next * rate;
                            }
                            let beat_f = grid.beat_at_sample(est_frames.max(0.0) as u64, st.sample_rate);
                            beat_now = Some((beat_f.floor() as i64, beat_f - beat_f.floor()));
                        }
                    } else {
                        last_beat = None;
                        est_frames = -1.0;
                    }

                    // ── Beat packet ──────────────────────────────────────────
                    let send_full = st.send_full.load(Ordering::Relaxed);
                    if send_full { if let (Some(grid), Some((beat, _))) = (cur_grid, beat_now) {
                        let seek = last_beat.map_or(false, |b| beat < b - 2 || beat > b + 8);
                        if last_beat.map_or(true, |b| beat > b) || seek {
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
                            last_beat = Some(beat);
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
                    // Status comes every 200 ms from any deck, paused or not.
                    // Without this, two decks that both went quiet (iOS
                    // reclaimed their sockets; the other side slept) each
                    // kept asking the other for a handoff that never came,
                    // and MASTER could not be set on either.
                    {
                        const MASTER_GONE_MS: u64 = 5000;
                        let cur = link.master_player.load(Ordering::Relaxed) as u8;
                        let seen = link.master_seen_ms.load(Ordering::Relaxed);
                        if cur != 0 && cur != link.player() && seen > 0 && link.now_ms().saturating_sub(seen) > MASTER_GONE_MS {
                            log::info!("ProDJ Link: master player {cur} silent for {}s — forgetting it", MASTER_GONE_MS / 1000);
                            link.master_player.store(0, Ordering::Relaxed);
                            link.yielded_from.store(0, Ordering::Relaxed);
                            // A synced player whose master vanishes promotes
                            // itself, so the synced group keeps a tempo
                            // reference (per the Pro DJ Link analysis; to be
                            // confirmed against the XDJ-1000MK2).
                            if link.sync.load(Ordering::Relaxed) && send_full {
                                link.take_master("our master vanished while we were synced");
                            }
                        }
                    }

                    // ── Master handoff ───────────────────────────────────────
                    if send_full && link.want_master.load(Ordering::Relaxed) && !link.master.load(Ordering::Relaxed) {
                        let since = *want_since.get_or_insert(now);
                        let cur = link.master_player.load(Ordering::Relaxed) as u8;
                        match (cur, link.peer_ip(cur)) {
                            (0, _) if now.duration_since(since) >= Duration::from_secs(2) => {
                                // Nobody has claimed master in two seconds: take it.
                                link.take_master("no master on the network");
                                want_since = None;
                            }
                            (0, _) => {}
                            (p, Some(ip)) => {
                                // Send the 0x26 takeover request and WAIT for the
                                // handshake: the master replies 0x27 and sets Mh
                                // (handoff) = us in its status, at which point we
                                // assert master (see listen_status handing_to_us /
                                // listen_beat 0x27 handler).  Do NOT assert now —
                                // a real CDJ only takes master after the current
                                // master agrees, per the DJ Link spec.  Re-request
                                // ~1/s until the handoff completes or the user
                                // cancels; if the master never agrees we simply
                                // don't take it (better than a rogue second
                                // master the deck ignores).
                                // Re-request until the master agrees (0x27,
                                // sets yielded_from); after that, stop and wait
                                // for it to name us in Mh — like Beat Link, which
                                // sends the request once and waits.
                                if link.yielded_from.load(Ordering::Relaxed) == 0
                                    && now.duration_since(last_request) >= Duration::from_millis(1000) {
                                    log::info!("ProDJ Link: requesting master from player {p} at {ip}");
                                    let _ = sock.send_to(&me.build_master_request(), (ip, PORT_BEAT));
                                    last_request = now;
                                }
                            }
                            (p, None) if now.duration_since(last_request) >= Duration::from_millis(500) => {
                                log::warn!("ProDJ Link: master is player {p} but its address is unknown yet");
                                last_request = now;
                            }
                            _ => {}
                        }
                    } else {
                        want_since = None;
                    }

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
                                if let Some((_, frac)) = beat_now {
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
                        let (beat_num, bib) = match (cur_grid, beat_now) {
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
                        let peers: Vec<(u8, Ipv4Addr)> = link.peers.lock().map(|p| p.iter().map(|(k, v)| (*k, *v)).collect()).unwrap_or_default();
                        let names = link.peer_names.lock().map(|n| n.clone()).unwrap_or_default();
                        if flags_changed {
                            log::info!("ProDJ tx: status master={} sync={} → {} peer(s)", fields.master, fields.sync, peers.len());
                        }
                        // Each device the status of its generation: a CDJ-3000's
                        // (with the loop) to all but older players.
                        for (pl, ip) in peers {
                            let name = names.get(&pl).map(String::as_str).unwrap_or("");
                            let _ = sock.send_to(if wants_cdj3000_status(name) { &long } else { &pkt }, (ip, PORT_STATUS));
                        }
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
            out.push((now, pp.advance(now, beat_ms, BPM, playing, DT)));
            t += DT * 1000.0;
        }
        out
    }

    /// Phase advanced (mod 1) between two frames.
    fn step(a: f32, b: f32) -> f32 { (b - a).rem_euclid(1.0) }

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
