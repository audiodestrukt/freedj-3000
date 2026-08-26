use opendeck_rekordbox::{read_export, read_anlz};
fn main() -> anyhow::Result<()> {
    let root = std::env::args().nth(1).expect("usage: list <usb-root> [track_id]");
    let exp = read_export(std::path::Path::new(&root))?;
    println!("{} tracks, {} playlist nodes", exp.tracks.len(), exp.playlists.len());
    // Analysis dump for one track (default: the 125 BPM Nostalgix OG Sins).
    let want = std::env::args().nth(2).and_then(|s| s.parse::<u32>().ok());
    for t in exp.tracks.iter().filter(|t| want.map_or(t.title.contains("OG Sins"), |id| t.id == id)).take(1) {
        println!("\n=== {} — {} ({} BPM) ===", t.artist, t.title, t.bpm);
        match t.analyze_on(&exp.root) {
            Some(p) if p.exists() => {
                let a = read_anlz(&p)?;
                println!("beats: {}  memory cues: {}  hot cues: {}",
                    a.beats.len(), a.memory_cues.len(), a.hot_cues.len());
                for b in a.beats.iter().take(5) {
                    println!("  beat @ {:>6}ms  {:.2} BPM  bar-pos {}", b.time_ms, b.bpm, b.beat_in_bar);
                }
                for c in a.memory_cues.iter().take(5) {
                    println!("  memory cue @ {:>6}ms  loop={} hot={:?}", c.time_ms, c.is_loop, c.hot_cue);
                }
                for c in a.hot_cues.iter().take(8) {
                    println!("  HOT cue {:?} @ {:>6}ms loop={}", c.hot_cue, c.time_ms, c.is_loop);
                }
            }
            _ => println!("  (no ANLZ file)"),
        }
    }
    Ok(())
}
