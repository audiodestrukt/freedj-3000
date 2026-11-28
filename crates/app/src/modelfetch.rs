//! The stem model, fetched by the app itself (iOS): no Files-app copying.
//!
//! The first time stems are on and the model is missing, the app downloads
//! one pinned file, HTDemucs converted to Core ML (MIT), from a fixed
//! revision on Hugging Face. The download goes into Application Support
//! (excluded from iCloud backup; it can always be fetched again). Then:
//!
//! 1. its size and SHA-256 are checked against the pinned values, so only
//!    these exact bytes are ever unpacked;
//! 2. it is unzipped as a stream (one 423 MB weights file: never held in
//!    memory), CRC-checked, into a temporary folder;
//! 3. it is renamed into place as `htdemucs.mlpackage`, and the zip is
//!    deleted.
//!
//! The native model loader (`ios/freedj/main.m`) looks there after the
//! OpenDeck folder (a model the user put there still wins), and compiles
//! the package once on first use, as before.
//!
//! The network part is iOS only, through `NSURLSession` in the native shim;
//! everything else (verify, unzip, install, the state machine) is plain Rust
//! and tested on the desktop.

// The fetch runs on iOS only; elsewhere the pieces are built and tested.
#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

use anyhow::{bail, ensure, Context, Result};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

/// A stem model the app can fetch.  Users never pick one: the app takes the
/// best one this device can run ([`pick`]).  More entries (a lighter model
/// for devices with less memory, a better-sounding one for bigger ones)
/// slot in here; a power-user override can come later.
#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    /// For the log.
    pub name: &'static str,
    /// Pinned revision: a new revision needs a new hash here.
    pub url: &'static str,
    pub zip_bytes: u64,
    pub zip_sha256: &'static str,
    /// What the zip unpacks to, and so what is installed.
    pub package: &'static str,
    /// Bytes once installed (for the space check).
    pub installed_bytes: u64,
    /// Memory a split needs above the app's own use, MB (measured).
    pub split_mb: u64,
}

/// Best first.
pub const CATALOG: &[ModelSpec] = &[
    // HTDemucs as Core ML, revision a914c07 of slaughters85j/htdemucs-coreml
    // (MIT).  Memory: see separate::model::SPLIT_NEEDS_MB.
    ModelSpec {
        name: "HTDemucs (Core ML)",
        url: "https://huggingface.co/slaughters85j/htdemucs-coreml/resolve/a914c0702e346a969b1099991f93b3fceb1d38fd/htdemucs.mlpackage.zip",
        zip_bytes: 287_214_824,
        zip_sha256: "753276a6bfe2013cf3b03ab33fd038cb6138e0d933ce985724d7cf385df66a98",
        package: "htdemucs.mlpackage",
        installed_bytes: 425_000_000,
        split_mb: super::separate::model::SPLIT_NEEDS_MB,
    },
];

/// What the app's own use comes to before a split, MB (measured ~550–700).
const APP_BASELINE_MB: u64 = 800;

/// The best model a device can run, given the memory the OS lets the app
/// have in all (footprint + what is left), MB.  None: no model fits, so
/// nothing is downloaded.
pub fn pick(limit_mb: u64) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|m| m.split_mb + APP_BASELINE_MB <= limit_mb)
}

/// The model this build installs today (the native loader looks for it by
/// name).
pub const MODEL: ModelSpec = CATALOG[0];
pub const PACKAGE: &str = MODEL.package;

/// Where the fetch has got, for the stem keys.
#[derive(Clone, Debug, PartialEq)]
pub enum Fetch {
    /// Not needed yet, or not on this platform.
    Idle,
    /// Downloading: bytes so far and the total.
    Downloading { got: u64, total: u64 },
    /// Checking and unpacking.
    Unpacking,
    /// Installed and ready to load.
    Ready,
    /// Gave up for now (no network, no space…); retried later.
    Failed(String),
    /// This device has too little memory for any model: nothing fetched.
    Unsupported,
}

