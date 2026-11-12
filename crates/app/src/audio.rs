//! Audio decode and real-time playback with variable-speed timestretching.
//!
//! Architecture:
//!
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │ processor thread (non-RT)                                           │
//! │  samples[position..] → TimestretechStage (RubberBand R3) → rtrb   │
//! └──────────────────────────────────────┬──────────────────────────────┘
//!                                        │ lock-free ring buffer
//! ┌──────────────────────────────────────▼──────────────────────────────┐
//! │ cpal callback (RT thread)                                           │
//! │  rtrb consumer → output device                                      │
//! └─────────────────────────────────────────────────────────────────────┘
//!
//! Shared atomics (all Relaxed unless noted):
//!   position    — source sample index; written by processor, readable by UI
//!   playing     — play/pause flag; written by UI/HID, read by both threads
//!   speed       — f32 bits; written by UI/HID, read by processor
//!   drain_flag  — set by processor on seek-detect; cleared by cpal on drain

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use crate::stems::{Mixer, StemControl};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opendeck_decode::{SymphoniaDecoder, TrackTags};
use opendeck_timestretch::Stretcher;
use opendeck_types::{Decoder, PipelineStage};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Frames fed to the timestretch engine per iteration.
const BLOCK_FRAMES: usize = 512;

/// Ring buffer capacity in device samples (not frames).
///
/// Keep this small so speed changes are heard immediately.  At 44.1kHz stereo
/// 8 192 samples ≈ 93 ms — large enough to absorb thread-scheduling jitter,
/// small enough that the pitch fader response feels instant.
const RING_BUFFER_SAMPLES: usize = 8_192;

/// Ring length in samples: the default above (~93 ms of stereo 44.1 k), or
/// OPENDECK_RING_MS at the device rate — more decode-ahead for a board whose
/// scheduler cannot wake the processor in time (a single-core Pi).  The ring
/// is flushed on every seek, so a longer one does not delay CUE.
fn ring_samples(device_sr: u32, device_ch: usize) -> usize {
    match std::env::var("OPENDECK_RING_MS").ok().and_then(|v| v.parse::<f64>().ok()) {
        Some(ms) if ms > 0.0 => ((ms / 1000.0 * device_sr as f64) as usize * device_ch).max(BACK_PRESSURE_SLOTS * 2),
        _ => RING_BUFFER_SAMPLES,
    }
}

/// Best-effort SCHED_FIFO for an audio thread (Linux).  Needs an rtprio
/// limit (`deploy/pi/rt-audio`); without one it logs once and carries on at
/// normal priority.  OPENDECK_RT=0 opts out.
#[cfg(target_os = "linux")]
fn try_realtime(name: &str, prio: i32) {
    if std::env::var("OPENDECK_RT").as_deref() == Ok("0") { return; }
    let param = libc::sched_param { sched_priority: prio };
    let rc = unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) };
    if rc == 0 {
        log::info!("{name}: SCHED_FIFO priority {prio}");
    } else {
        log::info!("{name}: real-time priority not permitted (rtprio limit 0?) — see deploy/pi/rt-audio; running at normal priority");
    }
}
#[cfg(not(target_os = "linux"))]
fn try_realtime(_name: &str, _prio: i32) {}

/// Minimum free slots required before processing another block.
///
/// At the minimum supported speed (0.25×) RubberBand outputs 4× input frames:
///   BLOCK_FRAMES / 0.25 × device_ch = 512 × 4 × 2 = 4 096 samples worst case.
/// Must be < RING_BUFFER_SAMPLES so back-pressure never permanently stalls.
const BACK_PRESSURE_SLOTS: usize = BLOCK_FRAMES * 4 * 2; // = 4 096

/// If position jumps by more than this many source samples the processor
/// treats it as a seek and resets the timestretch engine.
/// Sentinel for `seek_request`: no seek pending.
const NO_SEEK: u64 = u64::MAX;

// ── Audio integrity counters ──────────────────────────────────────────────────

/// Glitch counters for the real-time path.  A performance deck must never
/// dropout silently: the RT callback fills zeros when the ring is empty, so we
/// count those samples and surface them.  All Relaxed — these are diagnostics,
/// not synchronisation, and the RT callback only touches them on a glitch.
#[derive(Default)]
pub struct AudioStats {
    /// Samples of silence the callback filled because the ring was empty while
    /// playing — audible underruns (clicks / dropouts).  Steady-state only:
    /// the *expected* re-prime silence right after a seek is excluded (that goes
    /// to `seek_priming_samples`), so this is a clean "the core starved" signal.
    pub underrun_samples: AtomicU64,
    /// Callbacks that hit ≥1 steady-state underrun (distinct glitch events).
    pub underrun_events:  AtomicU64,
    /// Silence samples the callback filled on the buffer where it flushed the
    /// ring for a seek — expected re-prime gap, not a starvation glitch.
    pub seek_priming_samples: AtomicU64,
    /// Frames the producer dropped because the ring was full (overrun).
    pub dropped_frames:   AtomicU64,
}

// ── Public handle ─────────────────────────────────────────────────────────────

pub struct AudioHandle {
    /// Interleaved f32 PCM from the decoded file.  Swappable so a new track
    /// can be loaded at runtime without tearing down the audio thread/stream:
    /// the processor loads the current buffer once per block (lock-free).
    pub samples:     Arc<ArcSwap<Vec<f32>>>,
    /// Current source read position in samples (not frames).
    /// Updated by the processor thread; read by renderer for waveform scroll.
    pub position:    Arc<AtomicU64>,
    /// Play / pause.
    pub playing:     Arc<AtomicBool>,
    /// The processor thread, parked while paused; see `set_playing`.
    proc_thread:     std::thread::Thread,
    /// Playback speed as f32 bits (1.0 = normal, 0.5 = half, 2.0 = double).
    /// Set via `speed_store` / `speed_load` helpers.
    pub speed:       Arc<AtomicU32>,
    /// Master Tempo / key lock.  true = tempo changes but pitch (key) is held
    /// (time-stretch); false = varispeed, pitch tracks speed like vinyl.  Read
    /// by the processor each block.
    pub key_lock:    Arc<AtomicBool>,
    /// Loop: [loop_start, loop_end) in interleaved samples, honoured by the
    /// processor while `loop_active`.  The wrap happens INSIDE a source block
    /// (tail of the loop then its head, one continuous read) so the stretcher
    /// never sees a discontinuity — no reset, no click.
    pub loop_start:  Arc<AtomicU64>,
    pub loop_end:    Arc<AtomicU64>,
    pub loop_active: Arc<AtomicBool>,
    /// Total source samples the processor has read, ever — monotonic even
    /// while a loop wraps `position` back.  SLIP measures its shadow playhead
    /// against this: audio-clock accurate, no wall-clock drift.
    pub source_consumed: Arc<AtomicU64>,
    /// Source samples that have been decoded but are not yet audible: the
    /// contents of the ring buffer plus the stretcher's internal latency.
    ///
    /// `position` is the *decoder's* cursor and runs ahead of what the listener
    /// hears by this much.  Subtract it to get the true playhead.
    pub in_flight:   Arc<AtomicU64>,
    /// UI → processor seek channel (source sample index, or NO_SEEK). Separate
    /// from `position` so a seek can't be clobbered by the processor's own
    /// per-block progress store and lost.
    pub seek_request: Arc<AtomicU64>,
    /// Paused jog scrub: what the processor reads toward (see `scrub_to`).
    scrub:           ScrubShared,
    /// Stems: the loaded bank and the three stem levels (`stems.rs`).
    pub stems:       Arc<StemControl>,
    /// Real-time glitch counters (underruns / dropped frames).
    pub stats:       Arc<AudioStats>,
    pub sample_rate: u32,
    pub channels:    u8,
    /// Tags of the track decoded at open (the startup track); the deck copies
    /// these and replaces them on each browser LOAD.
    pub tags:        TrackTags,
    _stream:     StreamOwner,
    /// Tells the audio-dev supervisor (Linux) to stop when the deck is dropped.
    _stream_shutdown: Arc<AtomicBool>,
    _processor:  thread::JoinHandle<()>,
}

