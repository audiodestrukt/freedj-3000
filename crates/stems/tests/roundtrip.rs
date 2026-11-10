//! End to end: write a synthetic prepared stick, read it back through
//! every path (index.json, index-v2, each codec), and make sure tampering
//! anywhere is caught.

use opendeck_stems::synth::{write_stick, SynthCodec, SynthTrack};
use opendeck_stems::{v2, DirMedia, Index, Media, Namespace, Role, RoleReader};
use std::collections::BTreeMap;

/// A deterministic "track": three stems, the full mix their sum.
fn track(id: u32, path: &str, frames: usize) -> SynthTrack {
    let wave = |seed: u32, i: usize| -> i16 { ((i as u32).wrapping_mul(seed).wrapping_add(seed * 7919) % 20_001) as i16 - 10_000 };
    let mut roles = BTreeMap::new();
    for (r, seed) in [(Role::Drums, 3u32), (Role::Vocal, 5), (Role::Harmonics, 11)] {
        roles.insert(r, (0..frames * 2).map(|i| wave(seed, i) / 4).collect::<Vec<i16>>());
    }
    let full: Vec<i16> = (0..frames * 2).map(|i| roles.values().map(|v: &Vec<i16>| v[i] as i32).sum::<i32>().clamp(-32768, 32767) as i16).collect();
    roles.insert(Role::FullMix, full);
    SynthTrack { id, onelibrary_id: Some(id + 1000), file_path: path.into(), source: format!("source of {id}").into_bytes(), roles }
}

fn stick(codec: SynthCodec, v2_bits: Option<u32>) -> (tempfile::TempDir, Vec<SynthTrack>) {
    let d = tempfile::tempdir().unwrap();
    // 70 000 frames: 3 pages (32 768 frames each), the last one short.
    let tracks = vec![track(123, "/Contents/Artist/Track.mp3", 70_000), track(7, "/Contents/Ünïcode/B.flac", 1_000)];
    write_stick(d.path(), &tracks, codec, v2_bits).unwrap();
    (d, tracks)
}

#[test]
fn every_codec_round_trips() {
    for codec in [SynthCodec::Flac96, SynthCodec::Zlib96, SynthCodec::Flac44, SynthCodec::Wav44] {
        let (d, tracks) = stick(codec, None);
        let mut m = DirMedia::new(d.path());
        let ix = Index::read(&mut m).unwrap().unwrap();
        for t in &tracks {
            let e = ix.by_id(Namespace::DeviceLibrary, t.id).unwrap();
            assert_eq!(ix.by_path(&t.file_path).unwrap().unwrap(), e);
            assert_eq!(ix.by_id(Namespace::OneLibrary, t.id + 1000).unwrap(), e);
            for r in Role::ALL {
                let rr = RoleReader::open(&mut m, e, r).unwrap();
                assert_eq!(rr.read_all(&mut m).unwrap(), t.roles[&r], "{codec:?} {r:?}");
            }
        }
        // Pages stream on their own too.
        let e = ix.by_id(Namespace::DeviceLibrary, 123).unwrap();
        let rr = RoleReader::open(&mut m, e, Role::Vocal).unwrap();
        assert_eq!(rr.pages.len(), 3);
        assert_eq!(rr.page_frames(2), 70_000 - 2 * 32_768);
        assert_eq!(rr.page(&mut m, 1).unwrap(), tracks[0].roles[&Role::Vocal][32_768 * 2..65_536 * 2]);
    }
}

