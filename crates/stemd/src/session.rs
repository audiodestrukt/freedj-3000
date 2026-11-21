//! stemd **sessions** (`docs/api.md`, "Sessions"): one track separated
//! around a playhead, its finished ranges readable while the rest runs.
//! What a deck that wants stems "now" uses, rather than a whole-track job.
//!
//! Served: open (`POST /v1/sessions?frames=…`, an optional body from sample
//! 0), upload (`PUT …/pcm?offset=N`, any order), window (`PUT …/window`:
//! the needle the split follows), view (`GET …`: `arrived`, `ready`), ready
//! ranges (`GET …/stems/{name}?from=&to=`, with the `x-stemd-*` headers),
//! end (`DELETE`).  Output is `pcm32` / `f32le` or `pcm16` / `s16le` at
//! 44.1 kHz.  Not served: `pieces`, sharing a track between sessions, the
//! disk store, `mean` / `std` (the split starts once the whole track has
//! arrived: our model normalises by the whole track).

use super::*;

/// Segment and stride the view reports: stemd's Fast/Balanced figures (our
/// model's chunk is the same HTDemucs segment).
pub const SEGMENT: usize = 343_980;
pub const STRIDE: usize = 257_985;
/// Sessions open at once.
const MAX_SESSIONS: usize = 4;
/// A session nobody has asked about for this long is ended.
const IDLE: Duration = Duration::from_secs(120);

/// Sorted, disjoint `[from, to)` frame ranges.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Ranges(pub Vec<(usize, usize)>);

impl Ranges {
    pub fn add(&mut self, a: usize, b: usize) {
        if a >= b { return; }
        self.0.push((a, b));
        self.0.sort();
        let mut out: Vec<(usize, usize)> = Vec::with_capacity(self.0.len());
        for (a, b) in self.0.drain(..) {
            match out.last_mut() { Some(l) if a <= l.1 => l.1 = l.1.max(b), _ => out.push((a, b)) }
        }
        self.0 = out;
    }
    /// One range holds all of `[a, b)`.
    pub fn covers(&self, a: usize, b: usize) -> bool { self.0.iter().any(|(x, y)| *x <= a && b <= *y) }
}

pub struct Session {
    pub id: String,
    frames: usize,
    pcm16: bool,
    /// Uploads are f32le (else s16le), as the session was opened.
    upload_f32: bool,
    parts: Vec<Part>,
    include_derived: bool,
    derived: Option<Part>,
    mix: Mutex<Vec<f32>>,
    arrived: Mutex<Ranges>,
    /// drums, vocals, harmonics: interleaved, full length, valid where `ready`.
    stems: Mutex<[Vec<f32>; 3]>,
    ready: Mutex<Ranges>,
    needle: AtomicU64,
    queued: AtomicBool,
    cancel: AtomicBool,
    state: Mutex<&'static str>,
    touched: Mutex<Instant>,
}

impl Session {
    fn view(&self) -> Value {
        let r = |x: &Ranges| Value::Array(x.0.iter().map(|(a, b)| json!([a, b])).collect());
        json!({
            "id": self.id, "frames": self.frames, "sample_rate": RATE, "output_sample_rate": RATE,
            "format": if self.pcm16 { "pcm16" } else { "pcm32" },
            "parts": self.parts.iter().map(|p| p.name()).collect::<Vec<_>>(),
            "derived_stem": self.derived.map(Part::name),
            "arrived": r(&self.arrived.lock().unwrap()),
            "ready": r(&self.ready.lock().unwrap()),
            "needle": self.needle.load(Ordering::Relaxed),
            "segment_frames": SEGMENT, "stride_frames": STRIDE, "segments": self.frames.div_ceil(STRIDE),
            "state": *self.state.lock().unwrap(),
            "track": null, "verified": false, "pieces": [],
        })
    }
}

fn q<T: std::str::FromStr>(req: &Request, k: &str) -> Result<Option<T>> {
    req.query.get(k).map(|v| v.parse::<T>().map_err(|_| anyhow::anyhow!("{k}={v:?}"))).transpose()
}

pub(crate) fn route(st: &Arc<State>, req: &Request, rest: &[&str], body: Vec<u8>) -> Response {
    reap(st);
    let r = match (req.method.as_str(), rest) {
        ("POST", []) => open(st, req, body),
        (m, [id, tail @ ..]) => {
            let Some(s) = st.sessions.lock().unwrap().get(*id).cloned() else { return Response::error(404, "no such session") };
            *s.touched.lock().unwrap() = Instant::now();
            match (m, tail) {
                ("GET", []) => Ok(Response::json(200, s.view())),
                ("PUT", ["pcm"]) => upload(st, &s, req, &body),
                ("PUT", ["window"]) => window(&s, &body),
                ("GET", ["stems", name]) => range(&s, req, name),
                ("DELETE", []) => {
                    s.cancel.store(true, Ordering::Relaxed);
                    st.sessions.lock().unwrap().remove(&s.id);
                    Ok(Response::json(200, json!({ "id": s.id, "state": "ended" })))
                }
                ("GET", ["pieces", ..]) => Ok(Response::error(404, "pieces are not stored here")),
                _ => Ok(Response::error(405, "not a session call")),
            }
        }
        _ => Ok(Response::error(405, "not a session call")),
    };
    r.unwrap_or_else(|e| Response::error(400, &format!("{e:#}")))
}

