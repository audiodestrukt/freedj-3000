//! Builds synthetic prepared sticks for tests: our own writer of the open
//! format, so tests need no music and no OverCue install.  Its output is
//! checked against OverCue's reference validator (`verify_usb.py`) by
//! `examples/synth_stick.rs`.  Never used on a real stick.

use crate::pgz::wav_header;
use crate::v2::bucket_of;
use crate::{hex, sha256, Role, PAGE_BYTES};
use anyhow::Result;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// How the synthetic bank stores audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynthCodec { Flac96, Zlib96, Flac44, Wav44 }

impl SynthCodec {
    fn rate(self) -> u32 { match self { SynthCodec::Flac96 | SynthCodec::Zlib96 => 96_000, _ => 44_100 } }
}

/// One track to prepare.
pub struct SynthTrack {
    /// Device Library (export.pdb) ID.
    pub id: u32,
    /// Also listed under this OneLibrary ID, when set.
    pub onelibrary_id: Option<u32>,
    /// Where the "original" lives, from the USB root.
    pub file_path: String,
    /// The original file's bytes (written at `file_path`, hashed into the index).
    pub source: Vec<u8>,
    /// Interleaved stereo i16 per role; all the same length.
    pub roles: BTreeMap<Role, Vec<i16>>,
}

