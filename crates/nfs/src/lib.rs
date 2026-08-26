//! Minimal NFSv2-over-UDP client for reading a Pioneer CDJ/XDJ's linked USB.
//!
//! When a CDJ/XDJ has a USB inserted and is on the ProDJ Link network, it serves
//! that media over Sun RPC: portmapper (UDP 111) → mountd v1 → nfsd v2, all over
//! UDP.  This is the transport behind "load a track from a linked player" — the
//! player exports its stick and others mount + read `export.pdb`, the `ANLZ`
//! files, and the audio itself.
//!
//! Two Pioneer quirks (verified against a real XDJ-1000MK2):
//!   * the export path and all NFS filenames are **UTF-16LE** (`P\0I\0O\0…`),
//!   * requests should come from a **privileged source port** (< 1024).
//!
//! This is a deliberately tiny client: portmap GETPORT, mount MNT, and NFS
//! LOOKUP / READ / READDIR — enough to browse and pull files.  Reverse-engineered
//! after Deep Symmetry's crate-digger; targets nexus/nexus2-era gear (the
//! XDJ-1000MK2). The CDJ-3000 encrypts this and is not supported.

use anyhow::{anyhow, bail, Context, Result};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

const PROG_PORTMAP: u32 = 100_000;
const PROG_MOUNT:   u32 = 100_005;
const PROG_NFS:     u32 = 100_003;
const IPPROTO_UDP:  u32 = 17;

/// An opaque NFSv2 file handle (fixed 32 bytes).
pub type Fh = [u8; 32];

/// A directory entry from READDIR.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name:    String,
    pub file_id: u32,
}

pub struct Nfs {
    sock:       UdpSocket,
    server:     SocketAddrV4,
    mount_port: u16,
    nfs_port:   u16,
    xid:        u32,
}

// ── XDR helpers ───────────────────────────────────────────────────────────────

fn xdr_opaque(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
    let pad = (4 - bytes.len() % 4) % 4;
    buf.extend(std::iter::repeat(0).take(pad));
}

fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Pioneer NFS filenames are UTF-16LE.
fn wide(name: &str) -> Vec<u8> {
    name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

impl Nfs {
    /// Connect to a player and resolve its mountd + nfsd UDP ports via portmap.
    pub fn connect(ip: Ipv4Addr) -> Result<Self> {
        // Bind a privileged source port; the CDJ NFS server expects < 1024.
        // Fall back to an ephemeral port if we can't (works on some setups).
        let sock = bind_privileged().context("bind UDP socket")?;
        sock.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut me = Nfs {
            sock,
            server: SocketAddrV4::new(ip, 111),
            mount_port: 0,
            nfs_port: 0,
            xid: 0x0102_0304,
        };
        me.mount_port = me.getport(PROG_MOUNT, 1)?;
        me.nfs_port   = me.getport(PROG_NFS, 2)?;
        if me.mount_port == 0 || me.nfs_port == 0 {
            bail!("player {ip} does not advertise mountd/nfsd (not serving a USB, or a CDJ-3000)");
        }
        Ok(me)
    }

    /// One RPC call over UDP, with a few retries (UDP is lossy).  Returns the
    /// reply body *after* the accepted-reply header (i.e. the procedure result).
    fn call(&mut self, prog: u32, vers: u32, proc_: u32, port: u16, args: &[u8]) -> Result<Vec<u8>> {
        self.xid = self.xid.wrapping_add(1);
        let xid = self.xid;
        let mut msg = Vec::with_capacity(40 + args.len());
        msg.extend_from_slice(&xid.to_be_bytes());
        msg.extend_from_slice(&0u32.to_be_bytes());   // mtype = CALL
        msg.extend_from_slice(&2u32.to_be_bytes());   // rpcvers
        msg.extend_from_slice(&prog.to_be_bytes());
        msg.extend_from_slice(&vers.to_be_bytes());
        msg.extend_from_slice(&proc_.to_be_bytes());
        msg.extend_from_slice(&[0; 8]);               // cred = AUTH_NULL
        msg.extend_from_slice(&[0; 8]);               // verf = AUTH_NULL
        msg.extend_from_slice(args);

        let dst = SocketAddrV4::new(*self.server.ip(), port);
        let mut buf = [0u8; 65536];
        for _ in 0..4 {
            self.sock.send_to(&msg, dst).context("nfs send")?;
            match self.sock.recv_from(&mut buf) {
                Ok((n, _)) => {
                    let r = &buf[..n];
                    if n < 24 || be32(r, 0) != xid { continue; }      // not our reply
                    if be32(r, 4) != 1 { bail!("not an RPC reply"); }  // mtype REPLY
                    if be32(r, 8) != 0 { bail!("RPC msg denied"); }    // reply_stat = ACCEPTED
                    // verf: flavor(4) + len(4) + body(len, padded)
                    let vlen = be32(r, 16) as usize;
                    let mut off = 20 + ((vlen + 3) & !3);
                    let accept_stat = be32(r, off); off += 4;
                    if accept_stat != 0 { bail!("RPC accept_stat={accept_stat}"); }
                    return Ok(r[off..].to_vec());
                }
                Err(_) => continue,   // timeout → retry
            }
        }
        bail!("no RPC reply from {dst} (prog {prog} proc {proc_})")
    }

    fn getport(&mut self, prog: u32, vers: u32) -> Result<u16> {
        let mut a = Vec::new();
        a.extend_from_slice(&prog.to_be_bytes());
        a.extend_from_slice(&vers.to_be_bytes());
        a.extend_from_slice(&IPPROTO_UDP.to_be_bytes());
        a.extend_from_slice(&0u32.to_be_bytes());
        let r = self.call(PROG_PORTMAP, 2, 3, 111, &a)?;
        Ok(be32(&r, 0) as u16)
    }

    /// MNT an export (raw bytes as advertised by the server, e.g. `/\0C\0/\0`).
    pub fn mount(&mut self, export: &[u8]) -> Result<Fh> {
        let mut a = Vec::new();
        xdr_opaque(&mut a, export);
        let r = self.call(PROG_MOUNT, 1, 1, self.mount_port, &a)?;
        let status = be32(&r, 0);
        if status != 0 { bail!("MNT failed (status {status})"); }
        let mut fh = [0u8; 32];
        fh.copy_from_slice(&r[4..36]);
        Ok(fh)
    }

    /// Mount the standard CDJ USB export (`/C/`, UTF-16LE) and return its root fh.
    pub fn mount_usb(&mut self) -> Result<Fh> {
        self.mount(&wide("/C/"))
    }

    /// LOOKUP a name in a directory → (file handle, size in bytes).
    pub fn lookup(&mut self, dir: &Fh, name: &str) -> Result<(Fh, u32)> {
        let mut a = Vec::new();
        a.extend_from_slice(dir);
        xdr_opaque(&mut a, &wide(name));
        let r = self.call(PROG_NFS, 2, 4, self.nfs_port, &a)?;
        let status = be32(&r, 0);
        if status != 0 { bail!("LOOKUP {name:?} failed (status {status})"); }
        let mut fh = [0u8; 32];
        fh.copy_from_slice(&r[4..36]);
        let size = be32(&r, 36 + 20);   // fattr.size is the 6th u32
        Ok((fh, size))
    }

    /// Resolve a `/`-separated path from a root handle.
    pub fn lookup_path(&mut self, root: &Fh, path: &str) -> Result<(Fh, u32)> {
        let mut fh = *root;
        let mut size = 0;
        for comp in path.split('/').filter(|c| !c.is_empty()) {
            let (f, s) = self.lookup(&fh, comp)?;
            fh = f; size = s;
        }
        Ok((fh, size))
    }

    /// READ up to `count` bytes at `offset` (NFSv2 caps a single read ~8 KB).
    pub fn read(&mut self, fh: &Fh, offset: u32, count: u32) -> Result<Vec<u8>> {
        let mut a = Vec::new();
        a.extend_from_slice(fh);
        a.extend_from_slice(&offset.to_be_bytes());
        a.extend_from_slice(&count.to_be_bytes());
        a.extend_from_slice(&count.to_be_bytes());   // totalcount (ignored)
        let r = self.call(PROG_NFS, 2, 6, self.nfs_port, &a)?;
        let status = be32(&r, 0);
        if status != 0 { bail!("READ failed (status {status})"); }
        let dlen = be32(&r, 4 + 68) as usize;        // status + fattr(68) + opaque
        let start = 4 + 68 + 4;
        Ok(r[start..start + dlen].to_vec())
    }

    /// Read an entire file (chunked).
    pub fn read_file(&mut self, fh: &Fh, size: u32) -> Result<Vec<u8>> {
        const CHUNK: u32 = 8192;
        let mut out = Vec::with_capacity(size as usize);
        let mut off = 0u32;
        while off < size {
            let want = CHUNK.min(size - off);
            let block = self.read(fh, off, want)?;
            if block.is_empty() { break; }
            off += block.len() as u32;
            out.extend_from_slice(&block);
        }
        Ok(out)
    }

    /// List a directory (one READDIR; large dirs may need cookie paging — TODO).
    pub fn readdir(&mut self, fh: &Fh) -> Result<Vec<DirEntry>> {
        let mut a = Vec::new();
        a.extend_from_slice(fh);
        a.extend_from_slice(&0u32.to_be_bytes());     // cookie
        a.extend_from_slice(&8192u32.to_be_bytes());  // count
        let r = self.call(PROG_NFS, 2, 16, self.nfs_port, &a)?;
        let status = be32(&r, 0);
        if status != 0 { bail!("READDIR failed (status {status})"); }
        let mut out = Vec::new();
        let mut off = 4;
        while off + 4 <= r.len() {
            let more = be32(&r, off); off += 4;
            if more == 0 { break; }
            let file_id = be32(&r, off); off += 4;
            let nlen = be32(&r, off) as usize; off += 4;
            let raw = &r[off..off + nlen]; off += (nlen + 3) & !3;
            let _cookie = be32(&r, off); off += 4;
            // Names are UTF-16LE.
            let units: Vec<u16> = raw.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let name = String::from_utf16_lossy(&units);
            out.push(DirEntry { name, file_id });
        }
        Ok(out)
    }
}

/// Bind a UDP socket on a privileged local port (CDJ NFS wants source < 1024);
/// fall back to an ephemeral port if privileged binding is not permitted.
fn bind_privileged() -> Result<UdpSocket> {
    for port in [1023u16, 1022, 1021, 900, 800] {
        if let Ok(s) = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)) {
            return Ok(s);
        }
    }
    UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|e| anyhow!("bind udp: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Gated: OPENDECK_TEST_NFS=192.168.68.58  (a linked CDJ/XDJ with a USB in it)
    #[test]
    fn reads_export_pdb_over_nfs() {
        let Ok(ip) = std::env::var("OPENDECK_TEST_NFS") else { return };
        let ip: Ipv4Addr = ip.parse().expect("valid IPv4");
        let mut nfs = Nfs::connect(ip).expect("connect");
        let root = nfs.mount_usb().expect("mount /C/");
        let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb").expect("lookup pdb");
        assert!(size > 0, "pdb has size");
        let head = nfs.read(&fh, 0, 16).expect("read");
        // pdb: u32 0, then page_size (little-endian) = 4096
        assert_eq!(&head[0..4], &[0, 0, 0, 0], "pdb magic");
        let page_size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        assert_eq!(page_size, 4096, "pdb page_size");
        // READDIR the root should list PIONEER
        let entries = nfs.readdir(&root).expect("readdir");
        assert!(entries.iter().any(|e| e.name == "PIONEER"), "PIONEER in root");
        println!("OK: export.pdb {size} bytes, page_size {page_size}, {} root entries", entries.len());
    }
}
