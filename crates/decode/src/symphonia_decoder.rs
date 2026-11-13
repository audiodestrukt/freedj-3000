use opendeck_types::{DecodeError, Decoder};
use std::io::Cursor;
use std::path::Path;
use symphonia::core::{
    audio::AudioSpec,
    codecs::audio::{AudioDecoder, AudioDecoderOptions},
    formats::{probe::Hint, FormatOptions, FormatReader, SeekMode, SeekTo, TrackType},
    io::MediaSourceStream,
    meta::{MetadataOptions, MetadataRevision, StandardTag, Tag},
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
    /// Embedded cover art (ID3 APIC / FLAC picture / MP4 covr), encoded bytes
    /// as stored (JPEG or PNG), and its MIME type.  Served to other decks as
    /// the track's artwork over Link; not drawn by the deck itself yet.
    pub artwork: Option<(String, Vec<u8>)>,
}

impl TrackTags {
    /// Fold a metadata revision's tags in; earlier values win, so call with the
    /// most authoritative source first.
    fn absorb(&mut self, tags: &[Tag]) {
        // The raw value as written: a BPM tag of "128.5" stays 128.5 (the
        // standard form rounds it to a whole number).
        let set = |slot: &mut Option<String>, v: &Tag| {
            let s = v.raw.value.to_string();
            if slot.is_none() && !s.trim().is_empty() { *slot = Some(s.trim().to_string()); }
        };
        for t in tags {
            match t.std {
                Some(StandardTag::TrackTitle(_)) => set(&mut self.title,   t),
                Some(StandardTag::Artist(_))     => set(&mut self.artist,  t),
                Some(StandardTag::Album(_))      => set(&mut self.album,   t),
                Some(StandardTag::Genre(_))      => set(&mut self.genre,   t),
                // The recording date (ID3 TDRC / TYER, Vorbis DATE, MP4 ©day)
                // is the year, as Symphonia 0.5 read it; a release date only
                // when there is none (`absorb_revision`).
                Some(StandardTag::RecordingDate(_) | StandardTag::RecordingYear(_)) => set(&mut self.year, t),
                Some(StandardTag::Comment(_))    => set(&mut self.comment, t),
                Some(StandardTag::InitialKey(_)) => set(&mut self.key,     t),
                Some(StandardTag::Bpm(_)) => {
                    if self.bpm.is_none() { self.bpm = t.raw.value.to_string().trim().parse().ok(); }
                }
                _ => {
                    // Keys some taggers write under other names, and a BPM
                    // with decimals ("128.5"), which Symphonia's standard
                    // tag (a whole number) leaves unparsed.
                    let k = t.raw.key.to_ascii_uppercase();
                    if k == "TKEY" || k == "INITIALKEY" || k == "KEY" { set(&mut self.key, t); }
                    if (k == "TBPM" || k == "BPM" || k == "TEMPO") && self.bpm.is_none() {
                        self.bpm = t.raw.value.to_string().trim().parse().ok();
                    }
                }
            }
        }
    }

    fn absorb_revision(&mut self, rev: &MetadataRevision) {
        let all = || rev.media.tags.iter().chain(rev.per_track.iter().flat_map(|t| t.metadata.tags.iter()));
        self.absorb(&rev.media.tags);
        self.absorb_visuals(&rev.media.visuals);
        for t in &rev.per_track { self.absorb(&t.metadata.tags); self.absorb_visuals(&t.metadata.visuals); }
        if self.year.is_none() {
            if let Some(t) = all().find(|t| matches!(t.std, Some(StandardTag::ReleaseDate(_) | StandardTag::ReleaseYear(_)))) {
                let v = t.raw.value.to_string();
                if !v.trim().is_empty() { self.year = Some(v.trim().to_string()); }
            }
        }
    }

    /// Keep the first embedded picture (the front cover, in practice).
    fn absorb_visuals(&mut self, visuals: &[symphonia::core::meta::Visual]) {
        if self.artwork.is_none() {
            if let Some(v) = visuals.iter().find(|v| !v.data.is_empty()) {
                self.artwork = Some((v.media_type.clone().unwrap_or_default(), v.data.to_vec()));
            }
        }
    }
}

/// Hold MP3 and Vorbis to full scale.  Symphonia 0.5's decoders for these
/// clipped their output and 0.6's do not; a loud master decodes past 1.0
/// (one test track peaks at 1.55), and the deck's level, waveforms and
/// analysis were tuned on clipped files.  Other codecs were never clipped
/// (AAC, float WAV keep their headroom), so they are left alone.
fn clip(buf: &mut [f32]) {
    for s in buf { *s = s.clamp(-1.0, 1.0); }
}

pub struct SymphoniaDecoder {
    format:      Box<dyn FormatReader>,
    decoder:     Box<dyn AudioDecoder>,
    track_id:    u32,
    sample_rate: u32,
    channels:    u8,
    total_frames: Option<u64>,
    /// The last decoded packet, interleaved.
    sample_buf:  Option<Vec<f32>>,
    /// Samples of `sample_buf` already handed out; a packet larger than the
    /// caller's buffer is delivered across calls rather than truncated.
    buf_pos:     usize,
    tags:        TrackTags,
    /// Rate of the last decoded packet: the rate the audio really has.
    decoded_rate: Option<u32>,
    /// MP3 / Vorbis: clip to full scale (see `clip`).
    clip:        bool,
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

