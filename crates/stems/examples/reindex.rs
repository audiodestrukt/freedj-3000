//! Re-key a prepared stick's stems index to other track IDs (the IDs a
//! media server hands out for the same files), keeping the banks:
//!
//! cargo run -p opendeck-stems --example reindex -- <root> <new>=<old>[@<served path>] ...
//!
//! Each `<new>` gets the entry index.json has under `<old>`, with its
//! `file_path` replaced by `<served path>` when given.  Writes index.json
//! and a fresh index-v2 generation (`opendeck_stems::writer`).
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (root, maps) = args.split_first().ok_or_else(|| anyhow!("usage: reindex <root> <new>=<old>[@path] ..."))?;
    let root = std::path::Path::new(root);
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join("CDJMODS/index.json"))?)?;
    let mut entries = BTreeMap::new();
    for m in maps {
        let (new, rest) = m.split_once('=').context("want <new>=<old>")?;
        let (old, path) = match rest.split_once('@') { Some((o, p)) => (o, Some(p)), None => (rest, None) };
        let mut e = raw["tracks"][old].clone();
        if e.is_null() { return Err(anyhow!("no entry {old} in index.json")); }
        if let Some(p) = path { e["file_path"] = serde_json::Value::String(p.to_string()); }
        println!("{new} ← {old} ({})", e["file_path"]);
        entries.insert(new.parse::<u32>()?, e);
    }
    opendeck_stems::writer::write_index(root, &entries, 4)
}
