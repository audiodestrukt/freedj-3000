//! Fire one raw dbserver request at a device and dump the reply — for
//! discovering undocumented requests (e.g. the file-path query a CDJ makes
//! before loading a track from rekordbox).
//!
//!   dbserver_probe <ip> <device> <slot> <track-id> <type-hex> [menu-byte] [extra-u32...]
use opendeck_dbserver::{kind, query_db_port, Client, Field, Message, Slot, TrackType};
use std::net::Ipv4Addr;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let ip: Ipv4Addr = a[0].parse()?;
    let device: u8 = a[1].parse()?;
    let slot = match a[2].as_str() { "sd" => Slot::Sd, "collection" | "rb" => Slot::Collection, _ => Slot::Usb };
    let id: u32 = a[3].parse()?;
    let ty = u16::from_str_radix(a[4].trim_start_matches("0x"), 16)?;
    let menu: u8 = a.get(5).map(|m| m.parse()).transpose()?.unwrap_or(1);
    // Extra args after the DMST; the token ID stands for the track id, hex is allowed.
    let extra: Vec<Field> = a.iter().skip(6).map(|x| Field::U32(if x == "ID" { id } else if let Some(h) = x.strip_prefix("0x") { u32::from_str_radix(h, 16).unwrap() } else { x.parse().unwrap() })).collect();
    let explicit = !extra.is_empty();

    let port = query_db_port(ip)?;
    let mut c = Client::connect(ip, port, device)?;
    let mut args = vec![c.dmst(menu, slot, TrackType::Rekordbox)];
    if explicit { args.extend(extra); } else { args.push(Field::U32(id)); }
    println!("-> 0x{ty:04x} {args:?}");
    let r = c.request(ty, args)?;
    println!("<- 0x{:04x} {:?}", r.kind, r.args.iter().map(short).collect::<Vec<_>>());
    // PROBE_DUMP=<file>: write the reply's blob argument out whole, for layout work.
    if let (Ok(path), Some(Field::Blob(b))) = (std::env::var("PROBE_DUMP"), r.args.iter().find(|f| matches!(f, Field::Blob(_)))) {
        std::fs::write(&path, b)?;
        println!("   blob written to {path}");
    }
    if r.kind == kind::MENU_AVAILABLE {
        let count = r.args.get(1).map(|f| f.as_u32()).unwrap_or(0);
        println!("   menu with {count} items; rendering");
        // Render like Client::menu does, but print raw rows so unknown item shapes show.
        let items = c.menu(ty, slot, TrackType::Rekordbox, vec![Field::U32(id)])?;
        for i in items { println!("   [{:>10}] {:<12} type=0x{:04x} {:?} {:?} flags={} art={} pos={}", i.id, i.type_name(), i.item_type, i.label, i.label2, i.flags, i.artwork_id, i.position); }
    }
    Ok(())
}

fn short(f: &Field) -> String {
    match f {
        Field::Blob(b) => format!("blob[{}] {}", b.len(), b.iter().take(48).map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")),
        Field::Str(s) => format!("{s:?}"),
        other => format!("{other:?}"),
    }
}
#[allow(dead_code)] fn _unused(_: Message) {}
