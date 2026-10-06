# Feature: MIDI Effect Plugins

## Phase: 2 — MIDI & Sequencing
## ID: 2.4
## Priority: High
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 2.1

## Description
Built-in MIDI effects that process MIDI in real-time: arpeggiator, chord generator, velocity curve, note filter, transpose, randomizer. Inserted per-track before instrument.

## Requirements

- [ ] Arpeggiator: Up/Down/Up+Down/Random/Order/Chord modes, rate, octave range, latch
- [ ] Chord Generator: Single note → chord (major, minor, 7th, extensions, custom)
- [ ] Velocity Curve: Remap input velocity (compress, expand, fixed, random)
- [ ] Note Filter: Pass/block by note range, velocity range, channel
- [ ] Transpose: Fixed semitone offset, scale-aware option
- [ ] Randomizer: Note probability, velocity randomization, timing jitter
- [ ] Note Repeater: Delay repeats with feedback, decay, pitch shift
- [ ] MIDI Monitor: Display incoming/outgoing events (debug)
- [ ] Effect chain: Multiple MIDI effects per track, reorderable
- [ ] Bypass per effect, global MIDI FX bypass

## Technical Details

### MIDI Effect Trait
```rust
trait MidiEffect: Send + Sync {
    fn name(&self) -> &str;
    fn id(&self) -> EffectId;
    fn process(&mut self, events: &mut [MidiEvent], ctx: &MidiEffectContext) -> Vec<MidiEvent>;
    fn reset(&mut self);
    fn params(&self) -> Vec<ParamInfo>;
    fn set_param(&mut self, id: ParamId, value: f32);
    fn get_param(&self, id: ParamId) -> f32;
}

struct MidiEffectContext {
    sample_rate: f64,
    block_size: usize,
    current_tick: u64,
    tempo: f32,
    time_signature: (u8, u8),
    transport_state: TransportState,
}
```

### Arpeggiator
```rust
struct Arpeggiator {
    mode: ArpMode,          // Up, Down, UpDown, DownUp, Random, Order, Chord
    rate: NoteValue,        // 1/4, 1/8, 1/16, 1/32, triplet, dotted
    octave_range: u8,       // 1-4
    latch: bool,            // Hold notes after key release
    gate: f32,              // Note length % of step (0.1-1.0)
    swing: f32,             // 0.0-1.0
    held_notes: Vec<HeldNote>,
    step_counter: u32,
    last_tick: u64,
}

struct HeldNote {
    note: u8,
    velocity: u8,
    channel: u8,
    tick_pressed: u64,
}

impl MidiEffect for Arpeggiator {
    fn process(&mut self, events: &mut [MidiEvent], ctx: &MidiEffectContext) -> Vec<MidiEvent> {
        // 1. Collect note on/off from input events → update held_notes
        // 2. If transport playing, advance step_counter based on tempo/rate
        // 3. Generate output events from held_notes per mode
        // 4. Pass through non-note events (CC, pitch bend, etc.)
    }
}
```

### Chord Generator
```rust
struct ChordGenerator {
    chord_type: ChordType,  // Major, Minor, Dom7, Maj7, Min7, Dim, Aug, Sus2, Sus4, Custom
    voicing: Voicing,       // Close, Open, Drop2, Drop3, Custom
    strum: f32,             // Strum delay (ms)
    transpose: i8,          // Semitones
    custom_intervals: Vec<i8>, // For custom chord type
}
```

### Effect Chain on Track
```rust
struct MidiTrack {
    // ... existing fields
    midi_effects: Vec<Box<dyn MidiEffect>>,  // Ordered chain
    midi_fx_bypass: bool,
}
```

### Parameter Automation
- All MIDI effect parameters automatable
- Show in automation lane list
- Record/write automation like plugin params

## Acceptance Criteria

- [ ] Arpeggiator plays held notes in time with tempo
- [ ] Modes: Up/Down/UpDown/Random/Order/Chord all work
- [ ] Latch holds notes after release
- [ ] Chord generator produces correct intervals
- [ ] Velocity curve remaps smoothly
- [ ] Note filter passes/blocks correctly
- [ ] Effects chain processes in order
- [ ] Bypass works per-effect and globally
- [ ] Parameters automatable

## Progress Log

- YYYY-MM-DD: MidiEffect trait + chain infrastructure
- YYYY-MM-DD: Arpeggiator (core logic)
- YYYY-MM-DD: Arpeggiator modes + latch
- YYYY-MM-DD: Chord generator
- YYYY-MM-DD: Velocity curve
- YYYY-MM-DD: Note filter
- YYYY-MM-DD: Transpose + Randomizer
- YYYY-MM-DD: Note repeater
- YYYY-MM-DD: MIDI monitor
- YYYY-MM-DD: UI: Effect chain panel + param controls