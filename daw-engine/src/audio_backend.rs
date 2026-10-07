use crate::dsp::AudioBuffer;
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

/// Wrapper to make cpal::Stream Send + Sync safe
/// cpal::Stream contains a *mut () internally which is not Send/Sync
/// We wrap it in a struct that manually implements Send + Sync
/// Safety: The stream is only accessed from the audio thread, so it's safe to mark as Send
struct SendStream(Option<Stream>);

unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}

impl SendStream {
    fn new(stream: Stream) -> Self {
        SendStream(Some(stream))
    }
    
    fn take(&mut self) -> Option<Stream> {
        self.0.take()
    }
}

/// Filter function to determine if an audio device is a real hardware device
/// vs a virtual/pseudo device like pipewire, pulse, speex, etc.
fn is_real_hardware_device(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    // Filter out known virtual/pseudo devices
    let virtual_keywords = [
        "pipewire", "pulse", "pulseaudio", "speex", "jack", "alsa", "pipe",
        "virtual", "monitor", "null", "dummy", "easyeffects", "pipewire",
        "link", "loopback", "hdmi", "hd audio", "hdmi", "displayport",
        "surround", "iec958", "iec", "spdif", "digital", "hdmi",
    ];
    
    for keyword in &virtual_keywords {
        if name_lower.contains(keyword) {
            return false;
        }
    }
    true
}

/// Clean up device name to be more user-friendly (for display only)
fn clean_device_name(name: &str) -> String {
    let name = name.trim();
    
    // Remove common prefixes/suffixes that are not useful for identification
    let name = name
        .replace("surround", "")
        .replace("stereo", "")
        .replace("multichannel", "")
        .replace("iec958", "")
        .replace("iec", "")
        .replace("spdif", "")
        .replace("digital", "")
        .replace("hdmi", "")
        .replace("displayport", "")
        .replace("dp", "")
        .replace("analog", "")
        .replace("analog-output", "")
        .replace("analog-input", "")
        .replace("headphone", "")
        .replace("speaker", "")
        .replace("line", "")
        .replace("mic", "")
        .replace("microphone", "")
        .replace("input", "")
        .replace("output", "")
        .replace("device", "")
        .replace("card", "")
        .replace("dev", "")
        .replace("hw:", "")
        .replace("plughw:", "")
        .replace("default", "")
        .replace("default:", "")
        .replace("sysdefault", "")
        .replace("front", "")
        .replace("rear", "")
        .replace("center", "")
        .replace("lfe", "")
        .replace("side", "")
        .replace("unknown", "")
        .replace("generic", "")
        .replace("audio", "")
        .replace("sound", "")
        .replace("codec", "")
        .replace("dac", "")
        .replace("adc", "");
    
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
        "Audio Device".to_string()
    } else {
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
}

pub trait AudioBackend: Send + Sync {
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn sample_rate(&self) -> f32;
    fn channels(&self) -> u16;
    fn input_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
    fn start_input_stream(&self, device_id: &str, config: &StreamConfig, sender: Arc<Mutex<Option<mpsc::Sender<AudioBuffer>>>>) -> Result<()>;
    fn stop_input_stream(&self) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub raw_name: String,
    pub is_default_input: bool,
    pub is_default_output: bool,
    pub max_input_channels: u32,
    pub max_output_channels: u32,
    pub sample_rates: Vec<u32>,
}

pub struct CpalBackend {
    sample_rate: f32,
    channels: u16,
    running: Arc<Mutex<bool>>,
    input_running: Arc<Mutex<bool>>,
    input_sender: Arc<Mutex<Option<mpsc::Sender<AudioBuffer>>>>,
    output_stream: Arc<Mutex<SendStream>>,
    input_stream: Arc<Mutex<SendStream>>,
}

impl CpalBackend {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No output device available"))?;
        
        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0 as f32;
        let channels = config.channels();

