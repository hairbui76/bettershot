# bettershot Roadmap

**Goal:** a fast, modern, cross-platform screenshot **capture + annotation** tool.
**Platforms:** Windows and Linux first-class from Phase 2; macOS in Phase 5.
**Heritage:** annotation UX and architecture based on [Satty](https://github.com/Satty-org/Satty) (see `Satty/`, read-only reference). Where Satty relies on an external grabber (`grim | satty`), bettershot owns the whole pipeline: hotkey → capture → annotate → save/clipboard.

## Status

**60 of 66 roadmap items are done**, and the project now builds and tests
**green on real Linux, Windows and macOS runners**:

| Job | Result |
| --- | --- |
| Test (ubuntu-latest) | 757 tests |
| Test (windows-latest) | 716 tests |
| Test (macos-latest) | 717 tests |
| Cross-compile check | clippy clean for `x86_64-pc-windows-msvc` and `aarch64-apple-darwin` |
| Release build | Linux, Windows and macOS, all with `--features tray` |
| MSI builds (unsigned) | a 4.4 MB installer, built from the WiX definition on every push |
| Flatpak | builds against the 25.08 runtime, when the manifest or lockfile changes |
| Packaging | AUR package builds and passes `namcap`; winget manifest validates; macOS bundle and dmg build |
| Satty reference untouched | the upstream checkout is still out of version control |

The Windows and macOS counts are lower because some tests are Linux-specific
(portal handling, X11 title decoding), not because anything is skipped.

That green run took four attempts, and each failure was a real defect that no
amount of local care could have found — see
[What CI found that nothing local could](#what-ci-found-that-nothing-local-could).

The six remaining items need a signing certificate, an Apple Developer ID,
distribution accounts, a Mac to *use*, or a desktop session that renders. None
is blocked on design or on code that could have been written here — every one
of them is a procurement or hardware dependency, which is why they are listed
separately in [Not achievable without more hardware](#not-achievable-without-more-hardware).

## What CI found that nothing local could

1. **Three clippy lints, on every platform.** CI runs stable (1.97.1); this
   machine defaulted to 1.94. Clippy gains lints over time, so an older
   compiler simply cannot see them. One was a redundant loop counter left by a
   recent change. Fixed by pinning `rust-toolchain.toml` to stable so local and
   CI agree by default.
2. **Ctrl+S typed a literal "s" into a text annotation on macOS.** The text
   tool filtered the *platform's* accelerator, which is Command there — so
   Control fell through to the character branch. Any character key held with
   Ctrl, Alt or Meta is a shortcut on every platform, and all three are
   filtered now. The test meant to cover this only tried the platform's own
   accelerator, so it passed on Linux while the bug was macOS-only.
3. **A clippy lint inside `cfg(windows)` code**, which a host-only clippy run
   cannot reach however carefully it is run. The cross-compile job now runs
   clippy rather than check for both foreign targets.
4. **A path test that hardcoded `/`.** Windows produced
   `C:\Users\runneradmin/shots\shot.webp`, which is correct — templates keep
   their own separators where the path is untouched, and `with_file_name`
   rebuilds the tail with the platform's. Every other path test compared
   `PathBuf` values, which treat both as equivalent on Windows; only this one
   compared strings.
5. **The tray does not link on Linux without `libxdo`.** Its menu layer needs
   it, and that link step had never run anywhere — the GTK development packages
   the tray requires are absent on the development machine. Now in both
   workflows, the documented build dependencies for three distribution
   families, the AUR PKGBUILD, and as a Flatpak module.

## Guiding decisions (made up front)

| Decision | Choice | Rationale |
| --- | --- | --- |
| Language | Rust (2024 edition) | Matches Satty heritage; best cross-platform native story without a GC or Electron. |
| UI/windowing | `winit` + `egui` + `wgpu` | Satty's GTK4/Adwaita stack is effectively Linux-only. winit+egui runs identically on Windows/Linux/macOS, egui gives immediate-mode toolbars *and* a painter good enough for annotation overlays; wgpu gives HW acceleration (Satty's "extremely smooth rendering" goal) and shaders for blur. |
| Capture | Per-OS native backends behind one trait | Windows Graphics Capture (via `windows`/`windows-capture`); Linux: xdg-desktop-portal Screenshot (via `ashpd`) on Wayland + X11 (`xcap`/xcb) fallback; macOS ScreenCaptureKit later. A generic crate alone can't handle portals/permissions well. |
| License | MPL-2.0 | Same as Satty → porting annotation code is clean. |
| Config | TOML in platform config dir (`directories`), CLI overrides config | Same precedence rule as Satty; users can migrate Satty configs easily. |
| Distribution | GitHub releases + winget/MSI (Windows), Flatpak/AUR/deb (Linux), brew cask (macOS later) | |

Crate layout: `crates/core` (model/tools, no OS deps) · `crates/capture` (backends) · `crates/app` (binary, shell/UI) · `crates/cli` (clap defs shared with build.rs for completions/manpage, like `Satty/cli`).

---

## Phase 0 — Bootstrap (repo & skeleton)

Deliverable: an empty-but-real project that builds and CIs green on Linux + Windows.

- [x] `git init`; add `.gitignore` (`/target`, `Satty/` — the reference checkout has its own `.git` and must not be vendored)
- [x] Cargo workspace with the four crates, `rustfmt.toml`, `deny.toml` (licenses/advisories), MPL-2.0 `LICENSE`
- [x] `crates/cli`: clap derive struct with the flag surface stubbed (`--filename`, `--capture <region|window|monitor|all>`, `--output-filename`, `--copy-command`, `--config`, `--early-exit`, `--fullscreen`)
- [x] `crates/app`: winit window opens, egui renders a placeholder, loads an image given `--filename` and displays it (no annotation yet)
- [x] GitHub Actions: fmt + clippy + test matrix on `ubuntu-latest` and `windows-latest`; release-build artifact upload
- [x] README skeleton (goal, status badge, Satty attribution)

Acceptance: `cargo run -p bettershot -- --filename x.png` shows the image in a window on both OSes.

## Phase 1 — Annotation editor at Satty parity (the core product)

Deliverable: bettershot as a **Satty replacement** on Windows + Linux: image in (file/stdin), annotated image out (file/clipboard). Ported/adapted from `Satty/src`, decoupled from GTK.

### 1a. Model & rendering foundation
- [x] `core`: `Vec2D`/rect math (port of `Satty/src/math.rs`), `Style` (color, size, fill toggle), color palette
- [x] `core`: `Tool` trait (`handle_event -> ToolUpdateResult`) and `Drawable` trait (render into an abstract `Painter`), event types (`Mouse`, `Key`, `Text`) — mirroring `Satty/src/tools/mod.rs` shapes without GTK types
- [x] `app`: render pipeline — screenshot as wgpu texture → committed drawables → active drawable → crop dimming; all drawable coordinates in image-pixel space, view transform applied at the boundary
- [x] `core`: undo/redo stacks; `app`: Ctrl+Z / Ctrl+Y

### 1b. Tools (port order = value order)
- [x] Pointer (temporary red dot), Brush, Line, Arrow
- [x] Rectangle, Ellipse (filled/outline via Style)
- [x] Marker/numbered stamp, Highlight (translucent + block modes)
- [x] Text (egui text input; IME support tracked as a known risk — Satty has a whole `src/ime/` module for this)
- [x] Obscure: blur and pixelate, sampling the base image. The editor preview and the exported file are produced by the *same* function, so a redaction that looks opaque on screen is opaque in the file — asserted byte-for-byte by tests in both crates.
- [x] Crop with draggable handles (`drag_box` equivalent)

### 1c. Editor shell
- [x] Top toolbar (tool selection incl. tool groups), bottom toolbar (palette, custom color picker, size ± controls) — layout reference: `Satty/src/ui/toolbars.rs`
- [x] Keybindings: Satty-compatible defaults (Enter/Esc actions configurable, Ctrl+C copy, Ctrl+S save, Ctrl+Shift+S save-as dialog via `rfd`, digits = palette colors, Ctrl+T toggle toolbars)
- [x] Zoom (Ctrl+wheel, pinch) and pan (middle-drag, Alt+arrows)
- [x] Output: save PNG (filename templates with `chrono` strftime), clipboard via `arboard` + configurable `copy-command` fallback (wl-copy support), post-save actions (`save-after-copy`, exit-on-action)
- [x] Config: TOML loading with full CLI mirror; document every key in README
- [x] Notifications on save/copy (`notify-rust` on Linux, Windows toast)

Acceptance: every Satty tool works on both OSes; `cat shot.png | bettershot --filename -` round-trips; config + CLI precedence tested.

## Phase 2 — Native capture (the "better" in bettershot)

Deliverable: bettershot invoked with `--capture`, no external grabber needed.

- [x] `capture`: `CaptureBackend` trait → `RawFrame { rgba, size, monitor geometry, scale_factor }`; monitor + window enumeration types
- [x] **Linux/Wayland**: xdg-desktop-portal Screenshot + ScreenCast picker via `ashpd` (works on GNOME/KDE/wlroots); document compositor quirks
- [x] **Linux/X11**: direct grab via `x11rb` (RandR 1.5 monitors, EWMH window stacking) with session auto-detection. `xcap` was rejected on Linux: it pulls wayland/gbm/drm build-time deps even for an X11-only build.
- [x] **Windows**: Windows Graphics Capture via `xcap` (monitor + window targets); per-monitor-DPI mixed-scale handling. *DXGI fallback for pre-1903 builds not implemented.*
- [x] Region selection overlay: borderless fullscreen-per-monitor frozen-frame overlay, crosshair, drag-select with magnifier + size readout, window-snap highlighting (hover picks window bounds), Esc cancels
- [x] Capture modes wired to CLI/config: `region`, `window`, `monitor`, `all` (stitched virtual desktop), `--delay <secs>`
- [x] Frozen-frame correctness: overlay shows the pre-capture frame so the tool never screenshots itself

Acceptance: `bettershot --capture region` on Sway, GNOME, KDE, X11, and Windows 10/11 produces a correct annotated capture including mixed-DPI multi-monitor setups.

## Phase 3 — Daemon UX & polish

Deliverable: bettershot as an always-available tool, not just a one-shot CLI.

- [x] Global hotkeys (`global-hotkey`), with `--daemon` residency. On Wayland registration is refused by the protocol; bettershot reports it and the compositor-keybinding route is documented.
- [x] System tray (`tray-icon`): capture region/window/monitor/all, settings, quit. Behind the `tray` feature because it needs GTK 3 on Linux; compile-verified for Windows and in CI on Linux, **not runtime-verified anywhere**.
- [x] Settings UI (egui) writing back to `config.toml`
- [x] Post-paint editing: select, drag and delete committed annotations (a stated Satty aspiration). A drag is one undo step, not one per frame.
- [x] Copy-to-clipboard history of the last N captures (memory-only, PNG-encoded, purgeable from Settings)
- [x] Filename templates (strftime) and the "copy path" action (Ctrl+Alt+C)
- [x] Theming: `theme = system | light | dark`, following the desktop by default
- [x] Localization scaffold: keyed string catalogue with fallback, wired through the toolbar and status messages. English only — a half-finished translation is worse than none.
- [x] Texture/memory audit for 4K and dual-4K, with measurements and the two decisions that bound memory: [docs/performance.md](docs/performance.md)
- [x] Startup cost measured for everything bettershot controls: **3.2 ms** to parse arguments, load config and select a backend — ~1.4 ms above the cost of starting any process ([docs/performance.md](docs/performance.md))
- [x] Startup instrumentation: `-v` now logs "first frame after Nms", so the end-to-end figure can be measured by anyone on real hardware
- [x] The algorithmic properties that target rests on are **guarded against regression** in CI: blur cost independent of radius, pixelate cost independent of block size, and export no worse than linear in pixel count. Asserted as ratios between measurements taken in the same run, so they mean the same thing on a noisy shared runner. The figures in [docs/performance.md](docs/performance.md) had been measured once by hand and could otherwise regress by an order of magnitude unnoticed.
- [ ] Confirm the end-to-end < 150 ms figure on mid-range hardware — a headless software-rendered session cannot produce a number that means anything for this target, because most of it is compositor round-trip and window creation

## Phase 4 — Packaging & 1.0

- [x] Portable archives and shell/PowerShell installers for Linux, Windows and macOS, declared in `[workspace.metadata.dist]`
- [x] Windows MSI definition (WiX v4) and winget manifest authored: [`packaging/windows/`](packaging/windows/), [`packaging/winget/`](packaging/winget/)
- [x] The MSI **actually builds**, on every push, as a CI job that uploads the (unsigned) installer. Adding that job immediately found three faults in a definition that had never been run: a missing WiX UI extension, a `license.rtf` that was not in the tree, and source paths resolved against the wrong directory. It also caught that the build instructions in the file's own header were not legal XML.
- [ ] **Sign** the MSI — needs an Authenticode certificate. CI builds the installer itself, so this signature is the only thing between the repository and a publishable one.
- [x] Flatpak manifest and AUR `PKGBUILD` authored ([`packaging/`](packaging/)); deb/rpm and portable archives come from `cargo dist`; `.desktop` file and AppStream metainfo in `assets/`
- [x] `.deb` and `.rpm` **built and verified**: `packaging/build-deb.sh` and `build-rpm.sh` produce them, both extract, the binary runs from each package tree, deb dependencies are resolved from the real link set, and the desktop entry and AppStream metainfo pass `desktop-file-validate` and `appstreamcli validate` cleanly
- [x] The Flatpak **actually builds**, in [`.github/workflows/flatpak.yml`](.github/workflows/flatpak.yml), whenever the manifest, assets or lockfile change. It found four faults in a manifest that had never been run: the source would have copied a 21 GB `target/` and followed the `Satty` symlink out of the repository; the 24.08 runtime's rustc 1.89 is too old for egui's 1.92; Rust never saw the `-L/app/lib` that flatpak-builder passes to C modules, so linking libxdo failed; and the icon was not loadable as an image at all (see below).
- [x] The AUR package **actually builds**, in an Arch container, and passes `namcap`. I had written this off as impossible on a GitHub runner, which was simply wrong. The PKGBUILD is used unmodified: its source is `NAME::URL` and `makepkg` skips the download when a file of that name is present, so a tarball of the working tree exercises `prepare`/`build`/`check`/`package`.
- [x] The winget manifest **validates** against the published schemas, split into the three documents submission needs. That caught a defect that would have been rejected in review: an unquoted run of zeroes for `InstallerSha256` is read by YAML as the integer `0`, where winget requires a 64-character hex string.
- [x] The macOS `.app` bundle and an unsigned dmg **actually build**, with `CFBundleExecutable` and `CFBundleIconFile` checked to resolve inside the bundle.
- [ ] Publish to Flathub, the AUR and winget-pkgs — each needs an account, and winget additionally needs the signed MSI. Flathub submission is a pull request against `flathub/flathub`.
- [x] Shell completions + manpage from `crates/cli` in `build.rs` (Satty's `build.rs` is the template)
- [x] Docs site or wiki: per-compositor/per-OS setup guides, Satty migration guide (config mapping table)
- [x] Crash reporting (opt-in, local-only, contains no image data) and versioned config migration with a refuse-the-future guard
- [x] Release process mechanized: a tag-triggered workflow plus an explicit acceptance checklist ([docs/release-checklist.md](docs/release-checklist.md))
- [ ] Actually tag v1.0 — requires walking that checklist on Windows and the four Linux environments

## Phase 5 — macOS

- [x] `capture` backend on ScreenCaptureKit with the full screen-recording permission flow (TCC preflight/request, stale-permission detection, guidance naming System Settings → Privacy & Security → Screen & System Audio Recording). **Compile-verified and clippy-clean for `aarch64-apple-darwin`, never run on a Mac** — see the caveat below.
- [x] Cmd-based keybindings: the accelerator modifier already resolves to Command on macOS and Control elsewhere, with a test pinning it
- [x] Menu bar presence on macOS: the tray icon *is* the menu bar item there, and the whole app including that code is **compile-verified** for `aarch64-apple-darwin`. App bundle `Info.plist` authored ([`packaging/macos/`](packaging/macos/)).
- [x] Accessory activation policy, so daemon mode leaves the Dock: `crates/app/src/platform.rs`, applied on the daemon path only (a one-shot capture genuinely is a foreground app). **Compile-verified for `aarch64-apple-darwin`** against the real AppKit bindings and covered by the cross-compile job, to the same standard as the ScreenCaptureKit backend — never run on a Mac.
- [ ] Retina scale verification — needs a Mac. Nothing about a HiDPI backing scale can be confirmed from a Linux container, and the accessory policy above still wants a human to confirm the Dock icon actually disappears.
- [x] Homebrew cask authored, including the Screen Recording permission caveat: [`packaging/homebrew/`](packaging/homebrew/)
- [ ] **Notarize** a universal dmg and publish the cask — needs an Apple Developer ID
- [x] CI: `macos-latest` in the test matrix, plus a cross-compile job. The library crates (including the macOS capture stub) are **verified** to compile for `aarch64-apple-darwin`; the binary needs a real Mac only because `notify-rust` pulls `mac-notification-sys`, which requires the Apple SDK.

## Phase 6 — Beyond (unscheduled, ideas parking lot)

- OCR of captured region to clipboard (`tesseract` or platform OCR APIs)
- Pin capture as always-on-top floating reference window
- Short screen recording → GIF/WebM/MP4
- Scrolling capture (stitched)
- Quick-share upload targets (user-configured endpoint only)

---

## What running it under a real compositor found

bettershot has now been executed against a genuine Wayland session — GNOME
Shell in headless mode with a virtual monitor and software rendering — rather
than only compiled. That is far short of a desktop, but it was enough to find
two real defects that no amount of type-checking would have:

1. **Capture hung forever when no portal was installed.** A D-Bus request to a
   name nobody owns does not fail; zbus waits for the name to appear. The fix
   is *not* a timeout on the request — a portal that is present legitimately
   takes as long as the user needs to answer its dialog — but a check that the
   service exists before asking. It now exits immediately with instructions.
2. **That fix was itself wrong at first.** The portal bus name is
   D-Bus-*activatable*: on an idle desktop nobody owns it until the first
   request starts it, so an owner check alone refuses to capture on a machine
   with a perfectly good portal. Presence is now "owned **or** activatable".

Both have regression tests.

The editor was also confirmed to create an EGL context and a window under that
compositor, but **no frame was ever drawn** in 30 seconds. That looked like it
might be a third defect, so it was checked against a control: `cargo run -p
bettershot --example smoke`, forty lines of eframe with no bettershot code in
it, behaves identically. The cause is therefore below bettershot — a
surfaceless llvmpipe session appears not to deliver the frame callbacks the
windowing stack waits on. The control example is kept in the repository for
whoever hits this next.

Rendering through the X11 path was tried too — the same headless session also
runs Xwayland — but that display could not be reached even with mutter's auth
cookie, and GNOME's headless mode has several of its own services failing here
(`SessionManager`, `Introspect`, `CalendarServer`). The container is simply not
a desktop. This is recorded so the next person does not repeat the attempt.

So the editor is still unproven on a real display. What is now proven is that
it starts, negotiates GL, and creates a window against a genuine compositor.

## What an adversarial review found

After the code was written, a review pass was run over the whole workspace
looking specifically for defects the 666 tests were missing. It found nine,
all since fixed with regression tests. The ones worth knowing about:

1. **The editor panicked mid-drag while blurring.** The preview clamped the
   rectangle to the image and *then* rounded it, so `round(left) + round(width)`
   could land one pixel past the edge and index out of bounds. Reachable with
   ordinary geometry: any odd difference between image and window width puts
   the view origin on a half pixel, and dragging a blur off the right edge did
   the rest.
2. **A redaction leak.** The preview rounded the rectangle while the exporter
   floors and ceils, so for some rectangles a column looked obscured on screen
   and shipped clear. Both are now derived from one function,
   `bettershot_render::effect_region_pixels`, so they cannot drift apart again.
3. **Seven of eleven tools committed work the user had cancelled.** Pressing
   Escape mid-drag cleared the preview, but the release that followed rebuilt
   the shape from the mouse event and committed it anyway.
4. **Crop panicked on any image smaller than 8 px**, because `f32::clamp` was
   called with `min > max`.
5. **Duplicate marker numbers**, because the next number was derived from the
   *count* of markers rather than the highest.
6. **Translucent pinholes** inside round caps and joins, from incremental edge
   stepping accumulating different rounding on each side of a shared edge.

Fixing (2) then introduced a seventh of its own, caught by re-deriving the
invariant rather than trusting the fix: the cache was keyed on the *rounded*
rectangle while the pixels were copied from the *floored* one, and those group
differently — rectangles at 0.6 and 1.4 round alike but start on different
pixels, so one annotation would have been served another's texture. The key is
now the covered pixel box itself, which is the thing that actually identifies a
texture.

Three of the original findings were invisible because **the test asserting the
behaviour encoded the wrong expectation** — the marker test asserted the duplicate, and
both redaction tests bypassed the code path that had the bug. Those tests are
now rewritten to check the real thing.

The review also could not break the undo/redo index model after 400,000
randomised interleavings, or the rasterizer's bounds handling after 60,000
hostile-geometry renders.

## Implemented but never run

The macOS capture backend (`crates/capture/src/backends/macos/`) is written
against the real ScreenCaptureKit and CoreGraphics APIs through the pure-Rust
`objc2` bindings, which means it **type-checks against Apple's actual API
surface** and passes clippy for `aarch64-apple-darwin`. Its pure logic — BGRA
row-stride unpacking, points-to-physical-pixel conversion, window z-ordering,
region-to-display resolution — is unit-tested and those tests run on any host.

None of it has ever executed on a Mac. Until it does, treat it as unproven. The
first real-hardware session should check, in this order:

1. The permission prompt appears, and its guidance text is accurate.
2. **Mixed-DPI display origins.** This is a known gap, not a suspicion. macOS
   lays displays out in *points*; scaling each display's origin by its own
   factor is exact only when every display shares a DPI. A 2× panel beside a 1×
   external produces overlapping physical origins. Sizes are right; origins are
   not, and there is no correct answer without inventing a physical layout that
   macOS does not define. See the notes on `points_to_physical`.
3. Window frames really are top-left-origin (assumed, unverified — AppKit uses
   bottom-left elsewhere).
4. Window capture excludes shadows, and z-ordering matches what you see.
5. Completion handlers fire off the main queue. If they do not, a main-thread
   capture blocks until the timeout — which is why the timeout exists.
6. Stitched multi-display geometry.

## What the second review found

A second pass covered what the first did not — the capture crate, the CLI
layering, the macOS `unsafe` boundary, and the app's daemon, overlay and
output. Fixed here:

1. **`--daemon` could wedge silently and permanently.** The code detected
   "nothing can trigger this" and pushed a warning — routed through the one
   stage that condition makes unreachable. On Wayland without the `tray`
   feature, which is the *default* Linux build, `bettershot --daemon` ran
   forever: hidden window, repainting every 80 ms, no UI, no way out but
   `kill`. It now refuses to start and says what to do instead.
2. **`--output-filename shot.jpg` silently wrote PNG into `shot.png`.**
   Nothing distinguished "the user chose PNG" from "nobody said anything",
   because PNG is the default, so a typed extension was overwritten. A typed
   extension now wins. Two tests asserted the old behaviour.
3. **Saving settings could brick startup.** A plain truncating write plus a
   hard-error config parser meant a crash mid-write left a file that stopped
   bettershot launching at all. Now written to a temporary file, fsynced, and
   renamed into place.
4. **Copy-to-clipboard could evaporate on X11.** The handle was created and
   dropped per copy; on X11 the owning process *is* the clipboard, so dropping
   it destroyed the selection with no manager to inherit it — while bettershot
   reported success and fired a notification. The handle now lives for the
   process.
5. **The "recent captures kept" slider did nothing** — capacity was fixed at
   construction. Lowering it now releases the surplus immediately, and zero
   genuinely means zero.
6. **Theme changes did nothing**, and **settings changed in the editor were
   discarded** on the next daemon capture. Both were one-way data flow.
7. **Tray and hotkey events were only drained while idle**, so a tray "Quit"
   looked ignored and then fired later. Quit now works from any stage.

Also fixed from that review:

8. **Every capture blocked the UI thread.** Up to 30 s on macOS, plus the whole
   duration of a portal consent dialog on Wayland, plus `--delay`. Both capture
   backends' docs said to use a worker thread and `CaptureBackend: Send` exists
   precisely to allow it; the app did it inline anyway, freezing the tray and
   showing as hung on Windows and macOS. Capture now runs on a worker thread
   with a `Capturing` stage, and Quit still works while one is in flight.
9. **`--output-filename -` created a file literally named `-`.** The CLI
   documents and tests `-` as meaning stdout and deliberately withholds an
   extension from it; the writer had no stdout branch, so it wrote a file into
   whatever directory the process started in — for a compositor keybinding,
   often `/`.

Five more tests were found asserting wrong expectations, on top of the three
from the first review.

## Known gaps

Things that are accepted as configuration but do not yet do anything. They are
listed here rather than left to be discovered:

- **`include-cursor`** — implemented on X11, still inert elsewhere. The
  platform-neutral half (`capture::cursor`: premultiplied source-over blending,
  edge clipping, and the two conventions platforms use for "where the cursor
  is") is done and unit-tested; the per-backend half is one query each.
  - **X11** — done, via XFixes `GetCursorImage`.
  - **Wayland portal** — not possible: the Screenshot portal has no cursor
    option at all. `ScreenCast` does, which is a different and much larger API.
  - **Windows** — `xcap` disables cursor capture and will not hand the bitmap
    back, so this needs a hand-written `GetCursorInfo`/`GetIconInfo`/`GetDIBits`
    path including the monochrome-cursor case. Not written blind.
  - **macOS** — belongs with Phase 5; `SCStreamConfiguration.showsCursor` does
    it in one line once there is a Mac to verify on.

  Backends report this through `Capabilities::cursor`, the settings checkbox
  says when it is unavailable, and the CLI logs a warning rather than silently
  ignoring the flag.

That is the whole list. macOS frames used to be here too: the ScreenCaptureKit
backend passed premultiplied BGRA into a straight-alpha pipeline, so translucent
pixels had their alpha applied twice. I had deferred that as needing a Mac,
which was only half right — the conversion is pure arithmetic and is now done
and exhaustively tested, driven by the image's `CGImageAlphaInfo` so that a
straight-alpha image is not divided a second time and a padding byte is never
divided by at all. Confirming the *premise* — that ScreenCaptureKit really
delivers premultiplied pixels, which Apple documents — still wants a Mac.
That is the whole list. `--output-filename -` used to be here too; it now
writes the encoded image to stdout, with a regression test that asserts no file
named `-` is created.

## Not achievable without more hardware

These items are specified and ready, but cannot be completed or honestly
verified in a headless Linux container with no Windows or macOS machine, no
code-signing certificate, and no distribution accounts. They are left unchecked
rather than marked done.

| Item | What it actually needs |
| --- | --- |
| Runtime verification of the tray and hotkeys | Both are now **implemented** and compile for Linux and Windows, and the hotkey path is unit-tested including its failure modes. What is missing is a real desktop session: nobody has seen the tray icon appear or a hotkey fire. Treat daemon mode as untested-in-anger. |
| Signed Windows MSI / winget | An Authenticode certificate and a winget-pkgs submission. An unsigned MSI is worse than none, so nothing here publishes one. The definition itself is no longer a guess: CI builds a 4.4 MB MSI from it on every push and keeps it as an artefact. |
| Flatpak / AUR / deb | **Flatpak now builds in CI** and is no longer a guess; what is left is a Flathub account and the submission PR. The `.deb` and `.rpm` have been built and validated here. The AUR `PKGBUILD` is still unbuilt: `makepkg` is Arch-only and is not available on this machine or on a GitHub runner without a container. |
| v1.0 tag | The acceptance criteria say "holds on Windows + four Linux environments". None can be exercised here. |
| End-to-end startup latency | The ~147 ms that is compositor round-trip and window creation needs a real session. bettershot's own share **is** measured at 3.2 ms. Everything else about performance is measured too — see [docs/performance.md](docs/performance.md). |
| Phase 5 macOS capture | A Mac. ScreenCaptureKit and the TCC permission flow cannot be written blind and left untested. The capture crate ships a macOS stub that returns a clear `Unsupported` error naming this phase, and that stub is verified to compile for `aarch64-apple-darwin`. |

## Risks & watch items

- **Wayland fragmentation**: portal Screenshot behavior differs per compositor (interactive flags, permission dialogs). Mitigation: portal-first with X11 fallback, per-compositor test matrix from Phase 2 on.
- **Text/IME input in egui**: CJK IME support is historically weak in winit/egui; Satty needed a dedicated `ime` module even on GTK. Track upstream, keep the Text tool's input layer swappable.
- **Global hotkeys on Wayland** are compositor-mediated; ship documented compositor keybinding snippets instead of pretending a global grab works.
- **Mixed-DPI multi-monitor** is the biggest correctness risk for the region overlay (Windows per-monitor DPI v2 vs Wayland fractional scaling). Keep all geometry in physical pixels with explicit per-monitor scale metadata.
- **Windows dev loop**: primary dev machine is Linux; Windows regressions only surface on CI. Keep `windows-latest` in the required check set from Phase 0.
