# Feature: Piano Roll Editor

## Phase: 2 — MIDI & Sequencing
## ID: 2.2
## Priority: Critical
## Estimated: 4 weeks
## Status: `[~]` In Progress
## Depends On: 2.1

## Description
Full-featured piano roll: note entry/selection/editing, velocity lanes, CC lanes, multiple tools, zoom/scroll, keyboard shortcuts.

## Requirements

- [x] Note grid: Pitch (vertical) × Time (horizontal) with its own snap picker (Off, Bar, Beat, 1/8, 1/16, 1/32, 1/8T, 1/16T)
- [x] Note entry: Double-click to add (new notes take the current snap length), drag the right edge to resize, right-click to delete
- [x] Selection: Click, Ctrl/Shift+click, drag a marquee over empty space, Select All
- [~] Note editing: Drag move and resize implemented; velocity drag pending
- [x] Note audition: Clicking a note or dragging it to a new pitch plays it
- [~] Velocity lane: Bar graph per note with drag-to-edit, toggleable; curve tool pending
- [x] CC lanes: Up to four simultaneous controller lanes (off by default) with add/remove and draw/edit; CC10 Pan, CC7 Volume and CC11 Expression drive MIDI note playback
- [ ] Tool palette: Select, Pencil, Eraser, Mute, Scissors, Glue, Velocity, Line
- [x] MIDI Tools side panel: quantize, transpose, scale, velocity, note join/split and transform tools for the open clip
- [x] Playhead: Bar/beat ruler, draggable project playhead line and split-notes-at-playhead action
- [x] Zoom: Ctrl+wheel and +/− buttons; Fit resets the view
- [x] Scroll: Wheel pans time, Shift+wheel pans pitch, middle-drag pans both
- [ ] Fit selection, horizontal/vertical scrollbars
- [x] Ghost notes: Show notes from other MIDI tracks/clips overlapping the current clip (dimmed), toggleable globally and per track
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

- [x] Notes render correctly at all zoom levels
- [x] Double-click adds notes at the snap grid
- [x] Selection rectangle selects multiple notes
- [x] Drag moves notes (time and pitch) and resizes note lengths
- [x] Notes audition when clicked or dragged to a new pitch
- [x] Wheel/middle-drag pans the piano-roll view; Ctrl+wheel zooms
- [~] Velocity lane edits work
- [x] CC lanes: add/edit/delete points across multiple lanes
- [ ] Ghost notes show other clips
- [ ] Scale highlighting works (major/minor/modes)
- [ ] Drum mode shows note names
- [ ] 60 FPS with 1000+ notes visible

## Progress Log

