# Feature: Sandbox & Crash Recovery

## Phase: 5 — Plugin Hosting
## ID: 5.3
## Priority: Critical
## Estimated: 3 weeks
## Status: `[ ]` Not Started
## Depends On: 5.2

## Description
Process-per-plugin sandbox: isolate crashes, shared memory audio buffers, automatic restart, state preservation.

## Requirements

- [ ] Process isolation: Each plugin in separate process
- [ ] Shared memory: Audio buffers, parameters, events
- [ ] IPC: Host ↔ Plugin process communication
- [ ] Crash detection: Monitor child process, detect exit/crash
- [ ] Automatic restart: Reload plugin, restore state
- [ ] State preservation: Save plugin state before crash
- [ ] UI process: Separate process for plugin editor (optional)
- [ ] Resource limits: CPU, memory per plugin
- [ ] Permission sandbox: File system, network (optional)
- [ ] Bridge: 32-bit plugins on 64-bit host (via yabridge-style)

## Technical Details

### Sandbox Architecture
```
Host Process (DAW)                    Plugin Process (per plugin)
┌─────────────────────────┐           ┌─────────────────────────┐
│ PluginHost             │           │ PluginWrapper           │
│  - PluginInstance      │◀──IPC──▶│  - CLAP/VST3 entry       │
│  - SharedMemPool       │           │  - Plugin instance      │
│  - ProcessMonitor      │           │  - Audio buffers (shmem)│
└─────────────────────────┘           └─────────────────────────┘
```

### Shared Memory Layout
```rust
#[repr(C)]
struct SharedAudioRegion {
    // Ring buffer for parameter changes
    param_ring: RingBuffer<ParameterChange, 1024>,
    
    // Audio buffers (double-buffered)
    input_buffers: [AudioBuffer; MAX_CHANNELS],
    output_buffers: [AudioBuffer; MAX_CHANNELS],
    
    // Events (MIDI, note expression)
    event_ring: RingBuffer<Event, 2048>,
    
    // State
    process_state: ProcessState,      // Running, Suspended, Error
    latency: u32,
    tail_length: u32,
    
    // Crash info
    last_crash: Option<CrashInfo>,
}

struct CrashInfo {
    timestamp: SystemTime,
    signal: i32,                      // SIGSEGV, SIGABRT, etc.
    backtrace: String,                // If available
    plugin_state: Vec<u8>,            // Saved before crash
}
```

### IPC Protocol (Message Passing)
```rust
enum HostToPluginMsg {
    Process { block_size: usize, events: Vec<Event> },
    SetParam { id: u32, value: f32, offset: u32 },
    GetParam { id: u32 },
    SaveState,
    LoadState { data: Vec<u8> },
    Activate,
    Deactivate,
    SetLatency { latency: u32 },
    Ping,                             // Health check
    Shutdown,
}

enum PluginToHostMsg {
    ProcessDone { output_buffers: Vec<AudioBuffer>, events: Vec<Event> },
    ParamValue { id: u32, value: f32 },
    StateData { data: Vec<u8> },
    StateSaved,
    Error { code: u32, message: String },
    Crashed { info: CrashInfo },
    Pong,
}
```

### Process Monitor
```rust
struct ProcessMonitor {
    child: ChildProcess,
    shared_mem: Arc<SharedAudioRegion>,
    restart_count: u32,
    max_restarts: u32,                // Default 3
    health_check_interval: Duration,  // 100ms
}

impl ProcessMonitor {
    fn monitor(&mut self) {
        loop {
            sleep(self.health_check_interval);
            
            // Check if process alive
            if let Some(exit_status) = self.child.try_wait()? {
                // Process exited
                let crash_info = self.shared_mem.read_crash_info();
                self.handle_crash(exit_status, crash_info);
                break;
            }
            
            // Ping for responsiveness
            if !self.ping() {
                self.kill_and_restart();
            }
        }
    }
    
    fn handle_crash(&mut self, exit: ExitStatus, crash: Option<CrashInfo>) {
        if self.restart_count < self.max_restarts {
            self.restart_with_state(crash.and_then(|c| c.plugin_state));
        } else {
            // Disable plugin, notify UI
            self.notify_ui_plugin_failed();
        }
    }
}
```

### 32-bit Bridge (Optional)
- Use `yabridge` approach: 32-bit host process loads 32-bit plugin
- 64-bit DAW ↔ 32-bit bridge ↔ 32-bit plugin
- IPC via shared memory + pipes

## Acceptance Criteria

- [ ] Plugin crash → DAW continues running
- [ ] Auto-restart: plugin reloads, state restored
- [ ] Max 3 restarts, then disable with notification
- [ ] Shared memory: zero-copy audio buffers
- [ ] IPC latency < 0.1ms
- [ ] CPU/memory limits enforced
- [ ] 32-bit plugins work (if implemented)
- [ ] Crash report saved for debugging

## Progress Log

- YYYY-MM-DD: Process spawning + shared mem
- YYYY-MM-DD: IPC protocol + serialization
- YYYY-MM-DD: Audio processing via sandbox
- YYYY-MM-DD: Crash detection + restart
- YYYY-MM-DD: State save/restore on crash
- YYYY-MM-DD: Health monitoring
- YYYY-MM-DD: Resource limits
- YYYY-MM-DD: 32-bit bridge (optional)