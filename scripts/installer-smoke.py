"""Launch an installed release executable and capture its real main window."""

import argparse
import ctypes
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def window(pid):
    if sys.platform == "win32":
        from ctypes import wintypes

        user = ctypes.windll.user32
        user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user.IsWindowVisible.argtypes = [wintypes.HWND]
        user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
        user.SetForegroundWindow.argtypes = [wintypes.HWND]
        user.SetProcessDPIAware()
        found = []

        @ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)
        def visit(hwnd, _):
            owner = wintypes.DWORD()
            user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
            if owner.value == pid and user.IsWindowVisible(hwnd):
                rect = wintypes.RECT()
                user.GetWindowRect(hwnd, ctypes.byref(rect))
                if rect.right - rect.left > 200 and rect.bottom - rect.top > 200:
                    user.SetForegroundWindow(hwnd)
                    found.append((rect.left, rect.top, rect.right, rect.bottom))
            return True

        user.EnumWindows(visit, 0)
        return found[0] if found else None
    if sys.platform == "darwin":
        import Quartz

        for item in Quartz.CGWindowListCopyWindowInfo(
            Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID
        ):
            if item.get("kCGWindowOwnerPID") == pid and item.get("kCGWindowLayer") == 0:
                b = item["kCGWindowBounds"]
                if b["Width"] > 200 and b["Height"] > 200:
                    return tuple(int(v) for v in (
                        b["X"], b["Y"], b["X"] + b["Width"], b["Y"] + b["Height"]
                    ))
        return None
    # AppImage launchers can spawn the GUI instead of replacing themselves.
    # The app runs in its own session, so only inspect that process group.
    processes = subprocess.check_output(["ps", "-eo", "pid=,pgid="], text=True)
    pids = [int(owner) for owner, group in
            (line.split() for line in processes.splitlines()) if int(group) == pid]
    for owner in pids:
        result = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--pid", str(owner)],
            capture_output=True, text=True,
        )
        bounds = linux_window_bounds(result.stdout.splitlines())
        if bounds:
            return bounds
    return None


def linux_window_bounds(window_ids):
    for wid in window_ids:
        geometry = subprocess.check_output(
            ["xdotool", "getwindowgeometry", "--shell", wid], text=True
        )
        fields = dict(line.split("=", 1) for line in geometry.splitlines())
        x, y, w, h = (int(fields[k]) for k in ("X", "Y", "WIDTH", "HEIGHT"))
        if w > 200 and h > 200:
            return x, y, x + w, y + h
    return None


def stop_process(process):
    if sys.platform == "linux":
        # Include the GUI child and mount helpers, even if the launcher exited.
        try:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                pass
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
    elif process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("screenshot", type=Path)
    args = parser.parse_args()
    from PIL import ImageGrab, ImageStat

    args.screenshot.parent.mkdir(parents=True, exist_ok=True)
    with args.screenshot.with_suffix(".log").open("w") as log:
        process = subprocess.Popen(
            [str(args.executable.resolve())], stdout=log, stderr=log,
            start_new_session=sys.platform == "linux",
        )
        try:
            deadline = time.monotonic() + 90
            bounds = None
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"App exited before capture: {process.returncode}")
                bounds = window(process.pid)
                if bounds:
                    break
                time.sleep(1)
            if not bounds:
                raise RuntimeError("No visible application window within 90 seconds")
            # Allow the first frame to finish and catch immediate startup crashes.
            time.sleep(5)
            if process.poll() is not None or not window(process.pid):
                raise RuntimeError("App/window disappeared after startup")
            image = ImageGrab.grab(bbox=bounds)
            image.save(args.screenshot)
            if max(ImageStat.Stat(image.convert("RGB")).stddev) < 2:
                raise RuntimeError("Captured window is blank")
        finally:
            stop_process(process)


if __name__ == "__main__":
    main()
