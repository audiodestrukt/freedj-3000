//! upd-emucheck IN.(UPD|iso) WORKDIR [--key FILE] [--patch RESOURCES_DIR]
//!
//! 1. decrypts IN with the key (skipped for a plaintext .iso),
//! 2. reads the firmware info the wizard shows,
//! 3. picks the rk3399 `Image` from IMAGES.TAR.GZ,
//! 4. takes the largest embedded cpio out of it (our rootfs),
//! 5. with --patch, runs the emulator's patch-rootfs.sh over it on the host
//!    (the wizard's CDJ3K_INITRAMFS_HOST path); RESOURCES_DIR holds
//!    `patch/` (patch-rootfs.sh, patch-rootfs.d/, assets) and `tools/`.
use std::path::{Path, PathBuf};
use std::process::exit;

use cdj3k_emu_firmware as fw;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = vec![];
    let (mut key, mut patch) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--key" => key = it.next().cloned(),
            "--patch" => patch = it.next().cloned(),
            _ => pos.push(a.clone()),
        }
    }
    let [input, work] = &pos[..] else {
        eprintln!("usage: upd-emucheck IN WORKDIR [--key FILE] [--patch RESOURCES_DIR]");
        exit(2)
    };
    let work = PathBuf::from(work);
    std::fs::create_dir_all(&work).unwrap();
    let iso = if input.to_ascii_lowercase().ends_with(".upd") {
        let key = key.unwrap_or_else(|| fail("a .UPD needs --key"));
        let key = fw::LuksKey::from_file(Path::new(&key)).unwrap_or_else(|e| fail(&e.to_string()));
        let iso = work.join("decrypted.iso");
        fw::decrypt_upd(Path::new(input), &key, &iso).unwrap_or_else(|e| fail(&format!("decrypt: {e}")));
        println!("ok  decrypt");
        iso
    } else {
        PathBuf::from(input)
    };
    let info = fw::read_firmware_info(&iso).unwrap_or_else(|e| fail(&format!("firmware info: {e}")));
    println!(
        "ok  firmware info: release={:?} app={:?} system={:?} miniloader={} bytes",
        info.release, info.rev_apl, info.rev_system,
        info.miniloader.as_ref().map_or(0, |m| m.len())
    );
    let kernel = work.join("Image.carrier");
    fw::extract_kernel(&iso, &kernel, |m| println!("    {m}")).unwrap_or_else(|e| fail(&format!("kernel: {e}")));
    println!("ok  carrier Image");
    let cpio = work.join("initramfs.cpio");
    fw::extract_initramfs(&kernel, &cpio).unwrap_or_else(|e| fail(&format!("initramfs: {e}")));
    println!("ok  rootfs cpio ({} MiB)", std::fs::metadata(&cpio).unwrap().len() >> 20);
    match fw::read_cabinet_image(&iso) {
        Some(c) => println!("    cabinet: {} bytes", c.len()),
        None => println!("    cabinet: none (the wizard stages nothing)"),
    }
    if let Some(res) = patch {
        let gz = work.join("initramfs.cpio.gz");
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("gzip -1c '{}' > '{}'", cpio.display(), gz.display()))
            .status()
            .unwrap();
        assert!(status.success());
        let out = work.join("initramfs-patched.cpio.gz");
        fw::patch_initramfs(&gz, Path::new(&res), &out).unwrap_or_else(|e| fail(&format!("patch: {e}")));
        println!("ok  patched ({} MiB)", std::fs::metadata(&out).unwrap().len() >> 20);
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("FAIL {msg}");
    exit(1)
}
