//! The **server** half of the remote-database protocol: what a CDJ (or
//! rekordbox) talks to when it browses *our* media over Link.
//!
//! One thread answers the port query on TCP 12523; the database service itself
//! listens on a second port (given, or ephemeral).  Each client connection is
//! a thread running the request loop: greeting echo → setup → menu requests,
//! where a menu request is answered with a `0x4000` item count and the client
//! then asks us to *render* it (`0x3000`) with an offset and limit.
//!
//! The library is shared and mutable ([`SharedLibrary`]): the server starts
//! with whatever a folder scan found (file names) and an analysis thread
//! fills in tags, duration, tempo, beat grid and waveforms as it gets to each
//! track (`opendeck-mediaserver`).  Clients see the richer rows on their next
//! request.
//!
//! Anything we don't understand is answered with `0x4003` (unavailable) and
//! logged at info level, so a capture-free session against a real player
//! still shows which requests it makes.

use crate::{kind, Field, Message, Slot, TrackType};
use anyhow::{Context, Result};
use std::io::{BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::{Arc, RwLock};
use std::thread;

/// A track we serve.  `id` is what clients use in every later request.
#[derive(Clone, Debug, Default)]
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
    pub key:        String,
    pub comment:    String,
    pub bitrate:    u32,
    pub date_added: String,
    /// Beat grid, if analysed: (beat-in-bar 1–4, bpm, ms).
    pub beats:      Vec<(u8, f32, u32)>,
    /// Waveform preview (`0x4402`): 400 columns as Beat Link decodes them
    /// from a player — a (height 0–31, whiteness 0–7) byte pair each, 800
    /// bytes.  Empty = not analysed yet.
    pub preview:    Vec<u8>,
    /// Waveform detail (`0x4a02`): one byte per half-frame (150 per second),
    /// low 5 bits height, high 3 bits whiteness.  Empty = not analysed yet.
    pub detail:     Vec<u8>,
    /// The file has embedded cover art; menu rows then carry the track id as
    /// the artwork id and `0x2003` fetches it through [`Library::art`].
    pub has_art:    bool,
}

impl Track {
    /// The file name, for the FILENAME category.
    pub fn file_name(&self) -> &str { self.path.rsplit('/').next().unwrap_or(&self.path) }
}

/// Fetches a track's cover art by track id, on demand (the bytes are not kept
/// in the library; a JPEG per track adds up).
pub type ArtLoader = Arc<dyn Fn(u32) -> Option<Vec<u8>> + Send + Sync>;

/// What the server hands out.
#[derive(Default)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub name:   String,
    pub art:    Option<ArtLoader>,
}

/// The library as the server and the analysis thread share it.
pub type SharedLibrary = Arc<RwLock<Library>>;

impl Library {
    pub fn shared(self) -> SharedLibrary { Arc::new(RwLock::new(self)) }
    pub fn track(&self, id: u32) -> Option<&Track> { self.tracks.iter().find(|t| t.id == id) }
    pub fn track_mut(&mut self, id: u32) -> Option<&mut Track> { self.tracks.iter_mut().find(|t| t.id == id) }
}

/// Handle to a running server.
pub struct Server { pub db_port: u16, pub device: u8 }

fn s(v: &str) -> Field { Field::Str(v.to_string()) }
fn u(v: u32) -> Field { Field::U32(v) }
fn slen(v: &str) -> Field { Field::U32(((v.encode_utf16().count() + 1) * 2) as u32) }

/// A stable menu id for an artist or album name: FNV-1a, kept off 0 and the
/// `0xffffffff` "ALL" sentinel.  Names, not indexes, so the id survives the
/// analysis thread renaming tracks between a menu and its drill-down.
pub fn name_id(name: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes() { h ^= b as u32; h = h.wrapping_mul(0x0100_0193); }
    match h { 0 => 1, 0xffff_ffff => 0xffff_fffe, h => h }
}