impl AudioHandle {
    /// Start or stop playback.  Starting also wakes the processor thread,
    /// which parks while paused instead of polling — every PLAY path must
    /// come through here or the first block waits for the park timeout.
    pub fn set_playing(&self, on: bool) {
        if on { self.end_scrub(); }
        self.playing.store(on, Ordering::Relaxed);
        if on { self.proc_thread.unpark(); }
    }

    /// Paused jog: let the listener hear the track under the moving
    /// playhead. The processor reads from where the scrub began toward `to`
    /// (interleaved samples), its pitch following the wheel's speed as a
    /// record's does, and stops there once the wheel has been still for a
    /// moment. `position` moves to `to` at once for the UI. `from` is where
    /// the playhead was before this move (used when a scrub starts).
    pub fn scrub_to(&self, from: u64, to: u64) {
        if self.scrub.target.load(Ordering::Acquire) == NO_SEEK {
            self.scrub.from.store(from, Ordering::Relaxed);
        }
        self.scrub.target.store(to, Ordering::Release);
        self.scrub.seq.fetch_add(1, Ordering::Relaxed);
        self.position.store(to, Ordering::Relaxed);
        self.in_flight.store(0, Ordering::Relaxed);
        self.proc_thread.unpark();
    }

    /// Hand a scrub over to the transport: PLAY (or a seek) starts from
    /// where the scrub was heading.
    fn end_scrub(&self) {
        let t = self.scrub.target.swap(NO_SEEK, Ordering::AcqRel);
        if t != NO_SEEK { self.seek_request.store(t, Ordering::Release); }
    }

    /// Move the playhead.  The processor acts on `seek_request`; `position`
    /// and `in_flight` are set here too so the UI reads the sought spot at
    /// once.  The processor is woken so a seek lands while PAUSED as well:
    /// it used to sit pending until the next PLAY, and the block-end
    /// progress store could meanwhile overwrite the hint with the old
    /// cursor — which is what made a released CUE sometimes stick on screen
    /// where the preview stopped instead of snapping back.
    pub fn seek(&self, pos: u64) {
        self.scrub.target.store(NO_SEEK, Ordering::Release);
        self.seek_request.store(pos, Ordering::Release);
        self.position.store(pos, Ordering::Relaxed);
        self.in_flight.store(0, Ordering::Relaxed);
        self.proc_thread.unpark();
    }

    /// Convenience: read current speed.
    pub fn speed_load(&self) -> f32 {
        f32::from_bits(self.speed.load(Ordering::Relaxed))
    }

    /// Convenience: write a new speed.
    pub fn speed_store(&self, speed: f32) {
        self.speed.store(speed.to_bits(), Ordering::Relaxed);
    }

    /// Number of interleaved samples in the currently-loaded track.
    pub fn len(&self) -> usize { self.samples.load().len() }
    #[allow(dead_code)] // companion to len(); clippy expects it to exist
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// The currently-loaded sample buffer (cheap Arc clone).
    pub fn current(&self) -> Arc<Vec<f32>> { self.samples.load_full() }

    /// Swap in a freshly-decoded track and rewind to the start.  The processor
    /// picks up the new buffer on its next block; resetting `position` to 0
    /// trips its existing seek path (stretcher reset + ring drain).  Playback
    /// is left paused — the caller resumes, as a CDJ does after LOAD.
    ///
    /// The new track must match the running device configuration: same sample
    /// rate and channel count.  Resampling is WORKSTREAMS A1; until then a
    /// mismatch is refused rather than played at the wrong pitch or corrupted.
    pub fn load_samples(&self, new: Arc<Vec<f32>>, sample_rate: u32, channels: u8) -> Result<()> {
        if sample_rate != self.sample_rate || channels != self.channels {
            anyhow::bail!(
                "track is {}Hz/{}ch but the deck is running {}Hz/{}ch —                  resampling/channel-convert not yet implemented (A1)",
                sample_rate, channels, self.sample_rate, self.channels,
            );
        }
        self.playing.store(false, Ordering::Relaxed);
        self.stems.reset();
        self.samples.store(new);
        self.in_flight.store(0, Ordering::Relaxed);
        self.position.store(0, Ordering::Relaxed);
        self.seek_request.store(0, Ordering::Relaxed);
        Ok(())
    }
}

// ── Constructor ───────────────────────────────────────────────────────────────

impl AudioHandle {
    /// Open a deck with a decoded track.
    pub fn open(path: &Path) -> Result<Self> {
        let (decoded, file_sr, file_ch, tags) = decode_file(path)?;
        Self::open_inner(Some((decoded, file_sr, file_ch as usize)), tags)
    }

    /// Open an empty deck: device up, no track, paused.  The deck rate/channels
    /// follow the output device; a later LOAD resamples the track to them in
    /// `finish_load`.  Lets freedj boot to a browse-and-load state like a CDJ.
    pub fn open_empty() -> Result<Self> {
        Self::open_inner(None, TrackTags::default())
    }

