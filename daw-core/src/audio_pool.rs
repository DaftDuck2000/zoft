use crate::project::BitDepth;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

slotmap::new_key_type! {
    pub struct AudioFileRefId;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFileRef {
    pub id: AudioFileRefId,
    pub path: PathBuf,
    pub original_path: PathBuf,
    pub duration_samples: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: BitDepth,
    pub metadata: AudioMetadata,
    pub peak_level: f32,
    pub rms_level: f32,
    pub overview: Option<WaveformOverview>,
}

impl AudioFileRef {
    pub fn new(path: PathBuf, duration_samples: u64, sample_rate: u32, channels: u16, bit_depth: BitDepth) -> Self {
        Self {
            id: AudioFileRefId::default(),
            path: path.clone(),
            original_path: path,
            duration_samples,
            sample_rate,
            channels,
            bit_depth,
            metadata: AudioMetadata::default(),
            peak_level: 0.0,
            rms_level: 0.0,
            overview: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: u32,
    pub genre: String,
    pub comment: String,
    pub track_number: u32,
    pub total_tracks: u32,
    pub coding_history: String,
    pub originator: String,
    pub originator_ref: String,
    pub origination_date: String,
    pub origination_time: String,
    pub time_reference: u64,
    pub version: u16,
    pub umid: Vec<u8>,
}

impl Default for AudioMetadata {
    fn default() -> Self {
        Self {
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            year: 0,
            genre: String::new(),
            comment: String::new(),
            track_number: 0,
            total_tracks: 0,
            coding_history: String::new(),
            originator: String::new(),
            originator_ref: String::new(),
            origination_date: String::new(),
            origination_time: String::new(),
            time_reference: 0,
            version: 1,
            umid: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

        let duration_pixels = min_max.len() as u32;

        let rms: Vec<f32> = samples
            .chunks(frames_per_pixel as usize)
            .map(|chunk| {
                let sum: f32 = chunk.iter().map(|s| s * s).sum();
                (sum / chunk.len() as f32).sqrt()
            })
            .collect();

        Self {
            frames_per_pixel,
            min_max,
            rms: Some(rms),
            duration_pixels,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioPool {
    files: HashMap<AudioFileRefId, AudioFileRef>,
    file_order: Vec<AudioFileRefId>,
}

impl AudioPool {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            file_order: Vec::new(),
        }
    }

    pub fn add(&mut self, file: AudioFileRef) -> AudioFileRefId {
        let id = file.id;
        self.files.insert(id, file);
        self.file_order.push(id);
        id
    }

    pub fn get(&self, id: AudioFileRefId) -> Option<&AudioFileRef> {
        self.files.get(&id)
    }

    pub fn get_mut(&mut self, id: AudioFileRefId) -> Option<&mut AudioFileRef> {
        self.files.get_mut(&id)
    }

    pub fn remove(&mut self, id: AudioFileRefId) -> bool {
        if self.files.remove(&id).is_some() {
            self.file_order.retain(|&fid| fid != id);
            true
        } else {
            false
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &AudioFileRef> {
        self.file_order.iter().filter_map(|id| self.files.get(id))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn total_duration_samples(&self) -> u64 {
        self.files.values().map(|f| f.duration_samples).sum()
    }

    pub fn find_missing_files(&self) -> Vec<AudioFileRefId> {
        self.files
            .iter()
            .filter(|(_, f)| !f.path.exists())
            .map(|(id, _)| *id)
            .collect()
    }
}

impl Default for AudioPool {
    fn default() -> Self {
        Self::new()
    }
}