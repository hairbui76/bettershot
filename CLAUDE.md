# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

**bettershot** — a cross-platform screenshot *capture and annotation* tool written in Rust. Targets **Windows and Linux** first; **macOS is planned** (see ROADMAP.md). The product idea and annotation architecture are based on [Satty](https://github.com/Satty-org/Satty), but bettershot goes further: Satty only annotates images piped into it, while bettershot also performs the screen capture itself (region/window/monitor selection).

### The `Satty/` directory is a READ-ONLY reference

`Satty/` is a full checkout of the upstream Satty repo (GTK4/Relm4 + femtovg, Linux/Wayland-only). It exists purely as an architectural reference:

- **Never modify anything under `Satty/`.** No edits, no builds that write into it, no git operations inside it.
- Read it freely to understand proven designs before implementing the bettershot equivalent — especially `Satty/src/tools/` (the `Tool`/`Drawable` traits), `Satty/src/sketch_board.rs` (event flow, undo/redo), `Satty/src/femtovg_area/` (canvas rendering), and `Satty/src/configuration.rs` (config/CLI precedence).
- Satty is MPL-2.0. bettershot is also MPL-2.0 so that porting/adapting Satty code is license-clean; keep upstream copyright headers on any file that started as a port.

## Workspace layout

Cargo workspace (crates are created per ROADMAP phases; check which exist before referencing them):

- `crates/core` — platform/renderer-agnostic annotation model: geometry (`Vec2D`), `Style`, the `Tool` and `Drawable` traits, per-tool implementations, undo/redo stacks, crop state. **No windowing, GPU, or OS dependencies here.**
- `crates/capture` — screen capture abstraction (`CaptureBackend` trait) with per-OS backends behind `cfg(target_os)`: Windows Graphics Capture on Windows; xdg-desktop-portal (Wayland) and X11 fallback on Linux; macOS stub until Phase 4.
- `crates/app` — the binary: winit + egui + wgpu shell, fullscreen overlay for region selection, annotation editor window, toolbars, keybindings, clipboard/save output, config loading.
- `crates/cli` — clap argument definitions as a library, shared by the app and by `build.rs` for shell completions and the manpage (same pattern as `Satty/cli`).

## Commands

Standard Rust workspace; run everything from the repo root, never inside `Satty/`:

- Build: `cargo build --workspace` (release: `cargo build --workspace --release`)
- Run: `cargo run -p bettershot -- <args>` (e.g. `--filename shot.png`, `--capture region`)
- Test all: `cargo test --workspace`
- Single test: `cargo test -p bettershot-core <test_name>`
- Lint (CI-enforced): `cargo clippy --workspace --all-targets -- -D warnings`
- Format: `cargo fmt --all` (check: `cargo fmt --all -- --check`)

Platform notes:
- This dev machine is Linux; Windows-specific code compiles only under `cfg(target_os = "windows")` — cross-check it with `cargo check --workspace --target x86_64-pc-windows-msvc` when the toolchain target is installed (`cargo check` alone silently skips Windows-only modules; a compile-error in them will only surface on CI otherwise).
- Capture backends can't be exercised headlessly; keep capture logic thin and put everything testable (geometry, model, config parsing) in `crates/core`.

## Architecture (big picture)

The design deliberately mirrors Satty's proven event-driven model, decoupled from GTK:

1. **Event flow**: the app shell translates winit/egui input into semantic `InputEvent`s (mouse/key/text) and forwards them to the active tool. Each tool returns a `ToolUpdateResult` (`Redraw`, `Commit(Drawable)`, `Unmodified`, ...) — tools never draw or touch OS APIs directly. See `Satty/src/tools/mod.rs` for the reference trait shape.
2. **Scene model**: committed `Drawable`s go onto an undo stack; redo pops from a redo stack. Rendering = base screenshot texture → committed drawables in order → active tool's in-progress drawable → crop overlay. Blur/highlight are drawables that sample the base texture, not pixel edits — the original screenshot is never mutated until export.
3. **Capture is a separate concern**: `CaptureBackend::capture(target) -> RawFrame (RGBA + monitor geometry + scale factor)`. The annotation editor takes a `RawFrame` regardless of whether it came from a live capture, `--filename`, or stdin. This is what keeps macOS support additive.
4. **Config precedence**: CLI args > config file (`config.toml` in the platform config dir via `directories`) > built-in defaults — same rule as Satty. All user-tunable behavior (default tool, palette, save paths, `copy-command`, actions on Enter/Esc) lives in config; CLI mirrors it.
5. **HiDPI**: all tool/drawable coordinates are in image-pixel space, never window-logical space; the view transform (zoom/pan/scale factor) is applied only at render and input-translation time. Mixing these up is the classic bug class here — Satty's `math.rs` shows the transform helpers.

## Working rules

- ROADMAP.md is the plan of record — check which phase is current, work within it, and update checkboxes when a milestone lands.
- AGENTS.md carries the same rules for non-Claude agents; keep the two consistent when either changes.
- Keep `crates/core` free of `egui`/`winit`/`wgpu`/OS types — it must compile for all targets and hold nearly all unit tests.
