use crate::dsp::AudioBuffer;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

pub const DEFAULT_FRAMES_PER_PIXEL: u32 = 256;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

        let mono_samples: Vec<f32> = if channels == 2 {
            samples.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect()
        } else if channels > 2 {
            samples.chunks(channels).map(|c| c.iter().sum::<f32>() / channels as f32).collect()
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

#[derive(Debug, Clone)]
pub struct WaveformDetail {
    pub samples: Vec<f32>,
    pub window_start: u64,
    pub window_len: u32,
}

pub struct WaveformCache {
    cache: std::collections::HashMap<std::path::PathBuf, Arc<WaveformOverview>>,
    max_entries: usize,
}

impl WaveformCache {
    pub fn new() -> Self {
        Self {
            cache: std::collections::HashMap::new(),
            max_entries: 100,
        }
    }

    pub fn get_or_generate(&mut self, path: &Path, frames_per_pixel: u32) -> Result<Arc<WaveformOverview>> {
        if let Some(overview) = self.cache.get(path) {
            return Ok(Arc::clone(overview));
        }

        let overview = Arc::new(WaveformOverview::generate_from_file(path, frames_per_pixel)?);

        if self.cache.len() >= self.max_entries {
            if let Some(key) = self.cache.keys().next().cloned() {
                self.cache.remove(&key);
            }
        }
        self.cache.insert(path.to_path_buf(), Arc::clone(&overview));

        Ok(overview)
    }

    pub fn insert(&mut self, path: std::path::PathBuf, overview: Arc<WaveformOverview>) {
        if self.cache.len() >= self.max_entries {
            if let Some(key) = self.cache.keys().next().cloned() {
                self.cache.remove(&key);
            }
        }
        self.cache.insert(path, overview);
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

pub enum WaveformTask {
    GenerateOverview {
        path: std::path::PathBuf,
        frames_per_pixel: u32,
        sender: tokio::sync::oneshot::Sender<Result<Arc<WaveformOverview>>>,
    },
}

pub struct WaveformGenerator {
    task_sender: std::sync::OnceLock<Arc<mpsc::UnboundedSender<WaveformTask>>>,
}

impl WaveformGenerator {
    pub fn new() -> Self {
        Self {
            task_sender: std::sync::OnceLock::new(),
        }
    }

    fn get_or_init_sender(&self) -> &Arc<mpsc::UnboundedSender<WaveformTask>> {
        self.task_sender.get_or_init(|| {
            let (tx, mut rx) = mpsc::unbounded_channel::<WaveformTask>();

            tokio::spawn(async move {
                while let Some(task) = rx.recv().await {
                    match task {
                        WaveformTask::GenerateOverview { path, frames_per_pixel, sender } => {
                            let result = WaveformOverview::generate_from_file(&path, frames_per_pixel)
                                .map(Arc::new);
                            let _ = sender.send(result);
                        }
                    }
                }
            });

            Arc::new(tx)
        })
    }

    pub fn generate_overview(&self, path: std::path::PathBuf, frames_per_pixel: u32) -> tokio::sync::oneshot::Receiver<Result<Arc<WaveformOverview>>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.get_or_init_sender().send(WaveformTask::GenerateOverview {
            path,
            frames_per_pixel,
            sender: tx,
        });
        rx
    }
}

impl Default for WaveformGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WaveformGenerator {
    fn clone(&self) -> Self {
        Self {
            task_sender: std::sync::OnceLock::new(),
        }
    }
}