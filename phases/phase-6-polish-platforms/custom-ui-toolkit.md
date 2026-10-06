# Feature: Custom UI Toolkit Migration

## Phase: 6 — Polish & Platform Expansion
## ID: 6.3
## Priority: High
## Estimated: 6 weeks
## Status: `[ ]` Not Started
## Depends On: 6.1, 6.2

## Description
Migrate from egui to custom GPU-accelerated UI (iced + wgpu): timeline, piano roll, mixer, waveform, animations, theming.

## Requirements

- [ ] **Renderer**: wgpu backend (Vulkan/Metal/DX12/WebGPU)
- [ ] **Framework**: iced (retained mode, Elm architecture)
- [ ] **Custom Widgets**: Timeline, Piano Roll, Mixer Strip, Waveform View, Knob, Fader
- [ ] **Animation System**: Spring, easing, staggered, 60 FPS
- [ ] **Theming**: CSS-like, dark/light, user themes, per-widget styling
- [ ] **Layout**: Flexbox/Grid, responsive, docking panels
- [ ] **Input**: Multi-touch, pen, keyboard shortcuts, MIDI learn
- [ ] **Accessibility**: Screen reader, high contrast, focus management
- [ ] **Performance**: < 5ms frame, GPU waveform, virtualized lists
- [ ] **Migration**: Incremental, egui for dev tools, iced for main UI

## Technical Details

### Architecture
```rust
// iced application structure
struct ZoftApp {
    state: AppState,
    theme: Theme,
    renderer: wgpu::Renderer,
    window: Window,
}

struct AppState {
    project: ProjectHandle,
    engine: EngineHandle,
    ui: UiState,
    editors: HashMap<EditorId, EditorState>,
}

enum Message {
    // Engine messages
    Transport(TransportCmd),
    Track(TrackCmd),
    Clip(ClipCmd),
    Parameter(ParameterCmd),
    
    // UI messages
    Ui(UiMsg),
    Editor(EditorId, EditorMsg),
    
    // System
    Tick(Instant),
    Resize(PhysicalSize),
    ScaleChanged(f32),
}
```

### Custom Widgets

**Timeline Widget**
```rust
struct Timeline {
    time_range: Range<TickPos>,
    pixels_per_tick: f32,
    tracks: Vec<TrackView>,
    playhead: TickPos,
    loop_range: Option<Range<TickPos>>,
    markers: Vec<Marker>,
    selection: Selection,
}

impl Widget<Message, Theme, Renderer> for Timeline {
    fn layout(&self, renderer: &Renderer, limits: &LayoutLimits) -> Node { ... }
    fn draw(&self, renderer: &mut Renderer, layout: Layout, cursor: Cursor, viewport: &Rectangle) { ... }
    fn on_event(&mut self, event: Event, layout: Layout, cursor: Cursor, renderer: &Renderer, clipboard: &mut Clipboard, shell: &mut Shell<Message>) -> event::Status { ... }
}
```

**Waveform View (GPU)**
```rust
// Shader-based waveform rendering
struct WaveformView {
    overview_texture: wgpu::Texture,    // Min/max atlas
    detail_buffer: Option<wgpu::Buffer>, // Samples for zoom > 1:1
    viewport: Range<f32>,               // Visible sample range
    clip_gain: f32,
    fades: Vec<Fade>,
}

impl WaveformView {
    fn generate_overview_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, audio: &[f32]) {
        // Compute shader: min/max per pixel column
        // Store in texture atlas (1 pixel = 256 samples)
    }
    
    fn draw(&self, render_pass: &mut wgpu::RenderPass) {
        // Vertex shader: expand texture to screen space
        // Fragment shader: draw min/max envelope, gain, fades
    }
}
```

**Piano Roll**
```rust
struct PianoRoll {
    notes: Vec<NoteView>,
    grid: GridConfig,
    velocity_lane: VelocityLane,
    cc_lanes: Vec<CCLane>,
    tool: Tool,
    selection: Selection,
    ghost_notes: Vec<NoteView>,
}
```

### Animation System
```rust
struct Animation {
    id: AnimationId,
    target: AnimatedValue,
    from: f32,
    to: f32,
    easing: Easing,
    duration: Duration,
    start: Instant,
    on_complete: Option<Message>,
}

enum Easing {
    Linear,
    EaseIn(Exponent),
    EaseOut(Exponent),
    EaseInOut(Exponent),
    Spring { stiffness: f32, damping: f32 },
    Custom(Box<dyn Fn(f32) -> f32>),
}
```

### Theming
```rust
// themes/default.ron
Theme {
    name: "Dark",
    palette: Palette {
        background: 0x1e1e1e,
        surface: 0x252526,
        primary: 0x007acc,
        secondary: 0x3c3c3c,
        text: 0xcccccc,
        text_muted: 0x888888,
        border: 0x3c3c3c,
        accent: 0x007acc,
        success: 0x4ec9b0,
        warning: 0xdcdcaa,
        error: 0xf44747,
    },
    spacing: Spacing { xs: 4, sm: 8, md: 16, lg: 24, xl: 32 },
    border_radius: Radius { sm: 2, md: 4, lg: 8 },
    shadows: Shadows { sm: 1, md: 4, lg: 8 },
    typography: Typography {
        font_family: "JetBrains Mono",
        size_xs: 11, size_sm: 12, size_md: 13, size_lg: 15, size_xl: 18,
    },
}
```

## Acceptance Criteria

- [ ] iced + wgpu runs on Linux/macOS/Windows
- [ ] Timeline: 60 FPS with 100 tracks, 1000 clips
- [ ] Piano roll: smooth note drag, velocity edit
- [ ] Waveform: GPU overview, detail at zoom
- [ ] Mixer: 64 strips, meters animate smoothly
- [ ] Animations: spring physics, 60 FPS
- [ ] Themes: dark/light, user .ron files
- [ ] Accessibility: screen reader works
- [ ] Dev tools: egui still available for debug

## Progress Log

- YYYY-MM-DD: iced + wgpu setup
- YYYY-MM-DD: Basic widgets (button, slider, panel)
- YYYY-MM-DD: Timeline widget
- YYYY-MM-DD: Piano roll widget
- YYYY-MM-DD: Waveform GPU rendering
- YYYY-MM-DD: Mixer strip widget
- YYYY-MM-DD: Animation system
- YYYY-MM-DD: Theming engine
- YYYY-MM-DD: Migration from egui
- YYYY-MM-DD: Accessibility