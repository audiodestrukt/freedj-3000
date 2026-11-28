//! The clock.  Use these, never `std::time::Instant` / `SystemTime`
//! (tools/ci/check-clock.sh enforces it): in a browser std's clock panics, and this is
//! the one place that picks the browser's (`performance.now()`, via
//! web-time).  Natively these ARE std's types; nothing changes.
//!
//! `Duration` is std's everywhere: use `std::time::Duration`.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(target_arch = "wasm32")]
pub use web_time::{Instant, SystemTime, UNIX_EPOCH};

/// A file's timestamp, as `fs::Metadata::modified()` gives it: std's
/// `SystemTime` everywhere, whatever the clock above is.  Compare file
/// times with each other, never with [`SystemTime::now`].
pub type FileTime = std::time::SystemTime;

/// [`FileTime`]'s epoch, for a file with no time.
pub const FILE_EPOCH: FileTime = std::time::UNIX_EPOCH;

/// How long after the Unix epoch a file was modified.
pub fn file_time_since_epoch(t: FileTime) -> Option<std::time::Duration> {
    t.duration_since(FILE_EPOCH).ok()
}
