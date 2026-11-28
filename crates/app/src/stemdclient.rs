//! OpenDeck as a **stemd client**: stems for the loaded track from a stemd
//! server on the network (another OpenDeck, or stemd itself, e.g. with
//! CUDA on a desktop GPU), through stemd's **sessions** API, so the stems
//! under the playhead arrive first instead of after the whole track.
//! Design: `docs/design/stemd-server.md` ("OpenDeck as a client").
//!
//! The server is `OPENDECK_STEMD_SERVER=host[:port]`, else the first
//! `_stemd._tcp` found over mDNS that isn't this device.  `OPENDECK_STEMD_CLIENT=0`
//! turns the client off.
//!
//! Flow: open a session for the track at 44.1 kHz, put the window on the
//! playhead, upload the track (the stretch at the playhead first), then
//! read ready ranges a few seconds at a time from the playhead: harmonics
//! and vocals come over the wire, drums are the mix minus them (exact), and
//! all three are converted to the deck's rate into the bank the deck plays,
//! the original elsewhere.  A jump to somewhere not split yet moves the
//! window.

use crate::separate::Job;
use crate::stems::{self, StemBank, SpanSource, PAGED_MARGIN, PAGED_STEP_SECS, SPLIT_SCALE};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const RATE: u32 = 44_100;

/// How long a server may take to start splitting (upload, model load)
/// before the deck gives up on it.
const START_WAIT: Duration = Duration::from_secs(60);

// ── finding a server ─────────────────────────────────────────────────────────

static FOUND: OnceLock<Mutex<Option<SocketAddr>>> = OnceLock::new();

/// The stemd server to use, if any: `OPENDECK_STEMD_SERVER`, else one found
/// over mDNS (looked for in the background from the first call on).
/// Tests point the deck at an in-process server here (the environment is
/// shared by every test in the process).
#[cfg(test)]
pub(crate) static TEST_SERVER: Mutex<Option<SocketAddr>> = Mutex::new(None);
/// Held by the deck tests that start stems jobs, so one test's server
/// isn't asked by another test's deck.
#[cfg(test)]
pub(crate) static TEST_DECKS: Mutex<()> = Mutex::new(());

pub fn server() -> Option<SocketAddr> {
    #[cfg(test)]
    { return *TEST_SERVER.lock().unwrap(); }
    #[allow(unreachable_code)]
    if std::env::var("OPENDECK_STEMD_CLIENT").is_ok_and(|v| v == "0") { return None; }
    if let Ok(s) = std::env::var("OPENDECK_STEMD_SERVER") {
        let s = if s.contains(':') { s } else { format!("{s}:{}", opendeck_stemd::PORT) };
        return s.to_socket_addrs().ok().and_then(|mut a| a.next());
    }
    let slot = FOUND.get_or_init(|| {
        std::thread::Builder::new().name("stemd-browse".into()).spawn(browse).ok();
        Mutex::new(None)
    });
    *slot.lock().unwrap()
}

/// Browse `_stemd._tcp` and keep the first server that isn't this device.
fn browse() {
    let own = opendeck_net::local_ips();
    opendeck_net::mdns_browse("_stemd._tcp.local.", |f| {
        if let Some(&ip) = f.addrs.iter().find(|ip| ip.is_ipv4() && !own.contains(ip)) {
            log::info!("stemd: found a server at {ip}:{} ({})", f.port, f.fullname);
            if let Some(s) = FOUND.get() { *s.lock().unwrap() = Some(SocketAddr::new(ip, f.port)); }
        }
    });
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

struct Reply { status: u16, headers: Vec<(String, String)>, body: Vec<u8> }

impl Reply {
    fn json(&self) -> Result<Value> { serde_json::from_slice(&self.body).context("server reply is not JSON") }
    fn header(&self, k: &str) -> Option<&str> { self.headers.iter().find(|(h, _)| h.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str()) }
}

fn http(addr: SocketAddr, method: &str, path: &str, body: &[u8]) -> Result<Reply> {
    let mut c = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).with_context(|| format!("stemd {addr}"))?;
    c.set_read_timeout(Some(Duration::from_secs(30)))?;
    write!(c, "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len())?;
    if !body.is_empty() { write!(c, "Content-Type: application/octet-stream\r\n")?; }
    write!(c, "\r\n")?;
    c.write_all(body)?;
    let mut out = Vec::new();
    c.read_to_end(&mut out)?;
    let e = out.windows(4).position(|w| w == b"\r\n\r\n").context("no HTTP header")?;
    let head = String::from_utf8_lossy(&out[..e]).to_string();
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).context("no status")?;
    let headers = head.lines().skip(1).filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect();
    Ok(Reply { status, headers, body: out[e + 4..].to_vec() })
}

