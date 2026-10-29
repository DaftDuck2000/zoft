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
pub mod waveform;
pub mod playback;
pub mod monitor;
pub mod disk;
pub mod midi;

pub use audio_backend::*;
pub use process_graph::*;
pub use transport::*;
pub use scheduler::*;
pub use parameters::*;
pub use metering::*;
pub use dsp::*;
pub use plugin_host::*;
pub use bwf_writer::*;
pub use waveform::*;
pub use playback::*;
pub use monitor::*;
pub use disk::*;
pub use midi::*;

use crate::audio_backend::AudioBackend;
use crate::playback::{
    MetronomeConfig, PlayableClip, PlayableNote, PlayableTrack, PlaybackMixer,
    StereoMeter, TrackMeter,
};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

pub struct AudioEngine {
    backend: Box<dyn AudioBackend>,
    running: Arc<Mutex<bool>>,
    recording: Arc<Mutex<Option<RecordingState>>>,
    project_sample_rate: u32,
    project_bit_depth: u16,
    project_channels: u16,
    selected_input_device: Arc<Mutex<Option<String>>>,
    /// Whether live input monitoring is requested (kept in sync with the UI).
    monitoring_enabled: AtomicBool,
    /// Whether a playback mixer is currently loaded. Tracked explicitly so
    /// stream decisions never depend on a `try_lock` that can race the audio
    /// callback.
    playback_active: AtomicBool,
    /// Loop range in output-device frames, applied whenever a new mixer is
    /// created and to the live mixer when it changes.
    playback_loop: Arc<Mutex<Option<(u64, u64)>>>,
    /// Metronome settings, applied to every new mixer and live updates.
    playback_metronome: Arc<Mutex<Option<MetronomeConfig>>>,
    /// MIDI notes for the built-in sine voice, applied to every new mixer and
    /// live updates.
    playback_notes: Arc<Mutex<Vec<PlayableNote>>>,
    /// Per-track fader/effect settings, applied to every new mixer and live
    /// updates.
    playback_tracks: Arc<Mutex<Vec<PlayableTrack>>>,
    /// Latest peak/RMS reading per track, shared with the playback mixer.
    track_meters: Arc<Mutex<Vec<TrackMeter>>>,
    /// Latest stereo peak/RMS reading for the master output.
    master_meter: Arc<Mutex<StereoMeter>>,
}

