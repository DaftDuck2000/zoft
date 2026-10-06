# Feature: Project Save/Load

## Phase: 0 — Foundation
## ID: 0.5
## Priority: Critical
## Estimated: 1 week
## Status: `[ ]` Not Started
## Depends On: 0.1

## Description
Implement project serialization: binary `.zoft` format with versioning, migration, and fast load/save. Support for audio file references (not embedded).

## Requirements

- [ ] Binary format: `.zoft` (magic, version, sections)
- [ ] Serialize: Project metadata, tracks, clips, tempo map, automation
- [ ] Audio files: Referenced by relative path, not embedded
- [ ] Versioning: Schema version per section, migration on load
- [ ] Compression: `lz4` for large sections (automation, clip data)
- [ ] Recent projects menu (last 10)
- [ ] Auto-save: Configurable interval, crash recovery
- [ ] Project templates: Empty, Band, Electronic, Scoring

## Technical Details

### File Format: `.zoft`

```
Header (64 bytes):
  - Magic: "ZOFT" (4 bytes)
  - Version: u32 (file format version)
  - Schema: u32 (data schema version)
  - UUID: [u8; 16] (project ID)
  - Section count: u32
  - Section table offset: u64

Section Table (variable):
  - SectionType: u32
  - Offset: u64
  - Compressed size: u64
  - Uncompressed size: u64
  - Checksum: u32 (xxHash)

Section Types:
  0x01 = ProjectMeta (name, sample_rate, bit_depth, preferences)
  0x02 = TempoMap
  0x03 = TimeSignatureMap
  0x04 = Tracks (all tracks + clips)
  0x05 = AutomationLanes
  0x06 = PluginInstances
  0x07 = MarkerRegions
  0x08 = AudioPool (file references + metadata)
  0x09 = UndoHistory (optional, for crash recovery)
```

### Serialization
- Use `bincode` for binary encoding (fast, compact)
- `serde` with custom serializers for domain types
- Section-level versioning: each section has its own schema version
- Migration: `migrate_section(section_type, from_version, to_version, data)`

### Audio Pool
```rust
struct AudioFileRef {
    id: Uuid,
    path: PathBuf,           // Relative to project folder
    original_path: PathBuf,  // For missing file resolution
    duration_samples: u64,
    sample_rate: u32,
    channels: u16,
    bit_depth: BitDepth,
    metadata: AudioMetadata, // BWF bext chunk data
}
```

### Auto-Save
- Background thread: serialize to `.zoft.autosave` every N minutes
- On startup: if `.zoft.autosave` exists and newer than `.zoft`, prompt recovery
- Cleanup: remove autosave on clean save/close

## Acceptance Criteria

- [ ] Save project → `.zoft` file created
- [ ] Load project → identical state restored
- [ ] Audio files referenced correctly (relative paths)
- [ ] Version migration works (test with old schema)
- [ ] Auto-save creates recovery file
- [ ] Large projects (>100 tracks) save < 2s, load < 3s
- [ ] Corrupted file → graceful error, not panic

## Progress Log

- YYYY-MM-DD: Binary format defined
- YYYY-MM-DD: Serialization working for core types
- YYYY-MM-DD: Section versioning + migration
- YYYY-MM-DD: Audio pool references
- YYYY-MM-DD: Auto-save + recovery
- YYYY-MM-DD: Templates implemented