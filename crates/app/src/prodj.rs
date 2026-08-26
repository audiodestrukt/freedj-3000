//! ProDJ Link beat packet listener.
//!
//! Testing without a second deck: docs/reference/link-test-harness.md
//! (`make virtual-cdj` for a full virtual CDJ, `make two-deck` for beats only).
//!
//! Binds UDP on port 50002 and waits for Pioneer beat packets (0x28).
//! On each beat, updates beat2_bpm and bumps beat2_anchor (which triggers
//! a phase reset in the renderer, keeping the second beat grid locked to
//! the incoming beat).
//!
//! Works with real Pioneer CDJs/XDJs on the same LAN, or any tool that
//! sends ProDJ Link beat packets (e.g. dysentery, rekordbox in link mode).
//!
//! Run with RUST_LOG=opendeck::prodj=debug for per-packet logging.

use opendeck_link::prodj::{ProDjLink, PORT_ANNOUNCE, PORT_BEAT, PORT_STATUS};
use opendeck_types::{BeatGrid, EngineSnapshot};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::{SocketAddr, UdpSocket},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

pub struct ProDjHandle {
    _threads: Vec<thread::JoinHandle<()>>,
}

impl ProDjHandle {
    /// Spawn background listeners for ProDJ Link beat packets.
    ///
    /// Real CDJ/XDJ hardware sends beat packets on 50001.  `send_beat.py` and
    /// the earlier single-machine tests used 50002, so both ports are
    /// listened on and fed to the same handler.  A port that cannot be bound
    /// is skipped with a warning rather than failing the app.
    pub fn listen(
        beat2_bpm:    Arc<AtomicU32>,
        beat2_anchor: Arc<AtomicU64>,
        beat2_player: Arc<AtomicU32>,
        own_player:   u8,
    ) -> Option<Self> {
        let mut threads = Vec::new();
        for port in [PORT_BEAT, PORT_STATUS] {
            match listen_port(port, Arc::clone(&beat2_bpm), Arc::clone(&beat2_anchor), Arc::clone(&beat2_player), own_player) {
                Some(t) => threads.push(t),
                None    => log::warn!("ProDJ Link: not listening on {port}"),
            }
        }
        // Announce port is sniffed so a real deck's presence shows in the log
        // even before any beat arrives.
        spawn_sniffer(PORT_ANNOUNCE);
        if threads.is_empty() { None } else { Some(ProDjHandle { _threads: threads }) }
    }
}

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

