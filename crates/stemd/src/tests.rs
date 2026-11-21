//! The server as the CDJ-3000's stemd client drives it (cdj3k-mods
//! `stemd_client/`): health, POST f32le, poll, fetch harmonics and vocals,
//! rebuild drums as mix − harmonics − vocals.

use super::*;
use std::net::Ipv4Addr;

/// A stand-in model: drums half the mix, vocals and harmonics a quarter
/// each (they sum to the mix), slow enough to watch it run.
struct Fake { delay_ms: u64, runs: AtomicU32 }

impl Splitter for Fake {
    fn model_id(&self) -> String { "fake-00000001".into() }
    fn split(&self, mix: &[f32], progress: &(dyn Fn(f32) + Sync), cancel: &AtomicBool) -> Result<Split> {
        self.runs.fetch_add(1, Ordering::Relaxed);
        for k in 0..10 {
            if cancel.load(Ordering::Relaxed) { bail!("cancelled"); }
            std::thread::sleep(Duration::from_millis(self.delay_ms / 10));
            progress(k as f32 / 10.0);
        }
        let part = |g: f32| mix.iter().map(|v| v * g).collect();
        Ok(Split { drums: part(0.5), vocals: part(0.25), harmonics: part(0.25) })
    }
}

fn server(delay_ms: u64) -> (Server, Arc<Fake>) {
    let fake = Arc::new(Fake { delay_ms, runs: AtomicU32::new(0) });
    let resample: Resample = Arc::new(|x, _, _| x.to_vec());
    let s = start(Config { port: 0, advertise: false, resample, version: "test".into() }, fake.clone()).unwrap();
    (s, fake)
}

/// One request the way the deck's http.c sends it: `Connection: close`,
/// an exact Content-Length, the body read to EOF.
fn http(port: u16, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let mut c = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    write!(c, "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n").unwrap();
    if !body.is_empty() || method == "POST" { write!(c, "Content-Type: application/octet-stream\r\nContent-Length: {}\r\n", body.len()).unwrap(); }
    write!(c, "\r\n").unwrap();
    c.write_all(body).unwrap();
    let mut out = Vec::new();
    c.read_to_end(&mut out).unwrap();
    let head_end = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&out[..head_end]).to_string();
    let status: u16 = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let len: usize = head.lines().find_map(|l| l.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
    let body = out[head_end + 4..].to_vec();
    assert_eq!(body.len(), len, "Content-Length matches the body");
    (status, body)
}

fn json_of(b: &[u8]) -> Value { serde_json::from_slice(b).unwrap() }

fn noise(frames: usize) -> Vec<f32> {
    let mut x = 1u32;
    (0..frames * 2).map(|_| { x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); ((x >> 8) as f32 / 16_777_216.0 - 0.5) * 0.8 }).collect()
}

fn f32le(x: &[f32]) -> Vec<u8> { x.iter().flat_map(|v| v.to_le_bytes()).collect() }

