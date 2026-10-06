# Feature: Piano Roll Editor

## Phase: 2 — MIDI & Sequencing
## ID: 2.2
## Priority: Critical
## Estimated: 4 weeks
## Status: `[ ]` Not Started
## Depends On: 2.1

## Description
Full-featured piano roll: note entry/selection/editing, velocity lanes, CC lanes, multiple tools, zoom/scroll, keyboard shortcuts.

## Requirements

- [ ] Note grid: Pitch (vertical) × Time (horizontal), configurable snap
- [ ] Note entry: Click to add, drag length, right-click delete
- [ ] Selection: Rectangle, click note, Ctrl+click multi-select, Shift+click range
- [ ] Note editing: Drag move (pitch/time), drag resize, velocity drag
- [ ] Velocity lane: Bar graph per note, drag to edit, curve tool
- [ ] CC lanes: Multiple lanes (Mod wheel, Expression, Pitch Bend, custom)
- [ ] Tool palette: Select, Pencil, Eraser, Mute, Scissors, Glue, Velocity, Line
- [ ] Zoom: Horizontal (time), Vertical (pitch), Fit selection, Fit all
- [ ] Scroll: Horizontal (time), Vertical (pitch), Middle-mouse pan
- [ ] Ghost notes: Show notes from other clips on same track (dimmed)
- [ ] Scale highlighting: Highlight notes in current scale
- [ ] Drum mode: Note names instead of piano keys, one-row-per-drum

## Technical Details

### Piano Roll State
```rust
struct PianoRollState {
    clip_id: ClipId,
    scroll: (f32, f32),             // (time_px, pitch_px)
    zoom: (f32, f32),               // (px_per_tick, px_per_semitone)
    snap_grid: SnapGrid,
    active_tool: Tool,
    selection: Selection,
    velocity_lane_height: f32,
    cc_lanes: Vec<CCLaneState>,
    ghost_clips: Vec<ClipId>,
    scale: Option<Scale>,
    drum_map: Option<DrumMap>,
}

enum Tool {
    Select,
    Pencil,        // Click to add note at grid
    Eraser,        // Click to delete
    Mute,          // Click to toggle mute
    Scissors,      // Click to split note
    Glue,          // Drag to join adjacent notes
    Velocity,      // Drag to set velocity
    Line,          // Draw velocity/CC line
    Curve,         // Draw velocity/CC curve
}

struct Selection {
    notes: HashSet<NoteId>,
    time_range: Option<Range<u64>>,
    pitch_range: Option<Range<u8>>,
}
```

### Note Rendering (egui)
```rust
fn paint_piano_roll(ui: &mut Ui, state: &mut PianoRollState, clip: &MidiClip) {
    let grid = PianoGrid {
        time_range: visible_ticks(state.scroll.0, state.zoom.0),
        pitch_range: visible_pitches(state.scroll.1, state.zoom.1),
        snap: state.snap_grid,
    };
    
    // Background: piano key stripes
    for pitch in grid.pitch_range {
        let color = if is_black_key(pitch) { BLACK_KEY_BG } else { WHITE_KEY_BG };
        painter.rect(rect_for_pitch(pitch), color);
    }
    
    // Grid lines
    for tick in grid.time_grid {
        painter.line(vertical_line_at(tick), GRID_STROKE);
    }
    for pitch in grid.pitch_grid {
        painter.line(horizontal_line_at(pitch), GRID_STROKE);
    }
    
    // Notes
    for note in clip.notes_in_range(grid.time_range, grid.pitch_range) {
        let rect = note_rect(note, &grid);
        let color = if state.selection.notes.contains(&note.id) {
            SELECTED_NOTE_COLOR
        } else if note.muted { MUTED_NOTE_COLOR } else { NOTE_COLOR };
        painter.rect(rect, color);
        
        // Velocity indicator (thin line at top)
        painter.rect(top_edge(rect, velocity_height(note.velocity)), VELOCITY_COLOR);
    }
    
    // Velocity lane (below piano roll)
    paint_velocity_lane(ui, clip, state);
    
    // CC lanes (below velocity)
    for cc_lane in &state.cc_lanes {
        paint_cc_lane(ui, clip, cc_lane, state);
    }
}
```

### CC Lane State
```rust
struct CCLaneState {
    controller: u8,                 // CC number (1=mod, 11=expression, etc.)
    channel: Option<u8>,            // None = all channels
    height: f32,
    visible: bool,
    edit_mode: CCEditMode,          // Points, Line, Curve
}

enum CCEditMode {
    Points,     // Discrete values at ticks
    Line,       // Linear interpolation
    Curve,      // Bezier interpolation
}
```

### Keyboard Shortcuts
| Key | Action |
|-----|--------|
| 1 | Select tool |
| 2 | Pencil |
| 3 | Eraser |
| 4 | Mute |
| 5 | Scissors |
| 6 | Glue |
| 7 | Velocity |
| 8 | Line |
| 9 | Curve |
| [ / ] | Zoom horizontal |
| Shift+[ / ] | Zoom vertical |
| F | Fit selection |
| G | Toggle ghost notes |
| S | Toggle scale highlighting |
| D | Toggle drum mode |

## Acceptance Criteria

- [ ] Notes render correctly at all zoom levels
- [ ] Pencil tool adds notes at snap grid
- [ ] Selection rectangle selects multiple notes
- [ ] Drag moves notes (constrain: Shift=pitch only, Ctrl=time only)
- [ ] Velocity lane edits work
- [ ] CC lanes: add/edit/delete points
- [ ] Ghost notes show other clips
- [ ] Scale highlighting works (major/minor/modes)
- [ ] Drum mode shows note names
- [ ] 60 FPS with 1000+ notes visible

## Progress Log

- YYYY-MM-DD: Basic grid + note rendering
- YYYY-MM-DD: Pencil/Eraser/Select tools
- YYYY-MM-DD: Note drag + resize
- YYYY-MM-DD: Velocity lane
- YYYY-MM-DD: CC lanes
- YYYY-MM-DD: Ghost notes + scale highlight
- YYYY-MM-DD: Drum mode
- YYYY-MM-DD: Keyboard shortcuts + tool palette