impl Fetch {
    /// Percent downloaded, for "DOWNLOADING n%".
    pub fn percent(&self) -> Option<u8> {
        match self {
            Fetch::Downloading { got, total } if *total > 0 => Some((got * 100 / total).min(99) as u8),
            Fetch::Downloading { .. } => Some(0),
            _ => None,
        }
    }
}

static STATE: Mutex<Fetch> = Mutex::new(Fetch::Idle);
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The fetch's state now.
pub fn state() -> Fetch { STATE.lock().map(|s| s.clone()).unwrap_or(Fetch::Idle) }

fn set(s: Fetch) {
    if let Ok(mut g) = STATE.lock() { *g = s; }
}

/// Is the model installed in `dir`?
pub fn installed(dir: &Path) -> bool {
    dir.join(PACKAGE).join("Data/com.apple.CoreML/weights/weight.bin").is_file()
        && dir.join(PACKAGE).join("Manifest.json").is_file()
}

/// Make sure the model is (or will be) there: starts one background fetch
/// per launch if it is missing.  Cheap to call every time stems are needed.
pub fn ensure() {
    #[cfg(target_os = "ios")]
    {
        // Once per launch; the thread finds a model already there at once.
        if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
        let _ = std::thread::Builder::new().name("model-fetch".into()).spawn(|| {
            if super::separate::coreml::present() { set(Fetch::Ready); return; }
            // No point fetching 287 MB onto a device that could never run it.
            let (used, left) = super::separate::coreml::memory_mb();
            match pick(used + left) {
                Some(m) => { log::info!("stems model: fetching {} ({} MB limit)", m.name, used + left); run(&ios::models_dir(), &ios::Native); }
                None => { log::info!("stems model: none fits a {} MB memory limit; not fetched", used + left); set(Fetch::Unsupported); }
            }
        });
    }
}

/// Starts a download of `url` to `dest` and reports on it.  The app's is
/// `NSURLSession` (iOS); tests use a fake.
pub trait Downloader {
    /// Start; false if it could not be started.
    fn start(&self, url: &str, dest: &Path) -> bool;
    /// (bytes so far, total or 0, state: 1 running, 2 done, 3 failed).
    fn status(&self) -> (u64, u64, i32);
}

/// The fetch, start to finish, retried with a growing pause on failure.
pub fn run(dir: &Path, dl: &dyn Downloader) {
    if installed(dir) { set(Fetch::Ready); return; }
    let mut pause = std::time::Duration::from_secs(20);
    for attempt in 1..=6 {
        match once(dir, dl) {
            Ok(()) => { set(Fetch::Ready); log::info!("stems model: installed in {}", dir.display()); return; }
            Err(e) => {
                log::warn!("stems model: attempt {attempt} failed: {e:#}");
                set(Fetch::Failed(format!("{e:#}")));
            }
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(std::time::Duration::from_secs(600));
    }
}

fn once(dir: &Path, dl: &dyn Downloader) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let zip = dir.join("htdemucs.mlpackage.zip");
    let have = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
    if have != MODEL.zip_bytes {
        let _ = std::fs::remove_file(&zip);
        ensure!(free_bytes(dir).is_none_or(|f| f > MODEL.zip_bytes + MODEL.installed_bytes + 100_000_000),
                "not enough free space for the stems model (needs about 0.8 GB)");
        set(Fetch::Downloading { got: 0, total: MODEL.zip_bytes });
        ensure!(dl.start(MODEL.url, &zip), "could not start the download");
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let (got, total, st) = dl.status();
            set(Fetch::Downloading { got, total: if total > 0 { total } else { MODEL.zip_bytes } });
            match st {
                2 => break,
                3 => bail!("download failed"),
                _ => {}
            }
        }
    }
    set(Fetch::Unpacking);
    let r = install_zip(&zip, dir);
    let _ = std::fs::remove_file(&zip);
    r
}

