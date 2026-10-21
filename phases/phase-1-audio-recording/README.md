# Phase 1: Audio Recording & Editing

**Duration**: Months 4-8 (Weeks 13-32)
**Goal**: Record, edit, and manipulate audio clips non-destructively

## Features

| ID | Feature | Status | File |
|----|---------|--------|------|
| 1.1 | Audio Track & Recording | `[✓]` | `audio-track-record.md` |
| 1.2 | Waveform Rendering | `[~]` | `waveform-rendering.md` |
| 1.3 | Non-Destructive Clip Editing | `[~]` | `non-destructive-editing.md` |
| 1.4 | Clip Processing | `[~]` | `clip-processing.md` |
| 1.5 | Undo/Redo System | `[ ]` | `undo-redo.md` |

## Dependencies

- 1.1 → 1.2 → 1.3 (sequential)
- 1.4 can start after 1.2
- 1.5 should start early (integrates with all editing)

## Milestone: M1 — Record & Edit Audio (Week 20)

- [✓] Arm track → record → see waveform → edit → play back
- [✓] Non-destructive: original file untouched
- [ ] Undo/redo works for all edit operations (clip edits are undoable; track/automation edits are not wired up yet)

## Progress Summary

**1.1 Audio Track & Recording** - `[✓]` Done
- Audio track model has record_arm, monitor_mode, input/output
- CPAL backend supports input device enumeration
- BWF writer with `bext` chunk implemented
- Input stream support in audio backend
- Recording UI integrated - record arm button, record button, R key
- Input device selection dropdown in transport bar
- Background writer task with tokio for BWF writing

**1.2 Waveform Rendering** - `[~]` In Progress
- Overview (min/max + RMS) pre-computed per audio file, stored and serialized with the project
- Zoomable timeline; overview aggregation correct at any zoom (wheel scroll, ctrl+wheel zoom, Fit)
- Sample-accurate detail view at high zoom (windowed decode, LRU cache)
- Track colors, clip gain visualization, fade curves, selection highlight, silent-region dimming
- Overview generated on background threads (no UI hitch after recording)
- Remaining: GPU texture atlas layout, perf benchmarks (overview <1s/min, 100 tracks @60 FPS), runtime verification

**Playback (sound output)** - Pulled forward from M1 milestone
- `daw-engine/src/playback.rs`: real-time mixer (clip gain, fades, sample-rate conversion) feeding the CPAL output callback
- Engine API: `set_playback_clips`, `clear_playback`, `playback_position_frames`
- UI loads the project's clips on play; the playhead follows the engine's sample clock
- Playhead and timeline may extend past the last clip; the view auto-follows during playback
- Click or drag anywhere in the ruler to seek; the playhead follows the current snap grid (hold Alt to bypass), and seeking during playback/recording pauses the transport
- Timeline ruler defaults to musical bars/beats (labels like `3.2`), switchable to clock time with the Ruler dropdown; the time-signature picker in the transport bar (4/4, 3/4, 6/8, …) drives bar length, ruler labels and the snap grid
- Recording arms on ● and only captures from the moment the transport (Space/▶) rolls, so a take's length matches the playhead travel; pausing ends the take and shows the waveform right away
- Not yet: input monitoring while recording, mute/solo/per-track volume in the mix, loop playback, transport scrub

**1.3 Non-Destructive Clip Editing** - `[~]` In Progress
- Drag clips to move them (horizontal + between tracks) with snapping: Off / Bar / Beat (default) / 1/8 / 1/16 (hold Alt to bypass)
- Edge drags trim non-destructively (adjusts `source_offset`/`length`; the WAV file is never touched)
- Split at the playhead (S) creates a second clip referencing the same source
- Copy/paste/duplicate (Ctrl+C / Ctrl+V / Ctrl+D); Delete removes the clip from the track (data stays in the pool, undo restores it)
- Fade handles in the clip's top corners; clip inspector with fade curve (linear/exp/log/S/equal-power), fade length, gain and color
- Automatic equal-power crossfade where clips overlap on the same track, with an X overlay in the overlap region; contained clips are layered instead
- Snap: Off / Bar / Beat (default) / 1/8 / 1/16, optional Relative mode, Alt bypass; the grid follows the chosen time signature
- Trash browser in the track panel: deleted clips stay in the project and can be restored
- Undo/redo (Ctrl+Z, Ctrl+Shift+Z, Ctrl+Y) with merge for continuous slider edits — Phase 1.5 groundwork
- Playback mixer now slices clips by `source_offset`/`length`
- Next: the remaining 1.4 processing operations (pitch shift, time stretch, consolidate, bounce)

**1.4 Clip Processing** - `[~]` In Progress
- Peak Normalize (to −1 dBFS) in the clip inspector and right-click Process menu; implemented as a gain edit and fully undoable
- Reverse renders the clip's visible audio backwards to a new `*_reverse_<uuid>.wav` (original file untouched) and repoints the clip; undoable
- Right-click a clip for the Process menu (Normalize / Reverse)
- Reverse regenerates the waveform overview in the background; stale overviews from a replaced file are ignored
- Not yet: pitch shift, time stretch, fade rendering, consolidate, bounce in place, LUFS normalize

**1.5 Undo/Redo** - Not started (basic undo/redo already available in the arrange view)