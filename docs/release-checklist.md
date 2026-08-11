# Release checklist

The roadmap's v1.0 criterion is "Phase 1–3 acceptance criteria hold on Windows
plus the four Linux environments". This page turns that into something a person
can actually walk through, because "it builds" is not evidence that a
screenshot tool works.

Everything below needs a real desktop session. None of it can be checked on a
headless machine or in CI, which is exactly why it is a checklist and not a
test.

This was tested: a headless GNOME Shell session (virtual monitor, software
rendering) is enough to exercise the capture backend selection and the D-Bus
paths — it found two real bugs — but **not** enough to render a frame. A
minimal eframe control app (`cargo run -p bettershot --example smoke`) fails
there identically, so a session like that cannot substitute for the checks
below.

## Automated gates (CI does these)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo clippy -p bettershot --features tray --all-targets -- -D warnings`
- [ ] `cargo check --workspace --target x86_64-pc-windows-msvc`
- [ ] `cargo check --target aarch64-apple-darwin` for the libraries **and** the
      binary (`-p bettershot --no-default-features --features tray`)
- [ ] Tests pass on `ubuntu-latest`, `windows-latest` and `macos-latest`
- [ ] Release builds succeed on all three, with `--features tray`
- [ ] `Satty/` is still untracked

## Per-environment manual verification

Repeat for **Windows 11**, **GNOME (Wayland)**, **KDE Plasma (Wayland)**,
**Sway (wlroots)** and **X11**.

### Capture
- [ ] `--capture region` — the overlay covers every monitor and does **not**
      appear in its own screenshot
- [ ] Drag-select produces exactly the region shown, to the pixel
- [ ] `--capture window` — clicking a window captures its bounds
- [ ] Window snapping highlights the window under the pointer
- [ ] `--capture monitor` grabs the monitor under the pointer
- [ ] `--capture all` stitches every monitor with no seams or gaps
- [ ] `--delay 3` waits, so an open menu can be captured
- [ ] <kbd>Esc</kbd> in the overlay exits cleanly with no file written

### Mixed-DPI multi-monitor (the highest-risk area)
- [ ] A 100% and a 150% display side by side both capture at their true
      physical pixel size
- [ ] The stitched `--capture all` image has both at correct relative scale
- [ ] Region selection lands correctly when dragging **across** the boundary
- [ ] A monitor positioned left of or above the primary (negative coordinates)
      captures correctly

### Editor
- [ ] Every tool draws where the pointer is, at 100%, zoomed in, and zoomed out
- [ ] Annotations stay locked to the image while panning and zooming
- [ ] <kbd>Shift</kbd> snaps lines/arrows to 15° and shapes to squares
- [ ] Undo and redo walk the full history, including crop
- [ ] Crop rebases existing annotations correctly
- [ ] Numbered markers renumber correctly after undo
- [ ] Text accepts **CJK input via an IME**, including composition and
      backspace mid-composition — this is the known-weak area
- [ ] Blur and pixelate **look identical on screen and in the saved file**
- [ ] Post-paint: select, drag and delete a committed annotation

### Output
- [ ] <kbd>Ctrl</kbd>+<kbd>C</kbd> puts an image on the clipboard that pastes
      correctly into a browser, a chat client and an image editor
- [ ] <kbd>Ctrl</kbd>+<kbd>S</kbd> writes to the configured path, with
      strftime placeholders expanded
- [ ] <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> opens the native dialog
- [ ] `copy-command = "wl-copy"` works where the built-in clipboard does not
- [ ] Save and copy notifications appear
- [ ] Recent-capture history re-copies the right image

### Daemon mode (Windows; Linux where hotkeys are possible)
- [ ] `--daemon` starts with no visible window
- [ ] Each configured hotkey starts the right capture
- [ ] A hotkey already owned by another app is reported, and the others still work
- [ ] The tray menu drives every capture mode, opens Settings, and quits
- [ ] On Wayland: bettershot reports hotkeys are unavailable and keeps running
- [ ] Finishing a capture returns to waiting rather than exiting
- [ ] Memory does not grow across ~50 captures

### Performance
- [ ] Startup to visible overlay is under 150 ms on mid-range hardware
- [ ] Editing a 4K capture stays at the display's refresh rate
- [ ] Dragging a large blur does not stutter

## macOS first-run (once hardware exists)

The macOS backend has never been executed. Before it can be called supported:

- [ ] Screen Recording prompt appears on first capture
- [ ] Denying, then granting in System Settings, then relaunching, works — and
      the "you must relaunch" message appears in between
- [ ] **Mixed-DPI display origins** — a 2× and a 1× display side by side. This
      is a known gap: sizes should be right, origins may overlap. See ROADMAP.
- [ ] Window frames are top-left-origin (assumed, unverified)
- [ ] Window capture excludes shadows; z-order matches what is on screen
- [ ] A capture started from the main thread does not hang
- [ ] Menu bar item appears and its menu works
- [ ] Retina: a 2× capture is at true pixel size, not upscaled

## Before tagging

- [ ] Version bumped in the workspace `Cargo.toml`
- [ ] `Cargo.lock` updated and committed
- [ ] AppStream metainfo has a `<release>` entry for this version
- [ ] README and docs describe what actually ships
- [ ] `cargo dist plan` produces the expected artefacts
- [ ] Windows MSI signed; macOS dmg notarized and stapled
- [ ] A migration note if any config key changed meaning, and
      `CONFIG_VERSION` bumped with a migration if the schema changed

## Tagging

```sh
git tag -a v1.0.0 -m "bettershot 1.0.0"
git push origin v1.0.0
```

The release workflow builds and uploads the artefacts. Signing and notarization
are separate steps that need credentials the repository does not hold.