// ── a session ────────────────────────────────────────────────────────────────

/// Source frames of one role as fetched for a step, for `deck_span`
/// (zeros outside what was fetched, which `deck_span` never asks for).
struct Fetched { from: usize, data: Vec<i16>, frames: usize }

impl SpanSource for Fetched {
    fn frames(&self) -> usize { self.frames }
    fn source(&mut self, _: &mut dyn opendeck_stems::Media, a: usize, b: usize) -> Result<Vec<i16>> {
        let b = b.min(self.frames);
        Ok((a..b).flat_map(|f| {
            let i = f.checked_sub(self.from).map(|k| k * 2).filter(|k| k + 1 < self.data.len());
            match i { Some(k) => [self.data[k], self.data[k + 1]], None => [0, 0] }
        }).collect())
    }
}

/// Stems for `track` (interleaved stereo at `deck_sr`) from the server at
/// `addr`.  The bank goes on `job` as soon as it has stems in it; returns
/// once it is complete.  `Ok(None)`: the server can't (incompatible, no
/// sessions, never started).
pub fn load(addr: SocketAddr, track: &Arc<Vec<f32>>, deck_sr: u32, job: &Job) -> Result<Option<Arc<StemBank>>> {
    let t0 = Instant::now();
    let h = http(addr, "GET", "/v1/health", b"")?;
    if h.status != 200 { return Ok(None); }
    let hj = h.json()?;
    let text = String::from_utf8_lossy(&h.body);
    if hj["sample_rate"].as_u64() != Some(RATE as u64) || hj["channels"].as_u64() != Some(2) || !text.contains("\"harmonics\"") || !text.contains("\"vocals\"") {
        log::info!("stemd: {addr} can't serve this deck");
        return Ok(None);
    }
    let cancelled = || job.cancel.load(Ordering::Relaxed);
    let mix = Arc::new(crate::audio::resample_interleaved(track, 2, deck_sr, RATE)?);
    let frames = mix.len() / 2;
    ensure!(frames > 0, "empty track");
    let open = http(addr, "POST", &format!("/v1/sessions?frames={frames}&sample_rate={RATE}&channels=2&format=f32le&output_format=pcm32"), b"")?;
    if open.status != 201 && open.status != 200 {
        log::info!("stemd: {addr} has no sessions ({}): {}", open.status, String::from_utf8_lossy(&open.body));
        return Ok(None);
    }
    let id = open.json()?["id"].as_str().context("no session id")?.to_string();
    let res = run_session(addr, &id, &mix, track, deck_sr, job, t0);
    let _ = http(addr, "DELETE", &format!("/v1/sessions/{id}"), b"");
    if cancelled() { bail!("cancelled"); }
    res
}

fn ranges(v: &Value) -> Vec<(usize, usize)> {
    v["ready"].as_array().map(|a| a.iter().filter_map(|r| Some((r[0].as_u64()? as usize, r[1].as_u64()? as usize))).collect()).unwrap_or_default()
}

