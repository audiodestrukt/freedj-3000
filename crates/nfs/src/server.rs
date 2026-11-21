//! A minimal **NFSv2-over-UDP server** the way a CDJ serves its USB: portmap
//! → mountd v1 → nfsd v2, names on the wire in UTF-16LE, one export.  This is
//! what a player (or rekordbox) reads a track's audio from after dbserver has
//! told it the path.  Read-only; just enough procedures for a Pioneer client:
//! NULL, GETATTR, LOOKUP, READ, READDIR, STATFS (nfs) and NULL, MNT, UMNT,
//! EXPORT (mount).
//!
//! File handles are 32 bytes: a marker and the node index.  Nodes are
//! walked at start from the served folder and added as clients find new
//! files ([`Tree`] is live).

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
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
/// Largest READ we answer.  Players read 32 KiB at a time over UDP (one
/// fragmented datagram); capped at 8 KiB, a CDJ-3000 got short reads and
/// refused the track ("E-8302: CANNOT PLAY TRACK").
const READ_CAP: usize = 32 * 1024;

#[derive(Clone)]
struct Node { path: PathBuf, nfs: String, is_dir: bool, size: u64, mtime: u32, children: Vec<(String, u32)> }

struct Nodes { nodes: Vec<Node>, by_path: HashMap<PathBuf, u32> }

/// Called with the NFS path (`/CDJMODS/live/12`) of a name a client looked
/// up and did not find, before the server answers "no such file": a chance
/// to make it (or to start making it).
pub type MissHook = Arc<dyn Fn(&str) + Send + Sync>;

/// The served tree.  It is **live**: a LOOKUP or READDIR sees files made
/// after the scan, and GETATTR and READ see a file's current size, so a file
/// another thread is still writing can be read as it grows.  Node indices
/// (and so file handles) never change once given out.
pub struct Tree { inner: RwLock<Nodes>, on_miss: RwLock<Option<MissHook>> }

fn mtime_of(md: &fs::Metadata) -> u32 {
    md.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as u32).unwrap_or(0)
}

/// A name a client may look up: no hidden files, nothing that leaves the dir.
fn plain_name(n: &str) -> bool { !n.is_empty() && !n.starts_with('.') && !n.contains('/') && !n.contains('\\') }

fn join_nfs(dir: &str, name: &str) -> String { if dir == "/" { format!("/{name}") } else { format!("{dir}/{name}") } }

impl Nodes {
    fn add(&mut self, path: PathBuf, nfs: String) -> Result<u32> {
        let md = fs::metadata(&path).with_context(|| format!("stat {}", path.display()))?;
        if let Some(&i) = self.by_path.get(&path) { return Ok(i); }
        let idx = self.nodes.len() as u32;
        self.nodes.push(Node { path: path.clone(), nfs: nfs.clone(), is_dir: md.is_dir(), size: md.len(), mtime: mtime_of(&md), children: Vec::new() });
        self.by_path.insert(path.clone(), idx);
        if md.is_dir() {
            let mut names: Vec<(String, PathBuf)> = fs::read_dir(&path)?.flatten()
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .filter(|(n, _)| plain_name(n)).collect();
            names.sort();
            for (name, child) in names {
                let cn = join_nfs(&nfs, &name);
                if let Ok(ci) = self.add(child, cn) { self.nodes[idx as usize].children.push((name, ci)); }
            }
        }
        Ok(idx)
    }
}

impl Tree {
    /// Walk `root` (recursively) into a node table.  Hidden entries are skipped.
    pub fn scan(root: &Path) -> Result<Tree> {
        let mut n = Nodes { nodes: Vec::new(), by_path: HashMap::new() };
        n.add(root.to_path_buf(), "/".into())?;
        Ok(Tree { inner: RwLock::new(n), on_miss: RwLock::new(None) })
    }
    /// The NFS path (`/a/b.mp3`, forward slashes, relative to the export root)
    /// of every regular file — what dbserver hands out as a track's path.
    pub fn files(&self) -> Vec<(String, PathBuf)> {
        let t = self.inner.read().unwrap();
        t.nodes.iter().filter(|n| !n.is_dir).map(|n| (n.nfs.clone(), n.path.clone())).collect()
    }
    /// Serve local folder `dir` as `/<name>` at the export root, in place of
    /// any folder of that name in the scanned one (a media server keeps its
    /// own `CDJMODS` outside the user's music folder).
    pub fn overlay(&self, name: &str, dir: &Path) -> Result<()> {
        anyhow::ensure!(plain_name(name), "overlay name {name:?}");
        fs::create_dir_all(dir)?;
        let mut t = self.inner.write().unwrap();
        let ci = t.add(dir.to_path_buf(), format!("/{name}"))?;
        let root = &mut t.nodes[0].children;
        root.retain(|(n, _)| n != name);
        root.push((name.to_string(), ci));
        root.sort();
        Ok(())
    }
    /// Set the hook called on a lookup that finds nothing ([`MissHook`]).
    pub fn on_miss(&self, f: MissHook) { *self.on_miss.write().unwrap() = Some(f); }

