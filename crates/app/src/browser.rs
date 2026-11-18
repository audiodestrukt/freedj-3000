//! Source-based track browser — the BROWSE screen's model.
//!
//! Navigates a tree that can be the **filesystem** (a plain USB / folder), a
//! **rekordbox library** on local media (playlists → tracks, from
//! `export.pdb`), or a **linked player** over the network (LINK): the same
//! rekordbox library, read off a CDJ/XDJ over NFS.  This mirrors the XDJ browse
//! flow — pick a source, browse playlists, load a track.
//!
//! Navigation only; the deck does the loading.  `enter()` reports a `Load`
//! describing where the audio lives (a local path, or an NFS path on a player).

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use crate::prodj::LinkState;
use opendeck_dbserver::{Client as DbClient, MenuItem, Slot};
#[cfg(test)]
use opendeck_nfs::Nfs;
use opendeck_rekordbox::{read_export, read_export_from, RbExport};
use crate::media::{self, Access, Mount};
use std::time::{Duration, Instant};

// What the decoder actually plays (symphonia: no Opus decoder exists).
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "aiff", "aif", "ogg"];

/// Folders no one browses for music: `_CodeSignature` and its kin inside the
/// iOS app bundle (the browse root there), `__MACOSX` resource forks, and the
/// bundle's localisation / framework folders, and Core ML model packages.
fn is_housekeeping_dir(name: &str) -> bool {
    // A stick's system and library folders: a player's FOLDER view never
    // lists them (the rekordbox export, OverCue's stems, Windows / Android
    // housekeeping).
    const SYSTEM: [&str; 5] = ["PIONEER", "CDJMODS", "System Volume Information", "$RECYCLE.BIN", "LOST.DIR"];
    SYSTEM.iter().any(|s| name.eq_ignore_ascii_case(s))
        || name.starts_with('_')
        || name.ends_with(".lproj")
        // A stem separation model put in the music folder (separate.rs).
        || name.ends_with(".mlpackage") || name.ends_with(".mlmodelc")
        || (cfg!(target_os = "ios") && matches!(name, "Frameworks" | "PlugIns" | "SC_Info"))
}

fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.iter().any(|a| a.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// A linked track's file path (if the server has one) and its beat-grid blob.

/// Which category a dbserver root-menu row is: by its label when it is one
/// we know (a source may number the item types its own way), else by type.
/// A category row's name (`level`: entries picked so far).  Rows are named
/// by their label; a row with only a second label (a count, a key) shows
/// that.  BPM rows have neither: the tempo ×100 is the id of a BPM row
/// (a player's 0x000d row, as our own server sends it), and a range row's
/// id is its range, shown ±0 … ±6 as on the unit.
fn cat_row_name(cat: Cat, level: usize, i: &opendeck_dbserver::MenuItem) -> String {
    if !i.label.is_empty() { return i.label.clone(); }
    if !i.label2.is_empty() { return i.label2.clone(); }
    match (cat, level) {
        (Cat::Bpm, 0) if i.id % 100 == 0 => (i.id / 100).to_string(),
        (Cat::Bpm, 0) => format!("{:.1}", i.id as f32 / 100.0),
        (Cat::Bpm, 1) => format!("\u{b1}{}", i.id),
        _ => String::new(),
    }
}

/// The id a picked row adds to the request for the next level.  Most rows
/// are named by their id; KEY's second level (the key, its relatives,
/// wider) carries the key as the id on every row and the distance as the
/// parent, as a CDJ-3000 sends it (checked in cdj3k-emu: ids 1, 1, 1 for
/// "Abm", "Abm, B", "Abm, B, Dbm, Ebm"), so it is opened by the parent.
fn cat_row_id(cat: Cat, level: usize, i: &opendeck_dbserver::MenuItem) -> u32 {
    match (cat, level) {
        (Cat::Key, 1) => i.parent_id,
        _ => i.id,
    }
}

fn db_category(i: &opendeck_dbserver::MenuItem) -> &'static str {
    match i.label.to_ascii_uppercase().as_str() {
        "TRACK" => "root-track",
        "PLAYLIST" => "root-playlist",
        "ARTIST" => "root-artist",
        "ALBUM" => "root-album",
        "FILENAME" | "FILE NAME" => "root-filename",
        "GENRE" => "root-genre",
        "BPM" => "root-bpm",
        "RATING" => "root-rating",
        "YEAR" => "root-year",
        "LABEL" => "root-label",
        "COLOR" => "root-color",
        "TIME" | "DURATION" => "root-time",
        "HISTORY" => "root-history",
        "KEY" => "root-key",
        "ORIGINAL ARTIST" => "root-orig-artist",
        "FOLDER" => "root-folder",
        "SEARCH" => "root-search",
        "REMIXER" => "root-remixer",
        _ => i.type_name(),
    }
}

/// Where a track to load lives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Load {
    /// A local file (with an optional local ANLZ analysis file).
    Local { path: PathBuf, analyze: Option<PathBuf> },
    /// A track on a linked player, read over NFS.  Paths are rekordbox-relative.
    Link  { ip: Ipv4Addr, rel_path: String, analyze_rel: String, #[serde(default)] id: u32 },
    /// A track served by a peer's dbserver (a player's USB or SD, or a
    /// rekordbox laptop's collection when `rekordbox`): path and beat grid
    /// come from dbserver, the audio over the peer's NFS export.
    Db { ip: Ipv4Addr, id: u32, title: String, rekordbox: bool, #[serde(default)] sd: bool },
}

/// What the header tile shows as the source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceTile { None, Folder, Usb, Link }

/// One visible row.
#[derive(Clone)]
pub struct Entry {
    pub name:   String,
    pub is_dir: bool,
    /// Parsed rekordbox metadata; not yet drawn in the row (planned).
    #[allow(dead_code)]
    pub artist: Option<String>,
    #[allow(dead_code)]
    pub bpm:    Option<f32>,
    /// Musical key, for the CDJ-3000 skin's KEY column.
    pub key:    Option<String>,
    /// Cover image file (rekordbox tracks on local media).
    pub art:    Option<PathBuf>,
    kind:       EntryKind,
}

impl Entry {
    /// How to load this row, if it's a track (folders/links have none).
    pub fn load(&self) -> Option<&Load> {
        match &self.kind { EntryKind::Track(l) => Some(l), _ => None }
    }
}

use crate::dbsession::{DbInfo, InfoKey, Session as DbSession};

/// Lookups (metadata, cover, previews) queued on the session at once: few,
/// so a new highlight's row is next rather than behind the old neighbours.
const INFO_IN_FLIGHT: usize = 2;

#[derive(Clone)]
enum EntryKind {
    Descend(Loc),
    /// Connect to a linked player's media slot, then browse it.
    ConnectLink(Ipv4Addr, Slot),
    /// Connect to a rekordbox laptop's dbserver, then browse its collection.
    ConnectDb(Ipv4Addr),
    Track(Load),
    /// A category the source offers that this browser cannot walk yet
    /// (BPM, KEY, SEARCH, …): shown, but LOAD does nothing.
    Unsupported,
    /// A drive the OS must open up first, or iOS's ADD DRIVE: asks the OS
    /// (`media::request_access`) and stays put.
    Grant,
}

#[derive(Clone, PartialEq)]
enum Loc {
    Fs(PathBuf),
    /// The LINK source: a list of discovered players.
    Link,
    /// A rekordbox export's category menu (PLAYLIST / TRACK), what a player
    /// shows first for a USB; a stick with no playlists still lists its tracks.
    RbRoot,
    /// A folder in the current rekordbox library's playlist tree (0 = root).
    RbTree(u32),
    /// The tracks of a rekordbox playlist.
    RbPlaylist(u32),
    /// Every track in the export (the TRACK category).
    RbAllTracks,
    /// ARTIST / ALBUM / KEY categories: the names, then one name's tracks.
    RbNames(RbField),
    RbNamed(RbField, String),
    /// HISTORY: the history lists, then one list's tracks in play order.
    RbHistory,
    RbHistoryList(u32),
    /// SEARCH results for `Browser::search_query`, within the source below.
    Search,
    /// A dbserver source's category menu (PLAYLIST / ARTIST / ALBUM / TRACK /
    /// FILENAME …), the first thing a player shows for a linked source.
    DbRoot,
    /// dbserver categories: a playlist folder (0 = root), a playlist's tracks,
    /// every track, the artists, one artist's tracks, the albums, one
    /// album's tracks, every track by file name.
    DbFolder(u32),
    DbPlaylist(u32),
    DbAllTracks,
    DbArtists,
    DbArtist(u32),
    DbAlbums,
    DbAlbum(u32),
    DbFilenames,
    /// The other categories a player lists (GENRE, BPM, KEY, …): the ids
    /// picked so far.  Fewer than `Cat::depth` of them is a list of names to
    /// pick from; all of them is the tracks.
    DbCat(Cat, Vec<u32>),
    /// The player's folder tree (FOLDER): folder id, 0 = the root.
    DbTree(u32),
    /// A linked player being connected to (see [`Connecting`]): one row
    /// saying so, or why it did not answer.
    Connecting,
    /// A linked player's search results for a query: only the key its list
    /// is asked and cached under (SEARCH itself is [`Loc::Search`]).
    DbSearch(String),
}

/// A linked player's row was pressed: the session opens (and, for a player
/// with no database server, its `export.pdb` is read over NFS) on a thread
/// of its own, so the deck keeps drawing and playing while a player that
/// has gone away times out.  The answer is taken a frame later
/// ([`Browser::prefetch_meta`]); a failure stays on screen in place of the
/// list, where the key was pressed.  BACK leaves at once, and a late
/// answer is then dropped.
struct Connecting {
    ip:     Ipv4Addr,
    label:  String,
    rx:     std::sync::mpsc::Receiver<Connected>,
    failed: Option<String>,
}

enum Connected {
    Db(DbSession),
    Export(RbExport),
    Failed(String),
}

/// Read a linked player's `export.pdb` over NFS (a player with no database
/// server).
fn read_link_export(ip: Ipv4Addr, ports: crate::LinkPorts) -> anyhow::Result<RbExport> {
    let mut nfs = ports.nfs(ip, false)?;
    let root = nfs.mount_usb()?;
    let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb")?;
    let bytes = nfs.read_file(&fh, size)?;
    let exp = read_export_from(&mut std::io::Cursor::new(bytes), PathBuf::from(format!("link://{ip}")))?;
    log::info!("LINK {ip}: {} tracks, {} playlists", exp.tracks.len(), exp.playlists.len());
    Ok(exp)
}

/// A dbserver category beyond the ones with their own `Loc`: the menu that
/// lists it, and the requests that open one of its entries (Beat Link's /
/// dysentery's tables).  The argument after the sort order is the picked
/// entry's id; -1 means "all" where a request takes two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cat { Genre, Bpm, Rating, Year, Label, Color, Time, History, Key, OrigArtist, Remixer }

impl Cat {
    fn of_root(kind: &str) -> Option<Cat> {
        Some(match kind {
            "root-genre" => Cat::Genre, "root-bpm" => Cat::Bpm, "root-rating" => Cat::Rating,
            "root-year" => Cat::Year, "root-label" => Cat::Label, "root-color" => Cat::Color,
            "root-time" => Cat::Time, "root-history" => Cat::History, "root-key" => Cat::Key,
            "root-orig-artist" => Cat::OrigArtist, "root-remixer" => Cat::Remixer,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Cat::Genre => "GENRE", Cat::Bpm => "BPM", Cat::Rating => "RATING", Cat::Year => "YEAR",
            Cat::Label => "LABEL", Cat::Color => "COLOR", Cat::Time => "TIME", Cat::History => "HISTORY",
            Cat::Key => "KEY", Cat::OrigArtist => "ORIGINAL ARTIST", Cat::Remixer => "REMIXER",
        }
    }
    /// How many entries are picked before the tracks: GENRE → artist → tracks.
    fn depth(self) -> usize {
        match self {
            Cat::Rating | Cat::Color | Cat::Time | Cat::History => 1,
            // BPM → its range (±0 … ±6) → tracks, as on the unit.
            Cat::Bpm | Cat::Genre | Cat::Year | Cat::Label | Cat::Key | Cat::OrigArtist | Cat::Remixer => 2,
        }
    }
    /// The request that lists the names at this level (`picked` ids so far).
    fn names_request(self, picked: &[u32]) -> (u16, Vec<u32>) {
        use opendeck_dbserver::kind as k;
        match (self, picked) {
            (Cat::Genre, [])      => (k::GENRE_MENU, vec![]),
            (Cat::Genre, [g])     => (0x1101, vec![*g]),
            (Cat::Bpm, [])        => (k::BPM_MENU, vec![]),
            (Cat::Bpm, [t])       => (0x1106, vec![*t]),
            (Cat::Rating, _)      => (k::RATING_MENU, vec![]),
            (Cat::Year, [])       => (k::YEAR_MENU, vec![]),
            (Cat::Year, [d])      => (0x1108, vec![*d]),
            (Cat::Label, [])      => (k::LABEL_MENU, vec![]),
            (Cat::Label, [l])     => (0x110a, vec![*l]),
            (Cat::Color, _)       => (k::COLOR_MENU, vec![]),
            (Cat::Time, _)        => (k::TIME_MENU, vec![]),
            (Cat::History, _)     => (k::HISTORY_MENU, vec![]),
            (Cat::Key, [])        => (k::KEY_MENU, vec![]),
            (Cat::Key, [key])     => (0x1114, vec![*key]),
            (Cat::OrigArtist, []) => (k::ORIG_ARTIST_MENU, vec![]),
            (Cat::OrigArtist, [a]) => (0x1402, vec![*a]),
            (Cat::Remixer, [])    => (k::REMIXER_MENU, vec![]),
            (Cat::Remixer, [a])   => (0x1702, vec![*a]),
            _                     => (k::ALL_TRACKS, vec![]),
        }
    }
    /// The request that lists the tracks once every entry is picked.
    fn tracks_request(self, p: &[u32]) -> (u16, Vec<u32>) {
        const ALL: u32 = 0xffff_ffff;
        let at = |i: usize| p.get(i).copied().unwrap_or(0);
        match self {
            Cat::Genre      => (0x1301, vec![at(0), at(1), ALL]),
            Cat::Bpm        => (0x1206, vec![at(0), at(1)]),
            Cat::Rating     => (0x1107, vec![at(0)]),
            Cat::Year       => (0x1208, vec![at(0), at(1)]),
            Cat::Label      => (0x130a, vec![at(0), at(1), ALL]),
            Cat::Color      => (0x110d, vec![at(0)]),
            Cat::Time       => (0x1110, vec![at(0)]),
            Cat::History    => (0x1112, vec![at(0)]),
            Cat::Key        => (0x1214, vec![at(0), at(1)]),
            Cat::OrigArtist => (0x1502, vec![at(0), at(1)]),
            Cat::Remixer    => (0x1802, vec![at(0), at(1)]),
        }
    }
}

/// What a highlighted linked-player row opens, for the right-hand pane:
/// a category's next level (a BPM's ranges, a range's tracks, a genre's
/// artists), a playlist folder, a playlist, an artist's or album's tracks,
/// a FOLDER tree level.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Peek { Cat(Cat, Vec<u32>), Folder(u32), Playlist(u32), Artist(u32), Album(u32), Tree(u32) }

/// The pane's heading and rows (is a folder, name).
type PeekRows = (&'static str, Vec<(bool, String)>);

impl Peek {
    fn of(loc: &Loc) -> Option<Peek> {
        Some(match loc {
            Loc::DbCat(c, ids) => Peek::Cat(*c, ids.clone()),
            Loc::DbFolder(id)   => Peek::Folder(*id),
            Loc::DbPlaylist(id) => Peek::Playlist(*id),
            Loc::DbArtist(id)   => Peek::Artist(*id),
            Loc::DbAlbum(id)    => Peek::Album(*id),
            Loc::DbTree(id)     => Peek::Tree(*id),
            _ => return None,
        })
    }

    /// Whether it lists tracks (else the names of the next level).
    fn lists_tracks(&self) -> bool {
        match self {
            Peek::Cat(c, ids) => ids.len() >= c.depth(),
            Peek::Playlist(_) | Peek::Artist(_) | Peek::Album(_) => true,
            Peek::Folder(_) | Peek::Tree(_) => false,
        }
    }

    /// The request, as the level itself asks it when opened (`rebuild`).
    fn request(&self) -> impl FnMut(&mut DbClient, Slot) -> anyhow::Result<Vec<MenuItem>> + Send + 'static {
        let p = self.clone();
        move |c: &mut DbClient, s: Slot| match &p {
            Peek::Cat(cat, ids) if ids.len() >= cat.depth() => { let (k, a) = cat.tracks_request(ids); c.browse(k, s, &a) }
            Peek::Cat(cat, ids) => { let (k, a) = cat.names_request(ids); c.browse(k, s, &a) }
            Peek::Folder(id)   => c.playlist(s, *id, true),
            Peek::Playlist(id) => c.playlist(s, *id, false),
            Peek::Artist(id)   => c.tracks_for_artist(s, *id),
            Peek::Album(id)    => c.tracks_for_album(s, *id),
            Peek::Tree(id)     => c.browse(opendeck_dbserver::kind::FOLDER_MENU, s, &[*id, 0]),
        }
    }

    /// The pane's rows from the player's reply, named as the level names them.
    fn rows(&self, items: Vec<MenuItem>) -> PeekRows {
        if self.lists_tracks() {
            return ("TRACK", items.into_iter().filter(|i| i.type_name() == "track").map(|i| (false, i.label)).collect());
        }
        match self {
            Peek::Cat(cat, ids) => (cat.label(), items.iter().map(|i| (true, cat_row_name(*cat, ids.len(), i))).collect()),
            Peek::Folder(_) => ("PLAYLIST", items.into_iter().map(|i| (true, i.label)).collect()),
            _ => ("FOLDER", items.into_iter().filter_map(|i| match i.type_name() {
                "folder" => Some((true, i.label)),
                "track"  => Some((false, i.label)),
                _ => None,
            }).collect()),
        }
    }
}

/// A track field the rekordbox categories group by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RbField { Artist, Album, Key }

impl RbField {
    fn of(self, t: &opendeck_rekordbox::RbTrack) -> &str {
        match self { RbField::Artist => &t.artist, RbField::Album => &t.album, RbField::Key => &t.key }
    }
    fn label(self) -> &'static str {
        match self { RbField::Artist => "ARTIST", RbField::Album => "ALBUM", RbField::Key => "KEY" }
    }
}

/// Which media the currently-loaded rekordbox export came from — decides how a
/// selected track loads.
#[derive(Clone)]
enum RbSource {
    Local(PathBuf),   // paths resolve against this mount
    Link(Ipv4Addr),   // paths are NFS paths on this player
}

/// What kind of place a SOURCE row opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// The folder freedj started in (its local library).
    Local,
    /// A removable stick or card.
    Usb,
    /// A player's media, or a rekordbox laptop, on PRO DJ LINK.
    Link,
}

