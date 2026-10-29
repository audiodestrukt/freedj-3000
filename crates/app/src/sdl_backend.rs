//! SDL2 render backend (cargo feature `renderer-sdl`, `OPENDECK_RENDERER=sdl`).
//!
//! For boards wgpu cannot drive: the VideoCore IV (OpenGL ES 2.0 only) in the
//! Pi Zero / Zero 2 W / Pi 3, a CDJ-3000's fbdev rootfs, and bare KMSDRM with
//! no compositor.  SDL's 2D renderer runs on GLES2, software, KMSDRM, X11 and
//! Wayland alike, and `SDL_RenderGeometry` (SDL ≥ 2.0.18) draws exactly what
//! egui emits: textured, vertex-coloured triangle lists.
//!
//! Three parts:
//! - `run`: the event loop.  SDL owns the window and events; keys go through
//!   the same `DeckApp::key_input` as winit's, pointer and touch become egui
//!   events, and `DeckApp::render_frame` runs once per loop, paced by vsync
//!   (`present_vsync`) or a timer.
//! - `SdlHost`: the `Host` side — size, scale, egui `RawInput`.
//! - `SdlRenderer`: the `Renderer` side — the waveform rects rasterised on the
//!   CPU (`waveform_cpu`) into streaming textures, egui meshes through
//!   `render_geometry` with the font atlas and chrome sprites as SDL textures.
//!
//! Colour: egui's vertex colours and textures are premultiplied and, on the
//! wgpu path, blended in linear light against an sRGB surface.  SDL blends in
//! the framebuffer's own (gamma) space with straight alpha, so both are
//! un-premultiplied here and `BlendMode::Blend` does the rest — the same
//! arithmetic, differing only where the gamma-vs-linear blend of an anti-
//! aliased edge or a translucent fill lands a level or two apart.

use crate::renderer::{display_gain, ColorMode, Renderer, Viewports, WaveformParams};
use crate::snapshot::DeckSnapshot;
use crate::waveform_cpu::{self, Kind, Lut, OverviewKey, PxRect, WaveCache};
use crate::{DeckApp, Host};
use anyhow::{anyhow, Context, Result};
use opendeck_analysis::WaveformCache;
use sdl2::event::{Event as SdlEvent, WindowEvent};
use sdl2::keyboard::Scancode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::{FPoint, Rect};
use sdl2::render::{BlendMode, Canvas, ScaleMode, Texture, TextureCreator, Vertex, VertexIndices};
use sdl2::video::{Window, WindowContext};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use winit::keyboard::KeyCode;

// ── Event loop ────────────────────────────────────────────────────────────────

