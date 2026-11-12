//! Which tracks on a linked player have analysis files of a given kind?
//! Reads the player's export.pdb over NFS and checks each track's ANLZ path.
//!
//!   cargo run --release -p opendeck-app --example link_anlz_ids -- <player-ip> [EXT|2EX|DAT] [max]
use opendeck_nfs::Nfs;
use std::net::Ipv4Addr;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let ip: Ipv4Addr = a.next().expect("usage: link_anlz_ids <ip> [ext] [max]").parse()?;
    let ext = a.next().unwrap_or_else(|| "2EX".into());
    let max: usize = a.next().map(|m| m.parse()).transpose()?.unwrap_or(10);
    let mut nfs = Nfs::connect(ip)?;
    let root = nfs.mount_usb()?;
    let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb")?;
    let pdb = nfs.read_file(&fh, size)?;
    let exp = opendeck_rekordbox::read_export_from(&mut std::io::Cursor::new(pdb), "nfs://player".into())?;
    let mut found = 0;
    for t in &exp.tracks {
        if t.analyze_rel.is_empty() { continue; }
        let p = t.analyze_rel.trim_start_matches('/').replace(".DAT", &format!(".{ext}"));
        if let Ok((_, n)) = nfs.lookup_path(&root, &p) {
            println!("{:>5}  {:>7} B  {}  {} - {}", t.id, n, p, t.artist, t.title);
            found += 1;
            if found >= max { break; }
        }
    }
    Ok(())
}
