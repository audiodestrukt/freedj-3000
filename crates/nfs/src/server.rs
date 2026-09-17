//! A minimal **NFSv2-over-UDP server** the way a CDJ serves its USB: portmap
//! → mountd v1 → nfsd v2, names on the wire in UTF-16LE, one export.  This is
//! what a player (or rekordbox) reads a track's audio from after dbserver has
//! told it the path.  Read-only; just enough procedures for a Pioneer client:
//! NULL, GETATTR, LOOKUP, READ, READDIR, STATFS (nfs) and NULL, MNT, UMNT,
//! EXPORT (mount).
//!
//! File handles are 32 bytes: the node index (u64 BE) then zeros.  Nodes are
//! walked once at start from the served folder.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

const PROG_PORTMAP: u32 = 100_000;
const PROG_MOUNT:   u32 = 100_005;
const PROG_NFS:     u32 = 100_003;
const IPPROTO_UDP:  u32 = 17;
const NFSERR_NOENT: u32 = 2;
const NFSERR_IO:    u32 = 5;
const NFSERR_ACCES: u32 = 13;
const NFSERR_NOTDIR: u32 = 20;
const NFSERR_STALE: u32 = 70;
const MAX_READ: usize = 8192;

struct Node { path: PathBuf, is_dir: bool, size: u64, mtime: u32, children: Vec<(String, u32)> }

/// The served tree.
pub struct Tree { nodes: Vec<Node>, by_path: HashMap<PathBuf, u32> }

impl Tree {
    /// Walk `root` (recursively) into a node table.  Hidden entries are skipped.
    pub fn scan(root: &Path) -> Result<Tree> {
        let mut t = Tree { nodes: Vec::new(), by_path: HashMap::new() };
        t.add(root.to_path_buf())?;
        Ok(t)
    }
    fn add(&mut self, path: PathBuf) -> Result<u32> {
        let md = fs::metadata(&path).with_context(|| format!("stat {}", path.display()))?;
        let mtime = md.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as u32).unwrap_or(0);
        let idx = self.nodes.len() as u32;
        self.nodes.push(Node { path: path.clone(), is_dir: md.is_dir(), size: md.len(), mtime, children: Vec::new() });
        self.by_path.insert(path.clone(), idx);
        if md.is_dir() {
            let mut names: Vec<(String, PathBuf)> = fs::read_dir(&path)?.flatten()
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .filter(|(n, _)| !n.starts_with('.')).collect();
            names.sort();
            for (name, child) in names {
                if let Ok(ci) = self.add(child) { self.nodes[idx as usize].children.push((name, ci)); }
            }
        }
        Ok(idx)
    }
    /// The NFS path (`/a/b.mp3`, forward slashes, relative to the export root)
    /// of every regular file — what dbserver hands out as a track's path.
    pub fn files(&self) -> Vec<(String, PathBuf)> {
        let root = &self.nodes[0].path;
        self.nodes.iter().filter(|n| !n.is_dir).map(|n| {
            let rel = n.path.strip_prefix(root).unwrap_or(&n.path);
            let s = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
            (format!("/{s}"), n.path.clone())
        }).collect()
    }
}

