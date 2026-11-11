//! Android entry point: a NativeActivity app with no Java code of its own.
//!
//! The APK is put together by `deploy/android/build.sh` (cargo-ndk + aapt2,
//! no Gradle).  `android_main` runs on the activity's native thread, sets up
//! the folders and platform services the desktop build gets from the OS, then
//! calls the same `run()` as every other platform.
//!
//! Folders:
//! - app data (settings, tag list, grids, Link cache): the app's internal
//!   files dir, by pointing HOME at it (see `taglist::app_data_dir`).
//! - music: the shared `Music` folder.  With the audio-read permission the
//!   BROWSE screen lists every track in it; without it only the files this app
//!   put there (the demo track, in `Music/OpenDeck`).
//!
//! Platform calls go through JNI on the activity object.  Everything here is
//! best effort: a failed call logs and the deck runs without it.

use crate::{run, Config};
use jni::objects::{JObject, JValue};
use jni::JavaVM;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use winit::platform::android::activity::AndroidApp;

/// The activity handle, for `run()` to build its event loop on.
pub(crate) static APP: OnceLock<AndroidApp> = OnceLock::new();

/// Shared storage root (Environment.getExternalStorageDirectory on every
/// device that ships one user profile; secondary users get their own mount,
/// which this does not handle).
const SHARED: &str = "/storage/emulated/0";

#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("opendeck")
            .with_filter(android_logger::FilterBuilder::new().parse("info,wgpu=warn,naga=warn").build()),
    );
    std::panic::set_hook(Box::new(|p| log::error!("panic: {p}")));

    // Dev: Android launches apps with no environment of their own, so the
    // desktop's OPENDECK_* / WGPU_* switches come in through a system property:
    //   adb shell setprop debug.opendeck.env "WGPU_BACKEND=gl OPENDECK_IDLE_FPS=0"
    for kv in system_property("debug.opendeck.env").split_whitespace() {
        if let Some((k, v)) = kv.split_once('=') {
            log::info!("env from debug.opendeck.env: {k}={v}");
            std::env::set_var(k, v);
        }
    }

    // App data under the internal files dir: taglist::app_data_dir reads HOME.
    if let Some(dir) = app.internal_data_path() {
        std::env::set_var("HOME", &dir);
        log::info!("app data: {}", dir.display());
    }

    let music = PathBuf::from(SHARED).join("Music");
    let _ = crate::taglist::ANDROID_MUSIC.set(music.clone());

    let sdk = app.config().sdk_version();
    if let Err(e) = with_activity(&app, |env, act| {
        request_storage(env, act, sdk)?;
        multicast_lock(env, act)?;
        lock_orientation(env, act, &app)
    }) {
        log::warn!("android setup: {e}");
    }
    // A DJ deck must not lock mid-set; NativeActivity applies the flag on the
    // UI thread for us.
    app.set_window_flags(
        winit::platform::android::activity::WindowManagerFlags::KEEP_SCREEN_ON
            | winit::platform::android::activity::WindowManagerFlags::FULLSCREEN,
        winit::platform::android::activity::WindowManagerFlags::empty(),
    );

    // Same split as iOS: portrait deck with its faceplate on a tablet, the two
    // landscape pages on a phone (decided from the window size in DeckApp).
    std::env::set_var("OPENDECK_PORTRAIT", "1");

    let demo_dir = music.join("OpenDeck");
    seed_demo(&app, &demo_dir);
    let track = crate::first_audio_in(&demo_dir).or_else(|| crate::first_audio_in(&music));
    match &track {
        Some(t) => log::info!("track: {}", t.display()),
        None => log::info!("no audio in {} — booting to an empty deck", music.display()),
    }
    let player: Option<u8> = std::env::var("OPENDECK_PLAYER").ok().and_then(|v| v.parse().ok());
    let _ = APP.set(app);
    // Player 3 by default, like the iPad: "deck 3" next to CDJs 1-2.
    let cfg = Config { track, player, default_player: 3, deck_channel: 0, link_send: true,
                       browse_root: Some(music), autoplay: false, controllers: None };
    if let Err(e) = run(cfg) {
        log::error!("android_main: {e:#}");
    }
}

/// Copy the demo track out of the APK's assets once.  If the user deletes it,
/// it stays deleted (a marker in app data remembers the seeding).
fn seed_demo(app: &AndroidApp, dir: &Path) {
    let marker = crate::taglist::app_data_dir().join(".seeded");
    if marker.exists() { return; }
    let name = c"amplificate.mp3";
    let Some(mut asset) = app.asset_manager().open(name) else {
        log::info!("no demo track in the APK");
        return;
    };
    let bytes = match asset.buffer() {
        Ok(b) => b.to_vec(),
        Err(e) => { log::warn!("demo track: {e}"); return; }
    };
    let _ = std::fs::create_dir_all(dir);
    let dst = dir.join("amplificate.mp3");
    match std::fs::write(&dst, bytes) {
        Ok(()) => {
            log::info!("seeded {}", dst.display());
            let _ = std::fs::create_dir_all(marker.parent().unwrap_or(Path::new(".")));
            let _ = std::fs::write(&marker, b"");
        }
        Err(e) => log::warn!("seeding {}: {e}", dst.display()),
    }
}

