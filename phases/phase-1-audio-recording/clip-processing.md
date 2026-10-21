# Feature: Clip Processing

## Phase: 1 — Audio Recording & Editing
## ID: 1.4
## Priority: High
## Estimated: 2 weeks
## Status: `[~]` In Progress
## Depends On: 1.2

## Description
Offline clip processing: gain, pitch shift, time stretch, reverse, normalize. Non-destructive — creates new audio files, original preserved.

## Requirements

- [ ] Clip gain: Apply gain to create new rendered file
- [ ] Pitch shift: Semitone + fine tune, formant preservation option
- [ ] Time stretch: Multiple algorithms (tape, elastic, musical)
- [x] Reverse: Render the clip backwards to a new file (original preserved)
- [x] Normalize: Peak to −1 dBFS, per clip
- [ ] Fade in/out render: Bake fades into new file
- [ ] Consolidate: Merge adjacent clips on same track into one file
- [ ] Bounce in place: Render clip with inserts/sends to new file
- [x] Process menu: Right-click clip → Process → Normalize / Reverse (more operations to follow)

## Technical Details

### Implemented

- **Normalize**: computes the exact peak of the clip's visible source range by
  seeking in the WAV and streaming only that range, then sets clip gain so the
  peak reaches −1 dBFS. This is a `Shape` undo entry.
- **Reverse**: decodes the clip's visible source range, reverses frames while
  preserving channel order, writes a new 32-bit float WAV next to the source
  (`<name>_reverse_<uuid>.wav`), and points the clip at it with
  `source_offset = 0`. Previously generated suffixes are stripped from the base
  name, so reversing repeatedly does not grow the filename. Undo restores the
  previous `AudioFileRef`, offset and length via a `Source` undo entry. A fresh
  waveform overview is generated in the background; results for a replaced path
  are discarded.
- **Process menu**: right-click a clip in the arrange view. The same actions
  are available in the clip inspector.

### Processing Pipeline
```rust
enum ClipProcess {
    Gain { db: f32 },
    PitchShift { semitones: f32, cents: f32, preserve_formants: bool },
    TimeStretch { ratio: f32, mode: StretchMode },
    Reverse,
    Normalize { target: NormalizeTarget, mode: NormalizeMode },
    RenderFades,
    Consolidate,
    BounceInPlace { include_inserts: bool, include_sends: bool },
}

enum StretchMode {
    Tape,           // Resample (pitch changes with speed)
    Elastic,        // Phase vocoder (monophonic)
    Musical,        // Transient-preserving (polyphonic)
}

enum NormalizeTarget {
    Peak { db: f32 },
    LUFS { lufs: f32, true_peak: f32 },
}

enum NormalizeMode {
    PerClip,
    Relative { reference_clip: ClipId },
}
```

### Implementation: Rubber Band Library
- Use `rubberband` CLI via `std::process::Command` for Phase 1
- Later: Port to Rust or use `rubberband-sys` FFI
- Input: Source file + clip parameters (offset, length, gain, fades)
- Output: New WAV file in project audio folder
- Update clip: `source = new_file`, `source_offset = 0`, `gain = 1.0`

### Consolidation
- Adjacent clips on same track, same source file → single clip
- Adjacent clips, different sources → render to new file
- Preserves crossfades, fades, gain

### Bounce in Place
- Route clip through track's insert chain + sends
- Render in real-time (faster) or offline (accurate for automation)
- Creates new audio track with rendered clip, disables original

## Acceptance Criteria

- [ ] Pitch shift ±12 semitones sounds musical
- [ ] Time stretch 0.5x–2.0x without artifacts (elastic mode)
- [x] Reverse plays correctly
- [x] Normalize to -1 dB peak works
- [ ] Consolidate merges clips correctly
- [ ] Bounce in place includes plugin processing
- [x] All operations create undoable commands
- [x] Original files untouched

## Progress Log

- 2026-10-21: Peak normalize to −1 dBFS (exact per-channel peak over the clip range, gain edit, undoable)
- 2026-10-21: Reverse rendered to a new WAV per clip (channel-safe frame reversal, undo via source state); repeated reverses reuse a stable base filename
- 2026-10-21: Snap grid defaults to Beat
- 2026-10-21: Right-click Process menu in the arrange view
- Remaining: pitch shift, time stretch, fade rendering, consolidate, bounce in place, LUFS normalize