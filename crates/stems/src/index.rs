//! `/CDJMODS/index.json`: which bank holds a track's stems.

use crate::{media::components, Role, MAX_PAGES, PAGE_BYTES};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Which library a track ID belongs to.  The two number spaces are
/// separate: equal numbers do not mean the same song.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    /// Device Library, `PIONEER/rekordbox/export.pdb`.
    DeviceLibrary = 1,
    /// OneLibrary, `exportLibrary.db`.
    OneLibrary = 2,
}

impl Namespace {
    pub fn keyed_by(self) -> &'static str {
        match self { Namespace::DeviceLibrary => "export.pdb", Namespace::OneLibrary => "exportLibrary.db" }
    }
}

/// How a bank's audio is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// `.pgz` page files (FLAC or zlib pages; the file magic decides).
    Pages,
    /// Plain canonical WAV files (experimental 44.1 kHz CDJ-2000 profile).
    Wav,
}

/// One track's binding: where its stems are and what they must hash to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 16 lowercase hex characters: `/CDJMODS/stems/<bundle>/`.
    pub bundle: String,
    /// The exported music file, from the USB root.  Absent in index-v2 records.
    pub file_path: Option<String>,
    /// Stereo frames per role, leading silence included.
    pub frames: u64,
    /// 2: four isolated roles; 1: also three pair files (never read).
    pub three_part: u8,
    pub page_bytes: u32,
    /// 96 000 (CDJ-3000 profile) or 44 100 (experimental CDJ-2000 profiles).
    pub sample_rate: u32,
    pub layout: Layout,
    /// SHA-256 of the original music file, when recorded.
    pub source_sha256: Option<[u8; 32]>,
    /// Per role: SHA-256 of the expanded PCM, and of the header + page table.
    pub hashes: BTreeMap<Role, ([u8; 32], [u8; 32])>,
}

impl Entry {
    /// Path of a role's file.
    pub fn role_path(&self, role: Role) -> String {
        let ext = match self.layout { Layout::Pages => "pgz", Layout::Wav => "wav" };
        format!("/CDJMODS/stems/{}/stems-sidecar-{}.s16le.{ext}", self.bundle, role.file_name())
    }

    /// Bytes each role expands to (`frames * 4`).
    pub fn role_bytes(&self) -> u64 { self.frames * 4 }

    /// The fields that bind an entry to its audio; two entries for one song
    /// (Device Library and OneLibrary) must agree on these.
    pub fn same_binding(&self, other: &Entry) -> bool {
        (&self.bundle, self.frames, self.three_part, self.page_bytes, self.sample_rate, self.layout, &self.source_sha256, &self.hashes)
            == (&other.bundle, other.frames, other.three_part, other.page_bytes, other.sample_rate, other.layout, &other.source_sha256, &other.hashes)
    }

