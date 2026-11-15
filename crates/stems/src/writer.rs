//! Writes a stems index: `CDJMODS/index.json` and the bounded `index-v2`,
//! from entries keyed by the track IDs the media's owner hands out.  For a
//! media server that serves banks under its own library IDs (OverCue
//! players look a LINK track up by the owner's ID), and to re-key an
//! existing stick.  The banks themselves are not touched.

use crate::v2::bucket_of;
use crate::{hex, sha256};
use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// Write `index.json` (Device Library namespace, `keyed_by` export.pdb) and
/// `index-v2` with `bits` bucket bits for `entries` (track ID → the entry
/// as index.json holds it, `file_path` included).  A fresh generation each
/// time: readers holding the old one see `current` change, as the spec
/// asks of a writer.  The old generation's folder is removed afterwards.
pub fn write_index(root: &Path, entries: &BTreeMap<u32, Value>, bits: u32) -> Result<()> {
    if bits > 24 { bail!("index-v2: at most 24 bucket bits"); }
    let mods = root.join("CDJMODS");
    std::fs::create_dir_all(&mods)?;
    let mut tracks = Map::new();
    for (id, e) in entries {
        if *id == 0 { bail!("track ID 0 is not valid"); }
        if !e.is_object() { bail!("entry {id} is not an object"); }
        tracks.insert(id.to_string(), e.clone());
    }
    let index = serde_json::json!({ "schema": "overcue-index/1", "keyed_by": "export.pdb", "tracks": tracks, "tracks_onelibrary": {} });
    let index_bytes = serde_json::to_vec(&index)?;

    // A generation no earlier one shares: the hash of what it indexes and when.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut generation = [0u8; 16];
    generation.copy_from_slice(&sha256(&[&index_bytes[..], &now.to_be_bytes()[..]].concat())[..16]);
    if generation.iter().all(|b| *b == 0) { generation[0] = 1; }
    let v2 = mods.join("index-v2");
    let gdir = v2.join(hex(&generation));
    let mut buckets: BTreeMap<u32, Vec<(u32, Vec<u8>)>> = BTreeMap::new();
    for (id, e) in entries {
        // A record is the entry without its path, in a one-track index.
        let mut rec = e.as_object().unwrap().clone();
        rec.remove("file_path");
        let mut one = Map::new();
        one.insert(id.to_string(), Value::Object(rec));
        let body = serde_json::to_vec(&serde_json::json!({ "schema": "overcue-index/1", "keyed_by": "export.pdb", "tracks": one }))?;
        if body.len() > 8192 { bail!("index-v2 record for {id} is {} bytes (8192 at most)", body.len()); }
        buckets.entry(bucket_of(*id, bits)).or_default().push((*id, body));
    }
    std::fs::create_dir_all(gdir.join("1"))?;
    std::fs::create_dir_all(gdir.join("2"))?;
    for (bucket, mut rows) in buckets {
        rows.sort_by_key(|r| r.0);
        if rows.len() > 256 { bail!("index-v2: {} tracks in one bucket; use more bits", rows.len()); }
        let table_len = 96 + 48 * rows.len();
        let size = table_len + rows.iter().map(|r| r.1.len()).sum::<usize>();
        let mut head = Vec::with_capacity(40);
        head.extend_from_slice(b"OCSIBK02");
        head.extend_from_slice(&generation);
        for v in [1u32, bucket, rows.len() as u32, size as u32] { head.extend_from_slice(&v.to_be_bytes()); }
        let mut table = Vec::new();
        let mut off = table_len;
        for (id, rec) in &rows {
            for v in [*id, off as u32, rec.len() as u32, 1] { table.extend_from_slice(&v.to_be_bytes()); }
            table.extend_from_slice(&sha256(rec));
            off += rec.len();
        }
        let mut file = head.clone();
        file.extend_from_slice(&sha256(&[&head[..], &table[..]].concat()));
        file.extend_from_slice(&[0; 24]);
        file.extend_from_slice(&table);
        for (_, rec) in &rows { file.extend_from_slice(rec); }
        let p = gdir.join("1").join(format!("{:02X}", bucket >> 16)).join(format!("{:02X}", (bucket >> 8) & 0xff));
        std::fs::create_dir_all(&p)?;
        std::fs::write(p.join(format!("{:02X}.IDX", bucket & 0xff)), file)?;
    }
    // index.json, then the new generation goes live by replacing `current`.
    let tmp = mods.join("index.json.tmp");
    std::fs::write(&tmp, &index_bytes)?;
    std::fs::rename(&tmp, mods.join("index.json"))?;
    let old = crate::v2::current(&mut crate::DirMedia::new(root)).ok().flatten();
    let mut cur = b"OCSIDX02".to_vec();
    cur.extend_from_slice(&generation);
    cur.extend_from_slice(&1u32.to_be_bytes());
    cur.extend_from_slice(&bits.to_be_bytes());
    let h = sha256(&cur);
    cur.extend_from_slice(&h);
    let tmp = v2.join("current.tmp");
    std::fs::write(&tmp, cur)?;
    std::fs::rename(&tmp, v2.join("current")).context("index-v2 current")?;
    if let Some(o) = old.filter(|o| o.generation != generation) {
        let _ = std::fs::remove_dir_all(v2.join(hex(&o.generation)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{write_stick, SynthCodec, SynthTrack};
    use crate::{Index, Role};

    /// Re-keying a stick: the same banks under new IDs, found through both
    /// index.json and index-v2, old IDs gone.
    #[test]
    fn rekeyed_index_finds_the_same_banks_under_new_ids() {
        let d = tempfile::tempdir().unwrap();
        let pcm: Vec<i16> = (0..96_000).map(|i| (i % 300) as i16).collect();
        let roles: BTreeMap<Role, Vec<i16>> = Role::ALL.iter().map(|r| (*r, pcm.clone())).collect();
        let tracks: Vec<SynthTrack> = [3u32, 9].iter().map(|id| SynthTrack { id: *id, onelibrary_id: None,
            file_path: format!("/Contents/t{id}.mp3"), source: vec![*id as u8], roles: roles.clone() }).collect();
        write_stick(d.path(), &tracks, SynthCodec::Flac96, Some(2)).unwrap();
        let raw: Value = serde_json::from_slice(&std::fs::read(d.path().join("CDJMODS/index.json")).unwrap()).unwrap();
        let mut entries = BTreeMap::new();
        for (old, new) in [(3u32, 103u32), (9, 1009)] { entries.insert(new, raw["tracks"][old.to_string()].clone()); }
        write_index(d.path(), &entries, 4).unwrap();
        let mut m = crate::DirMedia::new(d.path());
        let idx = Index::read(&mut m).unwrap().unwrap();
        assert_eq!(idx.by_path("/Contents/t3.mp3").unwrap().unwrap().bundle, raw["tracks"]["3"]["bundle"].as_str().unwrap());
        for (new, old) in [(103u32, 3u32), (1009, 9)] {
            let e = crate::v2::lookup(&mut m, crate::index::Namespace::DeviceLibrary, new).unwrap().expect("found by new ID");
            assert_eq!(e.bundle, raw["tracks"][old.to_string()]["bundle"].as_str().unwrap());
            let _ = crate::RoleReader::open(&mut m, &e, Role::Drums).expect("its bank opens");
        }
        assert!(crate::v2::lookup(&mut m, crate::index::Namespace::DeviceLibrary, 3).unwrap().is_none(), "old ID gone");
        assert_eq!(std::fs::read_dir(d.path().join("CDJMODS/index-v2")).unwrap().count(), 2, "one generation + current");
    }
}
