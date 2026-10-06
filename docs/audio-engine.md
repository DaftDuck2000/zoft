# Audio Engine Design

## Overview

The audio engine is the heart of Zoft. It must provide deterministic, low-latency audio processing with sample-accurate timing.

## Requirements

- **Latency**: < 2ms round-trip at 48kHz/256 samples
- **Determinism**: Same input → same output, always
- **Real-time safety**: No allocations, locks, syscalls in audio callback
- **Scalability**: 100+ tracks, 50+ plugins
- **Extensibility**: Built-in DSP + plugin hosting

## Core Components

### 1. Audio Backend Abstraction

```rust
trait AudioBackend: Send + Sync {
    fn start(&mut self, config: StreamConfig) -> Result<(), AudioError>;
    fn stop(&mut self) -> Result<(), AudioError>;
    fn set_process_callback(&mut self, callback: Box<dyn AudioCallback>);
    fn current_buffer_size(&self) -> usize;
    fn current_sample_rate(&self) -> f64;
    fn latency(&self) -> (usize, usize); // (input, output) frames
}

struct StreamConfig {
    sample_rate: u32,
    buffer_size: usize,          // Frames per block
    input_channels: u16,
    output_channels: u16,
}
```

Implementations:
- **Linux**: JACK (pro), PipeWire (default), ALSA (fallback)
- **macOS**: Core Audio (direct, not cpal)
- **Windows**: WASAPI Exclusive (pro), Shared (default)

### 2. Audio Callback

```rust
trait AudioCallback: Send + Sync {
    fn process(&mut self, input: &[AudioBuffer], output: &mut [AudioBuffer], ctx: &ProcessContext);
}

struct ProcessContext {
    sample_rate: f64,
    block_size: usize,
    current_frame: u64,          // Absolute frame counter
    tempo: f32,                  // Current BPM
    time_signature: (u8, u8),    // Numerator, denominator
    transport_state: TransportState,
    project_time: TimelinePos,   // Musical + sample position
}
```

### 3. Process Graph

```rust
struct ProcessGraph {
    nodes: SlotMap<NodeId, Box<dyn AudioNode>>,
    connections: Vec<Connection>,
    layers: Vec<Vec<NodeId>>,    // Topological layers
    latency_compensation: HashMap<NodeId, usize>,
}

trait AudioNode: Send + Sync {
    fn process(&mut self, ctx: &mut ProcessContext, buffers: &mut NodeBuffers);
    fn latency(&self) -> usize;
    fn reset(&mut self);
    fn prepare(&mut self, sample_rate: f64, max_block: usize);
}

struct NodeBuffers {
    inputs: Vec<AudioBuffer>,
    outputs: Vec<AudioBuffer>,
    scratch: Vec<AudioBuffer>,   // Temporary buffers
}
```

**Topological Sort**: Kahn's algorithm, recomputed on graph changes.
**Parallel Processing**: Within each layer, nodes are independent → `rayon::par_iter()`.

### 4. Track Processing

```rust
struct TrackProcessor {
    track_id: TrackId,
    node_id: NodeId,             // In process graph
    input: TrackInput,
    output: TrackOutput,
    inserts: Vec<InsertSlot>,    // Pre-fader
    sends: Vec<SendSlot>,        // Post-fader
    post_fader_inserts: Vec<InsertSlot>,
    fader: ParameterSmoother,
    pan: PanProcessor,
    meter: MeterState,
}

enum TrackInput {
    Audio { hardware: DeviceId, channels: [u32; 2] },
    Bus { bus_id: BusId },
    Instrument { plugin: PluginInstanceId },
    None,
}

enum TrackOutput {
    Hardware { device: DeviceId, channels: [u32; 2] },
    Bus { bus_id: BusId },
    Master,
}
```

### 5. Parameter System

```rust
struct ParameterSystem {
    params: SlotMap<ParamId, Parameter>,
    automation: HashMap<ParamId, AutomationLane>,
    smoothers: HashMap<ParamId, ParameterSmoother>,
}

struct Parameter {
    id: ParamId,
    info: ParamInfo,
    value: f32,                  // Normalized 0..1
    automation_mode: AutomationMode,
}

struct ParameterSmoother {
    current: f32,
    target: f32,
    ramp_frames: u32,
    frame: u32,
    law: RampLaw,                // Linear, Exponential
}

impl ParameterSmoother {
    fn process(&mut self) -> f32 {
        if self.frame < self.ramp_frames {
            let t = self.frame as f32 / self.ramp_frames as f32;
            let eased = match self.law {
                RampLaw::Linear => t,
                RampLaw::Exponential => 1.0 - (-10.0 * t).exp(),
            };
            self.current = self.current + (self.target - self.current) * eased;
            self.frame += 1;
        }
        self.current
    }
}
```

