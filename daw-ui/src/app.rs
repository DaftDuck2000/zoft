//! Main application

use daw_core::Project;
use daw_engine::AudioEngine;
use anyhow::Result;
use eframe::egui;

pub struct App {
    project: Project,
    engine: AudioEngine,
}

impl App {
    pub fn new(project: Project, engine: AudioEngine) -> Result<Self> {
        Ok(Self { project, engine })
    }

    pub fn run(self) -> Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 800.0])
                .with_title("Zoft DAW"),
            ..Default::default()
        };

        let result = eframe::run_native(
            "Zoft DAW",
            options,
            Box::new(|cc| {
                cc.egui_ctx.set_visuals(egui::Visuals::dark());
                Ok(Box::new(AppUI::new(self.project, self.engine)))
            }),
        );
        
        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("eframe error: {:?}", e)),
        }
    }
}

struct AppUI {
    project: Project,
    engine: AudioEngine,
    playing: bool,
}

impl AppUI {
    fn new(project: Project, engine: AudioEngine) -> Self {
        Self {
            project,
            engine,
            playing: false,
        }
    }
}

impl eframe::App for AppUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top transport bar
        egui::TopBottomPanel::top("transport_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(if self.playing { "⏸" } else { "▶" }).clicked() {
                    self.playing = !self.playing;
                    if self.playing {
                        let _ = self.engine.start();
                    } else {
                        let _ = self.engine.stop();
                    }
                }
                ui.separator();
                ui.label(format!("Project: {}", self.project.name));
                ui.separator();
                ui.label(format!("Sample Rate: {} Hz", self.project.sample_rate));
                ui.separator();
                ui.label(format!("Tracks: {}", self.project.track_order.len()));
            });
        });

        // Left track list
        egui::SidePanel::left("track_list").show(ctx, |ui| {
            ui.heading("Tracks");
            ui.separator();
            for track_id in &self.project.track_order {
                if let Some(track) = self.project.tracks.get(track_id) {
                    ui.horizontal(|ui| {
                        let color = egui::Color32::from_rgb(track.color.0, track.color.1, track.color.2);
                        ui.colored_label(color, "■");
                        ui.label(&track.name);
                    });
                }
            }
        });

        // Central timeline/arrange area
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Arrange");
            ui.separator();
            ui.label("Timeline view - coming soon");
            
            // Show a simple timeline ruler
            let rect = ui.available_rect_before_wrap();
            let painter = ui.painter();
            
            // Draw ruler background
            painter.rect_filled(rect, 0.0, egui::Color32::from_gray(30));
            
            // Draw beat markers
            for i in 0..=16 {
                let x = rect.left() + (rect.width() / 16.0) * i as f32;
                painter.line_segment([
                    egui::pos2(x, rect.top()),
                    egui::pos2(x, rect.bottom()),
                ], egui::Stroke::new(1.0, egui::Color32::from_gray(60)));
            }
        });

        // Bottom status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(if self.playing { "Playing" } else { "Stopped" });
                ui.separator();
                ui.label(format!("CPU: {:.1}%", 0.0));
                ui.separator();
                ui.label("Zoft DAW v0.1.0");
            });
        });

        // Request repaint for smooth playback
        ctx.request_repaint();
    }
}