/// Run the deck under SDL until the window closes, Q/Esc, or a capture exits.
pub fn run(app: &mut DeckApp) -> Result<()> {
    // KMSDRM / software hints are the user's call through SDL's own env
    // (SDL_VIDEODRIVER, SDL_RENDER_DRIVER); nothing is forced here.
    let sdl   = sdl2::init().map_err(|e| anyhow!("SDL init: {e}"))?;
    let video = sdl.video().map_err(|e| anyhow!("SDL video: {e}"))?;
    let (win_w, win_h) = app.initial_window_size();
    let mut wb = video.window("freedj-3000", win_w, win_h);
    wb.position_centered().allow_highdpi();
    // OPENDECK_FULLSCREEN=1: the appliance case (KMSDRM has no windows anyway).
    if std::env::var("OPENDECK_FULLSCREEN").as_deref() == Ok("1") { wb.fullscreen_desktop(); }
    let window = wb.build().context("SDL window")?;

    // Vsync paces the loop: present() blocks until the flip, so one frame per
    // refresh with no timer.  OPENDECK_PRESENT=immediate turns it off (then
    // the FRAME_INTERVAL timer below paces instead), as on the wgpu path.
    let vsync = !matches!(std::env::var("OPENDECK_PRESENT").ok().as_deref(), Some("immediate"));
    let mut cb = window.into_canvas();
    if vsync { cb = cb.present_vsync(); }
    let canvas = cb.build().context("SDL renderer")?;
    let info = canvas.info();
    log::info!("SDL renderer: {} (video driver {}), vsync {}", info.name, video.current_video_driver(), vsync);
    log::info!("present mode: {}", if vsync { "vsync" } else { "immediate (timer-paced)" });

    if let Ok(mode) = video.current_display_mode(0) {
        if mode.refresh_rate > 0 {
            app.refresh_interval = Duration::from_secs_f64(1.0 / mode.refresh_rate as f64);
            log::info!("display refresh {} Hz → {:.3} ms per frame", mode.refresh_rate, app.refresh_interval.as_secs_f64() * 1000.0);
        }
    }

    let renderer = SdlRenderer::new(canvas, &app.waveform)?;
    let (pw, ph) = renderer.size;
    let (lw, _lh) = renderer.canvas.window().size();
    let ppp = if lw > 0 { pw as f32 / lw as f32 } else { 1.0 };
    log::info!("window {}x{} px, scale {:.2}", pw, ph, ppp);
    let pump = sdl.event_pump().map_err(|e| anyhow!("SDL event pump: {e}"))?;

    // On a one-core board the render loop and the audio threads share the
    // CPU; a frame here is tens of milliseconds of work, and the audio ring
    // starves when it waits behind one.  Lowering this thread's priority
    // (raising its nice value needs no privilege) lets audio-proc and the
    // device callback pre-empt rendering.  OPENDECK_RENDER_NICE overrides
    // (0 = leave it alone).
    #[cfg(target_os = "linux")]
    {
        let nice: i32 = std::env::var("OPENDECK_RENDER_NICE").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
        if nice != 0 {
            let tid = unsafe { libc::syscall(libc::SYS_gettid) } as libc::id_t;
            if unsafe { libc::setpriority(libc::PRIO_PROCESS, tid, nice) } == 0 {
                log::info!("render thread nice {nice}");
            }
        }
    }
    // OPENDECK_MAX_FPS caps the loop below the display rate — on a slow board
    // a frame budget the CPU can meet beats chasing vsync and missing it.
    let max_fps: Option<f32> = std::env::var("OPENDECK_MAX_FPS").ok().and_then(|v| v.parse().ok()).filter(|f: &f32| *f > 0.0);
    if let Some(f) = max_fps { log::info!("frame rate capped at {f} fps"); }
    app.host     = Some(Host::Sdl(SdlHost { pump, events: Vec::new(), modifiers: egui::Modifiers::default(),
                                            size_px: (pw, ph), ppp, start: Instant::now(), focused: true }));
    app.renderer = Some(Box::new(renderer));

    loop {
        // ── Events ───────────────────────────────────────────────────────
        let pending: Vec<SdlEvent> = match app.host.as_mut() {
            Some(Host::Sdl(h)) => h.pump.poll_iter().collect(),
            _ => Vec::new(),
        };
        let mut quit = false;
        let mut resized = false;
        for ev in pending {
            match &ev {
                SdlEvent::Quit { .. } => quit = true,
                SdlEvent::KeyDown { scancode: Some(sc), repeat, .. } => {
                    if let Some(code) = keycode(*sc) { if app.key_input(code, true, *repeat) { quit = true; } }
                }
                SdlEvent::KeyUp { scancode: Some(sc), .. } => {
                    if let Some(code) = keycode(*sc) { if app.key_input(code, false, false) { quit = true; } }
                }
                SdlEvent::Window { win_event: WindowEvent::SizeChanged(..) | WindowEvent::Resized(..), .. } => resized = true,
                _ => {}
            }
            if let Some(Host::Sdl(h)) = app.host.as_mut() { h.translate(&ev); }
        }
        if quit { break; }
        if resized {
            // The canvas is the truth for the pixel size; the host caches it.
            let px = app.renderer.as_mut().map(|r| { r.resize(0, 0); r.surface_size() });
            if let (Some(px), Some(Host::Sdl(h))) = (px, app.host.as_mut()) {
                if px.0 > 0 && px.1 > 0 { h.size_px = px; }
            }
        }

        // ── Frame ────────────────────────────────────────────────────────
        app.render_frame();
        if app.exit_after_capture { break; }

        // ── Pacing ───────────────────────────────────────────────────────
        // Idle (nothing moving): sleep to the idle rate.  Otherwise vsync in
        // present() already paced us; without vsync, hold the frame interval.
        let period = match (app.full_rate, app.idle_fps) {
            (false, Some(fps)) => Some(Duration::from_secs_f32(1.0 / fps)),
            _ if max_fps.is_some() => max_fps.map(|f| Duration::from_secs_f32(1.0 / f)),
            _ if !vsync        => Some(app.refresh_interval),
            _                  => None,
        };
        if let Some(period) = period {
            let due = app.last_render + period;
            let now = Instant::now();
            if due > now { std::thread::sleep(due - now); }
        }
    }
    Ok(())
}

