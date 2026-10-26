"""Provision CI-only Mesa llvmpipe next to an installed Windows executable."""

import argparse
import hashlib
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import urllib.request


VERSION = "26.2.5"
# Digests published on the pinned mmozeiko/build-mesa GitHub release.
BUILDS = {
    0x8664: ("x64", "d1b713ca2a7fba47a50c058099f13c435ddb76a16a422c334e4d787f9755a31d"),
    0xAA64: ("arm64", "e7df75398d50e07a68bbffa5cff2ece4c85e5f8ef8bda497f560dd04fe60cbbd"),
}


def pe_machine(path):
    data = Path(path).read_bytes()
    if len(data) < 64 or data[:2] != b"MZ":
        raise ValueError(f"Not a PE file: {path}")
    offset = struct.unpack_from("<I", data, 0x3C)[0]
    if offset < 64 or offset + 6 > len(data) or data[offset:offset + 4] != b"PE\0\0":
        raise ValueError(f"Invalid PE header: {path}")
    return struct.unpack_from("<H", data, offset + 4)[0]


def provision(executable):
    executable = Path(executable).resolve(strict=True)
    machine = pe_machine(executable)
    if machine not in BUILDS:
        raise ValueError(f"Unsupported PE machine: {machine:#x}")
    arch, expected = BUILDS[machine]
    destination = executable.with_name("opengl32.dll")
    if destination.exists():
        raise FileExistsError(f"Refusing to overwrite {destination}")
    name = f"mesa-llvmpipe-{arch}-{VERSION}.7z"
    url = f"https://github.com/mmozeiko/build-mesa/releases/download/{VERSION}/{name}"
    with tempfile.TemporaryDirectory(prefix="perseid-ci-mesa-") as directory:
        archive = Path(directory) / name
        urllib.request.urlretrieve(url, archive)
        actual = hashlib.sha256(archive.read_bytes()).hexdigest()
        if actual != expected:
            raise ValueError(f"Mesa SHA256 mismatch: expected {expected}, got {actual}")
        subprocess.run(
            ["7z", "x", str(archive), f"-o{directory}", "opengl32.dll", "-y"],
            check=True,
        )
        driver = Path(directory) / "opengl32.dll"
        if pe_machine(driver) != machine:
            raise ValueError("Mesa DLL architecture does not match installed executable")
        # This build statically links its dependencies; only the WGL DLL is needed.
        shutil.copyfile(driver, destination)
    print(f"CI-only Mesa llvmpipe {VERSION} ({arch}): {destination}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    provision(parser.parse_args().executable)
