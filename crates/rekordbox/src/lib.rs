//! Read a Pioneer rekordbox device export (the `PIONEER/rekordbox/export.pdb`
//! database on a CDJ/XDJ USB) into freedj's world: the playlist tree plus a
//! track list with resolved artist names, BPM, and absolute file paths — enough
//! to browse and load a library the way the XDJ does.
//!
//! Parsing is done by the (vendored, MPL-2.0) `rekordcrate` crate; this crate is
//! the thin adapter. Cue-point / beat-grid import from the `ANLZ` files is a
//! later step.

use anyhow::{anyhow, Context, Result};
use binrw::BinRead;
use rekordcrate::pdb::string::DeviceSQLString;
use rekordcrate::pdb::{Header, Row};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

/// One track from the rekordbox database.
#[derive(Debug, Clone)]
pub struct RbTrack {
    pub id:            u32,
    pub title:         String,
    pub artist:        String,
    pub bpm:           f32,
    pub duration_secs: u32,
    pub sample_rate:   u32,
    /// Path as stored by rekordbox, rooted at the USB (e.g. `/Contents/…/x.mp3`).
    pub rel_path:      String,
    /// Path to this track's ANLZ analysis file (beat grid, cues, waveform).
    pub analyze_rel:   String,
}

impl RbTrack {
    /// Absolute path to the audio file on media mounted at `usb_root`.
    pub fn path_on(&self, usb_root: &Path) -> PathBuf {
        usb_root.join(self.rel_path.trim_start_matches('/'))
    }
    /// Absolute path to the ANLZ analysis file, if one is set.
    pub fn analyze_on(&self, usb_root: &Path) -> Option<PathBuf> {
        (!self.analyze_rel.is_empty())
            .then(|| usb_root.join(self.analyze_rel.trim_start_matches('/')))
    }
}

/// A node in the playlist tree — either a folder (contains child nodes) or a
/// playlist (contains track entries). Mirrors the XDJ's Playlists browse.
#[derive(Debug, Clone)]
pub struct RbPlaylistNode {
    pub id:         u32,
    pub parent_id:  u32,   // 0 = top level
    pub name:       String,
    pub is_folder:  bool,
    pub sort_order: u32,
}

/// A parsed export: the media root, its tracks, and its playlist tree.
pub struct RbExport {
    pub root:      PathBuf,
    pub tracks:    Vec<RbTrack>,
    pub playlists: Vec<RbPlaylistNode>,
    /// playlist node id → ordered track ids.
    entries:       HashMap<u32, Vec<u32>>,
    /// track id → index into `tracks`.
    by_id:         HashMap<u32, usize>,
}

impl RbExport {
    /// Track by rekordbox id.
    pub fn track(&self, id: u32) -> Option<&RbTrack> {
        self.by_id.get(&id).map(|&i| &self.tracks[i])
    }
    /// Child nodes of a folder (0 = top level), ordered as rekordbox sorts them.
    pub fn children(&self, parent_id: u32) -> Vec<&RbPlaylistNode> {
        let mut v: Vec<&RbPlaylistNode> =
            self.playlists.iter().filter(|n| n.parent_id == parent_id).collect();
        v.sort_by_key(|n| (n.sort_order, n.id));
        v
    }
    /// Tracks in a playlist, in playlist order.
    pub fn playlist_tracks(&self, playlist_id: u32) -> Vec<&RbTrack> {
        self.entries
            .get(&playlist_id)
            .map(|ids| ids.iter().filter_map(|id| self.track(*id)).collect())
            .unwrap_or_default()
    }
}

fn text(s: &DeviceSQLString) -> String {
    s.clone().into_string().unwrap_or_default()
}

/// Collect every row in the database (all tables) from any reader.
fn read_all_rows<R: Read + Seek>(r: &mut R) -> Result<Vec<Row>> {
    let header = Header::read(r).map_err(|e| anyhow!("parse pdb header: {e}"))?;
    let mut rows = Vec::new();
    for table in &header.tables {
        let pages = header
            .read_pages(r, binrw::Endian::Little, (&table.first_page, &table.last_page))
            .map_err(|e| anyhow!("read pages for {:?}: {e}", table.page_type))?;
        for page in pages {
            for group in &page.row_groups {
                rows.extend(group.present_rows());
            }
        }
    }
    Ok(rows)
}

/// Read `usb_root/PIONEER/rekordbox/export.pdb` into tracks + playlist tree,
/// with artist names resolved and track order stable.
pub fn read_export(usb_root: &Path) -> Result<RbExport> {
    let pdb = usb_root.join("PIONEER/rekordbox/export.pdb");
    let mut f = File::open(&pdb).with_context(|| format!("open {}", pdb.display()))?;
    read_export_from(&mut f, usb_root.to_path_buf())
}