/// A menu row (`0x4101`): parent, id, len1, label1, len2, label2, type, flags,
/// artwork, position, 0, 0.
fn row(parent: u32, id: u32, label: &str, label2: &str, item_type: u32, art: u32) -> Vec<Field> {
    vec![u(parent), u(id), slen(label), s(label), slen(label2), s(label2), u(item_type), u(0), u(art), u(0), u(0), u(0)]
}

fn track_row(t: &Track, label: &str) -> Vec<Field> {
    row(0, t.id, label, &t.artist, 0x0004, if t.has_art { t.id } else { 0 })
}

/// Distinct non-empty names as (id, name), sorted by name.
fn names<'a>(it: impl Iterator<Item = &'a str>) -> Vec<(u32, &'a str)> {
    let mut v: Vec<&str> = it.filter(|n| !n.is_empty()).collect();
    v.sort_unstable(); v.dedup();
    v.into_iter().map(|n| (name_id(n), n)).collect()
}

/// Menu rows for the requests we implement.  `None` = unavailable.
fn menu_rows(lib: &Library, kind_: u16, args: &[Field]) -> Option<Vec<Vec<Field>>> {
    let arg = |i: usize| args.get(i).map(|f| f.as_u32()).unwrap_or(0);
    let all = |mut ts: Vec<&Track>| -> Vec<Vec<Field>> {
        ts.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        ts.iter().map(|t| track_row(t, &t.title)).collect()
    };
    Some(match kind_ {
        // Root menu: the browse categories a player shows for a slot.  The
        // same four rekordbox 7 serves, plus ALBUM (players list it too).
        kind::ROOT_MENU => vec![
            row(0, 0x85, "PLAYLIST", "", 0x0085, 0),
            row(0, 0x82, "ARTIST", "", 0x0082, 0),
            row(0, 0x83, "ALBUM", "", 0x0083, 0),
            row(0, 0x84, "TRACK", "", 0x0084, 0),
            row(0, 0x94, "FILENAME", "", 0x0094, 0),
        ],
        // All tracks (sort ignored) and its sort-by variants.
        kind::ALL_TRACKS | 0x1014 | 0x1015 | 0x1016 => all(lib.tracks.iter().collect()),
        kind::FILENAME_MENU => {
            let mut ts: Vec<&Track> = lib.tracks.iter().collect();
            ts.sort_by_key(|t| t.file_name().to_lowercase());
            ts.iter().map(|t| track_row(t, t.file_name())).collect()
        }
        kind::ARTIST_MENU => names(lib.tracks.iter().map(|t| t.artist.as_str())).into_iter()
            .map(|(id, a)| row(0, id, a, "", 0x0007, 0)).collect(),
        kind::ALBUM_MENU => names(lib.tracks.iter().map(|t| t.album.as_str())).into_iter()
            .map(|(id, a)| row(0, id, a, "", 0x0002, 0)).collect(),
        // Albums of one artist (sort, artist id).
        kind::ALBUMS_FOR_ARTIST => {
            let artist = arg(2);
            names(lib.tracks.iter().filter(|t| name_id(&t.artist) == artist).map(|t| t.album.as_str())).into_iter()
                .map(|(id, a)| row(0, id, a, "", 0x0002, 0)).collect()
        }
        // Tracks of one album (sort, album id).
        kind::TRACKS_FOR_ALBUM => {
            let album = arg(2);
            all(lib.tracks.iter().filter(|t| name_id(&t.album) == album).collect())
        }
        // Tracks of one artist (sort, artist id, album id or 0xffffffff = all).
        kind::TRACKS_FOR_ARTIST_ALBUM => {
            let (artist, album) = (arg(2), arg(3));
            all(lib.tracks.iter().filter(|t| name_id(&t.artist) == artist && (album == 0xffff_ffff || name_id(&t.album) == album)).collect())
        }
        // Playlist tree: we have no playlists.
        kind::PLAYLIST => Vec::new(),
        // Track metadata: the 16 rows rekordbox 7 returns, in its order.
        kind::METADATA => {
            let t = lib.track(arg(1))?;
            let art = if t.has_art { t.id } else { 0 };
            vec![
                row(t.id, t.id, &t.title, "", 0x0004, art),
                row(0, name_id(&t.artist), &t.artist, "", 0x0007, 0),
                row(0, name_id(&t.album), &t.album, "", 0x0002, 0),
                row(0, t.duration_s, "", "", 0x000b, 0),
                row(0, (t.bpm * 100.0).round() as u32, "", "", 0x000d, 0),
                row(0, 0, &t.key, "", 0x000f, 0),
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
                row(0, 0, &t.key, "", 0x000f, 0),
            ]
        }
        _ => return None,
    })
}

