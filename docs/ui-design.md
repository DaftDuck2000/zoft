# UI Design Document

## Overview

Zoft's UI evolves from egui (immediate mode, fast iteration) to custom iced/wgpu (retained mode, GPU-accelerated, professional look).

## Design Principles

1. **Musician-First** - Workflows match Logic Pro / Pro Tools mental models
2. **Efficiency** - Minimize clicks, maximize keyboard shortcuts
3. **Clarity** - Clear visual hierarchy, consistent color coding
4. **Performance** - 60 FPS at 4K, 100+ tracks
5. **Accessibility** - Screen readers, high contrast, keyboard navigation

## Phase 1: egui (Months 1-12)

### Layout
```
┌─────────────────────────────────────────────────────────────┐
│ Menu Bar: File Edit View Track Mix Window Help              │
├──────────────┬──────────────────────────────────────────────┤
│              │ Transport Bar                                 │
│              ├──────────────────────────────────────────────┤
│ Track List   │ Timeline Ruler (Bars/Beats, Timecode)        │
│              ├──────────────────────────────────────────────┤
│              │ Arrange Area (Clip Lanes)                    │
│              │                                              │
│              │                                              │
├──────────────┼──────────────────────────────────────────────┤
│              │ Status Bar: CPU, Disk, Sample Rate, Latency  │
└──────────────┴──────────────────────────────────────────────┘
```

### Track List
- **Columns**: Color, Name, Type Icon, M/S/R/Arm, Mini Fader, Pan, I/O
- **Interactions**: Drag reorder, Right-click context menu, Double-click rename
- **Keyboard**: ↑/↓ select, Enter rename, Delete remove, Ctrl+D duplicate

### Transport Bar
```
[◄◄] [►/❚❚] [►►] [●]    Position: 1.1.000  |  0:00.000  |  44100 smp
     Tempo: 120.000 BPM  ▼    Time Sig: 4/4  ▼    Metronome: ●
```

### Arrange Area
- **Horizontal**: Time (bars/beats), zoom Ctrl+wheel, scroll wheel/drag
- **Vertical**: Tracks, scroll wheel, drag track list
- **Clips**: Colored blocks, fade handles, gain handle, selection highlight
- **Tools**: Select (1), Split (2), Draw (3), Erase (4), Mute (5), Glue (6)

### Piano Roll (Bottom Panel)
- **Trigger**: Double-click MIDI clip or toolbar button
- **Layout**: Piano keys left, note grid right, velocity bottom, CC lanes bottom
- **Tools**: Same as arrange + Pencil, Velocity, Line, Curve

### Mixer (Separate Window)
- **Channel Strips**: Fader, Pan, M/S/R, Inserts (8), Sends (8), Meter, I/O
- **Meter Bridge**: Top section, detachable
- **Routing Matrix**: Right panel, visual patchbay

## Phase 2: iced/wgpu Custom UI (Months 24-30)

### Visual Design

**Color Palette (Dark Theme)**
```
Background:      #1e1e1e
Surface:         #252526
Surface Elevated:#2d2d2d
Primary:         #007acc
Primary Hover:   #005a9e
Secondary:       #3c3c3c
Border:          #3c3c3c
Text Primary:    #cccccc
Text Secondary:  #888888
Text Muted:      #666666
Accent:          #007acc
Success:         #4ec9b0
Warning:         #dcdcaa
Error:           #f44747
```

**Track Colors** (16 preset, user editable)
```
1: #e51400 (Red)      9: #00aba9 (Teal)
2: #a4c400 (Lime)     10: #8cbf26 (Green)
3: #00aba9 (Teal)     11: #ff8c00 (Orange)
4: #60a917 (Green)    12: #f0a30a (Amber)
5: #0050ef (Blue)     13: #e3008c (Magenta)
6: #aa00ff (Purple)   14: #825a2c (Brown)
7: #f0a30a (Amber)    15: #6d8764 (Olive)
8: #ff8c00 (Orange)   16: #1ba1e2 (Light Blue)
```

### Custom Widgets

#### Timeline Widget
```rust
struct Timeline {
    // Visual
    track_height: f32,           // 60-120px, user adjustable
    ruler_height: f32,           // 24px
    pixels_per_tick: f32,        // Zoom level
    
    // State
    visible_range: Range<TickPos>,
    playhead: TickPos,
    loop_range: Option<Range<TickPos>>,
    selection: Selection,
    
    // Interaction
    tool: Tool,
    drag_state: Option<DragState>,
}
```

**Features**:
- Virtualized rendering: only visible tracks/clips
- GPU waveform textures (overview + detail)
- Smooth playhead animation (interpolated)
- Marquee selection, rubber band
- Snap lines during drag

#### Piano Roll Widget
```rust
struct PianoRoll {
    // Visual
    key_width: f32,              // 24px
    note_height: f32,            // 12px (at 100% zoom)
    velocity_lane_height: f32,   // 60px
    cc_lane_height: f32,         // 60px each
    
    // State
    visible_pitch_range: Range<u8>,
    visible_time_range: Range<TickPos>,
    notes: Vec<NoteView>,
    ghost_notes: Vec<NoteView>,
    
    // Interaction
    tool: PianoTool,
    selection: Selection,
    velocity_drag: Option<VelocityDrag>,
}
```

