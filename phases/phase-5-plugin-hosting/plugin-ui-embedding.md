# Feature: Plugin UI Embedding

## Phase: 5 — Plugin Hosting
## ID: 5.4
## Priority: High
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 5.1, 5.3

## Description
Embed plugin editors in DAW window: native window parenting, resize handling, keyboard focus, HiDPI, multiple open editors.

## Requirements

- [ ] CLAP GUI: `clap_gui` extension, create window, set parent, resize
- [ ] VST3 GUI: `IPlugView`, `attached`, `removed`, `onSize`, `setFrame`
- [ ] Window parenting: X11/Wayland (Linux), Cocoa (macOS), HWND (Windows)
- [ ] Resize: Plugin requests size, host constrains, scroll if needed
- [ ] Keyboard focus: Tab navigation, accelerator keys, modal dialogs
- [ ] HiDPI: Scale factor from host, plugin renders at correct DPI
- [ ] Multiple editors: Tabbed or floating, bring to front
- [ ] Editor state: Remember position/size per plugin
- [ ] Generic editor: Fallback for plugins without UI (parameter sliders)

## Technical Details

### Editor Trait
```rust
trait PluginEditor: Send {
    fn create_window(&mut self, parent: WindowHandle, scale: f32) -> Result<EditorWindow, EditorError>;
    fn close_window(&mut self);
    fn set_bounds(&mut self, rect: Rect);
    fn get_bounds(&self) -> Rect;
    fn set_scale(&mut self, scale: f32);
    fn bring_to_front(&mut self);
    fn is_open(&self) -> bool;
}

struct EditorWindow {
    handle: WindowHandle,
    plugin_id: PluginInstanceId,
    bounds: Rect,
    scale: f32,
    is_floating: bool,
    tab_index: Option<usize>,
}
```

### Platform Window Handles
```rust
enum WindowHandle {
    X11(x11::Window),
    Wayland(wayland::WlSurface),
    Cocoa(cocoa::NSView),
    Windows(HWND),
    // For egui/iced: use raw window handle crate
    Raw(raw_window_handle::RawWindowHandle),
}
```

### CLAP GUI Embedding
```rust
// CLAP: host provides `clap_host_gui` with `create_window`, `destroy_window`
// Plugin calls `host->gui->create_window(host, &clap_window)` 
// Returns `clap_window` with `hwnd` (platform handle)

// Host implementation:
fn create_clap_window(&self, plugin: &ClapPlugin, parent: WindowHandle, scale: f32) -> ClapWindow {
    let window = create_platform_window(parent, plugin.desired_size());
    window.set_scale(scale);
    
    let clap_window = ClapWindow {
        hwnd: window.as_raw_handle(),
        // ... other fields
    };
    
    // Plugin will call `gui->set_size` etc.
    clap_window
}
```

### VST3 View Embedding
```rust
// VST3: IPlugView::attached(void* parent, FIDString type)
// type: "NSView" (macOS), "HWND" (Windows), "X11EmbedWindowID" (Linux)

fn attach_vst3_view(view: &mut IPlugView, parent: WindowHandle) -> Result<(), Vst3Error> {
    let (type_str, handle) = match parent {
        WindowHandle::Cocoa(nsview) => ("NSView", nsview as *mut c_void),
        WindowHandle::Windows(hwnd) => ("HWND", hwnd as *mut c_void),
        WindowHandle::X11(xid) => ("X11EmbedWindowID", &xid as *const _ as *mut c_void),
        WindowHandle::Wayland(_) => return Err(Vst3Error::UnsupportedPlatform), // Use X11 fallback
        _ => return Err(Vst3Error::UnsupportedPlatform),
    };
    
    unsafe { view.attached(handle, type_str.as_ptr()) }
}
```

### Generic Editor Fallback
```rust
struct GenericEditor {
    plugin: PluginInstanceId,
    params: Vec<ParamId>,
    layout: GenericLayout,
}

impl GenericEditor {
    fn build_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            for param_id in &self.params {
                if let Some(info) = self.get_param_info(*param_id) {
                    ParamSlider::new(info).ui(ui);
                }
            }
        });
    }
}
```

### Editor Manager
```rust
struct EditorManager {
    editors: HashMap<PluginInstanceId, Box<dyn PluginEditor>>,
    open_windows: Vec<EditorWindow>,
    tab_bar: Option<TabBar>,
}

impl EditorManager {
    fn open_editor(&mut self, plugin_id: PluginInstanceId, floating: bool) {
        if let Some(editor) = self.editors.get_mut(&plugin_id) {
            let window = editor.create_window(self.main_window_handle(), self.current_scale())?;
            self.open_windows.push(EditorWindow { ..., floating, tab_index: ... });
        }
    }
    
    fn close_all(&mut self) {
        for editor in self.editors.values_mut() {
            editor.close_window();
        }
        self.open_windows.clear();
    }
}
```

## Acceptance Criteria

- [ ] CLAP plugin UI embeds, resizes, receives keyboard
- [ ] VST3 plugin UI embeds (all 3 platforms)
- [ ] HiDPI: plugins render sharp at 150%/200%
- [ ] Multiple editors: tabbed + floating
- [ ] Position/size remembered per plugin
- [ ] Generic editor works for headless plugins
- [ ] Modal dialogs (file open) work
- [ ] Close project → all editors close cleanly

## Progress Log

- YYYY-MM-DD: Window handle abstraction
- YYYY-MM-DD: CLAP GUI embedding
- YYYY-MM-DD: VST3 view embedding (Linux)
- YYYY-MM-DD: VST3 view embedding (macOS)
- YYYY-MM-DD: VST3 view embedding (Windows)
- YYYY-MM-DD: HiDPI scaling
- YYYY-MM-DD: Multi-editor tab/float
- YYYY-MM-DD: Generic editor fallback
- YYYY-MM-DD: Focus + modal handling