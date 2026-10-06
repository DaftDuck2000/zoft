# Feature: CLAP/VST3 Wrapper

## Phase: 5 — Plugin Hosting
## ID: 5.1
## Priority: Critical
## Estimated: 4 weeks
## Status: `[ ]` Not Started
## Depends On: 0.2, 3.5

## Description
Plugin wrapper using `nih-plug` (CLAP) and `vst3-sys` (VST3). Parameter mapping, automation, preset handling, sample-accurate processing, note expression.

## Requirements

- [ ] CLAP support: Entry point, factory, plugin instance, audio ports, params, GUI, note expression
- [ ] VST3 support: Factory, component, controller, audio buses, parameters, edit controller
- [ ] Unified parameter model: Map CLAP/VST3 params to internal `ParamId`
- [ ] Sample-accurate automation: Parameter changes at sample offsets
- [ ] Note expression: Per-note pitch bend, pressure, timbre (MPE)
- [ ] Preset handling: Load/save plugin state (chunk), factory presets
- [ ] Latency reporting: Plugin latency → delay compensation
- [ ] Bypass: Hard bypass (audio passes through), soft bypass (plugin processes)
- [ ] Multi-timbral: Multiple MIDI channels → multiple audio outputs
- [ ] Sidechain: Host → plugin sidechain input

## Technical Details

### Plugin Wrapper Trait
```rust
trait PluginWrapper: Send + Sync {
    fn info(&self) -> PluginInfo;
    fn create_instance(&self, sample_rate: f64, max_block_size: usize) -> Box<dyn PluginInstance>;
    fn scan(&self) -> Result<Vec<PluginDescriptor>, ScanError>;
}

trait PluginInstance: Send {
    fn process(&mut self, input: &mut [AudioBuffer], output: &mut [AudioBuffer], events: &[Event], ctx: &ProcessContext);
    fn process_midi(&mut self, events: &[MidiEvent], output: &mut [MidiEvent]);
    fn set_param(&mut self, id: ParamId, value: f32, offset_frames: u32);
    fn get_param(&self, id: ParamId) -> f32;
    fn get_param_info(&self, id: ParamId) -> ParamInfo;
    fn get_state(&self) -> Vec<u8>;           // Chunk
    fn set_state(&mut self, data: &[u8]);
    fn get_presets(&self) -> Vec<PresetInfo>;
    fn load_preset(&mut self, index: usize);
    fn get_latency(&self) -> usize;
    fn activate(&mut self) -> Result<(), PluginError>;
    fn deactivate(&mut self);
    fn create_editor(&self) -> Option<Box<dyn PluginEditor>>;
    fn reset(&mut self);
}
```

### CLAP Integration (via `nih-plug`)
```rust
// nih-plug provides CLAP entry point and host-side wrapper
use nih_plug::prelude::*;
use nih_plug::clap::ClapPlugin;

// Host side:
let plugin = ClapPlugin::new(library_path)?;
let instance = plugin.instantiate(sample_rate, max_block_size)?;

// Parameter mapping:
for (i, param) in instance.params().iter().enumerate() {
    let internal_id = ParamId::new(plugin_id, i as u32);
    param_map.insert(internal_id, param.id());
}
```

### VST3 Integration (via `vst3-sys` + custom wrapper)
```rust
// VST3 is more complex - need COM initialization, factory, component, controller
use vst3_sys::{Steinberg::Vst::*, *};

struct Vst3Wrapper {
    module: Vst3Module,
    factory: *mut IPluginFactory,
    component: *mut IComponent,
    controller: *mut IEditController,
}

// Must initialize COM on Windows
// Linux/macOS: dlopen + vst3_module_get_factory
```

### Unified Parameter Model
```rust
struct PluginParameter {
    id: ParamId,
    plugin_id: PluginInstanceId,
    native_id: NativeParamId,    // CLAP: clap_id, VST3: param_id
    info: ParamInfo,
    automation: AutomationLane,
    modulation: ModulationState,
}

struct ParamInfo {
    name: String,
    label: String,               // "dB", "Hz", "%"
    min: f32,
    max: f32,
    default: f32,
    step_count: u32,             // 0 = continuous
    flags: ParamFlags,           // Automatable, Hidden, ReadOnly, etc.
}
```

### Sample-Accurate Automation
```rust
// Host sends parameter changes with frame offset
struct ParameterChange {
    param_id: ParamId,
    value: f32,
    frame_offset: u32,           // Samples into current block
}

// Plugin processes changes at exact sample
fn process_block(&mut self, input: &[AudioBuffer], output: &[AudioBuffer], changes: &[ParameterChange]) {
    let mut change_idx = 0;
    for frame in 0..block_size {
        while change_idx < changes.len() && changes[change_idx].frame_offset == frame as u32 {
            self.set_param_raw(changes[change_idx].param_id, changes[change_idx].value);
            change_idx += 1;
        }
        // Process frame
    }
}
```

### Note Expression (MPE)
```rust
struct NoteExpression {
    note_id: u16,                // CLAP note_id / VST3 note_id
    channel: u8,
    pitch_bend: f32,             // -1..1
    pressure: f32,               // 0..1
    timbre: f32,                 // 0..1 (CC74)
    // CLAP: note_expression_t
    // VST3: NoteExpressionValue
}
```

## Acceptance Criteria

- [ ] Loads CLAP plugins (Surge, Vital, Valhalla)
- [ ] Loads VST3 plugins (FabFilter, Soundtoys, etc.)
- [ ] Parameters map to automation system
- [ ] Sample-accurate automation: no zipper noise
- [ ] MPE: per-note expression works
- [ ] Presets: load factory, save/load user
- [ ] Latency compensation works
- [ ] Bypass: hard/soft
- [ ] Multi-out: multiple audio returns
- [ ] Sidechain input works

## Progress Log

- YYYY-MM-DD: nih-plug integration (CLAP)
- YYYY-MM-DD: VST3 wrapper (vst3-sys)
- YYYY-MM-DD: Unified parameter model
- YYYY-MM-DD: Sample-accurate automation
- YYYY-MM-DD: Note expression (MPE)
- YYYY-MM-DD: Preset handling
- YYYY-MM-DD: Latency reporting
- YYYY-MM-DD: Bypass modes
- YYYY-MM-DD: Multi-out + sidechain