/// Write a prepared stick under `root`: the originals, the banks,
/// `index.json`, and `index-v2` with `v2_bits` bucket bits when given.
pub fn write_stick(root: &Path, tracks: &[SynthTrack], codec: SynthCodec, v2_bits: Option<u32>) -> Result<()> {
    let mut index_dev = Vec::new();
    let mut index_one = Vec::new();
    type Rows = Vec<(u32, Vec<u8>)>; // (track ID, record)
    let mut v2: BTreeMap<(u32, u32), Rows> = BTreeMap::new(); // keyed by (namespace, bucket)
    for t in tracks {
        let src = root.join(t.file_path.trim_start_matches('/'));
        std::fs::create_dir_all(src.parent().unwrap())?;
        std::fs::write(&src, &t.source)?;

        let full = &t.roles[&Role::FullMix];
        let frames = full.len() / 2;
        let bundle = hex(&sha256(&[&t.id.to_be_bytes()[..], &sha256(&as_bytes(full))[..]].concat())[..8]);
        let dir = root.join("CDJMODS/stems").join(&bundle);
        std::fs::create_dir_all(&dir)?;
        let mut hashes = String::new();
        for r in Role::ALL {
            let pcm = as_bytes(&t.roles[&r]);
            assert_eq!(pcm.len(), frames * 4, "roles must share a length");
            let (file, table_hash) = match codec {
                SynthCodec::Wav44 => {
                    let mut f = wav_header(pcm.len() as u32).to_vec();
                    let h = sha256(&f);
                    f.extend_from_slice(&pcm);
                    (f, h)
                }
                _ => pgz(&pcm, codec),
            };
            let ext = if codec == SynthCodec::Wav44 { "wav" } else { "pgz" };
            std::fs::write(dir.join(format!("stems-sidecar-{}.s16le.{ext}", r.file_name())), &file)?;
            write!(hashes, r#","{k}_sha256":"{}","{k}_page_table_sha256":"{}""#, hex(&sha256(&pcm)), hex(&table_hash), k = r.key())?;
        }
        let rate = match codec {
            SynthCodec::Flac44 => r#","sample_rate":44100,"page_codec":"flac-44k1""#,
            SynthCodec::Wav44 => r#","sample_rate":44100,"page_codec":"wav-44k1""#,
            SynthCodec::Flac96 => r#","page_codec":"flac-96k""#,
            SynthCodec::Zlib96 => r#","page_codec":"zlib""#,
        };
        let binding = format!(r#""bundle":"{bundle}","frames":{frames},"three_part":2,"page_bytes":{PAGE_BYTES}"#);
        let src_hash = format!(r#","source_sha256":"{}""#, hex(&sha256(&t.source)));
        let entry = format!(r#"{{{binding},"file_path":{}{rate}{src_hash}{hashes}}}"#, serde_json::to_string(&t.file_path)?);
        index_dev.push(format!(r#""{}":{entry}"#, t.id));
        // index-v2 records keep only the binding fields, in index order.
        let rec = |ns: &str, id: u32| format!(r#"{{"schema":"overcue-index/1","keyed_by":"{ns}","tracks":{{"{id}":{{{binding}{src_hash}{hashes}}}}}}}"#).into_bytes();
        if let Some(bits) = v2_bits {
            v2.entry((1, bucket_of(t.id, bits))).or_default().push((t.id, rec("export.pdb", t.id)));
        }
        if let Some(one) = t.onelibrary_id {
            index_one.push(format!(r#""{one}":{entry}"#));
            if let Some(bits) = v2_bits {
                v2.entry((2, bucket_of(one, bits))).or_default().push((one, rec("exportLibrary.db", one)));
            }
        }
    }
    let index = format!(r#"{{"schema":"overcue-index/1","keyed_by":"export.pdb","tracks":{{{}}},"tracks_onelibrary":{{{}}}}}"#,
                        index_dev.join(","), index_one.join(","));
    std::fs::write(root.join("CDJMODS/index.json"), index)?;

    if let Some(bits) = v2_bits {
        let generation = [0x5a; 16];
        let gdir = root.join("CDJMODS/index-v2").join(hex(&generation));
        for ns in [1, 2] { std::fs::create_dir_all(gdir.join(ns.to_string()))?; }
        for ((ns, bucket), mut rows) in v2 {
            rows.sort_by_key(|r| r.0);
            let table_len = 96 + 48 * rows.len();
            let size = table_len + rows.iter().map(|r| r.1.len()).sum::<usize>();
            let mut head = Vec::with_capacity(40);
            head.extend_from_slice(b"OCSIBK02");
            head.extend_from_slice(&generation);
            for v in [ns, bucket, rows.len() as u32, size as u32] { head.extend_from_slice(&v.to_be_bytes()); }
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
            let p = gdir.join(ns.to_string()).join(format!("{:02X}", bucket >> 16)).join(format!("{:02X}", (bucket >> 8) & 0xff));
            std::fs::create_dir_all(&p)?;
            std::fs::write(p.join(format!("{:02X}.IDX", bucket & 0xff)), file)?;
        }
        let mut cur = b"OCSIDX02".to_vec();
        cur.extend_from_slice(&generation);
        cur.extend_from_slice(&1u32.to_be_bytes());
        cur.extend_from_slice(&bits.to_be_bytes());
        let h = sha256(&cur);
        cur.extend_from_slice(&h);
        std::fs::write(root.join("CDJMODS/index-v2/current"), cur)?;
    }
    Ok(())
}

fn as_bytes(s: &[i16]) -> Vec<u8> { s.iter().flat_map(|v| v.to_le_bytes()).collect() }

/// A `.pgz` file and the SHA-256 of its header + page table.
fn pgz(pcm: &[u8], codec: SynthCodec) -> (Vec<u8>, [u8; 32]) {
    let pages: Vec<&[u8]> = pcm.chunks(PAGE_BYTES as usize).collect();
    let payloads: Vec<Vec<u8>> = pages.iter().map(|p| match codec {
        SynthCodec::Zlib96 => {
            use std::io::Write;
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(p).unwrap();
            z.finish().unwrap()
        }
        _ => flac_verbatim(p, codec.rate()),
    }).collect();
    let magic: &[u8] = match codec { SynthCodec::Zlib96 => b"OVPGZ001", SynthCodec::Flac96 => b"OVPGZ003", _ => b"OVPGZ004" };
    let mut f = magic.to_vec();
    f.extend_from_slice(&PAGE_BYTES.to_be_bytes());
    f.extend_from_slice(&(pages.len() as u32).to_be_bytes());
    f.extend_from_slice(&(pcm.len() as u64).to_be_bytes());
    let mut off = 24 + 48 * pages.len() as u64;
    for (p, c) in pages.iter().zip(&payloads) {
        f.extend_from_slice(&off.to_be_bytes());
        f.extend_from_slice(&(c.len() as u32).to_be_bytes());
        f.extend_from_slice(&(p.len() as u32).to_be_bytes());
        f.extend_from_slice(&sha256(p));
        off += c.len() as u64;
    }
    let table_hash = sha256(&f);
    for c in &payloads { f.extend_from_slice(c); }
    (f, table_hash)
}

/// A minimal valid FLAC stream: fixed 4096-frame blocks, stereo, 16-bit,
/// verbatim subframes (no compression).  Enough to exercise a real FLAC
/// decoder on the read side.
pub fn flac_verbatim(pcm: &[u8], rate: u32) -> Vec<u8> {
    const BLOCK: usize = 4096;
    let frames = pcm.len() / 4;
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0, 0, 34]); // last metadata block, STREAMINFO, 34 bytes
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&[0; 6]); // frame sizes unknown
    let info: u64 = (rate as u64) << 44 | 1 << 41 | 15 << 36 | frames as u64;
    out.extend_from_slice(&info.to_be_bytes());
    out.extend_from_slice(&[0; 16]); // MD5 unknown
    let rate_code = match rate { 96_000 => 0xB, 44_100 => 0x9, 48_000 => 0xA, _ => panic!("rate") };
    for (n, block) in pcm.chunks(BLOCK * 4).enumerate() {
        let len = block.len() / 4;
        let mut fr = vec![0xFF, 0xF8, 0x70 | rate_code, 0x18];
        utf8_number(n as u32, &mut fr);
        fr.extend_from_slice(&((len - 1) as u16).to_be_bytes());
        fr.push(crc8(&fr));
        for ch in 0..2 {
            fr.push(0x02); // verbatim subframe
            for f in 0..len {
                let s = i16::from_le_bytes([block[f * 4 + ch * 2], block[f * 4 + ch * 2 + 1]]);
                fr.extend_from_slice(&s.to_be_bytes());
            }
        }
        let c = crc16(&fr);
        fr.extend_from_slice(&c.to_be_bytes());
        out.extend_from_slice(&fr);
    }
    out
}

fn utf8_number(n: u32, out: &mut Vec<u8>) {
    if n < 0x80 { out.push(n as u8); return; }
    let mut bytes = vec![];
    let mut v = n;
    while v >= (1 << (6 - bytes.len())) { bytes.push(0x80 | (v & 0x3f) as u8); v >>= 6; }
    let lead_bits = bytes.len() + 1;
    out.push(((0xFF00u16 >> lead_bits) as u8) | v as u8);
    out.extend(bytes.iter().rev());
}

fn crc8(d: &[u8]) -> u8 {
    d.iter().fold(0u8, |mut c, b| { c ^= b; for _ in 0..8 { c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 }; } c })
}

fn crc16(d: &[u8]) -> u16 {
    d.iter().fold(0u16, |mut c, b| { c ^= (*b as u16) << 8; for _ in 0..8 { c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 }; } c })
}
