use crate::track::{Track, TrackId, TrackType};
use crate::clip::{ClipId, AudioClip, MidiClip};
use crate::automation::{AutomationLane, AutomationEvent, AutomationLaneId};
use crate::audio_pool::{AudioFileRef, AudioPool};
use crate::serialization::ProjectSerialization;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub sample_rate: u32,
    pub bit_depth: BitDepth,
    pub tempo_map: TempoMap,
    pub time_signature_map: TimeSignatureMap,
    pub tracks: HashMap<TrackId, Track>,
    pub track_order: Vec<TrackId>,
    pub marker_regions: Vec<MarkerRegion>,
    pub audio_pool: AudioPool,
    pub automation_lanes: HashMap<AutomationLaneId, AutomationLane>,
    pub preferences: Preferences,
    pub project_path: Option<PathBuf>,
    pub modified: bool,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        let id = Uuid::new_v4();
        let mut project = Self {
            id,
            name: name.into(),
            sample_rate: 48000,
            bit_depth: BitDepth::Float32,
            tempo_map: TempoMap::default(),
            time_signature_map: TimeSignatureMap::default(),
            tracks: HashMap::new(),
            track_order: Vec::new(),
            marker_regions: Vec::new(),
            audio_pool: AudioPool::new(),
            automation_lanes: HashMap::new(),
            preferences: Preferences::default(),
            project_path: None,
            modified: false,
        };
        
        project.add_track(Track::new_audio("Audio 1"));
        project.add_track(Track::new_midi("MIDI 1"));
        project
    }

    pub fn add_track(&mut self, track: Track) -> TrackId {
        let id = track.id;
        self.tracks.insert(id, track);
        self.track_order.push(id);
        self.modified = true;
        id
    }

    pub fn remove_track(&mut self, track_id: TrackId) -> bool {
        if self.tracks.remove(&track_id).is_some() {
            self.track_order.retain(|&id| id != track_id);
            self.modified = true;
            true
        } else {
            false
        }
    }

    pub fn move_track(&mut self, track_id: TrackId, new_index: usize) -> bool {
        if let Some(pos) = self.track_order.iter().position(|&id| id == track_id) {
            self.track_order.remove(pos);
            self.track_order.insert(new_index.min(self.track_order.len()), track_id);
            self.modified = true;
            true
        } else {
            false
        }
    }

    pub fn get_track(&self, track_id: TrackId) -> Option<&Track> {
        self.tracks.get(&track_id)
    }

    pub fn get_track_mut(&mut self, track_id: TrackId) -> Option<&mut Track> {
        self.tracks.get_mut(&track_id)
    }

    pub fn tracks_iter(&self) -> impl Iterator<Item = &Track> {
        self.track_order.iter().filter_map(|id| self.tracks.get(id))
    }

    pub fn track_ids(&self) -> Vec<TrackId> {
        self.track_order.clone()
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        self.modified = true;
    }

    pub fn set_bit_depth(&mut self, bit_depth: BitDepth) {
        self.bit_depth = bit_depth;
        self.modified = true;
    }

    pub fn duration_samples(&self) -> u64 {
        self.tempo_map.duration_samples(self.sample_rate as f64)
    }

    pub fn duration_ticks(&self) -> u64 {
        self.tempo_map.duration_ticks()
    }

    pub fn save(&mut self, path: impl Into<PathBuf>) -> Result<()> {
        let path = path.into();
        ProjectSerialization::save(self, &path)?;
        self.project_path = Some(path);
        self.modified = false;
        Ok(())
    }

    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut project = ProjectSerialization::load(&path)?;
        project.project_path = Some(path);
        project.modified = false;
        Ok(project)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BitDepth {
    Int16,
    Int24,
    Int32,
    Float32,
}

