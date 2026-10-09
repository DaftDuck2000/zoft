# Feature: Audio Device Enumeration (libudev + optional PipeWire)

## Phase: 1 — Audio Recording & Editing
## ID: 1.6
## Priority: High
## Estimated: 2 weeks
## Status: `[~]` In Progress

## Description
Implement cross-platform audio device enumeration with user-friendly device names. Uses a hybrid approach:
- **Primary (Linux)**: Try PipeWire metadata for best device names (descriptions, icons, priorities)
- **Fallback (Linux)**: ALSA + libudev for hardware info (vendor, model, serial)
- **Windows/macOS**: Use cpal's native device names

## Requirements

- [ ] Add `libudev` and `regex` dependencies
- [ ] Add optional `pipewire` feature flag
- [ ] Create `DeviceEnumerator` trait for cross-platform abstraction
- [ ] Implement `UdevDeviceEnumerator` for Linux (libudev)
- [ ] Implement optional `PipeWireDeviceEnumerator` (feature-gated)
- [ ] Implement fallback to cpal device names
- [ ] Update `AudioDeviceInfo` with `raw_name` field for matching
- [ ] Update `clean_device_name` for user-friendly display names
- [ ] Add device enumeration to audio backend
- [ ] Update UI device selector to use new enumeration

## Technical Details

### Device Enumeration Strategy

```
┌─────────────────────────────────────────────────────────────┐
│                    DeviceEnumerator Trait                    │
├─────────────────────────────────────────────────────────────┤
│ enumerate_input_devices() -> Result<Vec<AudioDeviceInfo>>   │
│ enumerate_output_devices() -> Result<Vec<AudioDeviceInfo>>  │
└─────────────────────────────────────────────────────────────┘
                    │                    │
                    ▼                    ▼
┌─────────────────────────────┐ ┌─────────────────────────────┐
│   UdevDeviceEnumerator      │ │  PipeWireDeviceEnumerator   │
│   (Linux, always available) │ │  (Optional, feature-gated)  │
│   - libudev query           │ │  - libpipewire query        │
│   - Vendor/Model/Serial     │ │  - Descriptions, icons      │
│   - ALSA device matching    │ │  - Priorities, profiles     │
└─────────────────────────────┘ └─────────────────────────────┘
                    │                    │
                    └────────┬───────────┘
                             ▼
              ┌─────────────────────────────┐
              │   AudioBackend              │
              │   - input_devices()         │
              │   - output_devices()        │
              │   - start_input_stream()    │
              │   - stop_input_stream()     │
              └─────────────────────────────┘
```

### AudioDeviceInfo Enhancement

```rust
pub struct AudioDeviceInfo {
    pub id: String,                    // Unique ID (e.g., "input_0_Fifine_Microphone")
    pub name: String,                  // Clean display name ("Fifine Microphone")
    pub raw_name: String,              // Raw ALSA/PipeWire name for matching
    pub is_default_input: bool,
    pub is_default_output: bool,
    pub max_input_channels: u32,
    pub max_output_channels: u32,
    pub sample_rates: Vec<u32>,
}
```

### Name Cleaning Rules

1. Remove ALSA prefixes: `surround51:`, `front:`, `iec958:`, etc.
2. Remove ALSA identifiers: `CARD=Generic_1,DEV=0`
3. Remove common suffixes: `surround`, `stereo`, `iec958`, `hdmi`, `spdif`, `digital`, `analog`, `codec`, `dac`, `adc`
4. Remove generic terms: `generic`, `audio`, `sound`, `device`, `card`, `input`, `output`
4. Capitalize words properly
5. Fallback: "Audio Device"

## Acceptance Criteria

- [ ] Device dropdown shows "Fifine Microphone" instead of "surround51:CARD=Generic_1,DEV=0"
- [ ] All devices listed with meaningful names
- [ ] Default device marked correctly
- [ ] Recording works with selected device
- [ ] No regression on Windows/macOS
- [ ] PipeWire metadata used when available (feature-gated)

## Progress Log

- 2026-10-09: Added feature to plan
- 2026-10-09: Started implementation