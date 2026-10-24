#!/usr/bin/env python3
"""Check that the generated documentation screenshots are usable PNGs.

Usage: scripts/check_screenshots.py DIR (inside the current working directory)

This is deliberately dependency-free: CI runs it before uploading artifacts,
and scripts/screenshots.sh runs it before replacing docs/images.
"""

from __future__ import annotations

import os
import stat
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

    candidate = directory / name
    path = candidate.resolve(strict=True)
    if not is_within(path, directory):
        raise ValueError("screenshot resolves outside the screenshot directory")
    if path != candidate:
        raise ValueError("screenshot files cannot be symlinks")
    if not path.is_file():
        raise ValueError("not a regular file")
    return path


def open_directory_beneath(directory: Path) -> int:
    if (
        os.open not in os.supports_dir_fd
        or not hasattr(os, "O_DIRECTORY")
        or not hasattr(os, "O_NOFOLLOW")
    ):
        raise OSError("this platform lacks safe relative directory opens")

    root = Path.cwd().resolve(strict=True)
    try:
        relative = directory.relative_to(root)
    except ValueError as exc:
        raise ValueError("directory resolves outside the working directory") from exc

    directory_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    directory_flags |= getattr(os, "O_CLOEXEC", 0)
    directory_fd = os.open(".", directory_flags)
    try:
        for component in relative.parts:
            child_fd = os.open(component, directory_flags, dir_fd=directory_fd)
            parent_fd = directory_fd
            directory_fd = child_fd
            os.close(parent_fd)
        return directory_fd
    except BaseException:
        os.close(directory_fd)
        raise


def read_screenshot_header(directory: Path, name: str) -> tuple[bytes, int]:
    directory_fd = open_directory_beneath(directory)
    file_flags = os.O_RDONLY | os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0)
    file_flags |= getattr(os, "O_NONBLOCK", 0)
    try:
        file_fd = os.open(name, file_flags, dir_fd=directory_fd)
    finally:
        os.close(directory_fd)

    try:
        file_stat = os.fstat(file_fd)
        if not stat.S_ISREG(file_stat.st_mode):
            raise ValueError("not a regular file")
        return os.read(file_fd, 24), file_stat.st_size
    finally:
        os.close(file_fd)


def png_size(directory: Path, name: str) -> tuple[int, int, int]:
    resolve_screenshot(directory, name)
    data, size = read_screenshot_header(directory, name)
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
