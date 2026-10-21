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
| 1.5 | Undo/Redo System | `[~]` | `undo-redo.md` |

## Dependencies

- 1.1 → 1.2 → 1.3 (sequential)
- 1.4 can start after 1.2
- 1.5 should start early (integrates with all editing)

## Milestone: M1 — Record & Edit Audio (Week 20)

- [✓] Arm track → record → see waveform → edit → play back
- [✓] Non-destructive: original file untouched
- [ ] Undo/redo works for all edit operations (clip edits and track mute/solo/record-arm/volume have history, a History panel and labelled undo steps; automation edits are not wired up yet)

## Progress Summary

**1.1 Audio Track & Recording** - `[✓]` Done
- Audio track model has record_arm, monitor_mode, input/output
- CPAL backend supports input device enumeration
- BWF writer with `bext` chunk implemented
- Input stream support in audio backend
- Recording UI integrated - record arm button, record button, R key
- Input device selection dropdown in transport bar
- Background writer task with tokio for BWF writing
- Recording level meter in the transport bar (pre-fader peak + RMS from the input stream)
- Input monitoring: track **Mon** cycles Off/Auto/On and mixes the input into the output while the input stream is open (during a take); the mode is undoable

**1.2 Waveform Rendering** - `[~]` In Progress
- Overview (min/max + RMS) pre-computed per audio file, stored and serialized with the project
- Zoomable timeline; overview aggregation correct at any zoom (wheel scroll, ctrl+wheel zoom, Fit)
- Sample-accurate detail view at high zoom (windowed decode, LRU cache)
- Track colors, clip gain visualization, fade curves, selection highlight, silent-region dimming
- Overview generated on background threads (no UI hitch after recording)
- Remaining: GPU texture atlas layout, perf benchmarks (overview <1s/min, 100 tracks @60 FPS), runtime verification

**Playback (sound output)** - Pulled forward from M1 milestone
- `daw-engine/src/playback.rs`: real-time mixer (clip gain, track mute/solo/volume, fades, sample-rate conversion) feeding the CPAL output callback
- Engine API: `set_playback_clips`, `clear_playback`, `playback_position_frames`
- Track list has per-track volume sliders and M/S buttons; mute, solo and volume are applied by the mixer and recorded in undo history
- UI loads the project's clips on play; the playhead follows the engine's sample clock
- Playhead and timeline may extend past the last clip; the view auto-follows during playback
- Click the lower ruler (or drag it) to seek; the playhead follows the current snap grid (hold Alt to bypass), and seeking during playback/recording pauses the transport
- Timeline ruler defaults to musical bars/beats (labels like `3.2`), switchable to clock time with the Ruler dropdown; the time-signature picker in the transport bar (4/4, 3/4, 6/8, …) drives bar length, ruler labels and the snap grid
- Recording arms on ● and only captures from the moment the transport (Space/▶) rolls, so a take's length matches the playhead travel; pausing ends the take and shows the waveform right away
- Input monitoring: the track **Mon** button cycles Off → Auto (monitor when armed or recording) → On; the input stream opens when monitoring is needed, so armed/On tracks are audible before and during a take
- Punch recording: **Punch** enables a capture range set with **P-In**/**P-Out** at the playhead; an armed take waits for punch-in and ends at punch-out while the transport keeps rolling, and the ruler marks the region in red
- Transport loop: drag in the ruler's upper loop lane to set the range (or use **In**/**Out** at the playhead), then **Loop** repeats it; the engine wraps sample-accurately at the loop end and the ruler highlights the range
- Metronome **Click** toggle with accented downbeats, following the project tempo and time signature
- Tempo field (drag or type BPM) and **Tap** tempo; both are undoable and update the metronome live
- Not yet: transport scrub

