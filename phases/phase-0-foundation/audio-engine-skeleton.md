# Feature: Audio Engine Skeleton

## Phase: 0 — Foundation
## ID: 0.2
## Priority: Critical
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 0.1

## Description
Build the core audio engine: cpal audio stream, lock-free ring buffer for UI↔audio communication, sample-accurate event scheduler, and basic DSP graph infrastructure.

## Requirements

- [ ] CPAL audio output stream (configurable sample rate, buffer size)
- [ ] Lock-free ring buffer (SPSC) for UI → Audio thread messages
- [ ] Sample-accurate event scheduler (timestamped events, frame offset)
- [ ] Basic DSP graph: Node trait, connection, topological sort
- [ ] Audio thread: process callback, meter collection, CPU monitoring
- [ ] Parameter system: smooth parameter changes (linear/exponential ramps)
- [ ] Silence detection & auto-suspend (optional)

## Technical Details

### Threading Model
```
Main Thread (UI)                    Audio Thread (RT)
─────────────────                    ────────────────
┌─────────────────┐                  ┌─────────────────┐
│ UI Event Loop   │                  │ CPAL Callback   │
│                 │                  │                 │
│ send_msg() ──────┼── Ring Buffer ──▶│ recv_msgs()     │
│                 │   (MPSC, bounded)│                 │
│                 │                  │ process_graph() │
│                 │                  │ update_meters() │
│                 │                  │                 │
└─────────────────┘                  └─────────────────┘
```

### Message Types (UI → Audio)
```rust
enum EngineMsg {
    Transport(TransportCmd),        // Play, Stop, Seek, SetTempo
    Track(TrackCmd),                // Add/Remove/Reorder tracks
    Clip(ClipCmd),                  // Add/Move/Delete clips
    Parameter(ParameterCmd),        // Set param (sample-accurate)
    Plugin(PluginCmd),              // Load/Unload plugin (deferred)
    Project(ProjectCmd),            // Load/Save project
}

struct ParameterCmd {
    node_id: NodeId,
    param_id: ParamId,
    value: f32,
    ramp: Option<Ramp>,             // Linear/Exp over N frames
    frame_offset: u64,              // Sample-accurate scheduling
}
```

### DSP Graph
```rust
trait AudioNode: Send + Sync {
    fn process(&mut self, ctx: &mut ProcessContext, buffers: &mut [AudioBuffer]);
    fn latency(&self) -> usize;     // Samples of delay
    fn reset(&mut self);            // Clear internal state
}

struct ProcessContext {
    sample_rate: f64,
    block_size: usize,
    current_frame: u64,
    tempo: f32,
    time_signature: (u8, u8),
    transport_state: TransportState,
}
```

### Ring Buffer Implementation
- Use `crossbeam::channel::bounded` (MPSC) for simplicity
- Capacity: 256 messages (≈5ms at 48kHz/256 blocks)
- Audio thread drains all available messages each block
- Backpressure: UI blocks if full (acceptable for parameter changes)

## Acceptance Criteria

- [ ] Audio callback runs without xruns at 48kHz/256 samples
- [ ] Parameter changes from UI heard within 1 block
- [ ] CPU meter reports < 1% on idle
- [ ] Graph processes 16 tracks with gain plugins
- [ ] Graceful shutdown (no crashes on drop)

## Progress Log

- YYYY-MM-DD: CPAL stream initialized
- YYYY-MM-DD: Ring buffer working
- YYYY-MM-DD: DSP graph processes nodes
- YYYY-MM-DD: Parameter smoothing implemented