    /// Validate one index entry.  `with_path`: index.json entries carry
    /// `file_path`; index-v2 records do not.
    pub fn from_json(v: &Value, with_path: bool) -> Result<Entry> {
        let o = v.as_object().ok_or_else(|| anyhow!("entry is not an object"))?;
        let bundle = o.get("bundle").and_then(Value::as_str).unwrap_or_default();
        if bundle.len() != 16 || !bundle.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            bail!("invalid bundle name {bundle:?}");
        }
        let file_path = match o.get("file_path") {
            Some(Value::String(p)) => { components(p).with_context(|| "invalid file_path")?; Some(p.clone()) }
            Some(_) => bail!("file_path is not a string"),
            None if with_path => bail!("entry has no file_path"),
            None => None,
        };
        let frames = o.get("frames").and_then(Value::as_u64).filter(|f| *f > 0)
            .ok_or_else(|| anyhow!("invalid frames"))?;
        if frames > MAX_PAGES as u64 * PAGE_BYTES as u64 / 4 { bail!("{frames} frames exceeds {MAX_PAGES} pages"); }
        let three_part = match o.get("three_part").and_then(Value::as_u64) {
            Some(n @ (1 | 2)) => n as u8,
            _ => bail!("unsupported three_part"),
        };
        if o.get("page_bytes").and_then(Value::as_u64) != Some(PAGE_BYTES as u64) { bail!("page_bytes must be {PAGE_BYTES}"); }
        let sample_rate = match o.get("sample_rate") {
            None => 96_000,
            // OverCue Desktop writes the 96 kHz default out explicitly
            // ("sample_rate": 96000, page_codec "flac-96k"), as seen on a
            // real prepared stick; older writers leave it out.
            Some(v) => match v.as_u64() {
                Some(96_000) => 96_000,
                Some(44_100) => 44_100,
                _ => bail!("unsupported sample_rate {v}"),
            },
        };
        let layout = if o.get("page_codec").and_then(Value::as_str) == Some("wav-44k1") { Layout::Wav } else { Layout::Pages };
        if layout == Layout::Wav && sample_rate != 44_100 { bail!("WAV roles must be 44.1 kHz"); }
        let digest = |k: &str| -> Result<[u8; 32]> {
            let s = o.get(k).and_then(Value::as_str).ok_or_else(|| anyhow!("missing {k}"))?;
            parse_digest(s).ok_or_else(|| anyhow!("invalid {k}"))
        };
        let source_sha256 = if o.contains_key("source_sha256") { Some(digest("source_sha256")?) } else { None };
        let mut hashes = BTreeMap::new();
        for r in Role::ALL {
            hashes.insert(r, (digest(&format!("{}_sha256", r.key()))?, digest(&format!("{}_page_table_sha256", r.key()))?));
        }
        Ok(Entry { bundle: bundle.to_string(), file_path, frames, three_part, page_bytes: PAGE_BYTES, sample_rate, layout, source_sha256, hashes })
    }
}

/// 64 lowercase hex characters → 32 bytes.
pub(crate) fn parse_digest(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return None; }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() { *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?; }
    Some(out)
}

/// The parsed `index.json`.
#[derive(Debug, Default)]
pub struct Index {
    pub device: BTreeMap<u32, Entry>,
    pub onelibrary: BTreeMap<u32, Entry>,
}

impl Index {
    /// Read `/CDJMODS/index.json`; `None` when the stick has none.
    pub fn read(media: &mut dyn crate::Media) -> Result<Option<Index>> {
        match media.read_all("/CDJMODS/index.json", crate::INDEX_MAX)? {
            Some(bytes) => Index::parse(&bytes).map(Some),
            None => Ok(None),
        }
    }

    /// Parse and validate.  Duplicate JSON members are refused, as are
    /// unknown schemas.  An entry that fails validation fails the index:
    /// a damaged index means "repair it in Desktop", not "guess".
    pub fn parse(bytes: &[u8]) -> Result<Index> {
        let v = parse_unique(bytes)?;
        let o = v.as_object().ok_or_else(|| anyhow!("index is not an object"))?;
        if o.get("schema").and_then(Value::as_str) != Some("overcue-index/1") { bail!("unknown index schema"); }
        let empty = Map::new();
        let map = |k: &str| -> Result<&Map<String, Value>> {
            match o.get(k) { None => Ok(&empty), Some(Value::Object(m)) => Ok(m), Some(_) => bail!("{k} is not an object") }
        };
        // An older OneLibrary-keyed index puts its OneLibrary IDs in `tracks`.
        let (dev, one) = match o.get("keyed_by").and_then(Value::as_str) {
            None | Some("export.pdb") => (map("tracks")?, map("tracks_onelibrary")?),
            Some("exportLibrary.db") => {
                if o.contains_key("tracks") && o.contains_key("tracks_onelibrary") { bail!("ambiguous OneLibrary maps"); }
                (&empty, if o.contains_key("tracks_onelibrary") { map("tracks_onelibrary")? } else { map("tracks")? })
            }
            Some(k) => bail!("unknown keyed_by {k:?}"),
        };
        let entries = |m: &Map<String, Value>| -> Result<BTreeMap<u32, Entry>> {
            m.iter().map(|(k, v)| {
                let id: u32 = k.parse::<u32>().ok().filter(|id| *id > 0 && k == &id.to_string())
                    .ok_or_else(|| anyhow!("invalid track ID {k:?}"))?;
                Ok((id, Entry::from_json(v, true).with_context(|| format!("track {id}"))?))
            }).collect()
        };
        Ok(Index { device: entries(dev)?, onelibrary: entries(one)? })
    }

