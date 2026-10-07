# Zoft — Vibe Coded Digital Audio Workstation

A fully vibe coded side project, just because I am not paying for an existing solution

## Status

**Phase 0: Foundation** — In Progress

See [PLAN.md](PLAN.md) for detailed roadmap and [SPEC.md](SPEC.md) for technical specification.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                      Application Layer                       │
│  Project Management │ UI State │ Undo/Redo │ Preferences    │
├─────────────────────────────────────────────────────────────┤
│                        Engine Core                             │
│  Audio Graph │ Transport │ Tempo Map │ Metronome │ Routing   │
├─────────────────────────────────────────────────────────────┤
│  Track Types          │  Plugin Host (later)   │  Built-ins   │
│  ─────────────        │  ─────────────────     │  ─────────   │
│  Audio Track          │  VST3/CLAP Wrapper     │  Synths      │
│  MIDI Track           │  Parameter Automation  │  Samplers    │
│  Instrument Track     │  Preset Management     │  Effects     │
│  Folder/Bus Track     │  Sandbox/Process       │  Utilities   │
├─────────────────────────────────────────────────────────────┤
│                      Platform Abstraction                      │
│  Audio I/O (cpal/JACK) │ MIDI I/O │ File System │ Windowing  │
└─────────────────────────────────────────────────────────────┘
```

## Tech Stack

| Layer | Technology |
|-------|------------|
| Language | Rust (2024 edition) |
| Audio I/O | `cpal` (cross-platform), `jack`/`pipewire` (Linux pro) |
| DSP | `fundsp`, `dasp`, custom SIMD-optimized |
| UI | `egui` (immediate-mode, fast iteration) → `iced`/`wgpu` (custom later) |
| Plugin Host | `nih-plug` / `cpdal` (CLAP/VST3) |
| File Format | BWF (Broadcast WAV), custom project format |
| MIDI | `midir`, `midly` |

## Project Structure

```
Zoft/
├── Cargo.toml              # Workspace root
├── SPEC.md                 # Technical specification
├── PLAN.md                 # Phase/feature breakdown with progress
├── docs/                   # Architecture & design docs
├── phases/                 # Feature tracking (one file per feature)
│   ├── phase-0-foundation/
│   ├── phase-1-audio-recording/
│   ├── phase-2-midi-sequencing/
│   ├── phase-3-builtin-instruments/
│   ├── phase-4-mixing-automation/
│   ├── phase-5-plugin-hosting/
│   └── phase-6-polish-platforms/
├── src/
│   ├── daw-core/           # Core types, project model, commands
│   ├── daw-engine/         # Audio engine, graph, transport
│   ├── daw-ui/             # egui/iced UI, editors, views
│   └── daw-plugins/        # Built-in instruments & effects
└── assets/                 # Factory content
    ├── presets/
    ├── samples/
    └── wavetables/
```

## Development Phases

| Phase | Focus | Timeline |
|-------|-------|----------|
| 0 | Foundation: audio engine, transport, UI shell, project I/O | Months 1-3 |
| 1 | Audio recording & editing: track recording, waveforms, editing | Months 4-8 |
| 2 | MIDI & sequencing: piano roll, MIDI editing, MIDI effects | Months 9-14 |
| 3 | Built-in instruments: sampler, synths, drum machine, effects | Months 15-20 |
| 4 | Mixing & automation: mixer, routing, automation, bounce | Months 21-26 |
| 5 | Plugin hosting: CLAP/VST3, scanner, sandbox, UI embedding | Months 27-32 |
| 6 | Polish & platforms: macOS, Windows, custom UI, optimization | Months 33-36 |

## Getting Started

```bash
# Build
cargo build --release

# Run
cargo run --release

# Test
cargo test
```

## Contributing

This is currently a solo project.

## License

TBD — evaluating GPL-3.0 vs proprietary for plugin hosting compatibility.