/// SDL scancode → winit physical key, for the keys the deck binds.  Only the
/// shortcut set is mapped; anything else is ignored, as it is on winit.
fn keycode(sc: Scancode) -> Option<KeyCode> {
    use Scancode as S;
    Some(match sc {
        S::Return => KeyCode::Enter,   S::KpEnter => KeyCode::NumpadEnter,
        S::Space => KeyCode::Space,    S::Tab => KeyCode::Tab,
        S::Backspace => KeyCode::Backspace, S::Escape => KeyCode::Escape, S::Delete => KeyCode::Delete,
        S::Up => KeyCode::ArrowUp,     S::Down => KeyCode::ArrowDown,
        S::Left => KeyCode::ArrowLeft, S::Right => KeyCode::ArrowRight,
        S::Equals => KeyCode::Equal,   S::KpPlus => KeyCode::NumpadAdd,
        S::Minus => KeyCode::Minus,    S::KpMinus => KeyCode::NumpadSubtract,
        S::Num0 => KeyCode::Digit0,    S::Kp0 => KeyCode::Numpad0,
        S::LeftBracket => KeyCode::BracketLeft, S::RightBracket => KeyCode::BracketRight,
        S::Comma => KeyCode::Comma,    S::Period => KeyCode::Period,
        S::A => KeyCode::KeyA, S::B => KeyCode::KeyB, S::C => KeyCode::KeyC, S::G => KeyCode::KeyG,
        S::K => KeyCode::KeyK, S::M => KeyCode::KeyM, S::N => KeyCode::KeyN, S::P => KeyCode::KeyP,
        S::Q => KeyCode::KeyQ, S::S => KeyCode::KeyS, S::T => KeyCode::KeyT, S::V => KeyCode::KeyV,
        S::X => KeyCode::KeyX, S::Y => KeyCode::KeyY, S::Z => KeyCode::KeyZ,
        _ => return None,
    })
}

// ── Host ──────────────────────────────────────────────────────────────────────

/// The window/input side: an SDL event pump and the egui events translated
/// from it since the last frame.
pub struct SdlHost {
    pump:      sdl2::EventPump,
    events:    Vec<egui::Event>,
    modifiers: egui::Modifiers,
    size_px:   (u32, u32),
    ppp:       f32,
    start:     Instant,
    focused:   bool,
}

impl SdlHost {
    pub fn scale_factor(&self) -> f32 { self.ppp }
    pub fn size_px(&self) -> (u32, u32) { self.size_px }

    pub fn take_egui_input(&mut self) -> egui::RawInput {
        let (w, h) = self.size_px;
        let mut raw = egui::RawInput {
            time: Some(self.start.elapsed().as_secs_f64()),
            screen_rect: (w > 0 && h > 0).then(|| egui::Rect::from_min_size(
                egui::Pos2::ZERO, egui::Vec2::new(w as f32 / self.ppp, h as f32 / self.ppp))),
            modifiers: self.modifiers,
            events: std::mem::take(&mut self.events),
            focused: self.focused,
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.ppp);
        raw
    }

