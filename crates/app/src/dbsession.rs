//! A linked player's dbserver session on its own thread.
//!
//! BROWSE asks a player for menus (a key press opens a category) and, for
//! every highlighted track, its metadata, cover and two waveform previews:
//! four round trips a row.  Asked from the UI thread, each one stalled a
//! frame on the network.  The session now lives on a worker:
//!
//! - [`Session::call`] is for menus: it waits for its answer, as a key press
//!   did before, but goes ahead of the background work, so it waits behind
//!   at most one request in flight.
//! - [`Session::ask_info`] is for the highlighted rows: it returns at once,
//!   and the [`DbInfo`] arrives a frame or two later ([`Session::answers`]),
//!   as BROWSE's S marks do (`stems::StemMarks`).
//! - [`Session::ask_list`] is for the right-hand pane: what the highlighted
//!   row opens (a BPM's ranges, a genre's artists, a playlist's tracks), in
//!   the background too ([`Session::lists`]).  It goes ahead of the lookups.
//!
//! One session, not two: a player drops a session when another opens from
//! the same player number (seen on a CDJ-3000: the first then reads EOF), so
//! the worker owns the only connection, and reconnects once on an error.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::mpsc;
use std::sync::Arc;

use anyhow::Result;
use opendeck_dbserver::{Client, MenuItem, Slot, TrackMeta};

/// What a linked player's library knows about one track: its metadata, the
/// cover bytes (JPEG / PNG) when it has one, and its waveform previews.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DbInfo {
    pub meta:  TrackMeta,
    pub art:   Option<Vec<u8>>,
    /// 400 columns of (height 0–31, whiteness 0–7).
    pub wave:  Option<Arc<Vec<(u8, u8)>>>,
    /// CDJ-3000 3-band preview: (low, mid, high) per column.
    pub bands: Option<Arc<Vec<[u8; 3]>>>,
}

/// A track's cache key: player address, slot, rekordbox id.
pub type InfoKey = (Ipv4Addr, u8, u32);

/// The four requests behind one row's [`DbInfo`].  [`Client`] is the real
/// one; the tests use a fake player.
pub trait InfoSource: Send + 'static {
    fn metadata(&mut self, slot: Slot, id: u32) -> Result<Vec<MenuItem>>;
    fn artwork(&mut self, slot: Slot, artwork_id: u32) -> Result<Vec<u8>>;
    fn waveform_preview(&mut self, slot: Slot, id: u32) -> Result<Vec<u8>>;
    /// The `.2EX` PWV6 section (CDJ-3000 3-band preview).
    fn band_preview(&mut self, slot: Slot, id: u32) -> Result<Vec<u8>>;
}

impl InfoSource for Client {
    fn metadata(&mut self, slot: Slot, id: u32) -> Result<Vec<MenuItem>> { Client::metadata(self, slot, id) }
    fn artwork(&mut self, slot: Slot, artwork_id: u32) -> Result<Vec<u8>> { Client::artwork(self, slot, artwork_id) }
    fn waveform_preview(&mut self, slot: Slot, id: u32) -> Result<Vec<u8>> { Client::waveform_preview(self, slot, id) }
    fn band_preview(&mut self, slot: Slot, id: u32) -> Result<Vec<u8>> { self.anlz_tag(slot, id, b"PWV6", b"2EX") }
}

/// A menu request: run on the client (`Ok`), or told the session is gone
/// (`Err`, the reconnect failed).  `last` = no retry follows.  Returns
/// whether it is finished (false: failed, and would like one retry).
type CallFn<C> = Box<dyn FnMut(std::result::Result<&mut C, &anyhow::Error>, Slot, bool) -> bool + Send>;

/// A list for the right-hand pane, asked under a ticket of the caller's.
pub type ListFn<C> = Box<dyn FnMut(&mut C, Slot) -> Result<Vec<MenuItem>> + Send>;

enum Job<C> {
    Call(CallFn<C>),
    Info { key: InfoKey, verbose: bool },
    List { ticket: u64, f: ListFn<C> },
    /// The session was dropped: stop, even while handles are still around.
    Close,
}

pub struct Session<C: InfoSource = Client> {
    pub ip:   Ipv4Addr,
    pub slot: Slot,
    jobs:     mpsc::Sender<Job<C>>,
    answers:  mpsc::Receiver<(InfoKey, Option<DbInfo>)>,
    lists:    mpsc::Receiver<(u64, Result<Vec<MenuItem>>)>,
}

