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
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opendeck_decode::{SymphoniaDecoder, TrackTags};
use opendeck_timestretch::TimestretechStage;
use opendeck_types::{Decoder, PipelineStage};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc,
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
    /// Real-time glitch counters (underruns / dropped frames).
    pub stats:       Arc<AudioStats>,
    pub sample_rate: u32,
    pub channels:    u8,
    /// Tags of the track decoded at open (the startup track); the deck copies
    /// these and replaces them on each browser LOAD.
    pub tags:        TrackTags,
    _stream:     cpal::Stream,
    _processor:  thread::JoinHandle<()>,
}

impl AudioHandle {
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
        let (decoded, file_sr, file_ch, playing_init) = match initial {
            Some((d, sr, ch)) => (d, sr, ch, true),
            None              => (Vec::new(), device_sr, device_ch, false),
        };
        if !decoded.is_empty() && device_sr != file_sr {
            log::warn!(
                "device sample rate {}Hz != file sample rate {}Hz — pitch will be wrong \
                 (resampling not yet implemented)",
                device_sr, file_sr,
            );
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

        // ── 4. Ring buffer ─────────────────────────────────────────────────────
        let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(RING_BUFFER_SAMPLES);
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
                    file_sr,
                    file_ch,
                    device_ch,
                    producer,
                    proc_stats,
                );
            })
            .context("failed to spawn processor thread")?;

        // ── 6. cpal stream (RT callback, no allocation) ────────────────────────
        let cpal_playing    = Arc::clone(&playing);
        let cpal_stats      = Arc::clone(&stats);
        let stream = device
            .build_output_stream::<f32, _, _>(
                &stream_config,
                move |out: &mut [f32], _info| {
                    // On seek, flush stale buffered audio.  This same buffer will
                    // then fill with silence until the producer re-primes from the
                    // new position — expected, so attribute it to seek priming,
                    // not to starvation.
                    let draining = drain_flag.swap(false, Ordering::AcqRel);
                    if draining {
                        while consumer.pop().is_ok() {}
                    }

                    if !cpal_playing.load(Ordering::Relaxed) {
                        out.fill(0.0);
                        return;
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
                |err| log::error!("audio stream error: {err}"),
                None,
            )
            .context("failed to build output stream")?;

        stream.play().context("failed to start audio stream")?;
        log::info!("audio playback started (R3 timestretch pipeline)");

        Ok(Self {
            samples,
            position,
            playing,
            speed,
            key_lock,
            loop_start,
            loop_end,
            loop_active,
            source_consumed,
            in_flight,
            seek_request,
            stats,
            sample_rate: file_sr,
            channels: file_ch as u8,
            tags,
            _stream: stream,
            _processor: processor,
        })
    }
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
    if let Some((ls, le)) = lp {
        let pos = if pos >= le { ls } else { pos };
        if pos + want > le {
            let tail = le - pos;
            let head = (want - tail).min(le - ls);
            scratch.clear();
            scratch.extend_from_slice(&samples[pos..le]);
            scratch.extend_from_slice(&samples[ls..ls + head]);
            return (scratch.as_slice(), ls + head);
        }
        return (&samples[pos..pos + want], pos + want);
    }
    let end = (pos + want).min(samples.len());
    (&samples[pos..end], end)
}

#[cfg(test)]
mod loop_read_tests {
    use super::read_block;

    fn ramp(n: usize) -> Vec<f32> { (0..n).map(|i| i as f32).collect() }

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
    sample_rate: u32,
    file_ch:    usize,
    device_ch:  usize,
    mut producer: rtrb::Producer<f32>,
    stats:      Arc<AudioStats>,
) {
    let mut stretcher = TimestretechStage::new(sample_rate, file_ch as u8);

    // ── Pre-roll: push silence to warm up the RubberBand engine ──────────────
    // The R3 engine needs latency_frames of input before it produces output.
    // Without this, the first ~100ms of playback is silent (ring buffer stays
    // empty while RubberBand fills its internal pipeline).
    {
        let latency = stretcher.latency_frames();
        let silence = vec![0.0f32; latency * file_ch];
        let mut warmup = Vec::new();
        stretcher.process(&silence, &mut warmup);
        log::debug!("proc: pre-rolled {latency} silence frames");
    }

    // Internal read cursor — the processor owns this, UI/HID may jump `position`
    // to trigger a seek.
    let mut proc_pos: u64 = 0;

    // Output buffer for timestretched (file_ch interleaved) audio.
    let mut ts_out: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * file_ch * 8);
    // Scratch for a source block that wraps the loop boundary (tail + head).
    let mut loop_buf: Vec<f32> = Vec::with_capacity(BLOCK_FRAMES * file_ch);

    loop {
        // ── Pause handling ────────────────────────────────────────────────────
        if !playing.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(2));
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
            thread::sleep(Duration::from_millis(2));
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
        let fill_ratio = 1.0 - (free as f32 / RING_BUFFER_SAMPLES as f32);
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
            let ring_out_frames = (RING_BUFFER_SAMPLES - producer.slots()) / device_ch;
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

        lp.consumed.fetch_add(src_block.len() as u64, Ordering::Relaxed);

        let final_block = !looping && src_end >= samples.len();

        // Advance the shared position so the UI/renderer sees it.
        proc_pos = src_end as u64;
        position.store(proc_pos, Ordering::Relaxed);

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
    log::info!(
        "decoded {} frames ({:.1}s) at {}Hz {}ch",
        samples.len() / file_ch,
        samples.len() as f64 / file_ch as f64 / file_sr as f64,
        file_sr, file_ch,
    );
    Ok((samples, file_sr, file_ch, tags))
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

    // Re-interleave.
    let out_frames = out_planar.iter().map(|c| c.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(out_frames * channels);
    for f in 0..out_frames {
        for c in 0..channels { out.push(out_planar[c][f]); }
    }
    log::info!("resampled {}Hz → {}Hz ({} → {} frames)", src_sr, dst_sr, frames, out_frames);
    Ok(out)
}
