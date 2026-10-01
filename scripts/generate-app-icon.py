#!/usr/bin/env python3
"""Reproduce the blue tablet icon previously drawn by src/ui/canvas.rs.

The geometry and one-pixel coverage match app_icon in v0.15.5. The static
Windows-blue accent lets Explorer, shortcuts and the panel share one icon.
No imaging dependency is required; ICO images use 32-bit BGRA DIBs.
"""
import argparse
import math
from pathlib import Path
import struct

SIZES = (16, 32, 48, 64, 128, 256)
ACCENT = (0, 120, 215)


def rounded_box(x, y, half_x, half_y, radius):
    qx = abs(x) - half_x + radius
    qy = abs(y) - half_y + radius
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - radius


def coverage(distance):
    return min(1, max(0, 0.5 - distance))


def image(size):
    pixels = bytearray()
    stroke = max(size / 16, 1)
    for y in reversed(range(size)):
        for x in range(size):
            px, py = x + 0.5 - size / 2, y + 0.5 - size / 2
            body = coverage(rounded_box(px, py, size / 2 - 0.5, size / 2 - 0.5, size * 0.22))
            area = rounded_box(px, py, size * 0.3, size * 0.2, size * 0.05)
            ring = coverage(area) - coverage(area + stroke)
            dot = coverage(math.hypot(px, py) - size * 0.075)
            white = max(ring, dot)
            rgb = [math.floor(c + (255 - c) * white + 0.5) for c in ACCENT]
            pixels.extend((rgb[2], rgb[1], rgb[0], math.floor(body * 255 + 0.5)))
    # A 1-bit AND mask preserves transparent corners in legacy shell readers.
    stride = ((size + 31) // 32) * 4
    mask = bytearray(stride * size)
    for y in range(size):
        for x in range(size):
            if pixels[(y * size + x) * 4 + 3] == 0:
                mask[y * stride + x // 8] |= 0x80 >> (x % 8)
    header = struct.pack('<IiiHHIIiiII', 40, size, size * 2, 1, 32, 0, len(pixels), 0, 0, 0, 0)
    return header + pixels + mask


def generate():
    images = [image(size) for size in SIZES]
    directory = bytearray(struct.pack('<HHH', 0, 1, len(SIZES)))
    offset = 6 + len(SIZES) * 16
    for size, data in zip(SIZES, images):
        directory.extend(struct.pack('<BBBBHHII', size % 256, size % 256, 0, 0, 1, 32, len(data), offset))
        offset += len(data)
    return bytes(directory) + b''.join(images)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path(__file__).resolve().parents[1] / 'resources/opentabletdriver.ico')
    args = parser.parse_args()
    args.output.write_bytes(generate())
    print(f'Wrote {args.output} at {", ".join(map(str, SIZES))} pixels')
