//! Filesystem track browser — the BROWSE screen's model.
//!
//! A first version that browses the filesystem directly, exactly as a CDJ does
//! for a USB stick without a rekordbox export: a folder is a category, the audio
//! files inside are the rows.  No library/database yet (WORKSTREAMS F1/F2); when
//! that lands it slots in behind the same navigation interface.
//!
//! The model owns navigation only — turning the selector, entering a folder,
//! going back.  Actually loading the highlighted track is the deck's job
//! (`DeckApp::load_track`); `enter()` just reports which it is.

use std::path::{Path, PathBuf};

/// Audio extensions we offer as loadable rows.  Everything else in a directory
/// is hidden (a CDJ shows only playable files + folders).
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "aiff", "aif", "ogg", "opus"];

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.iter().any(|a| a.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

#[derive(Clone)]
pub struct Entry {
    pub name:   String,
    pub path:   PathBuf,
    pub is_dir: bool,
}

/// What `enter()` decided the highlighted row is.
pub enum Enter {
    /// Descended into a folder (the view has been refreshed).
    Folder,
    /// A track to load — the deck should load this path.
    Track(PathBuf),
    /// Nothing to do (empty directory, or unreadable entry).
    Nothing,
}

pub struct Browser {
    cwd:          PathBuf,
    entries:      Vec<Entry>,
    pub selected: usize,
}

impl Browser {
    /// Start in `dir` if it is a directory, otherwise in its parent (so passing
    /// the currently-loaded track file opens the folder it lives in).
    pub fn new(dir: &Path) -> Self {
        let start = if dir.is_dir() {
            dir.to_path_buf()
        } else {
            // A bare filename ("techno.mp3") has an empty parent — fall back to
            // the working directory so the browser opens somewhere real.
            match dir.parent() {
                Some(par) if !par.as_os_str().is_empty() => par.to_path_buf(),
                _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            }
        };
        let mut b = Browser { cwd: start, entries: Vec::new(), selected: 0 };
        b.refresh();
        b
    }

    pub fn cwd(&self) -> &Path { &self.cwd }
    pub fn entries(&self) -> &[Entry] { &self.entries }

    /// Re-read the current directory: subfolders first (alphabetical), then
    /// audio files (alphabetical).  Selection is clamped into range.
    pub fn refresh(&mut self) {
        let mut dirs:  Vec<Entry> = Vec::new();
        let mut files: Vec<Entry> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.cwd) {
            for ent in rd.flatten() {
                let path = ent.path();
                let name = ent.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') { continue; }          // hide dotfiles
                let is_dir = path.is_dir();
                if is_dir {
                    dirs.push(Entry { name, path, is_dir: true });
                } else if is_audio(&path) {
                    files.push(Entry { name, path, is_dir: false });
                }
            }
        }
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        self.entries = dirs;
        self.entries.extend(files);
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    /// Move the highlight by `delta` rows, clamped (no wrap — as on the unit).
    pub fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() { return; }
        let last = self.entries.len() as i32 - 1;
        let next = (self.selected as i32 + delta).clamp(0, last);
        self.selected = next as usize;
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Act on the highlighted row: descend into a folder, or report the track
    /// to load.
    pub fn enter(&mut self) -> Enter {
        let Some(sel) = self.entries.get(self.selected) else { return Enter::Nothing };
        if sel.is_dir {
            self.cwd = sel.path.clone();
            self.selected = 0;
            self.refresh();
            Enter::Folder
        } else {
            Enter::Track(sel.path.clone())
        }
    }

    /// Go up to the parent directory, keeping the child we came from highlighted.
    pub fn back(&mut self) {
        if let Some(parent) = self.cwd.parent().map(Path::to_path_buf) {
            let came_from = self.cwd.clone();
            self.cwd = parent;
            self.refresh();
            self.selected = self.entries.iter()
                .position(|e| e.path == came_from)
                .unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> PathBuf {
        let base = std::env::temp_dir().join(format!("odbrowse-{}", std::process::id()));
        let _ = fs::create_dir_all(&base);
        base
    }

    #[test]
    fn lists_folders_first_then_audio_hides_others() {
        let d = tmp().join("a");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("z.mp3"), b"x").unwrap();
        fs::write(d.join("a.wav"), b"x").unwrap();
        fs::write(d.join("notes.txt"), b"x").unwrap();   // hidden (not audio)
        let b = Browser::new(&d);
        let names: Vec<&str> = b.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "a.wav", "z.mp3"]);   // dir first, then audio sorted
    }

    #[test]
    fn opening_a_file_starts_in_its_folder() {
        let d = tmp().join("b");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let f = d.join("track.mp3");
        fs::write(&f, b"x").unwrap();
        let b = Browser::new(&f);   // pass a file, not a dir
        assert_eq!(b.cwd(), d.as_path());
    }

    #[test]
    fn enter_folder_then_back_restores_highlight() {
        let d = tmp().join("c");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("kids")).unwrap();
        fs::write(d.join("kids").join("t.mp3"), b"x").unwrap();
        let mut b = Browser::new(&d);
        assert!(matches!(b.enter(), Enter::Folder));         // into "kids"
        assert_eq!(b.cwd(), d.join("kids").as_path());
        assert!(matches!(b.enter(), Enter::Track(_)));       // t.mp3
        b.back();                                            // back to d
        assert_eq!(b.cwd(), d.as_path());
        assert_eq!(b.selected_entry().unwrap().name, "kids");// highlight restored
    }

    #[test]
    fn selection_clamps_no_wrap() {
        let d = tmp().join("e");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("a.mp3"), b"x").unwrap();
        fs::write(d.join("b.mp3"), b"x").unwrap();
        let mut b = Browser::new(&d);
        b.move_selection(-5);
        assert_eq!(b.selected, 0);
        b.move_selection(99);
        assert_eq!(b.selected, 1);
    }
}
