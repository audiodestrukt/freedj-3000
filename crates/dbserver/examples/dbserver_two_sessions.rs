//! Does a second dbserver session from the same device number break the
//! first? The app's browser holds one session while a LOAD opens another.
//!
//! `cargo run -p opendeck-dbserver --example dbserver_two_sessions -- <ip> [device] [usb|sd|collection]`

use opendeck_dbserver::{query_db_port, Client, Slot};
use std::net::Ipv4Addr;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let ip: Ipv4Addr = a.next().expect("usage: <ip> [device] [slot]").parse()?;
    let device: u8 = a.next().map(|d| d.parse()).transpose()?.unwrap_or(2);
    let slot = match a.next().as_deref() { Some("sd") => Slot::Sd, Some("collection") | Some("rb") => Slot::Collection, _ => Slot::Usb };
    let port = query_db_port(ip)?;
    let mut first = Client::connect(ip, port, device)?;
    println!("A root menu: {:?}", first.root_menu(slot).map(|v| v.len()));
    let mut second = Client::connect(ip, port, device)?;
    println!("B root menu: {:?}", second.root_menu(slot).map(|v| v.len()));
    println!("A again:     {:?}", first.root_menu(slot).map(|v| v.len()));
    drop(second);
    println!("A after B closed: {:?}", first.root_menu(slot).map(|v| v.len()));
    Ok(())
}
