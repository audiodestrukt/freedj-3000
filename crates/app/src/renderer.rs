//! wgpu + egui renderer for the MVP waveform display.
//!
//! Two render passes per frame:
//!   Pass 1 — custom waveform shader (fullscreen quad, scrolling storage buffer)
//!   Pass 2 — egui overlay (time counter, play state, instructions)

use anyhow::{Context, Result};
use opendeck_analysis::WaveformCache;
use crate::snapshot::DeckSnapshot;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

// ── Uniform struct ────────────────────────────────────────────────────────────

/// Sent to the waveform shader every frame.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct WaveformParams {
    /// Index of the column at the horizontal centre of the screen (float).
    playhead_col:     f32,
    /// How many columns are visible across the full screen width.
    cols_visible:     f32,
    /// Total number of valid columns in the buffer.
    num_cols:         f32,
    /// Surface width in pixels.
    screen_w:         f32,
    /// Surface height in pixels.
    screen_h:         f32,
    /// Beat grid: column index of anchor beat (0 if no grid).
    beat_anchor_col:  f32,
    /// Beat grid: columns per beat (0 if no grid).
    beat_period_cols: f32,
    /// Which beat within the bar beat 0 falls on (0 = beat 0 is a downbeat).
    downbeat_offset:  f32,
    /// Beats per bar (4 for 4/4).
    beats_per_bar:    f32,
    /// Second beat grid: fractional beat phase 0.0–1.0 (wall-clock time).
    beat2_phase_beats: f32,
    /// Second beat grid: columns per beat (0 if disabled).
    beat2_period_cols: f32,
    /// Waveform colour mode: 0 = RGB, 1 = 3 BAND, 2 = BLUE.
    color_mode:       f32,
    /// Enlarged-waveform rect in physical pixels: x, y, w, h.
    wave_rect:        [f32; 4],
    /// Overview-waveform rect in physical pixels: x, y, w, h.
    over_rect:        [f32; 4],
    /// Multiplier that brings the track's loudest column to its display height.
    amp_gain:         f32,
    /// 1.0 = dim the played part of the overview (REMAIN mode).
    dim_played:       f32,
    /// Start-cue column (source position), orange marker.
    cue_col:          f32,
    /// Active loop as columns; end <= start means no loop.  The looped span
    /// gets a tinted background and amber in/out lines on both waveforms.
    loop_start_col:   f32,
    loop_end_col:     f32,
    _pad:             [f32; 3],
}

/// Where the shader draws its two waveforms, in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Viewports {
    pub wave:     [f32; 4],
    pub overview: [f32; 4],
    /// REMAIN mode: the played part of the overview turns off.
    pub dim_played: bool,
}

/// Waveform colour scheme, matching the CDJ `Waveform Color` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode { Rgb = 0, ThreeBand = 1, Blue = 2 }

// ── Renderer ──────────────────────────────────────────────────────────────────

pub struct Renderer {
    surface:        wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device:         wgpu::Device,
    queue:          wgpu::Queue,

    // Waveform pass
    waveform_pipeline:   wgpu::RenderPipeline,
    waveform_bind_group: wgpu::BindGroup,
    waveform_bgl:        wgpu::BindGroupLayout,
    params_buf:          wgpu::Buffer,
    num_cols:            u32,

    // egui pass
    egui_renderer:  egui_wgpu::Renderer,
    egui_screen:    egui_wgpu::ScreenDescriptor,

    /// Largest surface dimension this device can back with a texture.
    max_dim:        u32,

    /// Kept so `render` can call `pre_present_notify` before each present.
    window:         Arc<Window>,

    pub color_mode: ColorMode,
    /// Waveform columns visible across the enlarged waveform (zoom).
    pub cols_visible: f32,
    /// Normalises bar height to the track's peak so quiet masters still fill
    /// the display, as they do on a CDJ.
    amp_gain:       f32,
    /// When set, the next frame is also written to this PNG path.
    capture:        Option<std::path::PathBuf>,
}

