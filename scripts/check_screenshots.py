#!/usr/bin/env python3
"""Check that the generated documentation screenshots are usable PNGs.

Usage: scripts/check_screenshots.py DIR (inside the current working directory)

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
    "perseid-project.png",
    "perseid-project-dialog.png",
    "perseid-proxy-websockets.png",
    "perseid-target-site-map.png",
    "perseid-target-scope.png",
    "perseid-proxy-intercept.png",
    "perseid-proxy-match-replace.png",
    "perseid-proxy-tls.png",
    "perseid-intruder.png",
    "perseid-repeater.png",
    "perseid-sequencer.png",
    "perseid-sequencer-result.png",
    "perseid-decoder.png",
    "perseid-comparer.png",
    "perseid-comparer-result.png",
    "perseid-search.png",
]
# Screenshots are captured at 2x (1280x820 points) and saved at native
# resolution so they stay sharp when GitHub or a HiDPI browser scales them.
EXPECTED = {name: (2560, 1600, 1700) for name in FULL_WINDOW_SCENES}
EXPECTED["perseid-memory.png"] = (970, 400, 900)
MIN_BYTES = 10_000


def is_within(path: Path, directory: Path) -> bool:
    try:
        path.relative_to(directory)
    except ValueError:
        return False
    return True


def resolve_directory(path: Path, working_directory: Path | None = None) -> Path:
    root = (working_directory or Path.cwd()).resolve(strict=True)
    candidate = path if path.is_absolute() else root / path
    directory = candidate.resolve(strict=True)
    if not directory.is_dir():
        raise ValueError("not a directory")
    if not is_within(directory, root):
        raise ValueError("directory resolves outside the working directory")
    return directory


def resolve_screenshot(directory: Path, name: str) -> Path:
    if Path(name).name != name or name in {".", ".."}:
        raise ValueError("screenshot name must be a single filename")

    path = (directory / name).resolve(strict=True)
    if not is_within(path, directory):
        raise ValueError("screenshot resolves outside the screenshot directory")
    if not path.is_file():
        raise ValueError("not a regular file")
    return path


def png_size(directory: Path, name: str) -> tuple[int, int, int]:
    path = resolve_screenshot(directory, name)
    size = path.stat().st_size
    with path.open("rb") as image:
        data = image.read(24)
    if len(data) < 24:
        raise ValueError("file is too short")
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("missing PNG signature")
    if data[12:16] != b"IHDR":
        raise ValueError("missing IHDR chunk")
    width, height = struct.unpack(">II", data[16:24])
    return width, height, size


def main() -> int:
    if len(sys.argv) != 2:
        print(
            f"usage: {sys.argv[0]} DIR (inside the current working directory)",
            file=sys.stderr,
        )
        return 2

    try:
        directory = resolve_directory(Path(sys.argv[1]))
    except (OSError, RuntimeError, ValueError) as exc:
        print(
            f"invalid screenshot directory {sys.argv[1]!r}: {exc}",
            file=sys.stderr,
        )
        return 2

    failed = False
    for name, (expected_width, min_height, max_height) in EXPECTED.items():
        path = directory / name
        try:
            width, height, size = png_size(directory, name)
        except FileNotFoundError:
            print(f"missing {path}", file=sys.stderr)
            failed = True
            continue
        except (OSError, RuntimeError, ValueError) as exc:
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
