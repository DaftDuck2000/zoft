# Feature: Undo/Redo System

## Phase: 1 — Audio Recording & Editing
## ID: 1.5
## Priority: Critical
## Estimated: 1 week (start early, integrate throughout)
## Status: `[ ]` Not Started
## Depends On: 0.1

## Description
Comprehensive undo/redo system using Command pattern with history compression, snapshots, and branching support.

## Requirements

- [ ] Command trait: `execute()`, `undo()`, `description()`
- [ ] History stack: Linear with capacity limit (configurable, default 1000)
- [ ] Macro commands: Group multiple commands as single undo step
- [ ] History compression: Merge similar commands (e.g., drag = one move)
- [ ] Snapshots: Full project state at intervals (for large jumps)
- [ ] Branching: Undo → new action creates new timeline (optional)
- [ ] Per-track undo scope (optional)
- [ ] Keyboard: Ctrl+Z / Ctrl+Shift+Z (Cmd on macOS)
- [ ] History panel: List with descriptions, jump to any point

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

- [ ] Ctrl+Z undoes last action
- [ ] Ctrl+Shift+Z redoes
- [ ] History panel shows readable descriptions
- [ ] Drag = single undo step
- [ ] Snapshots allow jumping back 100+ steps instantly
- [ ] Memory usage < 500 MB for 1000 commands
- [ ] Undo during playback works (parameter changes revert smoothly)

## Progress Log

- YYYY-MM-DD: Command trait + basic history
- YYYY-MM-DD: Track/clip commands
- YYYY-MM-DD: Parameter commands
- YYYY-MM-DD: Macro recording + compression
- YYYY-MM-DD: Snapshots
- YYYY-MM-DD: History panel UI
- YYYY-MM-DD: Integration with all edit ops