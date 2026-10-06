# Plugin Hosting Architecture

## Overview

Phase 5+ adds support for third-party CLAP and VST3 plugins with process isolation, crash recovery, and seamless integration.

## Design Goals

1. **Stability**: Plugin crash ≠ DAW crash
2. **Performance**: Zero-copy audio, minimal IPC overhead
3. **Compatibility**: CLAP 1.x, VST3 3.6+, future AUv3
4. **Integration**: Parameters → automation, presets → browser, UI → window

## Architecture

### Process-per-Plugin Sandbox

```
┌─────────────────────────────────────────────────────────────────┐
│                        HOST PROCESS                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐  │
│  │ Plugin Mgr  │  │ IPC Router  │  │ Shared Memory Pool      │  │
│  │  (Scanner)  │  │  (Channels) │  │  (Audio + Params)       │  │
│  └──────┬──────┘  └──────┬──────┘  └───────────┬─────────────┘  │
│         │                │                     │                │
│    ┌────┴────┐      ┌────┴────┐          ┌────┴────┐           │
│    │  UI     │      │ Monitor │          │ Audio   │           │
│    │ Thread  │      │ Thread  │          │ Thread  │           │
│    └────┬────┘      └────┬────┘          └────┬────┘           │
└─────────┼────────────────┼─────────────────────┼────────────────┘
          │                │                     │
          ▼                ▼                     ▼
┌─────────────────────────────────────────────────────────────────┐
│                      PLUGIN PROCESSES                            │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐              │
│  │ Plugin A    │  │ Plugin B    │  │ Plugin C    │  ...         │
│  │ (CLAP/VST3) │  │ (CLAP/VST3) │  │ (CLAP/VST3) │              │
│  └─────────────┘  └─────────────┘  └─────────────┘              │
└─────────────────────────────────────────────────────────────────┘
```

### Shared Memory Layout

```rust
#[repr(C, align(64))]
struct PluginSharedMemory {
    // Audio buffers (double-buffered for lock-free)
    audio: AudioBuffers,
    
    // Parameter ring buffer (Host → Plugin)
    param_ring: RingBuffer<ParamChange, 1024>,
    
    // Event ring buffer (Host → Plugin: MIDI, note expression)
    event_ring: RingBuffer<PluginEvent, 2048>,
    
    // Output event ring (Plugin → Host: MIDI out, parameter changes)
    output_ring: RingBuffer<PluginOutputEvent, 1024>,
    
    // State
    state: PluginState,           // Running, Suspended, Error, Restarting
    latency: u32,                 // Samples
    tail_length: u32,             // Samples
    
    // Crash info (written by plugin before exit)
    crash_info: Option<CrashInfo>,
}

struct AudioBuffers {
    input: [AudioBuffer; MAX_CHANNELS],
    output: [AudioBuffer; MAX_CHANNELS],
    sidechain: [AudioBuffer; MAX_SIDECHAIN_CHANNELS],
}
```

### IPC Protocol

**Host → Plugin** (via `param_ring` + `event_ring`):
```rust
enum HostToPluginMsg {
    // Audio processing
    ProcessBlock { 
        block_size: usize, 
        events: Vec<PluginEvent>,
        param_changes: Vec<ParamChange>,
    },
    
    // Parameter automation (sample-accurate)
    SetParameter { 
        param_id: u32, 
        value: f32, 
        frame_offset: u32 
    },
    
    // State management
    GetState,
    SetState { data: Vec<u8> },
    
    // Lifecycle
    Activate { sample_rate: f64, max_block: usize },
    Deactivate,
    Reset,
    
    // UI
    CreateEditor { parent: WindowHandle, scale: f32 },
    CloseEditor,
    
    // Health
    Ping,
    Shutdown,
}
```

**Plugin → Host** (via `output_ring`):
```rust
enum PluginToHostMsg {
    // Audio done
    ProcessDone { 
        output_buffers: AudioBuffers, 
        events: Vec<PluginOutputEvent>,
        tail_frames: u32 
    },
    
    // Parameter values (for automation readback)
    ParameterValue { param_id: u32, value: f32 },
    
    // State
    StateData { data: Vec<u8> },
    StateSaved,
    
    // UI
    EditorCreated { window_handle: WindowHandle },
    EditorClosed,
    EditorResize { width: u32, height: u32 },
    
    // Errors
    Error { code: u32, message: String },
    Crashed { info: CrashInfo },
    
    // Health
    Pong,
}
```

### Plugin Wrapper

