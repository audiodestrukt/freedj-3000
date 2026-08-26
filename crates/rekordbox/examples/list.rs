use opendeck_rekordbox::{read_export, RbExport};
fn walk(exp: &RbExport, parent: u32, depth: usize) {
    for node in exp.children(parent) {
        let pad = "  ".repeat(depth);
        if node.is_folder {
            println!("{pad}📁 {}", node.name);
            walk(exp, node.id, depth + 1);
        } else {
            let n = exp.playlist_tracks(node.id).len();
            println!("{pad}🎵 {} ({n} tracks)", node.name);
        }
    }
}
fn main() -> anyhow::Result<()> {
    let root = std::env::args().nth(1).expect("usage: list <usb-root>");
    let exp = read_export(std::path::Path::new(&root))?;
    println!("{} tracks, {} playlist nodes", exp.tracks.len(), exp.playlists.len());
    println!("--- playlist tree ---");
    walk(&exp, 0, 0);
    Ok(())
}
