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
    SYNC_OFF, SYNC_ON,
};
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
    /// player → ip, from announces.
    pub peers:         Mutex<HashMap<u8, Ipv4Addr>>,
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
            peers: Mutex::new(HashMap::new()),
        })
    }

    /// Assert the master role with a sync counter newer than anything seen.
    fn take_master(&self, why: &str) {
        let n = self.largest_sync.load(Ordering::Relaxed) + 1;
        self.our_sync.store(n, Ordering::Relaxed);
        self.master.store(true, Ordering::Relaxed);
        self.want_master.store(false, Ordering::Relaxed);
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
    pub send_full:   bool,
    pub position:    Arc<AtomicU64>,   // decoder cursor, interleaved samples
    pub in_flight:   Arc<AtomicU64>,   // samples decoded but not yet audible
    pub playing:     Arc<AtomicBool>,
    pub fader_speed: Arc<AtomicU32>,   // f32 bits; SYNC writes this
    pub speed:       Arc<AtomicU32>,   // f32 bits; phase nudges write this
    pub sample_rate: u32,
    pub channels:    u8,
    pub grid:        Option<BeatGrid>,
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

/// Pick the interface to speak Link on: first non-loopback, non-bridge IPv4
/// with a broadcast address, else loopback (which still reaches a second
/// instance on this machine).  Returns (ip, broadcast, mac, name).
fn link_interface() -> (Ipv4Addr, Ipv4Addr, [u8; 6], String) {
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if i.is_loopback() { continue; }
            if let if_addrs::IfAddr::V4(v4) = &i.addr {
                if let Some(bc) = v4.broadcast {
                    if i.name.starts_with("docker") || i.name.starts_with("br-") || i.name.starts_with("lxc") || i.name.starts_with("virbr") {
                        continue;
                    }
                    let mac = std::fs::read_to_string(format!("/sys/class/net/{}/address", i.name))
                        .ok()
                        .and_then(|m| {
                            let b: Vec<u8> = m.trim().split(':').filter_map(|h| u8::from_str_radix(h, 16).ok()).collect();
                            b.try_into().ok()
                        })
                        .unwrap_or([0x02, 0xfd, 0, 0, 0, 1]);
                    return (v4.ip, bc, mac, i.name.clone());
                }
            }
        }
    }
    (Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, [0x02, 0xfd, 0, 0, 0, 1], "lo".into())
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
    ) -> Option<Self> {
        let mut threads = Vec::new();
        if let Some(t) = listen_announce(Arc::clone(&link)) { threads.push(t); }
        if let Some(t) = listen_beat(Arc::clone(&link), beat2_bpm, beat2_anchor, Arc::clone(&beat2_player)) { threads.push(t); }
        if let Some(t) = listen_status(Arc::clone(&link), beat2_player) { threads.push(t); }
        if threads.is_empty() { None } else { Some(ProDjHandle { _threads: threads }) }
    }
}

fn spawn(name: &str, sock: UdpSocket, mut f: impl FnMut(&[u8], SocketAddr) + Send + 'static) -> Option<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let mut buf = [0u8; 1500];
            loop {
                match sock.recv_from(&mut buf) {
                    Ok((n, addr)) => f(&buf[..n], addr),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => log::error!("ProDJ Link recv: {e}"),
                }
            }
        })
        .ok()
}

