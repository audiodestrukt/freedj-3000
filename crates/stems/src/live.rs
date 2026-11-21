//! **Live banks**: stems a media server serves while its split is still
//! running.  An OpenDeck extension, not part of OverCue's spec, and kept
//! out of the spec's files: OverCue readers never look in `live/`.
//!
//! The spec pages a *finished* bank (each page's SHA-256 is in the role's
//! page table, and the table's hash is in the index, so nothing can be
//! published before every page exists).  A live bank keeps the spec's page
//! geometry and numbering ([`PAGE_BYTES`], 32,768 stereo frames a page) and
//! its `wav-44k1` role files, and adds a page map saying which pages exist
//! yet:
//!
//! ```text
//! /CDJMODS/live/<track id>/
//!     pages                               the map (below); poll it
//!     pages.sha                           per page: 4 × SHA-256, roles in Role::ALL order
//!     stems-sidecar-<role>.s16le.wav      the spec's canonical WAV, full length from the start
//! ```
//!
//! `pages`: a 32-byte header, then one byte per page, `1` once the page is
//! written in every role file and its hashes are in `pages.sha` (the writer
//! sets it last).  Header, big-endian:
//!
//! | Offset | Size | Field |
//! |---:|---:|---|
//! | 0 | 8 | `ODLIVE01` |
//! | 8 | 8 | frames per role (44.1 kHz) |
//! | 16 | 4 | page bytes (131,072) |
//! | 20 | 4 | page count |
//! | 24 | 1 | state: 0 splitting, 1 complete, 2 failed |
//! | 25 | 1 | headroom in dB: the PCM is stored this much below its level |
//! | 26 | 6 | zero |
//!
//! Every role, the full mix included, is 16-bit stereo at 44.1 kHz in the
//! source's own decode timeline (offset 0), so a player without an MP3
//! decoder can play the full mix from the same files.  A reader asks for a
//! track's stems by looking up its folder: the server starts a split when
//! a lookup misses (`opendeck_nfs::server::Tree::on_miss`).

use crate::pgz::wav_header;
use crate::{sha256, Media, Role, MAX_PAGES, PAGE_BYTES};
use anyhow::{bail, ensure, Context, Result};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 8] = b"ODLIVE01";
pub const HEADER: usize = 32;
/// Live banks are always at the separation model's rate.
pub const RATE: u32 = 44_100;
/// Frames in a full page.
pub const PAGE_FRAMES: usize = PAGE_BYTES as usize / 4;
const SHA_ROW: u64 = 32 * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State { Splitting, Complete, Failed }

/// The live folder of track `id`, from the USB root.
pub fn dir(id: u32) -> String { format!("/CDJMODS/live/{id}") }

fn role_file(r: Role) -> String { format!("stems-sidecar-{}.s16le.wav", r.file_name()) }

fn page_count(frames: u64) -> u32 { (frames * 4).div_ceil(PAGE_BYTES as u64) as u32 }

/// A parsed `pages` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Map {
    pub frames: u64,
    pub state: State,
    pub headroom_db: u8,
    pub done: Vec<bool>,
}

