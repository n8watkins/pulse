#!/usr/bin/env python3
"""Generate apps/native/assets/pulse.ico from the same geometry as apps/native/ui/logo.svg.

Needs only Python 3 and Pillow (`pip install pillow`).
Usage: python3 scripts/make-icon.py [output.ico]   (default: apps/native/assets/pulse.ico)

Each size is drawn on its own from the 256 unit logo at 16x supersampling, so edges stay clean.
Small sizes get a thicker line and a little less corner rounding so the heartbeat still reads at 16 px.
"""
import io
import struct
import sys
from pathlib import Path

from PIL import Image, ImageDraw

SIZES = [16, 20, 24, 32, 48, 64, 256]
BG = (0x0D, 0x0F, 0x0E, 255)
LINE = (0xB8, 0xF5, 0xD0, 255)
POINTS = [(50, 140), (92, 140), (112, 88), (140, 182), (160, 120), (174, 140), (206, 140)]
SS = 16


def stroke_width(size: int) -> float:
    """Stroke width in 256-unit logo space; the logo itself uses 18."""
    if size <= 16:
        return 34
    if size <= 24:
        return 30
    if size <= 32:
        return 26
    if size <= 48:
        return 22
    return 18


def draw(size: int) -> Image.Image:
    big = size * SS
    k = big / 256.0
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # At small sizes fill more of the canvas (the logo leaves a 16 unit margin).
    margin = 16 if size > 32 else (10 if size > 20 else 4)
    d.rounded_rectangle(
        [margin * k, margin * k, (256 - margin) * k - 1, (256 - margin) * k - 1],
        radius=52 * k * (256 - 2 * margin) / 224,
        fill=BG,
    )
    # Pull the line in from the old frame so it grows with the larger tile.
    scale = (256 - 2 * margin) / 224
    pts = [((128 + (x - 128) * scale) * k, (128 + (y - 128) * scale) * k) for x, y in POINTS]
    w = stroke_width(size) * k
    d.line(pts, fill=LINE, width=round(w), joint="curve")
    r = w / 2
    for x, y in pts:  # round caps and joins
        d.ellipse([x - r, y - r, x + r, y + r], fill=LINE)
    return img.resize((size, size), Image.LANCZOS)


def write_ico(images: list[Image.Image], path: Path) -> None:
    blobs = []
    for im in images:
        buf = io.BytesIO()
        im.save(buf, format="PNG")
        blobs.append(buf.getvalue())
    out = bytearray(struct.pack("<HHH", 0, 1, len(images)))
    offset = 6 + 16 * len(images)
    for im, blob in zip(images, blobs):
        s = im.width
        out += struct.pack("<BBBBHHII", 0 if s >= 256 else s, 0 if s >= 256 else s, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    for blob in blobs:
        out += blob
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(out))


def main() -> None:
    root = Path(__file__).resolve().parent.parent
    dest = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "apps/native/assets/pulse.ico"
    images = [draw(s) for s in SIZES]
    write_ico(images, dest)
    print(f"wrote {dest} ({dest.stat().st_size} bytes, sizes {SIZES})")


if __name__ == "__main__":
    main()
