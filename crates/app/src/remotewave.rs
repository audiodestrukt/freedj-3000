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
    have: Option<(Key, Arc<T>)>,
    busy: bool,
    /// A failed track is not asked for again for a while.
    tried: HashMap<Key, Instant>,
}

/// One fetched thing per track of another player.
pub struct RemoteCache<T>(Arc<Mutex<State<T>>>);

impl<T> Clone for RemoteCache<T> { fn clone(&self) -> Self { Self(self.0.clone()) } }
impl<T> Default for RemoteCache<T> {
    fn default() -> Self { Self(Arc::new(Mutex::new(State { have: None, busy: false, tried: HashMap::new() }))) }
}

/// The detail waveform (150 entries a second) and the beat grid of a track.
pub type RemoteWaves = RemoteCache<WaveData>;
pub struct WaveData {
    pub bytes: Vec<u8>,
    pub beats: Vec<opendeck_dbserver::GridBeat>,
    /// The 3-band detail (low, mid, high per 1/150 s) when the player has it:
    /// what the RGB and 3BAND waveform colours are drawn from.
    pub bands: Vec<[u8; 3]>,
}
pub type RemoteInfos = RemoteCache<InfoData>;

/// A track's metadata and cover.
pub struct InfoData { pub meta: TrackMeta, pub art: Option<Vec<u8>> }

pub fn fetch_wave(c: &mut DbClient, slot: Slot, id: u32) -> anyhow::Result<WaveData> {
    let bytes = c.waveform_detail(slot, id)?;
    let beats = c.beat_grid(slot, id).map(|b| opendeck_dbserver::parse_beat_grid(&b)).unwrap_or_default();
    // CDJ-3000 3-band detail (.2EX PWV7), possibly with its section header.
    let bands = c.anlz_tag(slot, id, b"PWV7", b"2EX").map(|b| {
        let n = b.len();
        let d: &[u8] = if n >= 24 && u32::from_be_bytes([b[12], b[13], b[14], b[15]]) == 3
            && 24 + u32::from_be_bytes([b[16], b[17], b[18], b[19]]) as usize * 3 == n { &b[24..] } else { &b[n - n % 3..] };
        d.chunks_exact(3).map(|e| [e[0], e[1], e[2]]).collect::<Vec<_>>()
    }).unwrap_or_default();
    log::info!("remote waveform {id}: {} bytes, {} beats, {} band entries", bytes.len(), beats.len(), bands.len());
    Ok(WaveData { bytes, beats, bands })
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
        if let Some((k, d)) = &s.have { if *k == key { return Some(d.clone()); } }
        if s.busy || s.tried.get(&key).is_some_and(|at| at.elapsed() < Duration::from_secs(30)) { return None; }
        s.busy = true;
        s.tried.insert(key, Instant::now());
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
                    Ok(v) => { log::info!("remote fetch {key:?}: done"); s.have = Some((key, Arc::new(v))); }
                    Err(e) => log::info!("remote fetch {key:?}: {e:#}"),
                }
            }
        });
        if spawned.is_err() { if let Ok(mut s) = self.0.lock() { s.busy = false; } }
        None
    }
}

/// The fetchers the screen's other-player views share.
#[derive(Default, Clone)]
pub struct Remote { pub waves: RemoteWaves, pub infos: RemoteInfos }