impl Map {
    pub fn parse(b: &[u8]) -> Result<Map> {
        ensure!(b.len() >= HEADER && &b[..8] == MAGIC, "live map: bad header");
        let frames = u64::from_be_bytes(b[8..16].try_into().unwrap());
        let page_bytes = u32::from_be_bytes(b[16..20].try_into().unwrap());
        let pages = u32::from_be_bytes(b[20..24].try_into().unwrap());
        ensure!(page_bytes == PAGE_BYTES, "live map: {page_bytes}-byte pages");
        ensure!(frames > 0 && pages == page_count(frames) && pages <= MAX_PAGES, "live map: {pages} pages for {frames} frames");
        ensure!(b.len() == HEADER + pages as usize, "live map: {} bytes for {pages} pages", b.len());
        let state = match b[24] { 0 => State::Splitting, 1 => State::Complete, 2 => State::Failed, s => bail!("live map: state {s}") };
        let done: Vec<bool> = b[HEADER..].iter().map(|v| *v == 1).collect();
        ensure!(state != State::Complete || done.iter().all(|d| *d), "live map: complete with pages missing");
        Ok(Map { frames, state, headroom_db: b[25], done })
    }
    pub fn pages(&self) -> usize { self.done.len() }
    /// Frames in page `p`.
    pub fn page_frames(&self, p: usize) -> usize {
        (self.frames as usize).saturating_sub(p * PAGE_FRAMES).min(PAGE_FRAMES)
    }
    fn encode(&self) -> Vec<u8> {
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(&self.frames.to_be_bytes());
        b.extend_from_slice(&PAGE_BYTES.to_be_bytes());
        b.extend_from_slice(&(self.pages() as u32).to_be_bytes());
        b.push(match self.state { State::Splitting => 0, State::Complete => 1, State::Failed => 2 });
        b.push(self.headroom_db);
        b.extend_from_slice(&[0; 6]);
        b.extend(self.done.iter().map(|d| *d as u8));
        b
    }
}

fn pwrite(f: &mut File, at: u64, data: &[u8]) -> Result<()> {
    f.seek(SeekFrom::Start(at))?;
    f.write_all(data)?;
    Ok(())
}

fn as_bytes(s: &[i16]) -> Vec<u8> { s.iter().flat_map(|v| v.to_le_bytes()).collect() }

/// Writes a live bank under a folder served as `/CDJMODS`.
pub struct LiveWriter {
    pub dir: PathBuf,
    map: Map,
    roles: Vec<File>,
    sha: File,
    pages: File,
}

impl LiveWriter {
    /// Start track `id`'s live bank of `frames` frames under `cdjmods`
    /// (the local folder served as `/CDJMODS`).  It is built under a
    /// temporary name and renamed into place, so a reader finds either
    /// nothing or a whole, empty bank.
    pub fn create(cdjmods: &Path, id: u32, frames: u64, headroom_db: u8) -> Result<LiveWriter> {
        let pages = page_count(frames);
        ensure!(frames > 0 && pages <= MAX_PAGES, "live bank: {frames} frames");
        let pcm = frames * 4;
        ensure!(pcm + 36 <= u32::MAX as u64, "live bank: too long for WAV");
        let live = cdjmods.join("live");
        let tmp = live.join(format!(".{id}.part"));
        let dir = live.join(id.to_string());
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        for r in Role::ALL {
            let mut f = File::create(tmp.join(role_file(r)))?;
            f.write_all(&wav_header(pcm as u32))?;
            f.set_len(44 + pcm)?;
        }
        File::create(tmp.join("pages.sha"))?.set_len(pages as u64 * SHA_ROW)?;
        let map = Map { frames, state: State::Splitting, headroom_db, done: vec![false; pages as usize] };
        std::fs::write(tmp.join("pages"), map.encode())?;
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&tmp, &dir).with_context(|| format!("{}", dir.display()))?;
        let open = |n: &str| std::fs::OpenOptions::new().write(true).open(dir.join(n)).with_context(|| n.to_string());
        let roles = Role::ALL.iter().map(|r| open(&role_file(*r))).collect::<Result<Vec<_>>>()?;
        Ok(LiveWriter { sha: open("pages.sha")?, pages: open("pages")?, roles, map, dir })
    }

    pub fn map(&self) -> &Map { &self.map }

    /// Page `p` of every role (interleaved stereo, [`Role::ALL`] order, each
    /// exactly [`Map::page_frames`] long).  Its map byte is set last.
    pub fn write_page(&mut self, p: usize, roles: [&[i16]; 4]) -> Result<()> {
        ensure!(p < self.map.pages(), "live bank: page {p} of {}", self.map.pages());
        let n = self.map.page_frames(p) * 2;
        let mut row = Vec::with_capacity(SHA_ROW as usize);
        for (f, pcm) in self.roles.iter_mut().zip(roles) {
            ensure!(pcm.len() == n, "live bank: page {p} is {} samples, not {n}", pcm.len());
            let b = as_bytes(pcm);
            row.extend_from_slice(&sha256(&b));
            pwrite(f, 44 + (p * PAGE_BYTES as usize) as u64, &b)?;
        }
        pwrite(&mut self.sha, p as u64 * SHA_ROW, &row)?;
        for f in &mut self.roles { f.flush()?; }
        self.sha.flush()?;
        pwrite(&mut self.pages, (HEADER + p) as u64, &[1])?;
        self.map.done[p] = true;
        Ok(())
    }

    fn set_state(&mut self, s: State) -> Result<()> {
        self.map.state = s;
        pwrite(&mut self.pages, 24, &[match s { State::Splitting => 0, State::Complete => 1, State::Failed => 2 }])
    }
    /// Every page is written.
    pub fn finish(mut self) -> Result<PathBuf> {
        ensure!(self.map.done.iter().all(|d| *d), "live bank: pages missing");
        self.set_state(State::Complete)?;
        Ok(self.dir)
    }
    /// The split gave up: readers stop waiting and keep what they have.
    pub fn fail(mut self) -> Result<()> { self.set_state(State::Failed) }
}

