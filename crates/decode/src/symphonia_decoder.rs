use opendeck_types::{DecodeError, Decoder};
use std::io::Cursor;
use std::path::Path;
use symphonia::core::{
    audio::SampleBuffer,
    codecs::DecoderOptions,
    formats::{FormatOptions, SeekMode, SeekTo},
    io::MediaSourceStream,
    meta::{MetadataOptions, StandardTagKey, Tag},
    probe::Hint,
    units::Time,
};

/// The track's own metadata (ID3 / Vorbis comment / MP4 atoms), read at probe
/// time.  Every field is optional; empty means the file didn't say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackTags {
    pub title:   Option<String>,
    pub artist:  Option<String>,
    pub album:   Option<String>,
    pub genre:   Option<String>,
    pub year:    Option<String>,
    pub comment: Option<String>,
    /// Musical key as tagged (ID3 TKEY / "INITIALKEY"), e.g. "Am", "8A".
    pub key:     Option<String>,
    /// BPM as tagged by the producer/DJ software (not our analysis).
    pub bpm:     Option<f32>,
}

impl TrackTags {
    /// Fold a metadata revision's tags in; earlier values win, so call with the
    /// most authoritative source first.
    fn absorb(&mut self, tags: &[Tag]) {
        let set = |slot: &mut Option<String>, v: &Tag| {
            let s = v.value.to_string();
            if slot.is_none() && !s.trim().is_empty() { *slot = Some(s.trim().to_string()); }
        };
        for t in tags {
            match t.std_key {
                Some(StandardTagKey::TrackTitle) => set(&mut self.title,   t),
                Some(StandardTagKey::Artist)     => set(&mut self.artist,  t),
                Some(StandardTagKey::Album)      => set(&mut self.album,   t),
                Some(StandardTagKey::Genre)      => set(&mut self.genre,   t),
                Some(StandardTagKey::Date)       => set(&mut self.year,    t),
                Some(StandardTagKey::Comment)    => set(&mut self.comment, t),
                Some(StandardTagKey::Bpm) => {
                    if self.bpm.is_none() { self.bpm = t.value.to_string().trim().parse().ok(); }
                }
                _ => {
                    // No standard key for musical key; catch the common raw ones.
                    let k = t.key.to_ascii_uppercase();
                    if k == "TKEY" || k == "INITIALKEY" || k == "KEY" { set(&mut self.key, t); }
                }
            }
        }
    }
}

pub struct SymphoniaDecoder {
    format:      Box<dyn symphonia::core::formats::FormatReader>,
    decoder:     Box<dyn symphonia::core::codecs::Decoder>,
    track_id:    u32,
    sample_rate: u32,
    channels:    u8,
    total_frames: Option<u64>,
    sample_buf:  Option<SampleBuffer<f32>>,
    /// Samples of `sample_buf` already handed out; a packet larger than the
    /// caller's buffer is delivered across calls rather than truncated.
    buf_pos:     usize,
    tags:        TrackTags,
}

impl SymphoniaDecoder {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let file = std::fs::File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        Self::from_source(mss, hint)
    }

    /// Decode from an in-memory buffer (e.g. a track read over NFS from a linked
    /// player). `ext` primes the format probe (e.g. "mp3", "m4a").
    pub fn open_bytes(bytes: Vec<u8>, ext: Option<&str>) -> Result<Self, DecodeError> {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let mut hint = Hint::new();
        if let Some(e) = ext { hint.with_extension(e); }
        Self::from_source(mss, hint)
    }

    fn from_source(mss: MediaSourceStream, hint: Hint) -> Result<Self, DecodeError> {
        let meta_opts = MetadataOptions::default();
        let fmt_opts = FormatOptions { enable_gapless: true, ..Default::default() };

        let mut probed = symphonia::default::get_probe()
            .format(&hint, mss, &fmt_opts, &meta_opts)
            .map_err(|e| DecodeError::UnsupportedFormat(e.to_string()))?;

        // Tags: container-level metadata found while probing (ID3v2 ahead of an
        // MP3 stream lands here) first, then format-level (Vorbis comments,
        // MP4 atoms).  Read now — it's just the header, and the reader may not
        // re-surface it once decoding starts.
        let mut tags = TrackTags::default();
        if let Some(mut md) = probed.metadata.get() {
            if let Some(rev) = md.skip_to_latest() { tags.absorb(rev.tags()); }
        }
        let mut format = probed.format;
        if let Some(rev) = format.metadata().skip_to_latest() { tags.absorb(rev.tags()); }

        let track = format.default_track()
            .ok_or_else(|| DecodeError::UnsupportedFormat("no default track".into()))?;

        let track_id   = track.id;
        let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
        let channels   = track.codec_params.channels
            .map(|c| c.count() as u8)
            .unwrap_or(2);
        let total_frames = track.codec_params.n_frames;

        let dec_opts = DecoderOptions::default();
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &dec_opts)
            .map_err(|e| DecodeError::Codec(e.to_string()))?;

        Ok(Self {
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            total_frames,
            sample_buf: None,
            buf_pos: 0,
            tags,
        })
    }

    /// The track's own metadata, as read at open.
    pub fn tags(&self) -> &TrackTags { &self.tags }
}

