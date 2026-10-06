# Feature: Clip Processing

## Phase: 1 — Audio Recording & Editing
## ID: 1.4
## Priority: High
## Estimated: 2 weeks
## Status: `[ ]` Not Started
## Depends On: 1.2

## Description
Offline clip processing: gain, pitch shift, time stretch, reverse, normalize. Non-destructive — creates new audio files, original preserved.

## Requirements

- [ ] Clip gain: Apply gain to create new rendered file
- [ ] Pitch shift: Semitone + fine tune, formant preservation option
- [ ] Time stretch: Multiple algorithms (tape, elastic, musical)
- [ ] Reverse: Play clip backwards
- [ ] Normalize: Peak or LUFS, per clip or selection
- [ ] Fade in/out render: Bake fades into new file
- [ ] Consolidate: Merge adjacent clips on same track into one file
- [ ] Bounce in place: Render clip with inserts/sends to new file
- [ ] Process menu: Right-click clip → Process → [operations]

## Technical Details

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
- [ ] Reverse plays correctly
- [ ] Normalize to -1 dB peak works
- [ ] Consolidate merges clips correctly
- [ ] Bounce in place includes plugin processing
- [ ] All operations create undoable commands
- [ ] Original files untouched

## Progress Log

- YYYY-MM-DD: Gain processing
- YYYY-MM-DD: Pitch shift (Rubber Band)
- YYYY-MM-DD: Time stretch modes
- YYYY-MM-DD: Reverse
- YYYY-MM-DD: Normalize
- YYYY-MM-DD: Consolidate
- YYYY-MM-DD: Bounce in place
- YYYY-MM-DD: Process menu UI