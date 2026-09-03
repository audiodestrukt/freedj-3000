fn main() {
    // APP_VERSION (screen.rs) is baked in with option_env!; recompile when the
    // iOS build script changes it.
    println!("cargo:rerun-if-env-changed=OPENDECK_VERSION");
}