/// One row of the SOURCE screen.
#[derive(Clone, Debug)]
pub struct SourceRow {
    pub kind:      SourceKind,
    pub label:     String,
    /// The player number badge (LINK media).
    pub player:    Option<u8>,
    /// Local folder or mount point.
    pub path:      Option<PathBuf>,
    /// A LINK source's address.
    pub ip:        Option<Ipv4Addr>,
    pub rekordbox: bool,
    /// A linked player's report on the media: tracks, playlists, date, sizes.
    pub info:      Option<opendeck_link::prodj::MediaInfo>,
}

impl SourceRow {
    /// Whether a track loaded from `load` came from this source.
    pub fn holds(&self, load: &Load) -> bool {
        match (load, self.kind) {
            (Load::Local { path, .. }, SourceKind::Local | SourceKind::Usb) =>
                self.path.as_ref().is_some_and(|root| path.starts_with(root)),
            (Load::Link { ip, .. } | Load::Db { ip, .. }, SourceKind::Link) => self.ip == Some(*ip),
            _ => false,
        }
    }
}

/// Total and free bytes of the filesystem holding `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 { return None; }
        let bs = st.f_frsize as u64;
        Some((st.f_blocks as u64 * bs, st.f_bavail as u64 * bs))
    }
    #[cfg(not(unix))]
    { let _ = path; None }
}

pub enum Enter {
    Folder,
    Track(Load),
    Nothing,
}

pub struct Browser {
    stack:    Vec<Loc>,
    entries:  Vec<Entry>,
    pub selected: usize,
    rb:        Option<Arc<RbExport>>,
    rb_source: Option<RbSource>,
    /// Open dbserver session to a peer (its address and the slot we browse:
    /// USB on a player, Collection on rekordbox), on its own thread.
    db:        Option<DbSession>,
    /// Removable media seen at the last scan (USB rows at the top level).
    media:     Vec<Mount>,
    media_at:  Option<Instant>,
    /// The stick being browsed was pulled (see `take_browsed_gone`).
    browsed_gone: bool,
    /// Name of the dbserver folder/playlist/artist/album being shown (set on
    /// descend), and the source's own label ("3 USB: OPENDECK") for the root.
    db_title:  String,
    db_label:  String,
    /// The linked player's category menu, kept for the left-hand column.
    db_cats:   Vec<Entry>,
    /// Library metadata of tracks shown in a dbserver list, fetched as each
    /// is highlighted (a list row carries only title and artist).
    meta_cache: std::collections::HashMap<InfoKey, Option<DbInfo>>,
    /// Lookups asked of the session and not answered yet.
    meta_asked: std::collections::HashSet<InfoKey>,
    /// The right-hand pane for a highlighted linked row: what it opens, by
    /// row, and the one list asked and not answered yet (its ticket).
    peeks:      std::collections::HashMap<Peek, PeekRows>,
    peek_asked: Option<(u64, Peek)>,
    peek_ticket: u64,
    /// The highlight is in the category column on the left (BACK from a
    /// category's top level): the wheel steps through the categories, each
    /// opening as it is reached, and pressing it goes back into the list.
    cat_focus:  bool,
    link:      Arc<LinkState>,
    /// The ports a linked player is reached on (a player's own by default;
    /// tests point them at a server on free ports).
    pub(crate) ports: crate::LinkPorts,
    connecting: Option<Connecting>,
    /// The linked player's lists this session (see [`Browser::db_list`]):
    /// those in, the one asked and not in yet (ticket, where, root
    /// fallback), the last that failed, and what waits for the one asked.
    list_cache: Vec<(Loc, Vec<Entry>)>,
    list_wait:  Option<(u64, Loc, bool)>,
    list_error: Option<(Loc, String)>,
    after_list: Option<Box<dyn FnOnce(&mut Browser)>>,
    /// BACK at the top level normally climbs to the parent folder; on iOS the
    /// start folder is the app's sandbox, and climbing out of it only shows the
    /// container's UUID directories — so the start is the floor there.
    floor:     Option<PathBuf>,
    /// The CDJ-3000 skin's right pane: the highlighted folder's contents,
    /// listed once per highlighted folder (see [`Browser::preview`]).
    preview:   std::cell::RefCell<Option<(PathBuf, Vec<(bool, String)>)>>,
    /// The folder freedj started in: the SOURCE screen's local row.
    home:      PathBuf,
    /// Tracks loaded this session (the CDJ draws them green in lists).
    played:    Vec<Load>,
    /// SEARCH: the typed text, and the audio files under a folder (built
    /// once per folder searched).
    search_query: String,
    fs_index:  std::cell::RefCell<Option<(PathBuf, Vec<PathBuf>)>>,
    /// Per stack level, the name of the row that led there ("" for the
    /// first level and for levels opened automatically, such as a rekordbox
    /// stick's category list).  BACK highlights the row it came out of;
    /// a saved position replays these names.
    names:     Vec<String>,
    /// The last position in each local source (the home folder, each
    /// stick), by the source's root: opening the source again returns there.
    positions: std::collections::HashMap<PathBuf, BrowsePos>,
    /// The source browsed last: the USB / BROWSE key goes back to it rather
    /// than to whatever stick was mounted most recently.
    last_root: Option<PathBuf>,
    /// PLAY LIST: the playlists only, no category column.
    playlist_only: bool,
    /// The second row of the track table: 0 off, then ARTIST, ALBUM, GENRE.
    detail: u8,
    info: bool,
    previews: bool,
    /// Nothing chosen yet since launch: BROWSE lists nothing until a source
    /// (SOURCE, USB, PLAY LIST) is picked, as a unit with nothing loaded.
    idle: bool,
    font: u8,
    /// Which local tracks have OverCue stems (the "S" mark in lists).
    stem_marks: std::cell::RefCell<crate::stems::StemMarks>,
}

/// Where the browser stood in a local source, by names (rows survive a
/// rescan, a restart and a re-read export, where indices and ids may not).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BrowsePos {
    /// The source's root folder: the home folder or a stick's mount.
    pub root:     PathBuf,
    /// Rows entered from the root, in order ("" = a level the browser opens
    /// on its own, a rekordbox stick's categories).
    pub names:    Vec<String>,
    /// The highlighted row there.
    pub selected: String,
}

/// Most files indexed for a folder SEARCH (a big library stays responsive).
const SEARCH_INDEX_MAX: usize = 50_000;

impl Browser {
    pub fn new(dir: &Path, link: Arc<LinkState>) -> Self {
        let start = if dir.is_dir() {
            dir.to_path_buf()
        } else {
            match dir.parent() {
                Some(par) if !par.as_os_str().is_empty() => par.to_path_buf(),
                _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            }
        };
        let mut b = Browser {
            floor: if cfg!(any(target_os = "ios", target_os = "android")) { Some(start.clone()) } else { None },
            stack: vec![Loc::Fs(start.clone())],
            entries: Vec::new(),
            selected: 0,
            rb: None,
            rb_source: None,
            db: None,
            media: media::mounts(),
            media_at: Some(Instant::now()),
            browsed_gone: false,
            db_title: String::new(),
            db_label: String::new(),
            db_cats: Vec::new(),
            meta_cache: Default::default(),
            meta_asked: Default::default(),
            peeks: Default::default(),
            peek_asked: None,
            peek_ticket: 0,
            cat_focus: false,
            link,
            ports: crate::LinkPorts::default(),
            connecting: None,
            list_cache: Vec::new(),
            list_wait: None,
            list_error: None,
            after_list: None,
            preview: Default::default(),
            home: start.clone(),
            played: Vec::new(),
            search_query: String::new(),
            fs_index: Default::default(),
            names: Vec::new(),
            positions: Default::default(),
            last_root: None,
            playlist_only: false,
            detail: 0,
            info: true,
            previews: true,
            idle: !cfg!(test),
            font: 1,
            stem_marks: Default::default(),
        };
        b.rebuild();
        b
    }

    pub fn entries(&self) -> &[Entry] { &self.entries }

    /// A track was loaded: lists mark it as played this session.
    pub fn note_played(&mut self, l: &Load) {
        if !self.played.contains(l) { self.played.push(l.clone()); }
    }
    pub fn was_played(&self, l: &Load) -> bool { self.played.contains(l) }
    /// A track with OverCue stems prepared, local or on a linked player's
    /// media (lists mark it "S").
    pub fn has_stems(&self, l: &Load) -> bool {
        match l {
            Load::Local { path, .. } => self.stem_marks.borrow_mut().has_stems(path),
            // A linked player's USB / SD, by the track's rekordbox ID; a
            // rekordbox laptop's collection has no stems folder.
            Load::Db { ip, id, rekordbox: false, sd, .. } =>
                self.stem_marks.borrow_mut().has(crate::stems::MarkKey::Link { ip: *ip, export: if *sd { "/B/" } else { "/C/" }, id: *id }),
            Load::Link { ip, id, .. } if *id > 0 =>
                self.stem_marks.borrow_mut().has(crate::stems::MarkKey::Link { ip: *ip, export: "/C/", id: *id }),
            _ => false,
        }
    }
    pub fn selected_entry(&self) -> Option<&Entry> { self.entries.get(self.selected) }

    /// Look up the highlighted dbserver track's library metadata once (title,
    /// artist, album, key, BPM, year, cover, previews) for the INFO pane and
    /// the table.  Call each frame; it never waits on the network: the
    /// session's worker asks the player, and the answer is taken here a
    /// frame or two later.
    pub fn prefetch_meta(&mut self) {
        self.poll_connect();
        let Some(db) = self.db.as_ref() else { return };
        let answers: Vec<_> = db.answers().collect();
        let arrived: Vec<_> = db.lists().collect();
        for (k, info) in answers {
            self.meta_asked.remove(&k);
            self.meta_cache.insert(k, info);
        }
        // The right-hand pane: what the highlighted row opens, one list at
        // a time (scrolling fast asks only for where the highlight stops).
        for (ticket, r) in arrived {
            // The list on screen (or one BROWSE moved on from: dropped).
            if let Some((t, _, _)) = &self.list_wait {
                if *t == ticket {
                    let (_, loc, fallback) = self.list_wait.take().unwrap();
                    self.list_arrived(loc, fallback, r);
                    continue;
                }
            }
            let Some((t, peek)) = self.peek_asked.take() else { continue };
            if t != ticket { self.peek_asked = Some((t, peek)); continue; }
            let rows = match r {
                Ok(items) => peek.rows(items),
                Err(e) => { log::info!("LINK pane {peek:?}: {e:#}"); (if peek.lists_tracks() { "TRACK" } else { "" }, Vec::new()) }
            };
            self.peeks.insert(peek, rows);
        }
        let Some(db) = self.db.as_ref() else { return };
        if self.peek_asked.is_none() {
            if let Some(peek) = self.selected_peek().filter(|p| !self.peeks.contains_key(p)) {
                self.peek_ticket += 1;
                if db.ask_list(self.peek_ticket, peek.request()) { self.peek_asked = Some((self.peek_ticket, peek)); }
            }
        }
        // The highlighted row first, then its neighbours, so the table's
        // previews and second rows fill in around it.
        let n = self.entries.len();
        let order = std::iter::once(self.selected)
            .chain((1..=10usize).flat_map(|d| [self.selected + d, self.selected.wrapping_sub(d)]))
            .filter(|&i| i < n);
        for i in order {
            if self.meta_asked.len() >= INFO_IN_FLIGHT { break; }
            let EntryKind::Track(l @ Load::Db { ip, .. }) = &self.entries[i].kind else { continue };
            if *ip != db.ip { continue; }
            let Some(k) = Self::meta_key(l) else { continue };
            if k.1 != db.slot as u8 || self.meta_cache.contains_key(&k) || self.meta_asked.contains(&k) { continue; }
            if db.ask_info(k, i == self.selected) { self.meta_asked.insert(k); }
        }
    }

    /// Lookups still under way (the app keeps drawing at full rate so their
    /// answers show as they land).
    pub fn db_pending(&self) -> bool { !self.meta_asked.is_empty() || self.peek_asked.is_some() || self.list_wait.is_some() || self.connecting() }

    fn selected_peek(&self) -> Option<Peek> {
        match self.selected_entry() {
            Some(Entry { kind: EntryKind::Descend(loc), .. }) => Peek::of(loc),
            _ => None,
        }
    }

    /// The right-hand pane for a highlighted linked-player row: the heading
    /// and rows of what it opens (empty until the player answers).  `None`
    /// for other rows.
    pub fn db_peek(&self) -> Option<PeekRows> {
        let p = self.selected_peek()?;
        Some(self.peeks.get(&p).cloned().unwrap_or_else(|| (if p.lists_tracks() { "TRACK" } else { "" }, Vec::new())))
    }

    /// The tags (title, artist, album, key, BPM, cover) fetched for a
    /// dbserver track, if any.
    pub fn db_tags(&self, l: &Load) -> Option<opendeck_decode::TrackTags> {
        let i = self.meta_cache.get(&Self::meta_key(l)?)?.as_ref()?;
        let m = &i.meta;
        Some(opendeck_decode::TrackTags {
            title: m.title.clone(), artist: m.artist.clone(), album: m.album.clone(), genre: m.genre.clone(),
            key: m.key.clone(), bpm: m.bpm, comment: m.comment.clone(),
            // A player's metadata has no year row for most tracks; the date
            // added fills the calendar row then.
            year: m.year.clone().or_else(|| m.added.clone()),
            artwork: i.art.as_ref().map(|b| (if b.starts_with(&[0x89, b'P']) { "image/png" } else { "image/jpeg" }.to_string(), b.clone())),
            ..Default::default()
        })
    }

    fn meta_key(l: &Load) -> Option<(Ipv4Addr, u8, u32)> {
        let Load::Db { ip, id, sd, rekordbox, .. } = l else { return None };
        let slot = if *rekordbox { Slot::Collection } else if *sd { Slot::Sd } else { Slot::Usb };
        Some((*ip, slot as u8, *id))
    }

    pub fn title(&self) -> String {
        match self.stack.last() {
            Some(Loc::Fs(p)) => p.file_name().and_then(|n| n.to_str()).unwrap_or("/").to_string(),
            Some(Loc::Link)  => "LINK".to_string(),
            Some(Loc::Connecting) => self.connecting.as_ref().map(|c| c.label.clone()).unwrap_or_else(|| "LINK".to_string()),
            Some(Loc::DbSearch(_)) => "SEARCH".to_string(),
            Some(Loc::RbRoot) => match &self.rb_source {
                Some(RbSource::Link(ip)) => format!("LINK  {ip}"),
                _ => "REKORDBOX".to_string(),
            },
            Some(Loc::RbTree(0)) => "PLAYLIST".to_string(),
            Some(Loc::RbAllTracks) => "TRACK".to_string(),
            Some(Loc::RbNames(f)) => f.label().to_string(),
            Some(Loc::RbNamed(_, n)) => if n.is_empty() { "(none)".to_string() } else { n.clone() },
            Some(Loc::RbHistory) => "HISTORY".to_string(),
            Some(Loc::Search) => "SEARCH".to_string(),
            Some(Loc::RbHistoryList(id)) => self.rb.as_ref()
                .and_then(|e| e.history.iter().find(|(h, _)| h == id).map(|(_, n)| n.clone()))
                .unwrap_or_else(|| "HISTORY".to_string()),
            Some(Loc::RbTree(id)) | Some(Loc::RbPlaylist(id)) => self.rb.as_ref()
                .and_then(|e| e.playlists.iter().find(|n| n.id == *id))
                .map(|n| n.name.clone()).unwrap_or_else(|| "rekordbox".to_string()),
            Some(Loc::DbRoot) => self.db_label.clone(),
            Some(Loc::DbFolder(0)) => "PLAYLIST".to_string(),
            Some(Loc::DbAllTracks) => "TRACK".to_string(),
            Some(Loc::DbArtists) => "ARTIST".to_string(),
            Some(Loc::DbAlbums) => "ALBUM".to_string(),
            Some(Loc::DbFilenames) => "FILENAME".to_string(),
            Some(Loc::DbCat(c, ids)) if ids.is_empty() => c.label().to_string(),
            Some(Loc::DbCat(..)) => self.db_title.clone(),
            Some(Loc::DbTree(0)) => "FOLDER".to_string(),
            Some(Loc::DbTree(_)) => self.db_title.clone(),
            Some(Loc::DbFolder(_)) | Some(Loc::DbPlaylist(_)) | Some(Loc::DbArtist(_)) | Some(Loc::DbAlbum(_)) => self.db_title.clone(),
            None => "/".to_string(),
        }
    }

