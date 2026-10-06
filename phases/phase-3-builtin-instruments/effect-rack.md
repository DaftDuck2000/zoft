# Feature: Effect Rack

## Phase: 3 — Built-in Instruments
## ID: 3.5
## Priority: Critical
## Estimated: 4 weeks
## Status: `[ ]` Not Started
## Depends On: 0.2 (DSP framework)

## Description
Comprehensive built-in effects: EQ, Compressor, Reverb, Delay, Saturation, Limiter, Gate, Filter, Chorus, Phaser, Flanger. Per-track inserts, sends, master bus. Stereo + mid/side.

## Requirements

- [ ] **EQ**: 8-band parametric, linear/minimum phase, spectrum analyzer, mid/side
- [ ] **Compressor**: VCA/FET/Opto/Vari-Mu models, sidechain, lookahead, mix (parallel)
- [ ] **Reverb**: Algorithmic (hall, plate, room, chamber), convolution (IR load), pre-delay, EQ, freeze
- [ ] **Delay**: Mono/stereo/ping-pong, tempo sync, feedback, filter, modulation, ducking
- [ ] **Saturation**: Tube, Tape, Transistor, Waveshaper, drive, mix, oversampling
- [ ] **Limiter**: True peak, lookahead, ISP detection, release modes, dither
- [ ] **Gate/Expander**: Hold, lookahead, sidechain, hysteresis
- [ ] **Filter**: SVF multi-mode, envelope follower, LFO, drive
- [ ] **Chorus/Flanger/Phaser**: Stereo, tempo sync, feedback, mix
- [ ] **Utility**: Gain, Pan (stereo/MS), Width, Phase, Mono, DC Block
- [ ] **Rack**: 8 insert slots + 8 sends per track, drag-reorder, bypass, solo
- [ ] **Presets**: Per-effect + rack presets, A/B comparison

## Technical Details

### Effect Trait
```rust
trait AudioEffect: Send + Sync {
    fn info(&self) -> EffectInfo;
    fn process(&mut self, input: &mut [AudioBuffer], ctx: &ProcessContext);
    fn reset(&mut self);
    fn params(&self) -> Vec<ParamInfo>;
    fn set_param(&mut self, id: ParamId, value: f32);
    fn get_param(&self, id: ParamId) -> f32;
    fn latency(&self) -> usize;   // Samples
    fn tail_length(&self) -> usize; // For reverb/delay tails
}

struct EffectInfo {
    id: EffectId,
    name: String,
    category: EffectCategory,
    version: u32,
    input_channels: u8,
    output_channels: u8,
    can_mono: bool,
    can_stereo: bool,
    can_mid_side: bool,
}
```

### EQ (Parametric, 8 Bands)
```rust
struct ParametricEQ {
    bands: [EQBand; 8],
    analyzer: SpectrumAnalyzer,
    mode: EQMode,      // MinimumPhase, LinearPhase
    ms_mode: bool,     // Mid/Side processing
}

struct EQBand {
    enabled: bool,
    freq: f32,         // 20 Hz - 20 kHz
    gain: f32,         // -24..+24 dB
    q: f32,            // 0.1 - 30
    shape: EQShape,    // Bell, LowShelf, HighShelf, LowPass, HighPass, BandPass, Notch
}
```

### Compressor (Multiple Models)
```rust
struct Compressor {
    model: CompModel,  // VCA, FET, Opto, VariMu, Digital
    threshold: f32,    // -60..0 dB
    ratio: f32,        // 1:1 - ∞:1
    attack: f32,       // 0.01 - 100 ms
    release: f32,      // 10 - 2000 ms
    knee: f32,         // 0 - 48 dB
    makeup: f32,       // 0 - 24 dB
    mix: f32,          // 0..1 (parallel)
    sidechain: SidechainConfig,
    lookahead: f32,    // 0 - 10 ms
    detector: DetectorType, // Peak, RMS, TruePeak
}

enum CompModel {
    VCA,      // Fast, clean (dbx 160 style)
    FET,      // Aggressive, fast (1176 style)
    Opto,     // Smooth, program-dependent (LA-2A style)
    VariMu,   // Tube, soft knee (Fairchild style)
    Digital,  // Precise, no coloration
}
```

