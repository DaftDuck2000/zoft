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
- [~] Input selection: Hardware source selection and channel filter implemented; virtual ports and MIDI from other tracks pending
- [x] MIDI channel filter: All or channels 1–16
- [x] Record arm: Captures MIDI to new clip on armed track
- [x] MIDI clip trimming: Drag either edge in the arrange view; hidden notes are preserved
- [x] MIDI clip moving: Drag the body in time or onto another MIDI/instrument track
- [x] MIDI clip split/duplicate: `S` splits at the playhead, `Ctrl+D` duplicates the visible span
- [x] MIDI multi-selection: click, Ctrl/Shift+click, marquee selection, group move/delete/duplicate/copy/paste/mute
- [x] Mixed audio/MIDI selection: both sets can be selected and moved together
- [x] Live MIDI audition while an armed/monitored MIDI or instrument track is recording
- [~] Chase events: CC10 pan, CC7 volume and CC11 expression follow the seek position; pitch bend/program/other CC pending
- [ ] Loop recording: Overdub (merge) or new take (lanes)
- [ ] Metronome count-in: Configurable bars before recording
- [x] MIDI thru: Echo input to the selected output port (for monitoring external gear)
- [x] MIDI panic: All notes off, clear scheduled playback notes
- [x] Input quantize: Recorded notes align to the Arrange snap grid when Input Q is enabled

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
- [x] MIDI input channel filter can restrict recording and audition to one channel
- [x] Record MIDI → clip appears with correct timing
- [x] MIDI clip playback through the built-in sine voice
- [x] Live MIDI notes are audible while recording or monitoring an armed MIDI/instrument track, and follow that track's fader and pan
- [x] CC10 Pan events affect MIDI note playback
- [x] MIDI note previews render inside clips in the arrange lane
- [x] MIDI clips can be cropped from either edge; trimming is non-destructive
- [x] MIDI clips can be dragged in time or onto another MIDI/instrument track
- [x] MIDI clips can be split at the playhead and duplicated
- [x] MIDI clips support the same multi-selection and group editing as audio clips
- [x] Audio and MIDI selections can be combined and moved together
- [x] Per-track real-time inserts (saturation, filter, bit crusher, tremolo) apply to audio and MIDI playback
- [~] Chase sends CC/pitch bend on seek (CC10/CC7/CC11 implemented)
- [ ] Loop recording: overdub merges, new take creates lane
- [x] MIDI thru echoes to the selected output port
- [x] Panic sends All Notes Off and clears scheduled MIDI playback
- [x] Input quantize aligns recorded notes to grid
- [ ] Multi-port input works (multiple hardware devices)

## Progress Log

- 2026-10-23: MIDI keyboard input support started: `daw-engine/src/midi.rs` lists input ports with `midir`; a dedicated MIDI bar auto-connects the first real keyboard (skipping Midi Through), offers a selector + Refresh, and shows live note/velocity activity. Audio/MIDI tracks can be added from the track list
- 2026-10-24: MIDI recording and basic playback. Armed MIDI tracks capture timestamped note-on/off events into a `MidiClip` (ticks at PPQN 960) on stop; notes keep their offset from the playhead. MIDI blocks render in the arrange lane, can be selected/deleted (undoable) and open a read-only piano-roll view on double-click. Playback pairs note events and renders them with a simple built-in sine voice, honoring mute/solo and track volume. Instruments, chase, loop takes and MIDI thru remain
- 2026-10-24: Live MIDI audition and arrange note previews. Incoming notes are drained every frame and fed to a dedicated polyphonic sine synth in the output callback, independent of transport playback; MIDI/instrument tracks follow the Off/Auto/On monitor switch, so an armed track is heard while recording. MIDI clips now draw a miniature note lane in the arrange view using the project tempo map, so notes are visible without opening the piano roll. Real-time track inserts (saturation, filter, bit crusher, tremolo) were added to every track type and run before the fader; the old clip right-click effects entry point was removed
- 2026-10-29: Live MIDI audition now follows the active MIDI/instrument track's fader and pan, so panning a monitored track is audible while playing live as well as during playback
- 2026-10-29: MIDI playback now reads CC10 Pan from clip events; notes are panned by the latest CC10 value at their start
- 2026-10-30: CC chase for MIDI playback. CC10 pan, CC7 volume and CC11 expression are sampled at the seek position for notes already sounding, so seeking into a CC curve restores the correct state. CC7 and CC11 scale note gain
- 2026-10-30: Added a MIDI Panic button in the MIDI bar. It clears the live synth and scheduled playback notes to stop hanging MIDI
- 2026-10-30: Added optional input quantization. With Input Q enabled, recorded notes are aligned to the Arrange snap grid while preserving their durations
- 2026-10-30: Moved all MIDI options into a top-bar MIDI menu. Source is a second-level menu that opens on hover, with Refresh, Panic, Input Quantize and live activity inside; the current input note remains visible next to the menu
- 2026-10-30: Added a MIDI channel filter (All or 1–16). Filtered input is used for both recording and live audition
- 2026-10-30: Added MIDI thru. The MIDI menu can select an output port and echo filtered input notes to it; Panic sends All Notes Off to the output as well
- 2026-10-24: MIDI clip cropping. Dragging either edge of a MIDI clip in the arrange view trims its visible window using the Arrange snap grid. Trimming is non-destructive: events stay in content coordinates and a `content_offset` marks the visible start, so dragging the edge back out reveals the hidden notes again. The piano roll shades trimmed regions and keeps hidden notes editable
- 2026-10-24: MIDI clip moving. Dragging a MIDI clip's body moves it in time using the Arrange snap settings (including relative snap), and dropping it on another MIDI/instrument track reattaches it there. Moves are undoable and update playback immediately
- 2026-10-24: MIDI clip split and duplicate. `S` splits the selected MIDI clip at the playhead into two non-destructive visible windows, and `Ctrl+D` duplicates the clip immediately after its visible span. Both operations are undoable
- 2026-10-24: MIDI clip multi-selection. MIDI clips now use the same selection model as audio: click, Ctrl/Shift+click and marquee selection build a set, with group move, delete, duplicate, copy/paste and mute. Group moves preserve relative time offsets and can shift across MIDI/instrument tracks together. Audio and MIDI selections are no longer exclusive, so a mixed selection can be moved as one group
- Remaining: instrument/plugin output, full chase events, loop recording modes, MIDI thru