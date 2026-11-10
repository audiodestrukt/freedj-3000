//! `/CDJMODS/index-v2`: one track's entry with a few small, bounded reads.
//!
//! A present but damaged index-v2 fails closed (an error, no fallback to
//! index.json); only a missing `index-v2/current` means "legacy stick".

use crate::index::{parse_unique, Entry, Namespace};
use crate::{hex, sha256, Media};
use anyhow::{anyhow, bail, ensure, Context, Result};

const MARKER: &[u8; 8] = b"OCSIDX02";
const BUCKET: &[u8; 8] = b"OCSIBK02";
const RECORD_MAX: u32 = 8192;
const BUCKET_MAX: u64 = 96 + 256 * (48 + RECORD_MAX as u64);

/// The active generation, from `index-v2/current`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Current { pub generation: [u8; 16], pub bits: u32 }

/// Read `index-v2/current`; `None` when the stick has no index-v2.
pub fn current(media: &mut dyn Media) -> Result<Option<Current>> {
    let Some(c) = media.read_all("/CDJMODS/index-v2/current", 64)? else { return Ok(None) };
    let bits = u32::from_be_bytes(c.get(28..32).ok_or_else(|| anyhow!("index-v2: short current"))?.try_into()?);
    ensure!(c.len() == 64 && &c[..8] == MARKER && c[8..24].iter().any(|b| *b != 0)
            && c[24..28] == 1u32.to_be_bytes() && bits <= 24 && sha256(&c[..32])[..] == c[32..],
            "index-v2: invalid current marker");
    Ok(Some(Current { generation: c[8..24].try_into().unwrap(), bits }))
}

/// The bucket a track ID hashes to.
pub fn bucket_of(id: u32, bits: u32) -> u32 {
    if bits == 0 { 0 } else { (id.wrapping_mul(2_654_435_761)) >> (32 - bits) }
}

/// Path of a bucket file.
pub fn bucket_path(generation: &[u8; 16], ns: Namespace, bucket: u32) -> String {
    format!("/CDJMODS/index-v2/{}/{}/{:02X}/{:02X}/{:02X}.IDX", hex(generation), ns as u32,
            bucket >> 16, (bucket >> 8) & 0xff, bucket & 0xff)
}

/// One track's entry from index-v2: `Ok(None)` when the track has no stems
/// (or the stick has no index-v2; check [`current`] to tell them apart).
/// The generation is read again afterwards; if a writer swapped it
/// mid-read, that is an error and the caller may retry.
pub fn lookup(media: &mut dyn Media, ns: Namespace, id: u32) -> Result<Option<Entry>> {
    let Some(cur) = current(media)? else { return Ok(None) };
    let found = lookup_in(media, &cur, ns, id)?;
    ensure!(current(media)? == Some(cur), "index-v2: generation changed while reading");
    Ok(found)
}

fn lookup_in(media: &mut dyn Media, cur: &Current, ns: Namespace, id: u32) -> Result<Option<Entry>> {
    let bucket = bucket_of(id, cur.bits);
    let path = bucket_path(&cur.generation, ns, bucket);
    let Some(data) = media.read_all(&path, BUCKET_MAX)? else { return Ok(None) };
    ensure!(data.len() >= 96 && &data[..8] == BUCKET && data[8..24] == cur.generation && data[72..96].iter().all(|b| *b == 0),
            "index-v2: invalid bucket header");
    let be = |o: usize| u32::from_be_bytes(data[o..o + 4].try_into().unwrap());
    let (got_ns, got_bucket, count, size) = (be(24), be(28), be(32), be(36));
    ensure!(got_ns == ns as u32 && got_bucket == bucket && (1..=256).contains(&count) && size as usize == data.len()
            && data.len() >= 96 + 48 * count as usize, "index-v2: invalid bucket table");
    let rows = &data[96..96 + 48 * count as usize];
    let mut hashed = data[..40].to_vec();
    hashed.extend_from_slice(rows);
    ensure!(sha256(&hashed)[..] == data[40..72], "index-v2: bucket checksum mismatch");

    let (mut found, mut end, mut prev) = (None, 96 + 48 * count as usize, 0u32);
    for (i, row) in rows.chunks_exact(48).enumerate() {
        let r = |o: usize| u32::from_be_bytes(row[o..o + 4].try_into().unwrap());
        let (key, offset, len, flags) = (r(0), r(4) as usize, r(8), r(12));
        let record = data.get(offset..offset + len as usize);
        ensure!(prev < key && offset == end && (1..=RECORD_MAX).contains(&len) && flags == 1
                && record.is_some_and(|rec| sha256(rec)[..] == row[16..48]), "index-v2: invalid record {i}");
        if key == id { found = record; }
        end += len as usize;
        prev = key;
    }
    ensure!(end == data.len(), "index-v2: unreferenced bucket bytes");
    let Some(record) = found else { return Ok(None) };

    let v = parse_unique(record).context("index-v2 record")?;
    let o = v.as_object().ok_or_else(|| anyhow!("index-v2 record is not an object"))?;
    ensure!(o.get("schema").and_then(|s| s.as_str()) == Some("overcue-index/1")
            && o.get("keyed_by").and_then(|s| s.as_str()) == Some(ns.keyed_by()), "index-v2 record: wrong schema or library");
    let tracks = o.get("tracks").and_then(|t| t.as_object()).ok_or_else(|| anyhow!("index-v2 record has no tracks"))?;
    if tracks.len() != 1 { bail!("index-v2 record must hold one track"); }
    let e = tracks.get(&id.to_string()).ok_or_else(|| anyhow!("index-v2 record is for another track"))?;
    Entry::from_json(e, false).map(Some)
}
