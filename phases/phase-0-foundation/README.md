# Phase 0: Foundation

**Duration**: Months 1-3 (Weeks 1-12)
**Goal**: Working audio engine skeleton, transport, minimal UI, project save/load

## Features

| ID | Feature | Status | File |
|----|---------|--------|------|
| 0.1 | Project Setup & Workspace | `[✓]` | `project-setup.md` |
| 0.2 | Audio Engine Skeleton | `[✓]` | `audio-engine-skeleton.md` |
| 0.3 | Basic Transport | `[✓]` | `basic-transport.md` |
| 0.4 | Minimal UI Shell | `[✓]` | `minimal-ui.md` |
| 0.5 | Project Save/Load | `[✓]` | `project-save-load.md` |

## Dependencies

- 0.1 blocks all others
- 0.2 → 0.3 → 0.4 (sequential)
- 0.5 can start after 0.1

## Milestone: M0 — Hello Audio (Week 5)

- [✓] Sine wave plays through audio output
- [✓] Transport play/stop works
- [✓] UI window opens with track list + transport bar
- [✓] Project saves and loads

## Progress Summary

All Phase 0 features are **complete** as of 2026-10-07. The DAW now has:
- Working Cargo workspace with 6 crates
- CPAL audio engine with lock-free UI↔Audio communication
- Transport (Play/Stop) with sample-accurate timing
- egui-based UI with track list, transport bar, timeline ruler
- Binary `.zoft` project format with LZ4 compression
- Application runs and opens a functional GUI window