//! What the CDJ-3000 screen shows about another player's track, fetched from
//! that player's database once per track on a worker thread: the detail
//! waveform (the phase-meter slot's WAVEFORM view) and the track's metadata
//! with its cover (the INFO screen's player tabs).
//!
//! When BROWSE has a dbserver session open to that player, the requests go
//! through it (`dbsession::Handle`): a CDJ-3000 drops the first of two
//! sessions from one player number, so a session of our own would cost
//! BROWSE its.  Otherwise a short session of its own is opened per fetch.

use opendeck_dbserver::{Client as DbClient, Slot, TrackMeta};
use opendeck_link::prodj::TrackRef;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use opendeck_types::time::Instant;
use std::time::Duration;

type Key = (u8, u8, u32);

struct State<T> {
    /// What has arrived, per track (a few: tabs switch back and forth).
    have: HashMap<Key, Arc<T>>,
    busy: bool,
    /// A failed track is not asked for again for a while.
    failed: HashMap<Key, Instant>,
}

/// One fetched thing per track of another player.
pub struct RemoteCache<T>(Arc<Mutex<State<T>>>);

impl<T> Clone for RemoteCache<T> { fn clone(&self) -> Self { Self(self.0.clone()) } }
impl<T> Default for RemoteCache<T> {
    fn default() -> Self { Self(Arc::new(Mutex::new(State { have: HashMap::new(), busy: false, failed: HashMap::new() }))) }
}

/// The detail waveform (150 entries a second) and the beat grid of a track.
pub type RemoteWaves = RemoteCache<WaveData>;
pub struct WaveData {
    pub bytes: Vec<u8>,
    pub beats: Vec<opendeck_dbserver::GridBeat>,
    /// The 3-band detail (low, mid, high per 1/150 s) when the player has it:
    /// what the RGB and 3BAND waveform colours are drawn from.
    pub bands: Vec<[u8; 3]>,
    /// The colour detail (`.EXT` PWV5): red, green, blue (0-7) and height (0-31).
    pub rgb: Vec<[u8; 4]>,
}
pub type RemoteInfos = RemoteCache<InfoData>;

/// A track's metadata and cover.
pub struct InfoData { pub meta: TrackMeta, pub art: Option<Vec<u8>> }

pub fn fetch_wave(c: &mut DbClient, slot: Slot, id: u32) -> anyhow::Result<WaveData> {
    let bytes = c.waveform_detail(slot, id)?;
    let beats = c.beat_grid(slot, id).map(|b| opendeck_dbserver::parse_beat_grid(&b)).unwrap_or_default();
    // CDJ-3000 3-band detail (.2EX PWV7, 3 bytes per entry) and the colour
    // detail (.EXT PWV5, 2 bytes per entry).
    let bands = c.anlz_tag(slot, id, b"PWV7", b"2EX").ok()
        .and_then(|b| anlz_entries(&b, 3).map(|d| d.chunks_exact(3).map(|e| [e[0], e[1], e[2]]).collect::<Vec<_>>()))
        .unwrap_or_default();
    let rgb = c.anlz_tag(slot, id, b"PWV5", b"EXT").ok()
        .and_then(|b| anlz_entries(&b, 2).map(|d| d.chunks_exact(2).map(|e| pwv5_entry(u16::from_be_bytes([e[0], e[1]]))).collect::<Vec<_>>()))
        .unwrap_or_default();
    log::info!("remote waveform {id}: {} bytes, {} beats, {} band entries, {} colour entries", bytes.len(), beats.len(), bands.len(), rgb.len());
    Ok(WaveData { bytes, beats, bands, rgb })
}

pub fn fetch_info(c: &mut DbClient, slot: Slot, id: u32) -> anyhow::Result<InfoData> {
    let meta = TrackMeta::from_rows(&c.metadata(slot, id)?);
    let art = if meta.artwork_id != 0 { c.artwork(slot, meta.artwork_id).ok().filter(|b| !b.is_empty()) } else { None };
    Ok(InfoData { meta, art })
}

impl<T: Send + Sync + 'static> RemoteCache<T> {
    /// The thing for the track `t` on the player at `ip`, if it has arrived;
    /// asks for it on a worker thread otherwise, on `shared` (BROWSE's
    /// session) when that is open to the same player.
    pub fn get(&self, ip: Ipv4Addr, our_player: u8, t: TrackRef, shared: Option<crate::dbsession::Handle>,
               fetch: fn(&mut DbClient, Slot, u32) -> anyhow::Result<T>) -> Option<Arc<T>> {
        if t.id == 0 { return None; }
        let key = (t.player, t.slot, t.id);
        let mut s = self.0.lock().ok()?;
        if let Some(d) = s.have.get(&key) { return Some(d.clone()); }
        if s.busy || s.failed.get(&key).is_some_and(|at| at.elapsed() < Duration::from_secs(30)) { return None; }
        s.busy = true;
        drop(s);
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("remote-fetch".into()).spawn(move || {
            let r = (|| -> anyhow::Result<T> {
                let slot = track_slot(t.slot);
                // Each request names its slot, so BROWSE's session (open on
                // some slot of that player) serves this track's too.
                if let Some(h) = shared.filter(|h| h.ip == ip) {
                    return h.call(move |c, _| fetch(c, slot, t.id));
                }
                let device = if (1..=4).contains(&our_player) { our_player } else { 1 };
                let mut c = DbClient::discover(ip, device)?;
                fetch(&mut c, slot, t.id)
            })();
            if let Ok(mut s) = me.0.lock() {
                s.busy = false;
                match r {
                    Ok(v) => {
                        log::info!("remote fetch {key:?}: done");
                        if s.have.len() >= 12 { s.have.clear(); }
                        s.have.insert(key, Arc::new(v));
                    }
                    Err(e) => { log::info!("remote fetch {key:?}: {e:#}"); s.failed.insert(key, Instant::now()); }
                }
            }
        });
        if spawned.is_err() { if let Ok(mut s) = self.0.lock() { s.busy = false; } }
        None
    }
}

