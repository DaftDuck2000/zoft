# Feature: Undo/Redo System

## Phase: 1 — Audio Recording & Editing
## ID: 1.5
## Priority: Critical
## Estimated: 1 week (start early, integrate throughout)
## Status: `[~]` In Progress
## Depends On: 0.1

## Description
Comprehensive undo/redo system using Command pattern with history compression, snapshots, and branching support.

## Requirements

- [x] Command trait: `execute()`, `undo()`, `description()`
- [x] History stack: Linear with capacity limit (configurable, default 1000)
- [x] Macro commands: Group multiple commands as single undo step
- [x] History compression: Merge similar commands (e.g., drag = one move)
- [ ] Snapshots: Full project state at intervals (for large jumps)
- [x] Branching: Undo → new action discards the redo branch (standard linear branching)
- [ ] Per-track undo scope (optional)
- [x] Keyboard: Ctrl+Z / Ctrl+Shift+Z (Cmd on macOS)
- [x] History panel: List with descriptions, jump to any point

## Implementation Notes

### Core history

`daw-core/src/undo.rs` now provides two layers:

- `History<T>`: a generic, linear undo/redo stack. It stores `HistoryEntry<T>`
  values (action, description, timestamp), enforces a capacity limit
  (default 1000), drops the oldest entries when full, and clears the redo
  branch when a new edit is pushed. `take_undo()` / `complete_undo()` and
  `take_redo()` / `complete_redo()` let the caller apply each action (and any
  UI side effects) without cloning the stored action.
- `UndoHistory` and `UndoManager`: a command-backed history for
  `dyn Command`. `execute()` runs the command and records it; `undo()` and
  `redo()` apply the inverse or the command again through `CommandContext`.
  `MacroRecorder` groups several commands into a `MacroCommand`.

### Arrange-view history

The arrange view uses `History<EditAction>` directly. `EditAction` describes
clip moves, shape edits (trim/fade/gain/color), source replacements, track
attach/detach, track mute/solo/record-arm/volume changes, time-signature and
tempo changes and grouped edits. Multi-clip moves, deletes, duplicates, pastes
and effect applications become labelled groups and undo as one step. Every
action produces a label for the History panel, for example:

- Move clip / Move clips
- Clip gain / Fade in / Fade out / Clip color / Trim clip
- Process clip / Edit clip source
- Add clip / Delete clip / Delete N clips / Duplicate clips / Paste clips
- Apply Normalize (N clips)
- Mute track / Solo track / Arm track / Track volume
- Time signature 3/4 / Tempo 128.0 BPM
- Split clip / Consolidate clips

Continuous edits (gain slider, fade length) merge into the previous entry while
the pointer gesture is active, so one drag is one undo step. Split and
Consolidate are stored as labelled groups and undo as a single step. The
floating **History** window lists edits newest-first, shows redoable states
above the current point and past edits below it, and clicking any entry jumps
the project to that state. Undo/redo during playback reloads the mixer
immediately.

### Remaining

- Snapshots (serialized project state every N commands) for very large jumps.
- Wire automation edits into the same history.
- Optional per-track undo scope.

## Technical Details

### Command Pattern
```rust
trait Command: Send + Sync {
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<(), CommandError>;
    fn undo(&mut self, ctx: &mut CommandContext) -> Result<(), CommandError>;
    fn description(&self) -> String;
    fn merge(&mut self, other: &dyn Command) -> Option<Box<dyn Command>>; // For compression
    fn is_noop(&self) -> bool;
}

struct CommandContext {
    project: &mut Project,
    engine_tx: &Sender<EngineMsg>,
    audio_pool: &mut AudioPool,
}
```

### Command Types
```rust
// Track level
struct AddTrackCmd { track: Track }
struct RemoveTrackCmd { track_id: TrackId, track_data: TrackData }
struct ReorderTracksCmd { track_ids: Vec<TrackId> }

// Clip level
struct AddClipCmd { track_id: TrackId, clip: AudioClip }
struct RemoveClipCmd { clip_id: ClipId, clip_data: ClipData }
struct MoveClipCmd { clip_id: ClipId, old_pos: TimelinePos, new_pos: TimelinePos }
struct TrimClipCmd { clip_id: ClipId, old_range: Range<u64>, new_range: Range<u64> }
struct SplitClipCmd { clip_id: ClipId, position: u64, new_clip_id: ClipId }

// Parameter level
struct SetParameterCmd { target: ParamTarget, old_value: f32, new_value: f32 }
struct RecordAutomationCmd { lane_id: LaneId, events: Vec<AutomationEvent> }

// Project level
struct SetTempoCmd { old_bpm: f32, new_bpm: f32, at_tick: u64 }
struct SetTimeSigCmd { ... }
```

### History Management
```rust
struct UndoHistory {
    past: Vec<HistoryEntry>,    // Commands done
    future: Vec<HistoryEntry>,  // Commands undone (redo stack)
    snapshots: Vec<Snapshot>,   // Periodic full state
    max_size: usize,
    snapshot_interval: usize,   // Every N commands
}

struct HistoryEntry {
    command: Box<dyn Command>,
    timestamp: Instant,
    macro_group: Option<MacroId>,
}

struct Snapshot {
    project_data: Vec<u8>,      // Serialized project
    command_index: usize,       // Index in past at snapshot time
}
```

### Macro Recording
```rust
struct MacroRecorder {
    commands: Vec<Box<dyn Command>>,
    is_recording: bool,
}

impl MacroRecorder {
    fn start(&mut self) { self.is_recording = true; }
    fn record(&mut self, cmd: Box<dyn Command>) { 
        if self.is_recording { self.commands.push(cmd); }
    }
    fn finish(&mut self) -> Option<Box<dyn Command>> {
        self.is_recording = false;
        if self.commands.len() > 1 {
            Some(Box::new(MacroCommand { commands: self.commands.drain(..).collect() }))
        } else {
            self.commands.pop()
        }
    }
}
```

### Compression Rules
- Consecutive `MoveClipCmd` on same clip → merge to single move
- Consecutive `SetParameterCmd` on same param → keep last
- Drag operations: Start macro on mouse down, finish on mouse up

## Acceptance Criteria

- [x] Ctrl+Z undoes last action
- [x] Ctrl+Shift+Z redoes
- [x] History panel shows readable descriptions
- [x] Drag = single undo step
- [ ] Snapshots allow jumping back 100+ steps instantly (the History panel currently jumps by replaying cheap edit actions; no serialized snapshots yet)
- [x] Memory usage < 500 MB for 1000 commands (entries are small enums/boxes; no audio data is cloned)
- [x] Undo during playback works (the mixer reloads after each undo/redo)

## Progress Log

- 2026-10-21: Added generic `History<T>` with capacity, descriptions and take/complete undo/redo; rewrote `UndoHistory` and `UndoManager` to execute, undo and redo `dyn Command`s correctly
- 2026-10-21: Arrange-view edits now use `History<EditAction>` with readable labels; split/consolidate are labelled groups and slider drags merge into one entry
- 2026-10-21: Track mute, solo, record-arm and volume controls are recorded as undoable track-state edits; mute/solo/volume feed the playback mixer
- 2026-10-21: Added the floating **History** panel (newest edits first) with jump-to-any-point
- Remaining: snapshots, automation integration, optional per-track scope