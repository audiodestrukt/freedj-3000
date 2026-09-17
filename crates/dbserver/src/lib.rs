//! Client for Pioneer's **remote database** protocol ("dbserver").
//!
//! Every Link-capable player, and rekordbox in LINK mode, runs a TCP service
//! that other devices use to *browse* media and fetch track metadata, artwork,
//! beat grids, waveforms and cue points by rekordbox id.  (The audio itself
//! travels over NFS — see `opendeck-nfs`.)  The port is discovered by asking
//! TCP [`PORT_QUERY`] (12523); it is usually 1051.
//!
//! Wire format (reverse-engineered by Deep Symmetry, `dysentery` →
//! `track_metadata.adoc`; beat-link is the reference client): a stream of
//! self-describing *fields*, each a tag byte then a payload —
//! `0x0f` u8, `0x10` u16, `0x11` u32, `0x14` blob (u32 len + bytes),
//! `0x26` string (u32 length in UTF-16 units incl. a trailing NUL, then
//! UTF-16BE).  A *message* is: magic `0x872349ae` (u32 field), transaction id
//! (u32), message type (u16), argument count (u8), a 12-byte blob of argument
//! type tags (`0x06` = u32, `0x02` = string, `0x03` = blob), then the arguments.
//!
//! Menus are two-step: a request (e.g. [`Client::all_tracks`]) is answered by
//! `0x4000` carrying the item count; a `0x3000` *render* request then streams
//! `0x4001` header, `0x4101` items, `0x4201` footer.
//!
//! This is the client half.  Serving (being a media source for other decks,
//! issue #44) reuses the same framing in the other direction.

pub mod server;

