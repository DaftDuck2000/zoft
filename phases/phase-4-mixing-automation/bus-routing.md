# Feature: Bus Routing & Groups

## Phase: 4 — Mixing & Automation
## ID: 4.2
## Priority: Critical
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 4.1

## Description
Advanced routing: Aux buses, VCA groups, Folder stacks, Sidechain routing, External hardware I/O, Multi-out instruments.

## Requirements

- [ ] Aux buses: Configurable count, stereo/mono, inserts, sends, output routing
- [ ] VCA groups: Master fader controls member faders (trim), automation-aware
- [ ] Folder tracks: Sum child tracks, fold/unfold, solo/mute propagates
- [ ] Sidechain: Any track/bus → any compressor/gate input, visual routing
- [ ] External hardware: Insert hardware FX (ping measurement, latency comp)
- [ ] Multi-out instruments: Instrument track → multiple audio returns
- [ ] Bus hierarchy: Bus → Bus → Master, unlimited depth
- [ ] Routing presets: Save/load routing templates

## Technical Details

### Bus Channel
```rust
struct BusChannel {
    id: BusId,
    name: String,
    channel_type: BusType,
    format: ChannelFormat,      // Mono, Stereo, 5.1, 7.1, Ambisonic
    channel_strip: ChannelStrip, // Reuses mixer strip
    children: Vec<TrackId>,     // Tracks/buses routed to this bus
    parent: Option<BusId>,      // Bus hierarchy
}

enum BusType {
    Aux,          // Send destination
    Subgroup,     // Track output destination
    Folder,       // Folder track sum
    VCA,          // VCA master (no audio)
    Master,       // Main output
    Cue,          // Headphone/cue mix
}
```

### VCA Implementation
```rust
struct VCAChannel {
    id: VCAId,
    name: String,
    fader: Fader,               // Controls member trim
    members: Vec<TrackId>,      // Tracks/buses in group
    automation: AutomationLane, // VCA automation lane
}

fn apply_vca(vca: &VCAChannel, members: &mut [ChannelStrip]) {
    let vca_gain = vca.fader.value;  // Linear gain
    for member in members {
        // VCA acts as trim: effective_gain = member.fader * vca_gain
        // But member fader position stays same (visual)
        member.effective_gain = member.fader.value * vca_gain;
        
        // Automation: VCA automation adds to member automation
        if let (Some(vca_auto), Some(mem_auto)) = (&vca.automation, &mut member.fader.automation) {
            mem_auto.effective_value = mem_auto.value + vca_auto.value; // In dB
        }
    }
}
```

### Folder Tracks
```rust
struct FolderTrack {
    base: TrackBase,
    children: Vec<TrackId>,       // Ordered
    folded: bool,
    sum_bus: BusId,               // Hidden bus summing children
}

impl FolderTrack {
    fn toggle_fold(&mut self, project: &mut Project) {
        self.folded = !self.folded;
        for child_id in &self.children {
            project.tracks.get_mut(child_id).visible = !self.folded;
        }
    }
    
    fn propagate_solo(&self, solo: bool, project: &mut Project) {
        for child_id in &self.children {
            project.tracks.get_mut(child_id).solo = solo;
            if let Track::Folder(f) = project.tracks.get_mut(child_id) {
                f.propagate_solo(solo, project); // Recursive
            }
        }
    }
}
```

### Sidechain Routing
```rust
struct SidechainRouter {
    sources: Vec<SidechainSource>,  // Any tap point
    destinations: Vec<SidechainDest>, // Compressor/gate sidechain inputs
    connections: HashMap<SidechainDestId, SidechainSourceId>,
}

struct SidechainSource {
    id: SidechainSourceId,
    tap_point: TapPoint,            // Pre-fader, Post-fader, Pre-insert, Post-insert
    channel: ChannelId,
    filter: Option<SidechainFilter>, // HP/LP for kick detection
}

struct SidechainDest {
    id: SidechainDestId,
    plugin: PluginId,
    param: ParamId,                 // Sidechain input parameter
}
```

### External Hardware Insert
```rust
struct HardwareInsert {
    send_output: HardwareOutput,
    return_input: HardwareInput,
    latency_samples: usize,         // Measured via ping
    latency_compensation: bool,     // Auto-compensate
    ping_enabled: bool,
}

fn measure_hardware_latency(insert: &mut HardwareInsert) {
    // Send click → record return → measure delay → store
}
```

### Multi-Out Instruments
```rust
struct MultiOutInstrument {
    instrument_track: TrackId,
    returns: Vec<AudioReturn>,      // One per output
}

struct AudioReturn {
    bus_id: BusId,                  // Hidden aux bus
    channel: u8,                    // Plugin output channel
    name: String,                   // "Kick", "Snare", "OH L", etc.
}
```

## Acceptance Criteria

- [ ] Aux buses: create, route sends, inserts, output
- [ ] VCA: master fader trims members, automation works
- [ ] Folder: fold/unfold, solo/mute propagates
- [ ] Sidechain: visual routing, filter, works on all dynamics
- [ ] Hardware insert: ping measures latency, compensation works
- [ ] Multi-out: instrument creates multiple mixer channels
- [ ] Bus hierarchy: Bus → Bus → Master works
- [ ] Routing presets save/load

## Progress Log

- YYYY-MM-DD: Aux bus implementation
- YYYY-MM-DD: VCA groups
- YYYY-MM-DD: Folder tracks
- YYYY-MM-DD: Sidechain routing UI
- YYYY-MM-DD: Hardware insert + ping
- YYYY-MM-DD: Multi-out instruments
- YYYY-MM-DD: Bus hierarchy
- YYYY-MM-DD: Routing presets