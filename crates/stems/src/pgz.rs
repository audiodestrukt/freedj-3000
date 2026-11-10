//! A role's audio: the `.pgz` page container (or a canonical WAV).

use crate::index::{Entry, Layout};
use crate::{sha256, Media, Role, MAX_PAGES};
use anyhow::{anyhow, bail, ensure, Context, Result};

/// Page codec, chosen by the file magic (never by extension or metadata).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// `OVPGZ001`: legacy zlib pages, 96 kHz.
    Zlib,
    /// `OVPGZ003`: FLAC pages, 96 kHz (current CDJ-3000 writer).
    Flac96,
    /// `OVPGZ004`: FLAC pages, 44.1 kHz (experimental CDJ-2000NXS2 profile).
    Flac44,
    /// Plain canonical WAV, 44.1 kHz (experimental CDJ-2000 profile).
    Wav,
}

impl Codec {
    pub fn magic(self) -> &'static [u8; 8] {
        match self { Codec::Zlib => b"OVPGZ001", Codec::Flac96 => b"OVPGZ003", Codec::Flac44 => b"OVPGZ004", Codec::Wav => b"RIFF\0\0\0\0" }
    }
    fn rate(self) -> u32 { match self { Codec::Zlib | Codec::Flac96 => 96_000, Codec::Flac44 | Codec::Wav => 44_100 } }
}

/// One page-table row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page { pub offset: u64, pub compressed: u32, pub expanded: u32, pub sha256: [u8; 32] }

/// A role file whose header and page table have been checked against the
/// index entry.  Pages are read and verified one at a time, so a player
/// can stream them (over LINK too) and use only the ones that check out.
#[derive(Debug)]
pub struct RoleReader {
    pub role: Role,
    pub path: String,
    pub codec: Codec,
    pub page_bytes: u32,
    pub total_bytes: u64,
    pub pages: Vec<Page>,
    full_sha256: [u8; 32],
}

/// The canonical 44-byte WAV header the spec requires for WAV roles.
pub fn wav_header(pcm_bytes: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + pcm_bytes).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&2u16.to_le_bytes());
    h[24..28].copy_from_slice(&44_100u32.to_le_bytes());
    h[28..32].copy_from_slice(&176_400u32.to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&pcm_bytes.to_le_bytes());
    h
}

impl RoleReader {
    /// Open a role: check the header and page table (or WAV header) against
    /// the entry.  Reads only the header and table, not the audio.
    pub fn open(media: &mut dyn Media, entry: &Entry, role: Role) -> Result<RoleReader> {
        let path = entry.role_path(role);
        let (full_sha256, table_sha256) = *entry.hashes.get(&role).ok_or_else(|| anyhow!("no {role:?} hashes"))?;
        let size = media.size(&path)?.with_context(|| format!("{path}: missing"))?;
        let total = entry.role_bytes();
        let ctx = || path.to_string();

        if entry.layout == Layout::Wav {
            let pcm = u32::try_from(total).context("WAV role too large")?;
            let header = media.read_at(&path, 0, 44).with_context(ctx)?;
            ensure!(header == wav_header(pcm) && size == 44 + total, "{path}: not a canonical 44.1 kHz stereo 16-bit WAV of {total} bytes");
            ensure!(sha256(&header) == table_sha256, "{path}: header checksum mismatch");
            // Same page geometry as .pgz, so callers can stream either the same way.
            let pages = (0..total.div_ceil(entry.page_bytes as u64)).map(|i| {
                let start = i * entry.page_bytes as u64;
                let n = (total - start).min(entry.page_bytes as u64) as u32;
                Page { offset: 44 + start, compressed: n, expanded: n, sha256: [0; 32] }
            }).collect();
            return Ok(RoleReader { role, path, codec: Codec::Wav, page_bytes: entry.page_bytes, total_bytes: total, pages, full_sha256 });
        }

        let header = media.read_at(&path, 0, 24).with_context(ctx)?;
        let codec = match &header[..8] {
            b"OVPGZ001" => Codec::Zlib,
            b"OVPGZ003" => Codec::Flac96,
            b"OVPGZ004" => Codec::Flac44,
            m => bail!("{path}: unsupported page container {:?}", String::from_utf8_lossy(m)),
        };
        ensure!(codec.rate() == entry.sample_rate, "{path}: {:?} pages in a {} Hz entry", codec, entry.sample_rate);
        let page_bytes = u32::from_be_bytes(header[8..12].try_into().unwrap());
        let count = u32::from_be_bytes(header[12..16].try_into().unwrap());
        let file_total = u64::from_be_bytes(header[16..24].try_into().unwrap());
        ensure!(page_bytes == entry.page_bytes && file_total == total && count > 0 && count <= MAX_PAGES
                && count as u64 == total.div_ceil(page_bytes as u64),
                "{path}: invalid size or page count");
        let table_end = 24 + 48 * count as u64;
        ensure!(size >= table_end, "{path}: truncated table");
        let table = media.read_at(&path, 24, 48 * count as usize).with_context(ctx)?;
        let mut hashed = header.clone();
        hashed.extend_from_slice(&table);
        ensure!(sha256(&hashed) == table_sha256, "{path}: table checksum mismatch");

        let mut pages = Vec::with_capacity(count as usize);
        let mut next = table_end;
        for (i, row) in table.chunks_exact(48).enumerate() {
            let offset = u64::from_be_bytes(row[0..8].try_into().unwrap());
            let compressed = u32::from_be_bytes(row[8..12].try_into().unwrap());
            let expanded = u32::from_be_bytes(row[12..16].try_into().unwrap());
            let want = (total - i as u64 * page_bytes as u64).min(page_bytes as u64) as u32;
            let limit = if codec == Codec::Zlib { page_bytes as u64 * 2 } else { want as u64 + 65_536 };
            ensure!(offset == next && compressed > 0 && compressed as u64 <= limit && expanded == want
                    && compressed as u64 <= size - offset, "{path}: invalid page {i} bounds");
            pages.push(Page { offset, compressed, expanded, sha256: row[16..48].try_into().unwrap() });
            next += compressed as u64;
        }
        ensure!(next == size, "{path}: trailing data");
        Ok(RoleReader { role, path, codec, page_bytes, total_bytes: total, pages, full_sha256 })
    }