/// Beat grid blob (`0x4602`): 20-byte header then 16-byte entries.
pub fn beat_grid_blob(beats: &[(u8, f32, u32)]) -> Vec<u8> {
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

/// Bytes Beat Link skips at the front of a `0x4a02` detail blob.
pub const DETAIL_LEAD: usize = 19;

/// A blob reply: request type, 0, length, bytes (Beat Link's argument list
/// for artwork, preview and detail; the beat grid adds a trailing 0).
fn blob_reply(txid: u32, want: u16, req: u16, blob: Vec<u8>, trailing_zero: bool) -> Message {
    let mut args = vec![u(req as u32), u(0), u(blob.len() as u32), Field::Blob(blob)];
    if trailing_zero { args.push(u(0)); }
    Message { txid, kind: want, args }
}

fn serve_client(lib: SharedLibrary, device: u8, stream: TcpStream) -> Result<()> {
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
        let unavailable = |w: &mut TcpStream| reply(w, Message { txid: m.txid, kind: kind::UNAVAILABLE, args: vec![u(m.kind as u32)] });
        let arg = |i: usize| m.args.get(i).map(|f| f.as_u32()).unwrap_or(0);
        match m.kind {
            kind::SETUP => {
                log::info!("dbserver: {peer:?} set up as device {}", arg(0));
                reply(&mut w, Message { txid: m.txid, kind: kind::MENU_AVAILABLE, args: vec![u(0), u(device as u32)] })?;
            }
            kind::RENDER => {
                let offset = arg(1) as usize;
                let limit  = arg(2) as usize;
                let rows = pending.clone().unwrap_or_default();
                reply(&mut w, Message { txid: m.txid, kind: kind::MENU_HEADER, args: vec![u(1), u(0)] })?;
                for r in rows.iter().skip(offset).take(limit.max(1)) {
                    reply(&mut w, Message { txid: m.txid, kind: kind::MENU_ITEM, args: r.clone() })?;
                }
                reply(&mut w, Message { txid: m.txid, kind: kind::MENU_FOOTER, args: vec![] })?;
            }
            kind::BEAT_GRID => {
                let blob = lib.read().unwrap().track(arg(1)).filter(|t| !t.beats.is_empty()).map(|t| beat_grid_blob(&t.beats));
                match blob {
                    Some(b) => reply(&mut w, blob_reply(m.txid, kind::BEAT_GRID_BLOB, m.kind, b, true))?,
                    None => unavailable(&mut w)?,
                }
            }
            // Waveform preview: (dmst, 4, track id, 0).
            kind::WAVE_PREVIEW => {
                let blob = lib.read().unwrap().track(arg(2)).filter(|t| !t.preview.is_empty()).map(|t| t.preview.clone());
                match blob {
                    Some(b) => reply(&mut w, blob_reply(m.txid, kind::WAVE_PREVIEW_BLOB, m.kind, b, false))?,
                    None => unavailable(&mut w)?,
                }
            }
            // Waveform detail: (dmst, track id, 0).
            kind::WAVE_DETAIL => {
                let blob = lib.read().unwrap().track(arg(1)).filter(|t| !t.detail.is_empty())
                    .map(|t| { let mut b = vec![0u8; DETAIL_LEAD]; b.extend_from_slice(&t.detail); b });
                match blob {
                    Some(b) => reply(&mut w, blob_reply(m.txid, kind::WAVE_DETAIL_BLOB, m.kind, b, false))?,
                    None => unavailable(&mut w)?,
                }
            }
            // Artwork: (dmst, artwork id); our artwork ids are track ids.
            kind::ARTWORK => {
                let art = lib.read().unwrap().art.clone();
                match art.and_then(|f| f(arg(1))) {
                    Some(b) => reply(&mut w, blob_reply(m.txid, kind::ARTWORK_BLOB, m.kind, b, false))?,
                    None => unavailable(&mut w)?,
                }
            }
            // Cue lists and raw ANLZ sections: nothing to serve (no memory
            // cues of our own yet; colour waveforms are not produced).
            kind::CUES | kind::CUES_EXT | kind::ANLZ_TAG => {
                log::debug!("dbserver: {peer:?} 0x{:04x} for track {} — none", m.kind, arg(1));
                unavailable(&mut w)?;
            }
            k if (0x1000..0x3000).contains(&k) => match menu_rows(&lib.read().unwrap(), k, &m.args) {
                Some(rows) => {
                    log::info!("dbserver: {peer:?} menu 0x{k:04x} {:?} → {} rows", m.args.iter().map(|f| f.as_u32()).collect::<Vec<_>>(), rows.len());
                    let n = rows.len() as u32;
                    pending = Some(rows);
                    reply(&mut w, Message { txid: m.txid, kind: kind::MENU_AVAILABLE, args: vec![u(k as u32), u(n)] })?;
                }
                None => {
                    log::info!("dbserver: {peer:?} unhandled 0x{k:04x} args {:?}", m.args);
                    unavailable(&mut w)?;
                }
            },
            k => {
                log::info!("dbserver: {peer:?} unknown 0x{k:04x} args {:?}", m.args);
                unavailable(&mut w)?;
            }
        }
    }
    log::info!("dbserver: {peer:?} closed");
    Ok(())
}