impl Decoder for SymphoniaDecoder {
    fn decode(&mut self, out: &mut [f32]) -> Result<usize, DecodeError> {
        let ch = self.channels as usize;
        loop {
            // Drain what is left of the last packet first.  Dropping the
            // remainder (as this once did) silently lost ~11 % of every FLAC
            // file, whose 4608-frame blocks overflow a 4096-sample buffer.
            if let Some(buf) = &self.sample_buf {
                let samples = buf.samples();
                if self.buf_pos < samples.len() {
                    let mut n = (samples.len() - self.buf_pos).min(out.len());
                    n -= n % ch;   // whole frames only
                    out[..n].copy_from_slice(&samples[self.buf_pos..self.buf_pos + n]);
                    self.buf_pos += n;
                    return Ok(n / ch);
                }
            }

            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(0); // EOF
                }
                Err(e) => return Err(DecodeError::Codec(e.to_string())),
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            // Skip a packet the codec can't decode (e.g. an MP3 bit-reservoir
            // desync: "invalid main_data offset") rather than aborting the whole
            // track — a few lost frames beat a track that won't load at all.
            let decoded = match self.decoder.decode(&packet) {
                Ok(d)  => d,
                Err(e) => { log::debug!("skipping undecodable packet: {e}"); continue; }
            };

            let spec   = *decoded.spec();
            let frames = decoded.frames();
            // Vorbis's first packet decodes to zero frames; symphonia's
            // SampleBuffer panics on a zero-length copy, so skip such packets.
            if frames == 0 { continue; }
            let needed = frames * spec.channels.count();

            // Reallocate if the buffer is absent or too small for this packet.
            if self.sample_buf.as_ref().map_or(true, |b| b.capacity() < needed) {
                self.sample_buf = Some(SampleBuffer::<f32>::new(frames as u64, spec));
            }
            let buf = self.sample_buf.as_mut().unwrap();
            buf.copy_interleaved_ref(decoded);
            self.buf_pos = 0;
        }
    }

    fn seek(&mut self, sample: u64) -> Result<u64, DecodeError> {
        // Anything still buffered from before the seek is stale.
        self.sample_buf = None;
        self.buf_pos = 0;
        let ts = sample as f64 / self.sample_rate as f64;
        let pos = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time { time: Time::from(ts), track_id: Some(self.track_id) },
        )
        .map_err(|e| DecodeError::Codec(e.to_string()))?;
        self.decoder.reset();
        Ok(pos.actual_ts)
    }

    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn channels(&self)    -> u8  { self.channels }
    fn total_frames(&self) -> Option<u64> { self.total_frames }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16-bit stereo WAV of `frames` frames of a ramp, built by hand.
    fn wav(frames: usize) -> Vec<u8> {
        let data_len = (frames * 4) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF"); b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt "); b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());          // PCM
        b.extend_from_slice(&2u16.to_le_bytes());          // stereo
        b.extend_from_slice(&44100u32.to_le_bytes());
        b.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes()); b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data"); b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames {
            let v = (i % 1000) as i16;
            b.extend_from_slice(&v.to_le_bytes()); b.extend_from_slice(&(-v).to_le_bytes());
        }
        b
    }

    /// Packets larger than the caller's buffer must be delivered in full
    /// across calls, never truncated (this once lost ~11 % of every FLAC).
    #[test]
    fn small_output_buffer_loses_nothing() {
        let frames = 20_000;
        let mut d = SymphoniaDecoder::open_bytes(wav(frames), Some("wav")).unwrap();
        let mut out = vec![0f32; 300];   // far smaller than a WAV packet, not a multiple of 4
        let mut got = Vec::new();
        loop {
            let n = d.decode(&mut out).unwrap();
            if n == 0 { break; }
            got.extend_from_slice(&out[..n * 2]);
        }
        assert_eq!(got.len(), frames * 2);
        // Sample order survived the split: the ramp is intact.
        for (i, f) in got.chunks(2).enumerate() {
            let v = (i % 1000) as f32 / 32768.0;
            assert!((f[0] - v).abs() < 1e-4 && (f[1] + v).abs() < 1e-4, "frame {i}");
        }
    }
}
