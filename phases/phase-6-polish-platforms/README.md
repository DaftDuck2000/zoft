# Phase 6: Polish & Platform Expansion

**Duration**: Months 33-36 (Weeks 129-152)
**Goal**: Production-ready on all platforms, custom UI, factory content

## Features

| ID | Feature | Status | File |
|----|---------|--------|------|
| 6.1 | macOS Support | `[ ]` | `macos-support.md` |
| 6.2 | Windows Support | `[ ]` | `windows-support.md` |
| 6.3 | Custom UI Toolkit Migration | `[ ]` | `custom-ui-toolkit.md` |
| 6.4 | Performance Optimization | `[ ]` | `performance-optimization.md` |
| 6.5 | Documentation & Factory Content | `[ ]` | `documentation-content.md` |

## Dependencies

- 6.1, 6.2 can start after 5.1 (parallel)
- 6.3 after 6.1, 6.2 (needs platform windowing)
- 6.4 throughout, focused at end
- 6.5 throughout, finalized at end

## Milestone: M6 — v1.0 Release (Week 148)

- [ ] macOS: Core Audio, AUv3, notarized, Apple Silicon
- [ ] Windows: WASAPI, VST3, installer, signed
- [ ] Custom UI: iced/wgpu, 60 FPS, animations
- [ ] Performance: < 2ms callback, SIMD DSP
- [ ] Docs: Manual, tutorials, factory library