//! The deck's sound through Web Audio.
//!
//! A `ScriptProcessorNode` asks for each buffer on the page's own thread,
//! and there the deck's processor runs too ([`AudioOut::runs_processor`]):
//! one thread, no shared memory needed.  (An AudioWorklet is the better
//! home, off the page's thread; it needs the page cross-origin isolated
//! for the shared ring.  Later.)
//!
//! Browsers start audio suspended until the page is touched, so the first
//! pointer or key press resumes it.

use anyhow::{anyhow, Result};
use opendeck_app::{AudioOut, Format, RenderSource, Running};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{AudioContext, AudioProcessingEvent};

/// Frames per Web Audio callback: ~43 ms at 48 kHz, enough to cover the
/// page's frame and the stretching done in the callback.
const BUFFER_FRAMES: u32 = 2048;

pub struct WebAudioOut {
    ctx: Option<AudioContext>,
}

impl WebAudioOut {
    pub fn new() -> WebAudioOut {
        let ctx = AudioContext::new().map_err(|e| log::error!("no Web Audio: {e:?}")).ok();
        WebAudioOut { ctx }
    }
}

impl AudioOut for WebAudioOut {
    fn format(&self) -> Format {
        let sample_rate = self.ctx.as_ref().map_or(48_000.0, |c| c.sample_rate()) as u32;
        Format { sample_rate, channels: 2 }
    }

    fn runs_processor(&self) -> bool { true }

    fn start(self: Box<Self>, src: RenderSource) -> Result<Running> {
        let ctx = self.ctx.ok_or_else(|| anyhow!("no Web Audio in this browser"))?;
        let node = ctx
            .create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(BUFFER_FRAMES, 0, 2)
            .map_err(|e| anyhow!("ScriptProcessorNode: {e:?}"))?;
        let mut render = src.render();
        let mut inter = vec![0f32; BUFFER_FRAMES as usize * 2];
        let (mut left, mut right) = (vec![0f32; BUFFER_FRAMES as usize], vec![0f32; BUFFER_FRAMES as usize]);
        let on_audio = Closure::<dyn FnMut(AudioProcessingEvent)>::new(move |ev: AudioProcessingEvent| {
            let Ok(buf) = ev.output_buffer() else { return };
            let n = buf.length() as usize;
            if inter.len() < n * 2 { inter.resize(n * 2, 0.0); left.resize(n, 0.0); right.resize(n, 0.0); }
            render.fill(&mut inter[..n * 2]);
            for i in 0..n { left[i] = inter[2 * i]; right[i] = inter[2 * i + 1]; }
            let _ = buf.copy_to_channel(&mut left[..n], 0);
            let _ = buf.copy_to_channel(&mut right[..n], 1);
        });
        node.set_onaudioprocess(Some(on_audio.as_ref().unchecked_ref()));
        node.connect_with_audio_node(&ctx.destination()).map_err(|e| anyhow!("connect: {e:?}"))?;

        // Audio starts suspended until the page is touched.
        let resume = {
            let ctx = ctx.clone();
            Closure::<dyn FnMut()>::new(move || {
                if ctx.state() != web_sys::AudioContextState::Running { let _ = ctx.resume(); }
            })
        };
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            for ev in ["pointerdown", "keydown", "touchstart"] {
                let _ = doc.add_event_listener_with_callback(ev, resume.as_ref().unchecked_ref());
            }
        }
        log::info!("Web Audio at {} Hz, {BUFFER_FRAMES}-frame buffers (starts on the first touch)", ctx.sample_rate());
        Ok(Running::holding((ctx, node, on_audio, resume)))
    }
}