fn listen_port(
    port:         u16,
    beat2_bpm:    Arc<AtomicU32>,
    beat2_anchor: Arc<AtomicU64>,
    beat2_player: Arc<AtomicU32>,
    own_player:   u8,
) -> Option<thread::JoinHandle<()>> {
    let sock = bind_shared(port)?;
    log::info!("ProDJ Link: listening for beat packets on port {port}");
    thread::Builder::new()
        .name(format!("prodj-rx-{port}"))
        .spawn(move || {
            let mut buf = [0u8; 1500];
            loop {
                match sock.recv_from(&mut buf) {
                    Ok((n, addr)) => {
                        // Full hex on the beat port: this is the ground truth
                        // for fixing build_beat/parse_beat_packet against real
                        // hardware.
                        log::info!("ProDJ rx :{port} {n} bytes from {addr} — {:02X?}", &buf[..n]);
                        if let Some((player, snap)) = ProDjLink::parse_packet(&buf[..n]) {
                            // Our own broadcasts come back to us; a deck is
                            // not its own master.
                            if player == own_player { continue; }
                            let old_bpm = f32::from_bits(beat2_bpm.load(Ordering::Relaxed));
                            beat2_bpm.store(snap.bpm.to_bits(), Ordering::Relaxed);
                            beat2_player.store(player as u32, Ordering::Relaxed);
                            beat2_anchor.fetch_add(1, Ordering::Relaxed);
                            log::info!(
                                "ProDJ beat: player {} @ {:.2} BPM (was {:.2}) via :{port}",
                                player, snap.bpm, old_bpm
                            );
                        }
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => log::error!("ProDJ Link recv :{port}: {e}"),
                }
            }
        })
        .ok()
}

/// Spawn a read-only sniffer on `port` that logs every packet received.
/// Used to verify that the virtual CDJ is actually sending traffic.
fn spawn_sniffer(port: u16) {
    let Some(sock) = bind_shared(port) else { return; };
    log::info!("ProDJ sniffer: listening on port {port}");

    thread::Builder::new()
        .name(format!("prodj-sniff-{port}"))
        .spawn(move || {
            let mut buf = [0u8; 1500];
            loop {
                match sock.recv_from(&mut buf) {
                    Ok((n, addr)) => {
                        log::info!(
                            "ProDJ port {port} rx: {n} bytes from {addr} — {:02X?}",
                            &buf[..n.min(48)]
                        );
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => log::error!("ProDJ sniffer port {port}: {e}"),
                }
            }
        })
        .ok();
}

// ── Sender ────────────────────────────────────────────────────────────────────

/// Announces this deck on the network and sends a beat packet at every beat
/// of the *audible* position — the "seen" and "heard" steps of Milestone 1.
pub struct ProDjSender {
    _thread: thread::JoinHandle<()>,
}

/// Shared deck state the sender reads; all atomics, nothing locked.
pub struct SenderState {
    pub position:    Arc<AtomicU64>,   // decoder cursor, interleaved samples
    pub in_flight:   Arc<AtomicU64>,   // samples decoded but not yet audible
    pub playing:     Arc<std::sync::atomic::AtomicBool>,
    pub fader_speed: Arc<AtomicU32>,   // f32 bits
    pub sample_rate: u32,
    pub channels:    u8,
    pub grid:        Option<BeatGrid>,
}

/// Pick the interface to speak Link on: first non-loopback IPv4 with a
/// broadcast address, else loopback (which still reaches a second instance
/// on this machine).  Returns (ip, broadcast, mac, name).
fn link_interface() -> (std::net::Ipv4Addr, std::net::Ipv4Addr, [u8; 6], String) {
    use std::net::Ipv4Addr;
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if i.is_loopback() { continue; }
            if let if_addrs::IfAddr::V4(v4) = &i.addr {
                if let Some(bc) = v4.broadcast {
                    // Skip container bridges: prefer the first interface with a
                    // default-route-looking name; good enough for a desk.
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

impl ProDjSender {
    pub fn start(player: u8, st: SenderState) -> Option<Self> {
        let sock = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| log::warn!("ProDJ Link: sender socket failed: {e}"))
            .ok()?;
        sock.set_broadcast(true).ok();
        let (ip, bcast, mac, iface) = link_interface();
        log::info!("ProDJ Link: sending as player {player} from {ip} ({iface}) to {bcast}");

        let link = ProDjLink::new(player);
        let t = thread::Builder::new()
            .name("prodj-tx".into())
            .spawn(move || {
                let announce = link.build_announce(ip, mac);
                let mut last_announce = std::time::Instant::now() - Duration::from_secs(5);
                let mut last_beat: Option<i64> = None;
                loop {
                    let now = std::time::Instant::now();
                    if now.duration_since(last_announce) >= Duration::from_millis(1500) {
                        let _ = sock.send_to(&announce, (bcast, PORT_ANNOUNCE));
                        last_announce = now;
                    }

                    if st.playing.load(Ordering::Relaxed) {
                        if let Some(grid) = &st.grid {
                            let pos    = st.position.load(Ordering::Relaxed);
                            let ahead  = st.in_flight.load(Ordering::Relaxed);
                            let frames = pos.saturating_sub(ahead) / st.channels as u64;
                            let beat   = grid.beat_at_sample(frames, st.sample_rate).floor() as i64;
                            if last_beat.map_or(true, |b| b != beat) {
                                let fader = f32::from_bits(st.fader_speed.load(Ordering::Relaxed));
                                let bib   = ((beat + grid.downbeat_offset as i64).rem_euclid(4) + 1) as u8;
                                let snap  = EngineSnapshot {
                                    position: pos, ghost_position: pos, speed: fader,
                                    bpm: grid.bpm as f32 * fader,
                                    beat_phase: 0.0, bar_phase: (bib - 1) as f32 / 4.0,
                                    is_playing: true, slip_active: false, key_lock: true,
                                    deck_id: player, timestamp_ns: 0,
                                };
                                let pkt = link.build_beat(&snap, bib);
                                let _ = sock.send_to(&pkt, (bcast, PORT_BEAT));
                                log::debug!("ProDJ tx: beat {beat} ({bib}/4) @ {:.2} BPM", snap.bpm);
                                last_beat = Some(beat);
                            }
                        }
                    } else {
                        last_beat = None;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            })
            .ok()?;
        Some(ProDjSender { _thread: t })
    }
}