/// The dbserver slot of a status packet's track slot byte.
fn track_slot(b: u8) -> Slot {
    match b { 1 => Slot::Cd, 2 => Slot::Sd, 4 | 5 => Slot::Collection, _ => Slot::Usb }
}

/// The fetchers the screen's other-player views share.
#[derive(Default, Clone)]
pub struct Remote { pub waves: RemoteWaves, pub infos: RemoteInfos, play: Arc<Mutex<Option<Smooth>>> }

struct Smooth { player: u8, est: f64, at: Instant }

impl Remote {
    /// The other player's playhead as it is drawn: a free-running estimate
    /// that advances at its pitch every frame and is pulled gently to what
    /// its position packets say.  The packets come in bursts over Wi-Fi, so
    /// taking each one as it arrives would shake the waveform.
    pub fn smooth_playhead(&self, player: u8, target: f64, pitch: f64, playing: bool) -> f64 {
        let Ok(mut g) = self.play.lock() else { return target };
        let now = Instant::now();
        match g.as_mut() {
            Some(s) if s.player == player => {
                let dt = now.duration_since(s.at).as_secs_f64().min(0.25);
                s.at = now;
                if playing { s.est += dt * pitch; }
                let err = target - s.est;
                if err.abs() > 0.6 { s.est = target; } else { s.est += err * (1.0 - (-dt / 0.4).exp()); }
                s.est
            }
            _ => { *g = Some(Smooth { player, est: target, at: now }); target }
        }
    }
}

/// One PWV5 colour-detail entry (big-endian u16): red, green, blue (3 bits
/// each, from the top) and height (5 bits), the low 2 bits unused.
fn pwv5_entry(v: u16) -> [u8; 4] {
    [(v >> 13 & 7) as u8, (v >> 10 & 7) as u8, (v >> 7 & 7) as u8, (v >> 2 & 31) as u8]
}

/// A PWV5 section as it is in the `.EXT` file (`anlz_section`: the tag,
/// header length, section length, entry size, entry count, then the entries
/// after the header) as entries. A player's reply has 4 bytes more in front
/// (`anlz_entries`).
pub(crate) fn pwv5_entries(section: &[u8]) -> Vec<[u8; 4]> {
    if section.len() < 20 { return Vec::new(); }
    let be = |i: usize| u32::from_be_bytes([section[i], section[i + 1], section[i + 2], section[i + 3]]) as usize;
    let (head, size, count) = (be(4), be(12), be(16));
    if size != 2 { return Vec::new(); }
    let Some(data) = section.get(head..) else { return Vec::new() };
    data[..data.len().min(count * 2)].chunks_exact(2).map(|e| pwv5_entry(u16::from_be_bytes([e[0], e[1]]))).collect()
}

/// A PWV4 section as it is in the `.EXT` (the RGB preview: 1,200 columns
/// of 6 bytes) as entries; empty if it is not that shape.
pub(crate) fn pwv4_entries(section: &[u8]) -> Vec<[u8; 6]> {
    if section.len() < 20 { return Vec::new(); }
    let be = |i: usize| u32::from_be_bytes([section[i], section[i + 1], section[i + 2], section[i + 3]]) as usize;
    let (head, size, count) = (be(4), be(12), be(16));
    if size != 6 { return Vec::new(); }
    let Some(data) = section.get(head..) else { return Vec::new() };
    data[..data.len().min(count * 6)].chunks_exact(6).map(|e| [e[0], e[1], e[2], e[3], e[4], e[5]]).collect()
}

