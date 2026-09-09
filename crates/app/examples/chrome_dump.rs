//! Dump the procedurally shaded chrome sprites to PNGs for eyeballing.
//!
//!     cargo run -p opendeck-app --example chrome_dump --release -- out_dir [jog_px]
use opendeck_app::chrome::{self, RoundKind};
use std::path::Path;

fn save(dir: &Path, name: &str, img: &egui::ColorImage) {
    let [w, h] = img.size;
    let mut raw = Vec::with_capacity(w * h * 4);
    for p in &img.pixels { raw.extend_from_slice(&[p.r(), p.g(), p.b(), p.a()]); }
    let f = std::fs::File::create(dir.join(name)).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&raw).unwrap();
    println!("{name}: {w}x{h}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(args.get(1).map(String::as_str).unwrap_or("."));
    let jog_px: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1200);
    std::fs::create_dir_all(dir).unwrap();
    let t = std::time::Instant::now();
    save(dir, "jog.png", &chrome::render_jog(jog_px));
    println!("jog bake: {:.0} ms", t.elapsed().as_secs_f32() * 1e3);
    let green = [0.10, 0.95, 0.25];
    let orange = [1.00, 0.42, 0.06];
    save(dir, "play-off.png", &chrome::render_round(240, RoundKind::Silver, None));
    save(dir, "play-on.png",  &chrome::render_round(240, RoundKind::Silver, Some(green)));
    save(dir, "cue-on.png",   &chrome::render_round(240, RoundKind::Silver, Some(orange)));
    save(dir, "reloop.png",   &chrome::render_round(100, RoundKind::Black, None));
    save(dir, "mt-off.png",   &chrome::render_round(80, RoundKind::Lamp, None));
    save(dir, "mt-on.png",    &chrome::render_round(80, RoundKind::Lamp, Some(orange)));
    save(dir, "browse.png",   &chrome::render_round(200, RoundKind::Knob, None));
    save(dir, "loop-off.png", &chrome::render_square(104, chrome::LOOP_ASPECT, [1.0, 0.62, 0.10], false));
    save(dir, "loop-on.png",  &chrome::render_square(104, chrome::LOOP_ASPECT, [1.0, 0.62, 0.10], true));
}
