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
use opendeck_nfs::Nfs;
use opendeck_rekordbox::{read_export, read_export_from, RbExport};
use crate::media::{self, Access, Mount};
use std::time::{Duration, Instant};

// What the decoder actually plays (symphonia: no Opus decoder exists).
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "aiff", "aif", "ogg"];

/// Folders no one browses for music: `_CodeSignature` and its kin inside the
/// iOS app bundle (the browse root there), `__MACOSX` resource forks, and the
/// bundle's localisation / framework folders.
fn is_housekeeping_dir(name: &str) -> bool {
    // A stick's system and library folders: a player's FOLDER view never
    // lists them (the rekordbox export, OverCue's stems, Windows / Android
    // housekeeping).
    const SYSTEM: [&str; 5] = ["PIONEER", "CDJMODS", "System Volume Information", "$RECYCLE.BIN", "LOST.DIR"];
    SYSTEM.iter().any(|s| name.eq_ignore_ascii_case(s))
        || name.starts_with('_')
        || name.ends_with(".lproj")
        || (cfg!(target_os = "ios") && matches!(name, "Frameworks" | "PlugIns" | "SC_Info"))
}

fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.iter().any(|a| a.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// A linked track's file path (if the server has one) and its beat-grid blob.
pub type DbTrackFile = (Option<String>, anyhow::Result<Vec<u8>>);

/// Which category a dbserver root-menu row is: by its label when it is one
/// we know (a source may number the item types its own way), else by type.
fn db_category(i: &opendeck_dbserver::MenuItem) -> &'static str {
    match i.label.to_ascii_uppercase().as_str() {
        "TRACK" => "root-track",
        "PLAYLIST" => "root-playlist",
        "ARTIST" => "root-artist",
        "ALBUM" => "root-album",
        "FILENAME" | "FILE NAME" => "root-filename",
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

#[derive(Clone)]
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
    /// Open dbserver session to a peer: its address, the client, and the slot
    /// we browse (USB on a player, Collection on rekordbox).
    db:        Option<(Ipv4Addr, DbClient, Slot)>,
    /// Removable media seen at the last scan (USB rows at the top level).
    media:     Vec<Mount>,
    media_at:  Option<Instant>,
    /// Name of the dbserver folder/playlist/artist/album being shown (set on
    /// descend), and the source's own label ("3 USB: OPENDECK") for the root.
    db_title:  String,
    db_label:  String,
    link:      Arc<LinkState>,
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
            db_title: String::new(),
            db_label: String::new(),
            link,
            preview: Default::default(),
            home: start.clone(),
            played: Vec::new(),
            search_query: String::new(),
            fs_index: Default::default(),
            names: Vec::new(),
            positions: Default::default(),
            last_root: None,
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
    pub fn selected_entry(&self) -> Option<&Entry> { self.entries.get(self.selected) }

    pub fn title(&self) -> String {
        match self.stack.last() {
            Some(Loc::Fs(p)) => p.file_name().and_then(|n| n.to_str()).unwrap_or("/").to_string(),
            Some(Loc::Link)  => "LINK".to_string(),
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
            Some(Loc::DbFolder(_)) | Some(Loc::DbPlaylist(_)) | Some(Loc::DbArtist(_)) | Some(Loc::DbAlbum(_)) => self.db_title.clone(),
            None => "/".to_string(),
        }
    }

    pub fn refresh(&mut self) { self.rebuild(); }

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
        let entries = match self.stack.last().cloned() {
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
            Some(Loc::DbRoot)         => self.db_root_entries(),
            Some(Loc::DbFolder(id))   => self.db_folder_entries(id),
            Some(Loc::DbPlaylist(id)) => self.db_track_entries(|c, s| c.playlist(s, id, false)),
            Some(Loc::DbAllTracks)    => self.db_track_entries(|c, s| c.all_tracks(s, 0)),
            Some(Loc::DbArtists)      => self.db_name_entries(|c, s| c.artists(s), Loc::DbArtist),
            Some(Loc::DbArtist(id))   => self.db_track_entries(|c, s| c.tracks_for_artist(s, id)),
            Some(Loc::DbAlbums)       => self.db_name_entries(|c, s| c.albums(s), Loc::DbAlbum),
            Some(Loc::DbAlbum(id))    => self.db_track_entries(|c, s| c.tracks_for_album(s, id)),
            Some(Loc::DbFilenames)    => self.db_track_entries(|c, s| c.filenames(s)),
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

    /// A dbserver source's category menu, as the source lists it.  Each row
    /// is typed (`root-track`, `root-playlist`, …); the ones we can walk
    /// descend, the rest are shown but inert.  A source without a root menu
    /// (older servers) falls back to PLAYLIST view with ALL TRACKS on top.
    fn db_root_entries(&mut self) -> Vec<Entry> {
        let items = match self.db_call(|c, slot| c.root_menu(slot)) {
            None => return Vec::new(),
            Some(Ok(items)) if !items.is_empty() => items,
            Some(Ok(_)) => { log::info!("LINK: empty category menu; showing playlists"); return self.db_folder_entries(0); }
            Some(Err(e)) => { log::info!("LINK: no category menu ({e:#}); showing playlists"); return self.db_folder_entries(0); }
        };
        items.into_iter().map(|i| {
            let kind = match db_category(&i) {
                "root-track"    => EntryKind::Descend(Loc::DbAllTracks),
                "root-playlist" => EntryKind::Descend(Loc::DbFolder(0)),
                "root-artist"   => EntryKind::Descend(Loc::DbArtists),
                "root-album"    => EntryKind::Descend(Loc::DbAlbums),
                "root-filename" => EntryKind::Descend(Loc::DbFilenames),
                _ => EntryKind::Unsupported,
            };
            Entry { name: i.label, is_dir: true, artist: None, bpm: None, key: None, art: None, kind }
        }).collect()
    }

    /// A list of names (artists, albums) that each open as a track list.
    fn db_name_entries(&mut self, fetch: impl FnMut(&mut DbClient, Slot) -> anyhow::Result<Vec<MenuItem>>, loc: fn(u32) -> Loc) -> Vec<Entry> {
        let Some(r) = self.db_call(fetch) else { return Vec::new() };
        match r {
            Ok(items) => items.into_iter().map(|i| Entry {
                name: i.label, is_dir: true, artist: None, bpm: None, key: None, art: None, kind: EntryKind::Descend(loc(i.id)),
            }).collect(),
            Err(e) => { log::warn!("LINK names: {e:#}"); Vec::new() }
        }
    }

    /// A playlist folder of a dbserver library (0 = the tree's root, which for
    /// a source without a category menu also lists ALL TRACKS on top).
    fn db_folder_entries(&mut self, folder: u32) -> Vec<Entry> {
        if self.db.is_none() { return Vec::new(); }
        let mut out = Vec::new();
        if folder == 0 && !matches!(self.stack.last(), Some(Loc::DbFolder(0))) {
            out.push(Entry { name: "ALL TRACKS".into(), is_dir: true, artist: None, bpm: None, key: None, art: None,
                             kind: EntryKind::Descend(Loc::DbAllTracks) });
        }
        match self.db_call(|c, slot| c.playlist(slot, folder, true)).unwrap_or_else(|| Ok(Vec::new())) {
            Ok(items) => for i in items {
                let is_folder = i.type_name() == "folder";
                out.push(Entry { name: i.label.clone(), is_dir: true, artist: None, bpm: None, key: None, art: None,
                    kind: EntryKind::Descend(if is_folder { Loc::DbFolder(i.id) } else { Loc::DbPlaylist(i.id) }) });
            },
            Err(e) => log::warn!("rekordbox folder {folder}: {e:#}"),
        }
        out
    }

    /// A track list: a playlist, a category's tracks, or the whole collection.
    fn db_track_entries(&mut self, fetch: impl FnMut(&mut DbClient, Slot) -> anyhow::Result<Vec<MenuItem>>) -> Vec<Entry> {
        let Some(r) = self.db_call(fetch) else { return Vec::new() };
        let Some((ip, _, slot)) = self.db.as_ref() else { return Vec::new() };
        let (ip, slot) = (*ip, *slot);
        match r {
            Ok(items) => items.into_iter().filter(|i| i.type_name() == "track").map(|i| Entry {
                name: i.label.clone(), is_dir: false,
                artist: (!i.label2.is_empty()).then(|| i.label2.clone()), bpm: None, key: None, art: None,
                kind: EntryKind::Track(Load::Db { ip, id: i.id, title: i.label, rekordbox: slot == Slot::Collection, sd: slot == Slot::Sd }),
            }).collect(),
            Err(e) => { log::warn!("LINK tracks: {e:#}"); Vec::new() }
        }
    }

    /// Run a request on the open dbserver session. A player drops a session
    /// when another opens from the same player number (seen on a CDJ-3000:
    /// the first then reads EOF), so on an error reconnect once and retry.
    /// `None` when no session is open.
    fn db_call<T>(&mut self, mut f: impl FnMut(&mut DbClient, Slot) -> anyhow::Result<T>) -> Option<anyhow::Result<T>> {
        let (ip, c, slot) = self.db.as_mut()?;
        let (ip, slot) = (*ip, *slot);
        match f(c, slot) {
            Ok(v) => Some(Ok(v)),
            Err(e) => {
                log::info!("LINK {ip}: request failed ({e:#}); reconnecting");
                self.db = None;
                if let Err(e) = self.connect_db(ip, slot) { return Some(Err(e)); }
                let (_, c, slot) = self.db.as_mut()?;
                Some(f(c, *slot))
            }
        }
    }

    /// For a LOAD from a linked player: the track's file path and beat-grid
    /// blob, asked on the browser's own session when it is open to that
    /// player and slot. Opening a second session would cost the browser its
    /// own (see `db_call`). `None` when no such session is open.
    pub fn db_track_file(&mut self, ip: Ipv4Addr, slot: Slot, id: u32) -> Option<anyhow::Result<DbTrackFile>> {
        if !matches!(&self.db, Some((cur, _, s)) if *cur == ip && *s == slot) { return None; }
        let path = match self.db_call(|c, s| c.file_path(s, id))? {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        let grid = self.db_call(|c, s| c.beat_grid(s, id)).unwrap_or_else(|| Err(anyhow::anyhow!("session closed")));
        Some(Ok((path, grid)))
    }

    /// Open a dbserver session to a peer (a player's USB slot, or rekordbox's
    /// collection).
    fn connect_db(&mut self, ip: Ipv4Addr, slot: Slot) -> anyhow::Result<()> {
        if matches!(&self.db, Some((cur, _, s)) if *cur == ip && *s == slot) { return Ok(()); }
        // Servers answer requests from a real player number (1–4).
        let device = if (1..=4).contains(&self.link.player()) { self.link.player() } else { 1 };
        let c = DbClient::discover(ip, device)?;
        log::info!("LINK {ip}: dbserver session open ({slot:?})");
        self.db = Some((ip, c, slot));
        Ok(())
    }

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
    fn search_entries(&self) -> Vec<Entry> {
        let q = self.search_query.to_lowercase();
        if q.is_empty() { return Vec::new(); }
        if self.stack.iter().any(|l| matches!(l, Loc::RbRoot)) {
            return self.rb_track_entries(|e| e.tracks.iter()
                .filter(|t| t.title.to_lowercase().contains(&q) || t.artist.to_lowercase().contains(&q))
                .collect());
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
        let Some(r) = self.stack.iter().position(|l| matches!(l, Loc::RbRoot)) else { return };
        self.stack.truncate(r + 1);
        self.rebuild();
        if i < self.entries.len() {
            self.selected = i;
            self.enter();
        }
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

    /// Connect to a linked player, read its `export.pdb` over NFS, and browse it.
    fn connect_link(&mut self, ip: Ipv4Addr) -> anyhow::Result<()> {
        let mut nfs = Nfs::connect_any(ip)?;
        let root = nfs.mount_usb()?;
        let (fh, size) = nfs.lookup_path(&root, "PIONEER/rekordbox/export.pdb")?;
        let bytes = nfs.read_file(&fh, size)?;
        let exp = read_export_from(&mut std::io::Cursor::new(bytes), PathBuf::from(format!("link://{ip}")))?;
        log::info!("LINK {ip}: {} tracks, {} playlists", exp.tracks.len(), exp.playlists.len());
        self.rb = Some(Arc::new(exp));
        self.rb_source = Some(RbSource::Link(ip));
        Ok(())
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() { return; }
        let last = self.entries.len() as i32 - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub fn enter(&mut self) -> Enter {
        let Some(sel) = self.entries.get(self.selected) else { return Enter::Nothing };
        let (sel_name, kind) = (sel.name.clone(), sel.kind.clone());
        match kind {
            EntryKind::Unsupported => { log::info!("BROWSE: {:?} is not browsable here yet", sel_name); Enter::Nothing }
            EntryKind::Grant => { media::request_access(); Enter::Nothing }
            EntryKind::Descend(loc) => {
                if matches!(loc, Loc::DbFolder(_) | Loc::DbPlaylist(_) | Loc::DbArtist(_) | Loc::DbAlbum(_)) { self.db_title = sel_name.clone(); }
                let from_usb_row = sel_name.starts_with("USB: ");
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
            EntryKind::ConnectDb(ip) => match self.connect_db(ip, Slot::Collection) {
                Ok(()) => {
                    self.db_label = sel_name.clone();
                    self.stack.push(Loc::DbRoot);
                    self.selected = 0;
                    self.rebuild();
                    Enter::Folder
                }
                Err(e) => { log::warn!("LINK rekordbox {ip} failed: {e:#}"); Enter::Nothing }
            },
            // A player: browse the slot over dbserver like a CDJ does; if it
            // has no database service, fall back to reading export.pdb over NFS.
            EntryKind::ConnectLink(ip, slot) => match self.connect_db(ip, slot) {
                Ok(()) => {
                    self.db_label = sel_name.clone();
                    self.stack.push(Loc::DbRoot);
                    self.selected = 0;
                    self.rebuild();
                    Enter::Folder
                }
                Err(e) => {
                    log::info!("LINK {ip}: no dbserver ({e:#}); trying export.pdb over NFS");
                    match self.connect_link(ip) {
                        // Its export opens on the categories, as a USB
                        // stick does (it went straight to PLAYLIST before).
                        Ok(()) => {
                            self.stack.push(Loc::RbRoot);
                            self.selected = 0;
                            self.rebuild();
                            Enter::Folder
                        }
                        Err(e) => { log::warn!("LINK connect {ip} failed: {e:#}"); Enter::Nothing }
                    }
                }
            },
            EntryKind::Track(load) => Enter::Track(load),
        }
    }

    /// The SOURCE screen's rows: the local library, each mounted stick, then
    /// each source on PRO DJ LINK (the same rows the LINK list shows).
    pub fn sources(&self) -> Vec<SourceRow> {
        let name = self.home.file_name().and_then(|n| n.to_str()).unwrap_or("/").to_string();
        let mut out = vec![SourceRow {
            kind: SourceKind::Local, label: name, player: None,
            path: Some(self.home.clone()), ip: None, rekordbox: media::has_rekordbox(&self.home),
        }];
        // A row with no path asks the OS for access when opened.
        for m in &self.media {
            let (label, path) = match m.access {
                Access::Ready      => (format!("USB: {}", m.label), Some(m.path.clone())),
                Access::NeedsGrant => (format!("USB: {} (ALLOW ACCESS)", m.label), None),
            };
            out.push(SourceRow { kind: SourceKind::Usb, label, player: None, path, ip: None, rekordbox: m.rekordbox });
        }
        if let Some(add) = media::ADD_DRIVE {
            out.push(SourceRow { kind: SourceKind::Usb, label: format!("USB: {add}"), player: None, path: None, ip: None, rekordbox: false });
        }
        for e in self.link_entries() {
            let (ip, rekordbox) = match e.kind {
                EntryKind::ConnectLink(ip, _) => (Some(ip), false),
                EntryKind::ConnectDb(ip) => (Some(ip), true),
                _ => (None, false),
            };
            // "3 USB: NAME" → badge 3, "USB: NAME".
            let (player, label) = match e.name.split_once(' ') {
                Some((n, rest)) if !rekordbox => match n.parse::<u8>() {
                    Ok(p) => (Some(p), rest.to_string()),
                    Err(_) => (None, e.name.clone()),
                },
                _ => (None, e.name.clone()),
            };
            out.push(SourceRow { kind: SourceKind::Link, label, player, path: None, ip, rekordbox });
        }
        out
    }

    /// Open the `i`th SOURCE row in the browser (what picking it on the
    /// CDJ's SOURCE screen does). Returns false when there is no such row or
    /// a LINK source would not connect.
    pub fn open_source(&mut self, i: usize) -> bool {
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

    /// The LINK source key: jump to the list of linked players (from wherever
    /// the browser is).
    pub fn go_link(&mut self) {
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
        if matches!(self.stack.last(), Some(Loc::Fs(_))) { self.rebuild(); }
        true
    }

    #[cfg(test)]
    fn force_media_rescan(&mut self) -> bool { self.media_at = None; self.poll_media() }

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
        for name in &pos.names {
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
        if self.stack.len() > 1 {
            self.stack.pop();
            let came_from = self.names.pop().unwrap_or_default();
            self.selected = 0;
            self.rebuild();
            self.select_named(&came_from);
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
mod tests {
    use super::*;

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
        b.back();
        assert_eq!(b.title(), "REKORDBOX");
        // The demo export has no playlists, but PLAYLIST is listed anyway (a
        // CDJ-3000 always shows it); TRACK lists both tracks.
        assert!(b.entries().iter().any(|e| e.name == "PLAYLIST"));
        let tr = b.entries().iter().position(|e| e.name.starts_with("TRACK")).expect("TRACK category");
        b.selected = tr;
        assert!(matches!(b.enter(), Enter::Folder));
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
    static MEDIA_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        b.back();
        b.selected = b.entries().iter().position(|e| e.name.starts_with("TRACK")).unwrap();
        assert!(matches!(b.enter(), Enter::Folder));
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
        assert!(matches!(b.enter(), Enter::Folder));                    // category menu
        let cats: Vec<&str> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert!(cats.contains(&"TRACK") && cats.contains(&"PLAYLIST"), "categories: {cats:?}");
        let all = b.entries().iter().position(|e| e.name == "TRACK").expect("TRACK category");
        b.selected = all;
        assert!(matches!(b.enter(), Enter::Folder));
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
