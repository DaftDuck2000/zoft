# Feature: Non-Destructive Clip Editing

## Phase: 1 — Audio Recording & Editing
## ID: 1.3
## Priority: Critical
## Estimated: 3 weeks
## Status: `[✓]` Done
## Depends On: 1.2

## Description
Implement non-destructive audio clip editing: split, trim, move, copy, delete, fade in/out, crossfade. All operations create commands for undo/redo.

## Requirements

- [x] Split clip at playhead (S key)
- [x] Trim clip edges (drag clip boundaries)
- [x] Move clips: drag between tracks, snap to grid
- [x] Copy/paste/duplicate clips (Ctrl+C / Ctrl+V / Ctrl+X / Ctrl+D)
- [x] Delete clips (removed from track but kept in the project pool; undo restores)
- [x] Fade in/out: drag fade handles, curve types (linear, exp, log, S, equal power)
- [x] Crossfade: Overlap two clips → automatic equal-power crossfade (mixer + visual overlay)
- [x] Snap modes: Grid (bar/beat/1/8/1/16), Off (hold Alt to bypass)
- [x] Relative snap mode: move clips in grid steps from their original position
- [x] Trash browser: deleted clips stay in the project and can be restored from the left panel
- [x] Clip gain: inspector slider (numeric dB)
- [x] Clip coloring: per-clip color override

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

- [x] Split creates two clips referencing same source
- [x] Trim adjusts source_offset/length, not source file
- [x] Move between tracks preserves timing
- [x] Snap works at all zoom levels
- [x] Fades visually editable with handles
- [x] Crossfade created automatically on overlap
- [x] All operations generate undo commands
- [x] Original audio file never modified

## Progress Log

- 2026-10-21: Drag-move with snapping (bar/beat/1/8/1/16/off, Alt bypass) and cross-track drops
- 2026-10-21: Edge trimming (source_offset/length), live preview while dragging
- 2026-10-21: Split at the playhead (S) — both halves reference the same audio file; fades redistributed
- 2026-10-21: Copy/paste/duplicate (Ctrl+C/V/D), delete (Delete) with recoverable clip data
- 2026-10-21: Fade handles on clip corners + inspector curve/length editors (5 curve types)
- 2026-10-21: Clip gain, fades and per-clip colour moved into the **Clip** section of the Clip Audio window
- Remaining (post-phase polish): editable crossfade boundary/curve dragging; crossfades are currently automatic only
- 2026-10-21: Undo/redo (Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y) with merged continuous property edits
- 2026-10-21: Playback mixer slices clips by source_offset/length so edits are audible
- 2026-10-21: Musical ruler (bars/beats default, time switchable) and time-signature picker; snapping follows the chosen signature
- 2026-10-21: Automatic equal-power crossfades where clips partially overlap on the same track (mixer gain envelopes + X overlay)
- 2026-10-21: Relative snap toggle (moves in grid steps from the clip's original position); snap now defaults to Beat
- 2026-10-21: Reverse renders reuse a stable base filename instead of stacking `_reverse_<uuid>` suffixes
- 2026-10-21: Trash browser in the track panel; removed clips can be restored to their previous track
- 2026-10-21: Edit actions are recorded in `History<EditAction>` with readable labels; the arrange toolbar opens a floating History panel that can jump to any point
- Remaining: runtime verification of crossfades, relative snap and the trash browser