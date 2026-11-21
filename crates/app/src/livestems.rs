//! The media server's side of **live stems**: a player asks for a served
//! track's stems by looking up `/CDJMODS/live/<track id>` over NFS; the
//! lookup misses, the NFS server tells us ([`LiveStems::hook`]), and we
//! split the track into a live bank (`opendeck_stems::live`) that the
//! player reads page by page while the split runs.  Design:
//! `docs/design/serving-split-stems.md`.
//!
//! One track at a time, newest request first.  The track is decoded at
//! 44.1 kHz (the model's rate and the live bank's), split into a
//! [`StemBank`] as the deck's own split is, and each 32,768-frame page is
//! written out once every sample in it is there.

use crate::separate::{self, Job};
use crate::stems::{StemBank, SPLIT_SCALE};
use anyhow::{Context, Result};
use opendeck_stems::live::{self, LiveWriter, PAGE_FRAMES, RATE};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Splits `track` (interleaved stereo at 44.1 kHz) into `bank`.
pub type SplitFn = Arc<dyn Fn(&Arc<Vec<f32>>, &Job, &StemBank) -> Result<()> + Send + Sync>;

/// The PCM in a live bank is stored this much below its level, as a
/// device split's bank is ([`SPLIT_SCALE`]).
pub const HEADROOM_DB: u8 = 6;

pub struct LiveStems {
    /// The local folder served as `/CDJMODS` (live banks go in `live/`).
    cdjmods: PathBuf,
    files: HashMap<u32, PathBuf>,
    /// Waiting requests, and the track being split now.
    queue: Mutex<(VecDeque<u32>, Option<u32>)>,
    wake: Condvar,
    /// Where each asking player is in the track (interleaved samples at
    /// 44.1 kHz), for the split to start there and follow its jumps.
    playheads: Mutex<HashMap<u32, Arc<std::sync::atomic::AtomicU64>>>,
    split: SplitFn,
    /// Bytes of live banks kept; the oldest finished ones go first.
    cap: u64,
}

/// How much the cache keeps by default (`OPENDECK_LIVE_STEMS_MB`
/// overrides): about 12 four-minute tracks on iOS, 24 elsewhere.
fn default_cap() -> u64 {
    let mb = std::env::var("OPENDECK_LIVE_STEMS_MB").ok().and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(if cfg!(any(target_os = "ios", target_os = "android")) { 2048 } else { 4096 });
    mb << 20
}

/// What a live bank of `frames` takes on disk.
fn bank_bytes(frames: u64) -> u64 { 4 * (44 + frames * 4) }

