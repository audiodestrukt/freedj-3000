pub mod prodj;
mod status_template;
mod status_template_cdj3000;
pub mod ableton;
pub mod lanhid;
// The CDJ session handshake presents as rekordbox: kept out of the iOS
// build until that is settled (docs/TODO-cdj-3000.md).
#[cfg(not(target_os = "ios"))]
pub mod lanhid_auth;
pub mod lanhid_display;
pub mod lanhid_frag;
pub mod lanhid_status;

pub use prodj::ProDjLink;
