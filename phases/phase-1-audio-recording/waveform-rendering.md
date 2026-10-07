# Feature: Waveform Rendering

## Phase: 1 — Audio Recording & Editing
## ID: 1.2
## Priority: Critical
## Estimated: 2 weeks
## Status: `[~]` In Progress

## Description
Render audio waveforms at multiple zoom levels: overview (min/max per pixel) for fast scrolling, and detail view (sample-accurate) for editing. Pre-compute overviews for performance.

## Requirements

- [ ] Overview waveform: Min/max envelope per pixel column (pre-computed)
- [ ] Detail waveform: Sample-accurate drawing at high zoom
- [ ] Color: Track color, clip gain visualization, selection highlight
- [ ] RMS envelope overlay (optional)
- [ ] Silent region detection (dimmed)
- [ ] Caching: Overview stored in project, detail decoded on-demand
- [ ] Async decoding: Background thread for overview generation
- [ ] GPU-ready: Texture atlas layout for future iced/wgpu migration

## Technical Details

### Data Structures

```rust
struct WaveformOverview {
    source_id: AudioFileRefId,
    sample_rate: u32,
    channels: u16,
    frames_per_pixel: u32,        // e.g., 256 samples per pixel
    min_max: Vec<(f32, f32)>,     // Per pixel: (min, max) normalized -1..1
    rms: Option<Vec<f32>>,        // Per pixel: RMS value
    duration_pixels: u32,
}

struct WaveformDetail {
    source_id: AudioFileRefId,
    window_start: u64,            // Sample offset in source
    window_len: u32,              // Samples to render
    samples: Vec<f32>,            // Interleaved or per-channel
}
```

### Overview Generation (Background)
```
Audio File → Decode → Downsample (min/max per N samples) → Compress → Store
                    │
                    └─▶ Parallel per channel → Interleave min/max
```

Algorithm:
```rust
fn generate_overview(samples: &[f32], frames_per_pixel: usize) -> Vec<(f32, f32)> {
    samples
        .chunks(frames_per_pixel)
        .map(|chunk| {
            let min = chunk.iter().copied().fold(f32::INFINITY, f32::min);
            let max = chunk.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            (min, max)
        })
        .collect()
}
```

### Rendering (egui)
```rust
fn paint_waveform(ui: &mut Ui, clip: &AudioClip, overview: &WaveformOverview, zoom: f32) {
    let rect = ui.available_rect_before_wrap();
    let painter = ui.painter();
    
    // Calculate visible pixel range
    let start_px = (clip.timeline_position.sample as f32 * zoom) as usize;
    let end_px = start_px + rect.width() as usize;
    
    // Draw min/max envelope
    for px in start_px..end_px.min(overview.min_max.len()) {
        let (min, max) = overview.min_max[px];
        let y_min = remap(min, -1.0, 1.0, rect.bottom(), rect.top());
        let y_max = remap(max, -1.0, 1.0, rect.bottom(), rect.top());
        painter.line_segment([pos_min, pos_max], stroke);
    }
    
    // Draw clip gain overlay
    // Draw fades
    // Draw selection highlight
}
```

### Caching Strategy
- Overview: Generated once per audio file, stored in project (.zoft)
- Detail: Decoded on-demand when zoom > 1:1
- LRU cache for detail buffers (max 50 MB)

## Acceptance Criteria

- [ ] Overview generates in < 1s per minute of audio (background)
- [ ] Scrolling 100 tracks at 60 FPS (overview only)
- [ ] Zoom to sample level shows individual samples
- [ ] Clip gain changes reflected visually
- [ ] Fade in/out curves visible
- [ ] Selection highlights waveform region
- [ ] Memory usage < 200 MB for 1-hour project

## Progress Log

- 2026-10-07: Overview generation algorithm implemented
- 2026-10-07: Background generation thread structure
- 2026-10-07: WaveformOverview and WaveformCache structures
- 2026-10-07: Basic timeline rendering with track lanes
- Next: Connect clip data access, implement waveform drawing