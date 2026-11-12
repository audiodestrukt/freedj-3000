//! Present finished SDL frames on a Linux framebuffer device (`/dev/fb0`).
//!
//! `OPENDECK_FBDEV=/dev/fb0` with the SDL renderer: the deck renders with
//! SDL's software renderer off-screen (`SDL_VIDEODRIVER=dummy`), and each
//! frame is copied into the mapped framebuffer. For displays where the DRM
//! page-flip path does not reach the screen but the kernel's fbdev
//! emulation does — cdj3k-emu's virtio-gpu without virgl is one: the
//! console shows, SDL's KMSDRM flips never do (deploy/cdj3000/emu).
//!
//! 32 bpp (XRGB8888) and 16 bpp (RGB565) framebuffers are handled.

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;

use anyhow::{anyhow, bail, Context, Result};

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOPUT_VSCREENINFO: libc::c_ulong = 0x4601;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Bitfield { offset: u32, length: u32, msb_right: u32 }

#[repr(C)]
#[derive(Default)]
struct VarInfo {
    xres: u32, yres: u32, xres_virtual: u32, yres_virtual: u32,
    xoffset: u32, yoffset: u32, bits_per_pixel: u32, grayscale: u32,
    red: Bitfield, green: Bitfield, blue: Bitfield, transp: Bitfield,
    nonstd: u32, activate: u32, height: u32, width: u32, accel_flags: u32,
    pixclock: u32, left_margin: u32, right_margin: u32, upper_margin: u32,
    lower_margin: u32, hsync_len: u32, vsync_len: u32, sync: u32, vmode: u32,
    rotate: u32, colorspace: u32, reserved: [u32; 4],
}

#[repr(C)]
struct FixInfo {
    id: [u8; 16], smem_start: libc::c_ulong, smem_len: u32, type_: u32,
    type_aux: u32, visual: u32, xpanstep: u16, ypanstep: u16, ywrapstep: u16,
    line_length: u32, mmio_start: libc::c_ulong, mmio_len: u32, accel: u32,
    capabilities: u16, reserved: [u16; 2],
}

pub struct Fb {
    map: *mut u8,
    len: usize,
    pub width: u32,
    pub height: u32,
    stride: usize,
    bpp: u32,
    _file: std::fs::File,
}

// The mapping is only touched from the render thread that owns the Fb.
unsafe impl Send for Fb {}

impl Fb {
    pub fn open(path: &str) -> Result<Fb> {
        let file = OpenOptions::new().read(true).write(true).open(path)
            .with_context(|| format!("open {path}"))?;
        let fd = file.as_raw_fd();
        let mut var = VarInfo::default();
        let mut fix: FixInfo = unsafe { std::mem::zeroed() };
        // SAFETY: both structs match <linux/fb.h> and outlive the calls.
        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var) < 0 { bail!("FBIOGET_VSCREENINFO: {}", std::io::Error::last_os_error()); }
            if libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix) < 0 { bail!("FBIOGET_FSCREENINFO: {}", std::io::Error::last_os_error()); }
        }
        if var.bits_per_pixel != 32 && var.bits_per_pixel != 16 {
            bail!("{path}: {} bpp is not supported (32 or 16)", var.bits_per_pixel);
        }
        // virtio-gpu's fbdev emulation (cdj3k-emu) comes up showing 240 of
        // its 720 rows, and only the visible rows reach the screen: ask for
        // the whole buffer.
        if var.yres < var.yres_virtual {
            let before = var.yres;
            var.yres = var.yres_virtual;
            var.yoffset = 0;
            // SAFETY: as above.
            unsafe {
                if libc::ioctl(fd, FBIOPUT_VSCREENINFO as _, &mut var) < 0 {
                    log::warn!("fbdev: could not grow {before} → {} rows: {}", var.yres, std::io::Error::last_os_error());
                }
                libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var);
                libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix);
            }
        }
        let len = fix.smem_len as usize;
        // SAFETY: a shared read-write mapping of the device's own memory size.
        let map = unsafe {
            libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0)
        };
        if map == libc::MAP_FAILED { return Err(anyhow!("mmap {path}: {}", std::io::Error::last_os_error())); }
        let rows = if fix.line_length > 0 { (len / fix.line_length as usize) as u32 } else { var.yres };
        let height = var.yres.min(rows);
        log::info!("fbdev: {path} {}x{} (virtual {}x{}, {} rows mapped) {} bpp, stride {}",
                   var.xres, var.yres, var.xres_virtual, var.yres_virtual, rows, var.bits_per_pixel, fix.line_length);
        Ok(Fb { map: map as *mut u8, len, width: var.xres, height,
                stride: fix.line_length as usize, bpp: var.bits_per_pixel, _file: file })
    }

    /// Copy a `w`×`h` frame of ARGB8888 pixels (B, G, R, A in memory on a
    /// little-endian host) to the top-left of the framebuffer, clipped.
    pub fn blit_argb(&mut self, px: &[u8], w: u32, h: u32) {
        let cw = w.min(self.width) as usize;
        let ch = h.min(self.height) as usize;
        let src_stride = w as usize * 4;
        for y in 0..ch {
            let src = &px[y * src_stride..y * src_stride + cw * 4];
            let off = y * self.stride;
            if off + cw * (self.bpp as usize / 8) > self.len { break; }
            // SAFETY: bounds checked against the mapping length above.
            let dst = unsafe { std::slice::from_raw_parts_mut(self.map.add(off), cw * (self.bpp as usize / 8)) };
            if self.bpp == 32 {
                dst.copy_from_slice(src);
            } else {
                for (d, s) in dst.chunks_exact_mut(2).zip(src.chunks_exact(4)) {
                    let v = ((s[2] as u16 >> 3) << 11) | ((s[1] as u16 >> 2) << 5) | (s[0] as u16 >> 3);
                    d.copy_from_slice(&v.to_le_bytes());
                }
            }
        }
    }
}

impl Drop for Fb {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly what open() mapped.
        unsafe { libc::munmap(self.map as *mut libc::c_void, self.len); }
    }
}