struct RecordingState {
    sender: mpsc::Sender<AudioBuffer>,
    track_id: daw_core::track::TrackId,
    file_path: PathBuf,
    writer_thread: std::thread::JoinHandle<()>,
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
            selected_input_device: Arc::new(Mutex::new(None)),
            monitoring_enabled: AtomicBool::new(false),
            playback_active: AtomicBool::new(false),
            playback_loop: Arc::new(Mutex::new(None)),
            playback_metronome: Arc::new(Mutex::new(None)),
            playback_notes: Arc::new(Mutex::new(Vec::new())),
            playback_tracks: Arc::new(Mutex::new(Vec::new())),
            track_meters: Arc::new(Mutex::new(Vec::new())),
            master_meter: Arc::new(Mutex::new(StereoMeter::default())),
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
        if *running && !self.monitoring_enabled.load(Ordering::SeqCst) {
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

    pub fn output_sample_rate(&self) -> u32 {
        self.backend.sample_rate() as u32
    }

    pub fn output_channels(&self) -> u16 {
        self.backend.channels()
    }

    /// Load playback content and reset the mixer position. `start_frame` is the
    /// timeline position in output-device frames where playback begins. Call
    /// `start()` afterwards to open the output stream.
    pub fn set_playback_clips(&self, clips: Vec<PlayableClip>, start_frame: u64) {
        let mut mixer = PlaybackMixer::new(
            self.backend.sample_rate() as u32,
            self.backend.channels(),
            clips,
            start_frame,
        );
        mixer.set_meters(Arc::clone(&self.track_meters));
        mixer.set_master_meter(Arc::clone(&self.master_meter));
        if let Some(range) = *self.playback_loop.lock().unwrap() {
            mixer.set_loop_range(Some(range));
        }
        if let Some(metronome) = *self.playback_metronome.lock().unwrap() {
            mixer.set_metronome(Some(metronome));
        }
        {
            let tracks = self.playback_tracks.lock().unwrap();
            if !tracks.is_empty() {
                mixer.set_tracks(tracks.clone());
            }
        }
        {
            let notes = self.playback_notes.lock().unwrap();
            if !notes.is_empty() {
                mixer.set_midi_notes(notes.clone());
            }
        }
        self.backend.set_playback_mixer(Some(mixer));
        self.playback_active.store(true, Ordering::SeqCst);
    }

    /// Set (or clear) the transport loop range in output-device frames. The
    /// range is applied to the live mixer immediately and to any mixer created
    /// later by [`set_playback_clips`](Self::set_playback_clips).
    pub fn set_playback_loop(&self, range: Option<(u64, u64)>) {
        *self.playback_loop.lock().unwrap() = range;
        self.backend.set_playback_loop(range);
    }

    /// Set (or clear) the metronome click. Applied to the live mixer
    /// immediately and to any mixer created later.
    pub fn set_playback_metronome(&self, config: Option<MetronomeConfig>) {
        *self.playback_metronome.lock().unwrap() = config;
        self.backend.set_playback_metronome(config);
    }

    /// Set the MIDI notes rendered by the built-in sine voice. Applied to the
    /// live mixer immediately and to any mixer created later.
    pub fn set_playback_notes(&self, notes: Vec<PlayableNote>) {
        *self.playback_notes.lock().unwrap() = notes.clone();
        self.backend.set_playback_notes(notes);
    }

    /// Set per-track fader/effect settings. Applied to the live mixer
    /// immediately and to any mixer created later.
    pub fn set_playback_tracks(&self, tracks: Vec<PlayableTrack>) {
        *self.playback_tracks.lock().unwrap() = tracks.clone();
        self.backend.set_playback_tracks(tracks);
    }

    /// Latest peak/RMS reading for every track, indexed by track order.
    pub fn track_meters(&self) -> Vec<TrackMeter> {
        self.track_meters.lock().unwrap().clone()
    }

    /// Latest stereo peak/RMS reading for the summed master output.
    pub fn master_meter(&self) -> StereoMeter {
        *self.master_meter.lock().unwrap()
    }

    /// Audition a MIDI note through the live sine synth, independent of
    /// transport playback.
    pub fn note_on(&self, note: u8, velocity: u8) {
        self.backend.note_on(note, velocity);
    }

    pub fn note_off(&self, note: u8) {
        self.backend.note_off(note);
    }

    pub fn all_notes_off(&self) {
        self.backend.all_notes_off();
    }

    /// Remove the playback content (output falls silent).
    pub fn clear_playback(&self) {
        self.backend.set_playback_mixer(None);
        self.playback_active.store(false, Ordering::SeqCst);
    }

    /// Current playback position in output frames, if playback is loaded.
    pub fn playback_position_frames(&self) -> Option<u64> {
        self.backend.playback_position()
    }

    pub fn set_selected_input_device(&self, device_id: Option<String>) -> Result<()> {
        *self.selected_input_device.lock().unwrap() = device_id;
        Ok(())
    }

    pub fn start_recording(&self, track_id: daw_core::track::TrackId, file_path: std::path::PathBuf) -> Result<()> {
        let mut recording_guard = self.recording.lock().unwrap();
        if recording_guard.is_some() {
            return Err(anyhow::anyhow!("Already recording"));
        }

        let bit_depth = self.project_bit_depth;

        // Resolve the input device to record from
        let device = self.resolve_input_device()?;

        let (tx, rx) = mpsc::channel::<AudioBuffer>();
        let sender = Arc::new(std::sync::Mutex::new(Some(tx.clone())));

        // If the monitoring stream is already open, close it so the recording
        // stream owns the device and its format.
        if self.backend.input_stream_running() {
            let _ = self.backend.stop_input_stream();
        }

        // Start the input stream first so we know the actual format it opened,
        // and so a failure leaves the engine in a clean, non-recording state.
        let (actual_rate, actual_channels) = match self.backend.start_input_stream(
            &device,
            self.project_sample_rate,
            Arc::clone(&sender),
        ) {
            Ok(format) => format,
            Err(e) => {
                drop(sender);
                drop(tx);
                return Err(e.context("Failed to start recording from input device"));
            }
        };

        // Monitoring may have been active before the take; keep it audible.
        self.backend
            .set_monitoring(self.monitoring_enabled.load(Ordering::SeqCst));

        let metadata = daw_core::audio_pool::AudioMetadata {
            originator: "Zoft DAW".to_string(),
            comment: format!("Track {}", track_id.0),
            ..Default::default()
        };

        // Create the writer using the stream's actual format so the file matches
        // the captured data exactly.
        let mut writer = match crate::bwf_writer::BwfWriter::create(
            &file_path,
            actual_rate,
            actual_channels,
            bit_depth,
            metadata,
        ) {
            Ok(writer) => writer,
            Err(e) => {
                let _ = self.backend.stop_input_stream();
                drop(sender);
                drop(tx);
                let _ = std::fs::remove_file(&file_path);
                return Err(e.context("Failed to create recording file"));
            }
        };

        // Drain captured buffers into the file on a background thread. Any audio
        // captured before this thread starts is buffered by the channel.
        let writer_thread = std::thread::spawn(move || {
            while let Ok(buffer) = rx.recv() {
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
            writer_thread,
        };

        *recording_guard = Some(state);

        Ok(())
    }

    pub fn stop_recording(&self) -> Result<Option<(daw_core::track::TrackId, std::path::PathBuf)>> {
        let mut recording_guard = self.recording.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = recording_guard.take() {
            // Stop input stream first so the backend releases its sender, then drop
            // our own sender to signal the writer thread to finish.
            let _ = self.backend.stop_input_stream();
            drop(state.sender);
            // Wait for the background writer to flush and finalize the WAV header.
            // Without this, callers that open the file immediately after stopping
            // would see a placeholder (zero) data size and read a 0-length file.
            let _ = state.writer_thread.join();
            // Reopen the input for monitoring when the UI still wants it.
            if self.monitoring_enabled.load(Ordering::SeqCst) {
                let _ = self.ensure_monitoring_stream();
                self.backend.set_monitoring(true);
            }
            Ok(Some((state.track_id, state.file_path)))
        } else {
            Ok(None)
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.lock().unwrap().is_some()
    }

    pub fn set_project_format(&mut self, sample_rate: u32, bit_depth: u16, channels: u16) {
        self.project_sample_rate = sample_rate;
        self.project_bit_depth = bit_depth;
        self.project_channels = channels;
    }

    /// Estimated seconds of recording available on the filesystem containing
    /// `path`, given the current project format.
    pub fn available_recording_seconds(&self, path: &std::path::Path) -> Option<f64> {
        crate::disk::recording_seconds_available(
            path,
            self.project_sample_rate,
            self.project_channels,
            self.project_bit_depth,
        )
    }

    pub fn input_devices(&self) -> Result<Vec<crate::audio_backend::AudioDeviceInfo>> {
        self.backend.input_devices()
    }

    /// Peak and RMS of the most recent input block, if the input stream is
    /// running (used by the recording level meter).
    pub fn input_level(&self) -> Option<(f32, f32)> {
        self.backend.input_level()
    }

    /// Enable or disable live input monitoring. When enabled and the transport
    /// is idle, an input stream is opened so armed/On tracks can be heard
    /// before a take starts.
    pub fn set_monitoring(&self, enabled: bool) {
        self.monitoring_enabled.store(enabled, Ordering::SeqCst);
        let recording = self.recording.lock().unwrap().is_some();

        if enabled {
            if !self.backend.input_stream_running() {
                let _ = self.ensure_monitoring_stream();
            }
            // Monitoring is heard through the output stream, so open it even
            // while the transport is stopped.
            if !self.is_running() {
                let _ = self.start();
            }
            self.backend.set_monitoring(true);
        } else {
            self.backend.set_monitoring(false);
            if !recording {
                let _ = self.backend.stop_input_stream();
            }
            let playback_loaded = self.playback_active.load(Ordering::SeqCst);
            if !recording && !playback_loaded {
                let _ = self.stop();
            }
        }
    }

    fn resolve_input_device(&self) -> Result<crate::audio_backend::AudioDeviceInfo> {
        let selected_device = self.selected_input_device.lock().unwrap().clone();
        let devices = self.backend.input_devices()?;

        selected_device
            .and_then(|selected_id| devices.iter().find(|d| d.id == selected_id).cloned())
            .or_else(|| devices.iter().find(|d| d.is_default_input).cloned())
            .or_else(|| devices.first().cloned())
            .ok_or_else(|| anyhow::anyhow!("No audio input device available"))
    }

    /// Open the input stream for monitoring while the transport is idle.
    fn ensure_monitoring_stream(&self) -> Result<()> {
        if self.backend.input_stream_running() {
            return Ok(());
        }
        let device = self.resolve_input_device()?;
        let (tx, _rx) = mpsc::channel::<AudioBuffer>();
        self.backend.start_input_stream(
            &device,
            self.project_sample_rate,
            Arc::new(Mutex::new(Some(tx))),
        )?;
        Ok(())
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        // Don't reopen the input stream for monitoring while tearing down.
        self.monitoring_enabled.store(false, Ordering::SeqCst);
        // Use catch_unwind to prevent panic in destructor from causing abort
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = self.stop();
            let _ = self.stop_recording();
        }));
    }
}