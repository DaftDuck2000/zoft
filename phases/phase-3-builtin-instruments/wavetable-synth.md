# Feature: Wavetable Synthesizer

## Phase: 3 — Built-in Instruments
## ID: 3.3
## Priority: High
## Estimated: 3 weeks
## Status: `[ ]` Not Started
## Depends On: 3.2

## Description
Modern wavetable synthesizer: WT import (Serum, Vital, Wave), wavetable morphing, spectral processing, unison, advanced modulation. Serum/Vital-style workflow.

## Requirements

- [ ] Wavetable oscillator: 256/512/1024 sample tables, interpolation
- [ ] Wavetable import: Serum (.wavetable), Vital (.vitaltable), Wave (.wav), custom
- [ ] WT position modulation: Smooth morphing between frames
- [ ] Spectral processing: FFT-based warp (FM, AM, Sync, Bend, Mirror, Quantize)
- [ ] Sub oscillator: Sine, Triangle, Saw, Square, Noise
- [ ] Noise oscillator: White, Pink, Brown, Sample-based
- [ ] Filter: Multiple types (Analog, Digital, Vowel, Comb, Phaser, etc.)
- [ ] Envelopes: ADSR + Hold, multiple stages (DAHDSR), loop modes
- [ ] LFOs: Shapes, tempo sync, phase, randomize, envelope mode
- [ ] Modulation matrix: Drag-and-drop, macro knobs, MPE
- [ ] Effects rack: 8 slots, serial/parallel, per-voice/global
- [ ] Unison: Up to 16 voices, detune, phase, stack, spread
- [ ] Wavetable editor: Draw, import, process, morph (stretch goal)

## Technical Details

### Wavetable Structure
```rust
struct Wavetable {
    frames: Vec<WavetableFrame>,  // Each frame = one cycle
    frame_size: usize,            // 256, 512, 1024, 2048
    name: String,
    metadata: WTMetadata,
}

struct WavetableFrame {
    samples: Vec<f32>,            // One cycle, normalized
    harmonic_profile: Vec<f32>,   // For spectral display
}

struct WavetableOsc {
    wavetable: Arc<Wavetable>,
    position: f32,                // 0.0 .. (frames-1) fractional
    phase: f32,
    phase_inc: f32,
    // For morphing: interpolate between floor/ceil frames
}
```

### Wavetable Morphing
```rust
fn process_wavetable(osc: &mut WavetableOsc, freq: f32, pos_mod: f32, block: &mut [f32]) {
    let frame_count = osc.wavetable.frames.len() as f32;
    let clamped_pos = (osc.position + pos_mod).clamp(0.0, frame_count - 1.0001);
    let frame_idx = clamped_pos.floor() as usize;
    let frac = clamped_pos.fract();
    
    let frame_a = &osc.wavetable.frames[frame_idx];
    let frame_b = &osc.wavetable.frames[(frame_idx + 1).min(frame_count as usize - 1)];
    
    // High-quality interpolation (cubic Hermite or spectral)
    for sample in block {
        let phase = osc.phase;
        let idx = phase * frame_a.samples.len() as f32;
        let a = interpolate_frame(frame_a, idx);
        let b = interpolate_frame(frame_b, idx);
        *sample = a + (b - a) * frac;
        osc.phase += osc.phase_inc;
        if osc.phase >= 1.0 { osc.phase -= 1.0; }
    }
}
```

### Spectral Warp Modes (FFT-based)
```rust
enum SpectralWarp {
    None,
    FM { amount: f32 },           // Frequency modulation within frame
    AM { amount: f32 },           // Amplitude modulation
    Sync { ratio: f32 },          // Hard sync simulation
    Bend { amount: f32 },         // Spectral pitch bend
    Mirror,                       // Reverse harmonics
    Quantize { steps: u32 },      // Harmonic quantization
    Formant { shift: f32 },       // Formant shifting
    Alias { fold: f32 },          // Digital aliasing
}
```

### Import Formats
| Format | Extension | Notes |
|--------|-----------|-------|
| Serum | `.wavetable` | JSON metadata + binary frames |
| Vital | `.vitaltable` | JSON + binary |
| Wave | `.wav` | Single cycle or multi-cycle (auto-detect) |
| Ableton | `.adv` | Simpler format |
| Custom | `.zoftwt` | Native format with metadata |

### Unison (Wavetable-Specific)
```rust
struct UnisonParams {
    voices: u8,           // 1-16
    detune: f32,          // Cents per voice
    phase_spread: f32,    // 0..1
    wt_spread: f32,       // Wavetable position spread
    stack: StackMode,     // Octave, Fifth, Custom
}
```

## Acceptance Criteria

- [ ] Loads Serum/Vital wavetables
- [ ] Smooth morphing across frames
- [ ] Spectral warps sound musical
- [ ] Sub + noise oscillators
- [ ] Multiple filter types with character
- [ ] Envelopes: loop, delay, hold stages
- [ ] LFOs: envelope mode, per-voice phase
- [ ] Mod matrix: intuitive drag-drop
- [ ] Unison: thick, wide, CPU-efficient
- [ ] Factory wavetables: 100+ frames

## Progress Log

- YYYY-MM-DD: Wavetable oscillator core
- YYYY-MM-DD: WT import (Serum, Vital, Wave)
- YYYY-MM-DD: Morphing + interpolation
- YYYY-MM-DD: Spectral warp modes
- YYYY-MM-DD: Sub/Noise oscillators
- YYYY-MM-DD: Filter types
- YYYY-MM-DD: Advanced envelopes/LFOs
- YYYY-MM-DD: Mod matrix UI
- YYYY-MM-DD: Unison + effects
- YYYY-MM-DD: Factory wavetable library