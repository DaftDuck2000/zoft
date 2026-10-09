//! Udev-based device enumerator for Linux
//! Uses libudev to query device properties for user-friendly names

use crate::audio_backend::AudioDeviceInfo;
use anyhow::{Result, Context};
use libudev::{Enumerator, Device};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Mutex;

/// Udev-based device enumerator for Linux
pub struct UdevDeviceEnumerator {
    // Cache of device info by syspath
    device_cache: Mutex<HashMap<String, AudioDeviceInfo>>,
}

impl UdevDeviceEnumerator {
    pub fn new() -> Self {
        Self {
            device_cache: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn query_device_info(&self, device: &Device) -> Option<AudioDeviceInfo> {
        let syspath = device.syspath().to_string_lossy().to_string();
        
        // Check cache first
        if let Some(cached) = self.device_cache.lock().unwrap().get(&syspath) {
            return Some(cached.clone());
        }

        // Try multiple udev properties to get the best device name
        let raw_name = device.property_value("ID_MODEL_FROM_DATABASE")
            .or_else(|| device.property_value("ID_MODEL"))
            .or_else(|| device.property_value("ID_MODEL_ID"))
            .or_else(|| device.property_value("ID_VENDOR_FROM_DATABASE"))
            .or_else(|| device.property_value("ID_VENDOR"))
            .or_else(|| device.property_value("ID_VENDOR_ID"))
            .or_else(|| device.property_value("ID_MODEL_ID"))
            .or_else(|| device.property_value("ID_PATH"))
            .or_else(|| device.property_value("ID_PATH_TAG"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Unknown Device".to_string());

        // Also get vendor info from database
        let vendor_name = device.property_value("ID_VENDOR_FROM_DATABASE")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let raw_name = if let Some(vendor) = vendor_name {
            format!("{} {}", vendor, raw_name)
        } else {
            raw_name
        };

        let cleaned_name = clean_device_name(&raw_name);
        
        // Determine if it's an input or output device
        let subsystem = device.subsystem().to_string_lossy().to_string();
        let is_input = subsystem == "sound" && device.property_value("ID_USB_INTERFACE_NUM").is_some();
        let is_output = subsystem == "sound" && device.property_value("ID_USB_INTERFACE_NUM").is_none();
        
        // Get channel info from device properties
        let max_input_channels = device.property_value("ID_USB_INTERFACE_NUM")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(if device.property_value("ID_USB_INTERFACE_NUM").is_some() { 2 } else { 0 });
            
        let max_output_channels = if device.property_value("ID_USB_INTERFACE_NUM").is_none() { 2 } else { 0 };

        let device_info = AudioDeviceInfo {
            id: format!("udev_{}", device.syspath().to_string_lossy().replace('/', "_")),
            name: cleaned_name.clone(),
            raw_name: raw_name.clone(),
            is_default_input: false,
            is_default_output: false,
            max_input_channels: if is_input { 2 } else { 0 },
            max_output_channels: if is_output { 2 } else { 0 },
            sample_rates: vec![44100, 48000, 96000, 192000],
        };

        // Cache the result
        self.device_cache.lock().unwrap().insert(syspath, device_info.clone());
        
        Some(device_info)
    }

    pub fn enumerate_input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let mut enumerator = libudev::Enumerator::new()?;
        enumerator.match_subsystem("sound")?;
        enumerator.match_property("ID_TYPE", "audio")?;
        
        let mut devices = Vec::new();
        
        for device in enumerator.scan_devices()? {
            if let Some(info) = self.query_device_info(&device) {
                // Only include devices that can be used for input
                if info.max_input_channels > 0 {
                    devices.push(info);
                }
            }
        }
        
        Ok(devices)
    }

    pub fn enumerate_output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let mut enumerator = libudev::Enumerator::new()?;
        enumerator.match_subsystem("sound")?;
        enumerator.match_property("ID_TYPE", "audio")?;
        
        let mut devices = Vec::new();
        
        for device in enumerator.scan_devices()? {
            if let Some(info) = self.query_device_info(&device) {
                // Only include devices that can be used for output
                if info.max_output_channels > 0 {
                    devices.push(info);
                }
            }
        }
        
        // Also check for output-specific devices
        let mut enumerator2 = libudev::Enumerator::new()?;
        enumerator2.match_subsystem("sound")?;
        enumerator2.match_property("ID_TYPE", "audio")?;
        enumerator2.match_property("ID_USB_INTERFACE_NUM", "")?;
        
        for device in enumerator2.scan_devices()? {
            if let Some(info) = self.query_device_info(&device) {
                if info.max_output_channels > 0 {
                    // Check if we already have this device
                    if !devices.iter().any(|d| d.raw_name == info.raw_name) {
                        devices.push(info);
                    }
                }
            }
        }
        
        Ok(devices)
    }
}

/// Clean up device name to be more user-friendly
fn clean_device_name(name: &str) -> String {
    let name = name.trim();
    
    // Remove ALSA-style identifiers like CARD=Generic_1,DEV=0
    let name = regex::Regex::new(r"(CARD|DEV|SUBDEV)=\w+")
        .unwrap()
        .replace_all(&name, "")
        .to_string();
    
    // Remove ALSA-style prefixes like surround51:, front:, etc.
    let name = regex::Regex::new(r"^\w+:")
        .unwrap()
        .replace_all(&name, "")
        .to_string();
    
    // Remove colons, equals, commas, semicolons
    let name = name.replace([':', '=', ',', ';', '#', '@'], " ");
    
    // Clean up multiple spaces
    let name = regex::Regex::new(r"\s+")
        .unwrap()
        .replace_all(&name, " ")
        .to_string();
    
    let name = name.trim().to_string();
    
    // If name is empty or too short, return a default
    if name.is_empty() || name.len() < 2 {
        return "Audio Device".to_string();
    }
    
    // Capitalize first letter of each word
    name.split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}