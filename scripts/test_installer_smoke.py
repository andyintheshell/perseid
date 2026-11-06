"""Behavior tests without a GUI; native capture is exercised on release runners."""

import importlib.util
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import MagicMock, call, patch


spec = importlib.util.spec_from_file_location(
    "installer_smoke", Path(__file__).with_name("installer-smoke.py")
)
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class PlatformMocks(unittest.TestCase):
    def setUp(self):
        # Replace module references: Windows has neither os.killpg nor SIGKILL.
        # Do not require those native symbols just to test the Linux behavior.
        self.enterContext(patch.object(smoke, "os", types.SimpleNamespace(killpg=MagicMock())))
        self.enterContext(patch.object(smoke, "signal", types.SimpleNamespace(SIGTERM=15, SIGKILL=9)))


class SmokeTests(PlatformMocks):
    def run_smoke(self, *, exited=False, blank=False, visible=True, platform="darwin"):
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
                patch.object(sys, "platform", platform),
                patch.object(sys, "argv", ["smoke", "installed-app", output]),
                patch.object(smoke.subprocess, "Popen", return_value=process) as launch,
                patch.object(smoke.os, "killpg"),
                patch.object(smoke, "window", return_value=(0, 0, 800, 600) if visible else None),
                patch.object(smoke.time, "sleep"),
                patch.object(smoke.time, "monotonic", side_effect=[0, 1, 100]),
            ):
                smoke.main()
                self.assertEqual(launch.call_args.kwargs["start_new_session"], platform == "linux")
        return process, grab, image

    def test_captures_installed_window_and_terminates(self):
        process, grab, image = self.run_smoke()
        grab.grab.assert_called_once_with(bbox=(0, 0, 800, 600))
        image.save.assert_called_once()
        process.terminate.assert_called_once()

    def test_linux_capture_uses_isolated_session(self):
        process, grab, image = self.run_smoke(platform="linux")
        grab.grab.assert_called_once_with(bbox=(0, 0, 800, 600))
        image.save.assert_called_once()
        process.wait.assert_any_call()

    def test_windows_capture_does_not_use_process_groups(self):
        process, grab, image = self.run_smoke(platform="win32")
        grab.grab.assert_called_once_with(bbox=(0, 0, 800, 600))
        image.save.assert_called_once()
        process.terminate.assert_called_once()
        smoke.os.killpg.assert_not_called()

    def test_rejects_startup_exit(self):
        with self.assertRaisesRegex(RuntimeError, "exited before capture"):
            self.run_smoke(exited=True)

    def test_rejects_blank_capture(self):
        with self.assertRaisesRegex(RuntimeError, "blank"):
            self.run_smoke(blank=True)

    def test_requires_visible_window(self):
        with self.assertRaisesRegex(RuntimeError, "No visible"):
            self.run_smoke(visible=False)


class LinuxSmokeTests(PlatformMocks):
    def test_finds_child_window_but_not_unrelated_process(self):
        with (
            patch.object(sys, "platform", "linux"),
            patch.object(smoke.subprocess, "check_output", side_effect=[
                " 10 10\n 11 10\n 12 10\n 99 99\n",
                "X=5\nY=10\nWIDTH=800\nHEIGHT=600\n",
            ]),
            patch.object(smoke.subprocess, "run", side_effect=[
                types.SimpleNamespace(stdout=""),
                types.SimpleNamespace(stdout="42\n"),
            ]) as search,
        ):
            self.assertEqual(smoke.window(10), (5, 10, 805, 610))
        self.assertEqual([c.args[0][-1] for c in search.call_args_list], ["10", "11"])

    def test_rejects_unrelated_window(self):
        with (
            patch.object(sys, "platform", "linux"),
            patch.object(smoke.subprocess, "check_output", return_value="10 10\n99 99\n"),
            patch.object(smoke.subprocess, "run", return_value=types.SimpleNamespace(stdout="")) as search,
        ):
            self.assertIsNone(smoke.window(10))
        search.assert_called_once_with(
            ["xdotool", "search", "--onlyvisible", "--pid", "10"],
            capture_output=True, text=True,
        )

    def test_rejects_small_window(self):
        with patch.object(smoke.subprocess, "check_output", return_value=
                          "X=0\nY=0\nWIDTH=100\nHEIGHT=100\n"):
            self.assertIsNone(smoke.linux_window_bounds(["42"]))

    def test_terminates_process_group_including_stubborn_children(self):
        for timeout in (False, True):
            with self.subTest(timeout=timeout):
                process = MagicMock(pid=10)
                if timeout:
                    process.wait.side_effect = [smoke.subprocess.TimeoutExpired("app", 10), None]
                with patch.object(sys, "platform", "linux"), patch.object(smoke.os, "killpg") as kill:
                    smoke.stop_process(process)
                self.assertEqual(kill.call_args_list, [
                    call(10, smoke.signal.SIGTERM), call(10, smoke.signal.SIGKILL),
                ])
                process.wait.assert_any_call()

    def test_cleanup_tolerates_already_exited_group(self):
        process = MagicMock(pid=10)
        with (
            patch.object(sys, "platform", "linux"),
            patch.object(smoke.os, "killpg", side_effect=ProcessLookupError),
        ):
            smoke.stop_process(process)
        process.wait.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
