#!/usr/bin/env python3
"""Regression tests for the screenshot verifier's path and file handling."""

from __future__ import annotations

import os
import struct
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import check_screenshots


class ScreenshotPathTests(unittest.TestCase):
    def test_directory_must_resolve_inside_the_working_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "workspace"
            screenshots = root / "artifacts"
            outside = Path(temporary) / "outside"
            screenshots.mkdir(parents=True)
            outside.mkdir()

            self.assertEqual(
                check_screenshots.resolve_directory(Path("artifacts"), root),
                screenshots.resolve(),
            )
            with self.assertRaisesRegex(ValueError, "outside"):
                check_screenshots.resolve_directory(Path("../outside"), root)

            link = root / "outside-link"
            try:
                link.symlink_to(outside, target_is_directory=True)
            except (NotImplementedError, OSError) as exc:
                self.skipTest(f"symlinks are unavailable: {exc}")
            with self.assertRaisesRegex(ValueError, "outside"):
                check_screenshots.resolve_directory(Path("outside-link"), root)

    def test_screenshot_name_must_be_a_single_filename(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()

            with self.assertRaisesRegex(ValueError, "single filename"):
                check_screenshots.resolve_screenshot(directory, "../secret")

    def test_screenshot_symlinks_cannot_escape_the_directory(self) -> None:
        with (
            tempfile.TemporaryDirectory() as workspace,
            tempfile.TemporaryDirectory() as outside,
        ):
            directory = Path(workspace).resolve()
            secret = Path(outside) / "secret"
            secret.write_bytes(b"not a screenshot")
            link = directory / "perseid-main.png"
            try:
                link.symlink_to(secret)
            except (NotImplementedError, OSError) as exc:
                self.skipTest(f"symlinks are unavailable: {exc}")

            with self.assertRaisesRegex(ValueError, "outside"):
                check_screenshots.resolve_screenshot(directory, link.name)

    def test_png_validation_reads_only_the_header(self) -> None:
        header = (
            b"\x89PNG\r\n\x1a\n"
            + struct.pack(">I", 13)
            + b"IHDR"
            + struct.pack(">II", 2560, 1600)
        )
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temporary:
            directory = Path(temporary).resolve()
            path = directory / "perseid-main.png"
            contents = header + b"\x00" * 8192
            path.write_bytes(contents)
            real_read = os.read
            read_sizes = []

            def record_read_size(fd: int, size: int) -> bytes:
                read_sizes.append(size)
                return real_read(fd, size)

            with patch.object(check_screenshots.os, "read", side_effect=record_read_size):
                self.assertEqual(
                    check_screenshots.png_size(directory, path.name),
                    (2560, 1600, len(contents)),
                )

            self.assertEqual(read_sizes, [24])

    def test_symlink_swapped_after_validation_is_not_opened(self) -> None:
        with (
            tempfile.TemporaryDirectory(dir=Path.cwd()) as workspace,
            tempfile.TemporaryDirectory() as outside,
        ):
            directory = Path(workspace).resolve()
            path = directory / "perseid-main.png"
            path.write_bytes(b"\x89PNG\r\n\x1a\n" + b"\x00" * 16)
            secret = Path(outside) / "secret"
            secret.write_bytes(b"secret contents")
            real_open = os.open

            def swap_before_open(file: str | os.PathLike[str], flags: int, **kwargs: int) -> int:
                if file == path.name and "dir_fd" in kwargs:
                    path.unlink()
                    path.symlink_to(secret)
                return real_open(file, flags, **kwargs)

            with patch.object(check_screenshots.os, "open", side_effect=swap_before_open):
                with self.assertRaises(OSError):
                    check_screenshots.png_size(directory, path.name)


if __name__ == "__main__":
    unittest.main()