/// 50000: announces → peer table.
fn listen_announce(link: Arc<LinkState>) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(PORT_ANNOUNCE)?;
    log::info!("ProDJ Link: listening for announces on port {PORT_ANNOUNCE}");
    spawn("prodj-rx-50000", sock, move |data, addr| {
        log::trace!("ProDJ rx :50000 {} bytes from {addr} — {:02X?}", data.len(), data);
        if let Some((player, ip)) = ProDjLink::parse_announce(data) {
            if player == link.player { return; }
            if let Ok(mut peers) = link.peers.lock() {
                if peers.insert(player, ip) != Some(ip) {
                    let name = String::from_utf8_lossy(&data[0x0c..0x20]).trim_end_matches('\0').to_string();
                    log::info!("ProDJ Link: player {player} \"{name}\" at {ip}");
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
) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(PORT_BEAT)?;
    log::info!("ProDJ Link: listening for beats on port {PORT_BEAT}");
    let me = ProDjLink::new(link.player);
    let tx = UdpSocket::bind("0.0.0.0:0").ok()?;
    spawn("prodj-rx-50001", sock, move |data, addr| {
        log::debug!("ProDJ rx :50001 {} bytes from {addr} — {:02X?}", data.len(), data);

        if let Some(b) = ProDjLink::parse_beat(data) {
            if b.player == link.player { return; }           // our own broadcast
            let master = link.master_player.load(Ordering::Relaxed) as u8;
            // The B2 strip follows the master if there is one, else whoever plays.
            if master == 0 || master == b.player {
                let old = f32::from_bits(beat2_bpm.load(Ordering::Relaxed));
                beat2_bpm.store(b.bpm.to_bits(), Ordering::Relaxed);
                beat2_player.store(b.player as u32, Ordering::Relaxed);
                beat2_anchor.fetch_add(1, Ordering::Relaxed);
                if (old - b.bpm).abs() > 0.005 {
                    log::info!("ProDJ beat: player {} @ {:.2} BPM (was {old:.2})", b.player, b.bpm);
                } else {
                    log::debug!("ProDJ beat: player {} @ {:.2} BPM beat {}/4", b.player, b.bpm, b.beat_in_bar);
                }
            }
            if master == b.player {
                link.master_bpm.store(b.bpm.to_bits(), Ordering::Relaxed);
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
            if yielded && link.want_master.load(Ordering::Relaxed) {
                link.take_master(&format!("player {from} yielded"));
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
    spawn("prodj-rx-50002", sock, move |data, addr| {
        log::trace!("ProDJ rx :50002 {} bytes from {addr} — {:02X?}", data.len(), data);
        let Some(st) = ProDjLink::parse_status(data) else { return };
        if st.player == link.player { return; }

        let changed = last.get(&st.player).map_or(true, |p| {
            p.play != st.play || p.master != st.master || p.sync != st.sync
                || p.bpm != st.bpm || p.track_loaded != st.track_loaded
                || p.handoff_to != st.handoff_to || (p.pitch - st.pitch).abs() > 0.0005
        });
        if changed {
            log::info!(
                "ProDJ status: player {} fw {} {:?} {}{}{}{}pitch {:+.2}% bpm {} beat {:?}/{:?} handoff {:?}",
                st.player, String::from_utf8_lossy(&st.firmware).trim(), st.play,
                if st.playing { "PLAY " } else { "" }, if st.master { "MASTER " } else { "" },
                if st.sync { "SYNC " } else { "" }, if st.on_air { "ONAIR " } else { "" },
                (st.pitch - 1.0) * 100.0,
                st.bpm.map(|b| format!("{b:.2}")).unwrap_or_else(|| "-".into()),
                st.beat, st.beat_in_bar, st.handoff_to,
            );
            last.insert(st.player, st);
        }

        link.largest_sync.fetch_max(st.sync_counter, Ordering::Relaxed);

        // Linked as soon as any peer sends us status.
        if beat2_player.load(Ordering::Relaxed) == 0 {
            beat2_player.store(st.player as u32, Ordering::Relaxed);
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
            // Someone else says it is master while we are.  The rule is the
            // sync counter: whoever holds the higher one wins.  A CDJ takes
            // master by asserting it with counter = max seen + 1 and holding;
            // the old master then drops its own flag on its own.  So we only
            // relinquish to a *strictly higher* counter — otherwise we keep
            // asserting and the peer yields (verified: the XDJ drops MASTER a
            // second or two after we start claiming with a higher counter).
            if st.sync_counter > link.our_sync.load(Ordering::Relaxed) {
                link.master.store(false, Ordering::Relaxed);
                log::info!("ProDJ Link: player {} asserts master with newer sync {} > {} — no longer master",
                           st.player, st.sync_counter, link.our_sync.load(Ordering::Relaxed));
            } else {
                log::debug!("ProDJ Link: player {} still claims master (sync {} ≤ ours {}); holding",
                            st.player, st.sync_counter, link.our_sync.load(Ordering::Relaxed));
            }
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
        let sock = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| log::warn!("ProDJ Link: sender socket failed: {e}"))
            .ok()?;
        sock.set_broadcast(true).ok();
        let (ip, bcast, mac, iface) = link_interface();
        let player = link.player;
        log::info!("ProDJ Link: sending as player {player} from {ip} ({iface}) to {bcast}");

        let me = ProDjLink::new(player);
        let t = thread::Builder::new()
            .name("prodj-tx".into())
            .spawn(move || {
                let announce = me.build_announce(ip, mac);
                let mut last_announce = Instant::now() - Duration::from_secs(5);
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
                    last_tick = now;
                    let playing = st.playing.load(Ordering::Relaxed);
                    let fader   = f32::from_bits(st.fader_speed.load(Ordering::Relaxed)) as f64;
                    let rate    = st.sample_rate as f64 * fader;

                    // ── Audible position estimate ────────────────────────────
                    let mut beat_now: Option<(i64, f64)> = None;   // (index, fractional)
                    if playing {
                        if let Some(grid) = &st.grid {
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
                    if st.send_full { if let (Some(grid), Some((beat, _))) = (&st.grid, beat_now) {
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
                            let _ = sock.send_to(&pkt, (bcast, PORT_BEAT));
                            log::debug!("ProDJ tx: beat {beat} ({bib}/4) @ {:.2} BPM  +{:.2}ms", snap.bpm, sent_at.duration_since(last_sent).as_secs_f64() * 1000.0);
                            last_sent = sent_at;
                            last_beat = Some(beat);
                        }
                    } }

                    // ── Announce ─────────────────────────────────────────────
                    if now.duration_since(last_announce) >= Duration::from_millis(1500) {
                        let _ = sock.send_to(&announce, (bcast, PORT_ANNOUNCE));
                        last_announce = now;
                    }

                    // ── Master handoff ───────────────────────────────────────
                    if st.send_full && link.want_master.load(Ordering::Relaxed) && !link.master.load(Ordering::Relaxed) {
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
                                // Send the polite 0x26 request once, then take
                                // master by assertion (higher sync counter) —
                                // which is what a real CDJ does and what the
                                // old master actually responds to.
                                if now.duration_since(last_request) >= Duration::from_millis(5000) {
                                    log::info!("ProDJ Link: requesting master from player {p} at {ip}");
                                    let _ = sock.send_to(&me.build_master_request(), (ip, PORT_BEAT));
                                    last_request = now;
                                }
                                link.take_master(&format!("asserting over player {p}"));
                                want_since = None;
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
                    if sync && st.grid.is_some() {
                        let grid = st.grid.as_ref().unwrap();
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
                    if st.send_full && (flags_changed || now.duration_since(last_status) >= Duration::from_millis(200)) {
                        let (beat_num, bib) = match (&st.grid, beat_now) {
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
                            bpm:     st.grid.as_ref().map(|g| g.bpm as f32),
                            beat:    beat_num,
                            beat_in_bar: bib,
                            handoff_to: match link.handoff_to.load(Ordering::Relaxed) { 0 => None, p => Some(p as u8) },
                            counter,
                            sync_counter: link.our_sync.load(Ordering::Relaxed),
                        };
                        counter = counter.wrapping_add(1);
                        let pkt = me.build_status(&fields);
                        let peers: Vec<Ipv4Addr> = link.peers.lock().map(|p| p.values().copied().collect()).unwrap_or_default();
                        if flags_changed {
                            log::info!("ProDJ tx: status master={} sync={} → {} peer(s)", fields.master, fields.sync, peers.len());
                        }
                        for ip in peers {
                            let _ = sock.send_to(&pkt, (ip, PORT_STATUS));
                        }
                        last_status = now;
                    }

                    thread::sleep(Duration::from_millis(1));
                }
            })
            .ok()?;
        Some(ProDjSender { _thread: t })
    }
}