impl Renderer {
    pub async fn new(window: Arc<Window>, waveform: &WaveformCache) -> Result<Self> {
        let size = window.inner_size();

        // ── wgpu instance / surface ───────────────────────────────────────────
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends:              wgpu::Backends::all(),
            dx12_shader_compiler:  wgpu::Dx12Compiler::default(),
            gles_minor_version:    wgpu::Gles3MinorVersion::Automatic,
            flags:                 wgpu::InstanceFlags::default(),
        });

        let surface = instance
            .create_surface(Arc::clone(&window))
            .context("failed to create wgpu surface")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference:       wgpu::PowerPreference::HighPerformance,
                compatible_surface:     Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("no compatible GPU adapter found")?;

        let info = adapter.get_info();
        log::info!(
            "GPU: {} ({:?}, {:?}) via {:?}",
            info.name, info.device_type, info.driver, info.backend,
        );

        // Keep the conservative downlevel feature set — it is what the Pi 5 /
        // GLES fallback can offer — but raise the *resolution* limits to what
        // this adapter actually supports.  `downlevel_defaults()` alone caps
        // max_texture_dimension_2d at 2048, which fails Surface::configure on
        // any display wider than 2048 physical pixels regardless of GPU.
        let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        let max_dim = limits.max_texture_dimension_2d;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label:             Some("opendeck"),
                    required_features: wgpu::Features::empty(),
                    required_limits:   limits,
                    memory_hints:      wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
            .context("failed to get GPU device")?;

        // ── Surface configuration ─────────────────────────────────────────────
        let caps   = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        // Frame pacing.
        //
        // The compositor is the clock.  On Wayland, winit only requests the
        // compositor's frame callback if the app calls `pre_present_notify()`
        // before presenting; without that call `request_redraw()` fires
        // immediately and the loop free-runs — measured at 12,500 fps with a
        // non-blocking swapchain.  With the callback in place, RedrawRequested
        // arrives once per refresh, phase-locked to the display.
        //
        // Given that, the swapchain must NOT add a second throttle.  Mailbox
        // never blocks on acquire: the compositor takes the newest image each
        // refresh and we render exactly one per callback.  Fifo was tried as
        // the sole pacer instead: at frame latency 1 it missed 18% of vsyncs
        // (compositor buffer release arrives after the deadline), at latency 2
        // it delivered frames in bursts.  Fifo remains the fallback for
        // platforms without Mailbox.
        // OPENDECK_PRESENT=fifo|mailbox|immediate overrides the default, for
        // measuring the render-thread busy-wait: with Mailbox the compositor
        // frame callback paces us but the winit/wayland loop can spin between
        // frames instead of sleeping (measured 92% of a core at 30fps on the
        // Pi 4 with a 7ms frame); Fifo blocks the thread on vsync inside
        // present(), which sleeps it.
        let want = std::env::var("OPENDECK_PRESENT").ok().map(|v| v.to_lowercase());
        let has = |m| caps.present_modes.contains(&m);
        let present_mode = match want.as_deref() {
            Some("fifo")      => wgpu::PresentMode::Fifo,
            Some("immediate") if has(wgpu::PresentMode::Immediate) => wgpu::PresentMode::Immediate,
            Some("mailbox")   if has(wgpu::PresentMode::Mailbox)   => wgpu::PresentMode::Mailbox,
            _ if has(wgpu::PresentMode::Mailbox)                   => wgpu::PresentMode::Mailbox,
            _                 => wgpu::PresentMode::Fifo,
        };
        log::info!("present mode: {present_mode:?} (available: {:?})", caps.present_modes);

        let surface_config = wgpu::SurfaceConfiguration {
            usage:                        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format,
            width:                        size.width.clamp(1, max_dim),
            height:                       size.height.clamp(1, max_dim),
            present_mode,
            alpha_mode:                   caps.alpha_modes[0],
            view_formats:                 vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // ── Waveform storage buffer ───────────────────────────────────────────
        // Pack each [R,G,B,A] column into a single u32 (little-endian bytes).
        // No texture dimension limits — storage buffers handle arbitrary sizes.
        let num_cols = waveform.len() as u32;
        let mut waveform_data: Vec<u32> = waveform.columns.iter()
            .map(|col| u32::from_le_bytes(*col))
            .collect();
        // wgpu rejects a zero-sized storage buffer, so an empty deck (no track
        // yet) needs at least one column here; `num_cols` stays the true count
        // (0), so the shader simply draws nothing until a track is loaded.
        if waveform_data.is_empty() { waveform_data.push(0); }

        // Peak amplitude byte across the track sets the display gain.
        let peak = waveform_data.iter().map(|v| (v >> 24) & 0xFF).max().unwrap_or(255) as f32 / 255.0;
        let amp_gain = (0.62 / peak.max(0.05)).min(6.0);
        log::info!("waveform peak {:.2} → display gain {:.2}", peak, amp_gain);

        let waveform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some("waveform_data"),
            contents: bytemuck::cast_slice(&waveform_data),
            usage:    wgpu::BufferUsages::STORAGE,
        });

        // ── Params uniform buffer ─────────────────────────────────────────────
        let initial_params = WaveformParams {
            playhead_col:     0.0,
            cols_visible:     600.0,
            num_cols:         num_cols as f32,
            screen_w:         size.width as f32,
            screen_h:         size.height as f32,
            beat_anchor_col:   0.0,
            beat_period_cols:  0.0,
            downbeat_offset:   0.0,
            beats_per_bar:     4.0,
            beat2_phase_beats: 0.0,
            beat2_period_cols: 0.0,
            color_mode:       ColorMode::Blue as u32 as f32,
            wave_rect:        [0.0; 4],
            over_rect:        [0.0; 4],
            amp_gain:         1.0,
            dim_played:       0.0,
            cue_col:          0.0,
            loop_start_col:   0.0,
            loop_end_col:     0.0,
            _pad:             [0.0; 3],
        };
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some("waveform_params"),
            contents: bytemuck::bytes_of(&initial_params),
            usage:    wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // ── Bind group layout ─────────────────────────────────────────────────
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:   Some("waveform_bgl"),
            entries: &[
                // binding 0: waveform storage buffer (read-only)
                wgpu::BindGroupLayoutEntry {
                    binding:    0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count: None,
                },
                // binding 1: params uniform
                wgpu::BindGroupLayoutEntry {
                    binding:    1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty:                 wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count: None,
                },
            ],
        });

        let waveform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:  Some("waveform_bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: waveform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: params_buf.as_entire_binding() },
            ],
        });

        // ── Waveform render pipeline ──────────────────────────────────────────
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label:  Some("waveform_shader"),
            source: wgpu::ShaderSource::Wgsl(WAVEFORM_WGSL.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label:                Some("waveform_layout"),
            bind_group_layouts:   &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let waveform_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:  Some("waveform_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module:      &shader,
                entry_point: "vs_main",
                buffers:     &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module:      &shader,
                entry_point: "fs_main",
                targets:     &[Some(wgpu::ColorTargetState {
                    format:     format,
                    blend:      None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology:  wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil:  None,
            multisample:    wgpu::MultisampleState::default(),
            multiview:      None,
            cache:          None,
        });

        // ── egui renderer ─────────────────────────────────────────────────────
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        let scale_factor  = window.scale_factor() as f32;
        let egui_screen   = egui_wgpu::ScreenDescriptor {
            size_in_pixels:   [size.width, size.height],
            pixels_per_point: scale_factor,
        };

        Ok(Self {
            surface,
            surface_config,
            device,
            queue,
            waveform_pipeline,
            waveform_bind_group,
            waveform_bgl: bind_group_layout,
            params_buf,
            num_cols,
            egui_renderer,
            egui_screen,
            max_dim,
            window,
            color_mode: ColorMode::Blue,
            cols_visible: crate::input::ZOOM_LEVELS[crate::input::ZOOM_DEFAULT],
            amp_gain,
            capture:    None,
        })
    }

    /// Replace the waveform on the GPU with a freshly-analysed track: rebuild
    /// the storage buffer, recompute the display gain, and rebind.  Called by
    /// the deck when the browser loads a new track.  The params/pipeline are
    /// untouched — only binding 0 (the waveform data) changes.
    pub fn set_waveform(&mut self, waveform: &WaveformCache) {
        let num_cols = waveform.len() as u32;
        let mut data: Vec<u32> = waveform.columns.iter().map(|c| u32::from_le_bytes(*c)).collect();
        if data.is_empty() { data.push(0); }   // wgpu rejects a zero-sized storage buffer
        let peak = data.iter().map(|v| (v >> 24) & 0xFF).max().unwrap_or(255) as f32 / 255.0;
        self.amp_gain = (0.62 / peak.max(0.05)).min(6.0);
        self.num_cols = num_cols;
        log::info!("waveform reloaded: {} columns, peak {:.2} → gain {:.2}", num_cols, peak, self.amp_gain);

        let waveform_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some("waveform_data"),
            contents: bytemuck::cast_slice(&data),
            usage:    wgpu::BufferUsages::STORAGE,
        });
        self.waveform_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:  Some("waveform_bg"),
            layout: &self.waveform_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: waveform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.params_buf.as_entire_binding() },
            ],
        });
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        // Never hand the surface a size the device cannot back with a texture —
        // Surface::configure panics rather than returning an error.
        let width  = width.min(self.max_dim);
        let height = height.min(self.max_dim);
        self.surface_config.width  = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.egui_screen.size_in_pixels = [width, height];
    }

    /// Write the next rendered frame to `path` as a PNG.
    pub fn request_capture(&mut self, path: std::path::PathBuf) {
        self.capture = Some(path);
    }

    pub fn render(
        &mut self,
        snap:        &DeckSnapshot,
        vp:          &Viewports,
        egui_ctx:    &egui::Context,
        full_output: egui::FullOutput,
    ) {
        let playhead_sample   = snap.position;
        let sample_rate       = snap.sample_rate;
        let channels          = snap.channels;
        let beat_grid         = snap.beat_grid;
        let beat2_bpm         = snap.beat2_bpm;
        let beat2_phase_beats = snap.beat2_phase_beats;
        let pixels_per_point = full_output.pixels_per_point;
        let egui_shapes      = full_output.shapes;
        let textures_delta   = full_output.textures_delta;

        // ── Update waveform scroll params ─────────────────────────────────────
        let hop_size     = opendeck_analysis::waveform::HOP_SIZE as f32;
        let playhead_col = playhead_sample as f32 / channels as f32 / hop_size;

        let (beat_anchor_col, beat_period_cols, downbeat_offset, beats_per_bar) = beat_grid
            .map(|g| {
                let anchor = g.anchor_sample as f32 / hop_size;
                let period = (sample_rate as f32 * 60.0 / g.bpm as f32) / hop_size;
                (anchor, period, g.downbeat_offset as f32, 4.0f32)
            })
            .unwrap_or((0.0, 0.0, 0.0, 4.0));

        // The cyan B2 beat strip under the waveform is not an XDJ-1000 element;
        // it was a bring-up aid for external-sync testing.  Disabled for
        // faithful reproduction — the external deck now lives only in the
        // phase meter.  Set to the commented expression to bring it back.
        //   (sample_rate as f32 * 60.0 / beat2_bpm) / hop_size * fader_speed
        let _ = (beat2_bpm, beat2_phase_beats);
        let beat2_period_cols = 0.0;

        let params = WaveformParams {
            playhead_col,
            cols_visible:      self.cols_visible,
            num_cols:          self.num_cols as f32,
            screen_w:          self.surface_config.width  as f32,
            screen_h:          self.surface_config.height as f32,
            beat_anchor_col,
            beat_period_cols,
            downbeat_offset,
            beats_per_bar,
            beat2_phase_beats,
            beat2_period_cols,
            color_mode:        self.color_mode as u32 as f32,
            wave_rect:         vp.wave,
            over_rect:         vp.overview,
            amp_gain:          self.amp_gain,
            dim_played:        if vp.dim_played { 1.0 } else { 0.0 },
            cue_col:           snap.cue_point as f32 / channels as f32 / hop_size,
            loop_start_col:    if snap.loop_active { snap.loop_start as f32 / channels as f32 / hop_size } else { 0.0 },
            loop_end_col:      if snap.loop_active { snap.loop_end   as f32 / channels as f32 / hop_size } else { 0.0 },
            _pad:              [0.0; 3],
        };
        self.queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&params));

        // ── Get surface texture ───────────────────────────────────────────────
        // Where a frame's time goes is the difference between "vsync-paced" and
        // "timer-paced with a compositor guessing".  Acquire is where a Fifo
        // swapchain blocks when it is genuinely throttling to the display.
        let t_acquire = std::time::Instant::now();
        let output = match self.surface.get_current_texture() {
            Ok(t)  => t,
            // A Lost/Outdated surface is recoverable — reconfigure and skip this
            // frame; the next acquire succeeds.  This self-heals a surface the OS
            // invalidated (iOS background→foreground) even if the Occluded path
            // didn't already reconfigure it, so we never spin on a dead surface.
            Err(e @ (wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated)) => {
                log::warn!("surface {e} — reconfiguring");
                self.surface.configure(&self.device, &self.surface_config);
                return;
            }
            Err(e) => { log::warn!("surface error: {e}"); return; }
        };
        let acquire_ms = t_acquire.elapsed().as_secs_f64() * 1000.0;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        // ── Pass 1: waveform ──────────────────────────────────────────────────
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("waveform_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view:           &view,
                    resolve_target: None,
                    ops:            wgpu::Operations {
                        load:  wgpu::LoadOp::Clear(wgpu::Color { r: 0.0015, g: 0.002, b: 0.003, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set:      None,
                timestamp_writes:         None,
            });
            pass.set_pipeline(&self.waveform_pipeline);
            pass.set_bind_group(0, &self.waveform_bind_group, &[]);
            pass.draw(0..3, 0..1);  // fullscreen triangle
        }

        // ── Pass 2: egui overlay ──────────────────────────────────────────────
        for (id, delta) in &textures_delta.set {
            self.egui_renderer.update_texture(&self.device, &self.queue, *id, delta);
        }
        for id in &textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        let _t_tess = std::time::Instant::now();
        let primitives = egui_ctx.tessellate(egui_shapes, pixels_per_point);
        crate::perf_accum("tessellate", _t_tess.elapsed());
        let _t_upl = std::time::Instant::now();
        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &primitives,
            &self.egui_screen,
        );
        crate::perf_accum("upload_bufs", _t_upl.elapsed());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view:           &view,
                        resolve_target: None,
                        ops:            wgpu::Operations {
                            load:  wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set:      None,
                    timestamp_writes:         None,
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &primitives, &self.egui_screen);
        }

        // ── Optional frame capture ────────────────────────────────────────────
        let capture = self.capture.take().map(|path| {
            let (w, h) = (self.surface_config.width, self.surface_config.height);
            let bpr = ((w * 4 + 255) / 256) * 256;   // COPY_BYTES_PER_ROW_ALIGNMENT
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("capture"),
                size:  (bpr * h) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::ImageCopyTexture {
                    texture: &output.texture, mip_level: 0,
                    origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All,
                },
                wgpu::ImageCopyBuffer {
                    buffer: &buf,
                    layout: wgpu::ImageDataLayout {
                        offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            (path, buf, w, h, bpr)
        });

        let _t_submit = std::time::Instant::now();
        self.queue.submit(std::iter::once(encoder.finish()));
        crate::perf_accum("submit", _t_submit.elapsed());

        if let Some((path, buf, w, h, bpr)) = capture {
            let slice = buf.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            self.device.poll(wgpu::Maintain::Wait);
            let data = slice.get_mapped_range();
            let bgra = self.surface_config.format.remove_srgb_suffix() == wgpu::TextureFormat::Bgra8Unorm;
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for row in 0..h {
                let r = &data[(row * bpr) as usize..(row * bpr + w * 4) as usize];
                for px in r.chunks_exact(4) {
                    if bgra { rgba.extend_from_slice(&[px[2], px[1], px[0], 255]); }
                    else    { rgba.extend_from_slice(&[px[0], px[1], px[2], 255]); }
                }
            }
            drop(data);
            buf.unmap();
            let written = std::fs::File::create(&path).map_err(anyhow::Error::from).and_then(|f| {
                let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                let mut wr = enc.write_header()?;
                wr.write_image_data(&rgba)?;
                Ok(())
            });
            match written {
                Ok(())  => log::info!("captured {}x{} frame → {}", w, h, path.display()),
                Err(e)  => log::error!("capture failed: {e}"),
            }
        }

        let t_present = std::time::Instant::now();
        // Required on Wayland: this is what requests the compositor frame
        // callback that paces the next RedrawRequested.  See Renderer::new.
        self.window.pre_present_notify();
        output.present();
        crate::perf_accum("present", t_present.elapsed());
        crate::perf_accum("acquire", std::time::Duration::from_secs_f64(acquire_ms / 1000.0));
    }
}