/// Parse an `export.pdb` from any reader (e.g. bytes read over NFS from a linked
/// player). `root` is the media root that track paths resolve against — a real
/// mount for local media, or a label for a network source.
pub fn read_export_from<R: Read + Seek>(reader: &mut R, root: PathBuf) -> Result<RbExport> {
    let rows = read_all_rows(reader)?;

    let mut artists: HashMap<u32, String> = HashMap::new();
    for row in &rows {
        if let Row::Artist(a) = row {
            artists.insert(a.id.0, text(&a.name));
        }
    }

    let mut tracks: Vec<RbTrack> = Vec::new();
    let mut playlists: Vec<RbPlaylistNode> = Vec::new();
    // playlist id → (entry_index, track_id), sorted into order afterwards.
    let mut raw_entries: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();

    for row in &rows {
        match row {
            Row::Track(t) => tracks.push(RbTrack {
                id:            t.id.0,
                title:         text(&t.title),
                artist:        artists.get(&t.artist_id.0).cloned().unwrap_or_default(),
                bpm:           t.tempo as f32 / 100.0,
                duration_secs: t.duration as u32,
                sample_rate:   t.sample_rate,
                rel_path:      text(&t.file_path),
                analyze_rel:   text(&t.analyze_path),
            }),
            Row::PlaylistTreeNode(n) => playlists.push(RbPlaylistNode {
                id:         n.id.0,
                parent_id:  n.parent_id.0,
                name:       text(&n.name),
                is_folder:  n.is_folder(),
                sort_order: n.sort_order,
            }),
            Row::PlaylistEntry(e) => raw_entries
                .entry(e.playlist_id.0)
                .or_default()
                .push((e.entry_index, e.track_id.0)),
            _ => {}
        }
    }

    tracks.sort_by_key(|t| t.id);
    let by_id: HashMap<u32, usize> =
        tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();

    let entries: HashMap<u32, Vec<u32>> = raw_entries
        .into_iter()
        .map(|(pid, mut v)| {
            v.sort_by_key(|(idx, _)| *idx);
            (pid, v.into_iter().map(|(_, tid)| tid).collect())
        })
        .collect();

    Ok(RbExport { root, tracks, playlists, entries, by_id })
}

// ── ANLZ analysis import (beat grid + cues) ───────────────────────────────────

use rekordcrate::anlz::{Content, CueListType, CueType, ANLZ};

/// One beat from the rekordbox beat grid.
#[derive(Debug, Clone, Copy)]
pub struct RbBeat {
    pub time_ms:     u32,
    pub bpm:         f32,
    /// Beat within the bar, 1–4 (1 = downbeat).
    pub beat_in_bar: u8,
}

/// A memory or hot cue (or loop) from the analysis.
#[derive(Debug, Clone, Copy)]
pub struct RbCue {
    pub time_ms: u32,
    pub is_loop: bool,
    pub loop_ms: u32,
    /// `Some(n)` if this is hot cue n, `None` for a memory cue.
    pub hot_cue: Option<u32>,
}

/// Parsed analysis for one track.
#[derive(Debug, Clone, Default)]
pub struct RbAnalysis {
    pub beats:       Vec<RbBeat>,
    pub memory_cues: Vec<RbCue>,
    pub hot_cues:    Vec<RbCue>,
}

/// Read a rekordbox `ANLZ####.DAT` analysis file: beat grid + cue points.
/// Waveforms are ignored (freedj renders its own). Missing sections are fine.
pub fn read_anlz(path: &Path) -> Result<RbAnalysis> {
    let mut r = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let anlz = ANLZ::read(&mut r).map_err(|e| anyhow!("parse ANLZ: {e}"))?;

    let mut out = RbAnalysis::default();
    for section in &anlz.sections {
        match &section.content {
            Content::BeatGrid(bg) => {
                out.beats = bg.beats.iter().map(|b| RbBeat {
                    time_ms:     b.time,
                    bpm:         b.tempo as f32 / 100.0,
                    beat_in_bar: b.beat_number as u8,
                }).collect();
            }
            Content::CueList(cl) => {
                let hot = cl.list_type == CueListType::HotCues;
                let dst = if hot { &mut out.hot_cues } else { &mut out.memory_cues };
                for c in &cl.cues {
                    dst.push(RbCue {
                        time_ms: c.time,
                        is_loop: c.cue_type == CueType::Loop,
                        loop_ms: c.loop_time,
                        hot_cue: hot.then_some(c.hot_cue),
                    });
                }
            }
            _ => {}
        }
    }
    out.memory_cues.sort_by_key(|c| c.time_ms);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Gated: OPENDECK_TEST_RB=/run/media/dan/CDJ1
    #[test]
    fn reads_anlz_beatgrid() {
        let Ok(root) = std::env::var("OPENDECK_TEST_RB") else { return };
        let root = std::path::PathBuf::from(root);
        let exp = read_export(&root).unwrap();
        let t = exp.tracks.iter().find(|t| t.title.contains("OG Sins")).expect("OG Sins");
        let ap = t.analyze_on(&root).expect("analyze path");
        let a = read_anlz(&ap).unwrap();
        assert!(!a.beats.is_empty(), "has beats");
        let first = a.beats[0];
        assert!((first.bpm - 125.0).abs() < 0.5, "≈125 BPM, got {}", first.bpm);
        assert_eq!(first.beat_in_bar, 1, "first beat is a downbeat");
        // beat spacing ≈ 480 ms at 125 BPM
        let gap = a.beats[1].time_ms - a.beats[0].time_ms;
        assert!((gap as i32 - 480).abs() <= 2, "≈480ms spacing, got {gap}");
        println!("OK: {} beats, first @ {}ms {} BPM", a.beats.len(), first.time_ms, first.bpm);
    }
}
