//! WSOLA time-stretch — the cheap, time-domain engine (`OPENDECK_STRETCH=wsola`).
//!
//! Waveform-similarity overlap-add, the family SoundTouch (old Mixxx) belongs
//! to: the output is assembled from fixed-length chunks of the input, each
//! chunk's start chosen within a small search window so that its first few
//! milliseconds line up with the tail of the chunk before it, and the two
//! crossfaded.  Tempo comes from how far the input cursor advances per
//! chunk.  No FFT, no windows: a correlation search and a linear crossfade,
//! which is why it fits a one-core board where even Bungee takes most of the
//! CPU.  The price is the WSOLA sound — transients can double or soften at
//! large ratios — so this is the engine for low-powered devices, not the
//! default.
//!
//! Pitch (Master Tempo off) is a rate change after the stretch: the stretch
//! runs at `speed / pitch_scale` and a Hermite resampler reads the stretched
//! stream at `pitch_scale`.  With Master Tempo off the deck sets
//! `pitch = speed`, the stretch ratio is 1, the search is skipped and the
//! engine is a plain varispeed read — what a CDJ does with MT off.
//!
//! Output is metered: after `latency_frames()` of input, exactly
//! `pushed / speed` frames come out per push (the stretch runs a chunk ahead
//! into a FIFO), so the deck's position accounting treats this engine like
//! the other two.

use opendeck_types::PipelineStage;

const SEQUENCE_MS: f32 = 40.0;   // chunk length placed per iteration
const SEEK_MS:     f32 = 15.0;   // search window for the best-matching start
const OVERLAP_MS:  f32 = 8.0;    // crossfade between consecutive chunks

pub struct WsolaStage {
    channels:   usize,
    seq:        usize,   // frames
    seek:       usize,
    ovl:        usize,

    speed:       f32,
    pitch_scale: f64,
    tempo:       f64,    // stretch ratio: input frames consumed per output frame

    /// Input FIFO (interleaved).
    input:      Vec<f32>,
    /// Tail of the last placed chunk (ovl frames, interleaved): what the next
    /// chunk's start is matched against and crossfaded with.
    prev:       Vec<f32>,
    /// Mono scratch for the search: candidates and the previous tail.
    mono_in:    Vec<f32>,
    mono_prev:  Vec<f32>,
    /// Fractional part of the input advance carried between chunks.
    skip_frac:  f64,
    /// Nothing placed since the last reset: the first chunk is copied straight.
    primed:     bool,

    /// Stretched output FIFO (interleaved), read by the resampler.
    stretched:  Vec<f32>,
    /// Resampler read position into `stretched`, in frames.
    read_pos:   f64,

    /// Metering: input frames seen since reset, and output owed vs released.
    in_frames:  u64,
    owed:       f64,
}

impl WsolaStage {
    pub fn new(sample_rate: u32, channels: u8) -> Self {
        let frames = |ms: f32| ((ms / 1000.0) * sample_rate as f32).round() as usize;
        let ch = channels.max(1) as usize;
        let (seq, seek, ovl) = (frames(SEQUENCE_MS), frames(SEEK_MS), frames(OVERLAP_MS));
        Self {
            channels: ch, seq, seek, ovl,
            speed: 1.0, pitch_scale: 1.0, tempo: 1.0,
            input: Vec::with_capacity((seq + seek) * ch * 4),
            prev: Vec::new(),
            mono_in: Vec::with_capacity(seq + seek),
            mono_prev: Vec::with_capacity(ovl),
            skip_frac: 0.0,
            primed: false,
            stretched: Vec::with_capacity(seq * ch * 4),
            read_pos: 0.0,
            in_frames: 0,
            owed: 0.0,
        }
    }

    fn update_tempo(&mut self) {
        self.tempo = (self.speed as f64 / self.pitch_scale).max(1e-3);
    }

    /// Input frames that must be buffered before a chunk can be placed.
    fn need(&self) -> usize { self.seq + self.seek }

