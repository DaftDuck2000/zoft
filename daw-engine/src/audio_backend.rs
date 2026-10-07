use crate::dsp::AudioBuffer;
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub trait AudioBackend: Send + Sync {
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn sample_rate(&self) -> f32;
    fn channels(&self) -> u16;
    fn input_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
    fn output_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
    fn start_input_stream(&self, device_id: &str, config: &StreamConfig, sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<AudioBuffer>>>>) -> Result<()>;
    fn stop_input_stream(&self) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
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
    input_sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<AudioBuffer>>>>,
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
        })
    }

    pub fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();
        
        for device in host.input_devices()? {
            let name = device.name().unwrap_or_default();
            let is_default = host.default_input_device().map(|d| d.name().unwrap_or_default() == name).unwrap_or(false);
            
            let mut sample_rates = Vec::new();
            if let Ok(configs) = device.supported_input_configs() {
                for config in configs {
                    sample_rates.push(config.min_sample_rate().0);
                    sample_rates.push(config.max_sample_rate().0);
                }
            }
            sample_rates.sort();
            sample_rates.dedup();
            
            let max_input_channels = device
                .supported_input_configs()
                .ok()
                .and_then(|configs| configs.max_by_key(|c| c.channels()))
                .map(|c| c.channels() as u32)
                .unwrap_or(0);
            
            devices.push(AudioDeviceInfo {
                id: name.clone(),
                name,
                is_default_input: is_default,
                is_default_output: false,
                max_input_channels,
                max_output_channels: 0,
                sample_rates,
            });
        }
        
        Ok(devices)
    }

    pub fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();
        
        for device in host.output_devices()? {
            let name = device.name().unwrap_or_default();
            let is_default = host.default_output_device().map(|d| d.name().unwrap_or_default() == name).unwrap_or(false);
            
            let mut sample_rates = Vec::new();
            if let Ok(configs) = device.supported_output_configs() {
                for config in configs {
                    sample_rates.push(config.min_sample_rate().0);
                    sample_rates.push(config.max_sample_rate().0);
                }
            }
            sample_rates.sort();
            sample_rates.dedup();
            
            let max_output_channels = device
                .supported_output_configs()
                .ok()
                .and_then(|configs| configs.max_by_key(|c| c.channels()))
                .map(|c| c.channels() as u32)
                .unwrap_or(0);
            
            devices.push(AudioDeviceInfo {
                id: name.clone(),
                name,
                is_default_input: false,
                is_default_output: is_default,
                max_input_channels: 0,
                max_output_channels,
                sample_rates,
            });
        }
        
        Ok(devices)
    }
}

impl AudioBackend for CpalBackend {
    fn start(&self) -> Result<()> {
        let running_lock = self.running.lock().unwrap();
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
        
        std::mem::forget(stream);
        
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        let mut running = self.running.lock().unwrap();
        *running = false;
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

    fn start_input_stream(&self, device_id: &str, _config: &StreamConfig, sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<AudioBuffer>>>>) -> Result<()> {
        let mut input_running = self.input_running.lock().unwrap();
        if *input_running {
            return Ok(());
        }
        drop(input_running);

        let host = cpal::default_host();
        let device = host.input_devices()?
            .find(|d| d.name().unwrap_or_default() == device_id)
            .ok_or_else(|| anyhow::anyhow!("Input device not found: {}", device_id))?;

        let config = device.default_input_config()?;
        let _sample_rate = config.sample_rate().0 as f32;
        let _channels = config.channels();

        let running = Arc::clone(&self.input_running);
        let sender_clone = Arc::clone(&sender);

        let stream = match config.sample_format() {
            SampleFormat::F32 => build_input_stream::<f32>(&device, &config.into(), &running, sender_clone)?,
            SampleFormat::I16 => build_input_stream::<i16>(&device, &config.into(), &running, sender_clone)?,
            SampleFormat::U16 => build_input_stream::<u16>(&device, &config.into(), &running, sender_clone)?,
            _ => return Err(anyhow::anyhow!("Unsupported sample format")),
        };

        stream.play()?;
        *running.lock().unwrap() = true;
        
        std::mem::forget(stream);
        
        Ok(())
    }

    fn stop_input_stream(&self) -> Result<()> {
        let mut running = self.input_running.lock().unwrap();
        *running = false;
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
    sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<AudioBuffer>>>>,
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
                        let _ = tx.try_send(buffer);
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