    pub fn refresh(&mut self) { self.playlist_only = false; self.cat_focus = false; self.rebuild(); }
    /// BPM and key the player lists for a linked track, once fetched.
    pub fn db_bpm_key(&self, l: &Load) -> (Option<f32>, Option<String>) {
        match Self::meta_key(l).and_then(|k| self.meta_cache.get(&k)).and_then(|i| i.as_ref()) {
            Some(i) => (i.meta.bpm, i.meta.key.clone()),
            None => (None, None),
        }
    }
    /// 0 small, 1 medium, 2 large: the table's text size ("A A A").
    pub fn font_size(&self) -> u8 { self.font }
    pub fn cycle_font(&mut self) { self.font = (self.font + 1) % 3; }
    /// The linked stick being browsed: its player number and colour label,
    /// for the header tile.  None outside a linked source.
    pub fn source_badge(&self) -> Option<(u8, u8)> {
        let in_link = self.stack.iter().any(|l| matches!(l, Loc::DbRoot | Loc::RbRoot | Loc::DbFolder(_) | Loc::DbCat(..) | Loc::DbAllTracks | Loc::DbTree(_)));
        let (ip, slot) = match (&self.db, &self.rb_source) {
            (Some(DbSession { ip, slot, .. }), _) if in_link && self.stack.iter().any(|l| matches!(l, Loc::DbRoot)) => (*ip, *slot as u8),
            (_, Some(RbSource::Link(ip))) if in_link => (*ip, 3),
            _ => return None,
        };
        let player = self.link.peer_player(ip)?;
        let color = self.link.media(player).into_iter().find(|m| m.slot == slot).map_or(0, |m| m.color);
        Some((player, color))
    }
    pub fn idle(&self) -> bool { self.idle }
    /// The source a loaded track came from, for the playback screen's tile:
    /// its kind, and for a linked stick its player number and colour label.
    pub fn load_tile(&self, l: &Load) -> (SourceTile, Option<(u8, u8)>) {
        let linked = |ip: Ipv4Addr, slot: u8| {
            let player = self.link.peer_player(ip);
            let color = player.and_then(|p| self.link.media(p).into_iter().find(|m| m.slot == slot)).map_or(0, |m| m.color);
            (SourceTile::Link, player.map(|p| (p, color)))
        };
        match l {
            Load::Local { path, .. } => {
                if self.media.iter().any(|m| path.starts_with(&m.path)) { (SourceTile::Usb, None) } else { (SourceTile::Folder, None) }
            }
            Load::Link { ip, .. } => linked(*ip, 3),
            Load::Db { ip, sd, .. } => linked(*ip, if *sd { 2 } else { 3 }),
        }
    }
    pub fn previews_on(&self) -> bool { self.previews }
    pub fn toggle_previews(&mut self) { self.previews = !self.previews; }
    /// Length and stars the player lists for a linked track.
    pub fn db_extra(&self, l: &Load) -> Option<(Option<u32>, u8)> {
        let m = &self.meta_cache.get(&Self::meta_key(l)?)?.as_ref()?.meta;
        Some((m.duration, m.rating))
    }
    pub fn info_on(&self) -> bool { self.info }
    pub fn toggle_info(&mut self) { self.info = !self.info; }
    pub fn playlist_only(&self) -> bool { self.playlist_only }
    /// The table's second-row key: off → ARTIST → ALBUM → GENRE → off.
    pub fn cycle_detail(&mut self) { self.detail = (self.detail + 1) % 3; }
    pub fn detail_name(&self) -> Option<&'static str> {
        [None, Some("ARTIST"), Some("ALBUM")][self.detail as usize]
    }
    /// What the second row shows for an entry (empty when not known yet).
    pub fn detail_text(&self, e: &Entry) -> String {
        let tags = e.load().and_then(|l| self.db_tags(l));
        match self.detail {
            1 => e.artist.clone().or_else(|| tags.and_then(|t| t.artist)).unwrap_or_default(),
            2 => tags.and_then(|t| t.album).unwrap_or_default(),
            _ => String::new(),
        }
    }
    /// The blue preview of a linked track, once fetched.
    /// The 3-band preview (low, mid, high per column) of a linked track.
    pub fn db_bands(&self, l: &Load) -> Option<Arc<Vec<[u8; 3]>>> {
        self.meta_cache.get(&Self::meta_key(l)?)?.as_ref()?.bands.clone()
    }
    pub fn db_wave(&self, l: &Load) -> Option<Arc<Vec<(u8, u8)>>> {
        self.meta_cache.get(&Self::meta_key(l)?)?.as_ref()?.wave.clone()
    }

    /// What is inside the highlighted row, if it is a folder on the
    /// filesystem: `(is_dir, name)` for its folders then audio files, sorted
    /// as BROWSE sorts them. Listed once per highlighted folder and cached.
    /// None for tracks, and for rekordbox / linked-player folders, which
    /// would cost a network round trip to peek into.
    pub fn preview(&self) -> Option<Vec<(bool, String)>> {
        let Some(Entry { kind: EntryKind::Descend(Loc::Fs(dir)), .. }) = self.selected_entry() else { return None };
        if let Some((p, v)) = self.preview.borrow().as_ref() {
            if p == dir { return Some(v.clone()); }
        }
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') { continue; }
                if path.is_dir() {
                    if !is_housekeeping_dir(&name) { dirs.push(name); }
                } else if is_audio(&path) {
                    files.push(name);
                }
            }
        }
        dirs.sort_by_key(|n| n.to_lowercase());
        files.sort_by_key(|n| n.to_lowercase());
        let v: Vec<(bool, String)> = dirs.into_iter().map(|n| (true, n)).chain(files.into_iter().map(|n| (false, n))).collect();
        *self.preview.borrow_mut() = Some((dir.clone(), v.clone()));
        Some(v)
    }

    fn rebuild(&mut self) {
        self.names.resize(self.stack.len(), String::new());
        let entries = match self.stack.last().cloned().filter(|_| !self.idle) {
            Some(Loc::Fs(dir))        => self.fs_entries(&dir),
            Some(Loc::Link)           => self.link_entries(),
            Some(Loc::RbRoot)         => self.rb_root_entries(),
            Some(Loc::RbTree(node))   => self.rb_tree_entries(node),
            Some(Loc::RbPlaylist(id)) => self.rb_playlist_entries(id),
            Some(Loc::RbAllTracks)    => self.rb_track_entries(|e| e.tracks.iter().collect()),
            Some(Loc::RbNames(f))     => self.rb_name_entries(f),
            Some(Loc::RbNamed(f, n))  => self.rb_track_entries(|e| e.tracks.iter().filter(|t| f.of(t) == n).collect()),
            Some(Loc::RbHistory)      => self.rb_history_entries(),
            Some(Loc::Search)         => self.search_entries(),
            Some(Loc::RbHistoryList(id)) => self.rb_track_entries(|e| e.history_tracks(id)),
            Some(loc @ (Loc::DbRoot | Loc::DbFolder(_) | Loc::DbPlaylist(_) | Loc::DbAllTracks | Loc::DbArtists | Loc::DbArtist(_)
                        | Loc::DbAlbums | Loc::DbAlbum(_) | Loc::DbFilenames | Loc::DbCat(..) | Loc::DbTree(_))) => self.db_list(loc),
            Some(Loc::DbSearch(_))    => Vec::new(),   // a cache key, never on the stack
            Some(Loc::Connecting)     => {
                let text = match &self.connecting {
                    Some(Connecting { failed: Some(why), label, .. }) => format!("{label}: not answering ({why})"),
                    Some(c) => format!("Connecting to {}…", c.label),
                    None => String::new(),
                };
                vec![Entry { name: text, is_dir: false, artist: None, bpm: None, key: None, art: None, kind: EntryKind::Unsupported }]
            }
            None                      => Vec::new(),
        };
        self.entries = entries;
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    fn fs_entries(&mut self, dir: &Path) -> Vec<Entry> {
        let mut out: Vec<Entry> = Vec::new();

        // Source rows appear only at the top browse level (like the XDJ's source
        // buttons), not inside every folder.
        let at_top = self.stack.len() == 1;
        if at_top {
            out.push(Entry { name: "LINK".into(), is_dir: true, artist: None, bpm: None, key: None, art: None,
                kind: EntryKind::Descend(Loc::Link) });
            // Every removable mount is a source row, like the XDJ's USB slot:
            // "USB: CDJ1" (the volume label), marked when it is a rekordbox stick.
            for m in &self.media {
                if m.path == *dir { continue; }   // already browsing it as the root
                let (name, kind) = match m.access {
                    Access::NeedsGrant => (format!("USB: {}   ALLOW ACCESS", m.label), EntryKind::Grant),
                    Access::Ready if m.rekordbox => (format!("USB: {}   rekordbox", m.label), EntryKind::Descend(Loc::Fs(m.path.clone()))),
                    Access::Ready => (format!("USB: {}", m.label), EntryKind::Descend(Loc::Fs(m.path.clone()))),
                };
                out.push(Entry { name, is_dir: true, artist: None, bpm: None, key: None, art: None, kind });
            }
            if let Some(add) = media::ADD_DRIVE {
                out.push(Entry { name: format!("USB: {add}"), is_dir: true, artist: None, bpm: None, key: None, art: None,
                    kind: EntryKind::Grant });
            }
        }
        // A rekordbox export at this folder (the local root, or a stick entered
        // from its USB row) adds the playlist tree as a "rekordbox" row.
        if media::has_rekordbox(dir) {
            let need = !matches!(&self.rb_source, Some(RbSource::Local(r)) if r == dir);
            if self.rb.is_none() || need {
                match read_export(dir) {
                    Ok(exp) => { self.rb = Some(Arc::new(exp)); self.rb_source = Some(RbSource::Local(dir.to_path_buf())); }
                    Err(e)  => log::warn!("rekordbox export at {}: {e:#}", dir.display()),
                }
            }
            if matches!(&self.rb_source, Some(RbSource::Local(r)) if r == dir) {
                out.push(Entry { name: "rekordbox".into(), is_dir: true, artist: None, bpm: None, key: None, art: None,
                    kind: EntryKind::Descend(Loc::RbRoot) });
            }
        }

        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for ent in rd.flatten() {
                let path = ent.path();
                let name = ent.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') { continue; }
                if path.is_dir() {
                    if is_housekeeping_dir(&name) { continue; }
                    dirs.push(Entry { name, is_dir: true, artist: None, bpm: None, key: None, art: None,
                        kind: EntryKind::Descend(Loc::Fs(path)) });
                } else if is_audio(&path) {
                    files.push(Entry { name, is_dir: false, artist: None, bpm: None, key: None, art: None,
                        kind: EntryKind::Track(Load::Local { path, analyze: None }) });
                }
            }
        }
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out.extend(dirs);
        out.extend(files);
        out
    }

    /// The LINK source: discovered players from the ProDJ Link peer table,
    /// one row per media slot they answered a media query for, labelled the
    /// way a player's LINK list does ("3 USB: OPENDECK").  A player that has
    /// not (yet) told us about its media is listed by number and address and
    /// tried as USB.  A rekordbox laptop (device number 17+, or announcing as
    /// "rekordbox") is browsed over its dbserver collection.
    fn link_entries(&self) -> Vec<Entry> {
        let peers = self.link.peers.lock().map(|p| p.clone()).unwrap_or_default();
        let names = self.link.peer_names.lock().map(|p| p.clone()).unwrap_or_default();
        let mut v: Vec<(u8, Ipv4Addr)> = peers.into_iter().collect();
        v.sort_by_key(|(p, _)| *p);
        let dir = |name: String, kind| Entry { name, is_dir: true, artist: None, bpm: None, key: None, art: None, kind };
        let mut out = Vec::new();
        for (player, ip) in v {
            let name = names.get(&player).cloned().unwrap_or_default();
            if player >= 17 || name.to_ascii_lowercase().contains("rekordbox") {
                out.push(dir(format!("rekordbox   {ip}"), EntryKind::ConnectDb(ip)));
                continue;
            }
            let media = self.link.media(player);
            if media.is_empty() {
                out.push(dir(format!("Player {player}   {ip}"), EntryKind::ConnectLink(ip, Slot::Usb)));
            }
            for m in media {
                let slot = match m.slot { 2 => Slot::Sd, 1 => Slot::Cd, _ => Slot::Usb };
                out.push(dir(format!("{player} {}: {}", m.slot_name(), m.name), EntryKind::ConnectLink(ip, slot)));
            }
        }
        out
    }

    /// A list on the linked player, without waiting for it: a list asked
    /// before comes from this session's cache; otherwise it is asked on the
    /// session's thread and a loading row stands in until it arrives
    /// ([`prefetch_meta`](Self::prefetch_meta) takes it a frame or two
    /// later).  A player that stops answering never blocks the deck: the
    /// list then says so, and BACK and the wheel work on the lists already
    /// in.
    fn db_list(&mut self, loc: Loc) -> Vec<Entry> {
        if let Some((_, e)) = self.list_cache.iter().find(|(l, _)| *l == loc) { return e.clone(); }
        let row = |name: String| vec![Entry { name, is_dir: false, artist: None, bpm: None, key: None, art: None, kind: EntryKind::Unsupported }];
        if let Some((_, why)) = self.list_error.as_ref().filter(|(l, _)| *l == loc) {
            return row(format!("{}: not answering ({why})", if self.db_label.is_empty() { "LINK" } else { &self.db_label }));
        }
        if !matches!(&self.list_wait, Some((_, l, _)) if *l == loc) {
            let Some(db) = self.db.as_ref() else { return Vec::new() };
            self.peek_ticket += 1;
            if !db.ask_list(self.peek_ticket, Self::db_request(&loc, false)) { return Vec::new(); }
            self.list_wait = Some((self.peek_ticket, loc, false));
        }
        row("Loading…".into())
    }

    /// The request that lists `loc` on the player.  `fallback`: a source
    /// with no category menu (older servers) shows its playlist root
    /// instead of DbRoot's categories.
    fn db_request(loc: &Loc, fallback: bool) -> Box<dyn FnMut(&mut DbClient, Slot) -> anyhow::Result<Vec<MenuItem>> + Send> {
        use opendeck_dbserver::kind;
        match loc.clone() {
            Loc::DbRoot if fallback => Box::new(|c, s| c.playlist(s, 0, true)),
            Loc::DbRoot          => Box::new(|c, s| c.root_menu(s)),
            Loc::DbFolder(id)    => Box::new(move |c, s| c.playlist(s, id, true)),
            Loc::DbPlaylist(id)  => Box::new(move |c, s| c.playlist(s, id, false)),
            Loc::DbAllTracks     => Box::new(|c, s| c.all_tracks(s, 0)),
            Loc::DbArtists       => Box::new(|c, s| c.artists(s)),
            Loc::DbArtist(id)    => Box::new(move |c, s| c.tracks_for_artist(s, id)),
            Loc::DbAlbums        => Box::new(|c, s| c.albums(s)),
            Loc::DbAlbum(id)     => Box::new(move |c, s| c.tracks_for_album(s, id)),
            Loc::DbFilenames     => Box::new(|c, s| c.filenames(s)),
            Loc::DbSearch(text)  => Box::new(move |c, s| c.search(s, &text)),
            Loc::DbTree(id)      => Box::new(move |c, s| c.browse(kind::FOLDER_MENU, s, &[id, 0])),
            Loc::DbCat(cat, picked) => {
                let (k, ids) = if picked.len() >= cat.depth() { cat.tracks_request(&picked) } else { cat.names_request(&picked) };
                Box::new(move |c, s| c.browse(k, s, &ids))
            }
            _ => Box::new(|_, _| Ok(Vec::new())),
        }
    }

    /// A list arrived: rows for it, cached for the session, shown if the
    /// browser is still on it, and whatever was waiting for it run.
    fn list_arrived(&mut self, loc: Loc, fallback: bool, r: anyhow::Result<Vec<MenuItem>>) {
        // A source with no category menu shows its playlist root instead.
        if loc == Loc::DbRoot && !fallback {
            match &r {
                Ok(items) if !items.is_empty() => {}
                Ok(_) => log::info!("LINK: empty category menu; showing playlists"),
                Err(e) => log::info!("LINK: no category menu ({e:#}); showing playlists"),
            }
            if !matches!(&r, Ok(items) if !items.is_empty()) {
                if let Some(db) = self.db.as_ref() {
                    self.peek_ticket += 1;
                    if db.ask_list(self.peek_ticket, Self::db_request(&loc, true)) {
                        self.list_wait = Some((self.peek_ticket, loc, true));
                        return;
                    }
                }
            }
        }
        match r {
            Ok(items) => {
                let rows = self.db_rows(&loc, fallback, items);
                self.list_cache.push((loc.clone(), rows));
            }
            Err(e) => { log::warn!("LINK list: {e:#}"); self.list_error = Some((loc.clone(), format!("{e:#}"))); }
        }
        if self.showing(&loc) {
            let keep = self.selected;
            self.rebuild();
            self.selected = keep.min(self.entries.len().saturating_sub(1));
            if let Some(then) = self.after_list.take() { then(self); }
        }
    }

    /// Whether the browser shows `loc`'s list: it is on top of the stack, or
    /// (a linked search) SEARCH is, with that query.
    fn showing(&self, loc: &Loc) -> bool {
        match loc {
            Loc::DbSearch(q) => matches!(self.stack.last(), Some(Loc::Search)) && self.search_query == *q,
            l => self.stack.last() == Some(l),
        }
    }

    /// The list is still on its way (the loading row is up).
    fn db_loading(&self) -> bool { matches!(&self.list_wait, Some((_, l, _)) if self.showing(l)) }

    /// Run `then` once the list on screen is in: now if it is, else when it
    /// arrives (a later wait replaces an earlier one).
    fn when_listed(&mut self, then: impl FnOnce(&mut Browser) + 'static) {
        if self.db_loading() { self.after_list = Some(Box::new(then)); } else { then(self); }
    }

    /// The rows of a list the player sent for `loc`.
    fn db_rows(&mut self, loc: &Loc, fallback: bool, items: Vec<MenuItem>) -> Vec<Entry> {
        let (ip, slot) = match self.db.as_ref() { Some(d) => (d.ip, d.slot), None => return Vec::new() };
        let dir = |name: String, kind: EntryKind| Entry { name, is_dir: true, artist: None, bpm: None, key: None, art: None, kind };
        let tracks = |items: Vec<MenuItem>| -> Vec<Entry> {
            // A row we don't take for a track is dropped; say what it was,
            // so a player that types its rows differently shows in the log.
            let dropped: Vec<_> = items.iter().filter(|i| i.type_name() != "track").map(|i| i.item_type).collect();
            log::info!("LINK tracks: {} rows, {} dropped (types {:x?})", items.len(), dropped.len(), &dropped[..dropped.len().min(8)]);
            items.into_iter().filter(|i| i.type_name() == "track").map(|i| Entry {
                name: i.label.clone(), is_dir: false,
                artist: (!i.label2.is_empty()).then(|| i.label2.clone()), bpm: None, key: None, art: None,
                kind: EntryKind::Track(Load::Db { ip, id: i.id, title: i.label, rekordbox: slot == Slot::Collection, sd: slot == Slot::Sd }),
            }).collect()
        };
        match loc.clone() {
            // A playlist folder (0 = the root, which for a source without a
            // category menu also lists ALL TRACKS on top).
            Loc::DbRoot if fallback => self.db_folder_rows(0, true, items),
            Loc::DbFolder(id) => self.db_folder_rows(id, false, items),
            // The category menu: each row is typed (`root-track`, …); the
            // ones we can walk descend, the rest are shown but inert.  FOLDER
            // and SEARCH stay off the list: SEARCH is its own key, and the
            // categories cover what FOLDER would show.
            Loc::DbRoot => {
                for i in &items { log::info!("LINK category: {:?} item_type 0x{:x} id {} → {}", i.label, i.item_type, i.id, db_category(i)); }
                let rows: Vec<Entry> = items.into_iter().filter(|i| !matches!(db_category(i), "root-folder" | "root-search")).map(|i| {
                    let kind = match db_category(&i) {
                        "root-track"    => EntryKind::Descend(Loc::DbAllTracks),
                        "root-playlist" => EntryKind::Descend(Loc::DbFolder(0)),
                        "root-artist"   => EntryKind::Descend(Loc::DbArtists),
                        "root-album"    => EntryKind::Descend(Loc::DbAlbums),
                        "root-filename" => EntryKind::Descend(Loc::DbFilenames),
                        "root-folder"   => EntryKind::Descend(Loc::DbTree(0)),
                        other => match Cat::of_root(other) {
                            Some(c) => EntryKind::Descend(Loc::DbCat(c, Vec::new())),
                            None    => EntryKind::Unsupported,
                        },
                    };
                    dir(i.label, kind)
                }).collect();
                self.db_cats = rows.clone();
                rows
            }
            Loc::DbArtists => items.into_iter().map(|i| dir(i.label, EntryKind::Descend(Loc::DbArtist(i.id)))).collect(),
            Loc::DbAlbums  => items.into_iter().map(|i| dir(i.label, EntryKind::Descend(Loc::DbAlbum(i.id)))).collect(),
            // FOLDER: one level of the player's folder tree, folders then the
            // tracks in them.
            Loc::DbTree(folder) => items.into_iter().filter_map(|i| match i.type_name() {
                "folder" => Some(dir(i.label, EntryKind::Descend(Loc::DbTree(i.id)))),
                "track" => Some(Entry { name: i.label.clone(), is_dir: false,
                    artist: (!i.label2.is_empty()).then(|| i.label2.clone()), bpm: None, key: None, art: None,
                    kind: EntryKind::Track(Load::Db { ip, id: i.id, title: i.label, rekordbox: slot == Slot::Collection, sd: slot == Slot::Sd }) }),
                other => { log::info!("LINK folder {folder}: row {:?} of type {other} (0x{:x}) skipped", i.label, i.item_type); None }
            }).collect(),
            // One of the `Cat` categories: the names to pick from at this
            // level, or the tracks once every entry is picked.
            Loc::DbCat(cat, picked) if picked.len() < cat.depth() => {
                // What the player sent, so a list of blank rows shows why.
                for i in items.iter().take(3) { log::info!("LINK {} row: type 0x{:04x} id {} {:?} {:?}", cat.label(), i.item_type, i.id, i.label, i.label2); }
                items.into_iter().map(|i| {
                    let mut next = picked.clone();
                    next.push(cat_row_id(cat, picked.len(), &i));
                    dir(cat_row_name(cat, picked.len(), &i), EntryKind::Descend(Loc::DbCat(cat, next)))
                }).collect()
            }
            _ => tracks(items),
        }
    }

    /// A playlist folder's rows: its folders and playlists, after ALL
    /// TRACKS at the root of a source without a category menu.
    fn db_folder_rows(&self, folder: u32, all_tracks: bool, items: Vec<MenuItem>) -> Vec<Entry> {
        let mut out = Vec::new();
        if folder == 0 && all_tracks {
            out.push(Entry { name: "ALL TRACKS".into(), is_dir: true, artist: None, bpm: None, key: None, art: None,
                             kind: EntryKind::Descend(Loc::DbAllTracks) });
        }
        out.extend(items.into_iter().map(|i| {
            let loc = if i.type_name() == "folder" { Loc::DbFolder(i.id) } else { Loc::DbPlaylist(i.id) };
            Entry { name: i.label, is_dir: true, artist: None, bpm: None, key: None, art: None, kind: EntryKind::Descend(loc) }
        }));
        out
    }

    /// For a LOAD from a linked player: a handle on the browser's own
    /// session when it is open to that player and slot (opening a second
    /// session would cost the browser its own, see [`crate::dbsession`]),
    /// for the loader thread to ask the file path and grid on, and the
    /// metadata the highlight already fetched.  `None` when no such session
    /// is open.  It never waits on the network.
    pub fn db_track_caller(&self, ip: Ipv4Addr, slot: Slot, id: u32) -> Option<(crate::dbsession::Caller, Option<opendeck_dbserver::TrackMeta>)> {
        let db = self.db.as_ref().filter(|d| d.ip == ip && d.slot == slot)?;
        let meta = self.meta_cache.get(&(ip, slot as u8, id)).and_then(|i| i.as_ref()).map(|i| i.meta.clone());
        Some((db.caller(), meta))
    }

    /// Connect to a peer (a player's USB or SD slot, or rekordbox's
    /// collection) on a thread of its own; see [`Connecting`].  With
    /// `fallback`, a player with no database server is read from its
    /// `export.pdb` instead.  Already connected to it: opens at once.
    fn start_connect(&mut self, ip: Ipv4Addr, slot: Slot, label: String, fallback: bool) {
        if matches!(&self.db, Some(d) if d.ip == ip && d.slot == slot) {
            let session = self.db.take().unwrap();
            self.connected(ip, label, Connected::Db(session));
            return;
        }
        // Servers answer requests from a real player number (1–4).
        let device = if (1..=4).contains(&self.link.player()) { self.link.player() } else { 1 };
        // The old session (if any) closes first: one session per player number.
        self.db = None;
        self.meta_asked.clear();
        self.peeks.clear();
        self.peek_asked = None;
        self.list_cache.clear();
        self.list_wait = None;
        self.list_error = None;
        self.after_list = None;
        let ports = self.ports;
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("link-connect".into()).spawn(move || {
            let r = match DbSession::open(ip, slot, move || DbClient::discover_at(ip, ports.query, device)) {
                Ok(session) => { log::info!("LINK {ip}: dbserver session open ({slot:?})"); Connected::Db(session) }
                Err(e) if fallback => {
                    log::info!("LINK {ip}: no dbserver ({e:#}); trying export.pdb over NFS");
                    match read_link_export(ip, ports) {
                        Ok(exp) => Connected::Export(exp),
                        Err(e2) => { log::warn!("LINK connect {ip} failed: {e2:#}"); Connected::Failed(format!("{e:#}")) }
                    }
                }
                Err(e) => { log::warn!("LINK {ip} failed: {e:#}"); Connected::Failed(format!("{e:#}")) }
            };
            let _ = tx.send(r);
        });
        let failed = spawned.err().map(|e| format!("{e}"));
        self.connecting = Some(Connecting { ip, label, rx, failed });
        self.stack.push(Loc::Connecting);
        self.names.resize(self.stack.len() - 1, String::new());
        self.names.push(String::new());
        self.selected = 0;
        self.rebuild();
    }

    /// Take a connect's answer, if it has come (each frame, from
    /// [`prefetch_meta`](Self::prefetch_meta)).  Dropped if BACK has left
    /// the CONNECTING row since.
    fn poll_connect(&mut self) {
        let Some(c) = self.connecting.as_ref() else { return };
        if !matches!(self.stack.last(), Some(Loc::Connecting)) { self.connecting = None; return; }
        if c.failed.is_some() { return; }
        let r = match c.rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Connected::Failed("the connection thread stopped".into()),
        };
        let (ip, label) = (c.ip, c.label.clone());
        self.connected(ip, label, r);
    }

    /// Open what a connect brought: the player's categories, its export's,
    /// or the reason on the CONNECTING row.
    fn connected(&mut self, ip: Ipv4Addr, label: String, r: Connected) {
        if matches!(self.stack.last(), Some(Loc::Connecting)) { self.stack.pop(); self.names.pop(); }
        match r {
            Connected::Db(session) => {
                self.connecting = None;
                self.db = Some(session);
                self.db_label = label;
                self.stack.push(Loc::DbRoot);
                self.selected = 0;
                self.rebuild();
                self.open_rb_category(0);
            }
            // Its export opens on the categories, as a USB stick does.
            Connected::Export(exp) => {
                self.connecting = None;
                self.rb = Some(Arc::new(exp));
                self.rb_source = Some(RbSource::Link(ip));
                self.stack.push(Loc::RbRoot);
                self.selected = 0;
                self.rebuild();
            }
            Connected::Failed(why) => {
                if let Some(c) = self.connecting.as_mut() { c.failed = Some(why); }
                self.stack.push(Loc::Connecting);
                self.names.push(String::new());
                self.selected = 0;
                self.rebuild();
            }
        }
    }

    /// A linked player is being connected to (the app keeps drawing).
    pub fn connecting(&self) -> bool { self.connecting.as_ref().is_some_and(|c| c.failed.is_none()) }

    fn rb_tree_entries(&self, node: u32) -> Vec<Entry> {
        let Some(exp) = self.rb.as_ref() else { return Vec::new() };
        exp.children(node).into_iter().map(|n| Entry {
            name: n.name.clone(), is_dir: true, artist: None, bpm: None, key: None, art: None,
            kind: EntryKind::Descend(if n.is_folder { Loc::RbTree(n.id) } else { Loc::RbPlaylist(n.id) }),
        }).collect()
    }

    /// The export's category menu: PLAYLIST only when the stick has any, then
    /// TRACK (everything, by title) so an unsorted stick is still browsable.
    /// A rekordbox export's categories, in the order a CDJ-3000 lists them
    /// (it shows PLAYLIST even when there are none). MATCHING is shown but
    /// not walkable yet; FOLDER is the stick's files.
    fn rb_root_entries(&self) -> Vec<Entry> {
        if self.rb.is_none() { return Vec::new() }
        let mut out = Vec::new();
        let cat = |name: &str, kind: EntryKind| Entry { name: name.into(), is_dir: true, artist: None, bpm: None, key: None, art: None, kind };
        out.push(cat("ARTIST", EntryKind::Descend(Loc::RbNames(RbField::Artist))));
        out.push(cat("ALBUM", EntryKind::Descend(Loc::RbNames(RbField::Album))));
        out.push(cat("TRACK", EntryKind::Descend(Loc::RbAllTracks)));
        out.push(cat("KEY", EntryKind::Descend(Loc::RbNames(RbField::Key))));
        out.push(cat("PLAYLIST", EntryKind::Descend(Loc::RbTree(0))));
        out.push(cat("HISTORY", EntryKind::Descend(Loc::RbHistory)));
        out.push(cat("MATCHING", EntryKind::Unsupported));
        if let Some(RbSource::Local(root)) = &self.rb_source {
            out.push(cat("FOLDER", EntryKind::Descend(Loc::Fs(root.clone()))));
        }
        out
    }

    /// HISTORY: the player's history lists, newest first, as the unit lists them.
    fn rb_history_entries(&self) -> Vec<Entry> {
        let Some(exp) = self.rb.as_ref() else { return Vec::new() };
        exp.history.iter().rev().map(|(id, name)| Entry {
            name: name.clone(), is_dir: true, artist: None, bpm: None, key: None, art: None,
            kind: EntryKind::Descend(Loc::RbHistoryList(*id)),
        }).collect()
    }

    /// ARTIST / ALBUM / KEY: the distinct names, sorted, each opening its
    /// tracks. Tracks with no name group under "(none)" at the end.
    fn rb_name_entries(&self, f: RbField) -> Vec<Entry> {
        let Some(exp) = self.rb.as_ref() else { return Vec::new() };
        let mut names: Vec<String> = exp.tracks.iter().map(|t| f.of(t).to_string()).collect();
        names.sort_by_key(|n| (n.is_empty(), n.to_lowercase()));
        names.dedup();
        names.into_iter().map(|n| Entry {
            name: if n.is_empty() { "(none)".into() } else { n.clone() },
            is_dir: true, artist: None, bpm: None, key: None, art: None,
            kind: EntryKind::Descend(Loc::RbNamed(f, n)),
        }).collect()
    }

    /// The category column of a rekordbox source (CDJ-3000 skin): the
    /// categories and which one is open, while browsing inside one.
    pub fn rb_categories(&self) -> Option<(Vec<String>, Option<usize>)> {
        if let Some(r) = self.stack.iter().position(|l| matches!(l, Loc::DbRoot)) {
            // A linked player: its categories down the left, like a unit.
            if self.db_cats.is_empty() { return None; }
            let group = |l: &Loc| -> (u8, Option<Cat>) { match l {
                Loc::DbFolder(_) | Loc::DbPlaylist(_) => (0, None),
                Loc::DbArtists | Loc::DbArtist(_) => (1, None),
                Loc::DbAlbums | Loc::DbAlbum(_) => (2, None),
                Loc::DbAllTracks => (3, None),
                Loc::DbFilenames => (4, None),
                Loc::DbTree(_) => (5, None),
                Loc::DbCat(c, _) => (6, Some(*c)),
                _ => (255, None),
            } };
            let open = self.stack.get(r + 1).and_then(|l| self.db_cats.iter().position(|c| match &c.kind {
                EntryKind::Descend(k) => group(k) == group(l) && group(l).0 != 255,
                _ => false,
            }));
            return Some((self.db_cats.iter().map(|c| c.name.clone()).collect(), open));
        }
        let r = self.stack.iter().position(|l| matches!(l, Loc::RbRoot))?;
        let cats = self.rb_root_entries();
        let open = self.stack.get(r + 1).and_then(|l| cats.iter().position(|c| match (&c.kind, l) {
            (EntryKind::Descend(Loc::RbNames(a)), Loc::RbNames(b) | Loc::RbNamed(b, _)) => a == b,
            (EntryKind::Descend(Loc::RbAllTracks), Loc::RbAllTracks) => true,
            (EntryKind::Descend(Loc::RbTree(_)), Loc::RbTree(_) | Loc::RbPlaylist(_)) => true,
            (EntryKind::Descend(Loc::RbHistory), Loc::RbHistory | Loc::RbHistoryList(_)) => true,
            (EntryKind::Descend(Loc::Fs(a)), Loc::Fs(b)) => b.starts_with(a),
            _ => false,
        }));
        Some((cats.into_iter().map(|c| c.name).collect(), open))
    }

    /// What the highlighted ARTIST / ALBUM / KEY row holds, for the CDJ-3000
    /// skin's right pane: an artist's albums, else the tracks.
    pub fn rb_preview(&self) -> Option<(&'static str, Vec<String>)> {
        let exp = self.rb.as_ref()?;
        let (f, n) = match self.selected_entry() {
            Some(Entry { kind: EntryKind::Descend(Loc::RbNamed(f, n)), .. }) => (f, n),
            Some(Entry { kind: EntryKind::Descend(Loc::RbHistoryList(id)), .. }) =>
                return Some(("TRACK", exp.history_tracks(*id).iter().map(|t| t.title.clone()).collect())),
            _ => return None,
        };
        let mine = exp.tracks.iter().filter(|t| f.of(t) == n);
        if *f == RbField::Artist {
            let mut albums: Vec<String> = mine.map(|t| t.album.clone()).filter(|a| !a.is_empty()).collect();
            albums.sort_by_key(|a| a.to_lowercase());
            albums.dedup();
            Some(("ALBUM", albums))
        } else {
            let mut titles: Vec<String> = mine.map(|t| t.title.clone()).collect();
            titles.sort_by_key(|a| a.to_lowercase());
            Some(("TRACK", titles))
        }
    }

    /// Whether the HISTORY lists are shown (the CDJ skin's row icon).
    pub fn on_rb_history(&self) -> bool { matches!(self.stack.last(), Some(Loc::RbHistory)) }

    /// SEARCH: start (or restart) a search from where the browser is.
    pub fn begin_search(&mut self) {
        self.playlist_only = false;
        self.cat_focus = false;
        if !self.in_search() { self.stack.push(Loc::Search); }
        self.search_query.clear();
        self.selected = 0;
        self.rebuild();
    }
    pub fn in_search(&self) -> bool { matches!(self.stack.last(), Some(Loc::Search)) }
    pub fn search_query(&self) -> &str { &self.search_query }
    /// New search text: the results follow, the first one highlighted.
    pub fn set_search_query(&mut self, q: String) {
        self.search_query = q;
        self.selected = 0;
        if self.in_search() { self.rebuild(); }
    }

    /// Results for the query, case-insensitive. Under a rekordbox source:
    /// tracks whose title or artist contains it (the unit's SEARCH). In a
    /// folder: audio files anywhere below it whose name contains it.
    /// An empty query has no results, as on the unit.
    fn search_entries(&mut self) -> Vec<Entry> {
        let q = self.search_query.to_lowercase();
        if q.is_empty() { return Vec::new(); }
        if self.stack.iter().any(|l| matches!(l, Loc::RbRoot)) {
            return self.rb_track_entries(|e| e.tracks.iter()
                .filter(|t| t.title.to_lowercase().contains(&q) || t.artist.to_lowercase().contains(&q))
                .collect());
        }
        // A linked player's library: its own search.
        if self.db.is_some() && self.stack.iter().any(|l| matches!(l, Loc::DbRoot | Loc::DbFolder(_) | Loc::DbCat(..) | Loc::DbAllTracks)) {
            if q.chars().count() < 2 { return Vec::new(); }
            return self.db_list(Loc::DbSearch(self.search_query.clone()));
        }
        let Some(dir) = self.stack.iter().rev().find_map(|l| match l { Loc::Fs(p) => Some(p.clone()), _ => None }) else { return Vec::new() };
        let mut idx = self.fs_index.borrow_mut();
        if idx.as_ref().map(|(d, _)| d != &dir).unwrap_or(true) {
            let mut files = Vec::new();
            let mut stack = vec![(dir.clone(), 0u8)];
            while let Some((d, depth)) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') { continue; }
                    if p.is_dir() {
                        if depth < 8 && !is_housekeeping_dir(&name) { stack.push((p, depth + 1)); }
                    } else if is_audio(&p) {
                        files.push(p);
                        if files.len() >= SEARCH_INDEX_MAX { break; }
                    }
                }
                if files.len() >= SEARCH_INDEX_MAX { log::warn!("SEARCH: indexed the first {SEARCH_INDEX_MAX} files"); break; }
            }
            files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
            *idx = Some((dir, files));
        }
        idx.as_ref().unwrap().1.iter()
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&q)))
            .map(|p| Entry {
                name: p.file_name().unwrap().to_string_lossy().into_owned(), is_dir: false,
                artist: None, bpm: None, key: None, art: None,
                kind: EntryKind::Track(Load::Local { path: p.clone(), analyze: None }),
            }).collect()
    }

    /// The rekordbox field a name list groups by, while one is shown.
    pub fn rb_names_field(&self) -> Option<RbField> {
        match self.stack.last() { Some(Loc::RbNames(f)) => Some(*f), _ => None }
    }

    /// Open the `i`th rekordbox category from anywhere inside the source.
    pub fn open_rb_category(&mut self, i: usize) {
        self.playlist_only = false;
        self.cat_focus = false;
        let Some(r) = self.stack.iter().position(|l| matches!(l, Loc::RbRoot | Loc::DbRoot)) else { return };
        self.stack.truncate(r + 1);
        self.names.truncate(r + 1);
        self.list_error = None;
        self.rebuild();
        // A linked player's categories may still be on their way.
        self.when_listed(move |b| if i < b.entries.len() {
            b.selected = i;
            b.enter();
        });
    }

    fn rb_playlist_entries(&self, id: u32) -> Vec<Entry> {
        self.rb_track_entries(|e| e.playlist_tracks(id))
    }

    fn rb_track_entries<'a>(&'a self, pick: impl FnOnce(&'a RbExport) -> Vec<&'a opendeck_rekordbox::RbTrack>) -> Vec<Entry> {
        let Some(exp) = self.rb.as_ref() else { return Vec::new() };
        let src = self.rb_source.clone();
        let mut tracks = pick(exp);
        if matches!(self.stack.last(), Some(Loc::RbAllTracks | Loc::RbNamed(..) | Loc::Search)) {
            tracks.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        }
        tracks.into_iter().filter_map(|t| {
            let load = match &src {
                Some(RbSource::Local(root)) => Load::Local { path: t.path_on(root), analyze: t.analyze_on(root) },
                Some(RbSource::Link(ip))    => Load::Link { ip: *ip, rel_path: t.rel_path.clone(), analyze_rel: t.analyze_rel.clone(), id: t.id },
                None => return None,
            };
            Some(Entry {
                name: t.title.clone(), is_dir: false,
                artist: Some(t.artist.clone()), bpm: Some(t.bpm),
                key: (!t.key.is_empty()).then(|| t.key.clone()),
                art: match &src { Some(RbSource::Local(root)) => t.artwork_on(root), _ => None },
                kind: EntryKind::Track(load),
            })
        }).collect()
    }

    /// A category's top level, with the category column beside it.
    fn at_category_top(&self) -> bool {
        if self.playlist_only { return false; }
        let Some(r) = self.stack.iter().position(|l| matches!(l, Loc::RbRoot | Loc::DbRoot)) else { return false };
        self.stack.len() == r + 2 && self.rb_categories().is_some_and(|(_, open)| open.is_some())
    }

    /// The highlight is in the category column (see `cat_focus`).
    pub fn cat_focus(&self) -> bool { self.cat_focus }

    /// A row of the list picked by touch: the highlight goes there (out of
    /// the category column, if it was there).
    pub fn pick_row(&mut self, i: usize) {
        self.cat_focus = false;
        if i < self.entries.len() { self.selected = i; }
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.cat_focus {
            // Through the categories, each opening as the highlight reaches it.
            let Some((names, Some(open))) = self.rb_categories() else { return };
            let to = (open as i32 + delta).clamp(0, names.len() as i32 - 1) as usize;
            if to != open { self.open_rb_category(to); }
            self.cat_focus = true;
            return;
        }
        if self.entries.is_empty() { return; }
        let last = self.entries.len() as i32 - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub fn enter(&mut self) -> Enter {
        // From the category column back into the list, on its first row.
        if self.cat_focus {
            self.cat_focus = false;
            self.selected = 0;
            return Enter::Folder;
        }
        let Some(sel) = self.entries.get(self.selected) else { return Enter::Nothing };
        let (sel_name, kind) = (sel.name.clone(), sel.kind.clone());
        match kind {
            EntryKind::Unsupported => { log::info!("BROWSE: {:?} is not browsable here yet", sel_name); Enter::Nothing }
            EntryKind::Grant => { media::request_access(); Enter::Nothing }
            EntryKind::Descend(loc) => {
                if matches!(loc, Loc::DbFolder(_) | Loc::DbPlaylist(_) | Loc::DbArtist(_) | Loc::DbAlbum(_) | Loc::DbCat(..) | Loc::DbTree(_)) { self.db_title = sel_name.clone(); }
                let from_usb_row = sel_name.starts_with("USB: ");
                if matches!(loc, Loc::Search) { self.search_query.clear(); }
                self.stack.push(loc);
                self.names.resize(self.stack.len() - 1, String::new());
                self.names.push(sel_name.clone());
                self.selected = 0;
                self.rebuild();
                // A rekordbox stick opens on its categories, as on the units;
                // its files stay one row away under FOLDER.
                if from_usb_row { self.into_rb_root(); }
                Enter::Folder
            }
            EntryKind::ConnectDb(ip) => { self.start_connect(ip, Slot::Collection, sel_name, false); Enter::Folder }
            // A player: browse the slot over dbserver like a CDJ does; if it
            // has no database service, read its export.pdb over NFS.
            EntryKind::ConnectLink(ip, slot) => { self.start_connect(ip, slot, sel_name, true); Enter::Folder }
            EntryKind::Track(load) => Enter::Track(load),
        }
    }

    /// The SOURCE screen's rows: the local library, each mounted stick, then
    /// each source on PRO DJ LINK (the same rows the LINK list shows).
    pub fn sources(&self) -> Vec<SourceRow> {
        let name = self.home.file_name().and_then(|n| n.to_str()).unwrap_or("/").to_string();
        let mut out = vec![SourceRow {
            kind: SourceKind::Local, label: name, player: None,
            path: Some(self.home.clone()), ip: None, rekordbox: media::has_rekordbox(&self.home), info: None,
        }];
        // A row with no path asks the OS for access when opened.
        for m in &self.media {
            let (label, path) = match m.access {
                Access::Ready      => (format!("USB: {}", m.label), Some(m.path.clone())),
                Access::NeedsGrant => (format!("USB: {} (ALLOW ACCESS)", m.label), None),
            };
            out.push(SourceRow { kind: SourceKind::Usb, label, player: None, path, ip: None, rekordbox: m.rekordbox, info: None });
        }
        if let Some(add) = media::ADD_DRIVE {
            out.push(SourceRow { kind: SourceKind::Usb, label: format!("USB: {add}"), player: None, path: None, ip: None, rekordbox: false, info: None });
        }
        for e in self.link_entries() {
            let (ip, rekordbox, slot) = match e.kind {
                EntryKind::ConnectLink(ip, slot) => (Some(ip), false, Some(slot as u8)),
                EntryKind::ConnectDb(ip) => (Some(ip), true, None),
                _ => (None, false, None),
            };
            // "3 USB: NAME" → badge 3, "USB: NAME".
            let (player, label) = match e.name.split_once(' ') {
                Some((n, rest)) if !rekordbox => match n.parse::<u8>() {
                    Ok(p) => (Some(p), rest.to_string()),
                    Err(_) => (None, e.name.clone()),
                },
                _ => (None, e.name.clone()),
            };
            let info = player.zip(slot).and_then(|(p, sl)| self.link.media(p).into_iter().find(|m| m.slot == sl));
            out.push(SourceRow { kind: SourceKind::Link, label, player, path: None, ip, rekordbox, info });
        }
        out
    }

    /// Open the `i`th SOURCE row in the browser (what picking it on the
    /// CDJ's SOURCE screen does). Returns false when there is no such row or
    /// a LINK source would not connect.
    pub fn open_source(&mut self, i: usize) -> bool {
        self.idle = false;
        self.cat_focus = false;
        self.playlist_only = false;
        let rows = self.sources();
        let Some(row) = rows.get(i) else { return false };
        if row.kind == SourceKind::Usb && row.path.is_none() { media::request_access(); return false; }
        self.remember();
        if let Some(root) = row.path.clone().filter(|_| row.kind != SourceKind::Link) {
            if let Some(pos) = self.positions.get(&root).cloned() {
                if self.restore(&pos) { return true; }
            }
        }
        self.stack = vec![Loc::Fs(self.home.clone())];
        self.selected = 0;
        match row.kind {
            SourceKind::Local => { self.rebuild(); true }
            SourceKind::Usb => {
                if let Some(p) = &row.path { self.stack.push(Loc::Fs(p.clone())); }
                self.rebuild();
                // A rekordbox stick opens on its categories, ARTIST first,
                // as a CDJ does.
                if self.entries.iter().any(|e| matches!(e.kind, EntryKind::Descend(Loc::RbRoot))) {
                    self.stack.push(Loc::RbRoot);
                    self.rebuild();
                    self.open_rb_category(0);
                }
                true
            }
            SourceKind::Link => {
                let first_link = rows.iter().position(|r| r.kind == SourceKind::Link).unwrap_or(i);
                self.stack.push(Loc::Link);
                self.rebuild();
                self.selected = i - first_link;
                if matches!(self.enter(), Enter::Folder) { true } else {
                    // Stay on the LINK list with the row highlighted.
                    self.selected = i - first_link;
                    false
                }
            }
        }
    }

    /// Songs and playlists in a source's rekordbox export, when it is the one
    /// currently read (the CDJ shows 0 for a stick without one).
    pub fn rekordbox_counts(&self, root: &Path) -> Option<(usize, usize)> {
        match (&self.rb, &self.rb_source) {
            (Some(rb), Some(RbSource::Local(r))) if r == root =>
                Some((rb.tracks.len(), rb.playlists.iter().filter(|n| !n.is_folder).count())),
            _ => None,
        }
    }

    /// PLAY LIST: from anywhere inside a rekordbox or linked source, its
    /// playlist tree; outside one, the stick the browser last used.
    pub fn open_playlists(&mut self) -> bool {
        let is_root = |l: &Loc| matches!(l, Loc::RbRoot | Loc::DbRoot);
        let keep = (self.stack.clone(), self.names.clone(), self.selected, self.idle, self.entries.clone(), self.playlist_only);
        self.idle = false;
        self.cat_focus = false;
        // Not inside a source: a linked player's session still open is the
        // one the last track came from.
        if !self.stack.iter().any(is_root) {
            if self.db.is_some() && !self.db_cats.is_empty() {
                self.stack = vec![self.stack.first().cloned().unwrap_or_else(|| Loc::Fs(self.home.clone())), Loc::Link, Loc::DbRoot];
                self.names.resize(3, String::new());
            } else { self.go_root(); }
        }
        let Some(r) = self.stack.iter().position(is_root) else {
            // No stick with a library: nothing to show, as on a unit with
            // nothing loaded.
            (self.stack, self.names, self.selected, self.idle, self.entries, self.playlist_only) = keep;
            return false;
        };
        self.playlist_only = true;
        self.stack.truncate(r + 1);
        self.selected = 0;
        self.rebuild();
        self.when_listed(|b| {
            let at = b.entries.iter().position(|e| matches!(e.kind,
                EntryKind::Descend(Loc::RbTree(0) | Loc::DbFolder(0))));
            if let Some(i) = at {
                b.selected = i;
                b.enter();
            }
        });
        true
    }

    /// Which kind of source is being browsed, for the header tile.
    pub fn source_kind(&self) -> SourceTile {
        if self.idle { return SourceTile::None; }
        if self.stack.iter().any(|l| matches!(l, Loc::Link | Loc::DbRoot)) || matches!(&self.rb_source, Some(RbSource::Link(_))) && self.stack.iter().any(|l| matches!(l, Loc::RbRoot)) {
            return SourceTile::Link;
        }
        let on_stick = self.stack.iter().any(|l| matches!(l, Loc::Fs(p) if self.media.iter().any(|m| m.path == *p)));
        if on_stick { SourceTile::Usb } else { SourceTile::Folder }
    }

    /// The LINK source key: jump to the list of linked players (from wherever
    /// the browser is).
    pub fn go_link(&mut self) {
        self.idle = false;
        self.playlist_only = false;
        self.remember();
        self.stack.truncate(1);
        self.stack.push(Loc::Link);
        self.selected = 0;
        self.rebuild();
    }

    /// Standing on a stick's root folder that holds a rekordbox export: step
    /// into the category list (ARTIST, ALBUM, TRACK, … FOLDER).
    fn into_rb_root(&mut self) {
        if self.entries.iter().any(|e| matches!(e.kind, EntryKind::Descend(Loc::RbRoot))) {
            self.stack.push(Loc::RbRoot);
            self.selected = 0;
            self.rebuild();
        }
    }

    /// The USB source key: back to the local source browsed last, where it
    /// was left; with none (or it is gone), onto the stick when exactly one
    /// is mounted (on its rekordbox categories when it has an export), else
    /// the local root folder.
    pub fn go_root(&mut self) {
        self.idle = false;
        self.playlist_only = false;
        // Back to the source browsed last, where it was left, while it is
        // still there: a stick mounted since does not take the key over.
        self.remember();
        if let Some(pos) = self.last_root.as_ref().and_then(|r| self.positions.get(r)).cloned() {
            if self.restore(&pos) { return; }
        }
        self.stack.truncate(1);
        let ready: Vec<&Mount> = self.media.iter().filter(|m| m.access == Access::Ready).collect();
        if let [m] = ready.as_slice() {
            if !matches!(self.stack.first(), Some(Loc::Fs(r)) if *r == m.path) {
                self.stack.push(Loc::Fs(m.path.clone()));
            }
        }
        self.selected = 0;
        self.rebuild();
        if self.stack.len() > 1 { self.into_rb_root(); }
    }

    /// Re-scan removable media at most once a second.  A new stick shows up
    /// as a USB row; a removed one drops out, and if we were browsing it the
    /// browser falls back to the root.  Returns true when the list changed.
    pub fn poll_media(&mut self) -> bool {
        if self.media_at.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) { return false; }
        self.media_at = Some(Instant::now());
        let now = media::mounts();
        if now == self.media { return false; }
        for gone in self.media.iter().filter(|m| !now.contains(m)) {
            log::info!("media removed: {}", gone.path.display());
            if self.stack.iter().any(|l| matches!(l, Loc::Fs(p) if p.starts_with(&gone.path)))
                || self.stack.iter().any(|l| matches!(l, Loc::RbRoot | Loc::RbTree(_) | Loc::RbPlaylist(_) | Loc::RbAllTracks | Loc::RbNames(_) | Loc::RbNamed(..) | Loc::RbHistory | Loc::RbHistoryList(_)))
                    && matches!(&self.rb_source, Some(RbSource::Local(r)) if r.starts_with(&gone.path))
            {
                self.stack.truncate(1);
                self.selected = 0;
                self.browsed_gone = true;
            }
            if matches!(&self.rb_source, Some(RbSource::Local(r)) if r.starts_with(&gone.path)) {
                self.rb = None;
                self.rb_source = None;
            }
        }
        for new in now.iter().filter(|m| !self.media.contains(m)) {
            log::info!("media inserted: {}{}", new.path.display(), if new.rekordbox { " (rekordbox)" } else { "" });
        }
        self.media = now;
        self.stem_marks.get_mut().clear();
        if matches!(self.stack.last(), Some(Loc::Fs(_))) { self.rebuild(); }
        true
    }

    /// Whether the stick being browsed was pulled since the last call: the
    /// deck then goes back to SOURCE, as a CDJ does, instead of leaving
    /// BROWSE on the local folder.
    pub fn take_browsed_gone(&mut self) -> bool { std::mem::take(&mut self.browsed_gone) }

    #[cfg(test)]
    pub(crate) fn force_media_rescan(&mut self) -> bool { self.media_at = None; self.poll_media() }

    /// Tests: browse an in-memory export as if it were on local media at
    /// `root`, standing on its category list.
    #[cfg(test)]
    fn browse_rb(&mut self, exp: RbExport, root: &Path) {
        self.rb = Some(Arc::new(exp));
        self.rb_source = Some(RbSource::Local(root.to_path_buf()));
        self.stack = vec![Loc::Fs(root.to_path_buf()), Loc::RbRoot];
        self.selected = 0;
        self.rebuild();
    }

    /// Whether the browser is on the LINK side (the player list, or a linked
    /// player's library) — lights the LINK source key rather than FILE.
    pub fn on_link(&self) -> bool {
        self.stack.iter().any(|l| matches!(l, Loc::Link | Loc::DbRoot))
            || (matches!(self.stack.last(), Some(Loc::RbTree(_) | Loc::RbPlaylist(_)))
                && matches!(self.rb_source, Some(RbSource::Link(_))))
    }

    /// The browse positions file: the source browsed last, then the last
    /// position in each local source.
    fn positions_file() -> PathBuf { crate::taglist::app_data_dir().join("browse.json") }

    /// Save where the browser is (and was in each local source), so the
    /// next launch opens BROWSE there.  Not in tests.
    pub fn save_positions(&mut self) {
        self.remember();
        if cfg!(test) { return; }
        let mut all: Vec<BrowsePos> = Vec::new();
        if let Some(p) = self.last_root.as_ref().and_then(|r| self.positions.get(r)) { all.push(p.clone()); }
        all.extend(self.positions.values().filter(|p| Some(&p.root) != self.last_root.as_ref()).cloned());
        match serde_json::to_vec_pretty(&all) {
            Ok(b) => if let Err(e) = std::fs::write(Self::positions_file(), b) { log::warn!("browse positions: {e}"); },
            Err(e) => log::warn!("browse positions: {e}"),
        }
    }

    /// At launch: the saved positions, and BROWSE back where it was left
    /// when that source is still there (a stick that is not inserted is
    /// skipped; its position waits for it).
    pub fn load_positions(&mut self) {
        if cfg!(test) { return; }
        let Ok(b) = std::fs::read(Self::positions_file()) else { return };
        let Ok(all) = serde_json::from_slice::<Vec<BrowsePos>>(&b) else { return };
        for p in &all { self.positions.insert(p.root.clone(), p.clone()); }
        if let Some(last) = all.first() {
            if self.restore(last) { log::info!("browse: back at {} / {}", last.root.display(), last.names.join(" / ")); }
        }
    }

    /// Highlight the row called `name`, if the list has one.
    fn select_named(&mut self, name: &str) {
        if name.is_empty() { return; }
        if let Some(i) = self.entries.iter().position(|e| e.name == name) { self.selected = i; }
    }

    /// The position in a local source, if the browser is in one: the root
    /// is the home folder or the stick whose folder is on the stack.
    pub fn position(&self) -> Option<BrowsePos> {
        if self.stack.iter().any(|l| matches!(l, Loc::Link | Loc::DbRoot | Loc::Search)) || self.on_link() { return None; }
        if matches!(self.rb_source, Some(RbSource::Link(_))) && self.stack.iter().any(|l| !matches!(l, Loc::Fs(_))) { return None; }
        // The source's base: the deepest folder that is a mount, else the
        // first (the home folder, or wherever BACK climbed to).
        let base = self.stack.iter().rposition(|l| matches!(l, Loc::Fs(p) if self.media.iter().any(|m| m.path == *p)))
            .unwrap_or(0);
        let Some(Loc::Fs(root)) = self.stack.get(base) else { return None };
        Some(BrowsePos {
            root: root.clone(),
            names: self.names.get(base + 1..).map(|n| n.to_vec()).unwrap_or_default(),
            selected: self.selected_entry().map(|e| e.name.clone()).unwrap_or_default(),
        })
    }

    /// Note where the browser is, for its source (see `positions`).
    pub fn remember(&mut self) {
        if let Some(p) = self.position() {
            self.last_root = Some(p.root.clone());
            self.positions.insert(p.root.clone(), p);
        }
    }

    /// Go to a saved position: its source's root, then the same rows by
    /// name.  Stops where a row is gone (a renamed folder, another stick),
    /// staying as deep as it got.  False when the root itself is gone.
    pub fn restore(&mut self, pos: &BrowsePos) -> bool {
        if !pos.root.is_dir() { return false; }
        let is_mount = self.media.iter().any(|m| m.path == pos.root && m.access == Access::Ready);
        if pos.root != self.home && !is_mount && !pos.root.starts_with(&self.home) { return false; }
        self.stack = vec![Loc::Fs(self.home.clone())];
        if pos.root != self.home { self.stack.push(Loc::Fs(pos.root.clone())); }
        self.names.clear();
        self.selected = 0;
        self.rebuild();
        // A rekordbox stick is browsed through its categories, folders under
        // FOLDER.  A position saved inside its folders without that level
        // (walked from the stick's own rows) comes back under FOLDER, so the
        // category column is there to get to ARTIST, TRACK… again.
        let on_rb_stick = pos.root != self.home
            && self.entries.iter().any(|e| matches!(e.kind, EntryKind::Descend(Loc::RbRoot)));
        let names: Vec<String> = if on_rb_stick && pos.names.first().is_some_and(|n| !n.is_empty()) {
            ["".to_string(), "FOLDER".to_string()].into_iter().chain(pos.names.iter().cloned()).collect()
        } else { pos.names.clone() };
        for name in &names {
            if name.is_empty() {
                // A level the browser opened on its own: a stick's categories.
                if !matches!(self.stack.last(), Some(Loc::RbRoot)) { self.into_rb_root(); }
                continue;
            }
            let Some(i) = self.entries.iter().position(|e| &e.name == name) else { break };
            if !matches!(self.entries[i].kind, EntryKind::Descend(_)) { break; }
            self.selected = i;
            if !matches!(self.enter(), Enter::Folder) { break; }
        }
        self.select_named(&pos.selected);
        self.last_root = Some(pos.root.clone());
        true
    }

    pub fn back(&mut self) {
        if self.playlist_only && matches!(self.stack.last(), Some(Loc::DbFolder(0) | Loc::RbTree(0))) { return; }
        // A source's categories are as far back as BACK goes, as on the unit:
        // from a category's top level (ALBUM's albums) it moves the highlight
        // into the category column, and there it does nothing.  The folders
        // and the other sources are reached through FOLDER and SOURCE, not by
        // backing out of the stick.
        if self.cat_focus { return; }
        if self.at_category_top() { self.cat_focus = true; return; }
        if matches!(self.stack.last(), Some(Loc::DbRoot | Loc::RbRoot)) { return; }
        if self.stack.len() > 1 {
            self.stack.pop();
            let came_from = self.names.pop().unwrap_or_default();
            self.selected = 0;
            self.rebuild();
            self.when_listed(move |b| b.select_named(&came_from));
        } else if let Some(Loc::Fs(dir)) = self.stack.last().cloned() {
            if self.floor.as_deref() == Some(dir.as_path()) { return; }
            if let Some(parent) = dir.parent().map(Path::to_path_buf) {
                self.stack = vec![Loc::Fs(parent)];
                self.rebuild();
                self.selected = self.entries.iter()
                    .position(|e| matches!(&e.kind, EntryKind::Descend(Loc::Fs(p)) if *p == dir))
                    .unwrap_or(0);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// BPM on a CDJ-3000: the tempos, then a tempo's ranges (±0 … ±6), then
    /// the tracks.  Neither level has labels: the tempo ×100 and the range
    /// are the rows' ids, and lists of them came up blank.
    #[test]
    fn bpm_lists_tempos_then_ranges_then_tracks() {
        let row = |t: u32, id: u32, label: &str| opendeck_dbserver::MenuItem {
            parent_id: 0, id, label: label.into(), label2: String::new(), item_type: t, flags: 0,
            artwork_id: 0, position: 0, key: String::new(), bpm_x100: 0 };
        assert_eq!(Cat::Bpm.depth(), 2);
        assert_eq!(Cat::Bpm.names_request(&[]), (opendeck_dbserver::kind::BPM_MENU, vec![]));
        assert_eq!(Cat::Bpm.names_request(&[12800]), (0x1106, vec![12800]));
        assert_eq!(Cat::Bpm.tracks_request(&[12800, 3]), (0x1206, vec![12800, 3]));
        assert_eq!(cat_row_name(Cat::Bpm, 0, &row(0x000d, 12800, "")), "128");
        assert_eq!(cat_row_name(Cat::Bpm, 0, &row(0x000d, 12850, "")), "128.5");
        assert_eq!(cat_row_name(Cat::Bpm, 1, &row(0, 0, "")), "\u{b1}0");
        assert_eq!(cat_row_name(Cat::Bpm, 1, &row(0, 6, "")), "\u{b1}6");
        // A label, when the player sends one, wins.
        assert_eq!(cat_row_name(Cat::Bpm, 1, &row(0, 2, "\u{b1}2")), "\u{b1}2");
        assert_eq!(cat_row_name(Cat::Genre, 0, &row(0x0006, 4, "Techno")), "Techno");
    }

    /// The right-hand pane shows what the highlighted row opens: a BPM's
    /// ranges, a range's tracks, a genre's artists, a playlist folder's
    /// playlists, named as the level itself names them.
    #[test]
    fn the_pane_lists_what_a_highlighted_linked_row_opens() {
        let row = |t: u32, id: u32, label: &str| opendeck_dbserver::MenuItem {
            parent_id: 0, id, label: label.into(), label2: String::new(), item_type: t, flags: 0,
            artwork_id: 0, position: 0, key: String::new(), bpm_x100: 0 };
        // BPM 128 highlighted: its ranges.
        let p = Peek::of(&Loc::DbCat(Cat::Bpm, vec![12800])).unwrap();
        assert!(!p.lists_tracks());
        assert_eq!(p.rows(vec![row(0, 0, ""), row(0, 1, "")]), ("BPM", vec![(true, "\u{b1}0".to_string()), (true, "\u{b1}1".to_string())]));
        // ±1 highlighted: the tracks, typed rows included, anything else left out.
        let p = Peek::of(&Loc::DbCat(Cat::Bpm, vec![12800, 1])).unwrap();
        assert!(p.lists_tracks());
        assert_eq!(p.rows(vec![row(0x0704, 9, "Song"), row(0x000d, 12800, "")]), ("TRACK", vec![(false, "Song".to_string())]));
        // A genre: its artists.
        let p = Peek::of(&Loc::DbCat(Cat::Genre, vec![4])).unwrap();
        assert_eq!(p.rows(vec![row(0x0007, 2, "Artist")]).1, vec![(true, "Artist".to_string())]);
        // A playlist folder: its playlists; the FOLDER tree: folders and tracks.
        assert_eq!(Peek::of(&Loc::DbFolder(3)).unwrap().rows(vec![row(0x0008, 5, "Set")]), ("PLAYLIST", vec![(true, "Set".to_string())]));
        assert_eq!(Peek::of(&Loc::DbTree(1)).unwrap().rows(vec![row(0x0001, 2, "Dir"), row(0x0004, 3, "Song")]).1,
                   vec![(true, "Dir".to_string()), (false, "Song".to_string())]);
        // Rows that open nothing on the player have no pane.
        assert!(Peek::of(&Loc::Link).is_none());
    }

    // A rekordbox stick appears as a USB row, its playlist tree opens from
    // the "rekordbox" row, and pulling the stick drops the browser back to
    // the root.  The "stick" is the rekordcrate demo export copied under a
    // fake media root that OPENDECK_MEDIA_DIRS points at (process-wide env:
    // this is the only test that sets it).
    /// Dev: what the browser lists for a real stick at each level.
    /// `OPENDECK_REAL_STICK=/run/media/$USER/CDJ2 cargo test -p opendeck-app real_stick -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_stick_listing() {
        let Ok(stick) = std::env::var("OPENDECK_REAL_STICK") else { return };
        let link = crate::prodj::LinkState::new(1);
        let home = crate::media::tests::tempfile_like::Dir::new("browser-real");
        let mut b = Browser::new(home.path(), link);
        b.media = vec![Mount { path: PathBuf::from(&stick), label: "STICK".into(), rekordbox: media::has_rekordbox(Path::new(&stick)), access: Access::Ready }];
        b.media_at = Some(Instant::now());
        b.rebuild();
        let show = |b: &Browser, what: &str| println!("{what} [{}]: {:?}", b.title(), b.entries().iter().map(|e| e.name.clone()).collect::<Vec<_>>());
        show(&b, "top");
        assert!(Path::new(&stick).join("PIONEER").is_dir(), "{stick} is not mounted");
        let rows = b.sources();
        let i = rows.iter().position(|r| r.kind == SourceKind::Usb).unwrap();
        assert!(b.open_source(i));
        show(&b, "SOURCE → USB");
        b.back();
        show(&b, "back");
        b.back();
        show(&b, "back");
        b.stack.truncate(1); b.rebuild();
        let u = b.entries().iter().position(|e| e.name.starts_with("USB:")).unwrap();
        b.selected = u; b.enter();
        show(&b, "USB row");
    }

    // A drive the OS has not opened up yet (Android before "All files
    // access") is a row that asks for access, in the browser and on SOURCE,
    // and never browses into the unreadable folder.
    #[test]
    fn a_drive_needing_access_asks_for_it_instead_of_opening() {
        let link = crate::prodj::LinkState::new(1);
        let home = crate::media::tests::tempfile_like::Dir::new("browser-grant");
        let mut b = Browser::new(home.path(), link);
        b.media = vec![Mount { path: PathBuf::from("/storage/1A2B-3C4D"), label: "1A2B-3C4D".into(),
                               rekordbox: false, access: Access::NeedsGrant }];
        b.media_at = Some(Instant::now());   // keep poll_media from rescanning
        b.rebuild();
        let i = b.entries().iter().position(|e| e.name == "USB: 1A2B-3C4D   ALLOW ACCESS").expect("grant row");
        let before = crate::media::REQUESTS.load(std::sync::atomic::Ordering::Relaxed);
        b.selected = i;
        assert!(matches!(b.enter(), Enter::Nothing));
        assert_eq!(b.stack.len(), 1, "stayed at the top");
        assert!(crate::media::REQUESTS.load(std::sync::atomic::Ordering::Relaxed) > before, "asked the OS");

        let rows = b.sources();
        let r = rows.iter().position(|r| r.kind == SourceKind::Usb).expect("SOURCE row");
        assert!(rows[r].path.is_none() && rows[r].label.contains("ALLOW ACCESS"));
        assert!(!b.open_source(r), "opening it asks, does not browse");
        // USB key with only a locked drive stays on the local root.
        b.go_root();
        assert_eq!(b.stack.len(), 1);
    }

    #[test]
    fn usb_stick_is_detected_entered_and_survives_removal() {
        let _env = MEDIA_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let (root, stick) = crate::media::tests::fake_media_root("browser-usb");
        std::env::set_var("OPENDECK_MEDIA_DIRS", root.path());
        let link = crate::prodj::LinkState::new(1);
        let home = crate::media::tests::tempfile_like::Dir::new("browser-home");
        let mut b = Browser::new(home.path(), link);

        let usb = b.entries().iter().position(|e| e.name.starts_with("USB: CDJ1")).expect("USB row");
        assert!(b.entries()[usb].name.contains("rekordbox"), "row marks the export: {}", b.entries()[usb].name);
        assert!(b.entries().iter().all(|e| e.name != "rekordbox"), "no rekordbox row at the home folder");

        // A rekordbox stick opens straight on its categories, as on the units.
        b.selected = usb;
        assert!(matches!(b.enter(), Enter::Folder));
        assert_eq!(b.title(), "REKORDBOX");
        // FOLDER shows its files, without the system folders a player hides.
        std::fs::create_dir_all(stick.join("CDJMODS")).unwrap();
        std::fs::create_dir_all(stick.join("System Volume Information")).unwrap();
        std::fs::create_dir_all(stick.join("Contents")).unwrap();
        let f = b.entries().iter().position(|e| e.name == "FOLDER").expect("FOLDER category");
        b.selected = f;
        assert!(matches!(b.enter(), Enter::Folder));
        let names: Vec<&str> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"Contents"), "{names:?}");
        for hidden in ["PIONEER", "CDJMODS", "System Volume Information"] {
            assert!(!names.contains(&hidden), "{hidden} hidden: {names:?}");
        }
        // BACK from FOLDER's top level: the highlight goes to the category
        // column (not out of the stick).
        b.back();
        assert!(b.cat_focus());
        // The demo export has no playlists, but PLAYLIST is listed anyway (a
        // CDJ-3000 always shows it); TRACK lists both tracks.
        let cats = b.rb_categories().unwrap().0;
        assert!(cats.iter().any(|c| c == "PLAYLIST"));
        b.open_rb_category(cats.iter().position(|c| c.starts_with("TRACK")).expect("TRACK category"));
        assert_eq!(b.title(), "TRACK");
        let names: Vec<&str> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Demo Track 1", "Demo Track 2"]);
        let load = b.entries()[0].load().expect("track row loads");
        assert!(matches!(load, Load::Local { path, analyze: Some(a) }
            if path.starts_with(&stick) && a.starts_with(&stick) && a.ends_with("ANLZ0000.DAT")),
            "paths resolve on the stick with its ANLZ grid");

        // The USB source key keeps the place on the stick being browsed.
        b.go_root();
        assert_eq!(b.title(), "TRACK", "the key returns where the stick was left");
        assert!(matches!(b.stack.get(1), Some(Loc::Fs(p)) if *p == stick));

        // Pull the stick: the row vanishes and the browser is back at home.
        std::fs::remove_dir_all(&stick).unwrap();
        assert!(b.force_media_rescan(), "change noticed");
        assert_eq!(b.stack.len(), 1);
        assert!(b.entries().iter().all(|e| !e.name.starts_with("USB:")), "no USB row left");
        assert!(b.rb.is_none(), "export dropped with the stick");
        std::env::remove_var("OPENDECK_MEDIA_DIRS");
    }

    // The CDJ-3000 skin's right pane: the highlighted folder's contents,
    // folders first then audio files, case-insensitively sorted, without
    // hidden files, non-audio files or housekeeping folders; nothing for a
    // highlighted track.
    #[test]
    fn preview_lists_the_highlighted_folder() {
        let home = crate::media::tests::tempfile_like::Dir::new("browser-preview");
        let crate_dir = home.path().join("Crate");
        std::fs::create_dir_all(crate_dir.join("inner")).unwrap();
        std::fs::create_dir_all(crate_dir.join("_CodeSignature")).unwrap();
        std::fs::create_dir_all(crate_dir.join("htdemucs.mlpackage")).unwrap();
        for f in ["b.mp3", "A.wav", "notes.txt", ".hidden.mp3"] {
            std::fs::write(crate_dir.join(f), b"").unwrap();
        }
        std::fs::write(home.path().join("loose.mp3"), b"").unwrap();
        let mut b = Browser::new(home.path(), crate::prodj::LinkState::new(1));

        b.selected = b.entries().iter().position(|e| e.name == "Crate").expect("Crate row");
        let want = vec![(true, "inner".to_string()), (false, "A.wav".to_string()), (false, "b.mp3".to_string())];
        assert_eq!(b.preview(), Some(want.clone()));
        assert_eq!(b.preview(), Some(want), "cached second call agrees");

        b.selected = b.entries().iter().position(|e| e.name == "loose.mp3").expect("track row");
        assert_eq!(b.preview(), None);
    }

    #[test]
    fn search_finds_files_below_the_folder() {
        let home = crate::media::tests::tempfile_like::Dir::new("browser-search");
        std::fs::create_dir_all(home.path().join("Crate/deep")).unwrap();
        for f in ["Crate/ALOTT - Lights Out.mp3", "Crate/deep/Alok - Cant Get Enough.mp3", "Other.mp3", "Crate/notes.txt"] {
            std::fs::write(home.path().join(f), b"").unwrap();
        }
        let mut b = Browser::new(home.path(), crate::prodj::LinkState::new(1));
        b.begin_search();
        assert!(b.in_search());
        assert!(b.entries().is_empty(), "empty query, no results");
        b.set_search_query("alo".into());
        let names: Vec<_> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alok - Cant Get Enough.mp3", "ALOTT - Lights Out.mp3"], "sorted by name");
        b.set_search_query("zzz".into());
        assert!(b.entries().is_empty());
        b.back();
        assert!(!b.in_search(), "BACK leaves SEARCH");
    }

    #[test]
    fn search_in_rekordbox_matches_title_or_artist() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/rekordcrate/data/complete_export/demo_tracks");
        let mut b = Browser::new(&root, crate::prodj::LinkState::new(1));
        b.selected = b.entries().iter().position(|e| e.name == "rekordbox").unwrap();
        b.enter();
        b.open_rb_category(0);
        b.begin_search();
        b.set_search_query("loopm".into());   // the artist
        assert_eq!(b.entries().len(), 2);
        b.set_search_query("track 2".into()); // a title
        assert_eq!(b.entries().len(), 1);
        assert_eq!(b.entries()[0].key.as_deref(), Some("Fm"));
    }

    /// SOURCE rows for LINK: a player's media carries its player number as a
    /// badge ("3" + "USB: OPENDECK"); a rekordbox laptop gets no badge.
    #[test]
    fn source_rows_for_link_players_and_rekordbox() {
        let home = crate::media::tests::tempfile_like::Dir::new("browser-src-link");
        let link = crate::prodj::LinkState::new(1);
        let cdj = Ipv4Addr::new(169, 254, 5, 55);
        let laptop = Ipv4Addr::new(169, 254, 9, 9);
        link.peers.lock().unwrap().extend([(3u8, cdj), (17u8, laptop)]);
        link.peer_names.lock().unwrap().extend([(3u8, "CDJ-3000".to_string()), (17u8, "rekordbox".to_string())]);
        link.peer_media.lock().unwrap().insert((3, 3), (opendeck_link::prodj::MediaInfo {
            player: 3, slot: 3, name: "OPENDECK".into(), tracks: 12, playlists: 1,
            date: "2026-09-17".into(), total: 32 << 30, free: 16 << 30, color: 0,
        }, std::time::Instant::now()));
        let b = Browser::new(home.path(), link);
        let rows = b.sources();
        let usb = rows.iter().find(|r| r.kind == SourceKind::Link && !r.rekordbox).expect("player media row");
        assert_eq!((usb.player, usb.label.as_str(), usb.ip), (Some(3), "USB: OPENDECK", Some(cdj)));
        let rb = rows.iter().find(|r| r.rekordbox && r.kind == SourceKind::Link).expect("rekordbox row");
        assert_eq!((rb.player, rb.ip), (None, Some(laptop)));
        // A track from that player belongs to its row, not to the laptop's.
        let from_cdj = Load::Db { ip: cdj, id: 7, title: String::new(), rekordbox: false, sd: false };
        assert!(usb.holds(&from_cdj) && !rb.holds(&from_cdj));
        assert!(!rows[0].holds(&from_cdj), "nor to the local folder");
    }

    /// Picking a rekordbox stick on SOURCE lands on its first category,
    /// ARTIST, as a CDJ does; a plain folder lands on its files.
    #[test]
    fn a_rekordbox_stick_from_source_opens_on_artist() {
        let _env = MEDIA_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let (root, _stick) = crate::media::tests::fake_media_root("browser-src-rb");
        std::env::set_var("OPENDECK_MEDIA_DIRS", root.path());
        let home = crate::media::tests::tempfile_like::Dir::new("browser-src-rb-home");
        let mut b = Browser::new(home.path(), crate::prodj::LinkState::new(1));
        let rows = b.sources();
        let i = rows.iter().position(|r| r.kind == SourceKind::Usb && r.label == "USB: CDJ1").expect("the stick");
        assert!(rows[i].rekordbox);
        assert!(b.open_source(i));
        assert_eq!(b.title(), "ARTIST");
        assert_eq!(b.rb_categories().unwrap().1, Some(0));
        assert!(b.open_source(0));
        assert!(b.rb_categories().is_none(), "the local folder has no categories");
        std::env::remove_var("OPENDECK_MEDIA_DIRS");
    }

    /// Tests that point OPENDECK_MEDIA_DIRS at a fake stick run one at a
    /// time: the variable is process-wide and the tests run in parallel.
    /// Tests that set OPENDECK_MEDIA_DIRS hold this (one process, one env).
    pub(crate) static MEDIA_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn library() -> RbExport {
        let t = |id: u32, title: &str, artist: &str, album: &str, key: &str| opendeck_rekordbox::RbTrack {
            id, title: title.into(), artist: artist.into(), album: album.into(), key: key.into(),
            bpm: 120.0 + id as f32, rel_path: format!("/Contents/{title}.mp3"), ..Default::default()
        };
        RbExport::from_parts(PathBuf::from("/stick"), vec![
            t(1, "Lights Out", "ALOTT", "Lights Out EP", "Fm"),
            t(2, "Daddy", "Ali Storm", "B Album", "Am"),
            t(3, "Fans Only", "Ali Storm", "A Album", "Am"),
            t(4, "Fans Only (Dub)", "Ali Storm", "A Album", ""),
            t(5, "Untitled", "", "", "Em"),
        ], Vec::new(), vec![
            (1, "HISTORY 001".into(), vec![2, 1]),
            (2, "HISTORY 002".into(), vec![5, 3, 1]),
        ])
    }

    #[test]
    fn rekordbox_names_group_sort_and_preview() {
        let mut b = Browser::new(Path::new("/"), crate::prodj::LinkState::new(1));
        b.browse_rb(library(), Path::new("/stick"));
        b.open_rb_category(0); // ARTIST
        let names: Vec<_> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Ali Storm", "ALOTT", "(none)"], "case-insensitive, unnamed last");
        // The right pane: an artist's albums, sorted and unique.
        b.selected = 0;
        assert_eq!(b.rb_preview(), Some(("ALBUM", vec!["A Album".to_string(), "B Album".to_string()])));
        b.enter();
        let titles: Vec<_> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(titles, ["Daddy", "Fans Only", "Fans Only (Dub)"]);
        // KEY: the keys, a track without one under (none); a key previews tracks.
        b.open_rb_category(3);
        let keys: Vec<_> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(keys, ["Am", "Em", "Fm", "(none)"]);
        b.selected = 0;
        assert_eq!(b.rb_preview().unwrap().1, vec!["Daddy".to_string(), "Fans Only".to_string()]);
        b.selected = 3;
        b.enter();
        assert_eq!(b.entries().len(), 1);
        assert_eq!(b.entries()[0].key, None);
        assert_eq!(b.title(), "(none)");
    }

    #[test]
    fn rekordbox_history_is_newest_first_in_play_order() {
        let mut b = Browser::new(Path::new("/"), crate::prodj::LinkState::new(1));
        b.browse_rb(library(), Path::new("/stick"));
        let h = b.rb_categories().unwrap().0.iter().position(|c| c == "HISTORY").unwrap();
        b.open_rb_category(h);
        assert!(b.on_rb_history());
        let lists: Vec<_> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(lists, ["HISTORY 002", "HISTORY 001"]);
        b.selected = 0;
        assert_eq!(b.rb_preview().unwrap(), ("TRACK", vec!["Untitled".into(), "Fans Only".into(), "Lights Out".into()]));
        b.enter();
        assert_eq!(b.title(), "HISTORY 002");
        let played: Vec<_> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(played, ["Untitled", "Fans Only", "Lights Out"], "play order, not sorted");
        assert_eq!(b.rb_categories().unwrap().1, Some(h), "still under HISTORY");
    }

    #[test]
    fn rekordbox_track_rows_carry_key_bpm_and_paths() {
        let mut b = Browser::new(Path::new("/"), crate::prodj::LinkState::new(1));
        b.browse_rb(library(), Path::new("/stick"));
        b.open_rb_category(2); // TRACK
        let e = b.entries().iter().find(|e| e.name == "Lights Out").unwrap();
        assert_eq!((e.key.as_deref(), e.bpm), (Some("Fm"), Some(121.0)));
        match e.load() {
            Some(Load::Local { path, .. }) => assert_eq!(path, &PathBuf::from("/stick/Contents/Lights Out.mp3")),
            _ => panic!("local load"),
        }
        b.begin_search();
        b.set_search_query("ali".into());
        assert_eq!(b.entries().len(), 3, "artist match: Ali Storm's three");
        b.set_search_query("DUB".into());
        assert_eq!(b.entries().len(), 1, "title match, any case");
    }

    #[test]
    fn played_tracks_are_remembered_once() {
        let home = crate::media::tests::tempfile_like::Dir::new("browser-played");
        let mut b = Browser::new(home.path(), crate::prodj::LinkState::new(1));
        let a = Load::Local { path: home.path().join("a.mp3"), analyze: None };
        let c = Load::Local { path: home.path().join("c.mp3"), analyze: None };
        assert!(!b.was_played(&a));
        b.note_played(&a);
        b.note_played(&a);
        assert!(b.was_played(&a) && !b.was_played(&c));
        assert_eq!(b.played.len(), 1);
    }

    /// rekordcrate's demo export (2 tracks, artist Loopmasters, key Fm).
    #[test]
    fn rekordbox_categories_walk_like_a_cdj() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/rekordcrate/data/complete_export/demo_tracks");
        let mut b = Browser::new(&root, crate::prodj::LinkState::new(1));
        b.selected = b.entries().iter().position(|e| e.name == "rekordbox").expect("rekordbox row");
        b.enter();
        let names: Vec<_> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(&names[..4], &["ARTIST", "ALBUM", "TRACK", "KEY"]);
        assert_eq!(&names[4..], &["PLAYLIST", "HISTORY", "MATCHING", "FOLDER"]);
        // At the root no category is open yet.
        assert_eq!(b.rb_categories().unwrap().1, None);

        b.open_rb_category(0);
        assert_eq!(b.title(), "ARTIST");
        assert_eq!(b.entries()[0].name, "Loopmasters");
        assert_eq!(b.rb_categories().unwrap().1, Some(0));
        b.enter();
        assert_eq!(b.title(), "Loopmasters");
        assert_eq!(b.entries().len(), 2);
        assert_eq!(b.rb_categories().unwrap().1, Some(0), "still under ARTIST");

        // Jump straight to KEY from inside ARTIST.
        b.open_rb_category(3);
        assert_eq!(b.title(), "KEY");
        assert_eq!(b.entries()[0].name, "Fm");
        b.enter();
        assert!(b.entries().iter().all(|e| e.key.as_deref() == Some("Fm") && e.load().is_some()));
        // TRACK lists everything, sorted by title.
        b.open_rb_category(2);
        let titles: Vec<_> = b.entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(titles, vec!["Demo Track 1".to_string(), "Demo Track 2".to_string()]);
        // HISTORY opens its lists (the demo export has none); MATCHING
        // is shown but does not open.
        let cats = b.rb_categories().unwrap().0;
        b.open_rb_category(cats.iter().position(|c| c == "HISTORY").unwrap());
        assert_eq!(b.title(), "HISTORY");
        assert!(b.entries().is_empty());
        b.open_rb_category(cats.iter().position(|c| c == "MATCHING").unwrap());
        assert_eq!(b.title(), "REKORDBOX", "stays on the categories");
    }

    /// BACK climbs to a category's top level, then moves the highlight into
    /// the category column, and there does nothing more, as on the unit: from
    /// ALBUM it does not back out of the stick (FOLDER and SOURCE lead
    /// there).  In the column the wheel opens each category it reaches, and
    /// pressing it goes back into the list.
    #[test]
    fn back_moves_into_the_category_column_and_no_further() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/rekordcrate/data/complete_export/demo_tracks");
        let mut b = Browser::new(&root, crate::prodj::LinkState::new(1));
        b.selected = b.entries().iter().position(|e| e.name == "rekordbox").expect("rekordbox row");
        b.enter();
        b.open_rb_category(1);
        assert_eq!(b.title(), "ALBUM");
        b.enter();
        assert_ne!(b.title(), "ALBUM", "into an album");
        b.back();
        assert_eq!((b.title().as_str(), b.cat_focus()), ("ALBUM", false), "out of the album");
        b.back();
        assert_eq!((b.title().as_str(), b.cat_focus()), ("ALBUM", true), "the highlight in the column, ALBUM still open");
        assert_eq!(b.rb_categories().unwrap().1, Some(1));
        b.back();
        b.back();
        assert_eq!((b.title().as_str(), b.cat_focus()), ("ALBUM", true), "BACK goes no further");
        // The wheel walks the column, opening each category.
        b.move_selection(1);
        assert_eq!((b.title().as_str(), b.cat_focus()), ("TRACK", true));
        b.move_selection(-2);
        assert_eq!((b.title().as_str(), b.cat_focus()), ("ARTIST", true));
        b.move_selection(-1);
        assert_eq!(b.title(), "ARTIST", "the top of the column");
        // Pressing it: back into the list, on its first row.
        assert!(matches!(b.enter(), Enter::Folder));
        assert_eq!((b.title().as_str(), b.cat_focus(), b.selected), ("ARTIST", false, 0));
        b.move_selection(1);
        assert_eq!((b.title().as_str(), b.cat_focus()), ("ARTIST", false), "the wheel is the list's again");
        // A tapped row takes the highlight back too (TRACK: two rows).
        b.back();
        assert!(b.cat_focus());
        b.move_selection(2);
        assert_eq!((b.title().as_str(), b.cat_focus()), ("TRACK", true));
        b.pick_row(1);
        assert_eq!((b.cat_focus(), b.selected), (false, 1));
    }

    /// Pulling the stick being browsed puts the browser back at its root and
    /// says so once (the deck then shows SOURCE); pulling one that is not
    /// being browsed does not.
    #[test]
    fn pulling_the_browsed_stick_is_reported_once() {
        let _env = MEDIA_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let home = crate::media::tests::tempfile_like::Dir::new("browser-pull-home");
        let (root, stick) = crate::media::tests::fake_media_root("browser-pull-usb");
        std::env::set_var("OPENDECK_MEDIA_DIRS", root.path());
        let mut b = Browser::new(home.path(), crate::prodj::LinkState::new(1));
        assert!(b.force_media_rescan() || !b.sources().is_empty());
        let usb = b.sources().iter().position(|r| r.kind == SourceKind::Usb && r.path.as_deref() == Some(stick.as_path())).unwrap();
        assert!(b.open_source(usb));
        assert_eq!(b.title(), "ARTIST");

        let empty = crate::media::tests::tempfile_like::Dir::new("browser-pull-gone");
        std::env::set_var("OPENDECK_MEDIA_DIRS", empty.path());
        assert!(b.force_media_rescan(), "the stick drops out");
        assert!(b.take_browsed_gone(), "the browsed stick was pulled");
        assert!(!b.take_browsed_gone(), "reported once");
        assert!(b.sources().iter().all(|r| r.kind != SourceKind::Usb), "no USB row left");

        // In again, browse home, pull it: not the browsed one.
        std::env::set_var("OPENDECK_MEDIA_DIRS", root.path());
        assert!(b.force_media_rescan());
        assert!(b.open_source(0));
        std::env::set_var("OPENDECK_MEDIA_DIRS", empty.path());
        assert!(b.force_media_rescan());
        assert!(!b.take_browsed_gone(), "home was being browsed");
    }

    /// A position saved inside a rekordbox stick's folders without going
    /// through its categories (walked from the stick's own rows) comes back
    /// under FOLDER: the category column is there, ARTIST and TRACK one tap
    /// away.  It used to come back as bare folders with no way to the
    /// categories but the `rekordbox` row.
    #[test]
    fn a_folder_position_on_a_rekordbox_stick_comes_back_under_folder() {
        let _env = MEDIA_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let home = crate::media::tests::tempfile_like::Dir::new("browser-rbfolder-home");
        let (root, stick) = crate::media::tests::fake_media_root("browser-rbfolder-usb");
        std::fs::create_dir_all(stick.join("Contents/Artist")).unwrap();
        std::fs::write(stick.join("Contents/Artist/a.mp3"), b"").unwrap();
        std::env::set_var("OPENDECK_MEDIA_DIRS", root.path());
        let mut b = Browser::new(home.path(), crate::prodj::LinkState::new(1));
        b.force_media_rescan();
        let pos = BrowsePos { root: stick.clone(), names: vec!["Contents".into(), "Artist".into()], selected: "a.mp3".into() };
        assert!(b.restore(&pos));
        assert!(b.stack.iter().any(|l| matches!(l, Loc::RbRoot)), "through the categories");
        assert_eq!(b.title(), "Artist");
        assert_eq!(b.selected_entry().map(|e| e.name.as_str()), Some("a.mp3"));
        assert!(b.rb_categories().is_some(), "the category column is there");
    }

    /// A stick inserted while browsing the home folder does not take the
    /// USB / BROWSE key over; each source keeps its own place, and a saved
    /// position (by names) puts a fresh browser back on the same row.
    #[test]
    fn a_new_stick_does_not_take_over_and_sources_keep_their_place() {
        let _env = MEDIA_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let empty = crate::media::tests::tempfile_like::Dir::new("browser-nomedia");
        std::env::set_var("OPENDECK_MEDIA_DIRS", empty.path());
        let home = crate::media::tests::tempfile_like::Dir::new("browser-keep");
        std::fs::create_dir_all(home.path().join("Crate")).unwrap();
        for f in ["a.mp3", "b.mp3"] { std::fs::write(home.path().join("Crate").join(f), b"").unwrap(); }
        let mut b = Browser::new(home.path(), crate::prodj::LinkState::new(1));
        b.selected = b.entries().iter().position(|e| e.name == "Crate").unwrap();
        b.enter();
        b.selected = b.entries().iter().position(|e| e.name == "b.mp3").unwrap();

        // A stick goes in.
        let (root, stick) = crate::media::tests::fake_media_root("browser-keep-usb");
        std::env::set_var("OPENDECK_MEDIA_DIRS", root.path());
        assert!(b.force_media_rescan());
        b.go_root();
        assert_eq!((b.title().as_str(), b.selected_entry().unwrap().name.as_str()), ("Crate", "b.mp3"),
                   "the key stays on the folder being browsed");

        // Over to the stick from SOURCE, into TRACK; back to home: Crate.
        let usb = b.sources().iter().position(|r| r.kind == SourceKind::Usb && r.path.as_deref() == Some(stick.as_path())).unwrap();
        assert!(b.open_source(usb));
        assert_eq!(b.title(), "ARTIST", "a rekordbox stick opens on ARTIST, as a CDJ");
        let cats = b.rb_categories().unwrap().0;
        b.open_rb_category(cats.iter().position(|c| c.starts_with("TRACK")).unwrap());
        b.selected = 1;
        assert_eq!(b.title(), "TRACK");
        assert!(b.open_source(0));
        assert_eq!(b.title(), "Crate");
        // And the stick again: TRACK, second row.
        assert!(b.open_source(usb));
        assert_eq!((b.title().as_str(), b.selected), ("TRACK", 1));
        // The USB key now means the stick (browsed last).
        b.open_source(0);
        b.open_source(usb);
        b.go_root();
        assert_eq!(b.title(), "TRACK");

        assert!(!b.take_browsed_gone(), "nothing was pulled yet");

        // Saved by names, restored in a fresh browser (the next launch).
        let pos = b.position().expect("a local position");
        assert_eq!(pos.root, stick);
        let mut fresh = Browser::new(home.path(), crate::prodj::LinkState::new(1));
        assert!(fresh.restore(&pos));
        assert_eq!((fresh.title().as_str(), fresh.selected_entry().unwrap().name.as_str()),
                   ("TRACK", b.selected_entry().unwrap().name.as_str()));
        // A position on a stick that is gone is not restored.
        std::fs::remove_dir_all(&stick).unwrap();
        assert!(fresh.force_media_rescan());
        assert!(!fresh.restore(&pos));
        std::env::remove_var("OPENDECK_MEDIA_DIRS");
    }

    #[test]
    fn source_screen_lists_home_and_opens_it_from_anywhere() {
        let home = crate::media::tests::tempfile_like::Dir::new("browser-sources");
        std::fs::create_dir_all(home.path().join("Crate")).unwrap();
        std::fs::write(home.path().join("Crate").join("t.mp3"), b"").unwrap();
        let mut b = Browser::new(home.path(), crate::prodj::LinkState::new(1));
        let rows = b.sources();
        assert_eq!(rows[0].kind, SourceKind::Local);
        assert_eq!(rows[0].path.as_deref(), Some(home.path()));
        // The local row holds a track under it, and nothing on LINK.
        let local = Load::Local { path: home.path().join("Crate").join("t.mp3"), analyze: None };
        assert!(rows[0].holds(&local));
        assert!(!rows[0].holds(&Load::Db { ip: Ipv4Addr::LOCALHOST, id: 1, title: String::new(), rekordbox: false, sd: false }));
        // Wander into a folder, then pick the local source again: still
        // there, on the same row; BACK climbs out onto the folder's row.
        b.selected = b.entries().iter().position(|e| e.name == "Crate").unwrap();
        b.enter();
        assert_eq!(b.title(), "Crate");
        assert!(b.open_source(0));
        assert_eq!(b.title(), "Crate", "the source opens where it was left");
        assert_eq!(b.selected_entry().unwrap().name, "t.mp3");
        b.back();
        assert_eq!(b.title(), home.path().file_name().unwrap().to_str().unwrap());
        assert_eq!(b.selected_entry().unwrap().name, "Crate", "BACK highlights the folder it left");
        assert!(!b.open_source(rows.len()), "no such row");
    }

    // Gated on a serving peer: OPENDECK_TEST_LINK=127.0.0.1 (e.g. `opendeck-serve`
    // running locally) or an XDJ / rekordbox address.  Browses LINK → the peer →
    // ALL TRACKS → first track, then reads that track's audio the way the deck
    // would: dbserver file path + NFS.
    #[test]
    fn link_source_browses_and_reads_a_track_over_dbserver_and_nfs() {
        let Ok(ip) = std::env::var("OPENDECK_TEST_LINK") else { return };
        let ip: Ipv4Addr = ip.parse().unwrap();
        let link = crate::prodj::LinkState::new(1);
        // OPENDECK_TEST_LINK_PLAYER: the peer's player number (rekordbox is 41).
        let player = std::env::var("OPENDECK_TEST_LINK_PLAYER").ok().and_then(|p| p.parse().ok()).unwrap_or(3);
        link.peers.lock().unwrap().insert(player, ip);
        let mut b = Browser::new(&std::env::temp_dir(), link);

        let li = b.entries().iter().position(|e| e.name == "LINK").expect("LINK row");
        b.selected = li;
        assert!(matches!(b.enter(), Enter::Folder));                    // LINK list
        assert_eq!(b.entries().len(), 1, "one peer listed");
        b.selected = 0;
        assert!(matches!(b.enter(), Enter::Folder));                    // connecting …
        let t0 = Instant::now();
        while b.connecting() && t0.elapsed() < std::time::Duration::from_secs(15) {
            b.prefetch_meta();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (cats, _) = b.rb_categories().expect("the player's categories");  // … its categories
        assert!(cats.iter().any(|c| c == "TRACK") && cats.iter().any(|c| c == "PLAYLIST"), "categories: {cats:?}");
        b.open_rb_category(cats.iter().position(|c| c == "TRACK").unwrap());
        assert_eq!(b.title(), "TRACK");
        assert!(b.entries().iter().all(|e| !e.is_dir) && !b.entries().is_empty(), "tracks");
        b.selected = 0;
        let (tip, id, rekordbox) = match b.enter() {
            Enter::Track(Load::Db { ip, id, rekordbox, .. }) => (ip, id, rekordbox),
            _ => panic!("expected a Db track"),
        };
        assert_eq!(tip, ip);

        // A second session as device 1 would clash with the browser's own (a
        // server drops one of two sessions from one device number): check as 2.
        let mut db = DbClient::discover(ip, 2).unwrap();
        let slot = if rekordbox { Slot::Collection } else { Slot::Usb };
        let path = db.file_path(slot, id).unwrap().expect("file path");
        let pm = if rekordbox { opendeck_nfs::PORTMAP_REKORDBOX } else { opendeck_nfs::PORTMAP_PLAYER };
        let mut nfs = Nfs::connect_at(ip, pm).unwrap();
        let export = nfs.exports().unwrap().into_iter().next().unwrap();
        let root = nfs.mount(&export).unwrap();
        let (fh, size) = nfs.lookup_path(&root, &path).unwrap();
        let bytes = nfs.read_file(&fh, size).unwrap();
        assert!(bytes.len() == size as usize && size > 0, "read {} of {size}", bytes.len());
        println!("OK: LINK → dbserver track {id} at {path} → {size} bytes over NFS");
    }
}

