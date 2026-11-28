//! A **stemd-compatible stem server**: OpenDeck answers the HTTP contract of
//! [stemd](https://github.com/nsaintot/stemd) (`docs/api.md`, protocol
//! `v=1`) so a CDJ-3000 running cdj3k-mods' Server Stems, or any stemd
//! client, gets its stems split by OpenDeck's model (an iPad's Core ML, a
//! desktop's HTDemucs).  Design: `docs/design/stemd-server.md`.
//!
//! What is served is the part of the contract the deck uses, plus enough
//! of the rest to be honest about what we are:
//!
//! - `GET /v1/health`: rate, channels, the parts we offer, `model_id`.
//! - `POST /v1/jobs`: raw PCM (`s16le` / `f32le`, 44.1 kHz stereo) in;
//!   `202` and a job, or `200` with a finished one on a cache hit.
//! - `GET /v1/jobs/{id}`: stage and progress, then the result.
//! - `GET /v1/jobs/{id}/stems/{name}`: one stem, `flac` / `wav` /
//!   `pcm16` / `pcm32`, at 44.1, 48, 24 or 96 kHz (96 kHz with
//!   `dsp_mode=1` through the CDJ-3000's own filter, [`mode1`]).
//! - `DELETE /v1/jobs/{id}`: stop it.
//!
//! Not served: sessions (the deck doesn't use them), `bass` / `other` as
//! separate parts (our split folds them into harmonics), `mp3`.
//!
//! The model is the caller's ([`Splitter`]); a general resampler too
//! ([`Config::resample`]).  One job runs at a time, newest last.

pub mod mode1;
mod session;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use opendeck_types::time::Instant;
use std::time::Duration;

/// stemd's port.
pub const PORT: u16 = 8420;
/// The model's rate: uploads must be at it.
pub const RATE: u32 = 44_100;
/// Longest track accepted (seconds).
pub const MAX_TRACK_SECS: u32 = 30 * 60;
/// Finished jobs whose stems are kept for download; older ones answer `410`.
const KEEP_DONE: usize = 6;

/// Three parts of a mix, interleaved stereo at 44.1 kHz, each as long as it.
pub struct Split { pub drums: Vec<f32>, pub vocals: Vec<f32>, pub harmonics: Vec<f32> }

/// The separation model.
pub trait Splitter: Send + Sync {
    /// Opaque identity of what produces the stems (stemd's `model_id`).
    fn model_id(&self) -> String;
    /// Whether a split can run at all now (a model on this device).
    fn ready(&self) -> bool { true }
    /// Split `mix` (interleaved stereo, 44.1 kHz).  `progress` takes 0–1;
    /// stop early (an error) once `cancel` is set.
    fn split(&self, mix: &[f32], progress: &(dyn Fn(f32) + Sync), cancel: &AtomicBool) -> Result<Split>;
    /// Split `mix` starting around `needle` (a frame, which may move while
    /// it runs) and report each finished stretch as it lands:
    /// `on_ready(first frame, drums, vocals, harmonics)`, interleaved and
    /// the same length.  For sessions.  The default splits the whole track
    /// and reports it once.
    fn split_live(&self, mix: &[f32], needle: &AtomicU64, on_ready: &(dyn Fn(usize, &[f32], &[f32], &[f32]) + Sync), cancel: &AtomicBool) -> Result<()> {
        let _ = needle;
        let s = self.split(mix, &|_| {}, cancel)?;
        on_ready(0, &s.drums, &s.vocals, &s.harmonics);
        Ok(())
    }
}

/// Interleaved stereo from one rate to another.
pub type Resample = Arc<dyn Fn(&[f32], u32, u32) -> Vec<f32> + Send + Sync>;

