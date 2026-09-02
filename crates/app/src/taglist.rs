//! TAG LIST — the unit's on-the-fly playlist.  In BROWSE, TAG TRACK / REMOVE
//! toggles the highlighted track; the TAG LIST screen lists the tagged tracks
//! in browse style and loads from them; there, TAG TRACK / REMOVE drops the
//! highlighted one.  Persisted as JSON in the app's data dir so it survives a
//! relaunch (the unit keeps its list on the USB).

use crate::browser::Load;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One tagged track: how to load it, and what to show in the list.
#[derive(Clone, Serialize, Deserialize)]
pub struct TagEntry {
    pub name:   String,
    pub artist: Option<String>,
    pub load:   Load,
}

pub struct TagList {
    entries:      Vec<TagEntry>,
    pub selected: usize,
    path:         PathBuf,
}

impl TagList {
    /// The unit holds up to 100 tagged tracks.
    pub const MAX: usize = 100;

    /// Load the persisted list (an absent or unreadable file is an empty list).
    pub fn open() -> Self {
        let path = app_data_dir().join("taglist.json");
        let entries = std::fs::read(&path).ok()
            .and_then(|b| serde_json::from_slice::<Vec<TagEntry>>(&b)
                .map_err(|e| log::warn!("tag list {}: {e}", path.display())).ok())
            .unwrap_or_default();
        if !entries.is_empty() { log::info!("tag list: {} tracks from {}", entries.len(), path.display()); }
        Self { entries, selected: 0, path }
    }

    pub fn entries(&self) -> &[TagEntry] { &self.entries }
    pub fn len(&self) -> usize { self.entries.len() }
    #[allow(dead_code)] // companion to len(); clippy expects it to exist
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn selected_entry(&self) -> Option<&TagEntry> { self.entries.get(self.selected) }
    pub fn contains(&self, load: &Load) -> bool { self.entries.iter().any(|e| &e.load == load) }

    /// TAG TRACK / REMOVE in BROWSE: add the track if absent, remove it if
    /// present.  Returns whether it is tagged afterwards.
    pub fn toggle(&mut self, e: TagEntry) -> bool {
        let tagged = if let Some(i) = self.entries.iter().position(|x| x.load == e.load) {
            self.entries.remove(i);
            false
        } else if self.entries.len() >= Self::MAX {
            log::info!("tag list: full ({} tracks)", Self::MAX);
            return false;
        } else {
            self.entries.push(e);
            true
        };
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.save();
        tagged
    }

    /// TAG TRACK / REMOVE on the TAG LIST screen: drop the highlighted track.
    pub fn remove_selected(&mut self) -> Option<TagEntry> {
        if self.entries.is_empty() { return None; }
        let e = self.entries.remove(self.selected);
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.save();
        Some(e)
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() { return; }
        let last = self.entries.len() as i32 - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                log::warn!("tag list: cannot create {}: {e}", dir.display());
                return;
            }
        }
        match serde_json::to_vec_pretty(&self.entries).map_err(anyhow::Error::from)
            .and_then(|b| std::fs::write(&self.path, b).map_err(anyhow::Error::from))
        {
            Ok(())  => log::debug!("tag list: saved {} tracks", self.entries.len()),
            Err(e)  => log::warn!("tag list: save failed: {e:#}"),
        }
    }
}

/// Where per-user app data lives.  iOS: the sandbox's Documents folder (also
/// visible in the Files app, so the list is inspectable).  Desktop:
/// $XDG_DATA_HOME or ~/.local/share, under opendeck/.
pub fn app_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "ios") {
        home.join("Documents")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local").join("share"))
            .join("opendeck")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: &str) -> TagEntry {
        TagEntry { name: n.into(), artist: None, load: Load::Local { path: PathBuf::from(n), analyze: None } }
    }
    fn fresh(tmp: &std::path::Path) -> TagList {
        TagList { entries: Vec::new(), selected: 0, path: tmp.join("taglist.json") }
    }

    #[test]
    fn toggle_adds_then_removes_and_persists() {
        let tmp = std::env::temp_dir().join(format!("opendeck-tl-{}", std::process::id()));
        let mut t = fresh(&tmp);
        assert!(t.toggle(entry("a.mp3")));
        assert!(t.toggle(entry("b.mp3")));
        assert!(t.contains(&entry("a.mp3").load));
        assert!(!t.toggle(entry("a.mp3")));             // second toggle removes
        assert_eq!(t.len(), 1);
        // Round-trips through the file.
        let back: Vec<TagEntry> = serde_json::from_slice(&std::fs::read(&t.path).unwrap()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "b.mp3");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn remove_selected_keeps_the_cursor_in_range() {
        let tmp = std::env::temp_dir().join(format!("opendeck-tl2-{}", std::process::id()));
        let mut t = fresh(&tmp);
        for n in ["a", "b", "c"] { t.toggle(entry(n)); }
        t.move_selection(10);                            // clamps to last
        assert_eq!(t.selected, 2);
        assert_eq!(t.remove_selected().unwrap().name, "c");
        assert_eq!(t.selected, 1);                       // pulled back to the new last
        t.remove_selected(); t.remove_selected();
        assert!(t.remove_selected().is_none());
        assert_eq!(t.selected, 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