/// BROWSE on a linked player, end to end: OpenDeck's own dbserver (and NFS
/// export) on free loopback ports stands in for the player, and the browser
/// walks it the way a DJ does: LINK → the player → its categories → a list
/// → a track, BACK into the category column, the right-hand pane, the
/// highlighted track's metadata, and a player that isn't there.
#[cfg(test)]
mod link_browse_tests {
    use super::*;
    use opendeck_dbserver::server::{Library, Server, Track};
    use std::time::Duration;

    /// Four tracks by two artists on two albums, keys and tempos set so
    /// KEY and the related-key lists have something to show.
    fn library() -> Library {
        let t = |id: u32, title: &str, artist: &str, album: &str, key: &str, bpm: f32| Track {
            id, title: title.into(), artist: artist.into(), album: album.into(), key: key.into(), bpm,
            path: format!("/Contents/{}.mp3", title.to_lowercase().replace(' ', "_")), duration_s: 300,
            beats: vec![(1, bpm, 0), (2, bpm, (60_000.0 / bpm) as u32)], ..Default::default()
        };
        Library { name: "OPENDECK".into(), tracks: vec![
            t(1, "Night Drive", "Aurora Grid", "Signals", "Am", 124.0),
            t(2, "Low Orbit", "Aurora Grid", "Signals", "C", 126.0),
            t(3, "Static Bloom", "Kessler", "Debris", "Em", 128.0),
            t(4, "Far Field", "Kessler", "Field Notes", "F#m", 140.0),
        ], ..Default::default() }
    }

