# Feature: Subtractive Synthesizer

## Phase: 3 — Built-in Instruments
## ID: 3.2
## Priority: Critical
## Estimated: 3 weeks
## Status: `[ ]` Not Started
## Depends On: 3.1 (shared DSP framework)

## Description
Classic subtractive synthesizer: 2-3 oscillators, noise, ring mod, SVF filter, dual ADSR, LFOs, modulation matrix. Analog-modelled components.

## Requirements

- [ ] Oscillators (×3): Saw, Square, Triangle, Sine, PWM, Sub, Noise
- [ ] Oscillator features: Detune, unison (2-8 voices), spread, phase sync
- [ ] Oscillator mixer: Levels, ring mod (OSC1 × OSC2), noise level
- [ ] Filter: State Variable Filter (SVF) — LP/HP/BP/Notch, 12/24 dB, drive
- [ ] Filter envelope: ADSR + sustain, velocity tracking, key tracking
- [ ] Amp envelope: ADSR + hold, velocity tracking
- [ ] LFOs (×3): Sine, Tri, Saw, Square, Random, Sample&Hold, tempo sync
- [ ] Modulation matrix: 16 slots, source → destination with depth/curve
- [ ] Effects: Chorus, Phaser, Delay, Reverb (send)
- [ ] Arpeggiator: Built-in (reuse MIDI arp)
- [ ] Unison: Per-oscillator, detune, phase spread, stereo width
- [ ] MPE support: Per-note pitch bend, pressure, timbre
- [ ] Preset browser: Category, author, tags, randomizer

## Technical Details

### Oscillator Design (Anti-Aliased)
```rust
trait Oscillator: Send + Sync {
    fn process(&mut self, freq: f32, params: &OscParams, block: &mut [f32]);
    fn reset(&mut self);
}

struct BandLimitedOsc {
    // MinBLEP or DPW (Differentiated Parabolic Wave) for saw/square
    // BLIT for pulse
    phase: f32,
    phase_inc: f32,
    minblep_buffer: Vec<f32>,
}

struct WavetableOsc {
    // For sub-oscillator, custom waves
    table: Arc<Wavetable>,
    phase: f32,
}
```

### SVF Filter (Analog-Modeled)
```rust
struct SVF {
    // Chamberlin state variable filter with non-linearities
    // 1-pole stages: LP → BP → HP (for 12dB) or cascaded for 24dB
    ic1eq: f32,   // LP state
    ic2eq: f32,   // BP state
    freq: f32,    // Normalized 0..1
    resonance: f32,
    drive: f32,   // Soft saturation in feedback path
    mode: FilterMode,  // LP, HP, BP, Notch
}
```

### Voice Architecture
```rust
struct SubtractiveVoice {
    oscillators: [Oscillator; 3],
    noise: NoiseGen,
    mixer: OscMixer,
    filter: SVF,
    amp_env: ADSR,
    filter_env: ADSR,
    lfos: [LFO; 3],
    mod_matrix: ModMatrix,
    params: VoiceParams,
    note: u8,
    velocity: f32,
    active: bool,
}

struct VoiceParams {
    osc: [OscParams; 3],
    noise_level: f32,
    ring_mod: f32,
    filter: FilterParams,
    amp_env: EnvParams,
    filter_env: EnvParams,
    lfo: [LfoParams; 3],
    mod_assignments: [ModAssignment; 16],
    effects: EffectParams,
}
```

### Modulation Sources
| Source | Range | Notes |
|--------|-------|-------|
| Velocity | 0..1 | Note-on velocity |
| Key Tracking | -1..1 | Middle C = 0 |
| Mod Wheel | 0..1 | CC1 |
| Pressure | 0..1 | Channel pressure |
| Pitch Bend | -1..1 | ±2 semitones default |
| LFO 1/2/3 | -1..1 | Bipolar |
| Env 1 (Amp) | 0..1 | Unipolar |
| Env 2 (Filter) | 0..1 | Unipolar |
| Macro 1-4 | 0..1 | User assignable |

### Modulation Destinations
Osc pitch, osc level, osc pw, filter cutoff, filter res, filter drive, amp level, pan, lfo rate, lfo depth, env attack/decay, effects sends

### Unison Implementation
```rust
fn process_unison(osc: &mut dyn Oscillator, freq: f32, params: &OscParams, block: &mut [f32]) {
    if params.unison_voices <= 1 {
        osc.process(freq, params, block);
        return;
    }
    // Sum multiple detuned voices
    let detune = params.unison_detune;  // Cents
    let spread = params.unison_spread;  // 0..1 phase spread
    for v in 0..params.unison_voices {
        let voice_freq = freq * 2.0_f32.powf((v as f32 - (params.unison_voices-1) as f32 / 2.0) * detune / 1200.0);
        let voice_phase = v as f32 * spread / params.unison_voices as f32;
        // Process with phase offset
    }
}
```

## Acceptance Criteria

- [ ] 3 oscillators with all waveforms
- [ ] Band-limited: no aliasing up to Nyquist
- [ ] Unison: 8 voices, detune, stereo spread
- [ ] Ring mod: OSC1 × OSC2 audible
- [ ] SVF: LP/HP/BP/Notch, 12/24dB, resonance self-oscillates
- [ ] Filter drive adds warmth/grit
- [ ] ADSR envelopes snappy (attack < 1ms)
- [ ] LFOs tempo-sync, multiple waveforms
- [ ] Mod matrix: 16 slots, all sources/destinations
- [ ] MPE: Per-note expression works
- [ ] CPU: < 5% for 16 voices at 48kHz

## Progress Log

- YYYY-MM-DD: Band-limited oscillators
- YYYY-MM-DD: SVF filter
- YYYY-MM-DD: ADSR envelopes
- YYYY-MM-DD: LFOs + tempo sync
- YYYY-MM-DD: Modulation matrix
- YYYY-MM-DD: Unison + ring mod
- YYYY-MM-DD: Effects (chorus, phaser, delay, reverb)
- YYYY-MM-DD: MPE support
- YYYY-MM-DD: Preset browser + randomizer