fn run_session(addr: SocketAddr, id: &str, mix: &Arc<Vec<f32>>, track: &Arc<Vec<f32>>, deck_sr: u32, job: &Job, t0: Instant) -> Result<Option<Arc<StemBank>>> {
    let frames = mix.len() / 2;
    let cancelled = || job.cancel.load(Ordering::Relaxed);
    let deck_frames = track.len() / 2;
    let src_of = |deck_frame: usize| (deck_frame as u64 * RATE as u64 / deck_sr as u64) as usize;
    let playhead = || job.playhead.as_ref().map_or(0, |p| p.load(Ordering::Relaxed) as usize / 2);
    let window = |needle: usize| {
        let body = serde_json::json!({ "needle": needle.min(frames - 1), "ahead": { "frames": RATE * 30, "priority": "critical" }, "behind": { "frames": RATE * 2, "priority": "bonus" } });
        http(addr, "PUT", &format!("/v1/sessions/{id}/window"), body.to_string().as_bytes())
    };
    // The needle the server splits from: the start of the deck step the
    // playhead is in, less the margin a step reads before itself, so the
    // first stretch split covers the step whole.
    let step = stems::step_frames(deck_sr);
    let needle_for = |deck_frame: usize| src_of(deck_frame / step * step).saturating_sub(PAGED_MARGIN + 2048);
    let mut told = needle_for(playhead());
    window(told)?;
    // Upload, the stretch at the playhead first (10 s pieces).
    let piece = RATE as usize * 10;
    let n = frames.div_ceil(piece);
    let first = (told / piece).min(n - 1);
    for k in (first..n).chain(0..first) {
        if cancelled() { bail!("cancelled"); }
        let (a, b) = (k * piece, ((k + 1) * piece).min(frames));
        let body: Vec<u8> = mix[a * 2..b * 2].iter().flat_map(|v| v.to_le_bytes()).collect();
        let r = http(addr, "PUT", &format!("/v1/sessions/{id}/pcm?offset={a}"), &body)?;
        ensure!(r.status == 200, "upload {a}: {} {}", r.status, String::from_utf8_lossy(&r.body));
    }
    log::info!("stemd: {addr} session {id}: {frames} frames up in {:.2}s", t0.elapsed().as_secs_f32());

    let mut bank = StemBank::filling_split(track);
    bank.prepared = true;
    let bank = Arc::new(bank);
    let steps = deck_frames.div_ceil(step).max(1);
    let mut done = vec![false; steps];
    let (mut n_done, mut first_at) = (0usize, None);
    // Source frames a deck step needs (deck_span's margin and rounding).
    let span = |k: usize| {
        let (a, b) = (k * step, ((k + 1) * step).min(deck_frames));
        let s0 = src_of(a).saturating_sub(PAGED_MARGIN + 1024);
        let s1 = (src_of(b) + PAGED_MARGIN + 2).min(frames);
        (a, b, s0, s1)
    };
    let mut ready: Vec<(usize, usize)> = Vec::new();
    let mut dummy = opendeck_stems::DirMedia::new("/nonexistent");
    let started = Instant::now();
    while n_done < steps {
        if cancelled() { bail!("cancelled"); }
        let at = (playhead() / step).min(steps - 1);
        let covered = |s0: usize, s1: usize| ready.iter().any(|(x, y)| *x <= s0 && s1 <= *y);
        let next = (at..steps).chain(0..at).find(|&k| !done[k] && { let (_, _, s0, s1) = span(k); covered(s0, s1) });
        let Some(k) = next else {
            // Waiting: the deck jumped somewhere not split yet → move the window.
            let now = needle_for(playhead());
            if !done[at] && now != told { told = now; window(told)?; }
            let v = http(addr, "GET", &format!("/v1/sessions/{id}"), b"")?;
            if v.status != 200 { bail!("session gone ({})", v.status); }
            let vj = v.json()?;
            ready = ranges(&vj);
            match vj["state"].as_str() {
                Some("failed") => { log::warn!("stemd: {addr} session {id} failed; {n_done} of {steps} steps have stems"); return Ok((n_done > 0).then_some(bank)); }
                Some("ended") if ready.is_empty() => return Ok(None),
                _ => {}
            }
            if n_done == 0 && ready.is_empty() && started.elapsed() > START_WAIT {
                log::info!("stemd: {addr} never started splitting");
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let (a, b, s0, s1) = span(k);
        let get = |name: &str| -> Result<Vec<f32>> {
            let r = http(addr, "GET", &format!("/v1/sessions/{id}/stems/{name}?from={s0}&to={s1}"), b"")?;
            ensure!(r.status == 200, "{name} {s0}..{s1}: {}", r.status);
            let gain: f32 = r.header("x-stemd-gain").and_then(|g| g.parse().ok()).unwrap_or(1.0);
            Ok(r.body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) / gain).collect())
        };
        let (h, v) = (get("harmonics")?, get("vocals")?);
        ensure!(h.len() == (s1 - s0) * 2 && v.len() == h.len(), "short range");
        // Drums = the mix minus the others: the three sum exactly.
        let d: Vec<f32> = (0..h.len()).map(|i| mix[s0 * 2 + i] - h[i] - v[i]).collect();
        let q = |x: &[f32]| -> Fetched { Fetched { from: s0, frames, data: x.iter().map(|v| (v / SPLIT_SCALE * 32768.0).round().clamp(-32768.0, 32767.0) as i16).collect() } };
        let mut got: [Vec<i16>; 3] = Default::default();
        for (s, x) in [&d, &v, &h].into_iter().enumerate() {
            got[s] = stems::deck_span(&mut q(x), &mut dummy, RATE, deck_sr, a as isize, b as isize)?;
        }
        bank.write(a * 2, [&got[0], &got[1], &got[2]]);
        if n_done == 0 { let _ = job.bank.set(Arc::clone(&bank)); }
        done[k] = true;
        n_done += 1;
        job.progress.store((n_done * 1000 / steps) as u32, Ordering::Relaxed);
        first_at.get_or_insert(t0.elapsed());
    }
    bank.finish(deck_sr);
    log::info!("stemd: {addr} session {id} complete; first {PAGED_STEP_SECS}s playable in {:.2}s, whole in {:.1}s",
               first_at.unwrap_or_default().as_secs_f32(), t0.elapsed().as_secs_f32());
    Ok(Some(bank))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};

    /// A jump while the session runs: the deck's playhead moves to a part
    /// not split yet, the client moves the window there, and the server's
    /// split follows (the stand-in records where the playhead it was given
    /// went).
    #[test]
    fn a_jump_moves_the_servers_window() {
        static LOCAL: AtomicBool = AtomicBool::new(false);
        static SEEN: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
        let slow: crate::livestems::SplitFn = Arc::new(|t, job, bank| {
            let ps = stems::PAGE;
            let n = t.len().div_ceil(ps);
            let mut done = vec![false; n];
            while let Some(p) = {
                let at = (job.playhead.as_ref().unwrap().load(Ordering::Relaxed) as usize / ps).min(n - 1);
                SEEN.lock().unwrap().push(at * ps / 2);
                (at..n).chain(0..at).find(|&p| !done[p])
            } {
                if job.cancel.load(Ordering::Relaxed) { anyhow::bail!("cancelled"); }
                let (a, b) = (p * ps, ((p + 1) * ps).min(t.len()));
                let q: Vec<i16> = t[a..b].iter().map(|v| (v * 0.25 / SPLIT_SCALE * 32768.0).round() as i16).collect();
                bank.write(a, [&q, &q, &q]);
                done[p] = true;
                std::thread::sleep(Duration::from_millis(3));
            }
            Ok(())
        });
        let resample: opendeck_stemd::Resample = Arc::new(|x, f, t| crate::audio::resample_interleaved(x, 2, f, t).unwrap());
        let srv = opendeck_stemd::start(opendeck_stemd::Config { port: 0, advertise: false, resample, version: "test".into() },
                                        Arc::new(crate::stemdsrv::ModelSplitter::with(slow, &LOCAL))).unwrap();
        let addr: SocketAddr = format!("127.0.0.1:{}", srv.port).parse().unwrap();
        let track: Arc<Vec<f32>> = Arc::new((0..RATE as usize * 60).flat_map(|i| { let v = ((i as f32) * 0.003).sin() * 0.4; [v, v] }).collect());
        let head = Arc::new(AtomicU64::new(RATE as u64 * 5 * 2));
        let job = Arc::new(Job { playhead: Some(Arc::clone(&head)), ..Default::default() });
        let (j, t) = (Arc::clone(&job), Arc::clone(&track));
        let h = std::thread::spawn(move || load(addr, &t, RATE, &j));
        let t0 = Instant::now();
        while !job.bank.get().is_some_and(|b| b.range_ready(RATE as usize * 5 * 2..RATE as usize * 6 * 2)) {
            assert!(t0.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(5));
        }
        head.store(RATE as u64 * 45 * 2, Ordering::Relaxed);   // the DJ jumps to 45 s
        while !job.bank.get().unwrap().range_ready(RATE as usize * 45 * 2..RATE as usize * 46 * 2) {
            assert!(t0.elapsed() < Duration::from_secs(20) && !h.is_finished(), "stems at the jump");
            std::thread::sleep(Duration::from_millis(5));
        }
        let bank = job.bank.get().unwrap().clone();
        assert!(!bank.complete(), "the jump was served before the rest of the track");
        assert!(SEEN.lock().unwrap().iter().any(|f| (RATE as usize * 40..RATE as usize * 46).contains(f)), "the server's split moved to the jump");
        job.cancel.store(true, Ordering::Relaxed);
        let _ = h.join();
    }

    /// The real model behind another OpenDeck's stemd server
    /// (`OPENDECK_SEPARATE_MODEL`), a real track (`OPENDECK_TEST_STEMD_SESSION_REAL=<file>`),
    /// a 48 kHz deck playing from 60 s in: prints when the stems at the
    /// playhead and the whole bank arrive.  Skipped without the variable.
    #[test]
    fn real_model_stemd_session() {
        let Ok(file) = std::env::var("OPENDECK_TEST_STEMD_SESSION_REAL") else { return };
        let resample: opendeck_stemd::Resample = Arc::new(|x, f, t| crate::audio::resample_interleaved(x, 2, f, t).unwrap());
        let srv = opendeck_stemd::start(opendeck_stemd::Config { port: 0, advertise: false, resample, version: "test".into() }, Arc::new(crate::stemdsrv::ModelSplitter::new())).unwrap();
        let addr: SocketAddr = format!("127.0.0.1:{}", srv.port).parse().unwrap();
        let (s, sr, _, _) = crate::audio::decode_file(std::path::Path::new(&file)).unwrap();
        let track = Arc::new(crate::audio::resample_interleaved(&s, 2, sr, 48_000).unwrap());
        let head = Arc::new(AtomicU64::new(48_000 * 60 * 2));
        let job = Arc::new(Job { playhead: Some(Arc::clone(&head)), ..Default::default() });
        let (j, t) = (Arc::clone(&job), Arc::clone(&track));
        let t0 = Instant::now();
        let h = std::thread::spawn(move || load(addr, &t, 48_000, &j));
        let at = 48_000 * 60 * 2;
        while !job.bank.get().is_some_and(|b| b.range_ready(at..at + 48_000 * 2 * 4)) {
            assert!(!h.is_finished(), "no stems");
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!("stemd session: stems at the playhead (60 s) after {:.2}s", t0.elapsed().as_secs_f32());
        let bank = h.join().unwrap().unwrap().expect("stems");
        eprintln!("stemd session: whole {:.0}s track after {:.1}s", track.len() as f32 / 96_000.0, t0.elapsed().as_secs_f32());
        assert!(bank.complete());
    }

    /// OpenDeck → OpenDeck through stemd sessions: a deck playing a track
    /// (at 44.1 and at 48 kHz) 10 s in gets stems from another OpenDeck's
    /// stemd server, the ones at its playhead before the split is done,
    /// then the whole track; drums rebuilt from the mix land where the
    /// model put them.  The server runs the app's own splitter path with a
    /// stand-in model that follows the playhead page by page, as the real
    /// split does.
    #[test]
    fn a_deck_plays_stems_from_another_opendecks_stemd_session() {
        static LOCAL: AtomicBool = AtomicBool::new(false);
        let fake: crate::livestems::SplitFn = Arc::new(|t, job, bank| {
            let ps = stems::PAGE;
            let n = t.len().div_ceil(ps);
            let mut done = vec![false; n];
            while let Some(p) = {
                let at = (job.playhead.as_ref().unwrap().load(Ordering::Relaxed) as usize / ps).min(n - 1);
                (at..n).chain(0..at).find(|&p| !done[p])
            } {
                if job.cancel.load(Ordering::Relaxed) { anyhow::bail!("cancelled"); }
                let (a, b) = (p * ps, ((p + 1) * ps).min(t.len()));
                let q = |k: f32| -> Vec<i16> { t[a..b].iter().map(|v| (v * k / SPLIT_SCALE * 32768.0).round() as i16).collect() };
                bank.write(a, [&q(0.5), &q(0.25), &q(0.25)]);
                done[p] = true;
                if p % 8 == 0 { std::thread::sleep(Duration::from_millis(4)); }
            }
            Ok(())
        });
        let resample: opendeck_stemd::Resample = Arc::new(|x, f, t| crate::audio::resample_interleaved(x, 2, f, t).unwrap());
        let srv = opendeck_stemd::start(opendeck_stemd::Config { port: 0, advertise: false, resample, version: "test".into() },
                                        Arc::new(crate::stemdsrv::ModelSplitter::with(fake, &LOCAL))).unwrap();
        let addr: SocketAddr = format!("127.0.0.1:{}", srv.port).parse().unwrap();
        for deck_sr in [44_100u32, 48_000] {
            let frames = deck_sr as usize * 40;
            let mut x = 7u32; let mut lp = 0f32;
            let track: Arc<Vec<f32>> = Arc::new((0..frames).flat_map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                lp = lp * 0.97 + ((x >> 16) as f32 / 32768.0 - 1.0) * 0.06;   // nothing above ~20 kHz (the model runs at 44.1)
                [lp, lp * 0.9]
            }).collect());
            let head = Arc::new(AtomicU64::new((deck_sr as u64 * 10) * 2));
            let job = Arc::new(Job { playhead: Some(Arc::clone(&head)), ..Default::default() });
            let (j, t) = (Arc::clone(&job), Arc::clone(&track));
            let h = std::thread::spawn(move || load(addr, &t, deck_sr, &j));
            let t0 = Instant::now();
            let at = deck_sr as usize * 10 * 2;
            while !job.bank.get().is_some_and(|b| b.range_ready(at..at + deck_sr as usize * 2 * 2)) {
                assert!(t0.elapsed() < Duration::from_secs(20) && !h.is_finished(), "no stems at the playhead");
                std::thread::sleep(Duration::from_millis(5));
            }
            let early = job.bank.get().unwrap().clone();
            let bank = h.join().unwrap().unwrap().expect("stems");
            assert!(Arc::ptr_eq(&early, &bank) && bank.complete());
            let d = bank.stem_levels(0);
            let skip = deck_sr as usize;   // the resampler's edges
            // What the stems can be: the track through the model's rate and back.
            let back = crate::audio::resample_interleaved(&crate::audio::resample_interleaved(&track, 2, deck_sr, RATE).unwrap(), 2, RATE, deck_sr).unwrap();
            let worst = back.iter().zip(&d).skip(skip).take(track.len() - 2 * skip).map(|(m, d)| (m * 0.5 - d).abs()).fold(0f32, f32::max);
            assert!(worst < 2e-3, "{deck_sr} Hz: drums off by {worst}");
        }
    }
}