use anyhow::{bail, Context, Result};
use std::io::{BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Duration;

/// TCP port that answers "which port is your database server on?".
pub const PORT_QUERY: u16 = 12_523;
const MAGIC: u32 = 0x8723_49ae;
const TIMEOUT: Duration = Duration::from_secs(5);

// ── message types ─────────────────────────────────────────────────────────────
pub mod kind {
    pub const SETUP:          u16 = 0x0000;
    pub const ALL_TRACKS:     u16 = 0x1004;
    pub const PLAYLIST:       u16 = 0x1105;
    pub const METADATA:       u16 = 0x2002;
    /// Track "info" menu: the row with item type 0 carries the absolute file
    /// path on the serving device (what a CDJ asks before loading from
    /// rekordbox — dysentery #5's "21 02" request).
    pub const TRACK_INFO:     u16 = 0x2102;
    pub const ARTWORK:        u16 = 0x2003;
    pub const WAVE_PREVIEW:   u16 = 0x2004;
    pub const CUES:           u16 = 0x2104;
    pub const BEAT_GRID:      u16 = 0x2204;
    pub const WAVE_DETAIL:    u16 = 0x2904;
    pub const CUES_EXT:       u16 = 0x2b04;
    pub const ANLZ_TAG:       u16 = 0x2c04;
    pub const RENDER:         u16 = 0x3000;
    pub const MENU_AVAILABLE: u16 = 0x4000;
    pub const MENU_HEADER:    u16 = 0x4001;
    pub const MENU_ITEM:      u16 = 0x4101;
    pub const MENU_FOOTER:    u16 = 0x4201;
    pub const UNAVAILABLE:    u16 = 0x4003;
    pub const ARTWORK_BLOB:   u16 = 0x4002;
    pub const WAVE_PREVIEW_BLOB: u16 = 0x4402;
    pub const BEAT_GRID_BLOB: u16 = 0x4602;
    pub const CUES_BLOB:      u16 = 0x4702;
    pub const WAVE_DETAIL_BLOB: u16 = 0x4a02;
    pub const CUES_EXT_BLOB:  u16 = 0x4e02;
    pub const ANLZ_TAG_BLOB:  u16 = 0x4f02;
}

/// Media slot (byte 2 of the DMST argument).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot { Cd = 1, Sd = 2, Usb = 3, Collection = 5 }

/// Track type (byte 3 of the DMST argument).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackType { Rekordbox = 1, Unanalyzed = 2, AudioCd = 5 }

/// One protocol field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Field { U8(u8), U16(u16), U32(u32), Blob(Vec<u8>), Str(String) }

impl Field {
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Field::U8(v)   => { out.push(0x0f); out.push(*v); }
            Field::U16(v)  => { out.push(0x10); out.extend_from_slice(&v.to_be_bytes()); }
            Field::U32(v)  => { out.push(0x11); out.extend_from_slice(&v.to_be_bytes()); }
            Field::Blob(b) => { out.push(0x14); out.extend_from_slice(&(b.len() as u32).to_be_bytes()); out.extend_from_slice(b); }
            Field::Str(s)  => {
                let units: Vec<u16> = s.encode_utf16().collect();
                out.push(0x26);
                out.extend_from_slice(&((units.len() + 1) as u32).to_be_bytes());
                for u in units { out.extend_from_slice(&u.to_be_bytes()); }
                out.extend_from_slice(&[0, 0]);
            }
        }
    }

    /// Argument-type tag as listed in a message header.
    fn arg_tag(&self) -> u8 {
        match self { Field::Str(_) => 0x02, Field::Blob(_) => 0x03, _ => 0x06 }
    }

    pub fn read<R: Read>(r: &mut R) -> Result<Field> {
        let mut tag = [0u8; 1];
        r.read_exact(&mut tag).context("dbserver: read field tag")?;
        Ok(match tag[0] {
            0x0f => { let mut b = [0; 1]; r.read_exact(&mut b)?; Field::U8(b[0]) }
            0x10 => { let mut b = [0; 2]; r.read_exact(&mut b)?; Field::U16(u16::from_be_bytes(b)) }
            0x11 => { let mut b = [0; 4]; r.read_exact(&mut b)?; Field::U32(u32::from_be_bytes(b)) }
            0x14 => {
                let mut l = [0; 4]; r.read_exact(&mut l)?;
                let mut b = vec![0; u32::from_be_bytes(l) as usize]; r.read_exact(&mut b)?;
                Field::Blob(b)
            }
            0x26 => {
                let mut l = [0; 4]; r.read_exact(&mut l)?;
                let n = u32::from_be_bytes(l) as usize;
                let mut b = vec![0; n * 2]; r.read_exact(&mut b)?;
                let mut units: Vec<u16> = b.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                while units.last() == Some(&0) { units.pop(); }
                Field::Str(String::from_utf16_lossy(&units))
            }
            t => bail!("dbserver: unknown field tag 0x{t:02x}"),
        })
    }

    pub fn as_u32(&self) -> u32 {
        match self { Field::U8(v) => *v as u32, Field::U16(v) => *v as u32, Field::U32(v) => *v, _ => 0 }
    }
    pub fn as_str(&self) -> &str { if let Field::Str(s) = self { s } else { "" } }
}

/// A dbserver message: type + arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message { pub txid: u32, pub kind: u16, pub args: Vec<Field> }

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        Field::U32(MAGIC).encode(&mut out);
        Field::U32(self.txid).encode(&mut out);
        Field::U16(self.kind).encode(&mut out);
        Field::U8(self.args.len() as u8).encode(&mut out);
        let mut tags = [0u8; 12];
        for (i, a) in self.args.iter().take(12).enumerate() { tags[i] = a.arg_tag(); }
        Field::Blob(tags.to_vec()).encode(&mut out);
        for a in &self.args { a.encode(&mut out); }
        out
    }

    pub fn read<R: Read>(r: &mut R) -> Result<Message> {
        let magic = Field::read(r)?;
        if magic != Field::U32(MAGIC) { bail!("dbserver: bad magic {magic:?}"); }
        let txid = Field::read(r)?.as_u32();
        let kind = Field::read(r)?.as_u32() as u16;
        let argc = Field::read(r)?.as_u32() as usize;
        let _tags = Field::read(r)?;
        let mut args = Vec::with_capacity(argc);
        for _ in 0..argc { args.push(Field::read(r)?); }
        Ok(Message { txid, kind, args })
    }
}

