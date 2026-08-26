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

use opendeck_link::prodj::{ProDjLink, Status, PORT_ANNOUNCE, PORT_BEAT, PORT_STATUS};
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
            // Last decoded status per player, so changes are logged once.
            let mut last_status: std::collections::HashMap<u8, Status> = Default::default();
            loop {
                match sock.recv_from(&mut buf) {
                    Ok((n, addr)) => {
                        // Full hex at debug: the ground truth for packet-format work.
                        log::debug!("ProDJ rx :{port} {n} bytes from {addr} — {:02X?}", &buf[..n]);
                        if let Some(st) = ProDjLink::parse_status(&buf[..n]) {
                            if st.player == own_player { continue; }
                            let changed = last_status.get(&st.player).map_or(true, |p| {
                                p.play != st.play || p.master != st.master || p.sync != st.sync
                                    || p.bpm != st.bpm || p.track_loaded != st.track_loaded
                                    || p.handoff_to != st.handoff_to || (p.pitch - st.pitch).abs() > 0.0005
                            });
                            if changed {
                                log::info!(
                                    "ProDJ status: player {} fw {} {:?} {}{}{}{} pitch {:+.2}% bpm {} beat {:?}/{:?} handoff {:?}",
                                    st.player, String::from_utf8_lossy(&st.firmware), st.play,
                                    if st.playing { "PLAY " } else { "" }, if st.master { "MASTER " } else { "" },
                                    if st.sync { "SYNC " } else { "" }, if st.on_air { "ONAIR " } else { "" },
                                    (st.pitch - 1.0) * 100.0,
                                    st.bpm.map(|b| format!("{b:.2}")).unwrap_or_else(|| "-".into()),
                                    st.beat, st.beat_in_bar, st.handoff_to,
                                );
                                last_status.insert(st.player, st);
                            }
                            // A status packet from a peer means we are linked,
                            // even before it plays a beat.
                            if beat2_player.load(Ordering::Relaxed) == 0 {
                                beat2_player.store(st.player as u32, Ordering::Relaxed);
                            }
                            continue;
                        }
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
                let mut last_sent  = std::time::Instant::now();
                // The decoder cursor advances a 512-frame block at a time and
                // `in_flight` swings ±35 ms between blocks, so beat crossings
                // read straight off `position - in_flight` land on block
                // boundaries (measured: intervals alternating 430/463 ms for a
                // 445 ms beat).  Do what the renderer does: free-run an
                // estimate of the audible position at the true rate and pull
                // it gently toward the (low-passed) reference.  Beat crossings
                // come from the estimate, which moves smoothly.
                let mut ahead_avg: f64 = 0.0;
                let mut est_frames: f64 = -1.0;
                let mut last_tick = std::time::Instant::now();
                loop {
                    let now = std::time::Instant::now();
                    if now.duration_since(last_announce) >= Duration::from_millis(1500) {
                        let _ = sock.send_to(&announce, (bcast, PORT_ANNOUNCE));
                        last_announce = now;
                    }

                    if st.playing.load(Ordering::Relaxed) {
                        if let Some(grid) = &st.grid {
                            let pos    = st.position.load(Ordering::Relaxed);
                            let ahead  = st.in_flight.load(Ordering::Relaxed) as f64;
                            let fader  = f32::from_bits(st.fader_speed.load(Ordering::Relaxed)) as f64;
                            ahead_avg += (ahead - ahead_avg) * 0.02;
                            let reference = (pos as f64 - ahead_avg).max(0.0) / st.channels as f64;
                            let dt = now.duration_since(last_tick).as_secs_f64();
                            if est_frames < 0.0 || (reference - est_frames).abs() > st.sample_rate as f64 * 0.5 {
                                est_frames = reference;                       // start, or a seek
                            } else {
                                est_frames += dt * st.sample_rate as f64 * fader;
                                est_frames += (reference - est_frames) * 0.02;
                            }
                            let frames = est_frames.max(0.0) as u64;
                            let beat   = grid.beat_at_sample(frames, st.sample_rate).floor() as i64;
                            let seek   = last_beat.map_or(false, |b| beat < b - 2 || beat > b + 8);
                            if last_beat.map_or(true, |b| beat > b) || seek {
                                let bib   = ((beat + grid.downbeat_offset as i64).rem_euclid(4) + 1) as u8;
                                let snap  = EngineSnapshot {
                                    position: pos, ghost_position: pos, speed: fader as f32,
                                    bpm: grid.bpm as f32 * fader as f32,
                                    beat_phase: 0.0, bar_phase: (bib - 1) as f32 / 4.0,
                                    is_playing: true, slip_active: false, key_lock: true,
                                    deck_id: player, timestamp_ns: 0,
                                };
                                let pkt = link.build_beat(&snap, bib);
                                let _ = sock.send_to(&pkt, (bcast, PORT_BEAT));
                                let now = std::time::Instant::now();
                                log::debug!("ProDJ tx: beat {beat} ({bib}/4) @ {:.2} BPM  +{:.1}ms", snap.bpm, now.duration_since(last_sent).as_secs_f64() * 1000.0);
                                last_sent = now;
                                last_beat = Some(beat);
                            }
                        }
                    } else {
                        last_beat = None;
                        est_frames = -1.0;
                    }
                    last_tick = now;
                    thread::sleep(Duration::from_millis(1));
                }
            })
            .ok()?;
        Some(ProDjSender { _thread: t })
    }
}