    fn from_source(mss: MediaSourceStream<'static>, hint: Hint) -> Result<Self, DecodeError> {
        let mut format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|e| DecodeError::UnsupportedFormat(e.to_string()))?;

        // Tags: every revision read so far, oldest first (ID3v2 ahead of an
        // MP3 stream, then the container's own: Vorbis comments, MP4 atoms);
        // earlier values win.  Read now — it's just the header.
        let mut tags = TrackTags::default();
        {
            let mut md = format.metadata();
            if let Some(rev) = md.current() { tags.absorb_revision(rev); }
            while md.pop().is_some() {
                if let Some(rev) = md.current() { tags.absorb_revision(rev); }
            }
        }

        let track = format.default_track(TrackType::Audio)
            .ok_or_else(|| DecodeError::UnsupportedFormat("no audio track".into()))?;
        let params = track.codec_params.as_ref().and_then(|p| p.audio())
            .ok_or_else(|| DecodeError::UnsupportedFormat("no audio codec parameters".into()))?
            .clone();

        let track_id   = track.id;
        let sample_rate = params.sample_rate.unwrap_or(44100);
        let header_channels = params.channels.as_ref().map(|c| c.count() as u8);
        let channels   = header_channels.unwrap_or(2);
        use symphonia::core::codecs::audio::well_known::{CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_VORBIS};
        let gapless = params.codec == CODEC_ID_VORBIS;
        // Frames as decoded: the playable count plus the encoder delay and
        // padding we keep (see below).
        let total_frames = track.num_frames.map(|n| if gapless { n } else {
            n + track.delay.unwrap_or(0) as u64 + track.padding.unwrap_or(0) as u64
        });

        // Keep the MP3 encoder delay (no gapless trim): rekordbox and the
        // players decode MP3s that way, so their beat grids and cues are on
        // that timeline. Trimmed, every rekordbox grid and cue sat ~24 ms
        // late against our audio (measured over a 226-track stick: median
        // -22.7 ms trimmed, 0.7 ms untrimmed; AAC was already untrimmed).
        // Vorbis is the exception: its first packet only primes the decoder,
        // and Symphonia 0.5 returned nothing for it; keep that.
        let dec_opts = AudioDecoderOptions::default().gapless(gapless);
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &dec_opts)
            .map_err(|e| DecodeError::Codec(e.to_string()))?;

        let mut me = Self {
            format,
            decoder,
            track_id,
            sample_rate,
            channels,
            total_frames,
            sample_buf: None,
            buf_pos: 0,
            tags,
            decoded_rate: None,
            clip: [CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_VORBIS].contains(&params.codec),
        };
        // No channel count in the header (ALAC in MP4 has none): decode the
        // first packet and take the layout from it. Guessing stereo read a
        // mono file as half as many stereo frames, at double speed.
        if header_channels.is_none() {
            if let Some(spec) = me.prime()? {
                me.channels = spec.channels().count() as u8;
                me.sample_rate = spec.rate();
            }
        }
        Ok(me)
    }

    /// Decode packets until one has audio, leaving it buffered for the first
    /// `decode` call. Returns its spec, or None at end of stream.
    fn prime(&mut self) -> Result<Option<AudioSpec>, DecodeError> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(None),
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(DecodeError::Codec(e.to_string())),
            };
            if packet.track_id != self.track_id { continue; }
            let Ok(decoded) = self.decoder.decode(&packet) else { continue };
            if decoded.frames() == 0 { continue; }
            let spec = decoded.spec().clone();
            let mut buf = Vec::new();
            decoded.copy_to_vec_interleaved(&mut buf);
            if self.clip { clip(&mut buf); }
            self.sample_buf = Some(buf);
            self.buf_pos = 0;
            self.decoded_rate = Some(spec.rate());
            return Ok(Some(spec));
        }
    }

    /// The track's own metadata, as read at open.
    pub fn tags(&self) -> &TrackTags { &self.tags }

    /// Sample rate of the decoded packets so far (None before the first).
    pub fn decoded_rate(&self) -> Option<u32> { self.decoded_rate }
}