/// Check the zip against the pinned size and hash, unpack it next to the
/// final place, and rename it in.  Nothing is left half-installed.
pub fn install_zip(zip: &Path, dir: &Path) -> Result<()> {
    install_zip_pinned(zip, dir, MODEL.zip_bytes, MODEL.zip_sha256)
}

fn install_zip_pinned(zip: &Path, dir: &Path, bytes: u64, sha256: &str) -> Result<()> {
    use sha2::Digest;
    let mut f = std::fs::File::open(zip).with_context(|| format!("open {}", zip.display()))?;
    let len = f.metadata()?.len();
    ensure!(len == bytes, "model download is {len} bytes, expected {bytes}");
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    ensure!(got == sha256, "model download checksum mismatch");
    let tmp = dir.join(format!("{PACKAGE}.unpacking"));
    let _ = std::fs::remove_dir_all(&tmp);
    unzip_package(&mut f, &tmp)?;
    let dst = dir.join(PACKAGE);
    let _ = std::fs::remove_dir_all(&dst);
    std::fs::rename(&tmp, &dst).context("move the model into place")?;
    Ok(())
}

/// Unzip `PACKAGE/…` entries (stored or deflate) into `out`, streaming each
/// file through and checking its CRC.  Anything outside the package, or
/// with `..` in its path, is refused.
fn unzip_package<R: Read + Seek>(f: &mut R, out: &Path) -> Result<()> {
    let entries = central_directory(f)?;
    ensure!(!entries.is_empty(), "empty model archive");
    for e in entries {
        let rel = e.name.strip_prefix(&format!("{PACKAGE}/")).with_context(|| format!("unexpected file {:?} in the model archive", e.name))?;
        ensure!(!rel.split('/').any(|c| c == ".." || c.is_empty() && !e.name.ends_with('/')), "bad path {:?}", e.name);
        let path = out.join(rel);
        if e.name.ends_with('/') { std::fs::create_dir_all(&path)?; continue; }
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        // Local header: 30 bytes, then its own name and extra lengths.
        f.seek(SeekFrom::Start(e.local_offset))?;
        let mut lh = [0u8; 30];
        f.read_exact(&mut lh)?;
        ensure!(u32::from_le_bytes(lh[0..4].try_into().unwrap()) == 0x0403_4b50, "bad local header for {:?}", e.name);
        let skip = u16::from_le_bytes([lh[26], lh[27]]) as i64 + u16::from_le_bytes([lh[28], lh[29]]) as i64;
        f.seek(SeekFrom::Current(skip))?;
        let body = (&mut *f).take(e.compressed);
        let mut reader: Box<dyn Read> = match e.method {
            0 => Box::new(body),
            8 => Box::new(flate2::read::DeflateDecoder::new(body)),
            m => bail!("{:?}: compression method {m} not supported", e.name),
        };
        let mut w = std::io::BufWriter::new(std::fs::File::create(&path)?);
        let mut crc = flate2::Crc::new();
        let mut total = 0u64;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 { break; }
            crc.update(&buf[..n]);
            w.write_all(&buf[..n])?;
            total += n as u64;
        }
        w.flush()?;
        ensure!(total == e.size && crc.sum() == e.crc32, "{:?}: corrupt in the archive", e.name);
    }
    Ok(())
}

struct ZipEntry { name: String, method: u16, crc32: u32, compressed: u64, size: u64, local_offset: u64 }

