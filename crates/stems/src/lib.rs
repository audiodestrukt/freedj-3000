//! Reads stems that OverCue Desktop writes to a rekordbox USB.
//!
//! The format is OverCue's open spec, MIT-licensed:
//! <https://github.com/OverCue-gg/overcue-stems-format> (`FORMAT.md`). In short:
//!
//! - `/CDJMODS/index.json` maps rekordbox track IDs (and file paths) to a
//!   bank.
//! - `/CDJMODS/index-v2/` holds the same entries in hashed buckets, so one
//!   track can be read without the whole index.
//! - Each bank is a folder of `.pgz` files, one per role (vocal, drums,
//!   harmonics, full mix). Each file is cut into pages that are FLAC
//!   (or legacy zlib) streams of 16-bit stereo PCM.
//!
//! Everything on the stick is untrusted. Every size, offset and hash the
//! spec lists is checked before audio is handed out, and a failed check is
//! an error, never a guess.  Media is read through [`Media`], so a local
//! stick and a linked player's NFS export share this code.  Nothing here
//! writes to the stick; [`synth`] builds test sticks in a temp dir.

pub mod decimate;
pub mod index;
pub mod media;
pub mod pgz;
pub mod synth;
pub mod v2;

pub use index::{Entry, Index, Namespace};
pub use media::{DirMedia, Media};
pub use pgz::RoleReader;

/// Uncompressed bytes per page; the spec and every writer use 128 KiB.
pub const PAGE_BYTES: u32 = 131_072;
/// Readers accept at most this many pages per role (~23 min at 96 kHz).
pub const MAX_PAGES: u32 = 4096;
/// Largest `index.json` the spec allows.
pub const INDEX_MAX: u64 = 256 * 1024 * 1024;

/// One stored audio selection.  Pair roles (instrumental, vocals+drums…)
/// are sums of these and are never read: the spec says to form them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role { Vocal, Drums, Harmonics, FullMix }

impl Role {
    pub const ALL: [Role; 4] = [Role::Vocal, Role::Drums, Role::Harmonics, Role::FullMix];
    /// The three stems that add up to the track; `FullMix` is a reference copy.
    pub const STEMS: [Role; 3] = [Role::Drums, Role::Vocal, Role::Harmonics];

    /// The role's name in file names (`stems-sidecar-<name>.s16le.pgz`).
    pub fn file_name(self) -> &'static str {
        match self { Role::Vocal => "vocal", Role::Drums => "drums", Role::Harmonics => "harmonics", Role::FullMix => "full-mix" }
    }
    /// The role's prefix in index hash keys (`<key>_sha256`).
    pub fn key(self) -> &'static str {
        match self { Role::Vocal => "vocal", Role::Drums => "drums", Role::Harmonics => "harmonics", Role::FullMix => "full_mix" }
    }
}

/// Lowercase hex of a digest.
pub(crate) fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

/// SHA-256 of `data`.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}
