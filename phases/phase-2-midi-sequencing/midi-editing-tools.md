# Feature: MIDI Editing Tools

## Phase: 2 — MIDI & Sequencing
## ID: 2.3
## Priority: High
## Estimated: 2 weeks
## Status: `[~]` In Progress
## Depends On: 2.2

## Description
MIDI transformation tools: quantize, humanize, transpose, scale conform, legato, velocity curves, note length, randomize.

## Requirements

- [x] Quantize: Notes to grid with strength (0-100%) and tuplet grids; swing pending
- [x] Humanize: Random timing/velocity offset
- [x] Transpose: Semitones and octave buttons; scale-aware transpose pending
- [ ] Scale Conform: Force notes to scale (major, minor, modes, custom)
- [x] Legato: Extend notes to next note start (with gap option)
- [~] Velocity Tools: Scale implemented; compress, expand, curve, randomize pending
- [x] Note Length: Set fixed length; scale/legato/staccato variants pending
- [ ] Randomize: Pitch, velocity, timing, note density
- [ ] Split Notes: At pitch, at velocity, at channel
- [ ] Join Notes: Adjacent same-pitch notes
- [ ] Mirror/Reverse: Time reverse, pitch mirror
- [x] Batch apply: To the current MIDI clip selection

## Technical Details

### Tool Interface
```rust
trait MidiEditTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Vec<ToolParam>;
    fn apply(&self, events: &mut [MidiEvent], params: &ToolParams, ctx: &EditContext) -> Result<Vec<MidiEditChange>, ToolError>;
    fn preview(&self, events: &[MidiEvent], params: &ToolParams) -> Vec<MidiEvent>; // For real-time preview
}

struct EditContext {
    clip: &MidiClip,
    project: &Project,
    tempo_map: &TempoMap,
    ppqn: u32,
}

struct ToolParams {
    values: HashMap<String, ParamValue>,
}

enum ParamValue {
    Float(f32),
    Int(i64),
    Bool(bool),
    Enum(String),
    Range(Range<f32>),
}
```

### Quantize Implementation
```rust
fn quantize(events: &mut [MidiEvent], params: &ToolParams, ctx: &EditContext) {
    let grid = params.get_enum("grid").unwrap();      // "1/4", "1/8", "1/16", "1/32"
    let strength = params.get_float("strength").unwrap_or(1.0);
    let swing = params.get_float("swing").unwrap_or(0.0);
    let tuplet = params.get_enum("tuplet").unwrap_or("none");
    
    let grid_ticks = grid_to_ticks(grid, tuplet, ctx.ppqn);
    
    for event in events {
        if let MidiMessage::NoteOn { .. } = event.message {
            let target = snap_to_grid(event.tick, grid_ticks, swing);
            let delta = (target as f32 - event.tick as f32) * strength;
            event.tick = (event.tick as f32 + delta).round() as u64;
        }
    }
}
```

### Scale Conform
```rust
fn conform_to_scale(events: &mut [MidiEvent], params: &ToolParams) {
    let scale = params.get_scale("scale").unwrap();  // Scale object with intervals
    let root = params.get_int("root").unwrap_or(60) as u8; // Middle C = 60
    
    for event in events {
        if let MidiMessage::NoteOn { note, .. } | MidiMessage::NoteOff { note, .. } = &mut event.message {
            *note = scale.nearest_note(*note, root);
        }
    }
}
```

### Velocity Curve
```rust
fn velocity_curve(events: &mut [MidiEvent], params: &ToolParams) {
    let curve_type = params.get_enum("curve").unwrap(); // "linear", "exp", "log", "s", "custom"
    let curve = match curve_type {
        "linear" => |v| v,
        "exp" => |v| (v as f32 / 127.0).powf(2.0) * 127.0,
        "log" => |v| (v as f32 / 127.0).sqrt() * 127.0,
        "s" => |v| { let x = v as f32 / 127.0; 3.0*x*x - 2.0*x*x*x },
        _ => custom_curve(params),
    };
    
    for event in events {
        if let MidiMessage::NoteOn { velocity, .. } = &mut event.message {
            *velocity = curve(*velocity) as u8;
        }
    }
}
```

### UI: Tool Panel
- Sidebar in piano roll: Tool list with parameters
- Real-time preview: Checkbox "Preview" shows result before apply
- Presets: Save/load tool parameter sets
- History: Each tool application = one undo step

## Acceptance Criteria

- [~] Quantize with strength/swing works musically
- [x] Humanize adds natural variation
- [x] Transpose preserves chord quality
- [ ] Scale conform maps notes correctly
- [x] Legato creates smooth transitions
- [~] Velocity curve shapes dynamics
- [ ] All tools support preview
- [x] Batch apply to selection/clip/track/project
- [x] Each tool = single undo step

## Progress Log

- 2026-10-24: MIDI Tools panel inside the piano roll. The left side of the piano-roll window now provides quantize (Bar/Beat/1/8/1/16/1/32/1/8T/1/16T, 0-100% strength), transpose (±24 semitones and octave buttons) and velocity scaling (0-200%). Tools apply to the clip open in the piano roll as one undo step; trimmed notes are included
- 2026-10-26: Added humanize (random timing and velocity), legato (extend toward the next note with an adjustable gap) and fixed note length to the piano-roll MIDI Tools panel. Each tool applies to the open clip as one undo step
- YYYY-MM-DD: Scale conform
- YYYY-MM-DD: Legato
- YYYY-MM-DD: Velocity tools
- YYYY-MM-DD: Note length
- YYYY-MM-DD: Randomize
- YYYY-MM-DD: Split/Join/Mirror
- YYYY-MM-DD: Tool panel UI + presets