**Features**:
- Note rectangles with velocity gradient
- Velocity lane: bars + curve editor
- CC lanes: point/line/curve editing
- Ghost notes from other clips (dimmed)
- Scale highlighting (major/minor/modes)
- Drum mode: note names, single row per drum

#### Mixer Strip Widget
```rust
struct MixerStrip {
    // Visual
    width: f32,                  // 80px narrow, 120px wide
    meter_width: f32,            // 12px
    fader_height: f32,           // 200px
    
    // Components (top to bottom)
    meter: MeterView,
    inserts: InsertRackView,     // 8 slots, drag reorder
    sends: SendRackView,         // 8 knobs
    eq_curve: MiniEQView,        // 4-band preview
    fader: FaderView,            // Touch/drag, automation indicator
    pan: PanView,                // Knob or slider
    io: IORoutingView,           // Dropdown menus
    name: NameView,              // Editable, color indicator
    solo_mute: SoloMuteView,     // Large buttons
    record_arm: RecordArmView,   // Red button
}
```

#### Waveform View (GPU)
```rust
struct WaveformView {
    // GPU Resources
    overview_texture: TextureView,    // Min/max atlas
    detail_buffer: Option<Buffer>,    // Samples for zoom > 1:1
    
    // State
    clip: AudioClipRef,
    viewport: Range<f32>,             // Visible sample range (0..1)
    gain: f32,
    fades: Vec<Fade>,
    selection: Option<Range<f32>>,
}
```

**Shader Pipeline**:
1. Compute shader: Generate min/max atlas from audio data
2. Vertex shader: Expand atlas to screen coordinates
3. Fragment shader: Draw envelope, gain, fades, selection

### Animation System

```rust
struct Animation {
    id: AnimationId,
    target: AnimatedProperty,    // Fader position, knob angle, panel width
    from: f32,
    to: f32,
    easing: Easing,
    duration: Duration,
    start: Instant,
}

enum Easing {
    Linear,
    EaseInOut(f32),              // Exponent
    Spring { stiffness: f32, damping: f32 },
    Custom(fn(f32) -> f32),
}
```

**Animated Properties**:
- Fader/knob value changes (parameter automation)
- Panel expand/collapse
- Playhead movement (interpolated)
- Clip drag (ghost follows cursor)
- Meter ballistics (peak hold decay)
- Crossfade curves

### Theming System

```rust
// themes/dark.ron
Theme {
    name: "Dark",
    palette: Palette { ... },
    spacing: Spacing { xs: 4, sm: 8, md: 16, lg: 24, xl: 32 },
    border_radius: Radius { sm: 2, md: 4, lg: 8, xl: 12 },
    shadows: Shadows { none: 0, sm: 2, md: 8, lg: 16, xl: 24 },
    typography: Typography {
        font_family: "JetBrains Mono",
        ui: 13,
        mono: 12,
        heading: 18,
        large: 24,
    },
    animation: AnimationConfig {
        fast: 100ms,
        normal: 200ms,
        slow: 300ms,
    },
}
```

**User Themes**: `.ron` files in `~/Zoft/themes/`, hot-reloadable.

### Layout System

**Docking Panels** (like VS Code / Blender):
- Panels can be tabbed, split horizontally/vertically
- Save/restore layout per project
- Default layouts: "Tracking", "Mixing", "Editing", "Composing"

**Responsive Breakpoints**:
- < 1400px: Collapse inspector, hide CC lanes
- < 1920px: Normal
- > 2560px: Show extra meters, wider strips

### Keyboard Shortcuts (Logic Pro Style)

| Category | Shortcut | Action |
|----------|----------|--------|
| Transport | Space | Play/Stop |
| | Enter | Return to Start |
| | R | Record |
| | . | Go to Next Marker |
| | , | Go to Previous Marker |
| Editing | 1-6 | Tools (Select, Split, Draw, Erase, Mute, Glue) |
| | A | Select All |
| | D | Duplicate |
| | B | Blade (Split at Playhead) |
| | J/K | Nudge Left/Right |
| | [/] | Zoom Horizontal |
| | Shift+[/] | Zoom Vertical |
| | F | Zoom to Selection |
| | G | Toggle Grid |
| | S | Toggle Snap |
| Mixer | M | Toggle Mixer Window |
| | Ctrl+M | New Aux Bus |
| | X | Toggle Insert Rack |
| | V | Toggle Send Rack |
| MIDI | P | Piano Roll |
| | Shift+P | Score/Notation |
| | Q | Quantize |
| | T | Transpose |
| | H | Humanize |

### Accessibility

- **Screen Readers**: ARIA labels, live regions for transport state
- **High Contrast**: Theme variant with 7:1 contrast ratios
- **Keyboard Navigation**: Tab order, focus indicators, shortcuts for all actions
- **Reduced Motion**: Disable animations, instant transitions
- **Color Blind**: Track color patterns + labels, not color-only

## Migration Plan (egui → iced)

1. **Month 24**: Add iced as dependency, create `daw-ui-iced` crate
2. **Month 25**: Port Transport Bar, Track List (low complexity)
3. **Month 26**: Port Timeline (high complexity, GPU waveforms)
4. **Month 27**: Port Piano Roll
5. **Month 28**: Port Mixer
6. **Month 29**: Port Plugin Editor embedding
7. **Month 30**: Remove egui, polish, performance
8. **Ongoing**: Keep egui for dev tools (debug UI, profiling)