//! PipeWire-based device enumerator for Linux
//! Uses PipeWire SPA to query device metadata for user-friendly names

#[cfg(all(target_os = "linux", feature = "pipewire"))]
use crate::audio_backend::AudioDeviceInfo;
#[cfg(all(target_os = "linux", feature = "pipewire")]
use anyhow::{Result, Context};
#[cfg(all(target_os = "linux", feature = "pipewire")]
use spa::pod::{Pod, PodDeserializer};
#[cfg(all(target_os = "linux", feature = "pipewire")]
use pipewire as pw;
#[cfg(all(target_os = "linux", feature = "pipewire")]
use spa::dict::Dict;
#[cfg(all(target_os = "linux", feature = "pipewire")]
use std::collections::HashMap;
#[cfg(all(target_os = "linux", feature = "pipewire")]
use std::sync::{Arc, Mutex};

#[cfg(all(target_os = "linux", feature = "pipewire"))]
pub struct PipeWireDeviceEnumerator {
    // Cache of device info by device name
    device_cache: Mutex<HashMap<String, AudioDeviceInfo>>,
    // PipeWire context
    #[allow(dead_code)]
    context: Arc<pw::Context>,
    main_loop: Arc<pw::MainLoop>,
    core: Arc<pw::Core>,
}

#[cfg(all(target_os = "linux", feature = "pipewire"))]
impl PipeWireDeviceEnumerator {
    pub fn new() -> Result<Self> {
        // Initialize PipeWire
        let main_loop = pw::MainLoop::new().context("Failed to create PipeWire main loop")?;
        let context = pw::Context::new(&main_loop).context("Failed to create PipeWire context")?;
        let core = context.connect(None).context("Failed to connect to PipeWire")?;

        Ok(Self {
            device_cache: Mutex::new(std::collections::HashMap::new()),
            context: Arc::new(context),
            main_loop: Arc::new(main_loop),
            core: Arc::new(core),
        }
    }

    pub fn enumerate_input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let mut devices = Vec::new();
        
        // Get the registry
        let registry = self.core.get_registry().context("Failed to get registry")?;
        
        // For now, fall back to cpal device list but with PipeWire names
        // In a full implementation, we would use PipeWire's device metadata
        
        // For now, return empty - will fall back to udev
        Ok(devices)
    }

    pub fn enumerate_output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let mut devices = Vec::new();
        
        // For now, return empty - will fall back to udev
        Ok(devices)
    }
}

impl Default for PipeWireDeviceEnumerator {
    fn default() -> Self {
        Self::new().expect("Failed to create PipeWire device enumerator")
    }
}