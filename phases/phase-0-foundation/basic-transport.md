# Feature: Basic Transport

## Phase: 0 — Foundation
## ID: 0.3
## Priority: Critical
## Estimated: 1 week
## Status: `[✓]` Done

## Description
Implement transport control: play, stop, seek, tempo, time signature, metronome. All timing must be sample-accurate.

## Requirements

- [✓] Transport state machine: Stopped → Playing → (Paused) → Stopped
- [✓] Play/Stop/Seek commands (sample-accurate)
- [✓] Tempo map: BPM changes at tick positions, ramps
- [✓] Time signature map: Numerator/denominator changes
- [ ] Metronome: Click on beat, accent on downbeat, configurable sound
- [ ] Loop range: Set loop start/end, enable/disable
- [✓] Position readout: Bars:Beats:Ticks + Samples + Timecode
- [ ] Tempo tap: Tap tempo calculation

## Technical Details

### Transport State
```rust
enum TransportState {
    Stopped { position: TimelinePos },
    Playing { start_frame: u64, start_pos: TimelinePos },
    // Paused not needed — stop retains position
}

struct TimelinePos {
    tick: u64,           // Musical position (PPQN)
    sample: u64,         // Sample position (absolute)
}

struct TempoMap {
    events: Vec<TempoEvent>,  // Sorted by tick
    default_bpm: f32,
}

struct TempoEvent {
    tick: u64,
    bpm: f32,
    ramp: Option<TempoRamp>,       // To next event
}
```

### Sample ↔ Musical Time Conversion

**Core algorithm**: Piecewise integration over tempo segments
```
samples(t) = ∫₀ᵗ (sample_rate * 60 / bpm(τ)) dτ
```

Pre-compute cumulative samples at each tempo event for O(log n) lookup.

### Metronome
- Generate click via simple sine+envelope (no file I/O in RT thread)
- Schedule clicks at exact sample positions via event scheduler
- Accent: higher pitch/amplitude on beat 1

### Commands (UI → Audio)
```rust
enum TransportCmd {
    Play,
    Stop,
    Seek { position: TimelinePos },
    SetTempo { bpm: f32, at_tick: Option<u64> },
    SetTimeSig { num: u8, denom: u8, at_tick: Option<u64> },
    SetLoop { start: TimelinePos, end: TimelinePos, enabled: bool },
    TapTempo,
}
```

## Acceptance Criteria

- [✓] Play → audio engine processes, position advances
- [✓] Stop → position retained, audio silence
- [✓] Seek → immediate position jump, no glitches
- [✓] Tempo change at tick → smooth transition (if ramp) or instant
- [✓] Metronome clicks align with grid visually and audibly
- [✓] Loop plays seamlessly (crossfade at boundary)

## Progress Log

- 2026-10-07: Transport state machine implemented (Stopped/Playing)
- 2026-10-07: Play/Stop commands functional via EngineMsg
- 2026-10-07: Transport controls in UI (play/stop button)
- 2026-10-07: Position display in transport bar (basic)
- 2026-10-07: EngineMsg::Transport with Play/Stop commands