    /// Stereo frames in page `i`.
    pub fn page_frames(&self, i: usize) -> usize { self.pages[i].expanded as usize / 4 }

    /// Page `i` as interleaved stereo i16, verified against its hash.
    /// (WAV roles have no per-page hash; [`read_all`](Self::read_all)
    /// checks the whole role.)
    pub fn page(&self, media: &mut dyn Media, i: usize) -> Result<Vec<i16>> {
        let p = self.pages.get(i).ok_or_else(|| anyhow!("page {i} out of range"))?;
        let bytes = media.read_at(&self.path, p.offset, p.compressed as usize)?;
        let raw = match self.codec {
            Codec::Wav => bytes,
            Codec::Zlib => inflate_page(&bytes, p.expanded as usize).with_context(|| format!("{} page {i}", self.path))?,
            Codec::Flac96 | Codec::Flac44 => flac_page(&bytes, self.codec.rate(), p.expanded as usize)
                .with_context(|| format!("{} page {i}", self.path))?,
        };
        if self.codec != Codec::Wav {
            ensure!(sha256(&raw) == p.sha256, "{} page {i}: checksum mismatch", self.path);
        }
        Ok(raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
    }

    /// Every page, checked, then the whole role against its full hash.
    pub fn read_all(&self, media: &mut dyn Media) -> Result<Vec<i16>> {
        use sha2::Digest;
        let mut out = Vec::with_capacity(self.total_bytes as usize / 2);
        let mut h = sha2::Sha256::new();
        for i in 0..self.pages.len() {
            let page = self.page(media, i)?;
            for s in &page { h.update(s.to_le_bytes()); }
            out.extend_from_slice(&page);
        }
        let got: [u8; 32] = h.finalize().into();
        ensure!(got == self.full_sha256, "{}: full PCM checksum mismatch", self.path);
        Ok(out)
    }
}

/// A zlib page: exactly one stream, no trailing bytes, exactly `want` bytes out.
fn inflate_page(data: &[u8], want: usize) -> Result<Vec<u8>> {
    use flate2::{Decompress, FlushDecompress, Status};
    let mut d = Decompress::new(true);
    let mut out = Vec::with_capacity(want + 1);
    let status = d.decompress_vec(data, &mut out, FlushDecompress::Finish).context("invalid zlib page")?;
    ensure!(status == Status::StreamEnd && d.total_in() as usize == data.len() && out.len() == want,
            "invalid zlib page length or trailing data");
    Ok(out)
}

/// A FLAC page: STREAMINFO is checked by hand first (rate, stereo, 16-bit,
/// exact frame count), then the stream is decoded with no resampling,
/// remix or gain, to exactly `want` bytes of s16le.
fn flac_page(data: &[u8], rate: u32, want: usize) -> Result<Vec<u8>> {
    ensure!(data.len() >= 42 && &data[..4] == b"fLaC" && data[4] & 0x7f == 0 && data[5..8] == [0, 0, 0x22],
            "invalid FLAC STREAMINFO");
    let info = u64::from_be_bytes(data[18..26].try_into().unwrap());
    ensure!(info >> 44 == rate as u64 && (info >> 41) & 7 == 1 && (info >> 36) & 31 == 15
            && info & ((1 << 36) - 1) == want as u64 / 4,
            "FLAC must be {rate} Hz, stereo, 16-bit with the page's exact frame count");

    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{Decoder, DecoderOptions};
    use symphonia::core::formats::{FormatOptions, FormatReader};
    use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(data.to_vec())), MediaSourceStreamOptions::default());
    let mut reader = symphonia::default::formats::FlacReader::try_new(mss, &FormatOptions::default()).context("FLAC page")?;
    let params = reader.default_track().ok_or_else(|| anyhow!("FLAC page has no track"))?.codec_params.clone();
    let mut dec = symphonia::default::codecs::FlacDecoder::try_new(&params, &DecoderOptions { verify: true })?;
    let mut out: Vec<u8> = Vec::with_capacity(want);
    let mut buf: Option<SampleBuffer<i16>> = None;
    loop {
        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e).context("FLAC page"),
        };
        let decoded = dec.decode(&packet).context("FLAC frame")?;
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, *decoded.spec()));
        if b.capacity() < decoded.capacity() * 2 { *b = SampleBuffer::new(decoded.capacity() as u64, *decoded.spec()); }
        b.copy_interleaved_ref(decoded);
        for s in b.samples() { out.extend_from_slice(&s.to_le_bytes()); }
        ensure!(out.len() <= want, "FLAC page decodes past its length");
    }
    ensure!(out.len() == want, "FLAC page decoded {} bytes, expected {want}", out.len());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real writers compress (LPC, stereo decorrelation), unlike our
    /// verbatim test writer.  Encode a page with the reference `flac` tool
    /// when it is installed and decode it here.
    #[test]
    fn decodes_compressed_flac_from_the_reference_encoder() {
        use std::process::{Command, Stdio};
        let frames = 32_768usize;
        let pcm: Vec<u8> = (0..frames).flat_map(|i| {
            let t = i as f64 / 96_000.0;
            let l = (9000.0 * (std::f64::consts::TAU * 220.0 * t).sin() + 3000.0 * (std::f64::consts::TAU * 3_000.0 * t).sin()) as i16;
            let r = (l as f64 * 0.7) as i16 ^ (i as i16 & 3);
            [l.to_le_bytes(), r.to_le_bytes()].concat()
        }).collect();
        // Files, not pipes: on stdout flac cannot go back and fill in the
        // sample count, which the page check requires.
        let d = tempfile::tempdir().unwrap();
        let (raw, enc) = (d.path().join("p.raw"), d.path().join("p.flac"));
        std::fs::write(&raw, &pcm).unwrap();
        let status = Command::new("flac").args(["--silent", "--force-raw-format", "--endian=little", "--sign=signed",
                "--channels=2", "--bps=16", "--sample-rate=96000", "--blocksize=4096", "-8", "--no-md5-sum", "--no-seektable",
                "--no-padding", "-o"]).arg(&enc).arg(&raw).stdout(Stdio::null()).status();
        let Ok(status) = status else { eprintln!("flac not installed; skipped"); return };
        assert!(status.success());
        let out = std::process::Output { status, stdout: std::fs::read(&enc).unwrap(), stderr: vec![] };
        assert!(out.stdout.len() < pcm.len() / 2, "the reference encoder should compress");
        assert_eq!(flac_page(&out.stdout, 96_000, pcm.len()).unwrap(), pcm);
        // Wrong expectations are refused before decoding.
        assert!(flac_page(&out.stdout, 44_100, pcm.len()).is_err());
        assert!(flac_page(&out.stdout, 96_000, pcm.len() - 4).is_err());
    }
}
