use crate::dsp::AudioBuffer;
use anyhow::Result;
use std::path::Path;

pub struct WaveformOverview {
    pub frames_per_pixel: u32,
    pub min_max: Vec<(f32, f32)>,
    pub rms: Option<Vec<f32>>,
    pub duration_pixels: u32,
}

impl WaveformOverview {
    pub fn generate(samples: &[f32], frames_per_pixel: u32) -> Self {
        let min_max: Vec<(f32, f32)> = samples
            .chunks(frames_per_pixel as usize)
            .map(|chunk| {
                let min = chunk.iter().copied().fold(f32::INFINITY, f32::min);
                let max = chunk.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                (min, max)
            })
            .collect();

        let rms: Vec<f32> = samples
            .chunks(frames_per_pixel as usize)
            .map(|chunk| {
                let sum: f32 = chunk.iter().map(|s| s * s).sum();
                (sum / chunk.len() as f32).sqrt()
            })
            .collect();

        let duration_pixels = min_max.len() as u32;
        Self {
            frames_per_pixel,
            min_max,
            rms: Some(rms),
            duration_pixels,
        }
    }

    pub fn generate_from_file(path: &Path, frames_per_pixel: u32) -> Result<Self> {
        let mut reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        let channels = spec.channels as usize;
        
        let samples: Vec<f32> = if spec.sample_format == hound::SampleFormat::Float {
            reader.samples::<f32>().filter_map(Result::ok).collect()
        } else {
            reader.samples::<i32>().filter_map(Result::ok)
                .map(|s| s as f32 / i32::MAX as f32)
                .collect()
        };

        // Convert to mono if stereo
        let mono_samples: Vec<f32> = if channels == 2 {
            samples.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect()
        } else {
            samples
        };

        Ok(Self::generate(&mono_samples, frames_per_pixel))
    }

    pub fn get_pixel_range(&self, start_sample: u64, samples_per_pixel: f32, num_pixels: u32) -> Vec<(f32, f32)> {
        let start_idx = (start_sample as f32 / samples_per_pixel) as usize;
        let end_idx = (start_idx + num_pixels as usize).min(self.min_max.len());
        self.min_max[start_idx..end_idx].to_vec()
    }
}

pub struct WaveformCache {
    cache: std::collections::HashMap<std::path::PathBuf, WaveformOverview>,
}

impl WaveformCache {
    pub fn new() -> Self {
        Self {
            cache: std::collections::HashMap::new(),
        }
    }

    pub fn get_or_generate(&mut self, path: &Path, frames_per_pixel: u32) -> Result<&WaveformOverview> {
        if !self.cache.contains_key(path) {
            let overview = WaveformOverview::generate_from_file(path, frames_per_pixel)?;
            self.cache.insert(path.to_path_buf(), overview);
        }
        Ok(self.cache.get(path).unwrap())
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

impl Default for WaveformCache {
    fn default() -> Self {
        Self::new()
    }
}