pub struct Config {
    /// TCP port (0: any).
    pub port: u16,
    /// Announce `_stemd._tcp` over mDNS.
    pub advertise: bool,
    /// The general resampler (`dsp_mode=0`).
    pub resample: Resample,
    /// Our version, for `/v1/health`.
    pub version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Part { Harmonics, Vocals, Drums }

impl Part {
    const ALL: [Part; 3] = [Part::Harmonics, Part::Vocals, Part::Drums];
    fn name(self) -> &'static str { match self { Part::Harmonics => "harmonics", Part::Vocals => "vocals", Part::Drums => "drums" } }
    fn parse(s: &str) -> Option<Part> { Part::ALL.into_iter().find(|p| p.name() == s) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Format { Flac, Wav, Pcm16, Pcm32 }

impl Format {
    fn parse(s: &str) -> Option<Format> {
        Some(match s { "flac" => Format::Flac, "wav" => Format::Wav, "pcm16" | "s16le" => Format::Pcm16, "pcm32" | "f32le" => Format::Pcm32, _ => return None })
    }
    fn name(self) -> &'static str { match self { Format::Flac => "flac", Format::Wav => "wav", Format::Pcm16 => "pcm16", Format::Pcm32 => "pcm32" } }
    fn content_type(self) -> &'static str { match self { Format::Flac => "audio/flac", Format::Wav => "audio/wav", _ => "application/octet-stream" } }
}

/// A job's request: everything that decides its stems (and so its cache key).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Params { format: Format, rate: u32, dsp_mode: u32, parts: Vec<Part>, derived: Option<Part>, include_derived: bool }

#[derive(Clone, Debug, PartialEq)]
enum Stage { Queued, Separating, Writing, Done, Failed(String), Cancelled }

impl Stage {
    fn name(&self) -> &'static str {
        match self { Stage::Queued => "queued", Stage::Separating => "separating", Stage::Writing => "writing", Stage::Done => "done", Stage::Failed(_) => "failed", Stage::Cancelled => "cancelled" }
    }
}

struct Stem { part: Part, bytes: Vec<u8>, gain: f32 }

struct Output { frames: usize, stems: Vec<Stem>, residual_db: f64, secs: f64 }

struct Job {
    id: String,
    key: [u8; 32],
    params: Params,
    stage: Mutex<Stage>,
    /// Fraction done, × 1e6.
    progress: AtomicU32,
    cancel: AtomicBool,
    pcm: Mutex<Option<Vec<f32>>>,
    out: Mutex<Option<Arc<Output>>>,
    /// The stems were dropped to make room (`410`).
    reaped: AtomicBool,
}

struct Inner { jobs: HashMap<String, Arc<Job>>, by_key: HashMap<[u8; 32], String>, queue: VecDeque<String>, done: VecDeque<String>, attempt: u64, running: Option<String> }

struct State { cfg: Config, splitter: Arc<dyn Splitter>, inner: Mutex<Inner>, wake: Condvar, sessions: Mutex<HashMap<String, Arc<session::Session>>> }

/// A running server.
pub struct Server {
    pub port: u16,
    _mdns: Option<opendeck_net::Advert>,
}

/// Start serving on `cfg.port`.
pub fn start(cfg: Config, splitter: Arc<dyn Splitter>) -> Result<Server> {
    let listener = TcpListener::bind(("0.0.0.0", cfg.port)).with_context(|| format!("bind stemd port {}", cfg.port))?;
    let port = listener.local_addr()?.port();
    let advertise = cfg.advertise;
    let model = splitter.model_id();
    let version = cfg.version.clone();
    let st = Arc::new(State { cfg, splitter, inner: Mutex::new(Inner { jobs: HashMap::new(), by_key: HashMap::new(), queue: VecDeque::new(), done: VecDeque::new(), attempt: 0, running: None }), wake: Condvar::new(), sessions: Mutex::new(HashMap::new()) });
    let w = Arc::clone(&st);
    std::thread::Builder::new().name("stemd-worker".into()).spawn(move || worker(w))?;
    let a = Arc::clone(&st);
    std::thread::Builder::new().name("stemd-accept".into()).spawn(move || {
        for conn in listener.incoming().flatten() {
            let s = Arc::clone(&a);
            let _ = std::thread::Builder::new().name("stemd-conn".into()).spawn(move || {
                if let Err(e) = serve(&s, conn) { log::debug!("stemd: {e:#}"); }
            });
        }
    })?;
    log::info!("stemd: serving on port {port} (model {model})");
    let _mdns = if advertise { advertise_mdns(port, &model, &version).map_err(|e| log::warn!("stemd: no mDNS: {e:#}")).ok() } else { None };
    Ok(Server { port, _mdns })
}

/// Advertise `_stemd._tcp` for CDJ-3000s and other OpenDecks to find.
fn advertise_mdns(port: u16, model: &str, version: &str) -> std::io::Result<opendeck_net::Advert> {
    let props = [("model", model), ("path", "/v1"), ("v", "1"), ("version", version)];
    opendeck_net::mdns_advertise("_stemd._tcp.local.", &format!("OpenDeck {}", opendeck_net::host_label()), port, &props)
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

struct Request { method: String, path: String, query: HashMap<String, String>, body_len: u64 }

struct Response { status: u16, content_type: &'static str, body: Vec<u8>, headers: Vec<(&'static str, String)> }

impl Response {
    fn json(status: u16, v: Value) -> Response { Response { status, content_type: "application/json", body: v.to_string().into_bytes(), headers: Vec::new() } }
    fn error(status: u16, msg: &str) -> Response { Response::json(status, json!({ "error": msg })) }
}

fn reason(s: u16) -> &'static str {
    match s { 200 => "OK", 201 => "Created", 202 => "Accepted", 204 => "No Content", 400 => "Bad Request", 404 => "Not Found", 405 => "Method Not Allowed", 409 => "Conflict", 410 => "Gone", 413 => "Payload Too Large", 429 => "Too Many Requests", 503 => "Service Unavailable", _ => "Error" }
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; continue; }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_request(r: &mut BufReader<&TcpStream>) -> Result<Request> {
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut it = line.split_whitespace();
    let (method, target) = (it.next().context("no method")?.to_string(), it.next().context("no target")?.to_string());
    let mut body_len = 0u64;
    let mut total = line.len();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 { break; }
        total += h.len();
        if total > 16 * 1024 { bail!("headers too long"); }
        let h = h.trim_end();
        if h.is_empty() { break; }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") { body_len = v.trim().parse().context("content-length")?; }
        }
    }
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let query = q.split('&').filter(|p| !p.is_empty()).map(|p| {
        let (k, v) = p.split_once('=').unwrap_or((p, ""));
        (unescape(k), unescape(v))
    }).collect();
    Ok(Request { method, path: path.to_string(), query, body_len })
}