/// End sessions nobody has touched for a while.
fn reap(st: &State) {
    st.sessions.lock().unwrap().retain(|_, s| {
        let keep = s.touched.lock().unwrap().elapsed() < IDLE;
        if !keep { s.cancel.store(true, Ordering::Relaxed); }
        keep
    });
}

fn decode(body: &[u8], f32le: bool) -> Result<Vec<f32>> {
    let w = if f32le { 8 } else { 4 };
    if body.len() % w != 0 { bail!("{} bytes is not whole stereo frames", body.len()); }
    Ok(if f32le {
        body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    } else {
        body.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect()
    })
}

fn open(st: &Arc<State>, req: &Request, body: Vec<u8>) -> Result<Response> {
    let frames: usize = q(req, "frames")?.context("frames is required")?;
    if frames == 0 || frames > MAX_TRACK_SECS as usize * RATE as usize { bail!("frames {frames}"); }
    if q::<u32>(req, "sample_rate")?.unwrap_or(RATE) != RATE { bail!("sessions take the model's rate, {RATE}"); }
    if q::<u32>(req, "channels")?.unwrap_or(2) != 2 { bail!("two channels"); }
    if q::<u32>(req, "output_sample_rate")?.is_some_and(|r| r != RATE) { bail!("sessions are served at {RATE} Hz here"); }
    let pcm16 = match req.query.get("output_format").map(String::as_str).unwrap_or("pcm32") {
        "pcm32" | "f32le" => false,
        "pcm16" | "s16le" => true,
        f => bail!("output_format {f:?} for a session: pcm32 or pcm16"),
    };
    let f32le = match req.query.get("format").map(String::as_str).unwrap_or("s16le") { "f32le" => true, "s16le" => false, f => bail!("format {f:?}") };
    let p = params(&req.query.iter().filter(|(k, _)| *k == "parts" || *k == "include_derived").map(|(k, v)| (k.clone(), v.clone())).collect())?;
    if !st.splitter.ready() { return Ok(Response::error(503, "no separation model on this device")); }
    {
        let mut ss = st.sessions.lock().unwrap();
        if ss.len() >= MAX_SESSIONS { return Ok(Response::error(429, "too many sessions")); }
        let mut i = st.inner.lock().unwrap();
        i.attempt += 1;
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        let id = format!("s{:06x}-{}", (t ^ i.attempt.wrapping_mul(0x9e37_79b9)) & 0xff_ffff, i.attempt);
        drop(i);
        let s = Arc::new(Session {
            id: id.clone(), frames, pcm16, upload_f32: f32le, parts: p.parts, include_derived: p.include_derived, derived: p.derived,
            mix: Mutex::new(vec![0f32; frames * 2]), arrived: Mutex::new(Ranges::default()),
            stems: Mutex::new([vec![0f32; frames * 2], vec![0f32; frames * 2], vec![0f32; frames * 2]]),
            ready: Mutex::new(Ranges::default()), needle: AtomicU64::new(0), queued: AtomicBool::new(false),
            cancel: AtomicBool::new(false), state: Mutex::new("uploading"), touched: Mutex::new(Instant::now()),
        });
        ss.insert(id, s.clone());
        drop(ss);
        if !body.is_empty() { place(st, &s, 0, &decode(&body, f32le)?)?; }
        log::info!("stemd: session {} opened ({frames} frames)", s.id);
        return Ok(Response::json(201, s.view()));
    }
}

fn place(st: &State, s: &Arc<Session>, offset: usize, pcm: &[f32]) -> Result<()> {
    let n = pcm.len() / 2;
    if offset + n > s.frames { bail!("{offset}+{n} past the track's {} frames", s.frames); }
    {
        let mut m = s.mix.lock().unwrap();
        let mut arrived = s.arrived.lock().unwrap();
        if arrived.covers(offset, offset + n) && m[offset * 2..(offset + n) * 2] != *pcm { bail!("409: samples differ from what arrived"); }
        m[offset * 2..(offset + n) * 2].copy_from_slice(pcm);
        arrived.add(offset, offset + n);
    }
    // The whole track is here: the split can start (it normalises by all of it).
    if s.arrived.lock().unwrap().covers(0, s.frames) && !s.queued.swap(true, Ordering::Relaxed) {
        *s.state.lock().unwrap() = "queued";
        let mut i = st.inner.lock().unwrap();
        i.queue.push_front(format!("s:{}", s.id));   // a session is someone playing now
        st.wake.notify_one();
    }
    Ok(())
}