/// One row of a rendered menu (`0x4101`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub parent_id:  u32,
    pub id:         u32,
    pub label:      String,
    pub label2:     String,
    pub item_type:  u32,
    pub flags:      u32,
    pub artwork_id: u32,
    pub position:   u32,
}

impl MenuItem {
    fn from_message(m: &Message) -> Option<MenuItem> {
        if m.kind != kind::MENU_ITEM || m.args.len() < 10 { return None; }
        Some(MenuItem {
            parent_id:  m.args[0].as_u32(),
            id:         m.args[1].as_u32(),
            label:      m.args[3].as_str().to_string(),
            label2:     m.args[5].as_str().to_string(),
            item_type:  m.args[6].as_u32(),
            flags:      m.args[7].as_u32(),
            artwork_id: m.args[8].as_u32(),
            position:   m.args[9].as_u32(),
        })
    }
    /// Human name for `item_type` (the low 16 bits; the upper ones carry flags).
    pub fn type_name(&self) -> &'static str {
        match self.item_type & 0xffff {
            0x0001 => "folder",   0x0002 => "album",    0x0003 => "disc",     0x0004 => "track",
            0x0006 => "genre",    0x0007 => "artist",   0x0008 => "playlist", 0x000a => "rating",
            0x000b => "duration", 0x000d => "tempo",    0x000e => "label",    0x000f => "key",
            0x0010 => "bitrate",  0x0011 => "year",     0x0013..=0x001a => "color",
            0x0023 => "comment",  0x0024 => "history",  0x0028 => "orig-artist", 0x0029 => "remixer",
            0x002e => "date-added", 0x0000 => "path",   0x002f => "flag?",  0x0080 => "root",   0x0081 => "root-genre", 0x0082 => "root-artist",
            0x0083 => "root-album", 0x0084 => "root-track", 0x0085 => "root-playlist",
            0x0086 => "root-bpm", 0x0087 => "root-rating", 0x0088 => "root-year", 0x0089 => "root-remixer",
            0x008a => "root-label", 0x008b => "root-orig-artist", 0x008c => "root-key",
            0x008e => "root-color", 0x0090 => "root-folder", 0x0091 => "root-search",
            0x0092 => "root-time", 0x0093 => "root-bitrate", 0x0094 => "root-filename",
            0x0095 => "root-history", 0x0098 => "root-hot-cue-bank", _ => "?",
        }
    }
}

/// Ask a device which TCP port its database server listens on.
pub fn query_db_port(ip: Ipv4Addr) -> Result<u16> {
    let mut s = TcpStream::connect_timeout(&SocketAddrV4::new(ip, PORT_QUERY).into(), TIMEOUT)
        .with_context(|| format!("dbserver: connect {ip}:{PORT_QUERY}"))?;
    s.set_read_timeout(Some(TIMEOUT))?;
    let mut q = vec![0, 0, 0, 0x0f];
    q.extend_from_slice(b"RemoteDBServer\0");
    s.write_all(&q)?;
    let mut p = [0u8; 2];
    s.read_exact(&mut p).context("dbserver: port query reply")?;
    Ok(u16::from_be_bytes(p))
}

/// A connected, set-up dbserver session.
pub struct Client {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    txid:   u32,
    /// Our device number as sent in the setup request.  rekordbox and players
    /// answer media requests only for numbers a real player could have (1–4),
    /// so borrow one that is free on the network if we announce as 5+.
    pub device: u8,
}