        Ok(Self {
            sample_rate,
            channels,
            running: Arc::new(Mutex::new(false)),
            input_running: Arc::new(Mutex::new(false)),
            input_sender: Arc::new(Mutex::new(None)),
            output_stream: Arc::new(Mutex::new(SendStream(None))),
            input_stream: Arc::new(Mutex::new(SendStream(None))),
        })
    }

    pub fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();
        
        for (index, device) in host.input_devices()?.enumerate() {
            let raw_name = device.name().unwrap_or_else(|_| format!("Unknown Input {}", index));
            let name = clean_device_name(&raw_name);
            let is_default = host.default_input_device()
                .map(|d| d.name().unwrap_or_default() == name)
                .unwrap_or(false);
            
            // Filter out virtual/pseudo devices - only show real hardware
            if !is_real_hardware_device(&name) {
                continue;
            }
            
            let mut sample_rates = Vec::new();
            let mut max_input_channels = 0;
            if let Ok(configs) = device.supported_input_configs() {
                for config in configs {
                    sample_rates.push(config.min_sample_rate().0);
                    sample_rates.push(config.max_sample_rate().0);
                    max_input_channels = max_input_channels.max(config.channels());
                }
            }
            sample_rates.sort();
            sample_rates.dedup();
            
            // Create a unique ID using index and name
            let id = format!("input_{}_{}", index, name.replace(' ', "_"));
            
            devices.push(AudioDeviceInfo {
                id,
                name,
                raw_name: raw_name.clone(),
                is_default_input: is_default,
                is_default_output: false,
                max_input_channels: max_input_channels as u32,
                max_output_channels: 0,
                sample_rates,
            });
        }
        
        // If no real hardware devices found, fall back to all devices
        if devices.is_empty() {
            for (index, device) in host.input_devices()?.enumerate() {
                let raw_name = device.name().unwrap_or_else(|_| format!("Unknown Input {}", index));
                let name = clean_device_name(&raw_name);
                let is_default = host.default_input_device()
                    .map(|d| d.name().unwrap_or_default() == raw_name)
                    .unwrap_or(false);
                
                let mut sample_rates = Vec::new();
                let mut max_input_channels = 0;
                if let Ok(configs) = device.supported_input_configs() {
                    for config in configs {
                        sample_rates.push(config.min_sample_rate().0);
                        sample_rates.push(config.max_sample_rate().0);
                    }
                }
                sample_rates.sort();
                sample_rates.dedup();
                
                let id = format!("input_{}_{}", index, name.replace(' ', "_"));
                
devices.push(AudioDeviceInfo {
                id,
                name,
                raw_name: raw_name.clone(),
                is_default_input: is_default,
                is_default_output: false,
                max_input_channels: 0,
                max_output_channels: 0,
                sample_rates,
            });
            }
        }
        
        Ok(devices)
    }

    pub fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();
        
        for (index, device) in host.output_devices()?.enumerate() {
            let raw_name = device.name().unwrap_or_else(|_| format!("Unknown Output {}", index));
            let name = clean_device_name(&raw_name);
            let is_default = host.default_output_device()
                .map(|d| d.name().unwrap_or_default() == raw_name)
                .unwrap_or(false);
            
            let mut sample_rates = Vec::new();
            let mut max_output_channels = 0;
            if let Ok(configs) = device.supported_output_configs() {
                for config in configs {
                    sample_rates.push(config.min_sample_rate().0);
                    sample_rates.push(config.max_sample_rate().0);
                    max_output_channels = max_output_channels.max(config.channels());
                }
            }
            sample_rates.sort();
            sample_rates.dedup();
            
            let id = format!("output_{}_{}", index, name.replace(' ', "_"));
            
            devices.push(AudioDeviceInfo {
                id,
                name,
                raw_name: raw_name.clone(),
                is_default_input: false,
                is_default_output: is_default,
                max_input_channels: 0,
                max_output_channels: max_output_channels as u32,
                sample_rates,
            });
        }
        
        Ok(devices)
    }
}

