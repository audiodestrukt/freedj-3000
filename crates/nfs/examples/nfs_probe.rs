//! Probe a Link device's NFS: portmap (111 for players, 50111 for rekordbox),
//! list exports, mount the first one and READDIR its root.
//!
//!   cargo run -p opendeck-nfs --example nfs_probe -- 192.168.68.60 50111
//!   cargo run -p opendeck-nfs --example nfs_probe -- 192.168.68.58
//!   cargo run -p opendeck-nfs --example nfs_probe -- 172.20.10.6 50111 "/Users/me/Music/x.mp3"
//!
//! With a third argument the file is looked up under the first export and read
//! in full (timed) — the path a dbserver 0x2102 reply gives for a track.
use opendeck_nfs::{Nfs, PORTMAP_PLAYER, PORTMAP_REKORDBOX};
use std::net::Ipv4Addr;

fn show(b: &[u8]) -> String {
    // Pioneer names are UTF-16LE; fall back to lossy UTF-8.
    if b.len() % 2 == 0 && b.iter().skip(1).step_by(2).all(|&x| x == 0) {
        let u: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(b).into_owned()
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let ip: Ipv4Addr = args.next().expect("usage: nfs_probe <ip> [portmap-port]").parse()?;
    let pm: u16 = args.next().map(|p| p.parse()).transpose()?.unwrap_or(PORTMAP_PLAYER);
    let label = if pm == PORTMAP_REKORDBOX { "rekordbox" } else { "player" };
    println!("portmap {ip}:{pm} ({label})");
    let mut nfs = Nfs::connect_at(ip, pm)?;
    let exports = nfs.exports()?;
    println!("exports: {}", exports.len());
    for e in &exports { println!("  {:?}  {}", show(e), e.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")); }
    let first = exports.first().cloned().unwrap_or_else(|| "/C/".encode_utf16().flat_map(|u| u.to_le_bytes()).collect());
    let root = nfs.mount(&first)?;
    println!("mounted {:?}", show(&first));
    for d in nfs.readdir(&root)?.iter().take(40) { println!("  {}", d.name); }
    if let Some(path) = args.next() {
        let t0 = std::time::Instant::now();
        let (fh, size) = nfs.lookup_path(&root, &path)?;
        println!("lookup {path:?}: {size} bytes ({:.0} ms)", t0.elapsed().as_secs_f64() * 1e3);
        let t1 = std::time::Instant::now();
        let data = nfs.read_file(&fh, size)?;
        let dt = t1.elapsed().as_secs_f64();
        println!("read {} bytes in {:.2} s = {:.1} MB/s; head {:02x?}", data.len(), dt, data.len() as f64 / dt / 1e6, &data[..data.len().min(8)]);
    }
    Ok(())
}