    fn open_inner(initial: Option<(Vec<f32>, u32, usize)>, tags: TrackTags) -> Result<Self> {
        // ── 2. Open cpal device ────────────────────────────────────────────────
        let host   = cpal::default_host();
        let device = host
            .default_output_device()
            .context("no output audio device found")?;
        let supported = device
            .default_output_config()
            .context("failed to get default output config")?;

        let device_sr = supported.sample_rate().0;
        let device_ch = supported.channels() as usize;

        // ── 1. Initial track, or an empty deck.  Empty takes the device rate/ch
        //       and starts paused; the processor idles until a track loads. ─────
        let (mut decoded, mut file_sr, file_ch, playing_init) = match initial {
            Some((d, sr, ch)) => (d, sr, ch, true),
            None              => (Vec::new(), device_sr, device_ch, false),
        };
        // The deck runs at the device's rate: there is no real-time SRC, so a
        // start-up track at another rate is converted here, as every later
        // LOAD is (`finish_load`). Keeping the file's rate made the whole
        // session play off-speed (a 48 kHz track on a 44.1 kHz device ran
        // 8 % slow) and broke SYNC, whose beat clock assumes the deck rate.
        if !decoded.is_empty() && device_sr != file_sr {
            let t0 = std::time::Instant::now();
            decoded = resample_interleaved(&decoded, file_ch, file_sr, device_sr)?;
            log::info!("resampled start-up track {file_sr} → {device_sr} Hz in {:.2}s", t0.elapsed().as_secs_f32());
            file_sr = device_sr;
        }
        let samples = Arc::new(ArcSwap::from_pointee(decoded));

        let stream_config = cpal::StreamConfig {
            channels:    device_ch as u16,
            sample_rate: supported.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };

        // ── 3. Shared state ────────────────────────────────────────────────────
        let position   = Arc::new(AtomicU64::new(0));
        let playing    = Arc::new(AtomicBool::new(playing_init));
        let speed      = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let key_lock   = Arc::new(AtomicBool::new(true));  // Master Tempo on by default
        let loop_start  = Arc::new(AtomicU64::new(0));
        let loop_end    = Arc::new(AtomicU64::new(0));
        let loop_active = Arc::new(AtomicBool::new(false));
        let source_consumed = Arc::new(AtomicU64::new(0));
        let drain_flag = Arc::new(AtomicBool::new(false));
        let in_flight  = Arc::new(AtomicU64::new(0));
        let seek_request = Arc::new(AtomicU64::new(NO_SEEK));
        let scrub = ScrubShared {
            target: Arc::new(AtomicU64::new(NO_SEEK)), from: Arc::new(AtomicU64::new(0)),
            seq: Arc::new(AtomicU64::new(0)), live: Arc::new(AtomicBool::new(false)),
        };

        // ── 4. Ring buffer ─────────────────────────────────────────────────────
        let ring_len = ring_samples(device_sr, device_ch);
        if ring_len != RING_BUFFER_SAMPLES { log::info!("audio ring: {} samples ({:.0} ms)", ring_len, ring_len as f64 / device_ch as f64 / device_sr as f64 * 1000.0); }
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(ring_len);
        let stats = Arc::new(AudioStats::default());

        // ── 5. Processor thread ────────────────────────────────────────────────
        let proc_samples    = Arc::clone(&samples);
        let proc_position   = Arc::clone(&position);
        let proc_playing    = Arc::clone(&playing);
        let proc_speed      = Arc::clone(&speed);
        let proc_key_lock   = Arc::clone(&key_lock);
        let proc_loop = LoopShared {
            start:  Arc::clone(&loop_start),
            end:    Arc::clone(&loop_end),
            active: Arc::clone(&loop_active),
            consumed: Arc::clone(&source_consumed),
        };
        let proc_drain_flag = Arc::clone(&drain_flag);
        let proc_in_flight  = Arc::clone(&in_flight);
        let proc_seek       = Arc::clone(&seek_request);
        let proc_stats      = Arc::clone(&stats);
        let proc_scrub      = scrub.clone();
        let stems           = Arc::new(StemControl::default());
        let proc_stems      = Arc::clone(&stems);

        let processor = thread::Builder::new()
            .name("audio-proc".into())
            .spawn(move || {
                processor_loop(
                    proc_samples,
                    proc_position,
                    proc_playing,
                    proc_speed,
                    proc_key_lock,
                    proc_loop,
                    proc_drain_flag,
                    proc_in_flight,
                    proc_seek,
                    proc_scrub,
                    proc_stems,
                    file_sr,
                    file_ch,
                    device_ch,
                    producer,
                    ring_len,
                    proc_stats,
                );
            })
            .context("failed to spawn processor thread")?;
        let proc_thread = processor.thread().clone();

        // ── 6. cpal stream (RT callback, no allocation) ────────────────────────
        // The device stream is owned by a supervisor thread on Linux so it can
        // be rebuilt when ALSA gives up: after an xrun on a direct hw device
        // (a loaded single-core Pi, an HDMI sink) cpal reports POLLERR from
        // every poll and never recovers, leaving the deck silent.  The ring
        // consumer lives behind a lock only the live callback ever takes
        // (uncontended, try_lock, never blocks the RT path) so the next
        // stream's callback can pick it up where the dead one left off.
        let consumer = Arc::new(Mutex::new(consumer));
        let failed   = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let parts = StreamParts {
            config: stream_config, device_ch, device_sr,
            consumer: Arc::clone(&consumer), playing: Arc::clone(&playing), stats: Arc::clone(&stats),
            drain_flag: Arc::clone(&drain_flag), failed: Arc::clone(&failed),
            scrub_live: Arc::clone(&scrub.live),
        };
        #[cfg(target_os = "linux")]
        let stream = {
            let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
            let sd = Arc::clone(&shutdown);
            let supervisor = thread::Builder::new().name("audio-dev".into()).spawn(move || {
                let mut stream = match build_stream(&device, &parts) {
                    Ok(st) => { let _ = tx.send(Ok(())); st }
                    Err(e) => { let _ = tx.send(Err(e)); return; }
                };
                while !sd.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(100));
                    if !parts.failed.load(Ordering::Relaxed) { continue; }
                    log::warn!("audio stream failed — rebuilding the device stream");
                    drop(stream);
                    let mut wait = Duration::from_millis(100);
                    stream = loop {
                        match build_stream(&device, &parts) {
                            Ok(st) => break st,
                            Err(e) => {
                                log::warn!("audio stream rebuild failed ({e:#}); retrying in {} ms", wait.as_millis());
                                thread::sleep(wait);
                                wait = (wait * 2).min(Duration::from_secs(2));
                                if sd.load(Ordering::Relaxed) { return; }
                            }
                        }
                    };
                    parts.failed.store(false, Ordering::Relaxed);
                    log::info!("audio stream rebuilt");
                }
            }).context("failed to spawn audio-dev thread")?;
            rx.recv().context("audio-dev thread died")??;
            StreamOwner::Thread(Some(supervisor))
        };
        #[cfg(not(target_os = "linux"))]
        let stream = StreamOwner::Inline(build_stream(&device, &parts)?);

        log::info!("audio playback started ({} key-lock pipeline)", opendeck_timestretch::Engine::from_env().name());

        Ok(Self {
            samples,
            position,
            playing,
            proc_thread,
            speed,
            key_lock,
            loop_start,
            loop_end,
            loop_active,
            source_consumed,
            in_flight,
            seek_request,
            scrub,
            stems,
            stats,
            sample_rate: file_sr,
            channels: file_ch as u8,
            tags,
            _stream: stream,
            _stream_shutdown: shutdown,
            _processor: processor,
        })
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) { self._stream_shutdown.store(true, Ordering::Relaxed); }
}

/// Everything a device stream's callback needs, so one can be built again
/// after the previous one died.
struct StreamParts {
    config:     cpal::StreamConfig,
    device_ch:  usize,
    device_sr:  u32,
    consumer:   Arc<Mutex<rtrb::Consumer<f32>>>,
    playing:    Arc<AtomicBool>,
    stats:      Arc<AudioStats>,
    drain_flag: Arc<AtomicBool>,
    failed:     Arc<AtomicBool>,
    /// A paused jog scrub is feeding the ring: play it, as if playing.
    scrub_live: Arc<AtomicBool>,
}

/// Who holds the cpal stream: the supervisor thread (Linux) or the handle.
enum StreamOwner {
    #[allow(dead_code)]
    Thread(Option<thread::JoinHandle<()>>),
    #[allow(dead_code)]
    Inline(cpal::Stream),
}