    struct Rig { _db: Server, b: Browser }

    /// A player (number 2) on loopback serving `lib`, and a browser that
    /// knows it from the Link peer table.
    fn rig(lib: Library) -> Rig {
        let db = Server::start_at(lib.shared(), Arc::new(|| 2), 0, 0).unwrap();
        let link = crate::prodj::LinkState::new(1);
        link.peers.lock().unwrap().insert(2, Ipv4Addr::LOCALHOST);
        let home = std::env::temp_dir();
        let mut b = Browser::new(&home, link);
        b.ports = crate::LinkPorts { query: db.query_port, portmap: None };
        Rig { _db: db, b }
    }

    fn names(b: &Browser) -> Vec<String> { b.entries().iter().map(|e| e.name.clone()).collect() }

    fn pick(b: &mut Browser, name: &str) -> Enter {
        b.selected = b.entries().iter().position(|e| e.name == name)
            .unwrap_or_else(|| panic!("no {name:?} in {:?}", names(b)));
        let e = b.enter();
        settle(b);
        e
    }

    /// Frames until the list on screen is in (lists from a linked player
    /// arrive a frame or two after the key, as in the app).
    fn settle(b: &mut Browser) {
        assert!(frames_until(b, |b| b.list_wait.is_none() && !b.connecting()), "the list came in: {:?}", names(b));
    }

