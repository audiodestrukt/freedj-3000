//! Access to a stick's files, by path from the USB root ("/CDJMODS/...").

use anyhow::{bail, Context, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

/// Read-only access to a stick.  Paths start with `/` and are relative to
/// the USB root.  Implemented for a local folder ([`DirMedia`]); the app
/// adds one over a linked player's NFS export.
pub trait Media {
    /// The file's size, or `None` when it does not exist.
    fn size(&mut self, path: &str) -> Result<Option<u64>>;
    /// Exactly `len` bytes from `offset`; a short read is an error.
    fn read_at(&mut self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>>;

    /// The whole file, refused above `max` bytes.
    fn read_all(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>> {
        let Some(size) = self.size(path)? else { return Ok(None) };
        if size > max { bail!("{path}: {size} bytes exceeds the {max}-byte limit"); }
        self.read_at(path, 0, size as usize).map(Some)
    }
}

/// Split a stick path into checked components: it must start with `/`,
/// and no component may be empty, `.`, `..`, or carry a separator or NUL.
/// Index strings are untrusted; this keeps them inside the USB root.
pub fn components(path: &str) -> Result<Vec<&str>> {
    let Some(rest) = path.strip_prefix('/') else { bail!("{path:?}: not an absolute stick path") };
    let parts: Vec<&str> = rest.split('/').collect();
    for p in &parts {
        if p.is_empty() || *p == "." || *p == ".." || p.contains('\\') || p.contains('\0') {
            bail!("{path:?}: invalid path component");
        }
    }
    Ok(parts)
}

/// A stick mounted (or copied) at a local folder.  Symbolic links anywhere
/// under the root are refused, as the reference validator does.
pub struct DirMedia { root: PathBuf }

impl DirMedia {
    pub fn new(root: impl Into<PathBuf>) -> Self { DirMedia { root: root.into() } }

    /// The local path, or `None` when a component is missing.
    fn local(&self, path: &str) -> Result<Option<PathBuf>> {
        let mut p = self.root.clone();
        for c in components(path)? {
            p.push(c);
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_symlink() => bail!("{path:?}: symbolic link on the stick"),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e).with_context(|| format!("{}", p.display())),
            }
        }
        Ok(Some(p))
    }
}

impl Media for DirMedia {
    fn size(&mut self, path: &str) -> Result<Option<u64>> {
        match self.local(path)? {
            Some(p) => {
                let m = std::fs::metadata(&p)?;
                Ok(m.is_file().then_some(m.len()))
            }
            None => Ok(None),
        }
    }

    fn read_at(&mut self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
        let p = self.local(path)?.with_context(|| format!("{path}: not found"))?;
        let mut f = std::fs::File::open(&p).with_context(|| format!("{}", p.display()))?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf).with_context(|| format!("{path}: short read at {offset}+{len}"))?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_root() {
        assert!(components("/CDJMODS/index.json").is_ok());
        for bad in ["CDJMODS/x", "/../etc", "/a//b", "/a/./b", "/a/", "/a\\b", "/a\0"] {
            assert!(components(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn dir_media_reads_and_refuses_symlinks() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("CDJMODS")).unwrap();
        std::fs::write(d.path().join("CDJMODS/x"), b"hello").unwrap();
        let mut m = DirMedia::new(d.path());
        assert_eq!(m.size("/CDJMODS/x").unwrap(), Some(5));
        assert_eq!(m.size("/CDJMODS/missing").unwrap(), None);
        assert_eq!(m.read_at("/CDJMODS/x", 1, 3).unwrap(), b"ell");
        assert!(m.read_at("/CDJMODS/x", 3, 3).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path().join("CDJMODS/x"), d.path().join("CDJMODS/link")).unwrap();
            assert!(m.size("/CDJMODS/link").is_err());
        }
    }
}
