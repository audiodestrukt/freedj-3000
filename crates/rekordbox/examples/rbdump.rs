fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let e = opendeck_rekordbox::read_export(&root).unwrap();
    println!("tracks {}  playlists {}", e.tracks.len(), e.playlists.len());
    for n in &e.playlists { println!("  pl id={} parent={} folder={} name={:?}", n.id, n.parent_id, n.is_folder, n.name); }
    for t in e.tracks.iter().take(5) { println!("  tr id={} {:?} rel={:?} anlz={:?}", t.id, t.title, t.rel_path, t.analyze_rel); }
}
