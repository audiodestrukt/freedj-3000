//! `opendeck-serve <music-dir> [--player N] [--peer IP]... [--ip IP] [--portmap 111|50111] [--nfsd 2049]`
//!
//! Stand-alone media source for a Pro DJ Link network: OpenDeck as "a player
//! with a USB stick in it".  Announces on 50000 (broadcast, plus unicast to
//! each `--peer` so a rekordbox / player across a routed link such as
//! Tailscale sees us), sends CDJ status to peers on 50002 with the USB slot
//! flagged loaded, answers media queries with a media response, and runs the
//! dbserver (TCP 12523 + database port) and NFSv2 (portmap / mountd / nfsd)
//! services that clients then use to browse and read.

use anyhow::{Context, Result};
use opendeck_dbserver::server::Server as DbServer;
use opendeck_link::prodj::{ProDjLink, StatusFields, PKT_MEDIA_QUERY, PORT_ANNOUNCE, PORT_STATUS};
use opendeck_nfs::server::NfsServer;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Opts { dir: PathBuf, player: u8, peers: Vec<Ipv4Addr>, ip: Option<Ipv4Addr>, portmap: u16, nfsd: u16 }

fn parse_args() -> Result<Opts> {
    let mut o = Opts { dir: PathBuf::new(), player: 3, peers: Vec::new(), ip: None, portmap: 111, nfsd: 2049 };
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--player"  => o.player = a.next().context("--player N")?.parse()?,
            "--peer"    => o.peers.push(a.next().context("--peer IP")?.parse()?),
            "--ip"      => o.ip = Some(a.next().context("--ip IP")?.parse()?),
            "--portmap" => o.portmap = a.next().context("--portmap PORT")?.parse()?,
            "--nfsd"    => o.nfsd = a.next().context("--nfsd PORT")?.parse()?,
            p => o.dir = PathBuf::from(p),
        }
    }
    if o.dir.as_os_str().is_empty() { anyhow::bail!("usage: opendeck-serve <music-dir> [--player N] [--peer IP] [--ip IP] [--portmap 111|50111] [--nfsd 2049]"); }
    Ok(o)
}