/// The archive's central directory (no ZIP64: the model's files are all
/// under 4 GB).
fn central_directory<R: Read + Seek>(f: &mut R) -> Result<Vec<ZipEntry>> {
    let len = f.seek(SeekFrom::End(0))?;
    let tail = len.min(66_000);
    f.seek(SeekFrom::Start(len - tail))?;
    let mut t = vec![0u8; tail as usize];
    f.read_exact(&mut t)?;
    let eocd = (0..t.len().saturating_sub(21)).rev()
        .find(|&i| u32::from_le_bytes(t[i..i + 4].try_into().unwrap()) == 0x0605_4b50)
        .context("not a zip archive")?;
    let count = u16::from_le_bytes([t[eocd + 10], t[eocd + 11]]) as usize;
    let cd_size = u32::from_le_bytes(t[eocd + 12..eocd + 16].try_into().unwrap()) as u64;
    let cd_off = u32::from_le_bytes(t[eocd + 16..eocd + 20].try_into().unwrap()) as u64;
    ensure!(cd_off + cd_size <= len && count < 10_000, "bad zip directory");
    f.seek(SeekFrom::Start(cd_off))?;
    let mut cd = vec![0u8; cd_size as usize];
    f.read_exact(&mut cd)?;
    let mut out = Vec::with_capacity(count);
    let mut p = 0usize;
    for _ in 0..count {
        ensure!(p + 46 <= cd.len() && u32::from_le_bytes(cd[p..p + 4].try_into().unwrap()) == 0x0201_4b50, "bad zip directory entry");
        let u16at = |o: usize| u16::from_le_bytes([cd[p + o], cd[p + o + 1]]);
        let u32at = |o: usize| u32::from_le_bytes(cd[p + o..p + o + 4].try_into().unwrap());
        let (nlen, xlen, clen) = (u16at(28) as usize, u16at(30) as usize, u16at(32) as usize);
        let (compressed, size, local) = (u32at(20), u32at(24), u32at(42));
        ensure!(compressed != u32::MAX && size != u32::MAX && local != u32::MAX, "ZIP64 archives are not supported");
        ensure!(p + 46 + nlen <= cd.len(), "bad zip directory entry");
        let name = String::from_utf8(cd[p + 46..p + 46 + nlen].to_vec()).context("zip entry name")?;
        out.push(ZipEntry { name, method: u16at(10), crc32: u32at(16), compressed: compressed as u64, size: size as u64, local_offset: local as u64 });
        p += 46 + nlen + xlen + clen;
    }
    Ok(out)
}

/// Free bytes on the volume holding `dir`, when the platform says.
fn free_bytes(dir: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 { return None; }
        #[allow(clippy::unnecessary_cast)]
        return Some(s.f_bavail as u64 * s.f_frsize as u64);
    }
    #[allow(unreachable_code)]
    { let _ = dir; None }
}

#[cfg(target_os = "ios")]
mod ios {
    use super::Downloader;
    use std::ffi::{c_char, CStr, CString};
    use std::path::{Path, PathBuf};

    extern "C" {
        fn freedj_models_dir(buf: *mut c_char, len: usize);
        fn freedj_dl_start(url: *const c_char, dest: *const c_char) -> bool;
        fn freedj_dl_status(got: *mut i64, total: *mut i64, state: *mut i32);
    }

    /// Application Support/opendeck/models, created and excluded from backup.
    pub fn models_dir() -> PathBuf {
        let mut b = [0u8; 1024];
        unsafe { freedj_models_dir(b.as_mut_ptr() as *mut c_char, b.len()) };
        PathBuf::from(CStr::from_bytes_until_nul(&b).map(|c| c.to_string_lossy().into_owned()).unwrap_or_default())
    }

