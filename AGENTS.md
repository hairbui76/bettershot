# AGENTS.md — instructions for AI coding agents

bettershot is a cross-platform (Windows + Linux, macOS planned) screenshot capture **and** annotation tool in Rust, architecturally based on the Satty project. Read `CLAUDE.md` for the full architecture overview and `ROADMAP.md` for the phased plan of record.

## Hard rules

1. **`Satty/` is a read-only upstream reference.** Never edit, build into, or run git commands inside it. Read it to understand designs (`src/tools/`, `src/sketch_board.rs`, `src/femtovg_area/`, `src/configuration.rs`) before implementing the bettershot equivalent.
2. **Licensing**: bettershot is MPL-2.0 (chosen to keep porting Satty code clean). Any file adapted from Satty keeps upstream's MPL-2.0 header and attribution.
3. **Layering**: `crates/core` (annotation model, tools, geometry, undo/redo) must stay free of windowing/GPU/OS dependencies. OS-specific capture code lives only in `crates/capture` behind `cfg(target_os)`. UI shell code lives only in `crates/app`.
4. **Coordinates**: tools and drawables operate in image-pixel space; view transforms (zoom, pan, HiDPI scale) are applied only at the render/input boundary in the app shell.
5. **Config precedence**: CLI > `config.toml` > defaults. Every new user-facing option gets both a config key and a CLI flag (defined in `crates/cli`), plus documentation in README.

## Workflow

- Work within the current ROADMAP phase; tick milestone checkboxes in `ROADMAP.md` when they land. Propose (don't silently do) scope that belongs to a later phase.
- Validation gate before claiming completion:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - If Windows-only code was touched and the target is installed: `cargo check --workspace --target x86_64-pc-windows-msvc`
- Capture backends are not headless-testable; put logic in `crates/core` and keep backends thin. New model/geometry/config code ships with unit tests next to it.
- Commit style: imperative subject, body explains why. Never commit changes under `Satty/`.

## Quick commands

| Task | Command |
| --- | --- |
| Build | `cargo build --workspace` |
| Run | `cargo run -p bettershot -- <args>` |
| Test all | `cargo test --workspace` |
| One test | `cargo test -p bettershot-core <name>` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Format | `cargo fmt --all` |