### Reverb (Algorithmic + Convolution)
```rust
struct Reverb {
    engine: ReverbEngine,
    pre_delay: f32,
    size: f32,
    decay: f32,
    dampening: f32,
    diffusion: f32,
    eq: ReverbEQ,
    freeze: bool,
    mix: f32,
}

enum ReverbEngine {
    Algorithmic(AlgorithmicReverb),  // FDN + early reflections
    Convolution(ConvolutionReverb),  // IR-based
}

struct ConvolutionReverb {
    ir: Arc<ImpulseResponse>,        // Stereo IR
    partitioner: PartitionedConvolver, // For long IRs
}
```

### Delay
```rust
struct Delay {
    mode: DelayMode,      // Mono, Stereo, PingPong
    time: DelayTime,      // ms or tempo-synced
    feedback: f32,        // 0..1
    feedback_tap: usize,  // Multi-tap
    filter: DelayFilter,  // LP/HP in feedback path
    modulation: DelayMod, // LFO on delay time
    ducking: f32,         // Duck wet with dry
    width: f32,           // Stereo width
    mix: f32,
}

enum DelayTime {
    Milliseconds(f32),
    TempoSynced(NoteValue),  // 1/4, 1/8, 1/16, dotted, triplet
}
```

### Saturation
```rust
struct Saturation {
    model: SatModel,      // Tube, Tape, Transistor, Waveshaper, Rectify
    drive: f32,           // 0..48 dB
    tone: f32,            // Pre/post EQ
    mix: f32,             // 0..1
    oversample: u8,       // 1x, 2x, 4x, 8x
    auto_gain: bool,      // Compensate output level
}

enum SatModel {
    Tube,         // Even harmonics, soft knee
    Tape,         // Odd + even, compression, hysteresis
    Transistor,   // Harder clipping, asymmetrical
    Waveshaper,   // Chebyshev polynomials
    Rectify,      // Half/full wave rectification
}
```

### Effect Rack (Per Track)
```rust
struct EffectRack {
    inserts: Vec<InsertSlot>,      // Pre-fader, serial
    sends: [SendSlot; 8],          // Post-fader, parallel
    post_fader_inserts: Vec<InsertSlot>, // Post-fader serial
    master_bus: bool,              // If true, this is master rack
}

struct InsertSlot {
    effect: Option<Box<dyn AudioEffect>>,
    enabled: bool,
    input_gain: f32,
    output_gain: f32,
    mix: f32,                      // For parallel processing
}

struct SendSlot {
    destination: BusId,
    level: f32,                    // 0..1 (pre/post fader)
    pan: f32,
    enabled: bool,
}
```

## Acceptance Criteria

- [ ] EQ: 8 bands, analyzer, mid/side, linear phase option
- [ ] Compressor: 5 models sound distinct, sidechain works
- [ ] Reverb: Algorithmic halls/plates + convolution IR loading
- [ ] Delay: Sync, ping-pong, modulation, ducking
- [ ] Saturation: 5 models, oversampling prevents aliasing
- [ ] Limiter: True peak, ISP detection, dither
- [ ] Gate: Lookahead, sidechain, smooth
- [ ] Filter: SVF, envelope follower, LFO
- [ ] Modulation effects: Chorus/Flanger/Phaser stereo
- [ ] Rack: Drag-reorder, bypass, A/B, presets
- [ ] CPU: Full rack < 10% on modern CPU

## Progress Log

- YYYY-MM-DD: Effect trait + rack infrastructure
- YYYY-MM-DD: Parametric EQ + analyzer
- YYYY-MM-DD: Compressor models
- YYYY-MM-DD: Algorithmic reverb
- YYYY-MM-DD: Convolution reverb
- YYYY-MM-DD: Delay (all modes)
- YYYY-MM-DD: Saturation models
- YYYY-MM-DD: Limiter + dither
- YYYY-MM-DD: Gate + Filter
- YYYY-MM-DD: Modulation effects
- YYYY-MM-DD: Utility effects
- YYYY-MM-DD: Rack UI + presets