// ── WGSL shader ───────────────────────────────────────────────────────────────

const WAVEFORM_WGSL: &str = r#"
// Waveform display shader — draws the enlarged waveform and the overview
// waveform into two rects; everything outside them is ground for egui.
//
// Waveform data is a storage buffer of u32 values, one per column.
// Each u32 packs [low, mid, high, amp] as little-endian bytes.

struct Params {
    playhead_col:       f32,  // column index at the enlarged-waveform centre
    cols_visible:       f32,  // columns visible across the enlarged waveform
    num_cols:           f32,  // number of valid columns in the buffer
    screen_w:           f32,
    screen_h:           f32,
    beat_anchor_col:    f32,  // column of beat 0 (0 = no grid)
    beat_period_cols:   f32,  // columns per beat (0 = no grid)
    downbeat_offset:    f32,  // which beat within the bar is beat 0
    beats_per_bar:      f32,  // 4 for 4/4
    beat2_phase_beats:  f32,  // external deck beat phase 0.0–1.0
    beat2_period_cols:  f32,  // external deck columns per beat (0 = disabled)
    color_mode:         f32,  // 0 RGB, 1 3-BAND, 2 BLUE
    wave_rect:          vec4<f32>,
    over_rect:          vec4<f32>,
    amp_gain:           f32,  // brings the track peak to its display height
    dim_played:         f32,  // 1 = REMAIN mode: played part of the overview turns off
    cue_col:            f32,  // start-cue column (orange marker)
    loop_start_col:     f32,  // active loop span in columns; end <= start = none
    loop_end_col:       f32,
    _p1: f32, _p2: f32, _p3: f32,
};

