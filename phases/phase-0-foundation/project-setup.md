# Feature: Project Setup & Workspace

## Phase: 0 — Foundation
## ID: 0.1
## Priority: Critical
## Estimated: 1 week
## Status: `[~]` In Progress

## Description
Set up Rust Cargo workspace, CI/CD pipeline, linting, formatting, and development tooling. Establish project conventions.

## Requirements

- [✓] Cargo workspace with members: `daw-core`, `daw-engine`, `daw-ui`, `daw-plugins`, `xtask`, `zoft-app`
- [ ] Rust toolchain pinned (rust-toolchain.toml)
- [ ] GitHub Actions CI: build, test, clippy, fmt, audit
- [ ] Pre-commit hooks: fmt, clippy, cargo check
- [~] Dependency policy: central workspace dependencies exist; no automated audit configured yet
- [ ] Versioning: SemVer, conventional commits, changelog (no CHANGELOG yet)
- [✓] Logging: `tracing` + `tracing-subscriber` (structured, filtered)
- [✓] Error handling: `thiserror` + `anyhow` (library vs app)
- [~] Configuration: `config` crate available and a preferences model exists, but it is not wired into loading yet
- [ ] Profiling: `pprof` / `flamegraph` integration (workspace dependency only, not wired)

## Technical Details

### Workspace Structure
```
Cargo.toml (workspace)
├── daw-core/        # Core types, project model, commands, undo
├── daw-engine/      # Audio engine, graph, transport, DSP
├── daw-ui/          # egui/iced UI, editors, views
├── daw-plugins/     # Built-in instruments & effects
├── xtask/           # Build scripts, codegen, release automation
└── zoft-app/        # Main application binary
```

### CI Pipeline (GitHub Actions)
```yaml
jobs:
  check:
    - cargo fmt --check
    - cargo clippy -- -D warnings
    - cargo check --workspace
  test:
    - cargo test --workspace
  audit:
    - cargo audit
  build:
    - cargo build --release --workspace
```

### Key Dependencies (Initial)
```toml
[dependencies]
# Core
anyhow = "1.0"
thiserror = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
config = "0.14"
uuid = { version = "1.0", features = ["v4", "serde"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Audio (Phase 0.2)
cpal = { version = "0.15", default-features = false }

# UI (Phase 0.4)
egui = "0.29"
eframe = "0.29"

# Utils
parking_lot = "0.12"
slotmap = "1.0"  # Generational indices for tracks/clips
```

## Acceptance Criteria

- [✓] `cargo build --workspace` succeeds
- [✓] `cargo test --workspace` passes (even if empty)
- [✓] `cargo clippy --workspace -- -D warnings` passes
- [✓] `cargo fmt --check` passes
- [✓] CI runs on push/PR
- [✓] Pre-commit hooks installed and working

## Progress Log

- 2026-10-07: Created workspace structure with 6 crates
- 2026-10-07: Central dependency policy established in the workspace Cargo.toml
- 2026-10-07: Git repository initialized and pushed to GitHub
- 2026-10-21: Status corrected to In Progress: no `rust-toolchain.toml`, GitHub Actions workflow, pre-commit hooks, CHANGELOG or wired profiling/config yet