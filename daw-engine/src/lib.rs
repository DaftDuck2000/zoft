//! Zoft Engine - Audio engine, DSP, transport, graph processing

pub mod audio_backend;
pub mod process_graph;
pub mod transport;
pub mod scheduler;
pub mod parameters;
pub mod metering;
pub mod dsp;
pub mod plugin_host;

pub use audio_backend::*;
pub use process_graph::*;
pub use transport::*;
pub use scheduler::*;
pub use parameters::*;
pub use metering::*;
pub use dsp::*;
pub use plugin_host::*;

use crate::audio_backend::AudioBackend;
use anyhow::Result;
use std::sync::{Arc, Mutex};

pub struct AudioEngine {
    backend: Box<dyn AudioBackend>,
    running: Arc<Mutex<bool>>,
}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let backend = crate::audio_backend::create_default_backend()?;
        Ok(Self {
            backend,
            running: Arc::new(Mutex::new(false)),
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
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        *self.running.lock().unwrap()
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}