use crate::project::Project;
use crate::track::{Track, TrackId, TrackType};
use crate::clip::{AudioClip, MidiClip, ClipId, TimelinePos};
use crate::automation::{AutomationLane, AutomationEvent, AutomationLaneId, ParameterId};
use crate::audio_pool::AudioFileRefId;
use anyhow::Result;
use serde::Serialize;
use std::any::Any;
use std::collections::HashMap;
use uuid::Uuid;

pub trait Command: Send + Sync + Any {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()>;
    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()>;
    fn description(&self) -> String;
    fn merge(&mut self, other: &dyn Command) -> Option<Box<dyn Command>> {
        None
    }
    fn is_noop(&self) -> bool {
        false
    }
}

pub struct CommandContext<'a> {
    pub project: &'a mut Project,
    pub engine_tx: &'a crossbeam::channel::Sender<EngineMsg>,
}

#[derive(Debug, Clone, Serialize)]
pub enum EngineMsg {
    Transport(TransportCmd),
    Track(TrackCmd),
    Clip(ClipCmd),
    Parameter(ParameterCmd),
    Plugin(PluginCmd),
    Project(ProjectCmd),
}

#[derive(Debug, Clone, Serialize)]
pub enum TransportCmd {
    Play,
    Stop,
    Seek { position: TimelinePos },
    SetTempo { bpm: f32, at_tick: Option<u64> },
    SetTimeSig { num: u8, denom: u8, at_tick: Option<u64> },
    SetLoop { start: TimelinePos, end: TimelinePos, enabled: bool },
    TapTempo,
}

#[derive(Debug, Clone, Serialize)]
pub enum TrackCmd {
    Add { track: Track },
    Remove { track_id: TrackId },
    Reorder { track_ids: Vec<TrackId> },
    SetName { track_id: TrackId, name: String },
    SetColor { track_id: TrackId, color: crate::project::Color },
    SetVolume { track_id: TrackId, volume: f32 },
    SetPan { track_id: TrackId, pan: f32 },
    SetMute { track_id: TrackId, mute: bool },
    SetSolo { track_id: TrackId, solo: bool },
    SetRecordArm { track_id: TrackId, armed: bool },
    SetMonitorMode { track_id: TrackId, mode: crate::track::MonitorMode },
    SetInput { track_id: TrackId, input: crate::track::TrackInput },
    SetOutput { track_id: TrackId, output: crate::track::TrackOutput },
}

#[derive(Debug, Clone, Serialize)]
pub enum ClipCmd {
    AddAudio { track_id: TrackId, clip: AudioClip },
    AddMidi { track_id: TrackId, clip: MidiClip },
    Remove { clip_id: ClipId },
    Move { clip_id: ClipId, new_track: TrackId, new_pos: TimelinePos },
    Copy { clip_ids: Vec<ClipId>, target_track: TrackId, target_pos: TimelinePos },
    Split { clip_id: ClipId, position: TimelinePos },
    TrimStart { clip_id: ClipId, new_offset: u64 },
    TrimEnd { clip_id: ClipId, new_length: u64 },
    SetGain { clip_id: ClipId, gain: f32 },
    SetFadeIn { clip_id: ClipId, fade: Option<crate::clip::Fade> },
    SetFadeOut { clip_id: ClipId, fade: Option<crate::clip::Fade> },
    SetLoop { clip_id: ClipId, enabled: bool, range: Option<std::ops::Range<u64>> },
    SetColor { clip_id: ClipId, color: Option<crate::project::Color> },
    SetMuted { clip_id: ClipId, muted: bool },
}

