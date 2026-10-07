use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SizedSample, Stream, StreamConfig};
use std::sync::{Arc, Mutex};

pub trait AudioBackend: Send + Sync {
    fn start(&self) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn sample_rate(&self) -> f32;
    fn channels(&self) -> u16;
}

pub struct CpalBackend {
    sample_rate: f32,
    channels: u16,
    running: Arc<Mutex<bool>>,
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
        })
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
        
        // Note: Stream is leaked to keep it alive
        // In a real implementation, we'd store it properly
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

pub fn create_default_backend() -> Result<Box<dyn AudioBackend>> {
    Ok(Box::new(CpalBackend::new()?))
}