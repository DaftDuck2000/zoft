# Zoft Architecture Documentation

## Overview

This document describes the high-level architecture of Zoft, a professional Digital Audio Workstation built in Rust.

## Design Principles

1. **Real-Time Safety** - Audio thread never allocates, locks, or blocks
2. **Sample-Accurate Timing** - All events scheduled at sample precision
3. **Non-Destructive Editing** - All edits are commands on immutable data
4. **Cross-Platform Abstraction** - Platform code isolated behind traits
5. **Plugin-Friendly** - Engine designed for CLAP/VST3 hosting from day one
6. **Solo-Dev Sustainable** - Composition over inheritance, minimal dependencies

## System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        APPLICATION LAYER                         │
│  Project Management │ UI State │ Undo/Redo │ Preferences        │
├─────────────────────────────────────────────────────────────────┤
│                          ENGINE CORE                             │
│  Audio Graph │ Transport │ Tempo Map │ Metronome │ Routing      │
├─────────────────────────────────────────────────────────────────┤
│  Track Types          │  Plugin Host (Phase 5)  │  Built-ins    │
│  ─────────────        │  ─────────────────      │  ─────────    │
│  Audio Track          │  CLAP/VST3 Wrapper      │  Sampler      │
│  MIDI Track           │  Parameter Automation   │  Subtractive  │
│  Instrument Track     │  Preset Management      │  Wavetable    │
│  Folder/Bus Track     │  Sandbox/Process        │  Drum Machine │
│                       │                         │  Effect Rack  │
├─────────────────────────────────────────────────────────────────┤
│                      PLATFORM ABSTRACTION                        │
│  Audio I/O (Core Audio / WASAPI / JACK) │ MIDI I/O │ Windowing │
└─────────────────────────────────────────────────────────────────┘
```

## Crate Structure

```
zoft/
├── Cargo.toml              # Workspace root
├── daw-core/               # Core types, project model, commands, undo
├── daw-engine/             # Audio engine, graph, transport, DSP
├── daw-ui/                 # UI (egui → iced/wgpu), editors, views
├── daw-plugins/            # Built-in instruments & effects
└── xtask/                  # Build scripts, codegen, release automation
```

### daw-core
- `Project`, `Track`, `Clip`, `Automation` data models
- `Command` trait + `UndoHistory` implementation
- `Preferences`, `KeyBindings`, `ColorTheme`
- Project serialization (`.zoft` binary format)
- Audio pool management

### daw-engine
- `AudioEngine`: CPAL/JACK/Core Audio/WASAPI backend
- `ProcessGraph`: Topological sort, parallel processing
- `Transport`: Play/stop/seek, tempo map, time signature
- `Scheduler`: Sample-accurate event scheduling
- `ParameterSystem`: Smooth parameter changes, automation
- `Metering`: Peak, RMS, LUFS, true peak
- DSP primitives: oscillators, filters, envelopes, delay lines

### daw-ui
- `App`: Main iced/egui application
- `TimelineView`: Arrange view with clips, ruler, playhead
- `MixerView`: Channel strips, meter bridge, routing matrix
- `PianoRollView`: Note grid, velocity, CC lanes
- `PluginEditorView`: Embedded plugin UI
- `BrowserView`: Files, plugins, presets
- `TransportBar`: Play/stop/record, tempo, position

### daw-plugins
- `Sampler`: SFZ/EXS24, multisample, velocity layers, RR
- `SubtractiveSynth`: 3 osc, SVF, ADSR, LFO, mod matrix
- `WavetableSynth`: WT import, morphing, spectral warp
- `DrumMachine`: Step seq, choke groups, P-locks
- `EffectRack`: EQ, Comp, Reverb, Delay, Sat, Limiter
- `PluginWrapper`: CLAP/VST3 hosting (Phase 5)

## Threading Model

```
┌─────────────────────────────────────────────────────────────────┐
│                        MAIN THREAD                               │
│  UI Event Loop │ Project Ops │ File I/O │ Plugin Scanner        │
└────────────────────────────┬────────────────────────────────────┘
                             │ crossbeam::channel (MPSC, bounded)
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                      AUDIO THREAD (RT)                           │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │
│  │ Input Mix   │→ │ Track Graph │→ │ Output Mix / Meters     │  │
│  └─────────────┘  └─────────────┘  └─────────────────────────┘  │
│         ▲               │                   │                    │
│         │    Lock-free  │    Lock-free      │                    │
│         │    Ring Buf   │    Ring Buf       │                    │
│         ▼               ▼                   ▼                    │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │           MESSAGE QUEUE (UI → Audio)                     │   │
│  │  Transport │ Track/Clip │ Parameter │ Plugin │ Project   │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### Message Passing
- **UI → Audio**: `crossbeam::channel::bounded(256)` - Transport, track, clip, parameter, plugin commands
- **Audio → UI**: Metering, CPU load, disk warnings (unbounded, drop-if-full)
- **Parameter changes**: Sample-accurate via `frame_offset` in message