fn dir_bytes(d: &std::path::Path) -> u64 {
    std::fs::read_dir(d).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// The track ID a live-bank path names (`/CDJMODS/live/12`, or a file in it).
pub fn requested_id(nfs: &str) -> Option<u32> { requested(nfs).map(|r| r.0) }

/// The track ID a live-bank path names and the page the player is at, when
/// it says (`/CDJMODS/live/12@40`: track 12, page 40).  A player tells the
/// source where it is by looking that name up; the lookup always misses.
pub fn requested(nfs: &str) -> Option<(u32, Option<usize>)> {
    let name = nfs.strip_prefix("/CDJMODS/live/")?.split('/').next()?;
    let (id, at) = match name.split_once('@') { Some((i, p)) => (i, Some(p.parse().ok()?)), None => (name, None) };
    Some((id.parse().ok().filter(|id| *id > 0)?, at))
}

/// The name a player looks up to say it is at `page` of track `id`.
pub fn at_path(id: u32, page: usize) -> String { format!("{}@{page}", live::dir(id)) }

impl LiveStems {
    /// Serve live stems of `files` (track ID → local file) from `cdjmods`.
    pub fn start(cdjmods: PathBuf, files: &[(u32, PathBuf)], split: SplitFn) -> Result<Arc<LiveStems>> {
        Self::start_capped(cdjmods, files, split, default_cap())
    }

    /// [`start`](Self::start) keeping at most `cap` bytes of banks.
    pub fn start_capped(cdjmods: PathBuf, files: &[(u32, PathBuf)], split: SplitFn, cap: u64) -> Result<Arc<LiveStems>> {
        std::fs::create_dir_all(cdjmods.join("live"))?;
        // Track IDs follow the folder's scan order, so a bank cached under an
        // ID may now be another track's: keep only those still matching.
        let by_id: HashMap<u32, PathBuf> = files.iter().cloned().collect();
        for e in std::fs::read_dir(cdjmods.join("live"))?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let keep = name.parse::<u32>().ok().and_then(|id| by_id.get(&id))
                .is_some_and(|p| std::fs::read_to_string(e.path().join(SOURCE)).ok() == Some(source_key(p)));
            if !keep { let _ = std::fs::remove_dir_all(e.path()); }
        }
        let me = Arc::new(LiveStems { cdjmods, files: files.iter().cloned().collect(), queue: Mutex::new((VecDeque::new(), None)), wake: Condvar::new(), playheads: Mutex::new(HashMap::new()), split, cap });
        let w = Arc::clone(&me);
        std::thread::Builder::new().name("live-stems".into()).spawn(move || w.run())?;
        Ok(me)
    }

    /// The NFS server's lookup-miss hook.
    pub fn hook(self: &Arc<Self>) -> opendeck_nfs::server::MissHook {
        let me = Arc::clone(self);
        Arc::new(move |p: &str| if let Some((id, at)) = requested(p) {
            if let Some(page) = at { me.player_at(id, page); }
            me.request(id)
        })
    }

    /// A player of track `id` is at live page `page`.
    pub fn player_at(&self, id: u32, page: usize) {
        if !self.files.contains_key(&id) { return; }
        self.playhead(id).store((page * PAGE_FRAMES * 2) as u64, std::sync::atomic::Ordering::Relaxed);
    }

    fn playhead(&self, id: u32) -> Arc<std::sync::atomic::AtomicU64> {
        Arc::clone(self.playheads.lock().unwrap().entry(id).or_default())
    }

    /// Split track `id` next (a served track with no live bank yet).
    pub fn request(&self, id: u32) {
        if !self.files.contains_key(&id) { return; }
        let mut q = self.queue.lock().unwrap();
        // A player polls by looking up again: one split per track.
        if q.1 == Some(id) || q.0.front() == Some(&id) { return; }
        q.0.retain(|x| *x != id);
        q.0.push_front(id);
        log::info!("live stems: track {id} asked for ({} waiting)", q.0.len());
        self.wake.notify_one();
    }

    fn run(&self) {
        loop {
            let id = {
                let mut q = self.queue.lock().unwrap();
                loop {
                    if let Some(id) = q.0.pop_front() { q.1 = Some(id); break id; }
                    q = self.wake.wait(q).unwrap();
                }
            };
            let t0 = std::time::Instant::now();
            match self.split_one(id) {
                Ok(pages) => log::info!("live stems: track {id} split, {pages} pages in {:.1}s", t0.elapsed().as_secs_f32()),
                Err(e) => log::warn!("live stems: track {id}: {e:#}"),
            }
            self.queue.lock().unwrap().1 = None;
        }
    }

    fn split_one(&self, id: u32) -> Result<usize> {
        let path = self.files.get(&id).context("not served")?;
        let pages = self.cdjmods.join("live").join(id.to_string()).join("pages");
        if let Ok(m) = std::fs::read(&pages).map_err(anyhow::Error::from).and_then(|b| live::Map::parse(&b)) {
            let same = std::fs::read_to_string(pages.with_file_name(SOURCE)).ok() == Some(source_key(path));
            if m.state == live::State::Complete && same { return Ok(m.pages()); }
        }
        let track = Arc::new(decode_44k1(path)?);
        let frames = track.len() / 2;
        self.make_room(id, bank_bytes(frames as u64));
        let mut w = LiveWriter::create(&self.cdjmods, id, frames as u64, HEADROOM_DB)?;
        std::fs::write(w.dir.join(SOURCE), source_key(path))?;
        let bank = Arc::new(StemBank::filling_split(&track));
        let job = Job { playhead: Some(self.playhead(id)), ..Default::default() };
        let done = std::sync::atomic::AtomicBool::new(false);
        let res = std::thread::scope(|s| {
            let h = s.spawn(|| { let r = (self.split)(&track, &job, &bank); done.store(true, std::sync::atomic::Ordering::Release); r });
            // Write each page out as soon as the split has filled it.
            while !done.load(std::sync::atomic::Ordering::Acquire) {
                if let Err(e) = write_ready(&mut w, &bank, &track) { job.cancel.store(true, std::sync::atomic::Ordering::Relaxed); let _ = h.join(); return Err(e); }
                std::thread::sleep(Duration::from_millis(40));
            }
            h.join().map_err(|_| anyhow::anyhow!("split panicked"))?
        });
        if let Err(e) = res.and_then(|_| write_ready(&mut w, &bank, &track)) {
            w.fail()?;
            return Err(e);
        }
        let pages = w.map().pages();
        w.finish()?;
        Ok(pages)
    }
}

