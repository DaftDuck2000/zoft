# Feature: Mixer View

## Phase: 4 — Mixing & Automation
## ID: 4.1
## Priority: Critical
## Estimated: 3 weeks
## Status: `[~]` In Progress
## Depends On: 0.4, 1.1, 3.5

## Description
Full mixer console: channel strips, meter bridge, insert/send racks, routing matrix, solo/mute groups, snapshot recall.

## Requirements

- [~] Channel strips: Name, color, mute/solo, volume fader, pan and inserts implemented; meters, sends and I/O pending
- [ ] Meter bridge: Peak, RMS, LUFS, true peak, correlation, K-system
- [~] Insert rack: 8 slots, drag-reorder, bypass, solo, preset
  - [x] Basic per-track insert chain (saturation, filter, bit crusher, tremolo) on every track type
  - [~] FX Rack: horizontal track columns with insert lists, add/remove/reorder/bypass and parameter controls; drag-reorder, solo and presets pending
- [ ] Send rack: 8 sends, pre/post fader, pan, level, destination
- [ ] Routing matrix: Visual patchbay for I/O, buses, sidechains
- [ ] Solo/Mute groups: Exclusive solo, solo safe, mute groups
- [ ] VCA faders: Control group level, trim automation
- [ ] Channel strip presets: Save/load strip state
- [ ] Mixer layouts: Narrow, wide, custom column visibility
- [ ] Meter bridge pop-out: Detachable window
- [ ] Touch/control surface: MCU/HUI protocol support

## Technical Details

### Mixer Data Model
```rust
struct Mixer {
    channels: Vec<ChannelStrip>,
    buses: Vec<BusChannel>,
    vcas: Vec<VCAChannel>,
    master: MasterChannel,
    solo_system: SoloSystem,
    meter_bridge: MeterBridge,
}

struct ChannelStrip {
    track_id: TrackId,
    fader: Fader,              // -∞..+12 dB, law: audio taper
    pan: PanControl,           // Stereo balance or MS
    meter: MeterState,         // Peak, RMS, LUFS momentary
    inserts: InsertRack,       // 8 slots pre-fader
    sends: SendRack,           // 8 sends post-fader
    post_inserts: InsertRack,  // 4 slots post-fader
    input: IORouting,          // Source selection
    output: IORouting,         // Destination (bus, hardware)
    solo: SoloState,
    mute: bool,
    record_arm: bool,
    automation: AutomationReadMode,
    color: Color,
    name: String,
}

struct Fader {
    value: f32,                // Linear gain 0.0..4.0 (-∞..+12 dB)
    law: FaderLaw,             // AudioTaper, Linear, Custom
    automation: AutomationLane,
}

struct MeterState {
    peak: f32,                 // Current peak (linear)
    peak_hold: f32,            // Peak hold with decay
    rms: f32,                  // RMS over 300ms
    lufs_m: f32,               // LUFS momentary (400ms)
    lufs_s: f32,               // LUFS short-term (3s)
    true_peak: f32,            // Oversampled peak
    correlation: f32,          // -1..1 (stereo correlation)
    clip_count: u32,           // Samples > 0 dBFS
}
```

### Metering (Audio Thread → UI)
```rust
// Audio thread computes per-block, sends via ring buffer
struct MeterMessage {
    channel: ChannelId,
    peak: f32,
    rms: f32,
    lufs_m: f32,
    true_peak: f32,
    correlation: f32,
    clip: bool,
}

// UI: smooth meter ballistics
fn update_meter_ui(state: &mut MeterState, msg: MeterMessage, dt: f32) {
    state.peak = msg.peak;
    state.peak_hold = state.peak_hold.max(msg.peak).mul_add(0.999, 0.0); // Slow decay
    state.rms = msg.rms;
    state.lufs_m = msg.lufs_m;
    state.true_peak = msg.true_peak;
    state.correlation = msg.correlation;
    if msg.clip { state.clip_count += 1; }
}
```

### Routing Matrix (Visual Patchbay)
```rust
struct RoutingMatrix {
    sources: Vec<RoutingSource>,     // Tracks, buses, hardware in
    destinations: Vec<RoutingDest>,  // Tracks, buses, hardware out, inserts
    connections: HashSet<(SourceId, DestId)>,
}

// Sources: Audio Track Out, Bus Out, Hardware In, Insert Send
// Destinations: Audio Track In, Bus In, Hardware Out, Insert Return
```

### Solo System (Pro Logic)
```rust
enum SoloMode {
    Latch,      // Click to solo, click again to unsolo (exclusive)
    Momentary,  // Hold key to solo
    Additive,   // Multiple solos allowed
}

struct SoloSystem {
    mode: SoloMode,
    soloed: HashSet<TrackId>,
    solo_safe: HashSet<TrackId>,    // Never muted by solo
    mute_groups: HashMap<u8, HashSet<TrackId>>, // Mute groups 1-8
}
```

## Acceptance Criteria

- [ ] 64+ channel strips at 60 FPS
- [ ] Meters: Peak/RMS/LUFS/True Peak accurate
- [ ] Insert/send racks: drag-reorder, bypass, solo
- [ ] Routing matrix: visual, click to connect
- [ ] Solo modes work (latch, momentary, additive)
- [ ] Solo safe protects tracks
- [ ] Mute groups work
- [ ] VCA faders control group level
- [ ] Channel strip presets save/load
- [ ] MCU/HUI: faders, pans, solo/mute, transport

## Progress Log

- 2026-10-26: Added the FX Rack (X), a Logic-style window with one horizontal column per track and that track's insert chain underneath. Tracks can be scrolled horizontally; slots can be added, removed, reordered, bypassed and edited in place
- 2026-10-26: FX Rack columns now include mute/solo and a volume fader. Continuous fader moves merge into one undo entry, and the live mixer is updated immediately while dragging
- 2026-10-26: Added equal-power pan to the FX Rack channel strips. Pan is applied in the playback mixer, is undoable and uses a centered unity-gain law
- 2026-10-26: Added a pan slider to the left track list alongside volume, with live mixer updates while dragging
- YYYY-MM-DD: Channel strip UI
- YYYY-MM-DD: Meter bridge + ballistics
- YYYY-MM-DD: Insert/send racks
- YYYY-MM-DD: Routing matrix
- YYYY-MM-DD: Solo/mute system
- YYYY-MM-DD: VCA faders
- YYYY-MM-DD: Strip presets
- YYYY-MM-DD: Layouts + pop-out meter bridge
- YYYY-MM-DD: MCU/HUI support