impl Server {
    /// Start the port-query service (TCP 12523) and the database service
    /// (`db_port`, 0 = ephemeral).  Threads run for the life of the process.
    pub fn start(lib: SharedLibrary, device: u8, db_port: u16) -> Result<Server> {
        let db = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, db_port)).context("bind dbserver")?;
        let db_port = db.local_addr()?.port();
        let pq = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, crate::PORT_QUERY)).context("bind 12523")?;
        log::info!("dbserver: port query on 12523, database on {db_port}, {} tracks", lib.read().unwrap().tracks.len());
        thread::Builder::new().name("dbserver-portquery".into()).spawn(move || {
            for c in pq.incoming().flatten() {
                let mut c = c;
                let mut q = [0u8; 64];
                let _ = std::io::Read::read(&mut c, &mut q);
                log::info!("dbserver: port query from {:?}", c.peer_addr().ok());
                let _ = c.write_all(&db_port.to_be_bytes());
            }
        })?;
        thread::Builder::new().name("dbserver-accept".into()).spawn(move || {
            for c in db.incoming().flatten() {
                let lib = Arc::clone(&lib);
                let _ = c.set_nodelay(true);
                thread::spawn(move || { if let Err(e) = serve_client(lib, device, c) { log::warn!("dbserver client: {e:#}"); } });
            }
        })?;
        Ok(Server { db_port, device })
    }
}

