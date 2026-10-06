# Feature: Documentation & Factory Content

## Phase: 6 — Polish & Platform Expansion
## ID: 6.5
## Priority: High
## Estimated: 4 weeks
## Status: `[ ]` Not Started
## Depends On: All previous phases

## Description
Complete user documentation, video tutorials, factory presets/samples/wavetables, demo projects.

## Requirements

- [ ] **User Manual**: Online (mdBook), PDF, searchable, versioned
- [ ] **Video Tutorials**: 10-15 videos (getting started, recording, MIDI, mixing, instruments)
- [ ] **API Docs**: Rustdoc for plugin developers, C API for extensions
- [ ] **Factory Presets**: 200+ for each built-in instrument
- [ ] **Factory Samples**: 500+ MB royalty-free (drums, loops, one-shots)
- [ ] **Factory Wavetables**: 100+ for wavetable synth
- [ ] **Demo Projects**: 5 complete songs showcasing features
- [ ] **Keyboard Shortcut Reference**: Printable PDF, in-app cheat sheet
- [ ] **Troubleshooting Guide**: Common issues, FAQ, log collection
- [ ] **Localization**: English + 3 languages (Spanish, German, Japanese)

## Technical Details

### Documentation (mdBook)
```markdown
# Zoft User Manual

## Getting Started
- Installation (Linux/macOS/Windows)
- First Project
- Audio/MIDI Setup
- Interface Overview

## Recording
- Audio Recording
- MIDI Recording
- Loop Recording
- Punch In/Out

## Editing
- Arrange View
- Piano Roll
- Audio Editing
- MIDI Editing

## Instruments
- Sampler
- Subtractive Synth
- Wavetable Synth
- Drum Machine

## Effects
- EQ, Compressor, Reverb, Delay
- Saturation, Limiter
- Modulation Effects

## Mixing
- Mixer Console
- Routing & Buses
- Automation
- VCA Groups

## Advanced
- Plugin Hosting
- Sidechaining
- External Hardware
- Video Sync

## Reference
- Keyboard Shortcuts
- Menu Reference
- Preferences
- Troubleshooting
```

### Factory Content Structure
```
assets/
├── presets/
│   ├── sampler/
│   │   ├── Piano/
│   │   ├── Strings/
│   │   ├── Pads/
│   │   └── ...
│   ├── subtractive/
│   │   ├── Bass/
│   │   ├── Lead/
│   │   ├── Pad/
│   │   └── ...
│   ├── wavetable/
│   │   ├── Aggressive/
│   │   ├── Atmospheric/
│   │   └── ...
│   ├── drum_machine/
│   │   ├── HipHop/
│   │   ├── House/
│   │   ├── Trap/
│   │   └── ...
│   └── effects/
│       ├── Mixing/
│       ├── Creative/
│       └── Mastering/
├── samples/
│   ├── drums/
│   │   ├── kicks/
│   │   ├── snares/
│   │   ├── hats/
│   │   └── percussion/
│   ├── loops/
│   │   ├── drum_loops/
│   │   ├── bass_loops/
│   │   └── melodic_loops/
│   └── one_shots/
│       ├── fx/
│       └── vocals/
├── wavetables/
│   ├── basic/
│   ├── spectral/
│   ├── vocal/
│   └── noise/
└── projects/
    ├── demo_hiphop.zoft
    ├── demo_edm.zoft
    ├── demo_rock.zoft
    ├── demo_ambient.zoft
    └── demo_scoring.zoft
```

### Content Licensing
- All samples: CC0 or custom royalty-free license
- Presets: MIT/CC0
- Wavetables: CC0
- Demo projects: CC-BY (credit required)

### In-App Help System
```rust
struct HelpSystem {
    manual_url: String,           // https://docs.zoft.daw
    videos_url: String,           // https://youtube.com/@zoftdaw
    shortcuts: ShortcutDatabase,
    tooltips: TooltipDatabase,
    context_help: HashMap<WidgetId, HelpTopic>,
}

impl HelpSystem {
    fn show_shortcut_cheatsheet(&self) -> Window { ... }
    fn open_manual_at(&self, topic: &str) { ... }
    fn show_tooltip(&self, widget: WidgetId) -> Option<String> { ... }
}
```

### Demo Project Metadata
```json
{
  "name": "EDM Demo",
  "description": "Shows wavetable synth, sidechain compression, automation",
  "bpm": 128,
  "key": "Am",
  "tags": ["edm", "wavetable", "sidechain", "automation"],
  "instruments_used": ["wavetable", "drum_machine", "sampler"],
  "effects_showcase": ["compressor", "reverb", "delay", "saturation"],
  "difficulty": "intermediate"
}
```

## Acceptance Criteria

- [ ] Manual: complete, searchable, PDF export
- [ ] Videos: 10-15, 5-15 min each, captions
- [ ] Presets: 200+ per instrument, categorized, tagged
- [ ] Samples: 500+ MB, organized, royalty-free
- [ ] Wavetables: 100+, diverse
- [ ] Demo projects: 5, different genres, documented
- [ ] Shortcuts: printable PDF, in-app searchable
- [ ] Localization: 4 languages, 90%+ coverage
- [ ] Total asset size: < 2 GB compressed

## Progress Log

- YYYY-MM-DD: Manual structure + writing
- YYYY-MM-DD: Video recording + editing
- YYYY-MM-DD: Preset creation (sampler)
- YYYY-MM-DD: Preset creation (subtractive)
- YYYY-MM-DD: Preset creation (wavetable)
- YYYY-MM-DD: Preset creation (drum machine)
- YYYY-MM-DD: Sample library curation
- YYYY-MM-DD: Wavetable library
- YYYY-MM-DD: Demo projects
- YYYY-MM-DD: Localization
- YYYY-MM-DD: Packaging + compression