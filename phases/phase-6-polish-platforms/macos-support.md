# Feature: macOS Support

## Phase: 6 — Polish & Platform Expansion
## ID: 6.1
## Priority: Medium
## Estimated: 4 weeks
## Status: `[ ]` Not Started
## Depends On: 5.1

## Description
Native macOS support: Core Audio, AUv3, notarization, Apple Silicon, Metal rendering, system integration.

## Requirements

- [ ] Core Audio backend: Replace cpal with direct Core Audio for lower latency
- [ ] Audio Unit v3: Host AUv3 plugins, validate, sandbox
- [ ] Notarization: Hardened runtime, codesign, notarize, staple
- [ ] Apple Silicon: Native arm64, Rosetta 2 for x86 plugins
- [ ] Metal rendering: iced/wgpu Metal backend for UI
- [ ] System integration: Menu bar, Dock, Fullscreen, Touch Bar (deprecated but supported)
- [ ] File associations: `.zoft` files open in Zoft
- [ ] Accessibility: VoiceOver, keyboard navigation
- [ ] Sandbox: App Sandbox entitlements, microphone access
- [ ] Sparkle updates: Auto-update framework

## Technical Details

### Core Audio Backend
```rust
// Replace cpal with coreaudio-sys for pro features
use coreaudio_sys::*;

struct CoreAudioEngine {
    device: AudioDeviceID,
    io_proc: AudioDeviceIOProcID,
    stream_format: AudioStreamBasicDescription,
    buffer_size: u32,
    sample_rate: f64,
}

impl CoreAudioEngine {
    fn create_io_proc(&self) -> AudioDeviceIOProcID {
        let callback: AudioDeviceIOProc = |_, _, input, _, output, _, user_data| {
            let engine = user_data as *mut CoreAudioEngine;
            unsafe { (*engine).process(input, output) }
        };
        AudioDeviceCreateIOProcID(self.device, callback, self as *mut _ as _)
    }
}
```

### AUv3 Hosting
```rust
// AUv3 = Audio Unit v3 (AudioComponent, AUAudioUnit)
// Different from VST3/CLAP - uses Apple's AudioToolbox

struct AuV3Wrapper {
    component: AUAudioUnit,
    view_controller: AUViewController,
    parameter_tree: AUParameterTree,
}

impl PluginWrapper for AuV3Wrapper {
    fn create_instance(&self, sample_rate: f64, max_block: usize) -> Box<dyn PluginInstance> {
        let desc = AudioComponentDescription {
            componentType: kAudioUnitType_Effect, // or Generator for instruments
            componentSubType: self.component.componentDescription.componentSubType,
            componentManufacturer: self.component.componentDescription.componentManufacturer,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        
        let au = AUAudioUnit::instantiate_with_description(desc)?;
        au.maximumFramesToRender = max_block as u32;
        
        Box::new(AuV3Instance { au })
    }
}
```

### Notarization Pipeline
```yaml
# GitHub Actions
- name: Build Release
  run: cargo build --release --target aarch64-apple-darwin
          --target x86_64-apple-darwin

- name: Create Universal Binary
  run: lipo -create target/aarch64/release/zoft target/x86_64/release/zoft -output zoft

- name: Codesign
  run: |
    codesign --force --options runtime --sign "Developer ID Application: ..." \
      --entitlements entitlements.plist zoft
    codesign --force --sign "Developer ID Application: ..." zoft.app

- name: Notarize
  run: |
    xcrun notarytool submit zoft.zip --apple-id $APPLE_ID --team-id $TEAM_ID --password $PASS --wait

- name: Staple
  run: xcrun stapler staple zoft.app
```

### Entitlements
```xml
<!-- entitlements.plist -->
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.allow-jit</key><true/>
    <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
    <key>com.apple.security.device.audio-input</key><true/>
    <key>com.apple.security.files.user-selected.read-write</key><true/>
</dict>
</plist>
```

### Universal Binary
- Build for `aarch64-apple-darwin` (Apple Silicon) and `x86_64-apple-darwin` (Intel)
- Use `lipo` to combine
- Test on both architectures

## Acceptance Criteria

- [ ] Core Audio: < 2ms latency at 48kHz/128
- [ ] AUv3 plugins load, process, automate
- [ ] Notarized app passes Gatekeeper
- [ ] Native arm64 + Rosetta x86 plugins
- [ ] Metal UI renders at 60 FPS
- [ ] Menu bar, Dock, Fullscreen work
- [ ] `.zoft` files open Zoft
- [ ] VoiceOver navigates UI
- [ ] Sparkle updates work

## Progress Log

- YYYY-MM-DD: Core Audio backend
- YYYY-MM-DD: AUv3 wrapper
- YYYY-MM-DD: Notarization pipeline
- YYYY-MM-DD: Universal binary
- YYYY-MM-DD: Metal UI backend
- YYYY-MM-DD: System integration
- YYYY-MM-DD: File associations
- YYYY-MM-DD: Accessibility
- YYYY-MM-DD: Sparkle updates