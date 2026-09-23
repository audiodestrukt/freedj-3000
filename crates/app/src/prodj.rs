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
    ProDjLink, Status, StatusFields, BECOME_MASTER, PORT_ANNOUNCE, PORT_BEAT, PORT_STATUS,
    SYNC_OFF, SYNC_ON, PKT_MEDIA_QUERY, MediaInfo};
use arc_swap::ArcSwap;
use opendeck_types::{BeatGrid, EngineSnapshot};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
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
    pub player:        u8,
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
}

impl LinkState {
    pub fn new(player: u8) -> Arc<Self> {
        Arc::new(Self {
            player,
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
            serve_tracks: AtomicU32::new(0),
            peer_media: Mutex::new(HashMap::new()),
            beat2_beat_ms: AtomicU64::new(0),
            beat2_seen_ms: AtomicU64::new(0),
            beat2_playing: AtomicBool::new(false),
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
fn link_interface(peers: &[Ipv4Addr]) -> (Ipv4Addr, Ipv4Addr, [u8; 6], String) {
    let mut best: Option<(u8, Ipv4Addr, Ipv4Addr, [u8; 6], String)> = None;
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if i.is_loopback() { continue; }
            let if_addrs::IfAddr::V4(v4) = &i.addr else { continue };
            let Some(bc) = v4.broadcast else { continue };
            if i.name.starts_with("docker") || i.name.starts_with("br-") || i.name.starts_with("lxc") || i.name.starts_with("virbr") {
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
                    Ok((n, addr)) => { failures = 0; f(&buf[..n], addr) }
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
fn listen_announce(link: Arc<LinkState>) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(PORT_ANNOUNCE)?;
    log::info!("ProDJ Link: listening for announces on port {PORT_ANNOUNCE}");
    spawn("prodj-rx-50000", PORT_ANNOUNCE, sock, move |data, addr| {
        log::trace!("ProDJ rx :50000 {} bytes from {addr} — {:02X?}", data.len(), data);
        if let Some((player, pkt_ip)) = ProDjLink::parse_announce(data) {
            if player == link.player { return; }
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
    let me = ProDjLink::new(link.player);
    let tx = UdpSocket::bind("0.0.0.0:0").ok()?;
    let mut last_beat_at: HashMap<u8, Instant> = HashMap::new();
    spawn("prodj-rx-50001", PORT_BEAT, sock, move |data, addr| {
        log::debug!("ProDJ rx :50001 {} bytes from {addr} — {:02X?}", data.len(), data);

        if let Some(b) = ProDjLink::parse_beat(data) {
            if b.player == link.player { return; }           // our own broadcast
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
                let _ = tx.send_to(&me.build_master_response(true), (addr.ip(), PORT_BEAT));
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
            if target != link.player { return; }
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
/// per change per player.
fn listen_status(link: Arc<LinkState>, beat2_player: Arc<AtomicU32>) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(PORT_STATUS)?;
    log::info!("ProDJ Link: listening for status on port {PORT_STATUS}");
    let mut last: HashMap<u8, Status> = HashMap::new();
    let reply_sock = UdpSocket::bind("0.0.0.0:0").ok();
    let me = ProDjLink::new(link.player);
    spawn("prodj-rx-50002", PORT_STATUS, sock, move |data, addr| {
        log::trace!("ProDJ rx :50002 {} bytes from {addr} — {:02X?}", data.len(), data);
        // Media query: a peer asks what is in one of our slots.  Answer when we
        // are serving (the media response names the library and its size).
        if ProDjLink::packet_type(data) == Some(PKT_MEDIA_QUERY) {
            if let Some((dev, rip, target, slot)) = ProDjLink::parse_media_query(data) {
                let n = link.serve_tracks.load(Ordering::Relaxed);
                log::info!("ProDJ Link: media query from device {dev} ({rip}) for player {target} slot {slot}; serving {n} tracks");
                if target == link.player && n > 0 && matches!(slot, 0 | 3) {
                    // To the address inside the packet and, across a routed
                    // link where that is the peer's LAN address, to where the
                    // query came from — on the status port either way.
                    let resp = me.build_media_response(rip, 3, "OPENDECK", n.min(u16::MAX as u32) as u16, 0, 32 << 30, 16 << 30);
                    if let Some(s) = &reply_sock {
                        let _ = s.send_to(&resp, (rip, PORT_STATUS));
                        if let std::net::SocketAddr::V4(f) = addr { if *f.ip() != rip { let _ = s.send_to(&resp, (*f.ip(), PORT_STATUS)); } }
                    }
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
        if st.player == link.player { return; }

        let changed = last.get(&st.player).map_or(true, |p| {
            p.play != st.play || p.master != st.master || p.sync != st.sync
                || p.bpm != st.bpm || p.track_loaded != st.track_loaded
                || p.handoff_to != st.handoff_to || (p.pitch - st.pitch).abs() > 0.0005
        });
        if changed {
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
        let handing_to_us = st.handoff_to == Some(link.player);
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
        let player = link.player;
        if let Ok(mut a) = link.own_addr.lock() { *a = format!("{ip} ({iface}) to {bcast}"); }
        // OPENDECK_LINK_UNICAST=ip,ip — also send announces straight to these
        // devices (broadcast does not cross a VPN such as Tailscale).
        let unicast_peers: Vec<Ipv4Addr> = std::env::var("OPENDECK_LINK_UNICAST").ok()
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect()).unwrap_or_default();
        if !unicast_peers.is_empty() { log::info!("ProDJ Link: announcing by unicast to {unicast_peers:?}"); }
        log::info!("ProDJ Link: sending as player {player} from {ip} ({iface}) to {bcast}");

        let me = ProDjLink::new(player);
        let t = thread::Builder::new()
            .name("prodj-tx".into())
            .spawn(move || {
                // Re-selected below once players are discovered.
                let (mut ip, mut bcast, mut mac) = (ip, bcast, mac);
                let mut announce = me.build_announce(ip, mac);
                let mut known_peers: Vec<Ipv4Addr>;
                let mut bcast_warned = false;
                let mut send_failures = 0u32;
                let mut last_announce = Instant::now() - Duration::from_secs(5);
                let mut last_media_query = Instant::now() - Duration::from_secs(5);
                let mut last_status   = Instant::now() - Duration::from_secs(5);
                let mut last_request  = Instant::now() - Duration::from_secs(5);
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
                // Status goes out immediately when our master/sync flags change:
                // the old master waits only briefly for the new one's claim.
                let mut last_flags = (false, false);

                loop {
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
                        if cur != 0 && cur != link.player && seen > 0 && link.now_ms().saturating_sub(seen) > MASTER_GONE_MS {
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
                                    let err = if frac > 0.5 { frac - 1.0 } else { frac };   // beats, ±0.5
                                    if err.abs() > 0.01 {
                                        // Correct half the error per master beat: bounded like a
                                        // jog nudge, and halving avoids the overshoot a full
                                        // correction gives (measured ±0.02 beat oscillation at gain 1).
                                        let offset = (-(err as f32) * 0.5).clamp(-0.03, 0.03);
                                        let period_s = 60.0 / master_bpm.max(1.0);
                                        nudge = Some((offset, now + Duration::from_secs_f32(period_s)));
                                        st.speed.store((want + offset).to_bits(), Ordering::Relaxed);
                                        log::debug!("ProDJ sync: phase err {err:+.3} beat → nudge {:+.2}%", offset * 100.0);
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
                        let fields = StatusFields {
                            playing,
                            track_loaded: true,
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
                        let mut pkt = me.build_status(&fields);
                        if link.serve_tracks.load(Ordering::Relaxed) > 0 {
                            pkt[0x6f] = 0x00;   // USB local state: loaded
                            pkt[0x75] = 0x01;   // link media available
                        }
                        let peers: Vec<Ipv4Addr> = link.peers.lock().map(|p| p.values().copied().collect()).unwrap_or_default();
                        if flags_changed {
                            log::info!("ProDJ tx: status master={} sync={} → {} peer(s)", fields.master, fields.sync, peers.len());
                        }
                        for ip in peers {
                            let _ = sock.send_to(&pkt, (ip, PORT_STATUS));
                        }
                        last_status = now;
                    }

                    // Tick at 1 ms while playing — the beat clock is read off
                    // this loop.  Paused there is no beat, so sleep to the next
                    // periodic deadline instead: a thousand wakeups a second,
                    // not the work they do, is what keeps a phone's core out of
                    // its idle state.  Capped so flag changes (MASTER, SYNC) and
                    // a fresh peer are still picked up within 100 ms.
                    let nap = if playing { Duration::from_millis(1) } else {
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