impl<C: InfoSource> Session<C> {
    /// Connect (on the calling thread, so a failure is reported where the
    /// key was pressed) and hand the connection to the worker.  `connect`
    /// is kept for reconnecting.
    pub fn open(ip: Ipv4Addr, slot: Slot, mut connect: impl FnMut() -> Result<C> + Send + 'static) -> Result<Self> {
        let client = connect()?;
        let (jobs, rx) = mpsc::channel();
        let (answer, answers) = mpsc::channel();
        let (listed, lists) = mpsc::channel();
        std::thread::Builder::new().name("dbserver-session".into()).spawn(move || {
            Worker { ip, slot, client: Some(client), connect: Box::new(connect), answer, listed }.run(rx);
        })?;
        Ok(Session { ip, slot, jobs, answers, lists })
    }

    /// Run a request on the session and wait for its answer.  It goes ahead
    /// of queued [`ask_info`](Self::ask_info) work.  On an error the worker
    /// reconnects once and retries.  It waits on the network: never call it
    /// on the UI thread (use [`ask_list`](Self::ask_list), or a
    /// [`caller`](Self::caller) / [`handle`](Self::handle) on another thread).
    pub fn call<T: Send + 'static>(&self, f: impl FnMut(&mut C, Slot) -> Result<T> + Send + 'static) -> Result<T> {
        call_on(&self.jobs, f)
    }

    /// A handle that runs requests on this session from another thread (a
    /// loader), so a load from the player BROWSE has open uses its session
    /// instead of opening a second one.
    pub fn caller(&self) -> Caller<C> { Caller { jobs: self.jobs.clone() } }

    /// A handle for other threads to run requests on this session (the
    /// other players' waveform and INFO fetches): a second session from our
    /// player number would cost this one.  It stops working when the
    /// session closes.
    pub fn handle(&self) -> Handle<C> { Handle { ip: self.ip, jobs: self.jobs.clone() } }

    /// Look a track up in the background (metadata, cover, previews); the
    /// answer comes through [`answers`](Self::answers), `None` if the player
    /// had no metadata for it.  `verbose` logs what the player sent.
    pub fn ask_info(&self, key: InfoKey, verbose: bool) -> bool {
        self.jobs.send(Job::Info { key, verbose }).is_ok()
    }

    /// The lookups finished since the last call.  Never blocks.
    pub fn answers(&self) -> impl Iterator<Item = (InfoKey, Option<DbInfo>)> + '_ {
        self.answers.try_iter()
    }

    /// List something in the background (a dropped session reconnects once).
    /// The rows come through [`lists`](Self::lists) under `ticket`.
    pub fn ask_list(&self, ticket: u64, f: impl FnMut(&mut C, Slot) -> Result<Vec<MenuItem>> + Send + 'static) -> bool {
        self.jobs.send(Job::List { ticket, f: Box::new(f) }).is_ok()
    }

    /// The lists finished since the last call.  Never blocks.
    pub fn lists(&self) -> impl Iterator<Item = (u64, Result<Vec<MenuItem>>)> + '_ {
        self.lists.try_iter()
    }
}

/// Runs requests on a [`Session`] from any thread; see [`Session::caller`].
pub struct Caller<C: InfoSource = Client> { jobs: mpsc::Sender<Job<C>> }

impl<C: InfoSource> Caller<C> {
    /// As [`Session::call`]: waits for the answer, so not on the UI thread.
    pub fn call<T: Send + 'static>(&self, f: impl FnMut(&mut C, Slot) -> Result<T> + Send + 'static) -> Result<T> {
        call_on(&self.jobs, f)
    }
}

impl<C: InfoSource> Drop for Session<C> {
    fn drop(&mut self) { let _ = self.jobs.send(Job::Close); }
}

/// Another thread's way onto a [`Session`] (see [`Session::handle`]).
pub struct Handle<C: InfoSource = Client> {
    /// The player the session is open to.
    pub ip: Ipv4Addr,
    jobs:   mpsc::Sender<Job<C>>,
}

impl<C: InfoSource> Clone for Handle<C> {
    fn clone(&self) -> Self { Handle { ip: self.ip, jobs: self.jobs.clone() } }
}

impl<C: InfoSource> Handle<C> {
    /// As [`Session::call`]: waits for the answer, ahead of the lookups.
    pub fn call<T: Send + 'static>(&self, f: impl FnMut(&mut C, Slot) -> Result<T> + Send + 'static) -> Result<T> {
        call_on(&self.jobs, f)
    }
}

