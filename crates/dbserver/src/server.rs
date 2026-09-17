//! The **server** half of the remote-database protocol: what a CDJ (or
//! rekordbox) talks to when it browses *our* media over Link.
//!
//! One thread answers the port query on TCP 12523; the database service itself
//! listens on a second port (given, or ephemeral).  Each client connection is
//! a thread running the request loop: greeting echo → setup → menu requests,
//! where a menu request is answered with a `0x4000` item count and the client
//! then asks us to *render* it (`0x3000`) with an offset and limit.
//!
//! Anything we don't understand is answered with `0x4003` (unavailable) and
//! logged at info level, so a capture-free session against rekordbox still
//! shows which requests a real client makes.

use crate::{kind, Field, Message, Slot, TrackType};
use anyhow::{Context, Result};
use std::io::{BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

/// A track we serve.  `id` is what clients use in every later request.
#[derive(Clone, Debug)]
pub struct Track {
    pub id:         u32,
    pub title:      String,
    pub artist:     String,
    pub album:      String,
    /// Absolute path *as the NFS client should request it*, e.g.
    /// `/Contents/x.mp3` under our export.
    pub path:       String,
    pub duration_s: u32,
    pub bpm:        f32,
    pub comment:    String,
    pub bitrate:    u32,
    pub date_added: String,
    /// Beat grid, if analysed: (beat-in-bar 1–4, bpm, ms).
    pub beats:      Vec<(u8, f32, u32)>,
}

/// What the server hands out.  Immutable for the life of the server.
pub struct Library {
    pub tracks: Vec<Track>,
    pub name:   String,
}

impl Library {
    fn track(&self, id: u32) -> Option<&Track> { self.tracks.iter().find(|t| t.id == id) }
}

/// Handle to a running server.
pub struct Server { pub db_port: u16, pub device: u8 }

fn s(v: &str) -> Field { Field::Str(v.to_string()) }
fn u(v: u32) -> Field { Field::U32(v) }
fn slen(v: &str) -> Field { Field::U32(((v.encode_utf16().count() + 1) * 2) as u32) }

/// A menu row (`0x4101`): parent, id, len1, label1, len2, label2, type, flags,
/// artwork, position, 0, 0.
fn row(parent: u32, id: u32, label: &str, label2: &str, item_type: u32, art: u32) -> Vec<Field> {
    vec![u(parent), u(id), slen(label), s(label), slen(label2), s(label2), u(item_type), u(0), u(art), u(0), u(0), u(0)]
}

/// Menu rows for the requests we implement.  `None` = unavailable.
fn menu_rows(lib: &Library, kind_: u16, args: &[Field]) -> Option<Vec<Vec<Field>>> {
    let arg = |i: usize| args.get(i).map(|f| f.as_u32()).unwrap_or(0);
    Some(match kind_ {
        // Root menu: the browse categories a player shows for a slot.
        0x1000 => vec![
            row(0, 0x84, "TRACK", "", 0x0084, 0),
            row(0, 0x85, "PLAYLIST", "", 0x0085, 0),
            row(0, 0x82, "ARTIST", "", 0x0082, 0),
            row(0, 0x94, "FILENAME", "", 0x0094, 0),
        ],
        // All tracks (sort ignored).
        kind::ALL_TRACKS | 0x1002 | 0x1003 | 0x1006 | 0x1007 | 0x100a | 0x100b | 0x100d | 0x100e | 0x100f | 0x1010 | 0x1011 | 0x1012 | 0x1013 | 0x1014 | 0x1015 | 0x1016 => {
            if kind_ == kind::ALL_TRACKS || kind_ == 0x1014 || kind_ == 0x1015 || kind_ == 0x1016 {
                lib.tracks.iter().map(|t| row(0, t.id, &t.title, &t.artist, 0x0004, 0)).collect()
            } else if kind_ == 0x1002 { // artist menu
                let mut names: Vec<&str> = lib.tracks.iter().map(|t| t.artist.as_str()).filter(|a| !a.is_empty()).collect();
                names.sort(); names.dedup();
                names.iter().enumerate().map(|(i, a)| row(0, i as u32 + 1, a, "", 0x0007, 0)).collect()
            } else { Vec::new() }
        }
        // Playlist tree: we have no playlists.
        kind::PLAYLIST => Vec::new(),
        // Track metadata: the 16 rows rekordbox 7 returns, in its order.
        kind::METADATA => {
            let t = lib.track(arg(1))?;
            vec![
                row(t.id, t.id, &t.title, "", 0x0004, 0),
                row(0, 1, &t.artist, "", 0x0007, 0),
                row(0, 0, &t.album, "", 0x0002, 0),
                row(0, t.duration_s, "", "", 0x000b, 0),
                row(0, (t.bpm * 100.0).round() as u32, "", "", 0x000d, 0),
                row(0, 0, "", "", 0x000f, 0),
                row(0, 0, "", "", 0x000a, 0),
                row(0, 0, "", "", 0x0013, 0),
                row(0, 0, "", "", 0x0006, 0),
                row(t.id, t.id, &t.date_added, "", 0x002e, 0),
                row(t.id, t.id, &t.comment, "", 0x0023, 0),
                row(0, t.bitrate, "", "", 0x0010, 0),
                row(0, 0, "", "", 0x0011, 0),
                row(0, 0, "", "", 0x000e, 0),
                row(0, 0, "", "", 0x0028, 0),
                row(0, 0, "", "", 0x0029, 0),
            ]
        }
        // Track info: the row with type 0 carries the path the client then
        // reads over NFS.
        kind::TRACK_INFO => {
            let t = lib.track(arg(1))?;
            vec![
                row(0, 1, "", "", 0x0004, 0),
                row(0, t.duration_s, "", "", 0x000b, 0),
                row(0, (t.bpm * 100.0).round() as u32, "", "", 0x000d, 0),
                row(t.id, t.id, &t.comment, "", 0x0023, 0),
                row(t.id, t.id, &t.path, "", 0x0000, 0),
                row(0, 1, "", "", 0x002f, 0),
                row(0, 0, "", "", 0x000f, 0),
            ]
        }
        _ => return None,
    })
}

/// Beat grid blob (`0x4602`): 20-byte header then 16-byte entries.
fn beat_grid_blob(beats: &[(u8, f32, u32)]) -> Vec<u8> {
    let mut b = vec![0u8; 20];
    b[4..8].copy_from_slice(&(beats.len() as u32).to_le_bytes());
    for &(bib, bpm, ms) in beats {
        let mut e = [0xffu8; 16];
        e[0] = bib; e[1] = 0;
        e[2..4].copy_from_slice(&((bpm * 100.0).round() as u16).to_le_bytes());
        e[4..8].copy_from_slice(&ms.to_le_bytes());
        b.extend_from_slice(&e);
    }
    b
}

fn serve_client(lib: Arc<Library>, device: u8, stream: TcpStream) -> Result<()> {
    let peer = stream.peer_addr().ok();
    let mut w = stream.try_clone()?;
    let mut r = BufReader::new(stream);
    // Greeting: the client sends a lone u32 field (1); echo it.
    let hello = Field::read(&mut r).context("greeting")?;
    let mut buf = Vec::new(); hello.encode(&mut buf); w.write_all(&buf)?;
    let mut pending: Option<Vec<Vec<Field>>> = None;
    loop {
        let m = match Message::read(&mut r) { Ok(m) => m, Err(_) => break };
        let reply = |w: &mut TcpStream, m: Message| -> Result<()> { w.write_all(&m.encode())?; Ok(()) };
        match m.kind {
            kind::SETUP => {
                log::info!("dbserver: {peer:?} set up as device {}", m.args.first().map(|f| f.as_u32()).unwrap_or(0));
                reply(&mut w, Message { txid: m.txid, kind: kind::MENU_AVAILABLE, args: vec![u(0), u(device as u32)] })?;
            }
            kind::RENDER => {
                let offset = m.args.get(1).map(|f| f.as_u32()).unwrap_or(0) as usize;
                let limit  = m.args.get(2).map(|f| f.as_u32()).unwrap_or(0) as usize;
                let rows = pending.clone().unwrap_or_default();
                reply(&mut w, Message { txid: m.txid, kind: kind::MENU_HEADER, args: vec![u(1), u(0)] })?;
                for r in rows.iter().skip(offset).take(limit.max(1)) {
                    reply(&mut w, Message { txid: m.txid, kind: kind::MENU_ITEM, args: r.clone() })?;
                }
                reply(&mut w, Message { txid: m.txid, kind: kind::MENU_FOOTER, args: vec![] })?;
            }
            kind::BEAT_GRID => {
                let id = m.args.get(1).map(|f| f.as_u32()).unwrap_or(0);
                match lib.track(id).filter(|t| !t.beats.is_empty()) {
                    Some(t) => {
                        let blob = beat_grid_blob(&t.beats);
                        reply(&mut w, Message { txid: m.txid, kind: kind::BEAT_GRID_BLOB,
                            args: vec![u(m.kind as u32), u(0), u(blob.len() as u32), Field::Blob(blob), u(0)] })?;
                    }
                    None => reply(&mut w, Message { txid: m.txid, kind: kind::UNAVAILABLE, args: vec![u(m.kind as u32)] })?,
                }
            }
            k if (0x1000..0x3000).contains(&k) => match menu_rows(&lib, k, &m.args) {
                Some(rows) => {
                    log::info!("dbserver: {peer:?} menu 0x{k:04x} {:?} → {} rows", m.args.iter().map(|f| f.as_u32()).collect::<Vec<_>>(), rows.len());
                    let n = rows.len() as u32;
                    pending = Some(rows);
                    reply(&mut w, Message { txid: m.txid, kind: kind::MENU_AVAILABLE, args: vec![u(k as u32), u(n)] })?;
                }
                None => {
                    log::info!("dbserver: {peer:?} unhandled 0x{k:04x} args {:?}", m.args);
                    reply(&mut w, Message { txid: m.txid, kind: kind::UNAVAILABLE, args: vec![u(k as u32)] })?;
                }
            },
            k => {
                log::info!("dbserver: {peer:?} unknown 0x{k:04x} args {:?}", m.args);
                reply(&mut w, Message { txid: m.txid, kind: kind::UNAVAILABLE, args: vec![u(k as u32)] })?;
            }
        }
    }
    log::info!("dbserver: {peer:?} closed");
    Ok(())
}

impl Server {
    /// Start the port-query service (TCP 12523) and the database service
    /// (`db_port`, 0 = ephemeral).  Threads run for the life of the process.
    pub fn start(lib: Library, device: u8, db_port: u16) -> Result<Server> {
        let lib = Arc::new(lib);
        let db = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, db_port)).context("bind dbserver")?;
        let db_port = db.local_addr()?.port();
        let pq = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, crate::PORT_QUERY)).context("bind 12523")?;
        log::info!("dbserver: port query on 12523, database on {db_port}, {} tracks", lib.tracks.len());
        thread::Builder::new().name("dbserver-portquery".into()).spawn(move || {
            for c in pq.incoming().flatten() {
                let mut c = c;
                let mut q = [0u8; 64];
                let _ = std::io::Read::read(&mut c, &mut q);
                log::info!("dbserver: port query from {:?}", c.peer_addr().ok());
                let _ = c.write_all(&db_port.to_be_bytes());
            }
        })?;
        let lib2 = Arc::clone(&lib);
        thread::Builder::new().name("dbserver-accept".into()).spawn(move || {
            for c in db.incoming().flatten() {
                let lib = Arc::clone(&lib2);
                let _ = c.set_nodelay(true);
                thread::spawn(move || { if let Err(e) = serve_client(lib, device, c) { log::warn!("dbserver client: {e:#}"); } });
            }
        })?;
        Ok(Server { db_port, device })
    }
}

// Keep Slot / TrackType referenced for callers building DMST-shaped things.
#[allow(dead_code)] fn _slots(_: Slot, _: TrackType) {}
