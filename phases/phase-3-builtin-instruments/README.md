# Phase 3: Built-in Instruments

**Duration**: Months 15-20 (Weeks 57-80)
**Goal**: Professional-grade built-in instruments and effects

## Features

| ID | Feature | Status | File |
|----|---------|--------|------|
| 3.1 | Sampler (SFZ/EXS24) | `[ ]` | `sampler.md` |
| 3.2 | Subtractive Synthesizer | `[ ]` | `subtractive-synth.md` |
| 3.3 | Wavetable Synthesizer | `[ ]` | `wavetable-synth.md` |
| 3.4 | Drum Machine / Sampler | `[ ]` | `drum-machine.md` |
| 3.5 | Effect Rack | `[ ]` | `effect-rack.md` |

## Dependencies

- 3.1 → 3.2 → 3.3 (sequential, shared DSP framework)
- 3.4 can start after 3.1
- 3.5 can start after 0.2 (DSP framework)

## Milestone: M3 — Built-in Instruments (Week 68)

- [ ] Sampler loads SFZ/EXS24, plays multisamples
- [ ] Subtractive synth: 2 osc, filter, ADSR, LFO, mod matrix
- [ ] Wavetable synth: morphing, spectral
- [ ] Drum machine: step seq, choke groups
- [ ] Effect rack: EQ, Comp, Reverb, Delay, Sat, Limiter