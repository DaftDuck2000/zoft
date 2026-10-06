# Zoft — Development Plan & Progress Tracking

**Project**: Zoft DAW
**Started**: 2026-10-06
**Target**: Solo, 2-3 years, Linux-first → macOS → Windows

---

## Progress Legend

- `[ ]` Not Started
- `[~]` In Progress
- `[?]` Blocked / Needs Decision
- `[✓]` Done
- `[R]` Review Needed

---

## Phase 0: Foundation (Months 1-3)

**Goal**: Working audio engine skeleton, transport, minimal UI, project save/load

### Phase 0 Features

| ID | Feature | Status | Est. | Actual | Notes |
|----|---------|--------|------|--------|-------|
| 0.1 | Project Setup & Workspace | `[ ]` | 1w | | Cargo workspace, CI, linting, pre-commit |
| 0.2 | Audio Engine Skeleton | `[ ]` | 2w | | cpal stream, ring buffer, sample-accurate scheduler |
| 0.3 | Basic Transport | `[ ]` | 1w | | Play/Stop/Seek, Tempo, Time Signature, Metronome |
| 0.4 | Minimal UI Shell | `[ ]` | 2w | | egui window, track list, transport bar, timeline ruler |
| 0.5 | Project Save/Load | `[ ]` | 1w | | Binary format (.zoft), versioning, migration |

---

## Phase 1: Audio Recording & Editing (Months 4-8)

**Goal**: Record, edit, and manipulate audio clips non-destructively

### Phase 1 Features

| ID | Feature | Status | Est. | Actual | Notes |
|----|---------|--------|------|--------|-------|
| 1.1 | Audio Track & Recording | `[ ]` | 2w | | Arm, monitor modes, BWF write, input selection |
| 1.2 | Waveform Rendering | `[ ]` | 2w | | Overview (min/max), detail view, GPU atlas later |
| 1.3 | Non-Destructive Clip Editing | `[ ]` | 3w | | Split, trim, move, copy, fade, crossfade, snap |
| 1.4 | Clip Processing | `[ ]` | 2w | | Gain, pitch shift, time stretch (Rubber Band), reverse |
| 1.5 | Undo/Redo System | `[ ]` | 1w | | Command pattern, history compression, snapshots |

---

## Phase 2: MIDI & Sequencing (Months 9-14)

**Goal**: Full MIDI recording, piano roll editing, MIDI effects

### Phase 2 Features

| ID | Feature | Status | Est. | Actual | Notes |
|----|---------|--------|------|--------|-------|
| 2.1 | MIDI Track Record/Playback | `[ ]` | 2w | | Input routing, chase events, loop recording |
| 2.2 | Piano Roll Editor | `[ ]` | 4w | | Note entry/selection, velocity, CC lanes, tools |
| 2.3 | MIDI Editing Tools | `[ ]` | 2w | | Quantize, humanize, transpose, scale, legato |
| 2.4 | MIDI Effect Plugins | `[ ]` | 2w | | Arpeggiator, chord, velocity curve, note filter |
| 2.5 | Notation View (Stretch) | `[ ]` | 3w | | Basic score display, print/export |

---

## Phase 3: Built-in Instruments (Months 15-20)

**Goal**: Professional-grade built-in instruments and effects

### Phase 3 Features

| ID | Feature | Status | Est. | Actual | Notes |
|----|---------|--------|------|--------|-------|
| 3.1 | Sampler (SFZ/EXS24) | `[ ]` | 4w | | Multisample, velocity layers, round-robin, loops |
| 3.2 | Subtractive Synthesizer | `[ ]` | 3w | | 2-3 osc, SVF filter, ADSR, LFO, mod matrix |
| 3.3 | Wavetable Synthesizer | `[ ]` | 3w | | WT import, morphing, spectral, unison |
| 3.4 | Drum Machine / Sampler | `[ ]` | 2w | | Step seq, choke groups, per-pad FX |
| 3.5 | Effect Rack | `[ ]` | 4w | | EQ, Comp, Reverb, Delay, Saturation, Limiter |

---

## Phase 4: Mixing & Automation (Months 21-26)

**Goal**: Complete mixing environment with full automation

### Phase 4 Features

| ID | Feature | Status | Est. | Actual | Notes |
|----|---------|--------|------|--------|-------|
| 4.1 | Mixer View | `[ ]` | 3w | | Faders, pans, sends, inserts, meters, solo/mute |
| 4.2 | Bus Routing & Groups | `[ ]` | 2w | | Aux buses, VCA, folder sums, sidechain |
| 4.3 | Automation System | `[ ]` | 3w | | Read/Write/Touch/Latch, curve editing, thinning |
| 4.4 | Bounce & Export | `[ ]` | 2w | | Stem export, mixdown, video sync, dither options |

