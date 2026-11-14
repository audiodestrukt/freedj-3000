//! What the CDJ-3000 screen shows about another player's track, fetched from
//! that player's database once per track on a worker thread: the detail
//! waveform (the phase-meter slot's WAVEFORM view) and the track's metadata
//! with its cover (the INFO screen's player tabs).

use opendeck_dbserver::{Client as DbClient, Slot, TrackMeta};
use opendeck_link::prodj::TrackRef;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
        .and_then(|b| anlz_entries(&b, 2).map(|d| d.chunks_exact(2).map(|e| {
            let v = u16::from_be_bytes([e[0], e[1]]);
            [(v >> 13 & 7) as u8, (v >> 10 & 7) as u8, (v >> 7 & 7) as u8, (v >> 2 & 31) as u8]
        }).collect::<Vec<_>>()))
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
    /// asks for it on a worker thread otherwise.
    pub fn get(&self, ip: Ipv4Addr, our_player: u8, t: TrackRef,
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
                let device = if (1..=4).contains(&our_player) { our_player } else { 1 };
                let mut c = DbClient::discover(ip, device)?;
                let slot = match t.slot { 1 => Slot::Cd, 2 => Slot::Sd, 4 | 5 => Slot::Collection, _ => Slot::Usb };
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