    /// Logical (point) coordinates: SDL reports mouse positions in window
    /// units, which with allow_highdpi are points, and touch as 0..1 of the
    /// window — both what egui wants once scaled by the window's point size.
    fn translate(&mut self, ev: &SdlEvent) {
        let (w, h) = self.size_px;
        let (pw, ph) = (w as f32 / self.ppp, h as f32 / self.ppp);
        match ev {
            SdlEvent::MouseMotion { x, y, .. } => self.events.push(egui::Event::PointerMoved(egui::pos2(*x as f32, *y as f32))),
            SdlEvent::MouseButtonDown { x, y, mouse_btn, .. } | SdlEvent::MouseButtonUp { x, y, mouse_btn, .. } => {
                let pressed = matches!(ev, SdlEvent::MouseButtonDown { .. });
                let button = match mouse_btn {
                    sdl2::mouse::MouseButton::Left   => egui::PointerButton::Primary,
                    sdl2::mouse::MouseButton::Right  => egui::PointerButton::Secondary,
                    sdl2::mouse::MouseButton::Middle => egui::PointerButton::Middle,
                    _ => return,
                };
                self.events.push(egui::Event::PointerButton { pos: egui::pos2(*x as f32, *y as f32), button, pressed, modifiers: self.modifiers });
            }
            SdlEvent::MouseWheel { precise_x, precise_y, .. } => {
                self.events.push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(*precise_x, *precise_y), modifiers: self.modifiers });
            }
            SdlEvent::FingerDown { touch_id, finger_id, x, y, pressure, .. }
            | SdlEvent::FingerUp { touch_id, finger_id, x, y, pressure, .. }
            | SdlEvent::FingerMotion { touch_id, finger_id, x, y, pressure, .. } => {
                let phase = match ev {
                    SdlEvent::FingerDown { .. }   => egui::TouchPhase::Start,
                    SdlEvent::FingerMotion { .. } => egui::TouchPhase::Move,
                    _                             => egui::TouchPhase::End,
                };
                // SDL also synthesises mouse events from the first finger (its
                // default), which is the pointer egui clicks with — the same
                // pairing egui-winit produces.
                self.events.push(egui::Event::Touch {
                    device_id: egui::TouchDeviceId(*touch_id as u64),
                    id: egui::TouchId(*finger_id as u64),
                    phase,
                    pos: egui::pos2(x * pw, y * ph),
                    force: Some(*pressure),
                });
            }
            SdlEvent::KeyDown { keymod, .. } | SdlEvent::KeyUp { keymod, .. } => {
                use sdl2::keyboard::Mod;
                self.modifiers = egui::Modifiers {
                    alt:     keymod.intersects(Mod::LALTMOD | Mod::RALTMOD),
                    ctrl:    keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD),
                    shift:   keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD),
                    mac_cmd: false,
                    command: keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD),
                };
            }
            SdlEvent::Window { win_event, .. } => match win_event {
                WindowEvent::Leave => self.events.push(egui::Event::PointerGone),
                WindowEvent::FocusGained => { self.focused = true;  self.events.push(egui::Event::WindowFocused(true)); }
                WindowEvent::FocusLost   => { self.focused = false; self.events.push(egui::Event::WindowFocused(false)); }
                _ => {}
            },
            _ => {}
        }
    }
}

// ── Renderer ──────────────────────────────────────────────────────────────────

pub struct SdlRenderer {
    canvas:   Canvas<Window>,
    tc:       TextureCreator<WindowContext>,
    /// Output size in pixels.
    size:     (u32, u32),
    /// egui textures (font atlas, chrome sprites) by id, with their size.
    textures: HashMap<egui::TextureId, (Texture, [usize; 2])>,
    /// Scratch for a mesh's vertices, reused across draws.
    verts:    Vec<Vertex>,

