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
        atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Frames fed to the timestretch engine per iteration.
const BLOCK_FRAMES: usize = 512;
/// A quantized SLIP REV's landing crossfade (into forward play, at the grid).
const LAND_XFADE_S: f32 = 0.005;

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

/// Free ring slots needed before the next block, at `speed`: room for twice
/// what one block puts out at that speed (the stretcher's output comes in
/// uneven chunks), up to the 0.25x worst case above.  At 1x that is 2 048
/// samples, so the ring fills to about 3/4: enough for an ALSA device whose
/// callback, woken late, asks for two periods at once (a UCA202 on a Pi:
/// 2 x 1 103 frames = 4 412 samples).  The old fixed 4 096 kept the ring
/// only half full, and every such request ran ~232 samples short (an
/// underrun every 200 ms on the Pi appliance).
fn back_pressure_slots(speed: f32, device_ch: usize) -> usize {
    let per_block = (BLOCK_FRAMES as f32 / speed.clamp(0.25, 4.0)).ceil() as usize * device_ch;
    (per_block * 2).min(BACK_PRESSURE_SLOTS)
}

/// If position jumps by more than this many source samples the processor
/// treats it as a seek and resets the timestretch engine.
/// Sentinel for `seek_request`: no seek pending.
const NO_SEEK: u64 = u64::MAX;
pub(crate) const NO_SEEK_PUB: u64 = NO_SEEK;
/// `JumpShared::request` when no jump is pending.
const NO_JUMP: i64 = i64::MIN;

/// A relative jump (BEAT JUMP): UI → processor, and the callback's count of
/// what was still unplayed when it flushed for it.
///
/// An absolute seek computed in the UI is only as good as the UI's idea of
/// what is playing: the screen's smoothed playhead, read up to a frame before
/// the processor and the callback act on it.  A beat jump landed a few to
/// tens of ms off the beat that way, a different amount each time — heard as
/// a stutter.  As Mixxx does (its jump is resolved in the engine), the offset
/// goes to the audio side, which knows exactly which source frame is playing
/// where the jump's crossfade starts.
#[derive(Clone)]
struct JumpShared {
    /// Offset in interleaved source samples, or NO_JUMP.
    request:  Arc<AtomicI64>,
    /// Output frames that were still in the ring after the buffer that
    /// flushed it: the jump's crossfade starts on the first of them.
    unplayed: Arc<AtomicU64>,
    /// Reverse play (the direction lever).  A change is resolved as a jump
    /// of nothing: from the frame being heard, crossfaded, the other way.
    reverse:  Arc<AtomicBool>,
    /// SLIP REV with QUANTIZE: the turn waits for the grid, `anchor` +
    /// k·`step` (source frames, f64 bits; a step of 0 turns at once), and
    /// turning back with `land` it lands where forward play would be.
    q_anchor: Arc<AtomicU64>,
    q_step:   Arc<AtomicU64>,
    q_land:   Arc<AtomicBool>,
    /// The way the processor reads now (a quantized turn waits for its beat).
    reading_rev: Arc<AtomicBool>,
}

/// A quantized turn's spot: the grid point (`anchor` + k·`step`, frames)
/// nearest frame `pos`.
fn grid_nearest(pos: f64, anchor: f64, step: f64) -> f64 {
    (anchor + ((pos - anchor) / step).round() * step).max(0.0)
}

/// SLIP REV's landing, turning back at `at` after turning at `from`: as far
/// past `from` as the reverse went before it (the same source frames, so the
/// same time at any speed), clamped and frame-aligned.
fn mirror(from: u64, at: u64, ch: usize, len: usize) -> u64 {
    let t = (2 * from as i64 - at as i64).clamp(0, len as i64) as u64;
    t / ch as u64 * ch as u64
}

// ── Audio integrity counters ──────────────────────────────────────────────────

/// Glitch counters for the real-time path.  A performance deck must never
/// dropout silently: the RT callback fills zeros when the ring is empty, so we
/// count those samples and surface them.  All Relaxed — these are diagnostics,
/// not synchronisation, and the RT callback only touches them on a glitch.
#[derive(Default)]
pub struct AudioStats {
    /// Samples of silence the callback filled because the ring was empty while
    /// playing — audible underruns (clicks / dropouts).  A seek while playing
    /// makes none (it crossfades, see `OutputFill`), so this is a clean "the
    /// core starved" signal.
    pub underrun_samples: AtomicU64,
    /// Callbacks that hit ≥1 underrun (distinct glitch events).
    pub underrun_events:  AtomicU64,
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
    /// KEY SHIFT: semitones up (+) or down (−) on top of whatever the speed
    /// and key lock do to the pitch; the tempo is untouched.
    pub key_shift:   Arc<AtomicI32>,
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
    /// Relative jumps (BEAT JUMP), resolved on the audio side; see `jump`.
    jump:            JumpShared,
    /// Paused jog scrub: what the processor reads toward (see `scrub_to`).
    scrub:           ScrubShared,
    /// Stems: the loaded bank and the three stem levels (`stems.rs`).
    pub stems:       Arc<StemControl>,
    /// Real-time glitch counters (underruns / dropped frames).
    pub stats:       Arc<AudioStats>,
    /// DVS: the record's state and whether it has the deck (`dvs.rs`).
    pub dvs:         Arc<crate::dvs::DvsShared>,
    /// The buffer holds the whole track.  False while a streaming load is
    /// still decoding: the processor waits at the end of `samples` instead
    /// of ending the track there (see `load_streaming`).
    pub complete:    Arc<AtomicBool>,
    /// The track's expected length in samples while it streams in (from the
    /// file's header), so the needle search, remaining time and seek clamps
    /// see the whole track; 0 once complete.  See [`len`](Self::len).
    pub total_hint:  Arc<AtomicU64>,
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

    /// Jump by `delta` interleaved samples from what is playing, resolved
    /// by the audio side at the very sample the jump is heard (see
    /// `JumpShared`).  `position` is left to the processor, which publishes
    /// the landing spot as soon as it knows it.
    pub fn jump(&self, delta: i64) {
        self.scrub.target.store(NO_SEEK, Ordering::Release);
        self.jump.request.store(delta, Ordering::Release);
        self.proc_thread.unpark();
    }

    /// Play backwards (or forwards again) from the frame being heard.
    pub fn set_reverse(&self, on: bool) {
        self.jump.q_step.store(0f64.to_bits(), Ordering::Relaxed);
        self.jump.reverse.store(on, Ordering::Release);
        self.proc_thread.unpark();
    }

    /// SLIP REV with QUANTIZE: turn at the next grid point the read reaches
    /// (`anchor` + k·`step`, source frames), not at once; `land` (letting
    /// go) then lands where forward play would be.  Called back before the
    /// beat, the turn is off.  Paused, in a loop, it turns at once.
    pub fn set_reverse_on_grid(&self, on: bool, anchor: f64, step: f64, land: bool) {
        self.jump.q_anchor.store(anchor.to_bits(), Ordering::Relaxed);
        self.jump.q_step.store(step.max(0.0).to_bits(), Ordering::Relaxed);
        self.jump.q_land.store(land, Ordering::Relaxed);
        self.jump.reverse.store(on, Ordering::Release);
        self.proc_thread.unpark();
    }

    /// Playing in reverse (as asked; the audio turns within a buffer).
    pub fn reverse(&self) -> bool { self.jump.reverse.load(Ordering::Relaxed) }

    /// The way the audio reads now: as asked, but a quantized turn only
    /// once its beat comes.
    pub fn reading_reverse(&self) -> bool { self.jump.reading_rev.load(Ordering::Relaxed) }

    /// Convenience: read current speed.
    pub fn speed_load(&self) -> f32 {
        f32::from_bits(self.speed.load(Ordering::Relaxed))
    }

    /// Convenience: write a new speed.
    pub fn speed_store(&self, speed: f32) {
        self.speed.store(speed.to_bits(), Ordering::Relaxed);
    }

    /// Number of interleaved samples in the currently-loaded track.
    /// The track's length in samples: while a streaming load is still
    /// decoding, the length the file's header promises (the deck shows and
    /// seeks across the whole track); otherwise the buffer's.
    pub fn len(&self) -> usize { self.samples.load().len().max(self.total_hint.load(Ordering::Relaxed) as usize) }

    /// Samples decoded so far (the buffer's length).
    pub fn decoded_len(&self) -> usize { self.samples.load().len() }
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
        self.complete.store(true, Ordering::Release);
        self.total_hint.store(0, Ordering::Relaxed);
        self.samples.store(new);
        self.in_flight.store(0, Ordering::Relaxed);
        self.position.store(0, Ordering::Relaxed);
        self.seek_request.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// Load the start of a track that is still decoding, as a CDJ plays a
    /// track long before it has read the whole file.  `total` is the
    /// expected length in samples (0 = unknown).  [`extend_samples`]
    /// (Self::extend_samples) swaps in longer buffers as they arrive.
    pub fn load_streaming(&self, prefix: Arc<Vec<f32>>, total: u64, sample_rate: u32, channels: u8) -> Result<()> {
        self.load_samples(prefix, sample_rate, channels)?;
        self.total_hint.store(total, Ordering::Relaxed);
        self.complete.store(false, Ordering::Release);
        Ok(())
    }

    /// More of a streaming track: `longer` starts with the samples already
    /// loaded, so positions, loops and cues stay where they are; the
    /// processor picks it up on its next block.  `done` = the whole track.
    pub fn extend_samples(&self, longer: Arc<Vec<f32>>, done: bool) {
        debug_assert!(longer.len() >= self.decoded_len(), "a streaming buffer only grows");
        self.samples.store(longer);
        if done {
            self.total_hint.store(0, Ordering::Relaxed);
            self.complete.store(true, Ordering::Release);
        }
    }
}

// ── Constructor ───────────────────────────────────────────────────────────────

impl AudioHandle {
    /// Open a deck with a decoded track.
    pub fn open(path: &Path) -> Result<Self> {
        let (decoded, file_sr, file_ch, tags) = decode_file(path)?;
        Self::open_inner(Some((decoded, file_sr, file_ch as usize)), tags)
    }

    /// Open an empty deck: device up, no track, paused.  The deck rate follows
    /// the output device and the deck is stereo; a later LOAD resamples to it in
    /// `finish_load`.  Lets freedj boot to a browse-and-load state like a CDJ.
    pub fn open_empty() -> Result<Self> {
        Self::open_inner(None, TrackTags::default())
    }

    /// Open a deck whose output is pulled by a plugin host instead of a
    /// sound card: stereo at the host's rate `sr`, with `track` loaded or
    /// empty.  The host's audio callback takes the [`HostOutput`] left in
    /// `slot` and calls its `fill` once per block, exactly as the device
    /// callback would.
    pub fn open_host(sr: u32, slot: &HostSlot, track: Option<&Path>) -> Result<Self> {
        let (initial, tags) = match track {
            Some(p) => { let (d, fsr, ch, tags) = decode_file(p)?; (Some((d, fsr, ch as usize)), tags) }
            None => (None, TrackTags::default()),
        };
        Self::start(initial, tags, Output::Host { sr, slot: Arc::clone(slot) })
    }

    /// A deck for tests: the real processor thread, with a stand-in device
    /// (48 kHz stereo) that pulls the output at real time, 10 ms at a time,
    /// through the same [`OutputFill`] as the device callback.
    /// A deck for latency tests: a stand-in device at `sr` taking `frames`
    /// every `every_us` (a low-latency interface), recording what it heard.
    #[cfg(test)]
    pub fn open_test_traced(samples: Vec<f32>, sr: u32, frames: usize, every_us: u64, trace: Trace) -> Result<Self> {
        let h = Self::start(Some((samples, sr, 2)), TrackTags::default(), Output::TestPulls { sr, frames, every_us, trace: Some(trace) })?;
        h.set_playing(false);
        Ok(h)
    }

