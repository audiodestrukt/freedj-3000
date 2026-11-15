//! Layer-by-layer agreement with Meta's PyTorch HTDemucs on a real chunk.
//!
//!   uv run … python tools/demucs/export_htdemucs.py DIR track.wav
//!   OPENDECK_DEMUCS_DIR=DIR cargo test -p opendeck-demucs --release -- --ignored --nocapture
//!
//! DIR defaults to ~/.local/share/opendeck-test/models/htdemucs-native.

use opendeck_demucs::{HtDemucs, Tensors, SEGMENT};
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::var_os("OPENDECK_DEMUCS_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share/opendeck-test/models/htdemucs-native")
    })
}

/// Agreement in dB: energy of the reference over energy of the difference.
fn agreement(want: &[f32], got: &[f32]) -> f64 {
    let (mut s, mut e) = (0f64, 0f64);
    for (a, b) in want.iter().zip(got) { s += (*a as f64).powi(2); e += (*a as f64 - *b as f64).powi(2) }
    10.0 * (s / e.max(1e-30)).log10()
}

#[test]
#[ignore]
fn matches_pytorch_layer_by_layer() {
    let d = dir();
    let model = HtDemucs::load(&d.join("htdemucs.safetensors")).expect("weights (run tools/demucs/export_htdemucs.py)");
    let reference = Tensors::open(&d.join("htdemucs-ref.safetensors")).expect("reference run");
    let (_, mix) = reference.get("mix").unwrap();
    assert_eq!(mix.len(), 2 * SEGMENT);
    let mut out = vec![0f32; 8 * SEGMENT];
    let mut worst = f64::INFINITY;
    let t0 = std::time::Instant::now();
    model.run_tapped(mix, &mut out, Some(&mut |name: &str, shape: &[usize], got: &[f32]| {
        let (rs, want) = reference.get(name).unwrap_or_else(|| panic!("no reference for {name}"));
        assert_eq!(&rs[1..], shape, "{name} shape");
        let a = agreement(want, got);
        println!("{name:8} {a:6.1} dB");
        if name != "spec" { worst = worst.min(a) }
    }));
    println!("one chunk in {:.2} s", t0.elapsed().as_secs_f64());
    let (_, stems) = reference.get("stems").unwrap();
    for (s, name) in opendeck_demucs::SOURCES.iter().enumerate() {
        let r = s * 2 * SEGMENT..(s + 1) * 2 * SEGMENT;
        let a = agreement(&stems[r.clone()], &out[r]);
        println!("{name:8} {a:6.1} dB vs PyTorch");
        // 102-123 dB when written (ONNX Runtime's export: 80-86, 56 on vocals).
        assert!(a > 95.0, "{name}: {a:.1} dB");
    }
    assert!(worst > 95.0, "a stage drifted to {worst:.1} dB");
}
