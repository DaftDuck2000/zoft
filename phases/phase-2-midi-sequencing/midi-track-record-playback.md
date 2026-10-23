# Feature: MIDI Track Record/Playback

## Phase: 2 — MIDI & Sequencing
## ID: 2.1
## Priority: Critical
## Estimated: 2 weeks
## Status: `[~]` In Progress
## Depends On: 0.2, 0.3, 1.1

## Description
Implement MIDI track: input routing, recording with chase events, loop recording, playback with sample-accurate timing.

## Requirements

- [x] MIDI keyboard input support: enumerate input ports with `midir`, choose one in the transport bar, and show live note/velocity activity
- [x] MIDI track type: extends base track with MIDI-specific fields
- [ ] Input selection: Hardware MIDI ports, virtual ports, MIDI from other tracks
- [x] Record arm: Captures MIDI to new clip on armed track
- [x] Live MIDI audition while an armed/monitored MIDI or instrument track is recording
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

- [x] MIDI keyboard input: ports are listed, one can be selected, and live note/velocity activity is shown
- [x] Record MIDI → clip appears with correct timing
- [x] MIDI clip playback through the built-in sine voice
- [x] Live MIDI notes are audible while recording or monitoring an armed MIDI/instrument track
- [x] MIDI note previews render inside clips in the arrange lane
- [x] Per-track real-time inserts (saturation, filter, bit crusher, tremolo) apply to audio and MIDI playback
- [ ] Chase sends CC/pitch bend on seek
- [ ] Loop recording: overdub merges, new take creates lane
- [ ] MIDI thru echoes to output port
- [ ] Panic sends All Notes Off + Reset Controllers
- [ ] Input quantize aligns recorded notes to grid
- [ ] Multi-port input works (multiple hardware devices)

## Progress Log

- 2026-10-23: MIDI keyboard input support started: `daw-engine/src/midi.rs` lists input ports with `midir`; a dedicated MIDI bar auto-connects the first real keyboard (skipping Midi Through), offers a selector + Refresh, and shows live note/velocity activity. Audio/MIDI tracks can be added from the track list
- 2026-10-24: MIDI recording and basic playback. Armed MIDI tracks capture timestamped note-on/off events into a `MidiClip` (ticks at PPQN 960) on stop; notes keep their offset from the playhead. MIDI blocks render in the arrange lane, can be selected/deleted (undoable) and open a read-only piano-roll view on double-click. Playback pairs note events and renders them with a simple built-in sine voice, honoring mute/solo and track volume. Instruments, chase, loop takes and MIDI thru remain
- 2026-10-24: Live MIDI audition and arrange note previews. Incoming notes are drained every frame and fed to a dedicated polyphonic sine synth in the output callback, independent of transport playback; MIDI/instrument tracks follow the Off/Auto/On monitor switch, so an armed track is heard while recording. MIDI clips now draw a miniature note lane in the arrange view using the project tempo map, so notes are visible without opening the piano roll. Real-time track inserts (saturation, filter, bit crusher, tremolo) were added to every track type and run before the fader; the old clip right-click effects entry point was removed
- Remaining: instrument/plugin output, chase events, loop recording modes, MIDI thru/panic, input quantize