//! Probe a Link device's NFS: portmap (111 for players, 50111 for rekordbox),
//! list exports, mount the first one and READDIR its root.
//!
//!   cargo run -p opendeck-nfs --example nfs_probe -- 192.168.68.60 50111
//!   cargo run -p opendeck-nfs --example nfs_probe -- 192.168.68.58
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
    for d in nfs.readdir(&root)? { println!("  {}", d.name); }
    Ok(())
}
