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
    if std::env::var_os("CARGO_FEATURE_BUNGEE").is_some() {
        build_bungee();
    }
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
    // On Windows, canonicalize returns a `\\?\` verbatim path that cl.exe
    // mishandles when it is both an -I include and the source file argument,
    // reducing the source arg to `\\<name>`. Strip the prefix.
    #[cfg(windows)]
    let root = {
        let s = root.to_string_lossy();
        PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
    };
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
    // On Windows, src/common/Thread.h pulls in <windows.h>, whose min/max
    // macros collide with std::min / std::numeric_limits::max() throughout
    // the Rubber Band sources. NOMINMAX suppresses the macros.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        build.define("NOMINMAX", None);
        build.define("WIN32_LEAN_AND_MEAN", None);
    }
    build.compile("rubberband");   // → librubberband.a, matching #[link(name = "rubberband")]

    if apple {
        // RubberBandSingle.cpp sets HAVE_VDSP on Apple.
        println!("cargo:rustc-link-lib=framework=Accelerate");
    }
    // cc emits the C++ standard library link flag itself (-lc++ / -lstdc++).
    // 32-bit ARM (Pi Zero W / Pi 1, ARMv6) has no native 64-bit atomics, so
    // Rubber Band's std::atomic<double> needs libatomic's helpers.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("arm") {
        println!("cargo:rustc-link-lib=atomic");
    }
}

/// Bungee (third_party/bungee, MPL-2.0): pffft as C, Bungee's sources plus
/// our C shim as C++20, into one static library `opendeck_bungee`.  Always
/// vendored: no distro ships it and it is small (~0.3 s to compile).
fn build_bungee() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/bungee");
    #[cfg(windows)]
    let root = {
        let s = root.canonicalize().unwrap().to_string_lossy().to_string();
        PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
    };
    assert!(root.join("bungee/Bungee.h").exists(), "third_party/bungee missing — see third_party/bungee/VENDORED.md");
    let shim = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("csrc/bungee_shim.cpp");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed={}", shim.display());

    let mut c = cc::Build::new();
    c.file(root.join("pffft/pffft.c"))
        .file(root.join("pffft/fftpack.c"))
        .include(root.join("pffft"))
        .define("_USE_MATH_DEFINES", None) // M_PI under MSVC's <math.h>
        .opt_level(3)
        .warnings(false)
        .flag_if_supported("-ffast-math")
        .flag_if_supported("-fno-finite-math-only")
        .flag_if_supported("-fno-exceptions");
    c.compile("opendeck_pffft");

    let mut cpp = cc::Build::new();
    cpp.cpp(true)
        .std("c++20")
        .include(&root)
        .include(root.join("eigen"))
        .include(root.join("pffft"))
        .define("BUNGEE_SELF_TEST", "0")
        .define("eigen_assert", "BUNGEE_ASSERT1")
        .define("EIGEN_DONT_PARALLELIZE", "1")
        .define("BUNGEE_VERSION", "\"vendored\"")
        .opt_level(3)
        .warnings(false)
        .flag_if_supported("-fwrapv")
        .file(&shim);
    for ent in std::fs::read_dir(root.join("src")).expect("third_party/bungee/src") {
        let path = ent.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("cpp") {
            cpp.file(path);
        }
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        cpp.define("NOMINMAX", None);
        cpp.define("WIN32_LEAN_AND_MEAN", None);
    }
    cpp.compile("opendeck_bungee");
}
