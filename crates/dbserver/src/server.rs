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
    /// 3-band preview (PWV6: 1,200 × low, mid, high) and detail (PWV7: 150 a
    /// second × 3), served as `.2EX` sections to a CDJ-3000.  Empty = none.
    pub band_preview: Vec<u8>,
    pub band_detail:  Vec<u8>,
    /// RGB preview (PWV4: 1,200 × 6 bytes) and detail (PWV5: 150 a second ×
    /// 2 bytes), served as `.EXT` sections.  Empty = none.
    pub rgb_preview: Vec<u8>,
    pub rgb_detail:  Vec<u8>,
    /// Phrase analysis (song structure): the `.EXT` PSSI section as it sits
    /// in a rekordbox file, fourcc to end, masked as rekordbox 6+ writes it.
    /// Empty = none.
    pub phrases:    Vec<u8>,
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
/// Our player number as the server reports it (it can change after start).
pub type DeviceNumber = Arc<dyn Fn() -> u8 + Send + Sync>;

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
        // Root menu: the categories a player shows for our "USB", as a
        // CDJ-3000 serves its own stick (ids, item types and the U+FFFA/FFFB-
        // wrapped labels as captured), limited to what we can list.  FILE
        // NAME is ours; players show it (type 0x94).
        kind::ROOT_MENU => vec![
            row(0, 2, &crate::mark("ARTIST"), "", 0x0081, 0),
            row(0, 3, &crate::mark("ALBUM"), "", 0x0082, 0),
            row(0, 4, &crate::mark("TRACK"), "", 0x0083, 0),
            row(0, 5, &crate::mark("PLAYLIST"), "", 0x0084, 0),
            row(0, 19, &crate::mark("FILENAME"), "", 0x0094, 0),
        ],
        // The sort choices a player offers in a list (as a CDJ-3000 lists
        // them, without the ones we have no data for); we keep our order.
        0x1400 => vec![
            row(0, 0, &crate::mark("DEFAULT"), "", 0x00a1, 0),
            row(0, 1, &crate::mark("ALPHABET"), "", 0x00a2, 0),
            row(0, 2, &crate::mark("ARTIST"), "", 0x0081, 0),
            row(0, 3, &crate::mark("ALBUM"), "", 0x0082, 0),
            row(0, 4, &crate::mark("BPM"), "", 0x0085, 0),
        ],
        // All tracks (sort ignored).  (0x1014 is the KEY menu, not a sort.)
        kind::ALL_TRACKS => all(lib.tracks.iter().collect()),
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