---

## Phase 5: Plugin Hosting (Months 27-32)

**Goal**: Host third-party CLAP/VST3 plugins reliably

### Phase 5 Features

| ID | Feature | Status | Est. | Actual | Notes |
|----|---------|--------|------|--------|-------|
| 5.1 | CLAP/VST3 Wrapper | `[ ]` | 4w | | nih-plug/cpdal integration, parameter mapping |
| 5.2 | Plugin Scanner & Manager | `[ ]` | 2w | | Recursive scan, validation, categorization, blocklist |
| 5.3 | Sandbox & Crash Recovery | `[ ]` | 3w | | Process isolation, shared mem, auto-restart |
| 5.4 | Plugin UI Embedding | `[ ]` | 2w | | Native window embedding, resize, key focus |
| 5.5 | Plugin State & Presets | `[ ]` | 1w | | Save/load plugin state, preset browser integration |

---

## Phase 6: Polish & Platform Expansion (Months 33-36)

**Goal**: Production-ready on all platforms, custom UI, factory content

### Phase 6 Features

| ID | Feature | Status | Est. | Actual | Notes |
|----|---------|--------|------|--------|-------|
| 6.1 | macOS Support | `[ ]` | 4w | | Core Audio, AUv3, notarization, Apple Silicon |
| 6.2 | Windows Support | `[ ]` | 3w | | WASAPI, VST3, installer (MSIX/NSIS) |
| 6.3 | Custom UI Toolkit Migration | `[ ]` | 6w | | iced/wgpu, custom widgets, animation system |
| 6.4 | Performance Optimization | `[ ]` | 3w | | SIMD DSP, profile-guided, multithread scaling |
| 6.5 | Documentation & Factory Content | `[ ]` | 4w | | Manual, tutorials, presets, samples, wavetables |

---

## Milestone Checkpoints

| Milestone | Target Date | Criteria |
|-----------|-------------|----------|
| M0: Hello Audio | 2026-11-06 | Sine wave plays, transport works, UI opens |
| M1: Record & Edit Audio | 2027-02-06 | Record → edit → play back works |
| M2: MIDI Sequencing | 2027-06-06 | Piano roll edits MIDI, plays instruments |
| M3: Built-in Instruments | 2027-10-06 | Sampler + 2 synths + effects usable |
| M4: Full Mix & Automate | 2028-02-06 | Mixer + automation + bounce works |
| M5: Plugin Host | 2028-06-06 | Load CLAP/VST3, stable, sandboxed |
| M6: v1.0 Release | 2028-10-06 | All platforms, custom UI, factory content |

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Audio engine complexity | High | High | Use `fundsp`/`dasp` initially; custom later |
| UI performance at scale | Medium | High | Virtualize timeline; GPU waveform rendering |
| Plugin compatibility | High | High | Test against Vital, Surge, Valhalla early |
| Burnout (solo, 3yr) | High | Critical | Monthly releases; users = motivation; monetize |
| Linux audio fragmentation | Medium | Medium | Target PipeWire only (covers JACK/Pulse/ALSA) |
| License decision blocks plugins | Medium | High | Decide by Phase 5; legal review |

---

## Dependency Graph

```
0.1 → 0.2 → 0.3 → 0.4 → 0.5
                    ↓
1.1 ← 1.2 ← 1.3 ← 1.4 ← 1.5
                    ↓
2.1 → 2.2 → 2.3 → 2.4 → 2.5
                    ↓
3.1 → 3.2 → 3.3 → 3.4 → 3.5
                    ↓
4.1 → 4.2 → 4.3 → 4.4
                    ↓
5.1 → 5.2 → 5.3 → 5.4 → 5.5
                    ↓
6.1 → 6.2 → 6.3 → 6.4 → 6.5
```

---

## Weekly Cadence (Suggested)

| Day | Focus |
|-----|-------|
| Mon | Plan week, review PRs/issues |
| Tue-Thu | Deep work on current feature |
| Fri | Testing, refactoring, docs, commit |
| Sat | Optional: learning, experiment |
| Sun | Rest |

---

## Notes

- Each feature file in `phases/` contains detailed spec, tasks, and progress log
- Update this file when feature status changes
- Commit with conventional commits: `feat(1.1): add audio track record arm`