fn wait_done(port: u16, id: &str) -> Value {
    let t0 = Instant::now();
    loop {
        let (s, b) = http(port, "GET", &format!("/v1/jobs/{id}"), b"");
        assert_eq!(s, 200);
        let d = json_of(&b);
        match d["stage"].as_str().unwrap() {
            "done" => return d,
            "failed" | "cancelled" => panic!("job ended: {d}"),
            _ => {}
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out: {d}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The deck's compatibility check, word for word: rate 44100, two
/// channels, and the strings "harmonics" and "vocals" in the document.
#[test]
fn health_passes_the_decks_compatibility_check() {
    let (s, _) = server(0);
    let (st, b) = http(s.port, "GET", "/v1/health", b"");
    assert_eq!(st, 200);
    let text = String::from_utf8(b.clone()).unwrap();
    let h = json_of(&b);
    assert_eq!((h["sample_rate"].as_u64(), h["channels"].as_u64()), (Some(44_100), Some(2)));
    assert!(text.contains("\"harmonics\"") && text.contains("\"vocals\""));
    assert_eq!(h["model_id"], "fake-00000001");
    assert_eq!(h["dsp_mode_pairs"], json!([[1, [44_100, 96_000]]]));
}

/// The deck's whole job: POST f32le asking for pcm16, poll through the
/// stages, fetch both stems, rebuild drums; the three sum to the mix.
#[test]
fn a_deck_job_round_trips_and_drums_rebuild_from_the_mix() {
    let (s, _) = server(200);
    let mix = noise(44_100 * 2);
    let (st, b) = http(s.port, "POST", "/v1/jobs?sample_rate=44100&channels=2&format=f32le&output_format=s16le", &f32le(&mix));
    assert_eq!(st, 202, "{}", String::from_utf8_lossy(&b));
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    assert!(id.len() <= 31 && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)), "{id}");
    let d = wait_done(s.port, &id);
    assert_eq!((d["sample_rate"].as_u64(), d["frames"].as_u64()), (Some(44_100), Some(44_100 * 2)));
    assert_eq!(d["derived_stem"], "drums");
    let names: Vec<&str> = d["stems"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["harmonics", "vocals"], "stemd's order");
    let fetch = |name: &str| -> Vec<f32> {
        let (st, b) = http(s.port, "GET", &format!("/v1/jobs/{id}/stems/{name}"), b"");
        assert_eq!(st, 200);
        let gain = d["stems"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap()["gain"].as_f64().unwrap() as f32;
        b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0 / gain).collect()
    };
    let (h, v) = (fetch("harmonics"), fetch("vocals"));
    let drums: Vec<f32> = mix.iter().zip(&h).zip(&v).map(|((m, h), v)| m - h - v).collect();
    let worst = mix.iter().zip(&drums).map(|(m, d)| (m * 0.5 - d).abs()).fold(0f32, f32::max);
    assert!(worst < 1e-4, "rebuilt drums off by {worst}");
}

/// The same audio again is a cache hit: 200, finished, no second run.
#[test]
fn the_same_track_twice_is_answered_from_the_cache() {
    let (s, fake) = server(50);
    let body = f32le(&noise(20_000));
    let path = "/v1/jobs?sample_rate=44100&channels=2&format=f32le&output_format=flac";
    let (_, b) = http(s.port, "POST", path, &body);
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    wait_done(s.port, &id);
    let (st, b) = http(s.port, "POST", path, &body);
    assert_eq!(st, 200);
    let d = json_of(&b);
    assert_eq!((d["stage"].as_str(), d["cached"].as_bool()), (Some("done"), Some(true)));
    assert_eq!(fake.runs.load(Ordering::Relaxed), 1);
    // FLAC stems are valid FLAC of the same 16-bit samples.
    let (st, flac) = http(s.port, "GET", &format!("/v1/jobs/{}/stems/vocals", d["id"].as_str().unwrap()), b"");
    assert_eq!(st, 200);
    assert_eq!(&flac[..4], b"fLaC");
}

/// DELETE stops a running job; asking for an unknown one is 404; what
/// isn't offered is a 400.
#[test]
fn cancel_unknown_and_refused_requests() {
    let (s, _) = server(2_000);
    let (_, b) = http(s.port, "POST", "/v1/jobs?format=f32le", &f32le(&noise(10_000)));
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(http(s.port, "DELETE", &format!("/v1/jobs/{id}"), b"").0, 200);
    let t0 = Instant::now();
    while json_of(&http(s.port, "GET", &format!("/v1/jobs/{id}"), b"").1)["stage"] != "cancelled" {
        assert!(t0.elapsed() < Duration::from_secs(3), "cancel took effect");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(http(s.port, "GET", "/v1/jobs/nope-1", b"").0, 404);
    for bad in ["output_format=mp3", "parts=bass,other,vocals", "output_sample_rate=96000&dsp_mode=1&sample_rate=48000", "output_sample_rate=48000&dsp_mode=1"] {
        let (st, _) = http(s.port, "POST", &format!("/v1/jobs?format=f32le&{bad}"), &f32le(&noise(100)));
        assert_eq!(st, 400, "{bad}");
    }
}

/// `dsp_mode=1`: the stems come back at 96 kHz through the CDJ-3000's own
/// filter, so drums rebuilt from a mix the deck converted with that filter
/// cancel.
#[test]
fn dsp_mode_1_stems_cancel_against_a_mix_converted_the_same_way() {
    let (s, _) = server(0);
    let mix = noise(44_100);
    let (_, b) = http(s.port, "POST", "/v1/jobs?sample_rate=44100&channels=2&format=f32le&output_format=pcm32&output_sample_rate=96000&dsp_mode=1", &f32le(&mix));
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    let d = wait_done(s.port, &id);
    assert_eq!(d["sample_rate"].as_u64(), Some(96_000));
    let get = |n: &str| -> Vec<f32> { http(s.port, "GET", &format!("/v1/jobs/{id}/stems/{n}"), b"").1.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect() };
    let (h, v) = (get("harmonics"), get("vocals"));
    let deck_mix = mode1::resample_stereo(&mix);
    assert_eq!(h.len(), deck_mix.len());
    let drums_want = mode1::resample_stereo(&mix.iter().map(|x| x * 0.5).collect::<Vec<_>>());
    let worst = deck_mix.iter().zip(&h).zip(&v).zip(&drums_want).map(|(((m, h), v), d)| (m - h - v - d).abs()).fold(0f32, f32::max);
    assert!(worst < 1e-5, "{worst}");
}

/// A stand-in that splits as a session wants: from the needle onward in
/// 0.5 s stretches (re-reading the needle each time), then the rest.
struct LiveFake;

impl Splitter for LiveFake {
    fn model_id(&self) -> String { "livefake-1".into() }
    fn split(&self, mix: &[f32], _: &(dyn Fn(f32) + Sync), _: &AtomicBool) -> Result<Split> {
        let part = |g: f32| mix.iter().map(|v| v * g).collect();
        Ok(Split { drums: part(0.5), vocals: part(0.25), harmonics: part(0.25) })
    }
    fn split_live(&self, mix: &[f32], needle: &AtomicU64, on_ready: &(dyn Fn(usize, &[f32], &[f32], &[f32]) + Sync), cancel: &AtomicBool) -> Result<()> {
        let frames = mix.len() / 2;
        let step = 22_050;
        let n = frames.div_ceil(step);
        let mut done = vec![false; n];
        while let Some(k) = { let at = (needle.load(Ordering::Relaxed) as usize / step).min(n - 1); (at..n).chain(0..at).find(|&k| !done[k]) } {
            if cancel.load(Ordering::Relaxed) { bail!("cancelled"); }
            std::thread::sleep(Duration::from_millis(15));
            let (a, b) = (k * step * 2, ((k + 1) * step * 2).min(mix.len()));
            let p = |g: f32| -> Vec<f32> { mix[a..b].iter().map(|v| v * g).collect() };
            on_ready(k * step, &p(0.5), &p(0.25), &p(0.25));
            done[k] = true;
        }
        Ok(())
    }
}

/// A session as a deck uses one: open, upload (out of order), point the
/// window at the needle; the ranges under the needle are ready first and
/// read back as split, with the x-stemd headers; a range not ready is 409;
/// the derived part sums exactly; DELETE ends it.
#[test]
fn a_session_serves_the_needle_first() {
    let resample: Resample = Arc::new(|x, _, _| x.to_vec());
    let s = start(Config { port: 0, advertise: false, resample, version: "test".into() }, Arc::new(LiveFake)).unwrap();
    let frames = 44_100 * 20;
    let mix = noise(frames);
    let (st, b) = http(s.port, "POST", &format!("/v1/sessions?frames={frames}&format=f32le&output_format=pcm32&include_derived=true"), b"");
    assert_eq!(st, 201, "{}", String::from_utf8_lossy(&b));
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    let needle = 44_100 * 12;
    let (st, _) = http(s.port, "PUT", &format!("/v1/sessions/{id}/window"), json!({"needle": needle, "ahead": {"frames": 441000, "priority": "critical"}}).to_string().as_bytes());
    assert_eq!(st, 200);
    let half = frames / 2;
    assert_eq!(http(s.port, "PUT", &format!("/v1/sessions/{id}/pcm?offset={half}"), &f32le(&mix[half * 2..])).0, 200);
    let v = json_of(&http(s.port, "GET", &format!("/v1/sessions/{id}"), b"").1);
    assert_eq!(v["state"], "uploading", "half the track is not enough to start");
    assert_eq!(http(s.port, "PUT", &format!("/v1/sessions/{id}/pcm?offset=0"), &f32le(&mix[..half * 2])).0, 200);
    // The first ready range is the one under the needle.
    let t0 = Instant::now();
    let first = loop {
        let v = json_of(&http(s.port, "GET", &format!("/v1/sessions/{id}"), b"").1);
        if let Some(r) = v["ready"].as_array().and_then(|r| r.first()).cloned() { break r; }
        assert!(t0.elapsed() < Duration::from_secs(5), "{v}");
        std::thread::sleep(Duration::from_millis(5));
    };
    let (a, b) = (first[0].as_u64().unwrap() as usize, first[1].as_u64().unwrap() as usize);
    assert!(a <= needle && needle < b, "first ready {a}..{b} holds the needle {needle}");
    assert_eq!(http(s.port, "GET", &format!("/v1/sessions/{id}/stems/vocals?from=0&to=1000"), b"").0, 409, "the top isn't ready yet");
    let get = |name: &str, from: usize, to: usize| -> Vec<f32> {
        let (st, body) = http(s.port, "GET", &format!("/v1/sessions/{id}/stems/{name}?from={from}&to={to}"), b"");
        assert_eq!(st, 200, "{name} {from}..{to}");
        body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    };
    let (h, vo, d) = (get("harmonics", needle, needle + 1000), get("vocals", needle, needle + 1000), get("drums", needle, needle + 1000));
    for i in 0..2000 {
        let m = mix[needle * 2 + i];
        assert!((h[i] - m * 0.25).abs() < 1e-6 && (vo[i] - m * 0.25).abs() < 1e-6, "as split");
        assert!((m - h[i] - vo[i] - d[i]).abs() < 1e-6, "the derived drums sum exactly");
    }
    // Everything ends up ready; then DELETE.
    let t0 = Instant::now();
    while json_of(&http(s.port, "GET", &format!("/v1/sessions/{id}"), b"").1)["state"] != "done" {
        assert!(t0.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(get("vocals", 0, 1000).len(), 2000);
    assert_eq!(http(s.port, "DELETE", &format!("/v1/sessions/{id}"), b"").0, 200);
    assert_eq!(http(s.port, "GET", &format!("/v1/sessions/{id}"), b"").0, 404);
}

#[test]
fn session_ranges_merge() {
    let mut r = session::Ranges::default();
    r.add(10, 20); r.add(30, 40); r.add(20, 30); r.add(50, 50);
    assert_eq!(r.0, vec![(10, 40)]);
    assert!(r.covers(12, 38) && !r.covers(5, 15) && !r.covers(35, 45));
}