    // Waveform
    cols:       Vec<u32>,
    num_cols:   u32,
    amp_gain:   f32,
    color_mode: ColorMode,
    cols_visible: f32,
    wave_tex:   Option<(Texture, u32, u32)>,
    over_tex:   Option<(Texture, u32, u32)>,
    /// What the overview texture currently shows; unchanged = no re-raster.
    over_key:   Option<OverviewKey>,
    /// Whole-pixel scroll cache for the enlarged waveform (`OPENDECK_WAVE_SCROLL`
    /// = pixel, the default here; `smooth` re-rasterises every frame).
    wave_cache: Option<WaveCache>,
    pix:        Vec<u32>,
    lut:        Lut,

    capture:    Option<std::path::PathBuf>,
}

impl SdlRenderer {
    pub fn new(mut canvas: Canvas<Window>, waveform: &WaveformCache) -> Result<Self> {
        let size = canvas.output_size().map_err(|e| anyhow!("SDL output size: {e}"))?;
        canvas.set_blend_mode(BlendMode::Blend);
        let tc = canvas.texture_creator();
        let cols: Vec<u32> = waveform.columns.iter().map(|c| u32::from_le_bytes(*c)).collect();
        let (peak, amp_gain) = display_gain(&cols);
        log::info!("waveform peak {:.2} → display gain {:.2}", peak, amp_gain);
        Ok(Self {
            canvas, tc, size,
            textures: HashMap::new(),
            verts: Vec::new(),
            num_cols: cols.len() as u32,
            cols,
            amp_gain,
            color_mode: ColorMode::Blue,
            cols_visible: crate::input::ZOOM_LEVELS[crate::input::ZOOM_DEFAULT],
            wave_tex: None,
            over_tex: None,
            over_key: None,
            wave_cache: match std::env::var("OPENDECK_WAVE_SCROLL").ok().as_deref() {
                Some("smooth") => None,
                _ => Some(WaveCache::default()),
            },
            pix: Vec::new(),
            lut: Lut::new(),
            capture: None,
        })
    }

    /// Create or update an egui texture from an atlas delta.  Premultiplied
    /// egui pixels become straight-alpha RGBA for SDL's blend.
    fn update_texture(&mut self, id: egui::TextureId, delta: &egui::epaint::ImageDelta) -> Result<()> {
        let [w, h] = delta.image.size();
        let mut bytes = Vec::with_capacity(w * h * 4);
        match &delta.image {
            egui::ImageData::Color(img) => for c in &img.pixels { bytes.extend_from_slice(&unmultiply(*c)); },
            egui::ImageData::Font(img)  => for c in img.srgba_pixels(None) { bytes.extend_from_slice(&unmultiply(c)); },
        }
        let scale = match delta.options.magnification {
            egui::TextureFilter::Nearest => ScaleMode::Nearest,
            egui::TextureFilter::Linear  => ScaleMode::Linear,
        };
        match delta.pos {
            None => {
                // Whole texture: (re)create at this size.
                let mut tex = self.tc.create_texture_static(PixelFormatEnum::ABGR8888, w as u32, h as u32)
                    .map_err(|e| anyhow!("egui texture {w}x{h}: {e}"))?;
                tex.set_blend_mode(BlendMode::Blend);
                tex.set_scale_mode(scale);
                tex.update(None, &bytes, w * 4).map_err(|e| anyhow!("egui texture upload: {e}"))?;
                if let Some((old, _)) = self.textures.insert(id, (tex, [w, h])) { unsafe { old.destroy(); } }
            }
            Some([x, y]) => {
                let (tex, _) = self.textures.get_mut(&id).ok_or_else(|| anyhow!("egui patched a texture it never created"))?;
                tex.update(Rect::new(x as i32, y as i32, w as u32, h as u32), &bytes, w * 4)
                    .map_err(|e| anyhow!("egui texture patch: {e}"))?;
            }
        }
        Ok(())
    }

