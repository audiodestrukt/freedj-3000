//! FFI to the C shim over Bungee (`csrc/bungee_shim.cpp`), built by build.rs
//! from the vendored `third_party/bungee` when the `bungee` feature is on.
#![allow(dead_code)]

use std::os::raw::{c_char, c_double, c_int, c_void};

#[link(name = "opendeck_bungee", kind = "static")]
extern "C" {
    pub fn od_bungee_create(sample_rate: c_int, channels: c_int, max_input_frames: c_int) -> *mut c_void;
    pub fn od_bungee_destroy(p: *mut c_void);
    pub fn od_bungee_version() -> *const c_char;
    pub fn od_bungee_process(
        p: *mut c_void,
        input: *const *const f32,
        output: *const *mut f32,
        in_frames: c_int,
        speed: c_double,
        pitch: c_double,
        capacity: c_int,
    ) -> c_int;
    pub fn od_bungee_latency(p: *mut c_void) -> c_double;
    pub fn od_bungee_reset(p: *mut c_void);
}
