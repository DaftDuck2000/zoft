# Feature: Automation System

## Phase: 4 — Mixing & Automation
## ID: 4.3
## Priority: Critical
## Estimated: 3 weeks
## Status: `[ ]` Not Started
## Depends On: 4.2

## Description
Comprehensive automation: Read/Write/Touch/Latch/Trim modes, curve editing (line, curve, S, step), thinning, per-parameter lanes, automation items, preview.

## Requirements

- [ ] Automation modes: Read, Write, Touch, Latch, Trim, Off
- [ ] Automation lanes: Per-parameter, show/hide, color, height
- [ ] Curve types: Linear, Exponential, Logarithmic, S-Curve, Step, Freehand
- [ ] Editing: Add/move/delete points, box select, nudge, scale, flip
- [ ] Thinning: Douglas-Peucker algorithm (configurable tolerance)
- [ ] Automation items: Clips of automation (copy/paste, loop, stretch)
- [ ] Preview mode: Hear automation without committing
- [ ] Trim mode: Offset existing automation relatively
- [ ] Touch/Latch timeout: Configurable release time
- [ ] Automation browser: List all automated params, jump to
- [ ] MIDI learn: Right-click param → move controller → learn

## Technical Details

### Automation Model
```rust
struct AutomationSystem {
    lanes: HashMap<ParamId, AutomationLane>,
    global_mode: AutomationMode,
    preview_buffer: Option<AutomationPreview>,
    thinning_tolerance: f32,
}

struct AutomationLane {
    param_id: ParamId,
    param_name: String,
    events: Vec<AutomationEvent>,   // Sorted by tick
    visible: bool,
    color: Color,
    height: f32,
    mode: AutomationMode,           // Override global
}

struct AutomationEvent {
    tick: u64,                      // Musical tick
    value: f32,                     // Normalized 0..1
    shape: CurveShape,              // To next event
    tension: f32,                   // For S-curve
}

enum CurveShape {
    Linear,
    Exponential,
    Logarithmic,
    SCurve { tension: f32 },
    Step,                           // Hold until next
    Freehand,                       // Dense points, no interpolation
}

enum AutomationMode {
    Off,        // Ignore automation
    Read,       // Play back automation
    Write,      // Write on any transport movement
    Touch,      // Write only while touching control
    Latch,      // Write on touch, continue after release
    Trim,       // Offset existing automation
    Preview,    // Play preview buffer
}
```

### Real-Time Automation Processing (Audio Thread)
```rust
struct AutomationProcessor {
    lanes: HashMap<ParamId, AutomationLane>,
    current_tick: u64,
    tick_increment: f64,          // Ticks per sample
    smoothing: HashMap<ParamId, ParameterSmoother>,
}

impl AutomationProcessor {
    fn process_block(&mut self, block_size: usize, params: &mut HashMap<ParamId, f32>) {
        for (param_id, lane) in &self.lanes {
            if lane.mode != AutomationMode::Read { continue; }
            
            let target_value = self.interpolate_lane(lane, self.current_tick, block_size);
            let smoothed = self.smoothing.entry(*param_id).or_default().process(target_value);
            params.insert(*param_id, smoothed);
        }
        self.current_tick += (block_size as f64 * self.tick_increment) as u64;
    }
    
    fn interpolate_lane(&self, lane: &AutomationLane, start_tick: u64, len: usize) -> f32 {
        // Find enclosing events, interpolate per sample
        // Linear/Exp/Log/S-curve/Step
    }
}
```

### Thinning (Douglas-Peucker)
```rust
fn thin_automation(events: &mut Vec<AutomationEvent>, tolerance: f32) {
    if events.len() < 3 { return; }
    
    let mut keep = vec![false; events.len()];
    keep[0] = true;
    keep[events.len() - 1] = true;
    
    fn douglas_peucker(events: &[AutomationEvent], keep: &mut [bool], start: usize, end: usize, tolerance: f32) {
        if end <= start + 1 { return; }
        
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
    
    douglas_peucker(events, &mut keep, 0, events.len() - 1, tolerance);
    events.retain(|_, i| keep[i]);
}
```

### Automation Items (Clips)
```rust
struct AutomationItem {
    id: ItemId,
    lane_id: ParamId,
    tick_range: Range<u64>,
    events: Vec<AutomationEvent>,  // Relative to item start
    loop_enabled: bool,
    loop_range: Option<Range<u64>>,
    color: Color,
}
```

### MIDI Learn
```rust
struct MidiLearn {
    pending: Option<ParamId>,
    mappings: HashMap<ParamId, MidiLearnMapping>,
}

struct MidiLearnMapping {
    channel: u8,
    controller: u8,           // CC number
    min: f32,                 // Param value at CC 0
    max: f32,                 // Param value at CC 127
    curve: CurveShape,
}
```

## Acceptance Criteria

- [ ] All 6 modes work: Off/Read/Write/Touch/Latch/Trim
- [ ] Lanes: create, show/hide, resize, color
- [ ] Curve shapes: all 6 types interpolate correctly
- [ ] Editing: point add/move/delete, box select, nudge
- [ ] Thinning reduces points > 90% with tolerance 0.001
- [ ] Automation items: copy/paste/loop/stretch
- [ ] Preview: hear without committing, commit/discard
- [ ] Trim: offsets automation relatively
- [ ] MIDI learn: CC → param, min/max/curve
- [ ] Sample-accurate: no zipper noise

## Progress Log

- YYYY-MM-DD: Automation lanes + modes
- YYYY-MM-DD: Curve interpolation
- YYYY-MM-DD: Lane editing UI
- YYYY-MM-DD: Thinning algorithm
- YYYY-MM-DD: Automation items
- YYYY-MM-DD: Preview + Trim modes
- YYYY-MM-DD: MIDI learn
- YYYY-MM-DD: Automation browser