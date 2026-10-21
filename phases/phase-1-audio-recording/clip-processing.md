# Feature: Clip Processing

## Phase: 1 — Audio Recording & Editing
## ID: 1.4
## Priority: High
## Estimated: 2 weeks
## Status: `[~]` In Progress
## Depends On: 1.2

## Description
Offline clip processing: gain, pitch shift, time stretch, reverse, normalize, bit crush, saturation, filtering and tremolo. Non-destructive — creates new audio files, original preserved.

## Requirements

- [x] Clip gain: Apply gain to create new rendered file (combined with fades in the Bake into Clip effect)
- [x] Pitch shift: Semitone slider with optional formant preservation (Rubber Band R3)
- [x] Time stretch: 0.5×–2.0× via Rubber Band R3
- [ ] Time-stretch modes: tape / elastic / musical presets
- [x] Reverse: Render the clip backwards to a new file (original preserved)
- [x] Normalize: Peak to −1 dBFS, per clip
- [x] Fade in/out render: Bake fades into new file (Bake into Clip effect)
- [x] Bit Crusher: bit-depth reduction and sample-and-hold decimation
- [x] Saturation: soft-clip drive with dry/wet mix
- [x] Filter: low-pass or high-pass cutoff
- [x] Tremolo: rate and depth modulation
- [ ] Consolidate: Merge adjacent clips on same track into one file
- [ ] Bounce in place: Render clip with inserts/sends to new file
- [x] Audio Effects window: right-click a clip, choose an effect from the list, adjust parameters, then press Apply

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
- **Pitch shift / time stretch**: the floating Audio Effects window exposes
  separate Pitch Shift and Time Stretch effects. Pitch is −12…+12 semitones
  (0.01 st = 1 cent) with an optional formant-preservation toggle; stretch is
  0.5×–2.0×. Applying writes the visible source range to a temporary 32-bit
  float WAV, runs `rubberband --fine --pitch <st> --time <ratio> [--formant]`,
  and points the clip at a new `<name>_process_<uuid>.wav`. The clip length
  follows the rendered duration, fades are clamped to it, and undo restores the
  previous source, offset, length and fades via a `Source` entry. Pitch/stretch
  controls reset after a successful apply. Rubber Band must be available in
  `PATH`.
- **Audio Effects window**: right-click a clip in the arrange view to open a
  floating window with the effect list (Normalize, Bake into Clip, Reverse,
  Pitch Shift, Time Stretch, Bit Crusher, Saturation, Filter, Tremolo). The
  selected effect shows its parameters as Logic-style rotary knobs and a single
  **Apply** button. Drag a knob vertically to adjust it, hold Shift for fine
  control, or double-click to restore its default. The window stays open, so
  several effects can be applied in sequence.
- **Bake into Clip**: renders the visible source range with the clip gain and
  fade-in/fade-out envelopes applied, matching the playback mixer's envelope.
  The output is a new `<name>_process_<uuid>.wav`; the clip then points at it
  with gain reset to 0 dB and fades removed. Undo restores the previous source,
  offset, length, fades and gain via a `Source` entry.
- **Bit Crusher**: quantizes samples to 1–16 bits and holds them for 1–64
  frames, producing digital aliasing and crunch.
- **Saturation**: tanh soft clipping with 0–36 dB drive and a dry/wet mix.
- **Filter**: first-order low-pass or high-pass with a logarithmic 20 Hz–20 kHz
  cutoff. The cutoff is clamped below Nyquist on render.
- **Tremolo**: sine amplitude modulation with 0.1–20 Hz rate and 0–100 % depth.
- The creative effects preserve the clip's gain, fades, timeline position and
  track; undo/redo uses the same `Source` state as the other file-rendering
  operations.

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
- Use `rubberband` CLI via `std::process::Command` for Phase 1 (implemented in
  fine/R3 mode with optional formant preservation)
- Later: Port to Rust or use `rubberband-sys` FFI
- Input: Source file + clip parameters (offset, length, gain, fades)
- Output: New WAV file next to the source; clip points at it
- Update clip: `source = new_file`, `source_offset = 0`, `length = rendered frames`; fades are kept (and clamped if the clip becomes shorter)

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
- 2026-10-21: Pitch shift (±12 st) and time stretch (0.5×–2.0×) via Rubber Band R3, with optional formant preservation and undoable source replacement
- 2026-10-21: Snap grid defaults to Beat
- 2026-10-21: Right-click opens a floating Audio Effects window with an effect list and one Apply button (replaces the inline processing controls and context menu)
- 2026-10-21: Bake into Clip (formerly Gain & Fades) bakes clip gain and fade envelopes into a new WAV, resets the clip's gain/fades, and is undoable
- 2026-10-21: Added Bit Crusher, Saturation, Filter and Tremolo effect processors; renamed the gain/fade bake action to Bake into Clip
- 2026-10-21: Replaced the Audio Effects sliders with Logic-style rotary knobs (vertical drag, Shift fine adjustment, double-click reset)
- Remaining: listening tests for pitch/time and creative effects, stretch mode presets, consolidate, bounce in place, LUFS normalize
