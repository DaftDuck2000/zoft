# Feature: Drum Machine / Sampler

## Phase: 3 — Built-in Instruments
## ID: 3.4
## Priority: High
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 3.1

## Description
Drum-focused sampler with step sequencer, pad-based UI, choke groups, per-pad effects, pattern chaining. MPC/Elektron-style workflow.

## Requirements

- [ ] Pad grid: 16/32/64 pads, bank switching
- [ ] Sample per pad: Drag-drop load, multi-sample layers (velocity)
- [ ] Step sequencer: 16/32/64 steps, per-pad pattern length
- [ ] Pattern chaining: Song mode, pattern length, time signature
- [ ] Choke groups: Mutually exclusive pads (hi-hats)
- [ ] Per-pad: Pitch, decay, filter, pan, volume, send
- [ ] Per-pad effects: Bitcrusher, distortion, filter, compressor
- [ ] Probability: Per-step trigger chance
- [ ] Ratchet/Repeats: Sub-step repeats (trap style)
- [ ] Swing: Global + per-track
- [ ] MIDI mapping: Pads to notes, CCs to parameters
- [ ] Pad perform mode: Velocity-sensitive, repeat, note repeat
- [ ] Export: Pattern → MIDI clip, Audio bounce

## Technical Details

### Drum Machine Structure
```rust
struct DrumMachine {
    pads: [Pad; 64],              // Fixed 64 pads (4 banks × 16)
    patterns: Vec<Pattern>,
    current_pattern: PatternId,
    song_mode: Option<SongMode>,
    global_swing: f32,
    tempo_sync: bool,
}

struct Pad {
    sample: Option<SampleRef>,    // Primary sample
    layers: Vec<PadLayer>,        // Velocity layers
    choke_group: Option<u8>,      // 0 = none, 1-16 = group
    pitch: f32,                   // Semitones
    decay: f32,                   // 0..1 (envelope decay)
    filter: PadFilter,
    pan: f32,
    volume: f32,
    sends: [f32; 2],              // Reverb, Delay
    effects: PadEffects,
    midi_note: u8,                // Output note
    color: Color,
}

struct PadLayer {
    sample: SampleRef,
    velocity_range: RangeInclusive<u8>,
    probability: f32,             // 0..1
}

struct PadFilter {
    enabled: bool,
    freq: f32,
    resonance: f32,
    mode: FilterMode,
    env_amount: f32,
}

struct PadEffects {
    bitcrusher: Option<BitcrusherParams>,
    distortion: Option<DistortionParams>,
    compressor: Option<CompressorParams>,
}
```

### Step Sequencer
```rust
struct Pattern {
    length: u16,                  // Steps (16, 32, 64, custom)
    resolution: u8,               // Steps per beat (4, 8, 16)
    time_sig: (u8, u8),           // e.g., 4/4, 3/4, 7/8
    tracks: [Track; 64],          // One per pad
    swing: f32,                   // Override global
}

struct Track {
    steps: Vec<Step>,             // Length = pattern.length
    mute: bool,
    solo: bool,
}

struct Step {
    triggered: bool,
    velocity: u8,                 // 1-127
    probability: f32,             // 0..1
    ratchet: u8,                  // 1 = normal, 2-8 = repeats
    micro_timing: i8,             // -127..127 (ticks offset)
    parameter_locks: HashMap<ParamId, f32>,  // P-locks
}
```

### Choke Groups
```rust
fn process_choke(voice_manager: &mut VoiceManager, pad_id: usize, new_voice: Voice) {
    if let Some(group) = voice_manager.pads[pad_id].choke_group {
        // Stop all voices in same choke group
        for (other_id, pad) in voice_manager.pads.iter().enumerate() {
            if other_id != pad_id && pad.choke_group == Some(group) {
                voice_manager.stop_voices_for_pad(other_id);
            }
        }
    }
    voice_manager.start_voice(pad_id, new_voice);
}
```

### Parameter Locks (P-Locks)
- Per-step parameter overrides (filter cutoff, pitch, decay, etc.)
- Stored in `Step.parameter_locks`
- Applied during step trigger
- Smooth interpolation between locks (optional)

### Pattern Chaining (Song Mode)
```rust
struct SongMode {
    chain: Vec<PatternChainEntry>,
    current_entry: usize,
    loop_range: Option<Range<usize>>,
}

struct PatternChainEntry {
    pattern: PatternId,
    repeats: u16,                 // 1-999
    transpose: i8,                // Semitones
    mute_groups: u64,             // Bitmask of muted pads
}
```

## Acceptance Criteria

- [ ] 64 pads, 4 banks, drag-drop samples
- [ ] Step sequencer: 16/32/64 steps, variable length
- [ ] Choke groups work (open/closed hat)
- [ ] Per-pad effects sound good
- [ ] Probability + ratchet create variation
- [ ] Swing feels musical
- [ ] Pattern chaining → song arrangement
- [ ] P-lock automation per step
- [ ] Export pattern as MIDI clip
- [ ] Pad perform mode: velocity, repeat, note repeat

## Progress Log

- YYYY-MM-DD: Pad grid + sample loading
- YYYY-MM-DD: Step sequencer core
- YYYY-MM-DD: Choke groups
- YYYY-MM-DD: Per-pad parameters + effects
- YYYY-MM-DD: Probability + ratchet + micro-timing
- YYYY-MM-DD: Parameter locks
- YYYY-MM-DD: Pattern chaining (song mode)
- YYYY-MM-DD: Swing + MIDI mapping
- YYYY-MM-DD: Pad perform mode
- YYYY-MM-DD: Export to MIDI/audio