fn serve(st: &Arc<State>, conn: TcpStream) -> Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut r = BufReader::new(&conn);
    let req = read_request(&mut r)?;
    // Read any body first, so even a refusal leaves nothing unread (closing
    // on unread bytes resets the connection under the reply).
    let resp = if req.body_len > (MAX_TRACK_SECS as u64) * RATE as u64 * 8 {
        Response::error(413, "body too large")
    } else {
        let mut body = vec![0u8; req.body_len as usize];
        match r.read_exact(&mut body) {
            Ok(()) => route(st, &req, body),
            Err(e) => Response::error(400, &format!("body: {e}")),
        }
    };
    let mut w = &conn;
    write!(w, "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n", resp.status, reason(resp.status), resp.content_type, resp.body.len())?;
    for (k, v) in &resp.headers { write!(w, "{k}: {v}\r\n")?; }
    write!(w, "\r\n")?;
    w.write_all(&resp.body)?;
    w.flush()?;
    Ok(())
}

fn route(st: &Arc<State>, req: &Request, body: Vec<u8>) -> Response {
    let parts: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    match (req.method.as_str(), parts.as_slice()) {
        ("GET", ["v1", "health"]) => Response::json(200, health(st)),
        ("POST", ["v1", "jobs"]) => match create(st, req, body) { Ok(r) => r, Err(e) => Response::error(400, &format!("{e:#}")) },
        ("GET", ["v1", "jobs", id]) => match job(st, id) { Some(j) => Response::json(200, doc(st, &j)), None => Response::error(404, "no such job") },
        ("DELETE", ["v1", "jobs", id]) => match job(st, id) {
            Some(j) => { cancel(st, &j); Response::json(200, json!({ "id": j.id, "stage": "cancelled" })) }
            None => Response::error(404, "no such job"),
        },
        ("GET", ["v1", "jobs", id, "stems", name]) => stem(st, id, name),
        (_, ["v1", "sessions", ..]) => session::route(st, req, &parts[2..], body),
        ("GET", ["v1", "logs"]) => Response::json(200, json!({ "lines": [] })),
        _ => Response::error(404, "not found"),
    }
}