impl Client {
    /// Connect to `ip:port` and run the greeting + setup handshake.
    pub fn connect(ip: Ipv4Addr, port: u16, device: u8) -> Result<Client> {
        let stream = TcpStream::connect_timeout(&SocketAddrV4::new(ip, port).into(), TIMEOUT)
            .with_context(|| format!("dbserver: connect {ip}:{port}"))?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_nodelay(true)?;
        let reader = BufReader::new(stream.try_clone()?);
        let mut c = Client { stream, reader, txid: 0, device };
        // Greeting: a lone u32 field with value 1; the server echoes it.
        let mut hello = Vec::new();
        Field::U32(1).encode(&mut hello);
        c.stream.write_all(&hello)?;
        let echo = Field::read(&mut c.reader).context("dbserver: greeting echo")?;
        if echo != Field::U32(1) { bail!("dbserver: unexpected greeting reply {echo:?}"); }
        // Setup: txid 0xfffffffe, type 0x0000, one u32 arg = our device number.
        let setup = Message { txid: 0xffff_fffe, kind: kind::SETUP, args: vec![Field::U32(device as u32)] };
        c.stream.write_all(&setup.encode())?;
        let r = Message::read(&mut c.reader).context("dbserver: setup reply")?;
        if r.kind != kind::MENU_AVAILABLE { bail!("dbserver: setup answered with 0x{:04x}", r.kind); }
        log::info!("dbserver {ip}:{port}: set up as device {device}; server is device {}", r.args.get(1).map(|f| f.as_u32()).unwrap_or(0));
        Ok(c)
    }

    /// Connect via the port query on 12523.
    pub fn discover(ip: Ipv4Addr, device: u8) -> Result<Client> {
        let port = query_db_port(ip)?;
        Self::connect(ip, port, device)
    }

    /// The four-byte "DMST" first argument: device, menu location, slot, track type.
    pub fn dmst(&self, menu: u8, slot: Slot, tt: TrackType) -> Field {
        Field::U32(u32::from_be_bytes([self.device, menu, slot as u8, tt as u8]))
    }

    fn next_txid(&mut self) -> u32 { self.txid = self.txid.wrapping_add(1); self.txid }

    /// Send one request and read one reply.
    pub fn request(&mut self, kind: u16, args: Vec<Field>) -> Result<Message> {
        let txid = self.next_txid();
        let m = Message { txid, kind, args };
        self.stream.write_all(&m.encode())?;
        let r = Message::read(&mut self.reader)?;
        if r.txid != txid { bail!("dbserver: reply txid {} for request {txid}", r.txid); }
        Ok(r)
    }

    /// Run a menu request, then render and collect every item.
    pub fn menu(&mut self, kind_: u16, slot: Slot, tt: TrackType, rest: Vec<Field>) -> Result<Vec<MenuItem>> {
        let mut args = vec![self.dmst(1, slot, tt)];
        args.extend(rest);
        let avail = self.request(kind_, args)?;
        if avail.kind == kind::UNAVAILABLE { bail!("dbserver: menu 0x{kind_:04x} unavailable"); }
        if avail.kind != kind::MENU_AVAILABLE { bail!("dbserver: menu 0x{kind_:04x} answered with 0x{:04x}", avail.kind); }
        let count = avail.args.get(1).map(|f| f.as_u32()).unwrap_or(0);
        if count == 0 { return Ok(Vec::new()); }
        let txid = self.next_txid();
        let render = Message { txid, kind: kind::RENDER, args: vec![
            self.dmst(1, slot, tt), Field::U32(0), Field::U32(count), Field::U32(0), Field::U32(count), Field::U32(0),
        ]};
        self.stream.write_all(&render.encode())?;
        let mut items = Vec::with_capacity(count as usize);
        loop {
            let m = Message::read(&mut self.reader)?;
            match m.kind {
                kind::MENU_HEADER => {}
                kind::MENU_ITEM   => { if let Some(i) = MenuItem::from_message(&m) { items.push(i); } }
                kind::MENU_FOOTER => break,
                k => bail!("dbserver: unexpected 0x{k:04x} while rendering"),
            }
        }
        Ok(items)
    }

    /// Every track in the slot (rekordbox: the collection), sorted by `sort`.
    pub fn all_tracks(&mut self, slot: Slot, sort: u32) -> Result<Vec<MenuItem>> {
        self.menu(kind::ALL_TRACKS, slot, TrackType::Rekordbox, vec![Field::U32(sort)])
    }

