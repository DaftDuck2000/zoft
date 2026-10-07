use crate::clip::{AudioClip, MidiClip, ClipId};
use crate::automation::{AutomationLane, AutomationEvent, AutomationLaneId};
use crate::project::Color;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackId(pub Uuid);

impl TrackId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TrackId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub color: Color,
    pub track_type: TrackType,
    pub volume: f32,
    pub pan: PanLaw,
    pub mute: bool,
    pub solo: bool,
    pub record_arm: bool,
    pub monitor_mode: MonitorMode,
    pub input: TrackInput,
    pub output: TrackOutput,
    pub sends: Vec<Send>,
    pub inserts: Vec<InsertSlot>,
    pub automation_lanes: HashMap<AutomationLaneId, AutomationLane>,
    pub clips: Vec<ClipId>,
    pub visible: bool,
    pub height: f32,
    pub folder_parent: Option<TrackId>,
    pub folder_children: Vec<TrackId>,
    pub is_folder: bool,
}

impl Track {
    pub fn new_audio(name: impl Into<String>) -> Self {
        Self {
            id: TrackId::new(),
            name: name.into(),
            color: Color::track_color(0),
            track_type: TrackType::Audio,
            volume: 1.0,
            pan: PanLaw::center(),
            mute: false,
            solo: false,
            record_arm: false,
            monitor_mode: MonitorMode::Auto,
            input: TrackInput::None,
            output: TrackOutput::Master,
            sends: Vec::new(),
            inserts: Vec::new(),
            automation_lanes: HashMap::new(),
            clips: Vec::new(),
            visible: true,
            height: 60.0,
            folder_parent: None,
            folder_children: Vec::new(),
            is_folder: false,
        }
    }

    pub fn new_midi(name: impl Into<String>) -> Self {
        let mut track = Self::new_audio(name);
        track.track_type = TrackType::Midi;
        track
    }

    pub fn new_instrument(name: impl Into<String>) -> Self {
        let mut track = Self::new_midi(name);
        track.track_type = TrackType::Instrument;
        track
    }

    pub fn new_bus(name: impl Into<String>) -> Self {
        let mut track = Self::new_audio(name);
        track.track_type = TrackType::Bus;
        track
    }

    pub fn new_folder(name: impl Into<String>) -> Self {
        let mut track = Self::new_audio(name);
        track.track_type = TrackType::Folder;
        track.is_folder = true;
        track
    }

    pub fn add_clip(&mut self, clip_id: ClipId) {
        self.clips.push(clip_id);
    }

    pub fn remove_clip(&mut self, clip_id: ClipId) -> bool {
        if let Some(pos) = self.clips.iter().position(|&id| id == clip_id) {
            self.clips.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 4.0);
    }

    pub fn set_pan(&mut self, pan: f32) {
        self.pan = PanLaw::from_linear(pan.clamp(-1.0, 1.0));
    }

    pub fn db_to_gain(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }

    pub fn gain_to_db(gain: f32) -> f32 {
        20.0 * gain.max(1e-10).log10()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TrackType {
    Audio,
    Midi,
    Instrument,
    Bus,
    Folder,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MonitorMode {
    Off,
    Auto,
    On,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrackInput {
    Audio { device_id: String, channels: [u32; 2] },
    Midi { device_id: String, channel: Option<u8> },
    Bus { bus_id: TrackId },
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrackOutput {
    Audio { device_id: String, channels: [u32; 2] },
    Bus { bus_id: TrackId },
    Master,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Send {
    pub destination: TrackId,
    pub level: f32,
    pub pan: f32,
    pub pre_fader: bool,
    pub enabled: bool,
}

impl Send {
    pub fn new(destination: TrackId) -> Self {
        Self {
            destination,
            level: 0.0,
            pan: 0.0,
            pre_fader: false,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertSlot {
    pub plugin_id: Option<PluginId>,
    pub enabled: bool,
    pub input_gain: f32,
    pub output_gain: f32,
    pub mix: f32,
}

impl InsertSlot {
    pub fn new() -> Self {
        Self {
            plugin_id: None,
            enabled: true,
            input_gain: 1.0,
            output_gain: 1.0,
            mix: 1.0,
        }
    }
}

impl Default for InsertSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(pub Uuid);

impl PluginId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PluginId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PanLaw {
    pub left_gain: f32,
    pub right_gain: f32,
}

impl PanLaw {
    pub fn center() -> Self {
        Self {
            left_gain: 1.0,
            right_gain: 1.0,
        }
    }

    pub fn from_linear(pan: f32) -> Self {
        let pan = pan.clamp(-1.0, 1.0);
        let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        Self {
            left_gain: angle.cos(),
            right_gain: angle.sin(),
        }
    }

    pub fn from_db(pan_db: f32) -> Self {
        let pan = (pan_db / 6.0).clamp(-1.0, 1.0);
        Self::from_linear(pan)
    }

    pub fn to_linear(&self) -> f32 {
        let angle = self.right_gain.atan2(self.left_gain);
        (angle / std::f32::consts::FRAC_PI_4) - 1.0
    }
}

impl Default for PanLaw {
    fn default() -> Self {
        Self::center()
    }
}