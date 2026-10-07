use crate::audio_pool::AudioFileRef;
use crate::project::Color;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::HashMap;
use std::ops::Range;
use uuid::Uuid;

slotmap::new_key_type! {
    pub struct ClipId;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioClip {
    pub id: ClipId,
    pub source: AudioFileRef,
    pub source_offset: u64,
    pub timeline_position: TimelinePos,
    pub length: u64,
    pub gain: f32,
    pub fade_in: Option<Fade>,
    pub fade_out: Option<Fade>,
    pub pitch_shift: f32,
    pub time_stretch: TimeStretchMode,
    pub loop_enabled: bool,
    pub loop_range: Option<Range<u64>>,
    pub color: Option<Color>,
    pub muted: bool,
    pub locked: bool,
}

impl AudioClip {
    pub fn new(source: AudioFileRef, timeline_position: TimelinePos, length: u64) -> Self {
        Self {
            id: ClipId::default(),
            source,
            source_offset: 0,
            timeline_position,
            length,
            gain: 1.0,
            fade_in: None,
            fade_out: None,
            pitch_shift: 0.0,
            time_stretch: TimeStretchMode::Elastic,
            loop_enabled: false,
            loop_range: None,
            color: None,
            muted: false,
            locked: false,
        }
    }

    pub fn end_position(&self) -> TimelinePos {
        let mut pos = self.timeline_position;
        pos.tick += self.length as u64; // Simplified - should use proper conversion
        pos
    }

    pub fn set_position(&mut self, position: TimelinePos) {
        self.timeline_position = position;
    }

    pub fn set_length(&mut self, length: u64) {
        self.length = length;
    }

    pub fn apply_gain(&mut self, gain_db: f32) {
        self.gain *= Self::db_to_gain(gain_db);
    }

    pub fn db_to_gain(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiClip {
    pub id: ClipId,
    pub events: Vec<MidiEvent>,
    pub timeline_position: TimelinePos,
    pub length: u64,
    pub loop_enabled: bool,
    pub loop_range: Option<Range<u64>>,
    pub color: Option<Color>,
    pub muted: bool,
}

impl MidiClip {
    pub fn new(timeline_position: TimelinePos, length: u64) -> Self {
        Self {
            id: ClipId::default(),
            events: Vec::new(),
            timeline_position,
            length,
            loop_enabled: false,
            loop_range: None,
            color: None,
            muted: false,
        }
    }

    pub fn add_event(&mut self, event: MidiEvent) {
        self.events.push(event);
        self.events.sort_by_key(|e| e.tick);
    }

    pub fn events_in_range(&self, start_tick: u64, end_tick: u64) -> Vec<&MidiEvent> {
        self.events
            .iter()
            .filter(|e| e.tick >= start_tick && e.tick < end_tick)
            .collect()
    }

    pub fn notes_in_range(&self, start_tick: u64, end_tick: u64, pitch_range: Option<Range<u8>>) -> Vec<&MidiEvent> {
        self.events
            .iter()
            .filter(|e| {
                e.tick >= start_tick
                    && e.tick < end_tick
                    && matches!(e.message, MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. })
                    && pitch_range.as_ref().map_or(true, |r| {
                        if let MidiMessage::NoteOn { note, .. } | MidiMessage::NoteOff { note, .. } = e.message {
                            r.contains(&note)
                        } else {
                            false
                        }
                    })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TimelinePos {
    pub tick: u64,
    pub sample: u64,
}

impl TimelinePos {
    pub fn new(tick: u64, sample: u64) -> Self {
        Self { tick, sample }
    }

    pub fn from_tick(tick: u64, sample_rate: f64, tempo: f32) -> Self {
        let sample = (tick as f64 / 960.0) * (60.0 / tempo as f64) * sample_rate;
        Self { tick, sample: sample as u64 }
    }

    pub fn from_sample(sample: u64, sample_rate: f64, tempo: f32) -> Self {
        let tick = (sample as f64 / sample_rate) * (tempo as f64 / 60.0) * 960.0;
        Self { tick: tick as u64, sample }
    }
}

impl Default for TimelinePos {
    fn default() -> Self {
        Self { tick: 0, sample: 0 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fade {
    pub length: u64,
    pub curve: FadeCurve,
}

impl Fade {
    pub fn new(length: u64, curve: FadeCurve) -> Self {
        Self { length, curve }
    }

    pub fn gain_at(&self, position: u64) -> f32 {
        let t = (position as f32 / self.length as f32).clamp(0.0, 1.0);
        self.curve.apply(t)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FadeCurve {
    Linear,
    Exponential,
    Logarithmic,
    SCurve,
    EqualPower,
}

impl FadeCurve {
    pub fn apply(&self, t: f32) -> f32 {
        match self {
            FadeCurve::Linear => t,
            FadeCurve::Exponential => t * t,
            FadeCurve::Logarithmic => t.sqrt(),
            FadeCurve::SCurve => 3.0 * t * t - 2.0 * t * t * t,
            FadeCurve::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TimeStretchMode {
    Tape,
    Elastic,
    Musical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiEvent {
    pub tick: u64,
    pub message: MidiMessage,
}

impl MidiEvent {
    pub fn note_on(tick: u64, channel: u8, note: u8, velocity: u8) -> Self {
        Self {
            tick,
            message: MidiMessage::NoteOn { channel, note, velocity },
        }
    }

    pub fn note_off(tick: u64, channel: u8, note: u8, velocity: u8) -> Self {
        Self {
            tick,
            message: MidiMessage::NoteOff { channel, note, velocity },
        }
    }

    pub fn control_change(tick: u64, channel: u8, controller: u8, value: u8) -> Self {
        Self {
            tick,
            message: MidiMessage::ControlChange { channel, controller, value },
        }
    }

    pub fn pitch_bend(tick: u64, channel: u8, value: u16) -> Self {
        Self {
            tick,
            message: MidiMessage::PitchBend { channel, value },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MidiMessage {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8, velocity: u8 },
    PolyPressure { channel: u8, note: u8, pressure: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
    ProgramChange { channel: u8, program: u8 },
    ChannelPressure { channel: u8, pressure: u8 },
    PitchBend { channel: u8, value: u16 },
    SysEx(Vec<u8>),
    Meta(MetaMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MetaMessage {
    Tempo(f32),
    TimeSignature { numerator: u8, denominator: u8 },
    Marker(String),
    EndOfTrack,
}