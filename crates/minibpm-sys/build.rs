//! Build MiniBPM (C++), except in a browser (wasm32), where there is no C++
//! toolchain to link it with: there `cfg(minibpm)` is off and the wrapper
//! detects no tempo (src/lib.rs), so a track's grid comes from its rekordbox
//! analysis or not at all.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(minibpm)");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") { return; }
    println!("cargo::rustc-cfg=minibpm");
    cc::Build::new()
        .cpp(true)
        .include("vendor")
        .file("vendor/MiniBpm.cpp")
        .file("vendor/minibpm_c.cpp")
        .flag_if_supported("-std=c++14")
        .flag_if_supported("-O2")
        .flag_if_supported("-fno-exceptions")
        .compile("minibpm");
}