fn health(st: &State) -> Value {
    let i = st.inner.lock().unwrap();
    json!({
        "version": st.cfg.version,
        "backend": "opendeck",
        "model": "htdemucs",
        "device": std::env::consts::ARCH,
        "sample_rate": RATE,
        "channels": 2,
        "stems": ["drums", "vocals", "harmonics"],
        "preset": "default",
        "model_id": st.splitter.model_id(),
        "ready": st.splitter.ready(),
        "completed_jobs": i.done.len(),
        "queue_depth": i.queue.len(),
        "running_job": i.running,
        "max_track_seconds": MAX_TRACK_SECS,
        "output_sample_rates": [24_000, 44_100, 48_000, 96_000],
        "output_formats": ["flac", "wav", "pcm16", "pcm32"],
        "dsp_modes": [0, 1],
        "dsp_mode_pairs": [[1, [mode1::IN_RATE, mode1::OUT_RATE]]],
        "default_output_format": "flac",
        "default_output_sample_rate": RATE,
        "derived_stem": "drums",
        "parts": ["harmonics", "vocals", "drums"],
        "default_parts": ["harmonics", "vocals"],
        "sessions": true,
        "segment_frames": session::SEGMENT,
        "stride_frames": session::STRIDE,
    })
}

fn job(st: &State, id: &str) -> Option<Arc<Job>> { st.inner.lock().unwrap().jobs.get(id).cloned() }

fn params(q: &HashMap<String, String>) -> Result<Params> {
    let format = match q.get("output_format") { Some(f) => Format::parse(f).with_context(|| format!("output_format {f:?} is not served (flac, wav, pcm16, pcm32)"))?, None => Format::Flac };
    let rate: u32 = q.get("output_sample_rate").map(|r| r.parse()).transpose().context("output_sample_rate")?.unwrap_or(RATE);
    if ![24_000, 44_100, 48_000, 96_000].contains(&rate) { bail!("output_sample_rate {rate}: 24000, 44100, 48000 or 96000"); }
    let dsp_mode: u32 = q.get("dsp_mode").map(|r| r.parse()).transpose().context("dsp_mode")?.unwrap_or(0);
    match dsp_mode {
        0 => {}
        1 if rate == mode1::OUT_RATE => {}
        1 => bail!("dsp_mode 1 covers 44100 -> 96000 only"),
        m => bail!("dsp_mode {m} is not offered"),
    }
    let mut parts = Vec::new();
    for p in q.get("parts").map(String::as_str).unwrap_or("harmonics,vocals").split(',').filter(|p| !p.is_empty()) {
        let part = Part::parse(p).with_context(|| format!("part {p:?} is not offered (harmonics, vocals, drums)"))?;
        if parts.contains(&part) { bail!("part {p} twice"); }
        parts.push(part);
    }
    if parts.len() < 2 { bail!("ask for at least two parts"); }
    parts.sort_by_key(|p| Part::ALL.iter().position(|a| a == p));
    let derived = Part::ALL.into_iter().find(|p| !parts.contains(p));
    let include_derived = q.get("include_derived").is_some_and(|v| v == "true" || v == "1");
    Ok(Params { format, rate, dsp_mode, parts, derived, include_derived })
}

