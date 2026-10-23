//! Replay a Pro DJ Link capture through OpenDeck's parser, offline.
//!
//! Reads the `.log` written by `tools/link_capture.py` (one packet per line:
//! `time port src len hex -- decoded`) or the app's own debug log (lines with
//! `ProDJ rx :PORT N bytes from ADDR — [51, 73, ...]`), parses every packet
//! with the same code the deck runs, and prints what it understood.  Exits 1
//! if a beat or status packet of a known size failed to parse, which is the
//! signal that a player generation we have not seen (a CDJ-3000, say) sends
//! something the parser does not accept.
//!
//!     cargo run -p opendeck-link --example replay_capture -- link-capture-*.log

use opendeck_link::prodj::*;
use std::collections::BTreeMap;

fn parse_line(line: &str) -> Option<(u16, Vec<u8>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') { return None; }
    if let Some(i) = line.find("ProDJ rx :").or_else(|| line.find("ProDJ port 500")) {
        // App debug log: "... rx :50002 292 bytes from 1.2.3.4:1262 — [51, 73, ...]"
        let rest = &line[i..];
        let port: u16 = rest.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty())
            .find_map(|s| s.parse::<u16>().ok().filter(|p| (50000..=50002).contains(p)))?;
        let open = rest.find('[')?; let close = rest.rfind(']')?;
        let bytes = rest[open + 1..close].split(',')
            .map(|h| u8::from_str_radix(h.trim(), 16)).collect::<Result<Vec<_>, _>>().ok()?;
        return Some((port, bytes));
    }
    // Capture tool: "time port src len hex -- decoded"
    let mut it = line.split_whitespace();
    let _time = it.next()?;
    let port: u16 = it.next()?.parse().ok()?;
    let _src = it.next()?;
    let _len = it.next()?;
    let hex = it.next()?;
    let bytes = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
        .collect::<Result<Vec<_>, _>>().ok()?;
    Some((port, bytes))
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: replay_capture <capture.log>...");
        std::process::exit(2);
    }
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();
    let mut players: BTreeMap<u8, (String, Vec<usize>)> = BTreeMap::new();
    let mut shown: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    for path in &paths {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| { eprintln!("{path}: {e}"); std::process::exit(2) });
        for (n, line) in text.lines().enumerate() {
            let Some((port, data)) = parse_line(line) else { continue };
            let Some(t) = ProDjLink::packet_type(&data) else {
                failed.push(format!("{path}:{} port {port}: not a Pro DJ Link packet ({} bytes)", n + 1, data.len()));
                continue;
            };
            let count = shown.entry((t, 0)).or_insert(0);
            *count += 1;
            let first = *count <= 2;
            match t {
                PKT_ANNOUNCE if port == PORT_ANNOUNCE => match ProDjLink::parse_announce(&data) {
                    Some((player, ip)) => {
                        let name = String::from_utf8_lossy(&data[0x0c..0x20]).trim_end_matches('\0').to_string();
                        players.entry(player).or_insert_with(|| (name.clone(), vec![])).0 = name.clone();
                        if first { println!("announce  player {player} {name:?} {ip}"); }
                        ok += 1;
                    }
                    None => failed.push(format!("{path}:{}: announce of {} bytes rejected", n + 1, data.len())),
                },
                PKT_BEAT => match ProDjLink::parse_beat(&data) {
                    Some(b) => { if first { println!("beat      player {} bpm {:.2} pitch {:+.2}% beat {}/4", b.player, b.bpm, (b.pitch - 1.0) * 100.0, b.beat_in_bar); } ok += 1; }
                    None => failed.push(format!("{path}:{}: beat of {} bytes rejected", n + 1, data.len())),
                },
                PKT_STATUS => match ProDjLink::parse_status(&data) {
                    Some(s) => {
                        let e = players.entry(s.player).or_insert_with(|| (String::new(), vec![]));
                        if !e.1.contains(&data.len()) { e.1.push(data.len()); }
                        if first {
                            println!("status    player {} len {:#x} fw {} {:?} play={} master={} sync={} onair={} pitch {:+.2}% bpm {:?} beat {:?}/{:?} handoff {:?} syncN {}",
                                s.player, data.len(), String::from_utf8_lossy(&s.firmware), s.play, s.playing, s.master, s.sync, s.on_air,
                                (s.pitch - 1.0) * 100.0, s.bpm, s.beat, s.beat_in_bar, s.handoff_to, s.sync_counter);
                        }
                        ok += 1;
                    }
                    None => failed.push(format!("{path}:{}: status of {} bytes rejected", n + 1, data.len())),
                },
                PKT_MASTER_REQ => { if let Some(p) = ProDjLink::parse_master_request(&data) { println!("master-req from player {p}"); ok += 1; } }
                PKT_MASTER_RSP => { if let Some((p, yes)) = ProDjLink::parse_master_response(&data) { println!("master-rsp from player {p} accepted={yes}"); ok += 1; } }
                PKT_SYNC_CTRL => { if let Some((p, _, cmd)) = ProDjLink::parse_sync_control(&data) { println!("sync-ctrl from player {p} cmd {cmd:#x}"); ok += 1; } }
                other => { if first { println!("type {other:#04x} on port {port}: {} bytes (not parsed, fine)", data.len()); } }
            }
        }
    }
    println!("\nparsed {ok} packets; {} rejected", failed.len());
    for (p, (name, lens)) in &players {
        println!("  player {p}: {name:?} status lengths {lens:?}");
    }
    for f in &failed { println!("  REJECTED {f}"); }
    if !failed.is_empty() { std::process::exit(1); }
}
