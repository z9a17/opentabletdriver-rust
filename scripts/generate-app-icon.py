#!/usr/bin/env python3
"""Build resources/opentabletdriver.ico from the SVG sources.

`opentabletdriver-small.svg` is drawn for 16-24 pixel slots (notification
area, small title bar icons); `opentabletdriver.svg` covers 32 pixels and up.
Each size is rasterised with the `resvg` CLI (`cargo install resvg`) and stored
as a PNG image, which Windows Vista and later read at every icon size.
"""
import argparse
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SMALL = (16, 20, 24)
LARGE = (32, 40, 48, 64, 96, 128, 256)


def render(svg, size, directory):
    output = Path(directory) / f'{size}.png'
    subprocess.run(['resvg', '-w', str(size), '-h', str(size), str(svg), str(output)], check=True)
    return output.read_bytes()


def generate():
    if shutil.which('resvg') is None:
        raise SystemExit('resvg not found; install it with `cargo install resvg`')
    resources = ROOT / 'resources'
    with tempfile.TemporaryDirectory() as directory:
        images = [(size, render(resources / 'opentabletdriver-small.svg', size, directory)) for size in SMALL]
        images += [(size, render(resources / 'opentabletdriver.svg', size, directory)) for size in LARGE]
    directory = bytearray(struct.pack('<HHH', 0, 1, len(images)))
    offset = 6 + len(images) * 16
    for size, data in images:
        directory.extend(struct.pack('<BBBBHHII', size % 256, size % 256, 0, 0, 1, 32, len(data), offset))
        offset += len(data)
    return bytes(directory) + b''.join(data for _, data in images), [size for size, _ in images]


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'resources/opentabletdriver.ico')
    args = parser.parse_args()
    icon, sizes = generate()
    args.output.write_bytes(icon)
    print(f'Wrote {args.output} at {", ".join(map(str, sizes))} pixels')