    pub struct Native;
    impl Downloader for Native {
        fn start(&self, url: &str, dest: &Path) -> bool {
            let (Ok(u), Ok(d)) = (CString::new(url), CString::new(dest.to_string_lossy().as_bytes())) else { return false };
            unsafe { freedj_dl_start(u.as_ptr(), d.as_ptr()) }
        }
        fn status(&self) -> (u64, u64, i32) {
            let (mut g, mut t, mut s) = (0i64, 0i64, 0i32);
            unsafe { freedj_dl_status(&mut g, &mut t, &mut s) };
            (g.max(0) as u64, t.max(0) as u64, s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A zip as `zip -r` writes it: stored and deflated entries, a folder.
    fn make_zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        use flate2::write::DeflateEncoder;
        let (mut out, mut cd) = (Vec::new(), Vec::new());
        for (name, data, deflate) in files {
            let body = if *deflate {
                let mut e = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else { data.to_vec() };
            let mut crc = flate2::Crc::new();
            crc.update(data);
            let off = out.len() as u32;
            let method: u16 = if *deflate { 8 } else { 0 };
            out.extend(0x0403_4b50u32.to_le_bytes());
            out.extend([20, 0, 0, 0]); out.extend(method.to_le_bytes()); out.extend([0; 4]);
            out.extend(crc.sum().to_le_bytes()); out.extend((body.len() as u32).to_le_bytes()); out.extend((data.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes()); out.extend(3u16.to_le_bytes());
            out.extend(name.as_bytes()); out.extend([1, 2, 3]);
            out.extend(&body);
            cd.extend(0x0201_4b50u32.to_le_bytes());
            cd.extend([20, 0, 20, 0, 0, 0]); cd.extend(method.to_le_bytes()); cd.extend([0; 4]);
            cd.extend(crc.sum().to_le_bytes()); cd.extend((body.len() as u32).to_le_bytes()); cd.extend((data.len() as u32).to_le_bytes());
            cd.extend((name.len() as u16).to_le_bytes()); cd.extend([0; 12]); cd.extend(off.to_le_bytes());
            cd.extend(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend(&cd);
        out.extend(0x0605_4b50u32.to_le_bytes()); out.extend([0; 4]);
        out.extend((files.len() as u16).to_le_bytes()); out.extend((files.len() as u16).to_le_bytes());
        out.extend((cd.len() as u32).to_le_bytes()); out.extend(cd_off.to_le_bytes()); out.extend([0, 0]);
        out
    }

    fn sha(b: &[u8]) -> String { use sha2::Digest; sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect() }

    fn package() -> Vec<u8> {
        let weights: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        make_zip(&[
            ("htdemucs.mlpackage/", b"", false),
            ("htdemucs.mlpackage/Manifest.json", b"{\"fileFormatVersion\": \"1.0.0\"}", false),
            ("htdemucs.mlpackage/Data/com.apple.CoreML/model.mlmodel", b"model bytes", true),
            ("htdemucs.mlpackage/Data/com.apple.CoreML/weights/weight.bin", &weights, true),
        ])
    }

    #[test]
    fn a_pinned_zip_installs_and_anything_else_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let z = package();
        let zp = dir.path().join("m.zip");
        std::fs::write(&zp, &z).unwrap();
        // Wrong hash or size: refused, nothing installed.
        assert!(install_zip_pinned(&zp, dir.path(), z.len() as u64, &sha(b"other")).is_err());
        assert!(install_zip_pinned(&zp, dir.path(), z.len() as u64 + 1, &sha(&z)).is_err());
        assert!(!installed(dir.path()));
        // The pinned bytes: installed, every file intact.
        install_zip_pinned(&zp, dir.path(), z.len() as u64, &sha(&z)).unwrap();
        assert!(installed(dir.path()));
        let w = std::fs::read(dir.path().join(PACKAGE).join("Data/com.apple.CoreML/weights/weight.bin")).unwrap();
        assert_eq!(w.len(), 3_000_000);
        assert_eq!(std::fs::read(dir.path().join(PACKAGE).join("Data/com.apple.CoreML/model.mlmodel")).unwrap(), b"model bytes");
        assert!(!dir.path().join(format!("{PACKAGE}.unpacking")).exists(), "no staging folder left");
    }

    /// The real download, installed with the pinned size and hash.
    /// `OPENDECK_MODEL_ZIP=…/htdemucs.mlpackage.zip cargo test --release -p opendeck-app real_model_zip -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_model_zip_installs() {
        let Some(z) = std::env::var_os("OPENDECK_MODEL_ZIP") else { return };
        let dir = tempfile::tempdir().unwrap();
        let t = opendeck_types::time::Instant::now();
        install_zip(Path::new(&z), dir.path()).unwrap();
        assert!(installed(dir.path()));
        let w = std::fs::metadata(dir.path().join(PACKAGE).join("Data/com.apple.CoreML/weights/weight.bin")).unwrap().len();
        println!("installed in {:.1}s, weights {w} bytes", t.elapsed().as_secs_f64());
        assert_eq!(w, 423_198_848);
    }

    /// The app picks the model by the device's memory; a device too small
    /// for any gets none (and fetches nothing).
    #[test]
    fn the_model_is_picked_by_memory() {
        assert_eq!(pick(16_000).map(|m| m.package), Some("htdemucs.mlpackage"));
        assert_eq!(pick(5_500).map(|m| m.package), Some("htdemucs.mlpackage"));
        // The iPad Air's standard limit (701 + 4418 MB), where 0.3.12 was
        // killed partway: no model.
        assert!(pick(701 + 4_418).is_none(), "the limit a split was killed under");
        assert!(CATALOG.iter().all(|m| m.zip_sha256.len() == 64 && m.url.contains("/resolve/") && m.zip_bytes > 0));
    }

    #[test]
    fn an_archive_escaping_the_package_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../evil", "htdemucs.mlpackage/../../evil", "other/file"] {
            let z = make_zip(&[(bad, b"x", false)]);
            let zp = dir.path().join("b.zip");
            std::fs::write(&zp, &z).unwrap();
            assert!(install_zip_pinned(&zp, dir.path(), z.len() as u64, &sha(&z)).is_err(), "{bad}");
        }
        assert!(!dir.path().join("evil").exists() && !dir.path().parent().unwrap().join("evil").exists());
    }

    /// The fetch end to end with a fake downloader: progress, then installed;
    /// a failed download leaves nothing and says why.
    #[test]
    fn the_fetch_reports_progress_and_installs() {
        struct Fake { zip: Vec<u8>, fail: bool, dest: Mutex<Option<PathBuf>>, polls: std::sync::atomic::AtomicU32 }
        impl Downloader for Fake {
            fn start(&self, _url: &str, dest: &Path) -> bool { *self.dest.lock().unwrap() = Some(dest.to_path_buf()); true }
            fn status(&self) -> (u64, u64, i32) {
                let n = self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let total = self.zip.len() as u64;
                if n < 2 { return (total * n as u64 / 2, total, 1); }
                if self.fail { return (0, total, 3); }
                std::fs::write(self.dest.lock().unwrap().as_ref().unwrap(), &self.zip).unwrap();
                (total, total, 2)
            }
        }
        let z = package();
        let dir = tempfile::tempdir().unwrap();
        // The real pinned values won't match a test archive, so drive the
        // steps the way `once` does, with this archive's pin.
        let fake = Fake { zip: z.clone(), fail: false, dest: Mutex::new(None), polls: Default::default() };
        let zp = dir.path().join("htdemucs.mlpackage.zip");
        assert!(fake.start(MODEL.url, &zp));
        let mut seen = Vec::new();
        loop {
            let (g, t, s) = fake.status();
            seen.push(Fetch::Downloading { got: g, total: t }.percent());
            if s == 2 { break; }
        }
        assert_eq!(seen[..2], [Some(0), Some(50)]);
        install_zip_pinned(&zp, dir.path(), z.len() as u64, &sha(&z)).unwrap();
        assert!(installed(dir.path()));

        let failing = Fake { zip: z, fail: true, dest: Mutex::new(None), polls: Default::default() };
        let d2 = tempfile::tempdir().unwrap();
        assert!(once(d2.path(), &failing).is_err());
        assert!(!installed(d2.path()));
    }
}