    /// An entry by library ID.
    pub fn by_id(&self, ns: Namespace, id: u32) -> Option<&Entry> {
        match ns { Namespace::DeviceLibrary => self.device.get(&id), Namespace::OneLibrary => self.onelibrary.get(&id) }
    }

    /// An entry by exact `file_path`, across both libraries.  Matches whose
    /// bindings disagree are an error (ambiguous), never a pick.
    pub fn by_path(&self, path: &str) -> Result<Option<&Entry>> {
        let mut hits = self.device.values().chain(self.onelibrary.values())
            .filter(|e| e.file_path.as_deref() == Some(path));
        let Some(first) = hits.next() else { return Ok(None) };
        if hits.any(|e| !e.same_binding(first)) { bail!("{path}: ambiguous stems entries"); }
        Ok(Some(first))
    }
}

/// Parse JSON, refusing duplicate object members anywhere.
pub(crate) fn parse_unique(bytes: &[u8]) -> Result<Value> {
    use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};
    struct Unique(Value);
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Unique;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("JSON") }
                fn visit_bool<E>(self, b: bool) -> Result<Unique, E> { Ok(Unique(Value::Bool(b))) }
                fn visit_i64<E>(self, n: i64) -> Result<Unique, E> { Ok(Unique(Value::from(n))) }
                fn visit_u64<E>(self, n: u64) -> Result<Unique, E> { Ok(Unique(Value::from(n))) }
                fn visit_f64<E>(self, n: f64) -> Result<Unique, E> { Ok(Unique(Value::from(n))) }
                fn visit_str<E>(self, s: &str) -> Result<Unique, E> { Ok(Unique(Value::String(s.to_owned()))) }
                fn visit_string<E>(self, s: String) -> Result<Unique, E> { Ok(Unique(Value::String(s))) }
                fn visit_unit<E>(self) -> Result<Unique, E> { Ok(Unique(Value::Null)) }
                fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                    let mut v = Vec::new();
                    while let Some(Unique(x)) = a.next_element()? { v.push(x); }
                    Ok(Unique(Value::Array(v)))
                }
                fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                    let mut m = Map::new();
                    while let Some((k, Unique(x))) = a.next_entry::<String, Unique>()? {
                        if m.insert(k.clone(), x).is_some() { return Err(A::Error::custom(format!("duplicate JSON member {k:?}"))); }
                    }
                    Ok(Unique(Value::Object(m)))
                }
            }
            d.deserialize_any(V)
        }
    }
    let s = std::str::from_utf8(bytes).context("index is not UTF-8")?;
    let Unique(v) = serde_json::from_str(s).context("index JSON")?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_json(bundle: &str, path: &str) -> String {
        let h = "ab".repeat(32);
        let mut s = format!(r#"{{"bundle":"{bundle}","file_path":"{path}","frames":1000,"three_part":2,"page_bytes":131072"#);
        for r in Role::ALL { s += &format!(r#","{k}_sha256":"{h}","{k}_page_table_sha256":"{h}""#, k = r.key()); }
        s + "}"
    }

    /// The fields OverCue Desktop wrote for a track on a real stick
    /// (CDJ2, 2026-11-10): an explicit 96 kHz rate and its codec tags.
    #[test]
    fn parses_an_explicit_96k_entry_as_overcue_writes_it() {
        let e = entry_json("e3b4246cea36a5e5", "/Contents/Davani/Brand New Funk/002 - Davani - Brand New Funk.mp3");
        let e = e.trim_end_matches('}').to_string()
            + r#","sample_rate":96000,"page_codec":"flac-96k","preparation_profile":"cdj3000-96k/1","separation":"bs-roformer/overcue-stems-v2","title":"Brand New Funk"}"#;
        let j = format!(r#"{{"schema":"overcue-index/1","keyed_by":"export.pdb","tracks":{{"2":{e}}},"tracks_onelibrary":{{}}}}"#);
        let ix = Index::parse(j.as_bytes()).unwrap();
        let got = ix.by_id(Namespace::DeviceLibrary, 2).unwrap();
        assert_eq!(got.sample_rate, 96_000);
        assert_eq!(got.layout, Layout::Pages);
        let bad = j.replace("96000", "48000");
        assert!(Index::parse(bad.as_bytes()).is_err(), "other rates are still refused");
    }

    #[test]
    fn parses_both_libraries_and_finds_by_path() {
        let j = format!(r#"{{"schema":"overcue-index/1","keyed_by":"export.pdb","tracks":{{"123":{}}},"tracks_onelibrary":{{"9":{}}}}}"#,
            entry_json("0123456789abcdef", "/Contents/A/T.mp3"), entry_json("0123456789abcdef", "/Contents/A/T.mp3"));
        let ix = Index::parse(j.as_bytes()).unwrap();
        let e = ix.by_id(Namespace::DeviceLibrary, 123).unwrap();
        assert_eq!(e.sample_rate, 96_000);
        assert_eq!(e.role_path(Role::FullMix), "/CDJMODS/stems/0123456789abcdef/stems-sidecar-full-mix.s16le.pgz");
        assert!(ix.by_id(Namespace::DeviceLibrary, 9).is_none(), "number spaces are separate");
        assert_eq!(ix.by_path("/Contents/A/T.mp3").unwrap().unwrap().bundle, "0123456789abcdef");
        assert!(ix.by_path("/Contents/other.mp3").unwrap().is_none());
    }

    #[test]
    fn disagreeing_entries_for_one_path_are_ambiguous() {
        let j = format!(r#"{{"schema":"overcue-index/1","tracks":{{"1":{},"2":{}}}}}"#,
            entry_json("0123456789abcdef", "/C/t.mp3"), entry_json("fedcba9876543210", "/C/t.mp3"));
        assert!(Index::parse(j.as_bytes()).unwrap().by_path("/C/t.mp3").is_err());
    }

    #[test]
    fn refuses_bad_input() {
        let ok = entry_json("0123456789abcdef", "/C/t.mp3");
        let cases = [
            r#"{"schema":"overcue-index/2","tracks":{}}"#.to_string(),
            r#"{"schema":"overcue-index/1","schema":"overcue-index/1","tracks":{}}"#.to_string(),
            format!(r#"{{"schema":"overcue-index/1","tracks":{{"1":{ok},"1":{ok}}}}}"#),
            format!(r#"{{"schema":"overcue-index/1","tracks":{{"01":{ok}}}}}"#),
            format!(r#"{{"schema":"overcue-index/1","tracks":{{"1":{}}}}}"#, ok.replace("0123456789abcdef", "0123456789ABCDEF")),
            format!(r#"{{"schema":"overcue-index/1","tracks":{{"1":{}}}}}"#, ok.replace("/C/t.mp3", "/C/../../etc/passwd")),
            format!(r#"{{"schema":"overcue-index/1","tracks":{{"1":{}}}}}"#, ok.replace("131072", "65536")),
            format!(r#"{{"schema":"overcue-index/1","tracks":{{"1":{}}}}}"#, ok.replace(r#""three_part":2"#, r#""three_part":3"#)),
            format!(r#"{{"schema":"overcue-index/1","tracks":{{"1":{}}}}}"#, ok.replace(r#""frames":1000"#, r#""frames":0"#)),
        ];
        for c in cases { assert!(Index::parse(c.as_bytes()).is_err(), "{c}"); }
    }

    #[test]
    fn older_onelibrary_keyed_index() {
        let j = format!(r#"{{"schema":"overcue-index/1","keyed_by":"exportLibrary.db","tracks":{{"5":{}}}}}"#, entry_json("0123456789abcdef", "/C/t.mp3"));
        let ix = Index::parse(j.as_bytes()).unwrap();
        assert!(ix.by_id(Namespace::OneLibrary, 5).is_some());
        assert!(ix.by_id(Namespace::DeviceLibrary, 5).is_none());
    }
}