@group(0) @binding(0) var<storage, read> waveform: array<u32>;
@group(0) @binding(1) var<uniform> p: Params;

const B2_STRIP_PX: f32 = 22.0;   // external-deck beat strip along the bottom of the enlarged waveform

// The surface is sRGB, so the shader outputs linear.  Author colours in sRGB
// (what you see) and convert once here.
fn srgb(r: f32, g: f32, b: f32) -> vec4<f32> {
    let c = vec3<f32>(r, g, b) / 255.0;
    return vec4<f32>(pow(c, vec3<f32>(2.2)), 1.0);
}
fn ground() -> vec4<f32>   { return srgb(3.0, 4.0, 6.0); }      // matches screen.rs BG
fn wave_bg() -> vec4<f32>  { return ground(); }                 // the unit draws the waveform on bare ground
fn over_bg() -> vec4<f32>  { return ground(); }
fn playhead() -> vec4<f32> { return srgb(232.0, 40.0, 40.0); }  // red on the unit
fn cue_color() -> vec4<f32> { return srgb(240.0, 138.0, 30.0); }  // orange start-cue marker
fn loop_color() -> vec4<f32> { return srgb(250.0, 200.0, 40.0); } // amber loop in/out lines (the unit's yellow LOOP keys)
fn loop_bg() -> vec4<f32> { return srgb(16.0, 40.0, 66.0); }      // tinted background across the looped span
fn in_loop(col: f32) -> bool { return p.loop_end_col > p.loop_start_col && col >= p.loop_start_col && col < p.loop_end_col; }
fn white() -> vec4<f32>    { return srgb(244.0, 246.0, 248.0); }
fn band_low() -> vec4<f32> { return srgb(58.0, 123.0, 240.0); }  // 3-band blue
fn band_mid() -> vec4<f32> { return srgb(240.0, 160.0, 48.0); }  // 3-band amber
// BLUE mode: blue body tinted toward white where the high band is strong,
// which is how the unit's blue waveform gets its bright transient spikes.
fn blue_wave(s: f32, hi: f32) -> vec4<f32> {
    let t = clamp(hi * 2.4 - 0.15, 0.0, 0.9);
    return srgb(mix(40.0, 235.0, t) * s, mix(125.0, 240.0, t) * s, mix(235.0, 250.0, t) * s);
}
fn cyan() -> vec4<f32>     { return srgb(0.0, 216.0, 216.0); }
fn strip_bg() -> vec4<f32> { return srgb(0.0, 22.0, 40.0); }
fn tick() -> vec4<f32>     { return srgb(90.0, 96.0, 104.0); }
fn tick_hi() -> vec4<f32>  { return srgb(230.0, 232.0, 236.0); }
fn down() -> vec4<f32>     { return srgb(120.0, 40.0, 40.0); }
fn down_hi() -> vec4<f32>  { return srgb(255.0, 60.0, 60.0); }