/// Reads a live bank through any [`Media`] (a local folder or NFS).
pub struct LiveReader { base: String, pub map: Map }

impl LiveReader {
    /// Track `id`'s live bank, or `None` when there is none (yet).
    pub fn open(media: &mut dyn Media, id: u32) -> Result<Option<LiveReader>> {
        let base = dir(id);
        let Some(b) = media.read_all(&format!("{base}/pages"), (HEADER + MAX_PAGES as usize) as u64)? else { return Ok(None) };
        let map = Map::parse(&b)?;
        let pcm = map.frames * 4;
        for r in Role::ALL {
            let p = format!("{base}/{}", role_file(r));
            ensure!(media.size(&p)? == Some(44 + pcm), "{p}: not {} bytes", 44 + pcm);
            ensure!(media.read_at(&p, 0, 44)? == wav_header(pcm as u32), "{p}: not the canonical WAV header");
        }
        ensure!(media.size(&format!("{base}/pages.sha"))? == Some(map.pages() as u64 * SHA_ROW), "{base}/pages.sha: wrong size");
        Ok(Some(LiveReader { base, map }))
    }

    /// Read the map again: pages written since, and the state.
    pub fn refresh(&mut self, media: &mut dyn Media) -> Result<&Map> {
        let b = media.read_at(&format!("{}/pages", self.base), 0, HEADER + self.map.pages())?;
        let m = Map::parse(&b)?;
        ensure!(m.frames == self.map.frames, "live map changed length");
        self.map = m;
        Ok(&self.map)
    }