- 2026-10-24: Basic read-only piano roll. Double-click a MIDI clip (or use Arrange → Piano Roll…) to open a window with pitch stripes, a C-labelled keyboard, beat/bar grid lines and note rectangles sized by tick position/duration and coloured by velocity. Editing, zoom/scroll, velocity/CC lanes and the tool palette are still to come
- 2026-10-24: MIDI clips now show a miniature piano-roll preview directly in the arrange lane, with pitch-normalized note lanes and velocity-based opacity, so takes can be read without opening the full piano-roll window
- 2026-10-24: Piano roll is editable. Double-click empty grid space adds a note at the Arrange snap setting, drag moves selected notes in time and pitch, dragging a note's right edge resizes it, and right-click/Delete removes notes. Ctrl/Shift+click builds a multi-note selection and Select All selects the clip. Every gesture is undoable through a new `MidiClip` history action. Velocity/CC lanes and the tool palette remain
- 2026-10-24: Piano-roll audition and navigation. Clicking a note plays it through the live synth, and dragging it to a new pitch retriggers the preview. The view now pans with the wheel (time), Shift+wheel (pitch) or middle-drag, zooms with Ctrl+wheel and the +/− buttons, and re-fits with Fit. The piano roll has its own snap picker (Off, Bar, Beat, 1/8, 1/16, 1/32, 1/8T, 1/16T), so note entry, dragging and resizing no longer depend on the arrange setting. Newly double-clicked notes are as long as the current snap value (Off falls back to the last used length)
- 2026-10-24: Trimmed MIDI clip regions are shaded in the piano roll. Notes outside the clip's visible window stay editable and reappear when the clip edge is dragged back out in the arrange view
- 2026-10-24: MIDI Tools now live in a left-side panel inside the piano-roll window. Quantize, transpose and velocity controls apply to the open clip as one undo step
- 2026-10-24: Piano-roll velocity colours now span a wide blue → cyan → yellow gradient. Dragging on empty grid space draws a selection marquee that selects every intersecting note; Ctrl/Shift makes the marquee additive. Middle-drag now pans without starting a note selection or edit
- 2026-10-24: Added a velocity lane below the piano-roll grid. Each note has a velocity bar; dragging a bar sets that note's velocity (or every selected note when the clicked bar is selected). The lane uses the same horizontal view/zoom as the grid, and each drag is one undo step
- 2026-10-24: Fixed the piano-roll window slowly growing to fill the screen. Grid height is now independent of the window's current size, breaking egui's content/window auto-resize feedback loop
- 2026-10-24: The velocity lane is taller and now scales with the piano-roll window height, so resizing the window gives both the note grid and velocity editor more room without re-triggering auto-growth
- 2026-10-24: Added a selectable CC lane below the velocity lane. Choose Mod Wheel, Breath, Volume, Pan, Expression, Sustain, Resonance or Cutoff, then click/drag to draw control points. Other controllers and notes are preserved, and each drag is one undo step
- 2026-10-24: The velocity lane can now be hidden with a Show velocity lane toggle, matching the CC lane. Hiding either lane gives the note grid more room, and the window remains stable
- 2026-10-26: MIDI Tools gained humanize (timing/velocity jitter), legato (extend toward the next note with a gap) and fixed note length. The left panel is scrollable so the additional tools don't affect window sizing
- 2026-10-26: Decluttered the MIDI Tools side panel. A Tools… menu with checkboxes lets the user show any combination of tools; Quantize is shown by default and other tools stay hidden until checked
- 2026-10-26: Added Scale Conform with root and scale selection; notes snap to the nearest scale degree while preserving register
- 2026-10-26: Added a Transform tool for time reverse, pitch mirror and seeded randomize of pitch, velocity and timing
- 2026-10-26: Quantize gained swing, and the Velocity tool gained compress, expand, curve and randomize modes alongside scaling
- 2026-10-29: Added up to four simultaneous CC lanes. The Controllers tool adds/removes lanes, each lane draws and edits its own controller independently, and lane heights adapt to keep the note grid usable
- 2026-10-29: CC10 Pan events now affect MIDI note playback. Each note uses the latest CC10 value at its start, applied before the track pan
- 2026-10-30: CC chase and expression. CC10 pan, CC7 volume and CC11 expression are sampled at the seek position for notes already sounding, and CC7/CC11 scale note gain
- 2026-10-26: Added the project playhead to the piano roll plus a Notes tool for joining same-pitch notes and splitting notes at the playhead
- 2026-10-26: Added a bar/beat ruler. Clicking or dragging the ruler, or dragging the playhead line anywhere in the grid, seeks the project playhead and pauses playback like the arrange timeline
- 2026-10-26: Added toggleable ghost notes for other MIDI tracks/clips in the project, mapped into the open clip's content coordinates and drawn dimmed behind the active notes like FL Studio's ghost channels
- 2026-10-26: Added a per-track ghost-note toggle. MIDI/instrument track headers have a G button (also available as “Show as ghost notes” in the track menu) so large projects can hide individual tracks from the ghost-note view
- 2026-10-26: Quantize is the default selected tool and the CC lane starts hidden, so the piano roll opens with a clean grid and one focused tool panel
- YYYY-MM-DD: Pencil/Eraser/Select tools
- YYYY-MM-DD: Note drag + resize
- YYYY-MM-DD: Velocity lane
- YYYY-MM-DD: CC lanes
- YYYY-MM-DD: Ghost notes + scale highlight
- YYYY-MM-DD: Drum mode
- YYYY-MM-DD: Keyboard shortcuts + tool palette