    /// Node `i` with its size and time read again from the disk.
    fn node(&self, i: u32) -> Option<Node> {
        let mut t = self.inner.write().unwrap();
        let nd = t.nodes.get_mut(i as usize)?;
        if let Ok(md) = fs::metadata(&nd.path) { nd.size = md.len(); nd.mtime = mtime_of(&md); }
        Some(Node { children: Vec::new(), ..nd.clone() })
    }
    /// `name` in directory `dir`: from the table, else from the disk (a file
    /// made since the scan), else after asking the miss hook.
    fn lookup(&self, dir: u32, name: &str) -> Option<u32> {
        if !plain_name(name) { return None; }
        let (path, nfs) = {
            let t = self.inner.read().unwrap();
            let d = t.nodes.get(dir as usize)?;
            if let Some((_, ci)) = d.children.iter().find(|(c, _)| c == name) {
                if t.nodes[*ci as usize].path.exists() { return Some(*ci); }
            }
            (d.path.join(name), join_nfs(&d.nfs, name))
        };
        let found = |tree: &Tree| -> Option<u32> {
            if !path.exists() { return None; }
            let mut t = tree.inner.write().unwrap();
            let ci = t.add(path.clone(), nfs.clone()).ok()?;
            let ch = &mut t.nodes[dir as usize].children;
            if !ch.iter().any(|(c, _)| c == name) { ch.push((name.to_string(), ci)); ch.sort(); }
            Some(ci)
        };
        if let Some(ci) = found(self) { return Some(ci); }
        let hook = self.on_miss.read().unwrap().clone();
        if let Some(h) = hook { h(&nfs); }
        found(self)
    }
    /// The children of directory `dir` as they are on the disk now.
    fn children(&self, dir: u32) -> Vec<(String, u32)> {
        let (path, nfs, overlays) = {
            let t = self.inner.read().unwrap();
            let Some(d) = t.nodes.get(dir as usize) else { return Vec::new() };
            // Children that live elsewhere (overlays) stay as they are.
            let ov: Vec<(String, u32)> = d.children.iter().filter(|(_, ci)| t.nodes[*ci as usize].path.parent() != Some(d.path.as_path())).cloned().collect();
            (d.path.clone(), d.nfs.clone(), ov)
        };
        let mut names: Vec<String> = fs::read_dir(&path).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| plain_name(n)).collect()).unwrap_or_default();
        names.retain(|n| !overlays.iter().any(|(o, _)| o == n));
        names.sort();
        let mut t = self.inner.write().unwrap();
        let mut out = overlays;
        for n in names {
            if let Ok(ci) = t.add(path.join(&n), join_nfs(&nfs, &n)) { out.push((n, ci)); }
        }
        out.sort();
        t.nodes[dir as usize].children = out.clone();
        out
    }
    fn len(&self) -> usize { self.inner.read().unwrap().nodes.len() }
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
/// File handles: a marker and the node index + 1.  The root's handle was
/// all zeros, which a CDJ-3000 took for no handle at all: it mounted and
/// then never sent an NFS call.
const FH_MARK: [u8; 4] = *b"ODK1";
fn fh_of(idx: u32) -> [u8; 32] {
    let mut f = [0u8; 32];
    f[..4].copy_from_slice(&FH_MARK);
    f[4..8].copy_from_slice(&(idx + 1).to_be_bytes());
    f
}
fn idx_of(fh: &[u8]) -> Option<u32> {
    let f = fh.get(..8)?;
    if f[..4] != FH_MARK { return None; }
    u32::from_be_bytes(f[4..8].try_into().ok()?).checked_sub(1)
}

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

/// This host's IPv4 addresses and netmasks.
fn local_v4() -> Vec<(std::net::Ipv4Addr, std::net::Ipv4Addr)> {
    if_addrs::get_if_addrs().map(|v| v.into_iter().filter_map(|i| match i.addr {
        if_addrs::IfAddr::V4(a) => Some((a.ip, a.netmask)),
        _ => None,
    }).collect()).unwrap_or_default()
}