    #[cfg(test)]
    pub fn open_test(samples: Vec<f32>, sr: u32) -> Result<Self> {
        let h = Self::start(Some((samples, sr, 2)), TrackTags::default(), Output::Test)?;
        h.set_playing(false);
        Ok(h)
    }

    fn open_inner(initial: Option<(Vec<f32>, u32, usize)>, tags: TrackTags) -> Result<Self> {
        // ── 2. Open cpal device ────────────────────────────────────────────────
        match default_device() {
            Ok(out) => Self::start(initial, tags, Output::Device(out.0, out.1)),
            // A deck that boots without its sound interface (a USB card not
            // plugged in, or dropped in a supply dip) still comes up: silent,
            // and on the interface as soon as it appears.  Linux only (the Pi
            // appliance); elsewhere a missing device is still an error.
            #[cfg(target_os = "linux")]
            Err(e) => {
                log::warn!("no audio output ({e:#}): starting silent, waiting for a device");
                Self::start(initial, tags, Output::Waiting)
            }
            #[cfg(not(target_os = "linux"))]
            Err(e) => Err(e),
        }
    }

    /// Everything after the device is chosen.
    fn start(initial: Option<(Vec<f32>, u32, usize)>, tags: TrackTags, output: Output) -> Result<Self> {
        let (device_sr, device_ch) = match &output {
            Output::Device(_, c) => (c.sample_rate().0, c.channels() as usize),
            Output::Test => (48_000, 2),
            Output::Host { sr, .. } => (*sr, 2),
            #[cfg(test)]
            Output::TestPulls { sr, .. } => (*sr, 2),
            // The rate a CD-sourced library plays at; a device that turns up
            // later is opened at this rate (ALSA's plug converts if it must).
            #[cfg(target_os = "linux")]
            Output::Waiting => (44_100, 2),
        };

        // ── 1. Initial track, or an empty deck.  Empty takes the device rate, in stereo
        //       and starts paused; the processor idles until a track loads. ─────
        let (mut decoded, mut file_sr, file_ch, playing_init) = match initial {
            Some((d, sr, ch)) => (d, sr, ch, true),
            None              => (Vec::new(), device_sr, 2, false),
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
            sample_rate: cpal::SampleRate(device_sr),
            buffer_size: cpal::BufferSize::Default,
        };

        // ── 3. Shared state ────────────────────────────────────────────────────
        let position   = Arc::new(AtomicU64::new(0));
        let playing    = Arc::new(AtomicBool::new(playing_init));
        let speed      = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let key_lock   = Arc::new(AtomicBool::new(true));  // Master Tempo on by default
        let key_shift  = Arc::new(AtomicI32::new(0));
        let loop_start  = Arc::new(AtomicU64::new(0));
        let loop_end    = Arc::new(AtomicU64::new(0));
        let loop_active = Arc::new(AtomicBool::new(false));
        let source_consumed = Arc::new(AtomicU64::new(0));
        let drain_flag = Arc::new(AtomicBool::new(false));
        let in_flight  = Arc::new(AtomicU64::new(0));
        let seek_request = Arc::new(AtomicU64::new(NO_SEEK));
        let jump = JumpShared { request: Arc::new(AtomicI64::new(NO_JUMP)), unplayed: Arc::new(AtomicU64::new(0)),
                               reverse: Arc::new(AtomicBool::new(false)),
                               q_anchor: Arc::new(AtomicU64::new(0)), q_step: Arc::new(AtomicU64::new(0)),
                               q_land: Arc::new(AtomicBool::new(false)), reading_rev: Arc::new(AtomicBool::new(false)) };
        let scrub = ScrubShared {
            target: Arc::new(AtomicU64::new(NO_SEEK)), from: Arc::new(AtomicU64::new(0)),
            seq: Arc::new(AtomicU64::new(0)), live: Arc::new(AtomicBool::new(false)),
        };

        // ── 4. Ring buffer ─────────────────────────────────────────────────────
        let ring_len = ring_samples(device_sr, device_ch);
        if ring_len != RING_BUFFER_SAMPLES { log::info!("audio ring: {} samples ({:.0} ms)", ring_len, ring_len as f64 / device_ch as f64 / device_sr as f64 * 1000.0); }
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(ring_len);
        let stats = Arc::new(AudioStats::default());
        let dvs = Arc::new(crate::dvs::DvsShared::default());

        // ── 5. Processor thread ────────────────────────────────────────────────
        let proc_samples    = Arc::clone(&samples);
        let proc_position   = Arc::clone(&position);
        let proc_playing    = Arc::clone(&playing);
        let proc_speed      = Arc::clone(&speed);
        let proc_key_lock   = Arc::clone(&key_lock);
        let proc_key_shift  = Arc::clone(&key_shift);
        let proc_loop = LoopShared {
            start:  Arc::clone(&loop_start),
            end:    Arc::clone(&loop_end),
            active: Arc::clone(&loop_active),
            consumed: Arc::clone(&source_consumed),
        };
        let proc_drain_flag = Arc::clone(&drain_flag);
        let proc_in_flight  = Arc::clone(&in_flight);
        let proc_seek       = Arc::clone(&seek_request);
        let proc_jump       = jump.clone();
        let proc_stats      = Arc::clone(&stats);
        let proc_scrub      = scrub.clone();
        let stems           = Arc::new(StemControl::default());
        let proc_stems      = Arc::clone(&stems);
        let complete        = Arc::new(AtomicBool::new(true));
        let proc_complete   = Arc::clone(&complete);
        let proc_dvs        = Arc::clone(&dvs);

        let processor = thread::Builder::new()
            .name("audio-proc".into())
            .spawn(move || {
                processor_loop(
                    proc_samples,
                    proc_position,
                    proc_playing,
                    proc_speed,
                    proc_key_lock,
                    proc_key_shift,
                    proc_loop,
                    proc_drain_flag,
                    proc_in_flight,
                    proc_seek,
                    proc_jump,
                    proc_scrub,
                    proc_stems,
                    proc_complete,
                    proc_dvs,
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
            scrub_live: Arc::clone(&scrub.live), unplayed: Arc::clone(&jump.unplayed),
            dvs: DvsParts {
                shared: Arc::clone(&dvs), samples: Arc::clone(&samples), position: Arc::clone(&position),
                in_flight: Arc::clone(&in_flight), seek_request: Arc::clone(&seek_request),
                loop_start: Arc::clone(&loop_start), loop_end: Arc::clone(&loop_end), loop_active: Arc::clone(&loop_active),
                file_ch, sample_rate: file_sr,
            },
        };
        let stream = match output {
            Output::Test => {
                // The tests' device: 10 ms of output every 10 ms.
                let sd = Arc::clone(&shutdown);
                let mut fill = OutputFill::new(&parts);
                let t = thread::Builder::new().name("audio-test-out".into()).spawn(move || {
                    let mut buf = vec![0f32; device_sr as usize / 100 * device_ch];
                    let t0 = std::time::Instant::now();
                    let mut n = 0u32;
                    while !sd.load(Ordering::Relaxed) {
                        fill.fill(&mut buf);
                        n += 1;
                        let due = t0 + Duration::from_millis(10) * n;
                        if let Some(w) = due.checked_duration_since(std::time::Instant::now()) { thread::sleep(w); }
                    }
                }).context("failed to spawn audio-test-out thread")?;
                StreamOwner::Thread(Some(t))
            }
            #[cfg(test)]
            Output::TestPulls { frames, every_us, trace, .. } => {
                let sd = Arc::clone(&shutdown);
                let mut fill = OutputFill::new(&parts);
                let t = thread::Builder::new().name("audio-test-pulls".into()).spawn(move || {
                    let mut buf = vec![0f32; frames * device_ch];
                    let t0 = std::time::Instant::now();
                    let mut n = 0u32;
                    while !sd.load(Ordering::Relaxed) {
                        fill.fill(&mut buf);
                        if let Some(t) = &trace {
                            let peak = buf.iter().fold(0f32, |m, v| m.max(v.abs()));
                            t.lock().unwrap().push((std::time::Instant::now(), peak));
                        }
                        n += 1;
                        let due = t0 + Duration::from_micros(every_us) * n;
                        if let Some(w) = due.checked_duration_since(std::time::Instant::now()) { thread::sleep(w); }
                    }
                }).context("failed to spawn audio-test-pulls thread")?;
                StreamOwner::Thread(Some(t))
            }
            Output::Device(device, _) => Self::open_device(device, parts, &shutdown)?,
            Output::Host { slot, .. } => {
                // The host's callback owns the pulling; nothing to run here.
                *slot.lock().unwrap() = Some(HostOutput { fill: OutputFill::new(&parts) });
                StreamOwner::Thread(None)
            }
            #[cfg(target_os = "linux")]
            Output::Waiting => {
                // No device yet: keep time at real time, silently (the deck
                // plays, its playhead moves), and look for the default device
                // every 2 s; once it opens, it is supervised like any other.
                let sd = Arc::clone(&shutdown);
                let t = thread::Builder::new().name("audio-dev".into()).spawn(move || {
                    let mut fill = OutputFill::new(&parts);
                    let mut buf = vec![0f32; device_sr as usize / 100 * device_ch];
                    let t0 = std::time::Instant::now();
                    let mut n = 0u32;
                    loop {
                        if sd.load(Ordering::Relaxed) { return; }
                        if n % 200 == 0 && n > 0 {
                            if let Some((dev, st)) = default_device().ok()
                                .and_then(|(d, _)| build_stream(&d, &parts).ok().map(|st| (d, st))) {
                                log::info!("audio output appeared ({}): playing to it",
                                           dev.name().unwrap_or_else(|_| "?".into()));
                                drop(fill);
                                supervise(dev, parts, st, &sd);
                                return;
                            }
                        }
                        fill.fill(&mut buf);
                        n += 1;
                        let due = t0 + Duration::from_millis(10) * n;
                        if let Some(w) = due.checked_duration_since(std::time::Instant::now()) { thread::sleep(w); }
                    }
                }).context("failed to spawn audio-dev thread")?;
                StreamOwner::Thread(Some(t))
            }
        };

        log::info!("audio playback started ({} key-lock pipeline)", opendeck_timestretch::Engine::from_env().name());
        Self::assemble(samples, position, playing, proc_thread, speed, key_lock, key_shift, loop_start, loop_end, loop_active,
                       source_consumed, in_flight, seek_request, jump, scrub, stems, stats, dvs, complete, file_sr, file_ch, tags,
                       stream, shutdown, processor)
    }

    /// The device stream: owned by a supervisor thread on Linux, inline elsewhere.
    fn open_device(device: cpal::Device, parts: StreamParts, shutdown: &Arc<AtomicBool>) -> Result<StreamOwner> {
        #[cfg(target_os = "linux")]
        let stream = {
            let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
            let sd = Arc::clone(&shutdown);
            let supervisor = thread::Builder::new().name("audio-dev".into()).spawn(move || {
                let stream = match build_stream(&device, &parts) {
                    Ok(st) => { let _ = tx.send(Ok(())); st }
                    Err(e) => { let _ = tx.send(Err(e)); return; }
                };
                supervise(device, parts, stream, &sd);
            }).context("failed to spawn audio-dev thread")?;
            rx.recv().context("audio-dev thread died")??;
            StreamOwner::Thread(Some(supervisor))
        };
        #[cfg(not(target_os = "linux"))]
        let stream = { let _ = shutdown; StreamOwner::Inline(build_stream(&device, &parts)?) };
        Ok(stream)
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(samples: Arc<ArcSwap<Vec<f32>>>, position: Arc<AtomicU64>, playing: Arc<AtomicBool>, proc_thread: thread::Thread,
                speed: Arc<AtomicU32>, key_lock: Arc<AtomicBool>, key_shift: Arc<AtomicI32>, loop_start: Arc<AtomicU64>, loop_end: Arc<AtomicU64>,
                loop_active: Arc<AtomicBool>, source_consumed: Arc<AtomicU64>, in_flight: Arc<AtomicU64>,
                seek_request: Arc<AtomicU64>, jump: JumpShared, scrub: ScrubShared, stems: Arc<StemControl>, stats: Arc<AudioStats>,
                dvs: Arc<crate::dvs::DvsShared>, complete: Arc<AtomicBool>, file_sr: u32, file_ch: usize, tags: TrackTags, stream: StreamOwner,
                shutdown: Arc<AtomicBool>, processor: thread::JoinHandle<()>) -> Result<Self> {
        Ok(Self {
            samples,
            position,
            playing,
            proc_thread,
            speed,
            key_lock,
            key_shift,
            loop_start,
            loop_end,
            loop_active,
            source_consumed,
            in_flight,
            seek_request,
            jump,
            scrub,
            stems,
            stats,
            dvs,
            complete,
            total_hint: Arc::new(AtomicU64::new(0)),
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
    /// Where the callback reports what a flush left unplayed (`JumpShared`).
    unplayed:   Arc<AtomicU64>,
    /// DVS: what the callback needs to play the record's way.
    dvs: DvsParts,
}

/// The deck state the output callback reads while the record drives.
#[derive(Clone)]
struct DvsParts {
    shared: Arc<crate::dvs::DvsShared>,
    samples: Arc<ArcSwap<Vec<f32>>>,
    position: Arc<AtomicU64>,
    in_flight: Arc<AtomicU64>,
    seek_request: Arc<AtomicU64>,
    loop_start: Arc<AtomicU64>,
    loop_end: Arc<AtomicU64>,
    loop_active: Arc<AtomicBool>,
    file_ch: usize,
    sample_rate: u32,
}

impl DvsParts {
    /// DVS off and nothing loaded (tests of the output path alone).
    #[cfg(test)]
    fn idle() -> DvsParts {
        DvsParts {
            shared: Default::default(), samples: Arc::new(ArcSwap::from_pointee(Vec::new())), position: Default::default(),
            in_flight: Default::default(), seek_request: Arc::new(AtomicU64::new(NO_SEEK)), loop_start: Default::default(),
            loop_end: Default::default(), loop_active: Default::default(), file_ch: 2, sample_rate: 48_000,
        }
    }

    fn render(&self) -> crate::dvs::DvsRender {
        let p = self.clone();
        crate::dvs::DvsRender {
            shared: p.shared, samples: p.samples, position: p.position, in_flight: p.in_flight, seek_request: p.seek_request,
            loop_start: p.loop_start, loop_end: p.loop_end, loop_active: p.loop_active, file_ch: p.file_ch,
            sample_rate: p.sample_rate, drive: Default::default(), cur: None, buf_id: 0, clock: 0, last_speed: None,
        }
    }
}

/// Who holds the cpal stream: the supervisor thread (Linux) or the handle.
enum StreamOwner {
    #[allow(dead_code)]
    Thread(Option<thread::JoinHandle<()>>),
    #[allow(dead_code)]
    Inline(cpal::Stream),
}

/// Tests: what a stand-in device heard, pull by pull (time, loudest sample).
#[cfg(test)]
pub type Trace = Arc<Mutex<Vec<(std::time::Instant, f32)>>>;

/// A plugin host's audio callback, standing in for the sound card's: the
/// deck's output, interleaved stereo at the rate the deck was opened at.
pub struct HostOutput { fill: OutputFill }

impl HostOutput {
    /// One block of output (interleaved stereo), from the RT thread: no
    /// allocation, no blocking.  Silence where the deck has nothing ready.
    pub fn fill(&mut self, out: &mut [f32]) { self.fill.fill(out) }
}

/// Where [`AudioHandle::open_host`] leaves the deck's [`HostOutput`].
pub type HostSlot = Arc<Mutex<Option<HostOutput>>>;

/// Where a deck's output goes.
enum Output {
    /// A plugin host pulls it (`open_host`).
    Host { sr: u32, slot: HostSlot },
    /// The system's output device, at its default config.
    Device(cpal::Device, cpal::SupportedStreamConfig),
    /// The tests' stand-in: pulls the output at real time, 48 kHz stereo.
    #[allow(dead_code)]
    Test,
    /// Tests: a stand-in device at `sr` (stereo) that takes `frames` every
    /// `every_ms`, as an ALSA device woken late takes two periods at once.
    /// `trace`, when given, records each pull's time and loudest sample.
    #[cfg(test)]
    TestPulls { sr: u32, frames: usize, every_us: u64, trace: Option<Trace> },
    /// No device at start: silent at real time until one appears (Linux).
    #[cfg(target_os = "linux")]
    Waiting,
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
fn supervise(device: cpal::Device, parts: StreamParts, mut stream: cpal::Stream, sd: &AtomicBool) {
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
}

/// The output callback's work, shared by the device stream and the tests'
/// stand-in device: pull the ring into `out`, fading out on stop and
/// flushing on a seek.  Real-time safe: no allocation, no blocking.
/// MENU → OUTPUT GAIN: a trim on what leaves the deck, as a linear factor
/// (f32 bits), 1.0 = 0 dB.  Only ever below 1: it turns the deck down
/// against a hot output (an iPad at full volume) without touching the
/// deck's own levels, meters or analysis.  Read once per output buffer.
static OUTPUT_GAIN: AtomicU32 = AtomicU32::new(0x3f80_0000);   // 1.0

/// Tests: OUTPUT_GAIN is one global, so the test that steps it holds this,
/// and a test deck starting up takes it to apply its own setting; otherwise
/// a deck built in parallel reset the gain under that test.
#[cfg(test)]
pub(crate) static GAIN_LOCK: Mutex<()> = Mutex::new(());

/// Set the output trim in dB (0 or below; anything above is taken as 0).
pub fn set_output_gain_db(db: f32) {
    let g = 10f32.powf(db.min(0.0) / 20.0);
    OUTPUT_GAIN.store(g.to_bits(), Ordering::Relaxed);
}

/// The output trim as a linear factor.
pub fn output_gain() -> f32 { f32::from_bits(OUTPUT_GAIN.load(Ordering::Relaxed)) }

struct OutputFill {
    consumer:   Arc<Mutex<rtrb::Consumer<f32>>>,
    playing:    Arc<AtomicBool>,
    scrub_live: Arc<AtomicBool>,
    drain_flag: Arc<AtomicBool>,
    stats:      Arc<AudioStats>,
    device_ch:  usize,
    fade_step:  f32,
    gain:       f32,
    /// The output trim as last applied; a change ramps across one buffer.
    trim:       f32,
    /// DVS: renders the record's way while it has the deck.
    dvs:        crate::dvs::DvsRender,
    /// A seek while playing (BEAT JUMP, hot cue): up to SEEK_HOLD_S of the
    /// audio the seek threw away, played on until the new position's audio
    /// is ready and then crossfaded into it.  `tail_at` is how far it has
    /// played; `xf_at` the frame of the crossfade once it has begun.
    tail:       Vec<f32>,
    tail_at:    usize,
    xf_at:      Option<usize>,
    /// Equal-power crossfade gains (the new audio's), one per frame.
    xfade:      Vec<f32>,
    unplayed:   Arc<AtomicU64>,
}

impl OutputFill {
    fn new(p: &StreamParts) -> OutputFill {
        // Stop fade: cutting to silence mid-waveform is a step, and a step is
        // the click heard on every CUE release / pause.  On stop the callback
        // keeps playing what the ring already holds while a gain ramps to
        // zero over STOP_FADE_S (below anything audible as a fade), THEN goes
        // silent and honours any pending flush.  Start is untouched — no
        // fade-in — so cue latency stays exactly as it is.
        const STOP_FADE_S: f32 = 0.003;
        // Seek crossfade: long enough that the cut is no click, short enough
        // to stay a jump (a 4 ms slur is below anything heard as a fade).
        const SEEK_XFADE_S: f32 = 0.004;
        // How long the old audio may carry on while the stretcher re-primes
        // from the new spot (Rubber Band wants several blocks after a reset).
        const SEEK_HOLD_S: f32 = 0.040;
        let xf_frames = ((p.device_sr as f32 * SEEK_XFADE_S) as usize).max(1);
        let hold_frames = ((p.device_sr as f32 * SEEK_HOLD_S) as usize).max(2 * xf_frames);
        OutputFill {
            consumer: Arc::clone(&p.consumer), playing: Arc::clone(&p.playing), scrub_live: Arc::clone(&p.scrub_live),
            drain_flag: Arc::clone(&p.drain_flag), stats: Arc::clone(&p.stats), device_ch: p.device_ch,
            fade_step: 1.0 / (p.device_sr as f32 * STOP_FADE_S).max(1.0),   // per frame
            gain: 1.0,
            trim: output_gain(),
            dvs: p.dvs.render(),
            tail: Vec::with_capacity(hold_frames * p.device_ch),
            tail_at: 0,
            xf_at: None,
            unplayed: Arc::clone(&p.unplayed),
            xfade: (0..xf_frames).map(|i| ((i as f32 + 0.5) / xf_frames as f32 * std::f32::consts::FRAC_PI_2).sin()).collect(),
        }
    }

    fn fill(&mut self, out: &mut [f32]) {
        self.fill_raw(out);
        // OUTPUT GAIN, last: 0 dB leaves the samples untouched; a change
        // moves linearly across this buffer, so stepping it never clicks.
        let target = output_gain();
        if target == 1.0 && self.trim == 1.0 { return; }
        let frames = (out.len() / self.device_ch.max(1)).max(1) as f32;
        let step = (target - self.trim) / frames;
        for frame in out.chunks_mut(self.device_ch.max(1)) {
            self.trim += step;
            for s in frame.iter_mut() { *s *= self.trim; }
        }
        self.trim = target;
    }

    fn fill_raw(&mut self, out: &mut [f32]) {
        // Only ever contended for the instant a replacement stream
        // takes over; silence is the right answer then.
        let Ok(mut consumer) = self.consumer.try_lock() else { out.fill(0.0); return; };
        // DVS: the record has the deck; what the processor left in the ring
        // is stale (it stands aside while the record drives).
        if self.dvs.fill(out, self.device_ch) {
            while consumer.pop().is_ok() {}
            self.gain = 0.0;
            self.tail_at = self.tail.len();   // and any jump crossfade is over
            return;
        }
        if !self.playing.load(Ordering::Relaxed) && !self.scrub_live.load(Ordering::Relaxed) {
            self.tail_at = self.tail.len();   // a pause ends any jump crossfade
            if self.gain > 0.0 {
                // Fading out: ring audio × a falling gain, frame by
                // frame.  A flush requested meanwhile waits until
                // the fade is done (the ring holds ~90 ms, plenty).
                for frame in out.chunks_mut(self.device_ch) {
                    let g = self.gain * self.gain;   // squared ramp: lands with zero slope
                    for sample in frame.iter_mut() {
                        *sample = consumer.pop().unwrap_or(0.0) * g;
                    }
                    self.gain = (self.gain - self.fade_step).max(0.0);
                    if self.gain == 0.0 { break; }
                }
                if self.gain > 0.0 { return; }
                // Fell to zero inside this buffer: the rest is
                // silence and the flush can proceed.
            }
            if self.drain_flag.load(Ordering::Acquire) {
                let mut flushed = 0usize;
                while consumer.pop().is_ok() { flushed += 1; }
                self.unplayed.store((flushed / self.device_ch) as u64, Ordering::Relaxed);
                self.drain_flag.store(false, Ordering::Release);
            }
            out.fill(0.0);
            return;
        }
        self.gain = 1.0;   // playing: no fade-in, and a restart mid-fade snaps up

        // A seek while playing (BEAT JUMP, a hot cue).  Cutting the ring here
        // left the output silent while the processor re-primed from the new
        // spot (a block at least, ~12 ms, more for Rubber Band), and the cut
        // itself was a step: a stutter and a click on every jump.  Instead
        // this buffer still plays the audio the ring already holds, up to
        // SEEK_HOLD_S more of it is kept as a tail, and only then is the rest
        // flushed and the flag cleared: the processor waits for the clear
        // before it pushes the new position, so nothing new is thrown away.
        // From the next buffer the tail plays on until the new audio is
        // there, then crossfades into it.  The jump lands as soon as the new
        // audio exists, where the flushed path played silence until then.
        if self.drain_flag.load(Ordering::Acquire) {
            let underran = Self::pop_into(&mut consumer, out);
            self.tail.clear();
            while self.tail.len() < self.tail.capacity() {
                match consumer.pop() { Ok(s) => self.tail.push(s), Err(_) => break }
            }
            self.tail.truncate(self.tail.len() / self.device_ch * self.device_ch);
            self.tail_at = 0;
            self.xf_at = None;
            let mut flushed = self.tail.len();
            while consumer.pop().is_ok() { flushed += 1; }
            // Published before the flag clears (Release): the processor reads
            // it once it sees the clear, to resolve a relative jump.
            self.unplayed.store((flushed / self.device_ch) as u64, Ordering::Relaxed);
            self.drain_flag.store(false, Ordering::Release);
            if underran > 0 {
                self.stats.underrun_samples.fetch_add(underran, Ordering::Relaxed);
                self.stats.underrun_events.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }

        // The hold and crossfade, frame by frame: the old tail at full level
        // until the ring holds the new audio for the whole crossfade and a
        // buffer after it, then old out and new in, equal power (the two are
        // unrelated music).  Starting on less ran the ring dry a buffer later:
        // Rubber Band re-primes in bursts.  A tail about to run out before
        // then fades out alone.
        //
        // On time: the new audio begins at the jump target, but the tail has
        // already played some frames of wall-clock time past the jump.  That
        // much of the new audio is skipped as the crossfade starts, so the
        // deck lands where it would have been had the new audio been ready
        // at once — a jump in a running mix stays on the beat however long
        // the stretcher took to re-prime.
        let mut out = out;
        if self.tail_at < self.tail.len() {
            let ch = self.device_ch;
            let xf = self.xfade.len();
            let ready = xf * ch + out.len();   // plus the frames to skip, below
            let mut done = 0;
            for frame in out.chunks_mut(ch) {
                if self.tail_at >= self.tail.len() { break; }
                let left = (self.tail.len() - self.tail_at) / ch;
                if self.xf_at.is_none() && left >= xf && consumer.slots() >= ready + self.tail_at {
                    for _ in 0..self.tail_at { let _ = consumer.pop(); }   // catch up: see above
                    self.xf_at = Some(0);
                }
                let old = &self.tail[self.tail_at..self.tail_at + ch];
                match self.xf_at {
                    Some(k) => {
                        let gin = self.xfade[k];
                        let gout = (1.0 - gin * gin).sqrt();
                        for (c, sample) in frame.iter_mut().enumerate() {
                            *sample = old[c] * gout + consumer.pop().unwrap_or(0.0) * gin;
                        }
                        self.xf_at = Some(k + 1);
                        // Crossfade complete: the rest of the tail is dropped.
                        if k + 1 == xf { self.tail_at = self.tail.len(); done += ch; break; }
                    }
                    None => {
                        let g = if left <= xf { left as f32 / xf as f32 } else { 1.0 };
                        for (c, sample) in frame.iter_mut().enumerate() { *sample = old[c] * g; }
                    }
                }
                self.tail_at += ch;
                done += ch;
            }
            out = &mut std::mem::take(&mut out)[done..];
        }

        // Fill from the ring; count any sample we had to invent as
        // silence.  No allocation, and the atomics are touched only
        // when silence actually happens, so the clean path stays a
        // bare pop().
        let underran = Self::pop_into(&mut consumer, out);
        if underran > 0 {
            // Genuine starvation — the core failed to keep up.
            self.stats.underrun_samples.fetch_add(underran, Ordering::Relaxed);
            self.stats.underrun_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Fill `out` from the ring; the count of samples it had to make silent.
    fn pop_into(consumer: &mut rtrb::Consumer<f32>, out: &mut [f32]) -> u64 {
        let mut underran = 0u64;
        for sample in out.iter_mut() {
            match consumer.pop() {
                Ok(s)  => *sample = s,
                Err(_) => { *sample = 0.0; underran += 1; }
            }
        }
        underran
    }
}

fn build_stream(device: &cpal::Device, p: &StreamParts) -> Result<cpal::Stream> {
    let failed = Arc::clone(&p.failed);
    let mut fill = OutputFill::new(p);
    let mut rt_asked = false;
    let stream = device
        .build_output_stream::<f32, _, _>(
            &p.config,
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

// ── Processor loop ────────────────────────────────────────────────────────────

/// Where a relative jump lands: the source sample heard at the first of the
/// `unplayed` output frames (the decoder's cursor `pos`, less (more, in
/// reverse) the queued
/// output at `speed` source frames per output frame and the stretcher's
/// latency, as `in_flight` reckons it), plus `delta`; clamped to the track
/// and on a frame boundary.
fn jump_target(pos: u64, unplayed: usize, speed: f32, latency: usize, delta: i64, rev: bool, ch: usize, len: usize) -> u64 {
    // Read backwards (`rev`), the queued audio lies after the cursor.
    let queued = (unplayed as f64 * speed as f64 + latency as f64) * ch as f64;
    let heard = if rev { pos as f64 + queued } else { pos as f64 - queued };
    let t = (heard + delta as f64).clamp(0.0, len as f64) as u64;
    t / ch as u64 * ch as u64
}

/// An absolute seek `pos` moved on by a jump `delta` that came right behind
/// it (nothing has played from `pos` yet), clamped and frame-aligned.
fn seek_plus(pos: u64, delta: i64, ch: usize, len: usize) -> u64 {
    let t = (pos as i64 + delta).clamp(0, len as i64) as u64;
    t / ch as u64 * ch as u64
}

/// Reverse play's next block, into `scratch`: the `want` samples before
/// `pos`, frame by frame from the last one back, `src(i)` giving sample `i`.
/// Inside a loop `[ls, le)` the read wraps from its start to its end, as a
/// forward read wraps the other way; at the track's start it stops short.
/// Returns the cursor after it (where the next block reads back from).
fn read_block_rev(src: &dyn Fn(usize) -> f32, len: usize, pos: usize, want: usize, ch: usize,
                  lp: Option<(usize, usize)>, scratch: &mut Vec<f32>) -> usize {
    scratch.clear();
    let mut cur = pos.min(len) / ch * ch;
    // Only a cursor inside (or past) the loop wraps in it.
    let wraps = lp.filter(|&(ls, le)| le > ls && cur > ls);
    for _ in 0..want / ch {
        if let Some((ls, le)) = wraps { if cur <= ls { cur = le / ch * ch; } }
        if cur < ch { break; }
        cur -= ch;
        for c in 0..ch { scratch.push(src(cur + c)); }
    }
    cur
}

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
    use super::{decode_file, pick_rate, read_block, resample_interleaved, to_stereo, StreamResampler};

    #[test]
    fn the_decoded_rate_wins_over_the_header() {
        // A header that guessed 44.1 kHz for 48 kHz audio played it 8 % slow.
        assert_eq!(pick_rate(44_100, Some(48_000)), 48_000);
        assert_eq!(pick_rate(48_000, Some(48_000)), 48_000);
        assert_eq!(pick_rate(44_100, None), 44_100, "nothing decoded yet: keep the header");
        assert_eq!(pick_rate(44_100, Some(0)), 44_100, "a zero rate is no answer");
    }

    #[test]
    fn mono_alac_loads_as_stereo_at_its_rate() {
        // A 0.5 s 440 Hz mono ALAC tone at 48 kHz: ALAC used to be refused
        // outright, and mono tracks were refused by the stereo deck.
        let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../decode/tests/fixtures/tone_mono_48k_alac.m4a");
        let (pcm, sr, ch, _) = decode_file(&f).unwrap();
        assert_eq!((sr, ch), (48_000, 2));
        let frames = pcm.len() / 2;
        assert!((frames as i64 - 24_000).abs() < 1_100, "{frames} frames");
        assert!(pcm.chunks(2).all(|f| f[0] == f[1]));
        assert!(pcm.iter().any(|x| x.abs() > 0.1), "silent");
    }

    #[test]
    fn to_stereo_keeps_front_left_and_right() {
        assert_eq!(to_stereo(vec![1.0, 2.0], 1), (vec![1.0, 1.0, 2.0, 2.0], 2));
        assert_eq!(to_stereo(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6), (vec![1.0, 2.0], 2));
        assert_eq!(to_stereo(vec![1.0, 2.0], 2), (vec![1.0, 2.0], 2));
    }

    fn ramp(n: usize) -> Vec<f32> { (0..n).map(|i| i as f32).collect() }

    /// The start-up track and every LOAD are converted to the device rate:
    /// a 1 kHz tone at 48 kHz must come out at 44.1 kHz the right length and
    /// still 1 kHz (the bug this fixed played it 8 % slow and flat).
    /// Load-time resampling cost: 200 s of 48 kHz stereo to 44.1 kHz.
    /// `cargo test --release -p opendeck-app resample_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn resample_speed() {
        let n = 48_000 * 200;
        let mut x = 1u32;
        let s: Vec<f32> = (0..n * 2).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; (x as f32 / u32::MAX as f32) - 0.5 }).collect();
        let t = std::time::Instant::now();
        let out = resample_interleaved(&s, 2, 48_000, 44_100).unwrap();
        println!("resample_speed: {} frames in {:.2}s", out.len() / 2, t.elapsed().as_secs_f32());
    }

    /// The loader feeds the resampler a decode packet at a time: odd sizes
    /// in, the same samples out as one pass over the whole buffer.
    #[test]
    fn stream_resampler_matches_one_pass() {
        let n = 48_000 * 3 + 777;
        let s: Vec<f32> = (0..n * 2).map(|i| ((i as f32) * 0.013).sin() * 0.5).collect();
        let whole = resample_interleaved(&s, 2, 48_000, 44_100).unwrap();
        let mut rs = StreamResampler::new(48_000, 44_100, 2).unwrap();
        let mut pieces = Vec::new();
        let mut i = 0;
        for step in [1152usize, 4096, 13, 20_000, 1].iter().cycle() {
            if i >= s.len() { break; }
            let e = (i + step * 2).min(s.len());
            pieces.extend(rs.push(&s[i..e]).unwrap());
            i = e;
        }
        pieces.extend(rs.finish().unwrap());
        assert_eq!(pieces.len(), whole.len());
        assert!(pieces.iter().zip(&whole).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn resampling_keeps_length_and_pitch() {
        // 96 kHz files on 48 / 44.1 kHz outputs, and the common pair both ways.
        for (src, dst) in [(48_000, 44_100), (44_100, 48_000), (96_000, 48_000), (96_000, 44_100), (88_200, 44_100)] {
            resample_case(src, dst);
        }
    }

    fn resample_case(src: u32, dst: u32) {
        let secs = 1.0f64;
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
        assert!((hz - 1000.0).abs() < 5.0, "{src} → {dst}: came out at {hz:.1} Hz");
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
        assert!((peak as i64 - want).abs() <= 1, "{src} → {dst}: click moved to frame {peak}, want {want}");
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
    key_shift:  Arc<AtomicI32>,
    lp:         LoopShared,
    drain_flag: Arc<AtomicBool>,
    in_flight:  Arc<AtomicU64>,
    seek_request: Arc<AtomicU64>,
    jump:       JumpShared,
    scrub:      ScrubShared,
    stems:      Arc<StemControl>,
    complete:   Arc<AtomicBool>,
    dvs:        Arc<crate::dvs::DvsShared>,
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
    // Source frames inside the stretcher: fed in, not yet out (output frames
    // × speed), counted since its last reset.  What is heard is `proc_pos`
    // less this and the ring.  Counted, not `latency_frames()`: Rubber Band's
    // reported latency is not its delay once the start pad/delay is trimmed,
    // and a jump reckoned from it landed ~40 ms early.
    let mut held: f64 = 0.0;
    // Reading backwards (the direction lever's REV / SLIP REV).
    let mut rev = false;
    // A quantized turn waiting for its grid point: the source sample it
    // turns at and, turning back from SLIP REV, where it lands.
    let mut turn_at: Option<(u64, Option<u64>)> = None;
    // Where the last quantized turn was (SLIP REV's landing mirrors it), and
    // a landing's crossfade still to make: from the old read (its spot and
    // way) into the new one.
    let mut turned_from: u64 = 0;
    let mut land_xf: Option<(u64, bool)> = None;
    // Until a quantized turn is heard, the queued audio still runs the old
    // way: the UI is shown the cursor as if it had gone on (the turn's
    // spot, the old way, the source samples read since) so that its
    // `position` less (more) `in_flight` stays what is heard.
    let mut turn_view: Option<(u64, bool, u64)> = None;
    let mut xf_buf: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * file_ch);

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
        // ── DVS ───────────────────────────────────────────────────────────────
        // The record has the deck: the output callback plays it. Stand aside
        // (and leave seek_request to it) until it hands back.
        if dvs.driving() {
            stretcher.reset();
            thread::park_timeout(Duration::from_millis(5));
            if !dvs.driving() {
                let req = seek_request.swap(NO_SEEK, Ordering::AcqRel);
                if req != NO_SEEK { proc_pos = req; position.store(proc_pos, Ordering::Relaxed); }
                in_flight.store(0, Ordering::Relaxed);
            }
            continue;
        }
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
            let mut req = seek_request.swap(NO_SEEK, Ordering::AcqRel);
            let mut jmp = jump.request.swap(NO_JUMP, Ordering::AcqRel);
            // A turn of the lever is a jump of nothing, the other way.
            let want_rev = jump.reverse.load(Ordering::Acquire);
            if jmp == NO_JUMP && want_rev != rev { jmp = 0; }
            turn_at = None;
            land_xf = None;
            turn_view = None;
            if req != NO_SEEK && jmp != NO_JUMP {
                // A jump right behind a seek: from the spot just sought.
                req = seek_plus(req, jmp, file_ch, samples.load().len());
            } else if req == NO_SEEK && jmp != NO_JUMP {
                // Paused, nothing is consumed: what is heard next is the
                // ring's head, as the UI's playhead has it.
                let queued = (ring_len - producer.slots()) / device_ch;
                let spd = f32::from_bits(speed.load(Ordering::Relaxed)).clamp(0.25, 4.0);
                req = jump_target(proc_pos, queued, spd, held.round() as usize, jmp, rev, file_ch, samples.load().len());
            }
            rev = want_rev;
            jump.reading_rev.store(rev, Ordering::Relaxed);
            if req != NO_SEEK {
                log::debug!("proc: seek {proc_pos} → {req} (paused)");
                stretcher.reset();
                held = 0.0;
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
        let mut jmp = jump.request.swap(NO_JUMP, Ordering::AcqRel);
        // A turn of the lever: a jump of nothing, then reading the other way.
        // SLIP REV with QUANTIZE waits for the grid instead (see below).
        let want_rev = jump.reverse.load(Ordering::Acquire);
        let q_step = f64::from_bits(jump.q_step.load(Ordering::Relaxed));
        let in_loop = lp.active.load(Ordering::Relaxed) && lp.end.load(Ordering::Relaxed) > lp.start.load(Ordering::Relaxed);
        // How a jump below turns: `flip` reads the other way after it, and
        // `mirror_about` lands it mirrored about that spot from what is heard
        // (a quantized turn whose beat has just been heard: as if it had
        // turned on it).  `reread` is a jump of nothing the same way, to
        // read again from what is heard up to a beat already read past.
        let mut flip = false;
        let mut mirror_about: Option<u64> = None;
        let mut reread = false;
        if want_rev == rev {
            if turn_at.take().is_some() { log::debug!("proc: quantized turn called off"); }
        } else if q_step > 0.0 && !in_loop && req == NO_SEEK && jmp == NO_JUMP {
            if turn_at.is_none() {
                // The grid point nearest what is heard now: at most half a
                // step to wait, or (just heard) none.
                let len = samples.len();
                let queued = (ring_len - producer.slots()) / device_ch;
                let spd = f32::from_bits(speed.load(Ordering::Relaxed)).clamp(0.25, 4.0);
                let heard = jump_target(proc_pos, queued, spd, held.round() as usize, 0, rev, file_ch, len);
                let anchor = f64::from_bits(jump.q_anchor.load(Ordering::Relaxed));
                let g = grid_nearest(heard as f64 / file_ch as f64, anchor, q_step).round() as u64 * file_ch as u64;
                let g = g.min(len as u64 / file_ch as u64 * file_ch as u64);
                let land = jump.q_land.load(Ordering::Relaxed);
                let ahead = if rev { g < heard } else { g > heard };
                if !ahead {
                    // Heard already: turn now, mirrored about the beat (or,
                    // turning back, about where the reverse began: where
                    // forward play would be).
                    jmp = 0;
                    flip = true;
                    mirror_about = Some(if land { turned_from } else { g });
                    if !land { turned_from = g; }
                    log::debug!("proc: beat {g} just heard (at {heard}): turning now");
                } else {
                    let land = land.then(|| mirror(turned_from, g, file_ch, len));
                    turn_at = Some((g, land));
                    // The read is past it already: read again from what is
                    // heard, then turn on it.
                    if if rev { proc_pos < g } else { proc_pos > g } { jmp = 0; reread = true; }
                    log::debug!("proc: turn at {g} (heard {heard}, read {proc_pos}){}{}",
                                land.map_or(String::new(), |l| format!(", landing at {l}")),
                                if reread { ", reading again" } else { "" });
                }
            }
        } else {
            if jmp == NO_JUMP { jmp = 0; }
            flip = true;
        }
        if req != NO_SEEK || jmp != NO_JUMP {
            if !reread { turn_at = None; }
            land_xf = None;
            turn_view = None;
            // For a relative jump: where the audio stands now, before the
            // flush (an absolute seek pending at the same time wins).
            let before  = proc_pos;
            let latency = held.round() as usize;
            let spd_now = f32::from_bits(speed.load(Ordering::Relaxed)).clamp(0.25, 4.0);
            let queued  = (ring_len - producer.slots()) / device_ch;
            stretcher.reset();
            held = 0.0;
            if req != NO_SEEK {
                log::debug!("proc: seek {proc_pos} → {req}");
                // A jump right behind it lands from the spot just sought.
                proc_pos = if jmp != NO_JUMP { seek_plus(req, jmp, file_ch, samples.len()) } else { req };
                position.store(proc_pos, Ordering::Relaxed);   // reflect immediately
            }
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
            if req == NO_SEEK {
                // The jump's crossfade starts on the first frame the flush
                // left unplayed (the callback counted them); no callback in
                // time (no device): the ring as it stood.
                let unplayed = if drain_flag.load(Ordering::Acquire) { queued }
                               else { jump.unplayed.load(Ordering::Relaxed) as usize };
                proc_pos = match mirror_about {
                    Some(about) => mirror(about, jump_target(before, unplayed, spd_now, latency, 0, rev, file_ch, samples.len()), file_ch, samples.len()),
                    None => jump_target(before, unplayed, spd_now, latency, jmp, rev, file_ch, samples.len()),
                };
                log::debug!("proc: jump {jmp:+} from {before} ({unplayed} frames unplayed) → {proc_pos}{}",
                            if flip { ", turning" } else { "" });
                position.store(proc_pos, Ordering::Relaxed);
            }
            if flip { rev = want_rev; }
            jump.reading_rev.store(rev, Ordering::Relaxed);
        }

        // ── A quantized turn on its grid point ────────────────────────────────
        // The read stops right on it (blocks are cut short below), turns, and
        // goes on the other way from the same sample: no gap, nothing to
        // crossfade.  Turning back from SLIP REV, it lands where forward play
        // would be instead, crossfaded from the reverse.
        if let Some((at, land)) = turn_at {
            if (!rev && proc_pos >= at) || (rev && proc_pos <= at) {
                turn_at = None;
                turn_view = Some((at, rev, 0));
                rev = !rev;
                match land {
                    None => turned_from = at,
                    Some(to) => {
                        land_xf = Some((at, !rev));
                        proc_pos = to;
                    }
                }
                log::debug!("proc: turned at {at}{}", land.map_or(String::new(), |l| format!(", landed at {l}")));
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
        let need = back_pressure_slots(f32::from_bits(speed.load(Ordering::Relaxed)), device_ch);
        if free < need {
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

        // ── Decode frontier ───────────────────────────────────────────────────
        // A streaming load is still decoding past here: wait for more rather
        // than ending the track (decode runs far ahead of real time, so this
        // only happens right after a seek into undecoded audio).
        let complete_now = complete.load(Ordering::Acquire);
        if !complete_now {
            let (ls, le) = (lp.start.load(Ordering::Relaxed) as usize, lp.end.load(Ordering::Relaxed) as usize);
            let in_loop = lp.active.load(Ordering::Relaxed) && le > ls && le <= samples.len();
            if !in_loop && !rev && proc_pos as usize + BLOCK_FRAMES * file_ch > samples.len() {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
        }

        // ── End of track ──────────────────────────────────────────────────────
        // The source is exhausted, but the ring buffer still holds already-
        // decoded audio that the device is playing out.  Keep publishing the
        // drain distance so the UI's audible-position estimate can actually
        // reach the end (otherwise in_flight freezes ~93 ms short and the deck
        // never registers end-of-track).
        // In reverse, the start of the track is its end.
        if (!rev && proc_pos >= samples.len() as u64) || (rev && proc_pos == 0) {
            let ring_out_frames = (ring_len - producer.slots()) / device_ch;
            in_flight.store(ring_out_frames as u64 * file_ch as u64, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(5));
            continue;
        }

        // ── Read a block from the source (loop-aware) ─────────────────────────
        let mut want = BLOCK_FRAMES * file_ch;
        // A quantized turn ahead: read no further than its grid point.
        if let Some((at, _)) = turn_at {
            let d = if rev { proc_pos.saturating_sub(at) } else { at.saturating_sub(proc_pos) } as usize;
            if d > 0 { want = want.min(d); }
        }
        let (ls, le) = (lp.start.load(Ordering::Relaxed) as usize, lp.end.load(Ordering::Relaxed) as usize);
        let looping  = lp.active.load(Ordering::Relaxed) && le > ls && le <= samples.len();
        let bank = stems.bank().filter(|b| b.is_for(&current));
        let (src_block, src_end): (&[f32], usize) = if rev {
            // Backwards: the frames before the cursor, last first, through
            // the stem mix sample by sample when it is in play.
            let stemmed = bank.is_some() || mixer.active();
            let src = |i: usize| if stemmed { mixer.at(&current, bank.as_deref(), i) } else { samples[i] };
            let end = read_block_rev(&src, samples.len(), proc_pos as usize, want, file_ch,
                                     looping.then_some((ls, le)), &mut loop_buf);
            (&loop_buf[..], end)
        } else {
            let (src_block, src_end) = read_block(samples, proc_pos as usize, want,
                                                  looping.then_some((ls, le)), &mut loop_buf);

            // Stems: the same ranges through the stem mix, before the stretcher
            // (one stretcher whatever the stems do).  Untouched when no bank is
            // loaded for this track or every stem is at full.
            let src_block: &[f32] = if bank.is_some() || mixer.active() {
                let (ranges, _) = block_ranges(samples.len(), proc_pos as usize, want, looping.then_some((ls, le)));
                if mixer.mix(samples, bank.as_deref(), stems.gains(), &ranges, &mut mix_buf) { &mix_buf } else { src_block }
            } else { src_block };
            (src_block, src_end)
        };

        // A SLIP REV landing: its first few ms crossfade (equal power) from
        // the reverse going on past the turn into the forward read.
        let src_block: &[f32] = if let Some((from, from_rev)) = land_xf.take() {
            let stemmed = bank.is_some() || mixer.active();
            let src = |i: usize| if stemmed { mixer.at(&current, bank.as_deref(), i) } else { samples[i] };
            let frames = samples.len() / file_ch;
            let n = ((sample_rate as f32 * LAND_XFADE_S) as usize).max(1).min(src_block.len() / file_ch);
            xf_buf.clear();
            xf_buf.extend_from_slice(src_block);
            let f0 = (from / file_ch as u64) as i64;
            for i in 0..n {
                let fr = if from_rev { f0 - 1 - i as i64 } else { f0 + i as i64 };
                let t = (i as f32 + 0.5) / n as f32 * std::f32::consts::FRAC_PI_2;
                for c in 0..file_ch {
                    let old = if fr >= 0 && (fr as usize) < frames { src(fr as usize * file_ch + c) } else { 0.0 };
                    let k = i * file_ch + c;
                    xf_buf[k] = xf_buf[k] * t.sin() + old * t.cos();
                }
            }
            &xf_buf
        } else { src_block };

        lp.consumed.fetch_add(src_block.len() as u64, Ordering::Relaxed);

        let final_block = !looping && complete_now && if rev { src_end == 0 } else { src_end >= samples.len() };

        // Advance the shared position so the UI/renderer sees it — unless a
        // seek arrived during this block: then the UI already published the
        // new spot and this store would overwrite it with the stale cursor.
        proc_pos = src_end as u64;
        if let Some(v) = turn_view.as_mut() { v.2 += src_block.len() as u64; }
        if seek_request.load(Ordering::Acquire) == NO_SEEK {
            position.store(shown_pos(proc_pos, turn_view), Ordering::Relaxed);
        }

        // ── Update speed + key-lock mode ─────────────────────────────────────────
        // Master Tempo on (key_lock): tempo moves, pitch held (pure time-stretch).
        // Master Tempo off (varispeed): pitch tracks speed like vinyl — achieved
        // by pitch-shifting the stretcher UP by the same factor as the tempo
        // change, so time_ratio (1/spd) and pitch_scale (spd) combine to a plain
        // speed change of the source.
        let spd = f32::from_bits(speed.load(Ordering::Relaxed)).clamp(0.25, 4.0);
        stretcher.set_speed(spd);
        // KEY SHIFT adds its semitones on top, in either mode.
        let shift = key_shift.load(Ordering::Relaxed) as f32;
        let semitones = shift + if key_lock.load(Ordering::Relaxed) { 0.0 } else { 12.0 * spd.log2() };
        stretcher.set_pitch_semitones(semitones);

        // ── Timestretch ───────────────────────────────────────────────────────
        ts_out.clear();
        stretcher.process(src_block, &mut ts_out);
        held += (src_block.len() / file_ch) as f64;

        if final_block && ts_out.is_empty() {
            // Flush RubberBand's tail at end of track.
            let silence = vec![0.0f32; BLOCK_FRAMES * file_ch];
            stretcher.process(&silence, &mut ts_out);
            held += BLOCK_FRAMES as f64;
        }
        held = (held - (ts_out.len() / file_ch) as f64 * spd as f64).max(0.0);

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
            (ring_out_frames as f32 * spd) as u64 + held.round() as u64;
        in_flight.store(src_frames_ahead * file_ch as u64, Ordering::Relaxed);
        // A quantized turn is heard once what was read since it is all that
        // is queued: from then on the real cursor, and the new way.
        if let Some((_, _, n)) = turn_view {
            if n >= src_frames_ahead * file_ch as u64 {
                turn_view = None;
                jump.reading_rev.store(rev, Ordering::Relaxed);
                if seek_request.load(Ordering::Acquire) == NO_SEEK { position.store(proc_pos, Ordering::Relaxed); }
            }
        }
    }
}

/// The cursor the UI is shown: the real one, or while a quantized turn is
/// still queued (`turn_view`: its spot, the old way, samples read since),
/// where the old way would have got to.
fn shown_pos(proc_pos: u64, turn_view: Option<(u64, bool, u64)>) -> u64 {
    match turn_view {
        Some((at, true, n))  => at.saturating_sub(n),
        Some((at, false, n)) => at + n,
        None => proc_pos,
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
    let (samples, file_ch) = to_stereo(samples, file_ch);
    log::info!(
        "decoded {} frames ({:.1}s) at {}Hz {}ch",
        samples.len() / file_ch,
        samples.len() as f64 / file_ch as f64 / file_sr as f64,
        file_sr, file_ch,
    );
    Ok((samples, file_sr, file_ch, tags))
}

/// The deck runs in stereo. A mono track is copied to both sides, and a
/// track with more than two channels keeps its front left and right, so
/// every track loads whatever its layout (a mono file used to be refused).
pub fn to_stereo(samples: Vec<f32>, ch: usize) -> (Vec<f32>, usize) {
    match ch {
        2 => (samples, 2),
        0 | 1 => (samples.iter().flat_map(|&x| [x, x]).collect(), 2),
        n => (samples.chunks_exact(n).flat_map(|f| [f[0], f[1]]).collect(), 2),
    }
}

/// The rate of the decoded audio itself. The container's rate is what the
/// decoder reports before decoding (44.1 kHz when the header has none); if
/// the packets disagree, they win, or the track plays at the wrong speed
/// (a 48 kHz track taken for 44.1 kHz runs 8 % slow).
fn true_rate(decoder: &SymphoniaDecoder, header_sr: u32) -> u32 {
    pick_rate(header_sr, decoder.decoded_rate())
}

/// The rule behind [`true_rate`]: decoded packets win over the header.
pub fn pick_rate(header_sr: u32, decoded: Option<u32>) -> u32 {
    match decoded {
        Some(r) if r != header_sr && r > 0 => {
            log::warn!("container says {header_sr} Hz but the audio decodes at {r} Hz; using {r}");
            r
        }
        _ => header_sr,
    }
}

/// Decode an audio file from an in-memory buffer (e.g. read over NFS from a
/// linked player). `ext` primes the format probe.  Same output as decode_file.
/// Loads stream through `Prep::stream`; this is for the NFS hardware test.
#[cfg(test)]
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
    let (samples, file_ch) = to_stereo(samples, file_ch);
    log::info!("decoded {} frames ({:.1}s) at {}Hz {}ch (from memory)",
        samples.len() / file_ch, samples.len() as f64 / file_ch as f64 / file_sr as f64, file_sr, file_ch);
    Ok((samples, file_sr, file_ch, tags))
}

/// Offline sample-rate conversion of an interleaved buffer, used at LOAD time so
/// a track recorded at a different rate than the deck's pipeline plays at the
/// right pitch.  This is a one-time cost per load, not the real-time SRC that a
/// 48 kHz *output device* would need (WORKSTREAMS A1) — that is still open.
/// One pass of [`StreamResampler`].
pub fn resample_interleaved(
    samples:  &[f32],
    channels: usize,
    src_sr:   u32,
    dst_sr:   u32,
) -> Result<Vec<f32>> {
    if src_sr == dst_sr || samples.is_empty() {
        return Ok(samples.to_vec());
    }
    let mut rs = StreamResampler::new(src_sr, dst_sr, channels)?;
    let mut out = rs.push(samples)?;
    out.extend(rs.finish()?);
    log::info!("resampled {}Hz → {}Hz ({} → {} frames)", src_sr, dst_sr, samples.len() / channels, out.len() / channels);
    Ok(out)
}

/// Sample-rate conversion of a stream that arrives in pieces (a track as it
/// decodes): interleaved in, interleaved out, the output aligned with the
/// input (the filter's delay dropped) and exactly `round(in × ratio)` frames
/// long once [`finish`](Self::finish)ed.
///
/// rubato's FFT resampler: the fixed ratio (147/160 for 48 → 44.1 kHz) as a
/// band-limited FFT filter.  A 256-tap sinc did the same at ~3x the cost: a
/// minute per track in cdj3k-emu (TCG) and on a Pi Zero.
pub struct StreamResampler {
    rs:        rubato::FftFixedIn<f32>,
    channels:  usize,
    ratio:     f64,
    /// Input frames waiting for a whole chunk, per channel.
    pending:   Vec<Vec<f32>>,
    /// Output frames still to drop: the filter's delay.
    skip:      usize,
    in_frames:  u64,
    out_frames: u64,
}

const RESAMPLE_CHUNK: usize = 16_384;

impl StreamResampler {
    pub fn new(src_sr: u32, dst_sr: u32, channels: usize) -> Result<Self> {
        use rubato::Resampler;
        let rs = rubato::FftFixedIn::<f32>::new(src_sr as usize, dst_sr as usize, RESAMPLE_CHUNK, 2, channels)
            .context("failed to create resampler")?;
        let skip = rs.output_delay();
        Ok(Self { rs, channels, ratio: dst_sr as f64 / src_sr as f64,
                  pending: vec![Vec::with_capacity(RESAMPLE_CHUNK); channels], skip, in_frames: 0, out_frames: 0 })
    }

    /// Feed interleaved input; returns whatever output is ready.
    pub fn push(&mut self, interleaved: &[f32]) -> Result<Vec<f32>> {
        use rubato::Resampler;
        let ch = self.channels;
        self.in_frames += (interleaved.len() / ch) as u64;
        let mut out = Vec::new();
        let mut i = 0;
        let frames = interleaved.len() / ch;
        while i < frames {
            let room = RESAMPLE_CHUNK - self.pending[0].len();
            let take = room.min(frames - i);
            for f in i..i + take {
                for c in 0..ch { self.pending[c].push(interleaved[f * ch + c]); }
            }
            i += take;
            if self.pending[0].len() == RESAMPLE_CHUNK {
                let planar = self.rs.process(&self.pending, None).context("resample chunk failed")?;
                for p in self.pending.iter_mut() { p.clear(); }
                self.emit(&planar, usize::MAX, &mut out);
            }
        }
        Ok(out)
    }

    /// The rest: the last partial chunk and the filter's tail, cut so the
    /// whole output is exactly `round(input frames × ratio)` long.
    pub fn finish(&mut self) -> Result<Vec<f32>> {
        use rubato::Resampler;
        let want = (self.in_frames as f64 * self.ratio).round() as u64;
        let mut out = Vec::new();
        if !self.pending[0].is_empty() {
            let rem = std::mem::replace(&mut self.pending, vec![Vec::new(); self.channels]);
            let planar = self.rs.process_partial(Some(&rem), None).context("resample tail failed")?;
            self.emit(&planar, (want - self.out_frames.min(want)) as usize, &mut out);
        }
        while self.out_frames < want {
            let planar = self.rs.process_partial::<Vec<f32>>(None, None).context("resample flush failed")?;
            if planar[0].is_empty() { break; }
            self.emit(&planar, (want - self.out_frames) as usize, &mut out);
        }
        Ok(out)
    }

    /// Interleave planar output into `out`, dropping the delay first and
    /// taking at most `limit` frames.
    fn emit(&mut self, planar: &[Vec<f32>], limit: usize, out: &mut Vec<f32>) {
        let n = planar[0].len();
        let start = self.skip.min(n);
        self.skip -= start;
        let end = n.min(start.saturating_add(limit));
        for f in start..end {
            for c in 0..self.channels { out.push(planar[c][f]); }
        }
        self.out_frames += (end - start) as u64;
    }
}

/// MENU → OUTPUT GAIN at the very end of the output: what the device gets.
#[cfg(test)]
mod output_gain_tests {
    use super::*;

    /// A playing deck's output through the real fill: 0 dB passes the
    /// samples untouched; −6 dB ramps down across one buffer (no click)
    /// and then holds at half amplitude; back to 0 dB ramps up again.
    #[test]
    fn output_gain_trims_the_output_without_a_jump() {
        let _gain = GAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(1 << 16);
        let parts = StreamParts {
            config: cpal::StreamConfig { channels: 2, sample_rate: cpal::SampleRate(48_000), buffer_size: cpal::BufferSize::Default },
            device_ch: 2, device_sr: 48_000,
            consumer: Arc::new(Mutex::new(rx)), playing: Arc::new(AtomicBool::new(true)), stats: Arc::new(AudioStats::default()),
            drain_flag: Arc::new(AtomicBool::new(false)), failed: Arc::new(AtomicBool::new(false)),
            scrub_live: Arc::new(AtomicBool::new(false)), unplayed: Arc::new(AtomicU64::new(0)),
            dvs: DvsParts::idle(),
        };
        set_output_gain_db(0.0);
        let mut fill = OutputFill::new(&parts);
        let mut buf = vec![0f32; 960];   // 10 ms of stereo
        let mut run = |fill: &mut OutputFill, tx: &mut rtrb::Producer<f32>| {
            for _ in 0..buf.len() { tx.push(0.8).unwrap(); }
            fill.fill(&mut buf);
            buf.clone()
        };
        assert!(run(&mut fill, &mut tx).iter().all(|&s| s == 0.8), "0 dB: untouched");

        set_output_gain_db(-6.0);
        let ramp = run(&mut fill, &mut tx);
        let half = 0.8 * 10f32.powf(-6.0 / 20.0);
        for w in ramp.chunks(2).collect::<Vec<_>>().windows(2) { assert!(w[1][0] <= w[0][0] + 1e-6, "falls, never jumps up"); }
        let steps: f32 = ramp.chunks(2).collect::<Vec<_>>().windows(2).map(|w| (w[0][0] - w[1][0]).abs()).fold(0.0, f32::max);
        assert!(steps < 0.01, "no step bigger than a sample's share of the ramp: {steps}");
        assert!((ramp[ramp.len() - 1] - half).abs() < 1e-3, "ends at −6 dB");
        assert!(run(&mut fill, &mut tx).iter().all(|&s| (s - half).abs() < 1e-5), "then holds at −6 dB");

        set_output_gain_db(0.0);
        let up = run(&mut fill, &mut tx);
        assert!(up[0] < 0.8 && (up[up.len() - 1] - 0.8).abs() < 1e-5, "back up over one buffer");
        // A boost is never applied: anything above 0 dB is 0 dB.
        set_output_gain_db(6.0);
        assert_eq!(output_gain(), 1.0);
        set_output_gain_db(0.0);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod no_device_tests {
    use super::*;

    /// A deck that boots without its sound interface (the Pi appliance with
    /// the USB card unplugged) still runs: silent, keeping real time, so the
    /// screen comes up and a track plays on; before, the app exited and the
    /// service restarted it until systemd gave up.
    #[test]
    fn starts_silent_without_a_device_and_keeps_time() {
        let sr = 44_100;
        let samples = vec![0.1f32; sr as usize * 2 * 4];   // 4 s stereo
        let h = AudioHandle::start(Some((samples, sr, 2)), TrackTags::default(), Output::Waiting).unwrap();
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

#[cfg(test)]
mod back_pressure_tests {
    use super::*;

    /// At normal speed the processor waits for room for two blocks, not the
    /// 0.25x worst case, so the ring fills far enough for an ALSA device that
    /// asks for two periods at once; slow speeds keep the worst-case room.
    #[test]
    fn the_ring_fills_further_at_normal_speed() {
        assert_eq!(back_pressure_slots(1.0, 2), 2_048);
        assert_eq!(back_pressure_slots(0.25, 2), BACK_PRESSURE_SLOTS, "worst case kept");
        assert_eq!(back_pressure_slots(0.5, 2), BACK_PRESSURE_SLOTS);
        assert!(back_pressure_slots(1.08, 2) < 2_048, "faster: smaller blocks out");
        // At 1x a 93 ms ring then holds more than two of a UCA202's periods.
        assert!(RING_BUFFER_SAMPLES - back_pressure_slots(1.0, 2) > 2 * 1_103 * 2);
    }

    /// The Pi's dropouts, end to end: a deck playing at 1x into a stand-in
    /// UCA202 whose callback takes two of its 1 103-frame periods at once
    /// (4 412 samples every 50 ms).  With the ring kept only half full this
    /// starved by ~232 samples on every such pull; now it never runs dry.
    #[test]
    fn a_device_taking_two_periods_at_once_never_starves() {
        let sr = 44_100;
        let samples = vec![0.1f32; sr as usize * 2 * 8];   // 8 s stereo
        let h = AudioHandle::start(Some((samples, sr, 2)), TrackTags::default(),
                                   Output::TestPulls { sr, frames: 2 * 1_103, every_us: 50_000, trace: None }).unwrap();
        h.set_playing(true);
        std::thread::sleep(Duration::from_millis(700));    // past the start-up fill
        let before = h.stats.underrun_events.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(2500));
        let after = h.stats.underrun_events.load(Ordering::Relaxed);
        assert_eq!(after - before, 0, "underruns while playing steadily: {}", after - before);
    }
}

/// Inside a plugin host (VST3 / CLAP): the host's audio callback pulls the
/// deck's output instead of a sound card.
#[cfg(test)]
mod host_output_tests {
    use super::*;

    /// A host pulling 512-frame blocks at real time, as Reaper does.
    fn pull(out: &mut HostOutput, blocks: usize) -> f32 {
        let mut buf = vec![0f32; 512 * 2];
        let mut peak = 0f32;
        for _ in 0..blocks {
            out.fill(&mut buf);
            peak = buf.iter().fold(peak, |m, v| m.max(v.abs()));
            thread::sleep(Duration::from_micros(512 * 1_000_000 / 44_100));
        }
        peak
    }

    /// Load a track into a host-pulled deck and press PLAY: the host hears
    /// it, at the host's rate; pause: the host hears silence.
    #[test]
    fn a_plugin_host_hears_the_deck_it_hosts() {
        let _gain = GAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_output_gain_db(0.0);
        let slot: HostSlot = Arc::new(Mutex::new(None));
        let deck = AudioHandle::open_host(44_100, &slot, None).unwrap();
        assert_eq!(deck.sample_rate, 44_100, "the deck runs at the host's rate");
        let mut out = slot.lock().unwrap().take().expect("the deck leaves its output for the host");
        assert_eq!(pull(&mut out, 10), 0.0, "an empty deck is silent");

        let tone: Vec<f32> = (0..44_100 * 4).flat_map(|i| {
            let s = 0.5 * (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin();
            [s, s]
        }).collect();
        deck.load_samples(Arc::new(tone), 44_100, 2).unwrap();
        deck.set_playing(true);
        assert!(pull(&mut out, 40) > 0.3, "PLAY: the host hears the track");

        deck.set_playing(false);
        pull(&mut out, 10);   // the stop fade and what the ring held
        assert_eq!(pull(&mut out, 10), 0.0, "paused: silence");
    }

    /// Two plugin decks in one process each fill their own output: the host
    /// hears deck A on A's track and silence on empty deck B (the first cut
    /// kept one process-wide slot, so a second deck had nowhere to play).
    #[test]
    fn two_host_decks_each_fill_their_own_output() {
        let _gain = GAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_output_gain_db(0.0);
        let (sa, sb): (HostSlot, HostSlot) = (Arc::new(Mutex::new(None)), Arc::new(Mutex::new(None)));
        let a = AudioHandle::open_host(44_100, &sa, None).unwrap();
        let b = AudioHandle::open_host(44_100, &sb, None).unwrap();
        let (mut oa, mut ob) = (sa.lock().unwrap().take().unwrap(), sb.lock().unwrap().take().unwrap());
        let tone: Vec<f32> = (0..44_100 * 4).flat_map(|i| {
            let s = 0.5 * (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin();
            [s, s]
        }).collect();
        a.load_samples(Arc::new(tone), 44_100, 2).unwrap();
        a.set_playing(true);
        b.set_playing(true);
        let (mut pa, mut pb) = (0f32, 0f32);
        let mut buf = vec![0f32; 512 * 2];
        for _ in 0..40 {
            oa.fill(&mut buf); pa = buf.iter().fold(pa, |m, v| m.max(v.abs()));
            ob.fill(&mut buf); pb = buf.iter().fold(pb, |m, v| m.max(v.abs()));
            thread::sleep(Duration::from_micros(512 * 1_000_000 / 44_100));
        }
        assert!(pa > 0.3, "deck A's output carries A's track ({pa})");
        assert_eq!(pb, 0.0, "deck B's output carries nothing of A's");
    }
}

/// Reverse play's reads.
#[cfg(test)]
mod reverse_tests {
    use super::*;

    /// A quantized turn goes to the grid point nearest what is heard.
    #[test]
    fn a_quantized_turn_takes_the_nearest_grid_point() {
        // Beats every 100 frames from frame 30.
        assert_eq!(grid_nearest(250.0, 30.0, 100.0), 230.0);
        assert_eq!(grid_nearest(290.0, 30.0, 100.0), 330.0);
        assert_eq!(grid_nearest(230.0, 30.0, 100.0), 230.0);
        assert_eq!(grid_nearest(-20.0, 30.0, 100.0), 0.0, "not before the track");
        // SLIP REV turned at 1000, back at 800: lands at 1200 (stereo).
        assert_eq!(mirror(1000, 800, 2, 100_000), 1200);
        assert_eq!(mirror(1000, 800, 2, 1100), 1100, "not past the end");
    }

    /// A mono ramp read backwards: the samples before the cursor, last first.
    #[test]
    fn reads_backwards_from_the_cursor() {
        let s: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let mut buf = Vec::new();
        let end = read_block_rev(&|i| s[i], s.len(), 50, 5, 1, None, &mut buf);
        assert_eq!(buf, vec![49.0, 48.0, 47.0, 46.0, 45.0]);
        assert_eq!(end, 45);
        // Stereo stays interleaved, frame by frame.
        let st: Vec<f32> = (0..20).map(|i| i as f32).collect();
        let end = read_block_rev(&|i| st[i], st.len(), 10, 4, 2, None, &mut buf);
        assert_eq!(buf, vec![8.0, 9.0, 6.0, 7.0]);
        assert_eq!(end, 6);
    }

    /// At the track's start it stops short; inside a loop it wraps to the end.
    #[test]
    fn stops_at_the_start_and_wraps_in_a_loop() {
        let s: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let mut buf = Vec::new();
        let end = read_block_rev(&|i| s[i], s.len(), 3, 10, 1, None, &mut buf);
        assert_eq!((buf.clone(), end), (vec![2.0, 1.0, 0.0], 0));
        let end = read_block_rev(&|i| s[i], s.len(), 22, 5, 1, Some((20, 30)), &mut buf);
        assert_eq!(buf, vec![21.0, 20.0, 29.0, 28.0, 27.0], "loop [20, 30): 20 → 29");
        assert_eq!(end, 27);
        // A cursor before the loop does not wrap into it.
        let end = read_block_rev(&|i| s[i], s.len(), 12, 3, 1, Some((20, 30)), &mut buf);
        assert_eq!((buf.clone(), end), (vec![11.0, 10.0, 9.0], 9));
    }

    /// Reversed, what is heard lies after the cursor: a jump (or a turn of
    /// the lever, a jump of 0) is measured from there.
    #[test]
    fn a_jump_in_reverse_is_measured_after_the_cursor() {
        // 100 frames queued at 1x, 10 inside the stretcher, stereo.
        assert_eq!(jump_target(10_000, 100, 1.0, 10, 0, false, 2, 100_000), 10_000 - 220);
        assert_eq!(jump_target(10_000, 100, 1.0, 10, 0, true, 2, 100_000), 10_000 + 220);
        assert_eq!(jump_target(10_000, 100, 1.0, 10, 1_000, true, 2, 100_000), 11_220);
    }

    /// Turning the lever while a tone plays: no silence and no click.
    #[test]
    fn a_turn_has_no_gap_and_no_click() {
        let _gain = GAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_output_gain_db(0.0);
        let sr = 44_100usize;
        let slot: HostSlot = Arc::new(Mutex::new(None));
        let deck = AudioHandle::open_host(sr as u32, &slot, None).unwrap();
        let mut out = slot.lock().unwrap().take().unwrap();
        let tone: Vec<f32> = (0..sr * 20).flat_map(|i| {
            let s = 0.5 * (i as f32 * 110.0 * std::f32::consts::TAU / sr as f32).sin();
            [s, s]
        }).collect();
        deck.load_samples(Arc::new(tone), sr as u32, 2).unwrap();
        deck.seek(5 * sr as u64 * 2);   // room to play backwards (it stops at the start)
        deck.set_playing(true);
        let mut buf = vec![0f32; 512 * 2];
        let mut rec = Vec::new();
        let mut pull = |n: usize, rec: &mut Vec<f32>| for _ in 0..n {
            out.fill(&mut buf);
            rec.extend(buf.iter().step_by(2));
            thread::sleep(Duration::from_micros(512 * 1_000_000 / 44_100));
        };
        pull(60, &mut rec);
        let p0 = deck.position.load(Ordering::Relaxed);
        rec.clear();
        pull(4, &mut rec);
        deck.set_reverse(true);
        pull(60, &mut rec);
        let p1 = deck.position.load(Ordering::Relaxed);
        assert!(p1 < p0, "reading backwards: {p0} → {p1}");
        let (mut run, mut gap) = (0, 0);
        for s in &rec { if s.abs() < 1e-4 { run += 1; gap = gap.max(run); } else { run = 0; } }
        let at = 4 * 512;
        let step = rec[at - 1..at + 3 * 512].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        eprintln!("turn: longest silence {gap} frames, biggest step {step:.3}");
        assert!(gap < 8, "silence at the turn: {gap} frames");
        assert!(step < 0.05, "click at the turn: step {step}");
    }
}

/// A beat jump while playing: what the listener hears across the jump.
#[cfg(test)]
mod seek_while_playing_tests {
    use super::*;

    /// Play a steady tone through a host-pulled deck, jump 8 beats ahead
    /// mid-play, and measure the longest run of silence and the biggest
    /// sample-to-sample step in the output around the jump.
    fn jump_gap_and_step() -> (usize, f32) {
        let sr = 44_100usize;
        let slot: HostSlot = Arc::new(Mutex::new(None));
        let deck = AudioHandle::open_host(sr as u32, &slot, None).unwrap();
        let mut out = slot.lock().unwrap().take().unwrap();
        // 110 Hz: a step between neighbouring samples is at most ~0.008.
        let tone: Vec<f32> = (0..sr * 20).flat_map(|i| {
            let s = 0.5 * (i as f32 * 110.0 * std::f32::consts::TAU / sr as f32).sin();
            [s, s]
        }).collect();
        deck.load_samples(Arc::new(tone), sr as u32, 2).unwrap();
        deck.set_playing(true);
        let mut buf = vec![0f32; 512 * 2];
        let mut pull = |n: usize, rec: &mut Vec<f32>| for _ in 0..n {
            out.fill(&mut buf);
            rec.extend(buf.iter().step_by(2));
            thread::sleep(Duration::from_micros(512 * 1_000_000 / 44_100));
        };
        let mut warm = Vec::new();
        pull(60, &mut warm);                                   // steady playback
        let mut rec = Vec::new();
        pull(4, &mut rec);
        deck.jump((8.0 * 60.0 / 128.0 * sr as f64) as i64 * 2);
        pull(40, &mut rec);
        let (mut run, mut gap) = (0, 0);
        for s in &rec { if s.abs() < 1e-4 { run += 1; gap = gap.max(run); } else { run = 0; } }
        // The step is measured around the jump: the buffer it lands in, the
        // crossfade on the next and one buffer past it.  (Bungee also fades
        // its first ~50 ms in after any reset, in grain-sized steps; that is
        // the engine's start-up, not the jump.)
        let at = 4 * 512;
        let step = rec[at - 1..at + 3 * 512].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        (gap, step)
    }

    /// The jump stays on time: with the old audio holding on while the new
    /// position re-primes, the new audio comes in exactly as far past the
    /// jump target as the wall clock has moved since the jump.  Samples are
    /// their own frame numbers (old: 0, 1, 2..; new: 10 000, 10 001..), so
    /// the output says which frame of which stream it is playing.
    #[test]
    fn a_jump_lands_where_the_wall_clock_says() {
        let (mut tx, rx) = rtrb::RingBuffer::<f32>::new(1 << 15);
        let parts = StreamParts {
            config: cpal::StreamConfig { channels: 1, sample_rate: cpal::SampleRate(48_000), buffer_size: cpal::BufferSize::Default },
            device_ch: 1, device_sr: 48_000,
            consumer: Arc::new(Mutex::new(rx)), playing: Arc::new(AtomicBool::new(true)), stats: Arc::new(AudioStats::default()),
            drain_flag: Arc::new(AtomicBool::new(false)), failed: Arc::new(AtomicBool::new(false)),
            scrub_live: Arc::new(AtomicBool::new(false)), unplayed: Arc::new(AtomicU64::new(0)),
            dvs: DvsParts::idle(),
        };
        let mut fill = OutputFill { trim: 1.0, ..OutputFill::new(&parts) };
        let _gain = GAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_output_gain_db(0.0);
        let xf = fill.xfade.len();
        let mut buf = vec![0f32; 256];
        for i in 0..4_000 { tx.push(i as f32).unwrap(); }        // old stream, 4 000 frames queued
        fill.fill(&mut buf);                                      // plays 0..256
        parts.drain_flag.store(true, Ordering::Release);          // the jump
        fill.fill(&mut buf);                                      // still old: 256..512, then the hold
        assert_eq!(buf[255], 511.0, "the jump buffer plays on, no silence");
        assert!(!parts.drain_flag.load(Ordering::Acquire), "flush done, the processor may push");
        // The processor is slow to re-prime: one buffer passes on the hold alone.
        fill.fill(&mut buf);
        assert_eq!(buf[0], 512.0, "the hold carries on from where the jump buffer stopped");
        // Now the new audio arrives; the wall clock is 256 frames past the jump.
        for i in 0..4_000 { tx.push(10_000.0 + i as f32).unwrap(); }
        fill.fill(&mut buf);
        let first_pure = buf[xf];                                 // right after the crossfade
        assert_eq!(first_pure, 10_000.0 + (256 + xf) as f32,
                   "lands 256 + crossfade frames past the target, as the clock says");
        set_output_gain_db(0.0);
    }

    /// A click on every beat at 128 BPM; play, jump 8 beats mid-beat, and
    /// return how far the beat after the jump is from where the steady clicks
    /// say it should be (ms; 0 = on the beat).  `relative`: the deck's jump
    /// (resolved on the audio side); otherwise the old way, an absolute seek
    /// from the heard position as the UI reckons it (`position − in_flight`).
    fn jump_beat_error_ms(relative: bool, speed: f32) -> (f64, f64) {
        let sr = 44_100usize;
        let period = sr as f64 * 60.0 / 128.0;
        let slot: HostSlot = Arc::new(Mutex::new(None));
        let deck = AudioHandle::open_host(sr as u32, &slot, None).unwrap();
        let mut out = slot.lock().unwrap().take().unwrap();
        let len = sr * 30;
        let mut track = vec![0f32; len * 2];
        let mut k = 0.0;
        while (k * period) as usize + 220 < len {
            let at = (k * period).round() as usize;
            for i in 0..220 {
                let v = 0.8 * (i as f32 * 1_000.0 * std::f32::consts::TAU / sr as f32).sin();
                track[(at + i) * 2] = v; track[(at + i) * 2 + 1] = v;
            }
            k += 1.0;
        }
        deck.load_samples(Arc::new(track), sr as u32, 2).unwrap();
        deck.speed.store(speed.to_bits(), Ordering::Relaxed);
        deck.key_lock.store(true, Ordering::Relaxed);
        deck.set_playing(true);
        let mut buf = vec![0f32; 512 * 2];
        let mut rec: Vec<f32> = Vec::new();
        let mut pull = |n: usize, rec: &mut Vec<f32>| for _ in 0..n {
            out.fill(&mut buf);
            rec.extend(buf.iter().step_by(2));
            thread::sleep(Duration::from_micros(512 * 1_000_000 / 44_100));
        };
        pull(130, &mut rec);                                   // ~1.5 s, mid-beat
        let jump_frame = rec.len();
        let delta = (8.0 * period).round() as i64 * 2;
        if relative {
            deck.jump(delta);
        } else {
            let heard = deck.position.load(Ordering::Relaxed) as i64 - deck.in_flight.load(Ordering::Relaxed) as i64;
            deck.seek(((heard + delta).max(0) as u64) & !1);
        }
        pull(130, &mut rec);
        // Click onsets: the first loud sample after a quiet stretch.
        let mut onsets: Vec<usize> = Vec::new();
        let mut last_loud: Option<usize> = None;
        for (i, x) in rec.iter().enumerate() {
            if x.abs() > 0.2 {
                if last_loud.map_or(true, |l| i > l + 2_000) { onsets.push(i); }
                last_loud = Some(i);
            }
        }
        // The clicks before the jump, past the first 0.3 s of the stretcher's start-up.
        let before: Vec<usize> = onsets.iter().copied().filter(|&o| o > sr * 3 / 10 && o < jump_frame).collect();
        let after = onsets.iter().copied().find(|&o| o > jump_frame + 1_000).expect("a click after the jump");
        assert!(before.len() >= 2, "clicks before the jump");
        // Where the steady clicks put the beat after the jump: their mean
        // phase on an even grid (one beat of output is the track's beat over
        // the speed).  The spread of the steady clicks about that grid is the
        // stretcher's own (WSOLA places grains up to a ms or two either way
        // off 1x) and comes back as the second value.
        let beat = period / speed as f64;
        let first = before[0] as f64;
        let offs: Vec<f64> = before.iter().map(|&o| { let o = o as f64 - first; o - (o / beat).round() * beat }).collect();
        let phase = first + offs.iter().sum::<f64>() / offs.len() as f64;
        let spread = offs.iter().fold(f64::MIN, |a, &b| a.max(b)) - offs.iter().fold(f64::MAX, |a, &b| a.min(b));
        let n = ((after as f64 - phase) / beat).round();
        let ms = |f: f64| f / sr as f64 * 1000.0;
        (ms(after as f64 - (phase + n * beat)), ms(spread))
    }

    #[test]
    fn a_jump_while_playing_stays_on_the_beat() {
        let _gain = GAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_output_gain_db(0.0);
        let (old, _) = jump_beat_error_ms(false, 1.0);
        for speed in [1.0, 1.08, 0.94] {
            let (new, spread) = jump_beat_error_ms(true, speed);
            eprintln!("beat after the jump at {speed}x: {new:+.2} ms off (steady clicks spread {spread:.2} ms; at 1x an absolute seek from the heard position: {old:+.2} ms)");
            assert!(new.abs() < 1.0 + spread, "at {speed}x the jump lands {new:+.2} ms off the beat");
        }
    }

    #[test]
    fn a_jump_while_playing_has_no_gap_and_no_click() {
        let _gain = GAIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_output_gain_db(0.0);
        let (gap, step) = jump_gap_and_step();
        eprintln!("beat jump: longest silence {gap} frames ({:.1} ms), biggest step {step:.3}", gap as f64 / 44.1);
        // A 110 Hz tone crosses zero, so a few silent samples are natural.
        assert!(gap < 8, "silence across the jump: {gap} frames");
        assert!(step < 0.05, "click across the jump: step {step}");
    }
}