impl AudioBackend for CpalBackend {
    fn start(&self) -> Result<()> {
        let mut running_lock = self.running.lock().unwrap();
        if *running_lock {
            return Ok(());
        }
        drop(running_lock);

        let host = cpal::default_host();
        let device = host.default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No output device available"))?;
        
        let config = device.default_output_config()?;
        let _sample_rate = config.sample_rate().0 as f32;
        let _channels = config.channels();

        let running = Arc::clone(&self.running);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config.into(), &running)?,
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config.into(), &running)?,
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config.into(), &running)?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        stream.play()?;
        *running.lock().unwrap() = true;
        
        // Store the stream so we can stop it later
        *self.output_stream.lock().unwrap() = SendStream::new(stream);
        
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let mut running = self.running.lock().unwrap();
        *running = false;
        
        // Properly stop and drop the output stream
        if let Some(stream) = self.output_stream.lock().unwrap().take() {
            drop(stream);
        }
        Ok(())
    }

    fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        self.list_input_devices()
    }

    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        self.list_output_devices()
    }

    fn start_input_stream(&self, device_id: &str, config: &StreamConfig, sender: Arc<Mutex<Option<mpsc::Sender<AudioBuffer>>>>) -> Result<()> {
        let mut input_running = self.input_running.lock().unwrap();
        if *input_running {
            return Ok(());
        }
        drop(input_running);

        let host = cpal::default_host();
        
        // First, get all input devices to find the one matching the device_id
        let devices = self.list_input_devices()?;
        let device_info = devices.iter()
            .find(|d| d.id == device_id)
            .ok_or_else(|| anyhow::anyhow!("Input device not found: {} (available: {:?})", device_id, devices.iter().map(|d| &d.id).collect::<Vec<_>>()))?;
        
        let host = cpal::default_host();
        let device = {
            let cpal_devices: Vec<_> = host.input_devices()?.collect();
            let available: Vec<_> = cpal_devices.iter().map(|d| d.name().unwrap_or_default()).collect();
            cpal_devices.into_iter()
                .find(|d| d.name().unwrap_or_default() == device_info.raw_name)
                .ok_or_else(|| anyhow::anyhow!("Input device not found (raw_name: {}), available: {:?}", device_info.raw_name, available))?
        };

        let supported_config = device.default_input_config()?;
        let _sample_rate = supported_config.sample_rate().0 as f32;
        let _channels = supported_config.channels();

        let running = Arc::clone(&self.input_running);
        let sender_clone = Arc::clone(&sender);

        let stream = match supported_config.sample_format() {
            SampleFormat::F32 => build_input_stream::<f32>(&device, &supported_config.into(), &running, sender_clone)?,
            SampleFormat::I16 => build_input_stream::<i16>(&device, &supported_config.into(), &running, sender_clone)?,
            SampleFormat::U16 => build_input_stream::<u16>(&device, &supported_config.into(), &running, sender_clone)?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        stream.play()?;
        *running.lock().unwrap() = true;
        
        // Store the stream so we can stop it later
        *self.input_stream.lock().unwrap() = SendStream::new(stream);
        
        Ok(())
    }

    fn stop_input_stream(&self) -> Result<()> {
        let mut running = self.input_running.lock().unwrap();
        *running = false;
        
        // Properly stop and drop the input stream
        if let Some(stream) = self.input_stream.lock().unwrap().take() {
            drop(stream);
        }
        Ok(())
    }
}

fn build_stream<T: Sample + SizedSample + Send + 'static + Default>(
    device: &cpal::Device,
    config: &StreamConfig,
    running: &Arc<Mutex<bool>>,
) -> Result<Stream> {
    let err_fn = |err: cpal::StreamError| eprintln!("Audio stream error: {}", err);
    let running = Arc::clone(running);
    
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let running = running.lock().unwrap();
            if *running {
                for sample in data.iter_mut() {
                    *sample = T::default();
                }
            }
        },
        err_fn,
        None,
    )?;
    
    Ok(stream)
}

fn build_input_stream<T: Sample + SizedSample + Send + 'static + Default + num_traits::ToPrimitive>(
    device: &cpal::Device,
    config: &StreamConfig,
    running: &Arc<Mutex<bool>>,
    sender: Arc<Mutex<Option<mpsc::Sender<AudioBuffer>>>>,
) -> Result<Stream> {
    let err_fn = |err: cpal::StreamError| eprintln!("Audio input stream error: {}", err);
    let running = Arc::clone(running);
    let sender = Arc::clone(&sender);
    
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let running = running.lock().unwrap();
            if *running {
                if let Ok(sender_guard) = sender.lock() {
                    if let Some(tx) = sender_guard.as_ref() {
                        let mut buffer = AudioBuffer::new(1, data.len());
                        for (i, sample) in data.iter().enumerate() {
                            if let Some(val) = sample.to_f32() {
                                buffer.data[i] = val;
                            }
                        }
                        let _ = tx.send(buffer);
                    }
                }
            }
        },
        err_fn,
        None,
    )?;
    
    Ok(stream)
}

pub fn create_default_backend() -> Result<Box<dyn AudioBackend>> {
    Ok(Box::new(CpalBackend::new()?))
}