//! The sound card, through cpal: the native platforms' [`AudioOut`].
//!
//! On Linux the device stream is owned by a supervisor thread so it can be
//! rebuilt when ALSA gives up: after an xrun on a direct hw device (a loaded
//! single-core Pi, an HDMI sink) cpal reports POLLERR from every poll and
//! never recovers, leaving the deck silent.  The ring consumer lives behind
//! a lock only the live callback ever takes (uncontended, try_lock, never
//! blocks the RT path) so the next stream's callback picks it up where the
//! dead one left off.  Elsewhere the stream is simply kept.

use crate::audio::try_realtime;
use crate::audio_out::{AudioOut, Format, RenderSource, Running};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};

/// The system's default output device, at its default config.
pub struct CpalOut {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
}

/// The default output device.  A deck that boots without its sound
/// interface (a USB card not plugged in, or dropped in a supply dip) still
/// comes up on Linux (the Pi appliance): silent, and on the interface as
/// soon as it appears.  Elsewhere a missing device is an error.
pub fn default_output() -> Result<Box<dyn AudioOut>> {
    match default_device() {
        Ok((device, config)) => Ok(Box::new(CpalOut { device, config })),
        #[cfg(target_os = "linux")]
        Err(e) => {
            log::warn!("no audio output ({e:#}): starting silent, waiting for a device");
            Ok(Box::new(Waiting))
        }
        #[cfg(not(target_os = "linux"))]
        Err(e) => Err(e),
    }
}

impl AudioOut for CpalOut {
    fn format(&self) -> Format {
        Format { sample_rate: self.config.sample_rate().0, channels: self.config.channels() as usize }
    }

    fn start(self: Box<Self>, src: RenderSource) -> Result<Running> {
        start_device(self.device, stream_config(src.format()), src)
    }
}

/// Linux: the stream lives on a supervisor thread that rebuilds it.
#[cfg(target_os = "linux")]
fn start_device(device: cpal::Device, config: cpal::StreamConfig, src: RenderSource) -> Result<Running> {
    let failed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
    let stop = Arc::new(AtomicBool::new(false));
    let sd = Arc::clone(&stop);
    std::thread::Builder::new().name("audio-dev".into()).spawn(move || {
        let stream = match build_stream(&device, &config, &src, &failed) {
            Ok(st) => { let _ = tx.send(Ok(())); st }
            Err(e) => { let _ = tx.send(Err(e)); return; }
        };
        supervise(device, config, src, failed, stream, &sd);
    }).context("failed to spawn audio-dev thread")?;
    rx.recv().context("audio-dev thread died")??;
    Ok(Running::stop_on_drop(stop))
}

/// Elsewhere the stream is kept as it is.
#[cfg(not(target_os = "linux"))]
fn start_device(device: cpal::Device, config: cpal::StreamConfig, src: RenderSource) -> Result<Running> {
    let failed = Arc::new(AtomicBool::new(false));
    Ok(Running::holding(build_stream(&device, &config, &src, &failed)?))
}

/// No device at start (Linux): keep time at real time, silently (the deck
/// plays, its playhead moves), and look for the default device every 2 s;
/// once it opens, it is supervised like any other.
#[cfg(target_os = "linux")]
pub struct Waiting;

#[cfg(target_os = "linux")]
impl AudioOut for Waiting {
    // The rate a CD-sourced library plays at; a device that turns up later
    // is opened at this rate (ALSA's plug converts if it must).
    fn format(&self) -> Format { Format { sample_rate: 44_100, channels: 2 } }

    fn start(self: Box<Self>, src: RenderSource) -> Result<Running> {
        use std::time::{Duration, Instant};
        let stop = Arc::new(AtomicBool::new(false));
        let sd = Arc::clone(&stop);
        let format = src.format();
        let config = stream_config(format);
        std::thread::Builder::new().name("audio-dev".into()).spawn(move || {
            let failed = Arc::new(AtomicBool::new(false));
            let mut fill = src.render();
            let mut buf = vec![0f32; format.sample_rate as usize / 100 * format.channels];
            let t0 = Instant::now();
            let mut n = 0u32;
            loop {
                if sd.load(Ordering::Relaxed) { return; }
                if n % 200 == 0 && n > 0 {
                    if let Some((dev, st)) = default_device().ok()
                        .and_then(|(d, _)| build_stream(&d, &config, &src, &failed).ok().map(|st| (d, st))) {
                        log::info!("audio output appeared ({}): playing to it",
                                   dev.name().unwrap_or_else(|_| "?".into()));
                        drop(fill);
                        supervise(dev, config, src, failed, st, &sd);
                        return;
                    }
                }
                fill.fill(&mut buf);
                n += 1;
                let due = t0 + Duration::from_millis(10) * n;
                if let Some(w) = due.checked_duration_since(Instant::now()) { std::thread::sleep(w); }
            }
        }).context("failed to spawn audio-dev thread")?;
        Ok(Running::stop_on_drop(stop))
    }
}