// ── Vertex: fullscreen triangle ───────────────────────────────────────────────
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32(vi & 1u) * 4.0 - 1.0;
    let y = f32((vi >> 1u) & 1u) * (-4.0) + 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// ── Helpers ───────────────────────────────────────────────────────────────────
fn in_rect(q: vec2<f32>, r: vec4<f32>) -> bool {
    return q.x >= r.x && q.x < r.x + r.z && q.y >= r.y && q.y < r.y + r.w;
}

fn unpack(v: u32) -> vec4<f32> {
    return vec4<f32>(
        f32( v         & 0xFFu),
        f32((v >>  8u) & 0xFFu),
        f32((v >> 16u) & 0xFFu),
        f32((v >> 24u) & 0xFFu),
    ) / 255.0;
}

// Colour of a bar pixel at normalised distance `dist` from the centre line
// (0 = centre, 1 = edge), given band energies c = (low, mid, high, amp).
// Alpha 0 means "not part of the bar".
fn bar_color(c_in: vec4<f32>, dist: f32) -> vec4<f32> {
    let c    = vec4<f32>(c_in.xyz, min(c_in.w * p.amp_gain, 1.0));
    let mode = u32(p.color_mode + 0.5);
    if mode == 1u {
        // 3 BAND: the dominant band reaches the full amplitude; the others are
        // stacked inside it in proportion.  White highs at the core, amber
        // mids, blue lows outermost — the Pioneer look.
        let m = max(c.x, max(c.y, c.z)) + 0.001;
        let h = c.xyz / m * c.w;
        if dist < h.z { return white(); }
        if dist < h.y { return band_mid(); }
        if dist < h.x { return band_low(); }
        return vec4<f32>(0.0);
    }
    if dist >= c.w { return vec4<f32>(0.0); }
    let shade = 1.0 - dist / (c.w + 0.001) * 0.3;
    if mode == 2u {
        return blue_wave(shade, c.z);
    }
    return vec4<f32>(pow(c.xyz * shade, vec3<f32>(2.2)), 1.0);
}

