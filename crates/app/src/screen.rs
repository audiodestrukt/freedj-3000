//! Screen chrome — everything drawn with egui on top of the waveform pass.
//!
//! Takes a [`DeckSnapshot`] and draws; holds no deck state of its own.

use crate::snapshot::DeckSnapshot;

/// Development overlay: one status line across the top.
pub fn draw(ctx: &egui::Context, snap: &DeckSnapshot) {
    let elapsed_s = snap.elapsed_secs();
    let total_s   = snap.total_secs();

    egui::TopBottomPanel::top("info")
        .frame(egui::Frame::default().fill(egui::Color32::from_black_alpha(160)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(snap.title)
                        .color(egui::Color32::WHITE)
                        .strong(),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "{} {:02.0}:{:05.2} / {:02.0}:{:05.2}  {}Hz",
                        if snap.playing { "▶" } else { "⏸" },
                        elapsed_s / 60.0,
                        elapsed_s % 60.0,
                        total_s / 60.0,
                        total_s % 60.0,
                        snap.sample_rate,
                    ))
                    .color(egui::Color32::LIGHT_GRAY)
                    .monospace(),
                );
                if let (Some(grid), Some(bpm)) = (snap.beat_grid, snap.bpm()) {
                    ui.separator();
                    let color = if grid.confidence >= 0.7 {
                        egui::Color32::from_rgb(80, 220, 80)
                    } else {
                        egui::Color32::from_rgb(220, 180, 60)
                    };
                    ui.label(
                        egui::RichText::new(format!("{:.1} BPM", bpm))
                            .color(color)
                            .monospace(),
                    );
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("B2: {:.1} BPM", snap.beat2_bpm))
                        .color(egui::Color32::from_rgb(0, 220, 220))
                        .monospace(),
                );
                ui.separator();
                let speed_color = if (snap.speed - 1.0).abs() < 0.01 {
                    egui::Color32::DARK_GRAY
                } else {
                    egui::Color32::from_rgb(240, 160, 60)
                };
                ui.label(
                    egui::RichText::new(format!("{:.2}×", snap.speed))
                        .color(speed_color)
                        .monospace(),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new("Space=play/pause  ←/→=seek  +/-=speed  Q=quit")
                        .color(egui::Color32::DARK_GRAY)
                        .small(),
                );
            });
        });
}