    /// Offset (frames, 0..seek) whose first `ovl` frames best continue `prev`.
    /// Normalised cross-correlation on a mono downmix; coarse step then a
    /// fine pass around the best, as SoundTouch does.
    fn best_offset(&mut self) -> usize {
        let (ch, ovl, seek) = (self.channels, self.ovl, self.seek);
        self.mono_in.clear();
        self.mono_in.extend((0..seek + ovl).map(|f| {
            let s = &self.input[f * ch..(f + 1) * ch];
            s.iter().sum::<f32>()
        }));
        self.mono_prev.clear();
        self.mono_prev.extend((0..ovl).map(|f| self.prev[f * ch..(f + 1) * ch].iter().sum::<f32>()));
        let score = |off: usize| -> f32 {
            let cand = &self.mono_in[off..off + ovl];
            let (mut corr, mut energy) = (0.0f32, 0.0f32);
            for (a, b) in cand.iter().zip(&self.mono_prev) {
                corr += a * b;
                energy += a * a;
            }
            corr / (energy.sqrt() + 1e-9)
        };
        const COARSE: usize = 4;
        let mut best = (f32::MIN, 0usize);
        let mut off = 0;
        while off < seek { let s = score(off); if s > best.0 { best = (s, off); } off += COARSE; }
        let lo = best.1.saturating_sub(COARSE - 1);
        let hi = (best.1 + COARSE).min(seek);
        for off in lo..hi { let s = score(off); if s > best.0 { best = (s, off); } }
        best.1
    }

    /// Place one chunk: crossfade the previous tail into it, copy its body,
    /// remember its tail, advance the input cursor by `out_chunk * tempo`.
    fn place_chunk(&mut self) {
        let (ch, seq, ovl) = (self.channels, self.seq, self.ovl);
        let off = if self.primed && (self.tempo - 1.0).abs() > 1e-6 { self.best_offset() } else { 0 };
        let start = off * ch;
        if self.primed {
            // Crossfade prev (fading out) with the chunk's head (fading in).
            for f in 0..ovl {
                let t = (f as f32 + 0.5) / ovl as f32;
                for c in 0..ch {
                    let a = self.prev[f * ch + c];
                    let b = self.input[start + f * ch + c];
                    self.stretched.push(a + (b - a) * t);
                }
            }
        } else {
            // First chunk after a reset: straight copy, full level from the
            // seek point (a CUE press must hit the transient).
            self.stretched.extend_from_slice(&self.input[start..start + ovl * ch]);
            self.primed = true;
        }
        // Body: from the end of the overlap to the end of the sequence minus
        // the next overlap, which becomes `prev`.
        let body_end = start + (seq - ovl) * ch;
        self.stretched.extend_from_slice(&self.input[start + ovl * ch..body_end]);
        self.prev.clear();
        self.prev.extend_from_slice(&self.input[body_end..body_end + ovl * ch]);
        // Advance: (seq - ovl) output frames cost (seq - ovl) * tempo input frames.
        self.skip_frac += (seq - ovl) as f64 * self.tempo;
        let skip = self.skip_frac.floor() as usize;
        self.skip_frac -= skip as f64;
        let skip = skip.min(self.input.len() / ch);
        self.input.drain(..skip * ch);
    }

    /// Pull up to `frames` resampled frames out of `stretched` into `out`.
    fn pull(&mut self, frames: usize, out: &mut Vec<f32>) -> usize {
        let ch = self.channels;
        let avail = self.stretched.len() / ch;
        let mut produced = 0;
        if (self.pitch_scale - 1.0).abs() < 1e-9 {
            // Master Tempo on: no rate change, straight copy.
            let n = frames.min(avail);
            out.extend_from_slice(&self.stretched[..n * ch]);
            self.stretched.drain(..n * ch);
            return n;
        }
        // Hermite (4-point) read at `pitch_scale` frames per output frame.
        while produced < frames {
            let i = self.read_pos.floor() as usize;
            if i + 3 > avail.saturating_sub(1) { break; }   // need i-1 .. i+2
            let t = (self.read_pos - i as f64) as f32;
            for c in 0..ch {
                let at = |k: isize| self.stretched[((i as isize + k).max(0) as usize) * ch + c];
                let (y0, y1, y2, y3) = (at(-1), at(0), at(1), at(2));
                let c0 = y1;
                let c1 = 0.5 * (y2 - y0);
                let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
                let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
                out.push(((c3 * t + c2) * t + c1) * t + c0);
            }
            self.read_pos += self.pitch_scale;
            produced += 1;
        }
        // Drop what the read head has passed, keeping one frame of history.
        let done = (self.read_pos.floor() as usize).saturating_sub(1);
        if done > 0 {
            self.stretched.drain(..done * ch);
            self.read_pos -= done as f64;
        }
        produced
    }
}

