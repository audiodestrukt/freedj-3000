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
use crate::prodj::LinkState;
use opendeck_nfs::Nfs;
use opendeck_rekordbox::{read_export, read_export_from, RbExport};

const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "aiff", "aif", "ogg", "opus"];

fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.iter().any(|a| a.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// Where a track to load lives.
#[derive(Clone)]
pub enum Load {
    /// A local file (with an optional local ANLZ analysis file).
    Local { path: PathBuf, analyze: Option<PathBuf> },
    /// A track on a linked player, read over NFS.  Paths are rekordbox-relative.
    Link  { ip: Ipv4Addr, rel_path: String, analyze_rel: String },
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
    kind:       EntryKind,
}

#[derive(Clone)]
enum EntryKind {
    Descend(Loc),
    /// Connect to a linked player, then browse its rekordbox library.
    ConnectLink(Ipv4Addr),
    Track(Load),
}

#[derive(Clone)]
enum Loc {
    Fs(PathBuf),
    /// The LINK source: a list of discovered players.
    Link,
    /// A folder in the current rekordbox library's playlist tree (0 = root).
    RbTree(u32),
    /// The tracks of a rekordbox playlist.
    RbPlaylist(u32),
}

/// Which media the currently-loaded rekordbox export came from — decides how a
/// selected track loads.
#[derive(Clone)]
enum RbSource {
    Local(PathBuf),   // paths resolve against this mount
    Link(Ipv4Addr),   // paths are NFS paths on this player
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
    link:      Arc<LinkState>,
}

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
            stack: vec![Loc::Fs(start)],
            entries: Vec::new(),
            selected: 0,
            rb: None,
            rb_source: None,
            link,
        };
        b.rebuild();
        b
    }

    pub fn entries(&self) -> &[Entry] { &self.entries }
    pub fn selected_entry(&self) -> Option<&Entry> { self.entries.get(self.selected) }

    pub fn title(&self) -> String {
        match self.stack.last() {
            Some(Loc::Fs(p)) => p.file_name().and_then(|n| n.to_str()).unwrap_or("/").to_string(),
            Some(Loc::Link)  => "LINK".to_string(),
            Some(Loc::RbTree(0)) => match &self.rb_source {
                Some(RbSource::Link(ip)) => format!("LINK  {ip}"),
                _ => "REKORDBOX".to_string(),
            },
            Some(Loc::RbTree(id)) | Some(Loc::RbPlaylist(id)) => self.rb.as_ref()
                .and_then(|e| e.playlists.iter().find(|n| n.id == *id))
                .map(|n| n.name.clone()).unwrap_or_else(|| "rekordbox".to_string()),
            None => "/".to_string(),
        }
    }

    pub fn refresh(&mut self) { self.rebuild(); }

    fn rebuild(&mut self) {
        let entries = match self.stack.last().cloned() {
            Some(Loc::Fs(dir))        => self.fs_entries(&dir),
            Some(Loc::Link)           => self.link_entries(),
            Some(Loc::RbTree(node))   => self.rb_tree_entries(node),
            Some(Loc::RbPlaylist(id)) => self.rb_playlist_entries(id),
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
            out.push(Entry { name: "LINK".into(), is_dir: true, artist: None, bpm: None,
                kind: EntryKind::Descend(Loc::Link) });
            if dir.join("PIONEER/rekordbox/export.pdb").is_file() {
                let need = !matches!(&self.rb_source, Some(RbSource::Local(r)) if r == dir);
                if self.rb.is_none() || need {
                    match read_export(dir) {
                        Ok(exp) => { self.rb = Some(Arc::new(exp)); self.rb_source = Some(RbSource::Local(dir.to_path_buf())); }
                        Err(e)  => log::warn!("rekordbox export at {}: {e:#}", dir.display()),
                    }
                }
                if matches!(&self.rb_source, Some(RbSource::Local(r)) if r == dir) {
                    out.push(Entry { name: "rekordbox".into(), is_dir: true, artist: None, bpm: None,
                        kind: EntryKind::Descend(Loc::RbTree(0)) });
                }
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
                    dirs.push(Entry { name, is_dir: true, artist: None, bpm: None,
                        kind: EntryKind::Descend(Loc::Fs(path)) });
                } else if is_audio(&path) {
                    files.push(Entry { name, is_dir: false, artist: None, bpm: None,
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

    /// The LINK source: discovered players from the ProDJ Link peer table.
    fn link_entries(&self) -> Vec<Entry> {
        let peers = self.link.peers.lock().map(|p| p.clone()).unwrap_or_default();
        let mut v: Vec<(u8, Ipv4Addr)> = peers.into_iter().collect();
        v.sort_by_key(|(p, _)| *p);
        v.into_iter().map(|(player, ip)| Entry {
            name: format!("Player {player}   {ip}"),
            is_dir: true, artist: None, bpm: None,
            kind: EntryKind::ConnectLink(ip),
        }).collect()
    }

    fn rb_tree_entries(&self, node: u32) -> Vec<Entry> {
        let Some(exp) = self.rb.as_ref() else { return Vec::new() };
        exp.children(node).into_iter().map(|n| Entry {
            name: n.name.clone(), is_dir: true, artist: None, bpm: None,
            kind: EntryKind::Descend(if n.is_folder { Loc::RbTree(n.id) } else { Loc::RbPlaylist(n.id) }),
        }).collect()
    }

    fn rb_playlist_entries(&self, id: u32) -> Vec<Entry> {
        let Some(exp) = self.rb.as_ref() else { return Vec::new() };
        let src = self.rb_source.clone();
        exp.playlist_tracks(id).into_iter().filter_map(|t| {
            let load = match &src {
                Some(RbSource::Local(root)) => Load::Local { path: t.path_on(root), analyze: t.analyze_on(root) },
                Some(RbSource::Link(ip))    => Load::Link { ip: *ip, rel_path: t.rel_path.clone(), analyze_rel: t.analyze_rel.clone() },
                None => return None,
            };
            Some(Entry {
                name: t.title.clone(), is_dir: false,
                artist: Some(t.artist.clone()), bpm: Some(t.bpm),
                kind: EntryKind::Track(load),
            })
        }).collect()
    }

    /// Connect to a linked player, read its `export.pdb` over NFS, and browse it.
    fn connect_link(&mut self, ip: Ipv4Addr) -> anyhow::Result<()> {
        let mut nfs = Nfs::connect(ip)?;
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
        match sel.kind.clone() {
            EntryKind::Descend(loc) => {
                self.stack.push(loc);
                self.selected = 0;
                self.rebuild();
                Enter::Folder
            }
            EntryKind::ConnectLink(ip) => match self.connect_link(ip) {
                Ok(()) => {
                    self.stack.push(Loc::RbTree(0));
                    self.selected = 0;
                    self.rebuild();
                    Enter::Folder
                }
                Err(e) => { log::warn!("LINK connect {ip} failed: {e:#}"); Enter::Nothing }
            },
            EntryKind::Track(load) => Enter::Track(load),
        }
    }

    pub fn back(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.rebuild();
            self.selected = 0;
        } else if let Some(Loc::Fs(dir)) = self.stack.last().cloned() {
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
    // Gated on a linked player: OPENDECK_TEST_NFS=192.168.68.58
    #[test]
    fn link_source_browses_to_a_loadable_track() {
        let Ok(ip) = std::env::var("OPENDECK_TEST_NFS") else { return };
        let ip: Ipv4Addr = ip.parse().unwrap();
        let link = crate::prodj::LinkState::new(1);
        link.peers.lock().unwrap().insert(2, ip);   // pretend player 2 is the XDJ
        let mut b = Browser::new(&std::env::temp_dir(), link);

        // LINK row at top → the player → its rekordbox tree → a playlist → tracks
        let li = b.entries().iter().position(|e| e.name == "LINK").expect("LINK row");
        b.selected = li;
        assert!(matches!(b.enter(), Enter::Folder));            // LINK list
        assert!(!b.entries().is_empty(), "a player is listed");
        b.selected = 0;
        assert!(matches!(b.enter(), Enter::Folder));            // connect + tree root
        assert!(!b.entries().is_empty(), "playlist tree");
        b.selected = 0;
        assert!(matches!(b.enter(), Enter::Folder));            // into a playlist
        assert!(b.entries().iter().all(|e| !e.is_dir), "tracks");
        b.selected = 0;
        match b.enter() {
            Enter::Track(Load::Link { ip: tip, rel_path, .. }) => {
                assert_eq!(tip, ip);
                assert!(rel_path.contains("/Contents/"), "nfs rel path: {rel_path}");
                println!("OK: LINK browse → loadable track {rel_path}");
            }
            _ => panic!("expected a Link track"),
        }
    }
}