fn create(st: &Arc<State>, req: &Request, raw: Vec<u8>) -> Result<Response> {
    let q = &req.query;
    let rate: u32 = q.get("sample_rate").map(|r| r.parse()).transpose().context("sample_rate")?.unwrap_or(RATE);
    let ch: u32 = q.get("channels").map(|r| r.parse()).transpose().context("channels")?.unwrap_or(2);
    if rate != RATE { bail!("upload at {RATE} Hz (got {rate})"); }
    if ch != 2 { bail!("upload two channels (got {ch})"); }
    let f32le = match q.get("format").map(String::as_str).unwrap_or("s16le") { "f32le" => true, "s16le" => false, f => bail!("format {f:?}: s16le or f32le") };
    let p = params(q)?;
    let width = if f32le { 8 } else { 4 };
    if req.body_len == 0 || req.body_len % width != 0 { bail!("body of {} bytes is not whole stereo frames", req.body_len); }
    if req.body_len > (MAX_TRACK_SECS as u64) * RATE as u64 * width { return Ok(Response::error(413, "track too long")); }
    if !st.splitter.ready() { return Ok(Response::error(503, "no separation model on this device")); }
    let pcm: Vec<f32> = if f32le {
        raw.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    } else {
        raw.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect()
    };
    drop(raw);
    let key = cache_key(&st.splitter.model_id(), &p, &pcm);
    let mut i = st.inner.lock().unwrap();
    if let Some(j) = i.by_key.get(&key).and_then(|id| i.jobs.get(id)).cloned() {
        let stage = j.stage.lock().unwrap().clone();
        let usable = matches!(stage, Stage::Queued | Stage::Separating | Stage::Writing) || (stage == Stage::Done && !j.reaped.load(Ordering::Relaxed));
        if usable {
            drop(i);
            let status = if stage == Stage::Done { 200 } else { 202 };
            let mut d = doc(st, &j);
            if status == 200 { d["cached"] = json!(true); }
            return Ok(Response::json(status, d));
        }
    }
    i.attempt += 1;
    let id = format!("{}-{}", hex(&key[..6]), i.attempt);
    let j = Arc::new(Job { id: id.clone(), key, params: p, stage: Mutex::new(Stage::Queued), progress: AtomicU32::new(0), cancel: AtomicBool::new(false), pcm: Mutex::new(Some(pcm)), out: Mutex::new(None), reaped: AtomicBool::new(false) });
    i.jobs.insert(id.clone(), Arc::clone(&j));
    i.by_key.insert(key, id.clone());
    i.queue.push_back(id.clone());
    log::info!("stemd: job {id} queued ({} frames, {} waiting)", j.pcm.lock().unwrap().as_ref().map_or(0, |p| p.len() / 2), i.queue.len());
    st.wake.notify_one();
    drop(i);
    Ok(Response::json(202, doc(st, &j)))
}

fn cache_key(model: &str, p: &Params, pcm: &[f32]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(b"opendeck-stemd-1");
    h.update(model.as_bytes());
    h.update(format!("{p:?}").as_bytes());
    for v in pcm { h.update(v.to_le_bytes()); }
    h.finalize().into()
}

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

fn cancel(st: &State, j: &Job) {
    j.cancel.store(true, Ordering::Relaxed);
    let mut i = st.inner.lock().unwrap();
    i.queue.retain(|q| *q != j.id);
    let mut s = j.stage.lock().unwrap();
    if matches!(*s, Stage::Queued) { *s = Stage::Cancelled; *j.pcm.lock().unwrap() = None; }
    if i.by_key.get(&j.key) == Some(&j.id) && *s != Stage::Done { i.by_key.remove(&j.key); }
}

