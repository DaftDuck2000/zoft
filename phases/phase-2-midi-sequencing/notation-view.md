# Feature: Notation View (Stretch Goal)

## Phase: 2 — MIDI & Sequencing
## ID: 2.5
## Priority: Low (Stretch)
## Estimated: 3 weeks
## Status: `[ ]` Not Started
## Depends On: 2.2

## Description
Basic music notation display: staff, clefs, notes, rests, accidentals, key/time signatures. Read-only initially, editing stretch goal. Export to PDF/MusicXML.

## Requirements

- [ ] Staff rendering: Treble, Bass, Alto, Tenor clefs
- [ ] Notes: Whole, half, quarter, eighth, sixteenth, thirty-second
- [ ] Rests: Corresponding durations
- [ ] Accidentals: Sharp, flat, natural, double sharp/flat
- [ ] Key signatures: Major/minor, circle of fifths
- [ ] Time signatures: Simple, compound, complex
- [ ] Beaming: Automatic grouping by beat
- [ ] Ties: Across bar lines
- [ ] Multiple voices per staff (max 2)
- [ ] Lyrics/text annotations (optional)
- [ ] Print layout: Page breaks, margins, systems
- [ ] Export: PDF (via print), MusicXML
- [ ] Sync with piano roll: Selection highlights in both

## Technical Details

### Notation Model
```rust
struct NotationView {
    clips: Vec<ClipId>,           // Clips to display
    clef: Clef,
    key_signature: KeySignature,
    time_signature: TimeSignature,
    layout: LayoutOptions,
}

enum Clef { Treble, Bass, Alto, Tenor }

struct KeySignature {
    root: PitchClass,
    mode: Mode,  // Major, Minor, Dorian, etc.
}

struct LayoutOptions {
    page_size: PageSize,          // A4, Letter, Custom
    margins: Margins,
    staff_size: f32,              // Staff height in mm
    systems_per_page: u8,
    measures_per_system: u8,
    justify_last_system: bool,
}
```

### Rendering Pipeline (using `vexflow` via WASM or native `rust-vexflow`)

```
MIDI Events → Quantize to Notation Grid → Voice Assignment → 
Measure Layout → Staff Layout → Page Layout → Render (SVG/PDF)
```

### Voice Assignment Algorithm
- Separate notes into voices by pitch overlap
- Max 2 voices per staff (stem up / stem down)
- Handle chords: single voice, multiple noteheads

### Beaming Rules
- Group by beat according to time signature
- Break beams at rests, syncopation, manual breaks
- Support feathered beams (accel/ritard)

### Integration with Piano Roll
```rust
struct NotationState {
    notation_view: NotationView,
    sync_selection: bool,  // Piano roll selection → notation highlight
}
```

### Export
- **PDF**: Render to SVG → `print` crate → PDF
- **MusicXML**: Generate MusicXML 4.0 from notation model

## Acceptance Criteria

- [ ] Simple melody renders correctly (treble clef, 4/4)
- [ ] Chords display as single stem with multiple noteheads
- [ ] Key signature shows correct accidentals
- [ ] Beaming follows standard rules
- [ ] Ties across bar lines work
- [ ] Two voices (stem up/down) render
- [ ] PDF export produces readable score
- [ ] MusicXML imports into MuseScore/Sibelius
- [ ] Selection sync with piano roll

## Progress Log

- YYYY-MM-DD: Staff rendering basics
- YYYY-MM-DD: Note/rest glyphs
- YYYY-MM-DD: Accidentals + key signatures
- YYYY-MM-DD: Time signatures + beaming
- YYYY-MM-DD: Voice assignment
- YYYY-MM-DD: Ties + layout
- YYYY-MM-DD: PDF export
- YYYY-MM-DD: MusicXML export
- YYYY-MM-DD: Piano roll sync

## Notes

- This is a stretch goal — defer if Phases 0-4 take longer
- Consider using `vexflow` via `wasm-bindgen` for rendering (battle-tested)
- Native Rust alternative: `musicxml` crate + custom layout engine
- Editing notation directly is Phase 6+ territory