#[derive(Debug, Clone, Serialize)]
pub enum ParameterCmd {
    Set { target: ParamTarget, value: f32 },
    SetWithRamp { target: ParamTarget, value: f32, frames: u32, law: crate::automation::RampLaw },
    AddAutomationPoint { lane_id: AutomationLaneId, event: AutomationEvent },
    RemoveAutomationPoint { lane_id: AutomationLaneId, index: usize },
    MoveAutomationPoint { lane_id: AutomationLaneId, index: usize, new_tick: u64, new_value: f32 },
    SetAutomationShape { lane_id: AutomationLaneId, index: usize, shape: crate::automation::CurveShape },
    SetAutomationMode { lane_id: AutomationLaneId, mode: crate::automation::AutomationMode },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub enum ParamTarget {
    TrackVolume(TrackId),
    TrackPan(TrackId),
    TrackMute(TrackId),
    TrackSolo(TrackId),
    SendLevel { track_id: TrackId, send_index: usize },
    PluginParam { plugin_id: crate::track::PluginId, param_id: ParameterId },
}

#[derive(Debug, Clone, Serialize)]
pub enum PluginCmd {
    Load { track_id: TrackId, plugin_path: String },
    Unload { plugin_id: crate::track::PluginId },
    SetParam { plugin_id: crate::track::PluginId, param_id: ParameterId, value: f32 },
    SetState { plugin_id: crate::track::PluginId, state: Vec<u8> },
}

#[derive(Debug, Clone, Serialize)]
pub enum ProjectCmd {
    SetSampleRate(u32),
    SetBitDepth(crate::project::BitDepth),
    SetName(String),
    AddMarker { marker: crate::project::MarkerRegion },
    RemoveMarker { marker_id: crate::project::MarkerId },
}

#[derive(Debug, Clone, Serialize)]
pub struct AddTrackCmd {
    pub track: Track,
}

impl Command for AddTrackCmd {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.project.add_track(self.track.clone());
        Ok(())
    }

    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.project.remove_track(self.track.id);
        Ok(())
    }

    fn description(&self) -> String {
        format!("Add track: {}", self.track.name)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoveTrackCmd {
    pub track_id: TrackId,
    pub track_data: Track,
}

impl Command for RemoveTrackCmd {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.project.remove_track(self.track_id);
        Ok(())
    }

    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.project.tracks.insert(self.track_id, self.track_data.clone());
        ctx.project.track_order.push(self.track_id);
        Ok(())
    }

    fn description(&self) -> String {
        format!("Remove track: {}", self.track_data.name)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReorderTracksCmd {
    pub track_ids: Vec<TrackId>,
    pub old_order: Vec<TrackId>,
}

impl Command for ReorderTracksCmd {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.project.track_order = self.track_ids.clone();
        Ok(())
    }

    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.project.track_order = self.old_order.clone();
        Ok(())
    }

    fn description(&self) -> String {
        "Reorder tracks".to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AddAudioClipCmd {
    pub track_id: TrackId,
    pub clip: AudioClip,
}

impl Command for AddAudioClipCmd {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()> {
        if let Some(track) = ctx.project.get_track_mut(self.track_id) {
            track.add_clip(self.clip.id);
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()> {
        if let Some(track) = ctx.project.get_track_mut(self.track_id) {
            track.remove_clip(self.clip.id);
        }
        Ok(())
    }

    fn description(&self) -> String {
        format!("Add audio clip to track {}", self.track_id.0)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MoveClipCmd {
    pub clip_id: ClipId,
    pub old_track: TrackId,
    pub new_track: TrackId,
    pub old_pos: TimelinePos,
    pub new_pos: TimelinePos,
}

impl Command for MoveClipCmd {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()> {
        if let Some(track) = ctx.project.get_track_mut(self.old_track) {
            track.remove_clip(self.clip_id);
        }
        if let Some(track) = ctx.project.get_track_mut(self.new_track) {
            track.add_clip(self.clip_id);
        }
        // Clip position update would be handled by the engine
        Ok(())
    }

    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()> {
        if let Some(track) = ctx.project.get_track_mut(self.new_track) {
            track.remove_clip(self.clip_id);
        }
        if let Some(track) = ctx.project.get_track_mut(self.old_track) {
            track.add_clip(self.clip_id);
        }
        Ok(())
    }

    fn description(&self) -> String {
        format!("Move clip {:?}", self.clip_id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SetParameterCmd {
    pub target: ParamTarget,
    pub old_value: f32,
    pub new_value: f32,
}

impl Command for SetParameterCmd {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.engine_tx.send(crate::commands::EngineMsg::Parameter(
            crate::commands::ParameterCmd::Set {
                target: self.target.clone(),
                value: self.new_value,
            }
        ))?;
        Ok(())
    }

    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.engine_tx.send(crate::commands::EngineMsg::Parameter(
            crate::commands::ParameterCmd::Set {
                target: self.target.clone(),
                value: self.old_value,
            }
        ))?;
        Ok(())
    }

    fn description(&self) -> String {
        format!("Set parameter: {:?}", self.target)
    }

    fn merge(&mut self, other: &dyn Command) -> Option<Box<dyn Command>> {
        if let Some(other) = (other as &dyn Any).downcast_ref::<SetParameterCmd>() {
            if self.target == other.target {
                self.new_value = other.new_value;
                return Some(Box::new(self.clone()));
            }
        }
        None
    }
}

pub struct MacroCommand {
    pub commands: Vec<Box<dyn Command>>,
}

impl Command for MacroCommand {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()> {
        for cmd in &mut self.commands {
            cmd.execute(ctx)?;
        }
        Ok(())
    }

    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()> {
        for cmd in self.commands.iter_mut().rev() {
            cmd.undo(ctx)?;
        }
        Ok(())
    }

    fn description(&self) -> String {
        if self.commands.len() == 1 {
            self.commands[0].description()
        } else {
            format!("Macro ({} commands)", self.commands.len())
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SetTempoCmd {
    pub old_bpm: f32,
    pub new_bpm: f32,
    pub at_tick: Option<u64>,
}

impl Command for SetTempoCmd {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.engine_tx.send(crate::commands::EngineMsg::Transport(
            crate::commands::TransportCmd::SetTempo { bpm: self.new_bpm, at_tick: self.at_tick }
        ))?;
        Ok(())
    }

    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()> {
        ctx.engine_tx.send(crate::commands::EngineMsg::Transport(
            crate::commands::TransportCmd::SetTempo { bpm: self.old_bpm, at_tick: self.at_tick }
        ))?;
        Ok(())
    }

    fn description(&self) -> String {
        format!("Set tempo: {:.1} BPM", self.new_bpm)
    }
}