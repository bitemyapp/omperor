"""Frames window captures for the README: rounded corners and a soft shadow
on a transparent background, as macOS draws a window, at three quarters of
the Retina capture's size.

usage: frame-screenshots.py <captures-dir> <output-dir>
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

RADIUS = 24
PAD = 80
BLUR = 36
DROP = 16


def frame(window: Image.Image) -> Image.Image:
    width, height = window.size
    mask = Image.new("L", window.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, width - 1, height - 1), RADIUS, fill=255)
    window = window.convert("RGBA")
    window.putalpha(mask)
    canvas = Image.new("RGBA", (width + 2 * PAD, height + 2 * PAD), (0, 0, 0, 0))
    shadow = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle(
        (PAD, PAD + DROP, PAD + width, PAD + height + DROP), RADIUS, fill=(0, 0, 0, 110)
    )
    canvas = Image.alpha_composite(canvas, shadow.filter(ImageFilter.GaussianBlur(BLUR)))
    canvas.alpha_composite(window, (PAD, PAD))
    return canvas.resize((canvas.width * 3 // 4, canvas.height * 3 // 4), Image.LANCZOS)


def main() -> None:
    source, destination = Path(sys.argv[1]), Path(sys.argv[2])
    destination.mkdir(parents=True, exist_ok=True)
    for path in sorted(source.glob("*.png")):
        frame(Image.open(path)).save(destination / path.name, optimize=True)
        print(destination / path.name)


if __name__ == "__main__":
    main()