impl LiveStems {
    /// Delete the oldest finished banks (not `id`'s) until `need` more bytes
    /// fit under the cap.
    fn make_room(&self, id: u32, need: u64) {
        let live = self.cdjmods.join("live");
        let mut banks: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(&live).map(|rd| rd.flatten()
            .filter(|e| e.file_name().to_string_lossy().parse::<u32>().is_ok_and(|n| n != id))
            .map(|e| {
                let t = std::fs::metadata(e.path().join("pages")).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                (t, dir_bytes(&e.path()), e.path())
            }).collect()).unwrap_or_default();
        let mut used: u64 = banks.iter().map(|b| b.1).sum();
        banks.sort();
        for (_, bytes, path) in banks {
            if used + need <= self.cap { break; }
            log::info!("live stems: cache full, removing {}", path.display());
            let _ = std::fs::remove_dir_all(&path);
            used -= bytes;
        }
    }
}

/// Hidden (never served): which file a live bank was split from.
const SOURCE: &str = ".source";

fn source_key(p: &std::path::Path) -> String {
    let md = std::fs::metadata(p).ok();
    let t = md.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    format!("{}\n{}\n{t}\n", p.display(), md.map_or(0, |m| m.len()))
}

fn to_i16(x: f32) -> i16 { (x / SPLIT_SCALE * 32768.0).round().clamp(-32768.0, 32767.0) as i16 }

/// Write every page the bank has filled that isn't written yet.
fn write_ready(w: &mut LiveWriter, bank: &StemBank, track: &[f32]) -> Result<()> {
    for p in 0..w.map().pages() {
        if w.map().done[p] { continue; }
        let (a, b) = (p * PAGE_FRAMES * 2, ((p + 1) * PAGE_FRAMES * 2).min(track.len()));
        if !bank.range_ready(a..b) { continue; }
        // Live roles in opendeck_stems::Role::ALL order: vocal, drums,
        // harmonics, full mix; the bank holds drums, vocal, harmonics.
        let stem = |s: usize| -> Vec<i16> { (a..b).map(|i| bank.sample(s, i)).collect() };
        let full: Vec<i16> = track[a..b].iter().map(|v| to_i16(*v)).collect();
        let (vocal, drums, harm) = (stem(1), stem(0), stem(2));
        w.write_page(p, [&vocal, &drums, &harm, &full])?;
    }
    Ok(())
}

/// `path` decoded to interleaved stereo at 44.1 kHz.
fn decode_44k1(path: &std::path::Path) -> Result<Vec<f32>> {
    let (s, sr, ch, _) = crate::audio::decode_file(path)?;
    let stereo: Vec<f32> = match ch {
        2 => s,
        1 => s.iter().flat_map(|v| [*v, *v]).collect(),
        n => s.chunks_exact(n).flat_map(|f| [f[0], f[1]]).collect(),
    };
    crate::audio::resample_interleaved(&stereo, 2, sr, RATE)
}

