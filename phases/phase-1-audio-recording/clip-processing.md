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
- [x] Pitch shift: Semitone knob with optional formant preservation (Rubber Band R3)
- [x] Time stretch: 0.5×–2.0× via Rubber Band R3
- [x] Time-stretch modes: tape / elastic / musical presets
- [x] Reverse: Render the clip backwards to a new file (original preserved)
- [x] Normalize: Peak to −1 dBFS, per clip
- [x] LUFS normalize: Integrated loudness target with a sample-peak safety cap
- [x] Fade in/out render: Bake fades into new file (Bake into Clip effect)
- [x] Bit Crusher: real-time FX Rack insert (removed from Clip Audio)
- [x] Saturation: real-time FX Rack insert (removed from Clip Audio)
- [x] Filter: real-time FX Rack insert (removed from Clip Audio)
- [x] Tremolo: real-time FX Rack insert (removed from Clip Audio)
- [x] Consolidate: Merge adjacent clips on same track into one file
- [ ] Bounce in place: Render clip with inserts/sends to new file
- [x] Clip Audio window: right-click a clip or use the arrange **Audio…** button; the **Clip** section has gain/fades/colour and the effect list has an Apply button

## Technical Details

### Implemented

- **Normalize**: computes the exact peak of the clip's visible source range by
  seeking in the WAV and streaming only that range, then sets clip gain so the
  peak reaches −1 dBFS. This is a `Shape` undo entry.
- **LUFS Normalize**: measures integrated loudness over the clip's visible
  source range using the ITU-R BS.1770 K-weighting filter, 400 ms blocks with
  100 ms hops, and the absolute/relative gates. It sets clip gain to the chosen
  target (−30…−6 LUFS, default −14 LUFS), capped so the sample peak stays at or
  below −1 dBFS rather than adding a limiter. Also a `Shape` undo entry.
- **Reverse**: decodes the clip's visible source range, reverses frames while
  preserving channel order, writes a new 32-bit float WAV next to the source
  (`<name>_reverse_<uuid>.wav`), and points the clip at it with
  `source_offset = 0`. Previously generated suffixes are stripped from the base
  name, so reversing repeatedly does not grow the filename. Undo restores the
  previous `AudioFileRef`, offset and length via a `Source` undo entry. A fresh
  waveform overview is generated in the background; results for a replaced path
  are discarded.
- **Pitch shift / time stretch**: the floating Clip Audio window exposes
  separate Pitch Shift and Time Stretch effects. Pitch is −12…+12 semitones
  (0.01 st = 1 cent) with an optional formant-preservation toggle; stretch is
  0.5×–2.0×. Time Stretch has three modes: **Tape** resamples for classic
  varispeed (length and pitch change together), **Elastic** uses Rubber Band R3
  fine mode for smooth sustained material, and **Musical** adds centre focus
  for mixed material. Applying writes the visible source range to a temporary
  32-bit float WAV (except Tape, which is resampled directly), runs
  `rubberband --quiet --fine [--centre-focus] --pitch <st> --time <ratio>
  [--formant]` for the Rubber Band modes, and points the clip at a new
  `<name>_process_<uuid>.wav`. The clip length follows the rendered duration,
  fades are clamped to it, and undo restores the previous source, offset, length
  and fades via a `Source` entry. Pitch/stretch controls reset after a successful
  apply. Rubber Band must be available in `PATH` for Pitch Shift, Elastic and
  Musical; Tape does not need it.
- **Clip Audio window**: right-click a clip in the arrange view (or use the
  arrange **Audio…** button) to open a floating window. The **Clip** entry at the
  top shows gain, fade in/out and colour, which apply immediately and are
  undoable. The effect list (Normalize, LUFS Normalize, Bake into Clip, Reverse,
  Pitch Shift, Time Stretch, Consolidate) shows the selected effect's parameters
  as Logic-style rotary knobs and a single **Apply** button. Drag a knob
  vertically to adjust it, hold Shift for fine control, or double-click to
  restore its default. The window stays open, so several operations can be
  applied in sequence. New per-clip controls live here instead of the arrange
  toolbar.
  With several clips selected, **Apply** runs the chosen effect on every
  selected clip and records a single undo step (Consolidate requires a single
  clip).
- **Bake into Clip**: renders the visible source range with the clip gain and
  fade-in/fade-out envelopes applied, matching the playback mixer's envelope.
  The output is a new `<name>_process_<uuid>.wav`; the clip then points at it
  with gain reset to 0 dB and fades removed. Undo restores the previous source,
  offset, length, fades and gain via a `Source` entry.
- **Bit Crusher, Saturation, Filter and Tremolo** are real-time track inserts
  in the FX Rack (X). They are no longer offline Clip Audio tools.
- **Consolidate**: merges the selected clip with the contiguous run of clips
  that touch or overlap it on the same track. The run is rendered through the
  playback mixer at the project sample rate, so clip gain, fades and automatic
  equal-power crossfades are baked into one new `*_process_<uuid>.wav`. The new
  clip spans the run, keeps the selected clip's colour, and has unity gain with
  no fades. The original clips remain in the project (undo restores them; the
  consolidated result is detached on undo). Headroom above full scale is
  preserved in the float file. The Clip Audio window closes after a
  successful consolidation because the selected clip was replaced.
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
- [x] LUFS normalize reaches the target without clipping sample peaks
- [x] Consolidate merges clips correctly
- [ ] Bounce in place includes plugin processing
- [x] All operations create undoable commands
- [x] Original files untouched

## Progress Log

- 2026-10-21: Peak normalize to −1 dBFS (exact per-channel peak over the clip range, gain edit, undoable)
- 2026-10-21: Reverse rendered to a new WAV per clip (channel-safe frame reversal, undo via source state); repeated reverses reuse a stable base filename
- 2026-10-21: Pitch shift (±12 st) and time stretch (0.5×–2.0×) via Rubber Band R3, with optional formant preservation and undoable source replacement
- 2026-10-21: Snap grid defaults to Beat
- 2026-10-21: Right-click opens a floating window with an effect list and one Apply button (replaces the inline processing controls and context menu)
- 2026-10-21: Renamed the window to **Clip Audio** and moved clip gain, fades and colour into its **Clip** section; the arrange toolbar now just has an **Audio…** button
- 2026-10-21: Bake into Clip (formerly Gain & Fades) bakes clip gain and fade envelopes into a new WAV, resets the clip's gain/fades, and is undoable
- 2026-10-21: Added Bit Crusher, Saturation, Filter and Tremolo effect processors; renamed the gain/fade bake action to Bake into Clip
- 2026-10-21: Replaced the effect sliders with Logic-style rotary knobs (vertical drag, Shift fine adjustment, double-click reset)
- 2026-10-28: Restored right-clicking an audio clip to reopen the Clip Audio tools window. Track effects stay in the FX Rack and are not part of the clip menu
- 2026-10-28: Removed Bit Crusher, Saturation, Filter and Tremolo from the Clip Audio tool list; they are only available as real-time FX Rack inserts
- 2026-10-21: Added LUFS Normalize with ITU-R BS.1770 integrated loudness measurement, −30…−6 LUFS target, and a −1 dBFS sample-peak safety cap
- 2026-10-21: Added Time Stretch modes: Tape varispeed, Elastic (R3 fine), Musical (R3 fine with centre focus)
- 2026-10-21: Added Consolidate: renders a touching/overlapping run of clips on a track through the playback mixer, preserving gain, fades and automatic crossfades; undo restores the originals
- Remaining: listening tests for pitch/time and creative effects, bounce in place (depends on insert/send processing from a later phase)