/// The 20-byte header of a `0x4a02` detail blob, as a CDJ-3000 sends it:
/// entry count, 1, entry count, entries a second (150), then 0xffffffff, all
/// little-endian.  We sent 19 zero bytes (Beat Link skips them), and a
/// player read zero entries: no scrolling waveform.
pub fn detail_header(entries: usize) -> [u8; 20] {
    let mut h = [0u8; 20];
    for (k, v) in [entries as u32, 1, entries as u32, 150, u32::MAX].into_iter().enumerate() {
        h[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    h
}

/// A blob reply: request type, 0, length, bytes (Beat Link's argument list
/// for artwork, preview and detail; the beat grid adds a trailing 0).
/// An ANLZ section as a player serves it (`0x4f02`'s blob): the section's
/// padded length (u32 LE), then the section as it sits in the file: fourcc,
/// header length, section length, entry size, entry count, (PWV7: a constant
/// 0x00960000), the entries, zero padding to 4 bytes.
pub fn anlz_section(fourcc: &[u8; 4], entry_bytes: u32, entries: &[u8], extra: Option<u32>) -> Vec<u8> {
    let header = 20 + if extra.is_some() { 4 } else { 0 };
    let len = header + entries.len();
    let padded = (len + 3) & !3;
    let mut b = Vec::with_capacity(4 + padded);
    b.extend_from_slice(&(padded as u32).to_le_bytes());
    b.extend_from_slice(fourcc);
    for v in [header as u32, len as u32, entry_bytes, entries.len() as u32 / entry_bytes] { b.extend_from_slice(&v.to_be_bytes()); }
    if let Some(x) = extra { b.extend_from_slice(&x.to_be_bytes()); }
    b.extend_from_slice(entries);
    b.resize(4 + padded, 0);
    b
}

/// A whole ANLZ section (fourcc on) as a player serves it: its padded
/// length (u32 LE), the section, zero padding to 4 bytes.
pub fn anlz_raw(section: &[u8]) -> Vec<u8> {
    let padded = (section.len() + 3) & !3;
    let mut b = Vec::with_capacity(4 + padded);
    b.extend_from_slice(&(padded as u32).to_le_bytes());
    b.extend_from_slice(section);
    b.resize(4 + padded, 0);
    b
}

/// The colour waveform or phrase section a player asked for (3-band or RGB) (`tag` = fourcc as a little-
/// endian u32), if we have it for that track.
fn band_tag(lib: &Library, id: u32, tag: u32) -> Option<Vec<u8>> {
    let t = lib.track(id)?;
    match &tag.to_le_bytes() {
        b"PWV6" if !t.band_preview.is_empty() => Some(anlz_section(b"PWV6", 3, &t.band_preview, None)),
        b"PWV7" if !t.band_detail.is_empty() => Some(anlz_section(b"PWV7", 3, &t.band_detail, Some(0x0096_0000))),
        b"PWV4" if !t.rgb_preview.is_empty() => Some(anlz_section(b"PWV4", 6, &t.rgb_preview, Some(0))),
        b"PWV5" if !t.rgb_detail.is_empty() => Some(anlz_section(b"PWV5", 2, &t.rgb_detail, Some(0x0096_0305))),
        b"PSSI" if !t.phrases.is_empty() => Some(anlz_raw(&t.phrases)),
        _ => None,
    }
}

/// A player's "no data" answer to a per-track request: the matching reply
/// kind, then request, status 0x32, length 0, the (omitted) empty blob and,
/// for most kinds, a trailing 0.  `None` for a request with no known reply.
pub fn no_data(txid: u32, req: u16) -> Message {
    let (want, trailing) = match req {
        kind::CUES_EXT => (kind::CUES_EXT_BLOB, true),
        kind::BEAT_GRID => (kind::BEAT_GRID_BLOB, true),
        kind::LOAD_INFO => (kind::LOAD_INFO_BLOB, false),
        kind::WAVE_PREVIEW => (kind::WAVE_PREVIEW_BLOB, true),
        kind::WAVE_DETAIL => (kind::WAVE_DETAIL_BLOB, true),
        _ => (kind::ANLZ_TAG_BLOB, true),   // 0x2c04 / 0x2d04
    };
    let mut args = vec![u(req as u32), u(0x32), u(0), Field::Blob(Vec::new())];
    if trailing { args.push(u(0)); }
    Message { txid, kind: want, args }
}

/// A cue list (0x4702) with no entries: request, 0, length 0, an empty blob,
/// the entry size (36), zero hot cues, zero memory points.
fn empty_cue_list(txid: u32) -> Message {
    Message { txid, kind: kind::CUES_BLOB, args: vec![u(kind::CUES as u32), u(0), u(0), Field::Blob(Vec::new()), u(0x24), u(0), u(0)] }
}

fn blob_reply(txid: u32, want: u16, req: u16, blob: Vec<u8>, trailing_zero: bool) -> Message {
    let mut args = vec![u(req as u32), u(0), u(blob.len() as u32), Field::Blob(blob)];
    if trailing_zero { args.push(u(0)); }
    Message { txid, kind: want, args }
}

fn serve_client(lib: SharedLibrary, device: DeviceNumber, stream: TcpStream) -> Result<()> {
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
                reply(&mut w, Message { txid: m.txid, kind: kind::MENU_AVAILABLE, args: vec![u(0), u(device() as u32)] })?;
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
                    None => reply(&mut w, no_data(m.txid, m.kind))?,
                }
            }
            // Waveform preview: (dmst, 4, track id, 0).
            kind::WAVE_PREVIEW => {
                let blob = lib.read().unwrap().track(arg(2)).filter(|t| !t.preview.is_empty()).map(|t| t.preview.clone());
                match blob {
                    Some(b) => { log::debug!("dbserver: {peer:?} preview for track {} — {} bytes", arg(2), b.len()); reply(&mut w, blob_reply(m.txid, kind::WAVE_PREVIEW_BLOB, m.kind, b, false))? }
                    None => reply(&mut w, no_data(m.txid, m.kind))?,
                }
            }
            // Waveform detail: (dmst, track id, 0).
            kind::WAVE_DETAIL => {
                let blob = lib.read().unwrap().track(arg(1)).filter(|t| !t.detail.is_empty())
                    .map(|t| { let mut b = detail_header(t.detail.len()).to_vec(); b.extend_from_slice(&t.detail); b });
                match blob {
                    Some(b) => { log::debug!("dbserver: {peer:?} detail for track {} — {} bytes", arg(1), b.len()); reply(&mut w, blob_reply(m.txid, kind::WAVE_DETAIL_BLOB, m.kind, b, false))? }
                    None => reply(&mut w, no_data(m.txid, m.kind))?,
                }
            }
            // Artwork: (dmst, artwork id); our artwork ids are track ids.
            kind::ARTWORK => {
                let art = lib.read().unwrap().art.clone();
                match art.and_then(|f| f(arg(1))) {
                    Some(b) => reply(&mut w, blob_reply(m.txid, kind::ARTWORK_BLOB, m.kind, b, false))?,
                    // No art: as a player answers, length 0 and the blob left out.
                    None => reply(&mut w, Message { txid: m.txid, kind: kind::ARTWORK_BLOB, args: vec![u(m.kind as u32), u(0), u(0), Field::Blob(Vec::new())] })?,
                }
            }
            // Cue lists and raw ANLZ sections: nothing to serve (no memory
            // cues of our own yet; colour waveforms are not produced).
            // The plain cue list must be answered: a CDJ-3000 loading one of
            // our tracks asked for it ~1,700 times when told "unavailable"
            // and never left NOW LOADING.  We have no memory or hot cues, so
            // the list is empty: no 36-byte entries, zero hot cues, zero
            // memory points (Beat Link reads the counts at args 5 and 6).
            kind::CUES => {
                log::debug!("dbserver: {peer:?} cue list for track {} — empty", arg(1));
                reply(&mut w, empty_cue_list(m.txid))?;
            }
            // Nothing to serve: answer as a player does when a track has no
            // such data (captured from a CDJ-3000): the reply type with status
            // 0x32, length 0 and the blob left out.  "Unavailable" (0x4003)
            // made a CDJ-3000 abandon a load and reconnect.
            // An ANLZ section by fourcc (arg 2, little-endian ASCII) and file
            // (arg 3): the colour waveforms and phrases a CDJ-3000 draws, from our own
            // analysis.  Anything else: "no data" below.
            kind::ANLZ_TAG | kind::ANLZ_TAG2 if band_tag(&lib.read().unwrap(), arg(1), arg(2)).is_some() => {
                let blob = band_tag(&lib.read().unwrap(), arg(1), arg(2)).unwrap();
                log::debug!("dbserver: {peer:?} {} for track {} — {} bytes", String::from_utf8_lossy(&arg(2).to_le_bytes()), arg(1), blob.len());
                reply(&mut w, Message { txid: m.txid, kind: kind::ANLZ_TAG_BLOB, args: vec![u(m.kind as u32), u(0), u(blob.len() as u32), Field::Blob(blob), u(1)] })?;
            }
            kind::CUES_EXT | kind::ANLZ_TAG | kind::ANLZ_TAG2 | kind::LOAD_INFO => {
                let what = if matches!(m.kind, kind::ANLZ_TAG | kind::ANLZ_TAG2) {
                    format!(" {}.{}", String::from_utf8_lossy(&arg(2).to_le_bytes()), String::from_utf8_lossy(&arg(3).to_le_bytes()).trim_end_matches('\0'))
                } else { String::new() };
                log::debug!("dbserver: {peer:?} 0x{:04x}{what} for track {} — none", m.kind, arg(1));
                reply(&mut w, no_data(m.txid, m.kind))?;
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
        Self::start_live(lib, Arc::new(move || device), db_port)
    }

    /// As `start`, but our player number is read per client, for a deck
    /// whose number is claimed (PLAYER No. AUTO) after the server is up.
    pub fn start_live(lib: SharedLibrary, device: DeviceNumber, db_port: u16) -> Result<Server> {
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
        let now = device();
        thread::Builder::new().name("dbserver-accept".into()).spawn(move || {
            for c in db.incoming().flatten() {
                let (lib, device) = (Arc::clone(&lib), Arc::clone(&device));
                let _ = c.set_nodelay(true);
                thread::spawn(move || { if let Err(e) = serve_client(lib, device, c) { log::warn!("dbserver client: {e:#}"); } });
            }
        })?;
        Ok(Server { db_port, device: now })
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

    #[test]
    fn serves_the_phrase_section_by_fourcc() {
        let mut l = lib();
        let pssi = u32::from_le_bytes(*b"PSSI");
        assert_eq!(band_tag(&l, 2, pssi), None, "no phrases, no section");
        // A 0x38-byte section (one entry) is served as-is behind its length.
        let mut sec = b"PSSI".to_vec();
        sec.resize(0x38, 7);
        l.tracks[1].phrases = sec.clone();
        let blob = band_tag(&l, 2, pssi).unwrap();
        assert_eq!(&blob[..4], &0x38u32.to_le_bytes());
        assert_eq!(&blob[4..], &sec[..]);
        // Odd lengths pad to 4.
        l.tracks[1].phrases.truncate(0x37);
        assert_eq!(band_tag(&l, 2, pssi).unwrap().len(), 4 + 0x38);
    }

    fn items(rows: Vec<Vec<Field>>) -> Vec<MenuItem> {
        rows.into_iter().map(|args| MenuItem::from_message(&Message { txid: 0, kind: kind::MENU_ITEM, args }).unwrap()).collect()
    }
    fn dmst() -> Field { Field::U32(0x0101_0301) }

    #[test]
    fn root_menu_lists_the_categories() {
        let rows = items(menu_rows(&lib(), kind::ROOT_MENU, &[dmst(), u(0), u(0xffffff)]).unwrap());
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["ARTIST", "ALBUM", "TRACK", "PLAYLIST", "FILENAME"]);
        // The item types a player reads for each (Beat Link's numbering, as
        // a CDJ-3000 serves them), and the marked labels a player expects.
        assert_eq!(rows.iter().map(|r| r.item_type).collect::<Vec<_>>(), [0x81, 0x82, 0x83, 0x84, 0x94]);
        assert_eq!(rows.iter().map(|r| r.type_name()).collect::<Vec<_>>(), ["root-artist", "root-album", "root-track", "root-playlist", "root-filename"]);
        let raw = menu_rows(&lib(), kind::ROOT_MENU, &[dmst(), u(0), u(0xffffff)]).unwrap();
        assert_eq!(raw[0][3], s("\u{fffa}ARTIST\u{fffb}"));
        assert!(menu_rows(&lib(), 0x1014, &[dmst(), u(0)]).is_none(), "0x1014 is KEY, not a track list");
        assert_eq!(menu_rows(&lib(), 0x1400, &[dmst(), u(0), u(0)]).unwrap().len(), 5, "sort choices");
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
        // "No data" as a CDJ-3000 sends it: kind, request, 0x32, 0, the
        // blob absent from the body, trailing 0 (bytes captured off the wire).
        let nd = super::no_data(0x7b, kind::CUES_EXT).encode();
        let want: Vec<u8> = [&[0x11, 0x87, 0x23, 0x49, 0xae, 0x11, 0, 0, 0, 0x7b, 0x10, 0x4e, 0x02, 0x0f, 0x05, 0x14, 0, 0, 0, 0x0c][..],
            &[6, 6, 6, 3, 6, 0, 0, 0, 0, 0, 0, 0], &[0x11, 0, 0, 0x2b, 0x04, 0x11, 0, 0, 0, 0x32, 0x11, 0, 0, 0, 0, 0x11, 0, 0, 0, 0]].concat();
        assert_eq!(nd, want);
        let back = Message::read(&mut std::io::Cursor::new(&nd)).unwrap();
        assert_eq!(back.args, vec![u(0x2b04), u(0x32), u(0), Field::Blob(vec![]), u(0)], "the omitted blob reads back");
        assert_eq!(super::no_data(1, kind::LOAD_INFO).kind, kind::LOAD_INFO_BLOB);
        assert_eq!(super::no_data(1, kind::ANLZ_TAG2).kind, kind::ANLZ_TAG_BLOB);
        // The PWV6 header as a CDJ-3000 sent it for a rekordbox track.
        let sec = super::anlz_section(b"PWV6", 3, &[0u8; 3600], None);
        assert_eq!(&sec[..24], &[0x24, 0x0e, 0, 0, b'P', b'W', b'V', b'6', 0, 0, 0, 0x14, 0, 0, 0x0e, 0x24, 0, 0, 0, 3, 0, 0, 0x04, 0xb0]);
        assert_eq!(sec.len(), 3624);
        // PWV7: 0x18 header, the 0x00960000 constant, padded to 4.
        let sec = super::anlz_section(b"PWV7", 3, &[0u8; 3 * 22911], Some(0x0096_0000));
        assert_eq!(&sec[..28], &[0x98, 0x0c, 1, 0, b'P', b'W', b'V', b'7', 0, 0, 0, 0x18, 0, 1, 0x0c, 0x95, 0, 0, 0, 3, 0, 0, 0x59, 0x7f, 0, 0x96, 0, 0]);
        assert_eq!(sec.len(), 68764);
        // Detail header as captured (26,986 entries).
        assert_eq!(super::detail_header(26986), [0x6a, 0x69, 0, 0, 1, 0, 0, 0, 0x6a, 0x69, 0, 0, 0x96, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        let cues = super::empty_cue_list(7);
        assert_eq!(cues.kind, kind::CUES_BLOB);
        assert_eq!(cues.args.len(), 7, "Beat Link reads hot and memory counts at args 5 and 6");
        assert!(cues.args[5].as_u32() == 0 && cues.args[6].as_u32() == 0);
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
