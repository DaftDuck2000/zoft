# Phase 0: Foundation

**Duration**: Months 1-3 (Weeks 1-12)
**Goal**: Working audio engine skeleton, transport, minimal UI, project save/load

## Features

| ID | Feature | Status | File |
|----|---------|--------|------|
| 0.1 | Project Setup & Workspace | `[~]` | `project-setup.md` |
| 0.2 | Audio Engine Skeleton | `[~]` | `audio-engine-skeleton.md` |
| 0.3 | Basic Transport | `[✓]` | `basic-transport.md` |
| 0.4 | Minimal UI Shell | `[~]` | `minimal-ui.md` |
| 0.5 | Project Save/Load | `[~]` | `project-save-load.md` |

## Dependencies

- 0.1 blocks all others
- 0.2 → 0.3 → 0.4 (sequential)
- 0.5 can start after 0.1

## Milestone: M0 — Hello Audio (Week 5)

- [✓] Sine wave plays through audio output
- [✓] Transport play/stop works
- [✓] UI window opens with track list + transport bar
- [ ] Project saves and loads through the app UI (serializer API exists; Save/Open not wired)

## Progress Summary

All Phase 0 features have working implementations, but the status was corrected
to reflect incomplete tooling and persistence work as of 2026-10-21:

- Working Cargo workspace with 6 crates, CPAL audio engine, transport, egui UI,
  timeline ruler and the `.zoft` serializer/loader
- 0.1 still lacks the pinned toolchain, GitHub Actions workflow, pre-commit
  hooks, CHANGELOG and wired config/profiling
- 0.4 still lacks add/remove/reorder track UI, markers, a theme toggle,
  vertical track-list scrolling and window-state persistence
- 0.5 has the binary format and serializer API, but the app does not expose
  Save/Open, recent projects, auto-save or templates yet
- Post-phase additions now in daily use: ruler seek + loop lane, zoom/scroll,
  metronome, tempo/tap, track M/S/R/Mon/volume, punch recording, recording level
  meter, History panel and the Clip Audio window