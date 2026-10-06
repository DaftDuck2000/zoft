# Feature: Plugin State & Presets

## Phase: 5 — Plugin Hosting
## ID: 5.5
## Priority: High
## Estimated: 1 week
## Status: `[ ]` Not Started
## Depends On: 5.1

## Description
Plugin state management: chunk save/load, preset browsing, parameter automation state, bank/program support, undo for plugin params.

## Requirements

- [ ] Chunk state: Save/load opaque plugin state (getState/setState)
- [ ] Preset browser: Factory presets, user presets, categories, search
- [ ] Bank/Program: VST3 program list, CLAP preset list
- [ ] Parameter automation: Full automation lane per param
- [ ] State versioning: Handle plugin version changes
- [ ] Undo/Redo: Plugin parameter changes in history
- [ ] Copy/Paste: Plugin state between instances
- [ ] Default state: Save as default for new instances
- [ ] Randomize: Randomize parameters (with constraints)

## Technical Details

### State Management
```rust
struct PluginStateManager {
    instances: HashMap<PluginInstanceId, PluginState>,
    user_presets: HashMap<PluginId, Vec<UserPreset>>,
    default_states: HashMap<PluginId, Vec<u8>>,
}

struct PluginState {
    chunk: Vec<u8>,                 // Opaque plugin state
    param_values: HashMap<ParamId, f32>, // For automation/undo
    current_preset: Option<PresetRef>,
    is_modified: bool,
}

struct UserPreset {
    name: String,
    chunk: Vec<u8>,
    param_values: HashMap<ParamId, f32>,
    category: String,
    tags: Vec<String>,
    author: String,
    created: SystemTime,
    plugin_version: String,         // For compatibility
}

enum PresetRef {
    Factory { bank: u32, program: u32 },
    User { preset_id: PresetId },
}
```

### Save/Load Chunk
```rust
impl PluginInstance {
    fn get_state(&self) -> Vec<u8> {
        // CLAP: clap_plugin->get_state(plugin, &mut chunk)
        // VST3: component->getState(&stream)
    }
    
    fn set_state(&mut self, data: &[u8]) {
        // CLAP: clap_plugin->set_state(plugin, data)
        // VST3: component->setState(&stream)
    }
}
```

### Preset Browser Integration
```rust
struct PresetBrowser {
    plugin_id: PluginId,
    presets: Vec<PresetInfo>,
    filter: PresetFilter,
    selected: Option<PresetId>,
}

struct PresetInfo {
    id: PresetId,
    name: String,
    category: String,
    is_factory: bool,
    plugin_version: String,
}

impl PresetBrowser {
    fn load_preset(&mut self, preset: &PresetInfo, instance: &mut PluginInstance) {
        match preset.source {
            PresetSource::Factory { bank, program } => {
                instance.load_factory_preset(bank, program);
            }
            PresetSource::User { chunk } => {
                instance.set_state(&chunk);
            }
        }
        // Update parameter values for automation
        self.sync_params(instance);
    }
}
```

### Parameter Automation Sync
```rust
fn sync_plugin_params(instance: &PluginInstance, lanes: &mut HashMap<ParamId, AutomationLane>) {
    for (param_id, lane) in lanes {
        if let Some(value) = instance.get_param(param_id.native) {
            // Update lane's current value for display
            lane.current_value = value;
        }
    }
}
```

### Undo for Plugin Parameters
```rust
struct SetPluginParamCmd {
    plugin_id: PluginInstanceId,
    param_id: ParamId,
    old_value: f32,
    new_value: f32,
}

impl Command for SetPluginParamCmd {
    fn execute(&mut self, ctx: &mut CommandContext) {
        ctx.engine.send(EngineMsg::Plugin(PluginMsg::SetParam {
            instance: self.plugin_id,
            param: self.param_id,
            value: self.new_value,
        }));
    }
    
    fn undo(&mut self, ctx: &mut CommandContext) {
        ctx.engine.send(EngineMsg::Plugin(PluginMsg::SetParam {
            instance: self.plugin_id,
            param: self.param_id,
            value: self.old_value,
        }));
    }
}
```

### Randomize with Constraints
```rust
fn randomize_plugin(instance: &mut PluginInstance, constraints: &RandomizeConstraints) {
    for param in instance.params() {
        if constraints.excluded.contains(&param.id) { continue; }
        if !param.flags.automatable { continue; }
        
        let range = constraints.range.get(&param.id).unwrap_or((param.min, param.max));
        let distribution = constraints.distribution.get(&param.id).unwrap_or(Distribution::Uniform);
        
        let value = match distribution {
            Distribution::Uniform => rand::random_range(range.0..=range.1),
            Distribution::Gaussian { mean, std } => gaussian_clamped(mean, std, range),
            Distribution::LogUniform => log_uniform(range),
        };
        
        instance.set_param(param.id, value, 0);
    }
}
```

## Acceptance Criteria

- [ ] Save project → plugin state restored on load
- [ ] Preset browser: factory + user presets, search, categories
- [ ] Bank/Program change works
- [ ] Parameter automation lanes created automatically
- [ ] Undo/Redo works for plugin params
- [ ] Copy plugin state → paste to another instance
- [ ] Save as default for new instances
- [ ] Randomize respects constraints
- [ ] State compatible across plugin versions (graceful fallback)

## Progress Log

- YYYY-MM-DD: Chunk save/load
- YYYY-MM-DD: Preset browser UI
- YYYY-MM-DD: Bank/Program support
- YYYY-MM-DD: Automation lane sync
- YYYY-MM-DD: Undo integration
- YYYY-MM-DD: Copy/paste state
- YYYY-MM-DD: Default state
- YYYY-MM-DD: Randomize