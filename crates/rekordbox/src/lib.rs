//! Read a Pioneer rekordbox device export (the `PIONEER/rekordbox/export.pdb`
//! database on a CDJ/XDJ USB) into freedj's world: a flat, sorted track list
//! with resolved artist names, BPM, and absolute file paths.
//!
//! Parsing is done by the (vendored, MPL-2.0) `rekordcrate` crate; this crate is
//! the thin adapter that pulls out the fields we care about and resolves paths.
//! Cue-point / beat-grid import from the `ANLZ` files is a later step.

use anyhow::{anyhow, Context, Result};
use binrw::BinRead;
use rekordcrate::pdb::{Header, Row};
use rekordcrate::pdb::string::DeviceSQLString;
use std::collections::HashMap;
use std::fs::File;
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

/// A parsed export: the media root plus its tracks.
pub struct RbExport {
    pub root:   PathBuf,
    pub tracks: Vec<RbTrack>,
}

fn text(s: &DeviceSQLString) -> String {
    s.clone().into_string().unwrap_or_default()
}

/// Collect every row in the database (all tables).
fn read_all_rows(pdb: &Path) -> Result<Vec<Row>> {
    let mut r = File::open(pdb).with_context(|| format!("open {}", pdb.display()))?;
    let header = Header::read(&mut r).map_err(|e| anyhow!("parse pdb header: {e}"))?;
    let mut rows = Vec::new();
    for table in &header.tables {
        let pages = header
            .read_pages(&mut r, binrw::Endian::Little, (&table.first_page, &table.last_page))
            .map_err(|e| anyhow!("read pages for {:?}: {e}", table.page_type))?;
        for page in pages {
            for group in &page.row_groups {
                rows.extend(group.present_rows());
            }
        }
    }
    Ok(rows)
}

/// Read `usb_root/PIONEER/rekordbox/export.pdb` into a sorted track list with
/// artist names resolved.
pub fn read_export(usb_root: &Path) -> Result<RbExport> {
    let pdb = usb_root.join("PIONEER/rekordbox/export.pdb");
    let rows = read_all_rows(&pdb)?;

    let mut artists: HashMap<u32, String> = HashMap::new();
    for row in &rows {
        if let Row::Artist(a) = row {
            artists.insert(a.id.0, text(&a.name));
        }
    }

    let mut tracks: Vec<RbTrack> = rows
        .iter()
        .filter_map(|row| match row {
            Row::Track(t) => Some(RbTrack {
                id:            t.id.0,
                title:         text(&t.title),
                artist:        artists.get(&t.artist_id.0).cloned().unwrap_or_default(),
                bpm:           t.tempo as f32 / 100.0,
                duration_secs: t.duration as u32,
                sample_rate:   t.sample_rate,
                rel_path:      text(&t.file_path),
                analyze_rel:   text(&t.analyze_path),
            }),
            _ => None,
        })
        .collect();
    tracks.sort_by_key(|t| t.id);

    Ok(RbExport { root: usb_root.to_path_buf(), tracks })
}
