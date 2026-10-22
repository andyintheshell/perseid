#!/usr/bin/env python3
"""Check that the generated documentation screenshots are usable PNGs.

Usage: scripts/check_screenshots.py DIR

This is deliberately dependency-free: CI runs it before uploading artifacts,
and scripts/screenshots.sh runs it before replacing docs/images.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

# name -> (expected width, minimum height, maximum height)
FULL_WINDOW_SCENES = [
    "perseid-main.png",
    "perseid-target-site-map.png",
    "perseid-target-scope.png",
    "perseid-proxy-intercept.png",
    "perseid-proxy-match-replace.png",
    "perseid-proxy-tls.png",
    "perseid-intruder.png",
    "perseid-repeater.png",
    "perseid-decoder.png",
    "perseid-comparer.png",
    "perseid-comparer-result.png",
]
# Screenshots are captured at 2x (1280x820 points) and saved at native
# resolution so they stay sharp when GitHub or a HiDPI browser scales them.
EXPECTED = {name: (2560, 1600, 1700) for name in FULL_WINDOW_SCENES}
EXPECTED["perseid-memory.png"] = (970, 400, 900)
MIN_BYTES = 10_000


def png_size(path: Path) -> tuple[int, int, int]:
    data = path.read_bytes()
    if len(data) < 24:
        raise ValueError("file is too short")
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("missing PNG signature")
    if data[12:16] != b"IHDR":
        raise ValueError("missing IHDR chunk")
    width, height = struct.unpack(">II", data[16:24])
    return width, height, len(data)


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} DIR", file=sys.stderr)
        return 2

    directory = Path(sys.argv[1])
    failed = False
    for name, (expected_width, min_height, max_height) in EXPECTED.items():
        path = directory / name
        if not path.is_file():
            print(f"missing {path}", file=sys.stderr)
            failed = True
            continue
        try:
            width, height, size = png_size(path)
        except (OSError, ValueError) as exc:
            print(f"invalid {path}: {exc}", file=sys.stderr)
            failed = True
            continue

        print(f"ok {name}: {width}x{height}, {size / 1024:.0f} KiB")
        if width != expected_width:
            print(
                f"  expected width {expected_width}px, got {width}px",
                file=sys.stderr,
            )
            failed = True
        if not min_height <= height <= max_height:
            print(
                f"  height {height}px is outside {min_height}..{max_height}",
                file=sys.stderr,
            )
            failed = True
        if size < MIN_BYTES:
            print(f"  file is suspiciously small ({size} bytes)", file=sys.stderr)
            failed = True

    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
