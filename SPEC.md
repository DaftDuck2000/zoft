# Zoft — Technical Specification

Version: 0.1.0
Status: Draft

---

## 1. Core Design Principles

1. **Real-time Safety First** — Audio thread never allocates, locks, or blocks. Lock-free message passing only.
2. **Sample-Accurate Timing** — All events scheduled at sample precision. Musical time ↔ sample time conversion is exact.
3. **Non-Destructive Editing** — Audio/MIDI edits are commands on immutable data. Original files never modified.
4. **Cross-Platform Abstraction** — Platform-specific code isolated behind traits. Linux (PipeWire/JACK) → macOS (Core Audio) → Windows (WASAPI).
5. **Plugin-Friendly Architecture** — Engine designed from day one to host CLAP/VST3 plugins in separate processes.
6. **Solo-Dev Sustainable** — Prefer composition over inheritance. Minimal dependencies. Clear module boundaries.

---

## 2. Data Models

### 2.1 Project

```rust
struct Project {
    id: Uuid,
    name: String,
    sample_rate: u32,           // Fixed per project (44.1k, 48k, 88.2k, 96k, 192k)
    bit_depth: BitDepth,        // 24-bit recording, 32-bit float internal
    tempo_map: TempoMap,        // Tempo changes over time
    time_signature_map: TimeSignatureMap,
    tracks: Vec<Track>,
    marker_regions: Vec<MarkerRegion>,
    audio_pool: AudioPool,      // References to audio files
    plugin_instances: Vec<PluginInstance>,
    undo_history: UndoHistory,
    preferences: Preferences,
}
```

### 2.2 Track Types

```rust
enum Track {
    Audio(AudioTrack),
    Midi(MidiTrack),
    Instrument(InstrumentTrack),  // MIDI input → plugin → audio output
    Bus(BusTrack),                // Summing bus, no clips
    Folder(FolderTrack),          // Organizational, can contain sub-tracks
}

struct AudioTrack {
    id: TrackId,
    name: String,
    color: Color,
    clips: Vec<AudioClip>,
    input: AudioInput,            // Hardware input, bus, or none
    output: AudioOutput,          // Hardware output, bus, or send
    sends: Vec<Send>,
    inserts: Vec<InsertSlot>,     // Plugin chain
    automation: AutomationLanes,
    record_arm: bool,
    monitor_mode: MonitorMode,    // Off / Auto / On
    mute: bool,
    solo: bool,
    volume: f32,                  // Linear gain
    pan: PanLaw,
}
```

### 2.3 Audio Clip (Non-Destructive)

```rust
struct AudioClip {
    id: ClipId,
    source: AudioFileRef,         // Reference to file in audio pool
    source_offset: SamplePos,     // Start sample in source file
    timeline_position: TimelinePos, // Position on track (musical + sample)
    length: SampleCount,          // Visible length (may be < source)
    gain: f32,                    // Clip gain (pre-fader)
    fade_in: Option<FadeCurve>,
    fade_out: Option<FadeCurve>,
    pitch_shift: f32,             // Semitones
    time_stretch: TimeStretchMode,
    loop_enabled: bool,
    loop_range: Option<Range<SamplePos>>,
}
```

### 2.4 MIDI Clip

```rust
struct MidiClip {
    id: ClipId,
    events: Vec<MidiEvent>,       // Owned events (note, CC, pitch bend, etc.)
    timeline_position: TimelinePos,
    length: TickCount,            // In musical ticks
    loop_enabled: bool,
    loop_range: Option<Range<TickCount>>,
}
```

### 2.5 Tempo Map

```rust
struct TempoMap {
    events: Vec<TempoEvent>,      // Sorted by tick position
    default_bpm: f32,
}

struct TempoEvent {
    tick: TickPos,
    bpm: f32,
    ramp: Option<TempoRamp>,      // Gradual tempo change to next event
}
```

**Sample ↔ Musical Time Conversion:**
```
samples = ∫(sample_rate * 60 / bpm(t)) dt  from 0 to tick
```
Implemented via piecewise integration over tempo segments.

---

## 3. Audio Engine

### 3.1 Processing Graph

- **Topological sort** of track routing (no cycles allowed except explicit feedback via sends)
- **Block processing**: 64-1024 samples per block (configurable)
- **Per-track state**: Each track maintains its own plugin chain state

### 3.2 Threading Model

```
┌────────────────────────────────────────────────────────────┐
│                     MAIN THREAD                              │
│  UI │ Project Management │ File I/O │ Plugin Scanner        │
└────────────────────────────┬─────────────────────────────────┘
                             │ Crossbeam channels (MPSC)
                             ▼
┌────────────────────────────────────────────────────────────┐
│                   AUDIO THREAD (RT)                         │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ Input Mix   │→ │ Track Graph │→ │ Output Mix / Meter  │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
│         ▲               │                   │                │
│         │    Lock-free  │    Lock-free      │                │
│         │    Ring Buf   │    Ring Buf       │                │
│         ▼               ▼                   ▼                │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              MESSAGE QUEUE (UI → Audio)              │   │
│  │  Transport commands │ Parameter changes │ Clip edits │   │
│  └─────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────┘
```

### 3.3 Lock-Free Communication

- **UI → Audio**: `crossbeam::channel::bounded` (capacity: 256 messages)
- **Audio → UI**: Metering, CPU load, disk warnings (unbounded, drop-if-full)
- **Parameter changes**: Sample-accurate scheduling via `ParameterEvent { frame_offset, param_id, value }`