/// The entries of an ANLZ tag section as the player sends it: a 4-byte
/// prefix, the tag name, the header length, the section length, the entry
/// size and the entry count, then the entries.
fn anlz_entries(b: &[u8], entry: usize) -> Option<&[u8]> {
    if b.len() < 28 { return None; }
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    let (head, size, count) = (be(8), be(16), be(20));
    if size != entry { return None; }
    let data = b.get(4 + head..)?;
    Some(&data[..data.len().min(count * entry)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `.EXT`'s PWV4 (the RGB preview), 6 bytes a column.
    #[test]
    fn pwv4_from_the_file() {
        let want = vec![[120, 150, 60, 9, 52, 46], [0; 6], [127, 128, 122, 122, 77, 31]];
        let file = opendeck_rekordbox::fixtures::anlz_file(&[opendeck_rekordbox::fixtures::pwv4(&want)]);
        assert_eq!(pwv4_entries(opendeck_rekordbox::anlz_section(&file, b"PWV4").unwrap()), want);
        assert!(pwv4_entries(&opendeck_rekordbox::fixtures::pwv5(&[[1, 2, 3, 4]])).is_empty(), "a PWV5 is not a PWV4");
    }

    /// The `.EXT`'s PWV5 read straight from the file (no reply prefix):
    /// every entry, in order, unpacked.
    #[test]
    fn pwv5_from_the_file_unpacks_every_entry() {
        let want = [[7, 4, 7, 31], [0, 0, 7, 5], [3, 2, 1, 0], [7, 0, 0, 16]];
        let sec = opendeck_rekordbox::fixtures::pwv5(&want);
        assert_eq!(pwv5_entries(&sec), want.to_vec());
        let file = opendeck_rekordbox::fixtures::anlz_file(&[sec]);
        let found = opendeck_rekordbox::anlz_section(&file, b"PWV5").expect("found in the file");
        assert_eq!(pwv5_entries(found), want.to_vec());
        assert!(pwv5_entries(&found[..10]).is_empty(), "a cut-short section reads as none");
    }

    /// A tag reply as a player sends it: 4 bytes before the tag, the tag
    /// name, header length, section length, entry size, entry count, then
    /// the entries after the header.
    fn reply(head: u32, entry: u32, entries: &[u8]) -> Vec<u8> {
        let count = entries.len() as u32 / entry;
        let mut b = vec![0u8; 4];
        b.extend_from_slice(b"PWV7");
        b.extend_from_slice(&head.to_be_bytes());
        b.extend_from_slice(&(head + entries.len() as u32).to_be_bytes());
        b.extend_from_slice(&entry.to_be_bytes());
        b.extend_from_slice(&count.to_be_bytes());
        b.resize(4 + head as usize, 0);
        b.extend_from_slice(entries);
        b
    }

    #[test]
    fn tag_entries_start_after_the_header_and_stop_at_the_count() {
        let e = [1, 2, 3, 4, 5, 6];
        assert_eq!(anlz_entries(&reply(24, 3, &e), 3), Some(&e[..]));
        // A longer header (the CDJ-3000's PWV7 has one) is skipped by its length.
        assert_eq!(anlz_entries(&reply(32, 3, &e), 3), Some(&e[..]));
        // Trailing bytes past the count are not entries.
        let mut b = reply(24, 3, &e);
        b.extend_from_slice(&[9, 9]);
        assert_eq!(anlz_entries(&b, 3), Some(&e[..]));
        // The wrong entry size (a PWV5 read as PWV7) or a short reply: none.
        assert_eq!(anlz_entries(&reply(24, 2, &[1, 2]), 3), None);
        assert_eq!(anlz_entries(&[0u8; 20], 3), None);
    }

    #[test]
    fn pwv5_entries_unpack_colour_and_height() {
        // red 7, green 0, blue 5, height 31
        assert_eq!(pwv5_entry(0b111_000_101_11111_00), [7, 0, 5, 31]);
        assert_eq!(pwv5_entry(0), [0, 0, 0, 0]);
        assert_eq!(pwv5_entry(0b000_111_000_00001_11), [0, 7, 0, 1], "the low 2 bits are not height");
    }

    #[test]
    fn status_slots_map_to_dbserver_slots() {
        assert_eq!(track_slot(1), Slot::Cd);
        assert_eq!(track_slot(2), Slot::Sd);
        assert_eq!(track_slot(3), Slot::Usb);
        assert_eq!(track_slot(4), Slot::Collection);
    }

    /// The other player's playhead as drawn: it runs on at the pitch between
    /// packets, eases toward a packet that is a little off (Wi-Fi bursts),
    /// and jumps to one that is far off (a cue, a loop, a needle search).
    #[test]
    fn the_playhead_runs_on_eases_and_jumps() {
        let r = Remote::default();
        assert_eq!(r.smooth_playhead(2, 10.0, 1.0, true), 10.0, "the first packet is taken as is");
        std::thread::sleep(Duration::from_millis(50));
        let a = r.smooth_playhead(2, 10.0, 1.0, true);
        assert!(a > 10.0 && a < 10.05, "ran on toward the stale packet, not stuck on it: {a}");
        // Paused: a packet 0.2 s off is eased toward, not jumped to.
        let r = Remote::default();
        r.smooth_playhead(2, 10.0, 1.0, false);
        std::thread::sleep(Duration::from_millis(20));
        let b = r.smooth_playhead(2, 10.2, 1.0, false);
        assert!(b > 10.0 && b < 10.2, "eased: {b}");
        // Far off: jumps.
        assert_eq!(r.smooth_playhead(2, 42.0, 1.0, false), 42.0);
        // Another player: starts over at its packet.
        assert_eq!(r.smooth_playhead(3, 5.0, 1.0, true), 5.0);
    }
}