fn doc(st: &State, j: &Job) -> Value {
    let stage = j.stage.lock().unwrap().clone();
    let frac = j.progress.load(Ordering::Relaxed) as f64 / 1e6;
    let (completed, total) = match stage {
        Stage::Queued => { let i = st.inner.lock().unwrap(); (i.queue.iter().position(|q| *q == j.id).unwrap_or(0) as u64, i.queue.len() as u64) }
        Stage::Done => (100, 100),
        _ => ((frac * 100.0) as u64, 100),
    };
    let mut d = json!({ "id": j.id, "stage": stage.name(), "completed": completed, "total": total, "fraction": frac });
    if let Stage::Failed(e) = &stage { d["error"] = json!(e); }
    if stage == Stage::Done {
        if let Some(o) = j.out.lock().unwrap().as_ref() {
            let p = &j.params;
            d["sample_rate"] = json!(p.rate);
            d["channels"] = json!(2);
            d["frames"] = json!(o.frames);
            d["format"] = json!(p.format.name());
            d["stems"] = Value::Array(o.stems.iter().map(|s| json!({
                "name": s.part.name(), "url": format!("/v1/jobs/{}/stems/{}", j.id, s.part.name()), "bytes": s.bytes.len(), "gain": s.gain,
            })).collect());
            d["derived_stem"] = json!(p.derived.map(Part::name));
            d["model_residual_db"] = json!(o.residual_db);
            d["separation_secs"] = json!(o.secs);
            d["realtime_factor"] = json!(if o.secs > 0.0 { o.frames as f64 / p.rate as f64 / o.secs } else { 0.0 });
            d["cached"] = json!(false);
            d["reaped"] = json!(j.reaped.load(Ordering::Relaxed));
        }
    }
    d
}

fn stem(st: &State, id: &str, name: &str) -> Response {
    let Some(j) = job(st, id) else { return Response::error(404, "no such job") };
    if j.reaped.load(Ordering::Relaxed) { return Response::error(410, "stems dropped; submit the job again"); }
    let Some(o) = j.out.lock().unwrap().clone() else { return Response::error(409, "not done yet") };
    match o.stems.iter().find(|s| s.part.name() == name) {
        Some(s) => Response { status: 200, content_type: j.params.format.content_type(), body: s.bytes.clone(), headers: Vec::new() },
        None => Response::error(404, "no such stem in this job"),
    }
}

// ── the worker ───────────────────────────────────────────────────────────────

fn worker(st: Arc<State>) {
    loop {
        let j = {
            let mut i = st.inner.lock().unwrap();
            loop {
                if let Some(id) = i.queue.pop_front() {
                    if let Some(sid) = id.strip_prefix("s:") {
                        let s = st.sessions.lock().unwrap().get(sid).cloned();
                        if let Some(s) = s {
                            i.running = Some(id.clone());
                            drop(i);
                            session::run(&st, &s);
                            i = st.inner.lock().unwrap();
                            i.running = None;
                        }
                        continue;
                    }
                    if let Some(j) = i.jobs.get(&id).cloned() { i.running = Some(id); break j; }
                    continue;
                }
                i = st.wake.wait(i).unwrap();
            }
        };
        let t0 = Instant::now();
        let r = run(&st, &j);
        let mut i = st.inner.lock().unwrap();
        i.running = None;
        let mut s = j.stage.lock().unwrap();
        match r {
            Ok(()) if !j.cancel.load(Ordering::Relaxed) => {
                *s = Stage::Done;
                j.progress.store(1_000_000, Ordering::Relaxed);
                log::info!("stemd: job {} done in {:.1}s", j.id, t0.elapsed().as_secs_f32());
                i.done.push_back(j.id.clone());
                while i.done.len() > KEEP_DONE {
                    let old = i.done.pop_front().unwrap();
                    if let Some(o) = i.jobs.get(&old) { o.reaped.store(true, Ordering::Relaxed); *o.out.lock().unwrap() = None; }
                }
            }
            Ok(()) => *s = Stage::Cancelled,
            Err(_) if j.cancel.load(Ordering::Relaxed) => *s = Stage::Cancelled,
            Err(e) => { log::warn!("stemd: job {} failed: {e:#}", j.id); *s = Stage::Failed(format!("{e:#}")); }
        }
        if *s != Stage::Done && i.by_key.get(&j.key) == Some(&j.id) { i.by_key.remove(&j.key); }
        *j.pcm.lock().unwrap() = None;
    }
}