/// The export group for a requester, as a CDJ writes it: the network of our
/// interface that shares the requester's subnet ("169.254.0.0/255.255.0.0").
/// A link-local requester with no matching interface gets 169.254/16; any
/// other gets everything.
pub fn export_group(requester: std::net::Ipv4Addr, local: &[(std::net::Ipv4Addr, std::net::Ipv4Addr)]) -> String {
    use std::net::Ipv4Addr;
    let net = |ip: Ipv4Addr, m: Ipv4Addr| Ipv4Addr::from(u32::from(ip) & u32::from(m));
    if let Some((ip, m)) = local.iter().find(|(ip, m)| !ip.is_loopback() && net(*ip, *m) == net(requester, *m)) {
        return format!("{}/{}", net(*ip, *m), m);
    }
    if requester.is_link_local() { return "169.254.0.0/255.255.0.0".into(); }
    "0.0.0.0/0.0.0.0".into()
}

fn serve_portmap(sock: UdpSocket, ports: Ports) {
    let mut buf = [0u8; 2048];
    while let Ok((n, from)) = sock.recv_from(&mut buf) {
        let d = &buf[..n];
        let Some((xid, prog, _vers, proc_, off)) = parse_call(d) else { log::debug!("portmap: {from} unparsed {n} bytes"); continue };
        log::debug!("portmap: {from} prog {prog} v{_vers} proc {proc_}");
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
        let Some((xid, prog, _v, proc_, off)) = parse_call(d) else { log::debug!("mountd: {from} unparsed {n} bytes"); continue };
        log::debug!("mountd: {from} prog {prog} v{_v} proc {proc_}");
        if prog != PROG_MOUNT { continue; }
        let body = match proc_ {
            0 => Vec::new(),
            1 => { // MNT: any export name mounts our root
                let name = get_opaque(d, off).map(|(b, _)| unwide(b)).unwrap_or_default();
                log::info!("mountd: {from} MNT {name:?}");
                let mut b = Vec::new(); put32(&mut b, 0); b.extend_from_slice(&fh_of(0)); b
            }
            3 => { log::info!("mountd: {from} UMNT"); Vec::new() }
            5 => { // EXPORT: one export, open to the requester's network
                // A player reads an empty group list as "not for you" and
                // stops there (a CDJ-3000 asked for EXPORT and never sent
                // MNT).  A CDJ names its subnet ("169.254.0.0/255.255.0.0"),
                // so we name the one the requester is on.
                let group = match from { std::net::SocketAddr::V4(f) => export_group(*f.ip(), &local_v4()), _ => "0.0.0.0/0.0.0.0".into() };
                log::info!("mountd: {from} EXPORT → {group}");
                let mut b = Vec::new();
                put32(&mut b, 1); put_opaque(&mut b, &export);
                put32(&mut b, 1); put_opaque(&mut b, group.as_bytes()); put32(&mut b, 0);
                put32(&mut b, 0); b
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
        let Some((xid, prog, _v, proc_, off)) = parse_call(d) else { log::debug!("nfsd: {from} unparsed {n} bytes"); continue };
        log::debug!("nfsd: {from} prog {prog} v{_v} proc {proc_}");
        if prog != PROG_NFS { continue; }
        let node_at = |o: usize| d.get(o..o + 32).and_then(idx_of).and_then(|i| tree.node(i).map(|nd| (i, nd)));
        let body = match proc_ {
            0 => Vec::new(),
            1 => match node_at(off) { // GETATTR
                Some((i, nd)) => { let mut b = Vec::new(); put32(&mut b, 0); fattr(&mut b, i, &nd); b }
                None => err(NFSERR_STALE),
            },
            4 => { // LOOKUP dir fh + name
                match node_at(off) {
                    Some((di, dir)) if dir.is_dir => {
                        let name = get_opaque(d, off + 32).map(|(b, _)| unwide(b)).unwrap_or_default();
                        match tree.lookup(di, &name).and_then(|ci| tree.node(ci).map(|nd| (ci, nd))) {
                            Some((ci, nd)) => {
                                log::debug!("nfsd: {from} LOOKUP {name:?} → {}", nd.path.display());
                                let mut b = Vec::new(); put32(&mut b, 0); b.extend_from_slice(&fh_of(ci)); fattr(&mut b, ci, &nd); b
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
                        let count = (be32(d, off + 36).unwrap_or(0) as usize).min(READ_CAP);
                        let mut data = vec![0u8; count];
                        let got = fs::File::open(&nd.path).and_then(|mut f| { f.seek(SeekFrom::Start(offset))?; let mut total = 0; while total < count { let k = f.read(&mut data[total..])?; if k == 0 { break; } total += k; } Ok(total) });
                        match got {
                            Ok(k) => { data.truncate(k); let mut b = Vec::new(); put32(&mut b, 0); fattr(&mut b, i, &nd); put_opaque(&mut b, &data); b }
                            Err(e) => { log::warn!("nfsd: read {}: {e}", nd.path.display()); err(NFSERR_IO) }
                        }
                    }
                    Some(_) => err(NFSERR_ACCES),
                    None => err(NFSERR_STALE),
                }
            }
            16 => { // READDIR fh, cookie, count
                match node_at(off) {
                    Some((di, dir)) if dir.is_dir => {
                        let children = tree.children(di);
                        let cookie = be32(d, off + 32).unwrap_or(0) as usize;
                        let count = be32(d, off + 36).unwrap_or(4096) as usize;
                        let mut b = Vec::new(); put32(&mut b, 0);
                        let mut used = 16; let mut i = cookie; let mut eof = true;
                        while i < children.len() {
                            let (name, ci) = &children[i];
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
pub struct NfsServer { pub portmap: u16, pub mountd: u16, pub nfsd: u16, pub tree: Arc<Tree> }

impl NfsServer {
    /// Serve `tree` as export `export_name` (e.g. `/C/`).  `portmap_port` is
    /// 111 for a player-style server or 50111 rekordbox-style; `nfsd_port` 0
    /// = ephemeral (players use 2049).
    pub fn start(tree: Tree, export_name: &str, portmap_port: u16, nfsd_port: u16) -> Result<NfsServer> {
        let any = |p: u16| UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, p));
        // 111 needs privilege (root on Linux; never granted to an iOS app), so
        // an unprivileged server does what rekordbox does and answers on
        // 50111 instead; clients try both (`Nfs::connect_any`).
        let pm = match any(portmap_port) {
            Ok(s) => s,
            Err(e) if portmap_port == crate::PORTMAP_PLAYER => {
                log::info!("nfs: portmap {portmap_port} not bindable ({e}); serving on {} like rekordbox", crate::PORTMAP_REKORDBOX);
                any(crate::PORTMAP_REKORDBOX).with_context(|| format!("bind portmap {}", crate::PORTMAP_REKORDBOX))?
            }
            Err(e) => return Err(e).with_context(|| format!("bind portmap {portmap_port}")),
        };
        let md = any(0).context("bind mountd")?;
        let nd = any(nfsd_port).or_else(|_| any(0)).context("bind nfsd")?;
        let tree = Arc::new(tree);
        let ports = NfsServer { portmap: pm.local_addr()?.port(), mountd: md.local_addr()?.port(), nfsd: nd.local_addr()?.port(), tree: Arc::clone(&tree) };
        log::info!("nfs: portmap {} mountd {} nfsd {}; export {export_name:?} = {} nodes", ports.portmap, ports.mountd, ports.nfsd, tree.len());
        let p = Ports { mountd: ports.mountd, nfsd: ports.nfsd };
        thread::Builder::new().name("portmap".into()).spawn(move || serve_portmap(pm, p))?;
        let export = wide(export_name);
        thread::Builder::new().name("mountd".into()).spawn(move || serve_mountd(md, export))?;
        thread::Builder::new().name("nfsd".into()).spawn(move || serve_nfsd(nd, tree))?;
        Ok(ports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_a_players_32k_read_whole() {
        let dir = tempfile::tempdir().unwrap();
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.path().join("t.mp3"), &body).unwrap();
        let srv = NfsServer::start(Tree::scan(dir.path()).unwrap(), "/C/", 0, 0).unwrap();
        let mut c = crate::Nfs::connect_at(Ipv4Addr::LOCALHOST, srv.portmap).unwrap();
        let root = c.mount_usb().unwrap();
        let (fh, size) = c.lookup_path(&root, "t.mp3").unwrap();
        assert_eq!(size, 100_000);
        let got = c.read(&fh, 32_768, 32_768).unwrap();
        assert_eq!(got.len(), 32_768, "a 32 KiB read comes back whole");
        assert_eq!(&got[..], &body[32_768..65_536]);
    }

    /// A media server writes stems while it serves: a player must find a
    /// file made after the server started, read a file as it grows, list
    /// a new folder, and see a cache folder served in place of one.
    #[test]
    fn a_player_sees_files_made_while_the_server_runs() {
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.mp3"), b"mp3").unwrap();
        std::fs::create_dir_all(dir.path().join("CDJMODS")).unwrap();
        std::fs::write(dir.path().join("CDJMODS/index.json"), b"stick's").unwrap();
        let srv = NfsServer::start(Tree::scan(dir.path()).unwrap(), "/C/", 0, 0).unwrap();
        srv.tree.overlay("CDJMODS", cache.path()).unwrap();
        let mut c = crate::Nfs::connect_at(Ipv4Addr::LOCALHOST, srv.portmap).unwrap();
        let root = c.mount_usb().unwrap();

        assert!(c.lookup_path_opt(&root, "CDJMODS/index.json").unwrap().is_none(), "the overlay hides the stick's CDJMODS");
        std::fs::create_dir_all(cache.path().join("live/7")).unwrap();
        std::fs::write(cache.path().join("live/7/map"), [0u8; 4]).unwrap();
        let (fh, size) = c.lookup_path(&root, "CDJMODS/live/7/map").expect("made after the start");
        assert_eq!(size, 4);
        std::fs::write(cache.path().join("live/7/map"), [1u8; 9]).unwrap();
        assert_eq!(c.read(&fh, 0, 64).unwrap(), vec![1u8; 9], "read at its new size");
        std::fs::write(dir.path().join("new.mp3"), b"x").unwrap();
        let names: Vec<String> = c.readdir(&root).unwrap().into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"new.mp3".to_string()) && names.contains(&"CDJMODS".to_string()), "{names:?}");
        assert_eq!(names.iter().filter(|n| *n == "CDJMODS").count(), 1, "{names:?}");
    }

    /// A lookup that misses asks the hook first, which may make the file.
    #[test]
    fn a_missed_lookup_asks_the_hook_and_finds_what_it_made() {
        let dir = tempfile::tempdir().unwrap();
        let srv = NfsServer::start(Tree::scan(dir.path()).unwrap(), "/C/", 0, 0).unwrap();
        let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (a, root_dir) = (Arc::clone(&asked), dir.path().to_path_buf());
        srv.tree.on_miss(Arc::new(move |p: &str| {
            a.lock().unwrap().push(p.to_string());
            if p == "/live" { std::fs::create_dir_all(root_dir.join("live")).unwrap(); }
        }));
        let mut c = crate::Nfs::connect_at(Ipv4Addr::LOCALHOST, srv.portmap).unwrap();
        let root = c.mount_usb().unwrap();
        assert!(c.lookup_path_opt(&root, "live").unwrap().is_some(), "the hook made it");
        assert!(c.lookup_path_opt(&root, "nothing").unwrap().is_none());
        assert!(c.lookup_path_opt(&root, "..").unwrap().is_none(), "never outside the export");
        assert_eq!(*asked.lock().unwrap(), vec!["/live".to_string(), "/nothing".to_string()]);
    }

    #[test]
    fn file_handles_are_never_blank_and_round_trip() {
        for i in [0u32, 1, 41, 1_000_000] {
            let fh = fh_of(i);
            assert!(fh.iter().any(|b| *b != 0), "handle for {i} is blank");
            assert_eq!(idx_of(&fh), Some(i));
        }
        assert_eq!(idx_of(&[0u8; 32]), None, "a blank handle names nothing");
    }

    #[test]
    fn export_group_names_the_requesters_network() {
        use std::net::Ipv4Addr as I;
        let local = [(I::new(127, 0, 0, 1), I::new(255, 0, 0, 0)),
                     (I::new(192, 168, 68, 64), I::new(255, 255, 252, 0)),
                     (I::new(169, 254, 77, 1), I::new(255, 255, 0, 0))];
        assert_eq!(export_group(I::new(169, 254, 5, 55), &local), "169.254.0.0/255.255.0.0");
        assert_eq!(export_group(I::new(192, 168, 69, 10), &local), "192.168.68.0/255.255.252.0");
        assert_eq!(export_group(I::new(169, 254, 5, 55), &[]), "169.254.0.0/255.255.0.0");
        assert_eq!(export_group(I::new(10, 0, 0, 5), &local), "0.0.0.0/0.0.0.0");
    }
}
