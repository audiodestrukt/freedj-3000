//! Count cues per track on an export: `cargo run -p opendeck-rekordbox --example cuecount -- /media/USB`
fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("usb root"));
    let e = opendeck_rekordbox::read_export(&root).unwrap();
    let (mut with_mem, mut with_hot, mut nmem, mut nhot, mut noanlz) = (0, 0, 0, 0, 0);
    for t in &e.tracks {
        let rel = &t.analyze_rel; if rel.is_empty() { noanlz += 1; continue }
        let p = root.join(rel.trim_start_matches('/'));
        match opendeck_rekordbox::read_anlz(&p) {
            Ok(a) => {
                if !a.memory_cues.is_empty() { with_mem += 1 }
                if !a.hot_cues.is_empty() { with_hot += 1 }
                nmem += a.memory_cues.len(); nhot += a.hot_cues.len();
            }
            Err(err) => { noanlz += 1; eprintln!("{}: {err}", p.display()) }
        }
    }
    println!("tracks {}  with memory cues {with_mem} ({nmem})  with hot cues {with_hot} ({nhot})  no/bad anlz {noanlz}", e.tracks.len());
}
