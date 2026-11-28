//! A deck with no window of its own: a plugin host shows its screen inside
//! the host's (Overdeck's display contract, `overdeck-display`).
//!
//! The host made a shared-memory screen and named it in
//! `PluginHost::display`.  The deck draws each frame off-screen at the
//! screen's size and copies it in; the touches the host's screen writes back
//! (0..1 of the frame) come in as this deck's pointer.

use overdeck_display::{Screen, Touch};
use std::cell::RefCell;
use std::rc::Rc;
use opendeck_types::time::Instant;

pub struct FrameHost {
    screen: Rc<RefCell<Screen>>,
    pub size_px: (u32, u32),
    pub ppp: f32,
    events: Vec<egui::Event>,
    start: Instant,
}

impl FrameHost {
    /// Attach to the screen at `path`; the layout is `layout_w` points across
    /// (the deck's screen design), so a smaller screen draws it smaller.
    pub fn open(path: &std::path::Path, layout_w: u32) -> anyhow::Result<FrameHost> {
        let screen = Screen::open(path)?;
        let (w, h) = screen.size();
        let ppp = w as f32 / layout_w.max(1) as f32;
        log::info!("display: drawing into {} ({w}x{h} px, {ppp:.2} px/pt)", path.display());
        Ok(FrameHost { screen: Rc::new(RefCell::new(screen)), size_px: (w, h), ppp, events: Vec::new(), start: Instant::now() })
    }

    /// Where the renderer's frames go: packed into the screen.
    pub fn sink(&self) -> crate::renderer::FrameSink {
        let screen = Rc::clone(&self.screen);
        Box::new(move |data, w, h, bpr| {
            screen.borrow_mut().write_frame(|px| {
                let row = w as usize * 4;
                for y in 0..h as usize {
                    let src = &data[y * bpr as usize..y * bpr as usize + row];
                    px[y * row..(y + 1) * row].copy_from_slice(src);
                }
            });
        })
    }

    /// Take the screen's touches as pointer events; true if there were any
    /// (the deck draws at once rather than at its idle rate).
    pub fn poll(&mut self) -> bool {
        let (pw, ph) = (self.size_px.0 as f32 / self.ppp, self.size_px.1 as f32 / self.ppp);
        let mut any = false;
        while let Some(t) = self.screen.borrow_mut().pop_touch() {
            any = true;
            let at = |x: f32, y: f32| egui::pos2(x.clamp(0.0, 1.0) * pw, y.clamp(0.0, 1.0) * ph);
            let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            match t {
                Touch::Down(x, y) => { let p = at(x, y); self.events.push(egui::Event::PointerMoved(p)); self.events.push(button(p, true)); }
                Touch::Move(x, y) => self.events.push(egui::Event::PointerMoved(at(x, y))),
                // A finger, not a mouse: nothing hovers once it lifts.
                Touch::Up(x, y) => { let p = at(x, y); self.events.push(button(p, false)); self.events.push(egui::Event::PointerGone); }
            }
        }
        any
    }

    pub fn take_egui_input(&mut self) -> egui::RawInput {
        self.poll();
        let (w, h) = self.size_px;
        let mut raw = egui::RawInput {
            time: Some(self.start.elapsed().as_secs_f64()),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32 / self.ppp, h as f32 / self.ppp))),
            events: std::mem::take(&mut self.events),
            focused: true,
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.ppp);
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(name: &str, w: u32, h: u32) -> (std::path::PathBuf, Screen) {
        let d = std::env::var_os("TMPDIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let p = d.join(format!("framehost-{}-{name}.screen", std::process::id()));
        let s = Screen::create(&p, w, h).unwrap();
        (p, s)
    }

    #[test]
    fn a_tap_on_the_hosts_screen_is_a_press_and_release_where_it_landed() {
        let (p, _host) = screen("tap", 512, 300);
        let mut f = FrameHost::open(&p, 1024).unwrap();
        assert_eq!(f.ppp, 0.5);
        let mut panel = Screen::open(&p).unwrap();
        panel.push_touch(Touch::Down(0.5, 0.25));
        panel.push_touch(Touch::Up(0.5, 0.25));
        let raw = f.take_egui_input();
        assert_eq!(raw.screen_rect.unwrap().size(), egui::vec2(1024.0, 600.0), "the layout is the deck's own size in points");
        let at = egui::pos2(512.0, 150.0);
        assert!(matches!(raw.events[0], egui::Event::PointerMoved(q) if q == at));
        assert!(matches!(raw.events[1], egui::Event::PointerButton { pos, pressed: true, .. } if pos == at));
        assert!(matches!(raw.events[2], egui::Event::PointerButton { pos, pressed: false, .. } if pos == at));
        assert!(matches!(raw.events[3], egui::Event::PointerGone));
        assert!(f.take_egui_input().events.is_empty());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_frame_with_padded_rows_reaches_the_screen_packed() {
        let (p, _host) = screen("frame", 3, 2);
        let f = FrameHost::open(&p, 3).unwrap();
        let mut sink = f.sink();
        // Rows 256 bytes apart (wgpu's alignment), 12 bytes of pixels each.
        let mut gpu = vec![0u8; 512];
        for (i, b) in gpu[..12].iter_mut().enumerate() { *b = i as u8; }
        for (i, b) in gpu[256..268].iter_mut().enumerate() { *b = 100 + i as u8; }
        sink(&gpu, 3, 2, 256);
        let reader = Screen::open(&p).unwrap();
        let mut out = Vec::new();
        assert_eq!(reader.read_frame(&mut out), Some(1));
        assert_eq!(&out[..12], &gpu[..12]);
        assert_eq!(&out[12..24], &gpu[256..268]);
        let _ = std::fs::remove_file(p);
    }
}