fn call_on<C: InfoSource, T: Send + 'static>(jobs: &mpsc::Sender<Job<C>>, mut f: impl FnMut(&mut C, Slot) -> Result<T> + Send + 'static) -> Result<T> {
    let (tx, rx) = mpsc::sync_channel(1);
    let job: CallFn<C> = Box::new(move |c, slot, last| {
        let r = match c {
            Ok(c) => f(c, slot),
            Err(e) => Err(anyhow::anyhow!("dbserver session lost: {e:#}")),
        };
        if r.is_err() && !last { return false; }
        let _ = tx.send(r);
        true
    });
    jobs.send(Job::Call(job)).map_err(|_| anyhow::anyhow!("dbserver session closed"))?;
    rx.recv().map_err(|_| anyhow::anyhow!("dbserver session closed"))?
}

/// A background lookup, one request per step, so a menu request never waits
/// behind all four.
struct InfoTask {
    key:     InfoKey,
    verbose: bool,
    step:    u8,
    meta:    Option<(TrackMeta, u32)>,
    info:    DbInfo,
}

struct Worker<C> {
    ip:      Ipv4Addr,
    slot:    Slot,
    client:  Option<C>,
    connect: Box<dyn FnMut() -> Result<C> + Send>,
    answer:  mpsc::Sender<(InfoKey, Option<DbInfo>)>,
    listed:  mpsc::Sender<(u64, Result<Vec<MenuItem>>)>,
}