#[test]
fn index_v2_matches_index_json() {
    for bits in [0, 1, 8] {
        let (d, tracks) = stick(SynthCodec::Flac96, Some(bits));
        let mut m = DirMedia::new(d.path());
        let ix = Index::read(&mut m).unwrap().unwrap();
        for t in &tracks {
            let full = ix.by_id(Namespace::DeviceLibrary, t.id).unwrap();
            for (ns, id) in [(Namespace::DeviceLibrary, t.id), (Namespace::OneLibrary, t.id + 1000)] {
                let e = v2::lookup(&mut m, ns, id).unwrap().unwrap();
                assert!(e.same_binding(full), "bits {bits} {ns:?} {id}");
                assert_eq!(e.file_path, None);
            }
        }
        assert_eq!(v2::lookup(&mut m, Namespace::DeviceLibrary, 999).unwrap(), None);
    }
}

#[test]
fn no_stems_on_a_plain_stick() {
    let d = tempfile::tempdir().unwrap();
    let mut m = DirMedia::new(d.path());
    assert!(Index::read(&mut m).unwrap().is_none());
    assert!(v2::current(&mut m).unwrap().is_none());
}

/// Flip one byte at `at` in a file.
fn corrupt(path: &std::path::Path, at: usize) {
    let mut b = std::fs::read(path).unwrap();
    b[at] ^= 0x01;
    std::fs::write(path, b).unwrap();
}

#[test]
fn tampering_is_caught() {
    let (d, _) = stick(SynthCodec::Flac96, Some(4));
    let mut m = DirMedia::new(d.path());
    let e = Index::read(&mut m).unwrap().unwrap().by_id(Namespace::DeviceLibrary, 123).unwrap().clone();
    let file = d.path().join(e.role_path(Role::Drums).trim_start_matches('/'));
    let len = std::fs::metadata(&file).unwrap().len() as usize;

    // Header / table: open fails.
    corrupt(&file, 30);
    assert!(RoleReader::open(&mut m, &e, Role::Drums).is_err());
    corrupt(&file, 30);
    // Audio payload of the last page: open succeeds, that page fails, the others still read.
    corrupt(&file, len - 100);
    let rr = RoleReader::open(&mut m, &e, Role::Drums).unwrap();
    assert!(rr.page(&mut m, 0).is_ok());
    assert!(rr.page(&mut m, 2).is_err());
    assert!(rr.read_all(&mut m).is_err());
    corrupt(&file, len - 100);
    // Trailing bytes after the last page.
    let mut b = std::fs::read(&file).unwrap();
    b.push(0);
    std::fs::write(&file, &b).unwrap();
    assert!(RoleReader::open(&mut m, &e, Role::Drums).is_err());
    b.pop();
    std::fs::write(&file, &b).unwrap();
    assert!(RoleReader::open(&mut m, &e, Role::Drums).unwrap().read_all(&mut m).is_ok());

    // A damaged index-v2 bucket fails closed.
    let cur = v2::current(&mut m).unwrap().unwrap();
    let bucket = v2::bucket_path(&cur.generation, Namespace::DeviceLibrary, v2::bucket_of(123, cur.bits));
    let bfile = d.path().join(bucket.trim_start_matches('/'));
    let blen = std::fs::metadata(&bfile).unwrap().len() as usize;
    corrupt(&bfile, blen - 5);
    assert!(v2::lookup(&mut m, Namespace::DeviceLibrary, 123).is_err());

    // So does a damaged marker.
    corrupt(&d.path().join("CDJMODS/index-v2/current"), 40);
    assert!(v2::current(&mut m).is_err());
}

#[test]
fn magic_must_match_the_entry_rate() {
    // A 96 kHz entry whose file claims 44.1 kHz pages is refused.
    let (d, _) = stick(SynthCodec::Flac96, None);
    let mut m = DirMedia::new(d.path());
    let e = Index::read(&mut m).unwrap().unwrap().by_id(Namespace::DeviceLibrary, 7).unwrap().clone();
    let file = d.path().join(e.role_path(Role::Vocal).trim_start_matches('/'));
    let mut b = std::fs::read(&file).unwrap();
    b[7] = b'4';
    std::fs::write(&file, &b).unwrap();
    assert!(RoleReader::open(&mut m, &e, Role::Vocal).is_err());
    let _ = m.size("/");
}