fn build_stream(device: &cpal::Device, p: &StreamParts) -> Result<cpal::Stream> {
    let cpal_playing = Arc::clone(&p.playing);
    let cpal_stats   = Arc::clone(&p.stats);
    let consumer     = Arc::clone(&p.consumer);
    let drain_flag   = Arc::clone(&p.drain_flag);
    let failed       = Arc::clone(&p.failed);
    let scrub_live   = Arc::clone(&p.scrub_live);
    let device_ch    = p.device_ch;
    // Stop fade: cutting to silence mid-waveform is a step, and a step is
    // the click heard on every CUE release / pause.  On stop the callback
    // keeps playing what the ring already holds while a gain ramps to
    // zero over STOP_FADE_S (below anything audible as a fade), THEN goes
    // silent and honours any pending flush.  Start is untouched — no
    // fade-in — so cue latency stays exactly as it is.
    const STOP_FADE_S: f32 = 0.003;
    let fade_step = 1.0 / (p.device_sr as f32 * STOP_FADE_S).max(1.0);   // per frame
    let mut gain: f32 = 1.0;
    let mut rt_asked = false;
    let stream = device
        .build_output_stream::<f32, _, _>(
            &p.config,
            move |out: &mut [f32], _info| {
                // The device thread is cpal's; ask for RT priority from inside
                // it, once (a syscall, no allocation).
                if !rt_asked { rt_asked = true; try_realtime("cpal output", 30); }
                // Only ever contended for the instant a replacement stream
                // takes over; silence is the right answer then.
                let Ok(mut consumer) = consumer.try_lock() else { out.fill(0.0); return; };
                if !cpal_playing.load(Ordering::Relaxed) && !scrub_live.load(Ordering::Relaxed) {
                    if gain > 0.0 {
                        // Fading out: ring audio × a falling gain, frame by
                        // frame.  A flush requested meanwhile waits until
                        // the fade is done (the ring holds ~90 ms, plenty).
                        for frame in out.chunks_mut(device_ch) {
                            let g = gain * gain;   // squared ramp: lands with zero slope
                            for sample in frame.iter_mut() {
                                *sample = consumer.pop().unwrap_or(0.0) * g;
                            }
                            gain = (gain - fade_step).max(0.0);
                            if gain == 0.0 { break; }
                        }
                        if gain > 0.0 { return; }
                        // Fell to zero inside this buffer: the rest is
                        // silence and the flush can proceed.
                    }
                    if drain_flag.swap(false, Ordering::AcqRel) {
                        while consumer.pop().is_ok() {}
                    }
                    out.fill(0.0);
                    return;
                }
                gain = 1.0;   // playing: no fade-in, and a restart mid-fade snaps up

                // On seek, flush stale buffered audio.  This same buffer will
                // then fill with silence until the producer re-primes from the
                // new position — expected, so attribute it to seek priming,
                // not to starvation.
                let draining = drain_flag.swap(false, Ordering::AcqRel);
                if draining {
                    while consumer.pop().is_ok() {}
                }

                // Fill from the ring; count any sample we had to invent as
                // silence.  No allocation, and the atomics are touched only
                // when silence actually happens, so the clean path stays a
                // bare pop().
                let mut underran = 0u64;
                for sample in out.iter_mut() {
                    match consumer.pop() {
                        Ok(s)  => *sample = s,
                        Err(_) => { *sample = 0.0; underran += 1; }
                    }
                }
                if underran > 0 {
                    if draining {
                        // Expected re-prime gap right after a seek/cue flush.
                        cpal_stats.seek_priming_samples.fetch_add(underran, Ordering::Relaxed);
                    } else {
                        // Genuine starvation — the core failed to keep up.
                        cpal_stats.underrun_samples.fetch_add(underran, Ordering::Relaxed);
                        cpal_stats.underrun_events.fetch_add(1, Ordering::Relaxed);
                    }
                }
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

// ── Processor loop ────────────────────────────────────────────────────────────

/// The next source block from `pos`, `want` samples long, and the cursor after
/// it.  With a loop `[ls, le)` active, a block that would run past `le` is
/// assembled in `scratch` as the loop's tail then its head — one continuous
/// stream across the wrap, so the stretcher never sees a discontinuity
/// (seamless, no reset, no click).  A cursor at/after `le` (a loop set behind
/// the playhead) wraps straight to `ls`.  Without a loop, or past the loop's
/// reach, it's a plain slice clamped to the end of the track.
fn read_block<'a>(
    samples: &'a [f32], pos: usize, want: usize,
    lp: Option<(usize, usize)>, scratch: &'a mut Vec<f32>,
) -> (&'a [f32], usize) {
    let (r, end) = block_ranges(samples.len(), pos, want, lp);
    if r[1].is_empty() { return (&samples[r[0].clone()], end); }
    scratch.clear();
    scratch.extend_from_slice(&samples[r[0].clone()]);
    scratch.extend_from_slice(&samples[r[1].clone()]);
    (scratch.as_slice(), end)
}

/// Where [`read_block`]'s samples come from: one range, or the loop's
/// tail then its head (the second range empty when there is no wrap), and
/// the cursor after.  The stem mix reads the same ranges.
fn block_ranges(len: usize, pos: usize, want: usize, lp: Option<(usize, usize)>) -> ([std::ops::Range<usize>; 2], usize) {
    if let Some((ls, le)) = lp {
        let pos = if pos >= le { ls } else { pos };
        if pos + want > le {
            let head = (want - (le - pos)).min(le - ls);
            return ([pos..le, ls..ls + head], ls + head);
        }
        return ([pos..pos + want, 0..0], pos + want);
    }
    let end = (pos + want).min(len);
    ([pos.min(end)..end, 0..0], end)
}

#[cfg(test)]
mod loop_read_tests {
    use super::{read_block, resample_interleaved};

    fn ramp(n: usize) -> Vec<f32> { (0..n).map(|i| i as f32).collect() }

    /// The start-up track and every LOAD are converted to the device rate:
    /// a 1 kHz tone at 48 kHz must come out at 44.1 kHz the right length and
    /// still 1 kHz (the bug this fixed played it 8 % slow and flat).
    #[test]
    fn resampling_keeps_length_and_pitch() {
        let (src, dst, secs) = (48_000u32, 44_100u32, 1.0f64);
        let n = (src as f64 * secs) as usize;
        let mut stereo = Vec::with_capacity(n * 2);
        for i in 0..n {
            let v = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / src as f64).sin() as f32 * 0.5;
            stereo.extend_from_slice(&[v, -v]);
        }
        let out = resample_interleaved(&stereo, 2, src, dst).unwrap();
        let frames = out.len() / 2;
        assert!((frames as f64 - dst as f64 * secs).abs() < dst as f64 * 0.01, "{frames} frames");
        // Count rising zero crossings of the left channel away from the edges.
        let left: Vec<f32> = out.chunks(2).map(|c| c[0]).collect();
        let mid = &left[dst as usize / 10..dst as usize * 9 / 10];
        let rising = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let hz = rising as f64 / (mid.len() as f64 / dst as f64);
        assert!((hz - 1000.0).abs() < 5.0, "came out at {hz:.1} Hz");
        // Channels stay channels.
        assert!(out.chunks(2).skip(1000).take(100).all(|c| (c[0] + c[1]).abs() < 1e-3));
        // Same rate: untouched.
        assert_eq!(resample_interleaved(&stereo[..20], 2, src, src).unwrap(), stereo[..20].to_vec());
        // Timing: a click at 0.5 s is still at 0.5 s (grids and cues are in
        // time, so a filter delay would put every beat early).
        let mut click = vec![0.0f32; n * 2];
        click[(n / 2) * 2] = 1.0;
        click[(n / 2) * 2 + 1] = 1.0;
        let out = resample_interleaved(&click, 2, src, dst).unwrap();
        assert_eq!(out.len() / 2, (n as f64 * dst as f64 / src as f64).round() as usize, "exact length");
        let peak = out.chunks(2).enumerate().max_by(|a, b| a.1[0].total_cmp(&b.1[0])).unwrap().0;
        let want = (0.5 * dst as f64).round() as i64;
        assert!((peak as i64 - want).abs() <= 1, "click moved to frame {peak}, want {want}");
    }

    #[test]
    fn plain_read_without_a_loop_clamps_to_track_end() {
        let s = ramp(1000);
        let mut sc = Vec::new();
        let (b, end) = read_block(&s, 900, 200, None, &mut sc);
        assert_eq!(b.len(), 100);
        assert_eq!(end, 1000);
        assert_eq!(b[0], 900.0);
    }

    #[test]
    fn block_inside_the_loop_is_a_plain_slice() {
        let s = ramp(1000);
        let mut sc = Vec::new();
        let (b, end) = read_block(&s, 120, 50, Some((100, 300)), &mut sc);
        assert_eq!(end, 170);
        assert_eq!((b[0], b[49]), (120.0, 169.0));
    }

    #[test]
    fn block_crossing_the_end_wraps_tail_then_head_continuously() {
        // Loop [100, 300), cursor at 280, want 50 → 20 of tail (280..300) then
        // 30 of head (100..130); cursor ends at 130.  No gap, no repeat.
        let s = ramp(1000);
        let mut sc = Vec::new();
        let (b, end) = read_block(&s, 280, 50, Some((100, 300)), &mut sc);
        assert_eq!(b.len(), 50);
        assert_eq!(end, 130);
        assert_eq!(&b[..20], &ramp(1000)[280..300]);
        assert_eq!(&b[20..], &ramp(1000)[100..130]);
    }

    #[test]
    fn cursor_at_or_past_the_end_wraps_to_start_first() {
        let s = ramp(1000);
        let mut sc = Vec::new();
        let (b, end) = read_block(&s, 300, 50, Some((100, 300)), &mut sc);
        assert_eq!((b[0], end), (100.0, 150));
        let (b, end) = read_block(&s, 750, 50, Some((100, 300)), &mut sc);
        assert_eq!((b[0], end), (100.0, 150));
    }

    #[test]
    fn scrub_reaches_the_wheel_and_stops_there() {
        // A mono ramp: the sample value is its frame number, so the output
        // shows where the cursor read.
        use super::scrub_block;
        let s: Vec<f32> = (0..48_000).map(|i| i as f32).collect();
        let mut out: Vec<f32> = Vec::new();
        let (mut cur, mut vel): (f64, f64) = (1000.0, 0.0);
        // The wheel jumps 0.1 s ahead: the cursor heads there, forward only.
        for _ in 0..200 {
            let before = out.len();
            (cur, vel) = scrub_block(&s, 1, cur, vel, 5800.0, 48_000, 128, &mut out);
            assert!(out[before..].windows(2).all(|w| w[1] >= w[0] - 1e-3), "it never runs backwards on the way");
        }
        assert!((cur - 5800.0).abs() < 2.0, "arrived: {cur}");
        assert!(vel.abs() < 0.05, "and stopped: {vel}");
        assert_eq!(out.len(), 200 * 128);
        // Heard audio, not a jump: the first block starts where the scrub began.
        assert!((out[0] - 1000.0).abs() < 1.0);
        // Never faster than 8× however far the wheel goes.
        let mut out: Vec<f32> = Vec::new();
        let (c, v) = scrub_block(&s, 1, 0.0, 0.0, 40_000.0, 48_000, 1024, &mut out);
        assert!(v <= 8.0 + 1e-9 && c <= 8.0 * 1024.0);
        // Stereo stays interleaved; reading clamps at the ends.
        let st: Vec<f32> = (0..200).flat_map(|i| [i as f32, -(i as f32)]).collect();
        let mut out: Vec<f32> = Vec::new();
        let (c, _) = scrub_block(&st, 2, 10.0, 0.0, -50.0, 48_000, 64, &mut out);
        assert!(c >= 0.0);
        assert!(out.chunks(2).all(|f| (f[0] + f[1]).abs() < 1e-3));
    }

    #[test]
    fn a_loop_shorter_than_a_block_never_overreads_the_head() {
        // Loop [100, 120) (20 samples), want 50 from 110: tail 10, head capped
        // at the loop length (20) — never more than one loop of head.
        let s = ramp(1000);
        let mut sc = Vec::new();
        let (b, end) = read_block(&s, 110, 50, Some((100, 120)), &mut sc);
        assert_eq!(b.len(), 30);
        assert_eq!(end, 120);
        assert_eq!(&b[..10], &ramp(1000)[110..120]);
        assert_eq!(&b[10..], &ramp(1000)[100..120]);
    }
}

/// Paused jog scrub, shared by the handle and the processor.
#[derive(Clone)]
struct ScrubShared {
    /// Where the wheel has put the playhead (interleaved samples), or NO_SEEK.
    target: Arc<AtomicU64>,
    /// Where the playhead was when this scrub began.
    from:   Arc<AtomicU64>,
    /// Bumped on every wheel movement.
    seq:    Arc<AtomicU64>,
    /// The ring holds scrub audio: the callback plays while paused.
    live:   Arc<AtomicBool>,
}

/// Frames rendered per scrub step, and how far ahead of the device the scrub
/// stays: little, so the sound follows the hand, but more than a device
/// period (PipeWire pulls ~1024 frames at a time; less starved the scrub).
const SCRUB_BLOCK: usize = 128;
const SCRUB_AHEAD_S: f64 = 0.050;
/// How long the wheel must be still before the scrub lets go.
const SCRUB_IDLE: Duration = Duration::from_millis(150);

/// Render `n` frames of scrub audio from `samples` (`ch` interleaved),
/// moving the read cursor `cur` (frames) toward `goal`, so a flick of the
/// wheel plays as a short burst that slows into place, at most 8× either
/// way. Linear
/// interpolation between frames. Returns the new cursor and speed.
#[allow(clippy::too_many_arguments, dead_code)]
fn scrub_block(samples: &[f32], ch: usize, cur: f64, vel: f64, goal: f64, sample_rate: u32,
               n: usize, out: &mut Vec<f32>) -> (f64, f64) {
    scrub_block_from(&|i| samples[i], samples.len(), ch, cur, vel, goal, sample_rate, n, out)
}

/// [`scrub_block`] reading through `src` (the stem mix while stems play).
#[allow(clippy::too_many_arguments)]
fn scrub_block_from(src: &dyn Fn(usize) -> f32, len: usize, ch: usize, mut cur: f64, mut vel: f64, goal: f64,
                    sample_rate: u32, n: usize, out: &mut Vec<f32>) -> (f64, f64) {
    let frames = len / ch.max(1);
    if frames < 2 { return (cur, 0.0); }
    let last = (frames - 1) as f64;
    // First order toward the goal (the gap closes with a 40 ms time
    // constant, so it cannot overshoot and hover), with the speed allowed to
    // change by at most full speed in 10 ms, so a flick ramps rather than
    // clicks.
    let lag = sample_rate as f64 * 0.040;
    let accel = 8.0 / (sample_rate as f64 * 0.010);
    for _ in 0..n {
        let want = ((goal - cur) / lag).clamp(-8.0, 8.0);
        vel += (want - vel).clamp(-accel, accel);
        cur = (cur + vel).clamp(0.0, last);
        let i = cur.floor() as usize;
        let j = (i + 1).min(frames - 1);
        let t = (cur - i as f64) as f32;
        for c in 0..ch {
            let a = src(i * ch + c);
            let b = src(j * ch + c);
            out.push(a + (b - a) * t);
        }
    }
    (cur, vel)
}

/// The processor's view of the loop state (see `AudioHandle::loop_*`).
struct LoopShared {
    start:  Arc<AtomicU64>,
    end:    Arc<AtomicU64>,
    active: Arc<AtomicBool>,
    consumed: Arc<AtomicU64>,
}

fn processor_loop(
    samples:    Arc<ArcSwap<Vec<f32>>>,
    position:   Arc<AtomicU64>,
    playing:    Arc<AtomicBool>,
    speed:      Arc<AtomicU32>,
    key_lock:   Arc<AtomicBool>,
    lp:         LoopShared,
    drain_flag: Arc<AtomicBool>,
    in_flight:  Arc<AtomicU64>,
    seek_request: Arc<AtomicU64>,
    scrub:      ScrubShared,
    stems:      Arc<StemControl>,
    sample_rate: u32,
    file_ch:    usize,
    device_ch:  usize,
    mut producer: rtrb::Producer<f32>,
    ring_len:   usize,
    stats:      Arc<AudioStats>,
) {
    try_realtime("audio-proc", 20);
    let mut stretcher = Stretcher::from_env(sample_rate, file_ch as u8);

    // ── Pre-roll: push silence to warm up the RubberBand engine ──────────────
    // The R3 engine needs latency_frames of input before it produces output.
    // Without this, the first ~100ms of playback is silent (ring buffer stays
    // empty while RubberBand fills its internal pipeline).
    {
        let latency = stretcher.latency_frames();
        let silence = vec![0.0f32; latency * file_ch];
        let mut warmup = Vec::new();
        stretcher.process(&silence, &mut warmup);
        // Then start clean, as after a seek: the first PLAY is armed the same
        // way (Rubber Band's start pad / delay), so a track's opening
        // transient comes out whole.
        stretcher.reset();
        log::debug!("proc: pre-rolled {latency} silence frames");
    }

    // Internal read cursor — the processor owns this, UI/HID may jump `position`
    // to trigger a seek.
    let mut proc_pos: u64 = 0;

    // Output buffer for timestretched (file_ch interleaved) audio.
    let mut ts_out: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * file_ch * 8);
    // Scratch for a source block that wraps the loop boundary (tail + head).
    let mut loop_buf: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * file_ch);

    // Paused jog scrub: the read cursor (frames), its speed (frames per
    // output frame), the last scrub request seen and when it came.
    let mut scrub_cur: Option<f64> = None;
    let mut scrub_vel = 0.0f64;
    let mut scrub_seen = 0u64;
    let mut scrub_moved = std::time::Instant::now();
    let mut scrub_buf: Vec<f32> = Vec::with_capacity(SCRUB_BLOCK * file_ch);

    // Stems: the ramp state and the mixed source block.
    let mut mixer = Mixer::default();
    let mut mix_buf: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * file_ch);

    loop {
        // ── Paused jog scrub ──────────────────────────────────────────────────
        // Read straight from the track toward the wheel's position (no
        // stretcher: the pitch follows the wheel, as on a record), a couple of
        // blocks ahead of the device. Ends when the wheel has been still for
        // SCRUB_IDLE and the cursor has arrived, or at once on PLAY / a seek.
        let target = scrub.target.load(Ordering::Acquire);
        if target != NO_SEEK && !playing.load(Ordering::Relaxed) && seek_request.load(Ordering::Acquire) == NO_SEEK {
            let current = samples.load_full();
            let frames = current.len() / file_ch.max(1);
            if frames > 1 {
                let cur = *scrub_cur.get_or_insert_with(|| {
                    scrub.live.store(true, Ordering::Release);
                    scrub_vel = 0.0;
                    log::debug!("proc: scrub from {}", scrub.from.load(Ordering::Relaxed));
                    (scrub.from.load(Ordering::Relaxed) / file_ch as u64) as f64
                });
                let seq = scrub.seq.load(Ordering::Relaxed);
                if seq != scrub_seen { scrub_seen = seq; scrub_moved = std::time::Instant::now(); }
                let goal = (target / file_ch as u64) as f64;
                let queued = (ring_len - producer.slots()) / device_ch;
                if queued as f64 >= SCRUB_AHEAD_S * sample_rate as f64 {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                scrub_buf.clear();
                let bank = stems.bank().filter(|b| b.is_for(&current));
                let src = |i: usize| mixer.at(&current, bank.as_deref(), i);
                let (c, v) = scrub_block_from(&src, current.len(), file_ch, cur, scrub_vel, goal, sample_rate, SCRUB_BLOCK, &mut scrub_buf);
                scrub_cur = Some(c);
                scrub_vel = v;
                for f in scrub_buf.chunks(file_ch) {
                    for d in 0..device_ch { let _ = producer.push(f[d.min(file_ch - 1)]); }
                }
                let arrived = (goal - c).abs() < 2.0 && v.abs() < 0.05;
                if log::log_enabled!(log::Level::Trace) { log::trace!("proc: scrub gap {:.1} vel {v:.4} queued {queued} seq {seq}", goal - c); }
                if !(arrived && scrub_moved.elapsed() > SCRUB_IDLE) { continue; }
                // Still and there: park on the spot, ready for PLAY.
                let _ = scrub.target.compare_exchange(target, NO_SEEK, Ordering::AcqRel, Ordering::Relaxed);
            }
        }
        if let Some(c) = scrub_cur.take() {
            // The scrub is over (arrived, PLAY, or a seek). PLAY and seeks
            // bring their own seek_request; an arrival parks the processor
            // where the wheel left the playhead.
            scrub.live.store(false, Ordering::Release);
            log::debug!("proc: scrub ends at frame {c:.0}");
            if seek_request.load(Ordering::Acquire) == NO_SEEK {
                let at = ((c.round().max(0.0)) as u64 * file_ch as u64).min(samples.load().len() as u64);
                seek_request.store(at, Ordering::Release);
            }
        }

        // ── Pause handling ────────────────────────────────────────────────────
        // Park rather than poll: `AudioHandle::set_playing(true)` unparks us,
        // so PLAY/CUE still start within a block, and a paused deck costs no
        // wakeups (this loop ran 500 times a second doing nothing, which on a
        // phone keeps the core out of its idle state).  The timeout is only a
        // safety net for a store that bypassed `set_playing`.
        if !playing.load(Ordering::Relaxed) {
            // A seek while paused is honoured now, not at the next PLAY: move
            // the cursor, reset the stretcher and flush the ring (the callback
            // swaps `drain_flag` every period even while paused), so both the
            // published position and the first block after PLAY are right.
            let req = seek_request.swap(NO_SEEK, Ordering::AcqRel);
            if req != NO_SEEK {
                log::debug!("proc: seek {proc_pos} → {req} (paused)");
                stretcher.reset();
                proc_pos = req;
                position.store(proc_pos, Ordering::Relaxed);
                in_flight.store(0, Ordering::Relaxed);
                drain_flag.store(true, Ordering::Release);
            }
            thread::park_timeout(Duration::from_millis(50));
            continue;
        }

        // Load the current track buffer once per block (lock-free).  A runtime
        // LOAD swaps this pointer; we pick up the new track on the next block,
        // and the seek path below drains the ring for the position reset.
        let current = samples.load_full();
        let samples: &[f32] = &current;

        // Empty deck (no track loaded yet): idle exactly like the paused path so
        // nothing indexes a zero-length buffer even if PLAY is pressed.
        if samples.is_empty() {
            thread::park_timeout(Duration::from_millis(50));
            continue;
        }

        // ── Seek detection ────────────────────────────────────────────────────
        let req = seek_request.swap(NO_SEEK, Ordering::AcqRel);
        if req != NO_SEEK {
            log::debug!("proc: seek {proc_pos} → {req}");
            stretcher.reset();
            proc_pos = req;
            position.store(proc_pos, Ordering::Relaxed);   // reflect immediately
            // Tell the cpal callback to flush stale buffered audio.
            drain_flag.store(true, Ordering::Release);
            // Then WAIT for it to actually flush before pushing the first
            // post-seek block.  Otherwise, when the ring is empty (e.g. a cue
            // preview from a paused deck), we push the block containing the cue
            // transient and the callback promptly drains it with the stale
            // audio — playback starts one block (~11 ms) late and the transient
            // is lost.  The callback runs every buffer period even while paused
            // (it swaps drain_flag before the play check), so this clears fast;
            // bounded so a stopped stream can't hang us.
            let mut spins = 0;
            while drain_flag.load(Ordering::Acquire) && spins < 50 {
                thread::sleep(Duration::from_millis(1));
                spins += 1;
            }
        }

        // ── Back-pressure ─────────────────────────────────────────────────────
        // At speeds below 1.0×, RubberBand outputs more frames than it takes in
        // (e.g. at 0.25× it outputs 4× input frames).  BACK_PRESSURE_SLOTS is
        // sized at 8× BLOCK_FRAMES × device_ch to guarantee we always have room
        // for the worst-case output before we process the next block.
        //
        // Sleep duration is proportional to how full the buffer is: when nearly
        // full (just above threshold) sleep ~5ms; when empty sleep ~0ms.  This
        // avoids both glitches (sleeping too long when buffer drains fast) and
        // CPU spin (sleeping too little when buffer is comfortably full).
        let free = producer.slots();
        if free < BACK_PRESSURE_SLOTS {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        // Buffer has room — sleep proportionally so we don't spin hot.
        // At 1× speed, one BLOCK_FRAMES = ~11.6ms of audio, so sleeping
        // (buffer_fill_ratio × 8ms) keeps us well ahead without wasting CPU.
        let fill_ratio = 1.0 - (free as f32 / ring_len as f32);
        let yield_ms   = (fill_ratio * 8.0) as u64;
        if yield_ms > 0 {
            thread::sleep(Duration::from_millis(yield_ms));
        }

        // ── End of track ──────────────────────────────────────────────────────
        // The source is exhausted, but the ring buffer still holds already-
        // decoded audio that the device is playing out.  Keep publishing the
        // drain distance so the UI's audible-position estimate can actually
        // reach the end (otherwise in_flight freezes ~93 ms short and the deck
        // never registers end-of-track).
        if proc_pos >= samples.len() as u64 {
            let ring_out_frames = (ring_len - producer.slots()) / device_ch;
            in_flight.store(ring_out_frames as u64 * file_ch as u64, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(5));
            continue;
        }

        // ── Read a block from the source (loop-aware) ─────────────────────────
        let want = BLOCK_FRAMES * file_ch;
        let (ls, le) = (lp.start.load(Ordering::Relaxed) as usize, lp.end.load(Ordering::Relaxed) as usize);
        let looping  = lp.active.load(Ordering::Relaxed) && le > ls && le <= samples.len();
        let (src_block, src_end) = read_block(samples, proc_pos as usize, want,
                                              looping.then_some((ls, le)), &mut loop_buf);

        // Stems: the same ranges through the stem mix, before the stretcher
        // (one stretcher whatever the stems do).  Untouched when no bank is
        // loaded for this track or every stem is at full.
        let bank = stems.bank().filter(|b| b.is_for(&current));
        let src_block: &[f32] = if bank.is_some() || mixer.active() {
            let (ranges, _) = block_ranges(samples.len(), proc_pos as usize, want, looping.then_some((ls, le)));
            if mixer.mix(samples, bank.as_deref(), stems.gains(), &ranges, &mut mix_buf) { &mix_buf } else { src_block }
        } else { src_block };

        lp.consumed.fetch_add(src_block.len() as u64, Ordering::Relaxed);

        let final_block = !looping && src_end >= samples.len();

        // Advance the shared position so the UI/renderer sees it — unless a
        // seek arrived during this block: then the UI already published the
        // new spot and this store would overwrite it with the stale cursor.
        proc_pos = src_end as u64;
        if seek_request.load(Ordering::Acquire) == NO_SEEK {
            position.store(proc_pos, Ordering::Relaxed);
        }

        // ── Update speed + key-lock mode ─────────────────────────────────────────
        // Master Tempo on (key_lock): tempo moves, pitch held (pure time-stretch).
        // Master Tempo off (varispeed): pitch tracks speed like vinyl — achieved
        // by pitch-shifting the stretcher UP by the same factor as the tempo
        // change, so time_ratio (1/spd) and pitch_scale (spd) combine to a plain
        // speed change of the source.
        let spd = f32::from_bits(speed.load(Ordering::Relaxed)).clamp(0.25, 4.0);
        stretcher.set_speed(spd);
        let semitones = if key_lock.load(Ordering::Relaxed) { 0.0 } else { 12.0 * spd.log2() };
        stretcher.set_pitch_semitones(semitones);

        // ── Timestretch ───────────────────────────────────────────────────────
        ts_out.clear();
        stretcher.process(src_block, &mut ts_out);

        if final_block && ts_out.is_empty() {
            // Flush RubberBand's tail at end of track.
            let silence = vec![0.0f32; BLOCK_FRAMES * file_ch];
            stretcher.process(&silence, &mut ts_out);
        }

        // ── Channel mix & push to ring buffer ─────────────────────────────────
        // Cap to available slots so we never silently drop frames.
        let out_frames    = ts_out.len() / file_ch;
        let slots_free    = producer.slots();
        let frames_to_push = out_frames.min(slots_free / device_ch);

        if frames_to_push < out_frames {
            let dropped = (out_frames - frames_to_push) as u64;
            stats.dropped_frames.fetch_add(dropped, Ordering::Relaxed);
            log::warn!("proc: ring buffer full, dropped {} frames", dropped);
        }

        for i in 0..frames_to_push {
            for dev_ch_idx in 0..device_ch {
                let src_ch = dev_ch_idx.min(file_ch - 1);
                // producer.push() can't fail here — we checked slots_free above
                let _ = producer.push(ts_out[i * file_ch + src_ch]);
            }
        }

        // ── Publish decode-ahead distance ─────────────────────────────────────
        // `position` is where the *decoder* has read to.  What the listener
        // hears is that minus everything still queued: the ring buffer plus the
        // stretcher's internal latency.  Expressed in source samples so the UI
        // can subtract it directly.
        let ring_out_frames = (RING_BUFFER_SAMPLES - producer.slots()) / device_ch;
        let src_frames_ahead =
            (ring_out_frames as f32 * spd) as u64 + stretcher.latency_frames() as u64;
        in_flight.store(src_frames_ahead * file_ch as u64, Ordering::Relaxed);
    }
}

// ── Decoding ──────────────────────────────────────────────────────────────────

/// Decode an entire audio file to interleaved f32 PCM in memory.  Returns the
/// samples plus their sample rate, channel count, and the file's own tags
/// (title/artist/…).  Shared by `open()` (first track) and the runtime
/// browser LOAD path.
pub fn decode_file(path: &Path) -> Result<(Vec<f32>, u32, usize, TrackTags)> {
    log::info!("decoding {}", path.display());
    let mut decoder = SymphoniaDecoder::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;

    let tags     = decoder.tags().clone();
    let file_sr  = decoder.sample_rate();
    let file_ch  = decoder.channels() as usize;
    let capacity = decoder
        .total_frames()
        .map(|f| f as usize * file_ch)
        .unwrap_or(44_100 * 2 * 300);

    let mut samples: Vec<f32> = Vec::with_capacity(capacity);
    let mut buf = vec![0f32; 4096 * file_ch];
    loop {
        match decoder.decode(&mut buf)? {
            0 => break,
            frames => samples.extend_from_slice(&buf[..frames * file_ch]),
        }
    }
    let file_sr = true_rate(&decoder, file_sr);
    log::info!(
        "decoded {} frames ({:.1}s) at {}Hz {}ch",
        samples.len() / file_ch,
        samples.len() as f64 / file_ch as f64 / file_sr as f64,
        file_sr, file_ch,
    );
    Ok((samples, file_sr, file_ch, tags))
}

/// The rate of the decoded audio itself. The container's rate is what the
/// decoder reports before decoding (44.1 kHz when the header has none); if
/// the packets disagree, they win, or the track plays at the wrong speed
/// (a 48 kHz track taken for 44.1 kHz runs 8 % slow).
fn true_rate(decoder: &SymphoniaDecoder, header_sr: u32) -> u32 {
    match decoder.decoded_rate() {
        Some(r) if r != header_sr && r > 0 => {
            log::warn!("container says {header_sr} Hz but the audio decodes at {r} Hz; using {r}");
            r
        }
        _ => header_sr,
    }
}

/// Decode an audio file from an in-memory buffer (e.g. read over NFS from a
/// linked player). `ext` primes the format probe.  Same output as decode_file.
pub fn decode_bytes(bytes: Vec<u8>, ext: Option<&str>) -> Result<(Vec<f32>, u32, usize, TrackTags)> {
    let mut decoder = SymphoniaDecoder::open_bytes(bytes, ext)
        .context("failed to open in-memory audio")?;
    let tags    = decoder.tags().clone();
    let file_sr = decoder.sample_rate();
    let file_ch = decoder.channels() as usize;
    let mut samples: Vec<f32> = Vec::new();
    let mut buf = vec![0f32; 4096 * file_ch];
    loop {
        match decoder.decode(&mut buf)? {
            0 => break,
            frames => samples.extend_from_slice(&buf[..frames * file_ch]),
        }
    }
    let file_sr = true_rate(&decoder, file_sr);
    log::info!("decoded {} frames ({:.1}s) at {}Hz {}ch (from memory)",
        samples.len() / file_ch, samples.len() as f64 / file_ch as f64 / file_sr as f64, file_sr, file_ch);
    Ok((samples, file_sr, file_ch, tags))
}

/// Offline sample-rate conversion of an interleaved buffer, used at LOAD time so
/// a track recorded at a different rate than the deck's pipeline plays at the
/// right pitch.  This is a one-time cost per load, not the real-time SRC that a
/// 48 kHz *output device* would need (WORKSTREAMS A1) — that is still open.
pub fn resample_interleaved(
    samples:  &[f32],
    channels: usize,
    src_sr:   u32,
    dst_sr:   u32,
) -> Result<Vec<f32>> {
    use rubato::{Resampler, SincFixedIn, SincInterpolationParameters,
                 SincInterpolationType, WindowFunction};
    if src_sr == dst_sr || samples.is_empty() {
        return Ok(samples.to_vec());
    }
    let ratio  = dst_sr as f64 / src_sr as f64;
    let frames = samples.len() / channels;
    let chunk  = 16_384;

    let params = SincInterpolationParameters {
        sinc_len:            256,
        f_cutoff:            0.95,
        oversampling_factor: 256,
        interpolation:       SincInterpolationType::Linear,
        window:              WindowFunction::BlackmanHarris2,
    };
    let mut rs = SincFixedIn::<f32>::new(ratio, 1.0, params, chunk, channels)
        .context("failed to create resampler")?;

    // De-interleave into per-channel planes.
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); channels];
    for f in 0..frames {
        for c in 0..channels {
            planar[c].push(samples[f * channels + c]);
        }
    }

    let mut out_planar: Vec<Vec<f32>> = vec![Vec::new(); channels];
    let mut pos = 0;
    let mut inbuf: Vec<Vec<f32>> = vec![vec![0.0f32; chunk]; channels];
    while pos + chunk <= frames {
        for c in 0..channels {
            inbuf[c].copy_from_slice(&planar[c][pos..pos + chunk]);
        }
        let out = rs.process(&inbuf, None).context("resample chunk failed")?;
        for c in 0..channels { out_planar[c].extend_from_slice(&out[c]); }
        pos += chunk;
    }
    if pos < frames {
        let rem: Vec<Vec<f32>> = (0..channels).map(|c| planar[c][pos..].to_vec()).collect();
        let out = rs.process_partial(Some(&rem), None).context("resample tail failed")?;
        for c in 0..channels { out_planar[c].extend_from_slice(&out[c]); }
    }
    // The last partial chunk comes back padded to a whole chunk (~20 ms of
    // extra silence on a 48 → 44.1 kHz track): cut to the exact length,
    // flushing first if the output is short. The start needs no trim; the
    // click test pins that the timing is already right.
    let want = (frames as f64 * ratio).round() as usize;
    while out_planar[0].len() < want {
        let out = rs.process_partial::<Vec<f32>>(None, None).context("resample flush failed")?;
        if out[0].is_empty() { break; }
        for c in 0..channels { out_planar[c].extend_from_slice(&out[c]); }
    }

    // Re-interleave.
    let avail = out_planar.iter().map(|c| c.len()).min().unwrap_or(0);
    let out_frames = want.min(avail);
    let mut out = Vec::with_capacity(out_frames * channels);
    for f in 0..out_frames {
        for c in 0..channels { out.push(out_planar[c][f]); }
    }
    log::info!("resampled {}Hz → {}Hz ({} → {} frames)", src_sr, dst_sr, frames, out_frames);
    Ok(out)
}
