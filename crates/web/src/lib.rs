//! OpenDeck in a browser.
//!
//! The deck is the same `opendeck-app` the desktop runs; this crate is the
//! platform under it ([`opendeck_app::Platform`]): its sound goes out
//! through Web Audio ([`audio`]), its background jobs run on the page's
//! event loop, its renderer is made when the browser's GPU answers, and its
//! window is a canvas on the page.  Built with `make web` (see www/).

#![cfg(target_arch = "wasm32")]

mod audio;

use opendeck_app::{Config, Event, EventSlot, Platform, Spawn, TrackData};
use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
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
    let events: EventSlot = Arc::new(Mutex::new(None));
    let platform = Platform {
        audio_out: Some(Box::new(audio::WebAudioOut::new())),
        spawn: Arc::new(PageJobs),
        run_future: |f| wasm_bindgen_futures::spawn_local(f),
        window: |a| a.with_append(true).with_prevent_default(true).with_focusable(true),
        events: Some(Arc::clone(&events)),
    };
    // The demo track, once the deck is up (run_on fills `events` first).
    wasm_bindgen_futures::spawn_local(load_demo(events));
    if let Err(e) = opendeck_app::run_on(cfg, platform) {
        log::error!("OpenDeck: {e:#}");
    }
}

/// The track the page serves beside it (`demo.mp3`, or `?track=NAME`),
/// fetched and handed to the deck.  PLAY after any touch of the page
/// (browsers start audio only then).
async fn load_demo(events: EventSlot) {
    let name = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|q| q.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix("track=").map(str::to_owned)))
        .unwrap_or_else(|| "demo.mp3".into());
    match fetch(&name).await {
        Ok(data) => {
            log::info!("{name}: {} bytes fetched", data.len());
            let Some(tx) = events.lock().unwrap().clone() else { log::error!("the deck is not up"); return };
            let _ = tx.send(Event::LoadData { name, data: TrackData(data), play: false });
        }
        Err(e) => log::warn!("{name}: {e}"),
    }
}

async fn fetch(url: &str) -> Result<Vec<u8>, String> {
    use wasm_bindgen_futures::JsFuture;
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_str(url)).await
        .map_err(|e| format!("{e:?}"))?.dyn_into().map_err(|_| "not a Response")?;
    if !resp.ok() { return Err(format!("HTTP {}", resp.status())); }
    let buf = JsFuture::from(resp.array_buffer().map_err(|e| format!("{e:?}"))?).await.map_err(|e| format!("{e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}
