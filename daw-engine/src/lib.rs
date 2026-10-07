//! Zoft Engine - Audio engine, DSP, transport, graph processing

pub mod audio_backend;
pub mod process_graph;
pub mod transport;
pub mod scheduler;
pub mod parameters;
pub mod metering;
pub mod dsp;
pub mod plugin_host;
pub mod bwf_writer;

pub use audio_backend::*;
pub use process_graph::*;
pub use transport::*;
pub use scheduler::*;
pub use parameters::*;
pub use metering::*;
pub use dsp::*;
pub use plugin_host::*;
pub use bwf_writer::*;

use crate::audio_backend::AudioBackend;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub struct AudioEngine {
    backend: Box<dyn AudioBackend>,
    running: Arc<Mutex<bool>>,
    recording: Arc<Mutex<Option<RecordingState>>>,
    project_sample_rate: u32,
    project_bit_depth: u16,
    project_channels: u16,
}

struct RecordingState {
    sender: mpsc::Sender<AudioBuffer>,
    track_id: daw_core::track::TrackId,
    file_path: PathBuf,
}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let backend = crate::audio_backend::create_default_backend()?;
        let sample_rate = backend.sample_rate() as u32;
        let channels = backend.channels();
        
        Ok(Self {
            backend,
            running: Arc::new(Mutex::new(false)),
            recording: Arc::new(Mutex::new(None)),
            project_sample_rate: sample_rate,
            project_bit_depth: 32, // 32-bit float
            project_channels: channels,
        })
    }

    pub fn start(&self) -> Result<()> {
        let mut running = self.running.lock().unwrap();
        if !*running {
            self.backend.start()?;
            *running = true;
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let mut running = self.running.lock().unwrap();
        if *running {
            self.backend.stop()?;
            *running = false;
        }
        // Also stop any recording
        self.stop_recording()?;
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        *self.running.lock().unwrap()
    }

    pub fn start_recording(&self, track_id: daw_core::track::TrackId, file_path: std::path::PathBuf) -> Result<()> {
        let mut recording_guard = self.recording.lock().unwrap();
        if recording_guard.is_some() {
            return Err(anyhow::anyhow!("Already recording"));
        }

        let sample_rate = self.project_sample_rate;
        let channels = self.project_channels;
        let bit_depth = self.project_bit_depth;

        let metadata = daw_core::audio_pool::AudioMetadata {
            originator: "Zoft DAW".to_string(),
            comment: format!("Track {}", track_id.0),
            ..Default::default()
        };

        let writer = crate::bwf_writer::BwfWriter::create(
            &file_path,
            sample_rate,
            channels,
            bit_depth,
            metadata,
        )?;

        let (tx, mut rx) = mpsc::channel::<AudioBuffer>(1024);

        // Spawn background writer task
        let mut writer = writer;
        tokio::spawn(async move {
            while let Some(buffer) = rx.recv().await {
                if let Err(e) = writer.write_samples(&buffer.data) {
                    eprintln!("Recording write error: {}", e);
                    break;
                }
            }
            if let Err(e) = writer.finalize() {
                eprintln!("Recording finalize error: {}", e);
            }
        });

        let state = RecordingState {
            sender: tx,
            track_id,
            file_path,
        };

        *recording_guard = Some(state);
        
        // Start input stream for the first armed track
        // For now, use default input device
        let devices = self.backend.input_devices()?;
        if let Some(device) = devices.first() {
            let config = cpal::StreamConfig {
                channels: self.project_channels,
                sample_rate: cpal::SampleRate(self.project_sample_rate),
                buffer_size: cpal::BufferSize::Default,
            };
            let sender = Arc::new(std::sync::Mutex::new(Some(mpsc::channel::<AudioBuffer>(1024).0)));
            self.backend.start_input_stream(&device.id, &config, sender)?;
        }

        Ok(())
    }

    pub fn stop_recording(&self) -> Result<()> {
        let mut recording_guard = self.recording.lock().unwrap();
        if let Some(state) = recording_guard.take() {
            // Signal end of recording by dropping sender
            drop(state.sender);
            // Stop input stream
            self.backend.stop_input_stream()?;
        }
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.recording.lock().unwrap().is_some()
    }

    pub fn set_project_format(&mut self, sample_rate: u32, bit_depth: u16, channels: u16) {
        self.project_sample_rate = sample_rate;
        self.project_bit_depth = bit_depth;
        self.project_channels = channels;
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.stop();
        let _ = self.stop_recording();
    }
}