// ── Enlarged waveform ─────────────────────────────────────────────────────────
fn draw_wave(q: vec2<f32>) -> vec4<f32> {
    let r  = p.wave_rect;
    let sx = (q.x - r.x) / r.z;

    // External-deck beat strip along the bottom edge, wall-clock driven so it
    // does not scroll with the audio.
    if q.y > r.y + r.w - B2_STRIP_PX && p.beat2_period_cols > 0.0 {
        let pixels_per_beat = p.beat2_period_cols * r.z / p.cols_visible;
        let phase_px        = p.beat2_phase_beats * pixels_per_beat;
        let beat_x = (((q.x - r.x) + phase_px) % pixels_per_beat + pixels_per_beat) % pixels_per_beat;
        if beat_x < 3.0 || beat_x > pixels_per_beat - 3.0 {
            return cyan();
        }
        return strip_bg();
    }

    // Playhead line at the horizontal centre — red, full height, as on the unit.
    if abs(q.x - (r.x + r.z * 0.5)) < 1.5 {
        return playhead();
    }

    let wave_h = r.w - select(0.0, B2_STRIP_PX, p.beat2_period_cols > 0.0);
    let sy     = (q.y - r.y) / wave_h;

    let half  = p.cols_visible * 0.5;
    let col_f = (p.playhead_col - half) + sx * p.cols_visible;
    // Start-cue marker — a ~2px orange line at the cue column.
    let cue_per_px = p.cols_visible / r.z;
    if abs(col_f - p.cue_col) < cue_per_px {
        return cue_color();
    }
    // Loop in / out lines (amber), when a loop is active.
    let looped = in_loop(col_f);
    if p.loop_end_col > p.loop_start_col
       && (abs(col_f - p.loop_start_col) < cue_per_px || abs(col_f - p.loop_end_col) < cue_per_px) {
        return loop_color();
    }
    let bg = select(wave_bg(), loop_bg(), looped);   // tint the looped span's field
    if col_f < 0.0 || col_f >= p.num_cols {
        return bg;
    }

    // Bilinear between adjacent columns so the scroll is sub-pixel smooth.
    let col_lo = u32(col_f);
    let col_hi = min(col_lo + 1u, u32(p.num_cols) - 1u);
    let c      = mix(unpack(waveform[col_lo]), unpack(waveform[col_hi]), col_f - f32(col_lo));

    let dist = abs(sy - 0.5) * 2.0;
    let bar  = bar_color(c, dist);
    if bar.a > 0.0 {
        return bar;
    }

    // Beat grid as edge ticks only — top and bottom of the field, red and
    // taller on downbeats, white on beats.  No full-height lines: that is how
    // the unit draws it.
    if p.beat_period_cols > 0.0 {
        let rel      = col_f - p.beat_anchor_col;
        let beat_pos = ((rel % p.beat_period_cols) + p.beat_period_cols) % p.beat_period_cols;
        let beat_num = floor(rel / p.beat_period_cols);
        let bpb      = p.beats_per_bar;
        let adjusted = ((beat_num + p.downbeat_offset) % bpb + bpb) % bpb;
        let is_down  = adjusted < 0.5;
        // Tick width is fixed in PIXELS, then converted to columns for the
        // `beat_pos` (column-space) test — otherwise the line thins or fattens
        // as the zoom (cols_visible) changes the pixels-per-column ratio.
        let cols_per_px = p.cols_visible / r.z;
        let tick_w_px   = select(1.5, 2.5, is_down);
        let tick_w      = tick_w_px * cols_per_px;
        // Draw at each beat's LEADING edge only.  Drawing both edges painted the
        // (wider) downbeat colour at the beat's trailing edge too, which showed
        // as a second red line one beat after every downbeat — the "doubled"
        // bar ticks.  One tick per boundary, coloured by the beat starting there.
        if beat_pos < tick_w {
            let len  = select(0.055, 0.10, is_down) * wave_h;
            let edge = (q.y - r.y) < len || (q.y - r.y) > wave_h - len;
            if edge {
                return select(tick_hi(), down_hi(), is_down);
            }
        }
    }
    return bg;
}

