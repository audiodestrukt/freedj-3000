//! Removable media: where a USB stick lands on each platform, and whether it
//! carries a rekordbox export.  The browser lists every mount as a `USB:` row
//! at its top level and re-scans once a second, so a stick inserted while the
//! app runs appears without a restart, and one pulled out drops away (a loaded
//! track keeps playing: it was fully decoded into memory at LOAD, as on a CDJ).
//!
//! Scan roots:
//! - Linux: `/media/usb` (the appliance automount, deploy/pi/usb-automount),
//!   `/media`, `/media/$USER`, `/run/media/$USER` (udisks2 / desktop
//!   automounters), `/mnt`.  A child counts only when it is a mount point
//!   (its device id differs from the root's), so stray folders are ignored.
//! - macOS: `/Volumes`, same mount-point rule (the boot volume is skipped).
//! - Windows: every drive letter but the system drive.
//! - `OPENDECK_MEDIA_DIRS` (path-list syntax): extra roots whose *every*
//!   child directory is taken as a mount — for custom mount scripts on a
//!   headless Pi, and for tests.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub path:      PathBuf,
    /// What the row says: the mount's directory name (automounters name it
    /// after the volume label) or the drive letter.
    pub label:     String,
    /// `PIONEER/rekordbox/export.pdb` is present.
    pub rekordbox: bool,
}

pub fn has_rekordbox(root: &Path) -> bool {
    root.join("PIONEER").join("rekordbox").join("export.pdb").is_file()
}

/// Every removable mount visible now, sorted by path.
pub fn mounts() -> Vec<Mount> {
    let mut roots: Vec<(PathBuf, bool)> = Vec::new();
    if let Some(extra) = std::env::var_os("OPENDECK_MEDIA_DIRS") {
        roots.extend(std::env::split_paths(&extra).filter(|p| !p.as_os_str().is_empty()).map(|p| (p, true)));
    }
    #[cfg(target_os = "linux")]
    {
        roots.push((PathBuf::from("/media"), false));
        roots.push((PathBuf::from("/media/usb"), false));   // deploy/pi/usb-automount
        if let Ok(user) = std::env::var("USER") {
            roots.push((PathBuf::from("/media").join(&user), false));
            roots.push((PathBuf::from("/run/media").join(&user), false));
        }
        roots.push((PathBuf::from("/mnt"), false));
    }
    #[cfg(target_os = "macos")]
    roots.push((PathBuf::from("/Volumes"), false));

    let mut out = scan(&roots);
    #[cfg(windows)]
    out.extend(windows_drives());
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// Mounts under `roots`; with `any_child` every subdirectory is a mount,
/// otherwise only real mount points (device id differs from the root's).
pub fn scan(roots: &[(PathBuf, bool)]) -> Vec<Mount> {
    let mut out = Vec::new();
    for (root, any_child) in roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        for ent in rd.flatten() {
            let path = ent.path();
            if !path.is_dir() { continue; }
            if !*any_child && !is_mount_point(&path, root) { continue; }
            let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if label.starts_with('.') { continue; }
            out.push(Mount { rekordbox: has_rekordbox(&path), path, label });
        }
    }
    out
}

#[cfg(unix)]
fn is_mount_point(dir: &Path, root: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(dir), std::fs::metadata(root)) {
        (Ok(a), Ok(b)) => a.dev() != b.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn is_mount_point(_dir: &Path, _root: &Path) -> bool { true }

#[cfg(windows)]
fn windows_drives() -> Vec<Mount> {
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_ascii_uppercase();
    (b'A'..=b'Z').filter_map(|c| {
        let letter = format!("{}:", c as char);
        if letter == system { return None; }
        let path = PathBuf::from(format!("{letter}\\"));
        path.is_dir().then(|| Mount { rekordbox: has_rekordbox(&path), path, label: letter })
    }).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The rekordcrate demo export (export.pdb + ANLZ files, no audio).
    pub(crate) fn demo_export() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/rekordcrate/data/complete_export/demo_tracks")
    }

    /// A fake media root with one "stick" that is a copy of the demo export.
    pub(crate) fn fake_media_root(name: &str) -> (tempfile_like::Dir, PathBuf) {
        let root = tempfile_like::Dir::new(name);
        let stick = root.path().join("CDJ1");
        copy_tree(&demo_export(), &stick);
        (root, stick)
    }

    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for ent in std::fs::read_dir(from).unwrap().flatten() {
            let (src, dst) = (ent.path(), to.join(ent.file_name()));
            if src.is_dir() { copy_tree(&src, &dst); } else { std::fs::copy(&src, &dst).unwrap(); }
        }
    }

    /// Minimal scoped temp dir (no tempfile dependency in this crate).
    pub(crate) mod tempfile_like {
        use std::path::{Path, PathBuf};
        pub struct Dir(PathBuf);
        impl Dir {
            pub fn new(name: &str) -> Self {
                let p = std::env::temp_dir().join(format!("opendeck-{name}-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&p);
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
            pub fn path(&self) -> &Path { &self.0 }
        }
        impl Drop for Dir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    }

    #[test]
    fn scan_lists_a_stick_and_sees_its_rekordbox_export() {
        let (root, stick) = fake_media_root("media-scan");
        let found = scan(&[(root.path().to_path_buf(), true)]);
        assert_eq!(found.len(), 1, "one stick");
        assert_eq!(found[0].path, stick);
        assert_eq!(found[0].label, "CDJ1");
        assert!(found[0].rekordbox, "export.pdb detected");

        // A plain folder (no export) is still a mount, just not rekordbox.
        std::fs::create_dir_all(root.path().join("PLAIN")).unwrap();
        let found = scan(&[(root.path().to_path_buf(), true)]);
        assert_eq!(found.len(), 2);
        assert!(!found.iter().find(|m| m.label == "PLAIN").unwrap().rekordbox);
    }

    #[test]
    fn scan_without_any_child_ignores_plain_subfolders() {
        // A subfolder of the temp dir shares its device, so it is not a mount.
        let root = tempfile_like::Dir::new("media-nomount");
        std::fs::create_dir_all(root.path().join("notastick")).unwrap();
        assert!(scan(&[(root.path().to_path_buf(), false)]).is_empty());
    }
}
