//! Key-lock through Bungee Basic (built by build.rs where C++ can be and
//! the `bungee` feature is on).

use crate::bungee_sys;
use crate::BLOCK_FRAMES;
use opendeck_types::PipelineStage;

// ── BungeeStage (key-lock via Bungee Basic) ───────────────────────────────────

/// Key-lock through Bungee's granular API behind a push adapter
/// (`csrc/bungee_shim.cpp`).  Same contract as `TimestretechStage`: feed
/// interleaved frames, collect whatever comes out; `latency_frames()` of
/// input must go in after a reset before anything comes out, and the deck
/// pre-rolls through that exactly as it does for Rubber Band.
pub struct BungeeStage {
    handle:          BungeeHandle,
    channels:        usize,
    speed:           f32,
    pitch_semitones: f32,
    pitch_scale:     f64,
    in_bufs:         Vec<Vec<f32>>,
    in_ptrs:         Vec<*const f32>,
    out_bufs:        Vec<Vec<f32>>,
    out_ptrs:        Vec<*mut f32>,
}

struct BungeeHandle(*mut std::os::raw::c_void);

// SAFETY: as for RbHandle — one owner, all access through `&mut self`.
unsafe impl Send for BungeeHandle {}
unsafe impl Send for BungeeStage {}

impl Drop for BungeeHandle {
    fn drop(&mut self) { unsafe { bungee_sys::od_bungee_destroy(self.0) } }
}

impl BungeeStage {
    /// Slowest speed the output buffers are sized for without reallocating
    /// (the deck's tempo range is ±100% at most, i.e. 0.5×; vinyl-mode
    /// crawl goes lower, and then the buffer simply grows once).
    const MIN_SPEED_PREALLOC: f32 = 0.25;

    pub fn new(sample_rate: u32, channels: u8) -> Self {
        let ch = channels as usize;
        let p = unsafe { bungee_sys::od_bungee_create(sample_rate as i32, ch as i32, BLOCK_FRAMES as i32) };
        assert!(!p.is_null(), "od_bungee_create returned null");
        let version = unsafe { std::ffi::CStr::from_ptr(bungee_sys::od_bungee_version()) }.to_string_lossy().into_owned();
        let out_cap = (BLOCK_FRAMES as f32 / Self::MIN_SPEED_PREALLOC).ceil() as usize + 1;
        let in_bufs:  Vec<Vec<f32>> = vec![vec![0.0f32; BLOCK_FRAMES]; ch];
        let out_bufs: Vec<Vec<f32>> = vec![vec![0.0f32; out_cap]; ch];
        let in_ptrs  = in_bufs.iter().map(|v| v.as_ptr()).collect();
        let out_ptrs = out_bufs.iter().map(|v| v.as_ptr() as *mut f32).collect();
        let stage = Self {
            handle: BungeeHandle(p), channels: ch, speed: 1.0, pitch_semitones: 0.0, pitch_scale: 1.0,
            in_bufs, in_ptrs, out_bufs, out_ptrs,
        };
        log::info!("Bungee {version} initialised — latency: {} frames", stage.latency_frames());
        stage
    }

    fn push_block(&mut self, frames: &[f32], output: &mut Vec<f32>) {
        let n = frames.len() / self.channels;
        debug_assert!(n <= BLOCK_FRAMES);
        if n == 0 { return; }
        for ch in 0..self.channels {
            for i in 0..n {
                self.in_bufs[ch][i] = frames[i * self.channels + ch];
            }
            self.in_ptrs[ch] = self.in_bufs[ch].as_ptr();
        }
        // Steady state yields n / speed frames; the first push after a reset
        // can carry up to half a block more (the owed fraction), so leave room.
        let need = (n as f64 / self.speed as f64).ceil() as usize + 2;
        for ch in 0..self.channels {
            if self.out_bufs[ch].len() < need { self.out_bufs[ch].resize(need, 0.0); }
            self.out_ptrs[ch] = self.out_bufs[ch].as_mut_ptr();
        }
        let got = unsafe {
            bungee_sys::od_bungee_process(
                self.handle.0,
                self.in_ptrs.as_ptr(),
                self.out_ptrs.as_ptr(),
                n as i32,
                self.speed as f64,
                self.pitch_scale,
                need as i32,
            )
        }.max(0) as usize;
        let prev_len = output.len();
        output.resize(prev_len + got * self.channels, 0.0);
        for i in 0..got {
            for ch in 0..self.channels {
                output[prev_len + i * self.channels + ch] = self.out_bufs[ch][i];
            }
        }
    }
}

impl PipelineStage for BungeeStage {
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        let stride = BLOCK_FRAMES * self.channels;
        let mut offset = 0;
        while offset < input.len() {
            let end = (offset + stride).min(input.len());
            self.push_block(&input[offset..end], output);
            offset = end;
        }
    }

    fn set_speed(&mut self, speed: f32) {
        // Stream is forward-only; clamp away from zero so `in / speed` stays finite.
        self.speed = speed.max(1e-3);
    }

    fn set_pitch_semitones(&mut self, semitones: f32) {
        if (semitones - self.pitch_semitones).abs() > 1e-4 {
            self.pitch_semitones = semitones;
            self.pitch_scale = 2f64.powf(semitones as f64 / 12.0);
        }
    }

    fn latency_frames(&self) -> usize {
        unsafe { bungee_sys::od_bungee_latency(self.handle.0) }.max(0.0).round() as usize
    }

    fn reset(&mut self) {
        unsafe { bungee_sys::od_bungee_reset(self.handle.0) }
    }
}
