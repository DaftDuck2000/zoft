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
- [~] Track list panel: track headers (name, colour, M/S/R, **Mon**, volume), **+A**/**+M** add buttons and **×** remove; track reordering not implemented yet
- [✓] Transport bar: play/stop/record, position display, tempo, time sig, metronome toggle
- [✓] Timeline ruler: bars/beats, timecode, loop range markers, playhead
- [✓] Clip lanes placeholder: empty track lanes showing clip blocks
- [✓] Keyboard shortcuts: Space (play/stop), Enter (return to start), R (record)
- [~] Theme: dark mode default; no light/user theme toggle yet
- [ ] Window state persistence: size, position, panel sizes (not wired; eframe persistence is not enabled)

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
- Track header: color swatch, name, M/S/R/Arm buttons, volume fader and pan knob (mini)

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
- [✓] Add/remove tracks works (audio and MIDI tracks can be added and removed; reordering not implemented)
- [✓] Transport controls engine (play/stop/seek)
- [✓] Position display updates in real-time
- [✓] Timeline ruler shows correct bars/beats for tempo
- [✓] Loop range visible and draggable (ruler loop lane + In/Out buttons)
- [✓] Keyboard shortcuts work
- [ ] Window state persists across restarts

## Progress Log

- 2026-10-07: Main window with panels (TopBottomPanel, SidePanel, CentralPanel)
- 2026-10-07: Track list functional with color indicators
- 2026-10-07: Transport bar connected (play/stop, position display)
- 2026-10-07: Timeline ruler rendering (16 divisions, beat markers)
- 2026-10-07: Keyboard shortcuts (Space play/stop, R record)
- 2026-10-21: Post-phase updates: ruler seek and loop lane, zoom/scroll, loop/In/Out, metronome, tempo/tap, track M/S/R/Mon/volume, Clip Audio window, History panel
- 2026-10-23: Track list rows are height-matched to the arrange lanes; **+A**/**+M** add audio/MIDI tracks and **×** removes a track (both undoable); a dedicated MIDI bar lists inputs, auto-connects the first keyboard, and shows live note/velocity activity with a Refresh button
- 2026-10-30: MIDI controls moved into a top-bar MIDI menu. The Source selector is a second-level menu, and live note/velocity activity stays visible next to the menu
- 2026-10-30: Added ruler markers. Shift+M or the + Marker button drops a named marker at the playhead; click a flag to jump there and right-click to delete it. Marker edits are undoable
- 2026-10-30: Added ◄◄ and ►► transport buttons plus Home/End shortcuts to jump to the project start or end
- 2026-10-30: Added track reordering. Drag the ⠿ handle at the left of a track header to move it; the new order is undoable

## Future Tasks (Post-Phase 0)

- [✓] **Cursor icon for playhead dragging**: `ResizeHorizontal` while hovering/dragging the playhead
- [✓] **Playhead click-to-seek**: ruler lower lane seeks; upper lane sets the loop
- [✓] **Timeline zoom**: Ctrl+wheel to zoom in/out on timeline
- [✓] **Timeline horizontal scroll**: Mouse wheel or middle-drag to scroll horizontally
- [✓] **Slider mouse wheel**: Wheel over a slider moves it by 5% of its travel; up increases, down decreases
- [ ] **Vertical scroll**: Track list overflow handling
- [✓] **Loop range UI**: Drag the ruler loop lane or use In/Out at the playhead
- [x] **Markers UI**: Flag icons on ruler, click to jump
- [x] **Track reordering**: Drag tracks to reorder in track list
- [ ] **Track context menu**: Right-click to add/remove/duplicate tracks
- [✓] **Track header controls**: M/S/R, monitor mode, volume fader and pan knob
- [x] **Transport bar enhancements**: Tap tempo, Bars/Time ruler dropdown and ◄◄ | ►► jump buttons
- [✓] **Metronome toggle**: Click toggle; count-in not yet
- [ ] **Dark/light theme toggle**: User preference (dark default only)
- [ ] **Window state persistence**: Save/restore panel sizes, window position