fn run(st: &State, j: &Job) -> Result<()> {
    let mix = j.pcm.lock().unwrap().take().context("no audio")?;
    *j.stage.lock().unwrap() = Stage::Separating;
    j.progress.store(100_000, Ordering::Relaxed);
    let t0 = Instant::now();
    let prog = |f: f32| j.progress.store(((0.1 + 0.8 * f.clamp(0.0, 1.0) as f64) * 1e6) as u32, Ordering::Relaxed);
    let split = st.splitter.split(&mix, &prog, &j.cancel)?;
    let secs = t0.elapsed().as_secs_f64();
    if j.cancel.load(Ordering::Relaxed) { bail!("cancelled"); }
    *j.stage.lock().unwrap() = Stage::Writing;
    j.progress.store(900_000, Ordering::Relaxed);
    let n = mix.len();
    anyhow::ensure!(split.drums.len() == n && split.vocals.len() == n && split.harmonics.len() == n, "split lengths differ from the mix");
    let (mut err, mut sig) = (0f64, 0f64);
    for i in 0..n {
        let r = mix[i] - split.drums[i] - split.vocals[i] - split.harmonics[i];
        err += (r as f64).powi(2);
        sig += (mix[i] as f64).powi(2);
    }
    let residual_db = if sig > 0.0 { 10.0 * (err.max(1e-30) / sig).log10() } else { -160.0 };
    let p = &j.params;
    let model_part = |part: Part| -> &Vec<f32> { match part { Part::Drums => &split.drums, Part::Vocals => &split.vocals, Part::Harmonics => &split.harmonics } };
    let mut ship: Vec<(Part, Vec<f32>)> = p.parts.iter().map(|q| (*q, model_part(*q).clone())).collect();
    // The part left out is the mix minus the others, so the three sum exactly.
    if let (Some(d), true) = (p.derived, p.include_derived) {
        let mut v = mix.clone();
        for (_, s) in &ship { for (a, b) in v.iter_mut().zip(s) { *a -= *b; } }
        ship.push((d, v));
    }
    let convert = |x: &[f32]| -> Vec<f32> {
        if p.rate == RATE { x.to_vec() } else if p.dsp_mode == 1 { mode1::resample_stereo(x) } else { (st.cfg.resample)(x, RATE, p.rate) }
    };
    let mut stems = Vec::new();
    let mut frames = 0;
    for (part, x) in ship {
        if j.cancel.load(Ordering::Relaxed) { bail!("cancelled"); }
        let y = convert(&x);
        frames = y.len() / 2;
        let (bytes, gain) = encode(&y, p.format, p.rate);
        stems.push(Stem { part, bytes, gain });
    }
    *j.out.lock().unwrap() = Some(Arc::new(Output { frames, stems, residual_db, secs }));
    Ok(())
}

/// One stem in `format` at `rate`, and its gain (the stored samples are
/// the stem times `gain`; a client multiplies by `1 / gain`).  16-bit
/// formats scale a stem that peaks past full scale to fit.
fn encode(y: &[f32], format: Format, rate: u32) -> (Vec<u8>, f32) {
    if format == Format::Pcm32 { return (y.iter().flat_map(|v| v.to_le_bytes()).collect(), 1.0); }
    let peak = y.iter().fold(0f32, |m, v| m.max(v.abs()));
    let full = 32767.0 / 32768.0;
    let gain = if peak > full { full / peak } else { 1.0 };
    let pcm: Vec<u8> = y.iter().flat_map(|v| ((v * gain * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes()).collect();
    let bytes = match format {
        Format::Pcm16 => pcm,
        Format::Wav => { let mut h = wav_header(pcm.len() as u32, rate); h.extend_from_slice(&pcm); h }
        Format::Flac => opendeck_stems::synth::flac_verbatim(&pcm, rate),
        Format::Pcm32 => unreachable!(),
    };
    (bytes, gain)
}

fn wav_header(data: u32, rate: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data.to_le_bytes());
    h
}

#[cfg(test)]
mod tests;
