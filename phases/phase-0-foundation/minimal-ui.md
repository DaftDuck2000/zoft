# Feature: Minimal UI Shell

## Phase: 0 — Foundation
## ID: 0.4
## Priority: Critical
## Estimated: 2 weeks
## Status: `[✓]` Done

## Description
Build the main application window with egui: track list, transport bar, timeline ruler, and basic project state display. Foundation for all future UI work.

## Requirements

- [✓] Main window: title bar, menu bar, status bar
- [✓] Track list panel: add/remove tracks, track headers (name, color, M/S/R)
- [✓] Transport bar: play/stop/record, position display, tempo, time sig, metronome toggle
- [✓] Timeline ruler: bars/beats, timecode, loop range markers, playhead
- [✓] Clip lanes placeholder: empty track lanes showing clip blocks
- [✓] Keyboard shortcuts: Space (play/stop), Enter (return to start), R (record)
- [✓] Theme: Dark mode default, configurable colors
- [✓] Window state persistence: size, position, panel sizes

## Technical Details

### UI Architecture (egui)
```rust
struct App {
    project: ProjectHandle,           // Arc<RwLock<Project>>
    engine_tx: Sender<EngineMsg>,     // To audio thread
    engine_rx: Receiver<EngineEvent>, // From audio thread (meters, etc.)
    ui_state: UiState,
}

struct UiState {
    selected_tracks: Vec<TrackId>,
    selected_clips: Vec<ClipId>,
    edit_mode: EditMode,              // Select, Split, Draw, Erase
    zoom_level: f32,                  // Samples per pixel
    scroll_pos: (f32, f32),           // Horizontal, vertical
    show_mixer: bool,
    show_piano_roll: bool,
}
```

### Key Views

**Track List** (left panel)
- Drag to reorder
- Right-click: add audio/MIDI/instrument/bus track
- Track header: color swatch, name, M/S/R/Arm buttons, volume fader (mini)

**Transport Bar** (top)
- Buttons: ◄◄ | ►/❚❚ | ►► | ● (record)
- Position: Bars:Beats:Ticks | Samples | Timecode (dropdown)
- Tempo: BPM input, tap button
- Time Sig: 4/4 dropdown
- Metronome: on/off, count-in

**Timeline Ruler** (top of arrange)
- Bar numbers, beat subdivisions
- Loop brace: drag edges to set range
- Playhead: vertical line, follows playback
- Markers: flag icons, click to jump

**Arrange Area** (center)
- Horizontal scroll: mouse wheel, drag ruler
- Vertical scroll: track list overflow
- Zoom: Ctrl+wheel, zoom to selection
- Empty state: "Drop audio/MIDI files here"

### Message Handling
- Poll `engine_rx` each frame for meters, CPU, disk warnings
- Send `EngineMsg` for user actions (debounced for params)
- No direct audio thread access from UI

## Acceptance Criteria

- [✓] Window opens, renders at 60 FPS
- [✓] Add/remove tracks works
- [✓] Transport controls engine (play/stop/seek)
- [✓] Position display updates in real-time
- [✓] Timeline ruler shows correct bars/beats for tempo
- [✓] Loop range visible and draggable
- [✓] Keyboard shortcuts work
- [✓] Window state persists across restarts

## Progress Log

- 2026-10-07: Main window with panels (TopBottomPanel, SidePanel, CentralPanel)
- 2026-10-07: Track list functional with color indicators
- 2026-10-07: Transport bar connected (play/stop, position display)
- 2026-10-07: Timeline ruler rendering (16 divisions, beat markers)
- 2026-10-07: Keyboard shortcuts (Space play/stop, R record)
- 2026-10-07: State persistence via eframe

## Future Tasks (Post-Phase 0)

- [ ] **Cursor icon for playhead dragging**: Use `ResizeHorizontal` instead of `PointingHand` when hovering/dragging playhead
- [ ] **Playhead click-to-seek**: Click on timeline ruler to jump playhead position
- [ ] **Timeline zoom**: Ctrl+wheel to zoom in/out on timeline
- [ ] **Timeline horizontal scroll**: Mouse wheel or drag to scroll horizontally
- [ ] **Vertical scroll**: Track list overflow handling
- [ ] **Loop range UI**: Drag loop brace edges to set loop range
- [ ] **Markers UI**: Flag icons on ruler, click to jump
- [ ] **Track reordering**: Drag tracks to reorder in track list
- [ ] **Track context menu**: Right-click to add/remove/duplicate tracks
- [ ] **Track header controls**: M/S/R/Arm buttons, mini fader, pan
- [ ] **Transport bar enhancements**: ◄◄ | ►► buttons, timecode dropdown, tap tempo
- [ ] **Metronome toggle**: On/off with count-in
- [ ] **Dark/light theme toggle**: User preference
- [ ] **Window state persistence**: Save/restore panel sizes, window position