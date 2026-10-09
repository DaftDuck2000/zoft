//! Device enumeration module
//! Provides cross-platform audio device enumeration with user-friendly names

use crate::audio_backend::{AudioDeviceInfo, AudioBackend};
use anyhow::{Result, Context};
use std::sync::Arc;
use std::sync::Mutex;

#[cfg(target_os = "linux")]
mod udev_enumerator;
#[cfg(target_os = "linux")]
pub use udev_enumerator::UdevDeviceEnumerator;

#[cfg(all(target_os = "linux", feature = "pipewire"))]
mod pipewire_enumerator;
#[cfg(all(target_os = "linux", feature = "pipewire"))]
pub use pipewire_enumerator::PipeWireDeviceEnumerator;

/// Trait for cross-platform device enumeration
pub trait DeviceEnumerator: Send + Sync {
    fn enumerate_input_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
    fn enumerate_output_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
}

/// Default device enumerator using cpal
pub struct CpalDeviceEnumerator {
    backend: Arc<dyn AudioBackend>,
}

impl CpalDeviceEnumerator {
    pub fn new(backend: Arc<dyn AudioBackend>) -> Self {
        Self { backend }
    }
}

impl DeviceEnumerator for CpalDeviceEnumerator {
    fn enumerate_input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        self.backend.input_devices()
    }

    fn enumerate_output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        self.backend.output_devices()
    }
}

/// Composite enumerator that tries PipeWire first, falls back to libudev
#[cfg(target_os = "linux")]
pub struct LinuxDeviceEnumerator {
    pipewire: Option<PipeWireDeviceEnumerator>,
    udev: UdevDeviceEnumerator,
}

#[cfg(target_os = "linux")]
impl LinuxDeviceEnumerator {
    pub fn new(pipewire: Option<PipeWireDeviceEnumerator>, udev: UdevDeviceEnumerator) -> Self {
        Self { pipewire, udev }
    }
}

#[cfg(target_os = "linux")]
impl DeviceEnumerator for LinuxDeviceEnumerator {
    fn enumerate_input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        // Try PipeWire first
        if let Some(pipewire) = &self.pipewire {
            if let Ok(devices) = pipewire.enumerate_input_devices() {
                if !devices.is_empty() {
                    return Ok(devices);
                }
            }
        }
        // Fallback to libudev
        self.udev.enumerate_input_devices()
    }

    fn enumerate_output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        // Try PipeWire first
        if let Some(pipewire) = &self.pipewire {
            if let Ok(devices) = pipewire.enumerate_output_devices() {
                if !devices.is_empty() {
                    return Ok(devices);
                }
            }
        }
        // Fallback to libudev
        self.udev.enumerate_output_devices()
    }
}

/// Factory function to create the appropriate device enumerator
pub fn create_device_enumerator(backend: Arc<dyn crate::audio_backend::AudioBackend>) -> Box<dyn DeviceEnumerator> {
    #[cfg(target_os = "linux")]
    {
        #[cfg(feature = "pipewire")]
        let pipewire = PipeWireDeviceEnumerator::new().ok();
        #[cfg(not(feature = "pipewire"))]
        let pipewire = None;
        
        let udev = UdevDeviceEnumerator::new();
        Box::new(LinuxDeviceEnumerator::new(pipewire, udev))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(CpalDeviceEnumerator::new(backend))
    }
}