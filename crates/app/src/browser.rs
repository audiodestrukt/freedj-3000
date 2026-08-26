//! Source-based track browser — the BROWSE screen's model.
//!
//! Navigates a tree that can be either the **filesystem** (a plain USB / folder,
//! as a CDJ shows a stick without a rekordbox export) or a **rekordbox library**
//! (playlists → tracks, read from `PIONEER/rekordbox/export.pdb`), the way the
//! XDJ browses a prepared USB.  When a browsed directory contains a rekordbox
//! export, a "rekordbox" entry appears at the top that descends into its playlist
//! tree; the raw folders/files remain browsable too.
//!
//! LINK (a linked player's media over the network) will slot in as another
//! location kind behind the same interface.
//!
//! The model owns navigation only — loading the highlighted track is the deck's
//! job (`DeckApp::load_track`); `enter()` reports which path to load.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use opendeck_rekordbox::{read_export, RbExport};

/// Audio extensions we offer as loadable rows in the filesystem view.
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "aiff", "aif", "ogg", "opus"];

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.iter().any(|a| a.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// One visible row.
#[derive(Clone)]
pub struct Entry {
    pub name:   String,
    pub is_dir: bool,               // folder / playlist (descend) vs track (load)
    pub artist: Option<String>,     // rekordbox second column
    pub bpm:    Option<f32>,
    kind:       EntryKind,
}

#[derive(Clone)]
enum EntryKind {
    Descend(Loc),
    Track(PathBuf),
    Nothing,
}

/// Where we can be browsing.
#[derive(Clone)]
enum Loc {
    /// A filesystem directory.
    Fs(PathBuf),
    /// A folder in the rekordbox playlist tree (0 = the tree root).
    RbTree(u32),
    /// The tracks of a rekordbox playlist.
    RbPlaylist(u32),
}

/// What `enter()` decided the highlighted row is.
pub enum Enter {
    Folder,
    Track(PathBuf),
    Nothing,
}

pub struct Browser {
    /// Navigation stack; the last element is the current location.
    stack:    Vec<Loc>,
    entries:  Vec<Entry>,
    pub selected: usize,
    /// The rekordbox export for the current media, loaded lazily when a rekordbox
    /// USB is browsed.  `root` is the media mount the paths resolve against.
    rb:       Option<Arc<RbExport>>,
    rb_root:  PathBuf,
}

impl Browser {
    /// Start in `dir` if it is a directory, otherwise in its parent.
    pub fn new(dir: &Path) -> Self {
        let start = if dir.is_dir() {
            dir.to_path_buf()
        } else {
            match dir.parent() {
                Some(par) if !par.as_os_str().is_empty() => par.to_path_buf(),
                _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            }
        };
        let mut b = Browser {
            stack:    vec![Loc::Fs(start)],
            entries:  Vec::new(),
            selected: 0,
            rb:       None,
            rb_root:  PathBuf::new(),
        };
        b.rebuild();
        b
    }

    pub fn entries(&self) -> &[Entry] { &self.entries }
    pub fn selected_entry(&self) -> Option<&Entry> { self.entries.get(self.selected) }

    /// Display label for the current location (the browse header).
    pub fn title(&self) -> String {
        match self.stack.last() {
            Some(Loc::Fs(p)) => p.file_name()
                .and_then(|n| n.to_str()).unwrap_or("/").to_string(),
            Some(Loc::RbTree(0)) => "REKORDBOX".to_string(),
            Some(Loc::RbTree(id)) | Some(Loc::RbPlaylist(id)) => self.rb.as_ref()
                .and_then(|e| e.playlists.iter().find(|n| n.id == *id))
                .map(|n| n.name.clone())
                .unwrap_or_else(|| "rekordbox".to_string()),
            None => "/".to_string(),
        }
    }

    /// Re-read the current location into `entries`, clamping the selection.
    pub fn refresh(&mut self) { self.rebuild(); }

    fn rebuild(&mut self) {
        let entries = match self.stack.last().cloned() {
            Some(Loc::Fs(dir))       => self.fs_entries(&dir),
            Some(Loc::RbTree(node))  => self.rb_tree_entries(node),
            Some(Loc::RbPlaylist(id))=> self.rb_playlist_entries(id),
            None                     => Vec::new(),
        };
        self.entries = entries;
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    /// Filesystem directory: subfolders then audio files, alphabetical.  If the
    /// directory holds a rekordbox export, load it and prepend a "rekordbox" row.
    fn fs_entries(&mut self, dir: &Path) -> Vec<Entry> {
        let mut out: Vec<Entry> = Vec::new();

        // rekordbox present here → offer the library, and cache the export.
        if dir.join("PIONEER/rekordbox/export.pdb").is_file() {
            if self.rb.is_none() || self.rb_root != dir {
                match read_export(dir) {
                    Ok(exp) => { self.rb = Some(Arc::new(exp)); self.rb_root = dir.to_path_buf(); }
                    Err(e)  => log::warn!("rekordbox export at {}: {e:#}", dir.display()),
                }
            }
            if self.rb.is_some() {
                out.push(Entry {
                    name: "rekordbox".to_string(), is_dir: true,
                    artist: None, bpm: None, kind: EntryKind::Descend(Loc::RbTree(0)),
                });
            }
        }

        let mut dirs:  Vec<Entry> = Vec::new();
        let mut files: Vec<Entry> = Vec::new();
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
                        kind: EntryKind::Track(path) });
                }
            }
        }
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out.extend(dirs);
        out.extend(files);
        out
    }

    /// A rekordbox playlist-tree folder: its child folders and playlists.
    fn rb_tree_entries(&self, node: u32) -> Vec<Entry> {
        let Some(exp) = self.rb.as_ref() else { return Vec::new() };
        exp.children(node).into_iter().map(|n| Entry {
            name:   n.name.clone(),
            is_dir: true,
            artist: None,
            bpm:    None,
            kind:   EntryKind::Descend(if n.is_folder {
                Loc::RbTree(n.id)
            } else {
                Loc::RbPlaylist(n.id)
            }),
        }).collect()
    }

    /// A rekordbox playlist: its tracks (load targets), in playlist order.
    fn rb_playlist_entries(&self, id: u32) -> Vec<Entry> {
        let Some(exp) = self.rb.as_ref() else { return Vec::new() };
        exp.playlist_tracks(id).into_iter().map(|t| Entry {
            name:   t.title.clone(),
            is_dir: false,
            artist: Some(t.artist.clone()),
            bpm:    Some(t.bpm),
            kind:   EntryKind::Track(t.path_on(&self.rb_root)),
        }).collect()
    }

    /// Move the highlight by `delta` rows, clamped (no wrap).
    pub fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() { return; }
        let last = self.entries.len() as i32 - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    /// Act on the highlighted row: descend, or report the track to load.
    pub fn enter(&mut self) -> Enter {
        let Some(sel) = self.entries.get(self.selected) else { return Enter::Nothing };
        match sel.kind.clone() {
            EntryKind::Descend(loc) => {
                self.stack.push(loc);
                self.selected = 0;
                self.rebuild();
                Enter::Folder
            }
            EntryKind::Track(path) => Enter::Track(path),
            EntryKind::Nothing     => Enter::Nothing,
        }
    }

    /// Go up one level (filesystem parent, or up the rekordbox tree).
    pub fn back(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.rebuild();
            self.selected = 0;
        } else if let Some(Loc::Fs(dir)) = self.stack.last().cloned() {
            // At the top filesystem level, ascend to the parent as before.
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
    // Gated on a real rekordbox USB: OPENDECK_TEST_RB=/run/media/dan/CDJ1
    #[test]
    fn browses_rekordbox_usb() {
        let Ok(root) = std::env::var("OPENDECK_TEST_RB") else { return };
        let root = std::path::PathBuf::from(root);
        let mut b = Browser::new(&root);
        // Top of the USB should offer a "rekordbox" entry.
        let rb_idx = b.entries().iter().position(|e| e.name == "rekordbox")
            .expect("rekordbox entry at USB root");
        b.selected = rb_idx;
        assert!(matches!(b.enter(), Enter::Folder));            // into the library
        assert!(!b.entries().is_empty(), "playlist tree non-empty");
        // Descend into the first playlist/folder until we reach tracks.
        b.selected = 0;
        assert!(matches!(b.enter(), Enter::Folder));            // into first playlist
        let track_count = b.entries().len();
        let all_tracks  = b.entries().iter().all(|e| !e.is_dir);
        assert!(track_count > 0, "playlist has tracks");
        assert!(all_tracks, "playlist entries are tracks");
        // A track loads to a path that exists on disk.
        b.selected = 0;
        match b.enter() {
            Enter::Track(p) => assert!(p.exists(), "track path resolves: {}", p.display()),
            _ => panic!("expected a track"),
        }
        println!("OK: {track_count} tracks in first playlist");
    }
}
