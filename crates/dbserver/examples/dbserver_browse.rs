//! Browse a Link device's database server: playlists, tracks, one track's metadata.
//!
//!   cargo run -p opendeck-dbserver --example dbserver_browse -- <ip> [device 1-4] [usb|sd|collection]
//!
//! rekordbox: use `collection` and click LINK in rekordbox first.  Players: `usb`.
use opendeck_dbserver::{query_db_port, Client, Slot};
use std::net::Ipv4Addr;

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let mut a = std::env::args().skip(1);
    let ip: Ipv4Addr = a.next().expect("usage: dbserver_browse <ip> [device] [usb|sd|collection]").parse()?;
    let device: u8 = a.next().map(|d| d.parse()).transpose()?.unwrap_or(1);
    let slot = match a.next().as_deref() { Some("sd") => Slot::Sd, Some("collection") | Some("rb") => Slot::Collection, _ => Slot::Usb };

    let port = query_db_port(ip)?;
    println!("{ip}: dbserver on port {port}");
    let mut c = Client::connect(ip, port, device)?;
    println!("set up as device {device}, slot {slot:?}");

    println!("\n== playlists (root folder) ==");
    match c.playlist(slot, 0, true) {
        Ok(items) => for i in &items { println!("  [{:>6}] {:<10} {}", i.id, i.type_name(), i.label); },
        Err(e) => println!("  error: {e:#}"),
    }

    println!("\n== all tracks ==");
    let tracks = c.all_tracks(slot, 0)?;
    println!("  {} tracks", tracks.len());
    for i in tracks.iter().take(30) { println!("  [{:>6}] {:<10} {}  {}", i.id, i.type_name(), i.label, i.label2); }

    if let Some(t) = tracks.first() {
        println!("\n== metadata for #{} ==", t.id);
        for i in c.metadata(slot, t.id)? {
            // Numeric rows (duration in s, tempo ×100, rating, year…) carry the value in `id`.
            println!("  {:<12} {:<32} {:<12} id={}", i.type_name(), i.label, i.label2, i.id);
        }
    }
    Ok(())
}