type JResult<T> = Result<T, Box<dyn std::error::Error>>;

fn with_activity<T>(app: &AndroidApp, f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> JResult<T>) -> JResult<T> {
    // SAFETY: both pointers come from the live NativeActivity and outlive this call.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM)? };
    let mut env = vm.attach_current_thread()?;
    let act = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let r = f(&mut env, &act);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    r
}

/// Ask for read access to the shared music.  Android 13 split it per media
/// type (READ_MEDIA_AUDIO); before that it was READ_EXTERNAL_STORAGE.  The
/// dialog pauses the activity; the answer is read on the next resume.
fn request_storage(env: &mut jni::JNIEnv, act: &JObject, sdk: i32) -> JResult<()> {
    if sdk < 23 { return Ok(()); }   // granted at install time
    let perm = if sdk >= 33 { "android.permission.READ_MEDIA_AUDIO" } else { "android.permission.READ_EXTERNAL_STORAGE" };
    let jperm = env.new_string(perm)?;
    let granted = env.call_method(act, "checkSelfPermission", "(Ljava/lang/String;)I", &[JValue::Object(&jperm)])?.i()? == 0;
    log::info!("{perm}: {}", if granted { "granted" } else { "asking" });
    if granted { return Ok(()); }
    let arr = env.new_object_array(1, "java/lang/String", &jperm)?;
    env.call_method(act, "requestPermissions", "([Ljava/lang/String;I)V", &[JValue::Object(&arr), JValue::Int(1)])?;
    Ok(())
}

/// Wi-Fi drivers drop broadcast and multicast frames to save power unless an
/// app holds a multicast lock — and Pro DJ Link is all broadcast.  Held for
/// the life of the process (the global ref is leaked on purpose).
fn multicast_lock(env: &mut jni::JNIEnv, act: &JObject) -> JResult<()> {
    let ctx = env.call_method(act, "getApplicationContext", "()Landroid/content/Context;", &[])?.l()?;
    let svc = env.new_string("wifi")?;
    let wifi = env.call_method(&ctx, "getSystemService", "(Ljava/lang/String;)Ljava/lang/Object;", &[JValue::Object(&svc)])?.l()?;
    if wifi.is_null() { return Err("no WifiManager".into()); }
    let tag = env.new_string("opendeck-link")?;
    let lock = env.call_method(&wifi, "createMulticastLock",
        "(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;", &[JValue::Object(&tag)])?.l()?;
    env.call_method(&lock, "setReferenceCounted", "(Z)V", &[JValue::Bool(0)])?;
    env.call_method(&lock, "acquire", "()V", &[])?;
    std::mem::forget(env.new_global_ref(&lock)?);
    log::info!("wifi multicast lock held");
    Ok(())
}

/// Phones get the landscape two-page layout, tablets the portrait deck (the
/// iOS split).  "smallest width" is the device's short side in dp, the
/// standard phone/tablet line being 600.
fn lock_orientation(env: &mut jni::JNIEnv, act: &JObject, app: &AndroidApp) -> JResult<()> {
    let sw = app.config().smallest_screen_width_dp().unwrap_or(0);
    const SENSOR_LANDSCAPE: i32 = 6;
    const SENSOR_PORTRAIT: i32 = 7;
    let o = if sw > 0 && sw < 600 { SENSOR_LANDSCAPE } else { SENSOR_PORTRAIT };
    env.call_method(act, "setRequestedOrientation", "(I)V", &[JValue::Int(o)])?;
    log::info!("smallest width {sw} dp → {}", if o == SENSOR_LANDSCAPE { "landscape (phone)" } else { "portrait (tablet)" });
    Ok(())
}

/// An Android system property, or "" when unset.
fn system_property(name: &str) -> String {
    extern "C" { fn __system_property_get(name: *const std::ffi::c_char, value: *mut std::ffi::c_char) -> i32; }
    let Ok(cname) = std::ffi::CString::new(name) else { return String::new() };
    let mut buf = [0 as std::ffi::c_char; 92];   // PROP_VALUE_MAX
    // SAFETY: buf is PROP_VALUE_MAX bytes, which is all the call ever writes.
    let n = unsafe { __system_property_get(cname.as_ptr(), buf.as_mut_ptr()) };
    if n <= 0 { return String::new(); }
    unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}
