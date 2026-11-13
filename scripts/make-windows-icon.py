#!/usr/bin/env python3
"""Regenerate assets/icon.ico from the SVG sources.

Sizes up to 32 px come from icon-small.svg (heavier strokes, simplified
timeline) so the taskbar and title bar stay crisp; larger sizes come from
icon.svg. Every entry is stored as PNG, which Windows has read since Vista
and which keeps the file ~50 KB instead of ~400 KB of raw bitmaps.

Requires rsvg-convert (librsvg).
"""

import pathlib
import struct
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"
SMALL = [16, 20, 24, 32]
LARGE = [40, 48, 64, 96, 128, 256]


def render(svg: pathlib.Path, size: int, out: pathlib.Path) -> bytes:
    subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), str(svg), "-o", str(out)],
        check=True,
    )
    return out.read_bytes()


def main() -> None:
    entries = []
    with tempfile.TemporaryDirectory() as tmp:
        for size in SMALL + LARGE:
            svg = ASSETS / ("icon-small.svg" if size in SMALL else "icon.svg")
            entries.append((size, render(svg, size, pathlib.Path(tmp) / f"{size}.png")))

    # ICONDIR, then one ICONDIRENTRY per image, then the PNG payloads.
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = len(header) + 16 * len(entries)
    directory = b""
    for size, png in entries:
        dim = size % 256  # 0 means 256
        directory += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
    data = header + directory + b"".join(png for _, png in entries)
    (ASSETS / "icon.ico").write_bytes(data)
    print(f"assets/icon.ico: {len(entries)} sizes, {len(data)} bytes")


if __name__ == "__main__":
    main()