**1.3 Non-Destructive Clip Editing** - `[~]` In Progress
- Drag clips to move them (horizontal + between tracks) with snapping: Off / Bar / Beat (default) / 1/8 / 1/16 (hold Alt to bypass)
- Edge drags trim non-destructively (adjusts `source_offset`/`length`; the WAV file is never touched)
- Split at the playhead (S) creates a second clip referencing the same source
- Copy/paste/cut/duplicate (Ctrl+C / Ctrl+V / Ctrl+X / Ctrl+D); Delete removes the clip from the track (data stays in the pool, undo restores it)
- Fade handles in the clip's top corners; the **Clip Audio** window has fade curve (linear/exp/log/S/equal-power), fade length, gain and colour controls
- Automatic equal-power crossfade where clips overlap on the same track, with an X overlay in the overlap region; contained clips are layered instead
- Snap: Off / Bar / Beat (default) / 1/8 / 1/16, optional Relative mode, Alt bypass; the grid follows the chosen time signature
- Trash browser in the track panel: deleted clips stay in the project and can be restored
- Undo/redo (Ctrl+Z, Ctrl+Shift+Z, Ctrl+Y) with merge for continuous slider edits and a floating **History** panel
- Playback mixer now slices clips by `source_offset`/`length`
- Next: bounce in place (remaining 1.4 processing operation)

**1.4 Clip Processing** - `[~]` In Progress
- Right-click a clip (or use the arrange **Audio…** button) to open the floating **Clip Audio** window. Its **Clip** section has gain, fade in/out and colour controls that apply immediately and are undoable; the effect list (Normalize, LUFS Normalize, Bake into Clip, Reverse, Pitch Shift, Time Stretch, Bit Crusher, Saturation, Filter, Tremolo, Consolidate) shows Logic-style knobs (drag vertically, hold Shift for fine control, double-click to reset) with a single **Apply** button
- Peak Normalize (to −1 dBFS) is a gain edit and fully undoable
- LUFS Normalize measures BS.1770 integrated loudness and sets gain to a −30…−6 LUFS target (default −14), capped so sample peaks stay at or below −1 dBFS
- Consolidate merges the selected clip with touching/overlapping clips on its track into one file, baking in gain, fades and automatic crossfades; undo restores the original clips
- Bake into Clip bakes the clip's gain and fade envelopes into a new `*_process_<uuid>.wav`, then resets the gain/fades; undo restores them
- Bit Crusher, Saturation, Filter and Tremolo render creative effects to a new file while preserving clip gain, fades, position and track; all are undoable
- Reverse renders the clip's visible audio backwards to a new `*_reverse_<uuid>.wav` (original file untouched) and repoints the clip; undoable
- Pitch shift (±12 semitones) and time stretch (0.5×–2.0×) rendered through Rubber Band R3, with optional formant preservation; Time Stretch offers Tape, Elastic and Musical modes. Clip length follows the rendered audio and undo restores the original source
- The window stays open, so clip properties and effects can be adjusted in sequence; new clip controls live there instead of the arrange toolbar
- Rendered files regenerate their waveform overview in the background; stale overviews from a replaced file are ignored
- Note: pitch/time rendering requires the `rubberband` binary in `PATH` (except Tape mode)
- Not yet: bounce in place

**1.5 Undo/Redo** - `[~]` In Progress
- `daw-core/src/undo.rs` provides a generic `History<T>` (capacity 1000, drops oldest, clears the redo branch on new edits) and a corrected command-backed `UndoHistory`/`UndoManager` for `dyn Command`
- Arrange-view edits use `History<EditAction>`: moves, trims, fades, gain, colour, source replacements, attach/detach, track mute/solo/record-arm/volume, time signature and labelled groups (Split, Consolidate)
- Every entry has a readable description; the **History** button in the arrange toolbar opens a floating panel that lists past edits, shows redoable edits and jumps to any point
- Continuous slider/drag edits merge into one undo step; undo/redo reload the mixer when playback is running
- Not yet: serialized snapshots for very large jumps, automation edit integration, optional per-track scope