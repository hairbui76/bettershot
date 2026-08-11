# bettershot Roadmap

**Goal:** a fast, modern, cross-platform screenshot **capture + annotation** tool.
**Platforms:** Windows and Linux first-class from Phase 2; macOS in Phase 5.
**Heritage:** annotation UX and architecture based on [Satty](https://github.com/Satty-org/Satty) (see `Satty/`, read-only reference). Where Satty relies on an external grabber (`grim | satty`), bettershot owns the whole pipeline: hotkey → capture → annotate → save/clipboard.

## Status

**53 of 59 roadmap items are done**, and the project now builds and tests
**green on real Linux, Windows and macOS runners**:

| Job | Result |
| --- | --- |
| Test (ubuntu-latest) | 711 tests |
| Test (windows-latest) | 694 tests |
| Test (macos-latest) | 694 tests |
| Cross-compile check | clippy clean for `x86_64-pc-windows-msvc` and `aarch64-apple-darwin` |
| Release build | Linux, Windows and macOS, all with `--features tray` |
| Satty reference untouched | the upstream checkout is still out of version control |

The Windows and macOS counts are lower because some tests are Linux-specific
(portal handling, X11 title decoding), not because anything is skipped.

That green run took four attempts, and each failure was a real defect that no
amount of local care could have found — see
[What CI found that nothing local could](#what-ci-found-that-nothing-local-could).

The six remaining items need a signing certificate, an Apple Developer ID,
distribution accounts, a Mac to *use*, or a desktop session that renders. None
is blocked on design or on code that could have been written here.

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

- **`include-cursor`** — the Wayland portal decides for itself whether the
  cursor is in the shot, and the X11 and Windows backends would each need their
  own compositing step. The setting is stored and the UI marks it unavailable.
- **`--output-filename -`** — documented as meaning stdout, and the CLI
  refuses to give it an extension, but the writer has no stdout branch and
  would create a file literally named `-`. Do not use it yet.

## Not achievable without more hardware

These items are specified and ready, but cannot be completed or honestly
verified in a headless Linux container with no Windows or macOS machine, no
code-signing certificate, and no distribution accounts. They are left unchecked
rather than marked done.

| Item | What it actually needs |
| --- | --- |
| Runtime verification of the tray and hotkeys | Both are now **implemented** and compile for Linux and Windows, and the hotkey path is unit-tested including its failure modes. What is missing is a real desktop session: nobody has seen the tray icon appear or a hotkey fire. Treat daemon mode as untested-in-anger. |
| Signed Windows MSI / winget | An Authenticode certificate and a winget-pkgs submission. An unsigned MSI is worse than none. Unsigned archives and installers *are* configured via `cargo-dist`. |
| Flatpak / AUR / deb | A build host to actually produce and install the artefacts. A Flatpak manifest that has never been built is a guess, not a deliverable. The `.desktop` file and AppStream metainfo they consume **are** done, in `assets/`. |
| v1.0 tag | The acceptance criteria say "holds on Windows + four Linux environments". None can be exercised here. |
| End-to-end startup latency | The ~147 ms that is compositor round-trip and window creation needs a real session. bettershot's own share **is** measured at 3.2 ms. Everything else about performance is measured too — see [docs/performance.md](docs/performance.md). |
| Phase 5 macOS capture | A Mac. ScreenCaptureKit and the TCC permission flow cannot be written blind and left untested. The capture crate ships a macOS stub that returns a clear `Unsupported` error naming this phase, and that stub is verified to compile for `aarch64-apple-darwin`. |

## Risks & watch items

- **Wayland fragmentation**: portal Screenshot behavior differs per compositor (interactive flags, permission dialogs). Mitigation: portal-first with X11 fallback, per-compositor test matrix from Phase 2 on.
- **Text/IME input in egui**: CJK IME support is historically weak in winit/egui; Satty needed a dedicated `ime` module even on GTK. Track upstream, keep the Text tool's input layer swappable.
- **Global hotkeys on Wayland** are compositor-mediated; ship documented compositor keybinding snippets instead of pretending a global grab works.
- **Mixed-DPI multi-monitor** is the biggest correctness risk for the region overlay (Windows per-monitor DPI v2 vs Wayland fractional scaling). Keep all geometry in physical pixels with explicit per-monitor scale metadata.
- **Windows dev loop**: primary dev machine is Linux; Windows regressions only surface on CI. Keep `windows-latest` in the required check set from Phase 0.