### 6. Transport & Timing

```rust
struct Transport {
    state: TransportState,
    tempo_map: TempoMap,
    time_sig_map: TimeSignatureMap,
    loop_range: Option<LoopRange>,
    metronome: Metronome,
}

enum TransportState {
    Stopped { position: TimelinePos },
    Playing { start_frame: u64, start_pos: TimelinePos },
}

struct TimelinePos {
    tick: u64,        // Musical position (PPQN)
    sample: u64,      // Sample position (absolute)
}
```

**Sample ↔ Musical Conversion**:
```rust
fn samples_to_ticks(samples: u64, tempo_map: &TempoMap, sample_rate: f64) -> u64 {
    // Piecewise integration: ∫(sample_rate * 60 / bpm(t)) dt
    // Pre-compute cumulative samples at each tempo event
    tempo_map.sample_to_tick(samples, sample_rate)
}

fn ticks_to_samples(ticks: u64, tempo_map: &TempoMap, sample_rate: f64) -> u64 {
    tempo_map.tick_to_sample(ticks, sample_rate)
}
```

### 7. Metering

```rust
struct MeterBridge {
    channels: Vec<ChannelMeter>,
    update_interval: Duration,   // 50ms default
}

struct ChannelMeter {
    peak: AtomicF32,             // Instantaneous peak
    peak_hold: AtomicF32,        // Peak hold with decay
    rms: AtomicF32,              // RMS over 300ms
    lufs_m: AtomicF32,           // LUFS momentary (400ms)
    lufs_s: AtomicF32,           // LUFS short-term (3s)
    true_peak: AtomicF32,        // 4x oversampled peak
    correlation: AtomicF32,      // Stereo correlation -1..1
    clip_count: AtomicU32,       // Samples > 0 dBFS
}
```

Updated from audio thread via lock-free ring buffer to UI.

## DSP Primitives (daw-engine/dsp)

### Oscillators
- **BLEP Saw/Square**: Band-limited step for alias-free
- **DPW**: Differentiated Parabolic Wave (efficient)
- **Wavetable**: Linear/cubic interpolation, spectral
- **Sine**: `std::simd` fast approx or `sinf`

### Filters
- **SVF**: State Variable Filter (LP/HP/BP/Notch, 12/24dB)
- **Biquad**: Direct Form II Transposed
- **Ladder**: Moog-style 4-pole (tanh saturation)
- **EQ**: Parametric (RBJ cookbook)

### Envelopes
- **ADSR**: Linear/exponential curves
- **AHDSR**: Hold stage
- **DAHDSR**: Delay, Attack, Hold, Decay, Sustain, Release
- **MSEG**: Multi-stage envelope generator

### Delay Lines
- **Fixed**: Power-of-2 buffer, modulo indexing
- **Variable**: Fractional delay with interpolation
- **Allpass**: For reverb diffusion

### Utilities
- **DC Block**: High-pass @ 10 Hz
- **Soft Clip**: tanh, atan, polynomial
- **Pan Laws**: Linear, Square Root, Sin/Cos, -3dB, -4.5dB, -6dB
- **Metering**: Peak, RMS, LUFS (EBU R128)

## Memory Management

- **No allocations in audio thread**: All buffers pre-allocated
- **Arena allocators**: `bumpalo` for temporary scratch buffers
- **Object pools**: Voice pools, event pools
- **Ring buffers**: Lock-free SPSC for UI↔Audio communication

## Error Handling

- **Audio thread**: Never panic. Log errors, output silence, continue.
- **XRuns**: Detect via timestamp discontinuity, increment counter, notify UI.
- **Plugin crashes**: Sandbox catches, restarts instance, restores state.

## Configuration

```toml
# config/audio.toml
[audio]
sample_rate = 48000
buffer_size = 256
backend = "auto"  # jack, pipewire, coreaudio, wasapi, auto

[engine]
max_tracks = 256
max_buses = 64
max_plugins_per_track = 16
polyphony_limit = 64
thread_count = 0  # 0 = auto (logical cores - 1)

[metering]
update_interval_ms = 50
peak_hold_decay_db_per_s = 12.0
lufs_momentary_window_ms = 400
lufs_short_term_window_s = 3
```