    /// LINK, then the player's row: the browser is on the player's categories.
    fn open_player(b: &mut Browser) {
        b.go_link();
        assert_eq!(names(b).len(), 1, "one player listed: {:?}", names(b));
        assert!(names(b)[0].contains("127.0.0.1"), "{:?}", names(b));
        b.selected = 0;
        assert!(matches!(b.enter(), Enter::Folder), "the player opens");
        assert!(frames_until(b, |b| !b.connecting()), "connected");
        settle(b);
        assert!(b.rb_categories().is_some(), "on its categories: {:?}", names(b));
    }

    /// Run `prefetch_meta` as the frames do until `done` (or 3 s).
    fn frames_until(b: &mut Browser, mut done: impl FnMut(&Browser) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(3) {
            b.prefetch_meta();
            if done(b) { return true; }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_linked_player_opens_on_its_categories_and_its_first_one() {
        let mut r = rig(library());
        open_player(&mut r.b);
        let (cats, open) = r.b.rb_categories().expect("the category column");
        for c in ["ARTIST", "ALBUM", "TRACK", "KEY", "PLAYLIST", "HISTORY", "FILENAME"] {
            assert!(cats.iter().any(|n| n == c), "{c} in {cats:?}");
        }
        // It opens on the first category, ARTIST, as a unit does.
        assert_eq!(open, Some(0));
        assert_eq!(names(&r.b), ["Aurora Grid", "Kessler"]);
        assert_eq!(r.b.source_kind(), SourceTile::Link);
    }

    #[test]
    fn an_artist_lists_their_tracks_and_a_track_loads_from_that_player() {
        let mut r = rig(library());
        open_player(&mut r.b);
        assert!(matches!(pick(&mut r.b, "Kessler"), Enter::Folder));
        assert_eq!(r.b.title(), "Kessler");
        assert_eq!(names(&r.b), ["Far Field", "Static Bloom"], "by title, as the player sorts them");
        assert!(r.b.entries().iter().all(|e| !e.is_dir && e.artist.as_deref() == Some("Kessler")));
        let Enter::Track(load) = pick(&mut r.b, "Far Field") else { panic!("a track") };
        let Load::Db { ip, id, rekordbox, sd, .. } = load.clone() else { panic!("{load:?}") };
        assert_eq!((ip, id, rekordbox, sd), (Ipv4Addr::LOCALHOST, 4, false, false));
        // LOAD asks the browser's own session for the file and grid (a
        // second session from our player number would cost the browser its).
        let (caller, _meta) = r.b.db_track_caller(ip, Slot::Usb, id).expect("the open session");
        let (path, grid) = std::thread::spawn(move || (caller.call(move |c, s| c.file_path(s, id)).unwrap(), caller.call(move |c, s| c.beat_grid(s, id))))
            .join().unwrap();   // the loader thread's side
        assert_eq!(path.as_deref(), Some("/Contents/far_field.mp3"));
        assert!(grid.is_ok_and(|g| !g.is_empty()), "the player's beat grid");
        // Not the session's player or slot: no answer, so the loader opens its own.
        assert!(r.b.db_track_caller(ip, Slot::Sd, id).is_none());
    }

    #[test]
    fn album_track_and_filename_list_as_the_player_serves_them() {
        let mut r = rig(library());
        open_player(&mut r.b);
        r.b.open_rb_category(1);                                   // ALBUM
        settle(&mut r.b);
        assert_eq!(names(&r.b), ["Debris", "Field Notes", "Signals"]);
        assert!(matches!(pick(&mut r.b, "Signals"), Enter::Folder));
        assert_eq!(names(&r.b), ["Low Orbit", "Night Drive"]);
        let cats = r.b.rb_categories().unwrap().0;
        r.b.open_rb_category(cats.iter().position(|c| c == "TRACK").unwrap());
        settle(&mut r.b);
        assert_eq!(names(&r.b).len(), 4, "every track: {:?}", names(&r.b));
        r.b.open_rb_category(cats.iter().position(|c| c == "FILENAME").unwrap());
        settle(&mut r.b);
        assert_eq!(names(&r.b), ["far_field.mp3", "low_orbit.mp3", "night_drive.mp3", "static_bloom.mp3"]);
    }

    /// KEY → a key → how far from it (same, relative, …) → the tracks in
    /// those keys.  OpenDeck serving OpenDeck: both ends are ours.
    #[test]
    fn key_lists_keys_then_related_sets_then_tracks() {
        let mut r = rig(library());
        open_player(&mut r.b);
        let cats = r.b.rb_categories().unwrap().0;
        r.b.open_rb_category(cats.iter().position(|c| c == "KEY").unwrap());
        settle(&mut r.b);
        assert_eq!(names(&r.b), ["Am", "C", "Em", "F#m"]);
        assert!(matches!(pick(&mut r.b, "Am"), Enter::Folder));
        let levels = names(&r.b);
        assert_eq!(levels.len(), 3, "{levels:?}");
        assert_eq!(levels[0], "Am", "distance 0: the key itself");
        assert_eq!(levels[1], "Am, C", "distance 1: the key and its relative major");
        // Distance 0: only the track in A minor.
        r.b.selected = 0;
        assert!(matches!(r.b.enter(), Enter::Folder));
        settle(&mut r.b);
        assert_eq!(names(&r.b), ["Night Drive"]);
        // Distance 1 adds the relative major (C), as the unit's "Abm, B";
        // distance 2 the neighbours a fifth away (E minor among them).
        let wider = |b: &mut Browser, row: usize| {
            b.back();
            settle(b);
            b.selected = row;
            assert!(matches!(b.enter(), Enter::Folder));
            settle(b);
            let mut got = names(b);
            got.sort();
            got
        };
        assert_eq!(wider(&mut r.b, 1), ["Low Orbit", "Night Drive"]);
        assert_eq!(wider(&mut r.b, 2), ["Low Orbit", "Night Drive", "Static Bloom"]);
    }

    #[test]
    fn back_from_a_category_steps_into_the_column_and_the_wheel_walks_it() {
        let mut r = rig(library());
        open_player(&mut r.b);
        assert!(matches!(pick(&mut r.b, "Kessler"), Enter::Folder));
        r.b.back();                                                 // to ARTIST's artists
        settle(&mut r.b);
        assert_eq!(names(&r.b), ["Aurora Grid", "Kessler"]);
        assert_eq!(r.b.selected_entry().unwrap().name, "Kessler", "the highlight comes back to where it was");
        r.b.back();                                                 // into the column
        settle(&mut r.b);
        assert!(r.b.cat_focus());
        r.b.back();                                                 // and no further
        settle(&mut r.b);
        assert!(r.b.cat_focus());
        assert_eq!(r.b.source_kind(), SourceTile::Link, "still on the player");
        r.b.move_selection(1);                                      // the wheel opens ALBUM
        settle(&mut r.b);
        assert_eq!(r.b.rb_categories().unwrap().1, Some(1));
        assert_eq!(names(&r.b), ["Debris", "Field Notes", "Signals"]);
        assert!(matches!(r.b.enter(), Enter::Folder));              // press: back into the list
        settle(&mut r.b);
        assert!(!r.b.cat_focus());
        assert_eq!(r.b.selected, 0);
    }

    /// The right-hand pane fills in with what the highlighted row opens, and
    /// a highlighted track's library metadata arrives a few frames later —
    /// neither waits on the player.
    #[test]
    fn the_pane_and_the_highlighted_tracks_metadata_arrive_without_waiting() {
        let mut r = rig(library());
        open_player(&mut r.b);
        r.b.selected = 1;                                           // Kessler
        assert_eq!(r.b.db_peek(), Some(("TRACK", vec![])), "empty until the player answers");
        assert!(frames_until(&mut r.b, |b| b.db_peek().is_some_and(|p| !p.1.is_empty())), "the pane filled");
        assert_eq!(r.b.db_peek().unwrap().1, vec![(false, "Far Field".into()), (false, "Static Bloom".into())]);

        assert!(matches!(pick(&mut r.b, "Kessler"), Enter::Folder));
        r.b.selected = 0;                                           // Far Field
        let load = r.b.selected_entry().unwrap().load().cloned().unwrap();
        assert!(r.b.db_tags(&load).is_none(), "nothing asked yet");
        assert!(frames_until(&mut r.b, |b| b.db_tags(&load).is_some()), "metadata arrived");
        let tags = r.b.db_tags(&load).unwrap();
        assert_eq!(tags.title.as_deref(), Some("Far Field"));
        assert_eq!(tags.artist.as_deref(), Some("Kessler"));
        assert_eq!(tags.album.as_deref(), Some("Field Notes"));
        assert_eq!(tags.key.as_deref(), Some("F#m"));
        assert!(tags.bpm.is_some_and(|b| (b - 140.0).abs() < 0.01), "{:?}", tags.bpm);
        assert!(frames_until(&mut r.b, |b| !b.db_pending()), "nothing left in flight");
    }

    /// PLAY LIST with the last track from a linked player: the player's
    /// playlists, without the category column, and BACK stays there.
    #[test]
    fn play_list_reopens_the_linked_players_playlists() {
        let mut r = rig(library());
        open_player(&mut r.b);
        r.b.go_root();                                              // somewhere else
        settle(&mut r.b);
        assert!(r.b.open_playlists());
        settle(&mut r.b);
        assert!(r.b.playlist_only());
        assert_eq!(r.b.source_kind(), SourceTile::Link);
        // OpenDeck has no playlists: only ALL TRACKS would be offered, and the
        // list is the player's playlist root.
        r.b.back();
        settle(&mut r.b);
        assert!(r.b.playlist_only(), "BACK stays in the playlists");
    }

    /// A player in front of a switch we can pull: a proxy for the port
    /// query and the database port that forwards to `db` until `frozen` is
    /// set, then holds every byte (a cable pulled mid-browse, Wi-Fi gone).
    struct Freezable { query: u16, frozen: Arc<std::sync::atomic::AtomicBool> }

    fn freezable(db: &Server) -> Freezable {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::sync::atomic::{AtomicBool, Ordering};
        let frozen = Arc::new(AtomicBool::new(false));
        let dbl = TcpListener::bind("127.0.0.1:0").unwrap();
        let db_proxy = dbl.local_addr().unwrap().port();
        let ql = TcpListener::bind("127.0.0.1:0").unwrap();
        let query = ql.local_addr().unwrap().port();
        std::thread::spawn(move || for c in ql.incoming().flatten() {
            let mut c = c;
            let mut q = [0u8; 64];
            let _ = c.read(&mut q);
            let _ = c.write_all(&db_proxy.to_be_bytes());
        });
        let real = db.db_port;
        let f = Arc::clone(&frozen);
        std::thread::spawn(move || for c in dbl.incoming().flatten() {
            let Ok(up) = TcpStream::connect(("127.0.0.1", real)) else { continue };
            for (mut from, mut to) in [(c.try_clone().unwrap(), up.try_clone().unwrap()), (up, c)] {
                let f = Arc::clone(&f);
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = from.read(&mut buf) {
                        if n == 0 { break; }
                        while f.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(20)); }
                        if to.write_all(&buf[..n]).is_err() { break; }
                    }
                });
            }
        });
        Freezable { query, frozen }
    }

    /// The player stops answering mid-browse: opening a list returns at once
    /// (the deck keeps drawing and playing), the list says it is loading,
    /// then that the player is not answering; BACK still works.
    #[test]
    fn a_player_that_stops_answering_never_blocks_the_deck() {
        let quick = |what: &str, t: Instant| assert!(t.elapsed() < Duration::from_millis(100), "{what} returned at once: {:?}", t.elapsed());
        let mut r = rig(library());
        let proxy = freezable(&r._db);
        r.b.ports = crate::LinkPorts { query: proxy.query, portmap: None };
        open_player(&mut r.b);
        assert_eq!(names(&r.b), ["Aurora Grid", "Kessler"]);
        let load = r.b.entries().iter().position(|e| e.name == "Kessler").unwrap();
        // A track to LOAD later, listed while the player still answered.
        r.b.selected = load;
        r.b.enter();
        settle(&mut r.b);
        let far_field = r.b.entries()[0].load().cloned().expect("a track row");
        r.b.back();
        settle(&mut r.b);

        // The cable is pulled.
        proxy.frozen.store(true, std::sync::atomic::Ordering::SeqCst);

        // Opening a list not in yet: at once, a loading row, the app drawing.
        let t = Instant::now();
        r.b.selected = r.b.entries().iter().position(|e| e.name == "Aurora Grid").unwrap();
        r.b.enter();
        quick("opening a list", t);
        assert_eq!(names(&r.b), ["Loading…"]);
        assert!(r.b.db_pending(), "the app keeps drawing while the list loads");

        // BACK: the list it came from, from this session's cache, at once.
        let t = Instant::now();
        r.b.back();
        quick("BACK", t);
        assert_eq!(names(&r.b), ["Aurora Grid", "Kessler"]);
        assert_eq!(r.b.selected_entry().unwrap().name, "Aurora Grid", "the highlight where it was");

        // The wheel through the category column onto a category not in yet.
        r.b.back();
        assert!(r.b.cat_focus());
        let t = Instant::now();
        r.b.move_selection(1);
        quick("the category wheel", t);
        assert_eq!(names(&r.b), ["Loading…"]);

        // SEARCH on the player.
        let t = Instant::now();
        r.b.begin_search();
        r.b.set_search_query("orbit".into());
        quick("SEARCH", t);

        // LOAD: the browser only hands over its session; the loader asks.
        let crate::browser::Load::Db { ip, id, .. } = far_field else { panic!("{far_field:?}") };
        let t = Instant::now();
        assert!(r.b.db_track_caller(ip, Slot::Usb, id).is_some());
        quick("LOAD's hand-over", t);

        // Meanwhile the frames keep running, and once the request gives up
        // the list says the player is not answering.
        let t = Instant::now();
        let mut slowest = Duration::ZERO;
        while r.b.list_wait.is_some() && t.elapsed() < Duration::from_secs(40) {
            let f = Instant::now();
            r.b.prefetch_meta();
            slowest = slowest.max(f.elapsed());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(slowest < Duration::from_millis(50), "no frame waited on the player: slowest {slowest:?}");
        assert!(r.b.list_wait.is_none(), "the request gave up");
        assert!(names(&r.b).iter().any(|n| n.contains("not answering")), "{:?}", names(&r.b));
    }

    /// A player that has gone away: the press returns at once (the deck
    /// keeps drawing and playing), the row says it is connecting, then why
    /// it did not answer; BACK returns to the LINK list at any point.
    #[test]
    fn a_player_that_is_not_there_never_blocks_the_deck() {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let link = crate::prodj::LinkState::new(1);
        link.peers.lock().unwrap().insert(3, Ipv4Addr::LOCALHOST);
        let mut b = Browser::new(&std::env::temp_dir(), link);
        // Nothing on the port query, nor on the portmapper for the
        // export.pdb fallback.
        b.ports = crate::LinkPorts { query: free, portmap: Some(free) };
        b.go_link();
        let before = names(&b);
        let t0 = Instant::now();
        b.selected = 0;
        assert!(matches!(b.enter(), Enter::Folder));
        assert!(t0.elapsed() < Duration::from_millis(100), "the press returned at once: {:?}", t0.elapsed());
        assert!(b.connecting() && b.db_pending(), "connecting, and the app keeps drawing");
        assert!(names(&b)[0].starts_with("Connecting to "), "{:?}", names(&b));
        // The answer: not answering, shown where the key was pressed.
        let t1 = Instant::now();
        while b.connecting() && t1.elapsed() < Duration::from_secs(30) {
            b.prefetch_meta();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!b.connecting(), "gave up");
        assert!(names(&b)[0].contains("not answering"), "{:?}", names(&b));
        b.back();
        assert_eq!(names(&b), before, "BACK: the LINK list again");

        // BACK while it is still connecting: out at once, the late answer dropped.
        b.selected = 0;
        b.enter();
        b.back();
        assert_eq!(names(&b), before);
        b.prefetch_meta();
        assert!(!b.connecting());
        assert_eq!(names(&b), before);
    }
}
