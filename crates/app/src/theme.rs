//! LIGHT MODE (MENU → LIGHT MODE): the deck screen on a white ground.
//!
//! One rule for every pixel of the screen, after it is drawn: keep the hue
//! and saturation, mirror the lightness (HSL L → 1 − L).  Black ground turns
//! white, white text black, greys swap ends; a saturated colour (red
//! playhead, blue lit key, amber waveform band) stays itself, and a dark
//! tint of one (the navy source tile) becomes a pale tint of the same hue.
//! Applying it at the end rather than restyling each widget means every
//! screen, both skins and anything drawn later, get a light look with
//! no second palette to keep in step.
//!
//! The egui shapes are flipped here (`lighten`); the two waveform rects are
//! flipped in the shader's last step and in `waveform_cpu` (`flip_rgb`), on
//! the same sRGB bytes, so the backends agree.  Images (album art) keep
//! their colours.

use egui::epaint::{ClippedShape, Shape};
use egui::{Color32, Rect, TextureId};

/// Mirror the lightness of an sRGB colour, keeping hue and saturation:
/// each channel moves by 255 − max − min.
#[inline(always)]
pub fn flip_rgb(c: [u8; 3]) -> [u8; 3] {
    let hi = c[0].max(c[1]).max(c[2]) as i32;
    let lo = c[0].min(c[1]).min(c[2]) as i32;
    let off = 255 - hi - lo;
    c.map(|v| (v as i32 + off).clamp(0, 255) as u8)
}

/// [`flip_rgb`] on an egui colour (premultiplied): flips the colour, keeps
/// its alpha, so a dark see-through band becomes a light one.
pub fn flip(c: Color32) -> Color32 {
    if c == Color32::PLACEHOLDER || c.a() == 0 { return c; }
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let [r, g, b] = flip_rgb([r, g, b]);
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Flip every shape that lies inside `screen` (the LCD; the faceplate and
/// letterbox around it stay as they are).  Textured shapes are images and
/// keep their colours.
pub fn lighten(shapes: &mut [ClippedShape], screen: Rect) {
    let inside = screen.expand(1.0);
    for s in shapes { lighten_shape(&mut s.shape, inside); }
}

fn lighten_shape(shape: &mut Shape, inside: Rect) {
    match shape {
        Shape::Vec(v) => for s in v { lighten_shape(s, inside) },
        Shape::Mesh(m) if m.texture_id != TextureId::default() => {}
        Shape::Rect(r) if r.fill_texture_id != TextureId::default() => {}
        Shape::Callback(_) | Shape::Noop => {}
        s => {
            if inside.contains_rect(s.visual_bounding_rect()) {
                egui::epaint::shape_transform::adjust_colors(s, |c| *c = flip(*c));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_goes_white_and_colours_keep_their_hue() {
        assert_eq!(flip_rgb([0, 0, 0]), [255, 255, 255]);
        assert_eq!(flip_rgb([255, 255, 255]), [0, 0, 0]);
        assert_eq!(flip_rgb([42, 42, 42]), [213, 213, 213]);
        assert_eq!(flip_rgb([255, 0, 0]), [255, 0, 0], "a full colour is its own mirror");
        // The navy source tile becomes a pale blue: still blue, now light.
        let [r, g, b] = flip_rgb([15, 50, 95]);
        assert!(b > g && g > r && r > 150, "{r},{g},{b}");
        // Twice is the identity.
        for c in [[3, 4, 6], [0, 112, 195], [240, 160, 48], [186, 186, 186]] {
            assert_eq!(flip_rgb(flip_rgb(c)), c);
        }
    }

    #[test]
    fn a_see_through_band_keeps_its_alpha() {
        let c = flip(Color32::from_rgba_unmultiplied(0, 0, 0, 200));
        assert_eq!(c.to_srgba_unmultiplied(), [255, 255, 255, 200]);
        assert_eq!(flip(Color32::TRANSPARENT), Color32::TRANSPARENT);
    }

    #[test]
    fn only_the_screen_flips_and_images_keep_their_colours() {
        let screen = Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(500.0, 400.0));
        let on  = Shape::rect_filled(Rect::from_min_max(egui::pos2(110.0, 110.0), egui::pos2(200.0, 200.0)), 0.0, Color32::BLACK);
        let off = Shape::rect_filled(Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(90.0, 90.0)), 0.0, Color32::BLACK);
        let mut art = egui::Mesh::with_texture(TextureId::User(7));
        art.add_rect_with_uv(Rect::from_min_max(egui::pos2(120.0, 120.0), egui::pos2(140.0, 140.0)),
                             Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        let clip = Rect::EVERYTHING;
        let mut shapes = vec![ClippedShape { clip_rect: clip, shape: on }, ClippedShape { clip_rect: clip, shape: off },
                              ClippedShape { clip_rect: clip, shape: Shape::mesh(art) }];
        lighten(&mut shapes, screen);
        let fill = |s: &Shape| match s { Shape::Rect(r) => r.fill, _ => unreachable!() };
        assert_eq!(fill(&shapes[0].shape), Color32::WHITE, "screen ground turns white");
        assert_eq!(fill(&shapes[1].shape), Color32::BLACK, "faceplate untouched");
        match &shapes[2].shape { Shape::Mesh(m) => assert!(m.vertices.iter().all(|v| v.color == Color32::WHITE)), _ => unreachable!() }
    }
}
