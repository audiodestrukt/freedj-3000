//! Link Rubber Band.
//!
//! Two ways to get it, tried in order:
//!
//!  1. **System library** (`librubberband-dev` via pkg-config).  This is what
//!     the Linux desktop / Raspberry Pi builds have always used, so it stays
//!     the first choice — no change to those builds.
//!  2. **Vendored source** (`third_party/rubberband`), compiled here into a
//!     static `librubberband.a` via the `cc` crate.  This is the path iOS and
//!     macOS take: there is no system librubberband on either, and pkg-config
//!     is deliberately inert when cross-compiling.
//!
//! Rubber Band ships `single/RubberBandSingle.cpp`, one translation unit that
//! `#include`s every source file (the C API among them) and selects a
//! dependency-free configuration: built-in resampler, no threading, and on
//! Apple platforms the vDSP FFT — which is why Apple targets also need the
//! Accelerate framework.
//!
//! Note for the iOS app: cargo's link directives do NOT propagate out of a
//! staticlib. Frameworks needed here (Accelerate) must ALSO be listed in the
//! Xcode target's OTHER_LDFLAGS — see ios/freedj.xcodeproj.

use std::path::PathBuf;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let apple = matches!(target_os.as_str(), "ios" | "macos" | "tvos" | "watchos" | "visionos");

    // 1. System library.  Skipped on Apple: pkg-config would at best find a
    //    Homebrew macOS build, which is wrong for an iOS link.
    if !apple {
        match pkg_config::Config::new().atleast_version("3.0.0").probe("rubberband") {
            Ok(_) => {
                // pkg-config emitted the right -L / -l flags automatically.
                println!("cargo:rustc-link-lib=stdc++");
                return;
            }
            Err(e) => println!("cargo:warning=pkg-config found no rubberband ({e}); building the vendored copy"),
        }
    }

    // 2. Vendored source.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party/rubberband")
        .canonicalize()
        .expect("third_party/rubberband missing — see crates/timestretch/build.rs");
    let single = root.join("single/RubberBandSingle.cpp");
    assert!(single.exists(), "{} missing", single.display());

    // RubberBandSingle.cpp includes the other .cpp files by relative path, so
    // only the one file is compiled — but any of them changing must rebuild it.
    println!("cargo:rerun-if-changed={}", root.join("src").display());
    println!("cargo:rerun-if-changed={}", root.join("rubberband").display());
    println!("cargo:rerun-if-changed={}", single.display());

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++14")
        .file(&single)
        .include(&root)
        .include(root.join("src"))
        .opt_level(3)
        // Rubber Band's own sources are not warning-clean under our flags and
        // we do not maintain them; silence the noise rather than patch a
        // vendored tree.
        .warnings(false)
        .flag_if_supported("-ffast-math");
    build.compile("rubberband");   // → librubberband.a, matching #[link(name = "rubberband")]

    if apple {
        // RubberBandSingle.cpp sets HAVE_VDSP on Apple.
        println!("cargo:rustc-link-lib=framework=Accelerate");
    }
    // cc emits the C++ standard library link flag itself (-lc++ / -lstdc++).
}