/// The split the app runs: the device's separation model, released again
/// once the split is done (a media server splits now and then; the model
/// is hundreds of MB).
pub fn model_split() -> SplitFn {
    Arc::new(|track, job, bank| {
        use separate::model;
        anyhow::ensure!(model::present(), "no separation model on this device");
        let (_, left) = model::memory_mb();
        let need = model::split_needs_mb();
        if left < need { return Err(separate::NotEnoughMemory { need_mb: need, have_mb: left }.into()); }
        let r = model::with(|m| separate::separate(track, RATE, m, job, bank));
        model::release();
        r.map(|_| ())
    })
}

/// Where a media server keeps the stems it splits (not the music folder).
pub fn cache_dir(app_cache: &std::path::Path) -> PathBuf { app_cache.join("cdjmods") }

#[allow(dead_code)]
pub fn live_dir(id: u32) -> String { live::dir(id) }

#[cfg(test)]
mod tests {
    use super::*;
    use opendeck_stems::live::{LiveReader, State};
    use std::sync::mpsc;

    /// What a deck does with it: a track loaded from another OpenDeck, its
    /// STEMS opened, plays the stems the source splits, the start of the
    /// track before the split is done, at the deck's own rate.  A source
    /// with no live stems answers at once, so the deck can split instead.
    #[test]
    fn a_deck_plays_the_stems_another_opendeck_splits() {
        let music = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let frames = 44_100 * 9;
        let pcm: Vec<i16> = (0..frames * 2).map(|i| (((i / 2) as f32 * 0.021).sin() * 9_000.0 + ((i / 2) as f32 * 0.0037).sin() * 6_000.0) as i16).collect();
        let mut wav = opendeck_stems::pgz::wav_header(frames as u32 * 4).to_vec();
        wav.extend(pcm.iter().flat_map(|v| v.to_le_bytes()));
        let file = music.path().join("t.wav");
        std::fs::write(&file, wav).unwrap();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go_rx = Mutex::new(go_rx);
        let split: SplitFn = Arc::new(move |t, _job, bank| {
            let part = |a: usize, b: usize| {
                let pick = |k: f32| -> Vec<i16> { t[a..b].iter().map(|v| to_i16(v * k)).collect() };
                bank.write(a, [&pick(0.25), &pick(0.5), &pick(0.25)]);
            };
            let first = 7 * PAGE_FRAMES * 2;   // ~5.2 s (one 4 s step and its margin), then it waits
            part(0, first);
            let _ = go_rx.lock().unwrap().recv();
            part(first, t.len());
            Ok(())
        });
        let mods = cache.path().join("cdjmods");
        let live = LiveStems::start(mods.clone(), &[(1, file.clone())], split).unwrap();
        let srv = opendeck_nfs::server::NfsServer::start(opendeck_nfs::server::Tree::scan(music.path()).unwrap(), "/C/", 0, 0).unwrap();
        srv.tree.overlay("CDJMODS", &mods).unwrap();
        srv.tree.on_miss(live.hook());
        let media = || crate::stems::NfsMedia::mount_at(std::net::Ipv4Addr::LOCALHOST, srv.portmap, "/C/").unwrap();

        for deck_sr in [44_100u32, 48_000] {
            let (s, sr, _, _) = crate::audio::decode_file(&file).unwrap();
            let track = Arc::new(crate::audio::resample_interleaved(&s, 2, sr, deck_sr).unwrap());
            let job = Arc::new(Job { playhead: Some(Arc::new(std::sync::atomic::AtomicU64::new(0))), ..Default::default() });
            let (j, t) = (Arc::clone(&job), Arc::clone(&track));
            let mut m = media();
            let reader = std::thread::spawn(move || crate::stems::live_from(&mut m, 1, &t, deck_sr, &j, Duration::from_secs(10)));
            if deck_sr == 44_100 {
                // The deck has the first seconds of stems while the source is still splitting.
                let t0 = std::time::Instant::now();
                while !job.bank.get().is_some_and(|b| b.range_ready(0..deck_sr as usize * 2 * 2)) {
                    assert!(t0.elapsed() < Duration::from_secs(10), "no early stems");
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(!job.bank.get().unwrap().complete(), "the split isn't done yet");
                go_tx.send(()).unwrap();
            }
            let bank = reader.join().unwrap().unwrap().expect("live stems");
            assert!(bank.complete());
            let (drums, vocal) = (bank.stem_levels(0), bank.stem_levels(1));
            let err = |stem: &[f32], k: f32| track.iter().zip(stem).map(|(x, y)| (x * k - y).abs()).fold(0f32, f32::max);
            let tol = if deck_sr == 44_100 { 1e-4 } else { 0.01 };
            assert!(err(&drums, 0.25) < tol && err(&vocal, 0.5) < tol, "{deck_sr} Hz: drums {} vocal {}", err(&drums, 0.25), err(&vocal, 0.5));
        }

        // A source with no live stems (no /CDJMODS/live): no answer, no wait.
        let plain = opendeck_nfs::server::NfsServer::start(opendeck_nfs::server::Tree::scan(music.path()).unwrap(), "/C/", 0, 0).unwrap();
        let mut m = crate::stems::NfsMedia::mount_at(std::net::Ipv4Addr::LOCALHOST, plain.portmap, "/C/").unwrap();
        let t0 = std::time::Instant::now();
        let track = Arc::new(vec![0f32; 1000]);
        assert!(crate::stems::live_from(&mut m, 1, &track, 44_100, &Job::default(), Duration::from_secs(10)).unwrap().is_none());
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    /// The source splits from where the player is: STEMS opened 6 s in,
    /// the split starts at that page; the player jumps back to 2 s and that
    /// page is split next, not after the rest of the track.
    #[test]
    fn the_source_splits_from_the_players_playhead_and_follows_its_jumps() {
        let music = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let frames = 44_100 * 30;
        let pcm: Vec<i16> = (0..frames * 2).map(|i| (((i / 2) as f32 * 0.02).sin() * 8_000.0) as i16).collect();
        let mut wav = opendeck_stems::pgz::wav_header(frames as u32 * 4).to_vec();
        wav.extend(pcm.iter().flat_map(|v| v.to_le_bytes()));
        let file = music.path().join("t.wav");
        std::fs::write(&file, wav).unwrap();
        // A stand-in that splits as the real one does: the page at the
        // playhead (or the next unsplit one after it) each time round.
        let order = Arc::new(Mutex::new(Vec::<usize>::new()));
        let o = Arc::clone(&order);
        let split: SplitFn = Arc::new(move |t, job, bank| {
            let ps = PAGE_FRAMES * 2;
            let n = t.len().div_ceil(ps);
            let mut done = vec![false; n];
            while let Some(p) = {
                let at = job.playhead.as_ref().unwrap().load(std::sync::atomic::Ordering::Relaxed) as usize / ps;
                (at.min(n - 1)..n).chain(0..at.min(n - 1)).find(|&p| !done[p])
            } {
                let (a, b) = (p * ps, ((p + 1) * ps).min(t.len()));
                let st: Vec<i16> = t[a..b].iter().map(|v| to_i16(v / 3.0)).collect();
                bank.write(a, [&st, &st, &st]);
                done[p] = true;
                o.lock().unwrap().push(p);
                std::thread::sleep(Duration::from_millis(30));
            }
            Ok(())
        });
        let mods = cache.path().join("cdjmods");
        let live = LiveStems::start(mods.clone(), &[(1, file.clone())], split).unwrap();
        let srv = opendeck_nfs::server::NfsServer::start(opendeck_nfs::server::Tree::scan(music.path()).unwrap(), "/C/", 0, 0).unwrap();
        srv.tree.overlay("CDJMODS", &mods).unwrap();
        srv.tree.on_miss(live.hook());

        let track = Arc::new(pcm.iter().map(|v| *v as f32 / 32768.0).collect::<Vec<f32>>());
        let head = Arc::new(std::sync::atomic::AtomicU64::new(44_100 * 2 * 6));
        let job = Arc::new(Job { playhead: Some(Arc::clone(&head)), ..Default::default() });
        let (j, t) = (Arc::clone(&job), Arc::clone(&track));
        let mut m = crate::stems::NfsMedia::mount_at(std::net::Ipv4Addr::LOCALHOST, srv.portmap, "/C/").unwrap();
        let reader = std::thread::spawn(move || crate::stems::live_from(&mut m, 1, &t, 44_100, &j, Duration::from_secs(10)));
        let t0 = std::time::Instant::now();
        while order.lock().unwrap().len() < 4 { assert!(t0.elapsed() < Duration::from_secs(10)); std::thread::sleep(Duration::from_millis(5)); }
        head.store(44_100 * 2 * 2, std::sync::atomic::Ordering::Relaxed);   // the player jumps back to 2 s
        let bank = reader.join().unwrap().unwrap().expect("live stems");
        assert!(bank.complete());
        let order = order.lock().unwrap().clone();
        let page_of = |secs: usize| secs * 44_100 / PAGE_FRAMES;
        assert_eq!(order[0], page_of(6), "the split starts where the player is: {order:?}");
        let jumped = order.iter().position(|p| *p == page_of(2)).unwrap();
        assert!(jumped < 12, "the page jumped to is split soon after the jump, not at the end: {order:?}");
        assert!(order[..4].iter().all(|p| *p >= page_of(6)), "{order:?}");
    }

    /// The cache keeps the newest banks under its cap.
    #[test]
    fn a_full_cache_drops_the_oldest_banks() {
        let music = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let frames = 44_100;
        let mut wav = opendeck_stems::pgz::wav_header(frames as u32 * 4).to_vec();
        wav.extend(std::iter::repeat(0u8).take(frames * 4));
        let files: Vec<(u32, PathBuf)> = (1..=3).map(|i| { let p = music.path().join(format!("{i}.wav")); std::fs::write(&p, &wav).unwrap(); (i, p) }).collect();
        let split: SplitFn = Arc::new(|t, _, bank| { let z = vec![0i16; t.len()]; bank.write(0, [&z, &z, &z]); Ok(()) });
        let mods = cache.path().join("cdjmods");
        // Room for two banks.
        let live = LiveStems::start_capped(mods.clone(), &files, split, 2 * bank_bytes(frames as u64) + 4096).unwrap();
        for id in 1..=3 {
            live.split_one(id).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        let kept: Vec<bool> = (1..=3).map(|i| mods.join(format!("live/{i}/pages")).exists()).collect();
        assert_eq!(kept, vec![false, true, true], "the oldest went");
    }

    #[test]
    fn a_live_request_is_a_track_id() {
        assert_eq!(requested_id("/CDJMODS/live/12"), Some(12));
        assert_eq!(requested_id("/CDJMODS/live/12/pages"), Some(12));
        assert_eq!(requested(&at_path(12, 40)), Some((12, Some(40))));
        assert_eq!(requested("/CDJMODS/live/12@x"), None);
        for p in ["/CDJMODS/live/0", "/CDJMODS/live/x", "/CDJMODS/index.json", "/live/3"] { assert_eq!(requested_id(p), None, "{p}"); }
    }

    /// End to end over NFS: a player looks up a served track's live folder,
    /// which starts the split; it reads the first pages while the split is
    /// still running, then the rest, every page as the split made it.
    #[test]
    fn a_player_reads_stems_while_the_source_splits() {
        let music = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        // 3.5 s of a 44.1 kHz stereo WAV: five live pages, the last short.
        let frames = 44_100 * 7 / 2;
        let pcm: Vec<i16> = (0..frames * 2).map(|i| (((i / 2) as f32 * 0.03).sin() * 12_000.0) as i16).collect();
        let mut wav = opendeck_stems::pgz::wav_header(frames as u32 * 4).to_vec();
        wav.extend(pcm.iter().flat_map(|v| v.to_le_bytes()));
        std::fs::write(music.path().join("t.wav"), wav).unwrap();

        // A stand-in split: drums = a quarter of the mix, vocal = minus a
        // quarter, harmonics = half; the first two pages, then it waits.
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go_rx = Mutex::new(go_rx);
        let split: SplitFn = Arc::new(move |t, _job, bank| {
            let part = |a: usize, b: usize| {
                let pick = |k: f32| -> Vec<i16> { t[a..b].iter().map(|v| to_i16(v * k)).collect() };
                bank.write(a, [&pick(0.25), &pick(-0.25), &pick(0.5)]);
            };
            let half = 2 * PAGE_FRAMES * 2;
            part(0, half);
            go_rx.lock().unwrap().recv().unwrap();
            part(half, t.len());
            Ok(())
        });
        let mods = cache.path().join("cdjmods");
        let live = LiveStems::start(mods.clone(), &[(1, music.path().join("t.wav"))], split).unwrap();
        let srv = opendeck_nfs::server::NfsServer::start(opendeck_nfs::server::Tree::scan(music.path()).unwrap(), "/C/", 0, 0).unwrap();
        srv.tree.overlay("CDJMODS", &mods).unwrap();
        srv.tree.on_miss(live.hook());

        let media = || crate::stems::NfsMedia::mount_at(std::net::Ipv4Addr::LOCALHOST, srv.portmap, "/C/").unwrap();
        let wait = |what: &str, mut f: Box<dyn FnMut() -> bool + '_>| {
            let t0 = std::time::Instant::now();
            while !f() { assert!(t0.elapsed() < Duration::from_secs(10), "timed out: {what}"); std::thread::sleep(Duration::from_millis(20)); }
        };
        assert!(LiveReader::open(&mut media(), 1).unwrap().is_none(), "the first lookup misses (and asks)");
        let mut r = None;
        wait("the live bank", Box::new(|| { r = LiveReader::open(&mut media(), 1).unwrap(); r.is_some() }));
        let mut r = r.unwrap();
        assert_eq!(r.map.pages(), 5);
        wait("the first pages", Box::new(|| r.refresh(&mut media()).unwrap().done[..2] == [true, true]));
        assert_eq!((r.map.state, &r.map.done[2..]), (State::Splitting, &[false, false, false][..]), "the rest isn't split yet");
        let expect = |p: usize, k: f32| -> Vec<i16> {
            let (a, b) = (p * PAGE_FRAMES * 2, ((p + 1) * PAGE_FRAMES * 2).min(pcm.len()));
            pcm[a..b].iter().map(|v| to_i16(*v as f32 / 32768.0 * k)).collect()
        };
        let p0 = r.page(&mut media(), 0).unwrap();
        assert_eq!((&p0[0], &p0[1], &p0[2], &p0[3]), (&expect(0, -0.25), &expect(0, 0.25), &expect(0, 0.5), &expect(0, 1.0)));

        go_tx.send(()).unwrap();
        wait("the whole bank", Box::new(|| r.refresh(&mut media()).unwrap().state == State::Complete));
        let mut m = media();
        for p in 0..5 { assert_eq!(r.page(&mut m, p).unwrap()[1], expect(p, 0.25), "drums page {p}"); }
        assert!(LiveReader::open(&mut media(), 2).unwrap().is_none(), "a track we don't serve: nothing");
        drop(live);

        // Started again: the finished bank is kept for the same file, and
        // dropped once ID 1 is another file.
        let never: SplitFn = Arc::new(|_, _, _| anyhow::bail!("not split again"));
        let _again = LiveStems::start(mods.clone(), &[(1, music.path().join("t.wav"))], Arc::clone(&never)).unwrap();
        assert!(mods.join("live/1/pages").exists());
        std::fs::write(music.path().join("other.wav"), b"x").unwrap();
        let _other = LiveStems::start(mods.clone(), &[(1, music.path().join("other.wav"))], never).unwrap();
        assert!(!mods.join("live/1").exists(), "ID 1 is another track now");
    }
}