impl<C: InfoSource> Worker<C> {
    fn run(mut self, rx: mpsc::Receiver<Job<C>>) {
        let mut calls: VecDeque<CallFn<C>> = VecDeque::new();
        let mut lists: VecDeque<(u64, ListFn<C>)> = VecDeque::new();
        let mut infos: VecDeque<InfoTask> = VecDeque::new();
        loop {
            // Everything asked so far, without waiting: menus jump the queue.
            loop {
                match rx.try_recv() {
                    Ok(Job::Close) => return,
                    Ok(j) => Self::queue(j, &mut calls, &mut lists, &mut infos),
                    Err(mpsc::TryRecvError::Empty) => break,
                    // The browser closed the session (another source, or quit).
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            if let Some(job) = calls.pop_front() { self.run_call(job); continue; }
            if let Some((ticket, mut f)) = lists.pop_front() {
                let slot = self.slot;
                let mut r = match self.client.as_mut() { Some(c) => f(c, slot), None => Err(anyhow::anyhow!("no session")) };
                // As a menu call: a dropped session reconnects once (a
                // player closes an idle one after some minutes).
                if r.is_err() {
                    log::info!("LINK {}: list failed; reconnecting", self.ip);
                    r = self.reconnect().and_then(|c| f(c, slot));
                }
                let _ = self.listed.send((ticket, r));
                continue;
            }
            if let Some(mut t) = infos.pop_front() {
                if self.info_step(&mut t) {
                    let InfoTask { key, meta, info, .. } = t;
                    let _ = self.answer.send((key, meta.map(|(m, _)| DbInfo { meta: m, ..info })));
                } else {
                    infos.push_front(t);
                }
                continue;
            }
            match rx.recv() {
                Ok(Job::Close) | Err(_) => return,
                Ok(j) => Self::queue(j, &mut calls, &mut lists, &mut infos),
            }
        }
    }

    fn queue(j: Job<C>, calls: &mut VecDeque<CallFn<C>>, lists: &mut VecDeque<(u64, ListFn<C>)>, infos: &mut VecDeque<InfoTask>) {
        match j {
            Job::Call(f) => calls.push_back(f),
            Job::List { ticket, f } => lists.push_back((ticket, f)),
            Job::Close => {}
            Job::Info { key, verbose } => infos.push_back(InfoTask { key, verbose, step: 0, meta: None, info: DbInfo::default() }),
        }
    }

    fn reconnect(&mut self) -> Result<&mut C> {
        self.client = None;
        let c = (self.connect)()?;
        log::info!("LINK {}: dbserver session open again ({:?})", self.ip, self.slot);
        Ok(self.client.insert(c))
    }

    fn run_call(&mut self, mut job: CallFn<C>) {
        let slot = self.slot;
        if let Some(c) = self.client.as_mut() {
            if job(Ok(c), slot, false) { return; }
            log::info!("LINK {}: request failed; reconnecting", self.ip);
        }
        match self.reconnect() {
            Ok(c) => { job(Ok(c), slot, true); }
            Err(e) => { job(Err(&e), slot, true); }
        }
    }

    /// One request of a lookup.  Returns whether the lookup is finished.
    fn info_step(&mut self, t: &mut InfoTask) -> bool {
        let (ip, slot, id, verbose) = (self.ip, self.slot, t.key.2, t.verbose);
        let step = t.step;
        t.step += 1;
        match step {
            // Metadata: without it there is nothing to show, so an error
            // reconnects once (the session may have been dropped), as a menu does.
            0 => {
                let first = match self.client.as_mut() { Some(c) => c.metadata(slot, id), None => Err(anyhow::anyhow!("no session")) };
                let rows = match first {
                    Ok(r) => Ok(r),
                    Err(e) => {
                        log::info!("LINK {ip}: metadata {id} failed ({e:#}); reconnecting");
                        self.reconnect().and_then(|c| c.metadata(slot, id))
                    }
                };
                match rows {
                    Ok(rows) => {
                        if verbose {
                            // What the player sent, once per track: shows which rows a
                            // player leaves out (key and year come up empty on some).
                            for r in &rows { log::info!("LINK metadata {id}: type 0x{:04x} id {} {:?} {:?}", r.item_type, r.id, r.label, r.label2); }
                        }
                        let meta = TrackMeta::from_rows(&rows);
                        let art = meta.artwork_id;
                        t.meta = Some((meta, art));
                        false
                    }
                    Err(e) => { log::info!("LINK metadata {id}: {e:#}"); true }
                }
            }
            1 => {
                let art = t.meta.as_ref().map_or(0, |m| m.1);
                if art != 0 {
                    if let Some(c) = self.client.as_mut() {
                        t.info.art = c.artwork(slot, art).ok().filter(|b| !b.is_empty());
                    }
                }
                false
            }
            2 => {
                let Some(c) = self.client.as_mut() else { return false };
                t.info.wave = match c.waveform_preview(slot, id) {
                    Ok(b) => {
                        if verbose { log::info!("LINK preview {id}: {} bytes", b.len()); }
                        parse_wave(&b)
                    }
                    Err(e) => { log::info!("LINK preview {id}: {e:#}"); None }
                };
                false
            }
            _ => {
                let Some(c) = self.client.as_mut() else { return true };
                t.info.bands = match c.band_preview(slot, id) {
                    Ok(b) => {
                        if verbose { log::info!("LINK preview3 {id}: {} bytes", b.len()); }
                        parse_bands(&b)
                    }
                    Err(e) => { if verbose { log::info!("LINK preview3 {id}: {e:#}"); } None }
                };
                true
            }
        }
    }
}

/// The blue preview (`0x4402`): 400 columns of two bytes each, height
/// (low 5 bits of the first) and whiteness (low 3 bits of the second), as
/// Beat Link reads it and as our own server sends it (800 bytes).  A
/// CDJ-3000 sends 900: the same 400 pairs, then a 100-byte tiny preview,
/// which is left out.  (It was read as the last 400 bytes, one column a
/// byte: the tiny preview and half the pairs, so the shapes were wrong.)
/// `None` when it is all silence.
fn parse_wave(b: &[u8]) -> Option<Arc<Vec<(u8, u8)>>> {
    let d = &b[..b.len().min(800)];
    Some(Arc::new(d.chunks_exact(2).map(|p| (p[0] & 0x1f, p[1] & 0x07)).collect::<Vec<_>>())).filter(|w| w.iter().any(|&(h, _)| h > 0))
}

/// PWV6: 1,200 columns of low, mid, high, behind whatever header the reply
/// carries.  `None` when it is all silence.
pub(crate) fn parse_bands(b: &[u8]) -> Option<Arc<Vec<[u8; 3]>>> {
    let d = if b.len() % 3 != 0 { &b[b.len() - b.len() / 3 * 3..] } else { b };
    let d = if d.len() > 3600 { &d[d.len() - 3600..] } else { d };
    Some(Arc::new(d.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect::<Vec<_>>())).filter(|w| w.iter().any(|c| c.iter().any(|&v| v > 0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use opendeck_types::time::Instant;
    use std::time::Duration;

    /// The blue preview as a CDJ-3000 sends it (400 pairs + a 100-byte tiny
    /// preview) and as our server sends it (the 400 pairs alone): the same
    /// 400 columns either way, the tail left out.
    #[test]
    fn the_blue_preview_is_400_height_whiteness_pairs() {
        let pairs: Vec<u8> = (0..400u32).flat_map(|i| [(i % 32) as u8 | 0xe0, (i % 8) as u8 | 0xf8]).collect();
        let mut cdj = pairs.clone();
        cdj.extend(std::iter::repeat(0x1f).take(100));
        for b in [pairs, cdj] {
            let w = parse_wave(&b).unwrap();
            assert_eq!(w.len(), 400);
            assert_eq!((w[0], w[33], w[399]), ((0, 0), (1, 1), (15, 7)));
        }
        // The server's own encoder, read back.
        let cache = opendeck_analysis::WaveformCache { columns: (0..2000).map(|i| [0, 0, (i % 50) as u8, (i % 100) as u8]).collect(), sample_rate: 44_100, hop_size: 512 };
        let (blob, _) = opendeck_mediaserver::waveform_blobs(&cache, 2000 * 512, 44_100);
        let w = parse_wave(&blob).unwrap();
        assert_eq!(w.len(), 400);
        assert!(w.iter().all(|&(h, wh)| h <= 31 && wh <= 7));
        assert_eq!(w.iter().map(|p| p.0).max(), Some(31), "the loudest column fills the height");
        assert!(parse_wave(&[0u8; 900]).is_none(), "silence");
    }

    /// A player that answers every request after `delay`, and fails each
    /// request while `down` is set (a dropped session).
    struct FakePlayer { delay: Duration, down: Arc<AtomicBool>, requests: Arc<AtomicUsize> }

    impl FakePlayer {
        fn ask(&self) -> Result<()> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            if self.down.load(Ordering::SeqCst) { anyhow::bail!("EOF"); }
            Ok(())
        }
    }

    fn title_row(id: u32) -> MenuItem {
        MenuItem { parent_id: 0, id, label: format!("track {id}"), label2: String::new(), item_type: 0x0004,
                   flags: 0, artwork_id: 0, position: 0, key: String::new(), bpm_x100: 0 }
    }

    impl InfoSource for FakePlayer {
        fn metadata(&mut self, _: Slot, id: u32) -> Result<Vec<MenuItem>> { self.ask()?; Ok(vec![title_row(id)]) }
        fn artwork(&mut self, _: Slot, _: u32) -> Result<Vec<u8>> { self.ask()?; Ok(vec![0x89, b'P']) }
        fn waveform_preview(&mut self, _: Slot, _: u32) -> Result<Vec<u8>> {
            // As a CDJ-3000 sends it: 400 (height, whiteness) pairs, then a 100-byte tiny preview.
            self.ask()?; Ok([5u8, 1].repeat(400).into_iter().chain([0x1f; 100]).collect())
        }
        fn band_preview(&mut self, _: Slot, _: u32) -> Result<Vec<u8>> { self.ask()?; Ok(vec![9; 3600]) }
    }

    struct Fake { session: Session<FakePlayer>, down: Arc<AtomicBool>, requests: Arc<AtomicUsize>, connects: Arc<AtomicUsize> }

    fn fake(delay_ms: u64) -> Fake {
        let down = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let connects = Arc::new(AtomicUsize::new(0));
        let (d, r, n) = (Arc::clone(&down), Arc::clone(&requests), Arc::clone(&connects));
        let session = Session::open(Ipv4Addr::LOCALHOST, Slot::Usb, move || {
            n.fetch_add(1, Ordering::SeqCst);
            Ok(FakePlayer { delay: Duration::from_millis(delay_ms), down: Arc::clone(&d), requests: Arc::clone(&r) })
        }).unwrap();
        Fake { session, down, requests, connects }
    }

    fn key(id: u32) -> InfoKey { (Ipv4Addr::LOCALHOST, Slot::Usb as u8, id) }

    fn wait_answers(s: &Session<FakePlayer>, n: usize) -> Vec<(InfoKey, Option<DbInfo>)> {
        let mut got = Vec::new();
        let t0 = Instant::now();
        while got.len() < n && t0.elapsed() < Duration::from_secs(5) {
            got.extend(s.answers());
            std::thread::sleep(Duration::from_millis(1));
        }
        got
    }

    #[test]
    fn ask_info_returns_at_once_and_the_answer_follows() {
        let f = fake(30);   // four requests: 120 ms on the worker
        let t0 = Instant::now();
        assert!(f.session.ask_info(key(7), false));
        assert!(f.session.answers().next().is_none(), "nothing yet");
        assert!(t0.elapsed() < Duration::from_millis(20), "ask_info waited {:?}", t0.elapsed());
        let got = wait_answers(&f.session, 1);
        assert_eq!(got.len(), 1);
        let (k, info) = &got[0];
        assert_eq!(*k, key(7));
        let info = info.as_ref().expect("an answer");
        assert_eq!(info.meta.title.as_deref(), Some("track 7"));
        assert_eq!(info.wave.as_ref().map(|w| w.len()), Some(400));
        assert_eq!(info.wave.as_ref().unwrap()[0], (5, 1));
        assert_eq!(info.bands.as_ref().map(|b| b.len()), Some(1200));
        assert!(info.art.is_none(), "no artwork id, no cover request");
        assert_eq!(f.requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_menu_request_goes_ahead_of_queued_lookups() {
        let f = fake(25);
        for id in 1..=10 { f.session.ask_info(key(id), false); }   // 30 requests, 750 ms
        std::thread::sleep(Duration::from_millis(10));               // the first one is under way
        let t0 = Instant::now();
        let r = f.session.call(|c, s| c.metadata(s, 99)).unwrap();
        let waited = t0.elapsed();
        assert_eq!(r[0].id, 99);
        // At most the request in flight, then its own: two round trips, not 31.
        assert!(waited < Duration::from_millis(150), "the menu waited {waited:?}");
        assert_eq!(wait_answers(&f.session, 10).len(), 10, "the lookups still finish");
    }

    #[test]
    fn a_list_comes_back_under_its_ticket_ahead_of_queued_lookups() {
        let f = fake(25);
        for id in 1..=10 { f.session.ask_info(key(id), false); }
        std::thread::sleep(Duration::from_millis(10));
        let t0 = Instant::now();
        assert!(f.session.ask_list(42, |c, s| c.metadata(s, 7)));
        assert!(t0.elapsed() < Duration::from_millis(20), "ask_list waited");
        let mut got = None;
        while got.is_none() && t0.elapsed() < Duration::from_secs(5) {
            got = f.session.lists().next();
            std::thread::sleep(Duration::from_millis(1));
        }
        let (ticket, rows) = got.expect("a list");
        assert_eq!(ticket, 42);
        assert_eq!(rows.unwrap()[0].id, 7);
        // Behind the request in flight only, not the 30 queued.
        assert!(t0.elapsed() < Duration::from_millis(150), "the list waited {:?}", t0.elapsed());
    }

    /// Another thread's requests go through the one session (no second
    /// connection), and fail cleanly once it is closed.
    #[test]
    fn a_handle_runs_requests_on_the_same_session() {
        let f = fake(0);
        let h = f.session.handle();
        assert_eq!(h.ip, Ipv4Addr::LOCALHOST);
        let r = std::thread::spawn({ let h = h.clone(); move || h.call(|c, s| c.metadata(s, 5)) }).join().unwrap();
        assert_eq!(r.unwrap()[0].id, 5);
        assert_eq!(f.connects.load(Ordering::SeqCst), 1, "no second connection");
        let Fake { session, .. } = f;
        drop(session);
        std::thread::sleep(Duration::from_millis(20));
        assert!(h.call(|c, s| c.metadata(s, 6)).is_err(), "closed with its session");
    }

    #[test]
    fn a_dropped_session_reconnects_once() {
        let f = fake(0);
        f.down.store(true, Ordering::SeqCst);
        // Down for good: the call fails after one reconnect.
        assert!(f.session.call(|c, s| c.metadata(s, 1)).is_err());
        assert_eq!(f.connects.load(Ordering::SeqCst), 2);
        // Back up: the next call works on the session it reopened.
        f.down.store(false, Ordering::SeqCst);
        assert_eq!(f.session.call(|c, s| c.metadata(s, 2)).unwrap()[0].id, 2);
        assert_eq!(f.connects.load(Ordering::SeqCst), 2);
        // A lookup whose metadata fails answers None, after reconnecting once.
        f.down.store(true, Ordering::SeqCst);
        f.session.ask_info(key(3), false);
        let got = wait_answers(&f.session, 1);
        assert_eq!(got, vec![(key(3), None)]);
        assert_eq!(f.connects.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn closing_the_session_stops_the_worker() {
        let f = fake(20);
        for id in 1..=50 { f.session.ask_info(key(id), false); }
        std::thread::sleep(Duration::from_millis(30));
        let Fake { session, requests, .. } = f;
        drop(session);
        std::thread::sleep(Duration::from_millis(60));
        let after = requests.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        // The worker finishes the request in flight and goes; it does not
        // work through the 200 requests still queued for a closed source.
        assert_eq!(requests.load(Ordering::SeqCst), after, "still asking after close");
        assert!(after < 10, "{after} requests");
    }
}