impl PipelineStage for WsolaStage {
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        let ch = self.channels;
        let frames = input.len() / ch;
        self.input.extend_from_slice(&input[..frames * ch]);
        // Output owed: everything past the latency, at the current speed.
        let latency = self.latency_frames() as u64;
        let before = self.in_frames;
        self.in_frames += frames as u64;
        let counted = self.in_frames.saturating_sub(latency) - before.saturating_sub(latency);
        self.owed += counted as f64 / self.speed as f64;
        // Stretch while there is a full search window plus chunk to work on.
        while self.input.len() >= self.need() * ch {
            self.place_chunk();
        }
        let want = self.owed.floor() as usize;
        if want > 0 {
            let got = self.pull(want, output);
            self.owed -= got as f64;
        }
    }

    fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.25, 4.0);
        self.update_tempo();
    }

    fn set_pitch_semitones(&mut self, semitones: f32) {
        self.pitch_scale = 2f64.powf(semitones as f64 / 12.0);
        self.update_tempo();
    }

    /// Input needed before the first chunk can be placed.
    fn latency_frames(&self) -> usize { self.need() }

    fn reset(&mut self) {
        self.input.clear();
        self.prev.clear();
        self.stretched.clear();
        self.read_pos = 0.0;
        self.skip_frac = 0.0;
        self.primed = false;
        self.in_frames = 0;
        self.owed = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    /// At 1.0× with Master Tempo on the engine is a delay line: the output is
    /// the input, sample for sample, after the latency.
    #[test]
    fn unity_is_a_pure_delay() {
        let mut s = WsolaStage::new(SR, 2);
        let n = 44_100usize;
        let input: Vec<f32> = (0..n * 2).map(|i| ((i / 2) as f32 * 0.01).sin() * if i % 2 == 0 { 1.0 } else { 0.5 }).collect();
        let mut out = Vec::new();
        for chunk in input.chunks(512 * 2) { s.process(chunk, &mut out); }
        let latency = s.latency_frames();
        assert!((out.len() / 2) as i64 - (n - latency) as i64 >= -2, "frames out {} vs {}", out.len() / 2, n - latency);
        // Output frame k is input frame k (the first chunk starts at frame 0).
        for k in [0usize, 1000, 20_000, 40_000] {
            assert!((out[k * 2] - input[k * 2]).abs() < 1e-6, "frame {k}: {} vs {}", out[k * 2], input[k * 2]);
        }
    }

    /// Master Tempo off (pitch = speed): the stretch ratio is 1 and the output
    /// is a resampled read — the right number of frames, real signal.
    #[test]
    fn varispeed_reads_at_rate() {
        for speed in [0.5f32, 1.08, 2.0] {
            let mut s = WsolaStage::new(SR, 2);
            s.set_speed(speed);
            s.set_pitch_semitones(12.0 * speed.log2());
            assert!((s.tempo - 1.0).abs() < 1e-6);
            let n = 2 * 44_100;
            let input: Vec<f32> = (0..n * 2).map(|i| ((i / 2) as f32 * 0.05).sin()).collect();
            let mut out = Vec::new();
            for chunk in input.chunks(512 * 2) { s.process(chunk, &mut out); }
            let expect = (n - s.latency_frames()) as f32 / speed;
            let got = (out.len() / 2) as f32;
            assert!((got - expect).abs() < 0.01 * expect + 4.0, "@{speed}: {got} frames, expected {expect}");
            let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
            assert!(rms > 0.5, "@{speed}: rms {rms}");
        }
    }
}