fn upload(st: &State, s: &Arc<Session>, req: &Request, body: &[u8]) -> Result<Response> {
    let offset: usize = q(req, "offset")?.unwrap_or(0);
    let f32le = match req.query.get("format").map(String::as_str) { Some("s16le") => false, Some("f32le") => true, None => s.upload_f32, Some(f) => bail!("format {f:?}") };
    let pcm = decode(body, f32le)?;
    match place(st, s, offset, &pcm) {
        Err(e) if format!("{e}").starts_with("409") => Ok(Response::error(409, &format!("{e}"))),
        Err(e) => Err(e),
        Ok(()) => Ok(Response::json(200, s.view())),
    }
}

fn window(s: &Session, body: &[u8]) -> Result<Response> {
    let v: Value = serde_json::from_slice(body).context("window JSON")?;
    let needle = v["needle"].as_u64().context("needle")? as usize;
    s.needle.store(needle.min(s.frames.saturating_sub(1)) as u64, Ordering::Relaxed);
    Ok(Response::json(200, s.view()))
}

fn range(s: &Session, req: &Request, name: &str) -> Result<Response> {
    let part = Part::parse(name).with_context(|| format!("no part {name:?}"))?;
    let shipped = s.parts.contains(&part) || (s.include_derived && s.derived == Some(part));
    if !shipped { return Ok(Response::error(404, "not a part of this session")); }
    let from: usize = q(req, "from")?.unwrap_or(0);
    let to: usize = q(req, "to")?.unwrap_or(s.frames).min(s.frames);
    if from >= to { bail!("from {from} to {to}"); }
    if !s.ready.lock().unwrap().covers(from, to) { return Ok(Response::error(409, "not ready")); }
    let st = s.stems.lock().unwrap();
    let (a, b) = (from * 2, to * 2);
    let y: Vec<f32> = match part {
        Part::Drums if s.derived == Some(Part::Drums) => {
            // The derived part is the mix minus the shipped ones: the three sum exactly.
            let m = s.mix.lock().unwrap();
            (a..b).map(|i| m[i] - st[1][i] - st[2][i]).collect()
        }
        Part::Drums => st[0][a..b].to_vec(),
        Part::Vocals => st[1][a..b].to_vec(),
        Part::Harmonics if s.derived == Some(Part::Harmonics) => {
            let m = s.mix.lock().unwrap();
            (a..b).map(|i| m[i] - st[0][i] - st[1][i]).collect()
        }
        Part::Harmonics => st[2][a..b].to_vec(),
    };
    let (body, gain, clipped) = if s.pcm16 {
        // One gain for the whole session: 3 dB of headroom over full scale.
        let gain = 1.0 / std::f32::consts::SQRT_2;
        let mut clipped = 0usize;
        let b: Vec<u8> = y.iter().flat_map(|v| {
            let x = v * gain * 32768.0;
            if !(-32768.0..=32767.0).contains(&x) { clipped += 1; }
            (x.round().clamp(-32768.0, 32767.0) as i16).to_le_bytes()
        }).collect();
        (b, gain, clipped)
    } else {
        (y.iter().flat_map(|v| v.to_le_bytes()).collect(), 1.0, 0)
    };
    Ok(Response { status: 200, content_type: "application/octet-stream", body, headers: vec![
        ("x-stemd-from", from.to_string()), ("x-stemd-frames", (to - from).to_string()),
        ("x-stemd-gain", gain.to_string()), ("x-stemd-clipped", clipped.to_string()),
    ] })
}

/// The worker's turn for a session: split around its needle, publishing
/// each finished stretch.
pub(crate) fn run(st: &State, s: &Arc<Session>) {
    if s.cancel.load(Ordering::Relaxed) { return; }
    *s.state.lock().unwrap() = "separating";
    let t0 = Instant::now();
    let mix = s.mix.lock().unwrap().clone();
    let on_ready = |from: usize, d: &[f32], v: &[f32], h: &[f32]| {
        let n = d.len().min(v.len()).min(h.len()) / 2;
        let n = n.min(s.frames.saturating_sub(from));
        if n == 0 { return; }
        {
            let mut stems = s.stems.lock().unwrap();
            let (a, b) = (from * 2, (from + n) * 2);
            stems[0][a..b].copy_from_slice(&d[..n * 2]);
            stems[1][a..b].copy_from_slice(&v[..n * 2]);
            stems[2][a..b].copy_from_slice(&h[..n * 2]);
        }
        s.ready.lock().unwrap().add(from, from + n);
    };
    let r = st.splitter.split_live(&mix, &s.needle, &on_ready, &s.cancel);
    let state = match r {
        Ok(()) if s.ready.lock().unwrap().covers(0, s.frames) => "done",
        Ok(()) => "ended",
        Err(_) if s.cancel.load(Ordering::Relaxed) => "ended",
        Err(e) => { log::warn!("stemd: session {} failed: {e:#}", s.id); "failed" }
    };
    log::info!("stemd: session {} {state} in {:.1}s", s.id, t0.elapsed().as_secs_f32());
    *s.state.lock().unwrap() = state;
}