## Data Flow

### Audio Processing
```
Hardware Input → Input Gain → Track Graph → Output Mix → Hardware Output
                     │              │              │
                     ▼              ▼              ▼
               Metering      Metering       Metering
                     │              │              │
                     └──────────────┼──────────────┘
                                    ▼
                              UI Thread (meters)
```

### MIDI Processing
```
MIDI Input → Chase Buffer → Track Graph → Instrument Plugins → Audio Output
    │              │              │
    ▼              ▼              ▼
MIDI Thru    MIDI Record    MIDI Effects
```

### Automation Processing
```
Automation Lanes → Automation Processor → Parameter Smoother → DSP Parameters
                                    (Audio Thread)
```

## Key Algorithms

### Tempo Map (Musical Time ↔ Sample Time)
- Piecewise integration over tempo segments
- Pre-computed cumulative samples at each tempo event
- O(log n) lookup via binary search

### Waveform Rendering
- Overview: Min/max per pixel column (pre-computed, stored in project)
- Detail: On-demand decode at zoom > 1:1
- GPU: Texture atlas of overviews, compute shader for real-time zoom

### Quantization
- Grid-based with strength (0-100%), swing, tuplets
- Applied per-event in musical tick space

### Automation Curves
- Linear, Exponential, Logarithmic, S-Curve, Step
- Interpolated per-block in audio thread
- Thinning: Douglas-Peucker for recorded automation

### Voice Stealing (Sampler/Synths)
- Priority: Sustain pedal > Volume > Age
- Max polyphony configurable (default 64)

## Plugin Hosting (Phase 5+)

### Sandbox Architecture
```
Host Process                    Plugin Process (per plugin)
┌──────────────────┐           ┌──────────────────┐
│ PluginInstance   │◀──IPC──▶│ PluginWrapper    │
│ SharedMemPool    │           │ CLAP/VST3 Entry  │
│ ProcessMonitor   │           │ Audio Buffers    │
└──────────────────┘           └──────────────────┘
```

- Shared memory for audio buffers, parameters, events
- IPC for state save/load, UI creation
- Crash detection → auto-restart with state restore
- 3 max restarts, then disable

## Performance Targets

| Metric | Target |
|--------|--------|
| Audio callback latency | < 2ms @ 48kHz/256 |
| DSP CPU (empty) | < 1% single core |
| DSP CPU (50 tracks + plugins) | < 50% single core |
| UI frame time | < 16ms (60 FPS) |
| Project load (100 tracks) | < 3s |
| Waveform overview | < 1s per minute audio |

## Testing Strategy

| Level | Tools | Coverage |
|-------|-------|----------|
| Unit | `cargo test` | 80% core modules |
| Integration | `cargo test --test integration` | Key workflows |
| Audio | `dasample` test signals | Regression detection |
| UI | `iced` test utils | Smoke tests |
| Plugin | `clap-validator`, `vst3_test_host` | Compatibility |

## Future Extensibility

- **Scripting**: Lua via `mlua` for macros/extensions
- **Video**: Sync to video file (MP4, seek via ffmpeg)
- **Cloud**: Project sharing, collaborative editing (CRDT)
- **Hardware**: MCU/HUI protocol for control surfaces
- **Mobile**: iOS/iPadOS via `tauri` or native