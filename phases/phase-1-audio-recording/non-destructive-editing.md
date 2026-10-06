# Feature: Non-Destructive Clip Editing

## Phase: 1 — Audio Recording & Editing
## ID: 1.3
## Priority: Critical
## Estimated: 3 weeks
## Status: `[ ]` Not Started
## Depends On: 1.2

## Description
Implement non-destructive audio clip editing: split, trim, move, copy, delete, fade in/out, crossfade. All operations create commands for undo/redo.

## Requirements

- [ ] Split clip at playhead / mouse position
- [ ] Trim clip edges (drag clip boundaries)
- [ ] Move clips: drag between tracks, snap to grid
- [ ] Copy/paste/duplicate clips
- [ ] Delete clips (move to trash, not permanent)
- [ ] Fade in/out: drag fade handles, curve types (linear, exp, log, S)
- [ ] Crossfade: Overlap two clips → automatic crossfade
- [ ] Snap modes: Grid, Relative, Off
- [ ] Clip gain: Drag gain handle, numeric input
- [ ] Clip coloring: Per-clip color override

## Technical Details

### Clip Model (Non-Destructive)
```rust
struct AudioClip {
    id: ClipId,
    source: AudioFileRef,           // Immutable reference
    source_offset: u64,             // Start sample in source file
    timeline_position: TimelinePos, // Position on track
    length: u64,                    // Visible length in samples
    gain: f32,                      // Clip gain (linear, 0.0..10.0)
    fade_in: Option<Fade>,
    fade_out: Option<Fade>,
    loop_enabled: bool,
    loop_range: Option<Range<u64>>,
    color: Option<Color>,           // Override track color
}

struct Fade {
    length: u64,                    // Samples
    curve: FadeCurve,
}

enum FadeCurve {
    Linear,
    Exponential,
    Logarithmic,
    SCurve,
    EqualPower,                     // For crossfades
}
```

### Editing Commands (for Undo/Redo)
```rust
enum ClipEditCmd {
    Split { clip_id: ClipId, position: TimelinePos },
    TrimStart { clip_id: ClipId, new_offset: u64 },
    TrimEnd { clip_id: ClipId, new_length: u64 },
    Move { clip_id: ClipId, new_track: TrackId, new_pos: TimelinePos },
    Copy { clip_ids: Vec<ClipId>, target_track: TrackId, target_pos: TimelinePos },
    Delete { clip_ids: Vec<ClipId> },
    SetGain { clip_id: ClipId, gain: f32 },
    SetFadeIn { clip_id: ClipId, fade: Option<Fade> },
    SetFadeOut { clip_id: ClipId, fade: Option<Fade> },
    SetLoop { clip_id: ClipId, enabled: bool, range: Option<Range<u64>> },
    SetColor { clip_id: ClipId, color: Option<Color> },
}
```

### Snap Logic
```rust
fn snap_position(pos: TimelinePos, grid: SnapGrid, relative: bool) -> TimelinePos {
    match grid {
        SnapGrid::Off => pos,
        SnapGrid::Grid(div) => quantize_to_grid(pos, div),
        SnapGrid::Relative => pos + (grid_offset - pos) % grid_value,
    }
}
```

### Crossfade Implementation
- When two clips overlap on same track:
  - Create crossfade region = overlap duration
  - Apply equal-power fade out on first, fade in on second
  - Render as single continuous output in engine
  - Editable: drag crossfade boundary, change curve

## Acceptance Criteria

- [ ] Split creates two clips referencing same source
- [ ] Trim adjusts source_offset/length, not source file
- [ ] Move between tracks preserves timing
- [ ] Snap works at all zoom levels
- [ ] Fades visually editable with handles
- [ ] Crossfade created automatically on overlap
- [ ] All operations generate undo commands
- [ ] Original audio file never modified

## Progress Log

- YYYY-MM-DD: Split command implemented
- YYYY-MM-DD: Trim (drag edges)
- YYYY-MM-DD: Move + snap
- YYYY-MM-DD: Copy/paste/duplicate
- YYYY-MM-DD: Fade in/out handles
- YYYY-MM-DD: Crossfade on overlap
- YYYY-MM-DD: Clip gain + color
- YYYY-MM-DD: All commands undoable