    /// Rasterise one waveform rect into its streaming texture and blit it.
    fn draw_waveform(&mut self, p: &WaveformParams, kind: Kind, rect: [f32; 4]) -> Result<()> {
        let Some(px) = PxRect::from_f32(rect) else { return Ok(()) };
        let slot = match kind { Kind::Wave => &mut self.wave_tex, Kind::Overview => &mut self.over_tex };
        let fresh = !matches!(slot, Some((_, w, h)) if *w == px.w && *h == px.h);
        if fresh {
            if let Some((old, _, _)) = slot.take() { unsafe { old.destroy(); } }
            let tex = self.tc.create_texture_streaming(PixelFormatEnum::ABGR8888, px.w, px.h)
                .map_err(|e| anyhow!("waveform texture {}x{}: {e}", px.w, px.h))?;
            *slot = Some((tex, px.w, px.h));
            if kind == Kind::Overview { self.over_key = None; }
        }
        // The overview only changes when a marker crosses a pixel; the
        // enlarged waveform scrolls every frame.
        let redraw = match kind {
            Kind::Wave => true,
            Kind::Overview => {
                let key = OverviewKey::new(p, px);
                let changed = self.over_key != Some(key);
                if changed { self.over_key = Some(key); }
                changed
            }
        };
        let (tex, _, _) = slot.as_mut().expect("just created");
        if redraw {
            let t = Instant::now();
            match (kind, self.wave_cache.as_mut()) {
                (Kind::Wave, Some(cache)) => waveform_cpu::draw_wave_cached(&self.lut, p, &self.cols, px, cache, &mut self.pix),
                _ => waveform_cpu::draw(&self.lut, p, &self.cols, kind, px, &mut self.pix),
            }
            crate::perf_accum("wave_cpu", t.elapsed());
            let t = Instant::now();
            tex.update(None, bytemuck::cast_slice(&self.pix), px.w as usize * 4).map_err(|e| anyhow!("waveform upload: {e}"))?;
            crate::perf_accum("wave_upload", t.elapsed());
        }
        self.canvas.copy(tex, None, Rect::new(px.x, px.y, px.w, px.h)).map_err(|e| anyhow!("waveform blit: {e}"))?;
        Ok(())
    }

    fn write_capture(&mut self, path: &std::path::Path) -> Result<()> {
        let (w, h) = self.size;
        let rgba = self.canvas.read_pixels(None, PixelFormatEnum::ABGR8888).map_err(|e| anyhow!("read pixels: {e}"))?;
        let f = std::fs::File::create(path)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header()?;
        // The framebuffer has no meaningful alpha; store opaque like the wgpu path.
        let mut opaque = rgba;
        for px in opaque.chunks_exact_mut(4) { px[3] = 255; }
        wr.write_image_data(&opaque)?;
        Ok(())
    }
}

/// Premultiplied egui colour → straight RGBA bytes.
#[inline]
fn unmultiply(c: egui::Color32) -> [u8; 4] {
    let [r, g, b, a] = c.to_array();
    if a == 0 { return [0, 0, 0, 0]; }
    if a == 255 { return [r, g, b, 255]; }
    let f = |v: u8| ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
    [f(r), f(g), f(b), a]
}

impl Renderer for SdlRenderer {
    fn color_mode(&self) -> ColorMode { self.color_mode }
    fn set_color_mode(&mut self, mode: ColorMode) { self.color_mode = mode; }
    fn set_cols_visible(&mut self, cols: f32) { self.cols_visible = cols; }
    fn surface_size(&self) -> (u32, u32) { self.size }

    fn set_waveform(&mut self, waveform: &WaveformCache) {
        self.cols = waveform.columns.iter().map(|c| u32::from_le_bytes(*c)).collect();
        self.num_cols = self.cols.len() as u32;
        let (peak, gain) = display_gain(&self.cols);
        self.amp_gain = gain;
        self.over_key = None;   // new columns: both waveforms must be drawn again
        if let Some(c) = self.wave_cache.as_mut() { c.invalidate(); }
        log::info!("waveform reloaded: {} columns, peak {:.2} → gain {:.2}", self.num_cols, peak, gain);
    }

    /// The canvas tracks the window itself; this just refreshes the cached
    /// pixel size (the arguments are ignored, the canvas is the truth).
    fn resize(&mut self, _width: u32, _height: u32) {
        if let Ok(sz) = self.canvas.output_size() { self.size = sz; }
    }

