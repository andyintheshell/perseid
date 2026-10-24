#!/usr/bin/env python3
"""Regression tests for the screenshot verifier's path and file handling."""

from __future__ import annotations

import struct
import tempfile
import unittest
from pathlib import Path
from unittest.mock import mock_open, patch

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
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            path = directory / "perseid-main.png"
            contents = header + b"\x00" * 8192
            path.write_bytes(contents)
            reader = mock_open(read_data=contents).return_value

            with patch.object(Path, "open", return_value=reader):
                self.assertEqual(
                    check_screenshots.png_size(directory, path.name),
                    (2560, 1600, len(contents)),
                )

            reader.read.assert_called_once_with(24)


if __name__ == "__main__":
    unittest.main()
