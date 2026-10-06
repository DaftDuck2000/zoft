# Feature: Performance Optimization

## Phase: 6 — Polish & Platform Expansion
## ID: 6.4
## Priority: High
## Estimated: 3 weeks
## Status: `[ ]` Not Started
## Depends On: 0.2, 3.1, 3.2, 3.3, 3.4, 3.5

## Description
End-to-end performance: SIMD DSP, multithreaded processing, profile-guided optimization, memory pooling, CPU scaling.

## Requirements

- [ ] SIMD DSP: `packed_simd` / `std::simd` for filters, oscillators, envelopes
- [ ] Multithreaded graph: Track-level parallelism, work stealing
- [ ] PGO: Profile-guided optimization (cargo-pgo)
- [ ] Memory pooling: Arena allocators, object pools for voices/buffers
- [ ] Lock-free structures: Ring buffers, queues, atomics
- [ ] CPU scaling: Dynamic thread count, core affinity, priority
- [ ] Plugin bridging: Shared memory zero-copy, batch parameters
- [ ] UI: Virtualized lists, GPU waveform, dirty rect rendering
- [ ] Startup: Lazy loading, background init, cache warming
- [ ] Profiling: `tracy`, `perf`, `instruments` integration

## Technical Details

### SIMD DSP Kernels
```rust
// Using std::simd (nightly) or packed_simd
use std::simd::{f32x4, f32x8, SimdFloat};

fn svf_process_simd(input: &[f32], output: &mut [f32], state: &mut SVFState, coeffs: &SVFCoeffs) {
    let g = f32x4::splat(coeffs.g);
    let k = f32x4::splat(coeffs.k);
    let a1 = f32x4::splat(coeffs.a1);
    let a2 = f32x4::splat(coeffs.a2);
    let a3 = f32x4::splat(coeffs.a3);
    
    let mut ic1eq = f32x4::splat(state.ic1eq);
    let mut ic2eq = f32x4::splat(state.ic2eq);
    
    for chunk in input.chunks_exact(4) {
        let x = f32x4::from_slice(chunk);
        
        // SVF topology
        let v3 = x - ic2eq;
        let v1 = a1 * ic1eq + a2 * v3;
        let v2 = ic2eq + a3 * v1;
        let out = v3 - v1;  // BP
        
        ic1eq = v1;
        ic2eq = v2;
        out.write_to_slice(&mut output[i..i+4]);
    }
    
    state.ic1eq = ic1eq[0];
    state.ic2eq = ic2eq[0];
}
```

### Multithreaded Graph Processing
```rust
struct ParallelGraph {
    nodes: Vec<NodeId>,
    layers: Vec<Vec<NodeId>>,     // Topological layers (parallelizable)
    thread_pool: rayon::ThreadPool,
}

impl ParallelGraph {
    fn process(&mut self, ctx: &mut ProcessContext, buffers: &mut [TrackBuffers]) {
        for layer in &self.layers {
            if layer.len() == 1 {
                // Single node: process on main audio thread
                self.process_node(layer[0], ctx, buffers);
            } else {
                // Multiple independent nodes: parallel
                self.thread_pool.install(|| {
                    layer.par_iter().for_each(|node| {
                        self.process_node(*node, ctx, buffers);
                    });
                });
            }
        }
    }
}
```

### Memory Pools
```rust
struct VoicePool {
    voices: Vec<Voice>,
    free_list: Vec<VoiceId>,
    arena: Arena<Voice>,          // bumpalo or custom
}

impl VoicePool {
    fn allocate(&mut self) -> VoiceId {
        self.free_list.pop().unwrap_or_else(|| {
            let id = self.voices.len();
            self.voices.push(Voice::default());
            id
        })
    }
    
    fn deallocate(&mut self, id: VoiceId) {
        self.voices[id].reset();
        self.free_list.push(id);
    }
}
```

### PGO Build
```toml
# .cargo/config.toml
[build]
rustflags = ["-C", "profile-generate=/path/to/pgo-data"]

# After running benchmarks:
# cargo build --release --features pgo-use
[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
```

### Profiling Integration
```rust
// Tracy integration
use tracy_client::{span, Zone};

fn process_block() {
    Zone!("Audio Process");
    span!("Graph Process");
    // ...
}
```

## Acceptance Criteria

- [ ] SIMD: 2-4x speedup on filter/oscillator kernels
- [ ] Multithread: 2-4x on 8+ core (track-level parallelism)
- [ ] PGO: 10-15% overall speedup
- [ ] Memory: < 50 MB base, no allocations in audio thread
- [ ] Lock-free: zero mutex in audio callback
- [ ] Startup: < 2s cold, < 500ms warm
- [ ] UI: 60 FPS at 4K, 100 tracks
- [ ] Plugin bridging: zero-copy audio

## Progress Log

- YYYY-MM-DD: SIMD kernels (SVF, osc, env)
- YYYY-MM-DD: Multithreaded graph
- YYYY-MM-DD: Memory pools + arenas
- YYYY-MM-DD: Lock-free audit
- YYYY-MM-DD: PGO pipeline
- YYYY-MM-DD: CPU scaling + affinity
- YYYY-MM-DD: UI virtualization
- YYYY-MM-DD: Profiling integration