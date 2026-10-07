use crate::project::Color;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use slotmap::SlotMap;
use std::collections::HashMap;
use uuid::Uuid;

slotmap::new_key_type! {
    pub struct AutomationLaneId;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationLane {
    pub id: AutomationLaneId,
    pub parameter_id: ParameterId,
    pub name: String,
    pub events: Vec<AutomationEvent>,
    pub visible: bool,
    pub color: Color,
    pub height: f32,
    pub mode: AutomationMode,
}

impl AutomationLane {
    pub fn new(parameter_id: ParameterId, name: impl Into<String>) -> Self {
        Self {
            id: AutomationLaneId::default(),
            parameter_id,
            name: name.into(),
            events: Vec::new(),
            visible: true,
            color: Color::default(),
            height: 60.0,
            mode: AutomationMode::Read,
        }
    }

    pub fn add_event(&mut self, event: AutomationEvent) {
        let idx = self.events.partition_point(|e| e.tick < event.tick);
        self.events.insert(idx, event);
    }

    pub fn remove_event(&mut self, index: usize) -> bool {
        if index < self.events.len() {
            self.events.remove(index);
            true
        } else {
            false
        }
    }

    pub fn value_at_tick(&self, tick: u64) -> f32 {
        if self.events.is_empty() {
            return 0.0;
        }

        if tick <= self.events[0].tick {
            return self.events[0].value;
        }

        for i in 1..self.events.len() {
            if tick <= self.events[i].tick {
                let prev = &self.events[i - 1];
                let next = &self.events[i];
                return Self::interpolate(prev, next, tick);
            }
        }

        self.events.last().unwrap().value
    }

    fn interpolate(prev: &AutomationEvent, next: &AutomationEvent, tick: u64) -> f32 {
        let t = (tick - prev.tick) as f32 / (next.tick - prev.tick) as f32;
        let t = t.clamp(0.0, 1.0);
        
        match prev.shape {
            CurveShape::Linear => prev.value + (next.value - prev.value) * t,
            CurveShape::Exponential => {
                let a = prev.value.max(0.001);
                let b = next.value.max(0.001);
                a * (b / a).powf(t)
            }
            CurveShape::Logarithmic => {
                let a = prev.value.max(0.001);
                let b = next.value.max(0.001);
                a * (b / a).powf(1.0 - t)
            }
            CurveShape::SCurve => {
                let eased = 3.0 * t * t - 2.0 * t * t * t;
                prev.value + (next.value - prev.value) * eased
            }
            CurveShape::Step => prev.value,
            CurveShape::Freehand => prev.value + (next.value - prev.value) * t,
        }
    }

    pub fn thin(&mut self, tolerance: f32) {
        if self.events.len() < 3 {
            return;
        }

        let mut keep = vec![false; self.events.len()];
        keep[0] = true;
        keep[self.events.len() - 1] = true;

        fn douglas_peucker(events: &[AutomationEvent], keep: &mut [bool], start: usize, end: usize, tolerance: f32) {
            if end <= start + 1 {
                return;
            }

            let mut max_dist = 0.0;
            let mut max_idx = start;

            for i in (start + 1)..end {
                let dist = perpendicular_distance(&events[i], &events[start], &events[end]);
                if dist > max_dist {
                    max_dist = dist;
                    max_idx = i;
                }
            }

            if max_dist > tolerance {
                keep[max_idx] = true;
                douglas_peucker(events, keep, start, max_idx, tolerance);
                douglas_peucker(events, keep, max_idx, end, tolerance);
            }
        }

        fn perpendicular_distance(p: &AutomationEvent, a: &AutomationEvent, b: &AutomationEvent) -> f32 {
            if a.tick == b.tick {
                return (p.value - a.value).abs();
            }
            let t = (p.tick - a.tick) as f32 / (b.tick - a.tick) as f32;
            let interpolated = a.value + (b.value - a.value) * t;
            (p.value - interpolated).abs()
        }

        douglas_peucker(&self.events, &mut keep, 0, self.events.len() - 1, tolerance);
        let mut new_events = Vec::new();
        for (i, event) in self.events.drain(..).enumerate() {
            if keep[i] {
                new_events.push(event);
            }
        }
        self.events = new_events;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationEvent {
    pub tick: u64,
    pub value: f32,
    pub shape: CurveShape,
    pub tension: f32,
}

impl AutomationEvent {
    pub fn new(tick: u64, value: f32) -> Self {
        Self {
            tick,
            value,
            shape: CurveShape::Linear,
            tension: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CurveShape {
    Linear,
    Exponential,
    Logarithmic,
    SCurve,
    Step,
    Freehand,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AutomationMode {
    Off,
    Read,
    Write,
    Touch,
    Latch,
    Trim,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParameterId(pub Uuid);

impl ParameterId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ParameterId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterInfo {
    pub id: ParameterId,
    pub name: String,
    pub label: String,
    pub min_value: f32,
    pub max_value: f32,
    pub default_value: f32,
    pub step_count: u32,
    pub flags: ParameterFlags,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParameterFlags {
    pub automatable: bool,
    pub hidden: bool,
    pub read_only: bool,
    pub sample_accurate: bool,
}

impl Default for ParameterFlags {
    fn default() -> Self {
        Self {
            automatable: true,
            hidden: false,
            read_only: false,
            sample_accurate: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSmoother {
    pub current: f32,
    pub target: f32,
    pub ramp_frames: u32,
    pub frame: u32,
    pub law: RampLaw,
}

impl ParameterSmoother {
    pub fn new() -> Self {
        Self {
            current: 0.0,
            target: 0.0,
            ramp_frames: 0,
            frame: 0,
            law: RampLaw::Linear,
        }
    }

    pub fn set_target(&mut self, target: f32, ramp_frames: u32, law: RampLaw) {
        self.target = target;
        self.ramp_frames = ramp_frames;
        self.frame = 0;
        self.law = law;
    }

    pub fn process(&mut self) -> f32 {
        if self.frame < self.ramp_frames {
            let t = self.frame as f32 / self.ramp_frames as f32;
            let eased = match self.law {
                RampLaw::Linear => t,
                RampLaw::Exponential => 1.0 - (-10.0 * t).exp(),
            };
            self.current = self.current + (self.target - self.current) * eased;
            self.frame += 1;
        } else {
            self.current = self.target;
        }
        self.current
    }

    pub fn set_immediate(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.frame = 0;
        self.ramp_frames = 0;
    }
}

impl Default for ParameterSmoother {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RampLaw {
    Linear,
    Exponential,
}