    /// Page `p` of every role ([`Role::ALL`] order), each checked against
    /// its hash.  An error for a page the map doesn't list yet.
    pub fn page(&self, media: &mut dyn Media, p: usize) -> Result<[Vec<i16>; 4]> {
        ensure!(self.map.done.get(p) == Some(&true), "live page {p} not written yet");
        let row = media.read_at(&format!("{}/pages.sha", self.base), p as u64 * SHA_ROW, SHA_ROW as usize)?;
        let n = self.map.page_frames(p) * 4;
        let mut out: [Vec<i16>; 4] = Default::default();
        for (k, r) in Role::ALL.iter().enumerate() {
            let path = format!("{}/{}", self.base, role_file(*r));
            let b = media.read_at(&path, 44 + (p * PAGE_BYTES as usize) as u64, n)?;
            ensure!(sha256(&b)[..] == row[k * 32..k * 32 + 32], "{path}: page {p} checksum mismatch");
            out[k] = b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DirMedia;

    fn page_of(frames: usize, seed: i16) -> Vec<i16> { (0..frames * 2).map(|i| (i as i16).wrapping_mul(7).wrapping_add(seed)).collect() }

    /// A reader sees exactly the pages written so far, in any order, reads
    /// them back as written, and sees the bank complete at the end.
    #[test]
    fn a_reader_follows_a_live_bank_as_it_is_written() {
        let d = tempfile::tempdir().unwrap();
        let mods = d.path().join("CDJMODS");
        let frames = 2 * PAGE_FRAMES as u64 + 1000;   // two full pages and a short one
        let mut m = DirMedia::new(d.path());
        assert!(LiveReader::open(&mut m, 5).unwrap().is_none(), "nothing before the split starts");
        let mut w = LiveWriter::create(&mods, 5, frames, 6).unwrap();
        let mut r = LiveReader::open(&mut m, 5).unwrap().expect("an empty bank is there at once");
        assert_eq!((r.map.pages(), r.map.state, r.map.headroom_db), (3, State::Splitting, 6));
        assert!(r.page(&mut m, 2).is_err(), "not written yet");

        let pages: Vec<[Vec<i16>; 4]> = (0..3).map(|p| std::array::from_fn(|k| page_of(r.map.page_frames(p), (p * 10 + k) as i16))).collect();
        let refs = |p: usize| -> [&[i16]; 4] { std::array::from_fn(|k| pages[p][k].as_slice()) };
        w.write_page(2, refs(2)).unwrap();    // from the playhead: the last page first
        r.refresh(&mut m).unwrap();
        assert_eq!(r.map.done, vec![false, false, true]);
        assert_eq!(r.page(&mut m, 2).unwrap(), pages[2]);
        w.write_page(0, refs(0)).unwrap();
        w.write_page(1, refs(1)).unwrap();
        let dir = w.finish().unwrap();
        assert_eq!(r.refresh(&mut m).unwrap().state, State::Complete);
        for p in 0..3 { assert_eq!(r.page(&mut m, p).unwrap(), pages[p]); }

        // A finished role is the spec's canonical wav-44k1 role, byte for byte.
        let wav = std::fs::read(dir.join("stems-sidecar-drums.s16le.wav")).unwrap();
        assert_eq!(&wav[..44], &wav_header(frames as u32 * 4));
        let drums: Vec<u8> = (0..3).flat_map(|p| as_bytes(&pages[p][1])).collect();
        assert_eq!(&wav[44..], &drums[..]);
    }

    #[test]
    fn a_damaged_page_or_map_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let mods = d.path().join("CDJMODS");
        let mut w = LiveWriter::create(&mods, 9, 1000, 0).unwrap();
        let pcm = page_of(1000, 3);
        w.write_page(0, [&pcm, &pcm, &pcm, &pcm]).unwrap();
        let mut m = DirMedia::new(d.path());
        let r = LiveReader::open(&mut m, 9).unwrap().unwrap();
        assert!(r.page(&mut m, 0).is_ok());
        let f = mods.join("live/9/stems-sidecar-vocal.s16le.wav");
        let mut b = std::fs::read(&f).unwrap();
        b[100] ^= 1;
        std::fs::write(&f, b).unwrap();
        assert!(r.page(&mut m, 0).is_err(), "a changed byte fails its page hash");

        let mut map = std::fs::read(mods.join("live/9/pages")).unwrap();
        assert!(Map::parse(&map).is_ok());
        map[24] = 1; map[HEADER] = 0;
        assert!(Map::parse(&map).is_err(), "complete with a page missing");
        map[20..24].copy_from_slice(&7u32.to_be_bytes());
        assert!(Map::parse(&map).is_err(), "page count must match the frames");
        assert!(Map::parse(b"ODLIVE00").is_err());
    }
}
