"""Behavior tests without a GUI; native capture is exercised on release runners."""

import importlib.util
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import MagicMock, patch


spec = importlib.util.spec_from_file_location(
    "installer_smoke", Path(__file__).with_name("installer-smoke.py")
)
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class SmokeTests(unittest.TestCase):
    def run_smoke(self, *, exited=False, blank=False, visible=True):
        process = MagicMock()
        process.poll.return_value = 1 if exited else None
        process.returncode = 1
        image = MagicMock()
        grab = MagicMock()
        grab.grab.return_value = image
        stat = MagicMock()
        stat.Stat.return_value.stddev = [0 if blank else 20] * 3
        pil = types.ModuleType("PIL")
        pil.ImageGrab, pil.ImageStat = grab, stat
        with tempfile.TemporaryDirectory() as directory:
            output = str(Path(directory) / "test.png")
            with (
                patch.dict(sys.modules, {"PIL": pil}),
                patch.object(sys, "argv", ["smoke", "installed-app", output]),
                patch.object(smoke.subprocess, "Popen", return_value=process),
                patch.object(smoke, "window", return_value=(0, 0, 800, 600) if visible else None),
                patch.object(smoke.time, "sleep"),
                patch.object(smoke.time, "monotonic", side_effect=[0, 1, 100]),
            ):
                smoke.main()
        return process, grab, image

    def test_captures_installed_window_and_terminates(self):
        process, grab, image = self.run_smoke()
        grab.grab.assert_called_once_with(bbox=(0, 0, 800, 600))
        image.save.assert_called_once()
        process.terminate.assert_called_once()

    def test_rejects_startup_exit(self):
        with self.assertRaisesRegex(RuntimeError, "exited before capture"):
            self.run_smoke(exited=True)

    def test_rejects_blank_capture(self):
        with self.assertRaisesRegex(RuntimeError, "blank"):
            self.run_smoke(blank=True)

    def test_requires_visible_window(self):
        with self.assertRaisesRegex(RuntimeError, "No visible"):
            self.run_smoke(visible=False)


if __name__ == "__main__":
    unittest.main()
