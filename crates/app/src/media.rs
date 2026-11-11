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
//! - iOS: no app sees a drive by itself.  The user picks it once in the
//!   system folder picker (the "USB: ADD DRIVE…" row, `ADD_DRIVE`); main.m
//!   keeps a bookmark and hands back the drives that are plugged in now.
//! - Android: drives mount under `/storage/XXXX-XXXX`, readable with "All
//!   files access" (fine outside Google Play).  Until it is granted a drive
//!   shows as a row that opens that settings page (`Access::NeedsGrant`).
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
    pub access:    Access,
}

/// Whether a mount can be browsed yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Ready,
    /// Seen, but the OS has to grant the app access first: opening the row
    /// asks for it (`request_access`) instead of browsing.  Android only.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    NeedsGrant,
}

/// iOS: the row that opens the folder picker to add a drive.  None elsewhere.
pub const ADD_DRIVE: Option<&str> = if cfg!(target_os = "ios") { Some("ADD DRIVE…") } else { None };

/// Ask the OS for access to removable drives: the folder picker on iOS, the
/// "All files access" settings page on Android.  Returns at once; a drive
/// the user grants turns up on the next scan.  A no-op on the desktop.
pub fn request_access() {
    log::info!("media: asking the OS for drive access");
    #[cfg(target_os = "ios")]
    unsafe { freedj_usb_pick(); }
    #[cfg(target_os = "android")]
    crate::android::request_all_files_access();
    #[cfg(test)]
    REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) static REQUESTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(target_os = "ios")]
extern "C" {
    /// ios/freedj/main.m: present the folder picker.
    fn freedj_usb_pick();
    /// ios/freedj/main.m: the picked drives plugged in now, one path per
    /// line, into `buf`; returns the length needed (truncated past `cap`).
    fn freedj_usb_paths(buf: *mut u8, cap: usize) -> usize;
}

#[cfg(target_os = "ios")]
fn ios_drives() -> Vec<Mount> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { freedj_usb_paths(buf.as_mut_ptr(), buf.len()) };
    if n > buf.len() {
        buf = vec![0u8; n];
        let m = unsafe { freedj_usb_paths(buf.as_mut_ptr(), buf.len()) };
        buf.truncate(m.min(n));
    } else {
        buf.truncate(n);
    }
    String::from_utf8_lossy(&buf).lines().filter(|l| !l.is_empty()).filter_map(|l| {
        let path = PathBuf::from(l);
        path.is_dir().then(|| Mount {
            label: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            rekordbox: has_rekordbox(&path), path, access: Access::Ready,
        })
    }).collect()
}

/// Android: the volumes under `/storage` other than the internal one.
/// Without all-files access they cannot be read, so they come back as
/// `NeedsGrant` rows.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn android_drives(storage: &Path, granted: bool) -> Vec<Mount> {
    let mut out = scan(&[(storage.to_path_buf(), cfg!(test))]);
    out.retain(|m| m.label != "emulated" && m.label != "self");
    if !granted {
        for m in &mut out { m.access = Access::NeedsGrant; m.rekordbox = false; }
    }
    out
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
    // Unit tests see only OPENDECK_MEDIA_DIRS: a stick plugged into the dev
    // machine must not change what a test finds.
    #[cfg(all(target_os = "linux", not(test)))]
    {
        roots.push((PathBuf::from("/media"), false));
        roots.push((PathBuf::from("/media/usb"), false));   // deploy/pi/usb-automount
        if let Ok(user) = std::env::var("USER") {
            roots.push((PathBuf::from("/media").join(&user), false));
            roots.push((PathBuf::from("/run/media").join(&user), false));
        }
        roots.push((PathBuf::from("/mnt"), false));
    }
    #[cfg(all(target_os = "macos", not(test)))]
    roots.push((PathBuf::from("/Volumes"), false));

    let mut out = scan(&roots);
    #[cfg(all(windows, not(test)))]
    out.extend(windows_drives());
    #[cfg(all(target_os = "ios", not(test)))]
    out.extend(ios_drives());
    #[cfg(all(target_os = "android", not(test)))]
    out.extend(android_drives(Path::new("/storage"), crate::android::all_files_access()));
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
            out.push(Mount { rekordbox: has_rekordbox(&path), path, label, access: Access::Ready });
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
        path.is_dir().then(|| Mount { rekordbox: has_rekordbox(&path), path, label: letter, access: Access::Ready })
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
    fn android_lists_removable_volumes_and_gates_them_on_access() {
        let root = tempfile_like::Dir::new("media-android");
        for d in ["emulated", "self"] { std::fs::create_dir_all(root.path().join(d)).unwrap(); }
        let stick = root.path().join("1A2B-3C4D");
        copy_tree(&demo_export(), &stick);

        let ok = android_drives(root.path(), true);
        assert_eq!(ok.len(), 1, "internal storage is not a drive: {ok:?}");
        assert_eq!((ok[0].label.as_str(), ok[0].access, ok[0].rekordbox), ("1A2B-3C4D", Access::Ready, true));

        let locked = android_drives(root.path(), false);
        assert_eq!(locked.len(), 1);
        assert_eq!(locked[0].access, Access::NeedsGrant);
        assert!(!locked[0].rekordbox, "unreadable until granted");
    }

    #[test]
    fn scan_without_any_child_ignores_plain_subfolders() {
        // A subfolder of the temp dir shares its device, so it is not a mount.
        let root = tempfile_like::Dir::new("media-nomount");
        std::fs::create_dir_all(root.path().join("notastick")).unwrap();
        assert!(scan(&[(root.path().to_path_buf(), false)]).is_empty());
    }
}