```rust
trait PluginWrapper: Send + Sync {
    fn info(&self) -> PluginInfo;
    fn create_instance(&self, config: InstanceConfig) -> Result<Box<dyn PluginInstance>, PluginError>;
    fn supported_formats(&self) -> Vec<PluginFormat>;
}

struct InstanceConfig {
    sample_rate: f64,
    max_block_size: usize,
    process_mode: ProcessMode,  // Realtime, Offline
    shared_memory: Arc<PluginSharedMemory>,
    ipc_sender: Sender<HostToPluginMsg>,
    ipc_receiver: Receiver<PluginToHostMsg>,
}

trait PluginInstance: Send {
    fn process(&mut self, block_size: usize) -> Result<ProcessResult, PluginError>;
    fn set_parameter(&mut self, id: u32, value: f32, offset: u32);
    fn get_parameter(&self, id: u32) -> f32;
    fn get_parameter_info(&self, id: u32) -> ParamInfo;
    fn get_state(&self) -> Vec<u8>;
    fn set_state(&mut self, data: &[u8]);
    fn get_presets(&self) -> Vec<PresetInfo>;
    fn load_preset(&mut self, index: usize);
    fn get_latency(&self) -> usize;
    fn get_tail_length(&self) -> usize;
    fn create_editor(&self) -> Option<Box<dyn PluginEditor>>;
    fn activate(&mut self) -> Result<(), PluginError>;
    fn deactivate(&mut self);
    fn reset(&mut self);
}
```

### CLAP Integration (via `nih-plug`)

```rust
// nih-plug provides both plugin and host-side CLAP support
use nih_plug::clap::{ClapPlugin, ClapHost};

struct ClapWrapper {
    plugin: ClapPlugin,
    entry_point: ClapEntryPoint,
}

impl PluginWrapper for ClapWrapper {
    fn create_instance(&self, config: InstanceConfig) -> Result<Box<dyn PluginInstance>, PluginError> {
        let instance = self.plugin.instantiate()?;
        instance.init(config.sample_rate, config.max_block_size)?;
        
        // Wrap in our instance adapter
        Ok(Box::new(ClapInstanceAdapter {
            instance,
            shared_mem: config.shared_memory,
            ipc_tx: config.ipc_sender,
            ipc_rx: config.ipc_receiver,
        }))
    }
}
```

### VST3 Integration (via `vst3-sys`)

```rust
// More complex due to COM, separate component/controller
use vst3_sys::{Steinberg::Vst::*, *};

struct Vst3Wrapper {
    module: Vst3Module,
    factory: *mut IPluginFactory,
    component_desc: PClassInfo,
    controller_desc: PClassInfo,
}

impl PluginWrapper for Vst3Wrapper {
    fn create_instance(&self, config: InstanceConfig) -> Result<Box<dyn PluginInstance>, PluginError> {
        // 1. Create component (audio processing)
        let component = self.factory.create_instance(&self.component_desc)?;
        let processor = component.query_interface::<IAudioProcessor>()?;
        
        // 2. Create controller (parameters, UI)
        let controller = self.factory.create_instance(&self.controller_desc)?;
        let edit_controller = controller.query_interface::<IEditController>()?;
        
        // 3. Connect controller → component
        edit_controller.set_component_handler(processor.as_component_handler())?;
        
        // 4. Initialize
        processor.setup_processing(&ProcessSetup {
            sample_rate: config.sample_rate,
            max_blocks_per_process: config.max_block_size as u32,
            process_mode: if config.process_mode == ProcessMode::Offline { kOffline } else { kRealtime },
            symbolic_sample_size: kSample32,
        })?;
        
        Ok(Box::new(Vst3InstanceAdapter { ... }))
    }
}
```

### Parameter Mapping

```rust
struct ParameterMapper {
    // Unified parameter ID space
    host_to_native: HashMap<ParamId, NativeParamId>,
    native_to_host: HashMap<NativeParamId, ParamId>,
    
    // Parameter info cache
    param_info: HashMap<ParamId, ParamInfo>,
}

impl ParameterMapper {
    fn map_clap_params(&mut self, plugin: &ClapPlugin) {
        for (i, param) in plugin.params().iter().enumerate() {
            let host_id = ParamId::new(plugin.instance_id(), i as u32);
            self.host_to_native.insert(host_id, NativeParamId::Clap(param.id()));
            self.native_to_host.insert(NativeParamId::Clap(param.id()), host_id);
            self.param_info.insert(host_id, ParamInfo::from_clap(param));
        }
    }
    
    fn map_vst3_params(&mut self, controller: &IEditController) {
        let count = controller.get_parameter_count();
        for i in 0..count {
            let mut info = ParamInfo::default();
            controller.get_parameter_info(i, &mut info);
            let host_id = ParamId::new(plugin.instance_id(), i as u32 + CLAP_OFFSET);
            self.host_to_native.insert(host_id, NativeParamId::Vst3(i));
            // ...
        }
    }
}
```