// Keep Slot / TrackType referenced for callers building DMST-shaped things.
#[allow(dead_code)] fn _slots(_: Slot, _: TrackType) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MenuItem;

    fn lib() -> Library {
        let t = |id, title: &str, artist: &str, album: &str, path: &str| Track {
            id, title: title.into(), artist: artist.into(), album: album.into(), path: path.into(), ..Default::default()
        };
        Library { name: "T".into(), art: None, tracks: vec![
            t(1, "Beta", "Ann", "One", "/b.mp3"),
            t(2, "Alpha", "Bob", "Two", "/a.mp3"),
            t(3, "Gamma", "Ann", "Two", "/c.mp3"),
        ] }
    }

    fn items(rows: Vec<Vec<Field>>) -> Vec<MenuItem> {
        rows.into_iter().map(|args| MenuItem::from_message(&Message { txid: 0, kind: kind::MENU_ITEM, args }).unwrap()).collect()
    }
    fn dmst() -> Field { Field::U32(0x0101_0301) }

    #[test]
    fn root_menu_lists_the_categories() {
        let rows = items(menu_rows(&lib(), kind::ROOT_MENU, &[dmst(), u(0), u(0xffffff)]).unwrap());
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["PLAYLIST", "ARTIST", "ALBUM", "TRACK", "FILENAME"]);
        assert!(rows.iter().all(|r| r.type_name().starts_with("root-")));
    }

    #[test]
    fn tracks_sort_by_title_and_filenames_by_name() {
        let by_title = items(menu_rows(&lib(), kind::ALL_TRACKS, &[dmst(), u(0)]).unwrap());
        assert_eq!(by_title.iter().map(|r| r.id).collect::<Vec<_>>(), [2, 1, 3]);
        assert_eq!(by_title[0].label2, "Bob");
        let by_file = items(menu_rows(&lib(), kind::FILENAME_MENU, &[dmst(), u(0)]).unwrap());
        assert_eq!(by_file.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["a.mp3", "b.mp3", "c.mp3"]);
    }

    #[test]
    fn artists_and_albums_drill_down() {
        let artists = items(menu_rows(&lib(), kind::ARTIST_MENU, &[dmst(), u(0)]).unwrap());
        assert_eq!(artists.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["Ann", "Bob"]);
        let ann = artists[0].id;
        assert_eq!(ann, name_id("Ann"));
        let albums = items(menu_rows(&lib(), kind::ALBUMS_FOR_ARTIST, &[dmst(), u(0), u(ann)]).unwrap());
        assert_eq!(albums.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["One", "Two"]);
        let all = items(menu_rows(&lib(), kind::TRACKS_FOR_ARTIST_ALBUM, &[dmst(), u(0), u(ann), u(0xffff_ffff)]).unwrap());
        assert_eq!(all.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 3]);
        let two = items(menu_rows(&lib(), kind::TRACKS_FOR_ALBUM, &[dmst(), u(0), u(name_id("Two"))]).unwrap());
        assert_eq!(two.iter().map(|r| r.id).collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn track_info_carries_the_path_in_row_type_zero() {
        let rows = items(menu_rows(&lib(), kind::TRACK_INFO, &[dmst(), u(2)]).unwrap());
        let path = rows.iter().find(|r| r.item_type & 0xffff == 0).unwrap();
        assert_eq!(path.label, "/a.mp3");
        assert!(menu_rows(&lib(), kind::TRACK_INFO, &[dmst(), u(9)]).is_none());
    }

    #[test]
    fn beat_grid_blob_round_trips_through_the_client_parser() {
        let beats = vec![(1u8, 128.0f32, 25u32), (2, 128.0, 494)];
        let g = crate::parse_beat_grid(&beat_grid_blob(&beats));
        assert_eq!(g.len(), 2);
        assert_eq!((g[1].beat_in_bar, g[1].bpm, g[1].time_ms), (2, 128.0, 494));
    }

    #[test]
    fn name_ids_are_stable_and_never_sentinels() {
        assert_eq!(name_id("Ann"), name_id("Ann"));
        assert_ne!(name_id("Ann"), name_id("Bob"));
        assert!(name_id("") != 0 && name_id("") != 0xffff_ffff);
    }
}
