# Feature: Bounce & Export

## Phase: 4 — Mixing & Automation
## ID: 4.4
## Priority: High
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 4.1

## Description
Export audio: Mixdown, stem export, batch export, video sync, dither, normalization, metadata, render in place.

## Requirements

- [ ] Mixdown: Master bus → stereo/multichannel file
- [ ] Stem export: Per-track, per-bus, per-folder, custom groups
- [ ] Batch export: Multiple regions/markers, naming patterns
- [ ] Render in place: Track with inserts → new audio clip
- [ ] Video sync: Export with video (copy video stream, replace audio)
- [ ] Sample rate conversion: High-quality SRC (SoXR)
- [ ] Bit depth: 16/24/32-bit float, dither (TPDF, shaped)
- [ ] Normalization: Peak, LUFS (integrated, true peak), EBU R128
- [ ] Metadata: BWF bext, ID3, Vorbis comments, artwork
- [ ] Real-time vs Offline: Real-time for hardware inserts, offline for speed
- [ ] Export queue: Multiple jobs, progress, cancel
- [ ] Import back: Auto-import bounced files to project

## Technical Details

### Export Configuration
```rust
struct ExportConfig {
    source: ExportSource,
    format: ExportFormat,
    sample_rate: u32,
    bit_depth: BitDepth,
    dither: DitherType,
    normalization: Normalization,
    metadata: ExportMetadata,
    real_time: bool,
    include_tail: bool,           // Reverb/delay tails
    loop_markers: bool,           // Add loop points from markers
}

enum ExportSource {
    MasterBus,
    Tracks(Vec<TrackId>),
    Buses(Vec<BusId>),
    Folders(Vec<TrackId>),
    CustomGroups(Vec<ExportGroup>),
    Region { start: TimelinePos, end: TimelinePos },
    MarkerRange { start: MarkerId, end: MarkerId },
    CycleRange,                   // Loop range
}

enum ExportFormat {
    WAV { bwf: bool },
    AIFF,
    FLAC { compression: u8 },
    MP3 { bitrate: u32 },
    OGG { quality: f32 },
    MP4 { video_track: Option<VideoTrack> },  // With video
}

enum DitherType {
    None,
    TPDF,
    Shaped { noise_shaping: NoiseShaping },
}

enum Normalization {
    None,
    Peak { target_db: f32 },
    LUFS { target_lufs: f32, true_peak_db: f32 },
    EBU_R128,
}
```

### Offline Bounce Engine
```rust
struct OfflineBounce {
    config: ExportConfig,
    project: Project,
    engine: OfflineAudioEngine,
    progress: ProgressReporter,
}

impl OfflineBounce {
    fn run(&mut self) -> Result<Vec<ExportResult>, BounceError> {
        // 1. Prepare project: resolve routing, compile automation
        // 2. Create offline engine (no cpal, max speed)
        // 3. For each export source:
        //    a. Configure engine routing
        //    b. Process blocks until end
        //    c. Write to file with metadata
        //    d. Report progress
        // 4. Return results
    }
}
```

### Real-Time Bounce (for Hardware Inserts)
```rust
struct RealtimeBounce {
    config: ExportConfig,
    engine: RealtimeAudioEngine,  // Uses cpal
    recorder: DiskRecorder,
    latency_compensation: usize,
}

impl RealtimeBounce {
    fn run(&mut self) -> Result<ExportResult, BounceError> {
        // 1. Measure total latency (hardware inserts)
        // 2. Start engine + recorder
        // 3. Wait for duration + tail
        // 4. Stop, finalize file
    }
}
```

### Stem Export
```rust
struct StemExporter {
    groups: Vec<StemGroup>,
    config: ExportConfig,
}

struct StemGroup {
    name: String,
    tracks: Vec<TrackId>,
    solo_safe: bool,              // Ignore solo when bouncing
    include_effects: bool,        // Include track inserts
    include_sends: bool,          // Include send effects
}
```

### Video Export
```rust
struct VideoExporter {
    video_path: PathBuf,
    audio_config: ExportConfig,
    ffmpeg: FFmpegWrapper,
}

impl VideoExporter {
    fn export(&self, output_path: PathBuf) -> Result<(), VideoExportError> {
        // 1. Bounce audio to temp WAV
        // 2. ffmpeg -i video.mp4 -i audio.wav -c:v copy -c:a pcm_s24le -map 0:v -map 1:a output.mp4
        // 3. Cleanup temp
    }
}
```

### Render in Place
```rust
fn render_in_place(track_id: TrackId, config: RenderConfig) -> Result<AudioClip, RenderError> {
    // 1. Create offline bounce of single track (with inserts, no sends)
    // 2. Write to new audio file in project folder
    // 3. Create new audio clip referencing file
    // 4. Disable original track inserts, mute original clips
    // 5. Add new clip to track
    // 6. Undoable command
}
```

## Acceptance Criteria

- [ ] Mixdown: stereo WAV, correct length, no clips
- [ ] Stems: each group separate file, aligned start
- [ ] Batch: marker regions export with naming pattern
- [ ] Render in place: sounds identical, original preserved
- [ ] Video: audio replaced, video unchanged, sync perfect
- [ ] SRC: 44.1k → 48k transparent quality
- [ ] Dither: 24→16 bit no truncation distortion
- [ ] Normalize: LUFS -14, true peak -1 dBTP
- [ ] Metadata: BWF bext + ID3 written
- [ ] Queue: multiple exports, cancel works

## Progress Log

- YYYY-MM-DD: Offline bounce engine
- YYYY-MM-DD: Real-time bounce (hardware)
- YYYY-MM-DD: Stem export
- YYYY-MM-DD: Batch export + naming
- YYYY-MM-DD: Render in place
- YYYY-MM-DD: Video export (ffmpeg)
- YYYY-MM-DD: SRC + dither + normalize
- YYYY-MM-DD: Metadata writing
- YYYY-MM-DD: Export queue UI