//! ProDJ Link beat packet listener.
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
    ) -> Option<Self> {
        let mut threads = Vec::new();
        for port in [PORT_BEAT, PORT_STATUS] {
            match listen_port(port, Arc::clone(&beat2_bpm), Arc::clone(&beat2_anchor)) {
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
                            let old_bpm = f32::from_bits(beat2_bpm.load(Ordering::Relaxed));
                            beat2_bpm.store(snap.bpm.to_bits(), Ordering::Relaxed);
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
