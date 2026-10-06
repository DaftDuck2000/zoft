# Feature: Sampler (SFZ/EXS24)

## Phase: 3 — Built-in Instruments
## ID: 3.1
## Priority: Critical
## Estimated: 4 weeks
## Status: `[ ]` Not Started
## Depends On: 0.2, 1.1

## Description
Professional sampler supporting SFZ 2.0 and EXS24 formats. Multisample mapping, velocity layers, round-robin, loop modes, per-zone envelopes/filters.

## Requirements

- [ ] SFZ 2.0 parser: regions, groups, opcodes (sample, key, velocity, loop, envelope, filter, effects)
- [ ] EXS24 parser: zones, groups, samples, parameters
- [ ] Sample loading: Background thread, streaming for large samples
- [ ] Voice management: Polyphony limit, voice stealing (oldest, quietest, lowest priority)
- [ ] Per-zone: Pitch/key tracking, velocity range, round-robin, random
- [ ] Loop modes: Forward, backward, alternate, ping-pong, sustain loop
- [ ] Envelopes: AHDSR (per zone + global), per-zone override
- [ ] Filters: SVF (LP/HP/BP/Notch) per zone + global, key tracking
- [ ] LFOs: Multiple, sync to tempo, multiple waveforms
- [ ] Modulation matrix: Source → Destination with depth, curve
- [ ] Effects sends: Built-in reverb/delay sends
- [ ] Disk streaming: Large samples stream from disk (not RAM)
- [ ] Preset browser: Category, search, favorites, tags

## Technical Details

### SFZ 2.0 Opcode Support (Priority)
| Category | Opcodes |
|----------|---------|
| **Region** | `sample`, `pitch_keycenter`, `lokey`/`hikey`, `lovel`/`hivel`, `seq_position`/`seq_length` (RR) |
| **Loop** | `loop_mode`, `loop_start`, `loop_end`, `loop_crossfade` |
| **Envelope** | `ampeg_attack`/`hold`/`decay`/`sustain`/`release`, `fileg_*` (filter) |
| **Filter** | `fil_type` (lpf/hpf/bpf/notch), `cutoff`, `resonance`, `fil_keytrack` |
| **Pitch** | `pitch_keytrack`, `tune`, `transpose` |
| **Amplitude** | `volume`, `amplitude`, `pan` |
| **Effects** | `effect1`/`effect2` (reverb/delay send) |

### Architecture
```rust
struct Sampler {
    instruments: HashMap<InstrumentId, Instrument>,
    voice_pool: VoicePool,
    sample_cache: SampleCache,      // LRU cache of decoded samples
    stream_handles: HashMap<SampleId, StreamHandle>,
}

struct Instrument {
    name: String,
    regions: Vec<Region>,
    global_params: GlobalParams,
    modulation_matrix: ModMatrix,
}

struct Region {
    sample: SampleRef,
    key_range: RangeInclusive<u8>,
    vel_range: RangeInclusive<u8>,
    rr_group: Option<u8>,           // Round-robin group
    loop_mode: LoopMode,
    loop_start: u64,
    loop_end: u64,
    envelope: EnvelopeParams,       // AHDSR
    filter: FilterParams,
    pitch: PitchParams,
    amplitude: AmplitudeParams,
    modulation: Vec<ModAssignment>,
}

enum LoopMode {
    NoLoop,
    Forward,
    Backward,
    Alternate,
    SustainForward,
    SustainAlternate,
}

struct Voice {
    region: RegionId,
    sample_pos: f64,                // Fractional for pitch shift
    envelope: EnvelopeState,
    filter: FilterState,
    lfos: Vec<LfoState>,
    modulation: ModState,
    active: bool,
    priority: f32,                  // For voice stealing
}
```

### Voice Stealing Algorithm
```rust
fn steal_voice(pool: &mut VoicePool, new_priority: f32) -> Option<VoiceId> {
    // Priority: 1. Sustain pedal held, 2. Volume, 3. Age
    pool.voices.iter()
        .filter(|v| v.active)
        .min_by_key(|v| steal_priority(v, new_priority))
        .map(|v| v.id)
}

fn steal_priority(voice: &Voice, new_priority: f32) -> u64 {
    let sustain_bonus = if voice.sustain_pedal { 1_000_000 } else { 0 };
    let volume = (voice.current_amplitude * 1000.0) as u64;
    let age = voice.start_time.elapsed().as_millis() as u64;
    sustain_bonus + volume + age
}
```

### Sample Cache & Streaming
- **Cache**: LRU, max 2 GB (configurable), stores decoded float samples
- **Streaming**: For samples > 10 MB, read chunks from disk via `std::fs::File` + `seek`
- **Background loading**: `tokio` task pool for async sample loading

### Modulation Matrix
```rust
struct ModAssignment {
    source: ModSource,
    destination: ModDest,
    depth: f32,          // -1.0 to 1.0
    curve: ModCurve,     // Linear, Exp, Log, S
}

enum ModSource {
    Velocity,
    KeyTracking,
    ModWheel,
    Pressure,
    PitchBend,
    LFO(u8),
    Envelope(u8),        // Amp, Filter, Aux
    Macro(u8),           // User-assignable macros
    Random,
}

enum ModDest {
    Pitch,
    Cutoff,
    Resonance,
    Volume,
    Pan,
    LFO_Freq(u8),
    LFO_Depth(u8),
    Env_Attack(u8),
    Env_Decay(u8),
    // ...
}
```

## Acceptance Criteria

- [ ] Loads SFZ files (VSCO2, Salamander, free libraries)
- [ ] Loads EXS24 (Logic factory library)
- [ ] Velocity layers switch correctly
- [ ] Round-robin cycles on repeated notes
- [ ] Loops play seamlessly (crossfade)
- [ ] Envelopes shape amplitude/filter
- [ ] Filter keytracking works
- [ ] Mod matrix routes sources to destinations
- [ ] 64+ voice polyphony at 48kHz
- [ ] Disk streaming works for 1 GB+ libraries
- [ ] Preset browser filters by category

## Progress Log

- YYYY-MM-DD: SFZ parser (core opcodes)
- YYYY-MM-DD: EXS24 parser
- YYYY-MM-DD: Voice engine + stealing
- YYYY-MM-DD: Envelopes + filters
- YYYY-MM-DD: Loop modes
- YYYY-MM-DD: Modulation matrix
- YYYY-MM-DD: Sample cache + streaming
- YYYY-MM-DD: Preset browser UI
- YYYY-MM-DD: Factory library integration