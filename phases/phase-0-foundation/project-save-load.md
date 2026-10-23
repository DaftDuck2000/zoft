# Feature: Project Save/Load

## Phase: 0 — Foundation
## ID: 0.5
## Priority: Critical
## Estimated: 1 week
## Status: `[~]` In Progress

## Description
Implement project serialization: binary `.zoft` format with versioning, migration, and fast load/save. Support for audio file references (not embedded). The app exposes File → New/Open/Save/Save As (Ctrl+S / Ctrl+Shift+S / Ctrl+O); recent projects, auto-save and templates are still pending.

## Requirements

- [✓] Binary format: `.zoft` (magic, version, sections)
- [✓] Serialize: Project metadata, tracks, clips, tempo map, automation
- [✓] Audio files: Referenced by relative path, not embedded
- [✓] Versioning: Schema version per section, migration on load
- [✓] Compression: `lz4` for large sections (automation, clip data)
- [✓] Save/Open UI wired into the app (File menu and Ctrl+S / Ctrl+Shift+S / Ctrl+O)
- [✓] Recent projects menu (last 10, stored in the user data directory)
- [✓] Auto-save: every 2 minutes while unsaved changes exist; a newer sibling `.autosave.zoft` is offered when opening the project
- [x] Project templates: Empty (1 audio track), Band (Drums/Bass/Guitar/Keys/Vocals), Electronic (Kick/Bass/Synth/FX), Scoring (Strings/Brass/Woodwinds/Percussion); File → New from Template

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

- [✓] Save project → `.zoft` file created
- [✓] Load project → identical state restored (covered by a round-trip test in `serialization.rs`)
- [✓] Audio files referenced correctly (relative paths)
- [~] Version migration works (migration framework exists; no old-schema fixture test yet)
- [x] Auto-save creates a sibling `<name>.autosave.zoft` every 2 minutes while edits are unsaved; opening a project offers a newer recovery copy
- [ ] Large projects (>100 tracks) save < 2s, load < 3s (not benchmarked)
- [~] Corrupted file → graceful error, not panic (magic/checksum errors are reported; not exhaustively tested)

## Progress Log

- 2026-10-07: Binary format defined with magic, version, sections
- 2026-10-07: Serialization working for core types (bincode + lz4 + xxHash)
- 2026-10-07: Section versioning + migration framework
- 2026-10-07: Audio pool references (relative paths)
- 2026-10-07: Project save/load serializer implemented in `daw-core/src/serialization.rs` (API only; no UI Save/Open yet)
- 2026-10-21: Status corrected to In Progress: the app launches a new in-memory project and does not yet wire Save/Open, recent projects, auto-save or templates
- 2026-10-23: Fixed the `.zoft` loader (it previously over-read fixed-size header/section-table blocks and failed); added a round-trip test and wired File → New/Open/Save/Save As plus Ctrl+S / Ctrl+Shift+S / Ctrl+O
- 2026-10-23: Added a Recent Projects list and a 2-minute autosave; opening a project offers a newer sibling autosave for recovery
- 2026-10-23: The project display name now follows the file name (without `.zoft`) on Save/Save As/Open
- 2026-10-23: Added File → New from Template with Empty, Band, Electronic and Scoring track setups