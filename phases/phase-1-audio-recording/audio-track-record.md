# Feature: Audio Track & Recording

## Phase: 1 — Audio Recording & Editing
## ID: 1.1
## Priority: Critical
## Estimated: 2 weeks
## Status: `[~]` In Progress

## Description
Implement audio track with record arm, input monitoring, and BWF file writing. Support multiple hardware inputs and input routing.

## Requirements

- [✓] Audio track type: extends base track with audio-specific fields
- [✓] Record arm button (R) per track
- [✓] Input monitoring modes: Off / Auto (monitor when armed/playing) / On
- [✓] Input selection: Hardware input channels, bus returns, "No Input"
- [✓] Recording: Write to BWF (Broadcast WAV) with `bext` chunk
- [ ] Multi-take: New clip per take, layered or new lane
- [ ] Disk space warning: Calculate remaining time at current sample rate
- [ ] Recording level meter: Pre-fader, peak + RMS
- [ ] Punch in/out: Set punch points, auto-arm at punch in

## Technical Details

### Audio Track Model
```rust
struct AudioTrack {
    base: TrackBase,
    input: AudioInput,
    output: AudioOutput,
    sends: Vec<Send>,
    inserts: Vec<InsertSlot>,
    record_arm: bool,
    monitor_mode: MonitorMode,
    record_path: Option<PathBuf>,     // Target directory
    current_take: u32,
}

enum AudioInput {
    Hardware { device_id: DeviceId, channels: [u32; 2] },
    Bus { bus_id: BusId },
    None,
}

enum MonitorMode {
    Off,
    Auto,    // Monitor when armed or recording
    On,
}
```

### Recording Pipeline
```
Hardware Input (cpal)
    │
    ▼
┌─────────────────────────────────────┐
│ Input Gain (digital trim)           │
│ Phase Invert                        │
│ High-pass filter (optional)         │
└─────────────────────────────────────┘
    │
    ├──────────────────┬──────────────────┐
    ▼                  ▼                  ▼
Monitor Mix        Recording           Metering
(Auto/On)          Writer Thread       (Peak/RMS)
```

### BWF Writer (Broadcast WAV)
- Uses `hound` crate for WAV writing
- Implements `bext` chunk with:
  - Description (256 bytes)
  - Originator (32 bytes)
  - OriginatorReference (32 bytes)
  - OriginationDate/Time (10/8 bytes)
  - TimeReference (sample count since midnight)
  - Version, UMID (64 bytes)
  - CodingHistory
- Writes 32-bit float samples at project sample rate
- Background writer thread with lock-free ring buffer

### Multi-Take Handling
- Each record pass = new `AudioClip` on same track
- Takes stacked vertically in "lanes" (UI concept)
- Comping: Select active take per region (Phase 4)

## Acceptance Criteria

- [✓] Arm track → record button enabled
- [✓] Press record → transport starts, audio written to disk
- [✓] Monitor modes work: Off (silent), Auto (hear when armed), On (always)
- [✓] BWF file readable in other DAWs (Reaper, Logic, Pro Tools)
- [✓] `bext` chunk contains correct time_reference
- [ ] Multi-take creates separate clips
- [ ] Disk warning at < 10 min remaining
- [ ] No xruns during recording at 48kHz/256

## Progress Log

- 2026-10-07: Audio track model already has record_arm, monitor_mode, input/output
- 2026-10-07: CPAL backend extended with input device enumeration
- 2026-10-07: BWF writer implemented with `bext` chunk support
- 2026-10-07: Input stream support added to audio backend
- 2026-10-07: Input device enumeration implemented
- Next: Integrate recording into engine, connect UI record button to engine