<div align="center">

<img src="assets/icons/bettershot-128.png" alt="" width="112" height="112">

# bettershot

**Capture the screen. Mark it up. Move on.**

A fast, cross-platform screenshot and annotation tool that takes the screenshot
*itself* — no external grabber, no shell pipeline, one binary and one hotkey.

[![CI](https://github.com/hairbui76/bettershot/actions/workflows/ci.yml/badge.svg)](https://github.com/hairbui76/bettershot/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/hairbui76/bettershot?include_prereleases&sort=semver)](https://github.com/hairbui76/bettershot/releases)
[![Licence: MPL-2.0](https://img.shields.io/badge/licence-MPL--2.0-blue)](LICENSE)

[Install](#install) · [Usage](#usage) · [Configuration](#configuration) · [Building](#building) · [Docs](#documentation)

</div>

---

## What it does

Press a key. The screen freezes, a capture bar appears, you pick a region, a
window or a monitor — then annotate it and send it to the clipboard or a file.

|  |  |
| --- | --- |
| **Capture** | Region, window, monitor or the whole desktop. Snap to window edges, delay for menus, optionally include the pointer. |
| **Annotate** | Arrows, lines, boxes, ellipses, freehand, text, numbered steps, highlight, and blur or pixelate for redaction. |
| **Finish** | Clipboard or file, with strftime-templated names. Crop, undo and redo throughout. |
| **Stay out of the way** | Runs in the background behind a global hotkey, with a tray icon. The editor floats above other windows, like the Snipping Tool. |

**Redaction is real.** Blur and pixelate always sample the *original*
screenshot, and the on-screen preview and the exported file come out of the
same code path — so what looks hidden is hidden in the file. Asserted
byte-for-byte by tests.

## Platform support

| Platform | Status |
| --- | --- |
| **Windows 10 1903+** | Supported. Windows Graphics Capture, installer, global hotkeys, tray. |
| **Linux — Wayland** | Supported via `xdg-desktop-portal`. Global hotkeys are impossible on Wayland by design; bind your compositor instead. |
| **Linux — X11** | Supported. Direct grab, global hotkeys, cursor capture. |
| **macOS 14+** | **Written but never run.** Compiles and passes clippy for `aarch64-apple-darwin`, including the ScreenCaptureKit backend, but no one has executed it on a Mac. See [ROADMAP.md](ROADMAP.md). |

> **Pre-1.0.** Builds are green on real Linux, Windows and macOS runners and the
> test suite is thorough, but releases are marked pre-release and the binaries
> are unsigned. See [ROADMAP.md](ROADMAP.md) for what is proven and what is not.

## Install

### Windows

Download the installer from the [latest release][releases]:
`bettershot-<version>-x64-unsigned.msi`.

It installs to your user profile (no admin needed), adds Start-menu shortcuts,
and — unless you untick it on the feature page — **starts bettershot in the
background at login**, so the hotkey is live before you need it.

> The installer is **not code-signed**. SmartScreen will warn on first run:
> *More info → Run anyway*. Signing needs an Authenticode certificate this
> project does not have yet.

### Linux

Portable archive from the [latest release][releases], or build from source. See
[docs/platform-setup.md](docs/platform-setup.md) for per-distribution
dependencies. Flatpak and AUR packaging is written and builds in CI, but is not
yet published to Flathub or the AUR.

[releases]: https://github.com/hairbui76/bettershot/releases

## Why another one

Heavily inspired by [Satty](https://github.com/Satty-org/Satty), whose
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

### First run

```sh
bettershot --daemon
```

That is the arrangement most people want: nothing on screen until you press the
key. It shows a tray icon, and tells you at startup which hotkeys are live —
a background app with no window has nowhere else to say so.

On Windows the installer sets this up at login for you.

### The capture bar

While the selection overlay is up, a floating bar at the top switches what a
click selects — **Region**, **Window** or **Monitor** — plus **Full screen** to
grab everything at once. Keys <kbd>1</kbd>–<kbd>4</kbd> do the same, and
<kbd>Esc</kbd> cancels. Switching mode mid-selection does not re-take the
screenshot: it is all chosen from the one frozen frame.

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
# Keep the window above other windows, like the Windows Snipping Tool, so the
# editor does not vanish behind whatever you just captured. Wayland
# compositors may ignore it.
always-on-top = true
action-on-enter = "save-to-clipboard"
action-on-escape = "exit"
output-filename = "~/Pictures/shot-%Y%m%d-%H%M%S.png"
color-palette = ["#eb4d4b", "#6ab04c", "#22a6b3", "#f0932b", "#c825b8", "#130f40"]

[capture]
mode = "region"
delay-seconds = 0
snap-to-windows = true
# Draw the mouse pointer into the shot. Works on X11 and Windows; the Wayland
# screenshot portal has no cursor control at all, and macOS does not supply one
# yet. The settings window says so rather than offering a checkbox that does
# nothing, and `--include-cursor` logs a warning instead of being ignored.
include-cursor = false
```

### Staying resident

This is the Snipping Tool arrangement: nothing on screen until you press the
key, then the frozen screen with the capture bar over it.

```sh
bettershot --daemon
```

It runs with no visible window until a global hotkey or the tray icon starts a
capture, then shows the overlay with the mode bar. On startup it sends a
desktop notification naming the keys that are live — a daemon has no window to
put that in, and on Windows the binary has no console either, so a hotkey that
failed to register would otherwise be indistinguishable from a program that is
not running.

> **Windows 11 takes PrintScreen.** Recent builds bind it to the built-in
> Snipping Tool, so bettershot's default binding collides on a stock install
> and the startup notification will say so. Either free the key up under
> *Settings → Accessibility → Keyboard*, or give bettershot a different one
> below.

While it is running you get a **tray icon** in the notification area, whose
menu starts a capture, opens Settings or quits — so there is always something
on screen saying it is alive, even when no hotkey is registered.

> **Windows hides new tray icons.** They go into the overflow behind the `^`
> arrow next to the clock until you drag one onto the taskbar. If you cannot
> see bettershot, look there first — the startup notification says whether the
> icon was actually created.

Hotkeys can be edited in **Settings → Hotkeys** rather than by hand, and are
re-registered as soon as you change them; you do not have to restart to find
out whether the key you picked was already taken.

Configure it with:

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