### Crash Recovery

```rust
struct PluginProcessMonitor {
    child: ChildProcess,
    shared_mem: Arc<PluginSharedMemory>,
    instance_id: PluginInstanceId,
    restart_count: u32,
    max_restarts: u32,
    saved_state: Option<Vec<u8>>,
}

impl PluginProcessMonitor {
    fn monitor(&mut self) {
        loop {
            sleep(Duration::from_millis(100));
            
            // Check process health
            match self.child.try_wait() {
                Ok(Some(exit_status)) => {
                    self.handle_crash(exit_status);
                    break;
                }
                Ok(None) => {
                    // Still running, ping
                    if !self.ping() {
                        self.kill_and_restart();
                    }
                }
                Err(e) => {
                    log::error!("Wait error: {}", e);
                    self.kill_and_restart();
                }
            }
        }
    }
    
    fn handle_crash(&mut self, exit: ExitStatus) {
        // 1. Read crash info from shared memory
        let crash_info = self.shared_mem.read_crash_info();
        
        // 2. Save plugin state if possible
        if let Some(state) = self.shared_mem.read_state() {
            self.saved_state = Some(state);
        }
        
        // 3. Notify host
        self.notify_host_crash(self.instance_id, crash_info);
        
        // 4. Restart if under limit
        if self.restart_count < self.max_restarts {
            self.restart_with_state(self.saved_state.take());
        } else {
            self.disable_plugin();
        }
    }
}
```

### UI Embedding

**CLAP**: `clap_gui` extension → `clap_window` with platform handle
**VST3**: `IPlugView::attached(parent, type)` → NSView/HWND/X11EmbedWindowID

```rust
trait PluginEditor: Send {
    fn create_window(&mut self, parent: WindowHandle, scale: f32) -> Result<EditorWindow, EditorError>;
    fn close_window(&mut self);
    fn set_bounds(&mut self, rect: Rect);
    fn set_scale(&mut self, scale: f32);
    fn is_open(&self) -> bool;
}
```

### Compatibility Matrix

| Feature | CLAP | VST3 | AUv3 (Future) |
|---------|------|------|---------------|
| Audio Processing | ✓ | ✓ | ✓ |
| Parameter Automation | ✓ | ✓ | ✓ |
| Sample-Accurate Params | ✓ | ✓ (via `setParamNormalized`) | ✓ |
| Note Expression (MPE) | ✓ | ✓ | ✓ |
| Preset Management | ✓ | ✓ | ✓ |
| Process Isolation | ✓ (native) | ✓ (via bridge) | ✓ (App Sandbox) |
| UI Embedding | ✓ | ✓ | ✓ |
| Sidechain | ✓ | ✓ | ✓ |
| Multi-timbral | ✓ | ✓ | ✓ |
| 32-bit Bridge | Via yabridge | Via jBridge | N/A |

## Configuration

```toml
# config/plugins.toml
[plugins]
scan_paths = [
    # Linux
    "/usr/lib/vst3",
    "/usr/local/lib/vst3",
    "$HOME/.vst3",
    "/usr/lib/clap",
    "/usr/local/lib/clap",
    "$HOME/.clap",
    # macOS
    "/Library/Audio/Plug-Ins/VST3",
    "$HOME/Library/Audio/Plug-Ins/VST3",
    "/Library/Audio/Plug-Ins/CLAP",
    "$HOME/Library/Audio/Plug-Ins/CLAP",
    # Windows
    "C:\\Program Files\\Common Files\\VST3",
    "C:\\Program Files (x86)\\Common Files\\VST3",
    "C:\\Program Files\\Common Files\\CLAP",
    "C:\\Program Files (x86)\\Common Files\\CLAP",
]

custom_paths = []

blocklist = [
    # Known problematic plugins
    "ProblematicPlugin.vst3",
]

sandbox = true
max_restarts = 3
health_check_interval_ms = 100
enable_32bit_bridge = false

[categories]
# User-defined category mappings
"FabFilter Pro-Q 3" = "EQ"
"Valhalla VintageVerb" = "Reverb"
```

## Testing

- **CLAP Validator**: `clap-validator` on all hosted plugins
- **VST3 Test Host**: Steinberg's `vst3_test_host`
- **Stress Test**: 50 plugins simultaneously, rapid parameter changes
- **Crash Test**: Force-crash plugin, verify host survival
- **Latency Test**: Measure round-trip with known-latency plugins