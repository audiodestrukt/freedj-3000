// Dev tool: read a rekordbox USB export and list its tracks.
fn main() -> anyhow::Result<()> {
    let root = std::env::args().nth(1).expect("usage: list <usb-root>");
    let exp = opendeck_rekordbox::read_export(std::path::Path::new(&root))?;
    println!("{} tracks under {}", exp.tracks.len(), exp.root.display());
    for t in exp.tracks.iter().take(20) {
        let exists = t.path_on(&exp.root).exists();
        println!("  [{:>3}] {:>6.2} BPM  {:>4}s  {} — {}   ({}{})",
            t.id, t.bpm, t.duration_secs, t.artist, t.title,
            t.rel_path, if exists { "" } else { "  <MISSING>" });
    }
    Ok(())
}
