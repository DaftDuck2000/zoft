# Phase 1: Audio Recording & Editing

**Duration**: Months 4-8 (Weeks 13-32)
**Goal**: Record, edit, and manipulate audio clips non-destructively

## Features

| ID | Feature | Status | File |
|----|---------|--------|------|
| 1.1 | Audio Track & Recording | `[ ]` | `audio-track-record.md` |
| 1.2 | Waveform Rendering | `[ ]` | `waveform-rendering.md` |
| 1.3 | Non-Destructive Clip Editing | `[ ]` | `non-destructive-editing.md` |
| 1.4 | Clip Processing | `[ ]` | `clip-processing.md` |
| 1.5 | Undo/Redo System | `[ ]` | `undo-redo.md` |

## Dependencies

- 1.1 → 1.2 → 1.3 (sequential)
- 1.4 can start after 1.2
- 1.5 should start early (integrates with all editing)

## Milestone: M1 — Record & Edit Audio (Week 20)

- [ ] Arm track → record → see waveform → edit → play back
- [ ] Non-destructive: original file untouched
- [ ] Undo/redo works for all edit operations