### 3.4 Plugin Hosting (Future)

- **Process-per-plugin** for stability (CLAP supports this natively)
- **Shared memory** for audio buffers between host and plugin process
- **IPC**: `cpdal` or custom `pipewire`-style protocol

---

## 4. UI Architecture

### 4.1 Current: egui (Immediate Mode)

Pros: Fast iteration, built-in widgets, easy layout, good for dev tools
Cons: Not "pro" look, limited custom drawing, retention-mode mental model mismatch

### 4.2 Future: iced + wgpu (Retained Mode, GPU-Accelerated)

- Custom widgets: Timeline, Piano Roll, Mixer Strips, Waveform View
- GPU waveform rendering (texture atlas + compute shader for overviews)
- Animation system for smooth parameter changes

### 4.3 Key Views

| View | Component | Complexity |
|------|-----------|------------|
| Arrange | Timeline, Track Headers, Rulers, Clip Lanes | High |
| Mixer | Channel Strips, Meter Bridge, Routing Matrix | High |
| Piano Roll | Note Grid, CC Lanes, Tool Palette | High |
| Plugin Editor | Embedded plugin UI (VST3/CLAP) | Medium |
| Browser | File tree, Plugin list, Preset search | Medium |
| Transport | Play/Stop/Record, Tempo, Time Display | Low |

---

## 5. File Formats

### 5.1 Project File: `.zoft` (Binary, versioned)

- **Header**: Magic bytes, version, UUID
- **Sections**: Project metadata, Track data, Tempo map, Automation, Plugin states
- **Compression**: `lz4` for large sections
- **Backward compatibility**: Versioned sections, migration on load

### 5.2 Audio Files: BWF (Broadcast Wave Format)

- Standard WAV + `bext` chunk (originator, time reference, coding history, UMID)
- `cue ` chunk for markers/regions
- `LIST` chunk for metadata (title, artist, etc.)
- Support for 32-bit float, 24-bit int

### 5.3 Plugin Presets: `.zoftpreset` (JSON + binary blob)

- JSON metadata (name, author, plugin ID, version)
- Binary plugin state (opaque bytes from plugin)

---

## 6. Critical Algorithms

### 6.1 Waveform Rendering

- **Overview**: Pre-computed min/max per pixel column (stored in project)
- **Detail**: On-demand decode at zoom level > 1:1
- **GPU**: Texture atlas of overviews, compute shader for real-time zoom

### 6.2 Time Stretching / Pitch Shifting

- **Library**: `rubberband` (CLI) → port to Rust or FFI
- **Modes**: Tape (resample), Elastic (phase vocoder), Musical (transient-preserving)

### 6.3 Quantization

```rust
fn quantize(events: &mut [MidiEvent], grid: NoteValue, strength: f32, swing: f32) {
    for event in events {
        let target = snap_to_grid(event.tick, grid, swing);
        event.tick = lerp(event.tick, target, strength);
    }
}
```

### 6.4 Automation Curves

- **Linear, Exponential, Logarithmic, S-curve, Step**
- **Sample-accurate**: Interpolated per block in audio thread
- **Thinning**: Douglas-Peucker algorithm for recorded automation

---

## 7. Plugin Support Plan

| Format | Priority | Notes |
|--------|----------|-------|
| CLAP | 1 | Modern, open, Linux-native, process-per-plugin |
| VST3 | 2 | Steinberg SDK, wide compatibility |
| AUv3 | 3 | macOS only, later phase |
| LV2 | 4 | Linux native, optional |

**Sandboxing**: Each plugin in separate process. Communication via shared memory + IPC.
**Crash Recovery**: Host detects plugin crash, disables instance, preserves project state.

---

## 8. Performance Targets

| Metric | Target |
|--------|--------|
| Audio callback latency | < 2ms @ 48kHz/256 samples |
| DSP CPU (empty project) | < 1% single core |
| DSP CPU (50 tracks + plugins) | < 50% single core |
| UI frame time | < 16ms (60 FPS) |
| Project load (100 tracks) | < 3s |
| Waveform overview generation | < 1s per minute of audio |

---

## 9. Testing Strategy

| Level | Tools | Coverage Target |
|-------|-------|-----------------|
| Unit | `cargo test` | 80% core modules |
| Integration | `cargo test --test integration` | Key workflows |
| Audio | `dasample` test signals, golden master | Regression detection |
| UI | `egui` test harness / `iced` test utils | Smoke tests |
| Plugin | `clap-validator`, `vst3_test_host` | Compatibility |

---

## 10. Open Questions

1. **License**: GPL-3.0 (forces plugins open) vs Proprietary (allows closed plugins)?
2. **Scripting**: Lua (via `mlua`) for macros/extensions?
3. **Video**: Sync to video file (MP4 container, seek via `ffmpeg`)?
4. **Cloud**: Project sharing, collaborative editing (CRDT)?
5. **Hardware**: MCU/HUI protocol for control surfaces?

---

## 11. Appendix: Glossary

- **SamplePos**: `u64` — Absolute sample position from project start
- **TickPos**: `u64` — Musical tick (960 PPQN default)
- **TimelinePos**: `{ tick: TickPos, sample: SamplePos }` — Dual representation
- **PPQN**: Pulses Per Quarter Note (temporal resolution)
- **BWF**: Broadcast Wave Format (EBU Tech 3285)
- **CLAP**: CLever Audio Plugin (cleveraudio.org)
- **SVF**: State Variable Filter
- **RT**: Real-time (audio thread context)