/// The address a peer would see us at: connect a UDP socket toward it and
/// read the local side (no packet is sent).
fn our_ip_toward(peer: Ipv4Addr) -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(SocketAddrV4::new(peer, 9)).ok()?;
    match s.local_addr().ok()? { std::net::SocketAddr::V4(v) => Some(*v.ip()), _ => None }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let o = parse_args()?;
    let ip = o.ip.or_else(|| o.peers.first().and_then(|p| our_ip_toward(*p))).or_else(|| our_ip_toward(Ipv4Addr::new(8, 8, 8, 8))).unwrap_or(Ipv4Addr::LOCALHOST);
    let mac = [0x02, 0x0d, 0xec, 0x00, 0x00, o.player];

    // ── library from the folder ───────────────────────────────────────────────
    // File names now; tags, duration, tempo, grid and waveforms as the
    // analysis thread gets to each track (cached in ~/.cache/opendeck).
    let scanned = opendeck_mediaserver::scan(&o.dir, "OpenDeck")?;
    let n_tracks = scanned.files.len() as u16;
    log::info!("library: {} tracks under {}", n_tracks, o.dir.display());
    for (id, p) in scanned.files.iter().take(10) { log::info!("  [{id}] {}", p.display()); }
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .map(|c| c.join("opendeck").join("linkcache"));

    // ── services ─────────────────────────────────────────────────────────────
    let _nfs = NfsServer::start(scanned.tree, "/C/", o.portmap, o.nfsd)?;
    let _db  = DbServer::start(Arc::clone(&scanned.library), o.player, 0)?;
    let _analysis = opendeck_mediaserver::start_analysis(scanned.library, scanned.files, cache)?;

    // ── Link: announce + status + media query ─────────────────────────────────
    let link = Arc::new(ProDjLink::new(o.player));
    let peers = Arc::new(Mutex::new(o.peers.clone()));
    let ann = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT_ANNOUNCE)).context("bind 50000")?;
    ann.set_broadcast(true)?;
    ann.set_read_timeout(Some(Duration::from_millis(300)))?;
    let st = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT_STATUS)).context("bind 50002")?;
    st.set_read_timeout(Some(Duration::from_millis(300)))?;
    log::info!("link: player {} \"freedj-3000\" at {ip}; unicast peers {:?}", o.player, o.peers);

    // 50000: announce every 1.5 s (broadcast + unicast), learn peers from theirs.
    {
        let (link, peers, ann) = (Arc::clone(&link), Arc::clone(&peers), ann.try_clone()?);
        std::thread::spawn(move || {
            let pkt = link.build_announce(ip, mac);
            let mut last = std::time::Instant::now() - Duration::from_secs(5);
            let mut buf = [0u8; 512];
            loop {
                if last.elapsed() >= Duration::from_millis(1500) {
                    let _ = ann.send_to(&pkt, SocketAddrV4::new(Ipv4Addr::BROADCAST, PORT_ANNOUNCE));
                    for p in peers.lock().unwrap().iter() { let _ = ann.send_to(&pkt, SocketAddrV4::new(*p, PORT_ANNOUNCE)); }
                    last = std::time::Instant::now();
                }
                if let Ok((n, from)) = ann.recv_from(&mut buf) {
                    if let Some((dev, pip)) = ProDjLink::parse_announce(&buf[..n]) {
                        if pip == ip { continue; }
                        let name = String::from_utf8_lossy(&buf[0x0c..0x20]).trim_end_matches('\0').to_string();
                        let mut ps = peers.lock().unwrap();
                        if !ps.contains(&pip) { log::info!("link: device {dev} \"{name}\" at {pip} (from {from})"); ps.push(pip); }
                    }
                }
            }
        });
    }
    // 50002: answer media queries; send status to peers 5×/s.
    {
        let (link, peers, st) = (Arc::clone(&link), Arc::clone(&peers), st.try_clone()?);
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            let mut last = std::time::Instant::now();
            let mut counter = 0u32;
            loop {
                if let Ok((n, from)) = st.recv_from(&mut buf) {
                    let d = &buf[..n];
                    match ProDjLink::packet_type(d) {
                        Some(PKT_MEDIA_QUERY) => match ProDjLink::parse_media_query(d) {
                            Some((dev, rip, target, slot)) => {
                                log::info!("link: media query from device {dev} {rip} (via {from}) for player {target} slot {slot}");
                                if target == o.player && (slot == 3 || slot == 0) {
                                    let resp = link.build_media_response(rip, 3, "OPENDECK", n_tracks, 0, 32 << 30, 16 << 30);
                                    let _ = st.send_to(&resp, SocketAddrV4::new(rip, PORT_STATUS));
                                    let _ = st.send_to(&resp, from);
                                    log::info!("link: → media response ({} tracks) to {rip}", n_tracks);
                                }
                            }
                            None => log::info!("link: short media query from {from}"),
                        },
                        Some(0x0a) => {}
                        Some(t) => log::debug!("link: 50002 type 0x{t:02x} {} bytes from {from}", n),
                        None => {}
                    }
                }
                if last.elapsed() >= Duration::from_millis(200) {
                    counter = counter.wrapping_add(1);
                    let mut pkt = link.build_status(&StatusFields { playing: false, track_loaded: false, master: false, sync: false,
                        on_air: false, pitch: 1.0, bpm: None, beat: None, beat_in_bar: None, handoff_to: None, counter, sync_counter: 0 });
                    pkt[0x6f] = 0x00;   // USB local state: loaded
                    pkt[0x73] = 0x04;   // SD: none
                    pkt[0x75] = 0x01;   // link media available
                    for p in peers.lock().unwrap().iter() { let _ = st.send_to(&pkt, SocketAddrV4::new(*p, PORT_STATUS)); }
                    last = std::time::Instant::now();
                }
            }
        });
    }
    log::info!("serving; Ctrl-C to stop");
    loop { std::thread::sleep(Duration::from_secs(3600)); }
}
