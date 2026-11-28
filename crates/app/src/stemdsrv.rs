//! OpenDeck as a **stemd server** (`opendeck-stemd`): CDJ-3000s running
//! cdj3k-mods' Server Stems, and any other stemd client, find us over mDNS
//! (`_stemd._tcp`, port 8420) and get stems split by this device's model.
//! On whenever this device serves live stems ([`crate::livestems::enabled`]:
//! a model, not a Pi); `OPENDECK_STEMD=0` turns it off.  The deck's own
//! split always comes first, as for live stems.  Design:
//! `docs/design/stemd-server.md`.

use crate::livestems::{self, LOCAL_SPLIT};
use crate::separate::{self, Job};
use crate::stems::StemBank;
use anyhow::Result;
use opendeck_stemd::{Split, Splitter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The model behind the server, with the deck's priority.
pub struct ModelSplitter { split: livestems::SplitFn, local: &'static AtomicBool }

impl ModelSplitter {
    pub fn new() -> ModelSplitter { ModelSplitter { split: livestems::model_split(), local: &LOCAL_SPLIT } }
    #[cfg(test)]
    pub(crate) fn with(split: livestems::SplitFn, local: &'static AtomicBool) -> ModelSplitter { ModelSplitter { split, local } }
}

impl Splitter for ModelSplitter {
    fn model_id(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (separate::model::device(), separate::model::units(), std::env::var("OPENDECK_SEPARATE_MODEL").unwrap_or_default()).hash(&mut h);
        format!("opendeck-htdemucs-{:08x}", h.finish() as u32)
    }

    fn ready(&self) -> bool { separate::model::present() || cfg!(test) }

    fn split(&self, mix: &[f32], progress: &(dyn Fn(f32) + Sync), cancel: &AtomicBool) -> Result<Split> {
        while self.local.load(Ordering::Relaxed) {
            if cancel.load(Ordering::Relaxed) { anyhow::bail!("cancelled"); }
            std::thread::sleep(Duration::from_millis(250));
        }
        let track = Arc::new(mix.to_vec());
        let bank = StemBank::filling_split(&track);
        let job = Job::default();
        let done = AtomicBool::new(false);
        let r = std::thread::scope(|s| {
            let h = s.spawn(|| { let r = (self.split)(&track, &job, &bank); done.store(true, Ordering::Release); r });
            while !done.load(Ordering::Acquire) {
                progress(job.progress.load(Ordering::Relaxed) as f32 / 1000.0);
                // The client cancelled, or this device's deck wants the model.
                if cancel.load(Ordering::Relaxed) || self.local.load(Ordering::Relaxed) { job.cancel.store(true, Ordering::Relaxed); }
                std::thread::sleep(Duration::from_millis(100));
            }
            h.join().map_err(|_| anyhow::anyhow!("split panicked"))?
        });
        r?;
        anyhow::ensure!(!job.cancel.load(Ordering::Relaxed), "cancelled");
        // The bank holds drums, vocal, harmonics.
        Ok(Split { drums: bank.stem_levels(0), vocals: bank.stem_levels(1), harmonics: bank.stem_levels(2) })
    }

    /// A session: the device split follows the client's needle exactly
    /// (no look-ahead) and each bank page is reported the moment it is
    /// whole.
    fn split_live(&self, mix: &[f32], needle: &std::sync::atomic::AtomicU64, on_ready: &(dyn Fn(usize, &[f32], &[f32], &[f32]) + Sync), cancel: &AtomicBool) -> Result<()> {
        use crate::stems::PAGE;
        while self.local.load(Ordering::Relaxed) {
            if cancel.load(Ordering::Relaxed) { anyhow::bail!("cancelled"); }
            std::thread::sleep(Duration::from_millis(250));
        }
        let track = Arc::new(mix.to_vec());
        let bank = StemBank::filling_split(&track);
        let head = Arc::new(std::sync::atomic::AtomicU64::new(needle.load(Ordering::Relaxed) * 2));
        let job = Job { playhead: Some(Arc::clone(&head)), at_playhead: true, ..Default::default() };
        let done = AtomicBool::new(false);
        let mut sent = vec![false; bank.page_count()];
        let report = |bank: &StemBank, sent: &mut Vec<bool>| {
            for p in 0..sent.len() {
                if sent[p] || !bank.page_ready(p) { continue; }
                let (a, b) = (p * PAGE, ((p + 1) * PAGE).min(bank.len()));
                let lv = |st: usize| -> Vec<f32> { (a..b).map(|i| bank.sample(st, i) as f32 / 32768.0 * bank.scale).collect() };
                on_ready(a / 2, &lv(0), &lv(1), &lv(2));
                sent[p] = true;
            }
        };
        let r = std::thread::scope(|s| {
            let h = s.spawn(|| { let r = (self.split)(&track, &job, &bank); done.store(true, Ordering::Release); r });
            while !done.load(Ordering::Acquire) {
                head.store(needle.load(Ordering::Relaxed) * 2, Ordering::Relaxed);
                if cancel.load(Ordering::Relaxed) || self.local.load(Ordering::Relaxed) { job.cancel.store(true, Ordering::Relaxed); }
                report(&bank, &mut sent);
                std::thread::sleep(Duration::from_millis(30));
            }
            h.join().map_err(|_| anyhow::anyhow!("split panicked"))?
        });
        report(&bank, &mut sent);
        r?;
        anyhow::ensure!(!job.cancel.load(Ordering::Relaxed), "cancelled");
        Ok(())
    }
}

/// Start the server if this device should run one.
pub fn start() -> Option<opendeck_stemd::Server> {
    if std::env::var("OPENDECK_STEMD").is_ok_and(|v| v == "0") || !livestems::enabled() { return None; }
    let resample: opendeck_stemd::Resample = Arc::new(|x, from, to| crate::audio::resample_interleaved(x, 2, from, to).unwrap_or_default());
    let cfg = opendeck_stemd::Config { port: opendeck_stemd::PORT, advertise: true, resample, version: env!("CARGO_PKG_VERSION").into() };
    match opendeck_stemd::start(cfg, Arc::new(ModelSplitter::new())) {
        Ok(s) => Some(s),
        Err(e) => { log::warn!("stemd: not started: {e:#}"); None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stems::SPLIT_SCALE;

    /// The real model behind the server (`OPENDECK_SEPARATE_MODEL`, a model
    /// feature) on a real track (`OPENDECK_TEST_STEMD_REAL=<file>`), driven
    /// over HTTP as the CDJ-3000's client drives it: f32le up, FLAC at
    /// 96 kHz through dsp mode 1 back.  Prints the times.  Skipped without
    /// the variable.
    #[test]
    fn real_model_stemd_job_over_http() {
        use std::io::{Read, Write};
        let Ok(file) = std::env::var("OPENDECK_TEST_STEMD_REAL") else { return };
        let (s, sr, _, _) = crate::audio::decode_file(std::path::Path::new(&file)).unwrap();
        let mix = crate::audio::resample_interleaved(&s, 2, sr, 44_100).unwrap();
        let resample: opendeck_stemd::Resample = Arc::new(|x, f, t| crate::audio::resample_interleaved(x, 2, f, t).unwrap());
        let srv = opendeck_stemd::start(opendeck_stemd::Config { port: 0, advertise: false, resample, version: "test".into() }, Arc::new(ModelSplitter::new())).unwrap();
        let http = |method: &str, path: &str, body: &[u8]| -> (u16, Vec<u8>) {
            let mut c = std::net::TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
            write!(c, "{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
            c.write_all(body).unwrap();
            let mut out = Vec::new();
            c.read_to_end(&mut out).unwrap();
            let e = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            (String::from_utf8_lossy(&out[..e]).split_whitespace().nth(1).unwrap().parse().unwrap(), out[e + 4..].to_vec())
        };
        let t0 = opendeck_types::time::Instant::now();
        let body: Vec<u8> = mix.iter().flat_map(|v| v.to_le_bytes()).collect();
        let (st, b) = http("POST", "/v1/jobs?sample_rate=44100&channels=2&format=f32le&output_format=flac&output_sample_rate=96000&dsp_mode=1", &body);
        assert_eq!(st, 202, "{}", String::from_utf8_lossy(&b));
        let id = serde_json::from_slice::<serde_json::Value>(&b).unwrap()["id"].as_str().unwrap().to_string();
        let d = loop {
            let d: serde_json::Value = serde_json::from_slice(&http("GET", &format!("/v1/jobs/{id}"), b"").1).unwrap();
            if d["stage"] == "done" { break d; }
            assert!(d["stage"] != "failed", "{d}");
            std::thread::sleep(Duration::from_millis(500));
        };
        let secs = mix.len() as f64 / 2.0 / 44_100.0;
        eprintln!("stemd: {secs:.0}s track split in {:.1}s ({:.1}x real time), residual {} dB", t0.elapsed().as_secs_f64(), secs / t0.elapsed().as_secs_f64(), d["model_residual_db"]);
        for n in ["harmonics", "vocals"] {
            let (st, b) = http("GET", &format!("/v1/jobs/{id}/stems/{n}"), b"");
            assert_eq!(st, 200);
            assert_eq!(&b[..4], b"fLaC");
            eprintln!("stemd: {n} {} bytes, gain {}", b.len(), d["stems"].as_array().unwrap().iter().find(|s| s["name"] == n).unwrap()["gain"]);
        }
        assert_eq!(d["sample_rate"].as_u64(), Some(96_000));
    }

    /// The app's splitter through the deck-priority path: the stems come
    /// out of the bank at their levels; a deck split holds it back and
    /// then cancels a split under way.
    #[test]
    fn the_model_splitter_yields_to_the_deck_and_returns_levels() {
        static LOCAL: AtomicBool = AtomicBool::new(false);
        let fake: livestems::SplitFn = Arc::new(|t, job, bank| {
            let q = |k: f32| -> Vec<i16> { t.iter().map(|v| (v * k / SPLIT_SCALE * 32768.0).round() as i16).collect() };
            for _ in 0..20 { if job.cancel.load(Ordering::Relaxed) { anyhow::bail!("cancelled") } std::thread::sleep(Duration::from_millis(10)); }
            bank.write(0, [&q(0.5), &q(0.25), &q(0.25)]);
            job.progress.store(1000, Ordering::Relaxed);
            Ok(())
        });
        let s = ModelSplitter::with(fake, &LOCAL);
        let mix: Vec<f32> = (0..20_000).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect();
        let last = std::sync::Mutex::new(0f32);
        let got = s.split(&mix, &|p| *last.lock().unwrap() = p, &AtomicBool::new(false)).unwrap();
        let worst = mix.iter().zip(&got.drums).map(|(m, d)| (m * 0.5 - d).abs()).fold(0f32, f32::max);
        assert!(worst < 1e-4, "{worst}");
        assert!(got.vocals.len() == mix.len() && got.harmonics.len() == mix.len());

        // The deck starts splitting mid-way: this one stops.
        let t = std::thread::spawn(|| { std::thread::sleep(Duration::from_millis(60)); LOCAL.store(true, Ordering::Relaxed); });
        assert!(s.split(&mix, &|_| {}, &AtomicBool::new(false)).is_err(), "gave way to the deck");
        t.join().unwrap();
        LOCAL.store(false, Ordering::Relaxed);
    }
}

