# bettershot

**Modern cross-platform screenshot capture and annotation.**

bettershot grabs a screenshot and lets you mark it up — arrows, boxes, text,
numbered steps, highlights, blur — then copies it to the clipboard or saves it
to a file. It runs on **Linux** and **Windows**.

**macOS support is written but unproven**: the whole program — including a
ScreenCaptureKit capture backend and its permission flow — compiles and passes
clippy for `aarch64-apple-darwin`, but nothing has ever been run on a Mac. See
[ROADMAP.md](ROADMAP.md) before relying on it.

> Status: in development, but building and testing green on real Linux, Windows
> and macOS runners. See [ROADMAP.md](ROADMAP.md) for what works today and what
> is written but still unproven.

## Why another one

It is heavily inspired by [Satty](https://github.com/Satty-org/Satty), whose
annotation UX is excellent — a small, obvious toolset with no menus to hunt
through. Two things are different here:

- **Cross-platform.** Satty is built on GTK4 and targets wlroots compositors.
  bettershot uses `winit` + `egui` + `wgpu`, so the same editor runs on Windows
  and Linux, and macOS is an additive step rather than a rewrite.
- **It takes the screenshot too.** Satty annotates an image you pipe into it
  (`grim -g "$(slurp)" - | satty --filename -`). bettershot owns the whole
  pipeline — region, window or monitor selection included — so there is one
  binary and one hotkey instead of a shell pipeline.

Satty's annotation model is genuinely good, so bettershot's is a deliberate
adaptation of it, and bettershot is MPL-2.0 like Satty so that porting is
licence-clean. The `Satty/` directory in this repository is a read-only
reference checkout, never modified and never built.

## Usage

```sh
# Capture a region, annotate it, copy to the clipboard
bettershot --capture region

# Capture a window, save straight to a dated file
bettershot --capture window --output-filename '~/shots/%Y-%m-%d_%H-%M-%S.png'

# Annotate an existing image
bettershot --filename screenshot.png

# Stay resident so a hotkey or the tray starts the next capture
bettershot --daemon

# Drop-in for a Satty-style pipeline
grim -g "$(slurp)" - | bettershot --filename -
```

### Keyboard

| Key | Action |
| --- | --- |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> | Copy to clipboard |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | Save to the configured output file |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> | Save as… |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> / <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Undo / redo |
| <kbd>Ctrl</kbd>+<kbd>T</kbd> | Toggle the toolbars |
| <kbd>Enter</kbd> | Configurable, default copy-to-clipboard (applies the crop while cropping) |
| <kbd>Esc</kbd> | Cancel the current shape; if there is none, the configured action (default exit) |
| <kbd>Shift</kbd>+<kbd>Delete</kbd> | Clear every annotation |
| <kbd>1</kbd>…<kbd>9</kbd>, <kbd>0</kbd> | Pick the nth palette colour |
| <kbd>Ctrl</kbd>+scroll, <kbd>Ctrl</kbd>+<kbd>+</kbd>/<kbd>-</kbd> | Zoom |
| <kbd>Ctrl</kbd>+<kbd>0</kbd> | Zoom to fit |
| Middle-drag, scroll | Pan |
| <kbd>Shift</kbd> while drawing | Snap lines/arrows to 15°, shapes to a square |

### Tools

Pointer, crop, line, arrow, rectangle, ellipse, text, numbered marker, brush,
highlight, and obscure (blur or pixelate).

Obscure annotations always sample the **original** screenshot, so drawing over
them, undoing, or re-cropping never leaks the pixels underneath.

## Configuration

CLI flags override the config file, which overrides the built-in defaults.

The config file lives at `~/.config/bettershot/config.toml` on Linux and
`%APPDATA%\bettershot\config.toml` on Windows.

```toml
initial-tool = "arrow"
initial-color = "#eb4d4b"
annotation-size-factor = 1.0
default-fill-shapes = false
action-on-enter = "save-to-clipboard"
action-on-escape = "exit"
output-filename = "~/Pictures/shot-%Y%m%d-%H%M%S.png"
color-palette = ["#eb4d4b", "#6ab04c", "#22a6b3", "#f0932b", "#c825b8", "#130f40"]

[capture]
mode = "region"
delay-seconds = 0
snap-to-windows = true
# Draw the mouse pointer into the shot. X11 only for now: the Wayland
# screenshot portal has no cursor control at all, and the Windows and macOS
# backends do not supply one yet. The settings window says so rather than
# offering a checkbox that does nothing, and `--include-cursor` logs a warning
# instead of being silently ignored.
include-cursor = false
```

### Staying resident

`bettershot --daemon` keeps the process running with no visible window until a
global hotkey or the tray icon starts a capture. Configure it with:

```toml
[daemon]
enabled = true
tray = true
hotkeys = [
  { key = "PrintScreen",       mode = "region"  },
  { key = "Shift+PrintScreen", mode = "window"  },
  { key = "Ctrl+PrintScreen",  mode = "monitor" },
]
```

Two caveats, both by design rather than oversight:

- **Wayland refuses global hotkeys.** The protocol will not let an application
  grab a key for itself, because anything that could would also be a keylogger.
  bettershot reports this and carries on; bind your compositor to
  `bettershot --capture region` instead.
- **The tray needs a feature flag on Linux.** `tray-icon` pulls in GTK 3 and
  libayatana-appindicator, which bettershot does not otherwise link, so build
  with `--features tray` if you want it.

## Building

Requires Rust 1.85+ (edition 2024).

```sh
cargo build --release --workspace
cargo test --workspace
```

### Features

| Feature | Default | What it does |
| --- | --- | --- |
| `notifications` | on | Desktop notifications on save and copy. |
| `tray` | off | System tray icon (the menu bar item on macOS). Needs GTK 3 and libayatana-appindicator development packages on Linux; free elsewhere. Distribution builds enable it. |

```sh
cargo build --release -p bettershot --features tray
```

Linux build dependencies are listed in
[docs/platform-setup.md](docs/platform-setup.md).

## Documentation

| Page | What it covers |
| --- | --- |
| [docs/platform-setup.md](docs/platform-setup.md) | Per-OS setup, capture backends, compositor quirks, troubleshooting |
| [docs/migrating-from-satty.md](docs/migrating-from-satty.md) | Flag and config-key mapping from Satty |
| [docs/performance.md](docs/performance.md) | Measured render cost, memory audit, startup breakdown |
| [docs/release-checklist.md](docs/release-checklist.md) | What has to be verified before tagging a release |
| [packaging/](packaging/) | Flatpak, AUR, winget, MSI and Homebrew manifests |

## Repository layout

| Crate | What it holds |
| --- | --- |
| `crates/core` | The annotation model: geometry, style, tools, drawables, undo/redo, config schema. No windowing, GPU or OS dependencies — and nearly all the tests. |
| `crates/capture` | Screen capture. The only crate with OS-specific code, always behind `cfg(target_os)`. |
| `crates/render` | A CPU rasterizer used to export the annotated image, and to test rendering headlessly. |
| `crates/cli` | Argument parsing and config layering, shared by the binary and its `build.rs`. |
| `crates/app` | The binary: the egui editor, the selection overlay, the resident daemon, clipboard and file output. |

All annotation coordinates are in **image-pixel space**; zoom, pan and HiDPI
scaling are applied only at the render and input boundary in `crates/app`.

## Licence

MPL-2.0. See [LICENSE](LICENSE).

Portions are adapted from [Satty](https://github.com/Satty-org/Satty),
Copyright the Satty authors, also MPL-2.0.
