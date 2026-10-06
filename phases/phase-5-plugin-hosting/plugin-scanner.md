# Feature: Plugin Scanner & Manager

## Phase: 5 — Plugin Hosting
## ID: 5.2
## Priority: Critical
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 5.1

## Description
Recursive plugin scanner with validation, categorization, blocklist, favorites, and fast startup caching.

## Requirements

- [ ] Scan paths: VST3 (standard locations), CLAP (standard), custom paths
- [ ] Recursive scan with progress UI
- [ ] Validation: Load each plugin, check compatibility, measure latency
- [ ] Categorization: Effect/Instrument, vendor, format, tags
- [ ] Blocklist: Crash plugins, denylist by ID
- [ ] Favorites: User-starred, top of browser
- [ ] Cache: Binary index (plugin metadata), invalidates on mtime change
- [ ] Background scan: Non-blocking UI, incremental
- [ ] Plugin info: Name, version, vendor, description, I/O, parameters
- [ ] Search: Fuzzy search by name, vendor, tag
- [ ] Drag-drop: From browser to track insert

## Technical Details

### Scanner Architecture
```rust
struct PluginScanner {
    search_paths: Vec<PathBuf>,
    cache: PluginCache,
    blocklist: HashSet<PluginId>,
    favorites: HashSet<PluginId>,
    tx: Sender<ScanEvent>,
}

#[derive(Serialize, Deserialize)]
struct PluginCache {
    version: u32,
    plugins: HashMap<PluginId, CachedPluginInfo>,
    scan_timestamp: SystemTime,
}

struct CachedPluginInfo {
    id: PluginId,
    path: PathBuf,
    name: String,
    vendor: String,
    version: String,
    format: PluginFormat,      // CLAP, VST3
    category: PluginCategory,
    i_o: (u32, u32),           // Inputs, outputs
    param_count: u32,
    latency: usize,
    mtime: SystemTime,         // For cache invalidation
    valid: bool,               // Loaded successfully
    tags: Vec<String>,
}

enum ScanEvent {
    Started { total_paths: usize },
    ScanningPath { path: PathBuf, current: usize, total: usize },
    FoundPlugin { info: CachedPluginInfo },
    PluginError { path: PathBuf, error: String },
    Finished { plugins_found: usize, errors: usize },
}
```

### Standard Scan Paths
| Platform | VST3 | CLAP |
|----------|------|------|
| Linux | `/usr/lib/vst3`, `/usr/local/lib/vst3`, `~/.vst3`, `~/.local/lib/vst3` | `/usr/lib/clap`, `/usr/local/lib/clap`, `~/.clap`, `~/.local/lib/clap` |
| macOS | `/Library/Audio/Plug-Ins/VST3`, `~/Library/Audio/Plug-Ins/VST3` | `/Library/Audio/Plug-Ins/CLAP`, `~/Library/Audio/Plug-Ins/CLAP` |
| Windows | `C:\Program Files\Common Files\VST3`, `C:\Program Files (x86)\Common Files\VST3` | `C:\Program Files\Common Files\CLAP`, `C:\Program Files (x86)\Common Files\CLAP` |

### Validation Process
```rust
fn validate_plugin(path: &Path) -> Result<CachedPluginInfo, ScanError> {
    // 1. Load library
    let wrapper = PluginWrapper::load(path)?;
    
    // 2. Create instance at 48kHz/512
    let instance = wrapper.create_instance(48000.0, 512)?;
    
    // 3. Get info
    let info = instance.info();
    
    // 4. Test process (silence)
    let mut buf = [0.0; 512];
    instance.process(&mut [buf], &mut [buf], &[], &default_ctx());
    
    // 5. Measure latency
    let latency = instance.get_latency();
    
    // 6. Get parameters
    let params = instance.param_infos();
    
    Ok(CachedPluginInfo { ... })
}
```

### Browser UI
```rust
struct PluginBrowser {
    plugins: Vec<CachedPluginInfo>,
    filter: BrowserFilter,
    search: String,
    selected: Option<PluginId>,
}

struct BrowserFilter {
    format: Option<PluginFormat>,
    category: Option<PluginCategory>,
    vendor: Option<String>,
    favorites_only: bool,
    valid_only: bool,
}
```

## Acceptance Criteria

- [ ] Scans all standard paths on startup (cached < 2s)
- [ ] Full rescan with progress UI
- [ ] Invalid plugins → blocklist, not rescanned
- [ ] Categories: EQ, Dynamics, Reverb, Delay, Synth, Sampler, etc.
- [ ] Favorites persist, show at top
- [ ] Search: fuzzy, instant (< 50ms)
- [ ] Drag to track → loads plugin
- [ ] Cache updates on file change

## Progress Log

- YYYY-MM-DD: Scanner core + cache
- YYYY-MM-DD: Standard path detection
- YYYY-MM-DD: Validation + blocklist
- YYYY-MM-DD: Categorization + tags
- YYYY-MM-DD: Favorites
- YYYY-MM-DD: Browser UI + search
- YYYY-MM-DD: Drag-drop to track
- YYYY-MM-DD: Background incremental scan