impl Default for BitDepth {
    fn default() -> Self {
        BitDepth::Float32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoMap {
    events: Vec<TempoEvent>,
    default_bpm: f32,
}

impl TempoMap {
    pub fn new(default_bpm: f32) -> Self {
        Self {
            events: vec![TempoEvent { tick: 0, bpm: default_bpm, ramp: None }],
            default_bpm,
        }
    }

    pub fn default_bpm(&self) -> f32 {
        self.default_bpm
    }

    pub fn set_default_bpm(&mut self, bpm: f32) {
        self.default_bpm = bpm;
        if let Some(first) = self.events.first_mut() {
            first.bpm = bpm;
        }
    }

    pub fn events(&self) -> &[TempoEvent] {
        &self.events
    }

    pub fn add_tempo_change(&mut self, tick: u64, bpm: f32, ramp: Option<TempoRamp>) {
        let idx = self.events.partition_point(|e| e.tick < tick);
        self.events.insert(idx, TempoEvent { tick, bpm, ramp });
    }

    pub fn bpm_at_tick(&self, tick: u64) -> f32 {
        self.events
            .iter()
            .rev()
            .find(|e| e.tick <= tick)
            .map(|e| e.bpm)
            .unwrap_or(self.default_bpm)
    }

    pub fn tick_to_sample(&self, tick: u64, sample_rate: f64) -> u64 {
        if self.events.len() <= 1 {
            let bpm = self.default_bpm as f64;
            return ((tick as f64 / 960.0) * (60.0 / bpm) * sample_rate) as u64;
        }

        let mut samples = 0.0;
        let mut prev_tick = 0u64;
        let mut prev_bpm = self.default_bpm as f64;

        for event in &self.events {
            if event.tick > tick {
                let segment_ticks = tick - prev_tick;
                samples += self.ticks_to_samples(segment_ticks, prev_bpm, sample_rate);
                break;
            }

            let segment_ticks = event.tick - prev_tick;
            samples += self.ticks_to_samples(segment_ticks, prev_bpm, sample_rate);
            prev_tick = event.tick;
            prev_bpm = event.bpm as f64;
        }

        samples as u64
    }

    pub fn sample_to_tick(&self, sample: u64, sample_rate: f64) -> u64 {
        if self.events.len() <= 1 {
            let bpm = self.default_bpm as f64;
            return ((sample as f64 / sample_rate) * (bpm / 60.0) * 960.0) as u64;
        }

        let mut accumulated_samples = 0.0;
        let mut prev_tick = 0u64;
        let mut prev_bpm = self.default_bpm as f64;

        for event in &self.events {
            let segment_ticks = event.tick - prev_tick;
            let segment_samples = self.ticks_to_samples(segment_ticks, prev_bpm, sample_rate);
            
            if accumulated_samples + segment_samples > sample as f64 {
                let remaining = sample as f64 - accumulated_samples;
                let ratio = remaining / segment_samples;
                return prev_tick + (segment_ticks as f64 * ratio) as u64;
            }

            accumulated_samples += segment_samples;
            prev_tick = event.tick;
            prev_bpm = event.bpm as f64;
        }

        prev_tick
    }

    fn ticks_to_samples(&self, ticks: u64, bpm: f64, sample_rate: f64) -> f64 {
        (ticks as f64 / 960.0) * (60.0 / bpm) * sample_rate
    }

    pub fn duration_ticks(&self) -> u64 {
        self.events.last().map(|e| e.tick).unwrap_or(0)
    }

    pub fn duration_samples(&self, sample_rate: f64) -> u64 {
        self.tick_to_sample(self.duration_ticks(), sample_rate)
    }
}

impl Default for TempoMap {
    fn default() -> Self {
        Self::new(120.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoEvent {
    pub tick: u64,
    pub bpm: f32,
    pub ramp: Option<TempoRamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoRamp {
    pub target_bpm: f32,
    pub end_tick: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSignatureMap {
    events: Vec<TimeSignatureEvent>,
}

impl TimeSignatureMap {
    pub fn new() -> Self {
        Self {
            events: vec![TimeSignatureEvent { tick: 0, numerator: 4, denominator: 4 }],
        }
    }

    pub fn events(&self) -> &[TimeSignatureEvent] {
        &self.events
    }

    pub fn add_change(&mut self, tick: u64, numerator: u8, denominator: u8) {
        let idx = self.events.partition_point(|e| e.tick < tick);
        self.events.insert(idx, TimeSignatureEvent { tick, numerator, denominator });
    }

    pub fn time_sig_at_tick(&self, tick: u64) -> (u8, u8) {
        self.events
            .iter()
            .rev()
            .find(|e| e.tick <= tick)
            .map(|e| (e.numerator, e.denominator))
            .unwrap_or((4, 4))
    }
}

impl Default for TimeSignatureMap {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSignatureEvent {
    pub tick: u64,
    pub numerator: u8,
    pub denominator: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkerRegion {
    pub id: MarkerId,
    pub name: String,
    pub start_tick: u64,
    pub end_tick: u64,
    pub color: Color,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MarkerId(pub Uuid);

impl MarkerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MarkerId {
    fn default() -> Self {
        Self::new()
    }
}



#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self(r, g, b, 255)
    }

    pub const fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(r, g, b, a)
    }

    pub fn track_color(index: usize) -> Self {
        const COLORS: &[Color] = &[
            Color::new(0xe5, 0x14, 0x00),
            Color::new(0xa4, 0xc4, 0x00),
            Color::new(0x00, 0xab, 0xa9),
            Color::new(0x60, 0xa9, 0x17),
            Color::new(0x00, 0x50, 0xef),
            Color::new(0xaa, 0x00, 0xff),
            Color::new(0xf0, 0xa3, 0x0a),
            Color::new(0xff, 0x8c, 0x00),
            Color::new(0x00, 0xab, 0xa9),
            Color::new(0x8c, 0xbf, 0x26),
            Color::new(0xff, 0x8c, 0x00),
            Color::new(0xf0, 0xa3, 0x0a),
            Color::new(0xe3, 0x00, 0x8c),
            Color::new(0x82, 0x5a, 0x2c),
            Color::new(0x6d, 0x87, 0x64),
            Color::new(0x1b, 0xa1, 0xe2),
        ];
        COLORS[index % COLORS.len()]
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::new(0xcc, 0xcc, 0xcc)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub audio: AudioPreferences,
    pub ui: UiPreferences,
    pub editing: EditingPreferences,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            audio: AudioPreferences::default(),
            ui: UiPreferences::default(),
            editing: EditingPreferences::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioPreferences {
    pub sample_rate: u32,
    pub buffer_size: usize,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub driver: String,
}

impl Default for AudioPreferences {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            buffer_size: 256,
            input_device: None,
            output_device: None,
            driver: "auto".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiPreferences {
    pub theme: String,
    pub track_height: f32,
    pub show_ruler: bool,
    pub show_grid: bool,
    pub font_size: f32,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            track_height: 60.0,
            show_ruler: true,
            show_grid: true,
            font_size: 13.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditingPreferences {
    pub snap_enabled: bool,
    pub snap_grid: SnapGrid,
    pub default_fade_duration: f32,
}

impl Default for EditingPreferences {
    fn default() -> Self {
        Self {
            snap_enabled: true,
            snap_grid: SnapGrid::Bar,
            default_fade_duration: 0.01,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SnapGrid {
    Off,
    Bar,
    Beat,
    Division(u8),
    Tick,
}