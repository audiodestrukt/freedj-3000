//! Screen-navigation rules, kept out of `DeckApp::apply` so they can be
//! tested without an audio device.

use crate::browser::{Load, SourceRow};
use crate::settings::Skin;
use crate::ScreenMode;

/// Move a list highlight by `delta`, clamped to the list (0 for an empty one).
pub fn step_cursor(cur: usize, delta: i32, len: usize) -> usize {
    if len == 0 { return 0; }
    (cur as i32 + delta).clamp(0, len as i32 - 1) as usize
}

/// LOAD PREVIOUS TRACK's highlight: nothing until the selector first turns,
/// which lands on the newest track whatever the direction (the unit does).
pub fn step_history_cursor(cur: Option<usize>, delta: i32, len: usize) -> Option<usize> {
    if len == 0 { return None; }
    Some(match cur { None => 0, Some(c) => step_cursor(c, delta, len) })
}

/// The CDJ's BROWSE key arrives as a level ("browse open"), not a press:
/// follow it. Returns the new mode, or None when nothing changes.
pub fn follow_browse_level(cur: ScreenMode, on: bool) -> Option<ScreenMode> {
    match (on, cur == ScreenMode::Browse) {
        (true, false) => Some(ScreenMode::Browse),
        (false, true) => Some(ScreenMode::Playback),
        _ => None,
    }
}

/// LOAD PREVIOUS TRACK toggles on the CDJ-3000 skin; the XDJ-1000MK2 has no
/// such screen, so there it only ever closes.
pub fn toggle_history(cur: ScreenMode, skin: Skin) -> ScreenMode {
    if cur == ScreenMode::History { ScreenMode::Playback }
    else if skin == Skin::Cdj3000 { ScreenMode::History }
    else { cur }
}

/// SOURCE opens on the row the loaded track came from. The last match wins:
/// a stick mounted inside the home folder belongs to its own row.
pub fn source_start(rows: &[SourceRow], loaded: Option<&Load>) -> usize {
    loaded.and_then(|l| rows.iter().rposition(|r| r.holds(l))).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::SourceKind;
    use std::path::PathBuf;

    #[test]
    fn list_cursor_clamps_and_survives_empty_lists() {
        assert_eq!(step_cursor(0, -1, 5), 0);
        assert_eq!(step_cursor(3, 1, 5), 4);
        assert_eq!(step_cursor(4, 10, 5), 4);
        assert_eq!(step_cursor(2, 0, 0), 0);
    }

    #[test]
    fn history_highlight_starts_on_the_first_turn() {
        assert_eq!(step_history_cursor(None, 1, 3), Some(0));
        assert_eq!(step_history_cursor(None, -1, 3), Some(0), "either direction lands on the newest");
        assert_eq!(step_history_cursor(Some(0), 1, 3), Some(1));
        assert_eq!(step_history_cursor(Some(2), 1, 3), Some(2));
        assert_eq!(step_history_cursor(None, 1, 0), None, "nothing to highlight");
    }

    #[test]
    fn the_cdj_browse_level_opens_and_closes_browse_only() {
        assert_eq!(follow_browse_level(ScreenMode::Playback, true), Some(ScreenMode::Browse));
        assert_eq!(follow_browse_level(ScreenMode::Browse, true), None);
        assert_eq!(follow_browse_level(ScreenMode::Browse, false), Some(ScreenMode::Playback));
        // Level off while on another screen leaves it alone.
        assert_eq!(follow_browse_level(ScreenMode::Menu, false), None);
        assert_eq!(follow_browse_level(ScreenMode::TagList, true), Some(ScreenMode::Browse));
    }

    #[test]
    fn load_previous_track_exists_only_on_the_cdj_skin() {
        assert_eq!(toggle_history(ScreenMode::Playback, Skin::Cdj3000), ScreenMode::History);
        assert_eq!(toggle_history(ScreenMode::History, Skin::Cdj3000), ScreenMode::Playback);
        assert_eq!(toggle_history(ScreenMode::Playback, Skin::Xdj1000Mk2), ScreenMode::Playback);
        assert_eq!(toggle_history(ScreenMode::History, Skin::Xdj1000Mk2), ScreenMode::Playback, "can still close");
    }

    #[test]
    fn source_opens_on_the_loaded_tracks_row() {
        let row = |kind, path: &str| SourceRow { kind, label: String::new(), player: None,
                                                 path: Some(PathBuf::from(path)), ip: None, rekordbox: false };
        // A stick mounted inside the home folder.
        let rows = vec![row(SourceKind::Local, "/home/dan"), row(SourceKind::Usb, "/home/dan/stick")];
        let on = |p: &str| Load::Local { path: PathBuf::from(p), analyze: None };
        assert_eq!(source_start(&rows, Some(&on("/home/dan/stick/a.mp3"))), 1, "the stick, not home");
        assert_eq!(source_start(&rows, Some(&on("/home/dan/b.mp3"))), 0);
        assert_eq!(source_start(&rows, Some(&on("/elsewhere/c.mp3"))), 0, "unknown: the first row");
        assert_eq!(source_start(&rows, None), 0);
    }
}