// ── Overview waveform ─────────────────────────────────────────────────────────
fn draw_overview(q: vec2<f32>) -> vec4<f32> {
    let r  = p.over_rect;
    let sx = (q.x - r.x) / r.z;
    let sy = (q.y - r.y) / r.w;

    // Playhead marker.
    let ph_x = r.x + p.playhead_col / p.num_cols * r.z;
    if abs(q.x - ph_x) < 1.0 {
        return white();
    }
    let cue_x = r.x + p.cue_col / p.num_cols * r.z;
    if abs(q.x - cue_x) < 1.5 {
        return cue_color();
    }
    // Loop in / out lines and the tinted looped span.
    let col_here = sx * p.num_cols;
    if p.loop_end_col > p.loop_start_col {
        let ls_x = r.x + p.loop_start_col / p.num_cols * r.z;
        let le_x = r.x + p.loop_end_col   / p.num_cols * r.z;
        if abs(q.x - ls_x) < 1.5 || abs(q.x - le_x) < 1.5 {
            return loop_color();
        }
    }
    let over_field = select(over_bg(), loop_bg(), in_loop(col_here));

    // Each pixel column covers many waveform columns; take the peak of each
    // band so transients survive the downsample instead of aliasing away.
    let cols_per_px = p.num_cols / r.z;
    let c0 = u32(sx * p.num_cols);
    let n  = min(u32(cols_per_px) + 1u, 64u);
    var c  = vec4<f32>(0.0);
    for (var i = 0u; i < n; i = i + 1u) {
        let idx = min(c0 + i, u32(p.num_cols) - 1u);
        c = max(c, unpack(waveform[idx]));
    }

    let dist = abs(sy - 0.5) * 2.0;
    let bar  = bar_color(c, dist);
    if bar.a > 0.0 {
        // REMAIN mode: the played part turns off from the left.  TIME mode
        // leaves the whole graph lit.
        return select(bar, bar * vec4<f32>(0.18, 0.18, 0.18, 1.0), q.x < ph_x && p.dim_played > 0.5);
    }
    return over_field;
}

// ── Fragment ──────────────────────────────────────────────────────────────────
@fragment
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
    let q = frag_pos.xy;
    if in_rect(q, p.wave_rect) { return draw_wave(q); }
    if in_rect(q, p.over_rect) { return draw_overview(q); }
    return ground();
}
"#;
