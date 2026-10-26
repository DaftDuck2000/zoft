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

/// Ghost-note visibility defaults to on for new and legacy tracks.
fn default_show_as_ghost() -> bool {
    true
}

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
    /// MIDI clips attached to this track (keys from `Project::midi_clips`).
    #[serde(default)]
    pub midi_clips: Vec<ClipId>,
    /// Include this track's notes as ghost notes in piano rolls.
    #[serde(default = "default_show_as_ghost")]
    pub show_as_ghost: bool,
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
            midi_clips: Vec::new(),
            show_as_ghost: true,
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

    pub fn add_midi_clip(&mut self, clip_id: ClipId) {
        if !self.midi_clips.contains(&clip_id) {
            self.midi_clips.push(clip_id);
        }
    }

    pub fn remove_midi_clip(&mut self, clip_id: ClipId) -> bool {
        if let Some(pos) = self.midi_clips.iter().position(|&id| id == clip_id) {
            self.midi_clips.remove(pos);
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

/// Filter response for the built-in track filter effect.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FilterMode {
    LowPass,
    HighPass,
}

/// Built-in real-time insert effects. These live on a track, so they work for
/// audio and MIDI/instrument tracks alike and can be targeted by automation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TrackEffect {
    BitCrusher { bits: u32, downsample: u32 },
    Saturation { drive_db: f32, mix: f32 },
    Filter { mode: FilterMode, cutoff: f32 },
    Tremolo { rate: f32, depth: f32 },
}

impl TrackEffect {
    /// A sensible starting point when adding a new effect slot.
    pub fn default_saturation() -> Self {
        Self::Saturation {
            drive_db: 12.0,
            mix: 1.0,
        }
    }

    pub fn default_bit_crusher() -> Self {
        Self::BitCrusher {
            bits: 8,
            downsample: 4,
        }
    }

    pub fn default_filter() -> Self {
        Self::Filter {
            mode: FilterMode::LowPass,
            cutoff: 8_000.0,
        }
    }

    pub fn default_tremolo() -> Self {
        Self::Tremolo {
            rate: 5.0,
            depth: 0.5,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::BitCrusher { .. } => "Bit Crusher",
            Self::Saturation { .. } => "Saturation",
            Self::Filter { .. } => "Filter",
            Self::Tremolo { .. } => "Tremolo",
        }
    }
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertSlot {
    pub plugin_id: Option<PluginId>,
    /// Built-in effect processed on the track. `None` leaves the slot empty.
    #[serde(default)]
    pub effect: Option<TrackEffect>,
    pub enabled: bool,
    pub input_gain: f32,
    pub output_gain: f32,
    pub mix: f32,
}

impl InsertSlot {
    pub fn new() -> Self {
        Self {
            plugin_id: None,
            effect: None,
            enabled: true,
            input_gain: 1.0,
            output_gain: 1.0,
            mix: 1.0,
        }
    }

    pub fn with_effect(effect: TrackEffect) -> Self {
        Self {
            effect: Some(effect),
            ..Self::new()
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
        let norm = std::f32::consts::SQRT_2;
        Self {
            left_gain: angle.cos() * norm,
            right_gain: angle.sin() * norm,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_slots_can_hold_builtin_effects() {
        let slot = InsertSlot::with_effect(TrackEffect::default_saturation());
        assert!(slot.enabled);
        assert!(slot.plugin_id.is_none());
        assert_eq!(slot.mix, 1.0);
        assert_eq!(
            slot.effect,
            Some(TrackEffect::Saturation {
                drive_db: 12.0,
                mix: 1.0
            })
        );
    }

    #[test]
    fn every_builtin_effect_has_a_default_and_label() {
        let effects = [
            TrackEffect::default_saturation(),
            TrackEffect::default_filter(),
            TrackEffect::default_bit_crusher(),
            TrackEffect::default_tremolo(),
        ];
        for effect in effects {
            assert!(!effect.kind_name().is_empty());
        }
    }

    #[test]
    fn effect_slots_compare_by_value_for_undo() {
        let a = InsertSlot::with_effect(TrackEffect::default_tremolo());
        let mut b = a.clone();
        assert_eq!(a, b);
        b.enabled = false;
        assert_ne!(a, b);
    }

    #[test]
    fn pan_law_is_unity_at_center_and_equal_power_at_edges() {
        let center = PanLaw::from_linear(0.0);
        assert!((center.left_gain - 1.0).abs() < 1e-6);
        assert!((center.right_gain - 1.0).abs() < 1e-6);

        let left = PanLaw::from_linear(-1.0);
        assert!((left.left_gain - std::f32::consts::SQRT_2).abs() < 1e-6);
        assert!(left.right_gain.abs() < 1e-6);

        let right = PanLaw::from_linear(1.0);
        assert!(right.left_gain.abs() < 1e-6);
        assert!((right.right_gain - std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn new_tracks_show_as_ghosts_by_default() {
        let track = Track::new_audio("Audio");
        assert!(track.show_as_ghost);
    }

    #[test]
    fn legacy_tracks_without_ghost_flag_load_as_visible() {
        let track = Track::new_audio("Audio");
        let mut value = serde_json::to_value(&track).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("show_as_ghost");
        let loaded: Track = serde_json::from_value(value).unwrap();
        assert!(loaded.show_as_ghost);
    }
}
