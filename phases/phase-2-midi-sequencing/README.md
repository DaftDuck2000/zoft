# Phase 2: MIDI & Sequencing

**Duration**: Months 9-14 (Weeks 33-56)
**Goal**: Full MIDI recording, piano roll editing, MIDI effects

## Features

| ID | Feature | Status | File |
|----|---------|--------|------|
| 2.1 | MIDI Track Record/Playback | `[~]` | `midi-track-record-playback.md` |
| 2.2 | Piano Roll Editor | `[~]` | `piano-roll-editor.md` |
| 2.3 | MIDI Editing Tools | `[~]` | `midi-editing-tools.md` |
| 2.4 | MIDI Effect Plugins | `[ ]` | `midi-effects.md` |
| 2.5 | Notation View (Stretch) | `[ ]` | `notation-view.md` |

## Dependencies

- 2.1 → 2.2 → 2.3 (sequential)
- 2.4 can start after 2.1
- 2.5 is stretch goal

## Status Notes

MIDI has started: tracks can be added as MIDI tracks, and MIDI input devices
are enumerated through `midir` with a dedicated MIDI bar (auto-connect,
selector, Refresh and live note/velocity activity). Armed MIDI tracks record
timestamped notes into clips that render in the arrange lane and play through
a basic built-in sine voice; live input is auditioned through the same voice
while recording, and MIDI clips show miniature note previews in the arrange
view. Double-clicking a MIDI clip opens an editable piano-roll view with
add/move/resize/delete, multi-selection, note audition, undo, wheel/middle-drag
panning with zoom, and its own snap picker. MIDI clips can be cropped from
either edge in the arrange view, dragged in time, dropped onto another
MIDI/instrument track, split at the playhead, duplicated, and multi-selected
with the same click/Ctrl/Shift/marquee model as audio for group moves, deletes,
duplicates, copy/paste and mute; audio and MIDI selections can be combined and
moved as one group. The MIDI Tools panel in the piano roll adds quantize with
swing, transpose, scale conform, velocity shaping (scale, compress, expand,
curve and randomize), humanize, legato, fixed note length, note join, split at
the playhead, reverse, pitch mirror and randomize (pitch, velocity, timing and
density), with a Tools… menu to choose which tool sections are visible. The
piano roll has a bar/beat ruler and a draggable playhead line, plus toggleable
ghost notes from other tracks/clips in the project. Each MIDI/instrument track
has a G toggle in its header to include or exclude it from ghost notes.
Multiple toggleable CC lanes and a velocity lane handle per-note dynamics and
controller automation. Trimming is non-destructive and the piano roll shades trimmed
regions. Every track type now has a real-time insert chain (saturation, filter,
bit crusher, tremolo) that runs before the track fader, edited from the
Logic-style FX Rack (X). Instruments, chase events, loop takes, MIDI thru and the remaining editing
tools are not implemented yet.

## Milestone: M2 — MIDI Sequencing (Week 44)

- [~] Record MIDI → see in piano roll → edit → play back
- [ ] Quantize, transpose, velocity tools work
- [ ] MIDI effects (arpeggiator) process in real-time