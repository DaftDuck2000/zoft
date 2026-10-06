# Phase 5: Plugin Hosting

**Duration**: Months 27-32 (Weeks 105-128)
**Goal**: Host third-party CLAP/VST3 plugins reliably

## Features

| ID | Feature | Status | File |
|----|---------|--------|------|
| 5.1 | CLAP/VST3 Wrapper | `[ ]` | `clap-vst3-wrapper.md` |
| 5.2 | Plugin Scanner & Manager | `[ ]` | `plugin-scanner.md` |
| 5.3 | Sandbox & Crash Recovery | `[ ]` | `sandbox-crash-recovery.md` |
| 5.4 | Plugin UI Embedding | `[ ]` | `plugin-ui-embedding.md` |
| 5.5 | Plugin State & Presets | `[ ]` | `plugin-state-presets.md` |

## Dependencies

- 5.1 → 5.2 → 5.3 → 5.4 (sequential)
- 5.5 can start after 5.1

## Milestone: M5 — Plugin Host (Week 116)

- [ ] Load CLAP/VST3 plugins
- [ ] Scanner finds & validates plugins
- [ ] Sandbox: crash doesn't kill DAW
- [ ] Plugin UI embeds in window
- [ ] State save/restore with project