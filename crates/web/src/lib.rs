//! OpenDeck in a browser.
//!
//! The deck is the same `opendeck-app` the desktop runs; this crate is the
//! platform under it ([`opendeck_app::Platform`]): its sound goes out
//! through Web Audio ([`audio`]), its background jobs run on the page's
//! event loop, its renderer is made when the browser's GPU answers, and its
//! window is a canvas on the page.  Built with `make web` (see www/).

#![cfg(target_arch = "wasm32")]

mod audio;

use opendeck_app::{Config, Platform, Spawn};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use winit::platform::web::WindowAttributesExtWebSys;

/// Background jobs, on the page's event loop: there is one thread here,
/// so a job runs after the frame that started it, not beside it.
struct PageJobs;

impl Spawn for PageJobs {
    fn spawn(&self, name: &str, job: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
        log::debug!("job {name}");
        wasm_bindgen_futures::spawn_local(async move { job() });
        Ok(())
    }
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    log::info!("OpenDeck in the browser");

    let cfg = Config {
        track: None,
        player: None,
        default_player: 1,
        deck_channel: 0,
        link_send: false,
        browse_root: None,
        autoplay: false,
        controllers: None,
        host: None,
        follow: None,
    };
    let platform = Platform {
        audio_out: Some(Box::new(audio::WebAudioOut::new())),
        spawn: Arc::new(PageJobs),
        run_future: |f| wasm_bindgen_futures::spawn_local(f),
        window: |a| a.with_append(true).with_prevent_default(true).with_focusable(true),
    };
    if let Err(e) = opendeck_app::run_on(cfg, platform) {
        log::error!("OpenDeck: {e:#}");
    }
}
