# Feature: MIDI Track Record/Playback

## Phase: 2 — MIDI & Sequencing
## ID: 2.1
## Priority: Critical
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 0.2, 0.3, 1.1

## Description
Implement MIDI track: input routing, recording with chase events, loop recording, playback with sample-accurate timing.

## Requirements

- [ ] MIDI track type: extends base track with MIDI-specific fields
- [ ] Input selection: Hardware MIDI ports, virtual ports, MIDI from other tracks
- [ ] Record arm: Captures MIDI to new clip on armed track
- [ ] Chase events: Send pending CC/pitch bend/program change on seek/play
- [ ] Loop recording: Overdub (merge) or new take (lanes)
- [ ] Metronome count-in: Configurable bars before recording
- [ ] MIDI thru: Echo input to output (for monitoring external gear)
- [ ] MIDI panic: All notes off, reset controllers
- [ ] Input quantize: Optional quantization during recording

## Technical Details

### MIDI Track Model
```rust
struct MidiTrack {
    base: TrackBase,
    input: MidiInput,
    output: MidiOutput,
    channel: Option<u8>,              // Output channel (None = omit)
    program: Option<u8>,              // Program change on play
    bank_select: Option<(u8, u8)>,    // MSB/LSB
    chase_enabled: bool,
    thru_enabled: bool,
}

enum MidiInput {
    Hardware { port_id: PortId, channel_mask: u16 },  // 16-bit mask
    Virtual { name: String },
    Track { track_id: TrackId },                       // MIDI from another track
    None,
}

enum MidiOutput {
    Hardware { port_id: PortId, channel: u8 },
    Virtual { name: String },
    Plugin { plugin_id: PluginId },                    // To instrument plugin
    Bus { bus_id: BusId },                             // MIDI bus
    None,
}
```

### MIDI Event Model
```rust
struct MidiEvent {
    tick: u64,                    // Musical tick (PPQN)
    message: MidiMessage,
}

enum MidiMessage {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8, velocity: u8 },
    PolyPressure { channel: u8, note: u8, pressure: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
    ProgramChange { channel: u8, program: u8 },
    ChannelPressure { channel: u8, pressure: u8 },
    PitchBend { channel: u8, value: u16 },  // 0..16383 (8192 = center)
    SysEx(Vec<u8>),
    Meta(MetaMessage),            // Tempo, time sig, marker (track-local)
}
```

### Chase Events Algorithm
```rust
fn chase_events(events: &[MidiEvent], seek_tick: u64) -> Vec<MidiMessage> {
    let mut state = ChaseState::default();
    for event in events {
        if event.tick > seek_tick { break; }
        state.apply(&event.message);
    }
    state.pending_messages()  // CCs, pitch bend, program, channel pressure
}
```

### Recording Pipeline
```
MIDI Input (midir)
    │
    ▼
┌─────────────────────────────────────┐
│ Input Filter (channel, message type)│
│ Timestamp (sample-accurate)         │
└─────────────────────────────────────┘
    │
    ├──────────────────┬──────────────────┐
    ▼                  ▼                  ▼
MIDI Thru         Record Buffer      Chase Buffer
(if enabled)      (per take)         (for seek)
```

### Loop Recording Modes
- **Overdub**: Merge new events into existing clip (extend length if needed)
- **New Take**: Create new clip on new lane, mute previous
- **Replace**: Delete existing clip in loop range, record new

## Acceptance Criteria

- [ ] Record MIDI → clip appears with correct timing
- [ ] Chase sends CC/pitch bend on seek
- [ ] Loop recording: overdub merges, new take creates lane
- [ ] MIDI thru echoes to output port
- [ ] Panic sends All Notes Off + Reset Controllers
- [ ] Input quantize aligns recorded notes to grid
- [ ] Multi-port input works (multiple hardware devices)

## Progress Log

- YYYY-MM-DD: MIDI track model
- YYYY-MM-DD: Input/output routing
- YYYY-MM-DD: Recording with timestamping
- YYYY-MM-DD: Chase events
- YYYY-MM-DD: Loop recording modes
- YYYY-MM-DD: MIDI thru + panic
- YYYY-MM-DD: Input quantize