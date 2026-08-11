# Performance and memory

Measured with `cargo run --release -p bettershot-render --example perf_audit`
on a mid-range x86-64 Linux machine. Reproduce it yourself before trusting
these numbers on your hardware.

## Export cost

Export renders the whole document through the CPU rasterizer, at full
resolution, with 31 annotations (10 rectangles, 10 arrows, 10 numbered markers,
one 400×300 blur):

| Screenshot | Render | Base image | Peak estimate |
| --- | --- | --- | --- |
| 1080p | 42 ms | 8.3 MB | 25 MB |
| 1440p | 32 ms | 14.7 MB | 44 MB |
| 4K | 40 ms | 33.2 MB | 100 MB |
| Dual 4K, stitched | 67 ms | 66.4 MB | 199 MB |

Export is on demand — Ctrl+C or Ctrl+S — so 50 ms at 4K is imperceptible.
It is *not* on the per-frame path; the editor draws through the GPU.

The peak estimate counts three copies of the image: the base, the render
destination, and the GPU texture the editor holds. That is the honest worst
case for a dual-4K capture and is the number to watch if it ever needs
reducing.

## Where the memory goes, and the two decisions that keep it bounded

**Obscure previews are per annotation, not per screen.** The obvious design is
to pre-blur the whole screenshot once and sample it. That costs a full extra
copy of the image per distinct blur strength, and about **0.78 seconds** to
compute at 4K — enough to visibly stall the editor the moment you drag a blur.
Instead each blur annotation processes only its own rectangle: a 400×300 region
costs about **8 ms**, and the memory scales with the size of the redaction
rather than the size of the display. See `crates/app/src/effects.rs`.

The cache is keyed on the exact rectangle and strength and evicted every frame
to whatever is currently on screen, so dragging a blur does not accumulate
textures.

**Capture history stores PNGs, not pixels.** Five 4K captures as raw RGBA would
be 166 MB. As PNG they are a few megabytes, and they are only decoded at the
moment one is copied. History is also memory-only and never written to disk —
see `crates/app/src/history.rs` for why.

## What is guarded, and what is not

Every number on this page was measured once, by hand, on one machine. That
characterises the program; it does not protect it. The properties the numbers
*depend on* are asserted in `crates/render/tests/end_to_end.rs`, as ratios
between two measurements taken in the same run rather than wall-clock budgets —
a ratio means the same thing on a fast laptop and a noisy CI runner, where an
absolute threshold either flakes or is set so loose it catches nothing:

| Guarded | How |
| --- | --- |
| Blur cost does not grow with radius | radius 64 vs radius 4, must stay under 4x (a kernel-sized implementation would be ~256x) |
| Pixelate cost does not grow with block size | 64px vs 4px blocks, same bound |
| Export does not go superlinear in image size | 4K vs 1080p, must stay under 8x for 4x the pixels |

The measured values are 1.0x, 0.9x and 3.6x, so there is real headroom in each.

What is **not** guarded is the end-to-end figure below: the compositor
round-trip and window creation need a real desktop session, and no headless
container can produce a number that means anything for that target.

## Blur cost is independent of radius

The blur is a three-pass sliding-window box filter, so it is O(1) per pixel per
pass regardless of radius. A `Size::Large` blur costs the same as a small one.
The arithmetic is exact fixed-point rather than floating point, because an f32
running sum is exact only up to a box radius of roughly 128 — beyond that it
would silently drift, which for a redaction is a correctness problem and not a
rounding one.

## Startup

The roadmap budgets 150 ms from launch to a visible overlay. That end-to-end
figure needs a real compositor, but the part bettershot itself controls can be
measured anywhere, and it is small:

| Phase | Per run |
| --- | --- |
| `/bin/true` (what the OS charges for *any* process) | 1.81 ms |
| bettershot: process spawn + dynamic linking | 3.01 ms |
| + argument parsing | 3.07 ms |
| + config load and capture-backend selection | 3.19 ms |

Release binary: 16.1 MB.

So everything bettershot does before it asks the OS for pixels costs about
**3.2 ms**, of which 1.8 ms is the cost of starting any process at all. Its own
overhead is roughly **1.4 ms — under 1% of the 150 ms budget**.

The remaining ~147 ms belongs to two things bettershot does not control: the
capture backend round-trip (on Wayland, a D-Bus call into the compositor, which
may show a permission dialog) and window creation. Both need a real desktop
session to measure, so the end-to-end number is left unclaimed rather than
guessed — see [ROADMAP.md](../ROADMAP.md).

Measure the process-level cost yourself with:

```sh
for i in $(seq 100); do ./target/release/bettershot --version; done
```

And the end-to-end figure, on a real desktop, with:

```sh
bettershot -v --filename shot.png     # logs "first frame after Nms"
```

That log line is the honest end-to-end number: it is emitted when the first
frame is actually drawn, not when the window is created.