// ── XDR ───────────────────────────────────────────────────────────────────────
fn be32(b: &[u8], off: usize) -> Option<u32> { b.get(off..off + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]])) }
fn put32(out: &mut Vec<u8>, v: u32) { out.extend_from_slice(&v.to_be_bytes()); }
fn put_opaque(out: &mut Vec<u8>, b: &[u8]) { put32(out, b.len() as u32); out.extend_from_slice(b); out.extend(std::iter::repeat(0).take((4 - b.len() % 4) % 4)); }
fn get_opaque(b: &[u8], off: usize) -> Option<(&[u8], usize)> {
    let n = be32(b, off)? as usize;
    let data = b.get(off + 4..off + 4 + n)?;
    Some((data, off + 4 + ((n + 3) & !3)))
}
fn wide(s: &str) -> Vec<u8> { s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect() }
fn unwide(b: &[u8]) -> String {
    if b.len() % 2 == 0 && b.iter().skip(1).step_by(2).all(|&x| x == 0) && !b.is_empty() {
        String::from_utf16_lossy(&b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
    } else { String::from_utf8_lossy(b).into_owned() }
}
fn fh_of(idx: u32) -> [u8; 32] { let mut f = [0u8; 32]; f[..8].copy_from_slice(&(idx as u64).to_be_bytes()); f }
fn idx_of(fh: &[u8]) -> Option<u32> { fh.get(..8).map(|x| u64::from_be_bytes(x.try_into().unwrap()) as u32) }

fn fattr(out: &mut Vec<u8>, idx: u32, n: &Node) {
    put32(out, if n.is_dir { 2 } else { 1 });                  // type NFDIR / NFREG
    put32(out, if n.is_dir { 0o040555 } else { 0o100444 });    // mode
    put32(out, 1); put32(out, 0); put32(out, 0);               // nlink uid gid
    put32(out, n.size.min(u32::MAX as u64) as u32);            // size
    put32(out, MAX_READ as u32);                               // blocksize
    put32(out, 0);                                             // rdev
    put32(out, ((n.size + 511) / 512) as u32);                 // blocks
    put32(out, 1);                                             // fsid
    put32(out, idx + 1);                                       // fileid
    for _ in 0..3 { put32(out, n.mtime); put32(out, 0); }      // atime mtime ctime
}

/// One RPC reply: accepted, AUTH_NULL verifier, success, then `body`.
fn reply(xid: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(24 + body.len());
    put32(&mut out, xid); put32(&mut out, 1);       // REPLY
    put32(&mut out, 0);                             // MSG_ACCEPTED
    put32(&mut out, 0); put32(&mut out, 0);         // verf AUTH_NULL
    put32(&mut out, 0);                             // SUCCESS
    out.extend_from_slice(body);
    out
}
fn reply_proc_unavail(xid: u32) -> Vec<u8> {
    let mut out = Vec::new();
    put32(&mut out, xid); put32(&mut out, 1); put32(&mut out, 0); put32(&mut out, 0); put32(&mut out, 0);
    put32(&mut out, 3);                             // PROC_UNAVAIL
    out
}

/// Parse the RPC call header; returns (xid, prog, vers, proc, args offset).
fn parse_call(d: &[u8]) -> Option<(u32, u32, u32, u32, usize)> {
    let xid = be32(d, 0)?; if be32(d, 4)? != 0 { return None; }
    let (prog, vers, proc_) = (be32(d, 12)?, be32(d, 16)?, be32(d, 20)?);
    // cred: flavor, len, body; verf: flavor, len, body
    let cl = be32(d, 28)? as usize; let mut off = 32 + ((cl + 3) & !3);
    let vl = be32(d, off + 4)? as usize; off += 8 + ((vl + 3) & !3);
    Some((xid, prog, vers, proc_, off))
}

struct Ports { mountd: u16, nfsd: u16 }

fn serve_portmap(sock: UdpSocket, ports: Ports) {
    let mut buf = [0u8; 2048];
    while let Ok((n, from)) = sock.recv_from(&mut buf) {
        let d = &buf[..n];
        let Some((xid, prog, _vers, proc_, off)) = parse_call(d) else { continue };
        if prog != PROG_PORTMAP { continue; }
        let body = match proc_ {
            0 => Vec::new(),
            3 => { // GETPORT
                let (p, v, proto) = (be32(d, off).unwrap_or(0), be32(d, off + 4).unwrap_or(0), be32(d, off + 8).unwrap_or(0));
                let port = match (p, proto) { (PROG_MOUNT, IPPROTO_UDP) => ports.mountd, (PROG_NFS, IPPROTO_UDP) => ports.nfsd, _ => 0 };
                log::info!("portmap: {from} GETPORT prog {p} v{v} → {port}");
                let mut b = Vec::new(); put32(&mut b, port as u32); b
            }
            4 => { // DUMP
                let mut b = Vec::new();
                for (p, v, port) in [(PROG_MOUNT, 1, ports.mountd), (PROG_NFS, 2, ports.nfsd)] {
                    put32(&mut b, 1); put32(&mut b, p); put32(&mut b, v); put32(&mut b, IPPROTO_UDP); put32(&mut b, port as u32);
                }
                put32(&mut b, 0); b
            }
            _ => { let _ = sock.send_to(&reply_proc_unavail(xid), from); continue; }
        };
        let _ = sock.send_to(&reply(xid, &body), from);
    }
}

fn serve_mountd(sock: UdpSocket, export: Vec<u8>) {
    let mut buf = [0u8; 4096];
    while let Ok((n, from)) = sock.recv_from(&mut buf) {
        let d = &buf[..n];
        let Some((xid, prog, _v, proc_, off)) = parse_call(d) else { continue };
        if prog != PROG_MOUNT { continue; }
        let body = match proc_ {
            0 => Vec::new(),
            1 => { // MNT: any export name mounts our root
                let name = get_opaque(d, off).map(|(b, _)| unwide(b)).unwrap_or_default();
                log::info!("mountd: {from} MNT {name:?}");
                let mut b = Vec::new(); put32(&mut b, 0); b.extend_from_slice(&fh_of(0)); b
            }
            3 => { log::info!("mountd: {from} UMNT"); Vec::new() }
            5 => { // EXPORT: one export, no group restriction
                let mut b = Vec::new(); put32(&mut b, 1); put_opaque(&mut b, &export); put32(&mut b, 0); put32(&mut b, 0); b
            }
            _ => { let _ = sock.send_to(&reply_proc_unavail(xid), from); continue; }
        };
        let _ = sock.send_to(&reply(xid, &body), from);
    }
}

fn serve_nfsd(sock: UdpSocket, tree: Arc<Tree>) {
    let mut buf = [0u8; 65536];
    let err = |code: u32| { let mut b = Vec::new(); put32(&mut b, code); b };
    while let Ok((n, from)) = sock.recv_from(&mut buf) {
        let d = &buf[..n];
        let Some((xid, prog, _v, proc_, off)) = parse_call(d) else { continue };
        if prog != PROG_NFS { continue; }
        let node_at = |o: usize| d.get(o..o + 32).and_then(idx_of).and_then(|i| tree.nodes.get(i as usize).map(|nd| (i, nd)));
        let body = match proc_ {
            0 => Vec::new(),
            1 => match node_at(off) { // GETATTR
                Some((i, nd)) => { let mut b = Vec::new(); put32(&mut b, 0); fattr(&mut b, i, nd); b }
                None => err(NFSERR_STALE),
            },
            4 => { // LOOKUP dir fh + name
                match node_at(off) {
                    Some((_, dir)) if dir.is_dir => {
                        let name = get_opaque(d, off + 32).map(|(b, _)| unwide(b)).unwrap_or_default();
                        match dir.children.iter().find(|(c, _)| *c == name) {
                            Some((_, ci)) => {
                                let nd = &tree.nodes[*ci as usize];
                                log::debug!("nfsd: {from} LOOKUP {name:?} → {}", nd.path.display());
                                let mut b = Vec::new(); put32(&mut b, 0); b.extend_from_slice(&fh_of(*ci)); fattr(&mut b, *ci, nd); b
                            }
                            None => { log::info!("nfsd: {from} LOOKUP {name:?}: not found"); err(NFSERR_NOENT) }
                        }
                    }
                    Some(_) => err(NFSERR_NOTDIR),
                    None => err(NFSERR_STALE),
                }
            }
            6 => { // READ fh, offset, count, totalcount
                match node_at(off) {
                    Some((i, nd)) if !nd.is_dir => {
                        let offset = be32(d, off + 32).unwrap_or(0) as u64;
                        let count = (be32(d, off + 36).unwrap_or(0) as usize).min(MAX_READ);
                        let mut data = vec![0u8; count];
                        let got = fs::File::open(&nd.path).and_then(|mut f| { f.seek(SeekFrom::Start(offset))?; let mut total = 0; while total < count { let k = f.read(&mut data[total..])?; if k == 0 { break; } total += k; } Ok(total) });
                        match got {
                            Ok(k) => { data.truncate(k); let mut b = Vec::new(); put32(&mut b, 0); fattr(&mut b, i, nd); put_opaque(&mut b, &data); b }
                            Err(e) => { log::warn!("nfsd: read {}: {e}", nd.path.display()); err(NFSERR_IO) }
                        }
                    }
                    Some(_) => err(NFSERR_ACCES),
                    None => err(NFSERR_STALE),
                }
            }
            16 => { // READDIR fh, cookie, count
                match node_at(off) {
                    Some((_, dir)) if dir.is_dir => {
                        let cookie = be32(d, off + 32).unwrap_or(0) as usize;
                        let count = be32(d, off + 36).unwrap_or(4096) as usize;
                        let mut b = Vec::new(); put32(&mut b, 0);
                        let mut used = 16; let mut i = cookie; let mut eof = true;
                        while i < dir.children.len() {
                            let (name, ci) = &dir.children[i];
                            let w = wide(name);
                            if used + 16 + w.len() > count.min(60000) { eof = false; break; }
                            put32(&mut b, 1); put32(&mut b, ci + 1); put_opaque(&mut b, &w); put32(&mut b, i as u32 + 1);
                            used += 16 + w.len(); i += 1;
                        }
                        put32(&mut b, 0); put32(&mut b, eof as u32); b
                    }
                    Some(_) => err(NFSERR_NOTDIR),
                    None => err(NFSERR_STALE),
                }
            }
            17 => { // STATFS
                let mut b = Vec::new(); put32(&mut b, 0);
                for v in [MAX_READ as u32, 4096, 1 << 20, 0, 0] { put32(&mut b, v); }
                b
            }
            p => { log::info!("nfsd: {from} unsupported proc {p}"); let _ = sock.send_to(&reply_proc_unavail(xid), from); continue; }
        };
        let _ = sock.send_to(&reply(xid, &body), from);
    }
}

/// A running server: the ports it took.
pub struct NfsServer { pub portmap: u16, pub mountd: u16, pub nfsd: u16 }

impl NfsServer {
    /// Serve `tree` as export `export_name` (e.g. `/C/`).  `portmap_port` is
    /// 111 for a player-style server or 50111 rekordbox-style; `nfsd_port` 0
    /// = ephemeral (players use 2049).
    pub fn start(tree: Tree, export_name: &str, portmap_port: u16, nfsd_port: u16) -> Result<NfsServer> {
        let any = |p: u16| UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, p));
        let pm = any(portmap_port).with_context(|| format!("bind portmap {portmap_port}"))?;
        let md = any(0).context("bind mountd")?;
        let nd = any(nfsd_port).or_else(|_| any(0)).context("bind nfsd")?;
        let ports = NfsServer { portmap: pm.local_addr()?.port(), mountd: md.local_addr()?.port(), nfsd: nd.local_addr()?.port() };
        log::info!("nfs: portmap {} mountd {} nfsd {}; export {export_name:?} = {} nodes", ports.portmap, ports.mountd, ports.nfsd, tree.nodes.len());
        let tree = Arc::new(tree);
        let p = Ports { mountd: ports.mountd, nfsd: ports.nfsd };
        thread::Builder::new().name("portmap".into()).spawn(move || serve_portmap(pm, p))?;
        let export = wide(export_name);
        thread::Builder::new().name("mountd".into()).spawn(move || serve_mountd(md, export))?;
        thread::Builder::new().name("nfsd".into()).spawn(move || serve_nfsd(nd, tree))?;
        Ok(ports)
    }
}
