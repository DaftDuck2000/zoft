//! Main application

use daw_core::Project;
use daw_core::track::TrackId;
use daw_engine::AudioEngine;
use anyhow::Result;
use eframe::egui;
use std::path::PathBuf;

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
    recording: bool,
    playhead_position: f32,  // 0.0 to 1.0
    last_playhead_update: std::time::Instant,
    dragging_playhead: bool,
    input_devices: Vec<daw_engine::audio_backend::AudioDeviceInfo>,
    selected_input_device: Option<String>,
    waveform_cache: daw_engine::waveform::WaveformCache,
}

impl AppUI {
    fn new(project: Project, engine: AudioEngine) -> Self {
        let input_devices = engine.input_devices().unwrap_or_default();
        Self {
            project,
            engine,
            playing: false,
            recording: false,
            playhead_position: 0.0,
            last_playhead_update: std::time::Instant::now(),
            dragging_playhead: false,
            input_devices,
            selected_input_device: None,
            waveform_cache: daw_engine::waveform::WaveformCache::new(),
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
        }
        // When paused, playhead stays at current position

        // Top transport bar
        egui::TopBottomPanel::top("transport_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(if self.playing { "⏸" } else { "▶" }).clicked() {
                    self.toggle_playback();
                }
                ui.separator();
                
                // Record button
                let record_text = if self.recording { "⏹" } else { "●" };
                let record_color = if self.recording { egui::Color32::RED } else { egui::Color32::WHITE };
                if ui.add(egui::Button::new(egui::RichText::new(record_text).color(record_color)).min_size([32.0, 24.0].into())).clicked() {
                    self.toggle_recording();
                }
                
                ui.separator();
                ui.label(format!("Project: {}", self.project.name));
ui.separator();
            ui.label(format!("Sample Rate: {} Hz", self.project.sample_rate));
            ui.separator();
            
            // Input device selector
            if !self.input_devices.is_empty() {
                ui.separator();
                ui.label("Input:");
                let mut selected_changed = false;
                let mut new_selection = self.selected_input_device.clone();
                egui::ComboBox::from_label("")
                    .selected_text(self.selected_input_device.as_deref().unwrap_or("Default"))
                    .show_ui(ui, |ui| {
                        for device in &self.input_devices {
                            let is_default = if device.is_default_input { " (default)" } else { "" };
                            if ui.selectable_value(
                                &mut new_selection,
                                Some(device.id.clone()),
                                format!("{}{}", device.name, is_default)
                            ).changed() {
                                selected_changed = true;
                            }
                        }
                    });
                if selected_changed {
                    let _ = self.engine.set_selected_input_device(new_selection.clone());
                    self.selected_input_device = new_selection;
                }
            }
            ui.separator();
            ui.label(format!("Tracks: {}", self.project.track_order.len()));
            });
        });

        // Left track list
        egui::SidePanel::left("track_list").show(ctx, |ui| {
            ui.heading("Tracks");
            ui.separator();
            for track_id in &self.project.track_order {
                if let Some(track) = self.project.tracks.get_mut(track_id) {
                    ui.horizontal(|ui| {
                        let color = egui::Color32::from_rgb(track.color.0, track.color.1, track.color.2);
                        ui.colored_label(color, "■");
                        ui.label(&track.name);
                        ui.separator();
                        
                        // Mute/Solo/Record Arm buttons
                        let mute_text = if track.mute { "M" } else { "M" };
                        let solo_text = if track.solo { "S" } else { "S" };
                        let arm_text = if track.record_arm { "●" } else { "○" };
                        
                        let mute_color = if track.mute { egui::Color32::YELLOW } else { egui::Color32::GRAY };
                        let solo_color = if track.solo { egui::Color32::GREEN } else { egui::Color32::GRAY };
                        let arm_color = if track.record_arm { egui::Color32::RED } else { egui::Color32::GRAY };
                        
                        if ui.add(egui::Button::new(egui::RichText::new(mute_text).color(mute_color)).min_size([20.0, 20.0].into())).clicked() {
                            track.mute = !track.mute;
                        }
                        if ui.add(egui::Button::new(egui::RichText::new(solo_text).color(solo_color)).min_size([20.0, 20.0].into())).clicked() {
                            track.solo = !track.solo;
                        }
                        if ui.add(egui::Button::new(egui::RichText::new(arm_text).color(arm_color)).min_size([20.0, 20.0].into())).clicked() {
                            track.record_arm = !track.record_arm;
                        }
                        
                        ui.label(format!("Vol: {:.0}%", track.volume * 100.0));
                    });
                }
            }
        });

        // Central timeline/arrange area
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Arrange");
            ui.separator();
            
            let rect = ui.available_rect_before_wrap();
            let painter = ui.painter();
            
            // Track height
            let track_height = 80.0;
            let ruler_height = 30.0;
            
            // Draw ruler background
            let ruler_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top() + ruler_height),
            );
            painter.rect_filled(ruler_rect, 0.0, egui::Color32::from_gray(25));
            
            // Draw beat markers on ruler
            for i in 0..=16 {
                let x = rect.left() + (rect.width() / 16.0) * i as f32;
                painter.line_segment([
                    egui::pos2(x, rect.top()),
                    egui::pos2(x, rect.top() + ruler_height),
                ], egui::Stroke::new(1.0, egui::Color32::from_gray(80)));
                
                // Beat numbers
                if i > 0 {
                    painter.text(
                        egui::pos2(x - 10.0, rect.top() + 4.0),
                        egui::Align2::CENTER_CENTER,
                        format!("{}", i),
                        egui::FontId::proportional(10.0),
                        egui::Color32::from_gray(180),
                    );
                }
            }
            
            // Draw track lanes
            let track_y_start = rect.top() + ruler_height;
            for (track_idx, track_id) in self.project.track_order.iter().enumerate() {
                if let Some(track) = self.project.tracks.get(track_id) {
                    let track_rect = egui::Rect::from_min_max(
                        egui::pos2(rect.left(), track_y_start + track_idx as f32 * track_height),
                        egui::pos2(rect.right(), track_y_start + (track_idx + 1) as f32 * track_height),
                    );
                    
                    // Track background
                    let bg_color = if track_idx % 2 == 0 { 
                        egui::Color32::from_gray(35) 
                    } else { 
                        egui::Color32::from_gray(30) 
                    };
                    painter.rect_filled(track_rect, 0.0, bg_color);
                    
                    // Track separator
                    painter.line_segment([
                        egui::pos2(rect.left(), track_rect.top()),
                        egui::pos2(rect.right(), track_rect.top()),
                    ], egui::Stroke::new(1.0, egui::Color32::from_gray(50)));
                    
                    // Track name label
                    painter.text(
                        egui::pos2(track_rect.left() + 8.0, track_rect.top() + 4.0),
                        egui::Align2::LEFT_TOP,
                        &track.name,
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_gray(200),
                    );
                    
                    // Render audio clips on this track
                    for clip_id in &track.clips {
                        // Find the clip in the project's clip storage
                        // For now, we need to access clips through a different mechanism
                        // Since clips are stored on tracks, we iterate through track.clips
                        // and the clip data is the AudioClip itself
                        // Note: In the current architecture, clips are stored on tracks
                        // and we need to get the actual clip data
                        // For now, we'll skip waveform rendering until clip storage is properly accessible
                        // This is a placeholder for the waveform rendering logic
                    }
                }
            }
            
            // Draw playhead
            let playhead_x = rect.left() + rect.width() * self.playhead_position.clamp(0.0, 1.0);
            painter.line_segment([
                egui::pos2(playhead_x, rect.top()),
                egui::pos2(playhead_x, rect.bottom()),
            ], egui::Stroke::new(2.0, egui::Color32::RED));
            
            // Playhead triangle
            let triangle_size = 8.0;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(playhead_x - triangle_size, rect.top()),
                    egui::pos2(playhead_x + triangle_size, rect.top()),
                    egui::pos2(playhead_x, rect.top() + triangle_size),
                ],
                egui::Color32::RED,
                egui::Stroke::NONE,
            ));

            // Playhead drag interaction
            let playhead_rect = egui::Rect::from_min_max(
                egui::pos2(playhead_x - 6.0, rect.top()),
                egui::pos2(playhead_x + 6.0, rect.bottom()),
            );
            let response = ui.interact(playhead_rect, ui.id().with("playhead"), egui::Sense::drag());
            if response.dragged() {
                if !self.dragging_playhead {
                    self.dragging_playhead = true;
                    if self.playing {
                        self.playing = false;
                        let _ = self.engine.stop();
                    }
                }
                let new_x = (response.drag_delta().x + playhead_x).clamp(rect.left(), rect.right());
                self.playhead_position = ((new_x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            } else if response.drag_stopped() {
                self.dragging_playhead = false;
            }
            
            // Visual feedback for draggable playhead
            let playhead_interact_rect = egui::Rect::from_min_max(
                egui::pos2(playhead_x - 6.0, rect.top()),
                egui::pos2(playhead_x + 6.0, rect.bottom()),
            );
            if ui.rect_contains_pointer(playhead_interact_rect) || self.dragging_playhead {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
            }
        });

        // Bottom status bar
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let status = if self.recording {
                    "● RECORDING"
                } else if self.playing {
                    "Playing"
                } else {
                    "Stopped"
                };
                ui.label(status);
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
            // Space: Play/Pause (toggle) - resumes from current position
            if ctx.input(|i| i.key_pressed(Key::Space)) {
                self.toggle_playback();
            }
            
            // Enter: Return to start (reset playhead to 0, stop)
            if ctx.input(|i| i.key_pressed(Key::Enter)) {
                self.playhead_position = 0.0;
                self.playing = false;
                let _ = self.engine.stop();
            }
            
            // R: Record (toggle recording)
            if ctx.input(|i| i.key_pressed(Key::R)) {
                self.toggle_recording();
            }
        }
    }
    
    fn toggle_playback(&mut self) {
        self.playing = !self.playing;
        if self.playing {
            let _ = self.engine.start();
            self.last_playhead_update = std::time::Instant::now();
        } else {
            let _ = self.engine.stop();
            // Playhead stays at current position when paused
        }
    }
    
    fn toggle_recording(&mut self) {
        self.recording = !self.recording;
        if self.recording {
            // Find first armed track
            for track_id in &self.project.track_order {
                if let Some(track) = self.project.tracks.get(track_id) {
                    if track.record_arm {
                        // Start recording
                        let file_path = std::path::PathBuf::from(format!("recording_{}.wav", track_id.0));
                        if let Err(e) = self.engine.start_recording(*track_id, file_path) {
                            eprintln!("Failed to start recording: {}", e);
                            self.recording = false;
                        }
                        break;
                    }
                }
            }
        } else {
            let _ = self.engine.stop_recording();
        }
    }
}