    /// Contents of a playlist folder (`id` 0 = root) or a playlist.
    pub fn playlist(&mut self, slot: Slot, id: u32, is_folder: bool) -> Result<Vec<MenuItem>> {
        self.menu(kind::PLAYLIST, slot, TrackType::Rekordbox,
                  vec![Field::U32(0), Field::U32(id), Field::U32(is_folder as u32)])
    }

    /// Metadata rows (title, artist, album, duration, tempo, key, …) for one track.
    pub fn metadata(&mut self, slot: Slot, rekordbox_id: u32) -> Result<Vec<MenuItem>> {
        self.menu(kind::METADATA, slot, TrackType::Rekordbox, vec![Field::U32(rekordbox_id)])
    }

    /// The track's absolute path on the serving device (e.g.
    /// `/Users/me/Music/rekordbox/x.wav` from rekordbox on a Mac), or None if
    /// the info menu has no path row.  Read it over NFS afterwards.
    pub fn file_path(&mut self, slot: Slot, rekordbox_id: u32) -> Result<Option<String>> {
        let rows = self.menu(kind::TRACK_INFO, slot, TrackType::Rekordbox, vec![Field::U32(rekordbox_id)])?;
        Ok(rows.into_iter().find(|r| r.item_type & 0xffff == 0 && !r.label.is_empty()).map(|r| r.label))
    }

    /// Send a blob request and return the blob argument of the reply.
    fn blob(&mut self, kind_: u16, want: u16, args: Vec<Field>) -> Result<Vec<u8>> {
        let r = self.request(kind_, args)?;
        if r.kind != want { bail!("dbserver: 0x{kind_:04x} answered with 0x{:04x}", r.kind); }
        r.args.iter().find_map(|f| if let Field::Blob(b) = f { Some(b.clone()) } else { None })
            .ok_or_else(|| anyhow::anyhow!("dbserver: 0x{want:04x} reply carries no blob"))
    }

    /// Waveform preview (the 400-column overview; 0x2004 → 0x4402).
    pub fn waveform_preview(&mut self, slot: Slot, id: u32) -> Result<Vec<u8>> {
        let d = self.dmst(8, slot, TrackType::Rekordbox);
        self.blob(kind::WAVE_PREVIEW, kind::WAVE_PREVIEW_BLOB, vec![d, Field::U32(4), Field::U32(id), Field::U32(0)])
    }

    /// Waveform detail (0x2904 → 0x4a02).
    pub fn waveform_detail(&mut self, slot: Slot, id: u32) -> Result<Vec<u8>> {
        let d = self.dmst(1, slot, TrackType::Rekordbox);
        self.blob(kind::WAVE_DETAIL, kind::WAVE_DETAIL_BLOB, vec![d, Field::U32(id), Field::U32(0)])
    }

    /// Beat grid (0x2204 → 0x4602).
    pub fn beat_grid(&mut self, slot: Slot, id: u32) -> Result<Vec<u8>> {
        let d = self.dmst(8, slot, TrackType::Rekordbox);
        self.blob(kind::BEAT_GRID, kind::BEAT_GRID_BLOB, vec![d, Field::U32(id)])
    }

    /// A raw ANLZ section by four-character tag (e.g. `b"PWV4"`, `b"PSSI"`)
    /// from the `.EXT` (`b"EXT"`) or `.DAT`/`.2EX` file (0x2c04 → 0x4f02).
    /// This is what rekordbox serves instead of some native requests.
    pub fn anlz_tag(&mut self, slot: Slot, id: u32, tag: &[u8; 4], ext: &[u8; 3]) -> Result<Vec<u8>> {
        let d = self.dmst(1, slot, TrackType::Rekordbox);
        let tag_code = u32::from_be_bytes([tag[3], tag[2], tag[1], tag[0]]);
        let ext_code = u32::from_be_bytes([0, ext[2], ext[1], ext[0]]);
        self.blob(kind::ANLZ_TAG, kind::ANLZ_TAG_BLOB, vec![d, Field::U32(id), Field::U32(tag_code), Field::U32(ext_code)])
    }
}

