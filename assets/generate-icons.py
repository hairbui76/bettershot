#!/usr/bin/env python3
"""Derive every shipped icon from the one source logo.

Run after changing `assets/bettershot-logo.png`:

    python3 assets/generate-icons.py

Committing the outputs rather than generating them during the build keeps the
build free of an image dependency, and keeps the Flatpak build offline-clean.

The source has a lot of empty margin. Icons are judged at 16-32px in a tray or
a task bar, where that margin is dead space, so the artwork is trimmed to its
ink and re-padded to a small, *consistent* margin. Doing it per size instead
would make the logo appear to breathe as the desktop picked a different one.
"""

import pathlib
import sys

from PIL import Image

HERE = pathlib.Path(__file__).parent
SOURCE = HERE / "bettershot-logo.png"
ICONS = HERE / "icons"

# Fraction of the final canvas left as clear space on each side.
MARGIN = 0.06

# hicolor sizes desktops actually look for, plus the ones Windows packs into a
# .ico. 16 is the Windows tray; 512 is the macOS bundle and app stores.
SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def trimmed_square(image: Image.Image) -> Image.Image:
    """The artwork, cropped to its ink and centred on a square canvas."""
    bbox = image.getchannel("A").getbbox()
    if bbox is None:
        raise SystemExit("the source logo is entirely transparent")
    art = image.crop(bbox)

    # Square by the longer edge so nothing is distorted.
    side = max(art.size)
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(art, ((side - art.width) // 2, (side - art.height) // 2))
    return canvas


def main() -> int:
    if not SOURCE.exists():
        raise SystemExit(f"no source logo at {SOURCE}")

    source = Image.open(SOURCE).convert("RGBA")
    art = trimmed_square(source)
    ICONS.mkdir(exist_ok=True)

    for size in SIZES:
        inner = round(size * (1 - 2 * MARGIN))
        # LANCZOS: the logo has thin strokes that box filtering turns to mud.
        scaled = art.resize((inner, inner), Image.LANCZOS)
        canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
        offset = (size - inner) // 2
        canvas.paste(scaled, (offset, offset))
        canvas.save(ICONS / f"bettershot-{size}.png", optimize=True)
        print(f"  {size}x{size}")

    # The canonical single file, for anything that wants just one.
    art.resize((512, 512), Image.LANCZOS).save(HERE / "bettershot.png", optimize=True)

    # Windows wants every size inside one .ico, or it picks badly and rescales.
    #
    # Pillow builds the entries by resizing *this* image down to each requested
    # size, so the base has to be the largest one. Handing it the 16x16 silently
    # produces a single-entry .ico and Windows then blows a 16px icon up to
    # 256 -- which is why the check below exists rather than trusting the save.
    largest = max(ICO_SIZES)
    Image.open(ICONS / f"bettershot-{largest}.png").save(
        HERE / "bettershot.ico",
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
    )

    written = sorted(Image.open(HERE / "bettershot.ico").info["sizes"])
    if written != sorted((s, s) for s in ICO_SIZES):
        raise SystemExit(f"the .ico only got {written}, expected {ICO_SIZES}")
    print(f"  bettershot.ico ({len(written)} sizes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
