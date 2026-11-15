//! A file that is still arriving: a reader for the decoder that waits for
//! bytes not there yet, and the feed the downloader pushes them through.  The
//! decoder can start on the first seconds while the rest downloads.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex};
use symphonia::core::io::MediaSource;

struct Shared { m: Mutex<St>, cv: Condvar, len: u64 }
struct St { buf: Vec<u8>, done: bool, failed: Option<String> }

/// The decoder's side: reads block until the bytes have arrived.
pub struct Growing { sh: Arc<Shared>, pos: u64 }
/// The downloader's side.
pub struct GrowFeed(Arc<Shared>);

/// A growing file of `len` bytes in total.
pub fn growing(len: u64) -> (Growing, GrowFeed) {
    let sh = Arc::new(Shared {
        m: Mutex::new(St { buf: Vec::with_capacity(len as usize), done: false, failed: None }),
        cv: Condvar::new(), len,
    });
    (Growing { sh: sh.clone(), pos: 0 }, GrowFeed(sh))
}

impl GrowFeed {
    /// Append the next bytes. False when the reader is gone (stop downloading).
    pub fn push(&self, bytes: &[u8]) -> bool {
        if Arc::strong_count(&self.0) <= 1 { return false; }
        let mut s = self.0.m.lock().unwrap();
        s.buf.extend_from_slice(bytes);
        if s.buf.len() as u64 >= self.0.len { s.done = true; }
        self.0.cv.notify_all();
        true
    }
    pub fn fail(&self, why: String) {
        self.0.m.lock().unwrap().failed = Some(why);
        self.0.cv.notify_all();
    }
}

impl Read for Growing {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.pos >= self.sh.len { return Ok(0); }
        let mut s = self.sh.m.lock().unwrap();
        loop {
            let have = s.buf.len() as u64;
            if self.pos < have {
                let n = out.len().min((have - self.pos) as usize);
                out[..n].copy_from_slice(&s.buf[self.pos as usize..self.pos as usize + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            if let Some(e) = &s.failed { return Err(io::Error::new(io::ErrorKind::Other, e.clone())); }
            if s.done { return Ok(0); }
            s = self.sh.cv.wait(s).unwrap();
        }
    }
}

impl Seek for Growing {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => self.sh.len as i128 + d as i128,
        };
        if p < 0 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start")); }
        self.pos = (p as u64).min(self.sh.len);
        Ok(self.pos)
    }
}

impl MediaSource for Growing {
    fn is_seekable(&self) -> bool { true }
    fn byte_len(&self) -> Option<u64> { Some(self.sh.len) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_wait_for_the_feed() {
        let (mut r, f) = growing(6);
        let t = std::thread::spawn(move || { f.push(b"abc"); std::thread::sleep(std::time::Duration::from_millis(30)); f.push(b"def"); });
        let mut v = Vec::new();
        r.read_to_end(&mut v).unwrap();
        t.join().unwrap();
        assert_eq!(v, b"abcdef");
    }

    /// A WAV that arrives in pieces decodes from its front before the rest is in.
    #[test]
    fn a_wav_decodes_while_it_arrives() {
        use opendeck_types::Decoder;
        let frames = 44100u32;
        let data_len = frames * 4;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF"); wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt "); wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&44100u32.to_le_bytes()); wav.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes()); wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data"); wav.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..frames { let v = ((i % 100) as i16 - 50) * 100; wav.extend_from_slice(&v.to_le_bytes()); wav.extend_from_slice(&v.to_le_bytes()); }
        let (r, f) = growing(wav.len() as u64);
        let total = wav.len();
        let t = std::thread::spawn(move || {
            for c in wav.chunks(8192) { std::thread::sleep(std::time::Duration::from_millis(2)); if !f.push(c) { break; } }
        });
        let mut d = crate::SymphoniaDecoder::open_growing(r, Some("wav")).expect("opens on the first bytes");
        let mut buf = vec![0f32; 4096 * 2];
        let mut got = 0usize;
        loop { let n = d.decode(&mut buf).unwrap(); if n == 0 { break; } got += n; }
        t.join().unwrap();
        assert_eq!(got, frames as usize, "{total} bytes in");
    }
}
