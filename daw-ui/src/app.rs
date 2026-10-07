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
    playhead_position: f32,  // 0.0 to 1.0
    last_playhead_update: std::time::Instant,
}

impl AppUI {
    fn new(project: Project, engine: AudioEngine) -> Self {
        Self {
            project,
            engine,
            playing: false,
            playhead_position: 0.0,
            last_playhead_update: std::time::Instant::now(),
        }
    }
}

impl eframe::App for AppUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle keyboard shortcuts
        self.handle_keyboard(ctx);

        // Update playhead position during playback
        if self.playing {
            let elapsed = self.last_playhead_update.elapsed().as_secs_f32();
            self.playhead_position = (self.playhead_position + elapsed * 0.1) % 1.0;
            self.last_playhead_update = std::time::Instant::now();
        } else {
            self.playhead_position = 0.0;
        }

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
            
            // Draw playhead
            if self.playing || self.playhead_position > 0.0 {
                let playhead_x = rect.left() + rect.width() * self.playhead_position;
                painter.line_segment([
                    egui::pos2(playhead_x, rect.top()),
                    egui::pos2(playhead_x, rect.bottom()),
                ], egui::Stroke::new(2.0, egui::Color32::RED));
                
                // Playhead triangle at top
                let triangle_size = 8.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        egui::pos2(playhead_x, rect.top()),
                        egui::pos2(playhead_x - triangle_size, rect.top() + triangle_size),
                        egui::pos2(playhead_x + triangle_size, rect.top() + triangle_size),
                    ],
                    egui::Color32::RED,
                    egui::Stroke::NONE,
                ));
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

impl AppUI {
    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        use egui::Key;
        
        // Only handle keyboard if no text input is focused
        if ctx.memory(|m| m.focused().is_none()) {
            // Space: Play/Stop
            if ctx.input(|i| i.key_pressed(Key::Space)) {
                self.playing = !self.playing;
                if self.playing {
                    let _ = self.engine.start();
                    self.last_playhead_update = std::time::Instant::now();
                } else {
                    let _ = self.engine.stop();
                    self.playhead_position = 0.0;
                }
            }
            
            // Enter: Return to start
            if ctx.input(|i| i.key_pressed(Key::Enter)) {
                self.playhead_position = 0.0;
                self.playing = false;
                let _ = self.engine.stop();
            }
            
            // R: Record (placeholder)
            if ctx.input(|i| i.key_pressed(Key::R)) {
                // TODO: Implement record
            }
        }
    }
}