fn stream_config(f: Format) -> cpal::StreamConfig {
    cpal::StreamConfig {
        channels:    f.channels as u16,
        sample_rate: cpal::SampleRate(f.sample_rate),
        buffer_size: cpal::BufferSize::Default,
    }
}

/// The default output device and its default config.
fn default_device() -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let device = cpal::default_host()
        .default_output_device()
        .context("no output audio device found")?;
    let supported = device
        .default_output_config()
        .context("failed to get default output config")?;
    Ok((device, supported))
}

/// Keep a device stream alive: when the callback reports a failure (an
/// xrun storm, a USB card pulled and put back), rebuild it, backing off to
/// one try every 2 s.  Returns at shutdown.
#[cfg(target_os = "linux")]
fn supervise(device: cpal::Device, config: cpal::StreamConfig, src: RenderSource, failed: Arc<AtomicBool>,
             mut stream: cpal::Stream, sd: &AtomicBool) {
    use std::time::Duration;
    while !sd.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
        if !failed.load(Ordering::Relaxed) { continue; }
        log::warn!("audio stream failed — rebuilding the device stream");
        drop(stream);
        let mut wait = Duration::from_millis(100);
        stream = loop {
            match build_stream(&device, &config, &src, &failed) {
                Ok(st) => break st,
                Err(e) => {
                    log::warn!("audio stream rebuild failed ({e:#}); retrying in {} ms", wait.as_millis());
                    std::thread::sleep(wait);
                    wait = (wait * 2).min(Duration::from_secs(2));
                    if sd.load(Ordering::Relaxed) { return; }
                }
            }
        };
        failed.store(false, Ordering::Relaxed);
        log::info!("audio stream rebuilt");
    }
}

fn build_stream(device: &cpal::Device, config: &cpal::StreamConfig, src: &RenderSource, failed: &Arc<AtomicBool>) -> Result<cpal::Stream> {
    let failed = Arc::clone(failed);
    let mut fill = src.render();
    let mut rt_asked = false;
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |out: &mut [f32], _info| {
                // The device thread is cpal's; ask for RT priority from inside
                // it, once (a syscall, no allocation).
                if !rt_asked { rt_asked = true; try_realtime("cpal output", 30); }
                fill.fill(out);
            },
            move |err| {
                // Once per failure: cpal repeats the error on every poll of a
                // dead ALSA stream, which would flood the log.
                if !failed.swap(true, Ordering::Relaxed) {
                    log::error!("audio stream error: {err}");
                }
            },
            None,
        )
        .context("failed to build output stream")?;
    stream.play().context("failed to start audio stream")?;
    Ok(stream)
}

#[cfg(all(test, target_os = "linux"))]
mod no_device_tests {
    use super::*;
    use crate::audio::AudioHandle;
    use std::time::Duration;

    /// A deck that boots without its sound interface (the Pi appliance with
    /// the USB card unplugged) still runs: silent, keeping real time, so the
    /// screen comes up and a track plays on; before, the app exited and the
    /// service restarted it until systemd gave up.
    #[test]
    fn starts_silent_without_a_device_and_keeps_time() {
        let sr = 44_100;
        let h = AudioHandle::open(Box::new(Waiting), None).unwrap();
        h.load_samples(Arc::new(vec![0.1f32; sr as usize * 2 * 4]), sr, 2).unwrap();   // 4 s stereo
        h.set_playing(true);
        std::thread::sleep(Duration::from_millis(300));
        let p1 = h.position.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(500));
        let p2 = h.position.load(Ordering::Relaxed);
        assert!(p1 > 0, "the playhead moves with no device");
        // Real time, not as fast as the CPU can go: 0.5 s is 22 050 frames
        // (44 100 samples); allow the ring's slack either way.
        let d = p2 - p1;
        assert!(d > 10_000 && d < 60_000, "advanced {d} in 0.5 s");
    }
}