/// One beat from a dbserver beat-grid blob (`0x4602`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridBeat { pub beat_in_bar: u8, pub bpm: f32, pub time_ms: u32 }

/// Parse a `0x4602` beat-grid blob: a 20-byte header (bytes 4..8 = beat count,
/// little-endian) then 16-byte entries — byte 0 beat-in-bar (1–4), bytes 2..4
/// tempo ×100 LE, bytes 4..8 time in ms LE, 8 bytes of 0xff.  Verified against
/// rekordbox 7: a 128.00 BPM track gives beats at 25 ms, 494 ms, …
pub fn parse_beat_grid(blob: &[u8]) -> Vec<GridBeat> {
    let mut out = Vec::new();
    if blob.len() < 20 { return out; }
    for e in blob[20..].chunks_exact(16) {
        let bpm = u16::from_le_bytes([e[2], e[3]]) as f32 / 100.0;
        let time_ms = u32::from_le_bytes([e[4], e[5], e[6], e[7]]);
        if e[0] == 0 || bpm <= 0.0 { continue; }
        out.push(GridBeat { beat_in_bar: e[0], bpm, time_ms });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_grid_blob_from_rekordbox_7() {
        let mut b = vec![0x00, 0x00, 0x08, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x17, 0x00, 0x00,
                         0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
        b.extend_from_slice(&[0x01, 0x00, 0x00, 0x32, 0x19, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        b.extend_from_slice(&[0x02, 0x00, 0x00, 0x32, 0xee, 0x01, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        let g = parse_beat_grid(&b);
        assert_eq!(g, vec![GridBeat { beat_in_bar: 1, bpm: 128.0, time_ms: 25 },
                           GridBeat { beat_in_bar: 2, bpm: 128.0, time_ms: 494 }]);
    }

    #[test]
    fn field_roundtrip() {
        let fields = vec![Field::U8(7), Field::U16(0x1234), Field::U32(0xdead_beef),
                          Field::Blob(vec![1, 2, 3]), Field::Str("Amplificaté".into())];
        let mut buf = Vec::new();
        for f in &fields { f.encode(&mut buf); }
        let mut cur = std::io::Cursor::new(buf);
        for f in &fields { assert_eq!(&Field::read(&mut cur).unwrap(), f); }
    }

    #[test]
    fn string_length_counts_trailing_nul() {
        let mut buf = Vec::new();
        Field::Str("ab".into()).encode(&mut buf);
        assert_eq!(buf, vec![0x26, 0, 0, 0, 3, 0, b'a', 0, b'b', 0, 0]);
    }

    #[test]
    fn message_roundtrip_and_header() {
        let m = Message { txid: 5, kind: kind::METADATA, args: vec![Field::U32(0x0101_0301), Field::U32(42)] };
        let bytes = m.encode();
        // magic, txid, type, argc, 12-byte tag blob
        assert_eq!(&bytes[..5], &[0x11, 0x87, 0x23, 0x49, 0xae]);
        assert_eq!(bytes[5], 0x11);
        assert_eq!(&bytes[10..13], &[0x10, 0x20, 0x02]);
        assert_eq!(&bytes[13..15], &[0x0f, 2]);
        assert_eq!(&bytes[15..20], &[0x14, 0, 0, 0, 12]);
        assert_eq!(&bytes[20..32], &[6, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let back = Message::read(&mut std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn menu_item_parses() {
        let m = Message { txid: 1, kind: kind::MENU_ITEM, args: vec![
            Field::U32(0), Field::U32(17), Field::U32(6), Field::Str("Title".into()),
            Field::U32(0), Field::Str("".into()), Field::U32(0x0004), Field::U32(0),
            Field::U32(9), Field::U32(1), Field::U32(0), Field::U32(0)] };
        let i = MenuItem::from_message(&m).unwrap();
        assert_eq!((i.id, i.label.as_str(), i.type_name(), i.artwork_id), (17, "Title", "track", 9));
    }
}