impl Decoder for SymphoniaDecoder {
    fn decode(&mut self, out: &mut [f32]) -> Result<usize, DecodeError> {
        let ch = self.channels as usize;
        loop {
            // Drain what is left of the last packet first.  Dropping the
            // remainder (as this once did) silently lost ~11 % of every FLAC
            // file, whose 4608-frame blocks overflow a 4096-sample buffer.
            if let Some(buf) = &self.sample_buf {
                let samples = &buf[..];
                if self.buf_pos < samples.len() {
                    let mut n = (samples.len() - self.buf_pos).min(out.len());
                    n -= n % ch;   // whole frames only
                    out[..n].copy_from_slice(&samples[self.buf_pos..self.buf_pos + n]);
                    self.buf_pos += n;
                    return Ok(n / ch);
                }
            }

            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(0), // EOF
                Err(symphonia::core::errors::Error::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(0); // EOF
                }
                Err(e) => return Err(DecodeError::Codec(e.to_string())),
            };

            if packet.track_id != self.track_id {
                continue;
            }

            // Skip a packet the codec can't decode (e.g. an MP3 bit-reservoir
            // desync: "invalid main_data offset") rather than aborting the whole
            // track — a few lost frames beat a track that won't load at all.
            let decoded = match self.decoder.decode(&packet) {
                Ok(d)  => d,
                Err(e) => { log::debug!("skipping undecodable packet: {e}"); continue; }
            };

            // Vorbis's first packet decodes to zero frames.
            if decoded.frames() == 0 { continue; }
            self.decoded_rate = Some(decoded.spec().rate());
            // Reuse the buffer: copy_to_vec_interleaved resizes it to fit.
            let buf = self.sample_buf.get_or_insert_with(Vec::new);
            decoded.copy_to_vec_interleaved(buf);
            if self.clip { clip(buf); }
            self.buf_pos = 0;
        }
    }

    fn seek(&mut self, sample: u64) -> Result<u64, DecodeError> {
        // Anything still buffered from before the seek is stale.
        self.sample_buf = None;
        self.buf_pos = 0;
        let ts = sample as f64 / self.sample_rate as f64;
        let time = Time::try_from_secs_f64(ts)
            .ok_or_else(|| DecodeError::Codec(format!("cannot seek to {ts} s")))?;
        let pos = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time { time, track_id: Some(self.track_id) },
        )
        .map_err(|e| DecodeError::Codec(e.to_string()))?;
        self.decoder.reset();
        Ok(pos.actual_ts.get().max(0) as u64)
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

    /// A stereo WAV at `sr` from frames of (L, R) in -1..1: `fmt` 1 = integer
    /// PCM at 32 bits, 3 = 32-bit float.
    fn wav32(sr: u32, fmt: u16, frames: &[(f64, f64)]) -> Vec<u8> {
        let data_len = (frames.len() * 8) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF"); b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt "); b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&fmt.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&sr.to_le_bytes());
        b.extend_from_slice(&(sr * 8).to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes()); b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(b"data"); b.extend_from_slice(&data_len.to_le_bytes());
        for &(l, r) in frames {
            for v in [l, r] {
                if fmt == 3 { b.extend_from_slice(&(v as f32).to_le_bytes()); }
                else { b.extend_from_slice(&((v * 2_147_483_647.0).round() as i32).to_le_bytes()); }
            }
        }
        b
    }

    /// 32-bit WAV, integer and float, at 96 kHz: the rate, the length and the
    /// samples come through (studio bounces are often 32-bit float).
    #[test]
    fn thirty_two_bit_wav_integer_and_float() {
        let frames: Vec<(f64, f64)> = (0..9_600)
            .map(|i| { let v = 0.8 * (i as f64 * 0.01).sin(); (v, -v * 0.5) }).collect();
        for fmt in [1u16, 3] {
            let mut d = SymphoniaDecoder::open_bytes(wav32(96_000, fmt, &frames), Some("wav")).unwrap();
            assert_eq!((d.sample_rate(), d.channels()), (96_000, 2), "format {fmt}");
            let mut out = vec![0f32; 4096];
            let mut got = Vec::new();
            loop {
                let n = d.decode(&mut out).unwrap();
                if n == 0 { break; }
                got.extend_from_slice(&out[..n * 2]);
            }
            assert_eq!(got.len(), frames.len() * 2, "format {fmt}");
            for (f, &(l, r)) in got.chunks(2).zip(&frames) {
                assert!((f[0] as f64 - l).abs() < 1e-6 && (f[1] as f64 - r).abs() < 1e-6, "format {fmt}");
            }
        }
    }

    /// 32-bit FLAC (FLAC 1.4+) stereo, coded mid/side with L and R near
    /// full scale and opposite, so the side channel needs all 33 bits.
    /// Symphonia 0.5 refused the file; 0.6.1 truncated the side channel and
    /// got two thirds of the samples wrong (upstream #585, fixed by the
    /// vendored PR #587, third_party/symphonia-bundle-flac).
    #[test]
    fn thirty_two_bit_flac_stereo_decodes_exactly() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/s32_side_96k.flac");
        let mut d = SymphoniaDecoder::open(&path).unwrap();
        assert_eq!((d.sample_rate(), d.channels(), d.total_frames()), (96_000, 2, Some(4800)));
        let mut out = vec![0f32; 4096];
        let mut got = Vec::new();
        loop {
            let n = d.decode(&mut out).unwrap();
            if n == 0 { break; }
            got.extend_from_slice(&out[..n * 2]);
        }
        assert_eq!(got.len(), 4800 * 2);
        for (i, f) in got.chunks(2).enumerate() {
            let v = 0.9 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 96_000.0).sin();
            assert!((f[0] as f64 - v).abs() < 1e-6 && (f[1] as f64 + v).abs() < 1e-6,
                    "frame {i}: {:?}, want ({v}, {})", f, -v);
        }
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
