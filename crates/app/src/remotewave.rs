//! The waveform of the track on another player, for the CDJ-3000 screen's
//! waveform meter (the phase-meter slot's WAVEFORM view): fetched once per
//! track from that player's database (the detail waveform, 150 entries a
//! second) on a worker thread.

use opendeck_dbserver::{Client as DbClient, Slot};
use opendeck_link::prodj::TrackRef;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Key = (u8, u8, u32);

#[derive(Default)]
struct State {
    have: Option<(Key, Arc<Vec<u8>>)>,
    busy: bool,
    /// A failed track is not asked for again for a while.
    tried: HashMap<Key, Instant>,
}

#[derive(Default, Clone)]
pub struct RemoteWaves(Arc<Mutex<State>>);

impl RemoteWaves {
    /// The detail waveform of the track `t` on the player at `ip`, if it has
    /// arrived; asks for it on a worker thread otherwise.
    pub fn get(&self, ip: Ipv4Addr, our_player: u8, t: TrackRef) -> Option<Arc<Vec<u8>>> {
        if t.id == 0 { return None; }
        let key = (t.player, t.slot, t.id);
        let mut s = self.0.lock().ok()?;
        if let Some((k, d)) = &s.have { if *k == key { return Some(d.clone()); } }
        if s.busy || s.tried.get(&key).is_some_and(|at| at.elapsed() < Duration::from_secs(30)) { return None; }
        s.busy = true;
        s.tried.insert(key, Instant::now());
        drop(s);
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("remote-wave".into()).spawn(move || {
            let r = (|| -> anyhow::Result<Vec<u8>> {
                let device = if (1..=4).contains(&our_player) { our_player } else { 1 };
                let mut c = DbClient::discover(ip, device)?;
                let slot = match t.slot { 1 => Slot::Cd, 2 => Slot::Sd, 4 | 5 => Slot::Collection, _ => Slot::Usb };
                c.waveform_detail(slot, t.id)
            })();
            if let Ok(mut s) = me.0.lock() {
                s.busy = false;
                match r {
                    Ok(b) => { log::info!("remote waveform {key:?}: {} bytes", b.len()); s.have = Some((key, Arc::new(b))); }
                    Err(e) => log::info!("remote waveform {key:?}: {e:#}"),
                }
            }
        });
        if spawned.is_err() { if let Ok(mut s) = self.0.lock() { s.busy = false; } }
        None
    }
}