    fn request_capture(&mut self, path: std::path::PathBuf) { self.capture = Some(path); }

    fn render(&mut self, snap: &DeckSnapshot, vp: &Viewports, egui_ctx: &egui::Context, full_output: egui::FullOutput) {
        let ppp = full_output.pixels_per_point;
        let (sw, sh) = self.size;
        let params = WaveformParams::from_frame(snap, vp, self.cols_visible, self.color_mode, self.amp_gain,
                                                self.num_cols, sw, sh);

        // ── Ground + waveforms (the shader's pass 1) ─────────────────────
        self.canvas.set_clip_rect(None);
        self.canvas.set_draw_color(Color::RGB(3, 4, 6));   // ground(), as screen.rs BG
        self.canvas.clear();
        if let Err(e) = self.draw_waveform(&params, Kind::Wave, vp.wave) { log::warn!("{e:#}"); }
        if let Err(e) = self.draw_waveform(&params, Kind::Overview, vp.overview) { log::warn!("{e:#}"); }

        // ── egui (pass 2) ────────────────────────────────────────────────
        for (id, delta) in &full_output.textures_delta.set {
            if let Err(e) = self.update_texture(*id, delta) { log::warn!("{e:#}"); }
        }
        let _t_tess = Instant::now();
        let primitives = egui_ctx.tessellate(full_output.shapes, ppp);
        crate::perf_accum("tessellate", _t_tess.elapsed());
        let _t_draw = Instant::now();
        for prim in &primitives {
            let egui::epaint::Primitive::Mesh(mesh) = &prim.primitive else { continue };
            if mesh.indices.is_empty() { continue; }
            // Clip rect in pixels, intersected with the surface.
            let c = prim.clip_rect;
            let x0 = (c.min.x * ppp).floor().max(0.0) as i32;
            let y0 = (c.min.y * ppp).floor().max(0.0) as i32;
            let x1 = ((c.max.x * ppp).ceil() as i32).min(sw as i32);
            let y1 = ((c.max.y * ppp).ceil() as i32).min(sh as i32);
            if x1 <= x0 || y1 <= y0 { continue; }
            self.canvas.set_clip_rect(Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32));
            self.verts.clear();
            self.verts.extend(mesh.vertices.iter().map(|v| {
                let [r, g, b, a] = unmultiply(v.color);
                Vertex { position: FPoint::new(v.pos.x * ppp, v.pos.y * ppp), color: Color::RGBA(r, g, b, a), tex_coord: FPoint::new(v.uv.x, v.uv.y) }
            }));
            let tex = self.textures.get(&mesh.texture_id).map(|(t, _)| t);
            if let Err(e) = self.canvas.render_geometry(&self.verts, tex, VertexIndices::U32(&mesh.indices)) {
                log::warn!("render_geometry: {e}");
            }
        }
        self.canvas.set_clip_rect(None);
        crate::perf_accum("egui_draw", _t_draw.elapsed());
        for id in &full_output.textures_delta.free {
            if let Some((t, _)) = self.textures.remove(id) { unsafe { t.destroy(); } }
        }

        // ── Capture ──────────────────────────────────────────────────────
        if let Some(path) = self.capture.take() {
            match self.write_capture(&path) {
                Ok(())  => log::info!("captured {}x{} frame → {}", sw, sh, path.display()),
                Err(e)  => log::error!("capture failed: {e:#}"),
            }
        }

        let t_present = Instant::now();
        self.canvas.present();
        crate::perf_accum("present", t_present.elapsed());
    }
}

impl Drop for SdlRenderer {
    fn drop(&mut self) {
        for (_, (t, _)) in self.textures.drain() { unsafe { t.destroy(); } }
        if let Some((t, _, _)) = self.wave_tex.take() { unsafe { t.destroy(); } }
        if let Some((t, _, _)) = self.over_tex.take() { unsafe { t.destroy(); } }
    }
}
